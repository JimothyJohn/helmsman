//! Module directories: the manifest format (the documented example, every
//! malformed line reported, render/parse round-trips, arbitrary text never
//! panics), the manifest held to the program's ports, and a module loaded
//! from a directory running with its params against the signal table.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use helmsman_iec::{ImageSizes, PortDir, Ty as StTy, compile};
use helmsman_io::{Binding, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::manifest::{CheckError, FILE, MAX_FILE, Manifest, ModuleDir, Param, SOURCE};
use helmsman_plc::spec::Defaults;
use helmsman_plc::{ConfigError, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

const D: Defaults = Defaults {
    period: Duration::from_millis(10),
    i: Window::new(0, 32),
    q: Window::new(0, 32),
    m: 1024,
};

const EXAMPLE: &str = "
# conveyor/module.manifest
name     conveyor
author   Nick <nick@example.com>
prompt   Run the belt while start is held; stop on the jam sensor.
prompt   Ramp to speed over ramp_ms.
scan_ms  50
q        0+2
reads    start jam
drives   motor lamp
param    speed   real 1.5 min=0 max=10
param    ramp_ms int  500 min=0 max=5000
param    beacon  bool true
";

fn errors(src: &str) -> Vec<(usize, String)> {
    Manifest::parse(src).expect_err(src)
}

/// A minimal valid manifest plus `extra` lines.
fn with(extra: &str) -> String {
    format!("name m\nauthor a\nscan_ms 10\nreads\ndrives\n{extra}")
}

#[test]
fn the_documented_example_parses() {
    let m = Manifest::parse(EXAMPLE).unwrap();
    assert_eq!(m.name, "conveyor");
    assert_eq!(m.author, "Nick <nick@example.com>");
    assert_eq!(
        m.prompt,
        [
            "Run the belt while start is held; stop on the jam sensor.",
            "Ramp to speed over ramp_ms."
        ]
    );
    assert_eq!(m.scan, Duration::from_millis(50));
    assert_eq!((m.i, m.q, m.m), (None, Some(Window::new(0, 2)), None));
    assert_eq!(m.reads, ["start", "jam"]);
    assert_eq!(m.drives, ["motor", "lamp"]);
    assert_eq!(
        m.params,
        [
            Param {
                name: "speed".into(),
                value: Value::Real(1.5),
                range: Some((Value::Real(0.0), Value::Real(10.0))),
            },
            Param {
                name: "ramp_ms".into(),
                value: Value::Int(500),
                range: Some((Value::Int(0), Value::Int(5000))),
            },
            Param {
                name: "beacon".into(),
                value: Value::Bool(true),
                range: None,
            },
        ]
    );
    let spec = m.spec(&PathBuf::from("mods/conveyor"), &D);
    assert_eq!(spec.path, PathBuf::from("mods/conveyor").join(SOURCE));
    assert_eq!(spec.name, "conveyor");
    assert_eq!(spec.period, Duration::from_millis(50));
    assert_eq!(
        (spec.i, spec.q, spec.m),
        (Window::new(0, 0), Window::new(0, 2), D.m)
    );
}

#[test]
fn a_minimal_manifest_has_no_windows_params_or_prompt() {
    let m = Manifest::parse(&with("")).unwrap();
    assert_eq!((m.i, m.q, m.m), (None, None, None));
    assert_eq!(m.reads, Vec::<String>::new());
    assert_eq!(m.drives, Vec::<String>::new());
    assert_eq!(m.params, Vec::<Param>::new());
    assert_eq!(m.prompt, Vec::<String>::new());
}

#[test]
fn every_missing_required_key_is_reported() {
    let errs = errors("# nothing\n\n");
    let msgs: Vec<&str> = errs.iter().map(|(_, e)| e.as_str()).collect();
    assert_eq!(
        msgs,
        [
            "missing name",
            "missing author",
            "missing scan_ms",
            "missing reads",
            "missing drives"
        ]
    );
    assert!(errs.iter().all(|(n, _)| *n == 0), "{errs:?}");
    let errs = errors("name m\nauthor a\nscan_ms 10\nreads x\n");
    assert_eq!(errs, [(0, "missing drives".to_string())]);
}

#[test]
fn every_bad_line_is_reported_with_its_number() {
    let bad = [
        "name m",     // 6: name given twice
        "author b",   // 7: author given twice
        "scan_ms 20", // 8: twice
        "reads y",    // 9: twice
        "drives z",   // 10: twice
        "colour red", // unknown key
        "Name m",     // keys are lower case
        "name",       // no value
        "scan_ms",    // no value
        "scan_ms 0",  // out of range
        "scan_ms 10001",
        "scan_ms +5",
        "i 1",
        "q 1+",
        "q -1+2",
        "m 65537",
        "m +1",
        "i 0+1 0+2",
        "param p",
        "param p int 1",       // int without a range
        "param p int 1 min=0", // half a range
        "param p int 1 min=0 max=2 min=1",
        "param p int 1 max=0 min=2", // min above max
        "param p int 5 min=0 max=2", // out of range
        "param p int -1 min=0 max=2",
        "param p int 1.5 min=0 max=2",
        "param p int 0x10 min=0 max=99",
        "param p int +1 min=0 max=2",
        "param p int 99999999999999999999 min=0 max=2",
        "param p real NaN min=0 max=2",
        "param p real inf min=0 max=2",
        "param p real 1 min=-inf max=2",
        "param p real 3 min=0 max=2",
        "param p bool true min=0 max=1",
        "param p bool 1",
        "param p string x",
        "param p int 1 min=0 max=2 step=1",
        "param 9p int 1 min=0 max=2",
        "param p-q bool true",
        "author",               // empty author
        "author \u{1b}[31mred", // escape sequence
        "author a\u{0}b",
        "author a\u{202e}b", // bidi override
        "author a\u{200b}b", // zero width space
        "prompt line\u{7}bell",
        "prompt \u{feff}bom",
        "name caf\u{e9}", // non-ASCII outside free text
        "name a\u{7f}",
        "reads caf\u{e9}",
        "name has space",
        &format!("name {}", "x".repeat(65)),
    ];
    let src = with(&bad.join("\n"));
    let errs = errors(&src);
    let lines: Vec<usize> = errs.iter().map(|(n, _)| *n).collect();
    assert_eq!(lines, (6..6 + bad.len()).collect::<Vec<_>>(), "{src}\n{errs:#?}");
}

#[test]
fn a_name_is_declared_once_across_reads_drives_and_params_in_any_case() {
    for extra in [
        "",
        "param a bool true",
        "param A bool true",
        "param b bool true\nparam B bool false",
    ] {
        let src = format!("name m\nauthor a\nscan_ms 10\nreads a\ndrives A\n{extra}");
        let errs = errors(&src);
        assert!(
            errs.iter().all(|(_, e)| e.contains("declared twice")),
            "{src}: {errs:?}"
        );
        assert_ne!(errs, Vec::new(), "{src}");
    }
    let errs = errors("name m\nauthor a\nscan_ms 10\nreads x x\ndrives\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
}

#[test]
fn free_text_keeps_unicode_and_hashes_structured_lines_take_comments() {
    let src = "name m\nscan_ms 10\nreads\ndrives\nauthor  Zoë 山田 <z@example.com>  \n\
         prompt  Keep line #3 under 40 °C — no exceptions 🙂\n\
         prompt\n\
         prompt\tthen stop\n\
         param p int 1 min=0 max=2   # a comment\n";
    let m = Manifest::parse(src).unwrap();
    assert_eq!(m.author, "Zoë 山田 <z@example.com>");
    assert_eq!(
        m.prompt,
        ["Keep line #3 under 40 °C — no exceptions 🙂", "", "then stop"]
    );
    assert_eq!(m.params.len(), 1);
}

#[test]
fn param_admits_only_its_type_within_its_range() {
    let m = Manifest::parse(&with(
        "param n int 1 min=-2 max=2\nparam x real 0.5 min=0 max=1\nparam b bool false",
    ))
    .unwrap();
    let [n, x, b] = &m.params[..] else { panic!() };
    assert!(n.admits(Value::Int(-2)) && n.admits(Value::Int(2)));
    assert!(!n.admits(Value::Int(3)) && !n.admits(Value::Real(1.0)));
    assert!(x.admits(Value::Real(1.0)) && !x.admits(Value::Real(1.000_001)));
    assert!(!x.admits(Value::Real(f64::NAN)));
    assert!(b.admits(Value::Bool(true)) && !b.admits(Value::Int(0)));
    assert_eq!(n.ty(), Ty::Int);
}

// ---- the manifest against the program ----------------------------------

const PROGRAM: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; Speed : REAL := 0.0; limit : SINT; END_VAR
VAR_OUTPUT motor : BOOL; END_VAR
VAR n : INT; END_VAR
  motor := start AND speed > 1.0 AND limit = 7;
END_PROGRAM";

fn ports(src: &str) -> Vec<helmsman_iec::Port> {
    compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap().ports()
}

fn manifest(reads: &str, drives: &str, params: &str) -> Manifest {
    let src = format!("name conveyor\nauthor a\nscan_ms 10\nreads {reads}\ndrives {drives}\n{params}");
    Manifest::parse(&src).unwrap()
}

const PARAMS: &str = "param SPEED real 1.5 min=0 max=10\nparam limit int 7 min=-128 max=127\n";

#[test]
fn a_manifest_matching_the_ports_in_any_case_checks() {
    manifest("START", "Motor", PARAMS).check(&ports(PROGRAM)).unwrap();
}

#[test]
fn every_disagreement_with_the_ports_is_reported() {
    // Leaves out `start` and `motor`, reads a VAR_OUTPUT, drives nothing
    // that exists, and has a param with no port.
    let m = manifest("motor", "lamp", &format!("{PARAMS}param ghost bool true\n"));
    let errs = m.check(&ports(PROGRAM)).unwrap_err();
    assert_eq!(
        errs,
        [
            CheckError::Undeclared {
                port: "START".into(),
                dir: PortDir::Input
            },
            CheckError::Undeclared {
                port: "MOTOR".into(),
                dir: PortDir::Output
            },
            CheckError::NoPort {
                name: "motor".into(),
                key: "reads"
            },
            CheckError::NoPort {
                name: "lamp".into(),
                key: "drives"
            },
            CheckError::NoPort {
                name: "ghost".into(),
                key: "param"
            },
        ]
    );
    for e in &errs {
        assert_ne!(e.to_string(), "");
    }
}

#[test]
fn an_output_declared_as_a_param_or_read_is_not_a_port() {
    let m = manifest("start motor", "", PARAMS);
    let errs = m.check(&ports(PROGRAM)).unwrap_err();
    assert!(errs.contains(&CheckError::NoPort {
        name: "motor".into(),
        key: "reads"
    }));
    assert!(errs.contains(&CheckError::Undeclared {
        port: "MOTOR".into(),
        dir: PortDir::Output
    }));
}

#[test]
fn a_param_must_fit_its_port_over_its_whole_range() {
    for (params, port, st) in [
        // SINT holds -128..=127; the range reaches 128 even if the value doesn't.
        (
            "param speed real 1.5 min=0 max=10\nparam limit int 7 min=0 max=128",
            "limit",
            StTy::SInt,
        ),
        (
            "param speed real 1.5 min=0 max=10\nparam limit real 7 min=0 max=10",
            "limit",
            StTy::SInt,
        ),
        (
            "param speed real 1.5 min=0 max=10\nparam limit bool true",
            "limit",
            StTy::SInt,
        ),
        (
            "param speed int 1 min=0 max=10\nparam limit int 7 min=0 max=10",
            "speed",
            StTy::Real,
        ),
        // REAL is 32 bit.
        (
            "param speed real 1.5 min=0 max=1e39\nparam limit int 7 min=0 max=10",
            "speed",
            StTy::Real,
        ),
    ] {
        let errs = manifest("start", "motor", params)
            .check(&ports(PROGRAM))
            .unwrap_err();
        assert_eq!(
            errs,
            [CheckError::ParamType {
                name: port.into(),
                st
            }],
            "{params}"
        );
    }
}

// ---- module directories ---------------------------------------------------

fn module_dir(name: &str, manifest: &str, source: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("modules")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE), manifest).unwrap();
    std::fs::write(dir.join(SOURCE), source).unwrap();
    dir
}

fn sig(name: &str, dir: Dir, channel: &str) -> Signal {
    Signal {
        name: name.into(),
        ty: Ty::Bool,
        dir,
        binding: Binding::Local {
            channel: channel.into(),
        },
        unit: None,
        poll: None,
    }
}

fn table() -> SignalTable {
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![sig("start", Dir::In, "di:0"), sig("motor", Dir::Out, "do:0")],
    };
    SignalTable::new(&map, &BTreeMap::from([("motor".to_string(), Value::Bool(false))])).unwrap()
}

