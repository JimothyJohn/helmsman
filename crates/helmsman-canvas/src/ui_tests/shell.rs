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
