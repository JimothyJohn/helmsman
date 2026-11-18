//! The box's wall clock: the time zone behind `DT_NOW()` (`helmsman_plc::zone`)
//! and the runtime handing it to every module it scans.
//!
//! Everything is an injected clock and a zone built in the test (a POSIX
//! rule or TZif bytes written here), so nothing sleeps and nothing depends
//! on the machine's zone; one test reads the machine's own database and
//! compares it with the rule.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use helmsman_iec::dt::days_from_civil;
use helmsman_iec::{ImageSizes, Value, compile};
use helmsman_plc::zone::{Zone, valid_name};
use helmsman_plc::{Module, Runtime, SignalTable, Window};

const SEC: i64 = 1_000_000_000;

/// Seconds since the epoch of a UTC civil time.
fn utc(y: i64, m: u32, d: u32, hh: i64, mm: i64, ss: i64) -> i64 {
    days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss
}

const CENTRAL: &str = "CST6CDT,M3.2.0,M11.1.0";

fn zone(rule: &str) -> Zone {
    Zone::from_posix("test", rule).unwrap_or_else(|e| panic!("{rule}: {e}"))
}

// ---- the rule ----

#[test]
fn central_time_changes_on_the_second_sunday_of_march_and_the_first_of_november() {
    let z = zone(CENTRAL);
    let h = 3600;
    // 2024-03-10 02:00 CST is 08:00 UTC.
    let spring = utc(2024, 3, 10, 8, 0, 0);
    assert_eq!(z.offset_at(spring - 1), -6 * h);
    assert_eq!(z.offset_at(spring), -5 * h);
    // 2024-11-03 02:00 CDT is 07:00 UTC.
    let fall = utc(2024, 11, 3, 7, 0, 0);
    assert_eq!(z.offset_at(fall - 1), -5 * h);
    assert_eq!(z.offset_at(fall), -6 * h);
    // The year before and the year after use their own Sundays.
    assert_eq!(z.offset_at(utc(2025, 3, 9, 8, 0, 0) - 1), -6 * h);
    assert_eq!(z.offset_at(utc(2025, 3, 9, 8, 0, 0)), -5 * h);
    assert_eq!(z.offset_at(utc(2023, 3, 12, 8, 0, 0)), -5 * h);
    assert_eq!(z.offset_at(utc(2024, 1, 1, 0, 0, 0)), -6 * h);
    assert_eq!(z.offset_at(utc(2024, 7, 1, 0, 0, 0)), -5 * h);
}

#[test]
fn a_southern_zone_is_on_daylight_time_over_new_year() {
    // Sydney: from the first Sunday of October 02:00 to the first Sunday of
    // April 03:00 (on the daylight clock).
    let z = zone("AEST-10AEDT,M10.1.0,M4.1.0/3");
    let h = 3600;
    assert_eq!(z.offset_at(utc(2024, 1, 1, 0, 0, 0)), 11 * h);
    assert_eq!(z.offset_at(utc(2024, 7, 1, 0, 0, 0)), 10 * h);
    // 2024-04-07 03:00 AEDT is 2024-04-06 16:00 UTC.
    assert_eq!(z.offset_at(utc(2024, 4, 6, 16, 0, 0) - 1), 11 * h);
    assert_eq!(z.offset_at(utc(2024, 4, 6, 16, 0, 0)), 10 * h);
    // 2024-10-06 02:00 AEST is 2024-10-05 16:00 UTC.
    assert_eq!(z.offset_at(utc(2024, 10, 5, 16, 0, 0) - 1), 10 * h);
    assert_eq!(z.offset_at(utc(2024, 10, 5, 16, 0, 0)), 11 * h);
}

