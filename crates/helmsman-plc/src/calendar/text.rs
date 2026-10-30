//! The calendar as a text file, one declaration per line:
//!
//! ```text
//! # Plant 2, Central European time; offsets change on the EU dates
//! zone +01:00
//! zone +02:00 from 2026-03-29T01:00Z
//! zone +01:00 from 2026-10-25T01:00Z
//!
//! shift 1 mon-fri 06:00-14:00
//! shift 2 mon-fri 14:00-22:00
//! shift 3 sun-thu 22:00-06:00        # crosses midnight
//! break 1 10:00-10:30
//! break 3 02:00-02:30
//! down  mon-fri 13:45-14:00          # weekly cleaning
//!
//! holiday 2026-12-25                 # no weekly shifts start that day
//! holiday 2026-12-24 3               # only shift 3 is off
//! shift 2 2026-11-06 14:00-24:00     # replaces Friday's shift 2
//! shift 1 2026-11-07 06:00-12:00     # Saturday overtime
//! down  2026-11-02 06:00-10:00       # dated planned downtime
//!
//! drive horn  on_break
//! drive beacon !working
//! drive heater shift=3
//! ```
//!
//! - `zone OFFSET [from YYYY-MM-DDTHH:MMZ]`: the UTC offset (`+01:00`,
//!   `-05:00`, `+05:45`, `UTC`) local times are in. One `zone` has no
//!   `from`; each other one changes the offset at that UTC instant. Zone
//!   names (`Europe/Berlin`) are refused: there is no time zone database.
//! - `shift N DAYS|DATE HH:MM-HH:MM`: shift `N` (1..=99). `DAYS` is
//!   `daily` or a comma list of days and wrapping ranges (`mon-fri`,
//!   `sun-thu`, `mon,wed,fri`); a date makes it a dated exception that
//!   replaces shift `N`'s weekly occurrence that day. An end at or before
//!   the start crosses midnight; `24:00` is midnight at the end of the day.
//! - `break N HH:MM-HH:MM`: a break in each occurrence of shift `N` that
//!   holds it.
//! - `down DAYS|DATE HH:MM-HH:MM`: planned downtime, shifts or not.
//! - `holiday DATE [N]`: cancels the weekly shifts (or only shift `N`)
//!   starting that date.
//! - `drive SIGNAL [!]COND`: the box drives the boolean output `SIGNAL`
//!   from the calendar; `COND` is `in_shift`, `on_break`, `planned_down`,
//!   `working` or `shift=N`.
//!
//! `#` starts a comment; tokens are separated by spaces or tabs.
//! [`parse`] checks the syntax, [`super::Calendar::validate`] the rest, and
//! [`load`] does both.

use std::fmt::Write as _;

use super::civil::{Date, civil_from_days, digits};
use super::{Break, Calendar, Cond, Down, Drive, Holiday, MAX_OFFSET_MIN, MAX_SHIFT, On, Shift, Span, Zone};

pub(crate) const DAY_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `HH:MM`, 00:00..=23:59, or 24:00 when `end`.
fn clock(s: &str, end: bool) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m) = (digits(h)?, digits(m)?);
    match (h, m) {
        (24, 0) if end => Some(1440),
        (0..=23, 0..=59) => Some(h * 60 + m),
        _ => None,
    }
}

fn span(s: &str) -> Result<Span, String> {
    let bad = || format!("{s:?}: expected HH:MM-HH:MM");
    let (a, b) = s.split_once('-').ok_or_else(bad)?;
    let (start, end) = (clock(a, false).ok_or_else(bad)?, clock(b, true).ok_or_else(bad)?);
    if start == end {
        return Err(format!("{s:?}: starts where it ends; a whole day is 00:00-24:00"));
    }
    let len = if end > start {
        end - start
    } else {
        end + 1440 - start
    };
    Span::new(start, len).ok_or_else(|| format!("{s:?}: a 24 h window must be 00:00-24:00"))
}

fn day_name(s: &str) -> Option<u8> {
    DAY_NAMES
        .iter()
        .position(|d| *d == s)
        .and_then(|p| u8::try_from(p).ok())
}

