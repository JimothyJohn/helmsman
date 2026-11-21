//! The time zone behind `DT_NOW()` on a box: what "after 22:00" means to
//! someone standing at the machine.
//!
//! No dependency reads it. The system's zone is the `TZif` file the OS
//! points `/etc/localtime` at (RFC 8536: the transitions, then a POSIX TZ
//! rule for the years after the last one); `--time-zone NAME` reads
//! `NAME` from the same database, or is `UTC`, which needs no files. The
//! offset is looked up for the instant of every tick
//! ([`Zone::wall_clock`]), so a daylight-saving change, a leap day and a
//! step of the system clock show in the next scan and a long-running box
//! never drifts from the clock it was given.
//!
//! What the program sees is local time as a person reads it, so it is not
//! monotonic: at the end of daylight saving `DT_NOW()` repeats an hour, and
//! at its start it skips one. A rule that must fire once an hour should
//! watch an edge, not a value.
//!
//! The file is the OS's own, but `--time-zone` is typed by a person, so
//! the name is checked before it is a path ([`valid_name`]) and the file
//! is checked before it is trusted (magic, sizes, order, offsets): a bad
//! one is an error naming the reason, never a guess and never a panic.

use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use helmsman_iec::dt::{civil_from_days, days_from_civil, days_in_month};

use crate::runtime::WallClock;

/// Where the OS keeps its zone database, in the order they are tried
/// (macOS keeps the live one under /var/db; the other is a link to it).
const DIRS: [&str; 4] = [
    "/usr/share/zoneinfo",
    "/var/db/timezone/zoneinfo",
    "/usr/lib/zoneinfo",
    "/usr/share/lib/zoneinfo",
];

/// The largest zone file read: the largest in the database is under 4 KB,
/// the fat ones 4 KB too.
const MAX_FILE: u64 = 64 * 1024;

/// The longest zone name taken (the longest in the database is 32).
const MAX_NAME: usize = 64;

/// The most an offset from UTC may be, either side, in seconds (RFC 8536
/// allows a little over 26 h; nobody lives there).
const MAX_OFFSET: i64 = 26 * 3600;

const NS: i64 = 1_000_000_000;

/// A time zone: the UTC offset at any instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    name: String,
    /// The offset before the first transition (and with no rule), seconds
    /// east of UTC.
    initial: i32,
    /// `(utc seconds, offset from then)`, strictly increasing.
    transitions: Vec<(i64, i32)>,
    /// What holds after the last transition.
    rule: Option<Rule>,
}

/// A POSIX TZ rule, as the `TZif` footer writes it (`CST6CDT,M3.2.0,M11.1.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    /// Seconds east of UTC (the TZ string counts west).
    std: i32,
    dst: Option<Dst>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Dst {
    off: i32,
    start: Change,
    end: Change,
}

/// When one change happens: a day of the year and the local seconds into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Change {
    day: Day,
    time: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Day {
    /// `Jn`: 1 to 365, 29 February never counted.
    Julian(u16),
    /// `n`: 0 to 365, 29 February counted.
    Zero(u16),
    /// `Mm.w.d`: day `d` (0 Sunday) of week `w` (5 is the last) of month `m`.
    Month(u8, u8, u8),
}

impl Zone {
    /// UTC.
    #[must_use]
    pub fn utc() -> Self {
        Self {
            name: "UTC".into(),
            initial: 0,
            transitions: Vec::new(),
            rule: None,
        }
    }

    /// The zone's name as it was asked for.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A zone from a POSIX TZ string such as `CST6CDT,M3.2.0,M11.1.0`.
    ///
    /// # Errors
    /// What is wrong with it.
    pub fn from_posix(name: &str, tz: &str) -> Result<Self, String> {
        let rule = Rule::parse(tz).ok_or_else(|| format!("{name}: not a POSIX time zone rule"))?;
        Ok(Self {
            name: name.into(),
            initial: rule.std,
            transitions: Vec::new(),
            rule: Some(rule),
        })
    }

