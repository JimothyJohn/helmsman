//! The app driven headlessly through egui with synthetic pointer and key
//! events, asserting on the app's state and on where it drew its
//! controls, not on pixels.

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use helmsman_diagram::NodeKind;

use crate::app::{App, Screen, Source};
use crate::st_view::Viewer;

struct Harness {
    ctx: egui::Context,
    app: App,
    time: f64,
    /// The window, in points.
    size: Vec2,
}

impl Harness {
    fn new(app: App) -> Self {
        let mut h = Self {
            ctx: egui::Context::default(),
            app,
            time: 0.0,
            size: Vec2::new(1400.0, 860.0),
        };
        // Two frames: panels settle their sizes, the view fits the graph.
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let Self { ctx, app, .. } = self;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        // No renderer here to upload textures to.
        out.textures_delta.clear();
    }

    /// Frames for `secs` of page time (animations end).
    fn wait(&mut self, secs: f64) {
        let end = self.time + secs;
        while self.time < end {
            self.frame(vec![]);
        }
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn click(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Self::button(at, true)]);
        self.frame(vec![Self::button(at, false)]);
    }

    /// A right click at `at` (opens a context menu).
    fn right_click(&mut self, at: Pos2) {
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![press(true)]);
        self.frame(vec![press(false)]);
        self.frame(vec![]);
    }

    /// Where the app drew control `key` last frame.
    fn rect(&self, key: &str) -> Rect {
        self.app.chrome.rect(key).unwrap_or_else(|| {
            let keys: Vec<&String> = self.app.chrome.hits.iter().map(|(k, _)| k).collect();
            panic!("no {key} on screen: {keys:?}")
        })
    }

    fn has(&self, key: &str) -> bool {
        self.app.chrome.rect(key).is_some()
    }

    fn press(&mut self, key: &str) {
        let at = self.rect(key).center();
        self.click(at);
    }

    /// Opens the View menu and presses `item` in it.
    fn view_menu(&mut self, item: &str) {
        self.press("bar view");
        self.press(item);
    }

    /// The header's controls, all there.
    fn header_is_up(&self) {
        for k in ["mark", "title", "editor", "help"] {
            assert!(self.has(k), "no {k} in the header");
        }
    }
}

/// The simulation's diagram.
fn sim_view(h: &Harness) -> &Viewer {
    h.app.sim.as_ref().unwrap().viewer.as_ref().expect("a diagram")
}

/// Screen centre and on-screen width of the first node matching `f`.
fn node(v: &Viewer, f: impl Fn(&NodeKind) -> bool) -> (Pos2, f32) {
    let n = v.diagram().nodes.iter().find(|n| f(&n.kind)).expect("the node");
    let cam = v.cams[v.scope];
    (
        cam.to_screen(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0),
        n.rect.w * cam.zoom,
    )
}

fn write_to(path: &str) -> impl Fn(&NodeKind) -> bool + '_ {
    move |k| *k == NodeKind::Write { path: path.into() }
}

// ---- the header and the Editor's frame ----

mod glyphs_and_help {
    use eframe::egui::Rect;

    use super::{App, Harness, Screen};

    /// Nick, 2026-11-01: run/stop, step and restart as glyphs over the
    /// diagram's top-right corner, with save, upload, download and help.
    #[test]
    fn the_glyphs_sit_on_the_diagram_and_drive_the_simulation() {
        let mut h = Harness::new(App::sim());
        let diagram = h.app.chrome.diagram;
        assert_ne!(diagram, Rect::NOTHING);
        for k in ["run", "step", "restart", "save", "upload", "download"] {
            let r = h.rect(k);
            assert!(
                diagram.contains(r.center()),
                "{k} over the diagram: {r:?} in {diagram:?}"
            );
            assert!(r.top() < diagram.top() + 40.0, "{k} along the top: {r:?}");
        }
        let last = h.rect("download");
        assert!(
            last.right() > diagram.right() - 40.0,
            "the strip ends at the right edge: {last:?}"
        );
        assert!(h.app.sim.as_ref().unwrap().session.running);
        h.press("run");
        assert!(!h.app.sim.as_ref().unwrap().session.running, "the glyph stops");
        let scans = h.app.sim.as_ref().unwrap().session.scans;
        h.press("step");
        assert_eq!(h.app.sim.as_ref().unwrap().session.scans, scans + 1);
        h.press("restart");
        assert_eq!(h.app.sim.as_ref().unwrap().session.scans, 0, "a cold restart");
        h.press("run");
        assert!(h.app.sim.as_ref().unwrap().session.running, "and starts again");
        // The Monitor has no diagram, so no glyphs.
        h.press("monitor");
        assert_eq!(h.app.screen, Screen::Monitor);
        assert!(!h.has("run") && !h.has("download"));
        assert!(h.has("help"), "? stays in the header");
    }

    /// Nick, 2026-11-01: the toggle flips on a click on either side; the
    /// Messages tab filters by level; the Projects pane has no "Files"
    /// header.
    #[test]
    fn the_toggle_flips_from_either_side_and_messages_filter_by_level() {
        use crate::editor::Bottom;
        let mut h = Harness::new(App::sim());
        assert_eq!(h.app.screen, Screen::Editor);
        h.press("editor");
        assert_eq!(h.app.screen, Screen::Monitor, "the active side flips too");
        h.press("monitor");
        assert_eq!(h.app.screen, Screen::Editor);
        h.press("bottom messages");
        h.frame(vec![]);
        for k in ["messages info", "messages warnings", "messages errors"] {
            assert!(h.has(k), "no {k}");
        }
        h.press("messages info");
        let log = &h.app.sim.as_ref().unwrap().log;
        assert_eq!(log.show, [false, true, true], "info filtered out");
        assert_eq!(h.app.chrome.bottom, Bottom::Messages);
        assert!(!h.has("st"), "no Files header row either");
    }

    #[test]
    fn the_bottom_panel_has_trend_and_messages_tabs() {
        use crate::editor::Bottom;
        let mut h = Harness::new(App::sim());
        assert!(h.app.chrome.trend);
        assert_eq!(h.app.chrome.bottom, Bottom::Trend);
        for k in ["bottom trend", "bottom messages"] {
            assert!(h.has(k), "no {k}");
        }
        h.press("bottom messages");
        assert_eq!(h.app.chrome.bottom, Bottom::Messages);
        h.frame(vec![]);
        h.press("bottom trend");
        assert_eq!(h.app.chrome.bottom, Bottom::Trend);
        // Kept per browser.
        h.app.chrome.bottom = Bottom::Messages;
        let back = crate::editor::Chrome::restored(&h.app.chrome.saved());
        assert_eq!(back.bottom, Bottom::Messages);
    }

