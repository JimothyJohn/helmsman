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
    /// What the last frame drew.
    shapes: Vec<egui::epaint::ClippedShape>,
    /// The pointer the last frame asked for.
    cursor: egui::CursorIcon,
    /// Text the app put on the clipboard, newest last.
    copied: Vec<String>,
}

impl Harness {
    fn new(app: App) -> Self {
        let mut h = Self {
            ctx: egui::Context::default(),
            app,
            time: 0.0,
            size: Vec2::new(1400.0, 860.0),
            shapes: Vec::new(),
            cursor: egui::CursorIcon::Default,
            copied: Vec::new(),
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
        self.shapes = out.shapes;
        self.cursor = out.platform_output.cursor_icon;
        for c in out.platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                self.copied.push(t);
            }
        }
    }

    /// Every piece of text the last frame drew outside the diagram (which
    /// zooms) below [`crate::theme::MIN_TEXT`]: its size and the text.
    fn small_text(&self) -> Vec<(f32, String)> {
        fn walk(shape: &egui::Shape, clip: Rect, diagram: Rect, out: &mut Vec<(f32, String)>) {
            match shape {
                egui::Shape::Vec(v) => {
                    for s in v {
                        walk(s, clip, diagram, out);
                    }
                }
                egui::Shape::Text(t) => {
                    if diagram.is_positive() && diagram.contains_rect(clip) {
                        return;
                    }
                    let job = &t.galley.job;
                    for sec in &job.sections {
                        let text = job
                            .text
                            .get(sec.byte_range.start.0..sec.byte_range.end.0)
                            .unwrap_or("");
                        if !text.trim().is_empty() && sec.format.font_id.size < crate::theme::MIN_TEXT - 0.01
                        {
                            out.push((sec.format.font_id.size, text.trim().chars().take(60).collect()));
                        }
                    }
                }
                _ => {}
            }
        }
        let diagram = self.app.chrome.diagram;
        let mut out = Vec::new();
        for c in &self.shapes {
            walk(&c.shape, c.clip_rect, diagram, &mut out);
        }
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out.dedup();
        out
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

    /// A press at `from`, moved in steps to `to`, released there.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Self::button(from, true)]);
        for k in 1..=8 {
            #[allow(clippy::cast_precision_loss)]
            self.frame(vec![Event::PointerMoved(from.lerp(to, k as f32 / 8.0))]);
        }
        self.frame(vec![Self::button(to, false)]);
        self.frame(vec![]);
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

    /// Whether the last frame drew a piece of text containing `needle`.
    fn drew(&self, needle: &str) -> bool {
        fn walk(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Vec(v) => v.iter().any(|s| walk(s, needle)),
                egui::Shape::Text(t) => t.galley.job.text.contains(needle),
                _ => false,
            }
        }
        self.shapes.iter().any(|c| walk(&c.shape, needle))
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
        // The Monitor has no diagram, so no file glyphs; the transport is
        // in its header (Nick, 2026-11-01).
        h.press("monitor");
        assert_eq!(h.app.screen, Screen::Monitor);
        h.frame(vec![]);
        assert!(h.has("run") && !h.has("download"));
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

    /// Nick, 2026-11-07: "I really need to be able to copy and paste
    /// errors". An error line under the header has a Copy button, the
    /// Messages tab a Copy all, and both put the whole text on the
    /// clipboard.
    #[test]
    fn errors_copy_to_the_clipboard() {
        let mut h = Harness::new(App::sim());
        h.wait(0.2);
        h.app.sim.as_mut().unwrap().session.fault = Some("division by zero at line 7".into());
        h.wait(0.2);
        h.press("fault copy");
        assert_eq!(
            h.copied.last().map(String::as_str),
            Some("Fault: division by zero at line 7. Run or Step restarts the program.")
        );
        h.app
            .sim
            .as_mut()
            .unwrap()
            .log
            .error(1.0, "deploy refused: no engineer session");
        h.press("bottom messages");
        h.frame(vec![]);
        h.press("messages copy");
        let all = h.copied.last().expect("Copy all copied").clone();
        assert!(
            all.contains("error   deploy refused: no engineer session"),
            "{all}"
        );
        assert_eq!(
            all,
            h.app.sim.as_ref().unwrap().log.text(),
            "what is shown, as text"
        );
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

    /// Nick, 2026-11-01: dark by default, View › Light mode flips the
    /// page, and the choice is kept per browser with the layout.
    #[test]
    fn light_mode_is_a_view_item_off_by_default_and_remembered() {
        use crate::editor::Chrome;
        let mut h = Harness::new(App::sim());
        let dark = |h: &Harness| h.ctx.global_style().visuals.dark_mode;
        assert!(dark(&h), "a fresh app is dark");
        assert!(!h.app.chrome.light);
        assert!(h.app.chrome.saved().contains("light=0"));
        h.view_menu("view light");
        h.frame(vec![]);
        assert!(h.app.chrome.light);
        assert!(!dark(&h), "the item flips egui's visuals");
        let saved = h.app.chrome.saved();
        assert!(saved.contains("light=1"), "{saved}");
        assert!(Chrome::restored(&saved).light, "read back");
        assert!(!Chrome::restored("left=1").light, "absent: dark");
        h.view_menu("view light");
        h.frame(vec![]);
        assert!(!h.app.chrome.light && dark(&h), "and back");
        // A browser that chose light opens light on its first frame.
        let mut app = App::sim();
        app.chrome = Chrome::restored("light=1");
        let h = Harness::new(app);
        assert!(!dark(&h));
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
        h.key(Modifiers::ALT, Key::Num4);
        assert!(h.app.chrome.devices_open(), "Alt+4: the Devices tab");
        h.key(Modifiers::ALT, Key::Num4);
        assert!(!h.app.chrome.left, "Alt+4 on the Devices tab shuts the pane");
        h.key(Modifiers::ALT, Key::Num1);
        assert!(h.app.chrome.code_open());
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
    fn ctrl_b_on_a_picked_block_pauses_the_scan_there_and_f10_runs_the_rest() {
        fn session(h: &Harness) -> &crate::playground::Session {
            &h.app.sim.as_ref().unwrap().session
        }
        let mut h = Harness::new(App::sim());
        h.key(Modifiers::ALT, Key::Num1);
        h.wait(0.6);
        let (at, _) = super::node(super::sim_view(&h), super::write_to("LAMP"));
        h.click(at);
        h.frame(vec![]);
        let id = super::sim_view(&h).picked_stmt().expect("a statement picked");
        h.key(Modifiers::COMMAND, Key::B);
        assert_eq!(session(&h).breaks, vec![id], "Ctrl B sets the breakpoint");
        assert_eq!(super::sim_view(&h).breaks, vec![id], "and the diagram marks it");
        // Stopped, F10 steps a scan: it stops before the statement.
        h.key(Modifiers::SHIFT, Key::F8);
        assert!(!session(&h).running);
        if session(&h).paused.is_some() {
            // It was running and stopped there on its own: finish that scan.
            h.key(Modifiers::NONE, Key::F10);
        }
        // The lamp's write is under `IF pulse.Q`: F10 steps scans until
        // the timer fires and the scan stops before it.
        let mut steps = 0;
        while session(&h).paused.is_none() && steps < 200 {
            h.key(Modifiers::NONE, Key::F10);
            steps += 1;
        }
        assert_eq!(session(&h).paused_at(), Some(id), "after {steps} steps");
        let scans = session(&h).scans;
        assert_eq!(super::sim_view(&h).paused, Some(id), "the diagram shows where");
        h.key(Modifiers::NONE, Key::F10);
        assert_eq!(session(&h).paused, None, "F10 again runs the rest");
        assert_eq!(session(&h).scans, scans + 1);
        h.key(Modifiers::COMMAND, Key::B);
        assert!(session(&h).breaks.is_empty(), "Ctrl B again clears it");
        h.press("bar edit");
        assert!(h.has("edit breakpoint"), "the Edit menu lists it with its key");
    }

    #[test]
    fn ctrl_shift_f_flips_the_picked_switch_and_opens_a_number_field() {
        use helmsman_iec::Value;
        let mut h = Harness::new(App::sim());
        h.app.sim.as_mut().unwrap().load(
            "PROGRAM t
VAR_INPUT enable : BOOL := TRUE; speed : INT; END_VAR
VAR_OUTPUT lamp : BOOL; out : INT; END_VAR
lamp := enable;
out := speed * 2;
END_PROGRAM",
            "force test",
        );
        h.wait(0.1);
        let pick = |h: &mut Harness, name: &str| {
            let v = h.app.sim.as_mut().unwrap().viewer.as_mut().unwrap();
            let n = v
                .diagram()
                .nodes
                .iter()
                .find(|n| matches!(&n.kind, helmsman_diagram::NodeKind::Read { path } if path.eq_ignore_ascii_case(name)))
                .expect("a read")
                .id;
            v.pick = Some(crate::st_view::Pick::Node(n));
            v.picked_read().expect("a read picked").0
        };
        let value = |h: &Harness, name: &str| h.app.sim.as_ref().unwrap().session.value(name);
        pick(&mut h, "enable");
        assert_eq!(value(&h, "enable"), Some(Value::Bool(true)));
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::F);
        assert_eq!(
            value(&h, "enable"),
            Some(Value::Bool(false)),
            "the switch flipped"
        );
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::F);
        assert_eq!(value(&h, "enable"), Some(Value::Bool(true)), "and back");
        let k = pick(&mut h, "speed");
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::F);
        h.frame(vec![]);
        let field = egui::Id::unique(("pg-map-number", k));
        assert!(
            h.ctx.memory(|m| m.has_focus(field)),
            "the speed's field has the keyboard"
        );
        // Nothing to release in the simulation; the key does nothing.
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::U);
        h.press("bar edit");
        assert!(h.has("edit force"));
        assert!(!h.has("edit release"), "release all is a box's");
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

    /// Nick, 2026-11-08: an arrow right of the Structured Text header,
    /// and one beside the running program in Files, deploys the latest
    /// edit; neither widens the pane.
    #[test]
    fn the_arrows_by_the_code_header_and_the_running_program_deploy_the_edit() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let left = h.rect("left");
        h.press("pane code");
        h.wait(0.3);
        assert!(h.has("code deploy"), "the arrow by the header");
        let edit = |h: &mut Harness, mark: &str| {
            let sim = h.app.sim.as_mut().unwrap();
            let current = sim.proj.current.clone();
            sim.proj.files.get_mut(&current).unwrap().push_str(mark);
            h.frame(vec![]);
        };
        let deployed = |h: &Harness| {
            h.app
                .sim
                .as_ref()
                .unwrap()
                .session
                .build
                .as_ref()
                .unwrap()
                .src
                .clone()
        };
        edit(&mut h, "\n(* one *)");
        assert!(!deployed(&h).contains("(* one *)"), "an edit is only checked");
        h.press("code deploy");
        h.wait(0.3);
        assert!(deployed(&h).contains("(* one *)"), "the arrow deployed it");
        let sim = h.app.sim.as_ref().unwrap();
        assert_eq!(sim.proj.running.as_deref(), Some(sim.proj.open.as_str()));
        // In Files the running program, open, carries the arrow too.
        h.press("pane files");
        h.wait(0.3);
        let arrow = h.rect("deploy arrow");
        let row = h.rect(&format!("project {}", h.app.sim.as_ref().unwrap().proj.open));
        assert!(
            arrow.left() >= row.right() - 1.0,
            "beside the row: {arrow:?} vs {row:?}"
        );
        assert!(arrow.right() <= h.rect("left").right(), "inside the pane");
        edit(&mut h, "\n(* two *)");
        h.press("deploy arrow");
        h.wait(0.3);
        assert!(deployed(&h).contains("(* two *)"), "the Files arrow deployed it");
        let grew = h.rect("left").right() - left.right();
        assert!((0.0..=8.0).contains(&grew), "the left pane grew {grew} pt");
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

    /// Nick, 2026-11-01: the Monitor needs play, start, stop. In the
    /// simulation its header carries the same run or stop, step and
    /// restart as the Editor's glyphs, and the keys work there too. A
    /// box's Monitor stays watch-only (card monitor-box-run-stop).
    #[test]
    fn the_monitor_runs_stops_steps_and_restarts_the_simulation() {
        let mut h = Harness::new(App::sim());
        h.press("monitor");
        assert_eq!(h.app.screen, Screen::Monitor);
        h.frame(vec![]);
        for k in ["run", "step", "restart"] {
            assert!(h.has(k), "the Monitor has {k}");
        }
        let running = |h: &Harness| h.app.sim.as_ref().unwrap().session.running;
        let scans = |h: &Harness| h.app.sim.as_ref().unwrap().session.scans;
        assert!(running(&h), "the example opens running");
        h.press("run");
        assert!(!running(&h), "stopped from the Monitor");
        let n = scans(&h);
        h.press("step");
        assert_eq!(scans(&h), n + 1, "one scan");
        h.press("restart");
        assert_eq!(scans(&h), 0, "a cold restart");
        h.press("run");
        assert!(running(&h), "and running again");
        // The keys, on the Monitor: Ctrl Enter stops, F10 steps.
        h.key(Modifiers::COMMAND, Key::Enter);
        assert!(!running(&h), "Ctrl Enter stops on the Monitor");
        let n = scans(&h);
        h.key(Modifiers::NONE, Key::F10);
        assert_eq!(scans(&h), n + 1, "F10 steps on the Monitor");
        // Still no Save, Upload or Download there: nothing to edit.
        assert!(!h.has("save") && !h.has("download"));
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

    /// Nick, 2026-11-05: no chat. The Agent tab is the instruction box,
    /// Send, and after an answer a status line with the reason and Undo;
    /// an instruction typed and sent updates the file, and Undo from the
    /// tab puts it back. The hit stays "tab claude" (the stored key).
    #[test]
    fn the_agent_tab_is_an_instruction_box_a_status_line_and_undo() {
        use crate::instruct::Kind;
        use crate::instruct_tests::{LAMP, follows};
        let mut h = Harness::new(App::sim());
        h.press("tab claude");
        h.wait(0.2);
        {
            let v = h.app.sim.as_mut().unwrap();
            v.chat.receive(crate::claude::Incoming::Proxy(true));
            v.proj.create("mine", None);
            v.proj.files.insert("main.st".into(), LAMP.into());
            v.proj.save();
            v.open_project("mine");
        }
        h.wait(0.2);
        for gone in ["page prev", "use code", "agent new", "agent result", "agent undo"] {
            assert!(!h.has(gone), "{gone}");
        }
        assert!(h.has("agent instruction") && h.has("agent send"));
        h.click(h.rect("agent instruction").center());
        h.frame(vec![Event::Text("comment it".into())]);
        h.press("agent send");
        h.frame(vec![]);
        assert!(
            h.has("agent stop") && h.has("agent working"),
            "Stop while it works"
        );
        let new = LAMP.replace("lamp := start;", "lamp := start; (* follows start *)");
        let answer = format!(
            "Why: it says why the lamp is on.\n\n```st\n{new}```\n\n{}",
            follows()
        );
        crate::instruct_tests::reply(h.app.sim.as_mut().unwrap(), &answer);
        h.wait(0.2);
        assert!(h.has("agent result") && h.has("agent why") && h.has("agent undo"));
        {
            let v = h.app.sim.as_ref().unwrap();
            assert_eq!(v.proj.files["main.st"], new);
            assert_eq!(
                v.instruct.outcome.as_ref().map(|o| o.status.as_str()),
                Some("Updated main.st: 1 scenario passed, kept as a test.")
            );
        }
        h.press("agent undo");
        h.wait(0.2);
        let v = h.app.sim.as_ref().unwrap();
        assert_eq!(v.proj.files["main.st"], LAMP);
        assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Undone));
        assert!(!h.has("agent undo"));
    }

    /// Every outcome the tab can show, with long words, and the tab while
    /// the Agent fixes an answer: the panels keep their width throughout.
    #[test]
    fn the_agent_tabs_outcomes_leave_the_panels_their_width() {
        use crate::instruct::{Attempt, Kind, Outcome, Run, Undo};
        let mut h = Harness::new(App::sim());
        h.press("tab claude");
        h.wait(0.2);
        let left = h.rect("left");
        let right = h.rect("right");
        let long = "Scenario The lamp stays off while start is not pressed and the conveyor_infeed_photo_eye_blocked_for_too_long_alarm holds (test_lamp_off_while_start_is_not_pressed.st) failed: lamp off failed at 0.04 s".to_string();
        let word = "x".repeat(400);
        let outcome = |kind, status: &str| Outcome {
            kind,
            status: status.into(),
            why: Some(format!("the lamp now latches {word}")),
            problems: vec![long.clone(), long.clone()],
            undo: Some(Undo {
                project: "p".into(),
                file: "main.st".into(),
                revision: 3,
                text: String::new(),
            }),
            review: vec![3, 4],
        };
        let states = [
            (
                outcome(Kind::Applied, &format!("Updated {word}.st: 2 scenarios passed")),
                "agent why",
            ),
            (outcome(Kind::Refused, &long), "agent result"),
            (outcome(Kind::Said, &long), "agent review"),
            (outcome(Kind::Failed, "stopped"), "agent result"),
            (outcome(Kind::Undone, &long), "agent undo"),
        ];
        for (o, key) in states {
            h.app.sim.as_mut().unwrap().instruct.outcome = Some(o);
            h.wait(0.2);
            assert!(h.has(key), "{key}");
            assert!((h.rect("left").right() - left.right()).abs() <= 8.0, "{key}");
            assert!((right.left() - h.rect("right").left()).abs() <= 8.0, "{key}");
        }
        // Fixing an answer: what failed is the tooltip, not a page of text.
        h.app.sim.as_mut().unwrap().instruct.run = Some(Run {
            instruction: word.clone(),
            project: "p".into(),
            scope: crate::scope::Scope::File(format!("{word}.st")),
            earlier: Vec::new(),
            asked: String::new(),
            retries: 1,
            asked_scenarios: false,
            attempts: vec![Attempt {
                asked: String::new(),
                answer: String::new(),
                problems: vec![long],
            }],
        });
        h.wait(0.2);
        assert!(h.has("agent working") && h.has("agent stop"));
        assert!(!h.has("agent send") && !h.has("agent result"));
        assert!((right.left() - h.rect("right").left()).abs() <= 8.0);
    }

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
    use eframe::egui::{self, Event, Key, Modifiers, Pos2, Rect, Vec2};

    use super::{Harness, node, sim_view, write_to};
    use crate::app::{App, Screen, Source, wants_sim};
    use crate::editor::{Chrome, Pane, STRIP, Tab};
    use crate::projects::Act;

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
        let devices = h.rect("pane devices");
        assert!(
            files.right() <= devices.left() && devices.right() <= code.left(),
            "Files | Devices | Code, left to right"
        );
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

    /// Nick, 2026-11-01: the toolbar's dropdowns and the right-click
    /// menus were three times wider than their text. A menu is as wide
    /// as its widest item, every item filling that width.
    #[test]
    fn menus_are_as_wide_as_their_widest_item() {
        let mut h = Harness::new(App::sim());
        let menu = |h: &mut Harness, open: &str, items: &[&str]| {
            h.press(open);
            // The frame the menu opens on is egui's sizing pass, where
            // nothing is justified; the menu shows at its size the next.
            h.frame(vec![]);
            let rects: Vec<Rect> = items.iter().map(|k| h.rect(k)).collect();
            let widest = rects.iter().map(Rect::width).fold(0.0, f32::max);
            // 220 pt at the old 12.5 pt body text, in proportion to today's.
            let cap = 220.0 * crate::theme::BODY / 12.5;
            assert!(widest <= cap, "{open}: an item {widest} pt wide");
            for (k, r) in items.iter().zip(&rects) {
                assert!(
                    (r.width() - widest).abs() < 1.0,
                    "{open}: {k} is {} pt wide against {widest}",
                    r.width()
                );
            }
            h.key(Modifiers::NONE, Key::Escape);
            h.frame(vec![]);
        };
        menu(&mut h, "bar file", &["file new", "file save", "file download"]);
        menu(&mut h, "bar edit", &["edit undo", "edit redo"]);
        menu(&mut h, "bar view", &["view st", "view files", "reset layout"]);
        h.view_menu("view files");
        h.frame(vec![]);
        let at = h.rect("file blink.st").center();
        h.right_click(at);
        let items = [
            "menu deploy",
            "menu copy",
            "menu paste",
            "menu duplicate",
            "menu rename",
            "menu delete",
        ];
        let widest = items.iter().map(|k| h.rect(k).width()).fold(0.0, f32::max);
        assert!(widest <= 220.0, "right-click: an item {widest} pt wide");
        for k in items {
            let w = h.rect(k).width();
            assert!(
                (w - widest).abs() < 1.0,
                "right-click: {k} is {w} pt wide against {widest}"
            );
        }
    }

    /// Right-click a file › Deploy runs the open project, as File ›
    /// Deploy does (Nick, 2026-11-05).
    #[test]
    fn a_files_right_click_deploys_the_project() {
        let mut h = Harness::new(App::sim());
        h.press("bar file");
        h.press("file copy");
        // The name popup settles into the centre on its second frame.
        h.wait(0.2);
        h.press("name ok");
        h.wait(0.3);
        assert_eq!(h.app.sim.as_ref().unwrap().proj.open, "blink-copy");
        assert_ne!(
            h.app.sim.as_ref().unwrap().proj.running.as_deref(),
            Some("blink-copy"),
            "a copy does not run until deployed"
        );
        h.right_click(h.rect("file blink.st").center());
        h.press("menu deploy");
        h.wait(0.3);
        assert_eq!(
            h.app.sim.as_ref().unwrap().proj.running.as_deref(),
            Some("blink-copy")
        );
    }

    /// Rename and Delete are the right-click menu's and the Delete key's,
    /// not buttons under the lists; the question is asked in the middle
    /// of the screen (Nick, 2026-11-02).
    #[test]
    fn rename_and_delete_are_the_menus_and_the_delete_key_and_ask_by_the_click() {
        let mut h = Harness::new(App::sim());
        h.press("bar file");
        h.press("file copy");
        // The name popup settles into the centre on its second frame.
        h.wait(0.2);
        h.press("name ok");
        h.wait(0.3);
        assert_eq!(h.app.sim.as_ref().unwrap().proj.open, "blink-copy");
        for k in ["rename project", "delete project", "rename file", "delete file"] {
            assert!(!h.has(k), "{k} is a button no more");
        }
        assert!(h.has("new file"), "New file stays a button");
        // Delete from the file's menu: asked in the centre, Keep says no.
        h.press("new file");
        // The name popup settles into the centre on its second frame.
        h.wait(0.2);
        h.press("name");
        h.key(Modifiers::COMMAND, Key::A);
        h.frame(vec![Event::Text("two.st".into())]);
        h.press("name ok");
        h.wait(0.3);
        assert!(h.has("file two.st"));
        h.right_click(h.rect("file two.st").center());
        let at = h.rect("menu delete").center();
        h.press("menu delete");
        h.frame(vec![]);
        let v = h.app.sim.as_ref().unwrap();
        assert_eq!(v.files_ui.confirm, Some(Act::DeleteFile("two.st".into())));
        // By the click, not across the screen (Nick, 2026-11-05: "going
        // all the way back and forth is very annoying").
        let c = h.rect("confirm");
        assert!(
            (c.top() - at.y).abs() < 60.0 && c.x_range().contains(at.x),
            "the question sits by the click at {at:?}, not at {c:?}"
        );
        h.press("confirm no");
        h.frame(vec![]);
        assert!(h.has("file two.st"), "kept");
        assert_eq!(h.app.sim.as_ref().unwrap().files_ui.confirm, None);
        // The Delete key on the file last clicked.
        h.press("file two.st");
        h.key(Modifiers::NONE, Key::Backspace);
        assert_eq!(
            h.app.sim.as_ref().unwrap().files_ui.confirm,
            Some(Act::DeleteFile("two.st".into()))
        );
        h.press("confirm yes");
        h.wait(0.3);
        assert!(!h.has("file two.st"), "deleted");
        // Escape answers no.
        h.press("project blink-copy");
        h.key(Modifiers::NONE, Key::Delete);
        assert_eq!(
            h.app.sim.as_ref().unwrap().files_ui.confirm,
            Some(Act::DeleteProject("blink-copy".into()))
        );
        h.key(Modifiers::NONE, Key::Escape);
        assert_eq!(h.app.sim.as_ref().unwrap().files_ui.confirm, None);
        assert!(h.has("project blink-copy"));
        // The Delete key on the project last clicked deletes the project.
        h.press("project blink-copy");
        h.key(Modifiers::NONE, Key::Backspace);
        h.press("confirm yes");
        h.wait(0.3);
        assert!(!h.has("project blink-copy"), "the project is gone");
        // An example is never offered: the key does nothing on blink.
        h.press("project blink");
        h.wait(0.2);
        h.key(Modifiers::NONE, Key::Backspace);
        assert_eq!(h.app.sim.as_ref().unwrap().files_ui.confirm, None);
    }

    /// A double click on a file opens it on the Code tab (Nick, 2026-11-02).
    /// Nick, 2026-11-02: the Devices tab lists the map's devices with
    /// their points under them, and adds a device the way a file is
    /// added: a right-click on the blank space, a form in the centre, one
    /// `device` line in io.map (made when the project has none). A
    /// built-in example is read-only, so its menu item is disabled.
    #[test]
    fn the_devices_tab_lists_the_map_and_adds_a_device_as_a_line_of_it() {
        let mut h = Harness::new(App::sim());
        h.press("pane devices");
        h.wait(0.3);
        assert!(h.app.chrome.devices_open());
        assert!(h.has("devices blank"), "the blank space is there to right-click");
        // The blink example has no map: nothing listed, and read-only.
        assert!(!h.has("device box"));
        h.right_click(h.rect("devices blank").center());
        assert!(h.has("menu new device"));
        h.press("menu new device");
        h.wait(0.2);
        assert!(!h.has("device form"), "an example is read-only: no form");
        h.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        // A project of one's own, with a map: the tree.
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.proj.add_file(
                "io.map",
                "device e1213 modbus-tcp 10.0.0.57:502 unit=1\nin di0 bool e1213 di:0\nout do0 bool e1213 coil:0\nout lamp bool box do:0\n"
                    .into(),
            );
        }
        h.wait(0.3);
        assert!(h.has("device e1213"), "the declared device");
        assert!(h.has("point di0") && h.has("point do0"), "its points under it");
        assert!(h.has("device box"), "the box's own channels, implied by a signal");
        assert!(h.has("point lamp"));
        // Folding a device hides its points.
        h.press("device e1213");
        h.wait(0.2);
        assert!(!h.has("point di0") && h.has("point lamp"));
        h.press("device e1213");
        h.wait(0.2);
        assert!(h.has("point di0"));
        // Add one: the form, filled in, writes a line onto io.map.
        h.right_click(h.rect("devices blank").center());
        h.press("menu new device");
        h.wait(0.2);
        assert!(h.has("device form"), "the form in the centre");
        h.press("device ok");
        h.wait(0.2);
        assert!(h.has("device form"), "an empty name is refused, the form stays");
        h.press("device name");
        h.frame(vec![Event::Text("rio".into())]);
        h.press("device enip-io");
        // The centred form changes height with the protocol's settings.
        h.wait(0.2);
        h.press("device endpoint");
        h.frame(vec![Event::Text("10.0.0.60".into())]);
        h.press("device ok");
        h.wait(0.3);
        assert!(!h.has("device form"), "added: the form is gone");
        let map = h.app.sim.as_ref().unwrap().proj.files["io.map"].clone();
        assert!(
            map.ends_with("device rio enip-io 10.0.0.60 profile=moxa-e1213 rpi_ms=100 poll_ms=20\n"),
            "{map}"
        );
        assert!(h.has("device rio"), "and listed");
        assert!(h.has("device e1213"), "beside the Modbus one: protocols mix");
        // The agent gets the same listing.
        let text = h.app.sim.as_ref().unwrap().devices_text();
        assert!(
            text.contains("e1213  modbus-tcp 10.0.0.57:502 unit=1  [simulated]\n  in  di0 bool di:0"),
            "{text}"
        );
        assert!(text.contains("rio  enip-io 10.0.0.60"), "{text}");
        // Remembered: the tab reads back.
        assert_eq!(Chrome::restored(&h.app.chrome.saved()).pane, Pane::Devices);
    }

    /// `device-import`: Import device… takes a vendor's EDS; the file joins
    /// the project as `NAME.eds` and the New device form opens filled in
    /// (EtherNet/IP, named after the EDS's product, `eds=NAME.eds`); the
    /// address typed, Add writes the line. Examples are read-only; junk
    /// is refused with a note.
    #[test]
    fn importing_an_eds_keeps_the_file_and_fills_the_device_form() {
        use crate::projects::Workspace;
        const FACTS: &str = include_str!("../../helmsman-proto/tests/fixtures/e1213-facts.eds");
        let mut h = Harness::new(App::sim());
        // An example is read-only: refused with a note, nothing kept.
        h.app
            .sim
            .as_mut()
            .unwrap()
            .upload("E1213.eds", FACTS.as_bytes().to_vec());
        {
            let sim = h.app.sim.as_ref().unwrap();
            assert!(!sim.proj.files.contains_key("e1213.eds"));
            assert!(
                sim.proj
                    .notice
                    .as_ref()
                    .is_some_and(|(t, err)| *err && t.contains("read-only")),
                "{:?}",
                sim.proj.notice
            );
            assert!(sim.devices_ui.adding.is_none());
        }
        h.app.sim.as_mut().unwrap().proj.create("mine", None);
        h.wait(0.2);
        h.app
            .sim
            .as_mut()
            .unwrap()
            .upload("Moxa E1213 (v2.0).EDS", FACTS.as_bytes().to_vec());
        h.press("pane devices");
        h.wait(0.3);
        {
            let sim = h.app.sim.as_ref().unwrap();
            assert_eq!(
                sim.proj.files.get("moxa-e1213-v2-0.eds").map(String::as_str),
                Some(FACTS)
            );
            let d = sim.devices_ui.adding.as_ref().expect("the form opened");
            assert_eq!(d.name, "e1213", "named after the EDS's product");
            assert_eq!(crate::devices::PROTOCOLS[d.protocol].0, "enip-io");
            assert_eq!(d.params, "eds=moxa-e1213-v2-0.eds rpi_ms=100 poll_ms=20");
        }
        assert!(h.has("device form"));
        h.press("device endpoint");
        h.frame(vec![Event::Text("10.0.0.57".into())]);
        h.press("device ok");
        h.wait(0.3);
        assert!(!h.has("device form"));
        let map = h.app.sim.as_ref().unwrap().proj.files["io.map"].clone();
        assert!(
            map.ends_with("device e1213 enip-io 10.0.0.57 eds=moxa-e1213-v2-0.eds rpi_ms=100 poll_ms=20\n"),
            "{map}"
        );
        assert!(h.has("device e1213"), "listed in the tree");
        // Again: the file is replaced, not duplicated.
        h.app
            .sim
            .as_mut()
            .unwrap()
            .upload("Moxa E1213 (v2.0).EDS", FACTS.as_bytes().to_vec());
        assert_eq!(
            h.app
                .sim
                .as_ref()
                .unwrap()
                .proj
                .files
                .keys()
                .filter(|k| crate::devices::is_eds(k))
                .count(),
            1
        );
        h.app.sim.as_mut().unwrap().devices_ui.adding = None;
        // Junk: not text, refused with a note; a nameless file, refused.
        h.app
            .sim
            .as_mut()
            .unwrap()
            .upload("bin.eds", vec![0xff, 0xfe, 0, 1]);
        {
            let sim = h.app.sim.as_ref().unwrap();
            assert!(
                sim.proj
                    .notice
                    .as_ref()
                    .is_some_and(|(t, err)| *err && t.contains("refused")),
                "{:?}",
                sim.proj.notice
            );
            assert!(sim.devices_ui.adding.is_none());
        }
        h.app
            .sim
            .as_mut()
            .unwrap()
            .upload("---.eds", FACTS.as_bytes().to_vec());
        assert!(h.app.sim.as_ref().unwrap().devices_ui.adding.is_none());
        // The right-click menu offers the import.
        h.right_click(h.rect("devices blank").center());
        assert!(h.has("menu import device"));
        h.press("menu import device");
        h.wait(0.2);
        assert!(
            !h.has("device form"),
            "the picker is the browser's; no form until a file comes back"
        );
    }

    /// `devices-repo`: Scan in the Devices tab lists what answers on the
    /// network (the simulation: its E1213), says whether the map binds it,
    /// and "Add to the map…" opens the form filled in by identity; once
    /// the map names it the row says so and the item is disabled.
    #[test]
    fn scan_lists_the_network_and_adds_a_found_device_by_identity() {
        use crate::devices::Scan;
        let mut h = Harness::new(App::sim());
        h.app.sim.as_mut().unwrap().proj.create("mine", None);
        h.press("pane devices");
        h.wait(0.3);
        assert!(h.has("devices scan"), "the Scan button");
        assert!(!h.has("network 192.168.127.254"));
        h.press("devices scan");
        h.wait(0.3);
        assert_eq!(h.app.sim.as_ref().unwrap().devices_ui.scan, Scan::Done);
        assert!(h.has("network 192.168.127.254"), "the simulated E1213 answered");
        let f = &h.app.sim.as_ref().unwrap().devices_ui.found[0];
        assert_eq!((f.vendor_id, f.product_code), (991, 515));
        assert_eq!(f.mapped, None, "a fresh project maps nothing");
        // The agent's context lists it too.
        let text = h.app.sim.as_ref().unwrap().devices_text();
        assert!(
            text.contains("On the network (ListIdentity):\n  E1213 192.168.127.254 vendor 991 product 515"),
            "{text}"
        );
        assert!(text.contains("not in the map"), "{text}");
        // Add to the map: the form, filled in by identity.
        h.right_click(h.rect("network 192.168.127.254").center());
        assert!(h.has("menu add network"));
        h.press("menu add network");
        h.wait(0.2);
        assert!(h.has("device form"));
        let d = h
            .app
            .sim
            .as_ref()
            .unwrap()
            .devices_ui
            .adding
            .clone()
            .expect("the form");
        assert_eq!(d.name, "e1213");
        assert_eq!(d.endpoint, "cip=991:515:0x00e12130");
        assert_eq!(d.protocol, 1, "EtherNet/IP");
        // The E1213's channels are listed; the program's lamp (on the box's
        // own channel) is bound to the first output already.
        assert_eq!(d.points.len(), 20, "12 inputs, 8 outputs");
        let on: Vec<(&str, &str)> = d
            .points
            .iter()
            .filter(|r| r.on)
            .map(|r| (r.name.as_str(), r.point.as_str()))
            .collect();
        assert_eq!(on, [("run", "di:0"), ("lamp", "do:0")]);
        h.press("device ok");
        h.wait(0.3);
        let map = h.app.sim.as_ref().unwrap().proj.files["io.map"].clone();
        assert!(map.contains("out lamp bool e1213 do:0 safe=false"), "{map}");
        assert!(!map.contains(" box do:0"), "moved, not doubled: {map}");
        assert!(
            map.contains(
                "device e1213 enip-io cip=991:515:0x00e12130 profile=moxa-e1213 rpi_ms=100 poll_ms=20\n"
            ),
            "{map}"
        );
        // Wired at once, with no second scan (it offered Use this device
        // again until a rescan, 2026-11-05).
        assert_eq!(
            h.app.sim.as_ref().unwrap().devices_ui.found[0].mapped.as_deref(),
            Some("e1213")
        );
        assert!(!h.has("use network 192.168.127.254"));
        h.press("devices scan");
        h.wait(0.3);
        assert_eq!(
            h.app.sim.as_ref().unwrap().devices_ui.found[0].mapped.as_deref(),
            Some("e1213")
        );
        h.right_click(h.rect("network 192.168.127.254").center());
        h.press("menu add network");
        h.wait(0.2);
        assert!(!h.has("device form"), "already in the map: nothing to add");
        assert!(
            !h.has("use network 192.168.127.254"),
            "a wired device has no Use button"
        );
    }

    /// Nick, 2026-11-05: "It says it's not in the map but I don't even
    /// know where the map is". On a read-only example, "Use this device"
    /// copies the project, gives it the io.map its program implies, opens
    /// the form with the E1213's channels, the program's I/O bound to
    /// them; a channel ticked and named becomes a new variable; the I/O
    /// wiring link opens io.map; a wired signal renames in io.map and the
    /// ST together.
    #[test]
    fn use_this_device_copies_an_example_and_wires_its_io() {
        let mut h = Harness::new(App::sim());
        {
            let sim = h.app.sim.as_mut().unwrap();
            // The start/stop latch: two BOOL inputs, a BOOL output.
            let ex = crate::playground::EXAMPLES
                .iter()
                .map(|e| e.src)
                .find(|src| src.contains("PROGRAM latch"))
                .map(|src| crate::sim_projects::example_project(src).0)
                .expect("the latch example");
            sim.open_project(&ex);
            assert!(sim.proj.is_example());
        }
        h.press("pane devices");
        h.press("devices scan");
        h.wait(0.3);
        assert!(
            h.has("use network 192.168.127.254"),
            "a visible button, not only a right-click"
        );
        let found = &h.app.sim.as_ref().unwrap().devices_ui.found[0];
        assert!(found.simulated, "the playground's E1213 says it is simulated");
        h.press("use network 192.168.127.254");
        h.wait(0.3);
        {
            let sim = h.app.sim.as_ref().unwrap();
            assert!(
                !sim.proj.is_example(),
                "copied into a project of the visitor's own"
            );
            assert!(sim.proj.open.starts_with("my-"), "{}", sim.proj.open);
            let map = &sim.proj.files["io.map"];
            assert!(map.contains(" bool box di:0"), "{map}");
        }
        assert!(h.has("device form"));
        // Start and stop on DI-00/01, the motor on DO-00, from the program.
        let rows = h
            .app
            .sim
            .as_ref()
            .unwrap()
            .devices_ui
            .adding
            .clone()
            .unwrap()
            .points;
        let on: Vec<(&str, &str)> = rows
            .iter()
            .filter(|r| r.on)
            .map(|r| (r.name.as_str(), r.point.as_str()))
            .collect();
        assert_eq!(on, [("start", "di:0"), ("stop", "di:1"), ("motor", "do:0")]);
        // Tick DI-05 and name it: a new variable.
        h.press("point 5 on");
        h.press("point 5 name");
        h.frame(vec![Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }]);
        h.frame(vec![Event::Text("door".into())]);
        h.wait(0.2);
        let r5 = h
            .app
            .sim
            .as_ref()
            .unwrap()
            .devices_ui
            .adding
            .clone()
            .unwrap()
            .points[5]
            .clone();
        assert!(r5.on && r5.name == "door", "{r5:?}");
        h.press("device ok");
        h.wait(0.3);
        assert!(
            !h.has("device form"),
            "{:?}",
            h.app.sim.as_ref().unwrap().devices_ui.adding
        );
        {
            let sim = h.app.sim.as_ref().unwrap();
            let map = &sim.proj.files["io.map"];
            assert!(!map.contains(" box di:") && !map.contains(" box do:"), "{map}");
            assert!(map.contains("in  start bool e1213 di:0\n"), "{map}");
            assert!(map.contains("in  door bool e1213 di:5\n"), "{map}");
            let st = sim.proj.files.values().find(|t| t.contains("PROGRAM")).unwrap();
            assert!(st.contains("  door : BOOL;"), "{st}");
            assert!(helmsman_iec::ast::parse(st).is_ok(), "{st}");
        }
        // The I/O wiring link opens the file in the editor.
        h.press("devices open map");
        h.wait(0.2);
        assert_eq!(h.app.sim.as_ref().unwrap().proj.current, "io.map");
        // Rename start: the wiring line and the program together.
        h.press("pane devices");
        h.wait(0.2);
        h.right_click(h.rect("point start").center());
        h.press("menu rename point");
        h.wait(0.2);
        assert!(h.has("rename form"));
        h.press("rename name");
        h.frame(vec![Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }]);
        h.frame(vec![Event::Text("go_button".into())]);
        h.wait(0.2);
        h.press("rename ok");
        h.wait(0.3);
        assert!(!h.has("rename form"));
        let sim = h.app.sim.as_ref().unwrap();
        assert!(sim.proj.files["io.map"].contains("in  go_button bool e1213 di:0\n"));
        let st = sim.proj.files.values().find(|t| t.contains("PROGRAM")).unwrap();
        assert!(
            st.contains("go_button : BOOL;") && st.contains("(go_button OR motor)"),
            "{st}"
        );
        assert!(!st.to_ascii_lowercase().contains("start :"), "{st}");
    }

    /// Nick, 2026-11-05: the fold arrow on a device showed the typing
    /// cursor. Every control shows the finger, a text field the I-beam, and
    /// a label is not text to select.
    #[test]
    fn controls_show_the_finger_and_text_fields_the_i_beam() {
        let mut h = Harness::new(App::sim());
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.proj.add_file(
                "io.map",
                "device e1213 enip-io 10.0.0.57 profile=moxa-e1213\nin  di0 bool e1213 di:0\n".into(),
            );
        }
        h.press("pane devices");
        h.wait(0.2);
        let hover = |h: &mut Harness, key: &str| {
            let at = h.rect(key).center();
            h.frame(vec![Event::PointerMoved(at)]);
            h.frame(vec![]);
            h.cursor
        };
        assert_eq!(
            hover(&mut h, "device e1213"),
            egui::CursorIcon::PointingHand,
            "the fold row"
        );
        assert_eq!(
            hover(&mut h, "point di0"),
            egui::CursorIcon::PointingHand,
            "a point row"
        );
        assert_eq!(
            hover(&mut h, "pane files"),
            egui::CursorIcon::PointingHand,
            "a tab"
        );
        assert_eq!(
            hover(&mut h, "devices scan"),
            egui::CursorIcon::PointingHand,
            "a button"
        );
        h.right_click(h.rect("devices blank").center());
        h.press("menu new device");
        h.wait(0.2);
        assert_eq!(
            hover(&mut h, "device name"),
            egui::CursorIcon::Text,
            "a text field"
        );
        assert_eq!(
            hover(&mut h, "device enip-io"),
            egui::CursorIcon::PointingHand,
            "a choice"
        );
        assert!(
            !h.ctx.style_of(h.ctx.theme()).interaction.selectable_labels,
            "labels are not selectable"
        );
    }

    /// Nick, 2026-11-05: "I need the ability to delete devices. The one on
    /// there is the old version and I can't seem to edit it." Edit opens a
    /// device's form from its line (an old E1213 gets its profile) and
    /// Save replaces it in place; Delete asks, then takes the device and
    /// its signals out, the program's other lines kept.
    #[test]
    fn a_device_is_edited_in_its_form_and_deleted_after_a_question() {
        let mut h = Harness::new(App::sim());
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.proj.add_file(
                "io.map",
                "out lamp bool box do:0 safe=false\ndevice e1213 enip-io cip=991:515:0x00e12130 rpi_ms=100 poll_ms=20\nin  run bool e1213 di:0\n"
                    .into(),
            );
        }
        h.press("pane devices");
        h.wait(0.2);
        assert!(
            h.has("edit device e1213") && h.has("delete device e1213"),
            "visible, not only a right-click"
        );
        assert!(
            !h.has("edit device box"),
            "the box's own channels are not a device to edit"
        );
        h.press("edit device e1213");
        h.wait(0.3);
        assert!(h.has("device form"));
        let d = h.app.sim.as_ref().unwrap().devices_ui.adding.clone().unwrap();
        assert_eq!(d.editing.as_deref(), Some("e1213"));
        assert!(d.params.contains("profile=moxa-e1213"), "{}", d.params);
        assert_eq!(d.points.iter().filter(|r| r.on).count(), 1, "run on DI-00");
        // Tick DO-00 for the lamp (the program's own), then Save.
        h.app
            .sim
            .as_mut()
            .unwrap()
            .devices_ui
            .adding
            .as_mut()
            .unwrap()
            .points[12]
            .use_ = crate::device_form::Use::Existing("lamp".into());
        {
            let r = &mut h
                .app
                .sim
                .as_mut()
                .unwrap()
                .devices_ui
                .adding
                .as_mut()
                .unwrap()
                .points[12];
            r.on = true;
            r.name = "lamp".into();
        }
        h.wait(0.2);
        h.press("device ok");
        h.wait(0.3);
        assert!(
            !h.has("device form"),
            "{:?}",
            h.app
                .sim
                .as_ref()
                .unwrap()
                .devices_ui
                .adding
                .as_ref()
                .map(|d| &d.error)
        );
        let map = h.app.sim.as_ref().unwrap().proj.files["io.map"].clone();
        assert_eq!(
            map.matches("device e1213").count(),
            1,
            "replaced, not added again: {map}"
        );
        assert!(
            map.contains(
                "device e1213 enip-io cip=991:515:0x00e12130 rpi_ms=100 poll_ms=20 profile=moxa-e1213\n"
            ),
            "{map}"
        );
        assert!(
            map.contains("in  run bool e1213 di:0\n")
                && map.contains("out lamp bool e1213 do:0 safe=false\n"),
            "{map}"
        );
        assert!(!map.contains("box do:0"), "{map}");
        // Delete: asks, then the device and its lines are gone.
        h.press("delete device e1213");
        h.wait(0.2);
        assert!(h.has("delete form"));
        h.press("delete cancel");
        h.wait(0.2);
        assert!(
            h.app.sim.as_ref().unwrap().proj.files["io.map"].contains("device e1213"),
            "Cancel keeps it"
        );
        h.right_click(h.rect("device e1213").center());
        h.press("menu delete device");
        h.wait(0.2);
        h.press("delete ok");
        h.wait(0.3);
        let map = h.app.sim.as_ref().unwrap().proj.files["io.map"].clone();
        assert!(!map.contains("e1213"), "{map}");
        assert!(!h.has("device e1213"));
    }

    /// The library's match fills the form's name and profile in.
    #[test]
    fn a_library_match_names_the_device_and_its_profile() {
        use crate::devices::{Found, NewDevice};
        use helmsman_project::devlib::Index;
        let index = Index::parse(r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213", "kind": "remote-io", "identity": { "enip": { "vendor_id": 991, "product_code": 515 } }, "protocols": { "enip": { "driver": "enip-io", "map": "enip.map", "profile": "moxa-e1213" } } } ] }"#).unwrap();
        let mut h = Harness::new(App::sim());
        h.app.sim.as_mut().unwrap().proj.create("mine", None);
        h.press("pane devices");
        h.press("devices scan");
        h.wait(0.2);
        h.app.sim.as_mut().unwrap().devices_ui.attach_library(&index);
        let f: Found = h.app.sim.as_ref().unwrap().devices_ui.found[0].clone();
        assert_eq!(
            f.library.as_ref().map(|l| l.model.as_str()),
            Some("ioLogik E1213")
        );
        let d = NewDevice::from_found(&f);
        assert_eq!(d.name, "iologik_e1213");
        assert_eq!(d.params, "profile=moxa-e1213 rpi_ms=100 poll_ms=20");
        assert!(
            h.app
                .sim
                .as_ref()
                .unwrap()
                .devices_text()
                .contains("library: Moxa ioLogik E1213")
        );
    }

    /// Nick, 2026-11-02 (sim-feedback-devices): a plant model's `{state}`
    /// variables are listed in the Devices tab with the simulation's value,
    /// the map signal that measures each on the real machine, or the sensor
    /// that would; "Add a sensor…" writes that signal's `in` line onto
    /// io.map, and the agent's context carries the same list.
    #[test]
    fn plant_states_list_the_models_quantities_and_add_a_sensor_line() {
        let mut h = Harness::new(App::sim());
        h.press("pane devices");
        h.wait(0.3);
        assert!(!h.has("plant states"), "the blink example keeps no plant model");
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.examples.insert(
                "pump-station".into(),
                helmsman_project::Files(
                    crate::test_programs::PUMP_STATION
                        .iter()
                        .map(|(n, t)| ((*n).to_string(), (*t).to_string()))
                        .collect(),
                ),
            );
            sim.proj.create("well", Some("pump-station"));
            let src = sim.proj.build_src();
            assert!(sim.session.load(&src), "the pump station compiles");
            // Loading stops the simulation, as a deploy would; run it so the
            // plant model scans and the trace has its values.
            sim.session.set_running(true);
        }
        h.wait(0.5);
        assert!(
            h.has("plant states"),
            "the starter's plant model marks its well level"
        );
        assert!(h.has("state level_m"));
        let states = {
            let sim = h.app.sim.as_ref().unwrap();
            let map = helmsman_project::iomap::skim(sim.proj.files.get("io.map").map_or("", String::as_str));
            sim.plant_states(&map)
        };
        assert_eq!(states.len(), 1, "{states:?}");
        // The well fills from 1.5 m while it runs: a live value, not the initial one.
        match states[0].value {
            Some(helmsman_iec::Value::Real(x)) if (1.4..2.0).contains(&x) => {}
            other => panic!("the model's value from the trace: {other:?}"),
        }
        assert_eq!(states[0].unit.as_deref(), Some("m"));
        assert_eq!(states[0].measured_by, None);
        assert_eq!(states[0].suggestion, "level transmitter");
        let text = h.app.sim.as_ref().unwrap().devices_text();
        assert!(text.contains("plant states"), "{text}");
        assert!(text.contains("level_m REAL unit=m = 1.5"), "{text}");
        assert!(text.contains("  in plant_model (plant_model.st:"), "{text}");
        assert!(text.contains("not measured; level transmitter would"), "{text}");
        // Add a sensor: the form, filled in for the state, writes one in line.
        h.right_click(h.rect("state level_m").center());
        assert!(h.has("menu add sensor"));
        h.press("menu add sensor");
        h.wait(0.2);
        assert!(h.has("sensor form"), "the form in the centre");
        assert!(
            h.has("sensor device box"),
            "no device in the map: the box's channels"
        );
        h.press("sensor ok");
        h.wait(0.3);
        assert!(!h.has("sensor form"), "added: the form is gone");
        let map = h.app.sim.as_ref().unwrap().proj.files["io.map"].clone();
        assert!(
            map.ends_with("in  level_m real box i2c:1:0x48:0:i16 unit=m\n"),
            "{map}"
        );
        h.wait(0.3);
        let states = {
            let sim = h.app.sim.as_ref().unwrap();
            let map = helmsman_project::iomap::skim(&sim.proj.files["io.map"]);
            sim.plant_states(&map)
        };
        let m = states[0].measured_by.as_ref().expect("now measured");
        assert_eq!((m.signal.as_str(), m.device.as_str()), ("level_m", "box"));
        let text = h.app.sim.as_ref().unwrap().devices_text();
        assert!(
            text.contains("measured by level_m (box i2c:1:0x48:0:i16)"),
            "{text}"
        );
        // Measured: the menu item is disabled, so pressing it opens nothing.
        h.right_click(h.rect("state level_m").center());
        h.press("menu add sensor");
        h.wait(0.2);
        assert!(!h.has("sensor form"), "a measured state offers no second sensor");
        h.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
    }

    /// The Files tab shows the program's files, marks main.st as the one
    /// running, and folds the earlier iterations under one row.
    #[test]
    fn the_files_tab_folds_the_earlier_iterations() {
        let mut h = Harness::new(App::sim());
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.proj
                .files
                .insert("v1.st".into(), "PROGRAM old END_PROGRAM".into());
            sim.proj
                .files
                .insert("v2.st".into(), "PROGRAM older END_PROGRAM".into());
            sim.proj.save();
            sim.open_project("mine");
        }
        h.press("pane files");
        h.wait(0.3);
        assert!(h.has("file main.st"));
        assert!(h.has("earlier"), "the fold row");
        assert!(!h.has("file v1.st") && !h.has("file v2.st"), "folded by default");
        h.press("earlier");
        h.wait(0.2);
        assert!(h.has("file v2.st") && h.has("file v1.st"));
        let v2 = h.rect("file v2.st");
        let v1 = h.rect("file v1.st");
        assert!(v2.top() < v1.top(), "newest first");
        h.press("earlier");
        h.wait(0.2);
        assert!(!h.has("file v1.st"));
    }

    /// The Files tab's History row folds the revisions; a revision opens
    /// in the centre with its diff and Restore, and Close shuts it.
    #[test]
    fn the_files_tab_lists_revisions_and_opens_one_in_the_centre() {
        let mut h = Harness::new(App::sim());
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.open_project("mine");
            sim.proj.current_text_mut().unwrap().push_str("\n(* one *)");
            sim.proj.save();
            sim.proj.current_text_mut().unwrap().push_str("\n(* two *)");
            sim.proj.save();
        }
        h.press("pane files");
        h.wait(0.3);
        assert!(h.has("history"), "the fold row");
        assert!(!h.has("revision 1"), "folded by default");
        h.press("history");
        h.wait(0.2);
        assert!(h.has("revision 1") && h.has("revision 2"));
        let r2 = h.rect("revision 2");
        let r1 = h.rect("revision 1");
        assert!(r2.top() < r1.top(), "newest first");
        h.press("revision 1");
        h.wait(0.3);
        assert!(h.has("revision form") && h.has("revision restore") && h.has("revision close"));
        assert!(
            h.app
                .sim
                .as_ref()
                .unwrap()
                .revision
                .as_ref()
                .is_some_and(|v| v.row.n == 1)
        );
        h.press("revision restore");
        h.wait(0.3);
        assert!(!h.has("revision form"), "restored and shut");
        let sim = h.app.sim.as_ref().unwrap();
        assert!(
            sim.proj.files["main.st"].ends_with("(* one *)"),
            "revision 1's text is back"
        );
        assert_eq!(sim.proj.revisions().len(), 3);
        assert_eq!(sim.proj.revisions()[2].what, "restored revision 1");
        h.press("revision 2");
        h.wait(0.3);
        assert!(h.has("revision form"));
        h.press("revision close");
        h.wait(0.2);
        assert!(!h.has("revision form"));
    }

    /// Requirement tests in the simulation (TODO.md spec-tests): the
    /// project's `test_*.st` files run green or red with why; a red one
    /// refuses Deploy and a block answer's "Use this code", naming it; a
    /// test file never builds into the program; fixed, the deploy goes
    /// through. (The Requirements tab that listed them is archived,
    /// 2026-11-08; the gate stays.)
    #[test]
    fn requirement_tests_gate_deploy_and_use_this_code_in_the_simulation() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        assert!(
            h.app.sim.as_ref().unwrap().tests.rows.is_empty(),
            "the blink example has no tests"
        );
        let main = "PROGRAM lamp\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := start;\nEND_PROGRAM\n";
        let red = "(* The lamp stays off while start is held. *)\nPROGRAM test_lamp\nVAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT start : BOOL; done : BOOL; END_VAR\nVAR n : INT; lamp_stays_off : ASSERT; END_VAR\nstart := TRUE;\nn := n + 1;\nIF n > 3 THEN lamp_stays_off(COND := NOT lamp); done := TRUE; END_IF;\nEND_PROGRAM\n";
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.proj.files.insert("main.st".into(), main.into());
            sim.proj.add_file("test_lamp.st", red.into());
            sim.proj.save();
            sim.open_project("mine");
        }
        h.wait(0.3);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(sim.tests.ran && sim.tests.rows.len() == 1, "{:?}", sim.tests);
        assert!(!sim.tests.rows[0].passed());
        assert_eq!(
            sim.tests.rows[0].requirement,
            "The lamp stays off while start is held."
        );
        assert!(
            !sim.session.src.contains("PROGRAM test_lamp"),
            "the test is not in the build"
        );
        // Deploy: refused, with the test named.
        h.key(Modifiers::COMMAND, Key::D);
        h.wait(0.2);
        let sim = h.app.sim.as_ref().unwrap();
        let notice = sim.notice.clone().unwrap_or_default();
        assert!(
            notice.starts_with("Refused: requirement test test_lamp:"),
            "{notice}"
        );
        // A block answer into this project: refused too; nothing added.
        let before = sim.proj.files.len();
        h.app.sim.as_mut().unwrap().take_code(
            "FUNCTION_BLOCK helper\nVAR_INPUT x : BOOL; END_VAR\nVAR_OUTPUT y : BOOL; END_VAR\ny := x;\nEND_FUNCTION_BLOCK\n"
                .into(),
        );
        h.wait(0.2);
        let sim = h.app.sim.as_ref().unwrap();
        assert_eq!(sim.proj.files.len(), before, "{:?}", sim.proj.files.keys());
        assert!(sim.notice.clone().unwrap_or_default().starts_with("Refused:"));
        // Fixed: green, Deploy goes through.
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.files.insert(
                "test_lamp.st".into(),
                red.replace("stays off", "comes on")
                    .replace("COND := NOT lamp", "COND := lamp"),
            );
            sim.run_tests();
        }
        h.wait(0.2);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(sim.tests.rows[0].passed(), "{:?}", sim.tests);
        h.key(Modifiers::COMMAND, Key::D);
        h.wait(0.2);
        let sim = h.app.sim.as_ref().unwrap();
        let notice = sim.notice.clone().unwrap_or_default();
        assert!(notice.starts_with("mine is running in this page"), "{notice}");
    }

    #[test]
    fn double_clicking_a_file_opens_the_code_tab() {
        let mut h = Harness::new(App::sim());
        h.app.chrome.show_files();
        h.wait(0.2);
        let at = h.rect("file blink.st").center();
        h.click(at);
        assert!(h.app.chrome.files_open(), "one click stays on Files");
        h.click(at);
        h.frame(vec![]);
        assert!(h.app.chrome.code_open(), "two open the Code tab");
    }

    /// Nick 2026-11-07: a double click on any pane tab (Files, Devices,
    /// Code; I/O, Agent and the rest on the right) hides that pane; one
    /// click only picks the tab.
    #[test]
    fn double_clicking_a_tab_hides_its_pane_and_one_click_does_not() {
        let mut h = Harness::new(App::sim());
        h.wait(0.2);
        for key in ["pane files", "pane devices", "pane code"] {
            h.app.chrome.left = true;
            h.wait(0.2);
            let at = h.rect(key).center();
            h.click(at);
            h.wait(0.5);
            assert!(h.app.chrome.left, "one click on {key} keeps the pane");
            h.click(at);
            h.click(at);
            h.frame(vec![]);
            assert!(!h.app.chrome.left, "a double click on {key} hides the pane");
            h.wait(0.5);
            h.press("left");
            h.wait(0.2);
            assert!(h.app.chrome.left, "the strip brings it back after {key}");
        }
        h.app.chrome.right = true;
        h.wait(0.2);
        let tabs: Vec<String> = h
            .app
            .chrome
            .hits
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| k.starts_with("tab "))
            .collect();
        assert!(
            tabs.iter().any(|k| k == "tab io") && tabs.iter().any(|k| k == "tab claude"),
            "{tabs:?}"
        );
        for key in tabs {
            h.app.chrome.right = true;
            h.wait(0.2);
            let at = h.rect(&key).center();
            h.click(at);
            h.wait(0.5);
            assert!(h.app.chrome.right, "one click on {key} keeps the panel");
            h.click(at);
            h.click(at);
            h.frame(vec![]);
            assert!(!h.app.chrome.right, "a double click on {key} hides the panel");
        }
    }

    /// The question is asked wherever the page is, the Files tab shut or
    /// not: the Agent's answer with unsaved edits, say.
    #[test]
    fn the_question_shows_with_the_files_tab_shut() {
        let mut h = Harness::new(App::sim());
        h.app.chrome.left = false;
        h.wait(0.2);
        let v = h.app.sim.as_mut().unwrap();
        v.proj.current_text_mut().unwrap().push_str("(* edit *)");
        v.take_code("PROGRAM other\nEND_PROGRAM\n".into());
        // The first PROGRAM answer asks for the program's name first (a
        // few frames: a new modal settles into the centre on its second).
        h.wait(0.2);
        assert!(h.has("program name") && h.has("program ok"));
        h.press("program ok");
        {
            let v = h.app.sim.as_ref().unwrap();
            assert!(
                v.program_name.is_none(),
                "the name was taken: {:?}",
                v.program_name
            );
            assert!(
                v.files_ui.confirm.is_some(),
                "the unsaved-edits question is pending: open={} dirty={} program={:?} files={:?}",
                v.proj.open,
                v.proj.dirty(),
                v.new_program,
                v.proj.files.keys().collect::<Vec<_>>()
            );
        }
        h.frame(vec![]);
        assert!(h.has("confirm yes") && h.has("confirm drop") && h.has("confirm no"));
        h.press("confirm no");
        h.frame(vec![]);
        assert!(!h.has("confirm"));
    }

    /// Only the layout string is kept per browser; egui's memory (panel
    /// widths) is not, so a width a pane once grew to does not come back.
    #[test]
    fn egui_memory_is_not_kept_across_loads() {
        assert!(!eframe::App::persist_egui_memory(&App::sim()));
    }

    #[test]
    fn a_right_click_on_a_file_copies_pastes_and_duplicates_it() {
        let mut h = Harness::new(App::sim());
        // An example is read-only: its file menu offers Copy alone. File ›
        // Copy project first to get a writable one; it lands on the Files
        // tab asking for the name.
        h.press("bar file");
        h.press("file copy");
        assert!(h.app.chrome.files_open(), "it lands on the Files tab");
        // The name popup settles into the centre on its second frame.
        h.wait(0.2);
        h.press("name ok");
        h.wait(0.3);
        assert!(h.has("file blink.st"), "blink-copy open with its file");
        assert!(
            !h.has("new project") && !h.has("copy project"),
            "the buttons are gone"
        );
        // A right-click on a project of your own offers its copy, rename
        // and delete (an example has none: a click makes your copy).
        h.right_click(h.rect("project blink-copy").center());
        for k in ["menu copy project", "menu rename project", "menu delete project"] {
            assert!(h.has(k), "{k}");
        }
        h.frame(vec![eframe::egui::Event::Key {
            key: eframe::egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: eframe::egui::Modifiers::NONE,
        }]);
        h.frame(vec![]);
        let text = h.app.sim.as_ref().unwrap().proj.files["blink.st"].clone();
        let at = h.rect("file blink.st").center();
        h.right_click(at);
        for k in [
            "menu deploy",
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
        let at = h.rect("menu new project").center();
        h.press("menu new project");
        h.wait(0.2);
        // A popup by the click (Nick, 2026-11-05), held on the screen.
        let form = h.rect("name form");
        assert!(
            (form.top() - at.y).abs() < 60.0 || form.bottom() >= h.size.y - 1.0,
            "the name is asked by the click at {at:?}: {form:?}"
        );
        assert!(
            form.left() >= 0.0 && form.right() <= h.size.x,
            "on the screen: {form:?}"
        );
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
        // The pane opens at its default width; the editor is what is
        // left of it beside the gutter.
        assert!(
            (50.0..crate::editor::LEFT_W).contains(&r.rect.width()),
            "{:?}",
            r.rect
        );

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
        let src = crate::test_programs::PROGRAMS
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

    #[test]
    fn ctrl_close_bracket_steps_into_the_picked_instance_and_ctrl_open_bracket_back_out() {
        let mut h = stations();
        h.key(Modifiers::SHIFT, Key::F8);
        let scans = |h: &Harness| h.app.sim.as_ref().unwrap().session.scans;
        // The first instance block on the program's diagram.
        let v = h.app.sim.as_mut().unwrap().viewer.as_mut().unwrap();
        let (n, instance) = v
            .diagram()
            .nodes
            .iter()
            .find_map(|n| match &n.kind {
                helmsman_diagram::NodeKind::Fb { instance, fb } if fb.eq_ignore_ascii_case("station") => {
                    Some((n.id, instance.clone()))
                }
                _ => None,
            })
            .expect("a station instance");
        v.pick = Some(Pick::Node(n));
        let before = scans(&h);
        h.key(Modifiers::COMMAND, Key::CloseBracket);
        let v = sim_view(&h);
        assert_eq!(v.diagram().pou, "STATION");
        assert!(
            v.scopes[v.scope]
                .prefix
                .eq_ignore_ascii_case(&format!("{instance}.")),
            "{:?}",
            v.scopes[v.scope]
        );
        assert_eq!(scans(&h), before + 1, "stopped, it steps one scan");
        // Nothing picked inside: Ctrl ] does nothing.
        h.key(Modifiers::COMMAND, Key::CloseBracket);
        assert_eq!(scans(&h), before + 1);
        h.key(Modifiers::COMMAND, Key::OpenBracket);
        let v = sim_view(&h);
        assert_eq!(v.scope, 0, "back on the program");
        assert_eq!(v.pick, Some(Pick::Node(n)), "the instance left is picked");
        assert_eq!(scans(&h), before + 2);
        // At the top, Ctrl [ does nothing; running, the keys only move.
        h.key(Modifiers::COMMAND, Key::OpenBracket);
        assert_eq!(scans(&h), before + 2);
        h.app.sim.as_mut().unwrap().session.set_running(true);
        h.key(Modifiers::COMMAND, Key::CloseBracket);
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

    /// Nick, 2026-11-07: drag a lane's label up or down to reorder the
    /// trend. A drag that ends on its own lane moves nothing.
    #[test]
    fn dragging_a_trend_label_moves_its_lane() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        // Lanes are drawn by name; a name the program lacks records as no
        // value, which is all this needs.
        let s = &mut h.app.sim.as_mut().unwrap().session;
        s.trend.signals = vec!["FIRST".into(), "SECOND".into(), "THIRD".into()];
        s.trend.samples.clear();
        h.wait(0.3);
        let order = |h: &Harness| h.app.sim.as_ref().unwrap().session.trend.signals.clone();
        let [a, b, c] = <[String; 3]>::try_from(order(&h)).unwrap();
        let handle = |h: &Harness, n: &str| h.rect(&format!("trend lane {}", n.to_ascii_lowercase()));
        // The first lane's label onto the third lane.
        let (from, to) = (handle(&h, &a).center(), handle(&h, &c).center());
        h.drag(from, to);
        assert_eq!(order(&h), [b.clone(), c.clone(), a.clone()]);
        // Back up to the top.
        let (from, to) = (handle(&h, &a).center(), handle(&h, &b).center());
        h.drag(from, to);
        assert_eq!(order(&h), [a.clone(), b.clone(), c.clone()]);
        // A small drag inside its own lane.
        let r = handle(&h, &b);
        h.drag(r.center(), r.center() + Vec2::new(4.0, r.height() * 0.3));
        assert_eq!(order(&h), [a, b, c]);
    }

    /// Nick, 2026-11-01: the trend's window is 1, 10 or 100 s, so the
    /// line can cross the pane fast or slow; the history is kept for the
    /// widest, so widening shows what already ran. Nick, 2026-11-02: one
    /// logarithmic slider from 100 ms to 10 min instead of three buttons,
    /// the plot following as it moves.
    #[test]
    fn the_trend_window_is_a_log_slider_from_100_ms_to_10_min() {
        use crate::playground_view::{WINDOW_MAX_MS, WINDOW_MIN_MS, window_label};
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        assert!(h.has("trend window"), "the slider");
        assert!(!h.has("trend window 10"), "the buttons are gone");
        let window = |h: &Harness| h.app.sim.as_ref().unwrap().session.trend.window_ms;
        assert_eq!(window(&h), 30_000, "opens on 30 s");
        let slider = h.rect("trend window");
        h.click(Pos2::new(slider.left() + 1.0, slider.center().y));
        assert_eq!(window(&h), WINDOW_MIN_MS, "the left end is 100 ms");
        let kept = h.app.sim.as_ref().unwrap().session.trend.samples.len();
        h.click(Pos2::new(slider.right() - 1.0, slider.center().y));
        assert_eq!(window(&h), WINDOW_MAX_MS, "the right end is 10 min");
        assert!(
            h.app.sim.as_ref().unwrap().session.trend.samples.len() >= kept,
            "narrowing dropped nothing"
        );
        // Logarithmic: the middle of the track is near the geometric mean
        // (~7.7 s), not the arithmetic one (5 min).
        h.click(Pos2::new(slider.center().x, slider.center().y));
        let mid = window(&h);
        assert!((3_000..=20_000).contains(&mid), "mid-track window {mid} ms");
        assert_eq!(window_label(100), "100 ms");
        assert_eq!(window_label(2_500), "2.5 s");
        assert_eq!(window_label(10_000), "10 s");
        assert_eq!(window_label(60_000), "1 min");
        assert_eq!(window_label(600_000), "10 min");
    }

    /// Card agent-scoped-edits: the Agent tab says what the next
    /// instruction may change before it is sent. A built-in example: a new
    /// project; a project of one's own: the open file (both in the box's
    /// hint); lines selected in the Code tab: a line of their own, with
    /// Whole file to go back; the panels keep their width with it.
    #[test]
    fn the_agent_tab_names_the_scope_before_the_instruction() {
        use crate::scope::Scope;
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let right = h.rect("right");
        h.press("pane code");
        h.press("tab claude");
        h.wait(0.3);
        assert!(h.has("agent instruction") && !h.has("agent scope"));
        assert_eq!(h.app.sim.as_ref().unwrap().scope(), Scope::NewProject);
        assert!(!h.has("scope whole file"));
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.open_project("mine");
        }
        h.wait(0.3);
        assert_eq!(h.app.sim.as_ref().unwrap().scope(), Scope::File("main.st".into()));
        assert!(!h.has("agent scope") && !h.has("scope whole file"));
        // Lines 2 and 3 selected in the editor.
        let text = h.app.sim.as_ref().unwrap().proj.files["main.st"].clone();
        assert!(text.lines().count() >= 3, "{text}");
        let start = text.lines().next().unwrap().chars().count() + 1;
        let end = start + text.lines().nth(1).unwrap().chars().count() + 2;
        let id = eframe::egui::Id::unique("pg-source");
        h.ctx.memory_mut(|m| m.request_focus(id));
        crate::editor::select(&h.ctx, id, start, end);
        h.wait(0.1);
        // The Agent tab takes the focus, and the editor drops its
        // selection: the scope stays the lines.
        h.ctx.memory_mut(|m| m.surrender_focus(id));
        h.wait(0.2);
        let scope = h.app.sim.as_ref().unwrap().scope();
        assert_eq!(
            scope,
            Scope::Lines {
                file: "main.st".into(),
                from: 2,
                to: 3
            }
        );
        assert_eq!(scope.label(), "Editing: lines 2–3 of main.st");
        assert!(h.has("agent scope") && h.has("scope whole file"));
        let r = right.left() - h.rect("right").left();
        assert!((0.0..=8.0).contains(&r), "the right panel grew {r} pt");
        h.press("scope whole file");
        h.wait(0.2);
        assert_eq!(h.app.sim.as_ref().unwrap().scope(), Scope::File("main.st".into()));
        assert!(!h.has("scope whole file"));
    }

    /// A resizable panel grows to any row that overflows it, every frame
    /// (four such rows were found on 2026-11-01). Every tab of every
    /// panel, visited; the panels end where they began, at most a scroll
    /// bar's width wider.
    /// Nick, 2026-11-05: nothing around the diagram is drawn below
    /// `MIN_TEXT`, on any pane, tab or bottom panel of the simulation.
    #[test]
    fn no_text_around_the_diagram_is_below_the_minimum() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let mut seen = Vec::new();
        for key in [
            "pane files",
            "pane devices",
            "pane code",
            "tab io",
            "tab claude",
            "bottom messages",
            "bottom trend",
        ] {
            h.press(key);
            h.wait(0.3);
            for (size, text) in h.small_text() {
                seen.push(format!("{key}: {size} pt {text:?}"));
            }
        }
        for menu in ["bar file", "bar edit", "bar view", "bar help"] {
            h.press(menu);
            h.frame(vec![]);
            h.frame(vec![]);
            for (size, text) in h.small_text() {
                seen.push(format!("{menu}: {size} pt {text:?}"));
            }
            h.frame(vec![eframe::egui::Event::Key {
                key: eframe::egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: eframe::egui::Modifiers::NONE,
            }]);
            h.wait(0.2);
        }
        h.app.screen = crate::app::Screen::Monitor;
        h.wait(0.3);
        for (size, text) in h.small_text() {
            seen.push(format!("monitor: {size} pt {text:?}"));
        }
        seen.dedup();
        assert_eq!(
            seen,
            [] as [String; 0],
            "text below the minimum:\n{}",
            seen.join("\n")
        );
    }

    #[test]
    fn ctrl_f_finds_in_the_st_and_ctrl_g_walks_the_matches() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        h.press("pane code");
        h.wait(0.3);
        let left = h.rect("left");
        h.key(Modifiers::COMMAND, Key::F);
        assert!(h.app.chrome.code_open());
        assert!(h.app.chrome.find.is_some(), "the find box opens");
        let editor = eframe::egui::Id::unique("pg-source");
        h.frame(vec![]);
        assert!(
            h.ctx.memory(|m| m.has_focus(editor.with("find"))),
            "and takes the keyboard"
        );
        h.frame(vec![Event::Text("PULSE".into())]);
        h.frame(vec![]);
        let src = h.app.sim.as_ref().unwrap().session.src.clone();
        let want = crate::find::matches(&src, "pulse");
        assert!(want.len() >= 2, "Blink names its timer more than once");
        // While the find box has the keyboard the editor keeps a caret:
        // at the current match's start.
        let selected = |h: &Harness| {
            let caret = eframe::egui::text_edit::TextEditState::load(&h.ctx, editor)
                .and_then(|s| s.cursor.char_range())
                .expect("a caret")
                .primary
                .index
                .0;
            let current = h.app.chrome.find.as_ref().map(|f| f.current);
            let at = want.iter().position(|r| src[..r.start].chars().count() == caret);
            assert_eq!(at, current, "the caret is at the current match");
            at
        };
        assert_eq!(selected(&h), Some(0), "typing selects the first match");
        h.key(Modifiers::COMMAND, Key::G);
        h.frame(vec![]);
        assert_eq!(selected(&h), Some(1), "Ctrl G: the next");
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::G);
        h.frame(vec![]);
        assert_eq!(selected(&h), Some(0), "Ctrl Shift G: back");
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::G);
        h.frame(vec![]);
        assert_eq!(selected(&h), Some(want.len() - 1), "and round to the last");
        let l = h.rect("left").right() - left.right();
        assert!((0.0..=8.0).contains(&l), "the find row widened the pane {l} pt");
        h.ctx.memory_mut(|m| m.request_focus(editor.with("find")));
        h.frame(vec![]);
        h.key(Modifiers::NONE, Key::Escape);
        assert!(h.app.chrome.find.is_none(), "Esc closes it");
        h.frame(vec![]);
        assert!(
            h.ctx.memory(|m| m.has_focus(editor)),
            "the editor takes the keyboard"
        );
        let last = &want[want.len() - 1];
        let chars = |byte: usize| src[..byte].chars().count();
        let (a, b) = crate::editor::selection(&h.ctx, editor).expect("the match selected");
        assert_eq!((a.min(b), a.max(b)), (chars(last.start), chars(last.end)));
    }

    /// Nick 2026-11-07: the pane kept widening to show a device's whole
    /// cip= identity. A project with identity-named devices and long
    /// names must leave both panels their width on every tab.
    #[test]
    fn long_device_identities_and_names_leave_the_panels_their_width() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let left = h.rect("left");
        let right = h.rect("right");
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("wide", None);
            sim.proj.add_file(
                "io.map",
                "device e1213 enip-io cip=991:515:0x2A629EAE profile=moxa-e1213 rpi_ms=100 poll_ms=20\n\
                 device e1213_by_mac_address_on_the_far_side_of_the_plant modbus-tcp mac=00-90-e8-7e-af-be unit=1 timeout_ms=300 poll_ms=20\n\
                 in  di00_the_part_present_sensor_at_the_infeed_of_station_one bool e1213 di:0\n\
                 out do0_the_clamp_solenoid_that_holds_the_part_while_it_is_drilled bool e1213 coil:0 safe=false\n\
                 in  counter_of_parts_seen_by_the_far_sensor int e1213_by_mac_address_on_the_far_side_of_the_plant ir:24:u32\n"
                    .into(),
            );
        }
        h.wait(0.3);
        for key in [
            "pane devices",
            "pane files",
            "pane code",
            "tab io",
            "tab claude",
            "pane devices",
            "tab io",
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

    #[test]
    fn every_tab_leaves_the_panels_their_width() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let left = h.rect("left");
        let right = h.rect("right");
        for key in [
            "pane files",
            "pane devices",
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
    use crate::stamp;
    use crate::theme::Theme;

    fn boxed(h: &Harness) -> &BoxView {
        h.app.boxv.as_ref().unwrap()
    }

    /// Nick 2026-11-07: once someone is signed in the sign-in panel
    /// collapses (the box's answer lands frames after its button), and the
    /// header shows the role: a hard hat for an operator, a wrench and a
    /// screwdriver for an engineer.
    #[test]
    fn signing_in_collapses_the_panel_and_the_header_shows_the_role() {
        use crate::review::Session;
        let mut h = Harness::new(App::boxed());
        h.wait(0.2);
        assert!(
            !h.has("role operator") && !h.has("role engineer"),
            "signed out: no role"
        );
        h.press("sign in");
        h.wait(0.2);
        assert!(h.app.login, "the panel opens");
        // The answer arrives a few frames later, as from the box.
        h.wait(0.2);
        h.app.review.as_mut().unwrap().state.session = Some(Session {
            user: None,
            role: "operator".into(),
        });
        h.wait(0.2);
        assert!(!h.app.login, "signed in: the panel collapses");
        assert!(h.has("role operator") && !h.has("role engineer"), "a hard hat");
        // Opened again while signed in, it stays open (to sign out).
        h.press("sign in");
        h.wait(0.2);
        assert!(h.app.login, "open again on a click");
        h.app.review.as_mut().unwrap().state.session = Some(Session {
            user: Some("alice".into()),
            role: "engineer".into(),
        });
        h.wait(0.2);
        assert!(!h.app.login, "a different person signed in: collapses");
        assert!(h.has("role engineer") && !h.has("role operator"), "the tools");
        // The glyph opens the panel too.
        h.press("role engineer");
        h.wait(0.2);
        assert!(h.app.login);
        h.app.review.as_mut().unwrap().state.session = None;
        h.wait(0.2);
        assert!(!h.app.login, "signed out: collapses");
        assert!(!h.has("role engineer"));
    }

    /// Card build-match: a box that answers as another build gets the
    /// banner, on the Editor and the Monitor alike, and the panels keep
    /// their width under it.
    #[test]
    fn a_box_of_another_build_shows_the_banner() {
        let mut h = Harness::new(App::boxed());
        h.wait(0.3);
        let (left, right) = (h.rect("left"), h.rect("right"));
        assert!(!h.has("build banner"), "no stamp heard yet");
        h.app.boxv.as_mut().unwrap().state.build = Some(format!("{}-other", stamp::CANVAS));
        h.wait(0.3);
        assert!(h.has("build banner") && h.has("reload"));
        assert!(
            h.rect("build banner").top() >= h.rect("header").bottom() - 1.0,
            "under the header"
        );
        assert!((h.rect("left").width() - left.width()).abs() <= 1.0);
        assert!((h.rect("right").width() - right.width()).abs() <= 1.0);
        h.app.screen = crate::app::Screen::Monitor;
        h.wait(0.3);
        assert!(h.has("build banner"), "on the Monitor too");
    }

    /// Nothing to compare is no mismatch: the same build, a box built
    /// without git ("unknown"), and the simulation show no banner.
    #[test]
    fn the_same_build_or_an_unknown_one_shows_no_banner() {
        let mut h = Harness::new(App::boxed());
        for build in [stamp::CANVAS, stamp::UNKNOWN, ""] {
            h.app.boxv.as_mut().unwrap().state.build = Some(build.to_string());
            h.wait(0.3);
            assert!(!h.has("build banner"), "{build:?}");
        }
        h.app.boxv.as_mut().unwrap().state.build = Some("other".into());
        h.app.source = Source::Sim;
        h.wait(0.3);
        assert!(!h.has("build banner"), "the simulation has no box to differ from");
    }

    /// The real box API names its build on the replies the canvas
    /// fetches, and a box and canvas from one checkout match.
    #[test]
    fn a_box_of_this_build_is_heard_and_matches() {
        let bench = Bench::new();
        let (state, _) = watch(&bench);
        assert_eq!(state.build.as_deref(), Some(stamp::CANVAS));
        let mut h = Harness::new(App::boxed());
        h.app.boxv.as_mut().unwrap().state = state;
        h.wait(0.3);
        assert!(!h.has("build banner"));
    }

    /// The minimum holds on a box too: its tabs, signed in, and the
    /// Proposals screen.
    #[test]
    fn no_text_on_a_box_is_below_the_minimum() {
        use crate::review::Session;
        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(App::boxed());
        h.app.boxv.as_mut().unwrap().state = state;
        h.wait(0.2);
        h.app.review.as_mut().unwrap().state.session = Some(Session {
            user: Some("nick".into()),
            role: "engineer".into(),
        });
        h.wait(0.2);
        let mut seen = Vec::new();
        for key in ["pane files", "pane devices", "pane code", "tab io", "tab claude"] {
            h.press(key);
            h.wait(0.3);
            for (size, text) in h.small_text() {
                seen.push(format!("{key}: {size} pt {text:?}"));
            }
        }
        h.app.screen = crate::app::Screen::Proposals;
        h.wait(0.3);
        for (size, text) in h.small_text() {
            seen.push(format!("proposals: {size} pt {text:?}"));
        }
        seen.dedup();
        assert_eq!(
            seen,
            [] as [String; 0],
            "text below the minimum:\n{}",
            seen.join("\n")
        );
    }

    /// The Agent tab on a box is the page's tab: signed out, Send stays
    /// off; signed in as an engineer with a program open, the instruction
    /// goes to `POST /api/chat` with the answer format, the whole file and
    /// no session yet; an agent that stores a proposal instead of sending
    /// the file back changes nothing, and its Review button opens the
    /// proposal under Proposals.
    #[test]
    fn the_agent_tab_on_a_box_sends_signed_in_and_opens_a_proposal_for_review() {
        use crate::app::Screen;
        use crate::box_agent::Incoming;
        use crate::box_projects::{Buf, OpenProject};
        use crate::instruct::Kind;
        use crate::instruct_tests::LAMP;
        use crate::review::Session;
        use serde_json::Value as Json;

        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(App::boxed());
        {
            let b = h.app.boxv.as_mut().unwrap();
            b.state = state;
            b.projects.available = Some(true);
            b.projects.open = Some(OpenProject {
                name: "line".into(),
                bufs: [(
                    "main.st".to_string(),
                    Buf {
                        text: LAMP.into(),
                        base: LAMP.into(),
                        version: Some("v1".into()),
                        saving: None,
                        loaded: true,
                    },
                )]
                .into(),
                current: Some("main.st".into()),
            });
        }
        h.frame(vec![]);
        h.frame(vec![]);
        h.press("tab claude");
        h.frame(vec![]);
        assert!(h.has("agent status") && h.has("agent instruction"));

        h.app.boxv.as_mut().unwrap().instruct.draft = "why does the conveyor stop?".into();
        h.frame(vec![]);
        h.press("agent send");
        h.frame(vec![]);
        assert_eq!(boxed(&h).chat_sent.len(), 0, "signed out, nothing is sent");

        h.app.review.as_mut().unwrap().state.session = Some(Session {
            user: Some("nick".into()),
            role: "engineer".into(),
        });
        h.frame(vec![]);
        h.press("agent send");
        h.frame(vec![]);
        let sent = &boxed(&h).chat_sent;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].url, "/api/chat");
        let body: Json = serde_json::from_str(&sent[0].body).unwrap();
        assert_eq!(body["session"], Json::Null);
        let message = body["message"].as_str().unwrap();
        assert!(message.starts_with(crate::instruct::BOX_RULES), "{message}");
        assert!(
            message.contains("\n\nwhy does the conveyor stop?\n\n<current_program>"),
            "{message}"
        );
        assert!(
            message.contains(&format!("text:\n{LAMP}</edit_scope>")),
            "{message}"
        );
        assert!(h.has("agent stop"), "Stop while it answers");

        let seq = sent[0].seq;
        let stream = "event: session\ndata: {\"id\":\"s1\"}\n\n\
            event: proposal\ndata: {\"ids\":[3],\"summary\":\"speed to 2\"}\n\n\
            event: done\ndata: {\"stop\":\"end_turn\",\"why\":null,\"steps\":2,\"output_tokens\":9,\"proposals\":[3]}\n\n";
        h.app.boxv.as_mut().unwrap().agent.apply(Incoming::Chunk {
            seq,
            text: stream.into(),
        });
        h.frame(vec![]);
        h.frame(vec![]);
        {
            let b = boxed(&h);
            let o = b.instruct.outcome.clone().expect("settled");
            assert_eq!(o.kind, Kind::Said, "{o:?}");
            assert_eq!(o.review, [3]);
            assert_eq!(
                b.projects.open.as_ref().unwrap().bufs["main.st"].text,
                LAMP,
                "untouched"
            );
        }
        h.press("agent review");
        h.frame(vec![]);
        h.frame(vec![]);
        assert_eq!(h.app.screen, Screen::Proposals);
        let asked: Vec<String> = h
            .app
            .review
            .as_ref()
            .unwrap()
            .sent
            .iter()
            .map(|r| r.url.clone())
            .collect();
        assert!(asked.iter().any(|u| u == "/api/proposals/3"), "{asked:?}");
    }

    /// Help mode on a box's Agent tab (card `help-mode-box`): signed out,
    /// Ask stays off; signed in as anyone, the question goes to
    /// `POST /api/help` with the page's Messages; the answer shows the
    /// diagnosis and the issue, whose Copy puts the whole Markdown on the
    /// clipboard; and the panels keep their width throughout.
    #[test]
    fn help_mode_on_a_box_asks_and_shows_a_copyable_issue() {
        use crate::box_agent::Incoming;
        use crate::review::Session;
        use serde_json::{Value as Json, json};

        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(App::boxed());
        h.app.boxv.as_mut().unwrap().state = state;
        h.wait(0.3);
        let left = h.rect("left");
        let right = h.rect("right");
        let kept = |h: &Harness, when: &str| {
            let l = h.rect("left").right() - left.right();
            let r = right.left() - h.rect("right").left();
            assert!((0.0..=8.0).contains(&l), "{when}: the left pane grew {l} pt");
            assert!((0.0..=8.0).contains(&r), "{when}: the right panel grew {r} pt");
        };
        h.press("tab claude");
        h.frame(vec![]);
        assert!(h.has("agent mode code") && h.has("agent mode help"));
        assert!(h.has("agent instruction") && !h.has("help question"));
        // Nick, 2026-11-08: the heading names the mode, the agent's state
        // is a dot beside it, and the glyph toggle sits at the top right.
        assert!(h.drew("Change the program") && !h.drew("Get help"));
        assert!(!h.drew("Agent on this box") && !h.drew("Agent online"));
        let (dot, code, help) = (
            h.rect("agent status"),
            h.rect("agent mode code"),
            h.rect("agent mode help"),
        );
        assert!(
            dot.left() < code.left() && code.right() <= help.left(),
            "{dot:?} {code:?} {help:?}"
        );
        assert!(
            help.right() >= h.rect("right").right() - 40.0,
            "at the right: {help:?}"
        );
        h.press("agent mode help");
        h.frame(vec![]);
        assert!(h.has("help question") && h.has("help ask") && !h.has("agent instruction"));
        assert!(h.drew("Get help") && !h.drew("Change the program"));

        h.app
            .boxv
            .as_mut()
            .unwrap()
            .log
            .error(2.0, "The box is unreachable.");
        h.app.boxv.as_mut().unwrap().help.draft = "The conveyor stopped.".into();
        h.frame(vec![]);
        h.press("help ask");
        h.frame(vec![]);
        assert!(boxed(&h).help_sent.is_empty(), "signed out, nothing is sent");

        h.app.review.as_mut().unwrap().state.session = Some(Session {
            user: Some("bob".into()),
            role: "operator".into(),
        });
        h.frame(vec![]);
        h.press("help ask");
        h.frame(vec![]);
        let sent = boxed(&h).help_sent.clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].url, "/api/help");
        let body: Json = serde_json::from_str(&sent[0].body).unwrap();
        assert_eq!(body["question"], "The conveyor stopped.");
        assert!(
            body["messages"]
                .as_str()
                .unwrap()
                .contains("error   The box is unreachable."),
            "{body}"
        );
        assert!(h.has("help stop") && h.has("help working"));
        kept(&h, "asking");

        let markdown = format!(
            "# Conveyor stops when rio drops\n\n### What happened\n\n{}\n\n### Context from the box\n\n- Devices:\n  - `rio`: down: connection refused\n",
            "The conveyor stopped and the panel lamp went red. ".repeat(12)
        );
        let issue = json!({
            "by": "agent", "title": "Conveyor stops when rio drops",
            "diagnosis": "The remote I/O module rio is not answering. Check its power and network cable.",
            "markdown": markdown,
        });
        let stream = format!(
            "event: help\ndata: {{\"context\":{{}}}}\n\nevent: issue\ndata: {issue}\n\nevent: done\ndata: {{\"stop\":\"end_turn\",\"why\":null,\"steps\":2,\"output_tokens\":9}}\n\n"
        );
        let seq = sent[0].seq;
        let b = h.app.boxv.as_mut().unwrap();
        b.help_answer(Incoming::Chunk { seq, text: stream });
        b.help_answer(Incoming::End { seq });
        h.frame(vec![]);
        h.frame(vec![]);
        assert!(h.has("help diagnosis") && h.has("help issue") && h.has("help issue copy"));
        assert!(!h.has("help stop"));
        kept(&h, "answered");
        let small = h.small_text();
        assert!(small.is_empty(), "text below the minimum: {small:?}");
        h.press("help issue copy");
        assert_eq!(h.copied.last(), Some(&markdown));
        assert!(
            boxed(&h)
                .log
                .text()
                .contains("Help: an issue was written (Conveyor stops when rio drops)."),
            "in Messages"
        );
        h.press("agent mode code");
        h.frame(vec![]);
        assert!(h.has("agent instruction"));
        kept(&h, "back to the code");
    }

    /// The Devices tab on a box: the map the box runs, each device with
    /// the link state the box reports and its points' live values; adding
    /// a device needs a project open and an engineer, so here the menu
    /// item is disabled.
    #[test]
    fn the_devices_tab_on_a_box_shows_the_running_map_with_link_states() {
        use crate::remote::DeviceRow;
        let bench = Bench::new();
        let (mut state, _) = watch(&bench);
        state.io_map = Some(
            "device rio modbus-tcp 10.0.0.57:502 unit=1\nin ready bool box di:0\nin part bool rio di:4\n"
                .into(),
        );
        state.devices = vec![
            DeviceRow {
                name: "box".into(),
                state: "up".into(),
                error: None,
                address: None,
            },
            DeviceRow {
                name: "rio".into(),
                state: "down".into(),
                error: Some("connection refused".into()),
                address: None,
            },
        ];
        let mut h = Harness::new(App::boxed());
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.frame(vec![]);
        h.press("pane devices");
        h.wait(0.3);
        assert!(h.app.chrome.devices_open());
        assert!(h.has("device rio") && h.has("device box"), "both devices");
        assert!(h.has("point part") && h.has("point ready"), "their points");
        h.right_click(h.rect("devices blank").center());
        assert!(h.has("menu new device"));
        h.press("menu new device");
        h.wait(0.2);
        assert!(
            !h.has("device form"),
            "no project open, nobody signed in: nothing to add to"
        );
    }

    /// On a box, Scan posts to `/api/devices/discover` and the answer's
    /// devices show with what the box said: mapped or not, the library's
    /// match; a refusal shows as text, and a hostile body lists nothing.
    #[test]
    fn scan_on_a_box_posts_and_lists_the_answer() {
        use crate::box_projects::PCall;
        use crate::devices::Scan;
        use crate::review::Answer;
        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(App::boxed());
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.press("pane devices");
        h.wait(0.2);
        h.press("devices scan");
        h.frame(vec![]);
        let sent = std::mem::take(&mut h.app.boxv.as_mut().unwrap().psent);
        let scan = sent
            .iter()
            .find(|r| r.call == PCall::Discover)
            .expect("the scan was sent");
        assert_eq!(
            (scan.method, scan.url.as_str()),
            ("POST", "/api/devices/discover?fresh=1")
        );
        assert_eq!(h.app.boxv.as_ref().unwrap().devices_ui.scan, Scan::Running);
        let body = r#"{ "devices": [ { "address": "10.0.0.57", "mac": "00-90-e8-7e-af-be", "vendor_id": 991, "product_code": 515,
            "revision": "3.1", "serial": "0x2a629eae", "product_name": "ioLogik E1213", "cip": "cip=991:515:0x2a629eae",
            "mapped": "e1213", "library": { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213", "driver": "enip-io", "profile": "moxa-e1213" } },
            { "address": "10.0.0.9", "vendor_id": 1, "product_code": 2, "product_name": "../etc/passwd\u0007", "mapped": null, "library": null } ],
            "age_ms": 3, "library": { "devices": 1 } }"#;
        let ctx = h.ctx.clone();
        h.app.boxv.as_mut().unwrap().step_all(
            &ctx,
            Vec::new(),
            vec![(
                PCall::Discover,
                Answer {
                    status: 200,
                    retry_after: None,
                    body: body.into(),
                },
            )],
            h.time,
        );
        h.frame(vec![]);
        h.frame(vec![]);
        let ui = &h.app.boxv.as_ref().unwrap().devices_ui;
        assert_eq!(ui.scan, Scan::Done);
        assert_eq!(ui.found.len(), 2);
        assert_eq!(ui.found[0].mapped.as_deref(), Some("e1213"));
        assert_eq!(
            ui.found[1].product_name, "../etc/passwd",
            "control characters dropped"
        );
        assert!(h.has("network 10.0.0.57") && h.has("network 10.0.0.9"));
        // A refusal.
        h.app.boxv.as_mut().unwrap().step_all(
            &ctx,
            Vec::new(),
            vec![(
                PCall::Discover,
                Answer {
                    status: 409,
                    retry_after: None,
                    body: r#"{"error":"busy"}"#.into(),
                },
            )],
            h.time,
        );
        assert!(matches!(
            h.app.boxv.as_ref().unwrap().devices_ui.scan,
            Scan::Failed(_)
        ));
        // Garbage lists nothing and fails softly.
        h.app.boxv.as_mut().unwrap().step_all(
            &ctx,
            Vec::new(),
            vec![(
                PCall::Discover,
                Answer {
                    status: 200,
                    retry_after: None,
                    body: "[[[".into(),
                },
            )],
            h.time,
        );
        assert!(matches!(
            h.app.boxv.as_ref().unwrap().devices_ui.scan,
            Scan::Failed(_)
        ));
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
        assert_eq!(b.status(h.time, &Theme::dark()).0, "● connected");
        assert!(
            !h.has("tab trend") && h.has("tab claude"),
            "I/O and the Agent on a box (the Trend is along the bottom)"
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
            b.status(t, &Theme::dark())
                .0
                .starts_with("● reconnecting (retry in 5s)"),
            "{:?}",
            b.status(t, &Theme::dark())
        );
        for k in 1..3 {
            b.state.apply(Msg::Failed(Stream::Values), t + f64::from(k));
        }
        assert!(matches!(b.state.link(), Link::Unreachable { .. }));
        assert!(
            b.status(t + 2.0, &Theme::dark())
                .0
                .starts_with("● box unreachable"),
            "{:?}",
            b.status(t + 2.0, &Theme::dark())
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
        // Above the button: what the deploy keeps and resets, in words.
        assert!(
            h.drew("Before you approve: this deploy keeps 5 values, starts HORN at the initial value (new)."),
            "the approval screen shows the preview"
        );
        assert!(h.drew("new HORN"), "the reset value is marked apart");
        let state = rect(&mut h, "state");
        assert!(state.top() < rect(&mut h, &format!("go {:?}", Target::Proposal(id))).top());
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
        assert!(
            h.drew("Applied: this deploy keeps 5 values, starts HORN at the initial value (new)."),
            "what the deploy did, as the box reported it"
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
        // A storm may have hit the panel's hide chevron: open it again.
        if !h.has(tab) {
            h.press("right");
            h.wait(0.3);
        }
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

// ---- the guided tour ----

/// The tour (`crate::tour`) driven through the real interface: every step
/// done by clicking and typing where its card points.
mod tour {
    use eframe::egui::{self, Event};

    use super::{App, Harness, Screen, Source};
    use crate::app::wants_tour;
    use crate::tour::{Step, Tour};

    fn step(h: &Harness) -> Step {
        h.app.tour.as_ref().expect("the tour runs").step
    }

    fn anchor(h: &Harness) -> Option<String> {
        h.app.tour.as_ref().and_then(|t| t.anchor.clone())
    }

    /// The card is on the screen, clear of the control it points at, and
    /// no text on the screen is below the minimum.
    fn card_is_fair(h: &Harness, at: &str) {
        let card = h.rect("tour card");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, h.size);
        assert!(
            screen.expand(0.5).contains_rect(card),
            "{at}: the card is off the screen: {card:?}"
        );
        if let Some(mark) = h.app.chrome.rect("tour mark") {
            assert!(
                !card.intersects(mark),
                "{at}: the card covers what it points at: {card:?} {mark:?}"
            );
        }
        assert_eq!(h.small_text(), [] as [(f32, String); 0], "{at}");
    }

    #[test]
    fn tour_parses_from_the_query() {
        for q in ["?tour", "tour", "?playground&tour", "?tour=1"] {
            assert!(wants_tour(q), "{q}");
        }
        for q in ["", "?tours", "?playground", "?detour"] {
            assert!(!wants_tour(q), "{q}");
        }
    }

    #[test]
    fn plain_pages_have_no_tour_and_help_starts_one() {
        let mut h = Harness::new(App::sim());
        assert!(h.app.tour.is_none());
        assert!(!h.has("tour card"));
        h.press("bar help");
        h.press("help tour");
        h.frame(vec![]);
        assert_eq!(step(&h), Step::Welcome);
        assert!(h.has("tour card"));
        // Not on a box: the tour is the simulation's.
        let mut b = Harness::new(App::boxed());
        b.press("bar help");
        assert!(!b.has("help tour"));
    }

    /// Start to finish, each step done by what its card asks: Blink
    /// copied under a name, its time changed, deployed, its lamp picked,
    /// the trend's window moved, a key pasted and an instruction sent.
    #[test]
    fn the_tour_runs_start_to_finish_on_the_real_interface() {
        let mut h = Harness::new(App::sim());
        h.app.tour = Some(Tour::new());
        h.wait(0.3);
        let (left, right) = (h.rect("left"), h.rect("right"));
        card_is_fair(&h, "welcome");
        assert_eq!(anchor(&h), None, "the welcome sits in the middle");
        h.press("tour start");
        h.wait(0.2);
        assert_eq!(step(&h), Step::OpenBlink);
        // The playground opens on the Code tab: the card points at Files.
        assert_eq!(anchor(&h).as_deref(), Some("pane files"));
        card_is_fair(&h, "files tab");
        h.press("pane files");
        h.wait(0.2);
        assert_eq!(anchor(&h).as_deref(), Some("project blink"));
        card_is_fair(&h, "blink");
        h.press("project blink");
        h.wait(0.3);
        assert_eq!(step(&h), Step::Name);
        assert_eq!(anchor(&h).as_deref(), Some("name"));
        card_is_fair(&h, "name");
        h.press("name ok");
        h.wait(0.3);
        assert_eq!(step(&h), Step::Edit);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(!sim.proj.is_example(), "a copy of its own: {}", sim.proj.open);
        let mine = sim.proj.open.clone();
        assert_eq!(anchor(&h).as_deref(), Some("pane code"));
        card_is_fair(&h, "code tab");
        h.press("pane code");
        h.wait(0.2);
        assert_eq!(anchor(&h).as_deref(), Some("st editor"));
        card_is_fair(&h, "editor");
        // Select 500 in T#500ms and type 200 over it.
        let id = egui::Id::unique("pg-source");
        let editor = h.ctx.read_response(id).expect("the editor").rect;
        h.click(editor.left_top() + egui::vec2(60.0, 6.0));
        let sim = h.app.sim.as_ref().unwrap();
        let text = sim.proj.files[&sim.proj.current].clone();
        let at = text[..text.find("500ms").expect("Blink's time")].chars().count();
        crate::editor::select(&h.ctx, id, at, at + 3);
        h.frame(vec![Event::Text("200".into())]);
        h.wait(0.6);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(sim.proj.files[&sim.proj.current].contains("T#200ms"));
        assert_eq!(step(&h), Step::Deploy);
        assert_eq!(anchor(&h).as_deref(), Some("bar file"));
        card_is_fair(&h, "file menu");
        h.press("bar file");
        assert_eq!(anchor(&h).as_deref(), Some("file deploy"));
        h.press("file deploy");
        h.wait(0.3);
        assert_eq!(step(&h), Step::Watch);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(sim.session.running && !sim.session.stale(), "the edit runs");
        assert_eq!(sim.proj.running.as_deref(), Some(mine.as_str()));
        assert_eq!(anchor(&h).as_deref(), Some("io outputs"));
        card_is_fair(&h, "outputs");
        // The lamp, the first output.
        let outs = h.rect("io outputs");
        h.click(outs.left_center() + egui::vec2(30.0, 0.0));
        h.wait(0.3);
        assert_eq!(step(&h), Step::Trend);
        assert_eq!(anchor(&h).as_deref(), Some("trend window"));
        card_is_fair(&h, "trend");
        let w = h.rect("trend window");
        h.click(w.left_center() + egui::vec2(4.0, 0.0));
        h.wait(0.3);
        assert_eq!(step(&h), Step::Agent);
        assert_eq!(anchor(&h).as_deref(), Some("tab claude"));
        card_is_fair(&h, "agent tab");
        h.press("tab claude");
        h.wait(0.3);
        assert_eq!(anchor(&h).as_deref(), Some("agent key"));
        card_is_fair(&h, "key");
        h.press("agent key");
        h.frame(vec![Event::Text("sk-ant-api03-tour".into())]);
        h.wait(0.2);
        assert_eq!(anchor(&h).as_deref(), Some("agent instruction"));
        h.press("agent instruction");
        h.frame(vec![Event::Text(
            "keep the lamp on for 1 s and off for 2 s".into(),
        )]);
        h.wait(0.2);
        h.press("agent send");
        h.wait(0.2);
        assert_eq!(step(&h), Step::Done);
        assert_eq!(
            h.app.sim.as_ref().unwrap().link.sent.len(),
            1,
            "one instruction sent"
        );
        card_is_fair(&h, "done");
        h.press("tour finish");
        h.wait(0.2);
        assert!(h.app.tour.is_none());
        assert!(!h.has("tour card"));
        // The side panels kept their widths the whole way.
        let l = h.rect("left").right() - left.right();
        let r = right.left() - h.rect("right").left();
        assert!((0.0..=8.0).contains(&l), "the left pane grew {l} pt");
        assert!((0.0..=8.0).contains(&r), "the right panel grew {r} pt");
    }

    /// Skip at the first card, or at the first step, leaves the app as a
    /// page with no tour would be.
    #[test]
    fn skip_leaves_the_app_untouched() {
        for presses in [&["tour skip"][..], &["tour start", "tour skip"][..]] {
            let mut plain = Harness::new(App::sim());
            let mut toured = Harness::new(App::sim());
            toured.app.tour = Some(Tour::new());
            toured.wait(0.2);
            plain.wait(0.2);
            for p in presses {
                let at = toured.rect(p).center();
                toured.click(at);
                toured.wait(0.2);
                // The same clicks' worth of time on the page without the
                // tour, the pointer nowhere.
                plain.wait(3.0 / 60.0 + 0.2);
            }
            toured.wait(0.2);
            plain.wait(0.2);
            assert!(toured.app.tour.is_none(), "{presses:?}");
            assert_eq!(toured.app.chrome.saved(), plain.app.chrome.saved(), "{presses:?}");
            assert_eq!(toured.app.screen, Screen::Editor);
            assert_eq!(toured.app.source, Source::Sim);
            let (a, b) = (toured.app.sim.as_ref().unwrap(), plain.app.sim.as_ref().unwrap());
            assert_eq!(a.proj.open, b.proj.open);
            assert_eq!(a.proj.files, b.proj.files);
            assert_eq!(a.proj.rows().len(), b.proj.rows().len());
            assert_eq!(a.session.running, b.session.running);
            assert!(a.files_ui.naming.is_none());
            assert_eq!(a.chat.seq(), 0);
        }
    }
}
