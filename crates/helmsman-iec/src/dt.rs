//! DATE, TOD and DT: the calendar arithmetic behind them.
//!
//! All three are a count of nanoseconds in a signed 64-bit integer, as
//! ruSTy holds them: DATE and DT from 1970-01-01T00:00:00 (the proleptic
//! Gregorian calendar, no time zone, no leap seconds), TOD from midnight.
//! Everything here is integer arithmetic on `i128` and total: it never
//! panics and never allocates, so it builds without `std`.
//!
//! The representable years run from 1677 to 2262 (the span 64 bits of
//! nanoseconds reaches); a literal or a conversion outside it is refused
//! at compile time, and arithmetic that leaves it wraps like TIME does.

// y, m, d, h, s are the calendar's own names.
#![allow(clippy::many_single_char_names)]

#[cfg(not(feature = "std"))]
use crate::prelude::*;

use core::fmt::Write;

/// Nanoseconds in a second, a day.
pub const NS_SEC: i128 = 1_000_000_000;
pub const NS_DAY: i128 = 86_400 * NS_SEC;

const MIN: i128 = i64::MIN as i128;
const MAX: i128 = i64::MAX as i128;

#[must_use]
pub const fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[must_use]
pub const fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days from 1970-01-01 to the civil date (Hinnant's `days_from_civil`);
/// the date is not validated.
#[must_use]
pub const fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date `days` after 1970-01-01 (Hinnant's `civil_from_days`).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub const fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// Midnight of a valid civil date, in nanoseconds; `None` for a date that
/// does not exist or lies beyond the 64-bit range.
#[must_use]
pub fn date_ns(y: i64, m: u32, d: u32) -> Option<i128> {
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    if !(1000..=9999).contains(&y) {
        return None;
    }
    let ns = i128::from(days_from_civil(y, m, d)) * NS_DAY;
    (MIN..=MAX).contains(&ns).then_some(ns)
}

/// A time of day in nanoseconds; `None` outside 00:00:00 to 23:59:59.999999999.
#[must_use]
pub fn tod_ns(h: u32, m: u32, s: u32, frac: u32) -> Option<i128> {
    if h > 23 || m > 59 || s > 59 || frac >= 1_000_000_000 {
        return None;
    }
    Some((i128::from(h) * 3600 + i128::from(m) * 60 + i128::from(s)) * NS_SEC + i128::from(frac))
}

/// `CONCAT_DATE(y, m, d)`: midnight of the date, or 0 (1970-01-01) when
/// there is no such date, as ruSTy's `concat_date` (a year or day that
/// does not fit 32 bits is no date either). A date beyond the 64-bit
/// range is the range's end on its side of 1970.
#[must_use]
pub fn concat_date(y: i128, m: i128, d: i128) -> i128 {
    let (Ok(y), Ok(m), Ok(d)) = (i32::try_from(y), u32::try_from(m), u32::try_from(d)) else {
        return 0;
    };
    // chrono's range, which ruSTy's function inherits.
    if !(-262_143..=262_142).contains(&y)
        || !(1..=12).contains(&m)
        || d < 1
        || d > days_in_month(i64::from(y), m)
    {
        return 0;
    }
    let ns = i128::from(days_from_civil(i64::from(y), m, d)) * NS_DAY;
    ns.clamp(MIN, MAX)
}

/// `CONCAT_TOD(h, m, s, ms)`: the time of day, or 0 when it is none
/// (hour above 23, minute or second above 59, milliseconds above 999,
/// or any part negative).
#[must_use]
pub fn concat_tod(h: i128, m: i128, s: i128, ms: i128) -> i128 {
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) || !(0..=59).contains(&s) || !(0..=999).contains(&ms)
    {
        return 0;
    }
    (h * 3600 + m * 60 + s) * NS_SEC + ms * 1_000_000
}

/// A reader over the characters of a literal's body.
struct Cur<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cur<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        let hit = self.peek() == Some(c);
        self.i += usize::from(hit);
        hit
    }

    /// 1 to `max` digits as a number; `exact` demands exactly `max`.
    fn digits(&mut self, max: usize, exact: bool) -> Option<u32> {
        let start = self.i;
        let mut v = 0u32;
        while self.i - start < max && self.peek().is_some_and(|c| c.is_ascii_digit()) {
            v = v * 10 + u32::from(self.s[self.i] - b'0');
            self.i += 1;
        }
        let n = self.i - start;
        (n > 0 && (!exact || n == max)).then_some(v)
    }

    fn done(&self) -> bool {
        self.i == self.s.len()
    }
}

