//! Every release opens every older project (TODO.md card
//! `project-compat-fixtures`). Project files carry no version of their
//! own (only `diagram.layout`'s `helmsman-layout 1`), so the only thing
//! that pins "a project an older release wrote still loads" is a project
//! that release wrote, kept frozen and loaded by this build.
//!
//! - **`tests/fixtures/projects/<release>/<project>/`** holds one
//!   representative project per release: ST modules and shared blocks,
//!   `io.map`, `project.settings`, `diagram.layout`, `icons.map`,
//!   `agent.md`, an earlier iteration (`vN.st`) and requirement tests
//!   (`test_NAME.st`, one green and one untested). Each is laid down as an
//!   older box left it (files in a folder under its projects root) and
//!   must, on the current build: open (every file read back as written),
//!   pass the file rules, build, run scans in the simulation with no
//!   fault and no module held, and keep its requirement tests green.
//! - **Adding a release.** When a release is tagged, copy the newest
//!   fixture folder to `tests/fixtures/projects/<tag>/` from the tagged
//!   tree (`git archive <tag> crates/helmsman-plc/tests/fixtures/projects`
//!   or the project as that release's box downloads it), and extend it
//!   with any file kind or construct the release added. Never edit an
//!   older release's folder: it is what that release wrote. The first
//!   folder, `unreleased-20261107`, is the format at dev d12a152 (no tag
//!   existed yet); the pump station comes from
//!   crates/helmsman-canvas/src/fixtures/pump-station.
//! - **Drift.** A construct a newer release no longer accepts must not
//!   lock the project out: it opens, and the build lists each problem with
//!   its file and line ([`a_project_with_rejected_constructs_still_opens_and_lists_them`],
//!   fixtures in `tests/fixtures/project-drift/`).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use helmsman_iec::ImageSizes;
use helmsman_iec::spec::{self, Outcome};
use helmsman_io::{Dir, Ty, Value};
use helmsman_plc::api::swap::compile_set;
use helmsman_plc::project::build::{self, Built, Diag, Shape};
use helmsman_plc::project::{FileKind, Files, Projects, file_name};
use helmsman_plc::spec::Defaults;
use helmsman_plc::{Runtime, SignalTable, Status, Window};

const I_LEN: usize = 32;
const Q_LEN: usize = 32;
const PERIOD: Duration = Duration::from_millis(10);
/// Scans each fixture runs in the simulation: 3 s of simulated time.
const SCANS: u64 = 300;

fn fixtures(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(dir)
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn shape() -> Shape {
    Shape {
        defaults: defaults(),
        i_len: I_LEN,
        q_len: Q_LEN,
    }
}

fn defaults() -> Defaults {
    Defaults {
        period: PERIOD,
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 256,
    }
}

/// A fresh projects root holding a copy of the fixture folder `src` as
/// project `name`, as an older box left it on disk; and the store on it.
fn lay_down(src: &Path, name: &str) -> (Projects, PathBuf) {
    static N: AtomicU64 = AtomicU64::new(0);
    let state = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "project-compat-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&state);
    let dest = state.join("projects").join(name);
    fs::create_dir_all(&dest).unwrap();
    for e in fs::read_dir(src).unwrap() {
        let p = e.unwrap().path();
        assert!(p.is_file(), "{}: a fixture holds files only", p.display());
        fs::copy(&p, dest.join(p.file_name().unwrap())).unwrap();
    }
    (Projects::in_state_dir(&state).unwrap(), state)
}

/// The fixture's files as they are in the repo, by name.
fn on_disk(src: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = fs::read_dir(src)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            let n = p.file_name().unwrap().to_str().unwrap().to_string();
            (n, fs::read_to_string(&p).unwrap())
        })
        .collect();
    v.sort();
    v
}

fn zero(ty: Ty) -> Value {
    match ty {
        Ty::Bool => Value::Bool(false),
        Ty::Int => Value::Int(0),
        Ty::Real => Value::Real(0.0),
    }
}

