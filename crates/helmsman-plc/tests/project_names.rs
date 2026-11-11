//! What a project may name a file, held to the same answer by every way a
//! project comes in (card `project-unknown-files`): a folder on the box
//! (`read_folder`), a `.zip` (`from_zip`) and the store's install (what the
//! API's upload and create end in). A name this build has no use for is
//! kept if it is safe, and refused if it is not, by all three alike; a name
//! that is kept comes back byte for byte from each.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use helmsman_plc::project::{self, FileKind, Files, Projects, file_name, read_folder};
use proptest::prelude::*;

const MAIN: &str = "PROGRAM p\nEND_PROGRAM\n";

fn scratch() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "project-names-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Text every kind of file accepts (a layout must read as one).
fn content(name: &str) -> String {
    if file_name(name) == Some(FileKind::Layout) {
        "helmsman-layout 1\n# ü\r\n".into()
    } else {
        "{\r\n\t\"ü\": [1, 2]\r\n}\r\n".into()
    }
}

/// What each entry point made of one file `name` beside `main.st`: the text
/// it holds for `name`, or `None` when it was refused or left out.
struct Outcome {
    folder: Option<String>,
    zip: Option<String>,
    install: Option<String>,
    on_disk: Option<Vec<u8>>,
}

fn run(name: &str) -> Outcome {
    let text = content(name);
    let tmp = scratch();

    // A folder as an older or newer box left it. The file system cannot
    // hold a name with a separator or a NUL: those never reach this reader.
    let folder = if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\0']) {
        None
    } else {
        let dir = tmp.join("folder");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), &text).unwrap();
        read_folder(&dir).ok().and_then(|f| f.0.get(name).cloned())
    };

    // An archive, `main.st` beside it so no folder is stripped as a prefix.
    let entries = vec![
        ("main.st".to_string(), MAIN.as_bytes().to_vec()),
        (name.to_string(), text.clone().into_bytes()),
    ];
    let zip = project::zip::write(&entries)
        .ok()
        .and_then(|z| project::zip::read(&z).ok())
        .and_then(|e| project::from_zip(e).ok())
        .and_then(|f| f.0.get(name).cloned());

    // The store, which the API installs through.
    let store = Projects::in_state_dir(&tmp.join("state")).unwrap();
    let files = Files(
        [("main.st", MAIN), (name, text.as_str())]
            .into_iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect(),
    );
    let install = store
        .install("p", &files, false)
        .ok()
        .and_then(|()| store.read("p").ok())
        .and_then(|f| f.0.get(name).cloned());
    let on_disk = install
        .as_ref()
        .and_then(|_| fs::read(tmp.join("state/projects/p").join(name)).ok());
    let _ = fs::remove_dir_all(&tmp);
    Outcome {
        folder,
        zip,
        install,
        on_disk,
    }
}

/// The answer must be the same everywhere, and a kept file exact.
fn agree(name: &str) -> Result<bool, String> {
    let o = run(name);
    let text = content(name);
    let folder_cannot = name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\0']);
    let accepted = o.install.is_some();
    if o.zip.is_some() != accepted {
        return Err(format!(
            "{name:?}: zip says {:?}, install {accepted}",
            o.zip.is_some()
        ));
    }
    if !folder_cannot && o.folder.is_some() != accepted {
        return Err(format!(
            "{name:?}: folder says {}, install {accepted}",
            o.folder.is_some()
        ));
    }
    if accepted {
        for (what, got) in [("zip", &o.zip), ("install", &o.install), ("folder", &o.folder)] {
            if what == "folder" && folder_cannot {
                continue;
            }
            if got.as_deref() != Some(text.as_str()) {
                return Err(format!("{name:?}: {what} changed the bytes: {got:?}"));
            }
        }
        if o.on_disk.as_deref() != Some(text.as_bytes()) {
            return Err(format!("{name:?}: the file on disk differs"));
        }
    }
    Ok(accepted)
}

fn arb_name() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[A-Za-z0-9_.-]{0,70}",
        2 => "[ -~]{0,20}",
        1 => "\\PC{0,12}",
        2 => "[a-z]{1,6}\\.(st|eds|wasm|ST|EDS|map|settings|layout|md|json|txt|v2)",
        1 => "(io|project|diagram|icons|agent|revision|page|drivers|con|nul|com1|lpt9|aux|prn)(\\.[a-zA-Z]{1,8})?",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(160))]

    #[test]
    fn every_entry_point_answers_a_name_alike_and_a_kept_file_comes_back_exact(name in arb_name()) {
        agree(&name).map_err(TestCaseError::fail)?;
    }
}

