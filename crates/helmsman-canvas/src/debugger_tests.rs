//! The simulation's debugger keys: a breakpoint stops the scan before its
//! statement and never changes what the program does.

use helmsman_iec::Value;
use helmsman_iec::trace::StmtId;

use crate::playground::Session;

const SRC: &str = "PROGRAM t
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n, m, s, i : INT; END_VAR
n := n + 1;
m := n * 2;
FOR i := 1 TO 3 DO
  s := s + i;
END_FOR;
lamp := start;
END_PROGRAM";

/// The statement starting on source line `line` of the running build.
fn at(s: &Session, line: u32) -> StmtId {
    let b = s.build.as_ref().expect("built");
    b.program.trace_map().at_line(line).next().expect("a statement")
}

/// A variable as the paused (or last) scan shows it.
fn shown(s: &Session, path: &str) -> Value {
    s.trace.as_ref().and_then(|t| t.get(path)).expect(path)
}

/// A variable of the program itself (what the scans committed).
fn committed(s: &Session, path: &str) -> Value {
    s.build.as_ref().and_then(|b| b.program.get(path)).expect(path)
}

fn int(n: i128) -> Value {
    Value::Int(n)
}

#[test]
fn step_stops_before_the_breakpoint_and_f10_runs_the_rest() {
    let mut s = Session::new(SRC);
    let m = at(&s, 6);
    s.toggle_break(m);
    s.step();
    assert_eq!(s.paused, Some(0));
    assert_eq!(s.paused_at(), Some(m));
    assert_eq!(
        (shown(&s, "n"), shown(&s, "m")),
        (int(1), int(0)),
        "n ran, m not yet"
    );
    assert_eq!(committed(&s, "n"), int(0), "nothing committed while paused");
    assert_eq!((s.scans, s.now_ms), (0, 0), "the clock waits");

    s.step();
    assert_eq!(s.paused, None);
    assert_eq!((committed(&s, "n"), committed(&s, "m")), (int(1), int(2)));
    assert_eq!(s.scans, 1);

    // The next step stops at the same place one scan on.
    s.step();
    assert_eq!(s.paused_at(), Some(m));
    assert_eq!(shown(&s, "n"), int(2));
}

#[test]
fn run_stops_at_each_hit_and_run_again_goes_on_to_the_next() {
    let mut s = Session::new(SRC);
    let body = at(&s, 8);
    s.toggle_break(body);
    s.set_running(true);
    s.advance(0.0);
    s.advance(0.05);
    assert!(!s.running, "a breakpoint stops the simulation");
    assert_eq!(s.paused, Some(0));
    assert_eq!(shown(&s, "i"), int(1));
    for pass in 2..=3 {
        s.set_running(true);
        assert!(!s.running, "pass {pass} stops again");
        assert_eq!(shown(&s, "i"), int(pass));
        assert_eq!(s.paused, Some(u32::try_from(pass).unwrap() - 1));
    }
    // No fourth pass: the scan finishes and the simulation runs on.
    let scans = s.scans;
    s.set_running(true);
    assert!(s.running);
    assert_eq!(s.paused, None);
    assert_eq!(s.scans, scans + 1);
    assert_eq!(committed(&s, "s"), int(6));
    // And stops in the next scan's first pass.
    s.advance(0.1);
    s.advance(0.2);
    assert_eq!(s.paused, Some(0));
    assert_eq!(shown(&s, "s"), int(6));
}

#[test]
fn a_paused_scan_follows_the_inputs_as_they_change() {
    let mut s = Session::new(SRC);
    let lamp = at(&s, 10);
    s.toggle_break(lamp);
    s.step();
    assert_eq!(shown(&s, "start"), Value::Bool(false));
    assert!(s.set_input("start", Value::Bool(true)));
    s.advance(1.0);
    assert_eq!(shown(&s, "start"), Value::Bool(true));
    s.step();
    assert_eq!(committed(&s, "lamp"), Value::Bool(true));
}

#[test]
fn breaking_never_changes_what_the_program_does() {
    let mut plain = Session::new(SRC);
    let mut broken = Session::new(SRC);
    for line in [5, 8, 10] {
        let id = at(&broken, line);
        broken.toggle_break(id);
    }
    for k in 0..20 {
        let on = k % 3 == 0;
        plain.set_input("start", Value::Bool(on));
        broken.set_input("start", Value::Bool(on));
        plain.step();
        broken.step();
        // Through every stop of the scan, then the rest of it.
        while broken.paused.is_some() {
            broken.set_running(true);
            broken.set_running(false);
            if broken.paused.is_some() {
                broken.step();
            }
        }
        for v in ["n", "m", "s", "i", "lamp"] {
            assert_eq!(committed(&plain, v), committed(&broken, v), "{v} after scan {k}");
        }
        assert_eq!((plain.scans, plain.now_ms), (broken.scans, broken.now_ms));
    }
}

#[test]
fn an_edit_keeps_the_breakpoints_whose_statements_stayed_put() {
    let mut s = Session::new(SRC);
    let (n, lamp) = (at(&s, 5), at(&s, 10));
    s.toggle_break(n);
    s.toggle_break(lamp);
    s.step();
    assert!(s.paused.is_some());
    // The FOR's body grows by a line: `lamp :=` moves down, `n :=` stays.
    let edited = SRC.replace("  s := s + i;", "  s := s + i;\n  m := m + 0;");
    assert!(s.set_source(&edited));
    assert_eq!(s.paused, None, "a new build drops the paused scan");
    assert_eq!(s.breaks, vec![at(&s, 5)]);
    s.toggle_break(at(&s, 5));
    assert!(s.breaks.is_empty(), "Shift F9 again clears it");
}

#[test]
fn restart_drops_the_paused_scan() {
    let mut s = Session::new(SRC);
    s.toggle_break(at(&s, 6));
    s.step();
    s.restart();
    assert_eq!(s.paused, None);
    assert_eq!(s.paused_at(), None);
    assert_eq!(s.breaks.len(), 1, "the breakpoints stay");
}