const BAD_DATE: &str = "a date is YYYY-MM-DD";
const BAD_TIME: &str = "a time of day is hh:mm:ss or hh:mm:ss.fraction";

fn date_part(c: &mut Cur<'_>, whole_day_in_range: bool) -> Result<i128, &'static str> {
    let y = c.digits(4, true).ok_or(BAD_DATE)?;
    if !c.eat(b'-') {
        return Err(BAD_DATE);
    }
    let m = c.digits(2, false).ok_or(BAD_DATE)?;
    if !c.eat(b'-') {
        return Err(BAD_DATE);
    }
    let d = c.digits(2, false).ok_or(BAD_DATE)?;
    if !(1..=12).contains(&m) {
        return Err("the month is 1 to 12");
    }
    if d < 1 || d > days_in_month(i64::from(y), m) {
        return Err("no such day in that month");
    }
    let ns = i128::from(days_from_civil(i64::from(y), m, d)) * NS_DAY;
    if whole_day_in_range && !(MIN..=MAX).contains(&ns) {
        return Err("beyond the range of a DATE (1677-09-22 to 2262-04-11)");
    }
    Ok(ns)
}

fn tod_part(c: &mut Cur<'_>) -> Result<i128, &'static str> {
    let h = c.digits(2, false).ok_or(BAD_TIME)?;
    if !c.eat(b':') {
        return Err(BAD_TIME);
    }
    let m = c.digits(2, false).ok_or(BAD_TIME)?;
    if !c.eat(b':') {
        return Err(BAD_TIME);
    }
    let s = c.digits(2, false).ok_or(BAD_TIME)?;
    let mut frac = 0u32;
    if c.eat(b'.') {
        let start = c.i;
        let mut digits = 0u32;
        while c.peek().is_some_and(|d| d.is_ascii_digit()) {
            if digits == 9 {
                return Err("a fraction of a second has at most 9 digits (nanoseconds)");
            }
            frac = frac * 10 + u32::from(c.s[c.i] - b'0');
            digits += 1;
            c.i += 1;
        }
        if c.i == start {
            return Err(BAD_TIME);
        }
        frac *= 10u32.pow(9 - digits);
    }
    if h > 23 {
        return Err("the hour is 0 to 23");
    }
    if m > 59 || s > 59 {
        return Err("minutes and seconds are 0 to 59");
    }
    tod_ns(h, m, s, frac).ok_or(BAD_TIME)
}

/// `YYYY-MM-DD` (the text after `D#`) as nanoseconds.
///
/// # Errors
/// What is wrong with it, in words.
pub fn parse_date(s: &[u8]) -> Result<i128, &'static str> {
    let mut c = Cur { s, i: 0 };
    let v = date_part(&mut c, true)?;
    if c.done() { Ok(v) } else { Err(BAD_DATE) }
}

/// `hh:mm:ss[.fraction]` (the text after `TOD#`) as nanoseconds.
///
/// # Errors
/// What is wrong with it, in words.
pub fn parse_tod(s: &[u8]) -> Result<i128, &'static str> {
    let mut c = Cur { s, i: 0 };
    let v = tod_part(&mut c)?;
    if c.done() { Ok(v) } else { Err(BAD_TIME) }
}

/// `YYYY-MM-DD-hh:mm:ss[.fraction]` (the text after `DT#`) as nanoseconds.
///
/// # Errors
/// What is wrong with it, in words.
pub fn parse_dt(s: &[u8]) -> Result<i128, &'static str> {
    let mut c = Cur { s, i: 0 };
    let d = date_part(&mut c, false)?;
    if !c.eat(b'-') {
        return Err("a date and time is YYYY-MM-DD-hh:mm:ss");
    }
    let t = tod_part(&mut c)?;
    if !c.done() {
        return Err(BAD_TIME);
    }
    let v = d + t;
    if !(MIN..=MAX).contains(&v) {
        return Err("beyond the range of a DT (years 1677 to 2262)");
    }
    Ok(v)
}

/// Splits a DT into whole days since 1970 and the nanoseconds into that day.
#[must_use]
pub fn split(ns: i128) -> (i128, i128) {
    (ns.div_euclid(NS_DAY), ns.rem_euclid(NS_DAY))
}

/// The DATE (midnight) of a DT; a value whose midnight lies below the
/// range gives the range's minimum, as ruSTy's `DATE_AND_TIME_TO_DATE`.
#[must_use]
pub fn dt_to_date(ns: i128) -> i128 {
    (split(ns).0 * NS_DAY).max(MIN)
}

/// The time of day of a DT.
#[must_use]
pub fn dt_to_tod(ns: i128) -> i128 {
    split(ns).1
}

