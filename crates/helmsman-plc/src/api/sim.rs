//! Sim replay through the API: a module draft replayed beside what runs
//! now over the box's rolling recording ([`crate::record`],
//! [`crate::replay::propose`]). Read-only: nothing here touches the
//! runtime, the signal table it scans, or the proposals. The replay runs
//! on a copy of the table taken at startup, on the API worker's thread,
//! one at a time.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use helmsman_iec::{ImageSizes, compile};
use serde_json::{Value as Json, json};

use super::render;
use crate::manifest::ModuleDir;
use crate::record::{Event, RecordTap, Recording};
use crate::replay::{self, Proposal, Timeline};
use crate::runtime::{Module, Status};
use crate::signals::SignalTable;
use crate::spec::Defaults;

/// Longest window a replay may ask for.
pub const MAX_MINUTES: u64 = 1440;
/// Most value changes per output signal returned for a chart.
pub const MAX_CHANGES: usize = 500;

/// What a replay runs against: the recorder's tap, the table and images
/// the runtime was built with, and the modules it runs.
#[derive(Debug)]
pub struct Source {
    pub tap: RecordTap,
    /// The live table as built (calendar attached); values are ignored.
    pub table: SignalTable,
    pub i_len: usize,
    pub q_len: usize,
    pub running: Vec<Module>,
    pub defaults: Defaults,
    /// One replay at a time; the next gets a 503.
    pub busy: Mutex<()>,
}

impl Source {
    /// `images` is `(i_len, q_len)`, as the runtime was built.
    #[must_use]
    pub fn new(
        tap: RecordTap,
        table: SignalTable,
        images: (usize, usize),
        running: Vec<Module>,
        defaults: Defaults,
    ) -> Self {
        Self {
            tap,
            table,
            i_len: images.0,
            q_len: images.1,
            running,
            defaults,
            busy: Mutex::new(()),
        }
    }
}

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

fn status(s: Status) -> Json {
    match s {
        Status::Running => json!("running"),
        Status::Held => json!("held"),
        Status::Faulted(f) => json!({ "faulted": render::fault(f) }),
    }
}

fn timeline(t: &Timeline, only: &str) -> Json {
    json!({
        "ticks": t.ticks,
        "outputs": t.outputs.iter().filter(|o| o.module == only).map(|o| json!({
            "signal": o.signal,
            "changes": o.changes.iter().take(MAX_CHANGES).map(|(at, v)| json!([ms(*at), render::value(*v)])).collect::<Vec<_>>(),
            "truncated": o.changes.len() > MAX_CHANGES,
        })).collect::<Vec<_>>(),
        "status": t.module(only).map(|m| m.spans.iter().map(|s| json!({
            "from_ms": ms(s.from), "to_ms": ms(s.to), "status": status(s.status),
        })).collect::<Vec<_>>()),
        "faults": t.faults.iter().map(|(at, m, f)| json!({ "at_ms": ms(*at), "module": m, "fault": render::fault(*f) })).collect::<Vec<_>>(),
    })
}

/// A replay's result as JSON: the window, what differs per output signal,
/// and the proposed module's outputs and statuses over time.
#[must_use]
pub fn render(p: &Proposal, name: &str, rec: &Recording) -> Json {
    json!({
        "module": name,
        "window": {
            "start_ms": ms(rec.start),
            "end_ms": ms(rec.end),
            "seconds": rec.end.saturating_sub(rec.start).as_secs(),
            "wall_ms_at_zero": rec.wall_ms,
        },
        "diff": p.diff.iter().map(|d| json!({
            "signal": d.signal,
            "identical": d.identical,
            "extra_rises": d.extra_rises(),
            "current": { "rises": d.current.rises, "changes": d.current.changes, "on_ms": ms(d.current.on_time) },
            "proposed": { "rises": d.proposed.rises, "changes": d.proposed.changes, "on_ms": ms(d.proposed.on_time) },
        })).collect::<Vec<_>>(),
        "proposed": timeline(&p.proposed, name),
    })
}

/// Replays a module draft (its manifest and source, already checked by
/// [`super::proposal::check`]) over the last `minutes` of the recording.
///
/// # Errors
/// Status and body: 503 while another replay runs, 422 when the draft
/// cannot be built or replayed.
pub fn run(src: &Source, manifest: &str, source: &str, minutes: u64) -> Result<Json, (u16, Json)> {
    let Ok(_guard) = src.busy.try_lock() else {
        return Err((503, json!({ "error": "another replay is running; try again" })));
    };
    let md = ModuleDir::from_text(Path::new("draft"), manifest, source.to_string(), &src.defaults)
        .map_err(|e| (422, json!({ "error": e })))?;
    let sizes = ImageSizes {
        i: md.spec.i.len,
        q: md.spec.q.len,
        m: md.spec.m,
    };
    let program = compile(source, sizes).map_err(|d| {
        let lines: Vec<String> = d.iter().map(ToString::to_string).collect();
        (
            422,
            json!({ "error": "source does not compile", "problems": lines }),
        )
    })?;
    let module = md.module(program).map_err(|e| (422, json!({ "error": e })))?;
    let name = module.name.clone();
    let rec = tail(&src.tap.snapshot(), minutes.clamp(1, MAX_MINUTES));
    let p = replay::propose(&src.table, src.i_len, src.q_len, &src.running, module, &rec)
        .map_err(|e| (422, json!({ "error": format!("cannot replay: {e}") })))?;
    Ok(render(&p, &name, &rec))
}
