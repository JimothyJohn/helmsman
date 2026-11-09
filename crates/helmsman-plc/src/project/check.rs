//! A project's check and requirement tests, the box's and the command
//! line's: `POST /api/projects/{p}/tests` and the deploy gate run
//! [`run_tests`], and `helmsman-plc test PROJECT_DIR` ([`cli`]) runs the
//! same function on a folder kept outside a box (in git, on a CI runner),
//! after the same build a deploy does ([`super::build::build`]). No box
//! runs, nothing is opened on the network and nothing is written but the
//! report asked for.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, spec};

use super::build::{self, Shape};
use super::junit::{self, Case, Outcome};
use super::{Files, Unread, read_folder};

/// How a box of `shape` runs requirement tests: its scan period and
/// image sizes.
#[must_use]
pub fn test_settings(shape: &Shape) -> spec::Settings {
    spec::Settings::new(ImageSizes {
        i: shape.i_len,
        q: shape.q_len,
        m: shape.defaults.m,
    })
    .period(shape.defaults.period)
}

/// Every requirement test (`test_*.st`) of `files`, run in the
/// simulation, in file order.
#[must_use]
pub fn run_tests(files: &Files, settings: spec::Settings) -> Vec<spec::Verdict> {
    spec::run_all(&spec::plan(files.st()), settings)
}

pub const USAGE: &str = "\
usage: helmsman-plc test PROJECT_DIR [--junit FILE] [--scan-ms MS]
  Checks the project in folder PROJECT_DIR as a deploy builds it, then
  runs its requirement tests (test_*.st) in the simulation, the same
  tests a box runs before it deploys. No box needs to be running and
  nothing goes on the network, so any CI runner can gate a project kept
  in git (docs/ci/README.md).
  --junit FILE   also write a JUnit XML report to FILE: written whole or
                 not at all. A requirement with no test yet is <skipped/>.
  --scan-ms MS   the scan period the tests run at, 1..=10000 (10, the
                 box's default)
exit status:
  0   the project builds and no test is red. A requirement with no test
      yet (\"untested\") never fails the run.
  1   the project builds and at least one test is red
  2   the project does not build or check: each problem is printed as
      PROJECT_DIR/FILE:LINE:COL: message
  64  usage: a bad flag, or PROJECT_DIR is missing, not a folder or holds
      no .st file
  74  a file could not be read, or the report could not be written";

/// Exit statuses ([`USAGE`] says what each means).
pub const GREEN: u8 = 0;
pub const RED: u8 = 1;
pub const BROKEN: u8 = 2;
pub const USAGE_ERROR: u8 = 64;
pub const IO_ERROR: u8 = 74;

/// What a run printed and how it exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub code: u8,
    pub out: String,
    pub err: String,
}

impl Ran {
    fn fail(code: u8, err: impl Into<String>) -> Self {
        Self {
            code,
            out: String::new(),
            err: err.into(),
        }
    }
}

struct Args {
    dir: PathBuf,
    junit: Option<PathBuf>,
    scan: Option<Duration>,
}

fn parse(args: &[String]) -> Result<Option<Args>, String> {
    let mut dir = None;
    let mut junit = None;
    let mut scan = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => return Ok(None),
            "--junit" => {
                let v = args.get(i + 1).ok_or("--junit: expected a file")?;
                junit = Some(PathBuf::from(v));
                i += 2;
            }
            "--scan-ms" => {
                scan = match args.get(i + 1).map(|v| v.parse::<u64>()) {
                    Some(Ok(ms @ 1..=10_000)) => Some(Duration::from_millis(ms)),
                    _ => return Err("--scan-ms: expected 1..=10000".into()),
                };
                i += 2;
            }
            w if w.starts_with('-') && w.len() > 1 => return Err(format!("unknown flag {w}")),
            w => {
                if dir.is_some() {
                    return Err(format!("one PROJECT_DIR only (also {w:?})"));
                }
                dir = Some(PathBuf::from(w));
                i += 1;
            }
        }
    }
    let dir = dir.ok_or("expected PROJECT_DIR")?;
    Ok(Some(Args { dir, junit, scan }))
}

/// `dir/file`, as messages name a project's file.
fn at(dir: &Path, file: &str) -> String {
    dir.join(file).display().to_string()
}

fn why_io(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::InvalidData => "not a regular file (a link is never followed)".into(),
        _ => e.to_string(),
    }
}