/// A TOD moved by a duration, wrapping at midnight.
#[must_use]
pub fn tod_shift(tod: i128, by: i128) -> i128 {
    (tod + by).rem_euclid(NS_DAY)
}

/// Day of the week of a DATE or DT: 0 is Sunday (1970-01-01 was a Thursday).
#[must_use]
pub fn weekday(ns: i128) -> i128 {
    (split(ns).0 + 4).rem_euclid(7)
}

/// Whether `v` is a day count the text forms print as four-digit years.
fn civil(ns: i128) -> (i64, u32, u32) {
    // |days| <= 106_752 for a 64-bit value, so the cast is exact.
    #[allow(clippy::cast_possible_truncation)]
    civil_from_days(split(ns).0 as i64)
}

struct Out<'a> {
    b: &'a mut [u8],
    n: usize,
}

impl Write for Out<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = self.b.len() - self.n;
        let k = s.len().min(room);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

/// Which of the three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Date,
    Tod,
    Dt,
}

/// The longest text [`format`] makes: `DT#2262-04-11-23:47:16.854775807`.
pub const TEXT_MAX: usize = 32;

/// The value as text in the literal's own form: `D#2024-02-29`,
/// `TOD#08:30:00.5` (the fraction only when there is one, without trailing
/// zeros), `DT#2024-02-29-08:30:00`. Returns the length written to `out`.
pub fn format(kind: Kind, ns: i128, out: &mut [u8; TEXT_MAX]) -> usize {
    let mut w = Out { b: out, n: 0 };
    let (y, m, d) = civil(ns);
    let t = split(ns).1;
    let (secs, frac) = (t / NS_SEC, t % NS_SEC);
    let (h, mi, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let _ = match kind {
        Kind::Date => write!(w, "D#{y:04}-{m:02}-{d:02}"),
        Kind::Tod => write!(w, "TOD#"),
        Kind::Dt => write!(w, "DT#{y:04}-{m:02}-{d:02}-"),
    };
    if kind != Kind::Date {
        let _ = write!(w, "{h:02}:{mi:02}:{s:02}");
        if frac != 0 {
            let mut digits = [b'0'; 9];
            let mut f = frac;
            for slot in digits.iter_mut().rev() {
                *slot = b'0' + u8::try_from(f % 10).unwrap_or(0);
                f /= 10;
            }
            let end = digits.iter().rposition(|&c| c != b'0').map_or(0, |p| p + 1);
            let _ = w.write_str(".");
            for &c in &digits[..end] {
                let _ = w.write_char(char::from(c));
            }
        }
    }
    w.n
}

/// A DATE, TOD or DT value written as the literal that reads back as it
/// (`TOD#08:30:00`); any other type gives its raw count.
#[must_use]
pub fn literal(ty: crate::types::Ty, ns: i128) -> String {
    use crate::types::Ty;
    let kind = match ty {
        Ty::Date => Kind::Date,
        Ty::Tod => Kind::Tod,
        Ty::Dt => Kind::Dt,
        _ => return ns.to_string(),
    };
    let mut b = [0u8; TEXT_MAX];
    let n = format(kind, ns, &mut b);
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// Text as a value of `kind`: the literal's body, optionally behind its
/// own prefix (`D#`, `DATE#`, `TOD#`, `TIME_OF_DAY#`, `DT#`,
/// `DATE_AND_TIME#`, any case). `None` for anything else.
#[must_use]
pub fn parse_text(kind: Kind, s: &[u8]) -> Option<i128> {
    let s = s.trim_ascii();
    let body = match s.iter().position(|&c| c == b'#') {
        Some(p) => {
            let names: &[&str] = match kind {
                Kind::Date => &["D", "DATE"],
                Kind::Tod => &["TOD", "TIME_OF_DAY"],
                Kind::Dt => &["DT", "DATE_AND_TIME"],
            };
            let prefix = &s[..p];
            if !names.iter().any(|n| prefix.eq_ignore_ascii_case(n.as_bytes())) {
                return None;
            }
            &s[p + 1..]
        }
        None => s,
    };
    match kind {
        Kind::Date => parse_date(body),
        Kind::Tod => parse_tod(body),
        Kind::Dt => parse_dt(body),
    }
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(kind: Kind, ns: i128) -> String {
        let mut b = [0u8; TEXT_MAX];
        let n = format(kind, ns, &mut b);
        String::from_utf8(b[..n].to_vec()).unwrap()
    }

    #[test]
    fn civil_round_trips_across_the_whole_range() {
        // Every day from year 1000 to 3000.
        let lo = days_from_civil(1000, 1, 1);
        let hi = days_from_civil(3000, 12, 31);
        let mut next = lo;
        for days in lo..=hi {
            let (y, m, d) = civil_from_days(days);
            assert!(d >= 1 && d <= days_in_month(y, m), "{days} -> {y}-{m}-{d}");
            assert_eq!(days_from_civil(y, m, d), days);
            if d == 1 && m == 1 {
                assert_eq!(days, next);
                next = days_from_civil(y + 1, 1, 1);
            }
        }
    }

    #[test]
    fn known_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn leap_years() {
        assert!(is_leap(2000) && is_leap(2024) && !is_leap(1900) && !is_leap(2100));
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(0), 4, "1970-01-01 was a Thursday");
        assert_eq!(weekday(parse_date(b"2024-02-29").unwrap()), 4);
        assert_eq!(weekday(parse_date(b"2000-01-01").unwrap()), 6);
        assert_eq!(weekday(-1), 3, "the last nanosecond of 1969-12-31 is a Wednesday");
    }

    #[test]
    fn text_round_trips() {
        for s in [
            "1970-01-01",
            "2024-02-29",
            "1900-03-01",
            "2262-04-11",
            "1677-09-22",
        ] {
            let ns = parse_date(s.as_bytes()).unwrap();
            assert_eq!(text(Kind::Date, ns), format!("D#{s}"));
            assert_eq!(parse_text(Kind::Date, text(Kind::Date, ns).as_bytes()), Some(ns));
        }
        for (s, shown) in [
            ("00:00:00", "00:00:00"),
            ("23:59:59.999999999", "23:59:59.999999999"),
            ("08:30:00.500", "08:30:00.5"),
            ("1:2:3", "01:02:03"),
        ] {
            let ns = parse_tod(s.as_bytes()).unwrap();
            assert_eq!(text(Kind::Tod, ns), format!("TOD#{shown}"));
            assert_eq!(parse_text(Kind::Tod, text(Kind::Tod, ns).as_bytes()), Some(ns));
        }
        let ns = parse_dt(b"2262-04-11-23:47:16.854775807").unwrap();
        assert_eq!(ns, MAX);
        assert_eq!(text(Kind::Dt, ns), "DT#2262-04-11-23:47:16.854775807");
        assert_eq!(text(Kind::Dt, ns).len(), TEXT_MAX);
        assert_eq!(text(Kind::Dt, MIN), "DT#1677-09-21-00:12:43.145224192");
    }

    #[test]
    fn refusals() {
        for bad in [
            "2024-13-01",
            "2024-00-10",
            "2023-02-29",
            "1900-02-29",
            "2024-04-31",
            "2024-1-0",
            "24-01-01",
            "2024/01/01",
            "2024-01-01x",
            "",
            "-2024-01-01",
            "2024-01-",
            "1600-01-01",
            "2262-04-12",
            "0000-01-01",
            "10000-01-01",
        ] {
            assert!(parse_date(bad.as_bytes()).is_err(), "{bad}");
        }
        for bad in [
            "24:00:00",
            "23:60:00",
            "23:59:60",
            "12:00",
            "12:00:",
            "12:00:00.",
            "12:00:00.1234567890",
            "12:00:00 ",
            "-1:00:00",
            "a:b:c",
            "123:00:00",
        ] {
            assert!(parse_tod(bad.as_bytes()).is_err(), "{bad}");
        }
        for bad in [
            "2024-01-01",
            "2024-01-01-",
            "2024-01-0112:00:00",
            "2262-04-11-23:47:16.854775808",
        ] {
            assert!(parse_dt(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn tod_wraps_at_midnight() {
        let t = |s: &str| parse_tod(s.as_bytes()).unwrap();
        assert_eq!(tod_shift(t("23:00:00"), 2 * 3600 * NS_SEC), t("01:00:00"));
        assert_eq!(tod_shift(t("00:00:00"), -1), NS_DAY - 1);
        assert_eq!(
            tod_shift(t("12:00:00"), i128::from(i64::MIN)),
            (t("12:00:00") + i128::from(i64::MIN)).rem_euclid(NS_DAY)
        );
    }

    #[test]
    fn dt_splits_before_1970_too() {
        let dt = parse_dt(b"1969-12-31-23:00:00").unwrap();
        assert_eq!(dt_to_date(dt), parse_date(b"1969-12-31").unwrap());
        assert_eq!(dt_to_tod(dt), 23 * 3600 * NS_SEC);
        assert_eq!(dt_to_date(MIN), MIN, "clamped as ruSTy's DATE_AND_TIME_TO_DATE");
    }
}