#[test]
fn week_five_is_the_last_one_of_the_month() {
    // Europe: the last Sunday of March 01:00 UTC; of October 01:00 UTC.
    let z = zone("CET-1CEST,M3.5.0,M10.5.0/3");
    assert_eq!(z.offset_at(utc(2024, 3, 31, 1, 0, 0) - 1), 3600);
    assert_eq!(z.offset_at(utc(2024, 3, 31, 1, 0, 0)), 7200);
    assert_eq!(z.offset_at(utc(2024, 10, 27, 1, 0, 0) - 1), 7200);
    assert_eq!(z.offset_at(utc(2024, 10, 27, 1, 0, 0)), 3600);
    // 2026: March has 31 days and its last Sunday is the 29th; October's the 25th.
    assert_eq!(z.offset_at(utc(2026, 3, 29, 1, 0, 0)), 7200);
    assert_eq!(z.offset_at(utc(2026, 3, 29, 1, 0, 0) - 1), 3600);
    assert_eq!(z.offset_at(utc(2026, 10, 25, 1, 0, 0)), 3600);
}

#[test]
fn fixed_offsets_in_odd_units_and_days_counted_from_the_year_start() {
    assert_eq!(zone("<+0530>-5:30").offset_at(0), 5 * 3600 + 1800);
    assert_eq!(zone("<-03>3").offset_at(1_700_000_000), -3 * 3600);
    assert_eq!(zone("UTC0").offset_at(-5_000_000_000), 0);
    // `J60` is 1 March whatever the year; `59` is the 60th day, 29 February
    // in a leap year and 1 March in a common one.
    let j = zone("XXX0YYY,J60,J300");
    assert_eq!(j.offset_at(utc(2024, 2, 29, 12, 0, 0)), 0);
    assert_eq!(j.offset_at(utc(2024, 3, 1, 12, 0, 0)), 3600);
    assert_eq!(j.offset_at(utc(2023, 3, 1, 12, 0, 0)), 3600);
    let n = zone("XXX0YYY,59,300");
    assert_eq!(n.offset_at(utc(2024, 2, 29, 12, 0, 0)), 3600);
    assert_eq!(n.offset_at(utc(2023, 2, 28, 12, 0, 0)), 0);
    assert_eq!(n.offset_at(utc(2023, 3, 1, 12, 0, 0)), 3600);
}

#[test]
fn rules_that_are_not_rules_are_refused_without_a_panic() {
    for bad in [
        "",
        "CST",
        "CST6CDT",
        "CST6CDT,M3.2.0",
        "CST6CDT,M13.2.0,M11.1.0",
        "CST6CDT,M3.6.0,M11.1.0",
        "CST6CDT,M3.2.7,M11.1.0",
        "CST6CDT,J0,J10",
        "CST6CDT,J366,J10",
        "CST6CDT,367,10",
        "CST6CDT,M3.2.0/200,M11.1.0",
        "CST6CDT,M3.2.0,M11.1.0,extra",
        "CST99999",
        "CST60:61",
        "CST27",
        "<>5",
        "<a>5",
        "AB5",
        "CST6CDT,M3.2.0/-,M11.1.0",
        "\u{0}",
        "CST6\u{1f600}",
        &"A".repeat(10_000),
        &format!("CST6CDT,M3.2.0,{}", "9".repeat(10_000)),
    ] {
        assert!(Zone::from_posix("bad", bad).is_err(), "{bad:?} was taken");
    }
}

// ---- TZif ----

/// A version-2 TZif file: `types` as (offset, is_dst), `trans` as (utc
/// seconds, type), and a POSIX footer.
fn tzif(types: &[(i32, bool)], trans: &[(i64, u8)], footer: &str) -> Vec<u8> {
    let block = |wide: bool| {
        let mut b = Vec::new();
        for (at, _) in trans {
            if wide {
                b.extend(at.to_be_bytes());
            } else {
                b.extend(i32::try_from(*at).unwrap_or(0).to_be_bytes());
            }
        }
        b.extend(trans.iter().map(|t| t.1));
        for (off, dst) in types {
            b.extend(off.to_be_bytes());
            b.push(u8::from(*dst));
            b.push(0);
        }
        b.extend(b"UTC\0");
        b
    };
    let header = |v: u8| {
        let mut h = b"TZif".to_vec();
        h.push(v);
        h.extend([0; 15]);
        for n in [0, 0, 0, trans.len(), types.len(), 4] {
            h.extend(u32::try_from(n).unwrap().to_be_bytes());
        }
        h
    };
    let mut f = header(b'2');
    f.extend(block(false));
    f.extend(header(b'2'));
    f.extend(block(true));
    f.extend(format!("\n{footer}\n").bytes());
    f
}

