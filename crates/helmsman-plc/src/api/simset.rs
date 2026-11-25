//! Setting a sim input ([`helmsman_io::Binding::Sim`]) through the API.
//!
//! ```text
//! PUT /api/signals/{name}     engineer session or the box token: {"value": true | 12 | 3.5}
//! ```
//!
//! Only an `in` signal bound to `sim` in the I/O map can be set: 404 for no
//! such signal, 409 for any other (a device input, a module's output, an
//! output). 400 for a value of the wrong kind (bool for bool, an integer
//! that fits the signal and every port reading it for int, a finite
//! number for real), a body that is not exactly `{"value": ...}`, and
//! JSON that does not read; 415 for another content type. A session below
//! engineer is refused 403, as for a force. Every outcome is audited.
//!
//! The value is queued for the scan thread ([`crate::force::Forces::set_sim`])
//! and the program sees it at its next scan; a read right after the reply
//! may still show the old one.

use helmsman_io::{Ty, Value};
use serde_json::{Value as Json, json};

use super::{Ctx, Request, Response, audited, is_json};
use crate::policy::Role;

pub(super) fn put(req: &Request, ctx: &Ctx, name: &str) -> Response {
    let refuse = |status: u16, why: &str| {
        audited(
            ctx,
            req,
            status,
            &json!({ "error": why }),
            &format!("sim set: refused, {why}"),
        )
    };
    if let Some(p) = &req.session
        && p.role < Role::Engineer
    {
        return refuse(
            403,
            &format!(
                "setting a sim input needs an engineer login; this session is {}",
                p.label()
            ),
        );
    }
    let Some(forces) = &ctx.forces else {
        return refuse(404, "this box does not set sim inputs");
    };
    let c = ctx.catalog();
    let Some(s) = c.signal(name).map(|i| &c.signals[i]) else {
        return refuse(404, "no such signal");
    };
    if !s.sim || s.dir != helmsman_io::Dir::In {
        return refuse(409, "only a sim input can be set");
    }
    if !is_json(req) {
        return refuse(415, "Content-Type must be application/json");
    }
    let raw = match serde_json::from_slice::<Json>(&req.body) {
        Ok(Json::Object(m)) if m.len() == 1 && m.contains_key("value") => m["value"].clone(),
        _ => return refuse(400, "expected exactly {\"value\": ...}"),
    };
    let value = match decode(s.ty, &raw) {
        Ok(v) => v,
        Err(why) => return refuse(400, &format!("value: {why}")),
    };
    if let Err(why) = fits(&c, &s.name, value) {
        return refuse(400, &format!("value: {why}"));
    }
    forces.set_sim(&s.name, value);
    let shown = super::render::value(Some(value));
    audited(
        ctx,
        req,
        200,
        &json!({ "name": s.name, "value": shown }),
        &format!("sim set: {} := {shown}", s.name),
    )
}

/// `raw` as a value of type `ty`, exactly: no coercion between kinds,
/// except that a whole number is a fine real.
fn decode(ty: Ty, raw: &Json) -> Result<Value, String> {
    match (ty, raw) {
        (Ty::Bool, Json::Bool(b)) => Ok(Value::Bool(*b)),
        (Ty::Int, Json::Number(n)) => n
            .as_i64()
            .map(Value::Int)
            .ok_or_else(|| "an integer that fits 64 bits".into()),
        (Ty::Real, Json::Number(n)) => n
            .as_f64()
            .filter(|x| x.is_finite())
            .map(Value::Real)
            .ok_or_else(|| "a finite number".into()),
        (Ty::Bool, _) => Err("true or false".into()),
        (Ty::Int, _) => Err("an integer".into()),
        (Ty::Real, _) => Err("a number".into()),
    }
}

/// Whether every port reading signal `name` can carry `v`: a module whose
/// input does not fit it would be held, so the value is refused instead.
fn fits(c: &super::catalog::Catalog, name: &str, v: Value) -> Result<(), String> {
    for m in &c.modules {
        for p in m
            .ports
            .iter()
            .filter(|p| p.dir == helmsman_iec::PortDir::Input && p.name.eq_ignore_ascii_case(name))
        {
            let ok = match v {
                Value::Int(n) if p.ty.is_integral() && p.ty != helmsman_iec::Ty::Bool => {
                    let (lo, hi) = p.ty.range();
                    (lo..=hi).contains(&i128::from(n))
                }
                #[allow(clippy::cast_possible_truncation)] // only compared to f32's range
                Value::Real(x) if p.ty == helmsman_iec::Ty::Real => x.abs() <= f64::from(f32::MAX),
                _ => true,
            };
            if !ok {
                return Err(format!(
                    "out of range for {} {} in module {}",
                    p.ty, p.name, m.name
                ));
            }
        }
    }
    Ok(())
}