    /// A zone from the bytes of a `TZif` file (RFC 8536, versions 1 to 4).
    ///
    /// # Errors
    /// What is wrong with the file.
    pub fn from_tzif(name: &str, bytes: &[u8]) -> Result<Self, String> {
        parse_tzif(name, bytes).map_err(|why| format!("{name}: not a time zone file ({why})"))
    }

    /// The zone `name` of the OS's database: `UTC`, or an IANA name such
    /// as `America/Chicago` read from the first of the usual directories
    /// that has it.
    ///
    /// # Errors
    /// A name that is not one ([`valid_name`]), a zone the OS does not
    /// have, a file that is not a zone file.
    pub fn named(name: &str) -> Result<Self, String> {
        if name == "UTC" {
            return Ok(Self::utc());
        }
        valid_name(name)?;
        let mut last = None;
        for dir in DIRS {
            match Self::named_in(Path::new(dir), name) {
                Err(e) if e.contains("no such zone") => last = Some(e),
                other => return other,
            }
        }
        Err(last.unwrap_or_else(|| format!("{name}: no such zone")))
    }

    /// [`Zone::named`], in database directory `dir`.
    ///
    /// # Errors
    /// As [`Zone::named`].
    pub fn named_in(dir: &Path, name: &str) -> Result<Self, String> {
        valid_name(name)?;
        let path = dir.join(name);
        let bytes = read_zone(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("{name}: no such zone in {}", dir.display()),
            _ => format!("{name}: {}: {e}", path.display()),
        })?;
        Self::from_tzif(name, &bytes)
    }

    /// The zone the system is set to: `/etc/localtime`, named by where it
    /// points (`.../zoneinfo/America/Chicago`) or, with a file that is no
    /// link, by `/etc/timezone` or "system".
    ///
    /// # Errors
    /// No such file, or not a zone file.
    pub fn system() -> Result<Self, String> {
        Self::system_at(Path::new("/etc/localtime"), Path::new("/etc/timezone"))
    }

    /// [`Zone::system`] with the two files given.
    ///
    /// # Errors
    /// As [`Zone::system`].
    pub fn system_at(localtime: &Path, timezone: &Path) -> Result<Self, String> {
        let name = fs::read_link(localtime)
            .ok()
            .and_then(|t| {
                let t = t.to_string_lossy().into_owned();
                t.split_once("zoneinfo/").map(|(_, n)| n.to_string())
            })
            .or_else(|| {
                let n = fs::read_to_string(timezone).ok()?;
                let n = n.trim();
                valid_name(n).is_ok().then(|| n.to_string())
            })
            .unwrap_or_else(|| "system".into());
        // A name out of a link is only a label, never a path.
        let name = if valid_name(&name).is_ok() {
            name
        } else {
            "system".into()
        };
        let bytes = read_zone(localtime).map_err(|e| format!("{}: {e}", localtime.display()))?;
        Self::from_tzif(&name, &bytes)
    }

    /// Seconds east of UTC at `utc` (seconds since the epoch, UTC).
    #[must_use]
    pub fn offset_at(&self, utc: i64) -> i32 {
        let Some(&(first, _)) = self.transitions.first() else {
            return self.rule.as_ref().map_or(self.initial, |r| r.offset_at(utc));
        };
        if utc < first {
            return self.initial;
        }
        let i = self.transitions.partition_point(|&(at, _)| at <= utc);
        let (last, off) = self.transitions[i - 1];
        match &self.rule {
            Some(r) if i == self.transitions.len() && utc >= last => r.offset_at(utc),
            _ => off,
        }
    }

    /// The date and time on a wall at `utc_ns` (nanoseconds since the
    /// epoch, UTC), as a `DT` holds it: nanoseconds since 1970-01-01T00:00
    /// local. Saturates at the edges of the `DT` range.
    #[must_use]
    pub fn local_ns(&self, utc_ns: i128) -> i64 {
        let ns = i128::from(NS);
        let secs =
            i64::try_from(utc_ns.div_euclid(ns)).unwrap_or(if utc_ns < 0 { i64::MIN } else { i64::MAX });
        let sub = utc_ns.rem_euclid(ns);
        let local = i128::from(secs) + i128::from(self.offset_at(secs));
        i64::try_from(local * ns + sub).unwrap_or(if local < 0 { i64::MIN } else { i64::MAX })
    }

    /// A wall clock for [`crate::Runtime::set_wall_clock`] that reads the
    /// system clock through this zone, every tick.
    #[must_use]
    pub fn wall_clock(self) -> WallClock {
        self.wall_clock_from(system_utc_ns)
    }

    /// [`Zone::wall_clock`] over any source of UTC nanoseconds: a test
    /// moves it by hand.
    #[must_use]
    pub fn wall_clock_from(self, utc_ns: impl Fn() -> i128 + Send + Sync + 'static) -> WallClock {
        WallClock::new(move || self.local_ns(utc_ns()))
    }

    /// `+01:00`, the offset at `utc`, for the journal.
    #[must_use]
    pub fn offset_text(&self, utc: i64) -> String {
        let off = self.offset_at(utc);
        let a = off.unsigned_abs();
        format!(
            "{}{:02}:{:02}",
            if off < 0 { '-' } else { '+' },
            a / 3600,
            a % 3600 / 60
        )
    }
}

