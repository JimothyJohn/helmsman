//! Local accounts: the accounts file, logins with per-account and box-wide
//! lockout, sessions (expiry, idle, logout, revocation), and approvals
//! over real sockets that carry the person into the audit log and the
//! snapshot history. Nothing is mocked; the only test hook is the clock
//! the lockouts and sessions read (so an hour does not take an hour).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::Value;
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Limits, Live, MemAudit, MemProposals, ModuleInfo, Plant,
    Publisher, Shared, swap,
};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::policy::account::{
    self, Accounts, LoginRefused, Logins, Mode, SessionLimits, SessionRefused,
};
use helmsman_plc::policy::pin::{Clock, Gate, Pins};
use helmsman_plc::policy::{FLOOR, Lockout, Role};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use proptest::prelude::*;
use serde_json::{Value as Json, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OPERATOR_PIN: &str = "op-246810";
const ENGINEER_PIN: &str = "eng-135797";
const ALICE: &str = "alice-pass-31337";
const BOB: &str = "bob-pass-27182";
const CAROL: &str = "carol-pass-16180";

const MANIFEST: &str = "\
name     conveyor
author   Test <test@example.com>
prompt   Light the lamp while start is held and the speed is up.
scan_ms  2
reads    start
drives   lamp
param    speed real 1.5 min=0 max=10
";

const SOURCE: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; speed : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n : UDINT; END_VAR
  n := n + 1;
  lamp := start AND speed > 1.0;
END_PROGRAM";

/// The same outputs, other logic: a `logic` change (engineer + replay).
const SOURCE_V2: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; speed : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n : UDINT; END_VAR
  n := n + 1;
  lamp := start AND speed > 0.25;
END_PROGRAM";

const MAP: &str = "\
in  start bool box di:0
out lamp  bool box do:0 safe=false
out horn  bool box do:1 safe=false
";

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "accounts-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// alice engineer, bob operator, carol integrator; one PBKDF2 iteration
/// keeps the tests quick (the binary refuses fewer than
/// `pin::MIN_ITERATIONS`; tests/bin.rs).
fn accounts() -> Accounts {
    let mut a = Accounts::default();
    a.add("alice", Role::Engineer, ALICE, 1, [1; 16]).unwrap();
    a.add("bob", Role::Operator, BOB, 1, [2; 16]).unwrap();
    a.add("carol", Role::Integrator, CAROL, 1, [3; 16]).unwrap();
    a
}

/// A clock the test moves, in whole seconds from a fixed start.
fn clock() -> (Clock, Arc<AtomicU64>) {
    let t0 = Instant::now();
    let secs = Arc::new(AtomicU64::new(0));
    let s = Arc::clone(&secs);
    (
        Arc::new(move || t0 + Duration::from_secs(s.load(Ordering::SeqCst))),
        secs,
    )
}

fn logins(dir: &Path) -> (Logins, Arc<AtomicU64>) {
    let (c, secs) = clock();
    (Logins::new(dir.to_path_buf(), 1, FLOOR.lockout, c), secs)
}

// ---- the accounts file -----------------------------------------------------

#[test]
fn the_file_round_trips_and_never_holds_a_password() {
    let dir = tmp("file");
    assert_eq!(Accounts::load(&dir, 1).unwrap(), None, "no file: PINs");
    let a = accounts();
    a.save(&dir).unwrap();
    let text = std::fs::read_to_string(dir.join(account::FILE)).unwrap();
    assert!(text.starts_with(account::HEADER), "{text}");
    for p in [ALICE, BOB, CAROL] {
        assert!(!text.contains(p), "{text}");
    }
    assert!(text.contains("alice engineer enabled pbkdf2-sha256 1 "), "{text}");
    assert_eq!(Accounts::load(&dir, 1).unwrap(), Some(a.clone()));
    assert!(!format!("{a:?}").contains("pbkdf2"), "Debug shows no hashes");
    // The iteration floor is the PIN file's.
    let e = Accounts::load(&dir, helmsman_plc::policy::pin::MIN_ITERATIONS).unwrap_err();
    assert!(e.contains("fewer than 100000"), "{e}");
}

#[cfg(unix)]
#[test]
fn an_accounts_file_others_can_read_is_refused_and_so_is_every_credential() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("perms");
    accounts().save(&dir).unwrap();
    let mode = std::fs::metadata(dir.join(account::FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "written private");
    std::fs::set_permissions(dir.join(account::FILE), std::fs::Permissions::from_mode(0o644)).unwrap();
    let e = Accounts::load(&dir, 1).unwrap_err();
    assert!(e.contains("readable by others"), "{e}");
    let (l, _) = logins(&dir);
    assert!(matches!(l.mode(), Mode::Broken(_)));
    assert!(matches!(
        l.login("alice", ALICE),
        Err(LoginRefused::Unavailable(_))
    ));
    // Over the API: no login, and no PIN either (fail closed).
    let b = Bench::with_accounts(None);
    accounts().save(&b.state).unwrap();
    std::fs::set_permissions(
        b.state.join(account::FILE),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    assert_eq!(get(b.addr, "/api/policy").json()["credentials"], "unavailable");
    let r = login(b.addr, "alice", ALICE);
    assert_eq!(r.status, 503, "{r:?}");
    assert!(!r.body.contains(ALICE));
    let id = b.param(0.5);
    let r = b.apply_pin(id, "operator", OPERATOR_PIN);
    assert_eq!(r.status, 503, "{r:?}");
    assert_eq!(b.speed(), json!(1.5), "nothing applied");
}

#[test]
fn malformed_files_are_refused_line_by_line() {
    let a = accounts().render();
    let line = a.lines().nth(1).unwrap().to_string();
    for (bad, want) in [
        (String::new(), ":0:"),
        ("helmsman-accounts 2\n".to_string(), "expected"),
        (format!("{}\n{line}\n{line}\n", account::HEADER), "given twice"),
        (line.replace("alice", "Alice"), "expected"),
        (
            format!("{}\n{}\n", account::HEADER, line.replace("engineer", "root")),
            "expected: USER ROLE",
        ),
        (
            format!("{}\n{}\n", account::HEADER, line.replace("enabled", "maybe")),
            "expected: USER ROLE",
        ),
        (
            format!("{}\n{} extra\n", account::HEADER, line),
            "expected: USER ROLE",
        ),
        (format!("{}\n{}\u{e9}\n", account::HEADER, line), "printable"),
        ("x".repeat(account::MAX_FILE + 1), "larger than"),
    ] {
        let errs = Accounts::parse(&bad, 1).unwrap_err();
        let text = format!("{errs:?}");
        let want = want.trim_start_matches(':').trim_end_matches(':');
        assert!(text.contains(want), "{bad:?}: {text}");
    }
}

#[test]
fn adding_checks_names_and_passwords() {
    let mut a = Accounts::default();
    for bad in ["", "Alice", "a b", ".x", &"a".repeat(account::USER_LEN + 1)] {
        assert!(a.add(bad, Role::Operator, ALICE, 1, [0; 16]).is_err(), "{bad:?}");
    }
    for bad in [
        "short",
        "tab\there-long",
        &"p".repeat(account::PASSWORD_LEN.1 + 1),
    ] {
        assert!(a.add("dave", Role::Operator, bad, 1, [0; 16]).is_err(), "{bad:?}");
    }
    a.add("dave", Role::Operator, ALICE, 1, [0; 16]).unwrap();
    assert!(a.add("dave", Role::Engineer, BOB, 1, [0; 16]).is_err(), "taken");
    assert!(a.passwd("eve", BOB, 1, [0; 16]).is_err());
    assert!(a.set_disabled("eve", true).is_err());
    assert_eq!(a.len(), 1);
}

fn arb_account() -> impl Strategy<Value = (String, Role, bool, String)> {
    (
        "[a-z0-9][a-z0-9._-]{0,31}",
        prop_oneof![Just(Role::Operator), Just(Role::Engineer), Just(Role::Integrator)],
        any::<bool>(),
        "[ -~]{8,40}",
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Whatever is written reads back as itself.
    #[test]
    fn rendered_accounts_parse_back(list in prop::collection::vec(arb_account(), 0..12)) {
        let mut a = Accounts::default();
        for (n, (user, role, disabled, pass)) in list.iter().enumerate() {
            let salt = [u8::try_from(n % 256).unwrap(); 16];
            if a.add(user, *role, pass, 1, salt).is_ok() {
                a.set_disabled(user, *disabled).unwrap();
            }
        }
        let back = Accounts::parse(&a.render(), 1).unwrap();
        prop_assert_eq!(back, a);
    }

    /// Any text: refused, or accepted as something that renders and reads
    /// back the same; never a panic.
    #[test]
    fn any_text_parses_or_is_refused(src in "(helmsman-accounts 1\n)?([ -~\t]{0,90}\n){0,8}") {
        if let Ok(a) = Accounts::parse(&src, 1) {
            prop_assert_eq!(Accounts::parse(&a.render(), 1).unwrap(), a);
        }
    }

    /// Mangling one byte of a good line never yields an account with a
    /// higher role than the line had.
    #[test]
    fn a_mangled_line_never_gains_a_role(k in 0usize..200, b in 0x20u8..0x7f) {
        let a = accounts();
        let mut text = a.render().into_bytes();
        let k = k % text.len();
        text[k] = b;
        let text = String::from_utf8(text).unwrap();
        if let Ok(m) = Accounts::parse(&text, 1) {
            for acc in m.iter() {
                if let Some(orig) = a.get(&acc.user) {
                    prop_assert!(acc.role <= orig.role, "{acc:?}");
                }
            }
        }
    }
}

// ---- logins and lockouts ----------------------------------------------------

#[test]
fn wrong_passwords_lock_that_account_only_and_back_off() {
    let dir = tmp("lock");
    accounts().save(&dir).unwrap();
    let (l, secs) = logins(&dir);
    let n = FLOOR.lockout.attempts;
    for k in 0..n {
        assert_eq!(
            l.login("alice", &format!("guess-{k}-xxxx")).unwrap_err(),
            LoginRefused::Wrong
        );
    }
    // Locked now: even the right password is not checked.
    assert_eq!(
        l.login("alice", ALICE).unwrap_err(),
        LoginRefused::Locked(FLOOR.lockout.first)
    );
    // Others are not.
    assert!(l.login("bob", BOB).is_ok());
    secs.fetch_add(FLOOR.lockout.first.as_secs(), Ordering::SeqCst);
    // One more wrong one doubles the wait.
    assert_eq!(
        l.login("alice", "guess-again-xx").unwrap_err(),
        LoginRefused::Wrong
    );
    assert_eq!(
        l.login("alice", ALICE).unwrap_err(),
        LoginRefused::Locked(FLOOR.lockout.first * 2)
    );
    secs.fetch_add(FLOOR.lockout.first.as_secs() * 2, Ordering::SeqCst);
    let ok = l.login("alice", ALICE).unwrap();
    assert_eq!((ok.who.user.as_str(), ok.who.role), ("alice", Role::Engineer));
    // A right password clears the count.
    for k in 0..n - 1 {
        assert_eq!(
            l.login("alice", &format!("guess-{k}-yyyy")).unwrap_err(),
            LoginRefused::Wrong
        );
    }
    assert!(l.login("alice", ALICE).is_ok());
}

#[test]
fn guesses_spread_over_accounts_lock_every_login() {
    let dir = tmp("global");
    accounts().save(&dir).unwrap();
    let (l, secs) = logins(&dir);
    let allowance = FLOOR.lockout.attempts * account::GLOBAL_FACTOR;
    // Fewer than the per-account allowance each, over known and unknown
    // names alike.
    let names = ["alice", "bob", "carol", "dave", "eve", "mallory", "trent"];
    for k in 0..allowance {
        let user = names[usize::try_from(k).unwrap() % names.len()];
        assert_eq!(
            l.login(user, "wrong-password").unwrap_err(),
            LoginRefused::Wrong,
            "{k}"
        );
    }
    for (user, pass) in [("alice", ALICE), ("bob", BOB), ("carol", CAROL)] {
        assert!(
            matches!(l.login(user, pass), Err(LoginRefused::Locked(_))),
            "{user}: every login locked"
        );
    }
    secs.fetch_add(FLOOR.lockout.first.as_secs(), Ordering::SeqCst);
    // The window still holds them: one more wrong one locks longer.
    assert_eq!(
        l.login("dave", "wrong-password").unwrap_err(),
        LoginRefused::Wrong
    );
    assert_eq!(
        l.login("carol", CAROL).unwrap_err(),
        LoginRefused::Locked(FLOOR.lockout.first * 2)
    );
    // Past the window everything is forgotten.
    secs.fetch_add(account::GLOBAL_WINDOW.as_secs(), Ordering::SeqCst);
    assert!(l.login("carol", CAROL).is_ok());
}

#[test]
fn concurrent_guesses_are_counted_exactly() {
    let dir = tmp("concurrent");
    accounts().save(&dir).unwrap();
    let (l, _) = logins(&dir);
    let l = Arc::new(l);
    let outcomes: Vec<Result<(), LoginRefused>> = (0..16)
        .map(|k| {
            let l = Arc::clone(&l);
            thread::spawn(move || l.login("alice", &format!("parallel-guess-{k}")).map(drop))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    let wrong = outcomes
        .iter()
        .filter(|o| **o == Err(LoginRefused::Wrong))
        .count();
    let locked = outcomes
        .iter()
        .filter(|o| matches!(o, Err(LoginRefused::Locked(_))))
        .count();
    assert_eq!(
        wrong,
        usize::try_from(FLOOR.lockout.attempts).unwrap(),
        "{outcomes:?}"
    );
    assert_eq!(wrong + locked, 16);
}

#[test]
fn concurrent_logins_each_get_their_own_session() {
    let dir = tmp("logins");
    accounts().save(&dir).unwrap();
    let (l, _) = logins(&dir);
    let l = Arc::new(l);
    let tokens: Vec<String> = (0..account::SESSIONS_PER_USER)
        .map(|_| {
            let l = Arc::clone(&l);
            thread::spawn(move || l.login("bob", BOB).unwrap().token)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    let mut unique = tokens.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), tokens.len(), "distinct tokens");
    for t in &tokens {
        assert_eq!(t.len(), 64);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(l.session(t).unwrap().user, "bob");
    }
    // One more login ends bob's oldest; nobody else's.
    let alice = l.login("alice", ALICE).unwrap().token;
    let _ = l.login("bob", BOB).unwrap();
    let live = tokens.iter().filter(|t| l.session(t).is_ok()).count();
    assert_eq!(live, account::SESSIONS_PER_USER - 1);
    assert!(l.session(&alice).is_ok());
}

#[test]
fn the_session_table_is_bounded() {
    let dir = tmp("bounded");
    accounts().save(&dir).unwrap();
    let (c, _) = clock();
    let l = Logins::new(dir, 1, FLOOR.lockout, c).with_limits(SessionLimits {
        max: 2,
        ..SessionLimits::default()
    });
    l.login("alice", ALICE).unwrap();
    l.login("bob", BOB).unwrap();
    assert_eq!(l.login("carol", CAROL).unwrap_err(), LoginRefused::Full);
    assert_eq!(l.sessions(), 2);
}

#[test]
fn unknown_users_cost_as_much_as_known_ones() {
    // Enough iterations that the PBKDF2 dominates.
    let dir = tmp("timing");
    let mut a = Accounts::default();
    a.add("alice", Role::Engineer, ALICE, 20_000, [1; 16]).unwrap();
    a.save(&dir).unwrap();
    let (c, _) = clock();
    let l = Logins::new(
        dir,
        1,
        Lockout {
            attempts: 1000,
            first: Duration::from_secs(30),
        },
        c,
    );
    let time = |user: &str| {
        let mut v: Vec<Duration> = (0..5)
            .map(|_| {
                let t = Instant::now();
                let _ = l.login(user, "not-the-password");
                t.elapsed()
            })
            .collect();
        v.sort();
        v[2]
    };
    let known = time("alice");
    let unknown = time("nobody");
    let long = {
        let t = Instant::now();
        let _ = l.login("alice", &"x".repeat(1 << 20));
        t.elapsed()
    };
    let ratio = unknown.as_secs_f64() / known.as_secs_f64();
    assert!(
        (0.5..2.0).contains(&ratio),
        "known {known:?}, unknown {unknown:?}"
    );
    assert!(
        long < known * 4,
        "a 1 MiB password costs one ordinary PBKDF2, not one per byte: {long:?} vs {known:?}"
    );
}

// ---- sessions ---------------------------------------------------------------

#[test]
fn sessions_end_when_idle_when_old_and_on_logout() {
    let dir = tmp("sessions");
    accounts().save(&dir).unwrap();
    let (l, secs) = logins(&dir);
    let idle = account::SESSION_IDLE.as_secs();
    let s = l.login("alice", ALICE).unwrap();
    assert_eq!(l.session(&s.token).unwrap().role, Role::Engineer);
    secs.fetch_add(idle - 1, Ordering::SeqCst);
    assert!(l.session(&s.token).is_ok(), "used just in time");
    secs.fetch_add(idle - 1, Ordering::SeqCst);
    assert!(l.session(&s.token).is_ok(), "using it restarts the idle time");
    secs.fetch_add(idle, Ordering::SeqCst);
    assert_eq!(l.session(&s.token).unwrap_err(), SessionRefused::Expired);
    assert_eq!(
        l.session(&s.token).unwrap_err(),
        SessionRefused::Unknown,
        "and gone"
    );
    // However busy, a session ends at its TTL.
    let s = l.login("alice", ALICE).unwrap();
    let ttl = account::SESSION_TTL.as_secs();
    let mut used = 0;
    while used + idle / 2 < ttl {
        secs.fetch_add(idle / 2, Ordering::SeqCst);
        used += idle / 2;
        assert!(l.session(&s.token).is_ok());
    }
    secs.fetch_add(ttl - used, Ordering::SeqCst);
    assert_eq!(l.session(&s.token).unwrap_err(), SessionRefused::Expired);
    // Logout.
    let s = l.login("alice", ALICE).unwrap();
    assert_eq!(l.logout(&s.token).unwrap().user, "alice");
    assert_eq!(l.session(&s.token).unwrap_err(), SessionRefused::Unknown);
    assert_eq!(l.logout(&s.token), None);
}

#[test]
fn disabling_an_account_or_changing_it_ends_its_sessions() {
    let dir = tmp("revoke");
    let mut a = accounts();
    a.save(&dir).unwrap();
    let (l, _) = logins(&dir);
    let s = l.login("alice", ALICE).unwrap();
    let other = l.login("bob", BOB).unwrap();
    a.set_disabled("alice", true).unwrap();
    a.save(&dir).unwrap();
    assert_eq!(l.session(&s.token).unwrap_err(), SessionRefused::Revoked);
    assert_eq!(
        l.login("alice", ALICE).unwrap_err(),
        LoginRefused::Wrong,
        "a disabled account looks like a wrong password"
    );
    assert!(l.session(&other.token).is_ok(), "others keep theirs");
    a.set_disabled("alice", false).unwrap();
    a.save(&dir).unwrap();
    let s = l.login("alice", ALICE).unwrap();
    a.passwd("alice", "a-new-password-1", 1, [9; 16]).unwrap();
    a.save(&dir).unwrap();
    assert_eq!(l.session(&s.token).unwrap_err(), SessionRefused::Revoked);
    assert_eq!(l.login("alice", ALICE).unwrap_err(), LoginRefused::Wrong);
    assert!(l.login("alice", "a-new-password-1").is_ok());
    // Removing the file ends every session (the box is back on PINs).
    std::fs::remove_file(dir.join(account::FILE)).unwrap();
    assert_eq!(l.session(&other.token).unwrap_err(), SessionRefused::Revoked);
    assert_eq!(l.login("bob", BOB).unwrap_err(), LoginRefused::NoAccounts);
}

// ---- over the API -----------------------------------------------------------

#[test]
fn pins_work_until_accounts_exist_then_only_sessions_do() {
    let b = Bench::with_accounts(None);
    assert_eq!(get(b.addr, "/api/policy").json()["credentials"], "pins");
    let r = login(b.addr, "alice", ALICE);
    assert_eq!(r.status, 409, "no accounts yet: {r:?}");
    let id = b.param(0.5);
    let r = b.apply_pin(id, "operator", OPERATOR_PIN);
    assert_eq!(r.status, 200, "PINs as before: {r:?}");
    assert_eq!(r.json()["user"], Json::Null);
    assert_eq!(b.approver().as_deref(), Some("operator (PIN)"));

    accounts().save(&b.state).unwrap();
    assert_eq!(get(b.addr, "/api/policy").json()["credentials"], "accounts");
    let id = b.param(0.75);
    for (role, pin) in [("operator", OPERATOR_PIN), ("engineer", ENGINEER_PIN)] {
        let r = b.apply_pin(id, role, pin);
        assert_eq!(r.status, 403, "{r:?}");
        assert!(r.error().contains("role PINs are off"), "{r:?}");
    }
    assert_eq!(b.speed(), json!(0.5), "nothing applied by PIN");
    let bob = login(b.addr, "bob", BOB);
    assert_eq!(bob.status, 200, "{bob:?}");
    assert!(bob.head.contains("Cache-Control: no-store"), "{}", bob.head);
    let j = bob.json();
    assert_eq!(
        (j["user"].as_str(), j["role"].as_str()),
        (Some("bob"), Some("operator"))
    );
    let token = j["token"].as_str().unwrap().to_string();
    let r = b.apply_session(id, &token, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["user"], "bob");
    assert_eq!(b.speed(), json!(0.75));
    assert_eq!(b.approver().as_deref(), Some("bob (operator)"));
    let p = get(b.addr, &format!("/api/proposals/{id}")).json();
    assert_eq!(p["approved_by"], "bob");
    // The undo, by the same person.
    let prev = r.json()["previous"].as_str().unwrap().to_string();
    let r = request(
        b.addr,
        "POST",
        &format!("/api/snapshots/{prev}/rollback"),
        Some(&token),
        "{}",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.speed(), json!(0.5));
    // Every apply and rollback the session made names bob in the audit.
    let log = audit(b.addr);
    let acts: Vec<&Json> = log
        .iter()
        .filter(|e| e["user"] == "bob" && e["path"].as_str().unwrap().contains("/api/"))
        .collect();
    assert!(
        acts.iter()
            .any(|e| e["path"].as_str().unwrap().ends_with("/apply") && e["status"] == 200)
    );
    assert!(
        acts.iter()
            .any(|e| e["path"].as_str().unwrap().ends_with("/rollback") && e["status"] == 200)
    );
    for e in &acts {
        assert_eq!(e["role"], "operator", "{e}");
    }
    assert!(
        log.iter()
            .any(|e| e["path"] == "/api/login" && e["user"] == "bob" && e["status"] == 200)
    );
    // The PIN refusals are audited too, without a person.
    assert!(
        log.iter()
            .any(|e| e["status"] == 403 && e["detail"].as_str().unwrap().contains("role PINs are off"))
    );
    assert_secrets_kept(&b, &[&token]);
}

#[test]
fn a_session_cannot_escalate_its_role() {
    let b = Bench::with_accounts(Some(accounts()));
    let bob = login(b.addr, "bob", BOB).json()["token"]
        .as_str()
        .unwrap()
        .to_string();
    // A logic change needs an engineer: an operator session is refused
    // before anything else, whatever the body claims.
    let (id, _) = b.propose(&json!({ "kind": "module", "manifest": MANIFEST, "source": SOURCE_V2 }));
    let replay = {
        let r = request(
            b.addr,
            "POST",
            &format!("/api/proposals/{id}/replay"),
            Some(&bob),
            "{}",
        );
        assert_eq!(r.status, 201, "a session can replay: {r:?}");
        r.json()["replay"].as_u64().unwrap()
    };
    let r = b.apply_session(id, &bob, Some(replay));
    assert_eq!(r.status, 403, "{r:?}");
    assert!(r.error().contains("engineer role"), "{r:?}");
    for body in [
        json!({ "role": "engineer", "pin": ENGINEER_PIN, "replay": replay }),
        json!({ "role": "integrator", "replay": replay }),
        json!({ "pin": "x", "replay": replay }),
    ] {
        let r = request(
            b.addr,
            "POST",
            &format!("/api/proposals/{id}/apply"),
            Some(&bob),
            &body.to_string(),
        );
        assert_eq!(r.status, 400, "{body}: {r:?}");
        assert!(
            r.error().contains("approves as its account (bob, operator)"),
            "{r:?}"
        );
    }
    // A made-up token, or one a letter off, is no session.
    let mut forged = bob.clone().into_bytes();
    forged[0] = if forged[0] == b'a' { b'b' } else { b'a' };
    for t in [String::from_utf8(forged).unwrap(), "f".repeat(64), String::new()] {
        let r = b.apply_session(id, &t, Some(replay));
        assert_eq!(r.status, 401, "{r:?}");
    }
    assert_eq!(b.source(), SOURCE, "nothing applied");
    // The engineer applies it, as herself.
    let alice = login(b.addr, "alice", ALICE).json()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let r = b.apply_session(id, &alice, Some(replay));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(b.approver().as_deref(), Some("alice (engineer)"));
    let log = audit(b.addr);
    let refused: Vec<&Json> = log
        .iter()
        .filter(|e| {
            e["path"].as_str().unwrap().ends_with("/apply") && e["status"] != 200 && e["status"] != 202
        })
        .collect();
    assert!(
        !refused.iter().all(|e| e["user"] != "bob"),
        "bob's refusals name him"
    );
    for e in refused.iter().filter(|e| e["status"] == 403) {
        assert_eq!(
            (e["user"].as_str(), e["role"].as_str()),
            (Some("bob"), Some("operator")),
            "{e}"
        );
    }
    assert_secrets_kept(&b, &[&bob, &alice]);
}

#[test]
fn api_logins_lock_out_and_logged_out_or_expired_sessions_are_refused() {
    let b = Bench::with_accounts(Some(accounts()));
    for k in 0..FLOOR.lockout.attempts {
        let r = login(b.addr, "alice", &format!("guess-{k}-zzzz"));
        assert_eq!(r.status, 403, "{r:?}");
        assert_eq!(r.error(), "wrong user or password");
    }
    let r = login(b.addr, "alice", ALICE);
    assert_eq!(r.status, 429, "{r:?}");
    assert!(r.head.contains("Retry-After: 30"), "{}", r.head);
    assert!(!r.body.contains("token"), "{r:?}");
    let bob = login(b.addr, "bob", BOB).json()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let id = b.param(0.5);
    // Logout ends it.
    let r = request(b.addr, "POST", "/api/logout", Some(&bob), "");
    assert_eq!(r.status, 200, "{r:?}");
    let r = b.apply_session(id, &bob, None);
    assert_eq!(r.status, 401, "{r:?}");
    let r = request(b.addr, "POST", "/api/logout", Some(&bob), "");
    assert_eq!(r.status, 401, "{r:?}");
    let r = request(b.addr, "POST", "/api/logout", Some(TOKEN), "");
    assert_eq!(r.status, 400, "the box token is no session: {r:?}");
    // Idle past the limit: refused, and told why.
    let bob = login(b.addr, "bob", BOB).json()["token"]
        .as_str()
        .unwrap()
        .to_string();
    b.clock
        .fetch_add(account::SESSION_IDLE.as_secs(), Ordering::SeqCst);
    let r = b.apply_session(id, &bob, None);
    assert_eq!(r.status, 401, "{r:?}");
    assert_eq!(r.error(), "session expired; log in again");
    // Disabled while logged in.
    let bob = login(b.addr, "bob", BOB).json()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut a = accounts();
    a.set_disabled("bob", true).unwrap();
    a.save(&b.state).unwrap();
    let r = b.apply_session(id, &bob, None);
    assert_eq!(r.status, 401, "{r:?}");
    assert!(r.error().contains("disabled or changed"), "{r:?}");
    assert_eq!(login(b.addr, "bob", BOB).status, 403);
    assert_eq!(b.speed(), json!(1.5), "nothing applied");
    // Locked-out refusals are logged, not audited (an unauthenticated
    // flood cannot fill the audit log); wrong passwords are audited.
    let log = audit(b.addr);
    let wrong = log
        .iter()
        .filter(|e| e["path"] == "/api/login" && e["status"] == 403)
        .count();
    assert!(wrong >= usize::try_from(FLOOR.lockout.attempts).unwrap());
    assert!(
        !log.iter()
            .any(|e| e["path"] == "/api/login" && e["status"] == 429)
    );
    assert!(b.logs().iter().any(|l| l.contains("logins locked")));
}

#[test]
fn secrets_never_reach_logs_audit_or_errors() {
    let b = Bench::with_accounts(Some(accounts()));
    let mut answers = Vec::new();
    // A password typed into the user field, a malformed body quoting the
    // password, extra fields, a password in the URL.
    answers.push(login(b.addr, ALICE, ALICE));
    answers.push(request(
        b.addr,
        "POST",
        "/api/login",
        None,
        &format!("{{\"user\": \"alice\", \"password\": \"{ALICE}\""),
    ));
    answers.push(request(
        b.addr,
        "POST",
        "/api/login",
        None,
        &json!({ "user": "alice", "password": ALICE, "role": "integrator" }).to_string(),
    ));
    answers.push(request(
        b.addr,
        "POST",
        &format!("/api/login?password={ALICE}"),
        None,
        "{}",
    ));
    for a in &answers {
        assert!([400, 403].contains(&a.status), "{a:?}");
        assert!(!a.body.contains(ALICE), "{a:?}");
    }
    let ok = login(b.addr, "alice", ALICE);
    let token = ok.json()["token"].as_str().unwrap().to_string();
    let id = b.param(0.5);
    assert_eq!(b.apply_session(id, &token, None).status, 200);
    assert_secrets_kept(&b, &[&token]);
}

#[test]
fn the_box_token_alone_never_approves_once_accounts_exist() {
    let b = Bench::with_accounts(Some(accounts()));
    // The box token alone still drafts; it never approves with accounts.
    let id = b.param(0.5);
    let r = request(
        b.addr,
        "POST",
        &format!("/api/proposals/{id}/apply"),
        Some(TOKEN),
        "{}",
    );
    assert_eq!(r.status, 403, "{r:?}");
    // MCP takes the box token only: a person's session is refused there.
    let alice = login(b.addr, "alice", ALICE).json()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let r = request(
        b.addr,
        "POST",
        "/mcp",
        Some(&alice),
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
    );
    assert_eq!(r.status, 401, "{r:?}");
    let r = request(b.addr, "POST", "/api/login", None, "not json");
    assert_eq!(r.status, 400);
    assert_eq!(r.error(), "expected JSON {\"user\": ..., \"password\": ...}");
}

/// No password, PIN or session token in any audit entry or log line.
fn assert_secrets_kept(b: &Bench, tokens: &[&str]) {
    let all = format!("{:?}{:?}", audit(b.addr), b.logs());
    for s in [ALICE, BOB, CAROL, OPERATOR_PIN, ENGINEER_PIN]
        .into_iter()
        .chain(tokens.iter().copied())
    {
        assert!(!all.contains(s), "{s} leaked: {all}");
    }
}

// ---- the bench ----------------------------------------------------------------

fn defaults() -> Defaults {
    Defaults {
        period: Duration::from_millis(2),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 16,
    }
}

fn running() -> ChangeSet {
    ChangeSet {
        modules: BTreeMap::from([(
            "conveyor".to_string(),
            ModuleFiles {
                manifest: MANIFEST.into(),
                source: SOURCE.into(),
            },
        )]),
        io_map: Some(MAP.into()),
        calendar: None,
    }
}

/// The box as `helmsman-plc --store --state-dir --api` runs it, with role
/// PINs and (if given) an accounts file in its state directory.
struct Bench {
    api: Option<Api>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    store: PathBuf,
    state: PathBuf,
    clock: Arc<AtomicU64>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl Bench {
    #[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
    fn with_accounts(accounts: Option<Accounts>) -> Self {
        let d = defaults();
        let md = ModuleDir::from_text(Path::new("conveyor"), MANIFEST, SOURCE.into(), &d).unwrap();
        let sizes = ImageSizes {
            i: md.spec.i.len,
            q: md.spec.q.len,
            m: md.spec.m,
        };
        let module = md.module(compile(SOURCE, sizes).unwrap()).unwrap();
        let parsed = helmsman_io::text::parse(MAP).unwrap();
        let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
        let mut rt = Runtime::with_signals(32, 32, vec![module], &table).unwrap();
        let start = table.index("start").unwrap();
        let modules = vec![
            ModuleInfo::of(
                &rt,
                "conveyor",
                "conveyor".into(),
                SOURCE.into(),
                Some(md.manifest.clone()),
            )
            .unwrap(),
        ];
        let catalog = Catalog {
            signals: Catalog::signals(&table, Some(&parsed.map), &modules),
            modules,
        };
        let store = tmp("store");
        Store::open(&store)
            .unwrap()
            .commit(
                &running(),
                &Meta {
                    author: "Test".into(),
                    prompt: String::new(),
                    approver: None,
                    time_ms: 1,
                },
            )
            .unwrap();
        let state = tmp("state");
        if let Some(a) = &accounts {
            a.save(&state).unwrap();
        }
        let mut pins = Pins::default();
        pins.set(Role::Operator, OPERATOR_PIN, 1, [1; 16]).unwrap();
        pins.set(Role::Engineer, ENGINEER_PIN, 1, [2; 16]).unwrap();
        let mut recorder = Recorder::new(&table, RecordConfig::default(), None);
        let (handle, swapper) = swap::channel(Duration::from_secs(5));
        let t0 = Instant::now();
        let (c, clock) = clock();
        let approval = Approval {
            policy: FLOOR,
            gate: Gate::new(pins, FLOOR.lockout, Arc::clone(&c)).with_logins(Logins::new(
                state.clone(),
                1,
                FLOOR.lockout,
                c,
            )),
            plant: Plant {
                table: table.clone(),
                i_len: 32,
                q_len: 32,
                defaults: d,
                record: Some(recorder.tap()),
            },
            swap: handle,
        };
        let table = Arc::new(Mutex::new(table));
        let live = Arc::new(Live::default());
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits {
                deadline: Duration::from_secs(5),
                ..Limits::default()
            },
            stream_every: Duration::from_millis(20),
            memory: 16,
            log: Arc::new(move |l: &str| sink.lock().unwrap_or_else(PoisonError::into_inner).push(l.into())),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices: Arc::new(Devices::new(["box".to_string()])),
            audit: Box::new(MemAudit::new(10_000)),
            proposals: Box::new(MemProposals::new(1000)),
            running: Some(running()),
            snapshots: Some(Mutex::new(Store::open(&store).unwrap())),
            approval: Some(approval),
            chat: None,
        };
        let api = Api::start(cfg, shared).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let scan = {
            let (stop, table, ticks) = (Arc::clone(&stop), Arc::clone(&table), Arc::clone(&ticks));
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                    t.set_input(start, Some(Value::Bool(true)));
                    swapper.serve(&mut rt, &mut t);
                    let now = t0.elapsed();
                    recorder.observe(now, &t);
                    rt.tick_with(now, &[], &mut t);
                    publisher.publish(now, &rt, &t);
                    ticks.fetch_add(1, Ordering::Relaxed);
                    drop(t);
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while ticks.load(Ordering::Relaxed) < 50 {
            assert!(Instant::now() < deadline, "the scan never ran");
            thread::sleep(Duration::from_millis(5));
        }
        Self {
            api: Some(api),
            addr,
            stop,
            scan: Some(scan),
            store,
            state,
            clock,
            logs,
        }
    }

    fn logs(&self) -> Vec<String> {
        self.logs.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn propose(&self, body: &Json) -> (u64, Json) {
        let r = request(
            self.addr,
            "POST",
            "/api/proposals",
            Some(TOKEN),
            &body.to_string(),
        );
        assert_eq!(r.status, 201, "{r:?}");
        let j = r.json();
        (j["id"].as_u64().unwrap(), j)
    }

    fn param(&self, value: f64) -> u64 {
        self.propose(&json!({ "kind": "param", "module": "conveyor", "param": "speed", "value": value }))
            .0
    }

    fn apply_pin(&self, id: u64, role: &str, pin: &str) -> Resp {
        request(
            self.addr,
            "POST",
            &format!("/api/proposals/{id}/apply"),
            Some(TOKEN),
            &json!({ "role": role, "pin": pin }).to_string(),
        )
    }

    fn apply_session(&self, id: u64, session: &str, replay: Option<u64>) -> Resp {
        let body = replay.map_or_else(|| json!({}), |n| json!({ "replay": n }));
        request(
            self.addr,
            "POST",
            &format!("/api/proposals/{id}/apply"),
            Some(session),
            &body.to_string(),
        )
    }

    fn speed(&self) -> Json {
        get(self.addr, "/api/modules/conveyor").json()["manifest"]["params"][0]["value"].clone()
    }

    fn source(&self) -> String {
        get(self.addr, "/api/modules/conveyor").json()["source"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Who the store says approved what runs.
    fn approver(&self) -> Option<String> {
        Store::open(&self.store).unwrap().head().unwrap()?.meta.approver
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.scan.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.store);
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

#[derive(Debug)]
struct Resp {
    status: u16,
    head: String,
    body: String,
}

impl Resp {
    fn json(&self) -> Json {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {self:?}"))
    }

    fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or_default().to_string()
    }
}

fn request(addr: SocketAddr, method: &str, path: &str, bearer: Option<&str>, body: &str) -> Resp {
    let auth = bearer.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
    let raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: box\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Resp {
        status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
        head: head.into(),
        body: body.into(),
    }
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    request(addr, "GET", path, None, "")
}

fn login(addr: SocketAddr, user: &str, password: &str) -> Resp {
    request(
        addr,
        "POST",
        "/api/login",
        None,
        &json!({ "user": user, "password": password }).to_string(),
    )
}

fn audit(addr: SocketAddr) -> Vec<Json> {
    get(addr, "/api/audit?limit=1000")
        .json()
        .as_array()
        .cloned()
        .unwrap()
}
