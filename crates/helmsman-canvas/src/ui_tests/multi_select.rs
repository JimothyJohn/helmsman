//! Selecting blocks of the diagram (Nick, 2026-11-16): a rubber band on
//! empty space, Shift or Ctrl and a click; the selected blocks are the
//! Agent's focus. Empty space no longer pans: Space and a drag, or the
//! middle button, do. Driven through egui on the simulation.

use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2};
use helmsman_diagram::{NodeId, NodeKind};

use super::{Harness, sim_view};
use crate::chat_about::MAX_CHIPS;
use crate::editor::Pane;

const SRC: &str = "\
PROGRAM board
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  motor AT %QX0.0 : BOOL;
  lamp AT %QX0.1 : BOOL;
  horn AT %QX0.2 : BOOL;
END_VAR
motor := start AND NOT stop;
lamp := motor OR stop;
horn := start AND stop;
END_PROGRAM
";

/// A latch: its blocks are drawn as one fold.
const LATCH: &str = "\
PROGRAM latch
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  motor AT %QX0.0 : BOOL;
END_VAR
motor := (start OR motor) AND NOT stop;
END_PROGRAM
";

/// Twelve statements, each of a few blocks.
fn many() -> String {
    use std::fmt::Write;
    let mut s = String::from("PROGRAM many\nVAR\n");
    for k in 0..12 {
        let _ = write!(s, "  a{k} : BOOL;\n  b{k} : BOOL;\n  o{k} : BOOL;\n");
    }
    s.push_str("END_VAR\n");
    for k in 0..12 {
        let _ = writeln!(s, "o{k} := a{k} AND NOT b{k};");
    }
    s.push_str("END_PROGRAM\n");
    s
}

impl Harness {
    /// The simulation with `src` as its one program file, deployed, the
    /// Code tab showing.
    fn selecting(src: &str) -> Self {
        let mut h = Self::code();
        {
            let v = h.app.sim.as_mut().unwrap();
            v.chat.receive(crate::claude::Incoming::Proxy(true));
            v.proj.create("multi", None);
            v.proj.files.insert("main.st".into(), src.into());
            v.proj.save();
            v.open_project("multi");
        }
        h.wait(0.4);
        h.wait(1.0);
        h
    }

    fn selected(&self) -> Vec<NodeId> {
        sim_view(self).selection().to_vec()
    }

    fn area(&self) -> Rect {
        self.app.chrome.diagram
    }

    /// Where block `n` is on screen.
    fn on_screen(&self, n: NodeId) -> Rect {
        sim_view(self).block_on_screen(n).expect("drawn")
    }

    fn write_of(&self, name: &str) -> NodeId {
        sim_view(self)
            .diagram()
            .nodes
            .iter()
            .find(|n| matches!(&n.kind, NodeKind::Write { path } if path.eq_ignore_ascii_case(name)))
            .unwrap_or_else(|| panic!("no block writes {name}"))
            .id
    }

    /// Every block of the diagram, by screen rectangle.
    fn all_blocks(&self) -> Vec<(NodeId, Rect)> {
        sim_view(self)
            .diagram()
            .nodes
            .iter()
            .map(|n| (n.id, self.on_screen(n.id)))
            .collect()
    }

    /// The blocks a band from `a` to `b` touches, by geometry alone.
    fn touched(&self, a: Pos2, b: Pos2) -> Vec<NodeId> {
        let band = Rect::from_two_pos(a, b).intersect(self.area());
        self.all_blocks()
            .into_iter()
            .filter(|(_, r)| r.intersects(band))
            .map(|(n, _)| n)
            .collect()
    }

    /// An empty corner of the diagram, below and left of the blocks.
    fn corner(&self) -> Pos2 {
        let a = self.area();
        let p = Pos2::new(a.left() + 10.0, a.bottom() - 10.0);
        assert!(
            self.all_blocks().iter().all(|(_, r)| !r.expand(30.0).contains(p)),
            "the corner is empty"
        );
        p
    }