fn load(md: &ModuleDir) -> Result<helmsman_plc::Module, String> {
    let s = &md.spec;
    let sizes = ImageSizes {
        i: s.i.len,
        q: s.q.len,
        m: s.m,
    };
    md.module(compile(&md.source, sizes).unwrap())
}

#[test]
fn a_module_directory_runs_with_its_params_and_is_held_without_its_inputs() {
    let src = format!("name belt\nauthor a\nscan_ms 10\nreads start\ndrives motor\n{PARAMS}");
    let dir = module_dir("belt", &src, PROGRAM);
    let md = ModuleDir::read(&dir, &D).unwrap();
    assert_eq!(md.spec.name, "belt");
    let module = load(&md).unwrap();
    let mut t = table();
    let mut rt = Runtime::with_signals(32, 32, vec![module], &t).unwrap();
    let (start, motor) = (t.index("start").unwrap(), t.index("motor").unwrap());

    rt.tick_with(Duration::ZERO, &[], &mut t);
    assert_eq!(rt.status("belt"), Some(Status::Held), "start has no value yet");
    assert_eq!(t.command(motor), Some(Value::Bool(false)));

    t.set_input(start, Some(Value::Bool(true)));
    rt.tick_with(Duration::from_millis(10), &[], &mut t);
    assert_eq!(rt.status("belt"), Some(Status::Running));
    assert_eq!(
        t.command(motor),
        Some(Value::Bool(true)),
        "speed 1.5 and limit 7 came from the manifest, not the program's 0.0 and 0"
    );

    t.set_input(start, None);
    rt.tick_with(Duration::from_millis(20), &[], &mut t);
    assert_eq!(rt.status("belt"), Some(Status::Held));
    assert_eq!(t.command(motor), Some(Value::Bool(false)), "safe again");
}

