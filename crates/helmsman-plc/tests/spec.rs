//! `--module SPEC` parsing: well-formed specs round-trip, anything else is
//! an error rather than a panic, and module names stay safe to log.

use std::path::Path;
use std::time::Duration;

use helmsman_plc::Window;
use helmsman_plc::spec::{Defaults, Spec};
use proptest::prelude::*;

const D: Defaults = Defaults {
    period: Duration::from_millis(10),
    i: Window::new(0, 32),
    q: Window::new(0, 32),
    m: 1024,
};

#[test]
fn a_bare_path_takes_every_default() {
    let s = Spec::parse("dir/blink.st", &D).unwrap();
    assert_eq!(s, Spec::from_path(Path::new("dir/blink.st"), &D));
    assert_eq!(s.name, "blink");
    assert_eq!((s.period, s.i, s.q, s.m), (D.period, D.i, D.q, D.m));
}

#[test]
fn every_key_sets_its_field() {
    let s = Spec::parse("a.xml,name=line-2,scan=250,i=4+8,q=16+2,m=0", &D).unwrap();
    assert_eq!(s.name, "line-2");
    assert_eq!(s.period, Duration::from_millis(250));
    assert_eq!((s.i, s.q, s.m), (Window::new(4, 8), Window::new(16, 2), 0));
}

#[test]
fn malformed_specs_are_errors() {
    for bad in [
        "",
        ",scan=10",
        "a.st,scan",
        "a.st,scan=",
        "a.st,scan=0",
        "a.st,scan=10001",
        "a.st,scan=-1",
        "a.st,scan=10,scan=20",
        "a.st,q=1",
        "a.st,q=+1",
        "a.st,q=1+",
        "a.st,q=-1+2",
        "a.st,q=1+2+3",
        "a.st,q= 1+2",
        "a.st,q=99999999999999999999999+1",
        "a.st,m=65537",
        "a.st,name=",
        "a.st,name=has space",
        "a.st,name=new\nline",
        "a.st,name=\u{1b}[31m",
        "a.st,name=ünï",
        &format!("a.st,name={}", "x".repeat(65)),
        "a.st,Name=x",
        "a.st,",
    ] {
        assert!(Spec::parse(bad, &D).is_err(), "{bad:?} parsed");
    }
}

#[test]
fn names_taken_from_file_stems_are_sanitized() {
    for (path, want) in [
        ("evil\nSTOP, outputs zeroed.st", "evil_STOP__outputs_zeroed"),
        ("\u{1b}[2J.st", "__2J"),
        ("ünï.st", "_n_"),
        (".st", "_st"),
    ] {
        assert_eq!(Spec::from_path(Path::new(path), &D).name, want, "{path:?}");
    }
    let long = format!("{}.st", "y".repeat(200));
    assert_eq!(Spec::from_path(Path::new(&long), &D).name.len(), 64);
}

fn name_ok(n: &str) -> bool {
    (1..=64).contains(&n.len())
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

proptest! {
    #[test]
    fn arbitrary_input_never_panics_and_names_stay_safe(s in ".{0,80}") {
        if let Ok(spec) = Spec::parse(&s, &D) {
            prop_assert!(name_ok(&spec.name), "{:?} -> {:?}", s, spec.name);
        }
        prop_assert!(name_ok(&Spec::from_path(Path::new(&s), &D).name));
    }

    #[test]
    fn well_formed_specs_round_trip(
        name in "[A-Za-z0-9_-]{1,64}",
        scan in 1u64..=10_000,
        i in (0usize..1000, 0usize..1000),
        q in (0usize..1000, 0usize..1000),
        m in 0usize..=65_536,
    ) {
        let text = format!("p.st,m={m},q={}+{},name={name},i={}+{},scan={scan}", q.0, q.1, i.0, i.1);
        let s = Spec::parse(&text, &D).unwrap();
        prop_assert_eq!(s.name, name);
        prop_assert_eq!(s.period, Duration::from_millis(scan));
        prop_assert_eq!(s.i, Window::new(i.0, i.1));
        prop_assert_eq!(s.q, Window::new(q.0, q.1));
        prop_assert_eq!(s.m, m);
    }
}
