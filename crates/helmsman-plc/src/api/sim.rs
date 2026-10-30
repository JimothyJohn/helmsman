//! `POST /api/proposals/replay`: a draft replayed beside what runs, over
//! the last minutes of the box's recording, before anything is stored.
//! The agent's `simulate_module` runs here so it can iterate on what the
//! simulator shows before a human sees a proposal.
//!
//! Read-only: it compiles both change sets, replays each on a copy of the
//! plant's table ([`crate::replay::replay`]) and compares them, exactly as
//! the evidence replay of a stored proposal does ([`super::approve`]),
//! with the same JSON shape. It is not kept as evidence: an apply cites a
//! replay of the stored proposal (`POST /api/proposals/{id}/replay`).

use std::time::Duration;

use serde_json::{Value as Json, json};

use super::render;
use super::swap::{Plant, compile_set};
use crate::record::{Event, Recording};
use crate::replay;
use crate::store::ChangeSet;

/// Longest window a draft replay may ask for, minutes.
pub const MAX_MINUTES: u64 = 1440;

/// The last `minutes` of `rec`: each channel's value at the new start as
/// its base, and the changes after it.
#[must_use]
pub fn tail(rec: &Recording, minutes: u64) -> Recording {
    let span = Duration::from_secs(minutes.saturating_mul(60));
    let start = rec.end.saturating_sub(span).max(rec.start);
    if start == rec.start {
        return rec.clone();
    }
    let base = (0..rec.channels.len())
        .map(|k| rec.value_at(u32::try_from(k).unwrap_or(u32::MAX), start))
        .collect();
    let events: Vec<Event> = rec.events.iter().filter(|e| e.at > start).copied().collect();
    Recording {
        wall_ms: rec.wall_ms,
        start,
        end: rec.end,
        channels: rec.channels.clone(),
        base,
        events,
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Replays `running` and `target` over the last `minutes` recorded.
///
/// # Errors
/// Status and body: 409 with nothing recorded, 422 when a module does not
/// compile or the replay cannot run.
pub fn run(
    plant: &Plant,
    running: &ChangeSet,
    target: &ChangeSet,
    minutes: u64,
) -> Result<Json, (u16, Json)> {
    let Some(tap) = &plant.record else {
        return Err((409, json!({ "error": "nothing is recorded on this box" })));
    };
    let (cur, new) = match (
        compile_set(running, &plant.defaults),
        compile_set(target, &plant.defaults),
    ) {
        (Ok(c), Ok(n)) => (c, n),
        (Err(e), _) | (_, Err(e)) => {
            return Err((
                422,
                json!({ "error": "a module does not compile", "problems": e }),
            ));
        }
    };
    let rec = tail(&tap.snapshot(), minutes.clamp(1, MAX_MINUTES));
    let replay_set = |m| replay::replay(&plant.table, plant.i_len, plant.q_len, m, &rec);
    let (current, proposed) = replay_set(cur)
        .and_then(|c| replay_set(new).map(|n| (c, n)))
        .map_err(|e| (422, json!({ "error": format!("the replay cannot run: {e}") })))?;
    let summary =
        |s: &replay::Summary| json!({ "rises": s.rises, "changes": s.changes, "on_ms": ms(s.on_time) });
    let signals: Vec<Json> = replay::compare(&current, &proposed)
        .iter()
        .map(|d| {
            json!({
                "signal": d.signal,
                "current": summary(&d.current),
                "proposed": summary(&d.proposed),
                "identical": d.identical,
                "extra_rises": d.extra_rises(),
            })
        })
        .collect();
    let faults: Vec<Json> = proposed
        .faults
        .iter()
        .map(|(at, m, f)| json!({ "at_ms": ms(*at), "module": m, "fault": render::fault(*f) }))
        .collect();
    let window = rec.end.saturating_sub(rec.start);
    Ok(json!({
        "base": running.id().to_string(),
        "content": target.id().to_string(),
        "start_ms": ms(rec.start),
        "end_ms": ms(rec.end),
        "window_ms": ms(window),
        "ticks": proposed.ticks,
        "signals": signals,
        "faults": faults,
        "evidence": false,
    }))
}
