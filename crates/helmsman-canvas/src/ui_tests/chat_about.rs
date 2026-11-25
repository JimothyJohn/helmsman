//! "Chat about this" (`crate::chat_about`), driven through egui: a right
//! click on a block puts a chip on the Agent tab, nothing is sent until
//! the engineer sends, the chip goes with that instruction, and eight
//! long chips leave the panels their width.

use eframe::egui::{Event, Pos2};
use helmsman_diagram::NodeKind;

use super::{Harness, sim_view};
use crate::chat_about::{Chip, Kind, MAX_CHIPS};
use crate::editor::Pane;

const SRC: &str = "\
PROGRAM latch
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  motor AT %QX0.0 : BOOL;
END_VAR
motor := (start OR motor) AND NOT stop;
END_PROGRAM
";

impl Harness {
    /// The simulation with `SRC` as its one program file, deployed, the
    /// Code tab showing so the Agent is not.
    fn chatting() -> Self {
        let mut h = Self::code();
        {
            let v = h.app.sim.as_mut().unwrap();
            v.chat.receive(crate::claude::Incoming::Proxy(true));
            v.proj.create("chat", None);
            v.proj.files.insert("main.st".into(), SRC.into());
            v.proj.save();
            v.open_project("chat");
        }
        h.wait(0.4);
        h
    }

    fn chips(&self) -> usize {
        self.app.sim.as_ref().unwrap().instruct.chips.len()
    }

    /// The motor's assignment block, on screen.
    fn motor_block(&mut self) -> Pos2 {
        let n = sim_view(self)
            .diagram()
            .nodes
            .iter()
            .find(|n| matches!(&n.kind, NodeKind::Write { path } if path.eq_ignore_ascii_case("motor")))
            .expect("the motor's block")
            .id;
        let v = self.app.sim.as_mut().unwrap().viewer.as_mut().unwrap();
        v.reveal(n);
        v.refit();
        self.frame(vec![]);
        self.frame(vec![]);
        sim_view(self).block_on_screen(n).expect("drawn").center()
    }

    /// Right-click the motor's block and choose "Chat about this".
    fn chat_about_motor(&mut self) {
        let at = self.motor_block();
        self.right_click(at);
        self.press("diagram menu chat");
        self.wait(0.2);
    }
}

#[test]
fn the_menu_of_a_block_collects_it_on_the_agent_tab_and_sends_nothing() {
    let mut h = Harness::chatting();
    assert!(
        !h.app.chrome.showing(Pane::Claude),
        "the Agent tab is not showing"
    );
    let sent = h.app.sim.as_ref().unwrap().link.sent.len();
    h.chat_about_motor();
    assert!(h.app.chrome.showing(Pane::Claude), "the Agent tab opens");
    assert_eq!(h.chips(), 1);
    assert!(h.has("chat chip 0") && h.has("chat chip 0 remove"));
    let chip = h.app.sim.as_ref().unwrap().instruct.chips[0].clone();
    assert_eq!(
        (chip.kind, chip.file.as_str(), chip.line),
        (Kind::Block, "main.st", 7)
    );
    assert_eq!(chip.name, "motor");
    assert_eq!(chip.text, "motor := (start OR motor) AND NOT stop;");
    assert!(chip.driven_by.iter().any(|n| n == "stop"), "{chip:?}");
    assert_eq!(
        h.app.sim.as_ref().unwrap().link.sent.len(),
        sent,
        "nothing is sent"
    );
    assert!(
        h.app.sim.as_ref().unwrap().instruct.draft.is_empty(),
        "nothing is typed for the engineer"
    );
}

#[test]
fn the_same_block_twice_is_one_chip_and_the_x_takes_it_off() {
    let mut h = Harness::chatting();
    h.chat_about_motor();
    h.app.chrome.show(Pane::Code);
    h.wait(0.2);
    h.chat_about_motor();
    assert_eq!(h.chips(), 1, "no duplicate");
    h.press("chat chip 0 remove");
    h.wait(0.2);
    assert_eq!(h.chips(), 0);
    assert!(!h.has("chat chip 0"));
}

#[test]
fn the_chips_go_with_the_instruction_that_is_sent_and_are_then_gone() {
    let mut h = Harness::chatting();
    h.chat_about_motor();
    h.click(h.rect("agent instruction").center());
    h.frame(vec![Event::Text("stop the motor when the lamp trips".into())]);
    h.press("agent send");
    h.wait(0.2);
    let v = h.app.sim.as_ref().unwrap();
    let (_, req) = v.link.sent.last().expect("the request");
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    let user = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(user.starts_with("stop the motor when the lamp trips"), "{user}");
    assert_eq!(user.matches("<chat_about>").count(), 1, "{user}");
    assert!(user.contains("block `motor` at main.st:7"), "{user}");
    assert!(
        user.contains("| motor := (start OR motor) AND NOT stop;"),
        "{user}"
    );
    assert!(v.instruct.chips.is_empty(), "they went with it");
    assert!(!h.has("chat chip 0"));
}