    #[test]
    fn editor_and_monitor_are_one_toggle_and_help_has_an_about() {
        let mut h = Harness::new(App::sim());
        let (e, m) = (h.rect("editor"), h.rect("monitor"));
        assert!(
            (e.right() - m.left()).abs() < 1.0 && (e.top() - m.top()).abs() < 1.0,
            "one control: {e:?} {m:?}"
        );
        h.press("monitor");
        assert_eq!(h.app.screen, Screen::Monitor);
        h.press("editor");
        assert_eq!(h.app.screen, Screen::Editor);
        h.press("bar help");
        for k in ["help docs", "help keys", "help about"] {
            assert!(h.has(k), "no {k}");
        }
        h.press("help about");
        assert!(h.app.about);
        h.frame(vec![]);
        assert!(h.has("about close"));
        h.press("about close");
        assert!(!h.app.about);
    }
}

mod polish {
    use eframe::egui::{self, Event, Key, Modifiers};

    use super::{App, Harness, Screen};
    use crate::app::HEADER_H;
    use crate::editor::{Tab, numbering};

    impl Harness {
        pub(super) fn key(&mut self, modifiers: Modifiers, key: Key) {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]);
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            }]);
        }
    }

    /// CODESYS's keys where the browser leaves them alone (docs/UI.md,
    /// Keyboard): the simulation's run controls, the panes, Save.
    #[test]
    fn the_shortcuts_run_step_stop_save_and_toggle_the_panes() {
        let mut h = Harness::new(App::sim());
        assert!(
            h.app.sim.as_ref().unwrap().session.running,
            "the playground opens running"
        );
        h.key(Modifiers::SHIFT, Key::F8);
        assert!(!h.app.sim.as_ref().unwrap().session.running, "Shift+F8 stops");
        let scans = h.app.sim.as_ref().unwrap().session.scans;
        h.key(Modifiers::NONE, Key::F10);
        assert_eq!(
            h.app.sim.as_ref().unwrap().session.scans,
            scans + 1,
            "F10 steps one scan"
        );
        h.key(Modifiers::COMMAND, Key::Enter);
        assert!(h.app.sim.as_ref().unwrap().session.running, "Ctrl+Enter runs");
        h.key(Modifiers::COMMAND, Key::Enter);
        assert!(!h.app.sim.as_ref().unwrap().session.running, "and stops");
        // The panes: the simulation starts on the left pane's Code tab.
        assert!(h.app.chrome.code_open());
        let right = h.app.chrome.right;
        h.key(Modifiers::ALT, Key::Num1);
        assert!(!h.app.chrome.left, "Alt+1 on the Code tab shuts the pane");
        h.key(Modifiers::ALT, Key::Num0);
        assert!(h.app.chrome.files_open(), "Alt+0: the Files tab");
        h.key(Modifiers::ALT, Key::Num1);
        assert!(h.app.chrome.code_open(), "Alt+1: the Code tab");
        h.key(Modifiers::ALT, Key::Num3);
        assert_eq!(h.app.chrome.right, !right, "Alt+3: the side panel");
        // Save: edit, then Ctrl+S.
        h.wait(0.2);
        let editor = h
            .ctx
            .read_response(egui::Id::unique("pg-source"))
            .expect("the editor")
            .rect;
        h.click(editor.left_top() + egui::vec2(60.0, 6.0));
        h.frame(vec![Event::Text("!".into())]);
        h.wait(0.6);
        assert!(h.has("dirty"));
        h.key(Modifiers::COMMAND, Key::S);
        h.frame(vec![]);
        assert!(!h.has("dirty"), "Ctrl+S saved");
        // A box's keys do nothing here.
        h.key(Modifiers::ALT, Key::F8);
        assert!(!h.app.login, "no sign-in in the simulation");
    }

    #[test]
    fn ctrl_shift_d_opens_the_declaration_and_ctrl_shift_x_lights_the_references() {
        let mut h = Harness::new(App::sim());
        h.key(Modifiers::ALT, Key::Num1);
        assert!(!h.app.chrome.code_open(), "ST shut for the test");
        // The pane's close moves the diagram: let it settle, then pick.
        h.wait(0.6);
        let (at, _) = super::node(super::sim_view(&h), super::write_to("LAMP"));
        h.click(at);
        h.frame(vec![]);
        assert_eq!(
            super::sim_view(&h).picked_var().as_deref(),
            Some("LAMP"),
            "the block is picked"
        );
        h.app.chrome.line = None;
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::D);
        assert!(h.app.chrome.code_open(), "the ST opens");
        let line = h.app.chrome.line.expect("sent to the declaration");
        let src = h.app.sim.as_ref().unwrap().session.src.clone();
        let decl = src.lines().nth(line as usize - 1).unwrap_or("");
        assert!(
            decl.to_ascii_lowercase().contains("lamp"),
            "line {line}: {decl:?}"
        );
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::X);
        h.frame(vec![]);
        let v = super::sim_view(&h);
        assert!(v.highlighted().is_some_and(|d| !d.is_empty()), "references lit");
        h.press("bar edit");
        for k in ["edit definition", "edit xref"] {
            assert!(h.has(k), "no {k}");
        }
    }

    #[test]
    fn alt_f8_opens_sign_in_and_ctrl_f8_signs_out_on_a_box() {
        let mut h = Harness::new(App::boxed());
        assert!(!h.app.login);
        h.key(Modifiers::ALT, Key::F8);
        assert!(h.app.login, "Alt+F8: Login");
        h.key(Modifiers::CTRL, Key::F8);
        assert!(!h.app.login, "Ctrl+F8: Logout closes it and ends any session");
        assert!(h.app.session().is_none());
        // Run keys do nothing on a box's page.
        h.key(Modifiers::COMMAND, Key::Enter);
        assert!(h.app.sim.is_none());
    }

    /// Nick, 2026-11-01: one tall row, menus left, the project right; the
    /// bar that held ST, Reset layout and Save as default is gone.
    #[test]
    fn the_header_is_one_tall_row_with_file_edit_and_view_menus() {
        let mut h = Harness::new(App::sim());
        let header = h.rect("header");
        assert!(header.height() >= HEADER_H, "{header:?}");
        for k in [
            "mark", "bar file", "bar edit", "bar view", "bar help", "title", "editor", "monitor", "help",
        ] {
            assert!(h.has(k), "no {k}");
        }
        assert!(!h.has("deploy"), "Deploy is the File menu's");
        assert!(!h.has("menu"), "no ⋯ any more");
        let (file, view, help, toggle) = (
            h.rect("bar file"),
            h.rect("bar view"),
            h.rect("help"),
            h.rect("monitor"),
        );
        assert!(
            file.left() < view.left() && view.right() < help.left(),
            "menus left, ? right"
        );
        assert!(
            help.left() >= toggle.right(),
            "? right of the Editor | Monitor toggle: {help:?} {toggle:?}"
        );
        assert!(help.right() <= 1400.0, "on screen");
        assert!(!h.has("st") && !h.has("reset layout"), "no second bar");
        // The menus are there on the Monitor too.
        h.press("monitor");
        assert_eq!(h.app.screen, Screen::Monitor);
        assert!(h.has("bar view"));
        h.view_menu("view editor");
        assert_eq!(h.app.screen, Screen::Editor);
    }

    #[test]
    fn the_view_menu_toggles_panels_tabs_layout_and_fit() {
        let mut h = Harness::new(App::sim());
        assert!(h.app.chrome.left && h.app.chrome.right);
        h.view_menu("view files");
        assert!(h.app.chrome.files_open(), "from the Code tab, Files shows");
        h.view_menu("view files");
        assert!(!h.app.chrome.left, "again: the pane shuts");
        h.view_menu("view panel");
        assert!(!h.app.chrome.right);
        h.view_menu("view tab claude");
        assert!(
            h.app.chrome.right && h.app.chrome.tab == Tab::Claude,
            "a tab opens the panel"
        );
        assert!(h.app.chrome.trend, "the Trend runs along the bottom");
        h.view_menu("view trend");
        assert!(!h.app.chrome.trend);
        h.press("bar view");
        for k in ["view fit", "reset layout", "save layout"] {
            assert!(h.has(k), "no {k}");
        }
        assert!(
            !h.has("view scan"),
            "scan detail is an engineer's, not the simulation's"
        );
        h.press("view fit");
        h.wait(0.2);
        assert_eq!(h.app.screen, Screen::Editor);
    }

    #[test]
    fn the_file_menu_saves_and_the_edit_menu_undoes_a_typed_character() {
        let mut h = Harness::new(App::sim());
        assert!(h.app.chrome.code_open(), "the ST open");
        let editor = h
            .ctx
            .read_response(egui::Id::unique("pg-source"))
            .expect("the editor")
            .rect;
        h.click(editor.left_top() + egui::vec2(60.0, 6.0));
        h.frame(vec![Event::Text("!".into())]);
        h.wait(0.6);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(
            sim.session.src.lines().next().unwrap().contains('!'),
            "{:?}",
            sim.session.src
        );
        assert!(h.has("dirty"), "edited");
        h.press("bar edit");
        h.press("edit undo");
        h.wait(0.6);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(
            !sim.session.src.lines().next().unwrap().contains('!'),
            "undone: {:?}",
            sim.session.src
        );
        h.press("bar edit");
        h.press("edit redo");
        h.wait(0.6);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(
            sim.session.src.lines().next().unwrap().contains('!'),
            "redone: {:?}",
            sim.session.src
        );
        h.press("bar file");
        h.press("file save");
        h.frame(vec![]);
        assert!(!h.has("dirty"), "saved from the File menu");
    }

    /// The hit stays "tab claude" (the stored key); only the label changed.
    #[test]
    fn the_third_tab_reads_agent() {
        assert_eq!(Tab::Claude.label(), "Agent");
    }

    #[test]
    fn a_wrapped_line_is_numbered_once_on_its_first_row() {
        // Line 1 wraps over three rows, line 2 fits, line 3 is empty.
        let rows = [false, false, true, true, true];
        assert_eq!(
            numbering(rows.into_iter()),
            [(1, true), (1, false), (1, false), (2, true), (3, true)]
        );
        assert_eq!(numbering(std::iter::empty()), []);
        // A last line without a newline is still numbered.
        assert_eq!(numbering([true, false].into_iter()), [(1, true), (2, true)]);
    }
}

