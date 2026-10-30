//! The floor screen: its layout file, checked against the I/O map, and
//! screen tiles as outputs under the same rules as a stack light (one
//! writer, safe values on a fault or a hold).

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_iec::{FaultKind, ImageSizes, compile};
use helmsman_io::{Ty, Value};
use helmsman_plc::screen::{self, Colour, Layout, MAX_TEXT, Tile, render};
use helmsman_plc::{ConfigError, Module, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

const EXAMPLE: &str = "\
helmsman-screen 1
# Hall 2, lines 1 and 2
title  Hall 2 andon
grid   2 1
code   line 0 grey   Idle
code   line 1 green  Running
code   line 2 yellow Material low
code   line 3 red    Down: call maintenance
tile   line1 0,0 line line1_state Line 1
tile   line2 1,0 line line2_state Line 2
value  line1 line1_parts parts today
";

const MAP: &str = "\
in  go          bool box    di:0
out line1_state int  screen line1 safe=3
out line1_parts int  screen line1
out line2_state int  screen line2
";

fn map() -> helmsman_io::text::MapFile {
    helmsman_io::text::parse(MAP).unwrap()
}

fn errors(src: &str) -> Vec<(usize, String)> {
    screen::parse(src)
        .unwrap_err()
        .into_iter()
        .map(|e| (e.line, e.why))
        .collect()
}

#[test]
fn the_documented_example_parses_and_fits_its_map() {
    let l = screen::parse(EXAMPLE).unwrap();
    assert_eq!(l.title.as_deref(), Some("Hall 2 andon"));
    assert_eq!((l.cols, l.rows), (2, 1));
    let t = &l.tiles[0];
    assert_eq!(
        (t.id.as_str(), t.state.as_str(), t.title.as_str()),
        ("line1", "line1_state", "Line 1")
    );
    assert_eq!(t.value.as_ref().map(|v| v.label.as_str()), Some("parts today"));
    let red = l.state(t, 3).unwrap();
    assert_eq!(
        (red.colour, red.text.as_str()),
        (Colour::Red, "Down: call maintenance")
    );
    assert_eq!(l.state(t, 4), None, "an undeclared code shows as unknown");
    l.check(&map().map).unwrap();
    // The module doc carries the same example.
    let doc = include_str!("../src/screen.rs");
    for line in EXAMPLE.lines() {
        assert!(
            doc.contains(&format!("//! {line}").trim_end().to_string()),
            "{line}"
        );
    }
}

#[test]
fn bad_lines_are_reported_with_their_numbers() {
    let src = "\
helmsman-screen 1
grid 9 1
grid 2 0
code line x green Running
code line 1 purple Running
code line 1 green
code 1line 1 green Running
code line 1 green Running
code line 1 red Twice
tile 9t 0,0 line s Title
tile t 0;0 line s Title
tile t 0,0 line s
tile t 0,0 line 1s Title
title A
title B
colour line 1 red
value t s
title has a \u{202e}bidi override
";
    let lines: Vec<usize> = errors(src).iter().map(|(n, _)| *n).collect();
    assert_eq!(
        lines,
        [2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 15, 16, 17, 18, 0],
        "{:?}",
        errors(src)
    );
    assert_eq!(errors("")[0].0, 0);
    assert_eq!(errors("grid 1 1\n")[0].0, 1, "the header comes first");
    assert_eq!(errors("# only a comment\n")[0].0, 0);
    let long = format!(
        "helmsman-screen 1\ngrid 1 1\ntitle {}\n",
        "x".repeat(MAX_TEXT + 1)
    );
    assert_eq!(errors(&long)[0].0, 3);
    let huge = format!("helmsman-screen 1\n{}", "# padding\n".repeat(10_000));
    assert_eq!(errors(&huge)[0].0, 0, "oversized files are refused whole");
}

#[test]
fn tiles_must_fit_the_grid_not_overlap_and_name_known_sets() {
    let src = "\
helmsman-screen 1
grid 2 2
code s 0 grey Idle
tile a 0,0,2,1 s x A
tile b 1,0 s y B
tile c 1,1,1,2 s z C
tile d 0,1 nosuch w D
value e v E
";
    let why: Vec<String> = errors(src).into_iter().map(|(_, w)| w).collect();
    assert!(why.iter().any(|w| w.contains("a and b overlap")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("c does not fit")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("no code set nosuch")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("tile e")), "{why:?}");
    assert_eq!(why.len(), 4, "{why:?}");
}

#[test]
fn a_hash_inside_text_is_text() {
    let l = screen::parse("helmsman-screen 1\ngrid 1 1\ncode s 0 red Call #3 now\ntile t 0,0 s x Press #3\n")
        .unwrap();
    assert_eq!(l.tiles[0].title, "Press #3");
    assert_eq!(l.sets["s"][&0].text, "Call #3 now");
    let unicode =
        screen::parse("helmsman-screen 1\ngrid 1 1\ncode s -1 red Störung\ntile t 0,0 s x Linie 1\n")
            .unwrap();
    assert_eq!(unicode.sets["s"][&-1].text, "Störung");
}

#[test]
fn the_layout_is_checked_against_the_map() {
    let l = screen::parse(EXAMPLE).unwrap();
    // A state signal that is a device output, a real state, a tile's
    // signal on another tile, a missing signal, and a screen output no
    // tile shows.
    let m = helmsman_io::text::parse(
        "out line1_state int box do:0\nout line1_parts real screen line2\nout line2_state real screen line2\nout spare int screen line3\n",
    )
    .unwrap();
    let errs = l.check(&m.map).unwrap_err();
    let has = |s: &str| errs.iter().any(|e| e.contains(s));
    assert!(
        has("line1_state must be declared `out line1_state TYPE screen line1`"),
        "{errs:?}"
    );
    assert!(has("line1_parts must be declared"), "{errs:?}");
    assert!(has("state line2_state is Real"), "{errs:?}");
    assert!(has("spare is a screen output (tile line3)"), "{errs:?}");
    let none = helmsman_io::text::parse("in go bool box di:0\n").unwrap();
    assert!(
        l.check(&none.map)
            .unwrap_err()
            .iter()
            .any(|e| e.contains("no signal line1_state"))
    );
}

fn ident() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,10}"
}

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec("[A-Za-z0-9#:äöü!.()-]{1,6}", 1..5)
        .prop_map(|w| w.join(" "))
        .prop_filter("fits", |t| t.chars().count() <= MAX_TEXT)
}