#[test]
fn a_param_value_changes_behaviour_without_touching_the_logic() {
    let src = "name belt2\nauthor a\nscan_ms 10\nreads start\ndrives motor\n\
               param speed real 0.5 min=0 max=10\nparam limit int 7 min=-128 max=127\n";
    let md = ModuleDir::read(&module_dir("belt2", src, PROGRAM), &D).unwrap();
    let mut t = table();
    let mut rt = Runtime::with_signals(32, 32, vec![load(&md).unwrap()], &t).unwrap();
    t.set_input(t.index("start").unwrap(), Some(Value::Bool(true)));
    rt.tick_with(Duration::ZERO, &[], &mut t);
    assert_eq!(t.command(t.index("motor").unwrap()), Some(Value::Bool(false)));
}

#[test]
fn a_directory_whose_manifest_lies_is_refused_with_every_reason() {
    let src = "name liar\nauthor a\nscan_ms 10\nreads start\ndrives\n";
    let md = ModuleDir::read(&module_dir("liar", src, PROGRAM), &D).unwrap();
    let err = load(&md).unwrap_err();
    assert!(err.contains("does not match module.st"), "{err}");
    assert!(err.contains("VAR_OUTPUT MOTOR is not in the manifest"), "{err}");
    assert!(err.contains("VAR_INPUT SPEED is not in the manifest"), "{err}");
    assert!(err.contains("VAR_INPUT LIMIT is not in the manifest"), "{err}");
}