fn central_slim() -> Vec<u8> {
    // The last transition in a slim file is a DST change long past; the
    // footer carries the rule from there.
    tzif(
        &[(-21_600, false), (-18_000, true)],
        &[(utc(2007, 3, 11, 8, 0, 0), 1), (utc(2007, 11, 4, 7, 0, 0), 0)],
        CENTRAL,
    )
}

#[test]
fn a_slim_file_reads_its_footer_after_the_last_transition() {
    let z = Zone::from_tzif("Test/Central", &central_slim()).unwrap();
    let want = zone(CENTRAL);
    for t in [
        utc(2007, 3, 11, 8, 0, 0) - 1,
        utc(2007, 3, 11, 8, 0, 0),
        utc(2007, 11, 4, 7, 0, 0) - 1,
        utc(2007, 11, 4, 7, 0, 0),
        utc(2024, 3, 10, 8, 0, 0) - 1,
        utc(2024, 3, 10, 8, 0, 0),
        utc(2024, 11, 3, 7, 0, 0),
        utc(2040, 7, 4, 12, 0, 0),
    ] {
        assert_eq!(z.offset_at(t), want.offset_at(t), "at {t}");
    }
    // Before the first transition the first type holds.
    assert_eq!(z.offset_at(utc(1990, 7, 4, 12, 0, 0)), -21_600);
    assert_eq!(z.name(), "Test/Central");
}

#[test]
fn transitions_are_used_where_the_file_lists_them_even_against_its_footer() {
    // The government changed its mind for 2024: no summer time that year.
    let off = utc(2024, 1, 1, 0, 0, 0);
    let on = utc(2025, 1, 1, 0, 0, 0);
    let f = tzif(&[(-21_600, false)], &[(off, 0), (on, 0)], CENTRAL);
    let z = Zone::from_tzif("Test/Odd", &f).unwrap();
    assert_eq!(z.offset_at(utc(2024, 7, 1, 0, 0, 0)), -21_600);
    assert_eq!(z.offset_at(utc(2025, 7, 1, 0, 0, 0)), -18_000);
}

#[test]
fn a_file_with_no_transitions_and_no_footer_is_one_offset() {
    let f = tzif(&[(3600, false)], &[], "");
    let z = Zone::from_tzif("Test/Fixed", &f).unwrap();
    assert_eq!(z.offset_at(0), 3600);
    assert_eq!(z.offset_at(i64::MAX), 3600);
    assert_eq!(z.offset_at(i64::MIN), 3600);
}

#[test]
fn a_damaged_zone_file_is_an_error_and_never_a_panic() {
    let good = central_slim();
    // Every prefix.
    for n in 0..good.len() {
        assert!(
            Zone::from_tzif("cut", &good[..n]).is_err(),
            "prefix of {n} bytes was taken"
        );
    }
    // Every byte changed to each of a few values: whatever it says, no panic.
    for k in 0..good.len() {
        for v in [0u8, 0xff, 0x7f, b'\n', b'Z'] {
            let mut b = good.clone();
            b[k] = v;
            let _ = Zone::from_tzif("flip", &b);
        }
    }
    // The checks that matter, named.
    let mut magic = good.clone();
    magic[0] = b'X';
    assert!(Zone::from_tzif("m", &magic).unwrap_err().contains("TZif"));
    let mut version = good.clone();
    version[4] = b'9';
    assert!(Zone::from_tzif("v", &version).is_err());
    // Counts that promise more than the file has.
    let mut huge = b"TZif2".to_vec();
    huge.extend([0; 15]);
    for _ in 0..6 {
        huge.extend(u32::MAX.to_be_bytes());
    }
    assert!(Zone::from_tzif("h", &huge).is_err());
    // No types.
    assert!(Zone::from_tzif("t", &tzif(&[], &[], "")).is_err());
    // Transitions out of order, and to a type that is not there.
    let order = tzif(&[(0, false)], &[(100, 0), (50, 0)], "");
    assert!(Zone::from_tzif("o", &order).unwrap_err().contains("order"));
    assert!(Zone::from_tzif("o", &tzif(&[(0, false)], &[(100, 0), (100, 0)], "")).is_err());
    assert!(Zone::from_tzif("o", &tzif(&[(0, false)], &[(100, 3)], "")).is_err());
    // An offset nobody lives at.
    assert!(Zone::from_tzif("x", &tzif(&[(26 * 3600 + 1, false)], &[], "")).is_err());
    assert!(Zone::from_tzif("x", &tzif(&[(i32::MIN, false)], &[], "")).is_err());
    // A footer that is no rule.
    assert!(Zone::from_tzif("f", &tzif(&[(0, false)], &[], "not a rule")).is_err());
    // Text that is no zone file at all.
    assert!(Zone::from_tzif("p", b"root:x:0:0:root:/root:/bin/sh\n").is_err());
    assert!(Zone::from_tzif("p", b"").is_err());
}

