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
use crate::projects::{Act, ProjectRow, Workspace};
use crate::sim_projects::{SimProjects, Storage, example_project};

/// A storage two page loads share (a browser's `localStorage`).
#[derive(Clone, Default)]
pub(crate) struct Shared(Rc<RefCell<Option<String>>>);

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
VAR_INPUT
  set : BOOL;
  reset : BOOL;
END_VAR
VAR_OUTPUT
  q : BOOL;
END_VAR
q := (q OR set) AND NOT reset;
END_FUNCTION_BLOCK
";

const MAIN: &str = "PROGRAM conveyor
VAR_INPUT
  start : BOOL;
  stop : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  l : Latch;
END_VAR
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
    assert!(rows.iter().all(ProjectRow::example), "{rows:?}");
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
    assert_eq!((d[0].file.as_str(), d[0].line), ("main.st", 13), "{d:?}");
    assert!(v.notice.as_ref().unwrap().contains("does not compile"));
    // Clicking it (in the app) opens main.st at line 13.
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
    assert_eq!(app.chrome.line, Some(13));
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
            "plant.L5X is not converted",
        ),
        ("bad.L5X", vec![0xFF, 0xFE], "not UTF-8"),
        ("evil.zip", b"PK\x05\x06garbage".to_vec(), "Upload refused"),
        ("../x.st", b"x".to_vec(), "not a project file name"),
        ("bidi.st", "a\u{202E}b".as_bytes().to_vec(), "not allowed"),
        ("notes.txt", b"hi".to_vec(), "upload takes .zip, .st, .xml"),
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
    // What the page has to say is in the Messages tab.
    app.chrome.bottom = crate::editor::Bottom::Messages;
    let ctx = egui::Context::default();
    for k in 0..30 {
        frame(&ctx, &mut app, f64::from(k) / 60.0);
    }
    let v = app.sim.as_mut().unwrap();
    let why = v.proj.storage_problem.clone().expect("said once");
    assert!(why.contains("does not keep projects"), "{why}");
    assert!(v.session.running, "the example runs anyway");
    assert!(app.chrome.rect("storage").is_some(), "the page says so");
    for k in ["mark", "title", "save", "bar file", "help"] {
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
    app.chrome.show_files();
    app.sim = Some(PlaygroundView::with_storage(Box::new(Shared::default())));
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, 0.0);
    frame(&ctx, &mut app, 0.1);
    // A program of the visitor's own to switch to.
    app.sim.as_mut().unwrap().proj.create("mine", Some("latch"));
    app.sim.as_mut().unwrap().open_project("blink");
    frame(&ctx, &mut app, 0.15);
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
        .rect("project mine")
        .expect("the visitor's program listed")
        .center();
    click(&ctx, &mut app, at, 0.3);
    let v = app.sim.as_ref().unwrap();
    assert_eq!(
        v.files_ui.confirm,
        Some(Act::OpenProject("mine".into())),
        "asked first"
    );
    assert_eq!(v.title(), "blink", "not opened yet");
    let at = app.chrome.rect("confirm no").unwrap().center();
    click(&ctx, &mut app, at, 0.4);
    assert_eq!(app.title(), "blink");
    assert!(app.sim.as_ref().unwrap().proj.dirty(), "the edits stay");
    let at = app.chrome.rect("project mine").unwrap().center();
    click(&ctx, &mut app, at, 0.5);
    let at = app.chrome.rect("confirm yes").unwrap().center();
    click(&ctx, &mut app, at, 0.6);
    assert_eq!(app.title(), "mine");
    let v = app.sim.as_ref().unwrap();
    assert!(!v.proj.dirty());
    assert!(
        v.session.running,
        "opened running (a program that sits still looks dead)"
    );
    assert_eq!(v.proj.running.as_deref(), Some("mine"));
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
        "PROGRAM other\nVAR\n  x : INT;\nEND_VAR\nx := 1;\nEND_PROGRAM\n".into();
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