/// `built` run in the simulation: every input read as zero, `SCANS`
/// ticks on a fake clock. Every module must scan each tick it is due,
/// fault never, be held never, and every output signal a module drives
/// must end with a command.
fn simulate(what: &str, files: &Files, built: &Built) {
    let modules = compile_set(&built.change, &defaults()).unwrap_or_else(|e| panic!("{what}: {e:?}"));
    let names: Vec<String> = modules.iter().map(|m| m.name.clone()).collect();
    let mut table = match files.get(helmsman_plc::project::IO_MAP) {
        Some(map) => {
            let parsed = helmsman_io::text::parse(map).unwrap_or_else(|e| panic!("{what}: io.map: {e:?}"));
            SignalTable::new(&parsed.map, &parsed.safe).unwrap_or_else(|e| panic!("{what}: {e:?}"))
        }
        None => SignalTable::default(),
    };
    for i in 0..table.len() {
        if let Some((Dir::In, ty)) = table.signal(i)
            && table.source(i).is_none()
        {
            table.set_input(i, Some(zero(ty)));
        }
    }
    let mut rt =
        Runtime::with_signals(I_LEN, Q_LEN, modules, &table).unwrap_or_else(|e| panic!("{what}: {e:?}"));
    let inputs = [0u8; I_LEN];
    for k in 0..SCANS {
        let now = PERIOD * u32::try_from(k).unwrap();
        let faults = rt.tick_with(now, &inputs, &mut table);
        assert!(faults.is_empty(), "{what}: scan {k} faulted: {faults:?}");
    }
    for n in &names {
        assert_eq!(rt.status(n), Some(Status::Running), "{what}: module {n}");
        let scans = rt.stats(n).unwrap().scans;
        let (period, _, _) = rt.schedule(n).unwrap();
        let due = u64::try_from(PERIOD.as_nanos() * u128::from(SCANS) / period.as_nanos()).unwrap();
        assert!(
            scans >= due.min(SCANS) && scans > 0,
            "{what}: module {n} scanned {scans} times"
        );
    }
    for i in 0..table.len() {
        if table.writer(i).is_some() {
            assert!(
                table.command(i).is_some(),
                "{what}: {} is driven but has no command",
                table.name(i).unwrap_or("?")
            );
        }
    }
}

#[test]
fn every_release_fixture_opens_builds_runs_and_passes_its_requirements() {
    let releases = subdirs(&fixtures("projects"));
    assert!(!releases.is_empty(), "no release fixtures");
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    let mut checked = 0;
    for release in &releases {
        let projects = subdirs(release);
        assert!(!projects.is_empty(), "{}: no project", release.display());
        for src in projects {
            let name = src.file_name().unwrap().to_str().unwrap().to_string();
            let what = format!("{}/{name}", release.file_name().unwrap().to_str().unwrap());
            let (store, state) = lay_down(&src, &name);

            // Opens: every file the release wrote reads back as it was.
            assert_eq!(store.list().unwrap(), std::slice::from_ref(&name), "{what}");
            let files = store
                .read(&name)
                .unwrap_or_else(|r| panic!("{what}: does not open: {r}"));
            let read: Vec<(String, String)> = files.0.clone().into_iter().collect();
            assert_eq!(
                read,
                on_disk(&src),
                "{what}: a file was dropped or changed on read"
            );
            for n in files.0.keys() {
                let kind = file_name(n).unwrap();
                kinds.insert(if kind == FileKind::St && spec::is_test(n) {
                    "test".into()
                } else {
                    format!("{kind:?}")
                });
            }

            // Checks and builds, with no problem listed.
            files.check().unwrap_or_else(|e| panic!("{what}: {e:?}"));
            let built = build::build(&name, &files, None, &shape()).unwrap_or_else(|d| {
                panic!(
                    "{what}: does not build:\n{}",
                    d.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
                )
            });
            assert!(!built.modules.is_empty(), "{what}: builds no module");

            simulate(&what, &files, &built);

            // Its requirement tests: none red, at least one green.
            let plan = spec::plan(files.st());
            let settings = spec::Settings::new(ImageSizes {
                i: I_LEN,
                q: Q_LEN,
                m: 256,
            })
            .period(PERIOD);
            let verdicts = spec::run_all(&plan, settings);
            assert!(spec::red(&verdicts).is_empty(), "{what}: {verdicts:#?}");
            assert!(
                verdicts.iter().any(spec::Verdict::passed),
                "{what}: no green test"
            );
            // The same again: a fixture's run is deterministic.
            assert_eq!(spec::run_all(&plan, settings), verdicts, "{what}");

            let _ = fs::remove_dir_all(state);
            checked += 1;
        }
    }
    assert!(checked > 0);
    // Between them the fixtures cover every kind of file a project holds
    // but a device's EDS (which needs a networked device in the map).
    for kind in [
        "St",
        "IoMap",
        "Settings",
        "Layout",
        "Icons",
        "Agent",
        "Iteration",
        "test",
    ] {
        assert!(kinds.contains(kind), "no fixture holds a {kind} file: {kinds:?}");
    }
}

