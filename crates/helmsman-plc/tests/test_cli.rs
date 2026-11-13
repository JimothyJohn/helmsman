//! `helmsman-plc test PROJECT_DIR`: a project folder checked and its
//! requirement tests run with no box, as a CI runner sees it: the exit
//! status, what it prints and the `JUnit` report (`--junit`).

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/projects/unreleased-20261107/pump-station"
);

const MAIN: &str = "\
PROGRAM main
VAR_INPUT start_pb : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
lamp := start_pb;
END_PROGRAM
";

/// Green: the lamp follows `start_pb`.
const LAMP_ON: &str = "\
(* The lamp is on while start_pb is held. *)
PROGRAM test_lamp_on
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR n : INT; lamp_lit : ASSERT; END_VAR
n := n + 1;
start_pb := TRUE;
IF n = 3 THEN
  lamp_lit(COND := lamp);
  done := TRUE;
END_IF;
END_PROGRAM
";

/// Red, with markup, quotes and non-ASCII in its requirement and check.
const LAMP_DARK: &str = "\
(* The lamp <stays> dark & \"quiet\" 'always' : Lampe aus, 灯 *)
PROGRAM test_lamp_dark
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR n : INT; lamp_dark : ASSERT; (* dark <&> \"ü\" *) END_VAR
n := n + 1;
start_pb := TRUE;
IF n = 3 THEN
  lamp_dark(COND := NOT lamp);
  done := TRUE;
END_IF;
END_PROGRAM
";

/// Untested: states its requirement, checks nothing yet.
const LATER: &str = "\
(* The lamp flashes on a fault. *)
PROGRAM test_flash
END_PROGRAM
";

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "test-cli-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A project folder of `files`.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = tmp(name).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for (f, text) in files {
        std::fs::write(dir.join(f), text).unwrap();
    }
    dir
}

/// The binary with `test` first, as a subcommand must be.
fn test_cmd(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .arg("test")
        .args(args)
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exited, not killed")
}

/// Panics unless `xml` is well-formed: one prolog, balanced tags, quoted
/// attributes, and no raw `<` or bare `&` in text or attribute values.
fn well_formed(xml: &str) {
    let body = xml
        .strip_prefix("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
        .expect("prolog");
    let entity_ok = |s: &str| {
        let mut rest = s;
        while let Some(i) = rest.find('&') {
            rest = &rest[i + 1..];
            assert!(
                ["amp;", "lt;", "gt;", "quot;", "apos;"]
                    .iter()
                    .any(|e| rest.starts_with(e)),
                "bare & in {s:?}"
            );
        }
    };
    let mut stack: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        entity_ok(&rest[..open]);
        assert!(!rest[..open].contains('>'), "stray > in text");
        rest = &rest[open + 1..];
        // A tag runs to the first `>` outside quotes.
        let mut quoted = false;
        let mut end = None;
        for (i, c) in rest.char_indices() {
            match c {
                '"' => quoted = !quoted,
                '<' if quoted => panic!("raw < in an attribute"),
                '>' if !quoted => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let tag = &rest[..end.expect("unclosed tag")];
        rest = &rest[tag.len() + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            assert_eq!(stack.pop().as_deref(), Some(name), "mismatched </{name}>");
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, mut attrs) = tag.split_once(' ').unwrap_or((tag, ""));
        assert!(!name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic() || c == '-'));
        while !attrs.trim().is_empty() {
            let (key, after) = attrs.trim_start().split_once("=\"").expect("key=\"value\"");
            assert!(key.chars().all(|c| c.is_ascii_alphabetic()), "attribute {key:?}");
            let (value, after) = after.split_once('"').expect("closing quote");
            entity_ok(value);
            attrs = after;
        }
        if !self_closing {
            stack.push(name.to_string());
        }
    }
    assert!(rest.trim().is_empty() && stack.is_empty(), "unclosed {stack:?}");
}

/// The report with its measured `time` attributes blanked.
fn timeless(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find(" time=\"") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 7..];
        rest = &rest[rest.find('"').unwrap() + 1..];
    }
    out.push_str(rest);
    out
}

