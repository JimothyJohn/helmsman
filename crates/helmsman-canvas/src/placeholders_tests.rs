//! Placeholder points for a Skipper ([`crate::placeholders`]).

use crate::placeholders::{HEADER, add, base, points, said};
use crate::playground::SIZES;

/// The Skipper's list as its `GET /api/io_map` gives it: a comment, a
/// device, points, a line that is neither.
const NODE: &str = "# bench: the MDrive\ndevice drive enip-io 192.168.33.1 config=1\nin  enabled bool drive in:0.0\nout slew int drive out:20:i32 # steps/s\nbogus line\n";

const LATCH: &str = "PROGRAM skipper\nVAR_INPUT\n  start : BOOL;\n  stop : BOOL;\n  enabled : BOOL;\n  speed : REAL;\nEND_VAR\nVAR_OUTPUT\n  running : BOOL;\n  slew : DINT;\nEND_VAR\nVAR\n  state : INT;\nEND_VAR\nrunning := (start OR running) AND NOT stop AND enabled;\nIF running THEN slew := REAL_TO_DINT(speed); ELSE slew := 0; END_IF;\nEND_PROGRAM\n";

#[test]
fn a_port_no_point_backs_gets_a_sim_line_of_its_type_and_direction() {
    let a = add(LATCH, SIZES, NODE).expect("start, stop, speed and running need one");
    assert_eq!(
        a.lines,
        [
            "in  start bool sim",
            "in  stop bool sim",
            "in  speed real sim",
            "out running bool sim",
        ]
    );
    assert!(a.cannot.is_empty(), "{:?}", a.cannot);
    // The Skipper's list whole, then the header, then the lines.
    assert!(a.map.starts_with(NODE.trim_end()), "{}", a.map);
    assert!(
        a.map.ends_with(&format!(
            "{HEADER}\nin  start bool sim\nin  stop bool sim\nin  speed real sim\nout running bool sim\n"
        )),
        "{}",
        a.map
    );
    // A VAR is no port, and a port with its point gets nothing.
    for n in ["state", "enabled", "slew"] {
        assert!(
            !a.lines.iter().any(|l| l.contains(&format!(" {n} "))),
            "{n}: {:?}",
            a.lines
        );
    }
}

#[test]
fn every_port_on_the_list_needs_nothing() {
    let all = "in enabled bool sim\nout slew int sim\n";
    let p = "PROGRAM p\nVAR_INPUT enabled : BOOL; END_VAR\nVAR_OUTPUT slew : DINT; END_VAR\nslew := 0;\nEND_PROGRAM\n";
    assert_eq!(add(p, SIZES, all), None);
}

#[test]
fn the_names_match_without_regard_to_case() {
    let p = "PROGRAM p\nVAR_INPUT Enabled : BOOL; END_VAR\nVAR_OUTPUT SLEW : DINT; END_VAR\nSLEW := 0;\nEND_PROGRAM\n";
    assert_eq!(add(p, SIZES, NODE), None);
}

#[test]
fn adding_twice_adds_nothing_the_second_time() {
    let a = add(LATCH, SIZES, NODE).unwrap();
    assert_eq!(add(LATCH, SIZES, &a.map), None, "{}", a.map);
    // A port added later goes under the one header.
    let more = LATCH.replace("  speed : REAL;\n", "  speed : REAL;\n  jog : BOOL;\n");
    let b = add(&more, SIZES, &a.map).unwrap();
    assert_eq!(b.lines, ["in  jog bool sim"]);
    assert_eq!(b.map.matches(HEADER).count(), 1, "{}", b.map);
}

#[test]
fn a_point_facing_the_other_way_or_a_type_with_no_point_is_said_not_added() {
    let p = "PROGRAM p\nVAR_INPUT slew : DINT; t : TIME; END_VAR\nVAR_OUTPUT enabled : BOOL; END_VAR\nenabled := FALSE;\nEND_PROGRAM\n";
    let a = add(p, SIZES, NODE).unwrap();
    assert!(a.lines.is_empty(), "{:?}", a.lines);
    assert_eq!(a.cannot.len(), 3, "{:?}", a.cannot);
    assert!(
        a.cannot[0].contains("slew is a VAR_INPUT in the program and an out point"),
        "{:?}",
        a.cannot
    );
    assert!(a.cannot[1].starts_with("t: "), "{:?}", a.cannot);
    assert!(
        a.cannot[2].contains("enabled is a VAR_OUTPUT in the program and an in point"),
        "{:?}",
        a.cannot
    );
    // Nothing added: the map is the list as it was.
    assert_eq!(a.map.trim_end(), NODE.trim_end());
    let words = said(&a).unwrap();
    assert!(words.starts_with("No placeholder for slew is"), "{words}");
}

#[test]
fn a_program_that_does_not_build_is_left_to_the_build() {
    assert_eq!(
        add(
            "PROGRAM p\nVAR_INPUT x : BOOL END_VAR\nEND_PROGRAM\n",
            SIZES,
            NODE
        ),
        None
    );
    assert_eq!(add("", SIZES, NODE), None);
}

#[test]
fn an_empty_list_gives_a_map_of_placeholders_alone() {
    let p = "PROGRAM p\nVAR_INPUT go : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := go;\nEND_PROGRAM\n";
    let a = add(p, SIZES, "").unwrap();
    assert_eq!(a.map, format!("{HEADER}\nin  go bool sim\nout lamp bool sim\n"));
}

#[test]
fn the_projects_map_is_the_base_once_it_has_a_point() {
    assert_eq!(base(None, NODE), NODE);
    assert_eq!(base(Some("# nothing yet\n"), NODE), NODE);
    assert_eq!(base(Some(""), NODE), NODE);
    let mine = "device rio modbus-tcp 192.168.3.60\nin start bool rio di:0\n";
    assert_eq!(base(Some(mine), NODE), mine);
}

#[test]
fn points_reads_in_and_out_lines_only() {
    assert_eq!(
        points(NODE),
        [("enabled".to_string(), true), ("slew".to_string(), false)]
    );
    // A commented-out point, a word alone, garbage: none of them.
    assert_eq!(points("# in x bool sim\nin\nout\n\u{0}in y\n"), []);
}

#[test]
fn the_words_name_the_points_and_where_to_set_them() {
    let a = add(LATCH, SIZES, NODE).unwrap();
    assert_eq!(
        said(&a).unwrap(),
        "Placeholder points added to io.map for start, stop, speed, running: no device has them yet. Set the inputs on the I/O tab."
    );
}
