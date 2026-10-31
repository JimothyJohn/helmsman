//! The shift calendar's contract: the file format, the rules it refuses, the
//! state at any instant (a pure function of wall-clock time), and the
//! calendar as a signal source and output driver in the runtime.

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_iec::{ImageSizes, Value as St, compile};
use helmsman_io::{Binding, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::calendar::{
    AttachError, CLOCK_FLOOR, CalError, Calendar, Date, SIGNALS, Source, State, load, parse, render, utc,
};
use helmsman_plc::{ConfigError, Module, Runtime, SignalTable, Status, TableError, Window};
use proptest::prelude::*;

fn cal(src: &str) -> Calendar {
    load(src).unwrap_or_else(|e| panic!("{e:?}"))
}

fn errors(src: &str) -> Vec<CalError> {
    parse(src)
        .expect("parses")
        .validate()
        .expect_err("should be refused")
}

/// In shift `id`, `minute` minutes in, not on a break, not down.
fn working(id: i64, minute: i64, left: i64) -> State {
    State {
        in_shift: true,
        on_break: false,
        planned_down: false,
        working: true,
        shift_id: id,
        shift_minute: minute,
        shift_left: left,
    }
}

const OFF: State = State {
    in_shift: false,
    on_break: false,
    planned_down: false,
    working: false,
    shift_id: 0,
    shift_minute: 0,
    shift_left: 0,
};

const PLANT: &str = "
# Plant 2
zone UTC
shift 1 mon-fri 06:00-14:00
shift 2 mon-fri 14:00-22:00
shift 3 sun-thu 22:00-06:00   # crosses midnight
break 1 10:00-10:30
break 3 02:00-02:30
down mon-fri 13:45-14:00
";

// ---- the empty calendar ----------------------------------------------------

#[test]
fn an_empty_calendar_is_never_in_shift() {
    for src in ["", "\n# nothing\n", "zone +01:00"] {
        let c = cal(src);
        for t in [i64::MIN, -1, 0, CLOCK_FLOOR, utc(2026, 11, 2, 10, 0), i64::MAX] {
            assert_eq!(c.at(t), OFF, "{src:?} at {t}");
        }
    }
}

// ---- weekly shifts ---------------------------------------------------------

#[test]
fn a_shift_holds_from_its_start_minute_up_to_but_not_its_end() {
    let c = cal(PLANT);
    // Monday 2026-11-02.
    assert_eq!(c.at(utc(2026, 11, 2, 5, 59) + 59), working(3, 479, 1));
    assert_eq!(c.at(utc(2026, 11, 2, 6, 0)), working(1, 0, 480));
    assert_eq!(c.at(utc(2026, 11, 2, 6, 0) + 1).shift_left, 480, "rounded up");
    assert_eq!(c.at(utc(2026, 11, 2, 9, 0)), working(1, 180, 300));
    assert_eq!(c.at(utc(2026, 11, 2, 13, 59) + 59).shift_id, 1);
    assert_eq!(c.at(utc(2026, 11, 2, 14, 0)), working(2, 0, 480));
}

#[test]
fn a_shift_that_crosses_midnight_belongs_to_the_day_it_starts() {
    let c = cal(PLANT);
    // Sunday 2026-11-01 22:00 starts shift 3 (sun-thu).
    assert_eq!(c.at(utc(2026, 11, 1, 21, 59)), OFF);
    assert_eq!(c.at(utc(2026, 11, 1, 22, 0)), working(3, 0, 480));
    assert_eq!(c.at(utc(2026, 11, 1, 23, 59)), working(3, 119, 361));
    assert_eq!(c.at(utc(2026, 11, 2, 0, 0)), working(3, 120, 360));
    assert_eq!(c.at(utc(2026, 11, 2, 5, 0)), working(3, 420, 60));
    // Thursday night runs into Friday morning; Friday night does not start one.
    assert_eq!(c.at(utc(2026, 11, 6, 3, 0)).shift_id, 3);
    assert_eq!(c.at(utc(2026, 11, 6, 23, 0)), OFF);
    assert_eq!(c.at(utc(2026, 11, 7, 3, 0)), OFF, "Saturday morning");
    // Saturday night does not start one either; Sunday's does.
    assert_eq!(c.at(utc(2026, 11, 8, 3, 0)), OFF);
}

#[test]
fn a_shift_crosses_a_year_boundary() {
    let c = cal("zone UTC\nshift 3 daily 22:00-06:00");
    assert_eq!(c.at(utc(2026, 12, 31, 23, 30)), working(3, 90, 390));
    assert_eq!(c.at(utc(2027, 1, 1, 5, 30)), working(3, 450, 30));
}

#[test]
fn a_whole_day_shift_is_midnight_to_midnight() {
    let c = cal("zone UTC\nshift 7 sat 00:00-24:00");
    assert_eq!(c.at(utc(2026, 11, 7, 0, 0)), working(7, 0, 1440));
    assert_eq!(c.at(utc(2026, 11, 7, 23, 59)), working(7, 1439, 1));
    assert_eq!(c.at(utc(2026, 11, 8, 0, 0)), OFF);
}

// ---- overlap ---------------------------------------------------------------

#[test]
fn overlapping_weekly_shifts_are_refused_and_touching_ones_are_not() {
    assert!(matches!(
        errors("zone UTC\nshift 1 mon 06:00-14:00\nshift 2 mon 13:00-22:00")[..],
        [CalError::Overlap { a: 1, b: 2, .. }]
    ));
    // Shift 3 Sunday night runs into Monday 06:00; a Monday 05:00 start overlaps.
    assert!(matches!(
        errors("zone UTC\nshift 3 sun 22:00-06:00\nshift 1 mon 05:00-13:00")[..],
        [CalError::Overlap { .. }]
    ));
    // The same shift twice on one day.
    assert!(matches!(
        errors("zone UTC\nshift 1 mon-fri 06:00-14:00\nshift 1 fri 10:00-12:00")[..],
        [CalError::Overlap { a: 1, b: 1, .. }]
    ));
    cal(
        "zone UTC\nshift 1 mon 06:00-14:00\nshift 2 mon 14:00-22:00\nshift 3 mon 22:00-06:00\nshift 1 tue 06:00-14:00",
    );
}

#[test]
fn a_dated_shift_that_overlaps_another_is_refused() {
    let base = "zone UTC\nshift 1 mon-fri 06:00-14:00\nshift 2 mon-fri 14:00-22:00\n";
    // Shift 1 on Friday until 16:00 runs into shift 2.
    assert!(matches!(
        errors(&format!("{base}shift 1 2026-11-06 06:00-16:00"))[..],
        [CalError::Overlap { a: 1, b: 2, .. }]
    ));
    // A dated night shift on Sunday running into Monday's shift 1.
    assert!(matches!(
        errors(&format!("{base}shift 9 2026-11-01 22:00-07:00"))[..],
        [CalError::Overlap { .. }]
    ));
    // ... unless Monday is a holiday.
    cal(&format!(
        "{base}shift 9 2026-11-01 22:00-07:00\nholiday 2026-11-02"
    ));
}

// ---- breaks ----------------------------------------------------------------

#[test]
fn a_break_is_inside_its_shift_and_stops_work() {
    let c = cal(PLANT);
    let s = c.at(utc(2026, 11, 2, 10, 15));
    assert!(s.in_shift && s.on_break && !s.working, "{s:?}");
    assert_eq!((s.shift_id, s.shift_minute), (1, 255));
    assert!(
        !c.at(utc(2026, 11, 2, 10, 30)).on_break,
        "end minute is not in it"
    );
    // Shift 3's break is after midnight, in the next day.
    assert!(c.at(utc(2026, 11, 3, 2, 10)).on_break);
    assert!(!c.at(utc(2026, 11, 3, 2, 30)).on_break);
    // Shift 2 has no break at 10:00 or 02:00.
    assert!(!c.at(utc(2026, 11, 2, 18, 0)).on_break);
}

#[test]
fn a_break_outside_its_shift_is_refused() {
    assert_eq!(
        errors("zone UTC\nshift 1 mon-fri 06:00-14:00\nbreak 1 15:00-15:30"),
        [CalError::BreakOutside(1)]
    );
    // Straddling the end.
    assert_eq!(
        errors("zone UTC\nshift 1 mon-fri 06:00-14:00\nbreak 1 13:50-14:10"),
        [CalError::BreakOutside(1)]
    );
    // Straddling the start of a night shift.
    assert_eq!(
        errors("zone UTC\nshift 3 daily 22:00-06:00\nbreak 3 21:50-22:10"),
        [CalError::BreakOutside(3)]
    );
    assert_eq!(
        errors("zone UTC\nbreak 4 10:00-10:30"),
        [CalError::NoSuchShift(4)]
    );
}

#[test]
fn a_break_applies_only_to_occurrences_that_hold_it() {
    // A short Saturday shift 1 does not reach the 10:00 break.
    let c = cal("zone UTC\nshift 1 mon-fri 06:00-14:00\nshift 1 sat 06:00-09:00\nbreak 1 10:00-10:30");
    assert!(c.at(utc(2026, 11, 6, 10, 0)).on_break);
    assert_eq!(c.at(utc(2026, 11, 7, 10, 0)), OFF);
    assert!(!c.at(utc(2026, 11, 7, 8, 0)).on_break);
}

// ---- planned downtime ------------------------------------------------------

#[test]
fn planned_downtime_is_independent_of_shifts() {
    let c = cal(&format!("{PLANT}down 2026-11-07 23:00-01:00\n"));
    let s = c.at(utc(2026, 11, 2, 13, 50));
    assert!(s.in_shift && s.planned_down && !s.working && !s.on_break);
    // Dated downtime on a Saturday night, no shift, crossing midnight.
    let s = c.at(utc(2026, 11, 8, 0, 30));
    assert_eq!(
        s,
        State {
            planned_down: true,
            ..OFF
        }
    );
    assert!(!c.at(utc(2026, 11, 8, 1, 0)).planned_down);
}

// ---- exceptions ------------------------------------------------------------

#[test]
fn a_holiday_cancels_the_weekly_shifts_that_start_that_day() {
    let c = cal(&format!("{PLANT}holiday 2026-12-25\n"));
    // Christmas Friday: no shift 1 or 2.
    assert_eq!(c.at(utc(2026, 12, 25, 9, 0)), OFF);
    assert_eq!(c.at(utc(2026, 12, 25, 15, 0)), OFF);
    // Thursday night's shift 3 started on the 24th and still runs into it.
    assert_eq!(c.at(utc(2026, 12, 25, 3, 0)).shift_id, 3);
    // Weekly downtime is not a shift: it still applies.
    assert!(c.at(utc(2026, 12, 25, 13, 50)).planned_down);
}

#[test]
fn a_holiday_for_one_shift_cancels_only_that_shift() {
    let c = cal(&format!("{PLANT}holiday 2026-12-24 3\n"));
    assert_eq!(c.at(utc(2026, 12, 24, 9, 0)).shift_id, 1);
    assert_eq!(c.at(utc(2026, 12, 24, 23, 0)), OFF);
    assert_eq!(
        errors("zone UTC\nholiday 2026-12-24 3"),
        [CalError::NoSuchShift(3)]
    );
}

#[test]
fn a_dated_shift_replaces_that_days_weekly_occurrence() {
    // Friday's shift 2 runs to midnight; nothing else changes.
    let c = cal(&format!("{PLANT}shift 2 2026-11-06 14:00-24:00\n"));
    assert_eq!(c.at(utc(2026, 11, 6, 23, 30)), working(2, 570, 30));
    assert_eq!(c.at(utc(2026, 11, 5, 23, 30)).shift_id, 3, "Thursday as usual");
    assert_eq!(c.at(utc(2026, 11, 13, 22, 30)), OFF, "next Friday as usual");
    // Shorter, too: the weekly end no longer holds.
    let c = cal(&format!("{PLANT}shift 1 2026-11-02 06:00-10:00\n"));
    assert_eq!(c.at(utc(2026, 11, 2, 11, 0)), OFF);
    assert_eq!(c.at(utc(2026, 11, 3, 11, 0)).shift_id, 1);
}

#[test]
fn a_dated_shift_adds_overtime_and_runs_on_a_holiday() {
    let c = cal(&format!(
        "{PLANT}shift 1 2026-11-07 06:00-12:00\nholiday 2026-12-25\nshift 1 2026-12-25 06:00-10:00\n"
    ));
    assert_eq!(c.at(utc(2026, 11, 7, 7, 0)), working(1, 60, 300));
    assert!(
        c.at(utc(2026, 11, 7, 10, 10)).on_break,
        "the break fits: it applies"
    );
    assert_eq!(c.at(utc(2026, 11, 14, 7, 0)), OFF, "one Saturday only");
    assert_eq!(c.at(utc(2026, 12, 25, 7, 0)).shift_id, 1);
    assert_eq!(c.at(utc(2026, 12, 25, 15, 0)), OFF);
}

// ---- offsets and offset changes --------------------------------------------

const DST: &str = "
zone +01:00
zone +02:00 from 2026-03-29T01:00Z
zone +01:00 from 2026-10-25T01:00Z
shift 1 daily 06:00-14:00
shift 3 sat 22:00-06:00
";

#[test]
fn local_time_follows_a_fixed_offset() {
    let c = cal("zone +05:45\nshift 1 daily 06:00-14:00");
    assert_eq!(c.at(utc(2026, 11, 2, 0, 15)), working(1, 0, 480));
    assert_eq!(c.at(utc(2026, 11, 2, 0, 14)), OFF);
    let c = cal("zone -05:00\nshift 1 mon 06:00-14:00");
    assert_eq!(c.at(utc(2026, 11, 2, 11, 0)), working(1, 0, 480));
    // Monday 06:00 local is still Monday's shift though UTC is Monday too;
    // Monday 22:00 local is Tuesday 03:00 UTC: not in shift.
    assert_eq!(c.at(utc(2026, 11, 3, 3, 0)), OFF);
}

#[test]
fn a_spring_forward_change_moves_the_shift_in_utc_and_shortens_the_night() {
    let c = cal(DST);
    assert_eq!(c.offset_at(utc(2026, 3, 29, 0, 59)), 60);
    assert_eq!(c.offset_at(utc(2026, 3, 29, 1, 0)), 120);
    // Saturday's shift 1 starts 05:00Z (+1), Monday's 04:00Z (+2).
    assert_eq!(c.at(utc(2026, 3, 28, 5, 0)), working(1, 0, 480));
    assert_eq!(c.at(utc(2026, 3, 30, 4, 0)), working(1, 0, 480));
    assert_eq!(c.at(utc(2026, 3, 30, 3, 59)), OFF);
    // The night shift: 22:00 +1 is 21:00Z; local clock jumps 02:00 -> 03:00.
    assert_eq!(c.at(utc(2026, 3, 28, 21, 0)), working(3, 0, 480));
    assert_eq!(c.at(utc(2026, 3, 29, 0, 59)), working(3, 239, 241));
    assert_eq!(c.at(utc(2026, 3, 29, 1, 0)), working(3, 300, 180));
    // Ends 06:00 +2 = 04:00Z: seven real hours.
    assert_eq!(c.at(utc(2026, 3, 29, 3, 59)).shift_id, 3);
    assert_eq!(
        c.at(utc(2026, 3, 29, 4, 0)),
        working(1, 0, 480),
        "Sunday shift 1, 06:00 +2"
    );
}

#[test]
fn a_fall_back_change_repeats_an_hour_and_lengthens_the_night() {
    let c = cal(DST);
    // Saturday 2026-10-24 22:00 +2 = 20:00Z.
    assert_eq!(c.at(utc(2026, 10, 24, 20, 0)), working(3, 0, 480));
    assert_eq!(c.at(utc(2026, 10, 25, 0, 59)), working(3, 299, 181));
    assert_eq!(c.at(utc(2026, 10, 25, 1, 0)), working(3, 240, 240), "02:00 again");
    // Ends 06:00 +1 = 05:00Z: nine real hours.
    assert_eq!(c.at(utc(2026, 10, 25, 4, 59)).shift_id, 3);
    assert_eq!(c.at(utc(2026, 10, 25, 5, 0)), working(1, 0, 480));
}

#[test]
fn zone_names_and_bad_offsets_are_refused() {
    let e = parse("zone Europe/Berlin").unwrap_err();
    assert_eq!(e[0].0, 1);
    assert!(
        e[0].1.contains("time zone database") && e[0].1.contains("+01:00"),
        "{e:?}"
    );
    for bad in [
        "zone +1",
        "zone +01:60",
        "zone +18:01",
        "zone 01:00",
        "zone +01:00 from 2026-03-29T01:00",
        "zone +01:00 from 2026-02-30T01:00Z",
        "zone +01:00 since 2026-03-29T01:00Z",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
    cal("zone -18:00");
    cal("zone +18:00");
}

#[test]
fn zones_need_one_base_and_increasing_changes() {
    assert_eq!(errors("shift 1 mon 06:00-14:00"), [CalError::NoZone]);
    assert_eq!(
        errors("zone +01:00\nzone +02:00\nshift 1 mon 06:00-14:00"),
        [CalError::TwoBaseZones]
    );
    assert_eq!(
        errors("zone +02:00 from 2026-03-29T01:00Z\nshift 1 mon 06:00-14:00"),
        [CalError::NoZone, CalError::ZoneOrder]
    );
    assert_eq!(
        errors("zone +01:00\nzone +01:00 from 2026-10-25T01:00Z\nzone +02:00 from 2026-03-29T01:00Z"),
        [CalError::ZoneOrder]
    );
}

// ---- leap days -------------------------------------------------------------

#[test]
fn leap_days_exist_only_in_leap_years() {
    for ok in ["2028-02-29", "2000-02-29", "2400-02-29"] {
        assert!(Date::parse(ok).is_some(), "{ok}");
    }
    for bad in [
        "2027-02-29",
        "2100-02-29",
        "1900-02-29",
        "2026-02-30",
        "2026-04-31",
        "2026-13-01",
        "2026-00-10",
        "0000-01-01",
        "2026-1-01",
        "+026-01-01",
    ] {
        assert!(Date::parse(bad).is_none(), "{bad}");
        assert!(parse(&format!("zone UTC\nholiday {bad}")).is_err(), "{bad}");
    }
}

#[test]
fn a_leap_day_is_a_tuesday_with_shifts_and_holidays() {
    let c = cal("zone UTC\nshift 1 tue 06:00-14:00\nshift 3 mon 22:00-06:00\nholiday 2032-02-29");
    // 2028-02-29 is a Tuesday: shift 1 runs, shift 3 from Monday the 28th runs into it.
    assert_eq!(c.at(utc(2028, 2, 29, 7, 0)).shift_id, 1);
    assert_eq!(c.at(utc(2028, 2, 29, 3, 0)), working(3, 300, 180));
    assert_eq!(c.at(utc(2028, 3, 1, 3, 0)), OFF, "no shift 3 on Tuesday night");
    // 2032-02-29 is a Sunday: a holiday with nothing to cancel is harmless.
    assert_eq!(c.at(utc(2032, 2, 29, 7, 0)), OFF);
    // A dated night shift from Feb 28 into Mar 1 in a common year.
    let c = cal("zone UTC\nshift 5 2027-02-28 22:00-06:00");
    assert_eq!(c.at(utc(2027, 3, 1, 5, 0)), working(5, 420, 60));
    assert_eq!(c.at(utc(2027, 3, 1, 6, 0)), OFF);
}

// ---- syntax ----------------------------------------------------------------

#[test]
fn bad_lines_are_refused_with_their_line_numbers() {
    let src = "zone UTC\nshift 0 mon 06:00-14:00\nshift 100 mon 06:00-14:00\nshift 1 mon 06:00-06:00\nshift 1 mon 24:00-06:00\nshift 1 mon 06:00-24:01\nshift 1 mon 6:00-14:00\nshift 1 funday 06:00-14:00\nshift 1 mon 06:00-14:00 extra\nbreak 1\nholiday 2026-12-25 1 2\ndrive 9lamp in_shift\ndrive lamp sometimes\ndrive lamp shift=0\nlunch 12:00-13:00\nshift 1 mon 06:00-14:00\u{7f}\nshift 1 mon 06:00\u{a0}-14:00";
    let errs = parse(src).unwrap_err();
    let lines: Vec<usize> = errs.iter().map(|(n, _)| *n).collect();
    assert_eq!(lines, (2..=17).collect::<Vec<_>>(), "{errs:#?}");
}

#[test]
fn day_lists_and_ranges_wrap_over_the_week() {
    let c = cal("zone UTC\nshift 1 fri-mon 06:00-07:00\nshift 2 wed,mon 08:00-09:00");
    let id = |d| c.at(utc(2026, 11, d, 6, 30)).shift_id;
    // 2026-11-02 Mon .. 2026-11-08 Sun.
    assert_eq!([2, 3, 4, 5, 6, 7, 8].map(id), [1, 0, 0, 0, 1, 1, 1]);
    let id = |d| c.at(utc(2026, 11, d, 8, 30)).shift_id;
    assert_eq!([2, 3, 4, 5, 6, 7, 8].map(id), [2, 0, 2, 0, 0, 0, 0]);
    assert_eq!(
        render(&c).lines().nth(1),
        Some("shift 1 mon,fri,sat,sun 06:00-07:00")
    );
}

#[test]
fn drives_are_checked() {
    assert_eq!(
        errors("drive lamp in_shift\ndrive LAMP on_break"),
        [CalError::DriveTwice("LAMP".into())]
    );
    assert_eq!(errors("drive lamp shift=4"), [CalError::NoSuchShift(4)]);
}

const ROUND: &str = "
zone +01:00
zone +02:00 from 2026-03-29T01:00Z
zone +01:00 from 2026-10-25T01:00Z
shift 1 mon-fri 06:00-14:00
shift 2 mon-fri 14:00-22:00
shift 3 sun-thu 22:00-06:00
shift 2 2026-11-06 14:00-24:00
shift 7 sat 00:00-24:00
break 1 10:00-10:30
break 3 02:00-02:30
down daily 13:45-14:00
down 2026-11-02 23:00-01:00
holiday 2026-12-25
holiday 2026-12-24 3
drive horn on_break
drive beacon !working
drive heater shift=3
";

#[test]
fn the_example_round_trips() {
    let c = cal(ROUND);
    let text = render(&c);
    assert_eq!(cal(&text), c, "{text}");
    assert_eq!(render(&cal(&text)), text);
    assert!(text.contains("shift 7 sat 00:00-24:00\n"));
    assert!(text.contains("shift 2 2026-11-06 14:00-00:00\n"));
}

// ---- properties ------------------------------------------------------------

fn day_token() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("daily".to_string()),
        prop::sample::select(vec![
            "mon",
            "tue",
            "wed",
            "thu",
            "fri",
            "sat",
            "sun",
            "mon-fri",
            "sun-thu",
            "fri-mon",
            "sat,sun",
            "mon,wed,fri"
        ])
        .prop_map(String::from),
        (
            prop::sample::select(vec![1u16, 1970, 2026, 2027, 2028, 2100, 9999]),
            1u8..=12,
            1u8..=31
        )
            .prop_map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")),
    ]
}