#[test]
fn a_green_project_exits_0_with_a_well_formed_report() {
    let out_dir = tmp("green-report");
    let report = out_dir.join("report.xml");
    let o = test_cmd(&[FIXTURE, "--junit", report.to_str().unwrap()]);
    let stdout = text(&o.stdout);
    assert_eq!(code(&o), 0, "{stdout}{}", text(&o.stderr));
    assert!(stdout.contains("builds: 1 module(s): pump_station"), "{stdout}");
    assert!(stdout.contains("pass      test_green_lamp"), "{stdout}");
    assert!(stdout.contains("untested  test_dry_well"), "{stdout}");
    assert!(
        stdout.contains("2 test(s): 1 passed, 0 red, 1 untested"),
        "{stdout}"
    );
    let xml = std::fs::read_to_string(&report).unwrap();
    well_formed(&xml);
    assert!(
        xml.contains("tests=\"3\" failures=\"0\" errors=\"0\" skipped=\"1\""),
        "{xml}"
    );
    assert!(
        xml.contains("<testcase classname=\"pump-station\" name=\"build\"/>"),
        "{xml}"
    );
    // Untested is skipped, never a failure; cases come in file order.
    assert!(xml.contains("<skipped message=\"no test yet: nothing checks this requirement\"/>"));
    let dry = xml.find("file=\"test_dry_well.st\"").unwrap();
    let green = xml.find("file=\"test_green_lamp.st\"").unwrap();
    assert!(dry < green);
    // Nothing left beside the report.
    let left: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["report.xml"]);
}

#[test]
fn the_report_is_the_same_every_run_but_for_time() {
    let dir = project(
        "same",
        &[
            ("main.st", MAIN),
            ("test_lamp_on.st", LAMP_ON),
            ("test_lamp_dark.st", LAMP_DARK),
            ("test_flash.st", LATER),
        ],
    );
    let a = tmp("same-a").join("a.xml");
    let b = tmp("same-b").join("b.xml");
    let dir = dir.to_str().unwrap();
    assert_eq!(code(&test_cmd(&[dir, "--junit", a.to_str().unwrap()])), 1);
    assert_eq!(code(&test_cmd(&["--junit", b.to_str().unwrap(), dir])), 1);
    let (a, b) = (
        std::fs::read_to_string(a).unwrap(),
        std::fs::read_to_string(b).unwrap(),
    );
    assert_eq!(timeless(&a), timeless(&b));
}

#[test]
fn a_red_test_exits_1_and_the_report_says_why() {
    let dir = project(
        "red",
        &[
            ("main.st", MAIN),
            ("test_lamp_on.st", LAMP_ON),
            ("test_lamp_dark.st", LAMP_DARK),
            ("test_flash.st", LATER),
        ],
    );
    let report = tmp("red-report").join("r.xml");
    let o = test_cmd(&[dir.to_str().unwrap(), "--junit", report.to_str().unwrap()]);
    let stdout = text(&o.stdout);
    assert_eq!(code(&o), 1, "{stdout}{}", text(&o.stderr));
    assert!(
        stdout.contains("RED       test_lamp_dark: The lamp <stays> dark"),
        "{stdout}"
    );
    assert!(
        stdout.contains("lamp dark (dark <&> \"ü\") failed at"),
        "{stdout}"
    );
    assert!(
        stdout.contains("3 test(s): 1 passed, 1 red, 1 untested"),
        "{stdout}"
    );
    let xml = std::fs::read_to_string(&report).unwrap();
    well_formed(&xml);
    assert!(
        xml.contains("tests=\"4\" failures=\"1\" errors=\"0\" skipped=\"1\""),
        "{xml}"
    );
    assert!(
        xml.contains(
            "name=\"test_lamp_dark: The lamp &lt;stays&gt; dark &amp; &quot;quiet&quot; &apos;always&apos; : Lampe aus, 灯\""
        ),
        "{xml}"
    );
    assert!(
        xml.contains("<failure message=\"lamp dark (dark &lt;&amp;&gt; &quot;ü&quot;) failed at 0.02 s (")
            && xml.contains(")\" type=\"failed\">"),
        "{xml}"
    );
}

