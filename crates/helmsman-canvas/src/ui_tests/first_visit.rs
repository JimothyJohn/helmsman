//! A first visit to the playground opens on the editor (Nick, 2026-11-27:
//! "Get rid of the stupid tank splash screen ... go straight to the
//! playground from the website"): the header, the panes and the default
//! project running (docs/UI.md, Examples: "a first visit shows the traffic
//! light running"), with no front door in front of it. A browser that
//! kept the old front door's `helmsman-canvas/front` key opens the same
//! way, and Help has no Front page any more.

use std::collections::BTreeMap;

use eframe::egui;

use super::{App, Harness, Screen};

/// The key the front door kept in a browser that had gone on from it.
const OLD_FRONT_KEY: &str = "helmsman-canvas/front";

/// The text the front door drew: none of it may show.
const FRONT_DOOR_TEXT: [&str; 4] = [
    "A control program that proves itself",
    "Open the editor",
    "Fail the high switch",
    "Front page",
];

/// A browser's storage as eframe hands it to the app at start.
#[derive(Default)]
struct Kept(BTreeMap<String, String>);

impl eframe::Storage for Kept {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }

    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }

    fn flush(&mut self) {}
}

/// The page as it starts with `kept` in the browser (None: no storage at
/// all, as a private window that refuses it).
fn start(kept: Option<&Kept>) -> Harness {
    let ctx = egui::Context::default();
    let mut cc = eframe::CreationContext::_new_kittest(ctx);
    cc.storage = kept.map(|k| k as &dyn eframe::Storage);
    let app = App::new(&cc);
    let mut h = Harness::new(app);
    h.wait(0.5);
    h
}

/// The editor is up on the default project, running, and nothing of the
/// front door is drawn.
fn opens_on_the_editor(h: &Harness, case: &str) {
    assert_eq!(h.app.screen, Screen::Editor, "{case}");
    h.header_is_up();
    let v = h
        .app
        .sim
        .as_ref()
        .unwrap_or_else(|| panic!("{case}: no simulation"));
    assert_eq!(v.title(), "traffic", "{case}: the default project");
    assert!(v.session.running, "{case}: running from the first frame");
    let front: Vec<&String> = h
        .app
        .chrome
        .hits
        .iter()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("front"))
        .collect();
    assert_eq!(front, Vec::<&String>::new(), "{case}: a front door's controls");
    for text in FRONT_DOOR_TEXT {
        assert!(!h.drew(text), "{case}: drew {text:?}");
    }
}

#[test]
fn a_first_visit_with_nothing_kept_opens_on_the_editor() {
    let kept = Kept::default();
    let h = start(Some(&kept));
    opens_on_the_editor(&h, "nothing kept");
}

#[test]
fn a_browser_that_keeps_nothing_opens_on_the_editor() {
    let h = start(None);
    opens_on_the_editor(&h, "no storage");
}

/// A visitor who went on from the old front door, or whose browser holds
/// anything at all under its key, opens on the editor the same way.
#[test]
fn an_old_front_door_key_in_storage_still_opens_on_the_editor() {
    for value in [
        "seen",
        "",
        "front",
        "{\"not\": \"what was kept\"}",
        "\u{0}\u{fffd}",
    ] {
        let mut kept = Kept::default();
        kept.0.insert(OLD_FRONT_KEY.into(), value.into());
        let h = start(Some(&kept));
        opens_on_the_editor(&h, &format!("{OLD_FRONT_KEY} = {value:?}"));
    }
}

/// A page shown for a while stays on the editor: nothing comes up in
/// front of it later.
#[test]
fn the_editor_stays_after_the_first_seconds() {
    let kept = Kept::default();
    let mut h = start(Some(&kept));
    h.wait(12.0);
    opens_on_the_editor(&h, "after 12 s");
}

#[test]
fn help_has_no_front_page() {
    let kept = Kept::default();
    let mut h = start(Some(&kept));
    h.press("bar help");
    h.frame(vec![]);
    assert!(h.has("help about"), "the Help menu is open");
    assert!(!h.has("help front"), "Help › Front page is gone");
    assert!(!h.drew("Front page"));
}

/// Leaving the page saves nothing for a front door to read back.
#[test]
fn leaving_keeps_no_front_door_key() {
    let mut kept = Kept::default();
    let mut h = start(Some(&kept));
    h.wait(1.0);
    eframe::App::save(&mut h.app, &mut kept);
    assert_eq!(kept.get(OLD_FRONT_KEY), None, "{:?}", kept.0.keys());
}

impl Kept {
    fn get(&self, key: &str) -> Option<&String> {
        self.0.get(key)
    }
}