#[test]
fn unreadable_directories_are_errors_not_panics() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("modules");
    let err = ModuleDir::read(&root.join("does-not-exist"), &D).unwrap_err();
    assert!(err.contains(FILE), "{err}");

    let dir = module_dir("nosource", &with(""), "");
    std::fs::remove_file(dir.join(SOURCE)).unwrap();
    let err = ModuleDir::read(&dir, &D).unwrap_err();
    assert!(err.contains(SOURCE), "{err}");

    let dir = module_dir("badmanifest", "name m\nwhat\n", "");
    let err = ModuleDir::read(&dir, &D).unwrap_err();
    assert!(err.contains(":2: unknown key"), "{err}");
    assert!(err.contains(":0: missing author"), "{err}");

    let dir = module_dir("binary", "", "");
    std::fs::write(dir.join(FILE), [0xff, 0xfe, 0x00]).unwrap();
    let err = ModuleDir::read(&dir, &D).unwrap_err();
    assert!(err.contains("not UTF-8"), "{err}");

    let dir = module_dir("huge", "", "");
    let mut big = with("");
    big.push_str(&"# padding\n".repeat(usize::try_from(MAX_FILE).unwrap() / 10 + 1));
    std::fs::write(dir.join(FILE), big).unwrap();
    let err = ModuleDir::read(&dir, &D).unwrap_err();
    assert!(err.contains("larger than"), "{err}");
}

#[test]
fn runtime_refuses_params_that_do_not_fit_even_without_a_manifest() {
    let md = ModuleDir::read(
        &module_dir(
            "rawparams",
            &format!("name raw\nauthor a\nscan_ms 10\nreads start\ndrives motor\n{PARAMS}"),
            PROGRAM,
        ),
        &D,
    )
    .unwrap();
    for params in [
        vec![("limit".to_string(), Value::Int(200))],
        vec![("ghost".to_string(), Value::Bool(true))],
        vec![("motor".to_string(), Value::Bool(true))],
        vec![
            ("speed".to_string(), Value::Real(1.0)),
            ("SPEED".to_string(), Value::Real(2.0)),
        ],
    ] {
        let mut m = load(&md).unwrap();
        let bad = params.last().unwrap().0.clone();
        m.params = params;
        let errs = Runtime::with_signals(32, 32, vec![m], &table()).unwrap_err();
        assert!(
            errs.contains(&ConfigError::Param {
                module: "raw".into(),
                param: bad
            }),
            "{errs:?}"
        );
    }
}