#[test]
fn a_project_that_does_not_build_exits_2_naming_file_and_line() {
    let broken = "PROGRAM main\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := ;\nEND_PROGRAM\n";
    let dir = project("broken", &[("main.st", broken), ("test_lamp_on.st", LAMP_ON)]);
    let report = tmp("broken-report").join("r.xml");
    let o = test_cmd(&[dir.to_str().unwrap(), "--junit", report.to_str().unwrap()]);
    let stderr = text(&o.stderr);
    assert_eq!(code(&o), 2, "{}{stderr}", text(&o.stdout));
    let at = format!("{}:3:", dir.join("main.st").display());
    assert!(stderr.lines().any(|l| l.starts_with(&at)), "{stderr}");
    let xml = std::fs::read_to_string(&report).unwrap();
    well_formed(&xml);
    assert!(
        xml.contains(
            "<error message=\"the project does not build (1 problem(s))\" type=\"error\">main.st:3:"
        ),
        "{xml}"
    );
}

/// A function block that declares RETAIN, which is not kept inside one.
const RETAIN_BLOCK: &str = "\
FUNCTION_BLOCK Latch
VAR_INPUT set : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
VAR RETAIN kept : INT; END_VAR
q := q OR set;
END_FUNCTION_BLOCK
";

const MAIN_WITH_LATCH: &str = "\
PROGRAM main
VAR_INPUT start_pb : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR l : Latch; END_VAR
l(set := start_pb);
lamp := start_pb;
END_PROGRAM
";

#[test]
fn a_warning_is_printed_apart_from_errors_and_never_changes_the_exit() {
    let dir = project(
        "warned",
        &[
            ("main.st", MAIN_WITH_LATCH),
            ("blocks.st", RETAIN_BLOCK),
            ("test_lamp_on.st", LAMP_ON),
        ],
    );
    let report = tmp("warned-report").join("r.xml");
    let o = test_cmd(&[dir.to_str().unwrap(), "--junit", report.to_str().unwrap()]);
    let (stdout, stderr) = (text(&o.stdout), text(&o.stderr));
    assert_eq!(code(&o), 0, "{stdout}{stderr}");
    let at = format!("{}:4:", dir.join("blocks.st").display());
    let line = stderr.lines().find(|l| l.starts_with(&at)).expect(&stderr);
    assert!(
        line.contains(": warning: ") && line.contains("RETAIN inside a function block"),
        "{line}"
    );
    assert!(
        stdout.contains("1 warning(s), which never stop a build"),
        "{stdout}"
    );
    // In the report the warning rides on the build case, which passed.
    let xml = std::fs::read_to_string(&report).unwrap();
    well_formed(&xml);
    assert!(xml.contains("<system-out>") && xml.contains("warning: "), "{xml}");
    assert!(!xml.contains("<failure") && !xml.contains("<error"), "{xml}");
}

#[test]
fn an_error_and_a_warning_are_both_printed_and_only_the_error_fails() {
    // It parses, so the checker reaches the function block's RETAIN too.
    let broken = "PROGRAM z\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := nothing_here;\nEND_PROGRAM\n";
    let dir = project(
        "mixed",
        &[
            ("main.st", MAIN_WITH_LATCH),
            ("blocks.st", RETAIN_BLOCK),
            ("z.st", broken),
        ],
    );
    let o = test_cmd(&[dir.to_str().unwrap()]);
    let stderr = text(&o.stderr);
    assert_eq!(code(&o), 2, "{}{stderr}", text(&o.stdout));
    let warning = stderr.lines().filter(|l| l.contains(": warning: ")).count();
    assert_eq!(warning, 1, "{stderr}");
    let error = format!("{}:3:", dir.join("z.st").display());
    let line = stderr.lines().find(|l| l.starts_with(&error)).expect(&stderr);
    assert!(!line.contains("warning"), "{line}");
}

#[test]
fn a_build_problem_outranks_a_red_test() {
    // A red test and a settings line naming no PROGRAM: the build wins.
    let dir = project(
        "both",
        &[
            ("main.st", MAIN),
            ("test_lamp_dark.st", LAMP_DARK),
            ("project.settings", "program nosuch scan_ms=10\n"),
        ],
    );
    let o = test_cmd(&[dir.to_str().unwrap()]);
    assert_eq!(code(&o), 2, "{}{}", text(&o.stdout), text(&o.stderr));
    assert!(
        text(&o.stderr).contains("project.settings:1:"),
        "{}",
        text(&o.stderr)
    );
}