/// `helmsman-plc test ARGS` on a box of `shape` (its scan period the
/// default for `--scan-ms`).
#[must_use]
pub fn cli(args: &[String], shape: &Shape) -> Ran {
    let a = match parse(args) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return Ran {
                code: GREEN,
                out: format!("{USAGE}\n"),
                err: String::new(),
            };
        }
        Err(e) => return Ran::fail(USAGE_ERROR, format!("error: {e}\n{USAGE}\n")),
    };
    let dir = &a.dir;
    let started = Instant::now();
    let (files, name) = match load(dir) {
        Ok(x) => x,
        Err(ran) => return ran,
    };
    let mut shape = shape.clone();
    if let Some(scan) = a.scan {
        shape.defaults.period = scan;
    }
    let mut out = String::new();
    let mut err = String::new();
    let _ = writeln!(out, "project {name} ({})", dir.display());
    let built = build::build(&name, &files, None, &shape);
    let mut cases = vec![build_case(&name, dir, &built, &mut out, &mut err)];
    let mut verdicts = run_tests(&files, test_settings(&shape));
    verdicts.sort_by(|a, b| a.file.cmp(&b.file));
    let (mut passed, mut red, mut untested) = (0, 0, 0);
    for v in &verdicts {
        let stem = v.file.trim_end_matches(".st");
        let label = format!("{stem}: {}", v.requirement);
        let outcome = if v.passed() {
            passed += 1;
            let _ = writeln!(out, "  pass      {label}");
            Outcome::Passed
        } else if v.untested() {
            untested += 1;
            let _ = writeln!(out, "  untested  {label} (no test yet)");
            Outcome::Skipped { message: v.why() }
        } else {
            red += 1;
            let _ = writeln!(out, "  RED       {label}\n            {}", v.why());
            Outcome::Failed {
                message: v.why(),
                detail: format!("{}\n{}", v.requirement, v.why()),
            }
        };
        let ran = if v.untested() {
            String::new()
        } else {
            format!(
                "{} scan(s), {:.2} s simulated at {} ms a scan",
                v.scans,
                v.elapsed.as_secs_f64(),
                shape.defaults.period.as_millis()
            )
        };
        cases.push(case(&name, &label, Some(&v.file), outcome, ran));
    }
    let _ = writeln!(
        out,
        "{} test(s): {passed} passed, {red} red, {untested} untested",
        verdicts.len()
    );
    let mut code = match (&built, red) {
        (Err(_), _) => BROKEN,
        (Ok(_), 0) => GREEN,
        (Ok(_), _) => RED,
    };
    if let Some(path) = &a.junit {
        let xml = junit::report(&name, &cases, started.elapsed());
        if let Err(e) = write_whole(path, xml.as_bytes()) {
            let _ = writeln!(err, "error: report {}: {e}; nothing written", path.display());
            code = IO_ERROR;
        }
    }
    Ran { code, out, err }
}

/// Project folder `dir`'s files and the project's name (the folder's), or
/// the run that says why not.
fn load(dir: &Path) -> Result<(Files, String), Ran> {
    if !fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
        return Err(Ran::fail(
            USAGE_ERROR,
            format!("error: {}: no such folder\n", dir.display()),
        ));
    }
    let files = read_folder(dir).map_err(|e| match e {
        Unread::Io(None, e) => Ran::fail(IO_ERROR, format!("error: {}: {e}\n", dir.display())),
        Unread::Io(Some(f), e) if e.kind() == io::ErrorKind::InvalidData => {
            Ran::fail(BROKEN, format!("{}: {}\n", at(dir, &f), why_io(&e)))
        }
        Unread::Io(Some(f), e) => Ran::fail(IO_ERROR, format!("error: {}: {e}\n", at(dir, &f))),
        Unread::Text(f, e) => Ran::fail(BROKEN, format!("{}: {e}\n", at(dir, &f))),
    })?;
    if files.st().next().is_none() {
        return Err(Ran::fail(
            USAGE_ERROR,
            format!("error: {}: not a project (no .st file)\n", dir.display()),
        ));
    }
    let name = fs::canonicalize(dir)
        .ok()
        .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .unwrap_or_else(|| "project".into());
    Ok((files, name))
}

/// The `build` case; what the build said goes to `out` and, each
/// problem as `DIR/FILE:LINE:COL: message`, to `err`.
fn build_case(
    name: &str,
    dir: &Path,
    built: &Result<build::Built, Vec<build::Diag>>,
    out: &mut String,
    err: &mut String,
) -> Case {
    let diags = match built {
        Ok(b) => {
            let modules: Vec<&str> = b.modules.iter().map(|m| m.name.as_str()).collect();
            let _ = writeln!(
                out,
                "  builds: {} module(s): {}",
                modules.len(),
                modules.join(", ")
            );
            return case(name, "build", None, Outcome::Passed, String::new());
        }
        Err(diags) => diags,
    };
    let mut lines = String::new();
    for d in diags {
        if d.file.is_empty() {
            let _ = writeln!(err, "{}: {}", dir.display(), d.message);
        } else {
            let _ = writeln!(err, "{}:{}:{}: {}", at(dir, &d.file), d.line, d.col, d.message);
        }
        let _ = writeln!(lines, "{d}");
    }
    let _ = writeln!(out, "  DOES NOT BUILD: {} problem(s)", diags.len());
    let message = format!("the project does not build ({} problem(s))", diags.len());
    case(
        name,
        "build",
        None,
        Outcome::Error {
            message,
            detail: lines,
        },
        String::new(),
    )
}

fn case(class: &str, name: &str, file: Option<&str>, outcome: Outcome, out: String) -> Case {
    Case {
        classname: class.to_string(),
        name: name.to_string(),
        file: file.map(str::to_string),
        outcome,
        out,
    }
}

/// Writes `path` whole or not at all: a temporary beside it, renamed over
/// it, removed if anything fails.
fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let file = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file name"))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(file);
    tmp_name.push(format!(".partial-{}", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let r = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}
