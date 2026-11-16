//! The design's Markdown reader (`crate::md_view`): what it reads, and that
//! hostile text neither panics nor hangs it. The recorded bottling line
//! must show no raw marker.

use super::*;

const LINE_1: &str = include_str!("fixtures/design/bottling-line-1.md");
const LINE_2: &str = include_str!("fixtures/design/bottling-line-2.md");

fn run(text: &str, bold: bool, italic: bool, code: bool) -> Run {
    Run {
        text: text.into(),
        bold,
        italic,
        code,
    }
}

#[test]
fn bold_italic_and_code_spans_are_read_and_their_marks_go() {
    assert_eq!(
        inline("**What the line is.** The `start_pb` input, *not* held."),
        [
            run("What the line is.", true, false, false),
            run(" The ", false, false, false),
            run("start_pb", false, false, true),
            run(" input, ", false, false, false),
            run("not", false, true, false),
            run(" held.", false, false, false),
        ]
    );
    // A snake_case name and a product of two numbers are not markup.
    assert_eq!(
        inline("a_b_c and 2 * 3 * 4"),
        [run("a_b_c and 2 * 3 * 4", false, false, false)]
    );
}

#[test]
fn a_marker_with_no_closer_is_the_text_it_was() {
    for s in ["**open", "*open", "`open", "a ** b", "**a* b", "``", "*"] {
        let t: String = inline(s).iter().map(|r| r.text.as_str()).collect();
        assert!(!t.is_empty() || s == "``", "{s}");
    }
    let t: String = inline("**open").iter().map(|r| r.text.as_str()).collect();
    assert_eq!(t, "**open");
}

#[test]
fn headings_lists_and_a_fence_are_blocks() {
    let b = parse(
        "## Machine\n\nOne line\ncontinues.\n\n- a\n  - b\n1. one\n2) two\n```st interface\nVAR\n  x : BOOL;\nEND_VAR\n```\nafter\n",
    );
    assert!(matches!(&b[0], Block::Heading(2, r) if r[0].text == "Machine"));
    assert_eq!(b[1], Block::Para(plain_runs("One line continues.")));
    assert!(matches!(&b[2], Block::Item { depth: 0, marker, .. } if marker == "•"));
    assert!(matches!(&b[3], Block::Item { depth: 1, .. }));
    assert!(matches!(&b[4], Block::Item { marker, .. } if marker == "1."));
    assert!(matches!(&b[5], Block::Item { marker, .. } if marker == "2."));
    assert_eq!(b[6], Block::Code("VAR\n  x : BOOL;\nEND_VAR".into()));
    assert_eq!(b[7], Block::Para(plain_runs("after")));
}

fn plain_runs(t: &str) -> Vec<Run> {
    vec![run(t, false, false, false)]
}

#[test]
fn a_pipe_table_is_a_grid_with_every_row_as_wide_as_its_head() {
    let b = parse("| A | B |\n|---|:-:|\n| 1 | 2 |\n| only |\n| x | y | z |\nnext");
    let Block::Table { head, rows } = &b[0] else {
        panic!("{b:?}")
    };
    assert_eq!(head.len(), 2);
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|r| r.len() == 2),
        "short rows padded, long ones cut: {rows:?}"
    );
    assert_eq!(b[1], Block::Para(plain_runs("next")));
    // A row of pipes with no separator under it is a paragraph.
    assert!(matches!(&parse("| a | b |\ntext")[0], Block::Para(_)));
}

#[test]
fn hostile_text_neither_panics_nor_hangs() {
    let long = "x".repeat(10_000);
    let many = "**a ".repeat(5_000);
    let stars = "*".repeat(20_000);
    let ticks = "`a".repeat(10_000);
    let deep: String = (0..20)
        .map(|k| format!("{}- level {k}\n", " ".repeat(k * 2)))
        .collect();
    let wide = format!(
        "|{}|\n|{}|\n|{}|\n",
        "a|".repeat(400),
        "-|".repeat(400),
        "b|".repeat(400)
    );
    for s in [
        long.as_str(),
        many.as_str(),
        stars.as_str(),
        ticks.as_str(),
        deep.as_str(),
        wide.as_str(),
        "```st\nnever closed\n  still code",
        "a\r\nb\r\n\r\n- c\rd",
        "\tindented\t|\t|\n\t|---|",
        "| a |\n| - |\n",
        "|\n|\n",
        "#\n# \n####### x\n",
        "-\n- \n1.\n12345. five digits\n",
        "",
        "\u{0}\u{202e}**x\u{202c}**",
    ] {
        let blocks = parse(s);
        // Drawn too: nothing in a frame panics.
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui, &blocks, 18.0, "t"));
        out.textures_delta.clear();
    }
    let b = parse(&deep);
    assert_eq!(b.len(), 20);
    assert!(
        b.iter()
            .all(|x| matches!(x, Block::Item { depth, .. } if *depth <= MAX_DEPTH))
    );
    let Block::Table { head, .. } = &parse(&wide)[0] else {
        panic!("a table")
    };
    assert_eq!(head.len(), MAX_COLS);
    assert!(matches!(&parse("```\nopen")[0], Block::Code(c) if c == "open"));
    assert_eq!(parse("a\r\nb"), parse("a\nb"), "CRLF is a line end");
}

#[test]
fn the_recorded_bottling_line_shows_no_raw_marker() {
    let text = format!("{LINE_1}{LINE_2}");
    let blocks = parse(&text);
    let tables = blocks.iter().filter(|b| matches!(b, Block::Table { .. })).count();
    assert!(tables > 10, "the state and signal tables are grids: {tables}");
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, Block::Code(c) if c.contains("VAR_INPUT")))
    );
    for t in texts(&blocks) {
        if t.contains("VAR_INPUT") {
            continue; // an interface block is code as written
        }
        assert!(!t.contains("**"), "{t}");
        assert!(!t.contains("|---"), "{t}");
        assert!(!t.contains('`'), "{t}");
    }
}