fn l5x_fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../helmsman-iec/tests/fixtures/l5x/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// An L5X upload in the simulation becomes the same `.st` the box makes
/// (the same converter, `helmsman_iec::ladder`), and it compiles and runs
/// in the page; one outside the subset is refused whole, with its
/// reasons, and changes nothing.
#[test]
fn an_l5x_upload_becomes_the_boxs_st_or_is_refused_whole() {
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    v.files_ui.confirm = None;
    v.proj.create("plant", None);
    v.open_project("plant");
    let bytes = l5x_fixture("ladder.L5X");
    v.upload("Ladder.L5X", bytes.clone());
    let (msg, err) = v.proj.notice.clone().unwrap();
    assert!(!err, "{msg}");
    let box_st = helmsman_iec::ladder::convert(std::str::from_utf8(&bytes).unwrap())
        .expect("the box converts it")
        .st;
    assert_eq!(v.proj.files.get("Ladder.st"), Some(&box_st), "the box's .st");
    assert_eq!(v.proj.current, "Ladder.st");
    // MainTask is periodic every 10 ms: project.settings says so, as the
    // box's import writes it.
    let settings = v.proj.files.get("project.settings").cloned().unwrap_or_default();
    assert!(
        settings.lines().any(|l| l == "program MainTask scan_ms=10"),
        "{settings}"
    );
    assert!(msg.contains("task rates"), "{msg}");
    // It builds and runs in the page (the starter's main.st aside), its
    // inputs the ladder's.
    v.proj.delete_file("main.st");
    v.deploy();
    assert!(v.diags().is_empty(), "{:?}", v.diags());
    let inputs: Vec<String> = v.session.inputs().map(|p| p.name.clone()).collect();
    assert_eq!(inputs, ["START", "STOP", "PART_EYE"], "{inputs:?}");

    // An inventory export (instructions outside the subset): refused
    // whole, with every reason the box gives.
    let before = v.proj.files.clone();
    let inventory = l5x_fixture("line3.L5X");
    v.upload("line3.L5X", inventory.clone());
    let (msg, err) = v.proj.notice.clone().unwrap();
    assert!(err && msg.starts_with("line3.L5X is not converted: "), "{msg}");
    let refused = helmsman_iec::ladder::convert(std::str::from_utf8(&inventory).unwrap()).unwrap_err();
    for p in refused.problems.iter().take(20) {
        assert!(msg.contains(&p.to_string()), "{p} missing from {msg}");
    }
    assert_eq!(v.proj.files, before, "nothing imported");
}

/// Nick, 2026-11-05: the examples are the only built-in list (no
/// Machines, no Templates), and clicking one asks for a name and opens the
/// visitor's own copy, the example unchanged.
#[test]
fn clicking_an_example_names_and_opens_your_own_copy() {
    let ctx = egui::Context::default();
    let mut app = App::sim();
    app.chrome.show_files();
    frame(&ctx, &mut app, 0.1);
    for e in &crate::playground::EXAMPLES {
        let (name, _) = example_project(e.src);
        assert!(app.chrome.rect(&format!("project {name}")).is_some(), "{name}");
    }
    for gone in ["group Machines", "group Templates", "project batch-mixer"] {
        assert!(app.chrome.rect(gone).is_none(), "{gone}");
    }
    let at = app.chrome.rect("project counter").unwrap().center();
    click(&ctx, &mut app, at, 0.2);
    frame(&ctx, &mut app, 0.3);
    let v = app.sim.as_ref().unwrap();
    assert_eq!(
        v.files_ui.naming,
        Some(crate::projects::Naming::FromStarter("counter".into())),
        "a click asks for a name, it opens nothing"
    );
    assert_eq!(v.files_ui.input, "counter-2", "a free name, the example's taken");
    let at = app.chrome.rect("name ok").expect("the name popup").center();
    click(&ctx, &mut app, at, 0.4);
    frame(&ctx, &mut app, 0.5);
    let v = app.sim.as_mut().unwrap();
    assert_eq!(v.title(), "counter-2");
    assert!(!v.proj.is_example(), "the visitor's own");
    let src = crate::playground::EXAMPLES
        .iter()
        .find(|e| e.name == "Counter")
        .unwrap()
        .src;
    assert_eq!(v.proj.files.get("counter.st").map(String::as_str), Some(src));
    let rows = v.proj.rows();
    assert_eq!(rows[0].name, "counter-2");
    assert!(
        rows.iter().any(|r| r.name == "counter" && r.example()),
        "the example stays"
    );
}