fn layout() -> impl Strategy<Value = Layout> {
    let codes = prop::collection::btree_map(
        any::<i64>(),
        (0..6usize, text()).prop_map(|(c, text)| screen::State {
            colour: Colour::ALL[c],
            text,
        }),
        1..5,
    );
    (
        prop::option::of(text()),
        1..=8u8,
        prop::collection::btree_map(ident(), codes, 1..4),
        prop::collection::vec(
            (ident(), ident(), text(), prop::option::of((ident(), text()))),
            0..8,
        ),
    )
        .prop_map(|(title, cols, sets, tiles)| {
            let names: Vec<String> = sets.keys().cloned().collect();
            // One tile per cell of a single row, left to right.
            let tiles: Vec<Tile> = tiles
                .into_iter()
                .take(usize::from(cols))
                .enumerate()
                .map(|(k, (id, state, title, value))| Tile {
                    id: format!("{id}{k}"),
                    col: u8::try_from(k).unwrap(),
                    row: 0,
                    w: 1,
                    h: 1,
                    codes: names[k % names.len()].clone(),
                    state,
                    title,
                    value: value.map(|(signal, label)| screen::Value { signal, label }),
                })
                .collect();
            Layout {
                title,
                cols,
                rows: 1,
                sets,
                tiles,
            }
        })
}

proptest! {
    #[test]
    fn render_then_parse_is_identity(l in layout()) {
        let text = render(&l);
        let back = screen::parse(&text).map_err(|e| TestCaseError::fail(format!("{e:?}\n{text}")))?;
        prop_assert_eq!(back, l, "{}", text);
    }

    #[test]
    fn arbitrary_text_never_panics(src in "(?s).{0,400}") {
        let _ = screen::parse(&src);
        let _ = screen::parse(&format!("helmsman-screen 1\n{src}"));
    }

    #[test]
    fn arbitrary_lines_of_the_right_words_never_panic(
        lines in prop::collection::vec(
            (prop::sample::select(vec!["title", "grid", "code", "tile", "value", "x"]),
             prop::collection::vec("[-0-9a-z,#]{0,6}", 0..7)),
            0..20,
        )
    ) {
        let mut src = String::from("helmsman-screen 1\n");
        for (k, words) in lines {
            src.push_str(k);
            for w in words { src.push(' '); src.push_str(&w); }
            src.push('\n');
        }
        if let Ok(l) = screen::parse(&src) {
            prop_assert_eq!(screen::parse(&render(&l)).unwrap(), l);
        }
    }
}

