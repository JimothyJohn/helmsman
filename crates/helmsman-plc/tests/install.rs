//! scripts/install.sh and scripts/package-release.sh: what a user runs to
//! put a release on a Linux machine. Run in a temp root
//! (`HELMSMAN_INSTALL_ROOT`: every path below it, systemd left alone) on
//! archives made here. The contract: what a release holds is installed
//! where the unit looks for it; an install that is already there keeps
//! its settings; and an archive that is not exactly a release (a wrong
//! checksum, a path that leaves the folder, a link, a stray file, a
//! missing piece) installs nothing at all.

// Bash scripts for a Linux machine: Unix hosts only.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const TARGET: &str = "x86_64-unknown-linux-musl";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A fresh folder under the test's temp dir.
fn fresh(what: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "install-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(cmd: &mut Command) -> Output {
    // Whatever locale the caller has, the scripts' patterns are bytes.
    cmd.env("LC_ALL", "C").output().expect("bash runs")
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A stand-in box (a script that says `text`) and an editor of two files.
fn pieces(dir: &Path, text: &str) -> (PathBuf, PathBuf) {
    let bin = dir.join("helmsman-plc");
    fs::write(&bin, format!("#!/bin/sh\necho {text}\n")).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let ui = dir.join("canvas-dist");
    fs::create_dir_all(ui.join("assets")).unwrap();
    fs::write(
        ui.join("index.html"),
        format!("<!doctype html><title>{text}</title>"),
    )
    .unwrap();
    fs::write(ui.join("assets/app.wasm"), text).unwrap();
    (bin, ui)
}

/// A release archive as scripts/package-release.sh packs it.
fn packed(text: &str) -> PathBuf {
    let dir = fresh("pack");
    let (bin, ui) = pieces(&dir, text);
    let out = run(Command::new("bash")
        .arg(repo().join("scripts/package-release.sh"))
        .args([
            TARGET.as_ref(),
            bin.as_os_str(),
            ui.as_os_str(),
            dir.join("dist").as_os_str(),
        ]));
    assert!(out.status.success(), "{}", said(&out));
    dir.join("dist").join(format!("helmsman-{TARGET}.tar.gz"))
}

/// An archive of `stage`'s `members`, with its checksum, named as a release.
fn archive_of(stage: &Path, members: &[&str]) -> PathBuf {
    let name = format!("helmsman-{TARGET}.tar.gz");
    let out = stage.join(&name);
    let tar = run(Command::new("tar")
        .current_dir(stage)
        .arg("-czf")
        .arg(&out)
        .args(members));
    assert!(tar.status.success(), "{}", said(&tar));
    let sum = run(Command::new("bash").current_dir(stage).arg("-c").arg(format!(
        "if command -v sha256sum >/dev/null; then sha256sum {name}; else shasum -a 256 {name}; fi > {name}.sha256"
    )));
    assert!(sum.status.success(), "{}", said(&sum));
    out
}

fn install(root: &Path, args: &[&str]) -> Output {
    run(Command::new("bash")
        .arg(repo().join("scripts/install.sh"))
        .args(args)
        .env("HELMSMAN_INSTALL_ROOT", root)
        .env("HELMSMAN_INSTALL_MACHINE", "x86_64"))
}

/// Every file under `root`, relative, sorted.
fn tree(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                walk(&p, root, out);
            } else {
                out.push(p.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn a_release_is_installed_where_the_unit_looks_and_the_installed_box_runs() {
    let root = fresh("root");
    let archive = packed("first");
    // Exactly a release's members, whatever machine packed it: a Mac's tar
    // adds a `._name` beside a file with extended attributes, and the
    // installer refuses such an archive (found on 2026-11-23).
    let listed = run(Command::new("tar").arg("-tzf").arg(&archive));
    let mut names: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(|l| l.trim_end_matches('/').to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "helmsman-plc",
            "helmsman-plc.service",
            "ui",
            "ui/assets",
            "ui/assets/app.wasm",
            "ui/index.html"
        ]
    );
    let out = install(&root, &["--local", archive.to_str().unwrap()]);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("verifying sha256"), "{}", said(&out));
    assert_eq!(
        tree(&root),
        [
            "etc/helmsman-plc/env",
            "etc/helmsman-plc/relay-token",
            "etc/systemd/system/helmsman-plc.service",
            "usr/local/bin/helmsman-plc",
            "usr/local/share/helmsman-plc/ui/assets/app.wasm",
            "usr/local/share/helmsman-plc/ui/index.html",
        ]
    );
    // The unit that ships is the repository's, and what it reads is there:
    // its EnvironmentFile with every variable it expands, and the file it
    // loads as the relay token's credential.
    let unit = fs::read_to_string(root.join("etc/systemd/system/helmsman-plc.service")).unwrap();
    assert_eq!(
        unit,
        fs::read_to_string(repo().join("deploy/helmsman-plc.service")).unwrap()
    );
    let env = fs::read_to_string(root.join("etc/helmsman-plc/env")).unwrap();
    assert_eq!(
        env,
        "PLC_SOURCE=--projects\nPLC_SEED=\nPLC_SCAN_MS=10\nPLC_API=127.0.0.1:8080\nPLC_UI=/usr/local/share/helmsman-plc/ui\nPLC_RELAY_ARGS=\n",
        "closed to the network unless asked, and the unit's real paths"
    );
    for var in [
        "PLC_SOURCE",
        "PLC_SEED",
        "PLC_SCAN_MS",
        "PLC_API",
        "PLC_UI",
        "PLC_RELAY_ARGS",
    ] {
        assert!(unit.contains(var), "the unit reads {var}");
    }
    assert!(unit.contains("LoadCredential=relay-token:/etc/helmsman-plc/relay-token"));
    let token = root.join("etc/helmsman-plc/relay-token");
    assert_eq!(fs::read(&token).unwrap(), b"");
    assert_eq!(fs::metadata(&token).unwrap().permissions().mode() & 0o777, 0o600);
    let bin = root.join("usr/local/bin/helmsman-plc");
    assert_eq!(fs::metadata(&bin).unwrap().permissions().mode() & 0o777, 0o755);
    let ran = run(&mut Command::new(&bin));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "first\n");
}