mod chrome {
    use eframe::egui::{self, Event, Pos2, Vec2};

    use super::{Harness, node, sim_view, write_to};
    use crate::app::{App, Screen, Source, wants_sim};
    use crate::editor::{Chrome, Pane, STRIP, Tab};

    #[test]
    fn the_header_is_on_every_screen() {
        let h = Harness::new(App::sim());
        h.header_is_up();
        assert_eq!(h.app.title(), "blink", "the program's name");
        let mut h = Harness::new(App::boxed());
        h.header_is_up();
        assert_eq!(h.app.title(), "This box", "no module yet");
        h.app.screen = Screen::Proposals;
        h.frame(vec![]);
        h.header_is_up();
    }

    #[test]
    fn view_opens_proposals_and_the_toggle_leads_back() {
        let mut h = Harness::new(App::boxed());
        h.press("bar view");
        for k in ["view proposals", "view sim"] {
            assert!(h.has(k), "no {k} in View on a box");
        }
        h.press("view proposals");
        assert_eq!(h.app.screen, Screen::Proposals);
        assert!(h.app.review.is_some());
        // Back by the toggle (either side leaves Proposals for the Editor).
        h.press("editor");
        assert_eq!(h.app.screen, Screen::Editor);
        h.press("bar view");
        h.press("view proposals");
        assert_eq!(h.app.screen, Screen::Proposals);
        h.press("monitor");
        assert_eq!(
            h.app.screen,
            Screen::Editor,
            "from Proposals either side goes to the Editor"
        );
        // A click elsewhere with the menu open closes it and changes nothing.
        h.press("bar view");
        h.click(Pos2::new(700.0, 500.0));
        assert_eq!(h.app.screen, Screen::Editor);
    }

    #[test]
    fn about_shows_the_boxs_https_fingerprint_when_it_has_one() {
        let mut h = Harness::new(App::boxed());
        h.press("bar help");
        h.press("help about");
        h.frame(vec![]);
        assert!(!h.has("tls fingerprint"), "not before the policy says");
        let fp = "AB:".repeat(31) + "AB";
        h.app.review.as_mut().unwrap().state.policy =
            Some(serde_json::json!({ "transport": { "https": true, "fingerprint": fp } }));
        h.frame(vec![]);
        assert!(h.has("tls fingerprint"));
        // Plain HTTP (--https off): nothing to show.
        h.app.review.as_mut().unwrap().state.policy =
            Some(serde_json::json!({ "transport": { "https": false, "fingerprint": null } }));
        h.frame(vec![]);
        assert!(!h.has("tls fingerprint"));
        h.press("about close");
    }

