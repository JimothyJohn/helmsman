//! The diagram arrangement file (`diagram.layout`): what it reads, what it
//! refuses, and that what it writes reads back the same.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use helmsman_project::layout::{
    self, FoldMove, FoldRef, HEADER, Layout, MAX_FOLDS, MAX_MOVES, MAX_TEXT, Move,
};
use helmsman_project::{FileKind, Files, LAYOUT, check_file, file_name, read_zip};
use proptest::prelude::*;

fn mv(pou: &str, label: &str, text: &str, k: usize, dx: f32, dy: f32) -> Move {
    Move {
        pou: pou.into(),
        label: label.into(),
        text: text.into(),
        k,
        dx,
        dy,
    }
}

#[test]
fn a_layout_reads_what_it_says() {
    let src = "# moved by alice\nhelmsman-layout 1\n\nmove\tmain\t0\t40\t-12.5\tTON\tt1(IN := run, PT := T#2s)\nmove\tLatch\t1\t0\t8\t:=\tq := (q OR set) AND NOT reset\n";
    assert_eq!(
        layout::parse(src).unwrap(),
        [
            mv("main", "TON", "t1(IN := run, PT := T#2s)", 0, 40.0, -12.5),
            mv("Latch", ":=", "q := (q OR set) AND NOT reset", 1, 0.0, 8.0),
        ]
    );
    assert_eq!(layout::parse("").unwrap(), [], "an empty file is no moves");
    assert_eq!(layout::parse(&format!("{HEADER}\n")).unwrap(), []);
    assert_eq!(file_name(LAYOUT), Some(FileKind::Layout));
}

#[test]
fn a_bad_layout_is_refused_whole_with_its_line() {
    for (src, want) in [
        (
            "helmsman-layout 3\n",
            "line 1: expected `helmsman-layout 1` or `helmsman-layout 2`",
        ),
        (
            "move\tmain\t0\t1\t1\ta\tb\n",
            "line 1: expected `helmsman-layout 1` or",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t0\t1\t1\ta\n",
            "line 2: expected move",
        ),
        (
            "helmsman-layout 1\nmove\t\t0\t1\t1\ta\tb\n",
            "line 2: no POU name",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t-1\t1\t1\ta\tb\n",
            "line 2: -1 is not a block number",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t0\tNaN\t1\ta\tb\n",
            "line 2: offset NaN",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t0\tinf\t1\ta\tb\n",
            "line 2: offset inf",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t0\t1\t2e6\ta\tb\n",
            "line 2: offset 2e6",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t0\t1\t1\ta\\x\tb\n",
            "line 2: unknown escape \\x",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t0\t1\t1\ta\tb\\\n",
            "line 2: a lone \\",
        ),
        (
            "helmsman-layout 1\nmove\tmain\t0\t1\t1\ta\tb\nhello\n",
            "line 3: expected move",
        ),
    ] {
        let e = layout::parse(src).unwrap_err();
        assert!(e.iter().any(|e| e.starts_with(want)), "{src:?}: {e:?}");
    }
    let long = format!("{HEADER}\nmove\tmain\t0\t1\t1\t{}\tb\n", "x".repeat(MAX_TEXT + 1));
    assert!(layout::parse(&long).unwrap_err()[0].contains("longer than"));
    let mut many = format!("{HEADER}\n");
    for k in 0..=MAX_MOVES {
        let _ = writeln!(many, "move\tmain\t{k}\t1\t1\ta\tb");
    }
    assert!(layout::parse(&many).unwrap_err()[0].contains(&format!("more than {MAX_MOVES} moves")));
}

/// A project holds its layout to the rules wherever it comes from: one
/// file saved, or a whole project (an upload's `.zip`).
#[test]
fn a_project_refuses_a_layout_that_does_not_read() {
    assert!(check_file(LAYOUT, b"helmsman-layout 1\nmove\tmain\t0\t1\t1\ta\tb\n").is_ok());
    assert!(
        check_file(LAYOUT, b"garbage")
            .unwrap_err()
            .contains("helmsman-layout 1")
    );
    assert!(
        check_file("main.st", b"garbage").is_ok(),
        "only the layout is parsed"
    );
    let mut files = Files(BTreeMap::from([
        ("main.st".to_string(), "PROGRAM main\nEND_PROGRAM\n".to_string()),
        (LAYOUT.to_string(), "garbage".to_string()),
    ]));
    assert!(files.check().unwrap_err()[0].starts_with("diagram.layout:"));
    let entries: Vec<(String, Vec<u8>)> = files
        .0
        .iter()
        .map(|(n, t)| (n.clone(), t.clone().into_bytes()))
        .collect();
    let zip = helmsman_project::zip::write(&entries).unwrap();
    assert!(read_zip(&zip).unwrap_err().contains("diagram.layout"));
    files.0.insert(
        LAYOUT.into(),
        layout::render(&[mv("main", "a", "b", 0, 1.0, 2.0)]),
    );
    let z = files.to_zip().unwrap();
    assert_eq!(read_zip(&z).unwrap(), files, "the layout travels in the .zip");
}