fn days(s: &str) -> Result<On, String> {
    if let Some(d) = Date::parse(s) {
        return Ok(On::Date(d));
    }
    if s == "daily" {
        return Ok(On::Weekly(0x7f));
    }
    let bad = || format!("{s:?}: expected daily, days like mon-fri or mon,wed, or YYYY-MM-DD");
    let mut mask = 0u8;
    for item in s.split(',') {
        let (a, b) = item.split_once('-').unwrap_or((item, item));
        let (a, b) = (day_name(a).ok_or_else(bad)?, day_name(b).ok_or_else(bad)?);
        let mut d = a;
        loop {
            mask |= 1 << d;
            if d == b {
                break;
            }
            d = (d + 1) % 7;
        }
    }
    Ok(On::Weekly(mask))
}

fn shift_id(s: &str) -> Result<u8, String> {
    digits(s)
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| (1..=MAX_SHIFT).contains(n))
        .ok_or_else(|| format!("shift {s:?}: a number 1..={MAX_SHIFT}"))
}

fn offset(s: &str) -> Result<i16, String> {
    if s == "UTC" || s == "Z" {
        return Ok(0);
    }
    if s.contains('/') {
        return Err(format!(
            "{s:?}: zone names need a time zone database, which the box does not have; \
             write the offset (+01:00) and each change as `zone +02:00 from 2026-03-29T01:00Z`"
        ));
    }
    let bad = || format!("{s:?}: expected an offset like +01:00, -05:00 or UTC");
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'+') => (1, &s[1..]),
        Some(b'-') => (-1, &s[1..]),
        _ => return Err(bad()),
    };
    let (h, m) = rest.split_once(':').ok_or_else(bad)?;
    if h.len() != 2 || m.len() != 2 {
        return Err(bad());
    }
    let (h, m) = (digits(h).ok_or_else(bad)?, digits(m).ok_or_else(bad)?);
    let min = i16::try_from(h * 60 + m).map_err(|_| bad())?;
    if m > 59 || min > MAX_OFFSET_MIN {
        return Err(format!("{s:?}: offsets are within ±18:00"));
    }
    Ok(sign * min)
}

/// `YYYY-MM-DDTHH:MMZ`, as seconds since the epoch.
fn instant(s: &str) -> Result<i64, String> {
    let bad = || format!("{s:?}: expected a UTC time like 2026-03-29T01:00Z");
    let body = s.strip_suffix('Z').ok_or_else(bad)?;
    let (date, time) = body.split_once('T').ok_or_else(bad)?;
    let date = Date::parse(date).ok_or_else(bad)?;
    let min = clock(time, false).ok_or_else(bad)?;
    Ok(date.day() * 86_400 + i64::from(min) * 60)
}

fn cond(s: &str) -> Result<(Cond, bool), String> {
    let (negate, s) = s.strip_prefix('!').map_or((false, s), |r| (true, r));
    let c = match s {
        "in_shift" => Cond::InShift,
        "on_break" => Cond::OnBreak,
        "planned_down" => Cond::PlannedDown,
        "working" => Cond::Working,
        _ => match s.strip_prefix("shift=") {
            Some(n) => Cond::Shift(shift_id(n)?),
            None => {
                return Err(format!(
                    "{s:?}: expected in_shift, on_break, planned_down, working or shift=N"
                ));
            }
        },
    };
    Ok((c, negate))
}

fn declaration(words: &[&str], cal: &mut Calendar) -> Result<(), String> {
    if let Some(w) = words.iter().find(|w| !w.bytes().all(|b| b.is_ascii_graphic())) {
        return Err(format!("{w:?}: only printable ASCII"));
    }
    match words {
        ["zone", off] => cal.zones.push(Zone {
            offset_min: offset(off)?,
            from: None,
        }),
        ["zone", off, "from", at] => cal.zones.push(Zone {
            offset_min: offset(off)?,
            from: Some(instant(at)?),
        }),
        ["shift", id, on, sp] => cal.shifts.push(Shift {
            id: shift_id(id)?,
            on: days(on)?,
            span: span(sp)?,
        }),
        ["break", id, sp] => cal.breaks.push(Break {
            shift: shift_id(id)?,
            span: span(sp)?,
        }),
        ["down", on, sp] => cal.downs.push(Down {
            on: days(on)?,
            span: span(sp)?,
        }),
        ["holiday", date, rest @ ..] if rest.len() <= 1 => cal.holidays.push(Holiday {
            date: Date::parse(date).ok_or_else(|| format!("{date:?}: expected a date YYYY-MM-DD"))?,
            shift: rest.first().map(|n| shift_id(n)).transpose()?,
        }),
        ["drive", signal, c] => {
            if !is_ident(signal) {
                return Err(format!("{signal:?} is not an identifier"));
            }
            let (cond, negate) = cond(c)?;
            cal.drives.push(Drive {
                signal: (*signal).into(),
                cond,
                negate,
            });
        }
        [
            w @ ("zone" | "shift" | "break" | "down" | "holiday" | "drive"),
            ..,
        ] => {
            return Err(format!("{w}: wrong number of words"));
        }
        [w, ..] => {
            return Err(format!(
                "{w:?}: expected zone, shift, break, down, holiday or drive"
            ));
        }
        [] => {}
    }
    Ok(())
}