// ---- properties -------------------------------------------------------------

fn ident() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,8}"
}

/// Free text: anything printable, surrounding blanks trimmed as the parser does.
fn text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 #<>@.,;:!?é山🙂°—-]{0,40}".prop_map(|s| s.trim_matches([' ', '\t']).to_string())
}

fn param_strategy() -> impl Strategy<Value = (u8, i64, i64, i64, f64, f64, f64, bool)> {
    (
        0..3u8,
        any::<i64>(),
        any::<i64>(),
        any::<i64>(),
        -1e12f64..1e12,
        -1e12f64..1e12,
        -1e12f64..1e12,
        any::<bool>(),
    )
}

fn manifest_strategy() -> impl Strategy<Value = Manifest> {
    (
        "[A-Za-z0-9_-]{1,64}",
        text().prop_filter("author is not empty", |s| !s.is_empty()),
        prop::collection::vec(text(), 0..4),
        1u64..=10_000,
        prop::option::of((0usize..100, 0usize..100)),
        prop::option::of((0usize..100, 0usize..100)),
        prop::option::of(0usize..=65_536),
        prop::collection::vec(ident(), 0..4),
        prop::collection::vec(ident(), 0..4),
        prop::collection::vec((ident(), param_strategy()), 0..4),
    )
        .prop_map(|(name, author, prompt, scan, i, q, m, reads, drives, params)| {
            // Unique names across every list, as the format requires.
            let mut k = 0;
            let mut fresh = |n: String| {
                k += 1;
                format!("{n}{k}")
            };
            let reads = reads.into_iter().map(&mut fresh).collect();
            let drives = drives.into_iter().map(&mut fresh).collect();
            let params = params
                .into_iter()
                .map(|(n, (t, a, b, c, x, y, z, flag))| {
                    let name = fresh(n);
                    match t {
                        0 => Param {
                            name,
                            value: Value::Bool(flag),
                            range: None,
                        },
                        1 => {
                            let mut v = [a, b, c];
                            v.sort_unstable();
                            Param {
                                name,
                                value: Value::Int(v[1]),
                                range: Some((Value::Int(v[0]), Value::Int(v[2]))),
                            }
                        }
                        _ => {
                            let mut v = [x, y, z];
                            v.sort_by(f64::total_cmp);
                            Param {
                                name,
                                value: Value::Real(v[1]),
                                range: Some((Value::Real(v[0]), Value::Real(v[2]))),
                            }
                        }
                    }
                })
                .collect();
            let win = |w: Option<(usize, usize)>| w.map(|(s, l)| Window::new(s, l));
            Manifest {
                name,
                author,
                prompt,
                scan: Duration::from_millis(scan),
                i: win(i),
                q: win(q),
                m,
                reads,
                drives,
                params,
            }
        })
}

proptest! {
    #[test]
    fn render_then_parse_is_identity(m in manifest_strategy()) {
        let text = m.render();
        let back = Manifest::parse(&text).map_err(|e| TestCaseError::fail(format!("{e:?}\n{text}")))?;
        prop_assert_eq!(&back, &m, "{}", text);
    }

    #[test]
    fn arbitrary_text_never_panics(src in "(?s).{0,400}") {
        let _ = Manifest::parse(&src);
    }

    #[test]
    fn a_valid_manifest_with_one_line_mangled_never_panics(
        m in manifest_strategy(),
        line in any::<prop::sample::Index>(),
        junk in "(?s).{0,40}",
    ) {
        let text = m.render();
        let mut lines: Vec<&str> = text.lines().collect();
        let k = line.index(lines.len());
        lines[k] = &junk;
        let _ = Manifest::parse(&lines.join("\n"));
    }
}

#[test]
fn a_huge_manifest_parses_without_panicking() {
    let mut src = with("");
    let names: Vec<String> = (0..20_000).map(|n| format!("p{n}")).collect();
    src = src.replace("reads\n", &format!("reads {}\n", names.join(" ")));
    src.push_str(&"prompt x\n".repeat(50_000));
    let m = Manifest::parse(&src).unwrap();
    assert_eq!(m.reads.len(), 20_000);
    let errs = errors(&"\u{0}".repeat(200_000));
    assert_eq!(errs.len(), 6, "one bad line and five missing keys");
}