fn time_token(end: bool) -> impl Strategy<Value = String> {
    let hours = if end { 0u16..=24 } else { 0u16..=23 };
    (hours, prop::sample::select(vec![0u16, 1, 15, 30, 59])).prop_map(|(h, m)| {
        if h == 24 {
            "24:00".into()
        } else {
            format!("{h:02}:{m:02}")
        }
    })
}

fn span_token() -> impl Strategy<Value = String> {
    (time_token(false), time_token(true)).prop_map(|(a, b)| format!("{a}-{b}"))
}

fn offset_token() -> impl Strategy<Value = String> {
    (any::<bool>(), 0u16..=18, prop::sample::select(vec![0u16, 30, 45]))
        .prop_map(|(neg, h, m)| format!("{}{h:02}:{m:02}", if neg { '-' } else { '+' }))
}

fn line() -> impl Strategy<Value = String> {
    let id = || 1u8..=6;
    prop_oneof![
        offset_token().prop_map(|o| format!("zone {o}")),
        (offset_token(), day_token(), time_token(false))
            .prop_map(|(o, d, t)| format!("zone {o} from {d}T{t}Z")),
        (id(), day_token(), span_token()).prop_map(|(n, d, s)| format!("shift {n} {d} {s}")),
        (id(), span_token()).prop_map(|(n, s)| format!("break {n} {s}")),
        (day_token(), span_token()).prop_map(|(d, s)| format!("down {d} {s}")),
        (day_token(), prop::option::of(id()))
            .prop_map(|(d, n)| format!("holiday {d}{}", n.map(|n| format!(" {n}")).unwrap_or_default())),
        (
            prop::sample::select(vec!["a", "b", "lamp"]),
            any::<bool>(),
            prop::sample::select(vec![
                "in_shift",
                "on_break",
                "planned_down",
                "working",
                "shift=1",
                "shift=3"
            ])
        )
            .prop_map(|(s, neg, c)| format!("drive {s} {}{c}", if neg { "!" } else { "" })),
    ]
}