#[test]
fn a_ninth_chip_is_not_taken_and_the_menu_item_is_off() {
    let mut h = Harness::chatting();
    {
        let v = h.app.sim.as_mut().unwrap();
        for k in 0..MAX_CHIPS {
            v.instruct.add_chip(Chip::new(
                &format!("n{k}"),
                Kind::Input,
                "main.st",
                100 + u32::try_from(k).unwrap(),
                "",
                vec![],
                vec![],
            ));
        }
    }
    let at = h.motor_block();
    h.right_click(at);
    assert!(h.has("diagram menu chat"));
    h.press("diagram menu chat");
    h.wait(0.2);
    assert_eq!(h.chips(), MAX_CHIPS, "the item is off while eight are collected");
}

/// A resizable panel grows to any row that overflows it: eight chips with
/// long names, at the right panel's default width, leave both panels where
/// they were, and every chip is inside the panel.
#[test]
fn eight_long_chips_leave_the_panels_their_width() {
    let mut h = Harness::chatting();
    h.app.chrome.dock(Pane::Claude, true);
    h.press("tab claude");
    h.wait(0.3);
    let (left, right) = (h.rect("left"), h.rect("right"));
    let long = "conveyor_infeed_photo_eye_blocked_for_too_long_alarm_latched_until_reset";
    for k in 0..MAX_CHIPS {
        let v = h.app.sim.as_mut().unwrap();
        v.instruct.add_chip(Chip::new(
            &format!("{long}_{k}"),
            if k % 2 == 0 { Kind::Block } else { Kind::Output },
            "a_file_with_a_rather_long_name.st",
            10 + u32::try_from(k).unwrap(),
            "x := 1;",
            vec![long.into()],
            vec![long.into()],
        ));
        h.wait(0.2);
        let (l, r) = (
            h.rect("left").right() - left.right(),
            right.left() - h.rect("right").left(),
        );
        assert!(
            (0.0..=8.0).contains(&l) && (0.0..=8.0).contains(&r),
            "after chip {k} the panels grew: {l} {r}"
        );
    }
    for k in 0..MAX_CHIPS {
        let chip = h.rect(&format!("chat chip {k}"));
        let x = h.rect(&format!("chat chip {k} remove"));
        // The instruction box is as wide as the column.
        let panel = h.rect("agent instruction");
        assert!(
            x.right() <= panel.right() && chip.left() >= panel.left(),
            "chip {k} inside the panel: {chip:?} {x:?} {panel:?}"
        );
        assert!(x.height() >= 18.0, "the x is a real button: {x:?}");
    }
}

/// Nick, 2026-11-25: a long instruction wrapped at the panel and the box
/// stayed three lines tall, the first line scrolled out of view. The box
/// grows with the text to ten lines, then scrolls inside; the panel keeps
/// its width the whole time.
#[test]
fn the_instruction_box_shows_what_was_typed_up_to_ten_lines() {
    let mut h = Harness::chatting();
    h.press("tab claude");
    h.wait(0.3);
    let left = h.rect("left");
    let line = 20.0;
    let one = h.rect("agent instruction").height();
    h.click(h.rect("agent instruction").center());
    h.frame(vec![]);
    let said = "Now, not only should the lamp go high, but the motor should spin at 1rps while it's active";
    // Twice over: six wrapped lines at the panel's width.
    let said = format!("{said} {said}");
    h.frame(vec![Event::Text(said.clone())]);
    h.wait(0.5);
    let r = h.rect("agent instruction");
    assert!(
        r.height() >= 5.0 * line && r.height() > one + 2.0 * line,
        "the box grew to hold the sentence at the panel's width: {one} -> {}",
        r.height()
    );
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.draft, said);
    // A long essay: ten lines at most, then it scrolls inside.
    h.frame(vec![Event::Text(format!(" {said}").repeat(8))]);
    h.wait(0.5);
    let big = h.rect("agent instruction");
    assert!(
        big.height() >= 9.0 * line && big.height() <= 10.0 * 24.0 + 12.0,
        "ten lines at most: {}",
        big.height()
    );
    let grew = h.rect("left").right() - left.right();
    assert!((0.0..=8.0).contains(&grew), "the panel grew: {grew}");
}