/// Nick, 2026-11-01: one program runs in the page; opening or deploying
/// another stops the one before and the running mark moves with it.
#[test]
fn opening_or_deploying_another_program_stops_the_one_running() {
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    v.open_project("blink");
    assert!(v.session.running);
    assert_eq!(v.proj.running.as_deref(), Some("blink"));
    assert!(v.session.src.contains("PROGRAM blink"));
    v.open_project("latch");
    assert!(v.session.running, "the new one runs");
    assert_eq!(v.proj.running.as_deref(), Some("latch"), "and is the one marked");
    assert!(
        !v.session.src.contains("PROGRAM blink"),
        "blink is gone from the page"
    );
    let rows = v.proj.rows();
    assert_eq!(rows.iter().filter(|r| r.running).count(), 1, "{rows:?}");
    // A deploy that does not compile is refused: what ran keeps running.
    v.proj.create("broken", None);
    v.open_project("broken");
    assert_eq!(
        v.proj.running.as_deref(),
        Some("broken"),
        "a new project compiles and runs"
    );
    *v.proj.current_text_mut().unwrap() = "PROGRAM broken\nx :=;\nEND_PROGRAM\n".into();
    v.deploy();
    assert!(v.session.running, "the last good build still runs");
    assert!(!v.session.diags.is_empty(), "and the error is listed");
}

/// Nick, 2026-11-01: the Messages tab keeps what the page had to say.
#[test]
fn the_log_keeps_refused_deploys_faults_and_state_changes() {
    use crate::messages::Level;
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    v.open_project("blink");
    v.proj.create("broken", None);
    v.open_project("broken");
    *v.proj.current_text_mut().unwrap() = "PROGRAM broken\nx :=;\nEND_PROGRAM\n".into();
    v.deploy();
    let errors: Vec<String> = v
        .log
        .iter()
        .filter(|m| m.level == Level::Error)
        .map(|m| m.text.clone())
        .collect();
    assert!(
        errors.iter().any(|e| e.contains("Deploy of broken refused")),
        "{errors:?}"
    );
    assert!(v.log.errors() > 0, "{}", v.log.text());
    let before = v.log.len();
    let ctx = eframe::egui::Context::default();
    let input = eframe::egui::RawInput {
        time: Some(1.0),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| v.tick(ui.ctx()));
    out.textures_delta.clear();
    assert!(v.log.len() >= before, "watching never loses entries");
    // Stopping is reported once, not every frame.
    v.session.set_running(false);
    for k in 2..6 {
        let input = eframe::egui::RawInput {
            time: Some(f64::from(k)),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| v.tick(ui.ctx()));
        out.textures_delta.clear();
    }
    let stops = v.log.iter().filter(|m| m.text == "Stopped.").count();
    assert_eq!(
        stops,
        1,
        "{:?}",
        v.log.iter().map(|m| &m.text).collect::<Vec<_>>()
    );
}

/// Nick, 2026-11-01: "Use this code" makes a new, named file (a new
/// project for a PROGRAM), opens it, and asks first about unsaved edits.
#[test]
fn the_agents_answer_becomes_a_named_project_or_file_and_asks_about_edits() {
    use crate::playground_view::answer_name;
    assert_eq!(answer_name("PROGRAM Lamp_Test\nEND_PROGRAM"), "lamp_test");
    assert_eq!(
        answer_name("(* x *) FUNCTION_BLOCK Debounce VAR END_VAR END_FUNCTION_BLOCK"),
        "debounce"
    );
    assert_eq!(answer_name("x := 1;"), "agent");
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    v.open_project("blink");
    assert!(!v.proj.dirty());
    let code = "PROGRAM lamp_test\nVAR\n  lamp : BOOL;\nEND_VAR\nlamp := TRUE;\nEND_PROGRAM\n";
    v.take_code(code.into());
    // The conversation's first PROGRAM asks for its name (agent-iterations).
    assert_eq!(v.proj.open, "blink", "nothing lands before the name");
    v.confirm_program_name();
    assert_eq!(v.proj.open, "lamp_test", "a PROGRAM is a project of its own");
    assert_eq!(v.proj.current, "main.st");
    assert!(v.proj.current_text_mut().unwrap().contains("PROGRAM lamp_test"));
    assert!(!v.proj.dirty(), "saved");
    assert!(v.session.diags.is_empty(), "{:?}", v.session.diag_lines());
    // Again: the same project, the earlier one kept as v1.st.
    v.take_code(code.into());
    assert_eq!(v.proj.open, "lamp_test");
    assert!(v.proj.files.contains_key("v1.st"));
    // A block goes into the open project as a file.
    let fb = "FUNCTION_BLOCK debounce\nVAR_INPUT\n  x : BOOL;\nEND_VAR\nVAR_OUTPUT\n  q : BOOL;\nEND_VAR\nq := x;\nEND_FUNCTION_BLOCK\n";
    v.take_code(fb.into());
    assert_eq!(v.proj.open, "lamp_test");
    assert_eq!(v.proj.current, "debounce.st");
    // Unsaved edits ask first; "save, then open" keeps them.
    v.proj.open_file("main.st");
    v.proj.current_text_mut().unwrap().push_str("(* mine *)");
    assert!(v.proj.dirty());
    v.take_code(code.into());
    assert!(
        matches!(v.files_ui.confirm, Some(Act::UseAnswer { ref name, .. }) if name == "lamp_test"),
        "asked"
    );
    assert!(!v.proj.files.contains_key("v2.st"), "nothing landed yet");
}