fn module(name: &str, src: &str) -> Module {
    Module {
        name: name.into(),
        program: compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap_or_else(|d| panic!("{name}: {d:?}")),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

const LINE1: &str = "
PROGRAM line1
VAR_INPUT go : BOOL; END_VAR
VAR_OUTPUT line1_state : INT; line1_parts : DINT; END_VAR
VAR zero : INT; END_VAR
  line1_state := 1;
  line1_parts := line1_parts + 1;
  IF NOT go THEN line1_state := line1_state / zero; END_IF;
END_PROGRAM";

#[test]
fn a_screen_tile_has_one_writer() {
    let m = map();
    let table = SignalTable::new(&m.map, &m.safe).unwrap();
    let other = "PROGRAM other VAR_OUTPUT line1_state : INT; END_VAR line1_state := 2; END_PROGRAM";
    let err = Runtime::with_signals(0, 0, vec![module("line1", LINE1), module("other", other)], &table)
        .unwrap_err();
    assert_eq!(
        err,
        vec![ConfigError::TwoWriters {
            signal: "LINE1_STATE".into(),
            a: "line1".into(),
            b: "other".into()
        }]
    );
}

#[test]
fn a_faulted_or_held_modules_tile_goes_to_its_safe_state() {
    let m = map();
    let mut table = SignalTable::new(&m.map, &m.safe).unwrap();
    let mut rt = Runtime::with_signals(0, 0, vec![module("line1", LINE1)], &table).unwrap();
    let (go, state, parts) = (
        table.index("go").unwrap(),
        table.index("line1_state").unwrap(),
        table.index("line1_parts").unwrap(),
    );
    assert_eq!(
        table.command(state),
        Some(Value::Int(3)),
        "starts at its safe state"
    );
    assert_eq!(table.signal(state).map(|(_, t)| t), Some(Ty::Int));

    // Held: its input is missing, so it never computes.
    rt.tick_with(Duration::ZERO, &[], &mut table);
    assert_eq!(rt.status("line1"), Some(Status::Held));
    assert_eq!(table.command(state), Some(Value::Int(3)));

    table.set_input(go, Some(Value::Bool(true)));
    rt.tick_with(Duration::from_millis(10), &[], &mut table);
    rt.tick_with(Duration::from_millis(20), &[], &mut table);
    assert_eq!(table.command(state), Some(Value::Int(1)), "running");
    assert_eq!(table.command(parts), Some(Value::Int(2)));

    table.set_input(go, Some(Value::Bool(false)));
    let faults = rt.tick_with(Duration::from_millis(30), &[], &mut table);
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(faults[0].1.kind, FaultKind::DivisionByZero);
    assert_eq!(
        table.command(state),
        Some(Value::Int(3)),
        "the declared safe state"
    );
    assert_eq!(
        table.command(parts),
        Some(Value::Int(0)),
        "the default safe value"
    );
}

#[test]
fn nothing_polls_or_writes_a_screen_tile() {
    let m = map();
    let links = helmsman_plc::link::links(
        &m.map,
        |d| panic!("no device to open: {}", d.id),
        Box::new(helmsman_io::local::Channels::new()),
    )
    .unwrap();
    let names: Vec<&str> = links.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["box"], "only the box's own channel di:0");
    let _: BTreeMap<String, Value> = m.safe;
}

/// A tile is an output's point like any other: a map reload carries a tile
/// signal over when it stays on its tile and treats a move to another tile
/// as a rebind. (map-reload and andon-screen landed together; the signal
/// table's binding match had no arm for screen tiles.)
#[test]
fn a_tile_is_a_point_across_a_map_reload() {
    use std::collections::BTreeMap;

    use helmsman_io::text;
    use helmsman_plc::SignalTable;

    let table = |tile: &str| {
        let src = format!("out line_state int screen {tile} safe=3\n");
        let file = text::parse(&src).unwrap();
        SignalTable::new(&file.map, &BTreeMap::<String, helmsman_io::Value>::new()).unwrap()
    };
    let old = table("line1");
    let i = old.index("line_state").unwrap();
    assert_eq!(old.point(i), Some(("screen", "line1")));

    let mut same = table("line1");
    assert_eq!(same.carry_from(&old), vec![Some(i)], "same tile: carried over");
    let mut moved = table("line2");
    assert_eq!(moved.carry_from(&old), vec![None], "another tile: a rebind");
}