    #[test]
    fn the_source_is_a_setting_in_view_on_a_box_and_absent_without_one() {
        let mut h = Harness::new(App::boxed());
        h.press("bar view");
        h.press("view sim");
        assert_eq!(h.app.source, Source::Sim);
        assert!(h.app.sim.is_some(), "the simulation runs");
        h.header_is_up();
        h.press("bar view");
        assert!(
            h.has("view box") && !h.has("view proposals"),
            "no proposals without a box"
        );
        h.press("view box");
        assert_eq!(h.app.source, Source::Box);

        // The public page: no box to go back to, no proposals.
        let mut h = Harness::new(App::sim());
        h.press("bar view");
        assert!(!h.has("view box") && !h.has("view sim") && !h.has("view proposals"));
        h.press("bar view");
        // Upload and Download live in the File menu.
        h.press("bar file");
        assert!(
            h.has("file upload") && h.has("file download"),
            "{:?}",
            h.app.chrome.hits
        );
        h.app.screen = Screen::Proposals;
        h.frame(vec![]);
        assert_eq!(h.app.screen, Screen::Editor, "proposals need a box");
        assert!(h.app.review.is_none(), "and nothing asks one");
    }

    #[test]
    fn no_old_mode_is_reachable() {
        // Every control the app draws, on every screen and source, with the
        // menu open: nothing leads to Lab, Site, Demo, Andon or Playground.
        let mut seen = Vec::new();
        for (make, proposals) in [
            (App::sim as fn() -> App, false),
            (App::boxed, false),
            (App::boxed, true),
        ] {
            let mut h = Harness::new(make());
            if proposals {
                h.app.screen = Screen::Proposals;
            }
            h.frame(vec![]);
            h.press("bar view");
            seen.extend(h.app.chrome.hits.iter().map(|(k, _)| k.to_ascii_lowercase()));
        }
        for gone in ["lab", "site", "demo", "andon", "playground"] {
            assert!(!seen.iter().any(|k| k.contains(gone)), "{gone} in {seen:?}");
        }
        // The query string picks only where the program runs.
        for q in ["?andon", "?site", "?lab", "?view=demo", ""] {
            assert!(!wants_sim(q, None), "{q}");
        }
    }

    #[test]
    fn old_playground_links_still_open_the_simulation() {
        for q in ["?playground", "?demo", "?x=1&playground", "?playground=1"] {
            assert!(wants_sim(q, None), "{q}");
        }
        assert!(wants_sim("", Some("playground")), "docs/playground/'s page");
        assert!(!wants_sim("?playgrounds", Some("box")));
    }

    #[test]
    fn both_sidebars_collapse_to_a_strip_and_come_back() {
        let mut h = Harness::new(App::sim());
        assert!(h.app.chrome.left && h.app.chrome.right);
        for (side, edge) in [("left", 0.0), ("right", h.size.x)] {
            let open = h.rect(side);
            h.press(side);
            h.wait(0.5);
            let shut = h.rect(side);
            assert!(
                (shut.center().x - edge).abs() <= STRIP,
                "{side} folds to a strip at its edge: {shut:?} (was {open:?})"
            );
            h.press(side);
            h.wait(0.5);
            assert!(h.has(&format!("tab {}", "io")) || side == "left");
        }
        assert!(h.app.chrome.left && h.app.chrome.right, "both open again");
        h.press("left");
        assert!(!h.app.chrome.left);
        h.press("right");
        assert!(!h.app.chrome.right);
        h.wait(0.5);
        assert!(!h.has("tab io"), "the tabs fold with the panel");
        // Remembered per browser: what the app saves reads back the same.
        let back = Chrome::restored(&h.app.chrome.saved());
        assert_eq!((back.left, back.right, back.pane), (false, false, Pane::Code));
    }

    #[test]
    fn the_left_pane_has_files_and_code_tabs_and_one_width() {
        let mut h = Harness::new(App::sim());
        assert!(h.app.chrome.code_open(), "the simulation opens on Code");
        let code = h.rect("pane code");
        let files = h.rect("pane files");
        assert!(files.right() <= code.left(), "Files | Code, left to right");
        let editor = h
            .ctx
            .read_response(egui::Id::unique("pg-source"))
            .expect("the editor")
            .rect;
        h.press("pane files");
        h.wait(0.3);
        assert!(h.app.chrome.files_open());
        assert!(h.has("file blink.st"), "the files show");
        assert!(
            h.ctx.read_response(egui::Id::unique("pg-source")).is_none(),
            "the editor is on the other tab"
        );
        let code_w = code.right() - files.left();
        let files_w = h.rect("pane code").right() - h.rect("pane files").left();
        assert!((code_w - files_w).abs() < 1.0, "one width for both tabs");
        h.press("pane code");
        h.wait(0.3);
        let again = h
            .ctx
            .read_response(egui::Id::unique("pg-source"))
            .expect("the editor")
            .rect;
        assert!((again.width() - editor.width()).abs() < 1.0);
        // Remembered: the tab reads back.
        let back = Chrome::restored(&h.app.chrome.saved());
        assert_eq!(back.pane, Pane::Code);
        assert_eq!(
            Chrome::restored("st=1").pane,
            Pane::Code,
            "the old key still reads"
        );
    }

