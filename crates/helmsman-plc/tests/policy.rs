//! The approval policy's pure parts: the tier classifier, the policy file
//! and its floor, and role PINs with their lockout. The API end of it (real
//! sockets, a real store, a real scan thread) is `tests/approval.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use helmsman_plc::calendar;
use helmsman_plc::manifest::Manifest;
use helmsman_plc::policy::pin::{Gate, Pins, Refused};
use helmsman_plc::policy::{FLOOR, Lockout, MAX_LOCKOUT, Policy, Role, Rule, Tier, classify};
use helmsman_plc::store::{ChangeSet, ModuleFiles};
use proptest::prelude::*;

// ---- change sets to classify -------------------------------------------------

/// A module as a test describes it.
#[derive(Clone, Debug)]
struct Spec {
    drives: Vec<&'static str>,
    q: Option<(usize, usize)>,
    speed: f64,
    max: f64,
    scan: u32,
    body: &'static str,
    comment: bool,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            drives: vec!["lamp"],
            q: None,
            speed: 1.5,
            max: 10.0,
            scan: 10,
            body: "lamp := start AND speed > 1.0;",
            comment: false,
        }
    }
}

fn manifest(name: &str, s: &Spec) -> String {
    let mut m = format!(
        "name {name}\nauthor Test\nscan_ms {}\nreads start\ndrives {}\nparam speed real {:?} min=0 max={:?}\n",
        s.scan,
        s.drives.join(" "),
        s.speed,
        s.max
    );
    if let Some((start, len)) = s.q {
        m += &format!("q {start}+{len}\n");
    }
    if s.comment {
        m = format!("# reviewed\n{m}");
    }
    m
}

fn files(name: &str, s: &Spec) -> ModuleFiles {
    ModuleFiles {
        manifest: manifest(name, s),
        source: format!("PROGRAM {name}\n  {}\nEND_PROGRAM\n", s.body),
    }
}

const MAP: &str =
    "in start bool box di:0\nout lamp bool box do:0 safe=false\nout horn bool box do:1 safe=false\n";
const CAL: &str = "zone UTC\nshift 1 mon-fri 06:00-14:00\ndrive horn on_break\n";

fn cs(modules: &[(&str, Spec)], map: &str, cal: Option<&str>) -> ChangeSet {
    ChangeSet {
        modules: modules
            .iter()
            .map(|(n, s)| ((*n).to_string(), files(n, s)))
            .collect(),
        io_map: Some(map.into()),
        calendar: cal.map(str::to_string),
    }
}

fn base() -> ChangeSet {
    cs(&[("conveyor", Spec::default())], MAP, Some(CAL))
}

fn tier(b: &ChangeSet, t: &ChangeSet) -> Tier {
    classify(b, t).tier
}

fn with(f: impl FnOnce(&mut Spec)) -> ChangeSet {
    let mut s = Spec::default();
    f(&mut s);
    cs(&[("conveyor", s)], MAP, Some(CAL))
}

#[test]
fn nothing_changed_is_param() {
    let c = classify(&base(), &base());
    assert_eq!(c.tier, Tier::Param);
    assert_eq!(c.reasons, [(Tier::Param, "no change".to_string())]);
}

#[test]
fn a_param_within_its_range_is_param_even_with_the_manifest_rerendered() {
    let t = with(|s| {
        s.speed = 2.5;
        s.comment = true;
    });
    let c = classify(&base(), &t);
    assert_eq!(c.tier, Tier::Param, "{c:?}");
    // The proposal path re-renders manifests through Manifest::render.
    let mut r = base();
    let f = r.modules.get_mut("conveyor").unwrap();
    let mut m = Manifest::parse(&f.manifest).unwrap();
    m.params[0].value = helmsman_io::Value::Real(9.0);
    f.manifest = m.render();
    assert_eq!(tier(&base(), &r), Tier::Param);
}

#[test]
fn a_param_outside_the_running_range_is_not_param() {
    // The range is the running manifest's: widening it in the same step
    // does not make an out-of-range value a param change.
    let t = with(|s| {
        s.speed = 50.0;
        s.max = 100.0;
    });
    assert_eq!(tier(&base(), &t), Tier::Logic);
}