// ---- names ----

#[test]
fn a_zone_name_that_could_be_a_path_is_refused() {
    for bad in [
        "",
        ".",
        "..",
        "../etc/passwd",
        "America/../../etc/passwd",
        "/etc/passwd",
        "/usr/share/zoneinfo/UTC",
        "America//Chicago",
        "America/",
        "America/.hidden",
        ".hidden",
        "America\\Chicago",
        "America/Chicago\n",
        "America/Chicago\0",
        "Amér/Chicago",
        "America/Chi cago",
        "~root",
        "$HOME",
        "a;b",
        "C:\\Windows",
        &"A".repeat(65),
    ] {
        assert!(valid_name(bad).is_err(), "{bad:?} was taken");
        assert!(Zone::named(bad).is_err(), "{bad:?} was looked up");
    }
    for good in [
        "UTC",
        "America/Chicago",
        "America/Argentina/Buenos_Aires",
        "Etc/GMT+5",
        "America/Port-au-Prince",
        "Asia/Kolkata",
    ] {
        assert!(valid_name(good).is_ok(), "{good:?} was refused");
    }
    assert_eq!(Zone::named("UTC").unwrap().offset_at(1_700_000_000), 0);
}

/// A directory of the test's own, for zone files.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("helmsman-zone-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_named_zone_is_read_from_the_database_and_checked() {
    let d = Dir::new("named");
    std::fs::create_dir_all(d.0.join("America")).unwrap();
    std::fs::write(d.0.join("America/Central"), central_slim()).unwrap();
    std::fs::write(d.0.join("Passwd"), b"root:x:0:0:root:/root:/bin/sh\n").unwrap();
    std::fs::write(d.0.join("Big"), vec![b'T'; 70_000]).unwrap();
    let z = Zone::named_in(&d.0, "America/Central").unwrap();
    assert_eq!(z.offset_at(utc(2024, 7, 1, 0, 0, 0)), -18_000);
    // Missing, not a zone file, a directory, too large: each an error that says so.
    assert!(
        Zone::named_in(&d.0, "America/Nowhere")
            .unwrap_err()
            .contains("no such zone")
    );
    assert!(
        Zone::named_in(&d.0, "Passwd")
            .unwrap_err()
            .contains("not a time zone file")
    );
    assert!(Zone::named_in(&d.0, "America").is_err());
    assert!(Zone::named_in(&d.0, "Big").unwrap_err().contains("64 KB"));
    // The path is the name's alone: a name cannot leave the directory.
    assert!(Zone::named_in(&d.0, "../Passwd").is_err());
}