    #[test]
    fn a_right_click_on_a_file_copies_pastes_and_duplicates_it() {
        let mut h = Harness::new(App::sim());
        // An example is read-only: its file menu offers Copy alone. File ›
        // Copy project first to get a writable one; it lands on the Files
        // tab asking for the name.
        h.press("bar file");
        h.press("file copy");
        assert!(h.app.chrome.files_open(), "the name is asked on the Files tab");
        h.press("name ok");
        h.wait(0.3);
        assert!(h.has("file blink.st"), "blink-copy open with its file");
        assert!(
            !h.has("new project") && !h.has("copy project"),
            "the buttons are gone"
        );
        // A right-click on a project offers its copy, rename and delete.
        h.right_click(h.rect("project blink").center());
        assert!(h.has("menu copy project"));
        assert!(
            !h.has("menu rename project") || {
                // Listed but disabled for an example: a press does nothing.
                h.press("menu rename project");
                h.app.sim.as_ref().unwrap().files_ui.naming.is_none()
            }
        );
        h.frame(vec![]);
        let text = h.app.sim.as_ref().unwrap().proj.files["blink.st"].clone();
        let at = h.rect("file blink.st").center();
        h.right_click(at);
        for k in [
            "menu copy",
            "menu paste",
            "menu duplicate",
            "menu rename",
            "menu delete",
        ] {
            assert!(h.has(k), "the menu has {k}");
        }
        h.press("menu duplicate");
        h.wait(0.3);
        let files = |h: &Harness| h.app.sim.as_ref().unwrap().proj.files.clone();
        assert_eq!(files(&h).get("blink-2.st"), Some(&text), "a duplicate beside it");
        assert!(h.has("file blink-2.st"));
        h.right_click(h.rect("file blink.st").center());
        h.press("menu copy");
        h.wait(0.2);
        h.right_click(h.rect("file blink-2.st").center());
        h.press("menu paste");
        h.wait(0.3);
        assert_eq!(
            files(&h).get("blink-3.st"),
            Some(&text),
            "pasted under the next free name"
        );
        // The clipboard crosses projects: a new, empty one takes the paste.
        // New project from a right-click on the pane's blank space.
        h.right_click(h.rect("files blank").center());
        h.press("menu new project");
        // The name field is drawn above the blank space, so a frame later.
        h.frame(vec![]);
        h.press("name");
        h.frame(vec![Event::Text("mine".into())]);
        h.press("name ok");
        h.wait(0.3);
        assert_ne!(h.app.sim.as_ref().unwrap().proj.open, "blink-copy");
        let heading = h.rect("new file").min - Vec2::new(0.0, 22.0);
        h.right_click(heading);
        assert!(h.has("menu paste"), "the files heading takes a paste");
        h.press("menu paste");
        h.wait(0.3);
        assert_eq!(files(&h).get("blink.st"), Some(&text), "pasted across projects");
    }

    #[test]
    fn the_layout_read_back_from_storage_is_forgiving() {
        let mut c = Chrome::default();
        c.tab = Tab::Claude;
        c.pane = Pane::Code;
        c.scan = true;
        c.trend = false;
        let back = Chrome::restored(&c.saved());
        assert_eq!(
            (back.left, back.right, back.pane, back.tab, back.scan, back.trend),
            (true, true, Pane::Code, Tab::Claude, true, false)
        );
        // Anything else keeps the defaults; the Why tab is gone.
        for junk in ["", "left", "tab=why", "=== =1"] {
            assert_eq!(Chrome::restored(junk), Chrome::default(), "{junk:?}");
        }
        let c = Chrome::restored("right=yes left=0 pane=code scan=yes \u{0}");
        assert_eq!(
            (c.left, c.right, c.pane, c.tab, c.scan),
            (false, false, Pane::Code, Tab::Io, false)
        );
    }

    #[test]
    fn the_st_toggle_opens_and_closes_the_editor_and_a_block_click_sends_it_to_the_line() {
        let mut h = Harness::new(App::sim());
        let editor = || egui::Id::unique("pg-source");
        assert!(h.app.chrome.code_open(), "the simulation starts with the ST open");
        assert!(h.ctx.read_response(editor()).is_some());
        h.view_menu("view st");
        assert!(!h.app.chrome.code_open());
        h.wait(0.5);
        assert!(h.ctx.read_response(editor()).is_none(), "shut");
        h.view_menu("view st");
        assert!(h.app.chrome.code_open());
        h.wait(0.5);
        let r = h.ctx.read_response(editor()).expect("open again");
        // The pane opens at 160 pt; the editor is what is left of it
        // beside the gutter.
        assert!((50.0..160.0).contains(&r.rect.width()), "{:?}", r.rect);

        // Click the lamp's assignment: the ST goes to its line.
        let (at, _) = node(sim_view(&h), write_to("LAMP"));
        h.click(at);
        let (line, text) = sim_view(&h).picked().unwrap();
        assert_eq!(text, "LAMP := NOT LAMP");
        assert_eq!(h.app.chrome.line, Some(line));
        h.frame(vec![]);
        let src = h.app.sim.as_ref().unwrap().session.src.clone();
        assert_eq!(
            src.lines().nth(line as usize - 1).unwrap().trim(),
            "lamp := NOT lamp;"
        );
        let chars: usize = src
            .split_inclusive('\n')
            .take(line as usize - 1)
            .map(|l| l.chars().count())
            .sum();
        let state = egui::text_edit::TextEditState::load(&h.ctx, editor()).expect("the editor's state");
        assert_eq!(
            state.cursor.char_range(),
            Some(egui::text::CCursorRange::one(egui::text::CCursor::new(chars))),
            "the caret at the start of line {line}"
        );
        // Shut, the line is still where it opens.
        h.view_menu("view st");
        h.wait(0.6);
        let (at, _) = node(sim_view(&h), write_to("LAMP"));
        h.click(at + Vec2::new(0.0, 0.0));
        assert_eq!(h.app.chrome.line, Some(line));
    }
}

// ---- the Editor on the simulation ----

mod simulated {
    use eframe::egui::{Event, Key, Modifiers, MouseWheelUnit, Pos2, Vec2};

    use super::{Harness, node, sim_view, write_to};
    use crate::app::App;
    use crate::editor::Tab;
    use crate::st_view::{Pick, TAP, node_near};

    fn stations() -> Harness {
        let mut h = Harness::new(App::sim());
        let src = crate::playground::EXAMPLES
            .iter()
            .find(|e| e.name.contains("stations"))
            .unwrap()
            .src;
        h.app.sim.as_mut().unwrap().load(src, "stations");
        h.wait(0.1);
        h
    }

    #[test]
    fn clicking_empty_canvas_clears_the_pick() {
        let mut h = Harness::new(App::sim());
        let (at, _) = node(sim_view(&h), write_to("LAMP"));
        h.click(at);
        assert!(matches!(sim_view(&h).pick, Some(Pick::Node(_))));
        // The canvas's bottom right corner, left of the right panel and
        // with the Trend folded away from the bottom.
        h.app.chrome.trend = false;
        h.wait(0.5);
        let far = Pos2::new(h.rect("right").left() - 30.0, h.size.y - 6.0);
        h.click(far);
        assert_eq!(sim_view(&h).pick, None);
    }

    #[test]
    fn a_tap_near_a_small_block_still_hits_it() {
        let mut h = Harness::new(App::sim());
        h.frame(vec![Event::Zoom(0.5)]);
        h.frame(vec![]);
        let (at, width) = node(sim_view(&h), write_to("LAMP"));
        // Just right of the block (assignments are rightmost), within half
        // a tap target.
        let near = at + Vec2::new(width / 2.0 + TAP / 2.0 - 4.0, 0.0);
        h.click(near);
        assert_eq!(
            sim_view(&h).because.as_deref(),
            Some("LAMP"),
            "zoom {}",
            sim_view(&h).cams[0].zoom
        );
        // Far from anything: nothing new is picked.
        assert_eq!(node_near(sim_view(&h).diagram(), -500.0, -500.0, 22.0), None);
    }

