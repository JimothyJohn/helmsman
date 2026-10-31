//! `POST /api/login` and `POST /api/logout`: per-person sessions
//! ([`crate::policy::account`]).
//!
//! A login takes `{user, password}` and no token (it is its own
//! credential), and answers a session token the caller sends as
//! `Authorization: Bearer` from then on: wherever the box token goes, and
//! for apply and rollback in place of a role PIN. The password is only
//! ever in the request body: never logged, audited or echoed (a malformed
//! body gets a fixed message, not serde's), and a user name is logged or
//! audited only when it names an account, so a password typed into the
//! user field never lands in the log.
//!
//! Audited: every login that worked and every wrong password, with the
//! account (and the session's role). Not audited, only logged: refusals
//! while logins are locked, and logins on a box without accounts, so an
//! unauthenticated flood cannot fill the audit log (which refuses rather
//! than forgets).

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Note, Request, Response, append_as, audit_full, clean, logins, record};
use crate::policy::account::{LoginRefused, Mode, valid_user};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    user: String,
    password: String,
}

/// `POST /api/login`.
pub(super) fn login(req: &Request, ctx: &Ctx) -> Response {
    let log = |what: &str| (ctx.cfg.log)(&format!("api: POST {}: {what}", clean(&req.path)));
    let Some(l) = logins(ctx) else {
        log("refused: this box has no accounts");
        return Response::json(
            409,
            &json!({ "error": "this box has no accounts: approvals use role PINs" }),
        );
    };
    // serde's message can quote the body, password included: never echoed.
    let Ok(body) = serde_json::from_slice::<Body>(&req.body) else {
        log("refused: malformed login");
        return Response::json(
            400,
            &json!({ "error": "expected JSON {\"user\": ..., \"password\": ...}" }),
        );
    };
    let known =
        valid_user(&body.user) && matches!(l.mode(), Mode::Accounts(a) if a.get(&body.user).is_some());
    let named = if known {
        body.user.clone()
    } else {
        "an unknown user".to_string()
    };
    let outcome = l.login(&body.user, &body.password);
    drop(body);
    match outcome {
        Ok(issued) => {
            let who = &issued.who;
            let detail = format!("login: {} ({})", who.user, who.role);
            let audited = append_as(
                ctx,
                req,
                200,
                &detail,
                (None, None),
                Some(who.user.clone()),
                Some(who.role),
            );
            if audited.is_none() {
                l.logout(&issued.token);
                return audit_full();
            }
            Response::json(
                200,
                &json!({
                    "token": issued.token,
                    "user": who.user,
                    "role": who.role.as_str(),
                    "expires_in_s": issued.ttl.as_secs(),
                    "idle_s": issued.idle.as_secs(),
                }),
            )
            .with("Cache-Control", "no-store".into())
        }
        Err(LoginRefused::Wrong) => {
            let user = known.then(|| named.clone());
            match append_as(
                ctx,
                req,
                403,
                &format!("login refused: wrong user or password ({named})"),
                (None, None),
                user,
                None,
            ) {
                Some(_) => Response::json(403, &json!({ "error": "wrong user or password" })),
                None => audit_full(),
            }
        }
        Err(LoginRefused::Locked(wait)) => {
            let secs = wait.as_secs().max(1);
            log(&format!("refused ({named}): logins locked for {secs} s"));
            Response::json(
                429,
                &json!({ "error": "too many wrong passwords; logins are locked", "retry_after_s": secs }),
            )
            .with("Retry-After", secs.to_string())
        }
        Err(LoginRefused::NoAccounts) => {
            log("refused: this box has no accounts");
            Response::json(
                409,
                &json!({ "error": "this box has no accounts: approvals use role PINs" }),
            )
        }
        Err(LoginRefused::Unavailable(why)) => {
            log(&format!("refused: {}", clean(&why)));
            Response::json(
                503,
                &json!({ "error": "the box's accounts cannot be used now; nobody can log in (see its journal)" }),
            )
        }
        Err(LoginRefused::Full) => {
            log(&format!("refused ({named}): too many sessions"));
            Response::json(503, &json!({ "error": "too many sessions; try again later" }))
        }
    }
}

/// `POST /api/logout`: ends the session it was sent with.
pub(super) fn logout(req: &Request, ctx: &Ctx) -> Response {
    let ended = match (&req.session, req.bearer.as_deref(), logins(ctx)) {
        (Some(_), Some(t), Some(l)) => l.logout(t),
        _ => None,
    };
    let (status, body, detail) = match ended {
        Some(who) => (
            200,
            json!({ "logged_out": true }),
            format!("logout: {}", who.user),
        ),
        None => (
            400,
            json!({ "error": "not a login session; nothing to log out" }),
            "logout: not a session".to_string(),
        ),
    };
    match record(ctx, req, status, &detail, Note::default()) {
        Some(_) => Response::json(status, &body),
        None => audit_full(),
    }
}
