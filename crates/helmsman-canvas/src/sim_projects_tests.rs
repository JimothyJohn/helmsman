//! The simulation's projects: kept in the page's storage (and fine
//! without it), the examples as read-only starters, Deploy compiling and
//! running in the page, and Upload and Download in the box's own `.zip`
//! (checked against helmsman-plc's project store, the box's real code).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use eframe::egui::{self, Pos2, RawInput, Rect, Vec2};
use helmsman_project::Files;

use crate::app::App;
use crate::playground_view::PlaygroundView;
use crate::projects::{Act, Workspace};
use crate::sim_projects::{SimProjects, Storage, example_project};

/// A storage two page loads share (a browser's `localStorage`).
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Option<String>>>);

impl Storage for Shared {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.0.borrow().clone())
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(text.to_string());
        Ok(())
    }
}

/// A browser whose storage throws on every access (a private window,
/// blocked site data).
struct Broken;

impl Storage for Broken {
    fn load(&self) -> Result<Option<String>, String> {
        Err("SecurityError: The operation is insecure.".into())
    }

    fn save(&mut self, _: &str) -> Result<(), String> {
        Err("QuotaExceededError".into())
    }
}

const BLOCKS: &str = "FUNCTION_BLOCK Latch
VAR_INPUT set : BOOL; reset : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
  q := (q OR set) AND NOT reset;
END_FUNCTION_BLOCK
";

const MAIN: &str = "PROGRAM conveyor
VAR_INPUT start : BOOL; stop : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR l : Latch; END_VAR
  l(set := start, reset := stop);
  lamp := l.q;
END_PROGRAM
";

fn frame(ctx: &egui::Context, app: &mut App, t: f64) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 860.0))),
        time: Some(t),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| app.show(ui));
    out.textures_delta.clear();
}

/// A click at `at`, over three frames from page time `t`.
fn click(ctx: &egui::Context, app: &mut App, at: Pos2, t: f64) {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for (k, events) in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(true)],
        vec![button(false)],
    ]
    .into_iter()
    .enumerate()
    {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 860.0))),
            time: Some(t + f64::from(u8::try_from(k).unwrap()) / 60.0),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
    }
}

/// A project of two files that need each other, made through the
/// sidebar's acts, saved.
fn two_files(v: &mut PlaygroundView) {
    v.files_ui.confirm = None;
    v.proj.create("two", None);
    v.open_project("two");
    v.proj.new_file("blocks.st");
    *v.proj.current_text_mut().unwrap() = BLOCKS.into();
    v.proj.open_file("main.st");
    *v.proj.current_text_mut().unwrap() = MAIN.into();
    v.save();
    assert!(!v.dirty(), "{:?}", v.proj.notice);
}

#[test]
fn examples_are_read_only_starters_and_saving_one_keeps_a_copy() {
    let store = Shared::default();
    let mut v = PlaygroundView::with_storage(Box::new(store.clone()));
    let (first, file) = example_project(crate::playground::EXAMPLES[0].src);
    assert_eq!((first.as_str(), file.as_str()), ("blink", "blink.st"));
    assert_eq!(v.title(), "blink", "opens the first example");
    assert!(v.session.running, "running, as the playground always opened");
    let rows = v.proj.rows();
    assert!(rows.iter().all(|r| r.example), "{rows:?}");
    assert_eq!(rows.len(), crate::playground::EXAMPLES.len());
    assert!(rows.iter().any(|r| r.name == "blink" && r.running));
    // Edited and saved: a copy under a free name, opened; the example
    // itself is unchanged.
    v.proj.current_text_mut().unwrap().insert_str(0, "(* mine *)\n");
    assert!(v.dirty());
    v.save();
    assert_eq!(v.title(), "blink-2");
    assert!(!v.dirty());
    assert!(
        v.proj
            .notice
            .as_ref()
            .unwrap()
            .0
            .contains("saved your copy as blink-2")
    );
    assert!(
        v.proj.examples["blink"]
            .get("blink.st")
            .unwrap()
            .starts_with("PROGRAM blink")
    );
    // The next visit finds it, open.
    let again = PlaygroundView::with_storage(Box::new(store));
    assert_eq!(again.title(), "blink-2");
    assert!(again.proj.files[&again.proj.current].starts_with("(* mine *)"));
}

