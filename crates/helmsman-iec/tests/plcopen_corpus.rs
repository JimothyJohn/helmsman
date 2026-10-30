//! Real editor exports: every `plc.xml` under `$HELMSMAN_PLCOPEN_CORPUS`
//! either compiles or is refused for a reason this test names: a feature
//! this runtime does not support, a library the editor's extensions supply
//! (Beremiz's SVGHMI types, `OpenPLC`'s board blocks), or a known defect in
//! the project itself. A panic, or any other error, fails the test.
//! Projects that compile today are listed and must keep compiling; two of
//! them also run and are checked scan by scan.
//!
//! `scripts/plcopen-corpus.sh` fetches the corpus (Beremiz and `OpenPLC`
//! Editor example projects, pinned by commit and SHA-256) and runs this
//! test against it. The projects are GPL, so they are not copied into this
//! repository; without the variable the test has nothing to read and
//! passes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helmsman_iec::{Image, ImageSizes, Program, Value, compile_plcopen};

const SIZES: ImageSizes = ImageSizes {
    i: 32,
    q: 32,
    m: 1024,
};

/// Diagnostics that mean "valid project, feature not supported here".
const UNSUPPORTED: &[&str] = &["not supported", "only one PROGRAM is supported"];

/// Projects (path suffix) that must compile.
const COMPILES: &[&str] = &[
    "beremiz/exemples/svghmi_json_table/plc.xml",
    "beremiz/tests/projects/genericmake/plc.xml",
    "beremiz/tests/projects/plc_sdk_minimal/plc.xml",
    "openplc/examples/Blink/plc.xml",
    "openplc/examples/Random_Generator_Pragma/plc.xml",
    "openplc/examples/Traffic_Light_FBD/plc.xml",
    "openplc/tests/projects/genericmake/plc.xml",
];

/// Defects in the projects themselves: (path suffix, diagnostic).
const DEFECTS: &[(&str, &str)] = &[(
    // An ADD block with IN1 left unconnected; Beremiz never compiles it.
    "openplc/tests/projects/svghmi_v2/plc.xml",
    "ADD needs at least 2 connected inputs, has 1",
)];

fn projects(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            projects(&p, out);
        } else if p.file_name().is_some_and(|n| n == "plc.xml") {
            out.push(p);
        }
    }
}

/// `unknown type X` where the project uses X but never defines it: a type
/// an editor extension provides.
fn external_type(msg: &str, src: &str) -> bool {
    let Some(name) = msg.strip_prefix("unknown type ") else {
        return false;
    };
    let up = src.to_ascii_uppercase();
    up.contains(&format!("<DERIVED NAME=\"{name}\""))
        && !up.contains(&format!("<DATATYPE NAME=\"{name}\""))
        && !up.contains(&format!("<POU NAME=\"{name}\""))
}

fn corpus() -> Option<(PathBuf, Vec<PathBuf>)> {
    let root = PathBuf::from(std::env::var_os("HELMSMAN_PLCOPEN_CORPUS")?);
    let mut files = Vec::new();
    projects(&root, &mut files);
    assert!(!files.is_empty(), "no plc.xml under {}", root.display());
    Some((root, files))
}

fn rel(root: &Path, f: &Path) -> String {
    f.strip_prefix(root)
        .unwrap_or(f)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn real_exports_compile_or_say_why_not() {
    let Some((root, files)) = corpus() else {
        eprintln!("HELMSMAN_PLCOPEN_CORPUS unset: no corpus to read");
        return;
    };
    let mut report = String::new();
    let mut bad = 0;
    for f in &files {
        let bytes = std::fs::read(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let src = String::from_utf8_lossy(&bytes);
        let name = rel(&root, f);
        let must_compile = COMPILES.iter().any(|c| name.ends_with(c));
        let defect = DEFECTS.iter().find(|(p, _)| name.ends_with(p)).map(|d| d.1);
        match std::panic::catch_unwind(|| compile_plcopen(&src, SIZES)) {
            Err(_) => {
                bad += 1;
                writeln!(report, "PANIC  {name}").unwrap();
            }
            Ok(Ok(_)) => writeln!(report, "ok     {name}").unwrap(),
            Ok(Err(diags)) => {
                let other: Vec<String> = diags
                    .iter()
                    .filter(|d| {
                        !UNSUPPORTED.iter().any(|u| d.msg.contains(u))
                            && !external_type(&d.msg, &src)
                            && defect.is_none_or(|m| !d.msg.contains(m))
                    })
                    .map(ToString::to_string)
                    .collect();
                let first = diags.first().map(ToString::to_string).unwrap_or_default();
                if must_compile || !other.is_empty() {
                    bad += 1;
                    let all: Vec<String> = diags.iter().map(ToString::to_string).collect();
                    writeln!(report, "FAIL   {name}:\n  {}", all.join("\n  ")).unwrap();
                } else if defect.is_some() {
                    writeln!(report, "defect {name}: {first}").unwrap();
                } else {
                    writeln!(report, "refused {name}: {first}").unwrap();
                }
            }
        }
    }
    eprint!("{report}");
    assert_eq!(bad, 0, "{bad} of {} projects broke:\n{report}", files.len());
}

fn find(files: &[PathBuf], root: &Path, suffix: &str) -> Option<String> {
    let f = files.iter().find(|f| rel(root, f).ends_with(suffix))?;
    Some(std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display())))
}