/// A storage with a backup slot beside the main one, both readable after
/// the projects took them.
#[derive(Clone, Default)]
struct Backed {
    main: Rc<RefCell<Option<String>>>,
    backup: Rc<RefCell<Option<String>>>,
}

impl Storage for Backed {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.main.borrow().clone())
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        *self.main.borrow_mut() = Some(text.to_string());
        Ok(())
    }

    fn backup(&mut self, text: &str) -> Result<(), String> {
        *self.backup.borrow_mut() = Some(text.to_string());
        Ok(())
    }
}

/// 2026-11-08 review: what the browser stored in a format this page does
/// not read (a newer page's, or one bad top-level field) is not written
/// over by the first project opened. It is kept as a backup and left alone
/// until the person saves, and the same for a store whose projects broke a
/// rule and were left out.
#[test]
fn a_store_this_page_cannot_read_is_backed_up_and_left_alone_until_a_save() {
    let ok = "PROGRAM fine\nEND_PROGRAM\n";
    let newer = serde_json::json!({
        "format": "helmsman-projects 2",
        "open": "mine",
        "projects": { "mine": { "main.st": ok } },
    })
    .to_string();
    let partly = serde_json::json!({
        "format": "helmsman-projects 1",
        "open": "mine",
        "projects": {
            "mine": { "main.st": ok },
            "broken": { "main.st": ok, "../x.st": "y" },
        },
    })
    .to_string();
    for raw in [newer, partly, "{not json".to_string()] {
        let store = Backed::default();
        *store.main.borrow_mut() = Some(raw.clone());
        let mut s = SimProjects::load(Box::new(store.clone()));
        assert!(s.storage_problem.is_some(), "said: {raw}");
        s.open_project(&example_project(crate::playground::EXAMPLES[1].src).0);
        assert_eq!(
            store.main.borrow().as_deref(),
            Some(raw.as_str()),
            "opening a project writes nothing: {raw}"
        );
        assert_eq!(store.backup.borrow().as_deref(), Some(raw.as_str()), "backed up");
        // The person saves: now the store is written, the backup stays.
        s.create("fresh", None);
        let now = store.main.borrow().clone().unwrap();
        assert_ne!(now, raw);
        assert!(now.contains("\"fresh\""), "{now}");
        assert_eq!(
            store.backup.borrow().as_deref(),
            Some(raw.as_str()),
            "backup kept"
        );
    }
}

/// A project's revisions go with its name, and a deleted project's do not
/// come back under a new project of the same name.
#[test]
fn renaming_keeps_a_projects_history_and_deleting_drops_it() {
    let store = Shared::default();
    let mut s = SimProjects::load(Box::new(store.clone()));
    s.create("a", None);
    s.current_text_mut().unwrap().push_str("(* one *)\n");
    s.save();
    s.current_text_mut().unwrap().push_str("(* two *)\n");
    s.save();
    assert_eq!(s.revisions().len(), 2);
    s.rename("a", "b");
    assert_eq!(s.open, "b");
    assert_eq!(s.revisions().len(), 2, "the revisions went with it");
    assert!(!s.history.contains_key("a"));
    let again = SimProjects::load(Box::new(store.clone()));
    assert_eq!(
        again.history["b"].len(),
        2,
        "and survive the next persist and reload"
    );
    s.delete("b");
    assert!(!s.history.contains_key("b"), "gone with the project");
    s.create("b", None);
    s.save();
    assert_eq!(
        s.revisions().iter().map(|r| r.n).collect::<Vec<_>>(),
        [1],
        "a new project of that name starts its own history"
    );
}