#[test]
fn a_project_of_several_files_deploys_in_the_page_and_errors_point_at_their_file() {
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    two_files(&mut v);
    v.deploy();
    assert!(v.session.diags.is_empty(), "{:?}", v.session.diags);
    assert!(v.session.running);
    assert_eq!(v.proj.running.as_deref(), Some("two"));
    assert!(
        v.notice.as_ref().unwrap().contains("two is running in this page"),
        "{:?}",
        v.notice
    );
    assert!(v.session.src.contains("(* file blocks.st *)"), "the box's build");
    // The latch works across files: start sets the lamp.
    assert!(v.session.set_input("START", helmsman_iec::Value::Bool(true)));
    v.session.step();
    assert_eq!(v.session.value("LAMP"), Some(helmsman_iec::Value::Bool(true)));
    // A mistake in main.st: the error names main.st and its own line.
    v.proj.open_file("main.st");
    *v.proj.current_text_mut().unwrap() = MAIN.replace("lamp := l.q;", "lamp := l.q +;");
    v.deploy();
    let d = v.diags();
    assert_eq!((d[0].file.as_str(), d[0].line), ("main.st", 6), "{d:?}");
    assert!(v.notice.as_ref().unwrap().contains("does not compile"));
    // Clicking it (in the app) opens main.st at line 6.
    v.proj.open_file("blocks.st");
    let mut app = App::sim();
    app.sim = Some(v);
    let ctx = egui::Context::default();
    for k in 0..40 {
        frame(&ctx, &mut app, f64::from(k) / 60.0);
    }
    let at = app
        .chrome
        .rect("diag 0")
        .expect("the error under the editor")
        .center();
    click(&ctx, &mut app, at, 1.0);
    assert_eq!(app.sim.as_ref().unwrap().proj.current, "main.st");
    assert_eq!(app.chrome.line, Some(6));
}