/// Parses a calendar file, checking only the syntax.
///
/// # Errors
/// Every bad line, as `(line number from 1, why)`.
pub fn parse(src: &str) -> Result<Calendar, Vec<(usize, String)>> {
    let mut cal = Calendar::default();
    let mut errs = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default();
        let words: Vec<&str> = line.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
        if let Err(e) = declaration(&words, &mut cal) {
            errs.push((n + 1, e));
        }
    }
    if errs.is_empty() { Ok(cal) } else { Err(errs) }
}

/// [`parse`] then [`Calendar::validate`], as messages for a person.
///
/// # Errors
/// Every syntax error (`line N: ...`), or else every validation error.
pub fn load(src: &str) -> Result<Calendar, Vec<String>> {
    let cal = parse(src).map_err(|errs| {
        errs.into_iter()
            .map(|(n, e)| format!("line {n}: {e}"))
            .collect::<Vec<_>>()
    })?;
    cal.validate()
        .map_err(|errs| errs.iter().map(ToString::to_string).collect::<Vec<_>>())?;
    Ok(cal)
}

fn hhmm(min: u16) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

fn render_span(s: Span) -> String {
    let end = s.start + s.len;
    let end = if end == 1440 && s.start == 0 {
        1440
    } else {
        end % 1440
    };
    format!("{}-{}", hhmm(s.start), hhmm(end))
}

fn render_on(on: On) -> String {
    match on {
        On::Date(d) => d.to_string(),
        On::Weekly(0x7f) => "daily".into(),
        On::Weekly(mask) => {
            let names: Vec<&str> = (0..7)
                .filter(|d| mask & (1 << d) != 0)
                .map(|d| DAY_NAMES[d])
                .collect();
            names.join(",")
        }
    }
}

fn render_offset(min: i16) -> String {
    let sign = if min < 0 { '-' } else { '+' };
    let a = min.unsigned_abs();
    format!("{sign}{:02}:{:02}", a / 60, a % 60)
}

fn render_instant(at: i64) -> String {
    let (year, month, day) = civil_from_days(at.div_euclid(86_400));
    let secs = at.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Writes a file that [`parse`] reads back as `cal`. A calendar that names
/// no weekdays in a `Weekly` pattern (mask 0) renders a line [`parse`]
/// refuses: no file can say that.
#[must_use]
pub fn render(cal: &Calendar) -> String {
    let mut s = String::new();
    for z in &cal.zones {
        let _ = write!(s, "zone {}", render_offset(z.offset_min));
        if let Some(t) = z.from {
            let _ = write!(s, " from {}", render_instant(t));
        }
        s.push('\n');
    }
    for x in &cal.shifts {
        let _ = writeln!(s, "shift {} {} {}", x.id, render_on(x.on), render_span(x.span));
    }
    for b in &cal.breaks {
        let _ = writeln!(s, "break {} {}", b.shift, render_span(b.span));
    }
    for d in &cal.downs {
        let _ = writeln!(s, "down {} {}", render_on(d.on), render_span(d.span));
    }
    for h in &cal.holidays {
        let _ = write!(s, "holiday {}", h.date);
        if let Some(n) = h.shift {
            let _ = write!(s, " {n}");
        }
        s.push('\n');
    }
    for d in &cal.drives {
        let neg = if d.negate { "!" } else { "" };
        let c = match d.cond {
            Cond::InShift => "in_shift".to_string(),
            Cond::OnBreak => "on_break".into(),
            Cond::PlannedDown => "planned_down".into(),
            Cond::Working => "working".into(),
            Cond::Shift(n) => format!("shift={n}"),
        };
        let _ = writeln!(s, "drive {} {neg}{c}", d.signal);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::civil::days_from_civil;

    #[test]
    fn instants_round_trip_through_days() {
        for t in [0_i64, 86_399, -60, 1_774_746_000, 253_402_300_740] {
            let t = t - t.rem_euclid(60);
            assert_eq!(instant(&render_instant(t)), Ok(t), "{t}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }
}