#[test]
fn the_system_zone_is_the_file_localtime_points_at() {
    let d = Dir::new("system");
    std::fs::create_dir_all(d.0.join("zoneinfo/America")).unwrap();
    std::fs::write(d.0.join("zoneinfo/America/Central"), central_slim()).unwrap();
    std::os::unix::fs::symlink(d.0.join("zoneinfo/America/Central"), d.0.join("localtime")).unwrap();
    let z = Zone::system_at(&d.0.join("localtime"), &d.0.join("timezone")).unwrap();
    assert_eq!(z.name(), "America/Central");
    assert_eq!(z.offset_at(utc(2024, 7, 1, 0, 0, 0)), -18_000);
    // Neither file: an error for the caller to turn into UTC.
    assert!(Zone::system_at(&d.0.join("none"), &d.0.join("none2")).is_err());
    // A link that is nothing like a zone file.
    std::fs::write(d.0.join("junk"), b"x").unwrap();
    assert!(Zone::system_at(&d.0.join("junk"), &d.0.join("none2")).is_err());
}

#[test]
fn the_machines_own_database_agrees_with_the_rule() {
    // Where the OS has the database (a CI image without tzdata does not).
    let Ok(z) = Zone::named("America/Chicago") else {
        eprintln!("no zone database on this machine: America/Chicago not compared");
        return;
    };
    let rule = zone(CENTRAL);
    for year in 2015..2040 {
        for (m, d) in [
            (1, 15),
            (3, 1),
            (3, 9),
            (3, 10),
            (3, 11),
            (3, 12),
            (4, 1),
            (7, 1),
            (10, 31),
            (11, 2),
            (11, 3),
            (11, 4),
            (12, 31),
        ] {
            for h in [0, 1, 6, 7, 8, 9, 12, 23] {
                let t = utc(year, m, d, h, 0, 0);
                assert_eq!(z.offset_at(t), rule.offset_at(t), "{year}-{m}-{d} {h}:00 UTC");
            }
        }
    }
}

// ---- local time as a DT ----

#[test]
fn local_time_is_the_utc_instant_plus_the_offset_in_force_then() {
    let z = zone(CENTRAL);
    let t = utc(2024, 7, 1, 12, 30, 15);
    let local = z.local_ns(i128::from(t) * i128::from(SEC) + 250);
    assert_eq!(local, (t - 5 * 3600) * SEC + 250);
    // Before the epoch, and at the edges of the range.
    assert_eq!(Zone::utc().local_ns(-1), -1);
    assert_eq!(Zone::utc().local_ns(i128::MAX), i64::MAX);
    assert_eq!(Zone::utc().local_ns(i128::MIN), i64::MIN);
    assert_eq!(z.offset_text(utc(2024, 7, 1, 0, 0, 0)), "-05:00");
    assert_eq!(zone("<+0530>-5:30").offset_text(0), "+05:30");
}

// ---- the runtime ----

const NOW: &str = "
PROGRAM clock
VAR x : DT; END_VAR
  x := DT_NOW();
END_PROGRAM";

const NOW_TOD: &str = "
PROGRAM clock
VAR x : DT; late : BOOL; END_VAR
  x := DT_NOW();
  late := DT_TO_TOD(x) >= TOD#22:00:00;
END_PROGRAM";