#[test]
fn a_file_that_breaks_the_rules_exits_2() {
    let dir = project("dirty", &[("main.st", MAIN)]);
    std::fs::write(dir.join("other.st"), b"PROGRAM x\n\x07END_PROGRAM\n").unwrap();
    let o = test_cmd(&[dir.to_str().unwrap()]);
    assert_eq!(code(&o), 2, "{}", text(&o.stderr));
    assert!(text(&o.stderr).contains("other.st"), "{}", text(&o.stderr));
}

#[test]
fn usage_errors_exit_64() {
    let empty = tmp("empty");
    std::fs::write(empty.join("notes.txt"), "not a project").unwrap();
    let file = empty.join("notes.txt");
    let missing = empty.join("missing");
    let fixture = FIXTURE;
    for (args, says) in [
        (vec![], "expected PROJECT_DIR"),
        (vec![missing.to_str().unwrap()], "no such folder"),
        (vec![file.to_str().unwrap()], "no such folder"),
        (vec![empty.to_str().unwrap()], "not a project (no .st file)"),
        (vec![fixture, "--frobnicate"], "unknown flag --frobnicate"),
        (vec![fixture, "--junit"], "--junit: expected a file"),
        (vec![fixture, "--scan-ms", "0"], "--scan-ms: expected 1..=10000"),
        (vec![fixture, fixture], "one PROJECT_DIR only"),
    ] {
        let o = test_cmd(&args);
        assert_eq!(code(&o), 64, "{args:?}: {}", text(&o.stderr));
        assert!(text(&o.stderr).contains(says), "{args:?}: {}", text(&o.stderr));
        assert!(o.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn help_lists_the_exit_statuses() {
    let o = test_cmd(&["--help"]);
    assert_eq!(code(&o), 0);
    let help = text(&o.stdout);
    for line in [
        "  0   the project builds",
        "  1   the project builds and at least one",
        "  2   the project does not build",
        "  64  usage",
        "  74  a file could not",
    ] {
        assert!(help.contains(line), "{line:?} in\n{help}");
    }
}

#[test]
fn an_unwritable_report_exits_74_and_writes_nothing() {
    let out = tmp("unwritable");
    // A folder that is not there, and a path that is a folder.
    let nowhere = out.join("no").join("such").join("r.xml");
    let a_dir = out.join("taken");
    std::fs::create_dir(&a_dir).unwrap();
    for path in [&nowhere, &a_dir] {
        let o = test_cmd(&[FIXTURE, "--junit", path.to_str().unwrap()]);
        assert_eq!(code(&o), 74, "{}", text(&o.stderr));
        assert!(text(&o.stderr).contains("nothing written"), "{}", text(&o.stderr));
        // The results still print.
        assert!(text(&o.stdout).contains("1 passed, 0 red, 1 untested"));
    }
    assert!(!out.join("no").exists());
    assert!(std::fs::read_dir(&a_dir).unwrap().next().is_none());
    let left: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["taken"], "a partial report was left behind");
}

#[test]
fn an_existing_report_is_replaced_whole() {
    let out = tmp("replace");
    let report = out.join("r.xml");
    std::fs::write(&report, "old").unwrap();
    assert_eq!(
        code(&test_cmd(&[FIXTURE, "--junit", report.to_str().unwrap()])),
        0
    );
    well_formed(&std::fs::read_to_string(&report).unwrap());
}

#[test]
fn scan_ms_sets_the_period_tests_run_at() {
    let report = tmp("scan").join("r.xml");
    let o = test_cmd(&[FIXTURE, "--scan-ms", "20", "--junit", report.to_str().unwrap()]);
    assert_eq!(code(&o), 0, "{}", text(&o.stderr));
    let xml = std::fs::read_to_string(&report).unwrap();
    assert!(xml.contains("simulated at 20 ms a scan"), "{xml}");
}