/// A layout that does not read is refused on the way in, but a project
/// already holding one still passes the rules a build holds it to.
#[test]
fn a_build_does_not_hold_the_layout_to_its_syntax() {
    let mut files = Files(BTreeMap::from([
        ("main.st".to_string(), "PROGRAM main\nEND_PROGRAM\n".to_string()),
        (LAYOUT.to_string(), "garbage".to_string()),
    ]));
    assert!(files.check().is_err());
    assert!(files.check_buildable().is_ok());
    files.0.insert(LAYOUT.into(), "a\0b".into());
    assert!(
        files.check_buildable().unwrap_err()[0].starts_with("diagram.layout:"),
        "still text within the limits"
    );
}

const V1_FILE: &str = "helmsman-layout 1\nmove\tmain\t0\t40\t-12.5\tTON\tt1\n";

fn folded() -> Layout {
    Layout {
        moves: vec![mv("main", "TON", "t1", 0, 40.0, -12.5)],
        opened: vec![FoldRef {
            pou: "main".into(),
            key: "latch:q".into(),
        }],
        collapsed: vec![
            FoldRef {
                pou: "main".into(),
                key: "sec\t1\\".into(),
            },
            FoldRef {
                pou: "Latch".into(),
                key: "arm:0".into(),
            },
        ],
        fold_moves: vec![FoldMove {
            pou: "main".into(),
            key: "latch:q".into(),
            dx: 3.5,
            dy: -8.0,
        }],
    }
}

#[test]
fn fold_state_round_trips_as_version_2() {
    let l = folded();
    let text = layout::render_all(&l);
    assert!(text.starts_with("helmsman-layout 2\n"), "{text:?}");
    assert_eq!(layout::parse_all(&text).unwrap(), l);
    assert_eq!(
        layout::parse(&text).unwrap(),
        l.moves,
        "old callers still get the moves"
    );
    assert!(check_file(LAYOUT, text.as_bytes()).is_ok());
}

#[test]
fn version_1_is_unchanged_and_written_byte_for_byte_without_fold_state() {
    let l = layout::parse_all(V1_FILE).unwrap();
    assert!(!l.has_folds());
    assert_eq!(l.moves.len(), 1);
    assert_eq!(layout::render_all(&l), V1_FILE);
    assert_eq!(layout::render(&l.moves), V1_FILE);
    assert_eq!(layout::render_all(&Layout::default()), "helmsman-layout 1\n");
    // A version 2 file with only moves is read the same.
    let v2 = V1_FILE.replace("layout 1", "layout 2");
    assert_eq!(layout::parse_all(&v2).unwrap(), l);
}