/// The system clock, in nanoseconds since the epoch (before it: negative).
#[must_use]
pub fn system_utc_ns() -> i128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
        Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// Whether `name` is an IANA zone name to look up in a directory:
/// `Area/Place` of letters, digits and `_ + - .`, no empty part, none that
/// starts with a dot (so no `..`, no hidden file, no absolute path), at
/// most 64 characters.
///
/// # Errors
/// What is wrong with it.
pub fn valid_name(name: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("time zone {name:?}: {why}"));
    if name.is_empty() {
        return bad("empty");
    }
    if name.len() > MAX_NAME {
        return bad("longer than 64 characters");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-' | b'.'))
    {
        return bad("only letters, digits and / _ + - . are allowed");
    }
    if name.split('/').any(|p| p.is_empty() || p.starts_with('.')) {
        return bad("an empty part, or one that starts with a dot");
    }
    Ok(())
}

fn read_zone(path: &Path) -> std::io::Result<Vec<u8>> {
    let f = fs::File::open(path)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a file",
        ));
    }
    let mut bytes = Vec::new();
    f.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "larger than 64 KB",
        ));
    }
    Ok(bytes)
}

// ---- TZif ----

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: u64) -> Result<&'a [u8], &'static str> {
        let n = usize::try_from(n).map_err(|_| "truncated")?;
        if n > self.0.len() {
            return Err("truncated");
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn be64(b: &[u8]) -> i64 {
    i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn offset(s: i64) -> Result<i32, &'static str> {
    if s.abs() > MAX_OFFSET {
        return Err("an offset beyond 26 hours");
    }
    i32::try_from(s).map_err(|_| "an offset beyond 26 hours")
}

/// The six counts of a header.
struct Counts {
    isut: u64,
    isstd: u64,
    leap: u64,
    time: u64,
    typ: u64,
    chars: u64,
}

fn header(r: &mut Reader<'_>) -> Result<(u8, Counts), &'static str> {
    let h = r.take(44)?;
    if &h[..4] != b"TZif" {
        return Err("no TZif header");
    }
    let c = |i: usize| u64::from(be32(&h[20 + 4 * i..]));
    Ok((
        h[4],
        Counts {
            isut: c(0),
            isstd: c(1),
            leap: c(2),
            time: c(3),
            typ: c(4),
            chars: c(5),
        },
    ))
}

fn parse_tzif(name: &str, bytes: &[u8]) -> Result<Zone, &'static str> {
    let mut r = Reader(bytes);
    let (version, c) = header(&mut r)?;
    if !matches!(version, 0 | b'2' | b'3' | b'4') {
        return Err("an unknown version");
    }
    // Version 1 holds 32-bit times; later ones repeat the block with
    // 64-bit times, and that second block is the one read.
    let mut width = 4;
    let mut c = c;
    if version != 0 {
        r.take(c.time * 4 + c.time + c.typ * 6 + c.chars + c.leap * 8 + c.isstd + c.isut)?;
        let (_, c2) = header(&mut r)?;
        c = c2;
        width = 8;
    }
    if c.typ == 0 || c.typ > 256 || c.time > 200_000 {
        return Err("impossible counts");
    }
    let times = r.take(c.time * width)?;
    let idx = r.take(c.time)?;
    let types = r.take(c.typ * 6)?;
    r.take(c.chars)?;
    r.take(c.leap * (width + 4))?;
    r.take(c.isstd)?;
    r.take(c.isut)?;
    let offs = (0..usize::try_from(c.typ).map_err(|_| "impossible counts")?)
        .map(|k| offset(i64::from(be32(&types[k * 6..]).cast_signed())))
        .collect::<Result<Vec<i32>, _>>()?;
    let mut transitions: Vec<(i64, i32)> = Vec::new();
    for (k, &t) in idx.iter().enumerate() {
        let at = if width == 8 {
            be64(&times[k * 8..])
        } else {
            i64::from(be32(&times[k * 4..]).cast_signed())
        };
        let off = *offs
            .get(usize::from(t))
            .ok_or("a transition to a type that is not there")?;
        if transitions.last().is_some_and(|&(prev, _)| prev >= at) {
            return Err("transitions out of order");
        }
        transitions.push((at, off));
    }
    let rule = if version == 0 {
        None
    } else {
        let rest = r.0;
        let foot = rest.strip_prefix(b"\n").ok_or("no footer")?;
        let end = foot.iter().position(|&b| b == b'\n').ok_or("no footer")?;
        let text = std::str::from_utf8(&foot[..end]).map_err(|_| "a footer that is not text")?;
        if text.is_empty() {
            None
        } else {
            Some(Rule::parse(text).ok_or("a footer rule that does not parse")?)
        }
    };
    Ok(Zone {
        name: name.into(),
        initial: offs[0],
        transitions,
        rule,
    })
}

// ---- POSIX TZ rules ----

struct Cur<'a>(&'a str);

impl Cur<'_> {
    fn peek(&self) -> Option<char> {
        self.0.chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        let hit = self.peek() == Some(c);
        if hit {
            self.0 = &self.0[c.len_utf8()..];
        }
        hit
    }

    /// A zone abbreviation: three or more letters, or `<...>`.
    fn abbr(&mut self) -> Option<()> {
        if self.eat('<') {
            let end = self.0.find('>')?;
            let body = &self.0[..end];
            if body.len() < 3
                || !body
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-')
            {
                return None;
            }
            self.0 = &self.0[end + 1..];
            return Some(());
        }
        let n = self.0.bytes().take_while(u8::is_ascii_alphabetic).count();
        if n < 3 {
            return None;
        }
        self.0 = &self.0[n..];
        Some(())
    }

    fn num(&mut self, max_digits: usize) -> Option<i64> {
        let n = self.0.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > max_digits {
            return None;
        }
        let v = self.0[..n].parse().ok()?;
        self.0 = &self.0[n..];
        Some(v)
    }

    /// `[+-]hh[:mm[:ss]]` in seconds, hours up to 167 (as the `TZif` footer
    /// allows).
    fn hms(&mut self) -> Option<i64> {
        let sign = if self.eat('-') {
            -1
        } else {
            self.eat('+');
            1
        };
        let h = self.num(3)?;
        let (mut m, mut s) = (0, 0);
        if self.eat(':') {
            m = self.num(2)?;
            if self.eat(':') {
                s = self.num(2)?;
            }
        }
        (h <= 167 && m < 60 && s < 60).then_some(sign * (h * 3600 + m * 60 + s))
    }

    fn change(&mut self) -> Option<Change> {
        let day = if self.eat('M') {
            let m = self.num(2)?;
            self.eat('.').then_some(())?;
            let w = self.num(1)?;
            self.eat('.').then_some(())?;
            let d = self.num(1)?;
            if !((1..=12).contains(&m) && (1..=5).contains(&w) && d <= 6) {
                return None;
            }
            Day::Month(
                u8::try_from(m).ok()?,
                u8::try_from(w).ok()?,
                u8::try_from(d).ok()?,
            )
        } else if self.eat('J') {
            let n = self.num(3)?;
            if !(1..=365).contains(&n) {
                return None;
            }
            Day::Julian(u16::try_from(n).ok()?)
        } else {
            let n = self.num(3)?;
            if n > 365 {
                return None;
            }
            Day::Zero(u16::try_from(n).ok()?)
        };
        let time = if self.eat('/') { self.hms()? } else { 7200 };
        Some(Change {
            day,
            time: i32::try_from(time).ok()?,
        })
    }
}