#[test]
fn an_upgrade_replaces_the_box_and_the_whole_editor_and_keeps_the_settings() {
    let root = fresh("root");
    let first = packed("first");
    assert!(
        install(&root, &["--lan", "--local", first.to_str().unwrap()])
            .status
            .success()
    );
    let env = root.join("etc/helmsman-plc/env");
    assert!(
        fs::read_to_string(&env)
            .unwrap()
            .contains("PLC_API=0.0.0.0:8080\n"),
        "--lan on a first install"
    );
    // The owner's own settings, a paired relay, and a file the old editor
    // had that the new one does not.
    fs::write(
        &env,
        "PLC_SOURCE=--projects\nPLC_API=10.0.0.5:9000\nPLC_RELAY_ARGS=--relay https://r\n",
    )
    .unwrap();
    fs::write(root.join("etc/helmsman-plc/relay-token"), "hmr1_secret").unwrap();
    let stale = root.join("usr/local/share/helmsman-plc/ui/old-0123.wasm");
    fs::write(&stale, "old").unwrap();

    let second = packed("second");
    let out = install(&root, &["--lan", "--local", second.to_str().unwrap()]);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(
        fs::read_to_string(&env).unwrap(),
        "PLC_SOURCE=--projects\nPLC_API=10.0.0.5:9000\nPLC_RELAY_ARGS=--relay https://r\n",
        "the settings are the owner's"
    );
    assert_eq!(
        fs::read_to_string(root.join("etc/helmsman-plc/relay-token")).unwrap(),
        "hmr1_secret"
    );
    let ran = run(&mut Command::new(root.join("usr/local/bin/helmsman-plc")));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "second\n");
    assert!(
        fs::read_to_string(root.join("usr/local/share/helmsman-plc/ui/index.html"))
            .unwrap()
            .contains("second")
    );
    assert!(!stale.exists(), "nothing of the old editor is left to be served");
    assert!(
        !tree(&root)
            .iter()
            .any(|f| f.contains(".new") || f.contains(".old"))
    );
}