    #[test]
    fn scroll_pans_pinch_zooms_and_the_instance_view_switches_diagrams() {
        let mut h = stations();
        // Between the ST (files 230, ST 440) and the right panel.
        let centre = Pos2::new(h.rect("right").left() - 150.0, 450.0);
        let before = sim_view(&h).cams[0];
        h.frame(vec![
            Event::PointerMoved(centre),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -120.0),
                modifiers: Modifiers::NONE,
                phase: eframe::egui::TouchPhase::Move,
                source: eframe::egui::MouseWheelSource::Wheel,
            },
        ]);
        h.wait(0.2);
        let panned = sim_view(&h).cams[0];
        assert!(panned.offset.y < before.offset.y, "{before:?} -> {panned:?}");
        h.frame(vec![Event::Zoom(1.5)]);
        assert!(sim_view(&h).cams[0].zoom > panned.zoom);
        let v = h.app.sim.as_mut().unwrap().viewer.as_mut().unwrap();
        v.open(1);
        h.frame(vec![]);
        assert_eq!(sim_view(&h).diagram().pou, "STATION");
    }

    /// Nick, 2026-11-01: the Trend runs along the bottom, where its time
    /// axis is long, not in the side panel's tabs.
    #[test]
    fn the_trend_runs_along_the_bottom_and_the_tabs_are_io_and_agent() {
        let mut h = Harness::new(App::sim());
        assert_eq!(h.app.chrome.tab, Tab::Io);
        h.wait(0.3);
        let panel = h.rect("right");
        for (key, tab) in [("tab claude", Tab::Claude), ("tab io", Tab::Io)] {
            h.press(key);
            assert_eq!(h.app.chrome.tab, tab);
        }
        h.wait(0.3);
        // The Agent tab's content fits the panel: at most the scroll bar's
        // width more, never the rows that used to widen it by a third.
        let grown = panel.left() - h.rect("right").left();
        assert!((0.0..=8.0).contains(&grown), "the panel grew {grown} pt");
        assert!(!h.has("tab trend"), "no Trend tab");
        assert!(h.app.chrome.trend, "open by default");
        let s = &mut h.app.sim.as_mut().unwrap().session;
        s.trend.toggle("LAMP");
        h.wait(0.2);
        let s = &h.app.sim.as_ref().unwrap().session;
        assert!(!s.trend.samples.is_empty(), "it records while shown");
        h.key(Modifiers::ALT, Key::Num2);
        assert!(!h.app.chrome.trend, "Alt+2 folds it");
    }

    /// A resizable panel grows to any row that overflows it, every frame
    /// (four such rows were found on 2026-11-01). Every tab of every
    /// panel, visited; the panels end where they began, at most a scroll
    /// bar's width wider.
    #[test]
    fn every_tab_leaves_the_panels_their_width() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let left = h.rect("left");
        let right = h.rect("right");
        for key in [
            "pane files",
            "pane code",
            "tab claude",
            "tab io",
            "bottom messages",
            "bottom trend",
            "pane files",
            "tab claude",
        ] {
            h.press(key);
            h.wait(0.3);
            let l = h.rect("left").right() - left.right();
            let r = right.left() - h.rect("right").left();
            assert!((0.0..=8.0).contains(&l), "after {key} the left pane grew {l} pt");
            assert!(
                (0.0..=8.0).contains(&r),
                "after {key} the right panel grew {r} pt"
            );
        }
    }
}

// ---- the Editor on a box ----

/// The box's Editor, driven headless: the state a browser built from the
/// real API, drawn and clicked through egui.
mod boxed {
    use super::{Harness, node, write_to};
    use crate::app::{App, Source};
    use crate::box_tests::{Bench, watch};
    use crate::box_view::BoxView;
    use crate::editor::Tab;
    use crate::remote::{Link, Msg, Stream};

    fn boxed(h: &Harness) -> &BoxView {
        h.app.boxv.as_ref().unwrap()
    }

    #[test]
    fn a_live_box_is_drawn_clicked_and_read_only_and_losing_it_offers_the_simulation() {
        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(App::boxed());
        assert!(boxed(&h).view.is_none());
        assert_eq!(boxed(&h).waiting(), "Looking for the box…");
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.frame(vec![]);
        h.header_is_up();
        assert_eq!(h.app.title(), "cell", "the module open");
        let b = boxed(&h);
        let v = b.view.as_ref().expect("the module is drawn");
        assert_eq!(v.because, None, "opens with no path lit");
        assert_eq!(b.status(h.time).0, "● connected");
        assert!(
            !h.has("tab trend") && !h.has("tab claude"),
            "I/O alone on a box (the Trend is along the bottom); Claude comes later"
        );
        assert_eq!(h.app.chrome.tab, Tab::Io);

        // The ST open beside the diagram, read-only.
        h.view_menu("view st");
        h.wait(0.4);
        let src = boxed(&h).view.as_ref().unwrap().src.clone();
        let st = h
            .ctx
            .read_response(eframe::egui::Id::unique("box-st"))
            .expect("the ST");
        h.click(st.rect.left_top() + eframe::egui::vec2(40.0, 6.0));
        h.frame(vec![eframe::egui::Event::Text("x".into())]);
        h.frame(vec![]);
        assert_eq!(
            boxed(&h).view.as_ref().unwrap().src,
            src,
            "typing changes nothing"
        );

        // Click the conveyor's assignment: the ST goes to its line.
        let (at, _) = node(boxed(&h).view.as_ref().unwrap(), write_to("CONVEYOR"));
        h.click(at);
        let (line, text) = boxed(&h).view.as_ref().unwrap().picked().unwrap();
        assert_eq!(text, "CONVEYOR := M.ON");
        assert_eq!(h.app.chrome.line, Some(line));
        assert_eq!(
            src.lines().nth(line as usize - 1).unwrap().trim(),
            "conveyor := m.on;"
        );

        // The box goes away: reconnecting, then unreachable, still drawn.
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        b.state.apply(Msg::Failed(Stream::Values), t);
        assert!(
            b.status(t).0.starts_with("● reconnecting (retry in 5s)"),
            "{:?}",
            b.status(t)
        );
        for k in 1..3 {
            b.state.apply(Msg::Failed(Stream::Values), t + f64::from(k));
        }
        assert!(matches!(b.state.link(), Link::Unreachable { .. }));
        assert!(
            b.status(t + 2.0).0.starts_with("● box unreachable"),
            "{:?}",
            b.status(t + 2.0)
        );
        h.frame(vec![]);
        assert!(boxed(&h).view.is_some(), "the last diagram stays up");
        h.press("simulate");
        assert_eq!(
            h.app.source,
            Source::Sim,
            "the operator can fall back to the simulation"
        );
        h.frame(vec![]);
        assert!(h.app.sim.is_some());
        h.header_is_up();
    }
}