#[test]
fn a_program_or_schedule_change_that_drives_the_same_outputs_is_logic() {
    assert_eq!(tier(&base(), &with(|s| s.body = "lamp := start;")), Tier::Logic);
    assert_eq!(tier(&base(), &with(|s| s.scan = 50)), Tier::Logic);
    assert_eq!(
        tier(&base(), &with(|s| s.drives = vec!["LAMP"])),
        Tier::Logic,
        "a drives name in another case is the same signal (ST is case-insensitive): not output-affecting"
    );
}

#[test]
fn a_param_change_hiding_a_new_drives_line_is_output_affecting() {
    let t = with(|s| {
        s.speed = 2.0;
        s.drives = vec!["lamp", "horn"];
    });
    let c = classify(&base(), &t);
    assert_eq!(c.tier, Tier::OutputAffecting, "{c:?}");
    assert!(
        c.reasons
            .iter()
            .any(|(t, why)| *t == Tier::OutputAffecting && why.contains("drives")),
        "{c:?}"
    );
}

#[test]
fn a_param_change_beside_a_safe_value_change_is_output_affecting() {
    let mut t = with(|s| s.speed = 2.0);
    t.io_map = Some(MAP.replace("do:0 safe=false", "do:0 safe=true"));
    assert_eq!(tier(&base(), &t), Tier::OutputAffecting);
    // Any map change at all, even a comment: the map is where outputs live.
    let mut c = base();
    c.io_map = Some(format!("# relabelled\n{MAP}"));
    assert_eq!(tier(&base(), &c), Tier::OutputAffecting);
}

#[test]
fn q_windows_and_driving_modules_coming_and_going_are_output_affecting() {
    assert_eq!(
        tier(&base(), &with(|s| s.q = Some((0, 1)))),
        Tier::OutputAffecting
    );
    let mut added = base();
    added
        .modules
        .insert("horn".into(), files("horn", &Spec::default()));
    assert_eq!(tier(&base(), &added), Tier::OutputAffecting);
    let mut quiet = base();
    let silent = Spec {
        drives: vec![],
        body: "speed := speed;",
        ..Spec::default()
    };
    quiet.modules.insert("watch".into(), files("watch", &silent));
    assert_eq!(tier(&base(), &quiet), Tier::Logic, "a module that drives nothing");
    let mut gone = base();
    gone.modules.clear();
    assert_eq!(tier(&base(), &gone), Tier::OutputAffecting);
}

#[test]
fn calendar_drive_lines_are_output_affecting_and_shift_times_are_logic() {
    let mut t = base();
    t.calendar = Some(CAL.replace("drive horn on_break", "drive horn !working"));
    assert_eq!(tier(&base(), &t), Tier::OutputAffecting);
    t.calendar = Some(CAL.replace("06:00-14:00", "07:00-15:00"));
    assert_eq!(tier(&base(), &t), Tier::Logic);
    t.calendar = Some("zone Europe/Berlin\n".into());
    assert_eq!(tier(&base(), &t), Tier::OutputAffecting, "unreadable: up");
    t.calendar = None;
    assert_eq!(tier(&base(), &t), Tier::OutputAffecting, "drives removed");
}

#[test]
fn an_unparseable_manifest_is_output_affecting() {
    let mut t = base();
    t.modules
        .get_mut("conveyor")
        .unwrap()
        .manifest
        .push_str("drives horn\n");
    assert_eq!(tier(&base(), &t), Tier::OutputAffecting);
}

/// What can drive an output, computed independently of the classifier: per
/// module its drives (any case) and `%Q` window, or "unreadable"; the map
/// text; the calendar's drives, or "unreadable".
fn surface(c: &ChangeSet) -> (BTreeMap<String, String>, Option<String>, String) {
    let modules = c
        .modules
        .iter()
        .map(|(n, f)| {
            let s = Manifest::parse(&f.manifest).map_or_else(
                |_| "unreadable".to_string(),
                |m| {
                    let d: BTreeSet<String> = m.drives.iter().map(|x| x.to_lowercase()).collect();
                    format!("{d:?} {:?}", m.q.map(|w| (w.start, w.len)).filter(|w| w.1 > 0))
                },
            );
            (n.clone(), s)
        })
        .filter(|(_, s)| s != "{} None")
        .collect();
    let cal = c.calendar.as_deref().map_or_else(
        || "[]".to_string(),
        |s| calendar::text::parse(s).map_or_else(|_| "unreadable".to_string(), |k| format!("{:?}", k.drives)),
    );
    (modules, c.io_map.clone(), cal)
}