fn instant() -> impl Strategy<Value = i64> {
    prop_oneof![
        any::<i64>(),
        utc(1, 1, 1, 0, 0)..utc(9999, 12, 31, 23, 59),
        utc(2026, 1, 1, 0, 0)..utc(2029, 1, 1, 0, 0),
        Just(i64::MIN),
        Just(i64::MAX),
    ]
}

/// What every state must satisfy, whatever the calendar.
fn consistent(s: &State) -> bool {
    s.on_break <= s.in_shift
        && s.working == (s.in_shift && !s.on_break && !s.planned_down)
        && (s.shift_id == 0) != s.in_shift
        && (0..=99).contains(&s.shift_id)
        && if s.in_shift {
            (0..1440).contains(&s.shift_minute) && (1..=1440).contains(&s.shift_left)
        } else {
            s.shift_minute == 0 && s.shift_left == 0
        }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Evaluation never panics and is consistent, for any instant and any
    /// calendar the parser accepts, valid or not.
    #[test]
    fn evaluation_is_total(lines in prop::collection::vec(line(), 0..12), ts in prop::collection::vec(instant(), 1..8)) {
        let src = lines.join("\n");
        if let Ok(c) = parse(&src) {
            let _ = c.validate();
            for t in ts {
                let s = c.at(t);
                prop_assert!(consistent(&s), "{src}\nat {t}: {s:?}");
                prop_assert_eq!(s, c.at(t), "deterministic");
            }
        }
    }

    /// A valid calendar is in at most one shift at once, so the state does
    /// not depend on declaration order.
    #[test]
    fn a_valid_calendar_does_not_depend_on_line_order(lines in prop::collection::vec(line(), 0..10), ts in prop::collection::vec(instant(), 1..8)) {
        let src = lines.join("\n");
        let rev: Vec<&str> = lines.iter().rev().map(String::as_str).collect();
        if let (Ok(a), Ok(b)) = (load(&src), load(&rev.join("\n"))) {
            for t in ts {
                prop_assert_eq!(a.at(t), b.at(t), "{}\nat {}", src, t);
            }
        }
    }

    #[test]
    fn whatever_parses_renders_back_to_itself(lines in prop::collection::vec(line(), 0..16)) {
        if let Ok(c) = parse(&lines.join("\n")) {
            let text = render(&c);
            prop_assert_eq!(parse(&text), Ok(c), "{}", text);
        }
    }

    #[test]
    fn the_parser_never_panics_on_any_text(src in ".{0,200}") {
        let _ = load(&src);
    }

    #[test]
    fn the_parser_never_panics_on_token_soup(words in prop::collection::vec(prop_oneof![
        prop::sample::select(vec!["zone", "shift", "break", "down", "holiday", "drive", "from", "daily", "mon-sun", "-", ":", "!", "#", "\n", "shift=", "24:00", "+18:00", "-99:99", "9999-12-31", "0001-01-01", "2028-02-29"]).prop_map(String::from),
        "[0-9:+\\-TZ]{0,12}",
        span_token(),
    ], 0..30)) {
        let src = words.join(" ");
        if let Ok(c) = load(&src) {
            let _ = c.at(0);
        }
    }
}