mod fallback {
    use super::Harness;
    use crate::app::{App, Source};
    use crate::remote::{Link, Msg, Stream};

    /// Nick, 2026-11-01: a page that finds no box runs the simulation
    /// by itself; a box that answered and went away keeps its view.
    #[test]
    fn a_page_no_box_ever_answered_falls_back_to_the_simulation_by_itself() {
        let mut h = Harness::new(App::boxed());
        assert_eq!(h.app.source, Source::Box);
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        assert!(!b.state.ever_connected());
        for k in 0..3 {
            b.state.apply(Msg::Failed(Stream::Values), t + f64::from(k));
        }
        assert!(matches!(b.state.link(), Link::Unreachable { .. }));
        h.frame(vec![]);
        assert_eq!(h.app.source, Source::Sim, "the simulation, unasked");
        h.frame(vec![]);
        let s = h.app.sim.as_ref().expect("running");
        assert!(
            s.notice.as_deref().is_some_and(|n| n.contains("No box answered")),
            "{:?}",
            s.notice
        );
        // Back to the box from View stays: the fallback happens once.
        h.press("bar view");
        h.press("view box");
        assert_eq!(h.app.source, Source::Box);
        h.wait(0.2);
        assert_eq!(h.app.source, Source::Box, "not yanked back to the simulation");
    }
}

// ---- proposals, behind the ⋯ menu ----

mod reviewing {
    use eframe::egui::{Event, Pos2, Rect, Vec2};

    use super::{Harness, Screen};
    use crate::app::App;
    use crate::review::{Answer, Phase, Replay, Target};
    use crate::review_tests::Bench;
    use crate::review_view::ReviewView;

    fn rv(h: &mut Harness) -> &mut ReviewView {
        h.app.review.as_mut().unwrap()
    }

    fn proposals() -> Harness {
        let mut h = Harness::new(App::boxed());
        h.app.screen = Screen::Proposals;
        h.frame(vec![]);
        h
    }

    /// Carries out what the screen sent, feeds the answers back, draws.
    fn pump(h: &mut Harness, b: &Bench) {
        for _ in 0..20 {
            // A click's request goes out on the frame after it.
            h.frame(vec![]);
            let sent = std::mem::take(&mut rv(h).sent);
            if sent.is_empty() {
                return;
            }
            let answers: Vec<_> = sent.iter().map(|q| (q.call.clone(), b.send(q))).collect();
            let (ctx, t) = (h.ctx.clone(), h.time);
            rv(h).step(&ctx, answers, t);
            h.frame(vec![]);
        }
        panic!("the screen never went quiet");
    }

    fn rect(h: &mut Harness, key: &str) -> Rect {
        let rects = &rv(h).rects;
        let keys: Vec<&String> = rects.iter().map(|(k, _)| k).collect();
        rects
            .iter()
            .find(|(k, _)| k == key)
            .map_or_else(|| panic!("no {key} on screen: {keys:?}"), |(_, r)| *r)
    }

    fn click(h: &mut Harness, key: &str) {
        let at = rect(h, key).center();
        h.click(at);
    }

    fn type_into(h: &mut Harness, key: &str, text: &str) {
        click(h, key);
        h.frame(vec![Event::Text(text.into())]);
        h.frame(vec![]);
    }

    /// Everything drawn fits the window's width: nothing to scroll sideways
    /// for on a tablet.
    fn fits(h: &mut Harness) {
        let w = h.size.x;
        let rects = rv(h).rects.clone();
        assert!(rects.len() > 3, "{rects:?}");
        for (k, r) in rects {
            assert!(r.right() <= w + 0.5, "{k} at {r:?} is past the {w} pt window");
        }
    }

    #[test]
    fn a_waiting_proposal_badges_the_menu_from_any_screen() {
        let b = Bench::new();
        let mut h = Harness::new(App::boxed());
        assert_eq!(h.app.screen, Screen::Editor);
        pump(&mut h, &b);
        assert_eq!(h.app.waiting(), Some(0));
        b.horn();
        // The list is read again every 30 s, without opening the screen.
        h.time += 31.0;
        pump(&mut h, &b);
        assert_eq!(h.app.waiting(), Some(1), "the draft waits");
        assert_eq!(h.app.screen, Screen::Editor);
        assert!(h.has("proposals waiting"), "the header badges it");
        h.press("proposals waiting");
        assert_eq!(h.app.screen, Screen::Proposals);
        pump(&mut h, &b);
        assert_eq!(rv(&mut h).state.list.len(), 1);
    }