    /// An empty point low on the diagram's right, clear of the corner's
    /// pan, zoom and fit glyphs (four 44 pt glyphs in the bottom-right
    /// corner since the lamp fixture went, 2026-11-26) and of every block.
    fn far_corner(&self) -> Pos2 {
        let a = self.area();
        let p = Pos2::new(a.right() - 260.0, a.bottom() - 8.0);
        assert!(
            self.all_blocks().iter().all(|(_, r)| !r.expand(30.0).contains(p)),
            "the far corner is empty"
        );
        p
    }

    /// A press at `from`, moved in steps to `to`, released there, with
    /// `button` and the modifiers held.
    fn pull(&mut self, from: Pos2, to: Pos2, button: PointerButton, held: Modifiers) {
        self.frame(vec![Event::ModifiersChanged(held)]);
        let ev = |at: Pos2, pressed: bool| Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: held,
        };
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![ev(from, true)]);
        for k in 1..=8 {
            #[allow(clippy::cast_precision_loss)]
            self.frame(vec![Event::PointerMoved(from.lerp(to, k as f32 / 8.0))]);
        }
        self.frame(vec![ev(to, false)]);
        self.frame(vec![]);
        self.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    }

    fn band(&mut self, from: Pos2, to: Pos2) {
        self.pull(from, to, PointerButton::Primary, Modifiers::NONE);
    }

    /// A click with `held` down, a second of page time after the last.
    fn click_with(&mut self, at: Pos2, held: Modifiers) {
        self.wait(1.0);
        self.frame(vec![Event::ModifiersChanged(held)]);
        self.click(at);
        self.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    }

    fn space(&mut self, down: bool) {
        self.frame(vec![Event::Key {
            key: Key::Space,
            physical_key: None,
            pressed: down,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
    }

    fn offset(&self) -> Vec2 {
        let v = sim_view(self);
        v.cams[v.scope].offset
    }

    /// Selects the blocks of the first `k` writes with a band over all.
    fn select_all(&mut self) {
        let a = self.area();
        let (from, to) = (self.corner(), Pos2::new(a.right() - 4.0, a.top() + 60.0));
        self.band(from, to);
    }
}

#[test]
fn a_band_selects_exactly_the_blocks_it_touches_whichever_way_it_is_drawn() {
    let mut h = Harness::selecting(SRC);
    let corner = h.corner();
    let all = h.all_blocks().len();
    // Up and to the right of the corner, over some of the blocks.
    let mid = h.on_screen(h.write_of("motor")).center();
    let want = h.touched(corner, mid);
    assert!(
        !want.is_empty() && want.len() < all,
        "the band discriminates: {want:?} of {all}"
    );
    h.band(corner, mid);
    assert_eq!(h.selected(), want);
    // The same rectangle drawn the other way round: from the far
    // corner back to the empty one needs an empty start, so start at
    // another empty corner and end on the opposite one.
    let far = h.far_corner();
    let want = h.touched(far, mid);
    assert!(!want.is_empty(), "the second band touches something");
    h.wait(1.0);
    h.band(far, mid);
    assert_eq!(h.selected(), want, "right to left and bottom to top replaces");
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.focus.more, 0);
}

#[test]
fn a_band_over_everything_selects_every_block_and_marks_them() {
    let mut h = Harness::selecting(SRC);
    h.select_all();
    let all: Vec<NodeId> = h.all_blocks().into_iter().map(|(n, _)| n).collect();
    assert_eq!(h.selected(), all);
    // Selected blocks are outlined in the selection's colour.
    let blue = crate::st_view::brand::SELECT;
    let drew = h.shapes.iter().any(|c| {
        fn walk(s: &eframe::egui::Shape, blue: eframe::egui::Color32) -> bool {
            match s {
                eframe::egui::Shape::Vec(v) => v.iter().any(|s| walk(s, blue)),
                eframe::egui::Shape::Rect(r) => r.stroke.color == blue,
                _ => false,
            }
        }
        walk(&c.shape, blue)
    });
    assert!(drew, "an outline in the selection colour");
}

#[test]
fn a_drag_of_no_length_is_a_click_and_selects_nothing() {
    let mut h = Harness::selecting(SRC);
    let corner = h.corner();
    h.band(corner, corner);
    assert!(h.selected().is_empty(), "nothing is selected");
    // And it clears what was selected, as a click does.
    h.select_all();
    assert!(!h.selected().is_empty(), "something is selected");
    h.wait(1.0);
    h.band(corner, corner);
    assert!(h.selected().is_empty(), "a click on empty space clears");
}

#[test]
fn a_band_that_ends_outside_the_diagram_selects_what_it_touched_inside() {
    let mut h = Harness::selecting(SRC);
    let a = h.area();
    let corner = h.corner();
    // Out through the diagram's right edge, onto a panel, and above the
    // window.
    for out in [
        Pos2::new(a.right() + 120.0, a.center().y),
        Pos2::new(a.center().x, -50.0),
    ] {
        h.wait(1.0);
        let want = h.touched(corner, out);
        h.band(corner, out);
        assert_eq!(h.selected(), want, "ending at {out:?}");
        assert!(sim_view(&h).selection().len() <= h.all_blocks().len());
    }
}

#[test]
fn shift_and_ctrl_add_to_the_selection_with_a_band_and_toggle_with_a_click() {
    let mut h = Harness::selecting(SRC);
    let corner = h.corner();
    let motor = h.write_of("motor");
    let horn = h.write_of("horn");
    // A band over the motor's block only: from the corner up to it.
    let r = h.on_screen(motor);
    let first = h.touched(corner, r.center());
    h.band(corner, r.center());
    assert_eq!(h.selected(), first);
    // A shift band elsewhere adds.
    let far = h.far_corner();
    let more = h.touched(far, h.on_screen(horn).center());
    h.wait(1.0);
    h.pull(
        far,
        h.on_screen(horn).center(),
        PointerButton::Primary,
        Modifiers::SHIFT,
    );
    let mut want: Vec<NodeId> = first.iter().chain(&more).copied().collect();
    want.sort();
    want.dedup();
    assert_eq!(h.selected(), want, "Shift adds");
    // Ctrl adds too.
    h.click_with(corner, Modifiers::NONE);
    assert!(h.selected().is_empty(), "nothing is selected");
    h.band(corner, r.center());
    h.wait(1.0);
    h.pull(
        far,
        h.on_screen(horn).center(),
        PointerButton::Primary,
        Modifiers::COMMAND,
    );
    assert_eq!(h.selected(), want, "Ctrl adds");

    // Clicks: Shift on a block toggles it, Ctrl too, and neither picks it.
    h.click_with(corner, Modifiers::NONE);
    let before_pick = sim_view(&h).pick;
    let at = h.on_screen(horn).center();
    h.click_with(at, Modifiers::SHIFT);
    assert_eq!(h.selected(), vec![horn]);
    h.click_with(h.on_screen(motor).center(), Modifiers::COMMAND);
    assert_eq!(
        h.selected(),
        vec![motor, horn]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );
    h.click_with(at, Modifiers::SHIFT);
    assert_eq!(h.selected(), vec![motor], "a second Shift click takes it out");
    assert_eq!(sim_view(&h).pick, before_pick, "the clicks pick nothing");
    // A Shift click on empty space changes nothing.
    h.click_with(corner, Modifiers::SHIFT);
    assert_eq!(h.selected(), vec![motor]);
}

#[test]
fn a_click_on_empty_space_and_escape_clear_the_selection() {
    let mut h = Harness::selecting(SRC);
    let corner = h.corner();
    h.select_all();
    assert!(!h.selected().is_empty(), "something is selected");
    h.click_with(corner, Modifiers::NONE);
    assert!(h.selected().is_empty(), "a click off clears");
    h.wait(1.0);
    h.select_all();
    h.key(Modifiers::NONE, Key::Escape);
    assert!(h.selected().is_empty(), "Escape clears");
    // Nothing selected: Escape does nothing and breaks nothing.
    h.key(Modifiers::NONE, Key::Escape);
    assert!(h.selected().is_empty(), "nothing is selected");
}

#[test]
fn a_plain_click_on_a_block_selects_that_block_alone() {
    let mut h = Harness::selecting(SRC);
    let motor = h.write_of("motor");
    // With nothing selected.
    h.click_with(h.on_screen(motor).center(), Modifiers::NONE);
    assert_eq!(h.selected(), vec![motor], "the block clicked");
    // With many selected: only the one clicked stays.
    h.select_all();
    assert!(h.selected().len() > 1);
    h.click_with(h.on_screen(motor).center(), Modifiers::NONE);
    assert_eq!(h.selected(), vec![motor], "and no other");
    // Another block: the selection moves to it.
    let other = h.write_of("horn");
    h.click_with(h.on_screen(other).center(), Modifiers::NONE);
    assert_eq!(h.selected(), vec![other]);
}

#[test]
fn a_click_off_the_diagram_lets_the_selection_go_but_not_one_in_the_agent_tab() {
    let mut h = Harness::selecting(SRC);
    h.app.chrome.dock(Pane::Claude, true);
    h.wait(0.3);
    h.press(&Pane::Claude.hit());
    h.wait(0.3);
    h.select_all();
    let sel = h.selected();
    assert!(!sel.is_empty(), "something is selected");
    // The Agent tab's own button, then the instruction box in it: the
    // selection is what the instruction is about.
    h.wait(1.0);
    h.press(&Pane::Claude.hit());
    h.wait(1.0);
    assert_eq!(h.selected(), sel, "the Agent tab's button keeps it");
    h.press("agent instruction");
    h.wait(1.0);
    assert_eq!(h.selected(), sel, "a click in the Agent tab keeps it");
    // The menu bar, off the diagram and off the Agent tab: gone.
    let bar = h.rect("bar menu").center();
    assert!(!h.area().contains(bar));
    h.click(bar);
    h.wait(0.3);
    assert!(h.selected().is_empty(), "a click anywhere else lets it go");
    // A press off the diagram that is dragged, not clicked, changes nothing.
    h.wait(1.0);
    h.select_all();
    let sel = h.selected();
    let from = h.rect("bar menu").center();
    h.pull(
        from,
        from + Vec2::new(80.0, 0.0),
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert_eq!(h.selected(), sel, "a drag is not a click");
}

#[test]
fn a_band_shows_what_it_touches_while_the_button_is_down_and_selects_on_release() {
    let mut h = Harness::selecting(SRC);
    let motor = h.write_of("motor");
    h.click_with(h.on_screen(motor).center(), Modifiers::NONE);
    assert_eq!(h.selected(), vec![motor]);
    h.wait(1.0);
    let corner = h.corner();
    let a = h.area();
    let to = Pos2::new(a.right() - 4.0, a.top() + 60.0);
    let want = h.touched(corner, to);
    assert!(want.len() > 1);
    let press = |at: Pos2, pressed: bool| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.frame(vec![Event::PointerMoved(corner)]);
    h.frame(vec![press(corner, true)]);
    h.frame(vec![]);
    assert_eq!(h.selected(), vec![motor], "a press alone changes nothing");
    assert_eq!(sim_view(&h).drew_selected, vec![motor], "and the highlight stays");
    for k in 1..=8 {
        #[allow(clippy::cast_precision_loss)]
        h.frame(vec![Event::PointerMoved(corner.lerp(to, k as f32 / 8.0))]);
    }
    h.frame(vec![]);
    assert_eq!(
        h.selected(),
        vec![motor],
        "held: the selection is not changed yet"
    );
    let mut drew = sim_view(&h).drew_selected.clone();
    drew.sort_unstable();
    assert_eq!(drew, want, "held: what the band touches is drawn selected");
    h.frame(vec![press(to, false)]);
    h.frame(vec![]);
    assert_eq!(h.selected(), want, "released: that is the selection");
}

#[test]
fn dragging_a_block_keeps_the_selection_it_had() {
    let mut h = Harness::selecting(SRC);
    h.select_all();
    let sel = h.selected();
    let motor = h.write_of("motor");
    let at = h.on_screen(motor);
    let from = Pos2::new(at.center().x, at.top() + 6.0);
    h.wait(1.0);
    h.pull(
        from,
        from + Vec2::new(40.0, 30.0),
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert_eq!(
        h.selected(),
        sel,
        "the click was not released on it: nothing changes"
    );
    assert_eq!(sim_view(&h).drew_selected.len(), sel.len());
}

#[test]
fn escape_belongs_to_a_menu_or_the_find_box_first() {
    let mut h = Harness::selecting(SRC);
    h.select_all();
    let sel = h.selected();
    // A right click opens the block's menu; Escape shuts it and leaves
    // the selection.
    let motor = h.write_of("motor");
    h.wait(1.0);
    h.right_click(h.on_screen(motor).center());
    assert!(h.app.sim.as_ref().unwrap().dmenu.is_open());
    h.key(Modifiers::NONE, Key::Escape);
    assert!(
        !h.app.sim.as_ref().unwrap().dmenu.is_open(),
        "Escape shuts the menu"
    );
    assert_eq!(h.selected(), sel, "and leaves the selection");
    h.key(Modifiers::NONE, Key::Escape);
    assert!(h.selected().is_empty(), "the next one clears it");

    // The find box with the keyboard: Escape closes it first. (The Code
    // tab is opened before the blocks are selected: a click on its
    // button is a click off the diagram.)
    h.wait(1.0);
    h.press("pane code");
    h.wait(0.3);
    h.select_all();
    let sel = h.selected();
    h.key(Modifiers::COMMAND, Key::F);
    h.frame(vec![]);
    assert!(h.app.chrome.find.is_some(), "the find box opens");
    h.key(Modifiers::NONE, Key::Escape);
    assert!(h.app.chrome.find.is_none(), "Escape closes the find box");
    assert_eq!(h.selected(), sel, "the selection stays");
}

#[test]
fn a_drag_on_a_block_still_moves_it_and_selects_nothing() {
    let mut h = Harness::selecting(SRC);
    let motor = h.write_of("motor");
    let at = h.on_screen(motor);
    let from = Pos2::new(at.center().x, at.top() + 6.0);
    let before = sim_view(&h).diagram().nodes[motor.0 as usize].rect;
    h.pull(
        from,
        from + Vec2::new(0.0, 40.0),
        PointerButton::Primary,
        Modifiers::NONE,
    );
    let after = sim_view(&h).diagram().nodes[motor.0 as usize].rect;
    assert!(after.y > before.y + 10.0, "{before:?} -> {after:?}");
    assert!(h.selected().is_empty(), "nothing is selected");
}

#[test]
fn space_and_a_drag_or_the_middle_button_pan_the_view_and_select_nothing() {
    let mut h = Harness::selecting(SRC);
    let corner = h.corner();
    let by = Vec2::new(40.0, -30.0);
    let start = h.offset();
    // Plain drag on empty space: no pan.
    h.band(corner, corner + Vec2::new(30.0, -20.0));
    assert_eq!(h.offset(), start, "a plain drag no longer pans");
    // Middle button.
    h.wait(1.0);
    h.pull(corner, corner + by, PointerButton::Middle, Modifiers::NONE);
    let moved = h.offset() - start;
    assert!(
        moved.x > 20.0 && moved.y < -15.0,
        "middle drag moved the view by {moved:?}"
    );
    assert!(h.selected().is_empty(), "nothing is selected");
    // Space and a drag, even from over a block (nothing is moved).
    let motor = h.write_of("motor");
    let block = sim_view(&h).diagram().nodes[motor.0 as usize].rect;
    let from = h.on_screen(motor).center();
    let mid = h.offset();
    h.wait(1.0);
    h.space(true);
    h.pull(from, from + by, PointerButton::Primary, Modifiers::NONE);
    h.space(false);
    let moved = h.offset() - mid;
    assert!(
        moved.x > 20.0 && moved.y < -15.0,
        "Space and a drag moved the view by {moved:?}"
    );
    assert!(h.selected().is_empty(), "no band");
    assert_eq!(
        sim_view(&h).diagram().nodes[motor.0 as usize].rect,
        block,
        "the block stayed"
    );
}

#[test]
fn the_wheel_still_zooms_and_the_keys_still_pan_and_neither_touches_the_selection() {
    let mut h = Harness::selecting(SRC);
    h.select_all();
    let sel = h.selected();
    let zoom = sim_view(&h).cams[0].zoom;
    let at = h.area().center();
    h.frame(vec![
        Event::PointerMoved(at),
        Event::MouseWheel {
            unit: eframe::egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 120.0),
            modifiers: Modifiers::NONE,
            phase: eframe::egui::TouchPhase::Move,
            source: eframe::egui::MouseWheelSource::Wheel,
        },
    ]);
    h.wait(0.3);
    assert!(sim_view(&h).cams[0].zoom > zoom);
    let from = h.offset();
    h.frame(vec![Event::Key {
        key: Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    h.wait(0.2);
    assert_ne!(h.offset(), from);
    assert_eq!(h.selected(), sel);
}

#[test]
fn selected_blocks_are_chips_on_the_agent_tab_and_their_x_deselects() {
    let mut h = Harness::selecting(SRC);
    h.app.chrome.show(Pane::Claude);
    h.wait(0.3);
    let motor = h.write_of("motor");
    let lamp = h.write_of("lamp");
    h.wait(1.0);
    h.click_with(h.on_screen(motor).center(), Modifiers::SHIFT);
    h.click_with(h.on_screen(lamp).center(), Modifiers::SHIFT);
    h.wait(0.2);
    let i = &h.app.sim.as_ref().unwrap().instruct;
    assert!(i.chips.is_empty(), "nothing is collected");
    let names: Vec<&str> = i.focus.chips.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["motor", "lamp"], "{names:?}");
    assert!(h.has("chat chip 0") && h.has("chat chip 1"));
    // The x of the first deselects it.
    h.press("chat chip 0 remove");
    h.wait(0.2);
    assert_eq!(h.selected(), vec![lamp]);
    let i = &h.app.sim.as_ref().unwrap().instruct;
    assert_eq!(i.focus.chips.len(), 1);
    assert_eq!(i.focus.chips[0].name, "lamp");
    // Deselecting any other way removes the chip with no copy left.
    h.key(Modifiers::NONE, Key::Escape);
    h.wait(0.2);
    assert!(
        h.app.sim.as_ref().unwrap().instruct.focus.chips.is_empty(),
        "no chips"
    );
    assert!(!h.has("chat chip 0"));
}

#[test]
fn selecting_does_not_switch_tabs_or_send() {
    let mut h = Harness::selecting(SRC);
    assert!(!h.app.chrome.showing(Pane::Claude));
    let sent = h.app.sim.as_ref().unwrap().link.sent.len();
    h.select_all();
    assert!(!h.app.chrome.showing(Pane::Claude), "the Agent tab did not open");
    assert_eq!(h.app.sim.as_ref().unwrap().link.sent.len(), sent);
}

#[test]
fn an_instruction_carries_the_selection_once_per_statement_and_it_stays_selected() {
    let mut h = Harness::selecting(SRC);
    h.app.chrome.show(Pane::Claude);
    h.wait(0.3);
    h.select_all();
    let sel = h.selected();
    h.wait(0.3);
    // Three statements, several blocks each: three chips.
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.focus.chips.len(), 3);
    h.click(h.rect("agent instruction").center());
    h.frame(vec![Event::Text("explain these".into())]);
    assert!(
        h.rect("agent send").bottom() <= h.size.y,
        "Send stays on the page under the chips"
    );
    h.press("agent send");
    h.wait(0.2);
    let v = h.app.sim.as_ref().unwrap();
    let (_, req) = v.link.sent.last().expect("the request");
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    let user = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(user.matches("<chat_about>").count(), 1, "{user}");
    for line in [
        "motor := start AND NOT stop;",
        "lamp := motor OR stop;",
        "horn := start AND stop;",
    ] {
        assert_eq!(
            user.matches(&format!("| {line}")).count(),
            1,
            "{line} once: {user}"
        );
    }
    assert!(!user.contains("more selected"), "{user}");
    assert_eq!(h.selected(), sel, "the selection survives the send");
    assert!(
        h.has("chat chip 0"),
        "and is still the focus: its chips are shown again"
    );
}

#[test]
fn thirty_blocks_stay_within_the_cap_say_how_many_more_and_leave_the_panels_their_width() {
    let src = many();
    let mut h = Harness::selecting(&src);
    // Twelve statements fill the view: zoom out so a corner is empty.
    h.frame(vec![Event::PointerMoved(h.area().center()), Event::Zoom(0.5)]);
    h.wait(0.3);
    h.app.chrome.show(Pane::Claude);
    h.wait(0.3);
    let left = h.rect("left");
    h.select_all();
    h.wait(0.3);
    assert!(h.selected().len() >= 30, "{} blocks selected", h.selected().len());
    let i = &h.app.sim.as_ref().unwrap().instruct;
    assert_eq!(i.focus.chips.len() + i.chips.len(), MAX_CHIPS);
    assert_eq!(i.focus.more, 12 - MAX_CHIPS);
    assert!(h.drew("more selected"), "the row says how many are left out");
    let grew = h.rect("left").right() - left.right();
    assert!((0.0..=8.0).contains(&grew), "the pane grew {grew} pt");
    assert!(
        h.rect("agent send").bottom() <= h.size.y,
        "Send stays on the page under eight chips"
    );
    // Send: the envelope says it too.
    h.click(h.rect("agent instruction").center());
    h.frame(vec![Event::Text("check these".into())]);
    h.press("agent send");
    h.wait(0.2);
    let v = h.app.sim.as_ref().unwrap();
    let (_, req) = v.link.sent.last().expect("the request");
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    let user = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(user.matches("   statement:").count(), MAX_CHIPS, "{user}");
    assert!(
        user.contains("4 more selected statements are not included"),
        "{user}"
    );
}

#[test]
fn collected_chips_leave_less_room_for_the_selection_and_stay_as_they_are() {
    let mut h = Harness::selecting(SRC);
    h.app.chrome.show(Pane::Claude);
    h.wait(0.3);
    // One collected chip for the motor, then the motor's block selected:
    // it is that chip already, not a second one.
    let motor = h.write_of("motor");
    h.wait(1.0);
    h.right_click(h.on_screen(motor).center());
    h.press("diagram menu chat");
    h.wait(0.3);
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.chips.len(), 1);
    h.select_all();
    h.wait(0.3);
    let i = &h.app.sim.as_ref().unwrap().instruct;
    assert_eq!(i.chips.len(), 1, "the collected chip stays");
    let names: Vec<&str> = i.focus.chips.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["lamp", "horn"], "the motor is not twice: {names:?}");
}

#[test]
fn a_selected_fold_stands_for_its_blocks_and_a_fold_change_clears_it() {
    let mut h = Harness::selecting(LATCH);
    h.app.chrome.show(Pane::Claude);
    h.wait(0.3);
    // The latch's blocks (the OR, the NOT and the AND) are one on screen.
    let drawn = h.on_screen(NodeId(1));
    assert_eq!(drawn, h.on_screen(NodeId(2)));
    h.wait(1.0);
    h.click_with(drawn.center(), Modifiers::SHIFT);
    let n = h.selected().len();
    assert_eq!(n, 3, "the latch is one block on screen of {n} blocks");
    h.wait(0.2);
    let i = &h.app.sim.as_ref().unwrap().instruct;
    assert_eq!(i.focus.chips.len(), 1, "one statement, one chip");
    assert_eq!(i.focus.chips[0].name, "motor");
    // Its Shift click again takes all of them out.
    h.click_with(drawn.center(), Modifiers::SHIFT);
    assert!(h.selected().is_empty(), "nothing is selected");
    // Opened into its blocks, the selection is cleared and may be made again.
    h.click_with(drawn.center(), Modifiers::SHIFT);
    let key = sim_view(&h).fold_keys().first().cloned().expect("a fold");
    let v = h.app.sim.as_mut().unwrap().viewer.as_mut().unwrap();
    v.set_fold(&key, true);
    h.wait(0.3);
    assert!(h.selected().is_empty(), "a fold change clears the selection");
    assert!(
        h.app.sim.as_ref().unwrap().instruct.focus.chips.is_empty(),
        "no chips"
    );
    h.wait(1.0);
    h.select_all();
    assert!(!h.selected().is_empty(), "the blocks as drawn select again");
}

#[test]
fn a_scope_change_clears_the_selection() {
    let mut h = Harness::new(crate::app::App::sim());
    let src = crate::test_programs::PROGRAMS
        .iter()
        .find(|e| e.name.contains("stations"))
        .unwrap()
        .src;
    h.app.sim.as_mut().unwrap().load(src, "stations");
    h.wait(1.1);
    h.select_all();
    assert!(!h.selected().is_empty(), "something is selected");
    let v = h.app.sim.as_mut().unwrap().viewer.as_mut().unwrap();
    v.open(1);
    assert!(v.selection().is_empty(), "the other diagram has its own blocks");
    h.wait(0.3);
    assert!(
        h.app.sim.as_ref().unwrap().instruct.focus.chips.is_empty(),
        "no chips"
    );
}

#[test]
fn a_box_diagram_takes_a_band_and_escape_too() {
    use crate::app::App;
    use crate::box_tests::{Bench, watch};
    let bench = Bench::new();
    let (state, _) = watch(&bench);
    let mut h = Harness::new(App::boxed());
    h.app.boxv.as_mut().unwrap().state = state;
    h.wait(1.2);
    let view = |h: &Harness| {
        h.app
            .boxv
            .as_ref()
            .unwrap()
            .view
            .as_ref()
            .expect("a diagram")
            .selection()
            .len()
    };
    let a = h.area();
    let from = Pos2::new(a.left() + 10.0, a.bottom() - 10.0);
    let to = Pos2::new(a.right() - 4.0, a.top() + 60.0);
    h.band(from, to);
    assert!(view(&h) > 0, "the band selected blocks on the box's diagram");
    // The tabs draw chips only for a project the box holds; this box has
    // none, so the Agent has nothing to carry but nothing broke.
    assert!(
        h.app.boxv.as_ref().unwrap().instruct.focus.chips.is_empty(),
        "no chips"
    );
    h.key(Modifiers::NONE, Key::Escape);
    assert_eq!(view(&h), 0, "Escape clears it");
    h.wait(1.0);
    h.band(from, to);
    h.wait(1.0);
    h.click(from);
    assert_eq!(view(&h), 0, "a click off clears it");
}
