//! The diagram arrangement file (`diagram.layout`): what it reads, what it
//! refuses, and that what it writes reads back the same.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use helmsman_project::layout::{self, HEADER, MAX_MOVES, MAX_TEXT, Move};
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
        ("helmsman-layout 2\n", "line 1: expected `helmsman-layout 1`"),
        (
            "move\tmain\t0\t1\t1\ta\tb\n",
            "line 1: expected `helmsman-layout 1`",
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