    #[test]
    fn an_engineer_signs_in_with_a_pin_reviews_replays_and_approves_by_clicking() {
        let b = Bench::new();
        let id = b.horn();
        let mut h = proposals();
        // Tall enough that the whole review is on screen without scrolling.
        h.size = Vec2::new(1400.0, 4000.0);
        h.frame(vec![]);
        pump(&mut h, &b);
        click(&mut h, &format!("row {id}"));
        pump(&mut h, &b);
        assert_eq!(rv(&mut h).state.open.as_ref().map(|o| o.id), Some(id));

        // Not signed in: no token field anywhere, approving is off, and the
        // panel asks for a role and its PIN (the box has no accounts).
        let keys: Vec<String> = rv(&mut h).rects.iter().map(|(k, _)| k.clone()).collect();
        for gone in ["token", "use", "user", "password"] {
            assert!(!keys.contains(&gone.to_string()), "{gone} on screen: {keys:?}");
        }
        for k in [
            "role operator",
            "role engineer",
            "role integrator",
            "pin",
            "login",
        ] {
            assert!(keys.contains(&k.to_string()), "no {k}: {keys:?}");
        }
        let t = Target::Proposal(id);
        click(&mut h, &format!("go {t:?}"));
        pump(&mut h, &b);
        assert!(
            matches!(rv(&mut h).state.open.as_ref().unwrap().step.phase, Phase::Idle),
            "a disabled button sends nothing"
        );

        click(&mut h, "role engineer");
        assert_eq!(rv(&mut h).role_input, "engineer");
        type_into(&mut h, "pin", "eng-135797");
        assert_eq!(rv(&mut h).pin_input, "eng-135797");
        click(&mut h, "login");
        h.frame(vec![]);
        let sent = rv(&mut h).sent.clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].body.as_deref().unwrap().contains("eng-135797"));
        assert!(!format!("{sent:?}").contains("eng-135797"), "redacted in Debug");
        assert_eq!(rv(&mut h).pin_input, "", "cleared on submit");
        pump(&mut h, &b);
        assert_eq!(rv(&mut h).state.signed_in().as_deref(), Some("engineer (PIN)"));
        assert!(b.jar.session().is_some(), "the browser holds the cookie");
        let keys: Vec<String> = rv(&mut h).rects.iter().map(|(k, _)| k.clone()).collect();
        assert!(keys.contains(&"logout".to_string()), "{keys:?}");
        assert!(!keys.contains(&"pin".to_string()), "{keys:?}");

        click(&mut h, &format!("replay {t:?}"));
        pump(&mut h, &b);
        let replay = rv(&mut h).state.open.as_ref().unwrap().step.replay.clone();
        assert!(matches!(replay, Replay::Done(_)), "{replay:?}");

        // Approve as the engineer signed in: no role, no PIN in the body.
        click(&mut h, &format!("go {t:?}"));
        h.frame(vec![]);
        let sent = rv(&mut h).sent.clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        let body = sent[0].body.as_deref().unwrap();
        assert!(!body.contains("pin") && !body.contains("role"), "{body}");
        pump(&mut h, &b);
        let phase = rv(&mut h).state.open.as_ref().unwrap().step.phase.clone();
        assert!(matches!(phase, Phase::Applied(_)), "{phase:?}");
        assert!(
            rv(&mut h).state.open.as_ref().unwrap().undo.is_some(),
            "the undo is offered"
        );
        assert_eq!(h.app.waiting(), Some(0), "nothing waits once applied");

        // Readable on a tablet either way up, and on a small laptop.
        for size in [
            Vec2::new(800.0, 1280.0),
            Vec2::new(1280.0, 800.0),
            Vec2::new(1024.0, 768.0),
        ] {
            h.size = size;
            h.frame(vec![]);
            h.frame(vec![]);
            fits(&mut h);
            h.header_is_up();
        }

        // Sign out: the box ends it and clears the cookie; the panel asks
        // for a PIN again and approving is off.
        h.size = Vec2::new(1400.0, 4000.0);
        h.frame(vec![]);
        click(&mut h, "logout");
        pump(&mut h, &b);
        assert_eq!(rv(&mut h).state.signed_in(), None);
        assert_eq!(b.jar.session(), None);
        let keys: Vec<String> = rv(&mut h).rects.iter().map(|(k, _)| k.clone()).collect();
        assert!(keys.contains(&"pin".to_string()), "{keys:?}");
    }

    #[test]
    fn on_a_box_with_accounts_the_panel_asks_for_a_user_and_a_session_end_brings_it_back() {
        let b = Bench::new();
        let mut accounts = helmsman_plc::policy::account::Accounts::default();
        accounts
            .add(
                "alice",
                helmsman_plc::policy::Role::Engineer,
                "alice-pass-31337",
                1,
                [1; 16],
            )
            .unwrap();
        accounts.save(b.state_dir()).unwrap();
        let mut h = proposals();
        h.size = Vec2::new(1400.0, 2000.0);
        h.frame(vec![]);
        pump(&mut h, &b);
        let keys: Vec<String> = rv(&mut h).rects.iter().map(|(k, _)| k.clone()).collect();
        for k in ["user", "password", "login"] {
            assert!(keys.contains(&k.to_string()), "no {k}: {keys:?}");
        }
        assert!(
            !keys.iter().any(|k| k.starts_with("role ") || k == "pin"),
            "{keys:?}"
        );
        type_into(&mut h, "user", "alice");
        type_into(&mut h, "password", "alice-pass-31337");
        click(&mut h, "login");
        pump(&mut h, &b);
        assert_eq!(rv(&mut h).state.signed_in().as_deref(), Some("alice (engineer)"));
        assert_eq!(rv(&mut h).password_input, "", "cleared on submit");

        // The box ends the session (here: her account is disabled): the
        // next call is 401 and the sign-in panel is back.
        accounts.set_disabled("alice", true).unwrap();
        accounts.save(b.state_dir()).unwrap();
        rv(&mut h).state.refresh();
        pump(&mut h, &b);
        assert_eq!(rv(&mut h).state.signed_in(), None);
        let keys: Vec<String> = rv(&mut h).rects.iter().map(|(k, _)| k.clone()).collect();
        assert!(keys.contains(&"password".to_string()), "{keys:?}");
        assert_eq!(b.jar.session(), None, "the box cleared the cookie");
    }

    #[test]
    fn the_screen_without_a_box_says_so_and_survives_clicks_everywhere() {
        let mut h = proposals();
        let calls: Vec<_> = rv(&mut h).sent.iter().map(|q| q.call.clone()).collect();
        assert_eq!(
            calls.len(),
            4,
            "the list, the outputs, the policy and who is signed in: {calls:?}"
        );
        let ctx = h.ctx.clone();
        let answers = std::mem::take(&mut rv(&mut h).sent)
            .into_iter()
            .map(|q| {
                let a = Answer {
                    status: 0,
                    retry_after: None,
                    body: "connection refused".into(),
                };
                (q.call, a)
            })
            .collect();
        rv(&mut h).step(&ctx, answers, 0.0);
        h.frame(vec![]);
        let e = rv(&mut h).state.list_error.clone().unwrap();
        assert!(e.contains("connection refused"), "{e}");
        // Below the header (its controls leave this screen, as they should).
        let below = h.rect("header").bottom() + 4.0;
        for x in (0..1400u16).step_by(97) {
            for y in (0..860u16).step_by(83) {
                let y = f32::from(y).max(below);
                h.click(Pos2::new(f32::from(x), y));
            }
        }
        assert!(rv(&mut h).state.open.is_none());
        assert_eq!(h.app.screen, Screen::Proposals, "{:?}", h.app.screen);
    }
}

#[test]
fn the_simulation_survives_a_click_storm_on_every_tab() {
    let mut h = Harness::new(App::sim());
    for tab in ["tab io", "tab claude"] {
        h.press(tab);
        let below = h.rect("header").bottom() + 4.0;
        for x in (0..1400u16).step_by(131) {
            for y in (0..860u16).step_by(97) {
                h.click(Pos2::new(f32::from(x), f32::from(y).max(below)));
            }
        }
        h.header_is_up();
        assert_eq!(h.app.screen, Screen::Editor);
        assert_eq!(h.app.source, Source::Sim);
    }
}