/// The box refuses a file name that differs only by case (409) and an
/// import onto a name that is taken, offering Replace; so does the
/// simulation.
#[test]
fn files_follow_the_boxs_rules_for_case_and_for_imports_onto_a_taken_name() {
    let mut s = SimProjects::load(Box::new(Shared::default()));
    s.create("p", None);
    s.add_file("blocks.st", "(* blocks *)\n".into());
    s.rename_file("blocks.st", "MAIN.st");
    assert!(s.files.contains_key("blocks.st") && !s.files.contains_key("MAIN.st"));
    assert!(s.notice.as_ref().unwrap().1, "{:?}", s.notice);

    // The same name: refused and offered, not overwritten.
    let before = s.files.clone();
    s.upload("blocks.st", b"(* imported *)\n");
    assert_eq!(s.files, before, "nothing overwritten");
    let (msg, err) = s.notice.clone().unwrap();
    assert!(err && msg.contains("already has that file"), "{msg}");
    assert!(s.pending_file.is_some());
    s.replace();
    assert_eq!(s.files["blocks.st"], "(* imported *)\n");
    assert!(s.pending_file.is_none());

    // A name that differs only by case is refused and not offered.
    s.upload("BLOCKS.st", b"(* other *)\n");
    assert!(!s.files.contains_key("BLOCKS.st"));
    assert!(s.pending_file.is_none());
    assert!(s.notice.as_ref().unwrap().1);
}

/// 2026-11-10 (card `project-unknown-files`): a stored project whose
/// `diagram.layout` does not read, or that holds a file this page has no use
/// for (a newer release's), is not dropped on load, and neither is a
/// revision with either. Only a save or an upload refuses a bad layout.
#[test]
fn a_stored_project_and_its_history_survive_a_layout_that_does_not_read_and_an_unknown_file() {
    let ok = "PROGRAM fine\nEND_PROGRAM\n";
    let files = serde_json::json!({
        "main.st": ok,
        "diagram.layout": "helmsman-layout 9\nwho knows\n",
        "pump-curves.json": "{\r\n  \"curve\": [0, 1]\r\n}\r\n",
    });
    let rev = |n: u32| {
        serde_json::json!({
            "n": n, "at_ms": 1, "by": "you", "what": "saved", "page": null, "files": files,
        })
    };
    let raw = serde_json::json!({
        "format": "helmsman-projects 1",
        "open": "mine",
        "projects": { "mine": files },
        "history": { "mine": [rev(1), rev(2)] },
    })
    .to_string();
    let store = Backed::default();
    *store.main.borrow_mut() = Some(raw);
    let s = SimProjects::load(Box::new(store.clone()));
    assert!(s.storage_problem.is_none(), "{:?}", s.storage_problem);
    assert!(store.backup.borrow().is_none(), "nothing was left out");
    let kept = s.saved.get("mine").expect("the project is kept");
    assert_eq!(
        kept.get("pump-curves.json"),
        Some("{\r\n  \"curve\": [0, 1]\r\n}\r\n")
    );
    assert_eq!(kept.get("diagram.layout"), Some("helmsman-layout 9\nwho knows\n"));
    assert_eq!(s.history["mine"].len(), 2, "both revisions kept");

    // A name that is not safe is still left out, as before.
    let unsafe_name = serde_json::json!({
        "format": "helmsman-projects 1",
        "projects": { "mine": { "main.st": ok, "../x": "y" } },
    })
    .to_string();
    let store = Backed::default();
    *store.main.borrow_mut() = Some(unsafe_name);
    let s = SimProjects::load(Box::new(store));
    assert!(s.storage_problem.is_some() && !s.saved.contains_key("mine"));
}

// ---- Structured Text is formatted on Save and on Deploy (Nick, 2026-11-11) ----

/// A valid program in no particular layout: lower-case keywords, whole
/// sections on one line, no final line end.
const MESSY: &str = "PROGRAM lamp\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nif start then lamp := TRUE; else lamp := FALSE; end_if;\nEND_PROGRAM";

/// A program in a layout of its own, which a save must not touch unless
/// its file was edited.
const OTHER: &str = "PROGRAM other\nVAR x : INT; END_VAR\nx := 1;\nEND_PROGRAM\n";