fn spec_strategy() -> impl Strategy<Value = Spec> {
    (
        prop::sample::subsequence(vec!["lamp", "horn", "Lamp"], 0..=2),
        prop::option::of((0usize..4, 0usize..3)),
        prop::sample::select(vec![0.5, 1.5, 2.5, 11.0]),
        prop::sample::select(vec![10.0f64, 20.0]),
        prop::sample::select(vec![10u32, 20]),
        prop::sample::select(vec!["lamp := start;", "lamp := NOT start;"]),
        any::<bool>(),
    )
        .prop_map(|(drives, q, speed, max, scan, body, comment)| Spec {
            drives,
            q,
            speed,
            max: max.max(speed),
            scan,
            body,
            comment,
        })
}

fn set_strategy() -> impl Strategy<Value = ChangeSet> {
    (
        prop::collection::btree_map(prop::sample::select(vec!["a", "b", "c"]), spec_strategy(), 0..3),
        prop::sample::select(vec![
            MAP.to_string(),
            MAP.replace("safe=false\nout horn", "safe=true\nout horn"),
            MAP.replace("do:1", "do:2"),
        ]),
        prop::sample::select(vec![
            None,
            Some(CAL.to_string()),
            Some(CAL.replace("on_break", "!working")),
            Some(CAL.replace("06:00", "07:00")),
            Some("zone Mars/Olympus\n".to_string()),
        ]),
        any::<bool>(),
    )
        .prop_map(|(mods, map, cal, garble)| {
            let mut c = ChangeSet {
                modules: mods
                    .iter()
                    .map(|(n, s)| ((*n).to_string(), files(n, s)))
                    .collect(),
                io_map: Some(map),
                calendar: cal,
            };
            if garble && let Some(f) = c.modules.values_mut().next() {
                f.manifest.push_str("bogus line\n");
            }
            c
        })
}