fn problems(diags: &[Diag]) -> Vec<(String, u32)> {
    diags.iter().map(|d| (d.file.clone(), d.line)).collect()
}

#[test]
fn a_project_with_rejected_constructs_still_opens_and_lists_them() {
    // Code, settings and map lines this build refuses: each is a problem
    // on its own file and line, all listed at once, and the project opens.
    let src = fixtures("project-drift/rejected-code");
    let (store, state) = lay_down(&src, "rejected-code");
    let files = store.read("rejected-code").expect("the project opens");
    assert_eq!(
        files.0.clone().into_iter().collect::<Vec<_>>(),
        on_disk(&src),
        "every file is read, the refused ones too"
    );
    let diags = build::build("rejected-code", &files, None, &shape()).expect_err("does not build");
    let at = problems(&diags);
    for want in [("io.map", 3), ("main.st", 4), ("project.settings", 2)] {
        assert!(
            at.iter().any(|(f, l)| (f.as_str(), *l) == want),
            "{want:?} not listed: {diags:#?}"
        );
    }
    assert!(diags.iter().all(|d| !d.message.is_empty()));
    // Its tests run and say why they are red, naming the program.
    let v = spec::run_all(
        &spec::plan(files.st()),
        spec::Settings::new(ImageSizes { i: 32, q: 32, m: 256 }),
    );
    assert_eq!(v.len(), 1);
    assert!(
        matches!(v[0].outcome, Outcome::ProgramDoesNotCompile(_)),
        "{v:#?}"
    );
    assert_eq!(spec::red(&v).len(), 1);
    let _ = fs::remove_dir_all(state);

    // A diagram arrangement from another layout version: listed by file,
    // the project opens, and the program's tests still run green (the
    // arrangement is drawing only).
    let src = fixtures("project-drift/other-layout");
    let (store, state) = lay_down(&src, "other-layout");
    let files = store.read("other-layout").expect("the project opens");
    assert!(files.get(helmsman_plc::project::LAYOUT).is_some());
    let built = build::build("other-layout", &files, None, &shape()).expect("builds anyway");
    assert_eq!(built.problems.len(), 1, "{:#?}", built.problems);
    assert!(
        built.problems[0].file == "diagram.layout" && built.problems[0].message.contains("helmsman-layout 2"),
        "{:#?}",
        built.problems
    );
    let v = spec::run_all(
        &spec::plan(files.st()),
        spec::Settings::new(ImageSizes { i: 32, q: 32, m: 256 }),
    );
    assert!(v.iter().all(spec::Verdict::passed) && !v.is_empty(), "{v:#?}");
    let _ = fs::remove_dir_all(state);
}