/// A project `name` saved as it is given, with no formatting (`save_as`
/// does none), so what a test calls untouched is untouched.
fn seeded(name: &str, files: &[(&str, &str)]) -> SimProjects {
    let mut p = SimProjects::load(Box::new(Shared::default()));
    p.create(name, None);
    p.files.clear();
    for (n, t) in files {
        p.files.insert((*n).to_string(), (*t).to_string());
    }
    p.save_as("seeded", None);
    assert!(!p.dirty(), "{:?}", p.notice);
    p
}

/// Nick, 2026-11-11: Save puts an edited `.st` file in the one layout. A
/// `.st` file with no edits and a file that is not Structured Text
/// (io.map, edited or not) are kept byte for byte, and saving again
/// changes nothing.
#[test]
fn save_formats_an_edited_st_file_and_leaves_the_others_byte_for_byte() {
    let map = "in   start   bool   box   di:0\n";
    let mut p = seeded(
        "fmt",
        &[
            ("main.st", OTHER),
            ("other.st", OTHER),
            ("io.map", "in start bool box di:0\n"),
        ],
    );
    // main.st and io.map are edited; other.st is not touched.
    p.files.insert("main.st".into(), MESSY.into());
    p.files.insert("io.map".into(), map.into());
    let tidy = helmsman_iec::tidy::format(MESSY);
    assert_ne!(tidy, MESSY, "the fixture needs formatting");
    assert!(tidy.contains("IF start THEN"), "keywords in capitals: {tidy}");

    p.save();
    assert!(!p.dirty(), "{:?}", p.notice);
    let stored = |p: &SimProjects, f: &str| p.saved["fmt"].0[f].clone();
    assert_eq!(stored(&p, "main.st"), tidy, "the edited file is stored formatted");
    assert_eq!(p.files["main.st"], tidy, "and the editor shows what was stored");
    assert_eq!(
        stored(&p, "io.map"),
        map,
        "an edited io.map is not Structured Text"
    );
    assert_eq!(stored(&p, "other.st"), OTHER, "untouched, so not rewritten");

    // Saving again: nothing is edited, nothing changes.
    let before = p.saved["fmt"].0.clone();
    assert_eq!(p.format(), 0);
    p.save();
    assert_eq!(p.saved["fmt"].0, before);
    assert_eq!(p.files, before);
}

/// Nick, 2026-11-11: a file that does not compile is still saved: laid
/// out as far as it goes, and every character that is not white space
/// (code and comments) survives, keywords apart from their capitals.
#[test]
fn a_file_that_does_not_compile_is_still_saved_with_everything_in_it() {
    let broken = "(* keep me *)\nPROGRAM p\nVAR x : INT; END_VAR\nIF x > 1 THEN x := 2;\n(* and me *) x := x +;\nEND_PROGRAM";
    let mut p = seeded("broken", &[("main.st", OTHER)]);
    p.files.insert("main.st".into(), broken.into());
    p.save();
    assert!(!p.dirty(), "{:?}", p.notice);
    let stored = p.saved["broken"].0["main.st"].clone();
    let bare = |t: &str| {
        t.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    assert_eq!(bare(&stored), bare(broken), "nothing lost: {stored}");
    assert_ne!(stored, broken, "and its layout was tidied: {stored}");
    assert!(stored.ends_with('\n'), "{stored:?}");
    assert_eq!(p.files["main.st"], stored);
}

/// Nick, 2026-11-11: Deploy in the page formats the edited file first, so
/// the program that runs is the formatted text.
#[test]
fn deploy_in_the_page_formats_the_edited_file_and_runs_the_formatted_text() {
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    v.proj.create("run", None);
    v.open_project("run");
    v.proj.open_file("main.st");
    *v.proj.current_text_mut().unwrap() = MESSY.into();
    let tidy = helmsman_iec::tidy::format(MESSY);
    v.deploy();
    assert!(v.session.running, "{:?}", v.notice);
    assert_eq!(v.proj.files["main.st"], tidy, "the file is the formatted text");
    assert_eq!(
        v.session.build.as_ref().expect("a program").src,
        tidy,
        "and that is what was built and runs"
    );
    assert_eq!(v.session.src, tidy, "the editor's source follows");
    // The edits stay edits until Save: Deploy does not save.
    assert!(v.proj.dirty());
}