// ---- the calendar as signals -----------------------------------------------

fn out(name: &str, ty: Ty, channel: &str) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir: Dir::Out,
        binding: Binding::Local {
            channel: channel.into(),
        },
        unit: None,
        poll: None,
    }
}

fn table(safe: &[(&str, Value)]) -> SignalTable {
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![
            out("lamp", Ty::Bool, "do:0"),
            out("horn", Ty::Bool, "do:1"),
            Signal {
                name: "button".into(),
                ty: Ty::Bool,
                dir: Dir::In,
                binding: Binding::Local {
                    channel: "di:0".into(),
                },
                unit: None,
                poll: None,
            },
        ],
    };
    let safe: BTreeMap<String, Value> = safe.iter().map(|(k, v)| ((*k).into(), *v)).collect();
    SignalTable::new(&map, &safe).unwrap()
}

fn module(name: &str, src: &str) -> Module {
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap_or_else(|d| panic!("{name}: {d:?}"));
    Module {
        name: name.into(),
        program: program.into(),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

const COUNTER: &str = "
PROGRAM counter
VAR_INPUT In_Shift : BOOL; shift_id : INT; shift_minute : DINT; shift_left : DINT; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR seen_id : INT; seen_min : DINT; seen_left : DINT; END_VAR
  lamp := in_shift AND shift_left <= 10;
  seen_id := shift_id;
  seen_min := shift_minute;
  seen_left := shift_left;
END_PROGRAM";

fn command(t: &SignalTable, name: &str) -> Value {
    t.command(t.index(name).unwrap()).unwrap()
}

#[test]
fn a_module_reads_the_calendar_by_name_like_any_signal() {
    let mut t = table(&[]);
    let src = Source::attach(cal(PLANT), &mut t).unwrap();
    for (name, ty) in SIGNALS {
        assert_eq!(t.signal(t.index(name).unwrap()), Some((Dir::In, ty)), "{name}");
    }
    let mut rt = Runtime::with_signals(0, 0, vec![module("counter", COUNTER)], &t).unwrap();

    src.update(Some(utc(2026, 11, 2, 13, 55)), &mut t);
    assert_eq!(rt.tick_with(Duration::ZERO, &[], &mut t), Vec::new());
    let p = rt.program("counter").unwrap();
    assert_eq!(p.get("seen_id"), Some(St::Int(1)));
    assert_eq!(p.get("seen_min"), Some(St::Int(475)));
    assert_eq!(p.get("seen_left"), Some(St::Int(5)));
    assert_eq!(command(&t, "lamp"), Value::Bool(true));

    src.update(Some(utc(2026, 11, 2, 14, 0)), &mut t);
    rt.tick_with(Duration::from_millis(10), &[], &mut t);
    assert_eq!(command(&t, "lamp"), Value::Bool(false));
    assert_eq!(rt.program("counter").unwrap().get("seen_id"), Some(St::Int(2)));
}

#[test]
fn an_untrusted_clock_holds_the_readers_and_sends_driven_outputs_safe() {
    let mut t = table(&[("lamp", Value::Bool(true)), ("horn", Value::Bool(true))]);
    let src = Source::attach(cal(&format!("{PLANT}drive horn !working")), &mut t).unwrap();
    let mut rt = Runtime::with_signals(0, 0, vec![module("counter", COUNTER)], &t).unwrap();

    src.update(Some(utc(2026, 11, 2, 9, 0)), &mut t);
    rt.tick_with(Duration::ZERO, &[], &mut t);
    assert_eq!(rt.status("counter"), Some(Status::Running));
    assert_eq!(
        (command(&t, "lamp"), command(&t, "horn")),
        (Value::Bool(false), Value::Bool(false))
    );

    for wall in [None, Some(0), Some(CLOCK_FLOOR - 1)] {
        src.update(wall, &mut t);
        rt.tick_with(Duration::from_millis(10), &[], &mut t);
        assert_eq!(rt.status("counter"), Some(Status::Held), "{wall:?}");
        assert_eq!(command(&t, "lamp"), Value::Bool(true), "module's output safe");
        assert_eq!(command(&t, "horn"), Value::Bool(true), "calendar's output safe");
        assert_eq!(src.state(wall), None);
    }

    src.update(Some(utc(2026, 11, 2, 10, 5)), &mut t);
    rt.tick_with(Duration::from_millis(20), &[], &mut t);
    assert_eq!(rt.status("counter"), Some(Status::Running));
    assert_eq!(command(&t, "horn"), Value::Bool(true), "on break: not working");
}

#[test]
fn the_calendar_drives_an_output_on_schedule() {
    let mut t = table(&[]);
    let src = Source::attach(
        cal(&format!("{PLANT}drive lamp shift=3\ndrive horn on_break")),
        &mut t,
    )
    .unwrap();
    let steps = [
        (utc(2026, 11, 2, 9, 0), false, false),
        (utc(2026, 11, 2, 10, 0), false, true),
        (utc(2026, 11, 2, 22, 0), true, false),
        (utc(2026, 11, 3, 2, 15), true, true),
        (utc(2026, 11, 7, 2, 15), false, false),
    ];
    for (wall, lamp, horn) in steps {
        src.update(Some(wall), &mut t);
        assert_eq!(command(&t, "lamp"), Value::Bool(lamp), "lamp at {wall}");
        assert_eq!(command(&t, "horn"), Value::Bool(horn), "horn at {wall}");
    }
    // A calendar-only box: no module at all.
    assert!(Runtime::with_signals(0, 0, Vec::new(), &t).is_ok());
}

#[test]
fn a_module_cannot_drive_an_output_the_calendar_drives() {
    let mut t = table(&[]);
    Source::attach(cal("drive lamp in_shift"), &mut t).unwrap();
    let errs = Runtime::with_signals(0, 0, vec![module("counter", COUNTER)], &t).unwrap_err();
    assert_eq!(
        errs,
        [ConfigError::TwoWriters {
            signal: "LAMP".into(),
            a: "calendar".into(),
            b: "counter".into(),
        }]
    );
}

#[test]
fn drives_must_name_bool_outputs_and_calendar_names_must_be_free() {
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![
            out("Working", Ty::Bool, "do:0"),
            Signal {
                name: "button".into(),
                ty: Ty::Bool,
                dir: Dir::In,
                binding: Binding::Local {
                    channel: "di:0".into(),
                },
                unit: None,
                poll: None,
            },
        ],
    };
    let mut t = SignalTable::new(&map, &BTreeMap::new()).unwrap();
    let errs = Source::attach(cal("drive button in_shift\ndrive nothing in_shift"), &mut t).unwrap_err();
    assert_eq!(
        errs,
        [
            AttachError::Name(TableError::SameName {
                a: "Working".into(),
                b: "working".into(),
            }),
            AttachError::NotOutput("button".into()),
            AttachError::UnknownSignal("nothing".into()),
        ]
    );
    // An int output on a Modbus holding register: bool outputs only.
    let rio = helmsman_io::text::parse("device rio modbus-tcp 127.0.0.1:1\nout level int rio hr:5").unwrap();
    let mut t = SignalTable::new(&rio.map, &rio.safe).unwrap();
    let errs = Source::attach(cal("drive level in_shift"), &mut t).unwrap_err();
    assert_eq!(errs, [AttachError::NotBool("level".into())]);
}