#[test]
fn names_that_are_safe_are_kept_and_names_that_are_not_are_refused_everywhere() {
    let kept = [
        "notes.txt",
        "pump-curves.json",
        "recipes.v2",
        "run.sh",
        "README",
        "Makefile",
        "a",
        "x.y.z",
        "v1.stx",
        "data_2026-11-10.csv",
        &format!("{}.csv", "n".repeat(60)),
        &"n".repeat(64),
    ];
    for name in kept {
        assert_eq!(agree(name), Ok(true), "{name:?} should be kept");
    }
    let refused = [
        "",
        ".",
        "..",
        "../x",
        "../../etc/passwd",
        "a/b",
        "/etc/passwd",
        "a\\b",
        "C:evil",
        "a\0b",
        ".hidden",
        ".env",
        ".save-1-2",
        "trailing.",
        "a..b",
        "a b",
        "tab\t",
        "new\nline",
        "\u{1}x",
        "bidi\u{202E}.txt",
        "é.txt",
        "%2e%2e",
        "CON",
        "nul.txt",
        "Com1",
        "lpt9.log",
        "Aux.log",
        "revision.json",
        "Revision.JSON",
        "page.md",
        "PAGE.MD",
        "drivers",
        "Drivers",
        "IO.MAP",
        "Io.Map",
        "Project.Settings",
        "DIAGRAM.LAYOUT",
        "Icons.map",
        "Agent.md",
        "x.ST",
        "main.St",
        "x.EDS",
        "E1213.eds",
        "a.b.eds",
        "x.wasm",
        "x.WASM",
        "V1.ST",
        &"n".repeat(65),
        &format!("{}.st", "n".repeat(65)),
    ];
    for name in refused {
        assert_eq!(agree(name), Ok(false), "{name:?} should be refused");
    }
}

#[test]
fn names_that_differ_only_by_case_are_refused_together() {
    // Two names a case-folding file system would merge, kept or known.
    for (a, b) in [
        ("Notes.txt", "notes.txt"),
        ("MAIN.st", "main.st"),
        ("README", "readme"),
    ] {
        let files = Files(
            [
                (a.to_string(), "x".to_string()),
                (b.to_string(), "y".to_string()),
                ("p.st".to_string(), MAIN.to_string()),
            ]
            .into(),
        );
        let e = files.check().expect_err("twins");
        assert!(e.iter().any(|m| m.contains("only by case")), "{e:?}");
        let tmp = scratch();
        let z = project::zip::write(
            &files
                .0
                .iter()
                .map(|(n, t)| (n.clone(), t.clone().into_bytes()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let from_zip = project::from_zip(project::zip::read(&z).unwrap());
        assert!(from_zip.is_err(), "{a} / {b}: {from_zip:?}");
        let store = Projects::in_state_dir(&tmp).unwrap();
        assert!(store.install("p", &files, false).is_err(), "{a} / {b}");
        let _ = fs::remove_dir_all(&tmp);
    }
}

/// A folder holding things that are not files, or not text, or too many:
/// the reader leaves them out and the project still opens.
#[test]
fn a_folder_with_odd_entries_still_opens_and_keeps_only_clean_files() {
    let dir = scratch();
    fs::write(dir.join("main.st"), MAIN).unwrap();
    fs::write(dir.join("notes.txt"), "kept\n").unwrap();
    fs::create_dir(dir.join("docs")).unwrap();
    fs::write(dir.join("blob.bin"), [0xFF, 0xFE, 0x00]).unwrap();
    fs::write(dir.join("big.dat"), vec![b'x'; project::MAX_FILE + 1]).unwrap();
    fs::write(dir.join("ctl.txt"), "a\u{7}b").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", dir.join("link.txt")).unwrap();
    fs::write(dir.join(".save-1-1"), "temporary").unwrap();
    let files = read_folder(&dir).expect("opens");
    assert_eq!(
        files.0.keys().map(String::as_str).collect::<Vec<_>>(),
        ["main.st", "notes.txt"]
    );
    // Hand-placed files past the kept-file count are not all read in.
    for k in 0..100 {
        fs::write(dir.join(format!("extra{k:03}.txt")), "x").unwrap();
    }
    let files = read_folder(&dir).expect("still opens");
    assert!(files.check().is_ok(), "{:?}", files.check());
    let _ = fs::remove_dir_all(&dir);
}
