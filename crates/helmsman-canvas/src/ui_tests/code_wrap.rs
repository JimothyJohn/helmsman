//! Word wrap in the Code tab continues a long line under its own
//! indentation plus two columns (hanging indent), with the line number on
//! the first row only.

use eframe::egui::{self, FontId, RawInput};

use super::Harness;
use crate::app::App;
use crate::editor::{self, Lines};
use crate::theme::Theme;

const NO: Lines<'static> = Lines {
    errors: &[],
    warnings: &[],
    hints: &[],
    folded: &[],
};

/// The galley `hang` gives `text` wrapped at `wrap`, and one glyph's width.
fn laid(text: &str, wrap: f32) -> (std::sync::Arc<egui::Galley>, f32, String) {
    let ctx = egui::Context::default();
    let font = FontId::monospace(16.0);
    let mut out = None;
    for _ in 0..2 {
        let mut o = ctx.run_ui(RawInput::default(), |ui| {
            let digit = ui
                .painter()
                .layout_no_wrap("0".into(), font.clone(), egui::Color32::WHITE)
                .size()
                .x;
            let job = editor::highlight(text, &NO, None, &[], &font, wrap, &Theme::dark());
            let jt = job.text.clone();
            out = Some((editor::hang(ui, job, digit), digit, jt));
        });
        // No renderer here to upload textures to.
        o.textures_delta.clear();
    }
    out.unwrap()
}

/// x of the first glyph of row `i`.
fn row_x(g: &egui::Galley, i: usize) -> f32 {
    let r = &g.rows[i];
    r.pos.x + r.glyphs.first().map_or(0.0, |c| c.pos.x)
}

const LONG: &str = "alpha_one := beta_two + gamma_three + delta_four + epsilon_five + zeta_six + eta_seven;";

#[test]
fn a_wrapped_lines_continuation_starts_right_of_the_first_row() {
    let (g, _, _) = laid(LONG, 236.0 - 40.0);
    assert!(g.rows.len() >= 3, "the line wraps: {} rows", g.rows.len());
    let first = row_x(&g, 0);
    for i in 1..g.rows.len() {
        assert!(row_x(&g, i) > first + 1.0, "row {i} is indented");
    }
}

#[test]
fn the_continuation_is_two_columns_in_from_the_lines_own_indent() {
    let wrap = 300.0;
    let (g0, digit, _) = laid(LONG, wrap);
    let (g4, _, _) = laid(&format!("    {LONG}"), wrap);
    let d0 = row_x(&g0, 1) - row_x(&g0, 0);
    let d4 = row_x(&g4, 1) - row_x(&g4, 0);
    assert!((d0 - 2.0 * digit).abs() < 1.0, "unindented: {d0} vs {}", 2.0 * digit);
    assert!((d4 - 6.0 * digit).abs() < 1.0, "four spaces in: {d4} vs {}", 6.0 * digit);
    assert!(d4 > d0);
}

#[test]
fn short_lines_keep_their_text_at_column_zero() {
    let (g, _, _) = laid("a := 1;\nb := 2;\n", 300.0);
    for i in 0..g.rows.len() {
        let r = &g.rows[i];
        if r.glyphs.is_empty() {
            continue;
        }
        assert!(row_x(&g, i).abs() < 0.5, "row {i} starts at x {}", row_x(&g, i));
    }
}

#[test]
fn the_galley_holds_the_whole_text_and_rows_mark_the_line_ends() {
    let text = format!("a := 1;\n{LONG}\n    b := 2;\n\nc;");
    let (g, _, jt) = laid(&text, 220.0);
    assert_eq!(g.job.text, jt, "one galley of the whole buffer");
    assert_eq!(jt, text);
    let ends = g.rows.iter().filter(|r| r.ends_with_newline).count();
    assert_eq!(ends, text.matches('\n').count(), "a row ends each line");
    // A wrapped line's rows other than its last do not end with one.
    assert!(g.rows.len() > ends + 1, "the long line has more than one row");
}

#[test]
fn no_row_is_wider_than_the_wrap_width() {
    for wrap in [120.0_f32, 196.0, 280.0, 400.0] {
        let (g, _, _) = laid(&format!("{LONG}\n    {LONG}\n"), wrap);
        for (i, r) in g.rows.iter().enumerate() {
            let right = r.pos.x + r.glyphs.last().map_or(0.0, |c| c.pos.x + c.advance_width);
            assert!(right <= wrap + 0.5, "wrap {wrap}: row {i} reaches {right}");
        }
    }
}

#[test]
fn a_word_longer_than_the_width_does_not_hang_or_panic() {
    let long = "x".repeat(500);
    let (g, _, jt) = laid(&format!("{long}\n"), 150.0);
    assert_eq!(g.job.text, jt);
    assert!(g.rows.len() > 2);
}

#[test]
fn a_long_line_in_the_code_tab_lays_out_at_the_default_width() {
    let mut h = Harness::new(App::sim());
    h.app.chrome.show_code();
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("mine", None);
        v.proj.files.insert("main.st".into(), format!("PROGRAM main\n  {LONG}\nEND_PROGRAM\n"));
        v.proj.save_as("long", None);
        v.open_project("mine");
    }
    h.wait(0.4);
    assert!(h.app.chrome.wrap, "wrap is on by default");
    let left = h.rect("left");
    let ed = h.rect("st editor");
    assert!(ed.right() <= left.right() + 1.0, "the editor stays in the pane");
    assert!(left.width() < crate::editor::LEFT_W + 60.0, "the pane did not widen: {}", left.width());
    assert!(h.drew("end_program") || h.drew("END_PROGRAM"));
}
