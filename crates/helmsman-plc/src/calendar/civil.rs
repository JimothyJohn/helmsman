//! Proleptic Gregorian dates as day numbers (days since 1970-01-01), the
//! arithmetic the calendar needs and nothing more. Algorithms from Howard
//! Hinnant, "chrono-Compatible Low-Level Date Algorithms".

/// A calendar date, checked: year 1..=9999, a real day of that month.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub y: u16,
    pub m: u8,
    pub d: u8,
}

const fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

const fn month_len(y: i64, m: u8) -> u8 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Date {
    /// `None` unless it is a real date in years 1..=9999.
    #[must_use]
    pub const fn new(y: u16, m: u8, d: u8) -> Option<Self> {
        if y == 0 || y > 9999 || m == 0 || m > 12 || d == 0 || d > month_len(y as i64, m) {
            return None;
        }
        Some(Self { y, m, d })
    }

    /// Days since 1970-01-01 (negative before it).
    #[must_use]
    pub const fn day(self) -> i64 {
        days_from_civil(self.y as i64, self.m as i64, self.d as i64)
    }

    /// Parses `YYYY-MM-DD`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        Self::new(
            digits(&s[0..4])?,
            u8::try_from(digits(&s[5..7])?).ok()?,
            u8::try_from(digits(&s[8..10])?).ok()?,
        )
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

/// Only ASCII digits (no sign, no spaces), as a number.
pub(crate) fn digits(s: &str) -> Option<u16> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
#[must_use]
pub const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number, as `(year, month, day)`.
#[must_use]
pub const fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Weekday of a day number, Monday = 0 .. Sunday = 6.
#[must_use]
pub fn weekday(day: i64) -> u8 {
    // 1970-01-01 was a Thursday (3). No overflow for any day number.
    u8::try_from((day % 7 + 10) % 7).unwrap_or(0)
}

/// Seconds since the Unix epoch of a UTC civil time; for tests and logs.
#[must_use]
pub const fn utc(y: i64, m: i64, d: i64, hh: i64, mm: i64) -> i64 {
    days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60
}