/// A `diagram.layout` this build cannot read costs the drawing, not the
/// deploy: the build never reads the file, so the project builds exactly
/// as it does without one, and the layout is listed as a problem on its
/// own file and line.
#[test]
fn a_layout_that_does_not_read_is_listed_and_the_project_still_builds() {
    let src = fixtures("project-drift/other-layout");
    let (store, state) = lay_down(&src, "other-layout");
    let mut files = store.read("other-layout").expect("the project opens");
    let _ = fs::remove_dir_all(state);
    files.0.remove(helmsman_plc::project::LAYOUT);
    let automatic = build::build("p", &files, None, &shape()).expect("builds without a layout");

    let header = "helmsman-layout 1\n";
    let cases: [(&str, String, u32); 5] = [
        ("garbage", "this is not a layout\n".into(), 1),
        ("truncated header", "helmsman-lay".into(), 1),
        ("truncated move", format!("{header}move\tconveyor\t0\t0"), 2),
        (
            "unknown key",
            format!("{header}# kept\nshift\tconveyor\t0\t0\t1\ta\tb\n"),
            3,
        ),
        (
            "bad number",
            format!("{header}move\tconveyor\t0\tx\t1\ta\tb\n"),
            2,
        ),
    ];
    for (what, text, line) in cases {
        files.0.insert(helmsman_plc::project::LAYOUT.into(), text);
        let built = build::build("p", &files, None, &shape())
            .unwrap_or_else(|d| panic!("{what}: a bad layout stopped the build: {d:#?}"));
        assert_eq!(
            built.change, automatic.change,
            "{what}: the layout changed the build"
        );
        assert_eq!(built.modules, automatic.modules, "{what}");
        assert_eq!(built.problems.len(), 1, "{what}: {:#?}", built.problems);
        let p = &built.problems[0];
        assert_eq!(
            (p.file.as_str(), p.line),
            ("diagram.layout", line),
            "{what}: {p:?}"
        );
        assert!(
            p.message.contains("automatic layout"),
            "{what}: the message says what is used instead: {p:?}"
        );
    }

    // A layout that reads lists nothing.
    files.0.insert(
        helmsman_plc::project::LAYOUT.into(),
        format!("{header}move\tconveyor\t0\t0\t120\tLAMP :=\tLAMP := START\n"),
    );
    let built = build::build("p", &files, None, &shape()).unwrap();
    assert!(built.problems.is_empty(), "{:#?}", built.problems);
    assert_eq!(built.change, automatic.change);
}

/// A file a newer release wrote, whose name this build has no use for
/// (card `project-unknown-files`): the project opens with it, the build is
/// the build of the project without it and lists it as a problem on its own
/// file, and writing the project back, or round-tripping it through a
/// `.zip`, keeps it byte for byte. A newer release's file survives a trip
/// through an older box.
#[test]
fn a_file_this_build_has_no_use_for_is_listed_and_kept() {
    let src = fixtures("project-drift/newer-release-file");
    let (store, state) = lay_down(&src, "newer");
    let disk = on_disk(&src);
    let kept = ["pump-curves.json", "recipes.v2"];
    for k in kept {
        assert!(disk.iter().any(|(n, _)| n == k), "fixture lacks {k}");
    }

    let files = store.read("newer").expect("the project opens");
    assert_eq!(
        files.0.clone().into_iter().collect::<Vec<_>>(),
        disk,
        "an unrecognised file is read, not skipped"
    );
    files.check().expect("it passes the file rules");

    let mut known = files.clone();
    for k in kept {
        known.0.remove(k);
    }
    let plain = build::build("newer", &known, None, &shape()).expect("builds without them");
    assert!(plain.problems.is_empty(), "{:#?}", plain.problems);
    let built = build::build("newer", &files, None, &shape()).expect("builds with them");
    assert_eq!(built.change, plain.change, "a kept file changes nothing built");
    assert_eq!(built.modules, plain.modules);
    let mut listed: Vec<&str> = built.problems.iter().map(|p| p.file.as_str()).collect();
    listed.sort_unstable();
    assert_eq!(listed, kept, "{:#?}", built.problems);
    for p in &built.problems {
        assert!(
            p.message.contains("not used by this release") && p.message.contains("kept"),
            "{p:?}"
        );
    }
    simulate("newer", &files, &built);

    // Written back over the project, and through a .zip, it is all still there.
    store.install("newer", &files, true).expect("installs whole");
    assert_eq!(
        store.read("newer").unwrap().0.into_iter().collect::<Vec<_>>(),
        disk,
        "a replace keeps the file byte for byte"
    );
    for (n, text) in &disk {
        let path = state.join("projects/newer").join(n);
        assert_eq!(fs::read(path).unwrap(), text.as_bytes(), "{n}");
    }
    let zip = files.to_zip().unwrap();
    assert_eq!(
        helmsman_plc::project::from_zip(helmsman_plc::project::zip::read(&zip).unwrap()).unwrap(),
        files
    );
    let _ = fs::remove_dir_all(state);
}
