//! The shell's keys, header and editor gutter: bugs found reading the
//! shell (2026-11-08), each with the regression that failed first.

use eframe::egui::{Event, Key, Modifiers, Vec2};

use super::Harness;
use crate::app::{App, Screen};

impl Harness {
    /// A press and a release of `key`.
    fn chord(&mut self, modifiers: Modifiers, key: Key) {
        for pressed in [true, false] {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }]);
        }
    }

    /// The simulation's open file.
    fn sim_text(&self) -> String {
        let p = &self.app.sim.as_ref().unwrap().proj;
        p.files[&p.current].clone()
    }

    /// Clicks into the ST editor (it takes the keyboard) and types `s`.
    fn type_in_editor(&mut self, s: &str) {
        let at = self.rect("st editor").left_top() + Vec2::new(60.0, 6.0);
        self.click(at);
        self.frame(vec![Event::Text(s.into())]);
    }
}

#[test]
fn ctrl_z_in_the_editor_undoes_one_step() {
    let mut h = Harness::new(App::sim());
    h.wait(0.3);
    h.type_in_editor("(*X*)");
    // Past the editor's undo grouping time, so each typing is a step.
    h.wait(1.5);
    let with_x = h.sim_text();
    h.frame(vec![Event::Text("(*Y*)".into())]);
    h.wait(1.5);
    assert_ne!(h.sim_text(), with_x);
    h.chord(Modifiers::COMMAND, Key::Z);
    h.wait(0.2);
    assert_eq!(h.sim_text(), with_x, "one Ctrl+Z undoes the last typing only");
    h.chord(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
    h.wait(0.2);
    assert!(h.sim_text().contains("(*Y*)"), "and Ctrl+Shift+Z puts it back");
}

#[test]
fn undo_after_a_file_switch_does_not_bring_the_other_files_text() {
    let mut h = Harness::new(App::sim());
    h.wait(0.3);
    h.type_in_editor("(*AAA*)");
    h.wait(1.5);
    h.app
        .sim
        .as_mut()
        .unwrap()
        .proj
        .add_file("other.st", "(* other *)\n".into());
    h.wait(0.5);
    let other = h.sim_text();
    assert!(!other.contains("AAA"));
    let at = h.rect("st editor").left_top() + Vec2::new(60.0, 6.0);
    h.click(at);
    h.chord(Modifiers::COMMAND, Key::Z);
    h.wait(0.3);
    assert_eq!(h.sim_text(), other, "the other file's history stays with it");
}

#[test]
fn save_and_deploy_do_nothing_on_the_monitor() {
    let mut h = Harness::new(App::sim());
    h.wait(0.3);
    h.type_in_editor("!");
    h.wait(0.6);
    assert!(h.has("dirty"));
    h.app.screen = Screen::Monitor;
    h.wait(0.2);
    h.app.sim.as_mut().unwrap().notice = None;
    h.chord(Modifiers::COMMAND, Key::S);
    h.chord(Modifiers::COMMAND, Key::D);
    h.wait(0.2);
    assert!(h.app.sim.as_ref().unwrap().proj.dirty(), "Ctrl+S did not save");
    assert!(
        h.app.sim.as_ref().unwrap().notice.is_none(),
        "Ctrl+D did not deploy the hidden project"
    );
    h.app.screen = Screen::Editor;
    h.wait(0.2);
    h.chord(Modifiers::COMMAND, Key::D);
    h.wait(0.2);
    assert!(
        h.app.sim.as_ref().unwrap().notice.is_some(),
        "on the Editor it deploys"
    );
}

/// Every header control's rect, the header's own aside.
fn header_hits(h: &Harness) -> Vec<(String, eframe::egui::Rect)> {
    let header = h.rect("header");
    h.app
        .chrome
        .hits
        .iter()
        .filter(|(k, r)| k != "header" && header.contains_rect(*r) && r.height() < header.height() + 1.0)
        .cloned()
        .collect()
}

/// At a phone's width the right group (title, screens) was drawn over the
/// left (mark, menus): nothing truncated or collapsed.
#[test]
fn no_two_header_controls_overlap_at_a_phones_width() {
    for width in [390.0, 360.0, 320.0] {
        for screen in [Screen::Editor, Screen::Monitor] {
            let mut h = Harness::new(App::sim());
            h.size = Vec2::new(width, 780.0);
            h.app.screen = screen;
            h.wait(0.5);
            let hits = header_hits(&h);
            assert!(hits.len() >= 3, "{hits:?}");
            for (i, (a, ra)) in hits.iter().enumerate() {
                for (b, rb) in &hits[i + 1..] {
                    assert!(
                        !ra.shrink(0.5).intersects(rb.shrink(0.5)),
                        "{a} {ra:?} overlaps {b} {rb:?} at {width} pt on {screen:?}"
                    );
                }
                assert!(ra.right() <= width + 0.5, "{a} {ra:?} is off a {width} pt screen");
            }
            assert!(h.has("bar menu"), "the four menus are one button on a phone");
            assert!(!h.has("bar file"));
        }
    }
}

/// The galleys of the last frame whose text satisfies `f`.
fn galleys(h: &Harness, f: impl Fn(&str) -> bool) -> Vec<std::sync::Arc<eframe::egui::Galley>> {
    fn walk(
        shape: &eframe::egui::Shape,
        f: &impl Fn(&str) -> bool,
        out: &mut Vec<std::sync::Arc<eframe::egui::Galley>>,
    ) {
        match shape {
            eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, f, out)),
            eframe::egui::Shape::Text(t) if f(&t.galley.job.text) => out.push(t.galley.clone()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, &f, &mut out);
    }
    out
}

/// The line numbers drifted one row early after a line that wrapped: the
/// gutter was laid out at a width the editor does not wrap at.
#[test]
fn the_gutter_has_a_row_for_every_row_the_editor_wraps_to() {
    let mut h = Harness::new(App::sim());
    let mut text = String::from("PROGRAM p\n");
    for n in 0..60 {
        text.push_str(&format!("(* {} *)\n", "w".repeat(14 + n)));
    }
    text.push_str("END_PROGRAM\n");
    {
        let p = &mut h.app.sim.as_mut().unwrap().proj;
        let cur = p.current.clone();
        p.files.insert(cur, text.clone());
    }
    h.wait(0.5);
    let editor = galleys(&h, |t| t.starts_with("PROGRAM p"));
    let gutter = galleys(&h, |t| t.starts_with("  1\n"));
    assert_eq!((editor.len(), gutter.len()), (1, 1));
    let (e, g) = (&editor[0], &gutter[0]);
    assert!(e.rows.len() > text.lines().count(), "some lines wrap in the pane");
    assert!(g.rows.len() >= e.rows.len(), "a gutter row for every editor row");
    for (k, row) in e.rows.iter().enumerate() {
        let numbered = k == 0 || e.rows[k - 1].ends_with_newline;
        let shown = g.rows[k].glyphs.iter().any(|c| c.chr.is_ascii_digit());
        assert_eq!(shown, numbered, "row {k}: a number only on a line's first row");
        assert!(
            (g.rows[k].pos.y - row.pos.y).abs() < 0.5,
            "row {k} is at {} in the gutter, {} in the editor",
            g.rows[k].pos.y,
            row.pos.y
        );
    }
}