#[test]
fn an_archive_that_is_not_exactly_a_release_installs_nothing() {
    // (what it is, how it is made, what the installer says)
    type Make = fn(&Path) -> PathBuf;
    let cases: [(&str, Make, &str); 7] = [
        (
            "a checksum that does not match",
            |_| {
                let a = packed("tampered");
                let mut bytes = fs::read(&a).unwrap();
                let k = bytes.len() / 2;
                bytes[k] ^= 0x55;
                fs::write(&a, bytes).unwrap();
                a
            },
            "checksum mismatch",
        ),
        (
            "a path that leaves the folder",
            |d| {
                pieces(d, "x");
                fs::create_dir_all(d.join("ui")).unwrap();
                fs::write(d.join("ui/index.html"), "x").unwrap();
                fs::write(d.join("helmsman-plc.service"), "[Unit]\n").unwrap();
                fs::write(d.join("escaped"), "x").unwrap();
                fs::create_dir_all(d.join("in")).unwrap();
                // tar keeps the name as given: `in/../escaped`.
                let name = format!("helmsman-{TARGET}.tar.gz");
                let out = Command::new("tar")
                    .current_dir(d.join("in"))
                    .args([
                        "-czf",
                        &format!("../{name}"),
                        "../escaped",
                        "../helmsman-plc",
                        "../helmsman-plc.service",
                        "../ui",
                    ])
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
                d.join(name)
            },
            "nothing installed",
        ),
        (
            "a file a release does not hold",
            |d| {
                pieces(d, "x");
                fs::rename(d.join("canvas-dist"), d.join("ui")).unwrap();
                fs::write(d.join("helmsman-plc.service"), "[Unit]\n").unwrap();
                fs::write(d.join("post-install.sh"), "rm -rf /\n").unwrap();
                archive_of(
                    d,
                    &["helmsman-plc", "helmsman-plc.service", "ui", "post-install.sh"],
                )
            },
            "a file it should not",
        ),
        (
            "a symbolic link in the editor",
            |d| {
                pieces(d, "x");
                fs::rename(d.join("canvas-dist"), d.join("ui")).unwrap();
                fs::write(d.join("helmsman-plc.service"), "[Unit]\n").unwrap();
                std::os::unix::fs::symlink("/etc/passwd", d.join("ui/passwd.html")).unwrap();
                archive_of(d, &["helmsman-plc", "helmsman-plc.service", "ui"])
            },
            "symbolic link",
        ),
        (
            "no box",
            |d| {
                pieces(d, "x");
                fs::rename(d.join("canvas-dist"), d.join("ui")).unwrap();
                fs::write(d.join("helmsman-plc.service"), "[Unit]\n").unwrap();
                archive_of(d, &["helmsman-plc.service", "ui"])
            },
            "holds no helmsman-plc",
        ),
        (
            "no editor",
            |d| {
                pieces(d, "x");
                fs::write(d.join("helmsman-plc.service"), "[Unit]\n").unwrap();
                archive_of(d, &["helmsman-plc", "helmsman-plc.service"])
            },
            "holds no editor",
        ),
        (
            "not an archive",
            |d| {
                let a = d.join(format!("helmsman-{TARGET}.tar.gz"));
                fs::write(&a, "<html>404 Not Found</html>").unwrap();
                a
            },
            "nothing installed",
        ),
    ];
    for (what, make, says) in cases {
        let root = fresh("root");
        let archive = make(&fresh("bad"));
        let out = install(&root, &["--local", archive.to_str().unwrap()]);
        assert!(!out.status.success(), "{what}: {}", said(&out));
        assert!(said(&out).contains(says), "{what}: {}", said(&out));
        assert_eq!(tree(&root), Vec::<String>::new(), "{what}: nothing is installed");
    }
}

#[test]
fn a_tag_that_is_not_a_release_tag_or_a_machine_with_no_release_is_refused_before_any_download() {
    for tag in [
        "0.1.0",
        "v",
        "latest",
        "v1/../../evil",
        "v1.0.0/x",
        "v1 0",
        "v1;id",
        "v1$(id)",
        "-v1",
    ] {
        let root = fresh("root");
        // No network is reached: curl is not on this PATH's refusal path,
        // and a tag that passed would fail on the download instead.
        let out = install(&root, &[tag]);
        assert!(!out.status.success(), "{tag}: {}", said(&out));
        let text = said(&out);
        assert!(
            text.contains("not a release tag")
                || text.contains("unknown option")
                || text.contains("install.sh"),
            "{tag}: {text}"
        );
        assert!(!text.contains("fetching"), "{tag}: nothing is fetched: {text}");
        assert_eq!(tree(&root), Vec::<String>::new(), "{tag}");
    }
    let root = fresh("root");
    let archive = packed("x");
    let out = run(Command::new("bash")
        .arg(repo().join("scripts/install.sh"))
        .args(["--local", archive.to_str().unwrap()])
        .env("HELMSMAN_INSTALL_ROOT", &root)
        .env("HELMSMAN_INSTALL_MACHINE", "riscv64"));
    assert!(!out.status.success());
    assert!(
        said(&out).contains("unsupported machine: riscv64"),
        "{}",
        said(&out)
    );
    assert_eq!(tree(&root), Vec::<String>::new());
    // A tag and a file at once, or neither, is a usage error.
    let both = install(&root, &["v0.1.0", "--local", archive.to_str().unwrap()]);
    assert!(
        !both.status.success() && said(&both).contains("not both"),
        "{}",
        said(&both)
    );
    assert!(!install(&root, &[]).status.success());
}

#[test]
fn an_archive_without_its_checksum_is_installed_with_a_warning_and_the_packer_refuses_what_it_cannot_pack() {
    let root = fresh("root");
    let archive = packed("unsummed");
    fs::remove_file(format!("{}.sha256", archive.display())).unwrap();
    let out = install(&root, &["--local", archive.to_str().unwrap()]);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("WARNING: no .sha256"), "{}", said(&out));

    let dir = fresh("pack");
    let (bin, ui) = pieces(&dir, "x");
    let pack = |target: &str, bin: &Path, ui: &Path| {
        run(Command::new("bash")
            .arg(repo().join("scripts/package-release.sh"))
            .args([
                target.as_ref(),
                bin.as_os_str(),
                ui.as_os_str(),
                dir.join("out").as_os_str(),
            ]))
    };
    assert!(!pack("x86_64-pc-windows-msvc", &bin, &ui).status.success());
    assert!(!pack(TARGET, &dir.join("missing"), &ui).status.success());
    assert!(!pack(TARGET, &bin, &dir.join("no-ui")).status.success());
    assert!(!dir.join("out").join(format!("helmsman-{TARGET}.tar.gz")).exists());
}
