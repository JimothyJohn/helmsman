//! The Skipper's wall clock. A Skipper has no clock of its own: it counts
//! milliseconds since it started, and holds a wall time only once a page
//! has told it (`PUT /api/time`, in RAM, gone at its next restart). A page
//! watching one reads `GET /api/node` every [`ASK_EVERY`] s; when the
//! Skipper has no `unix_ms`, or its is more than [`MAX_SKEW_MS`] off this
//! browser's clock, and the page holds the Skipper's token, the page sets
//! it from the browser's time, at most once per [`SET_EVERY`] s.
//!
//! Every time the page shows for the Skipper is [`SkipperClock::wall`]:
//! the Skipper's own `unix_ms` when it has one, else the browser's clock
//! minus the uptime difference.

use serde_json::Value as Json;

use crate::box_projects::{Answer, PCall, PReq};

/// How often the Skipper's node info is read, page seconds.
const ASK_EVERY: f64 = 10.0;
/// The most often the page sets the Skipper's clock, page seconds.
pub const SET_EVERY: f64 = 30.0;
/// How far the Skipper's clock may be from the browser's before the page
/// sets it, ms.
pub const MAX_SKEW_MS: u64 = 2000;

/// A Skipper's clock reading at one page time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Base {
    /// The Skipper's uptime and the unix time that went with it.
    uptime_ms: u64,
    unix_ms: u64,
    /// The Skipper said `unix_ms` itself (else it is the browser's clock).
    from_node: bool,
    /// Page time of the reading.
    page_at: u64,
}

/// A time to show, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// `14:03:27`, in the browser's time zone.
    pub time: String,
    /// `2026-11-26 14:03:27`.
    pub dated: String,
    /// The Skipper's own clock said it (else the browser's did).
    pub from_node: bool,
}

#[derive(Debug, Default)]
pub struct SkipperClock {
    base: Option<Base>,
    asked: Option<f64>,
    set_at: Option<f64>,
    out: Vec<PReq>,
}

impl SkipperClock {
    /// Asks for the node info when due. `watches`: the page watches a
    /// Skipper. Returns the requests to send.
    pub fn step(&mut self, now: f64, watches: bool) -> Vec<PReq> {
        if !watches {
            *self = Self::default();
            return Vec::new();
        }
        if self.asked.is_none_or(|a| now - a >= ASK_EVERY) {
            self.asked = Some(now);
            self.out.push(PReq::get(PCall::NodeInfo, "/api/node".into()));
        }
        std::mem::take(&mut self.out)
    }

    /// The answer to `GET /api/node` at page time `now` (browser clock
    /// `wall_ms`); `token`: the page holds the Skipper's token.
    pub fn node_info(&mut self, a: &Answer, now: f64, wall_ms: u64, token: bool) {
        if a.status != 200 {
            return;
        }
        let Ok(j) = serde_json::from_str::<Json>(&a.body) else {
            return;
        };
        let Some(uptime_ms) = j["uptime_ms"].as_u64() else {
            return;
        };
        let theirs = j["unix_ms"].as_u64();
        self.base = Some(Base {
            uptime_ms,
            unix_ms: theirs.unwrap_or(wall_ms),
            from_node: theirs.is_some(),
            page_at: page_ms(now),
        });
        let off = theirs.is_none_or(|t| t.abs_diff(wall_ms) > MAX_SKEW_MS);
        if token && off && self.set_at.is_none_or(|s| now - s >= SET_EVERY) {
            self.set_at = Some(now);
            self.out.push(PReq::json(
                PCall::SetTime,
                "PUT",
                "/api/time".into(),
                &serde_json::json!({ "unix_ms": wall_ms }),
            ));
        }
    }

    /// The answer to `PUT /api/time`: read the node info again at once.
    pub fn time_set(&mut self, a: &Answer) {
        if (200..300).contains(&a.status) {
            self.asked = None;
        }
    }

    /// The Skipper's uptime `at_ms` as a time, or `None` before its info
    /// was read.
    #[must_use]
    pub fn wall(&self, at_ms: u64) -> Option<Shown> {
        let b = self.base?;
        // Checked: what a Skipper answers is not trusted (the tester's
        // find: u64::MAX for both overflowed the sum).
        let unix = b.unix_ms.checked_add(at_ms)?.checked_sub(b.uptime_ms)?;
        Some(Shown {
            time: clock_text(unix),
            dated: format!("{} {}", date_text(unix), clock_text(unix)),
            from_node: b.from_node,
        })
    }

    /// The Skipper's uptime now, ms, from the last reading.
    #[must_use]
    pub fn uptime_now(&self, now: f64) -> Option<u64> {
        let b = self.base?;
        Some(b.uptime_ms.saturating_add(page_ms(now).saturating_sub(b.page_at)))
    }

    /// The tooltip for a shown time.
    #[must_use]
    pub fn tip(s: &Shown, what: &str) -> String {
        let source = if s.from_node {
            "the Skipper's own clock, which this page set"
        } else {
            "this browser's clock less how long the Skipper has been up, since the Skipper's clock is not set"
        };
        format!("{what}, {} (your time zone). From {source}.", s.dated)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn page_ms(now: f64) -> u64 {
    (now.max(0.0) * 1000.0) as u64
}

/// `(year, month, day, hour, minute, second)` of `unix_ms` in the
/// browser's time zone (UTC off the browser).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parts(unix_ms: u64) -> (i64, u32, u32, u32, u32, u32) {
    #[cfg(target_arch = "wasm32")]
    {
        let d = web_sys::js_sys::Date::new(&web_sys::wasm_bindgen::JsValue::from_f64(unix_ms as f64));
        (
            i64::from(d.get_full_year()),
            d.get_month() + 1,
            d.get_date(),
            d.get_hours(),
            d.get_minutes(),
            d.get_seconds(),
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs = unix_ms / 1000;
        let rem = secs % 86_400;
        // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
        let z = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = yoe + era * 400 + i64::from(mo <= 2);
        (
            y,
            mo,
            d,
            (rem / 3600) as u32,
            (rem % 3600 / 60) as u32,
            (rem % 60) as u32,
        )
    }
}

fn clock_text(unix_ms: u64) -> String {
    let (_, _, _, h, m, s) = parts(unix_ms);
    format!("{h:02}:{m:02}:{s:02}")
}

fn date_text(unix_ms: u64) -> String {
    let (y, mo, d, ..) = parts(unix_ms);
    format!("{y:04}-{mo:02}-{d:02}")
}