/// A target derived from `b` by small edits, so that param-only and logic
/// steps come up often, not just wholesale replacement.
fn step_strategy() -> impl Strategy<Value = (ChangeSet, ChangeSet)> {
    (set_strategy(), set_strategy(), any::<u8>()).prop_map(|(b, other, how)| {
        let t = match how % 4 {
            0 => other,
            1 => {
                // Only param values (maybe out of range) and comments.
                let mut t = b.clone();
                for f in t.modules.values_mut() {
                    f.manifest = f.manifest.replace("speed real 1.5", "speed real 2.5");
                    f.manifest = f.manifest.replace("speed real 0.5", "speed real 11.0");
                }
                t
            }
            2 => {
                // One module from the other set dropped in.
                let mut t = b.clone();
                if let Some((n, f)) = other.modules.into_iter().next() {
                    t.modules.insert(n, f);
                }
                t
            }
            _ => {
                let mut t = b.clone();
                t.io_map = other.io_map;
                t.calendar = other.calendar;
                t
            }
        };
        (b, t)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// No step that alters what can drive an output is ever classified
    /// below output-affecting; a param step changes nothing but param
    /// values; a step's tier is the highest of its reasons.
    #[test]
    fn nothing_that_moves_what_drives_an_output_is_classified_below_output_affecting(
        (b, t) in step_strategy(),
    ) {
        let c = classify(&b, &t);
        if surface(&b) != surface(&t) {
            prop_assert_eq!(c.tier, Tier::OutputAffecting, "{:?}\n{:?}\n{:?}", c, b, t);
        }
        if c.tier == Tier::Param {
            prop_assert_eq!(&b.io_map, &t.io_map);
            prop_assert_eq!(&b.calendar, &t.calendar);
            prop_assert_eq!(b.modules.keys().collect::<Vec<_>>(), t.modules.keys().collect::<Vec<_>>());
            for (n, f) in &b.modules {
                prop_assert_eq!(&f.source, &t.modules[n].source);
            }
        }
        prop_assert_eq!(Some(c.tier), c.reasons.iter().map(|r| r.0).max());
        // Classifying is a function of the two sets alone.
        prop_assert_eq!(classify(&b, &t), c);
    }
}

// ---- the policy file and its floor ------------------------------------------

#[test]
fn no_policy_file_is_the_floor() {
    assert_eq!(Policy::default(), FLOOR);
    assert_eq!(Policy::parse("helmsman-policy 1\n").unwrap(), FLOOR);
    assert_eq!(FLOOR.rule(Tier::Param).role, Role::Operator);
    assert!(!FLOOR.rule(Tier::Param).replay);
    for t in [Tier::Logic, Tier::OutputAffecting] {
        assert_eq!(FLOOR.rule(t).role, Role::Engineer, "{t}");
        assert!(FLOOR.rule(t).replay, "{t}");
    }
    assert!(FLOOR.below_floor().is_empty());
}

#[test]
fn a_stricter_policy_loads_and_round_trips() {
    let src = "\
helmsman-policy 1
# plant 2: integrators only for anything that drives
param             engineer    replay=yes
logic             engineer    replay=yes
output-affecting  integrator  replay=yes   # trailing comment
lockout           3 120
min-replay        300
";
    let p = Policy::parse(src).unwrap();
    assert_eq!(
        p.param,
        Rule {
            role: Role::Engineer,
            replay: true
        }
    );
    assert_eq!(p.output.role, Role::Integrator);
    assert_eq!(
        p.lockout,
        Lockout {
            attempts: 3,
            first: Duration::from_secs(120)
        }
    );
    assert_eq!(p.min_replay, Duration::from_secs(300));
    assert_eq!(Policy::parse(&p.render()).unwrap(), p);
}

#[test]
fn a_policy_that_loosens_the_floor_is_refused_at_load_naming_the_line() {
    for (line, why) in [
        ("logic operator replay=yes", "below the floor"),
        ("logic engineer replay=no", "requires a sim replay"),
        ("output-affecting operator replay=yes", "below the floor"),
        ("output-affecting engineer replay=no", "requires a sim replay"),
        ("lockout 6 30", "lockout"),
        ("lockout 0 30", "lockout"),
        ("lockout 5 29", "lockout"),
        ("lockout 5 7200", "lockout"),
    ] {
        let errs = Policy::parse(&format!("helmsman-policy 1\n# first\n{line}\n")).unwrap_err();
        assert!(
            errs.iter().any(|(n, e)| *n == 3 && e.contains(why)),
            "{line}: {errs:?}"
        );
    }
}

#[test]
fn a_higher_tier_may_not_ask_less_than_a_lower_one() {
    let errs = Policy::parse("helmsman-policy 1\nparam integrator replay=no\n").unwrap_err();
    assert!(
        errs.iter().any(|(_, e)| e.contains("logic asks less than param")),
        "{errs:?}"
    );
    let errs = Policy::parse("helmsman-policy 1\nparam operator replay=yes\nlogic engineer replay=yes\n")
        .map(|_| ())
        .err();
    assert_eq!(errs, None, "equal or stricter going up is fine");
}

#[test]
fn malformed_policy_files_are_refused_with_every_problem() {
    for (src, why) in [
        ("", "empty"),
        (
            "param engineer replay=no\n",
            "expected \"helmsman-policy 1\" first",
        ),
        ("helmsman-policy 2\n", "expected \"helmsman-policy 1\" first"),
        ("helmsman-policy 1\nparam engineer\n", "expected: param ROLE"),
        ("helmsman-policy 1\nparam root replay=no\n", "expected operator"),
        (
            "helmsman-policy 1\nparam engineer replay=maybe\n",
            "replay=yes or replay=no",
        ),
        (
            "helmsman-policy 1\nparam engineer replay=no\nparam engineer replay=no\n",
            "twice",
        ),
        ("helmsman-policy 1\nallow everything\n", "expected param"),
        ("helmsman-policy 1\nmin-replay -1\n", "whole seconds"),
        ("helmsman-policy 1\nlockout 5 30 extra\n", "expected: lockout"),
        (
            "helmsman-policy 1\nparam engineer replay=no\u{202e}\n",
            "printable ASCII",
        ),
    ] {
        let errs = Policy::parse(src).unwrap_err();
        assert!(errs.iter().any(|(_, e)| e.contains(why)), "{src:?}: {errs:?}");
    }
    assert!(Policy::parse(&"#".repeat(70_000)).is_err(), "oversized");
}

fn role_strategy() -> impl Strategy<Value = Role> {
    prop::sample::select(Role::ALL.to_vec())
}

fn rule_strategy() -> impl Strategy<Value = Rule> {
    (role_strategy(), any::<bool>()).prop_map(|(role, replay)| Rule { role, replay })
}

proptest! {
    /// Whatever a policy file says, what loads is never looser than the
    /// floor, and what the floor allows loads.
    #[test]
    fn whatever_loads_is_at_least_the_floor(
        param in rule_strategy(), logic in rule_strategy(), output in rule_strategy(),
        attempts in 0u32..12, first in 0u64..8000, min in 0u64..1000,
    ) {
        let p = Policy {
            param, logic, output,
            lockout: Lockout { attempts, first: Duration::from_secs(first) },
            min_replay: Duration::from_secs(min),
        };
        match Policy::parse(&p.render()) {
            Ok(q) => {
                prop_assert_eq!(q, p);
                for t in Tier::ALL {
                    prop_assert!(q.rule(t).at_least(FLOOR.rule(t)), "{}", t);
                }
                prop_assert!(q.lockout.attempts >= 1 && q.lockout.attempts <= FLOOR.lockout.attempts);
                prop_assert!(q.lockout.first >= FLOOR.lockout.first && q.lockout.first <= MAX_LOCKOUT);
            }
            Err(_) => prop_assert!(!p.below_floor().is_empty()),
        }
    }
}

// ---- PINs and the lockout ----------------------------------------------------

fn pins() -> Pins {
    let mut p = Pins::default();
    p.set(Role::Operator, "op-246810", 1, [1; 16]).unwrap();
    p.set(Role::Engineer, "eng-135797", 1, [2; 16]).unwrap();
    p
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "policy-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_pin_verifies_only_for_its_role_and_is_never_stored_or_shown() {
    let p = pins();
    assert!(p.verify(Role::Engineer, "eng-135797"));
    assert!(!p.verify(Role::Operator, "eng-135797"), "another role's PIN");
    assert!(!p.verify(Role::Integrator, "eng-135797"), "a role with no PIN");
    assert!(!p.verify(Role::Engineer, "eng-13579"));
    assert!(!p.verify(Role::Engineer, ""));
    let text = p.render();
    let shown = format!("{p:?}");
    for pin in ["op-246810", "eng-135797"] {
        assert!(!text.contains(pin) && !shown.contains(pin));
    }
    assert!(!shown.contains("pbkdf2"), "Debug shows roles only: {shown}");
    // Same PIN, different salt: different hash.
    let mut q = Pins::default();
    q.set(Role::Engineer, "eng-135797", 1, [3; 16]).unwrap();
    assert_ne!(q.render(), p.render().replace("operator", "x"));
}

#[test]
fn weak_pins_are_refused() {
    let mut p = Pins::default();
    for bad in ["", "12345", "123 456", "12345\n", &"9".repeat(65), "pïn-123456"] {
        assert!(p.set(Role::Operator, bad, 1, [0; 16]).is_err(), "{bad:?}");
    }
    assert!(
        p.set(Role::Operator, "123456", 0, [0; 16]).is_err(),
        "zero iterations"
    );
}

#[test]
fn the_pin_file_is_private_and_round_trips() {
    let dir = tmp("pins");
    assert_eq!(Pins::load(&dir, 1).unwrap(), Pins::default(), "no file: no PINs");
    let p = pins();
    p.save(&dir).unwrap();
    let path = dir.join(helmsman_plc::policy::pin::FILE);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let back = Pins::load(&dir, 1).unwrap();
        assert!(back.verify(Role::Engineer, "eng-135797"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = Pins::load(&dir, 1).unwrap_err();
        assert!(e.contains("readable by others"), "{e}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let e = Pins::load(&dir, 100_000).unwrap_err();
    assert!(e.contains("fewer than 100000"), "a cheap hash is refused: {e}");
}

#[test]
fn malformed_pin_files_are_refused() {
    let good = pins().render();
    let line = good.lines().nth(1).unwrap().to_string();
    for bad in [
        String::new(),
        "helmsman-pins 2\n".into(),
        format!("helmsman-pins 1\n{line}\n{line}\n"),
        format!("helmsman-pins 1\n{}\n", line.replace("pbkdf2-sha256", "md5")),
        format!("helmsman-pins 1\n{}\n", line.replace("operator", "root")),
        format!("helmsman-pins 1\n{}0\n", line),
        format!("helmsman-pins 1\n{}\n", &line[..line.len() - 2]),
    ] {
        assert!(Pins::parse(&bad, 1).is_err(), "{bad:?}");
    }
    assert!(Pins::parse(&good, 1).is_ok());
}

/// A clock a test moves by hand.
fn clock() -> (Arc<AtomicU64>, helmsman_plc::policy::pin::Clock) {
    let t0 = Instant::now();
    let off = Arc::new(AtomicU64::new(0));
    let o = Arc::clone(&off);
    (
        off,
        Arc::new(move || t0 + Duration::from_secs(o.load(Ordering::SeqCst))),
    )
}

#[test]
fn wrong_pins_lock_entry_with_doubling_backoff_and_a_right_one_clears_it() {
    let (off, clk) = clock();
    let g = Gate::new(pins(), FLOOR.lockout, clk);
    for _ in 0..4 {
        assert_eq!(g.check(Role::Engineer, "guess-000"), Err(Refused::Wrong));
    }
    assert_eq!(
        g.check(Role::Engineer, "eng-135797"),
        Ok(()),
        "4 wrong: still open"
    );
    for _ in 0..5 {
        assert_eq!(g.check(Role::Operator, "guess-000"), Err(Refused::Wrong));
    }
    // Locked: even the right PIN of any role is not checked.
    assert_eq!(
        g.check(Role::Engineer, "eng-135797"),
        Err(Refused::Locked(Duration::from_secs(30)))
    );
    assert_eq!(
        g.check(Role::Operator, "op-246810"),
        Err(Refused::Locked(Duration::from_secs(30)))
    );
    off.store(29, Ordering::SeqCst);
    assert!(matches!(
        g.check(Role::Operator, "op-246810"),
        Err(Refused::Locked(_))
    ));
    off.store(30, Ordering::SeqCst);
    // One more wrong: locked again, twice as long.
    assert_eq!(g.check(Role::Operator, "guess-001"), Err(Refused::Wrong));
    assert_eq!(
        g.check(Role::Operator, "op-246810"),
        Err(Refused::Locked(Duration::from_secs(60)))
    );
    off.store(90, Ordering::SeqCst);
    assert_eq!(g.check(Role::Operator, "guess-002"), Err(Refused::Wrong));
    assert_eq!(
        g.check(Role::Operator, "op-246810"),
        Err(Refused::Locked(Duration::from_secs(120)))
    );
    off.store(210, Ordering::SeqCst);
    assert_eq!(
        g.check(Role::Operator, "op-246810"),
        Ok(()),
        "a right PIN clears it"
    );
    for _ in 0..4 {
        assert_eq!(g.check(Role::Operator, "guess-003"), Err(Refused::Wrong));
    }
    assert_eq!(g.check(Role::Operator, "op-246810"), Ok(()));
}

#[test]
fn the_backoff_stops_at_an_hour() {
    let (off, clk) = clock();
    let g = Gate::new(pins(), FLOOR.lockout, clk);
    let mut t = 0;
    let mut last = Duration::ZERO;
    for _ in 0..80 {
        off.store(t, Ordering::SeqCst);
        let _ = g.check(Role::Engineer, "guess-000");
        if let Err(Refused::Locked(d)) = g.check(Role::Engineer, "guess-000") {
            last = d;
            t += d.as_secs();
        }
    }
    assert_eq!(last, MAX_LOCKOUT);
}

#[test]
fn parallel_guessing_gets_no_more_tries_than_serial() {
    let (_off, clk) = clock();
    let g = Arc::new(Gate::new(pins(), FLOOR.lockout, clk));
    let wrong = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..16)
        .map(|k| {
            let (g, wrong) = (Arc::clone(&g), Arc::clone(&wrong));
            std::thread::spawn(move || {
                for n in 0..10 {
                    if g.check(Role::Engineer, &format!("guess-{k}-{n}")) == Err(Refused::Wrong) {
                        wrong.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(
        wrong.load(Ordering::SeqCst),
        5,
        "160 guesses from 16 threads: only 5 were ever checked"
    );
}