#[test]
fn fold_lines_need_version_2_and_unknown_lines_are_named() {
    for (src, want) in [
        (
            "helmsman-layout 1\nopen\tmain\tk\n",
            "line 2: `open` needs `helmsman-layout 2`",
        ),
        (
            "helmsman-layout 1\nfoldmove\tmain\tk\t1\t1\n",
            "line 2: `foldmove` needs",
        ),
        (
            "helmsman-layout 2\nhello there\n",
            "line 2: expected move, open, collapse or foldmove, got hello there",
        ),
        (
            "helmsman-layout 2\nmove\tmain\t0\t1\t1\ta\tb\nfold\tmain\tk\n",
            "line 3: expected move, open",
        ),
        (
            "helmsman-layout 2\nopen\tmain\n",
            "line 2: expected open or collapse",
        ),
        ("helmsman-layout 2\nopen\t\tk\n", "line 2: no POU name"),
        ("helmsman-layout 2\nopen\tmain\t\n", "line 2: no fold key"),
        (
            "helmsman-layout 2\nopen\tmain\tk\nopen\tmain\tk\n",
            "line 3: k is already listed under open",
        ),
        (
            "helmsman-layout 2\ncollapse\tmain\tk\ncollapse\tmain\tk\n",
            "line 3:",
        ),
        (
            "helmsman-layout 2\nfoldmove\tmain\tk\t1\t1\nfoldmove\tmain\tk\t2\t2\n",
            "line 3: k is already moved",
        ),
        (
            "helmsman-layout 2\nfoldmove\tmain\tk\tNaN\t1\n",
            "line 2: offset NaN",
        ),
        (
            "helmsman-layout 2\nfoldmove\tmain\tk\t1\t9e99\n",
            "line 2: offset",
        ),
        (
            "helmsman-layout 2\nfoldmove\tmain\tk\t1\n",
            "line 2: expected foldmove",
        ),
        ("helmsman-layout 2\nopen\tmain\tk\\x\n", "line 2: unknown escape"),
    ] {
        let e = layout::parse_all(src).unwrap_err();
        assert!(e.iter().any(|e| e.starts_with(want)), "{src:?}: {e:?}");
    }
}

#[test]
fn hostile_fold_files() {
    // Empty, CRLF, trailing whitespace on the header, comments before it.
    assert_eq!(layout::parse_all("").unwrap(), Layout::default());
    assert_eq!(layout::parse_all("# c\r\n\r\n").unwrap(), Layout::default());
    let crlf = "helmsman-layout 2\r\nopen\tmain\tk\r\ncollapse\tmain\tj\r\nfoldmove\tmain\tk\t1\t2\r\n";
    let l = layout::parse_all(crlf).unwrap();
    assert_eq!((l.opened.len(), l.collapsed.len(), l.fold_moves.len()), (1, 1, 1));
    assert_eq!(
        layout::parse_all("helmsman-layout 2  \nopen\tmain\tk\n")
            .unwrap()
            .opened
            .len(),
        1
    );
    // The same key may be open in one POU and another.
    assert!(layout::parse_all("helmsman-layout 2\nopen\ta\tk\nopen\tb\tk\n").is_ok());
    // A huge key and too many fold lines are refused whole.
    let long = format!("helmsman-layout 2\nopen\tmain\t{}\n", "x".repeat(MAX_TEXT + 1));
    assert!(layout::parse_all(&long).unwrap_err()[0].contains("longer than"));
    let mut many = String::from("helmsman-layout 2\n");
    for k in 0..=MAX_FOLDS {
        let _ = writeln!(many, "open\tmain\tk{k}");
    }
    assert!(layout::parse_all(&many).unwrap_err()[0].contains(&format!("more than {MAX_FOLDS} fold lines")));
    for token in ["inf", "-inf", "nan", "1e40", "0x10", "+", ""] {
        let src = format!("helmsman-layout 2\nfoldmove\tmain\tk\t{token}\t0\n");
        assert!(layout::parse_all(&src).is_err(), "{token}");
    }
}

fn any_text() -> impl Strategy<Value = String> {
    // Anything a block's label or source text may hold: tabs, line breaks
    // and backslashes included.
    "[a-zA-Z0-9 :=()#.,;\\\\\t\n\r\u{e9}\u{4e2d}-]{0,40}"
}

proptest! {
    #[test]
    fn what_render_writes_parse_reads_back(
        moves in prop::collection::vec(
            ("[A-Za-z_][A-Za-z0-9_]{0,20}", any_text(), any_text(), 0usize..1000, -1.0e5f32..1.0e5, -1.0e5f32..1.0e5)
                .prop_map(|(pou, label, text, k, dx, dy)| Move { pou, label, text, k, dx, dy }),
            0..40,
        )
    ) {
        let text = layout::render(&moves);
        prop_assert!(check_file(LAYOUT, text.as_bytes()).is_ok(), "{:?}", text);
        prop_assert_eq!(layout::parse(&text).unwrap(), moves);
    }

    #[test]
    fn parse_never_panics(src in "\\PC{0,400}") {
        let _ = layout::parse(&src);
        let _ = layout::parse(&format!("{HEADER}\n{src}"));
    }

    #[test]
    fn parse_never_panics_on_move_lines(fields in prop::collection::vec("[^\t\n]{0,12}", 0..9)) {
        let _ = layout::parse(&format!("{HEADER}\nmove\t{}\n", fields.join("\t")));
    }
}