impl Rule {
    fn parse(tz: &str) -> Option<Self> {
        let mut c = Cur(tz);
        c.abbr()?;
        // POSIX counts offsets west of UTC; this is east.
        let std = -c.hms()?;
        if c.0.is_empty() {
            return Some(Self {
                std: offset(std).ok()?,
                dst: None,
            });
        }
        c.abbr()?;
        let dst = if matches!(c.peek(), Some(',') | None) {
            std + 3600
        } else {
            -c.hms()?
        };
        // No rule after the name would mean the C library's default rule;
        // a database footer always has one, so it is refused here.
        c.eat(',').then_some(())?;
        let start = c.change()?;
        c.eat(',').then_some(())?;
        let end = c.change()?;
        c.0.is_empty().then_some(())?;
        Some(Self {
            std: offset(std).ok()?,
            dst: Some(Dst {
                off: offset(dst).ok()?,
                start,
                end,
            }),
        })
    }

    fn offset_at(&self, utc: i64) -> i32 {
        let Some(d) = &self.dst else { return self.std };
        let (std, dst) = (i64::from(self.std), i64::from(d.off));
        // The year is the one on the wall in standard time.
        let (year, _, _) = civil_from_days((utc + std).div_euclid(86_400));
        // The change into DST is read on the standard-time wall, the one
        // out of it on the DST wall.
        let on = d.start.day.seconds(year) + i64::from(d.start.time) - std;
        let off = d.end.day.seconds(year) + i64::from(d.end.time) - dst;
        let in_dst = if on < off {
            (on..off).contains(&utc)
        } else {
            !(off..on).contains(&utc)
        };
        if in_dst { d.off } else { self.std }
    }
}

impl Day {
    /// Seconds from the epoch to this day's midnight, in `year`.
    fn seconds(self, year: i64) -> i64 {
        let leap = days_in_month(year, 2) == 29;
        let jan1 = days_from_civil(year, 1, 1);
        let day = match self {
            Self::Julian(n) => {
                // 1 to 365 skipping 29 February: after it, days shift by one.
                let n = i64::from(n) - 1;
                jan1 + n + i64::from(leap && n >= 59)
            }
            Self::Zero(n) => jan1 + i64::from(n),
            Self::Month(m, w, d) => {
                let first = days_from_civil(year, u32::from(m), 1);
                // 1970-01-01 was a Thursday (4).
                let weekday = (first + 4).rem_euclid(7);
                let to_d = (i64::from(d) - weekday).rem_euclid(7);
                let nth = first + to_d + 7 * (i64::from(w) - 1);
                let len = i64::from(days_in_month(year, u32::from(m)));
                // Week 5 is the last one: back off if it fell past the month.
                if nth >= first + len { nth - 7 } else { nth }
            }
        };
        day * 86_400
    }
}

impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}