#[test]
fn download_then_upload_round_trips_in_the_simulation_and_with_the_box() {
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    two_files(&mut v);
    v.proj.new_file("io.map");
    *v.proj.current_text_mut().unwrap() = "in start bool box di:0\n".into();
    v.save();
    v.download();
    let (name, zip) = v.downloads.pop().expect("a download");
    assert_eq!(name, "two.zip");
    // The box reads it as the same project.
    let files = helmsman_plc::project::zip::read(&zip)
        .and_then(helmsman_plc::project::from_zip)
        .expect("the box takes it");
    assert_eq!(files.0, v.proj.files);

    // Uploaded under another name: a new project, the same files, open.
    v.upload("Two (copy).zip", zip.clone());
    assert_eq!(v.title(), "two-copy", "{:?}", v.proj.notice);
    assert_eq!(v.proj.files, v.proj.saved["two"].0);
    v.download();
    let (_, again) = v.downloads.pop().unwrap();
    assert_eq!(again, zip, "download, upload, download is byte for byte");
    // The same name again: it waits for Replace.
    v.upload("two.zip", zip);
    assert!(v.proj.pending.is_some(), "{:?}", v.proj.notice);
    assert_eq!(v.title(), "two-copy");
    v.proj.replace();
    assert_eq!(v.proj.open, "two");

    // A project the box zipped (its store, on disk) uploads here.
    let dir = std::env::temp_dir().join(format!("helmsman-canvas-simzip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = helmsman_plc::project::Projects::in_state_dir(&dir).unwrap();
    store.create("from-box").unwrap();
    store.save("from-box", "main.st", MAIN.as_bytes(), None).unwrap();
    store
        .save("from-box", "blocks.st", BLOCKS.as_bytes(), None)
        .unwrap();
    let boxed = store.zip("from-box").unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    v.upload("from-box.zip", boxed);
    assert_eq!(v.title(), "from-box", "{:?}", v.proj.notice);
    v.deploy();
    assert!(v.session.diags.is_empty(), "{:?}", v.session.diags);

    // Single files into the open project; hostile ones refused.
    v.upload(
        "Extra.st",
        b"FUNCTION f : INT\n  f := 1;\nEND_FUNCTION\n".to_vec(),
    );
    assert!(v.proj.files.contains_key("Extra.st"), "{:?}", v.proj.notice);
    for (file, bytes, why) in [
        (
            "plant.L5X",
            b"<RSLogix5000Content/>".to_vec(),
            "L5X import is not built yet",
        ),
        ("evil.zip", b"PK\x05\x06garbage".to_vec(), "Upload refused"),
        ("../x.st", b"x".to_vec(), "not a project file name"),
        ("bidi.st", "a\u{202E}b".as_bytes().to_vec(), "not allowed"),
        ("notes.txt", b"hi".to_vec(), "upload takes .zip, .st or .xml"),
    ] {
        let before = v.proj.files.clone();
        v.upload(file, bytes);
        let (msg, err) = v.proj.notice.clone().unwrap();
        assert!(err && msg.contains(why), "{file}: {msg}");
        assert_eq!(v.proj.files, before, "{file} changed nothing");
    }
}

#[test]
fn the_simulation_renders_and_works_when_the_browsers_storage_fails() {
    let mut app = App::sim();
    app.sim = Some(PlaygroundView::with_storage(Box::new(Broken)));
    let ctx = egui::Context::default();
    for k in 0..30 {
        frame(&ctx, &mut app, f64::from(k) / 60.0);
    }
    let v = app.sim.as_mut().unwrap();
    let why = v.proj.storage_problem.clone().expect("said once");
    assert!(why.contains("does not keep projects"), "{why}");
    assert!(v.session.running, "the example runs anyway");
    assert!(app.chrome.rect("storage").is_some(), "the page says so");
    for k in ["mark", "title", "save", "deploy", "menu"] {
        assert!(app.chrome.rect(k).is_some(), "{k} drawn");
    }
    // Projects still work, in memory.
    let v = app.sim.as_mut().unwrap();
    two_files(v);
    v.deploy();
    assert!(v.session.diags.is_empty(), "{:?}", v.session.diags);
    for k in 30..40 {
        frame(&ctx, &mut app, f64::from(k) / 60.0);
    }
    assert_eq!(app.title(), "two");

    // Storage that holds something unreadable, or a project that breaks
    // the rules: started afresh, the bad project dropped.
    let junk = Shared(Rc::new(RefCell::new(Some("{not json".into()))));
    let v = PlaygroundView::with_storage(Box::new(junk));
    assert!(v.proj.storage_problem.is_some());
    assert_eq!(v.title(), "blink");
    let evil = serde_json::json!({
        "format": "helmsman-projects 1",
        "open": "../etc",
        "projects": {
            "../etc": { "main.st": "x" },
            "ok": { "main.st": "PROGRAM ok\nEND_PROGRAM\n", "../x.st": "y" },
            "fine": { "main.st": "PROGRAM fine\nEND_PROGRAM\n" },
        },
    });
    let v = PlaygroundView::with_storage(Box::new(Shared(Rc::new(RefCell::new(Some(evil.to_string()))))));
    assert_eq!(v.proj.saved.keys().collect::<Vec<_>>(), ["fine"]);
    assert_eq!(v.title(), "blink", "the open name it held is not a project");
}

#[test]
fn opening_another_project_with_unsaved_edits_asks_first() {
    let mut app = App::sim();
    app.sim = Some(PlaygroundView::with_storage(Box::new(Shared::default())));
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, 0.0);
    frame(&ctx, &mut app, 0.1);
    app.sim
        .as_mut()
        .unwrap()
        .proj
        .current_text_mut()
        .unwrap()
        .push_str("(* edit *)");
    frame(&ctx, &mut app, 0.2);
    let at = app
        .chrome
        .rect("project latch")
        .expect("the example listed")
        .center();
    click(&ctx, &mut app, at, 0.3);
    let v = app.sim.as_ref().unwrap();
    assert_eq!(
        v.files_ui.confirm,
        Some(Act::OpenProject("latch".into())),
        "asked first"
    );
    assert_eq!(v.title(), "blink", "not opened yet");
    let at = app.chrome.rect("confirm no").unwrap().center();
    click(&ctx, &mut app, at, 0.4);
    assert_eq!(app.title(), "blink");
    assert!(app.sim.as_ref().unwrap().proj.dirty(), "the edits stay");
    let at = app.chrome.rect("project latch").unwrap().center();
    click(&ctx, &mut app, at, 0.5);
    let at = app.chrome.rect("confirm yes").unwrap().center();
    click(&ctx, &mut app, at, 0.6);
    assert_eq!(app.title(), "latch");
    let v = app.sim.as_ref().unwrap();
    assert!(!v.proj.dirty());
    assert!(!v.session.running, "opened stopped: Deploy runs it");
    // A fresh page with nothing stored starts clean.
    assert!(!SimProjects::load(Box::new(Shared::default())).dirty());
    assert!(Files(BTreeMap::new()).check().is_ok());
}

#[test]
fn two_programs_in_the_simulation_are_refused_at_the_second() {
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    v.proj.create("pair", None);
    v.open_project("pair");
    v.proj.new_file("other.st");
    *v.proj.current_text_mut().unwrap() =
        "PROGRAM other\nVAR x : INT; END_VAR\nx := 1;\nEND_PROGRAM\n".into();
    v.deploy();
    let d = v.diags();
    assert!(
        d.iter()
            .any(|d| d.message.contains("only one PROGRAM") && d.file == "other.st" && d.line == 1),
        "{d:?}"
    );
}

/// Where the block whose text is `text` sits.
fn block_at(v: &PlaygroundView, text: &str) -> (f32, f32) {
    let n = v
        .viewer
        .as_ref()
        .unwrap()
        .diagram()
        .nodes
        .iter()
        .find(|n| n.text == text)
        .unwrap_or_else(|| panic!("no {text}"));
    (n.rect.x, n.rect.y)
}

fn nudge(v: &mut PlaygroundView, text: &str, by: Vec2) {
    let w = v.viewer.as_mut().unwrap();
    let n = w.diagram().nodes.iter().find(|n| n.text == text).unwrap().id;
    w.move_node(n, by);
    v.arrange();
}

/// Card `arrangement-persist` in the simulation: the open project keeps a
/// shared default layout (in its stored files and its `.zip`), this
/// browser keeps tweaks on top, and both come back on a reload.
#[test]
fn the_simulation_keeps_a_default_layout_and_tweaks_across_a_reload() {
    let (projects, tweaks) = (Shared::default(), Shared::default());
    let ctx = egui::Context::default();
    let mut v = PlaygroundView::with_stores(Box::new(projects.clone()), Box::new(tweaks.clone()));
    // An example is read-only: its layout is not saved.
    assert!(v.proj.can_save_layout().is_err());
    v.save_layout();
    assert!(v.proj.notice.as_ref().unwrap().1, "{:?}", v.proj.notice);
    assert_eq!(v.proj.layout(), "");

    two_files(&mut v);
    v.deploy();
    v.tick(&ctx);
    let (lamp, horn) = ("LAMP := L.Q", "L(SET := START, RESET := STOP)");
    let auto = (block_at(&v, lamp), block_at(&v, horn));
    nudge(&mut v, lamp, Vec2::new(0.0, 90.0));
    v.save_layout();
    assert!(v.proj.layout().contains("LAMP := L.Q"), "{:?}", v.proj.notice);
    assert!(!v.layers.tweaked(), "saved as the default: no tweak");
    let zip = v.proj.saved["two"].to_zip().unwrap();
    assert!(
        helmsman_project::read_zip(&zip)
            .unwrap()
            .get("diagram.layout")
            .is_some_and(|t| t.contains("LAMP := L.Q")),
        "in the project's .zip"
    );
    // A personal tweak on top.
    nudge(&mut v, horn, Vec2::new(70.0, 0.0));
    assert!(v.layers.tweaked());
    let both = (block_at(&v, lamp), block_at(&v, horn));
    assert_eq!(both.0, (auto.0.0, auto.0.1 + 90.0));
    assert_eq!(both.1, (auto.1.0 + 70.0, auto.1.1));

    // Reloaded: the project opens as it was left, laid out as it was.
    let mut v = PlaygroundView::with_stores(Box::new(projects), Box::new(tweaks));
    assert_eq!(v.title(), "two");
    v.tick(&ctx);
    assert_eq!((block_at(&v, lamp), block_at(&v, horn)), both);
    // Reset: the default; again: the automatic layout.
    let w = v.viewer.as_mut().unwrap();
    v.layers.reset(w);
    assert_eq!(block_at(&v, horn), auto.1);
    assert_eq!(block_at(&v, lamp), both.0);
    let w = v.viewer.as_mut().unwrap();
    v.layers.reset(w);
    assert_eq!((block_at(&v, lamp), block_at(&v, horn)), auto);
}