fn module(name: &str, src: &str) -> Module {
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap_or_else(|d| panic!("{d:?}"));
    Module {
        name: name.into(),
        program: program.into(),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

fn dt(rt: &Runtime, module: &str) -> i64 {
    match rt.program(module).and_then(|p| p.get("X")) {
        Some(Value::Int(n)) => i64::try_from(n).unwrap(),
        other => panic!("{other:?}"),
    }
}

/// A box with one module on a system clock the test moves, in `zone`.
struct Bench {
    rt: Runtime,
    table: SignalTable,
    /// The injected system clock, UTC nanoseconds.
    utc: Arc<AtomicI64>,
    /// The scan clock, which only ever counts forward by one period.
    scan: Duration,
}

impl Bench {
    fn new(zone: Zone, src: &str, at: i64) -> Self {
        let utc = Arc::new(AtomicI64::new(at * SEC));
        let mut rt = Runtime::new(0, 0, vec![module("clock", src)]).unwrap();
        let source = Arc::clone(&utc);
        rt.set_wall_clock(zone.wall_clock_from(move || i128::from(source.load(Ordering::SeqCst))));
        Self {
            rt,
            table: SignalTable::default(),
            utc,
            scan: Duration::ZERO,
        }
    }

    /// The system clock is at `t` (UTC seconds); one scan runs.
    fn at(&mut self, t: i64) -> i64 {
        self.utc.store(t * SEC, Ordering::SeqCst);
        assert!(self.rt.tick_with(self.scan, &[], &mut self.table).is_empty());
        self.scan += Duration::from_millis(10);
        dt(&self.rt, "clock")
    }
}

#[test]
fn without_a_wall_clock_dt_now_counts_from_1970() {
    let mut rt = Runtime::new(0, 0, vec![module("clock", NOW)]).unwrap();
    rt.tick(Duration::from_secs(90), &[]);
    assert_eq!(dt(&rt, "clock"), 90 * SEC);
}

#[test]
fn dt_now_is_the_system_clock_through_the_zone_at_every_scan() {
    let mut b = Bench::new(zone(CENTRAL), NOW, utc(2024, 7, 1, 17, 0, 0));
    // 12:00 CDT, to the nanosecond of the scan clock's own resolution.
    assert_eq!(b.at(utc(2024, 7, 1, 17, 0, 0)), utc(2024, 7, 1, 12, 0, 0) * SEC);
    // The system clock ran an hour while the scan clock ran 10 ms: a box
    // that anchored once at start would say 12:00:00.01; this one follows.
    assert_eq!(b.at(utc(2024, 7, 1, 18, 0, 0)), utc(2024, 7, 1, 13, 0, 0) * SEC);
    // And a step backwards (an NTP correction) is followed too.
    assert_eq!(b.at(utc(2024, 7, 1, 17, 59, 0)), utc(2024, 7, 1, 12, 59, 0) * SEC);
}

#[test]
fn a_day_boundary_and_a_leap_day_pass_in_local_time() {
    // In UTC: the leap day arrives at the system clock's midnight.
    let mut b = Bench::new(Zone::utc(), NOW_TOD, utc(2024, 2, 28, 23, 59, 58));
    let day = |y, m, d, h, mi, s| utc(y, m, d, h, mi, s) * SEC;
    assert_eq!(b.at(utc(2024, 2, 28, 23, 59, 59)), day(2024, 2, 28, 23, 59, 59));
    assert_eq!(b.at(utc(2024, 2, 29, 0, 0, 0)), day(2024, 2, 29, 0, 0, 0));
    assert_eq!(b.at(utc(2024, 2, 29, 23, 59, 59)), day(2024, 2, 29, 23, 59, 59));
    assert_eq!(b.at(utc(2024, 3, 1, 0, 0, 0)), day(2024, 3, 1, 0, 0, 0));
    // In Chicago, local midnight is 06:00 UTC in winter, and "after 22:00"
    // is the local evening and not the UTC one.
    let mut c = Bench::new(zone(CENTRAL), NOW_TOD, utc(2024, 1, 10, 3, 59, 59));
    c.at(utc(2024, 1, 10, 3, 59, 59));
    let late = |b: &Bench| b.rt.program("clock").unwrap().get("LATE");
    assert_eq!(late(&c), Some(Value::Bool(false)), "21:59:59 local");
    c.at(utc(2024, 1, 10, 4, 0, 0));
    assert_eq!(late(&c), Some(Value::Bool(true)), "22:00:00 local");
    assert_eq!(c.at(utc(2024, 1, 10, 6, 0, 0)), utc(2024, 1, 10, 0, 0, 0) * SEC);
    assert_eq!(late(&c), Some(Value::Bool(false)), "midnight local");
}

#[test]
fn clocks_spring_forward_across_the_missing_hour() {
    let mut b = Bench::new(zone(CENTRAL), NOW, utc(2024, 3, 10, 7, 59, 58));
    let local = |h, m, s| utc(2024, 3, 10, h, m, s) * SEC;
    assert_eq!(b.at(utc(2024, 3, 10, 7, 59, 58)), local(1, 59, 58));
    assert_eq!(b.at(utc(2024, 3, 10, 7, 59, 59)), local(1, 59, 59));
    // The next second is 03:00 CDT: 02:xx never exists.
    assert_eq!(b.at(utc(2024, 3, 10, 8, 0, 0)), local(3, 0, 0));
    assert_eq!(b.at(utc(2024, 3, 10, 8, 0, 1)), local(3, 0, 1));
}

#[test]
fn clocks_fall_back_and_repeat_an_hour() {
    let mut b = Bench::new(zone(CENTRAL), NOW, utc(2024, 11, 3, 6, 59, 58));
    let local = |h, m, s| utc(2024, 11, 3, h, m, s) * SEC;
    assert_eq!(b.at(utc(2024, 11, 3, 6, 59, 59)), local(1, 59, 59));
    // 01:00 CST: the hour from 01:00 to 02:00 has gone by once already.
    assert_eq!(b.at(utc(2024, 11, 3, 7, 0, 0)), local(1, 0, 0));
    assert_eq!(b.at(utc(2024, 11, 3, 7, 59, 59)), local(1, 59, 59));
    assert_eq!(b.at(utc(2024, 11, 3, 8, 0, 0)), local(2, 0, 0));
}

#[test]
fn a_southern_zone_changes_in_the_other_direction_on_the_other_dates() {
    let mut b = Bench::new(
        zone("AEST-10AEDT,M10.1.0,M4.1.0/3"),
        NOW,
        utc(2024, 4, 6, 15, 59, 59),
    );
    // 2024-04-07 02:59:59 AEDT, then the clock goes back to 02:00:00 AEST.
    assert_eq!(
        b.at(utc(2024, 4, 6, 15, 59, 59)),
        utc(2024, 4, 7, 2, 59, 59) * SEC
    );
    assert_eq!(b.at(utc(2024, 4, 6, 16, 0, 0)), utc(2024, 4, 7, 2, 0, 0) * SEC);
    // 2024-10-06 01:59:59 AEST, then 03:00:00 AEDT.
    assert_eq!(
        b.at(utc(2024, 10, 5, 15, 59, 59)),
        utc(2024, 10, 6, 1, 59, 59) * SEC
    );
    assert_eq!(b.at(utc(2024, 10, 5, 16, 0, 0)), utc(2024, 10, 6, 3, 0, 0) * SEC);
}

#[test]
fn every_way_a_program_gets_installed_reads_the_same_clock() {
    let t = utc(2024, 7, 1, 17, 0, 0);
    let want = utc(2024, 7, 1, 12, 0, 0) * SEC;
    let mut b = Bench::new(zone(CENTRAL), NOW, t);
    assert_eq!(b.at(t), want);
    // A hot swap of the module: the new program's first scan.
    b.rt.swap(module("clock", NOW), &mut b.table).unwrap();
    assert_eq!(b.at(t), want, "after swap");
    // Reconfigure with a new program, and rebind.
    b.rt.reconfigure(vec![module("clock", NOW_TOD)], &mut b.table)
        .unwrap();
    assert_eq!(b.at(t), want, "after reconfigure");
    b.rt.rebind(vec![module("clock", NOW)], &mut b.table).unwrap();
    assert_eq!(b.at(t), want, "after rebind");
    // Stop, then a cold restart.
    b.rt.stop("clock", &mut b.table).unwrap();
    b.rt.restart("clock").unwrap();
    assert_eq!(b.at(t), want, "after restart");
}

// ---- the command line ----

fn plc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn a_hostile_time_zone_on_the_command_line_stops_the_box_with_the_reason() {
    for bad in [
        "../../etc/passwd",
        "/etc/passwd",
        "",
        "America/Chicago\n",
        "a b",
        "America/NoSuchPlace",
        "Atlantis",
        "..",
    ] {
        let out = plc(&[
            "--time-zone",
            bad,
            "--api",
            "off",
            "--opcua",
            "off",
            "no-such-program.st",
        ]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{bad:?} started a box: {err}");
        assert!(err.contains("--time-zone"), "{bad:?}: {err}");
    }
    let twice = plc(&["--time-zone", "UTC", "--time-zone", "UTC"]);
    assert!(!twice.status.success());
    assert!(String::from_utf8_lossy(&twice.stderr).contains("twice"));
    assert!(!plc(&["--time-zone"]).status.success());
}
