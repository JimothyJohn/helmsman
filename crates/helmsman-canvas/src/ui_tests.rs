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

    /// The page as it opened before the traffic light took first place
    /// (2026-11-12): the Blink example open and running. The tests that
    /// click its LAMP block or count its lines start here.
    /// The simulation with its Code tab shown (the page opens on the
    /// Agent since 2026-11-13).
    fn code() -> Self {
        let mut h = Self::new(App::sim());
        h.app.chrome.show_code();
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn blink() -> Self {
        let mut h = Self::code();
        h.app.sim.as_mut().expect("the simulation").open_project("blink");
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

    /// The text the last frame drew in the code face left of `x`.
    fn mono_left_of(&self, x: f32) -> Vec<String> {
        fn walk(shape: &egui::Shape, x: f32, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, x, out)),
                egui::Shape::Text(t) if t.pos.x < x => {
                    let job = &t.galley.job;
                    for sec in &job.sections {
                        let text = job
                            .text
                            .get(sec.byte_range.start.0..sec.byte_range.end.0)
                            .unwrap_or("");
                        if !text.trim().is_empty() && sec.format.font_id.family == egui::FontFamily::Monospace
                        {
                            out.push(text.trim().chars().take(60).collect());
                        }
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for c in &self.shapes {
            walk(&c.shape, x, &mut out);
        }
        out
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

    /// The simulation's ST editor's widget id for the open file.
    fn editor_id(&self) -> egui::Id {
        self.app.sim.as_ref().unwrap().source_id()
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

/// All the text the last frames drew.
fn all_text(h: &mut Harness) -> String {
    fn walk(s: &egui::Shape, out: &mut String) {
        match s {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            _ => {}
        }
    }
    h.wait(0.1);
    let mut out = String::new();
    for c in &h.shapes {
        walk(&c.shape, &mut out);
    }
    out
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

/// The bounds of the line and path strokes the last frame drew inside
/// `area` (a hand-drawn glyph is lines and paths; the fills under it are
/// rects, left out).
fn stroke_bounds_in(h: &Harness, area: Rect) -> Rect {
    fn walk(s: &egui::Shape, area: Rect, out: &mut Rect) {
        match s {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            egui::Shape::LineSegment { points, .. } => {
                for p in points.iter().filter(|p| area.contains(**p)) {
                    out.extend_with(*p);
                }
            }
            egui::Shape::Path(path) => {
                for p in path.points.iter().filter(|p| area.contains(**p)) {
                    out.extend_with(*p);
                }
            }
            _ => {}
        }
    }
    let mut out = Rect::NOTHING;
    for c in &h.shapes {
        walk(&c.shape, area, &mut out);
    }
    out
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
        for k in [
            "state", "run", "stop", "step", "restart", "save", "upload", "download",
        ] {
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
        // Nick, 2026-11-11: the state and the clock are right beside the
        // glyphs that change them, not in the header.
        let (state, run) = (h.rect("state"), h.rect("run"));
        assert!(state.right() <= run.left() && run.left() - state.right() < 24.0);
        assert!(
            (state.center().y - run.center().y).abs() < 8.0,
            "{state:?} beside {run:?}"
        );
        assert!(h.drew("running · "), "the running clock");
        assert!(h.app.sim.as_ref().unwrap().session.running);
        // Pause holds the state; Step goes to the next change.
        h.press("run");
        assert!(!h.app.sim.as_ref().unwrap().session.running, "the glyph pauses");
        h.frame(vec![]);
        assert!(h.drew("paused · "));
        let (scans, now) = {
            let s = &h.app.sim.as_ref().unwrap().session;
            (s.scans, s.now_ms)
        };
        assert!(scans > 0, "what ran is kept");
        h.wait(0.5);
        assert_eq!(
            h.app.sim.as_ref().unwrap().session.now_ms,
            now,
            "the clock stands"
        );
        h.press("step");
        assert!(
            h.app.sim.as_ref().unwrap().session.scans > scans,
            "a step moves on"
        );
        h.press("restart");
        assert_eq!(h.app.sim.as_ref().unwrap().session.scans, 0, "a cold restart");
        h.press("run");
        assert!(h.app.sim.as_ref().unwrap().session.running, "and starts again");
        // Stop ends the run: initial values, clock at zero, no scan shown.
        h.wait(0.3);
        h.press("stop");
        {
            let s = &h.app.sim.as_ref().unwrap().session;
            assert!(!s.running && s.trace.is_none());
            assert_eq!((s.scans, s.now_ms), (0, 0));
        }
        h.frame(vec![]);
        assert!(h.drew("stopped · 0.0s"));
        h.press("run");
        assert!(
            h.app.sim.as_ref().unwrap().session.running,
            "and runs from the start"
        );
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
        h.press("bottom messages");
        // The panel takes the tab's height: let it settle.
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
        // Nick, 2026-11-11: no Close button under the text. A click
        // inside the window leaves it open; one outside it, or Escape,
        // shuts it.
        assert!(!h.has("about close"));
        let about = h.rect("about");
        h.click(about.center());
        assert!(h.app.about, "a click on the window keeps it");
        h.click(about.left_bottom() + eframe::egui::Vec2::new(-40.0, 40.0));
        assert!(!h.app.about, "a click outside shuts it");
        h.press("bar help");
        h.press("help about");
        h.frame(vec![]);
        assert!(h.app.about);
        h.key(eframe::egui::Modifiers::NONE, eframe::egui::Key::Escape);
        assert!(!h.app.about, "Escape too");
    }

    /// The UI review of 2026-11-10: "Monitor" touched the edge of the
    /// Editor | Monitor switch. Since 2026-11-13 each side is a glyph (the
    /// code brackets, a screen): the two sides are a tap target wide, the
    /// same width, and each glyph's strokes stay clear of its side's edge.
    #[test]
    fn the_mode_switch_is_two_glyphs_clear_of_its_edges() {
        let h = Harness::new(App::sim());
        let (editor, monitor) = (h.rect("editor"), h.rect("monitor"));
        assert!((editor.width() - monitor.width()).abs() < 0.01);
        assert!(editor.width() >= crate::st_view::TAP, "a tap target: {editor:?}");
        for side in [editor, monitor] {
            let ink = super::stroke_bounds_in(&h, side);
            assert!(ink.is_positive(), "a glyph in {side:?}");
            assert!(
                side.shrink(4.0).contains_rect(ink),
                "{ink:?} clear of the edges of {side:?}"
            );
        }
        assert!(!h.drew("Editor") && !h.drew("Monitor"), "no words in the switch");
    }

    /// The Agent tab's Send is a paper plane, not the word (2026-11-13).
    #[test]
    fn the_agents_send_is_a_glyph() {
        let mut h = Harness::new(App::sim());
        h.app.chrome.show(crate::editor::Pane::Claude);
        h.frame(vec![]);
        h.frame(vec![]);
        let send = h.rect("agent send");
        let ink = super::stroke_bounds_in(&h, send);
        assert!(
            ink.is_positive() && send.shrink(2.0).contains_rect(ink),
            "{ink:?} in {send:?}"
        );
        assert!(!h.drew("Send"), "no word on it");
    }

    /// The same review: a small program sat in a sixth of the canvas at
    /// zoom 1.2, its port names 12 pt. A fit goes to `FIT_MAX`, where they
    /// are the 16 pt the panels hold to; the Code tab opens at `LEFT_W`,
    /// wide enough that the blink example's longest line wraps once.
    #[test]
    fn a_small_program_is_fitted_large_and_the_code_tab_opens_wide() {
        let h = Harness::blink();
        let zoom = crate::ui_tests::sim_view(&h).cams[0].zoom;
        assert!(
            (zoom - crate::st_view::FIT_MAX).abs() < 1e-3,
            "blink fits the canvas at the cap: {zoom}"
        );
        assert!(10.0 * zoom >= crate::theme::MIN_TEXT, "port names at {zoom}");
        // The diagram starts where the left pane ends.
        let left = h.app.chrome.diagram.left();
        assert!(
            (left - crate::editor::LEFT_W).abs() <= 16.0,
            "the left pane at its default width: {left}"
        );
    }

    /// The same review, on a box nobody is signed in to: the popup was
    /// headed "Signed in" over empty fields, and save, upload and download
    /// glyphs were drawn with nothing to act on.
    #[test]
    fn a_signed_out_box_page_offers_sign_in_and_no_glyphs() {
        let mut h = Harness::new(App::boxed());
        h.wait(0.2);
        for k in ["save", "upload", "download"] {
            assert!(!h.has(k), "a {k} glyph with nobody signed in");
        }
        h.key(eframe::egui::Modifiers::ALT, eframe::egui::Key::F8);
        h.frame(vec![]);
        assert!(h.app.login);
        assert!(h.drew("Sign in") && !h.drew("Signed in"), "the popup's heading");
    }

    /// The same review: two tiles on a 1400 pt Monitor sat in the left
    /// half of a four-column grid. Few tiles take the width between them.
    #[test]
    fn the_monitors_tiles_take_the_width() {
        let mut h = Harness::blink();
        h.press("monitor");
        h.wait(0.3);
        let (status, outputs) = (h.rect("status blink"), h.rect("outputs blink/"));
        assert!((status.top() - outputs.top()).abs() < 1.0, "one row");
        assert!(
            outputs.right() > h.size.x * 0.9,
            "the row reaches the right edge: {outputs:?}"
        );
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
    use crate::editor::{Pane, numbering};

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
        let mut h = Harness::code();
        assert!(
            h.app.sim.as_ref().unwrap().session.running,
            "the playground opens running"
        );
        h.key(Modifiers::SHIFT, Key::F8);
        assert!(!h.app.sim.as_ref().unwrap().session.running, "Shift+F8 stops");
        let scans = h.app.sim.as_ref().unwrap().session.scans;
        h.key(Modifiers::NONE, Key::F8);
        assert_eq!(
            h.app.sim.as_ref().unwrap().session.scans,
            scans + 1,
            "F8 steps one scan"
        );
        h.key(Modifiers::COMMAND, Key::Enter);
        assert!(h.app.sim.as_ref().unwrap().session.running, "Ctrl+Enter runs");
        h.key(Modifiers::COMMAND, Key::Enter);
        assert!(!h.app.sim.as_ref().unwrap().session.running, "and stops");
        // The panes, from the Code tab.
        assert!(h.app.chrome.code_open());
        let pane = Modifiers::ALT.plus(Modifiers::SHIFT);
        h.key(pane, Key::Num1);
        assert!(!h.app.chrome.left, "Alt+Shift+1 on the Code tab shuts the pane");
        h.key(pane, Key::Num0);
        assert!(h.app.chrome.files_open(), "Alt+Shift+0: the Files tab");
        h.key(pane, Key::Num1);
        assert!(h.app.chrome.code_open(), "Alt+Shift+1: the Code tab");
        h.key(pane, Key::Num4);
        assert!(h.app.chrome.devices_open(), "Alt+Shift+4: the Devices tab");
        h.key(pane, Key::Num4);
        assert!(
            !h.app.chrome.left,
            "Alt+Shift+4 on the Devices tab shuts the pane"
        );
        h.key(pane, Key::Num1);
        assert!(h.app.chrome.code_open());
        // Alt+Shift+5: the left pane's tab to a pane on the right, the
        // one under it shown on the left (Nick, 2026-11-13).
        assert!(h.app.chrome.rights.is_empty(), "every tab on the left at first");
        h.key(pane, Key::Num5);
        assert_eq!(h.app.chrome.rights, [Pane::Code]);
        assert!(h.app.chrome.code_open(), "the Code tab shown on the right");
        assert_eq!(h.app.chrome.pane, Pane::Io, "the tab under it on the left");
        assert!(h.app.chrome.left, "the left pane stays open");
        h.wait(0.3);
        assert!(h.has("right"), "the right pane drawn");
        h.key(pane, Key::Num3);
        assert!(
            !h.app.chrome.right && !h.app.chrome.code_open(),
            "Alt+Shift+3 shuts the right pane"
        );
        h.key(pane, Key::Num3);
        assert!(h.app.chrome.code_open(), "and opens it");
        // Alt alone on a digit is Firefox's tab switch: not ours any more.
        h.key(Modifiers::ALT, Key::Num1);
        assert!(h.app.chrome.code_open(), "Alt+1 does nothing");
        // Save: edit, then Ctrl+S.
        h.wait(0.2);
        let editor = h.ctx.read_response(h.editor_id()).expect("the editor").rect;
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
    fn alt_shift_d_opens_the_declaration_and_alt_shift_x_lights_the_references() {
        let mut h = Harness::blink();
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::Num1);
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
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::D);
        assert!(h.app.chrome.code_open(), "the ST opens");
        let line = h.app.chrome.line.expect("sent to the declaration");
        let src = h.app.sim.as_ref().unwrap().session.src.clone();
        let decl = src.lines().nth(line as usize - 1).unwrap_or("");
        assert!(
            decl.to_ascii_lowercase().contains("lamp"),
            "line {line}: {decl:?}"
        );
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::X);
        h.frame(vec![]);
        let v = super::sim_view(&h);
        assert!(v.highlighted().is_some_and(|d| !d.is_empty()), "references lit");
        h.press("bar edit");
        for k in ["edit definition", "edit xref"] {
            assert!(h.has(k), "no {k}");
        }
    }

    #[test]
    fn shift_f9_on_a_picked_block_pauses_the_scan_there_and_f8_runs_the_rest() {
        fn session(h: &Harness) -> &crate::playground::Session {
            &h.app.sim.as_ref().unwrap().session
        }
        let mut h = Harness::blink();
        h.key(Modifiers::ALT, Key::Num1);
        h.wait(0.6);
        let (at, _) = super::node(super::sim_view(&h), super::write_to("LAMP"));
        h.click(at);
        h.frame(vec![]);
        let id = super::sim_view(&h).picked_stmt().expect("a statement picked");
        h.key(Modifiers::SHIFT, Key::F9);
        assert_eq!(session(&h).breaks, vec![id], "Shift F9 sets the breakpoint");
        assert_eq!(super::sim_view(&h).breaks, vec![id], "and the diagram marks it");
        // Stopped, F8 steps a scan: it stops before the statement.
        h.key(Modifiers::SHIFT, Key::F8);
        assert!(!session(&h).running);
        if session(&h).paused.is_some() {
            // It was running and stopped there on its own: finish that scan.
            h.key(Modifiers::NONE, Key::F8);
        }
        // The lamp's write is under `IF pulse.Q`: F8 steps scans until
        // the timer fires and the scan stops before it.
        let mut steps = 0;
        while session(&h).paused.is_none() && steps < 200 {
            h.key(Modifiers::NONE, Key::F8);
            steps += 1;
        }
        assert_eq!(session(&h).paused_at(), Some(id), "after {steps} steps");
        let scans = session(&h).scans;
        assert_eq!(super::sim_view(&h).paused, Some(id), "the diagram shows where");
        h.key(Modifiers::NONE, Key::F8);
        assert_eq!(session(&h).paused, None, "F8 again runs the rest");
        assert_eq!(session(&h).scans, scans + 1);
        h.key(Modifiers::SHIFT, Key::F9);
        assert!(session(&h).breaks.is_empty(), "Shift F9 again clears it");
        h.press("bar edit");
        assert!(h.has("edit breakpoint"), "the Edit menu lists it with its key");
    }

    #[test]
    fn alt_shift_f_flips_the_picked_switch_and_opens_a_number_field() {
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
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::F);
        assert_eq!(
            value(&h, "enable"),
            Some(Value::Bool(false)),
            "the switch flipped"
        );
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::F);
        assert_eq!(value(&h, "enable"), Some(Value::Bool(true)), "and back");
        let k = pick(&mut h, "speed");
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::F);
        h.frame(vec![]);
        let field = egui::Id::unique(("pg-map-number", k));
        assert!(
            h.ctx.memory(|m| m.has_focus(field)),
            "the speed's field has the keyboard"
        );
        // Nothing to release in the simulation; the key does nothing.
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::U);
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

    /// Nick, 2026-11-08: the Code header is the open file's name, with a
    /// word-wrap toggle between it and the deploy arrow. Wrap off, a long
    /// line is one row behind a sideways scroll bar and the pane keeps its
    /// width; the choice is remembered with the layout.
    #[test]
    fn the_code_header_names_the_file_and_toggles_word_wrap() {
        use crate::editor::Chrome;
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let left = h.rect("left");
        h.press("pane code");
        h.wait(0.3);
        let file = h.app.sim.as_ref().unwrap().proj.current.clone();
        assert!(h.drew(&file), "the header is the file name {file}");
        assert!(!h.drew("Structured Text"), "no generic header");
        let (name, wrap, arrow) = (h.rect("code file"), h.rect("code wrap"), h.rect("code deploy"));
        assert!(
            name.right() <= wrap.left() && wrap.right() <= arrow.left(),
            "name, wrap, arrow in a row: {name:?} {wrap:?} {arrow:?}"
        );
        assert!(h.app.chrome.wrap, "wrap is on by default");
        // "left" is the pane's toggle, on the pane's right edge.
        let pane = left.right();
        let wrapped = h.rect("st editor");
        assert!(
            wrapped.right() <= pane,
            "wrapped, the editor fits the pane: {wrapped:?}"
        );
        // A line far wider than the pane.
        {
            let sim = h.app.sim.as_mut().unwrap();
            let current = sim.proj.current.clone();
            let long = format!("\n(* {} *)", "wide ".repeat(200));
            sim.proj.files.get_mut(&current).unwrap().push_str(&long);
        }
        h.frame(vec![]);
        assert!(h.rect("st editor").right() <= pane, "still wrapped");
        h.press("code wrap");
        h.wait(0.3);
        assert!(!h.app.chrome.wrap, "the toggle turned wrap off");
        assert!(
            h.rect("st editor").width() > 2.0 * pane,
            "unwrapped, the long line runs on: {} pt wide",
            h.rect("st editor").width()
        );
        let grew = h.rect("left").right() - left.right();
        assert!(
            (0.0..=8.0).contains(&grew),
            "wrap off: the left pane grew {grew} pt"
        );
        let saved = h.app.chrome.saved();
        assert!(saved.contains("wrap=0"), "{saved}");
        assert!(!Chrome::restored(&saved).wrap, "read back");
        assert!(Chrome::restored("left=1").wrap, "absent: on");
        h.press("code wrap");
        h.wait(0.3);
        assert!(h.app.chrome.wrap, "and back on");
        assert!(h.rect("st editor").right() <= pane, "wrapped again");
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
        for k in ["run", "stop", "step", "restart"] {
            assert!(h.has(k), "the Monitor has {k}");
        }
        let running = |h: &Harness| h.app.sim.as_ref().unwrap().session.running;
        let scans = |h: &Harness| h.app.sim.as_ref().unwrap().session.scans;
        assert!(running(&h), "the example opens running");
        h.press("run");
        assert!(!running(&h), "paused from the Monitor");
        let n = scans(&h);
        assert!(n > 0, "a pause keeps what ran");
        h.press("step");
        assert!(scans(&h) > n, "on to the next change");
        h.press("restart");
        assert_eq!(scans(&h), 0, "a cold restart");
        h.press("run");
        assert!(running(&h), "and running again");
        // The keys, on the Monitor: Ctrl Enter stops, F8 steps.
        h.key(Modifiers::COMMAND, Key::Enter);
        assert!(!running(&h), "Ctrl Enter pauses on the Monitor");
        let n = scans(&h);
        h.key(Modifiers::NONE, Key::F8);
        assert!(scans(&h) > n, "F8 steps on the Monitor");
        // Shift F8 and the Stop glyph end the run.
        h.key(Modifiers::SHIFT, Key::F8);
        assert_eq!(scans(&h), 0, "Shift F8 stops: nothing of the run is left");
        h.press("run");
        h.wait(0.2);
        h.press("stop");
        assert!(!running(&h));
        assert_eq!(scans(&h), 0, "the glyph too");
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
        assert!(
            h.app.chrome.rights.is_empty(),
            "nothing docked on the right: nothing to show"
        );
        h.view_menu("view tab claude");
        assert!(
            h.app.chrome.left && h.app.chrome.showing(Pane::Claude),
            "a tab opens its pane"
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
        let mut h = Harness::code();
        assert!(h.app.chrome.code_open(), "the ST open");
        let editor = h.ctx.read_response(h.editor_id()).expect("the editor").rect;
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

    /// Nick, 2026-11-05: no chat; 2026-11-13: the box takes the tab's
    /// height, a change that lands is seen on the canvas (running, marked)
    /// and not described in the tab, one that fails is an error in the
    /// Messages with the box outlined red, and Before / After swaps the
    /// file at once, marked each way. The hit stays "tab claude".
    /// Nick, 2026-11-13: "I would love to see progressive changes being
    /// made rather than waiting a long time assuming something is
    /// happening." While the Agent thinks: the time and its latest
    /// thought; once it writes: the code as it arrives, in the Agent tab
    /// and in the Code tab, marked a draft; the file itself unchanged
    /// until the checks pass; both panes keep their widths throughout.
    #[test]
    fn the_agent_shows_its_thinking_then_the_code_as_it_is_written() {
        use crate::claude::Incoming;
        use crate::instruct_tests::{LAMP, follows};
        let ev = |name: &str, data: serde_json::Value| format!("event: {name}\ndata: {data}\n\n");
        let mut h = Harness::new(App::sim());
        // The Code tab beside the Agent, docked right at its narrowest.
        h.app.chrome.dock(Pane::Code, true);
        {
            let v = h.app.sim.as_mut().unwrap();
            v.chat.receive(Incoming::Proxy(true));
            v.proj.create("mine", None);
            v.proj.files.insert("main.st".into(), LAMP.into());
            v.proj.save();
            v.open_project("mine");
        }
        h.wait(0.3);
        let (left, right) = (h.rect("left"), h.rect("right"));
        let field = h.rect("agent instruction");
        h.click(field.center());
        h.frame(vec![Event::Text("comment it".into())]);
        h.press("agent send");
        h.frame(vec![]);
        let seq = h.app.sim.as_ref().unwrap().link.sent.last().unwrap().0;
        let chunk = |h: &mut Harness, text: String| {
            h.app
                .sim
                .as_mut()
                .unwrap()
                .link
                .inbox
                .push(Incoming::Chunk { seq, text });
            h.wait(0.3);
        };
        assert!(h.drew("Thinking · "), "the clock from the start");
        // Thinking: its last line, faint.
        chunk(
            &mut h,
            [
                ev("message_start", serde_json::json!({"type":"message_start","message":{"model":"m","id":"m","type":"message","role":"assistant","content":[],"stop_reason":null}})),
                ev("content_block_start", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})),
                ev("content_block_delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"The lamp follows start.\nA comment after the assignment says why"}})),
            ]
            .concat(),
        );
        let thought = h.rect("agent thought");
        assert!(
            thought.bottom() <= h.size.y,
            "the thought is on screen: {thought:?}"
        );
        assert!(h.has("agent thought") && h.drew("A comment after the assignment says why"));
        assert!(!h.has("agent draft") && !h.has("st draft"), "no code yet");
        // Writing: the code so far, in both tabs, the file untouched.
        let new = LAMP.replace("lamp := start;", "lamp := start; (* follows start *)");
        let (head, tail) = new.split_at(new.find("lamp :=").unwrap());
        chunk(
            &mut h,
            [
                ev("content_block_stop", serde_json::json!({"type":"content_block_stop","index":0})),
                ev("content_block_start", serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}})),
                ev("content_block_delta", serde_json::json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":format!("Why: it says why the lamp is on.\n\n```st\n{head}")}})),
            ]
            .concat(),
        );
        let lines = head.lines().count();
        assert!(
            h.drew(&format!("Writing main.st · {lines} lines")),
            "the lines so far"
        );
        assert!(h.has("agent draft") && !h.has("agent thought"));
        let draft = h.rect("agent draft");
        assert!(draft.bottom() <= h.size.y, "the draft is on screen: {draft:?}");
        assert!(h.has("st draft"), "the Code tab shows the draft");
        assert!(h.drew("END_VAR"), "the code written so far is drawn");
        assert_eq!(
            h.app.sim.as_ref().unwrap().proj.files["main.st"],
            LAMP,
            "the file is untouched"
        );
        let (l, r) = (
            h.rect("left").right() - left.right(),
            right.left() - h.rect("right").left(),
        );
        assert!(
            (0.0..=8.0).contains(&l) && (0.0..=8.0).contains(&r),
            "the panes grew: {l} {r}"
        );
        // The rest: checked, written, the draft gone.
        chunk(
            &mut h,
            [
                ev("content_block_delta", serde_json::json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":format!("{tail}```\n\n{}", follows())}})),
                ev("content_block_stop", serde_json::json!({"type":"content_block_stop","index":1})),
                ev("message_delta", serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
                ev("message_stop", serde_json::json!({"type":"message_stop"})),
            ]
            .concat(),
        );
        h.app.sim.as_mut().unwrap().link.inbox.push(Incoming::End { seq });
        h.wait(0.3);
        assert_eq!(
            h.app.sim.as_ref().unwrap().proj.files["main.st"],
            new,
            "written once checked"
        );
        assert!(!h.has("agent draft") && !h.has("st draft") && !h.has("agent working"));
    }

    /// Stopped while it writes: the draft goes and the file is as it was.
    #[test]
    fn a_draft_stopped_half_written_leaves_the_file_as_it_was() {
        use crate::claude::Incoming;
        use crate::instruct_tests::LAMP;
        let ev = |name: &str, data: serde_json::Value| format!("event: {name}\ndata: {data}\n\n");
        let mut h = Harness::new(App::sim());
        h.app.chrome.dock(Pane::Code, true);
        {
            let v = h.app.sim.as_mut().unwrap();
            v.chat.receive(Incoming::Proxy(true));
            v.proj.create("mine", None);
            v.proj.files.insert("main.st".into(), LAMP.into());
            v.proj.save();
            v.open_project("mine");
        }
        h.wait(0.3);
        h.click(h.rect("agent instruction").center());
        h.frame(vec![Event::Text("rewrite it".into())]);
        h.press("agent send");
        h.frame(vec![]);
        let seq = h.app.sim.as_ref().unwrap().link.sent.last().unwrap().0;
        h.app.sim.as_mut().unwrap().link.inbox.push(Incoming::Chunk {
            seq,
            text: [
                ev("content_block_start", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
                ev("content_block_delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Why: x.\n\n```st\nPROGRAM gone\nVAR\n"}})),
            ]
            .concat(),
        });
        h.wait(0.3);
        assert!(h.has("st draft") && h.drew("PROGRAM gone"));
        h.press("agent stop");
        h.wait(0.3);
        assert!(!h.has("st draft") && !h.has("agent draft"));
        assert_eq!(h.app.sim.as_ref().unwrap().proj.files["main.st"], LAMP);
        assert!(!h.drew("PROGRAM gone"), "nothing of the draft is left on screen");
    }

    #[test]
    fn the_agent_tab_is_a_tall_box_and_before_and_after_swap_at_once() {
        use crate::agent_marks::Side;
        use crate::instruct_tests::{LAMP, follows};
        let mut h = Harness::new(App::sim());
        // Docked right: the Agent at the right pane's default width, its narrowest place.
        h.app.chrome.dock(crate::editor::Pane::Claude, true);
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
        for gone in [
            "page prev",
            "use code",
            "agent new",
            "agent result",
            "agent undo",
            "agent before",
        ] {
            assert!(!h.has(gone), "{gone}");
        }
        assert!(h.has("agent instruction") && h.has("agent send"));
        let field = h.rect("agent instruction");
        assert!(
            field.height() > 0.5 * h.rect("right").height(),
            "the box takes the tab's height: {field:?} in {:?}",
            h.rect("right")
        );
        h.click(field.center());
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
        {
            let v = h.app.sim.as_ref().unwrap();
            assert_eq!(v.proj.files["main.st"], new);
            assert!(v.session.running, "the change runs at once");
            let logged = v
                .log
                .iter()
                .map(|e| e.text.clone())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(logged.contains("Why: it says why the lamp is on."), "{logged}");
        }
        assert!(
            !h.has("agent result"),
            "a change that landed is not described in the tab"
        );
        assert!(h.has("agent before") && h.has("agent after"));
        assert!(
            h.app.chrome.st_marks.as_ref().is_some_and(|m| m.applies(&new)),
            "After is marked"
        );
        // Before: the old file, running, marked against the new.
        h.press("agent before");
        h.wait(0.2);
        {
            let v = h.app.sim.as_ref().unwrap();
            assert_eq!(v.proj.files["main.st"], LAMP);
            assert_eq!(v.agent_change.as_ref().map(|c| c.side), Some(Side::Before));
            assert!(v.session.running);
        }
        assert!(
            h.app.chrome.st_marks.as_ref().is_some_and(|m| m.applies(LAMP)),
            "Before is marked"
        );
        // And back, as often as wanted.
        h.press("agent after");
        h.wait(0.2);
        assert_eq!(h.app.sim.as_ref().unwrap().proj.files["main.st"], new);
        h.press("agent before");
        h.wait(0.2);
        assert_eq!(h.app.sim.as_ref().unwrap().proj.files["main.st"], LAMP);
        h.press("agent after");
        h.wait(0.2);
        // An edit ends the swap: the file is the engineer's again.
        h.app
            .sim
            .as_mut()
            .unwrap()
            .proj
            .files
            .insert("main.st".into(), format!("{new}\n"));
        h.wait(0.2);
        assert!(!h.has("agent before"), "an edit ends Before / After");
    }

    /// A refused answer: an error in the Messages (red count, opened), the
    /// box outlined, and one line in the tab that opens the Messages.
    #[test]
    fn a_refused_answer_is_an_error_in_the_messages_and_one_red_line() {
        use crate::instruct_tests::LAMP;
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
        h.click(h.rect("agent instruction").center());
        h.frame(vec![Event::Text("break it".into())]);
        h.press("agent send");
        h.frame(vec![]);
        let broken = "Why: no.\n\n```st\nPROGRAM lamp\nlamp := ;\nEND_PROGRAM\n```\n";
        for _ in 0..3 {
            crate::instruct_tests::reply(h.app.sim.as_mut().unwrap(), broken);
            h.frame(vec![]);
        }
        h.wait(0.2);
        let v = h.app.sim.as_ref().unwrap();
        assert_eq!(v.proj.files["main.st"], LAMP, "nothing written");
        assert!(v.log.errors() >= 1, "an error, so the Messages count it red");
        assert!(h.has("agent result"));
        h.app.chrome.trend = false;
        h.press("agent result");
        assert!(h.app.chrome.trend, "the line opens the Messages");
        assert_eq!(h.app.chrome.bottom, crate::editor::Bottom::Messages);
    }

    /// Shift+Enter goes on with a list, and ends it on an empty item.
    #[test]
    fn a_list_goes_on_and_ends_on_an_empty_item() {
        use crate::instruct::continue_list;
        assert_eq!(continue_list("- latch the lamp"), ("\n- ".into(), 0));
        assert_eq!(continue_list("  * start wins"), ("\n  * ".into(), 0));
        assert_eq!(continue_list("3. then stop"), ("\n4. ".into(), 0));
        assert_eq!(continue_list("9) last"), ("\n10) ".into(), 0));
        assert_eq!(continue_list("plain words"), ("\n".into(), 0));
        assert_eq!(continue_list("    indented"), ("\n    ".into(), 0));
        assert_eq!(
            continue_list("- "),
            (String::new(), 2),
            "an empty item ends the list"
        );
        assert_eq!(continue_list("  12. "), (String::new(), 6));
        assert_eq!(
            continue_list("2024 was a year"),
            ("\n".into(), 0),
            "a number is not a list"
        );
    }

    /// Nick, 2026-11-08: Enter in the chat box sends; Shift+Enter and
    /// Ctrl+Enter add a line, and the Ctrl+Enter Run/Stop hotkey does not
    /// fire while someone types in the box.
    #[test]
    fn enter_sends_the_instruction_and_shift_or_ctrl_enter_adds_a_line() {
        use crate::instruct_tests::LAMP;
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
        let running = h.app.sim.as_ref().unwrap().session.running;
        h.click(h.rect("agent instruction").center());
        h.frame(vec![Event::Text("latch the lamp".into())]);
        h.key(Modifiers::SHIFT, Key::Enter);
        let v = h.app.sim.as_ref().unwrap();
        assert_eq!(v.instruct.draft, "latch the lamp\n", "Shift+Enter adds a line");
        assert!(!h.has("agent stop"), "and sends nothing");
        h.key(Modifiers::COMMAND, Key::Enter);
        let v = h.app.sim.as_ref().unwrap();
        assert_eq!(v.instruct.draft, "latch the lamp\n\n", "Ctrl+Enter adds a line");
        assert!(!h.has("agent stop"), "and sends nothing");
        assert_eq!(
            v.session.running, running,
            "Ctrl+Enter in the box does not Run or Stop"
        );
        h.frame(vec![Event::Text("x".into())]);
        h.key(Modifiers::NONE, Key::Enter);
        h.frame(vec![]);
        assert!(h.has("agent stop") && h.has("agent working"), "Enter sends");
        assert_eq!(h.app.sim.as_ref().unwrap().session.running, running);
    }

    /// Every outcome the tab can show, with long words, and the tab while
    /// the Agent fixes an answer: the panels keep their width throughout.
    #[test]
    fn the_agent_tabs_outcomes_leave_the_panels_their_width() {
        use crate::instruct::{Attempt, Kind, Outcome, Run, Undo};
        let mut h = Harness::new(App::sim());
        // Docked right: the Agent at the right pane's default width, its narrowest place.
        h.app.chrome.dock(crate::editor::Pane::Claude, true);
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
                "agent undo",
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
        let open = h.app.sim.as_ref().unwrap().proj.open.clone();
        h.app.sim.as_mut().unwrap().instruct.run = Some(Run {
            instruction: word.clone(),
            project: open,
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
        assert_eq!(crate::editor::Tab::Claude.label(), "Agent");
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
    use crate::editor::{Chrome, Pane, STRIP};
    use crate::projects::Act;

    #[test]
    fn the_header_is_on_every_screen() {
        let h = Harness::blink();
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
        // Docked right: a tab on each side, so both panes are measured.
        h.app.chrome.dock(crate::editor::Pane::Io, true);
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
        // Remembered per browser: what the app saves reads back the same.
        let back = Chrome::restored(&h.app.chrome.saved());
        assert_eq!((back.left, back.right, back.pane), (false, false, Pane::Claude));
    }

    /// Nick, 2026-11-11: a folded pane keeps its tabs as glyphs down the
    /// strip, and a click on one opens the pane on that tab.
    #[test]
    fn a_folded_panes_strip_keeps_its_tabs_and_each_opens_the_pane_on_it() {
        let mut h = Harness::new(App::sim());
        // Docked right: a tab on each side, so both panes are measured.
        h.app.chrome.dock(crate::editor::Pane::Io, true);
        h.press("left");
        h.press("right");
        h.wait(0.5);
        assert!(!h.app.chrome.left && !h.app.chrome.right);
        for key in ["pane files", "pane devices", "pane code", "tab io", "tab claude"] {
            let r = h.rect(key);
            let edge = if key == "tab io" { h.size.x } else { 0.0 };
            assert!(
                (r.center().x - edge).abs() <= STRIP,
                "{key} is on its strip: {r:?}"
            );
            assert!(
                r.width() >= 24.0 && r.height() >= 24.0,
                "{key} is a target: {r:?}"
            );
        }
        h.press("pane devices");
        h.wait(0.5);
        assert!(h.app.chrome.left, "the pane opens");
        assert_eq!(h.app.chrome.pane, Pane::Devices, "on the tab clicked");
        h.press("tab io");
        h.wait(0.5);
        assert!(h.app.chrome.right, "the right pane opens");
        assert!(h.app.chrome.showing(Pane::Io));
        // The chevron under them still opens a pane where it was.
        h.press("left");
        h.wait(0.5);
        h.press("left");
        h.wait(0.5);
        assert!(h.app.chrome.left);
        assert_eq!(h.app.chrome.pane, Pane::Devices);
    }

    /// Nick, 2026-11-11: every block that folds has an arrow in the Code
    /// tab's gutter. A click folds the block to its first and last lines
    /// (the lines under it move up, and a mark says how many are hidden),
    /// a second click opens it, and a line inside a folded block that has
    /// to be seen opens it by itself.
    #[test]
    fn a_block_folds_from_its_gutter_arrow_and_opens_for_a_line_that_must_show() {
        let mut h = Harness::blink();
        h.app.chrome.show_code();
        h.wait(0.3);
        // Blink: PROGRAM on 1, VAR_OUTPUT on 2, VAR on 5, the IF on 10.
        for line in [1, 2, 5, 10] {
            let r = h.rect(&format!("fold {line}"));
            assert!(
                r.right() <= h.rect("st editor").left(),
                "arrow {line} is in the gutter: {r:?}"
            );
        }
        assert!(!h.has("fold 9"), "a call on one line has nothing to fold");
        let (var, the_if) = (h.rect("fold 5").top(), h.rect("fold 10").top());
        // Lines 1 and 2 are a row each (a long line lower down wraps).
        let row = h.rect("fold 2").top() - h.rect("fold 1").top();
        h.press("fold 2");
        h.wait(0.2);
        assert!(h.app.chrome.folds.folded(2));
        assert!(h.has("folded 2"), "the mark that says how many lines are hidden");
        assert!(h.drew("… 1 line"));
        // VAR_OUTPUT hid one line: everything under it is one row up.
        assert!(
            (h.rect("fold 5").top() - (var - row)).abs() < 1.0,
            "{var} less {row}"
        );
        assert!((h.rect("fold 10").top() - (the_if - row)).abs() < 1.0);
        // Folding the program hides the arrows of what is inside it.
        h.press("fold 1");
        h.wait(0.2);
        assert!(!h.has("fold 5") && !h.has("fold 10") && !h.has("folded 2"));
        assert!(h.has("folded 1"));
        h.press("fold 1");
        h.wait(0.2);
        assert!(
            h.has("fold 5") && h.has("folded 2"),
            "open again, the inner fold kept"
        );
        h.press("fold 2");
        h.wait(0.2);
        assert!((h.rect("fold 5").top() - var).abs() < 1.0, "as it was");
        // A line inside a folded block that has to be seen (a block click
        // sends the editor to line 11): the block opens.
        h.press("fold 10");
        h.wait(0.2);
        assert!(h.app.chrome.folds.folded(10));
        h.app.chrome.goto(11);
        h.wait(0.2);
        assert!(!h.app.chrome.folds.folded(10), "the line has to be seen");
        // Nothing of the text changed by folding and unfolding.
        assert!(
            h.app
                .sim
                .as_ref()
                .unwrap()
                .proj
                .files
                .values()
                .all(|t| t.contains("END_PROGRAM"))
        );
    }

    /// Nick, 2026-11-11: style hints are a third, lighter level. A hinted
    /// line is marked in the Code tab, says what it is on hover, and is
    /// told to the Messages as information once the typing has settled:
    /// never as an error, and it stops nothing.
    #[test]
    fn a_style_hint_is_marked_in_the_code_told_on_hover_and_said_in_the_messages() {
        use crate::st_hints::SETTLE;
        let mut h = Harness::new(App::sim());
        h.app.chrome.show_code();
        h.wait(0.3);
        assert!(!h.has("hint 3"), "the example has nothing to hint at");
        let errors = h.app.sim.as_ref().unwrap().log.errors();
        *h.app.sim.as_mut().unwrap().proj.current_text_mut().unwrap() =
            "PROGRAM p\nVAR\n  spare : INT;\nEND_VAR\nEND_PROGRAM\n".into();
        h.wait(0.3);
        let band = h.rect("hint 3");
        let editor = h.rect("st editor");
        assert!(editor.contains(band.center()), "{band:?} in {editor:?}");
        let said = |h: &Harness| h.app.sim.as_ref().unwrap().log.text();
        assert!(!said(&h).contains("hint(s)"), "not while the text is new");
        h.wait(SETTLE + 0.3);
        let log = said(&h);
        assert!(log.contains("1 hint(s) in the open file"), "{log}");
        assert!(log.contains("line 3  spare: declared but never used"), "{log}");
        assert!(
            log.lines().any(|l| l.contains("info") && l.contains("hint(s)")),
            "{log}"
        );
        assert_eq!(
            h.app.sim.as_ref().unwrap().log.errors(),
            errors,
            "a hint is no error"
        );
        // On hover the line says what the hint is.
        h.frame(vec![Event::PointerMoved(band.center())]);
        h.wait(1.5);
        assert!(h.drew("Hint: spare: declared but never used."));
        // Used, the hint goes.
        *h.app.sim.as_mut().unwrap().proj.current_text_mut().unwrap() =
            "PROGRAM p\nVAR\n  spare : INT;\nEND_VAR\nspare := spare + 1;\nEND_PROGRAM\n".into();
        h.wait(0.3);
        assert!(!h.has("hint 3"));
    }

    /// Nick, 2026-11-11: nothing is said in a line under the menus. What
    /// needs saying is in the Messages tab, which goes red on an error
    /// (pulsing at first), stays red while the error stands or is unread,
    /// and opens the bottom panel if it was shut.
    #[test]
    fn an_error_is_in_the_messages_tab_which_goes_red_and_opens_a_shut_panel() {
        use crate::editor::Bottom;
        use crate::messages::FLASH;
        let mut h = Harness::new(App::sim());
        h.wait(0.2);
        let top = h.app.chrome.diagram.top();
        let log = |h: &Harness| {
            let l = &h.app.sim.as_ref().unwrap().log;
            (l.alarm(h.time, false), l.flashing(h.time))
        };
        assert_eq!(log(&h), (None, false), "nothing wrong, nothing red");
        // The bottom panel shut: a fault opens it on the Messages.
        h.app.chrome.trend = false;
        h.wait(0.2);
        h.app.sim.as_mut().unwrap().session.fault = Some("division by zero at line 7".into());
        h.wait(0.2);
        h.wait(0.5);
        assert!(h.app.chrome.trend, "the panel opened");
        assert_eq!(h.app.chrome.bottom, Bottom::Messages);
        assert!(h.has("fault"), "the fault is in the tab");
        assert!(h.rect("fault").top() > h.rect("bottom messages").bottom());
        let now = h.app.chrome.diagram.top();
        assert!(
            (now - top).abs() < 0.5,
            "no line pushed the page down: {top} to {now}"
        );
        let (alarm, flashing) = log(&h);
        assert!(alarm.is_some() && flashing, "red and pulsing");
        // On the Trend tab the fault still stands: red, steady once the
        // pulse is over, and it never stops while the fault is there.
        h.press("bottom trend");
        h.wait(FLASH + 0.5);
        let l = &h.app.sim.as_ref().unwrap().log;
        assert!(!l.flashing(h.time), "the pulse ends");
        assert!(l.alarm(h.time, true).is_some(), "red while the fault stands");
        // A new error while looking at the Trend: unread, so red on its
        // own account until the Messages are looked at.
        let at = h.time;
        h.app.sim.as_mut().unwrap().session.fault = None;
        h.app.sim.as_mut().unwrap().log.error(at, "deploy refused");
        h.wait(FLASH + 0.5);
        assert!(log(&h).0.is_some(), "unread: red");
        assert_eq!(h.app.chrome.bottom, Bottom::Trend, "an open panel keeps its tab");
        h.press("bottom messages");
        h.wait(0.2);
        assert_eq!(log(&h), (None, false), "read, and nothing stands");
    }

    #[test]
    fn the_left_pane_has_files_and_code_tabs_and_one_width() {
        let mut h = Harness::blink();
        assert!(h.app.chrome.code_open(), "the Code tab shown");
        let code = h.rect("pane code");
        let files = h.rect("pane files");
        // Every tab in the one pane (Nick, 2026-11-13), the Agent first.
        let xs: Vec<f32> = Pane::ALL.iter().map(|p| h.rect(&p.hit()).left()).collect();
        assert!(
            xs.windows(2).all(|w| w[0] < w[1]),
            "Agent | Code | I/O | Devices | Files | History | Palette, left to right: {xs:?}"
        );
        let editor = h.ctx.read_response(h.editor_id()).expect("the editor").rect;
        h.press("pane files");
        h.wait(0.3);
        assert!(h.app.chrome.files_open());
        assert!(h.has("file blink.st"), "the files show");
        assert!(
            h.ctx.read_response(h.editor_id()).is_none(),
            "the editor is on the other tab"
        );
        let code_w = code.right() - files.left();
        let files_w = h.rect("pane code").right() - h.rect("pane files").left();
        assert!((code_w - files_w).abs() < 1.0, "one width for both tabs");
        h.press("pane code");
        h.wait(0.3);
        let again = h.ctx.read_response(h.editor_id()).expect("the editor").rect;
        assert!((again.width() - editor.width()).abs() < 1.0);
        // Remembered: the tab reads back.
        let back = Chrome::restored(&h.app.chrome.saved());
        assert_eq!(back.pane, Pane::Code);
        assert_eq!(
            Chrome::restored("st=1").pane,
            Pane::Claude,
            "a layout from before the one pane opens on the Agent"
        );
    }

    /// Nick, 2026-11-01: the toolbar's dropdowns and the right-click
    /// menus were three times wider than their text. A menu is as wide
    /// as its widest item, every item filling that width.
    #[test]
    fn menus_are_as_wide_as_their_widest_item() {
        let mut h = Harness::blink();
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
        let mut h = Harness::blink();
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
        let mut h = Harness::blink();
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
        h.app.chrome.advanced = true;
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
        // The right-click menu offers the import (under Advanced).
        h.app.chrome.advanced = true;
        h.right_click(h.rect("devices blank").center());
        assert!(h.has("menu import device"));
        h.press("menu import device");
        h.wait(0.2);
        assert!(
            !h.has("device form"),
            "the picker is the browser's; no form until a file comes back"
        );
    }

    /// Add by click (PROTOCOLS.md step 3, 2026-11-12): Scan lists what
    /// answers on the network by vendor and model (the simulation: its
    /// E1213, which the library knows); Add opens a form that asks one
    /// thing, the device's name, and lists the channels it will get; Add
    /// copies the profile into the project and writes the device line by
    /// identity and one `ch:` signal a channel. No protocol, address or
    /// point is asked. Once the map names it the row says so.
    #[test]
    fn scan_lists_the_network_and_a_known_device_is_added_by_a_click() {
        use crate::devices::Scan;
        let mut h = Harness::new(App::sim());
        h.app.sim.as_mut().unwrap().proj.create("mine", None);
        h.press("pane devices");
        h.wait(0.3);
        assert!(h.has("devices scan"), "the Scan button");
        assert!(!h.app.chrome.advanced, "Advanced is off until asked for");
        assert!(!h.has("network 192.168.127.254"));
        h.press("devices scan");
        h.wait(0.3);
        assert_eq!(h.app.sim.as_ref().unwrap().devices_ui.scan, Scan::Done);
        assert!(h.has("network 192.168.127.254"), "the simulated E1213 answered");
        let f = h.app.sim.as_ref().unwrap().devices_ui.found[0].clone();
        assert_eq!((f.vendor_id, f.product_code), (991, 515));
        assert_eq!(f.mapped, None, "a fresh project maps nothing");
        assert_eq!(
            f.library.as_ref().and_then(|l| l.profile_file.as_deref()),
            Some("moxa-iologik-e1213.device")
        );
        assert!(h.has("add network 192.168.127.254"), "Add, in sight");
        assert!(
            !h.has("use network 192.168.127.254"),
            "the typed form is Advanced's"
        );
        // The agent's context lists it too.
        let text = h.app.sim.as_ref().unwrap().devices_text();
        assert!(
            text.contains("On the network (ListIdentity):\n  E1213 192.168.127.254 vendor 991 product 515"),
            "{text}"
        );
        assert!(text.contains("not in the map"), "{text}");
        // Add: one field, the name, filled in from the model; the channels
        // listed once the (simulated) library answered.
        h.press("add network 192.168.127.254");
        h.wait(0.3);
        assert!(h.has("quick form"), "the quick form, not the typed one");
        assert!(!h.has("device form"));
        let q = h
            .app
            .sim
            .as_ref()
            .unwrap()
            .devices_ui
            .quick
            .clone()
            .expect("the quick form");
        assert_eq!(q.name, "iologik_e1213", "named by the library's model");
        assert_eq!(q.identity, "cip=991:515:0x00e12130");
        let (_, skim) = q.profile.as_ref().expect("answered").as_ref().expect("read");
        assert_eq!(skim.channels.len(), 40);
        h.press("quick ok");
        h.wait(0.3);
        assert!(!h.has("quick form"), "added: the form is gone");
        let sim = h.app.sim.as_ref().unwrap();
        assert!(
            sim.proj.files.contains_key("moxa-iologik-e1213.device"),
            "the profile joined the project: {:?}",
            sim.proj.files.keys().collect::<Vec<_>>()
        );
        let map = sim.proj.files["io.map"].clone();
        assert!(
            map.contains("device iologik_e1213 device:moxa-iologik-e1213 cip=991:515:0x00e12130\n"),
            "{map}"
        );
        assert!(
            map.contains("in  iologik_e1213_di_04 bool iologik_e1213 ch:DI-04\n"),
            "{map}"
        );
        assert!(
            map.contains("out iologik_e1213_do_00 bool iologik_e1213 ch:DO-00 safe=false\n"),
            "{map}"
        );
        assert!(
            map.contains("in  iologik_e1213_di_04_count int iologik_e1213 ch:DI-04-count\n"),
            "{map}"
        );
        assert_eq!(map.matches(" iologik_e1213 ch:").count(), 40, "{map}");
        assert!(
            !map.contains("enip-io") && !map.contains("modbus-tcp"),
            "no protocol: {map}"
        );
        assert_eq!(
            sim.devices_ui.found[0].mapped.as_deref(),
            Some("iologik_e1213"),
            "wired at once"
        );
        assert!(!h.has("add network 192.168.127.254"), "a wired device has no Add");
        // The listing shows it with its forty points.
        assert!(h.has("device iologik_e1213"));
        h.press("devices scan");
        h.wait(0.3);
        assert_eq!(
            h.app.sim.as_ref().unwrap().devices_ui.found[0].mapped.as_deref(),
            Some("iologik_e1213")
        );
    }

    /// Advanced (PROTOCOLS.md step 3): the typed New device form, Import
    /// device… and Use this device sit behind a tick box beside Scan, off
    /// by default and remembered per browser; on, "Use this device…" on
    /// a found device opens the form filled in by identity as before.
    #[test]
    fn advanced_hides_the_typed_form_and_is_remembered_and_use_this_device_still_works() {
        use crate::editor::Chrome;
        let mut h = Harness::new(App::sim());
        h.app.sim.as_mut().unwrap().proj.create("mine", None);
        h.press("pane devices");
        h.wait(0.3);
        // Off: the blank space's menu offers nothing to type.
        h.right_click(h.rect("devices blank").center());
        h.wait(0.2);
        assert!(!h.has("menu new device"), "New device… is Advanced's");
        assert!(!h.has("menu import device"));
        h.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        h.press("devices advanced");
        h.wait(0.2);
        assert!(h.app.chrome.advanced);
        assert!(Chrome::restored(&h.app.chrome.saved()).advanced, "remembered");
        assert!(!Chrome::restored("left=1").advanced, "off unless saved on");
        h.right_click(h.rect("devices blank").center());
        h.wait(0.2);
        assert!(h.has("menu new device") && h.has("menu import device"));
        h.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        h.press("devices scan");
        h.wait(0.3);
        assert!(
            h.has("use network 192.168.127.254"),
            "Use this device, beside Add"
        );
        assert!(h.has("add network 192.168.127.254"));
        // Use this device…: the typed form, filled in by identity.
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
        assert_eq!(d.name, "iologik_e1213", "named by the library's model");
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
        assert!(
            map.contains("out lamp bool iologik_e1213 do:0 safe=false"),
            "{map}"
        );
        assert!(!map.contains(" box do:0"), "moved, not doubled: {map}");
        assert!(
            map.contains(
                "device iologik_e1213 enip-io cip=991:515:0x00e12130 profile=moxa-e1213 rpi_ms=100 poll_ms=20\n"
            ),
            "{map}"
        );
        // Wired at once, with no second scan (it offered Use this device
        // again until a rescan, 2026-11-05).
        assert_eq!(
            h.app.sim.as_ref().unwrap().devices_ui.found[0].mapped.as_deref(),
            Some("iologik_e1213")
        );
        assert!(!h.has("use network 192.168.127.254"));
        h.press("devices scan");
        h.wait(0.3);
        assert_eq!(
            h.app.sim.as_ref().unwrap().devices_ui.found[0].mapped.as_deref(),
            Some("iologik_e1213")
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
        h.app.chrome.advanced = true;
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
            assert!(map.contains("in  start bool iologik_e1213 di:0\n"), "{map}");
            assert!(map.contains("in  door bool iologik_e1213 di:5\n"), "{map}");
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
        assert!(sim.proj.files["io.map"].contains("in  go_button bool iologik_e1213 di:0\n"));
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
        h.app.chrome.advanced = true;
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

    /// Nick, 2026-11-13: the open project shows its files under it, with
    /// no files section and no Program section below.
    #[test]
    fn the_files_tab_lists_the_open_projects_files_under_it() {
        let mut h = Harness::new(App::sim());
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.open_project("mine");
        }
        h.press("pane files");
        h.wait(0.3);
        let project = h.rect("project mine");
        let file = h.rect("file main.st");
        assert!(file.top() >= project.bottom() - 1.0, "under its project");
        assert!(file.left() > project.left() + 4.0, "indented under it");
        assert!(!h.has("history"), "no History row: it is a tab");
        let text = crate::ui_tests::all_text(&mut h);
        assert!(!text.contains("mine: files"), "no files heading");
        assert!(!text.lines().any(|l| l.trim() == "Program"), "no Program section");
    }

    /// Nick, 2026-11-13: History is a tab of its own, newest at the top,
    /// one line per change; Newer and Older (and a click) run that
    /// version marked against the one before; a double click opens its
    /// details with Restore.
    #[test]
    fn the_history_tab_steps_through_versions_and_runs_each() {
        let mut h = Harness::new(App::sim());
        let (one, two);
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", None);
            sim.open_project("mine");
            sim.proj.current_text_mut().unwrap().push_str("\n(* zero *)");
            sim.proj.save();
            sim.proj.current_text_mut().unwrap().push_str("\n(* one *)");
            sim.proj.save();
            one = sim.proj.files["main.st"].clone();
            sim.proj.current_text_mut().unwrap().push_str("\n(* two *)");
            sim.proj.save();
            two = sim.proj.files["main.st"].clone();
        }
        h.press("pane history");
        h.wait(0.3);
        let newest = h.app.sim.as_ref().unwrap().proj.revisions().last().unwrap().n;
        let older = newest - 1;
        assert!(h.has(&format!("revision {newest}")) && h.has(&format!("revision {older}")));
        assert!(
            h.rect(&format!("revision {newest}")).top() < h.rect(&format!("revision {older}")).top(),
            "newest first"
        );
        // Older: the version before, running, its change marked.
        h.press("history older");
        h.wait(0.3);
        {
            let sim = h.app.sim.as_ref().unwrap();
            assert_eq!(sim.proj.files["main.st"], one);
            assert_eq!(sim.history_at, Some(older));
            assert!(sim.session.running, "it runs");
        }
        {
            let sim = h.app.sim.as_ref().unwrap();
            let c = sim.agent_change.as_ref().expect("the change is marked");
            assert!(
                c.lines().applies(&one),
                "marked against the version before it: {:?}",
                c.lines()
            );
            assert!(
                c.lines()
                    .added
                    .iter()
                    .any(|&l| one.lines().nth(l as usize - 1) == Some("(* one *)"))
            );
        }
        // Newer: back to the newest, nothing left unsaved.
        h.press("history newer");
        h.wait(0.3);
        {
            let sim = h.app.sim.as_ref().unwrap();
            assert_eq!(sim.proj.files["main.st"], two);
            assert_eq!(sim.history_at, None);
            assert!(!sim.proj.dirty());
        }
        // A click runs a row; the keyboard steps too.
        h.press(&format!("revision {older}"));
        h.wait(0.3);
        assert_eq!(h.app.sim.as_ref().unwrap().proj.files["main.st"], one);
        h.frame(vec![eframe::egui::Event::Key {
            key: eframe::egui::Key::ArrowUp,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: eframe::egui::Modifiers::NONE,
        }]);
        h.wait(0.3);
        assert_eq!(
            h.app.sim.as_ref().unwrap().proj.files["main.st"],
            two,
            "Up steps newer"
        );
        // A double click: the details in the centre, with Restore.
        let at = h.rect(&format!("revision {older}")).center();
        h.click(at);
        h.click(at);
        h.wait(0.3);
        assert!(h.has("revision form") && h.has("revision restore") && h.has("revision close"));
        h.press("revision restore");
        h.wait(0.3);
        let sim = h.app.sim.as_ref().unwrap();
        assert_eq!(sim.proj.files["main.st"], one, "restored");
        assert_eq!(
            sim.proj.revisions().last().unwrap().what,
            format!("restored revision {older}")
        );
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
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Enter);
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
        h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Enter);
        h.wait(0.2);
        let sim = h.app.sim.as_ref().unwrap();
        let notice = sim.notice.clone().unwrap_or_default();
        assert!(notice.starts_with("mine is running in this page"), "{notice}");
    }

    #[test]
    fn double_clicking_a_file_opens_the_code_tab() {
        let mut h = Harness::blink();
        h.app.chrome.show_files();
        h.wait(0.2);
        let at = h.rect("file blink.st").center();
        h.click(at);
        assert!(h.app.chrome.files_open(), "one click stays on Files");
        h.click(at);
        h.frame(vec![]);
        assert!(h.app.chrome.code_open(), "two open the Code tab");
    }

    /// Nick 2026-11-07: a double click on any pane tab hides that pane, on
    /// either side; one click only picks the tab.
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
        // The same on the right, with two tabs docked there.
        h.app.chrome.dock(Pane::Io, true);
        h.app.chrome.dock(Pane::Claude, true);
        h.wait(0.2);
        let tabs: Vec<String> = h
            .app
            .chrome
            .hits
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| k == "tab io" || k == "tab claude")
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
        let mut h = Harness::blink();
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

    /// Nick, 2026-11-13: every tab in one pane on the left, the Agent
    /// shown; a tab dragged to the window's right edge docks in a pane
    /// there, and dragged back onto the left pane it returns.
    #[test]
    fn a_tab_dragged_to_the_right_edge_docks_there_and_back() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        assert_eq!(h.app.chrome.lefts, Pane::ALL, "every tab on the left");
        assert_eq!(h.app.chrome.pane, Pane::Claude, "the Agent shown");
        assert!(!h.has("right"), "no right pane drawn");
        let wide = h.app.chrome.diagram;
        // Dropped on the diagram: nothing moves.
        let io = h.rect("tab io").center();
        h.drag(io, wide.center());
        assert!(h.app.chrome.rights.is_empty(), "the diagram is no place to dock");
        // Dropped at the right edge: docked there, shown, the Agent kept.
        let io = h.rect("tab io").center();
        h.drag(io, Pos2::new(h.size.x - 20.0, 300.0));
        h.wait(0.3);
        assert_eq!(h.app.chrome.rights, [Pane::Io]);
        assert!(h.app.chrome.showing(Pane::Io) && h.app.chrome.showing(Pane::Claude));
        assert!(h.has("right") && h.has("io outputs"));
        assert!(
            h.app.chrome.diagram.width() < wide.width(),
            "the diagram makes room"
        );
        // Back onto the left pane.
        let io = h.rect("tab io").center();
        let left = h.rect("pane code").center() + egui::vec2(0.0, 200.0);
        h.drag(io, left);
        h.wait(0.3);
        assert!(h.app.chrome.rights.is_empty() && h.app.chrome.lefts.contains(&Pane::Io));
        assert!(h.app.chrome.showing(Pane::Io), "shown where it landed");
        assert!(!h.has("right"), "the empty right pane goes");
    }

    /// Docking keeps the shown tab unless it is the one moved: then the
    /// tab under it shows, or the one before when it was the last. The
    /// regression: docking I/O while the Agent showed switched the left
    /// pane to Devices.
    #[test]
    fn docking_a_tab_shows_the_one_under_it_only_when_it_was_shown() {
        let mut c = Chrome::default();
        c.dock(Pane::Io, true);
        assert_eq!((c.pane, c.side), (Pane::Claude, Pane::Io), "the Agent stays");
        c.pane = Pane::Code;
        c.send_right();
        assert_eq!(
            (c.pane, c.side),
            (Pane::Devices, Pane::Code),
            "the tab under Code"
        );
        assert_eq!(c.rights, [Pane::Io, Pane::Code]);
        c.pane = Pane::Palette;
        c.send_right();
        assert_eq!(c.pane, Pane::History, "the last one: the one before");
        c.dock(Pane::Io, false);
        assert_eq!(
            (c.pane, c.side),
            (Pane::Io, Pane::Palette),
            "back on the left, shown; the right keeps its tab"
        );
        assert_eq!(c.lefts.last(), Some(&Pane::Io), "at the end");
        // Everything to the right: the left pane has nothing, and still draws.
        while !c.lefts.is_empty() {
            c.pane = c.lefts[0];
            c.send_right();
        }
        assert_eq!(c.rights.len(), Pane::ALL.len());
        let mut h = Harness::new(App::sim());
        h.app.chrome = c;
        h.wait(0.3);
        assert!(!h.has("left") && h.has("right"), "only the right pane");
    }

    proptest::proptest! {
        /// Any run of docks and sends keeps every tab in exactly one pane,
        /// each pane showing one of its own.
        #[test]
        fn docking_keeps_every_tab_once(moves in proptest::collection::vec((0usize..7, proptest::bool::ANY, proptest::bool::ANY), 0..40)) {
            let mut c = Chrome::default();
            for (k, right, send) in moves {
                if send {
                    // Alt Shift 5 on whichever left tab shows.
                    if !c.lefts.is_empty() {
                        c.pane = c.lefts[k % c.lefts.len()];
                    }
                    c.send_right();
                } else {
                    c.dock(Pane::ALL[k], right);
                }
                let mut all: Vec<Pane> = c.lefts.iter().chain(&c.rights).copied().collect();
                all.sort_by_key(|p| Pane::ALL.iter().position(|q| q == p));
                proptest::prop_assert_eq!(&all[..], &Pane::ALL[..]);
                proptest::prop_assert!(c.lefts.is_empty() || c.lefts.contains(&c.pane));
                proptest::prop_assert!(c.rights.is_empty() || c.rights.contains(&c.side));
                proptest::prop_assert_eq!(Chrome::restored(&c.saved()).rights, c.rights.clone());
            }
        }
    }

    #[test]
    fn the_layout_read_back_from_storage_is_forgiving() {
        let mut c = Chrome::default();
        c.dock(Pane::Io, true);
        c.pane = Pane::Code;
        c.scan = true;
        c.trend = false;
        let back = Chrome::restored(&c.saved());
        assert_eq!(back, c, "{}", c.saved());
        assert_eq!(back.rights, [Pane::Io]);
        assert!(!back.lefts.contains(&Pane::Io));
        // Anything else keeps the defaults; the Why tab is gone.
        for junk in [
            "",
            "left",
            "tab=why",
            "=== =1",
            "dock=2 lefts=why,,code rights=code",
        ] {
            let c = Chrome::restored(junk);
            let mut all: Vec<Pane> = c.lefts.iter().chain(&c.rights).copied().collect();
            all.sort_by_key(|p| Pane::ALL.iter().position(|q| q == p));
            assert_eq!(all, Pane::ALL, "every tab once: {junk:?}");
        }
        for junk in ["", "left", "tab=why", "=== =1"] {
            assert_eq!(Chrome::restored(junk), Chrome::default(), "{junk:?}");
        }
        let c = Chrome::restored("dock=2 right=yes left=0 pane=code scan=yes \u{0}");
        assert_eq!(
            (c.left, c.right, c.pane, c.scan),
            (false, false, Pane::Code, false)
        );
        // A layout from before 2026-11-13 (two side panels): its panes
        // are not read back, the rest is.
        let old = Chrome::restored("left=1 right=1 pane=files tab=io scan=1 light=1");
        assert_eq!(
            (old.pane, old.lefts.clone(), old.rights.len()),
            (Pane::Claude, Pane::ALL.to_vec(), 0)
        );
        assert!(old.scan && old.light);
    }

    #[test]
    fn the_st_toggle_opens_and_closes_the_editor_and_a_block_click_sends_it_to_the_line() {
        let mut h = Harness::blink();
        let editor = h.editor_id();
        assert!(h.app.chrome.code_open(), "the Code tab shown");
        assert!(h.ctx.read_response(editor).is_some());
        h.view_menu("view st");
        assert!(!h.app.chrome.code_open());
        h.wait(0.5);
        assert!(h.ctx.read_response(editor).is_none(), "shut");
        h.view_menu("view st");
        assert!(h.app.chrome.code_open());
        h.wait(0.5);
        let r = h.ctx.read_response(editor).expect("open again");
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
        let state = egui::text_edit::TextEditState::load(&h.ctx, editor).expect("the editor's state");
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
    use crate::editor::Pane;
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
        let mut h = Harness::blink();
        let (at, _) = node(sim_view(&h), write_to("LAMP"));
        h.click(at);
        assert!(matches!(sim_view(&h).pick, Some(Pick::Node(_))));
        // The canvas's bottom right corner, left of the right panel and
        // with the Trend folded away from the bottom.
        h.app.chrome.trend = false;
        h.wait(0.5);
        let far = Pos2::new(h.app.chrome.diagram.right() - 30.0, h.size.y - 6.0);
        h.click(far);
        assert_eq!(sim_view(&h).pick, None);
    }

    #[test]
    fn a_tap_near_a_small_block_still_hits_it() {
        let mut h = Harness::blink();
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
        // Inside the diagram, left of its right edge.
        let centre = Pos2::new(h.app.chrome.diagram.right() - 150.0, 450.0);
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
        h.key(Modifiers::ALT, Key::CloseBracket);
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
        // Nothing picked inside: Alt ] does nothing.
        h.key(Modifiers::ALT, Key::CloseBracket);
        assert_eq!(scans(&h), before + 1);
        h.key(Modifiers::ALT, Key::OpenBracket);
        let v = sim_view(&h);
        assert_eq!(v.scope, 0, "back on the program");
        assert_eq!(v.pick, Some(Pick::Node(n)), "the instance left is picked");
        assert_eq!(scans(&h), before + 2);
        // At the top, Alt [ does nothing; running, the keys only move.
        h.key(Modifiers::ALT, Key::OpenBracket);
        assert_eq!(scans(&h), before + 2);
        h.app.sim.as_mut().unwrap().session.set_running(true);
        h.key(Modifiers::ALT, Key::CloseBracket);
        assert_eq!(sim_view(&h).diagram().pou, "STATION");
    }

    /// Nick, 2026-11-01: the Trend runs along the bottom, where its time
    /// axis is long, not in the side panel's tabs.
    #[test]
    fn the_trend_runs_along_the_bottom_and_the_tabs_are_io_and_agent() {
        let mut h = Harness::new(App::sim());
        assert_eq!(h.app.chrome.pane, Pane::Claude, "the Agent first");
        h.wait(0.3);
        let panel = h.rect("left");
        for (key, tab) in [("tab io", Pane::Io), ("tab claude", Pane::Claude)] {
            h.press(key);
            assert_eq!(h.app.chrome.pane, tab);
        }
        h.wait(0.3);
        // The Agent tab's content fits the pane: at most the scroll bar's
        // width more, never the rows that used to widen it by a third.
        let grown = h.rect("left").right() - panel.right();
        assert!((0.0..=8.0).contains(&grown), "the panel grew {grown} pt");
        assert!(!h.has("tab trend"), "no Trend tab");
        assert!(h.app.chrome.trend, "open by default");
        let s = &mut h.app.sim.as_mut().unwrap().session;
        s.trend.toggle("LAMP");
        h.wait(0.2);
        let s = &h.app.sim.as_ref().unwrap().session;
        assert!(!s.trend.history.is_empty(), "it records while shown");
        h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::Num2);
        assert!(!h.app.chrome.trend, "Alt+Shift+2 folds it");
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

    /// Nick, 2026-11-11: a trend signal's colour is the user's to pick. A
    /// click on a lane's label opens the palette; the swatch clicked is
    /// that signal's colour whichever lane it is in, and the browser
    /// keeps it.
    #[test]
    fn a_click_on_a_trend_label_picks_that_signals_colour() {
        use crate::editor::Chrome;
        use crate::playground_view::{lane_color, palette_color};
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let s = &mut h.app.sim.as_mut().unwrap().session;
        s.trend.signals = vec!["FIRST".into(), "SECOND".into()];
        h.wait(0.3);
        let t = crate::theme::Theme::dark();
        let color = |h: &Harness, name: &str, k: usize| lane_color(&t, &h.app.chrome, name, k);
        assert_eq!(color(&h, "FIRST", 0), palette_color(&t, 0), "its lane's to start");
        assert!(!h.has("trend color 3"), "no palette until asked");
        h.press("trend lane first");
        h.wait(0.2);
        h.press("trend color 3");
        h.wait(0.2);
        assert_eq!(h.app.chrome.color("first"), Some(3));
        assert_eq!(color(&h, "FIRST", 0), palette_color(&t, 3));
        assert_eq!(color(&h, "FIRST", 1), palette_color(&t, 3), "in any lane");
        assert_eq!(
            color(&h, "SECOND", 1),
            palette_color(&t, 1),
            "the others keep theirs"
        );
        assert!(!h.has("trend color 3"), "the palette shuts on a pick");
        // The order is as it was: a click is not a drag.
        let order = h.app.sim.as_ref().unwrap().session.trend.signals.clone();
        assert_eq!(order, ["FIRST", "SECOND"]);
        // Kept per browser, and what is read back is bounded and clean.
        let back = Chrome::restored(&h.app.chrome.saved());
        assert_eq!(back.color("FIRST"), Some(3));
        let hostile = format!(
            "colors=a:1,:2,b,c:x,d:99999999999999999999,{}",
            (0..500).map(|k| format!("s{k}:1")).collect::<Vec<_>>().join(",")
        );
        let back = Chrome::restored(&hostile);
        assert_eq!(back.color("a"), Some(1));
        assert!(back.colors.len() <= crate::editor::MAX_COLORS);
        assert_eq!(Chrome::restored(&back.saved()).colors, back.colors);
        let mut c = Chrome::default();
        c.set_color("bad name", 1);
        c.set_color("x=y", 1);
        c.set_color("", 1);
        assert!(
            c.colors.is_empty(),
            "a name the saved text cannot hold is not kept"
        );
        // Any place in the palette draws: a stored index past its end wraps.
        c.set_color("lamp", 1_000_003);
        let _ = lane_color(&t, &c, "lamp", 0);
    }

    /// Nick, 2026-11-11: the Code tab keeps its code face; the Files and
    /// Devices tabs are in the face the rest of the page is in.
    #[test]
    fn only_the_code_tab_of_the_left_pane_is_in_the_code_face() {
        use crate::editor::Pane;
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let edge = h.rect("left").right();
        for pane in [Pane::Files, Pane::Devices] {
            h.app.chrome.pane = pane;
            h.wait(0.3);
            assert_eq!(h.mono_left_of(edge), Vec::<String>::new(), "{pane:?}");
        }
        h.app.chrome.pane = Pane::Code;
        h.wait(0.3);
        assert!(!h.mono_left_of(edge).is_empty(), "the code is in the code face");
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
        let changes = |h: &Harness| h.app.sim.as_ref().unwrap().session.trend.history.changes("RED");
        let kept = changes(&h);
        h.click(Pos2::new(slider.right() - 1.0, slider.center().y));
        assert_eq!(window(&h), WINDOW_MAX_MS, "the right end is 10 min");
        assert!(changes(&h) >= kept, "narrowing dropped nothing");
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
        // Docked right: the Agent at the right pane's default width, its narrowest place.
        h.app.chrome.dock(crate::editor::Pane::Claude, true);
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
        let id = h.editor_id();
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
        let mut h = Harness::blink();
        h.wait(0.3);
        h.press("pane code");
        h.wait(0.3);
        let left = h.rect("left");
        h.key(Modifiers::COMMAND, Key::F);
        assert!(h.app.chrome.code_open());
        assert!(h.app.chrome.find.is_some(), "the find box opens");
        let editor = h.editor_id();
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
        // Docked right: a tab on each side, so both panes are measured.
        h.app.chrome.dock(crate::editor::Pane::Io, true);
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

    /// A warning row under the editor wraps at the pane's width, and the
    /// panels stay where they were (the default width, 236 pt on the right).
    #[test]
    fn a_long_warning_row_wraps_and_leaves_the_panels_their_width() {
        let mut h = Harness::new(App::sim());
        // Docked right: a tab on each side, so both panes are measured.
        h.app.chrome.dock(crate::editor::Pane::Io, true);
        h.press("pane code");
        h.wait(0.3);
        let (left, right) = (h.rect("left"), h.rect("right"));
        let long = "x".repeat(300);
        let sim = h.app.sim.as_mut().unwrap();
        sim.session.warnings = vec![helmsman_iec::Diagnostic {
            pos: helmsman_iec::Pos { line: 1, col: 1 },
            msg: format!("kept: RETAIN inside a function block is not kept across a restart yet {long}"),
        }];
        h.wait(0.3);
        assert!(h.has("diag 0"), "the warning is listed");
        let l = h.rect("left").right() - left.right();
        let r = right.left() - h.rect("right").left();
        assert!((0.0..=8.0).contains(&l), "the left pane grew {l} pt");
        assert!((0.0..=8.0).contains(&r), "the right panel grew {r} pt");
    }

    /// A page built with the free chat: no key field and no model picker,
    /// one notice under the Agent's box. Closed or out of reach, the tab
    /// says so in place of the box and asks its one question. The right
    /// panel keeps its default width through every one of these states.
    #[test]
    fn the_free_chat_tab_has_no_key_and_keeps_the_panel_width() {
        use crate::claude::{Free, Incoming};
        let mut h = Harness::new(App::sim());
        // Docked right: the Agent at the right pane's default width, its narrowest place.
        h.app.chrome.dock(crate::editor::Pane::Claude, true);
        h.wait(0.3);
        let right = h.rect("right");
        h.press("tab claude");
        h.wait(0.3);
        assert!(h.has("agent key"), "with no free chat the page asks for a key");
        let grew = |h: &Harness, what: &str| {
            let r = right.left() - h.rect("right").left();
            assert!((0.0..=8.0).contains(&r), "{what}: the right panel grew {r} pt");
        };
        h.app
            .sim
            .as_mut()
            .unwrap()
            .chat
            .set_free_base("https://relay.example");
        h.wait(0.3);
        assert!(!h.has("agent key") && h.has("agent instruction") && h.has("agent free notice"));
        grew(&h, "checking");
        assert_eq!(
            h.app.sim.as_ref().unwrap().link.free_checks,
            ["https://relay.example"],
            "the page asks the free chat once whether it is open"
        );
        h.app
            .sim
            .as_mut()
            .unwrap()
            .chat
            .receive(Incoming::Free(Free::Open));
        h.wait(0.3);
        assert!(!h.has("agent key") && h.has("agent instruction") && h.has("agent free notice"));
        assert!(!h.has("agent free closed") && !h.has("survey google"));
        grew(&h, "open");
        for state in [
            Free::Closed { reopens: false },
            Free::Unreachable,
            Free::Closed { reopens: true },
        ] {
            h.app.sim.as_mut().unwrap().chat.receive(Incoming::Free(state));
            h.wait(0.3);
            assert!(h.has("agent free closed"), "{state:?}");
            assert!(
                !h.has("agent instruction") && !h.has("agent key") && !h.has("agent free notice"),
                "{state:?}"
            );
            assert_eq!(
                h.has("survey google") && h.has("survey other field"),
                state != Free::Unreachable,
                "{state:?}: the question needs the relay"
            );
            grew(&h, &format!("{state:?}"));
        }
        // One click answers it, once.
        h.press("survey own-hardware");
        h.wait(0.3);
        let sim = h.app.sim.as_ref().unwrap();
        assert!(sim.chat.surveyed);
        assert_eq!(sim.link.posted.len(), 1);
        assert_eq!(sim.link.posted[0].url, "https://relay.example/demo/v1/survey");
        assert_eq!(sim.link.posted[0].body, r#"{"choice":"own-hardware"}"#);
        assert!(h.has("agent free closed") && !h.has("survey google"));
        grew(&h, "answered");
    }

    #[test]
    fn every_tab_leaves_the_panels_their_width() {
        let mut h = Harness::new(App::sim());
        // Docked right: a tab on each side, so both panes are measured.
        h.app.chrome.dock(crate::editor::Pane::Io, true);
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
    use crate::editor::Pane;
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

    /// The box is read whichever screen shows: on Proposals nothing else
    /// draws it, and its streams carry on, so an unread backlog of values
    /// events (a JSON string each) would pile up and be parsed in one frame.
    #[test]
    fn the_box_is_kept_up_on_the_proposals_screen() {
        use crate::app::Screen;
        use crate::remote::{Cmd, Fetch};
        let mut h = Harness::new(App::boxed());
        h.wait(0.2);
        let b = h.app.boxv.as_mut().unwrap();
        b.sent.clear();
        // Something the box view has to carry out when it is ticked.
        b.state.relist();
        h.app.screen = Screen::Proposals;
        h.wait(0.2);
        let sent = &h.app.boxv.as_ref().unwrap().sent;
        assert!(
            sent.contains(&Cmd::Get(Fetch::Modules, "/api/modules".into())),
            "{sent:?}"
        );
    }

    /// Simulating in the browser leaves the box with nobody to read its
    /// streams: they are closed, and open again when the page goes back.
    #[test]
    fn simulating_in_the_browser_closes_the_boxs_streams_and_going_back_opens_them() {
        use crate::remote::{Cmd, Stream};
        let mut h = Harness::new(App::boxed());
        h.wait(0.2);
        h.app.boxv.as_mut().unwrap().sent.clear();
        h.app.source = Source::Sim;
        h.wait(12.0);
        let sent = std::mem::take(&mut h.app.boxv.as_mut().unwrap().sent);
        for s in [Stream::Values, Stream::Trace, Stream::Traces] {
            assert!(sent.contains(&Cmd::Close(s)), "{s:?} closed: {sent:?}");
        }
        assert!(
            sent.iter().all(|c| matches!(c, Cmd::Close(_))),
            "nothing is asked of a box nobody watches: {sent:?}"
        );
        h.app.source = Source::Box;
        h.wait(0.2);
        let sent = std::mem::take(&mut h.app.boxv.as_mut().unwrap().sent);
        assert!(
            sent.contains(&Cmd::Listen(Stream::Values, "/api/events".into())),
            "{sent:?}"
        );
    }

    /// The Editor's Run and Stop act on what the Monitor's do (the program
    /// shown), not on every module: a module left stopped on purpose is not
    /// started by a click meant for another, and a second click before the
    /// box's list shows the first one's effect does not send again.
    #[test]
    fn the_editors_run_glyph_acts_on_the_program_shown_and_only_once() {
        use crate::remote::ModuleRow;
        use crate::review::Session;
        let row = |name: &str, state: &str| ModuleRow {
            name: name.into(),
            state: state.into(),
            fault: None,
            stopped_by: None,
            scans: 0,
            scan_ms: Some(10.0),
            overruns: 0,
            overrun_ago: None,
            timing: None,
        };
        let mut h = Harness::new(App::boxed());
        h.app
            .review
            .get_or_insert_with(crate::review_view::ReviewView::new)
            .state
            .session = Some(Session {
            user: Some("alice".into()),
            role: "engineer".into(),
        });
        h.app.boxv.as_mut().unwrap().state.modules = vec![row("a", "running"), row("b", "stopped")];
        // The program shown is b, which an engineer left stopped.
        h.app.monitor.at = Some(("b".into(), String::new()));
        h.wait(0.2);
        h.app.boxv.as_mut().unwrap().psent.clear();
        h.press("run");
        let urls = |h: &mut Harness| -> Vec<String> {
            h.wait(0.1);
            std::mem::take(&mut h.app.boxv.as_mut().unwrap().psent)
                .into_iter()
                .filter(|q| q.method == "POST")
                .map(|q| q.url)
                .collect()
        };
        assert_eq!(
            urls(&mut h),
            ["/api/modules/b/run"],
            "b alone, and run, not a's stop"
        );
        // The box's list lags: b still reads stopped.
        h.press("run");
        assert_eq!(
            urls(&mut h),
            Vec::<String>::new(),
            "the first click is still on its way"
        );
        // Once the list shows it, the next click decides afresh.
        h.app.boxv.as_mut().unwrap().state.modules = vec![row("a", "running"), row("b", "running")];
        h.wait(0.2);
        h.press("run");
        assert_eq!(urls(&mut h), ["/api/modules/b/stop"]);
    }

    /// Card build-match: a box that answers as another build gets the
    /// banner, on the Editor and the Monitor alike, and the panels keep
    /// their width under it.
    #[test]
    fn a_box_of_another_build_shows_the_banner() {
        let mut h = Harness::new(App::boxed());
        // Docked right: a tab on each side, so both panes are measured.
        h.app.chrome.dock(crate::editor::Pane::Io, true);
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
                runs: None,
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
        // Docked right: the Agent at the right pane's default width, its narrowest place.
        h.app.chrome.dock(crate::editor::Pane::Claude, true);
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
        // Enter asks; Shift+Enter or Ctrl+Enter adds a line (Nick, 2026-11-08).
        h.click(h.rect("help question").center());
        h.key(eframe::egui::Modifiers::SHIFT, eframe::egui::Key::Enter);
        h.key(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Enter);
        assert_eq!(boxed(&h).help.draft, "The conveyor stopped.\n\n");
        assert!(boxed(&h).help_sent.is_empty(), "a new line asks nothing");
        h.app.boxv.as_mut().unwrap().help.draft = "The conveyor stopped.".into();
        h.frame(vec![]);
        h.key(eframe::egui::Modifiers::NONE, eframe::egui::Key::Enter);
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
                transport: None,
                health: None,
            },
            DeviceRow {
                name: "rio".into(),
                state: "down".into(),
                error: Some("connection refused".into()),
                address: None,
                transport: None,
                health: None,
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
        h.app.chrome.advanced = true;
        h.right_click(h.rect("devices blank").center());
        assert!(h.has("menu new device"));
        h.press("menu new device");
        h.wait(0.2);
        assert!(
            !h.has("device form"),
            "no project open, nobody signed in: nothing to add to"
        );
    }

    /// Card cip-adapter-visible: the Devices tab names the EtherNet/IP
    /// adapter's address from the real `GET /api/adapter`, in one wrapped
    /// line that leaves the panel at its default width; a box with the
    /// adapter off, or an older one without the route, shows nothing.
    #[test]
    fn the_devices_tab_names_the_adapter_in_a_line_that_wraps_at_the_default_width() {
        use crate::remote::{Fetch, Msg};
        let bound = "192.168.100.200:44818".parse().unwrap();
        let on = Bench::with_adapter(Some(bound));
        let (mut state, _) = watch(&on);
        let (status, body) = on.get("/api/adapter");
        assert_eq!(status, 200, "{body}");
        state.apply(Msg::Got(Fetch::Adapter, Ok(body)), 1.0);
        state.io_map = Some("in ready bool box di:0\n".into());
        let mut h = Harness::new(App::boxed());
        // Docked right: a tab on each side, so both panes are measured.
        h.app.chrome.dock(crate::editor::Pane::Io, true);
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.frame(vec![]);
        let left = h.rect("left");
        let right = h.rect("right");
        h.press("pane devices");
        h.wait(0.3);
        assert!(h.app.chrome.devices_open());
        let line = h.rect("devices adapter");
        let pane = h.rect("left");
        assert!(
            line.right() <= crate::editor::LEFT_W,
            "the line {line:?} stays inside the pane's {} pt default width",
            crate::editor::LEFT_W
        );
        assert!(line.height() > 30.0, "the long line wraps: {line:?}");
        assert!(
            (0.0..=8.0).contains(&(pane.right() - left.right())),
            "the left pane grew {} pt",
            pane.right() - left.right()
        );
        assert!(
            (0.0..=8.0).contains(&(right.left() - h.rect("right").left())),
            "the right panel grew"
        );
        // Off: the box answers, and says nothing on the tab.
        let off = Bench::with_adapter(None);
        let (status, body) = off.get("/api/adapter");
        assert_eq!(status, 200, "{body}");
        let mut h = Harness::new(App::boxed());
        let mut state = watch(&off).0;
        state.apply(Msg::Got(Fetch::Adapter, Ok(body)), 1.0);
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.press("pane devices");
        h.wait(0.3);
        assert!(!h.has("devices adapter"), "off: no line");
        // An older box answers 404: no line.
        let mut h = Harness::new(App::boxed());
        let mut state = watch(&off).0;
        state.apply(Msg::Got(Fetch::Adapter, Err("HTTP 404".into())), 1.0);
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.press("pane devices");
        h.wait(0.3);
        assert!(!h.has("devices adapter"), "a 404: no line");
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
        assert_eq!(h.app.chrome.pane, Pane::Claude, "the Agent first");

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
        // The first end of a stream that was delivering is reopened quietly.
        b.state.apply(Msg::Failed(Stream::Values), t);
        b.state.apply(Msg::Failed(Stream::Values), t);
        assert!(
            b.status(t, &Theme::dark())
                .0
                .starts_with("● reconnecting (retry in 2s)"),
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
    use crate::box_popup::PROBE_AFTER;
    use crate::remote::{Fetch, Msg, Probe, Stream};

    /// All the text the last frames drew.
    fn texts(h: &mut Harness) -> String {
        fn walk(s: &eframe::egui::Shape, out: &mut String) {
            match s {
                eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                eframe::egui::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                _ => {}
            }
        }
        h.wait(0.1);
        let mut out = String::new();
        for c in &h.shapes {
            walk(&c.shape, &mut out);
        }
        out
    }

    /// The first look failed: the page simulates at once and says so in
    /// the middle of the screen, then looks once more in the background.
    fn fell_back() -> Harness {
        let mut h = Harness::new(App::boxed());
        assert_eq!(h.app.source, Source::Box);
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        assert!(!b.state.ever_connected());
        // One failure is enough (Nick, 2026-11-13: no waiting twice).
        b.state.apply(Msg::Failed(Stream::Values), t);
        h.frame(vec![]);
        assert_eq!(
            h.app.source,
            Source::Sim,
            "the simulation, unasked, on the first failure"
        );
        h.frame(vec![]);
        assert!(h.has("popup ok"), "the No box popup is up");
        let text = texts(&mut h);
        assert!(text.contains("Simulating in this browser"), "{text}");
        assert!(text.contains("Looking once more in the background."), "{text}");
        assert!(
            h.app.sim.as_ref().is_some_and(|s| s.session.build.is_some()),
            "the simulation runs meanwhile"
        );
        h
    }

    /// Waits for the second look to go out.
    fn asked(h: &mut Harness) {
        assert!(matches!(
            h.app.boxv.as_ref().unwrap().state.probe(),
            Probe::Due(_)
        ));
        h.wait(PROBE_AFTER + 0.1);
        assert_eq!(
            h.app.boxv.as_ref().unwrap().state.probe(),
            Probe::Asked,
            "the second look went"
        );
    }

    /// Nick, 2026-11-01: a page that finds no box runs the simulation
    /// by itself; a box that answered and went away keeps its view.
    #[test]
    fn a_page_no_box_ever_answered_falls_back_to_the_simulation_by_itself() {
        let mut h = fell_back();
        let s = h.app.sim.as_ref().expect("running");
        assert!(
            s.notice.as_deref().is_some_and(|n| n.contains("No box answered")),
            "{:?}",
            s.notice
        );
        h.press("popup ok");
        h.frame(vec![]);
        assert!(!h.has("popup ok"), "closed");
        // Back to the box from View stays: the fallback happens once.
        h.press("bar view");
        h.press("view box");
        assert_eq!(h.app.source, Source::Box);
        h.wait(0.2);
        assert_eq!(h.app.source, Source::Box, "not yanked back to the simulation");
    }

    /// Nick, 2026-11-13: the second look runs in the background once the
    /// simulation is up; a box found then is offered in a popup.
    #[test]
    fn a_box_found_on_the_second_look_is_offered_and_connected_on_a_click() {
        let mut h = fell_back();
        asked(&mut h);
        let t = h.time;
        h.app
            .boxv
            .as_mut()
            .unwrap()
            .state
            .apply(Msg::Got(Fetch::Modules, Ok(r#"[{"name":"main"}]"#.into())), t);
        h.frame(vec![]);
        h.frame(vec![]);
        assert!(h.has("popup connect") && h.has("popup keep"), "the Found popup");
        assert!(!h.has("popup ok"), "it replaces the No box popup");
        assert!(texts(&mut h).contains("Box found"));
        assert_eq!(h.app.source, Source::Sim, "nothing moves until asked");
        h.press("popup connect");
        assert_eq!(h.app.source, Source::Box);
        h.frame(vec![]);
        assert!(
            !h.app.boxv.as_ref().unwrap().state.paused(),
            "the box's streams open again"
        );
        assert!(!h.has("popup connect"));
    }

    #[test]
    fn keep_simulating_closes_the_found_popup_for_good() {
        let mut h = fell_back();
        asked(&mut h);
        let t = h.time;
        h.app
            .boxv
            .as_mut()
            .unwrap()
            .state
            .apply(Msg::Got(Fetch::Modules, Ok(r#"[{"name":"main"}]"#.into())), t);
        h.frame(vec![]);
        h.frame(vec![]);
        h.press("popup keep");
        h.wait(PROBE_AFTER * 2.0);
        assert_eq!(h.app.source, Source::Sim);
        assert!(!h.has("popup connect"), "asked once");
    }

    /// A slow first look says so in the middle of the screen; a lost box
    /// says it is reconnecting until it is back, or hidden.
    #[test]
    fn connecting_and_a_lost_box_are_said_in_the_middle_of_the_screen() {
        let mut h = Harness::new(App::boxed());
        h.frame(vec![]);
        assert!(!texts(&mut h).contains("Connecting to the box"), "no flash");
        h.wait(crate::box_popup::CONNECTING_AFTER);
        assert!(texts(&mut h).contains("Connecting to the box"));
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        b.state
            .apply(Msg::Got(Fetch::Modules, Ok(r#"[{"name":"main"}]"#.into())), t);
        b.state.apply(Msg::Opened(Stream::Values), t);
        assert!(
            !texts(&mut h).contains("Connecting to the box"),
            "gone once the box answers"
        );
        // The box goes away: a stream that was delivering is reopened
        // quietly once, then it is a failure.
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        b.state.apply(Msg::Failed(Stream::Values), t);
        b.state.apply(Msg::Failed(Stream::Values), t);
        let text = texts(&mut h);
        assert!(text.contains("Reconnecting to the box"), "{text}");
        assert!(text.contains("keep running on the box"), "{text}");
        assert_eq!(h.app.source, Source::Box, "a box that answered stays the box");
        h.press("popup hide");
        assert!(!texts(&mut h).contains("Reconnecting to the box"), "hidden");
        // Back on the next retry, then lost again: said again.
        h.wait(crate::remote::RETRY_SECS);
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        b.state.apply(Msg::Opened(Stream::Values), t);
        b.state.apply(
            Msg::Event(Stream::Values, "values".into(), r#"{"seq":9}"#.into()),
            t,
        );
        h.frame(vec![]);
        assert_eq!(
            h.app.boxv.as_ref().unwrap().state.link(),
            crate::remote::Link::Connected
        );
        assert!(!h.app.lost_hidden, "a box that is back resets Hide");
        let b = h.app.boxv.as_mut().unwrap();
        // It was delivering: the first end is reopened quietly.
        b.state.apply(Msg::Failed(Stream::Values), t + 0.2);
        b.state.apply(Msg::Failed(Stream::Values), t + 0.2);
        assert!(texts(&mut h).contains("Reconnecting to the box"), "said again");
    }

    /// The second look finds nothing: the popup says so, and no third look
    /// goes (the page stays quiet until View › Back to this box).
    #[test]
    fn a_second_look_that_finds_nothing_is_said_and_not_repeated() {
        let mut h = fell_back();
        asked(&mut h);
        let t = h.time;
        h.app
            .boxv
            .as_mut()
            .unwrap()
            .state
            .apply(Msg::Got(Fetch::Modules, Err("HTTP 404".into())), t);
        h.frame(vec![]);
        assert_eq!(h.app.boxv.as_ref().unwrap().state.probe(), Probe::Missed);
        assert!(texts(&mut h).contains("found no box either"));
        h.wait(PROBE_AFTER * 3.0);
        assert_eq!(
            h.app.boxv.as_ref().unwrap().state.probe(),
            Probe::Missed,
            "no third look"
        );
        assert!(!h.has("popup connect"));
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
        // Nothing is read before a sign-in (card read-lock); an operator's will do.
        rv(&mut h)
            .state
            .login_pin("operator", crate::review_tests::OPERATOR.into());
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
        // Not signed in: no row to click (every read needs a session, card
        // read-lock), no token field anywhere, and the panel asks for a
        // role and its PIN (the box has no accounts).
        assert!(rv(&mut h).state.list.is_empty(), "nothing read before a sign-in");
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

        // Signed in, the list came, and the row opens.
        click(&mut h, &format!("row {id}"));
        pump(&mut h, &b);
        assert_eq!(rv(&mut h).state.open.as_ref().map(|o| o.id), Some(id));
        // Above the button: what the deploy keeps and resets, in words.
        assert!(
            h.drew("Before you approve: this deploy keeps 5 values, starts HORN at the initial value (new)."),
            "the approval screen shows the preview"
        );
        assert!(h.drew("new HORN"), "the reset value is marked apart");
        let t = Target::Proposal(id);
        let state = rect(&mut h, "state");
        assert!(state.top() < rect(&mut h, &format!("go {t:?}")).top());

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
            5,
            "the list, the outputs, the policy, who is signed in and how to sign in: {calls:?}"
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
        // One pane on the default layout (every tab on the left).
        assert!(!h.has("right"), "no right pane until a tab is docked there");
        let left = h.rect("left");
        card_is_fair(&h, "welcome");
        assert_eq!(anchor(&h), None, "the welcome sits in the middle");
        h.press("tour start");
        h.wait(0.2);
        assert_eq!(step(&h), Step::OpenBlink);
        // The playground opens on the Agent: the card points at Files.
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
        let id = h.editor_id();
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
        // The Code tab is in front: the card points at the I/O tab first.
        assert_eq!(anchor(&h).as_deref(), Some("tab io"));
        h.press("tab io");
        h.wait(0.2);
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
        // The pane kept its width the whole way.
        let l = h.rect("left").right() - left.right();
        assert!((0.0..=8.0).contains(&l), "the left pane grew {l} pt");
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

/// The outputs as lamps in the diagram's corner (`lamps`; Nick,
/// 2026-11-12: the first screen should show itself without words).
mod lamps {
    use eframe::egui::{Color32, Pos2, Shape};

    use super::Harness;
    use crate::app::App;
    use crate::st_view::brand::{GREEN, RED, YELLOW};

    /// The circles of radius `r` the last frame drew inside the diagram,
    /// top to bottom: centre and fill.
    fn lamps(h: &Harness, r: f32) -> Vec<(Pos2, Color32)> {
        fn walk(s: &Shape, r: f32, out: &mut Vec<(Pos2, Color32)>) {
            match s {
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, r, out)),
                Shape::Circle(c) if (c.radius - r).abs() < 0.01 => out.push((c.center, c.fill)),
                _ => {}
            }
        }
        let area = h.app.chrome.diagram;
        let mut out = Vec::new();
        for c in &h.shapes {
            walk(&c.shape, r, &mut out);
        }
        out.retain(|(c, _)| area.contains(*c));
        out.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)));
        out
    }

    #[test]
    fn the_page_opens_on_the_traffic_light_lit_in_the_diagrams_corner() {
        let mut h = Harness::new(App::sim());
        h.wait(0.5);
        let sim = h.app.sim.as_ref().unwrap();
        assert_eq!(sim.title(), "traffic");
        assert!(sim.session.running);
        let area = h.app.chrome.diagram;
        let l = lamps(&h, 15.0);
        assert_eq!(l.len(), 3, "red over yellow over green: {l:?}");
        assert!(
            l.iter()
                .all(|(c, _)| c.x > area.center().x && c.y > area.center().y),
            "in the bottom-right corner of {area:?}: {l:?}"
        );
        assert!(
            l.iter().all(|(c, _)| (c.x - l[0].0.x).abs() < 0.01),
            "one column: {l:?}"
        );
        // Green holds the first 3 s: full colour; the other two nearly out.
        assert_eq!(l[2].1, GREEN, "{l:?}");
        assert_ne!(l[0].1, RED, "{l:?}");
        assert_ne!(l[1].1, YELLOW, "{l:?}");
        // Then one second of yellow, then red.
        h.wait(3.0);
        let l = lamps(&h, 15.0);
        assert_eq!(l[1].1, YELLOW, "{l:?}");
        assert_ne!(l[2].1, GREEN, "{l:?}");
        h.wait(1.5);
        let l = lamps(&h, 15.0);
        assert_eq!(l[0].1, RED, "{l:?}");
        assert_ne!(l[1].1, YELLOW, "{l:?}");
    }

    #[test]
    fn any_other_bool_output_is_a_pilot_lamp_with_its_name() {
        let mut h = Harness::blink();
        h.wait(0.3);
        let l = lamps(&h, 11.0);
        assert_eq!(l.len(), 1, "Blink's one lamp: {l:?}");
        assert!(h.drew("lamp"), "named under it");
        // It blinks: on at full brand yellow within a period.
        let mut seen_on = false;
        for _ in 0..40 {
            h.wait(0.05);
            seen_on |= lamps(&h, 11.0).first().is_some_and(|(_, c)| *c == YELLOW);
        }
        assert!(seen_on);
    }
}

/// The glyph library beside every point (`glyph_lib`).
mod glyph_library {
    use eframe::egui::{self, Rect, Shape, TextureId};

    use super::Harness;
    use crate::app::App;
    use crate::editor::Pane;

    /// How many library glyphs (meshes on a texture other than the font
    /// atlas) the last frame drew inside `area`.
    fn glyphs_in(h: &Harness, area: Rect) -> usize {
        fn walk(s: &Shape, area: Rect, n: &mut usize) {
            match s {
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, n)),
                Shape::Mesh(m)
                    if m.texture_id != TextureId::default()
                        && m.vertices.first().is_some_and(|v| area.contains(v.pos)) =>
                {
                    *n += 1;
                }
                _ => {}
            }
        }
        let mut n = 0;
        for c in &h.shapes {
            walk(&c.shape, area, &mut n);
        }
        n
    }

    #[test]
    fn the_traffic_lights_timer_block_carries_the_timer_glyph_in_its_header() {
        let mut h = Harness::new(App::sim());
        h.wait(0.3);
        let v = super::sim_view(&h);
        let (centre, width) = super::node(
            v,
            |k| matches!(k, helmsman_diagram::NodeKind::Fb { fb, .. } if fb == "TON"),
        );
        let block = Rect::from_center_size(centre, eframe::egui::vec2(width, width));
        assert_eq!(
            glyphs_in(&h, block),
            1,
            "the TON block's header has the timer glyph"
        );
    }

    /// The Palette tab (`palette`): a piece dragged onto the diagram, or
    /// clicked, is sent to the Agent as its request, and the panel turns
    /// to the Agent tab to show the answer coming.
    #[test]
    fn a_palette_piece_dropped_on_the_diagram_or_clicked_asks_the_agent() {
        let mut h = Harness::new(App::sim());
        h.app
            .sim
            .as_mut()
            .unwrap()
            .chat
            .key_mut()
            .push_str("sk-ant-test-key");
        // The panels' widths show as the diagram's: it keeps its size.
        let before = h.app.chrome.diagram;
        let tab = h.rect("tab palette").center();
        h.click(tab);
        assert!(h.app.chrome.showing(Pane::Palette));
        let panel = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(before.left(), h.size.y));
        assert_eq!(glyphs_in(&h, panel), 12, "the twelve pieces are drawn");
        assert_eq!(h.app.chrome.diagram, before, "the grid fits the panel");
        let sent = |h: &Harness| h.app.sim.as_ref().unwrap().link.sent.len();
        assert_eq!(sent(&h), 0);
        // Dropped outside the diagram (back on the panel): nothing.
        let piece = h.rect("palette push-button").center();
        h.drag(piece, piece + eframe::egui::vec2(0.0, 300.0));
        assert_eq!(sent(&h), 0, "dropped on the panel");
        // Dropped on the diagram: asked.
        let piece = h.rect("palette push-button").center();
        h.drag(piece, h.app.chrome.diagram.center());
        assert_eq!(sent(&h), 1);
        assert!(h.app.chrome.showing(Pane::Claude), "the answer's tab");
        let body = &h.app.sim.as_ref().unwrap().link.sent[0].1.body;
        assert!(body.contains("Add a push button"), "{body}");
        // A click asks too.
        h.app.sim.as_mut().unwrap().stop_answer();
        h.click(tab);
        h.click(h.rect("palette timer").center());
        assert_eq!(sent(&h), 2);
        assert!(
            h.app.sim.as_ref().unwrap().link.sent[1]
                .1
                .body
                .contains("Add a timer (TON)")
        );
    }

    #[test]
    fn every_output_in_the_io_tab_has_a_glyph_and_the_panel_keeps_its_width() {
        let mut h = Harness::new(App::sim());
        h.app.chrome.show(Pane::Io);
        let before = h.app.chrome.diagram;
        h.wait(0.3);
        let outputs = h.rect("io outputs");
        // The traffic light's three lamps.
        assert_eq!(glyphs_in(&h, outputs), 3);
        assert!(
            h.app.chrome.rect("io inputs").is_none(),
            "run is a control on the diagram"
        );
        assert_eq!(
            h.app.chrome.diagram, before,
            "the glyph column did not widen the panel"
        );
    }
}

/// A device's own configuration on the Devices tab (`device_config.rs`).
mod devconf {
    use super::Harness;
    use crate::app::App;
    use crate::box_projects::{Buf, OpenProject, PCall, PReq};
    use crate::box_tests::{Bench, watch};
    use crate::device_config::ConfigOp;
    use crate::review::{Answer, Session};

    const MAP: &str = "device rio device:moxa-e1213 mac=00-90-e8-7e-af-be\nin part bool rio ch:DI-04\n\
                       device plain device:other-box mac=00-90-e8-7e-af-bf\nin other bool plain ch:DI-00\n";
    const SECRET: &str = "hunter2-secret";

    fn buf(text: &str) -> Buf {
        Buf {
            text: text.into(),
            base: text.into(),
            loaded: true,
            ..Buf::default()
        }
    }

    fn facts() -> String {
        r#"{"device":"rio","facts":{"firmware":"V3.1",
            "watchdog":{"enabled":false,"value":"0","label":"Disabled"},
            "outputs":[
              {"channel":"DO-00","safe":{"value":"0"},"power_on":{"value":"1"}},
              {"channel":"DO-01","safe":{"value":"0"},"power_on":{"value":"1"}},
              {"channel":"DO-02","safe":{"value":"0"},"power_on":{"value":"0"}}],
            "eip_enabled":false,"rest_enabled":true},
            "snapshot":{"file":"rio.devconf","version":"v","bytes":9},
            "drift":{"changed":[{"before":"A=1","after":"A=2"}],"added":["B=2"],"removed":["C=3"]},
            "password_stored":true}"#
            .into()
    }

    /// A signed-in box view with a project whose map has a configurable
    /// device (`rio`) and one that is not (`plain`), the Devices tab open.
    fn open(role: &str) -> Harness {
        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(App::boxed());
        let v = h.app.boxv.as_mut().unwrap();
        v.state = state;
        v.projects.available = Some(true);
        // The app hands the box view the review view's session each frame.
        h.app.review.as_mut().unwrap().state.session = Some(Session {
            user: Some("alice".into()),
            role: role.into(),
        });
        let v = h.app.boxv.as_mut().unwrap();
        let mut o = OpenProject {
            name: "demo".into(),
            ..OpenProject::default()
        };
        o.bufs.insert("io.map".into(), buf(MAP));
        o.bufs.insert(
            "moxa-e1213.device".into(),
            buf("helmsman-device 1\nvendor Moxa\nconfigure moxa-e1200\n"),
        );
        o.bufs.insert(
            "other-box.device".into(),
            buf("helmsman-device 1\nvendor Other\n"),
        );
        v.projects.open = Some(o);
        h.frame(vec![]);
        h.press("pane devices");
        h.wait(0.3);
        h
    }

    fn sent(h: &mut Harness) -> Vec<PReq> {
        h.frame(vec![]);
        std::mem::take(&mut h.app.boxv.as_mut().unwrap().psent)
    }

    fn answer(h: &mut Harness, op: ConfigOp, status: u16, body: &str) {
        let ctx = h.ctx.clone();
        let a = Answer {
            status,
            retry_after: None,
            body: body.into(),
        };
        let t = h.time;
        h.app.boxv.as_mut().unwrap().step_all(
            &ctx,
            Vec::new(),
            vec![(PCall::Config("rio".into(), op), a)],
            t,
        );
        h.wait(0.2);
    }

    /// All the text the last frame drew.
    fn texts(h: &mut Harness) -> String {
        fn walk(s: &eframe::egui::Shape, out: &mut String) {
            match s {
                eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                eframe::egui::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                _ => {}
            }
        }
        // An opening window draws nothing on its first frame.
        h.wait(0.1);
        let mut out = String::new();
        for c in &h.shapes {
            walk(&c.shape, &mut out);
        }
        out
    }

    fn config_calls(reqs: &[PReq]) -> Vec<(&'static str, String, String)> {
        reqs.iter()
            .filter(|r| matches!(r.call, PCall::Config(..)))
            .map(|r| {
                (
                    r.method,
                    r.url.clone(),
                    r.body
                        .as_ref()
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Reads the configuration of `rio` only: its profile names a configurator,
    /// the other's does not.
    fn ready(role: &str) -> Harness {
        let mut h = open(role);
        let first = sent(&mut h);
        assert_eq!(
            config_calls(&first),
            [("GET", "/api/devices/rio/config".to_string(), String::new())],
            "one read, for the device whose profile has a configuration"
        );
        answer(&mut h, ConfigOp::Read, 200, &facts());
        let _ = sent(&mut h);
        h
    }

    #[test]
    fn the_section_is_for_a_device_with_configuration_and_reads_first_in_plain_words() {
        let mut h = ready("engineer");
        assert!(h.has("config actions rio"));
        assert!(
            !h.has("config actions plain"),
            "a device without configuration has no section"
        );
        let t = texts(&mut h);
        for want in [
            "Outputs hold their state if this box stops: the device's watchdog is off.",
            "DO-00 and DO-01 switch on at power-up.",
            "EtherNet/IP is off on the device.",
            "firmware V3.1",
        ] {
            assert!(t.contains(want), "{want:?} not in:\n{t}");
        }
        assert!(!t.contains("HTTP"), "no protocol talk");
        // The box says there is nothing to configure: the section goes.
        let mut h = open("engineer");
        answer(
            &mut h,
            ConfigOp::Read,
            404,
            r#"{"error":"x","code":"not_configurable"}"#,
        );
        assert!(!h.has("config actions rio"));
    }

    #[test]
    fn drift_counts_in_words_and_shows_its_lines_on_demand() {
        let mut h = ready("engineer");
        assert!(texts(&mut h).contains("3 settings differ from the project's saved configuration"));
        assert!(!texts(&mut h).contains("A=2"));
        h.press("config drift rio");
        h.wait(0.2);
        let t = texts(&mut h);
        assert!(t.contains("A=2") && t.contains("B=2") && t.contains("C=3"), "{t}");
    }

    #[test]
    fn the_panel_keeps_its_width_with_the_section_and_its_lines_open() {
        let mut h = open("engineer");
        // Docked right: the Devices tab at the right pane's default
        // width, its narrowest place.
        h.app.chrome.dock(crate::editor::Pane::Devices, true);
        h.wait(0.3);
        let (left, right) = (h.rect("left"), h.rect("right"));
        answer(&mut h, ConfigOp::Read, 200, &facts());
        h.press("config drift rio");
        h.press("config actions rio");
        h.wait(0.3);
        assert!(
            (0.0..=8.0).contains(&(h.rect("left").right() - left.right())),
            "the left pane grew"
        );
        assert!(
            (0.0..=8.0).contains(&(right.left() - h.rect("right").left())),
            "the right panel grew"
        );
    }

    #[test]
    fn an_operator_cannot_act_and_is_told_why() {
        let mut h = ready("operator");
        assert!(h.has("config locked rio"));
        h.press("config actions rio");
        h.press("config item snapshot");
        h.press("config item apply");
        assert!(config_calls(&sent(&mut h)).is_empty(), "nothing was sent");
        assert!(!h.has("config form"), "no question opened");
    }

    #[test]
    fn an_engineer_saves_a_snapshot() {
        let mut h = ready("engineer");
        h.press("config actions rio");
        h.press("config item snapshot");
        let calls = config_calls(&sent(&mut h));
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].0, calls[0].1.as_str()),
            ("POST", "/api/devices/rio/config/snapshot")
        );
    }

    #[test]
    fn a_restart_says_the_outage_first_and_asks_again_when_modules_are_interrupted() {
        let mut h = ready("engineer");
        h.press("config actions rio");
        h.press("config item apply");
        assert!(texts(&mut h).contains("offline for about 21 seconds"));
        assert!(
            config_calls(&sent(&mut h)).is_empty(),
            "nothing before the confirmation"
        );
        h.press("config confirm");
        let calls = config_calls(&sent(&mut h));
        assert_eq!(
            (calls[0].0, calls[0].1.as_str(), calls[0].2.as_str()),
            ("POST", "/api/devices/rio/config/restart", "{}")
        );
        assert!(
            texts(&mut h).contains("restarting"),
            "the device is shown restarting"
        );
        answer(
            &mut h,
            ConfigOp::Restart,
            409,
            r#"{"error":"the project reads it","code":"interrupts","modules":["fill","mix"]}"#,
        );
        let t = texts(&mut h);
        assert!(t.contains("fill, mix"), "names the modules: {t}");
        assert!(!t.contains("409"), "no raw status");
        assert!(
            config_calls(&sent(&mut h)).is_empty(),
            "asks again before resending"
        );
        h.press("config confirm");
        let calls = config_calls(&sent(&mut h));
        assert_eq!(calls[0].2, r#"{"interrupts":true}"#);
        answer(
            &mut h,
            ConfigOp::Restart,
            200,
            r#"{"restarted":true,"took_ms":21000}"#,
        );
        assert!(texts(&mut h).contains("Applied."));
    }

    #[test]
    fn a_refusal_is_shown_in_the_boxs_words() {
        let mut h = ready("engineer");
        h.press("config actions rio");
        h.press("config item snapshot");
        let _ = sent(&mut h);
        answer(
            &mut h,
            ConfigOp::Snapshot,
            409,
            r#"{"error":"No project is running.","code":"no_project"}"#,
        );
        let t = texts(&mut h);
        assert!(t.contains("No project is running."), "{t}");
        assert!(!t.contains("409"));
    }

    #[test]
    fn a_change_is_staged_and_says_so_until_a_restart() {
        let mut h = ready("engineer");
        h.press("config actions rio");
        h.press("config item settings");
        h.press("config safe DO-02");
        h.press("config stage");
        let calls = config_calls(&sent(&mut h));
        assert_eq!(calls[0].1, "/api/devices/rio/config/set");
        assert!(
            calls[0].2.contains("11_safeM.htm") && calls[0].2.contains(r#""SetSM02":"1""#),
            "{}",
            calls[0].2
        );
        assert!(!texts(&mut h).contains("Staged, not applied yet"));
        answer(&mut h, ConfigOp::Set, 200, r#"{"staged":true,"diff":{}}"#);
        assert!(texts(&mut h).contains("Staged, not applied yet"));
        assert!(h.has("config staged rio"));
        // The read after it, then a restart clears the note.
        let _ = sent(&mut h);
        answer(&mut h, ConfigOp::Restart, 200, r#"{"restarted":true}"#);
        assert!(!texts(&mut h).contains("Staged, not applied yet"));
    }

    #[test]
    fn a_restore_keeps_the_ip_and_asks_before_it_runs() {
        let mut h = ready("engineer");
        h.press("config actions rio");
        h.press("config item restore");
        assert!(texts(&mut h).contains("IP address stays as it is"));
        assert_eq!(config_calls(&sent(&mut h)).len(), 0);
        h.press("config confirm");
        let calls = config_calls(&sent(&mut h));
        assert_eq!(calls[0].1, "/api/devices/rio/config/import");
        assert_eq!(
            calls[0].2, r#"{"from":"snapshot"}"#,
            "the network overwrite is never sent"
        );
    }

    #[test]
    fn the_password_field_never_shows_a_stored_value_or_what_was_typed_afterwards() {
        let mut h = ready("engineer");
        h.press("config actions rio");
        h.press("config item password");
        assert!(texts(&mut h).contains("A password is stored."));
        h.press("config password field");
        h.frame(vec![eframe::egui::Event::Text(SECRET.into())]);
        h.frame(vec![]);
        assert!(!texts(&mut h).contains(SECRET), "typed text is masked");
        h.press("config password go");
        let calls = config_calls(&sent(&mut h));
        assert_eq!(
            (calls[0].0, calls[0].1.as_str()),
            ("PUT", "/api/devices/rio/config/password")
        );
        assert!(calls[0].2.contains(SECRET), "the box gets it");
        answer(&mut h, ConfigOp::SetPassword, 200, r#"{"password_stored":true}"#);
        assert!(!texts(&mut h).contains(SECRET));
        assert!(!format!("{:?}", h.app.boxv.as_ref().unwrap().devices_ui.config).contains(SECRET));
        // Opened again, the field is empty.
        h.press("config actions rio");
        h.press("config item password");
        assert!(!texts(&mut h).contains(SECRET));
        let _ = sent(&mut h);
        let mut sent_again = false;
        h.press("config password field");
        h.frame(vec![]);
        if h.has("config password go") {
            h.press("config password go");
            sent_again = !config_calls(&sent(&mut h)).is_empty();
        }
        assert!(!sent_again, "an empty field sends nothing");
    }

    #[test]
    fn a_failed_read_says_why_and_can_be_asked_again() {
        let mut h = open("engineer");
        let _ = sent(&mut h);
        answer(
            &mut h,
            ConfigOp::Read,
            502,
            r#"{"error":"The device did not answer.","code":"unreachable"}"#,
        );
        assert!(texts(&mut h).contains("The device did not answer."));
        h.press("config again rio");
        assert_eq!(config_calls(&sent(&mut h)).len(), 1);
    }
}

mod shell;