fn boolean(p: &Program, path: &str) -> bool {
    match p.get(path) {
        Some(Value::Bool(b)) => b,
        other => panic!("{path} = {other:?}"),
    }
}

/// `OpenPLC` Editor's Blink: a contact on `blink_led` (negated) into TON0,
/// TON0's ENO enabling TOF0, TOF0.Q into a coil on `blink_led`. 500 ms on,
/// 500 ms off, starting off.
#[test]
fn openplc_blink_ladder_blinks() {
    let Some((root, files)) = corpus() else { return };
    let Some(xml) = find(&files, &root, "openplc/examples/Blink/plc.xml") else {
        return;
    };
    let mut p = compile_plcopen(&xml, SIZES).unwrap_or_else(|d| panic!("{d:#?}"));
    let mut img = Image::new(SIZES);
    let mut trace = Vec::new();
    for ms in (0..=2500).step_by(10) {
        p.scan_at(&mut img, Duration::from_millis(ms)).unwrap();
        trace.push(boolean(&p, "blink_led"));
    }
    let edges: Vec<usize> = trace
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] != w[1])
        .map(|(k, _)| (k + 1) * 10)
        .collect();
    // On at 500 ms; TOF0 holds it 500 ms after TON0 drops; TON0 restarts
    // the scan after blink_led goes low.
    assert_eq!(edges.first(), Some(&500), "{edges:?}");
    for w in edges.windows(2) {
        assert!((500..=520).contains(&(w[1] - w[0])), "{edges:?}");
    }
    assert!(edges.len() >= 3, "{edges:?}");
}

/// `OpenPLC` Editor's `Traffic_Light_FBD`: located outputs `%QX0.0..3`,
/// `First_cycle AT %QX0.3 := TRUE`, falling-edge inputs into three TOFs.
/// Red 6 s, then green 6 s, then orange 4 s, then red again.
#[test]
fn openplc_traffic_light_cycles_through_located_outputs() {
    let Some((root, files)) = corpus() else { return };
    let Some(xml) = find(&files, &root, "openplc/examples/Traffic_Light_FBD/plc.xml") else {
        return;
    };
    let mut p = compile_plcopen(&xml, SIZES).unwrap_or_else(|d| panic!("{d:#?}"));
    let mut img = Image::new(SIZES);
    let mut at = |ms: u64| {
        p.scan_at(&mut img, Duration::from_millis(ms)).unwrap();
        img.q[0] & 0b0111
    };
    let (red, orange, green) = (0b001, 0b010, 0b100);
    let mut lights = Vec::new();
    for ms in (0..=40_000).step_by(100) {
        let q = at(ms);
        // Between two lights there may be a scan with none lit: the next
        // TOF starts on the falling edge of the previous light.
        if q != 0 && lights.last().is_none_or(|(_, l)| *l != q) {
            lights.push((ms, q));
        }
    }
    let want = [
        (0, red),
        (6000, green),
        (12000, orange),
        (16000, red),
        (22000, green),
        (28000, orange),
        (32000, red),
    ];
    for (k, (ms, l)) in want.iter().enumerate() {
        let (got_ms, got) = lights[k];
        assert_eq!(got, *l, "light {k}: {lights:?}");
        assert!(got_ms.abs_diff(*ms) <= 1000, "light {k} at {got_ms}: {lights:?}");
    }
}
