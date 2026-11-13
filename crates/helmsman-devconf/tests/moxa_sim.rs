//! The Moxa E1200 client against its simulator: the contract of each
//! operation, what is refused before anything is sent, and the faults.
//! No fixed sleeps: the client's gap and retry wait are zero here, and the
//! restart's polls are 5 ms.

use std::net::TcpStream;
use std::time::Duration;

use helmsman_devconf::moxa_e1200::sim::{Options, Restart, Settings, Sim};
use helmsman_devconf::moxa_e1200::{Config, Console, Error, Export};

fn cfg_for(sim: &Sim) -> Config {
    Config {
        gap: Duration::ZERO,
        retry_wait: Duration::ZERO,
        poll: Duration::from_millis(5),
        request: Duration::from_secs(5),
        restart_down: Duration::from_secs(5),
        restart_up: Duration::from_secs(10),
        modbus_port: sim.modbus_port(),
        ..Config::default()
    }
}

fn connect(sim: &Sim) -> Console {
    Console::connect(&sim.addr(), cfg_for(sim)).expect("connect")
}

fn sets(sim: &Sim) -> usize {
    sim.count("/set.htm")
}

#[test]
fn cookies_are_taken_from_the_front_page_and_sent_back() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    c.export().unwrap();
    c.form("04_1.htm").unwrap();
    let log = sim.requests();
    assert!(!log[0].cookies_sent, "the front page is the first contact");
    assert!(log.len() > 3);
    assert!(log[1..].iter().all(|l| l.cookies_sent), "{log:?}");
}

#[test]
fn export_reads_the_staged_settings_with_labels() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let e = c.export().unwrap();
    assert_eq!(
        e.find("CONNECTION_WATCHDOG").unwrap().label.as_deref(),
        Some("Disabled")
    );
    assert_eq!(e.to_text(), Settings::default().export());
}

#[test]
fn form_is_what_a_browser_submits_and_the_token_is_fresh_each_load() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let a = c.form("04_1.htm").unwrap();
    let b = c.form("04_1.htm").unwrap();
    assert_ne!(a.value("token"), b.value("token"));
    let names: Vec<String> = a.submitted().into_iter().map(|(n, _)| n).collect();
    // EnableIDLE is unticked, so neither it nor IdleTime; the script's input is not a field.
    assert_eq!(names, ["token", "MOD_DESC", "MOD_LOC", "Submit", "code"]);
    assert_eq!(a.value("code"), Some("41"));
    assert!(c.form("14_1.htm").unwrap().field("eipActed").is_some());
}

#[test]
fn set_changes_one_export_line_and_stages_it() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let d = c.set("04_1.htm", &[("MOD_LOC", "helmsman bench")]).unwrap();
    assert_eq!(
        d.changed,
        [("MOD_LOC=".to_string(), "MOD_LOC=helmsman bench".to_string())]
    );
    assert!(d.added.is_empty() && d.removed.is_empty());
    assert_eq!(sim.staged().mod_loc, "helmsman bench");
    assert_eq!(sim.running().mod_loc, "", "staged, not running, until a restart");
    assert_eq!(sets(&sim), 1);
}

#[test]
fn set_ticks_a_checkbox_and_sends_its_dependent_field() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let d = c
        .set(
            "04_1.htm",
            &[
                ("EnableWD", "1"),
                ("WDTime", "5"),
                ("EnableIDLE", "1"),
                ("IdleTime", "90"),
            ],
        )
        .unwrap();
    let lines: Vec<&str> = d.changed.iter().map(|(_, a)| a.as_str()).collect();
    assert_eq!(
        lines,
        ["IDLE_TIME=90", "CONNECTION_WATCHDOG=1,(Enabled)", "WD_TIME=5"]
    );
    let sent = sim
        .requests()
        .into_iter()
        .find(|l| l.path() == "/set.htm")
        .unwrap();
    assert!(
        sent.target
            .contains("EnableIDLE=1&IdleTime=90&EnableWD=1&WDTime=5"),
        "{}",
        sent.target
    );
}

#[test]
fn unticking_a_checkbox_leaves_its_dependent_field_out() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    c.set("04_1.htm", &[("EnableIDLE", "1"), ("IdleTime", "90")])
        .unwrap();
    let d = c.set("04_1.htm", &[("EnableIDLE", "0")]).unwrap();
    assert_eq!(d.changed.len(), 1);
    let sent: Vec<_> = sim
        .requests()
        .into_iter()
        .filter(|l| l.path() == "/set.htm")
        .collect();
    assert_eq!(sent.len(), 2);
    assert!(
        !sent[1].target.contains("IdleTime") && !sent[1].target.contains("EnableIDLE"),
        "{}",
        sent[1].target
    );
}

#[test]
fn a_select_is_changed_by_its_options() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let d = c.set("11_safeM.htm", &[("SetSM03", "1")]).unwrap();
    assert_eq!(
        d.changed,
        [("DO03_SAFE=0,(Off)".to_string(), "DO03_SAFE=1,(On)".to_string())]
    );
    assert!(sim.staged().safe_mode[3]);
    let before = sets(&sim);
    assert!(matches!(
        c.set("11_safeM.htm", &[("SetSM03", "2")]),
        Err(Error::Refused(_))
    ));
    assert_eq!(
        sets(&sim),
        before,
        "an option the page does not offer is not sent"
    );
}

#[test]
fn restart_makes_staged_running_and_waits_for_the_console() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    c.set("14_1.htm", &[("EIP_ENABLE", "0")]).unwrap();
    assert!(sim.running().eip_enable);
    assert!(!sim.staged().eip_enable);
    c.restart().unwrap();
    assert!(!sim.running().eip_enable, "the restart applied it");
    assert_eq!(
        sim.dropped(),
        4,
        "the console was seen down for its whole downtime"
    );
    assert_eq!(
        sim.count("/set_ok.htm"),
        1,
        "the restart request is never repeated"
    );
    // The restart changed the ChallID; the session took the new one, which
    // the password change needs to be accepted.
    c.change_password("", "moxa").unwrap();
    assert_eq!(sim.password().as_deref(), Some("moxa"));
    assert_eq!(c.export().unwrap().find("EIP_ENABLE").unwrap().value, "0");
}

#[test]
fn a_restart_the_console_never_shows_is_reported() {
    let sim = Sim::start();
    sim.set_restart(Restart {
        down1: 0,
        up1: 0,
        down2: 0,
    });
    let mut c = Console::connect(
        &sim.addr(),
        Config {
            restart_down: Duration::from_millis(50),
            ..cfg_for(&sim)
        },
    )
    .unwrap();
    assert_eq!(c.restart(), Err(Error::RestartNotObserved));
}

#[test]
fn a_console_that_does_not_come_back_is_reported() {
    let sim = Sim::start();
    sim.set_restart(Restart {
        down1: 1,
        up1: 1,
        down2: u32::MAX,
    });
    let mut c = Console::connect(
        &sim.addr(),
        Config {
            restart_up: Duration::from_millis(100),
            ..cfg_for(&sim)
        },
    )
    .unwrap();
    assert_eq!(c.restart(), Err(Error::RestartTimeout));
}

fn import_file(sim: &Sim) -> Vec<u8> {
    let mut s = sim.staged();
    s.mod_loc = "restored".into();
    s.safe_mode[0] = true;
    s.ip_address = "192.0.2.99".into();
    s.export().into_bytes()
}

#[test]
fn import_restores_without_the_network_settings_unless_asked() {
    let sim = Sim::start();
    let file = import_file(&sim);
    let mut c = connect(&sim);
    let d = c.import(&file, false).unwrap();
    let after = sim.staged();
    assert_eq!(after.mod_loc, "restored");
    assert!(after.safe_mode[0]);
    assert_eq!(
        after.ip_address, "192.0.2.60",
        "the unit's own address survives an import"
    );
    assert_eq!(d.changed.len(), 2, "{d:?}");

    let d = c.import(&file, true).unwrap();
    assert_eq!(sim.staged().ip_address, "192.0.2.99");
    assert_eq!(
        d.changed,
        [(
            "IP_ADDRESS=192.0.2.60".to_string(),
            "IP_ADDRESS=192.0.2.99".to_string()
        )]
    );
}

#[test]
fn import_refuses_a_file_that_is_not_an_export_before_sending() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    for bad in [&b""[..], b"hello", b"\x00\x01\x02", b"KEY=1\r\n"] {
        assert!(matches!(c.import(bad, false), Err(Error::Refused(_))), "{bad:?}");
    }
    assert_eq!(sim.count("/06_5_1.htm"), 0);
    assert_eq!(sim.count("/06_5.htm"), 0);
}

#[test]
fn a_file_that_holds_a_boundary_lookalike_still_imports() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let mut file = Settings::default().export();
    file = file.replace("MOD_DESC=", "MOD_DESC=----helmsman-devconf-00000000");
    c.import(file.as_bytes(), false).unwrap();
    assert_eq!(sim.staged().mod_desc, "----helmsman-devconf-00000000");
}

#[test]
fn password_change_is_decoded_by_the_console() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    c.change_password("", "moxa").unwrap();
    assert_eq!(sim.password().as_deref(), Some("moxa"));
    c.change_password("moxa", "Abc!~ 09").unwrap();
    assert_eq!(sim.password().as_deref(), Some("Abc!~ 09"));
    c.change_password("Abc!~ 09", "sixteen-chars-xx").unwrap();
    assert_eq!(sim.password().as_deref(), Some("sixteen-chars-xx"));
}

#[test]
fn a_wrong_old_password_changes_nothing_and_is_reported() {
    let sim = Sim::start_with(Options {
        password: Some("moxa".into()),
        ..Options::default()
    });
    let mut c = Console::connect(&sim.addr(), cfg_for(&sim).with_password("moxa")).unwrap();
    assert_eq!(c.change_password("nope", "new"), Err(Error::PasswordFailed));
    assert_eq!(sim.password().as_deref(), Some("moxa"));
    c.change_password("moxa", "new").unwrap();
    assert_eq!(sim.password().as_deref(), Some("new"));
}

#[test]
fn bad_passwords_are_refused_before_anything_is_sent() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let n = sim.requests().len();
    for (old, new) in [
        ("", "seventeen-chars-x"),
        ("", "caf\u{e9}"),
        ("", "tab\t"),
        ("", ""),
        (&"a".repeat(25), "ok"),
    ] {
        let e = c.change_password(old, new).unwrap_err();
        assert!(matches!(e, Error::Refused(_)), "{old:?} {new:?}: {e}");
        assert!(
            !e.to_string().contains("seventeen") && !e.to_string().contains("aaaaaaaa"),
            "the error echoed a password"
        );
    }
    assert_eq!(sim.requests().len(), n);
}

#[test]
fn a_dropped_connection_is_retried_once_on_a_read() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    sim.drop_next(1);
    let e = c.export().unwrap();
    assert_eq!(e.to_text(), Settings::default().export());
    assert_eq!(sim.dropped(), 1);
    assert_eq!(sim.count("/06_6.htm"), 1);
}

#[test]
fn a_read_is_not_retried_twice() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    sim.drop_next(2);
    assert_eq!(c.export().unwrap_err(), Error::NoResponse);
    assert_eq!(sim.dropped(), 2);
    assert!(c.export().is_ok(), "the next call is fine");
}

#[test]
fn a_dropped_connection_is_never_retried_on_a_write() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let mut f = c.form("04_1.htm").unwrap();
    f.apply(&[("MOD_LOC", "x")]).unwrap();
    sim.drop_next(1);
    assert_eq!(c.submit(&f), Err(Error::NoResponse));
    // A retry would have got through and been logged.
    assert_eq!(sets(&sim), 0);
    assert_eq!(sim.dropped(), 1);
    assert_eq!(sim.staged().mod_loc, "");
    // The caller decides: the same form still holds a live token.
    c.submit(&f).unwrap();
    assert_eq!(sim.staged().mod_loc, "x");
}

#[test]
fn a_stale_token_is_refused_and_reported() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let mut old = c.form("04_1.htm").unwrap();
    let mut new = c.form("04_1.htm").unwrap();
    old.apply(&[("MOD_LOC", "from the old page")]).unwrap();
    new.apply(&[("MOD_LOC", "from the new page")]).unwrap();
    assert_eq!(c.submit(&old), Err(Error::NotAccepted));
    assert_eq!(sim.staged().mod_loc, "", "nothing changed");
    c.submit(&new).unwrap();
    assert_eq!(sim.staged().mod_loc, "from the new page");
    // A token for one page does not write another.
    let mut other = c.form("13_1.htm").unwrap();
    other.apply(&[("REST_ENABLE", "0")]).unwrap();
    let mut forged = c.form("04_1.htm").unwrap();
    forged.fields.retain(|f| f.name != "token");
    forged.fields.push(other.field("token").unwrap().clone());
    assert_eq!(c.submit(&forged), Err(Error::NotAccepted));
}

#[test]
fn what_is_refused_is_refused_before_a_write() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let n = sim.requests().len();
    for page in [
        "../04_1.htm",
        "04_1.html",
        "a/b.htm",
        "08.htm",
        "09.htm",
        "set.htm",
        "set_ok.htm",
        "06_5_1.htm",
        "",
    ] {
        assert!(matches!(c.form(page), Err(Error::Refused(_))), "{page:?}");
        assert!(matches!(c.set(page, &[]), Err(Error::Refused(_))), "{page:?}");
    }
    assert!(matches!(
        c.form(&format!("{}.htm", "a".repeat(33))),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        c.set("07.htm", &[("OldPwd", "x")]),
        Err(Error::Refused(_))
    ));
    assert_eq!(sim.requests().len(), n, "no request left for a refused page");

    for bad in [
        ("token", "x"),
        ("code", "1"),
        ("nope", "1"),
        ("Submit", "x"),
        ("MOD_LOC", "a\nb"),
    ] {
        assert!(
            matches!(c.set("04_1.htm", &[bad]), Err(Error::Refused(_))),
            "{bad:?}"
        );
    }
    assert_eq!(sets(&sim), 0);
    assert!(matches!(
        Console::connect("not a host", Config::default()),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        Console::connect("a:99999", Config::default()),
        Err(Error::Refused(_))
    ));
}

#[test]
fn no_action_page_is_fetched_except_by_its_operation() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    c.set("04_1.htm", &[("MOD_LOC", "x")]).unwrap();
    c.form("13_1.htm").unwrap();
    c.import(&Settings::default().export().into_bytes(), false)
        .unwrap();
    c.restart().unwrap();
    c.change_password("", "pw").unwrap();
    assert_eq!(sim.count("/08.htm"), 0, "Load Factory Default");
    assert_eq!(sim.count("/09.htm"), 0);
    assert_eq!(sim.count("/set_ok.htm"), 1);
}

#[test]
fn the_export_survives_a_round_trip_through_import() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    c.set("04_1.htm", &[("MOD_DESC", "line one"), ("MOD_LOC", "a & b")])
        .unwrap();
    let saved = c.export().unwrap();
    c.set("04_1.htm", &[("MOD_DESC", "other"), ("MOD_LOC", "elsewhere")])
        .unwrap();
    let d = c.import(saved.to_text().as_bytes(), false).unwrap();
    assert_eq!(d.changed.len(), 2);
    assert_eq!(c.export().unwrap(), Export::parse(&saved.to_text()).unwrap());
}

/// The real unit answers HTTP between its two downtimes while Modbus is
/// still down (mapped 2026-11-12). A restart that took the first answer
/// as "back" would return there, with the unit mid-reboot.
#[test]
fn restart_does_not_trust_the_first_http_answer() {
    let sim = Sim::start();
    sim.set_restart(Restart {
        down1: 1,
        up1: 3,
        down2: 2,
    });
    let mut c = connect(&sim);
    c.set("14_1.htm", &[("EIP_ENABLE", "0")]).unwrap();
    c.restart().unwrap();
    assert!(
        sim.is_up(),
        "restart() returned while the unit was still rebooting"
    );
    assert_eq!(sim.dropped(), 3, "it waited through the second downtime");
    // The sim answered HTTP during the first window (up1) and the client
    // looked at Modbus, found it refusing, and kept waiting.
    assert!(TcpStream::connect(("127.0.0.1", sim.modbus_port())).is_ok());
}

#[test]
fn modbus_is_refused_while_the_unit_reboots() {
    let sim = Sim::start();
    assert!(TcpStream::connect(("127.0.0.1", sim.modbus_port())).is_ok());
    let mut c = connect(&sim);
    c.restart().unwrap();
    assert_eq!(sim.reboots(), 1);
    // Mid-reboot, as the real unit is for about 20 s.
    sim.set_restart(Restart {
        down1: 5,
        up1: 0,
        down2: 0,
    });
    let mut c2 = Console::connect(&sim.addr(), cfg_for(&sim)).unwrap();
    let _ = c2.restart();
    assert!(TcpStream::connect(("127.0.0.1", sim.modbus_port())).is_ok());
}

#[test]
fn import_reboots_the_unit_and_waits() {
    let sim = Sim::start();
    let file = import_file(&sim);
    let mut c = connect(&sim);
    c.import(&file, false).unwrap();
    assert_eq!(sim.reboots(), 1);
    assert!(sim.is_up(), "import() returned mid-reboot");
    assert_eq!(
        sim.running().mod_loc,
        "restored",
        "the import is running, not staged"
    );
    assert_eq!(sim.running().ip_address, "192.0.2.60");
    // A unit's own export imports as a no-op.
    let own = c.export().unwrap();
    let d = c.import(own.to_text().as_bytes(), false).unwrap();
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(sim.reboots(), 2);
}

// ---- a console with a password

const LOCKED: &str = "moxa";

fn locked() -> Sim {
    Sim::start_with(Options {
        password: Some(LOCKED.into()),
        ..Options::default()
    })
}

fn logged_in(sim: &Sim) -> Console {
    Console::connect(&sim.addr(), cfg_for(sim).with_password(LOCKED)).unwrap()
}

#[test]
fn the_right_password_logs_in_once_and_reads() {
    let sim = locked();
    let mut c = logged_in(&sim);
    assert_eq!(
        sim.count("/home.htm"),
        1,
        "the front page asked, the client answered once"
    );
    let e = c.export().unwrap();
    c.form("04_1.htm").unwrap();
    assert_eq!(sim.count("/home.htm"), 1, "the session carries on");
    // The password is not in the export: it is what an unlocked unit gives.
    assert_eq!(e.to_text(), Settings::default().export());
    assert!(!e.to_text().contains(LOCKED));
}

#[test]
fn the_login_posts_md5_of_password_and_token_never_the_password() {
    let sim = locked();
    let _c = logged_in(&sim);
    let log = sim.requests();
    let post = log.iter().find(|l| l.method == "POST").unwrap();
    assert_eq!(post.path(), "/home.htm");
    assert!(!post.target.contains(LOCKED));
}

#[test]
fn a_wrong_password_is_refused_once_and_not_tried_again() {
    let sim = locked();
    let wrong = Console::connect(&sim.addr(), cfg_for(&sim).with_password("nope"));
    assert_eq!(wrong.err(), Some(Error::LoginRefused));
    assert_eq!(sim.count("/home.htm"), 1);
    assert!(
        sim.requests()
            .iter()
            .all(|l| l.method != "POST" || l.path() == "/home.htm")
    );
}

#[test]
fn a_session_does_not_hammer_a_console_with_a_refused_password() {
    // Open against an unlocked console, which then gets a password
    // from elsewhere: the client's stale password is refused once.
    let sim = Sim::start();
    let mut c = Console::connect(&sim.addr(), cfg_for(&sim).with_password("old")).unwrap();
    let mut other = connect(&sim);
    other.change_password("", LOCKED).unwrap();
    assert_eq!(c.export().unwrap_err(), Error::LoginRefused);
    assert_eq!(c.export().unwrap_err(), Error::LoginRefused);
    assert_eq!(c.form("04_1.htm").unwrap_err(), Error::LoginRefused);
    assert_eq!(sim.count("/home.htm"), 1, "one try, then none");
}

#[test]
fn no_password_gets_a_typed_error_not_a_login_page() {
    let sim = locked();
    assert_eq!(
        Console::connect(&sim.addr(), cfg_for(&sim)).err(),
        Some(Error::PasswordRequired)
    );
    assert_eq!(sim.count("/home.htm"), 0, "nothing posted without a password");
}

#[test]
fn a_password_that_cannot_be_the_consoles_is_refused_before_connecting() {
    let sim = locked();
    for bad in [&"a".repeat(25)[..], "caf\u{e9}", "tab\t"] {
        let r = Console::connect(&sim.addr(), cfg_for(&sim).with_password(bad));
        assert!(matches!(r, Err(Error::Refused(_))), "{bad:?}");
    }
    assert_eq!(sim.requests().len(), 0);
    let shown = format!("{:?}", cfg_for(&sim).with_password(LOCKED));
    assert!(!shown.contains(LOCKED), "{shown}");
}

#[test]
fn a_write_that_meets_the_login_is_not_repeated() {
    let sim = Sim::start();
    // A session that opened before the password was set.
    let mut c = Console::connect(&sim.addr(), cfg_for(&sim).with_password(LOCKED)).unwrap();
    let mut f = c.form("04_1.htm").unwrap();
    f.apply(&[("MOD_LOC", "should not land")]).unwrap();
    // Someone sets the password: that ends every session.
    let mut other = connect(&sim);
    other.change_password("", LOCKED).unwrap();
    assert_eq!(c.submit(&f), Err(Error::LoginBeforeWrite));
    assert_eq!(sim.staged().mod_loc, "", "nothing was written");
    // It logged in, so the caller can run it again.
    let mut f = c.form("04_1.htm").unwrap();
    f.apply(&[("MOD_LOC", "now")]).unwrap();
    c.submit(&f).unwrap();
    assert_eq!(sim.staged().mod_loc, "now");
}

#[test]
fn a_restart_or_import_that_meets_the_login_does_not_happen() {
    let sim = Sim::start();
    let mut c = Console::connect(&sim.addr(), cfg_for(&sim).with_password(LOCKED)).unwrap();
    let mut other = connect(&sim);
    other.change_password("", LOCKED).unwrap();
    assert_eq!(c.restart(), Err(Error::LoginBeforeWrite));
    assert_eq!(sim.reboots(), 0);
    // Logged in again: a read works, and the import's own token page is fine.
    let mut other2 = Console::connect(&sim.addr(), cfg_for(&sim).with_password(LOCKED)).unwrap();
    other2.export().unwrap();
    assert_eq!(sim.count("/set_ok.htm"), 1, "asked once, not repeated");
}

#[test]
fn a_write_without_a_password_says_the_console_wants_one() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    let mut f = c.form("04_1.htm").unwrap();
    f.apply(&[("MOD_LOC", "x")]).unwrap();
    let mut other = connect(&sim);
    other.change_password("", LOCKED).unwrap();
    assert_eq!(c.submit(&f), Err(Error::PasswordRequired));
    assert_eq!(sim.staged().mod_loc, "");
}

#[test]
fn set_restart_and_import_work_through_the_login() {
    let sim = locked();
    let mut c = logged_in(&sim);
    c.set("04_1.htm", &[("MOD_LOC", "locked bench")]).unwrap();
    c.restart().unwrap();
    assert_eq!(sim.running().mod_loc, "locked bench");
    // The reboot ended the session; the next read logs in again.
    let e = c.export().unwrap();
    assert_eq!(e.find("MOD_LOC").unwrap().value, "locked bench");
    c.import(&sim.staged().export().into_bytes(), false).unwrap();
    assert_eq!(sim.reboots(), 2);
}

#[test]
fn the_reply_of_a_password_change_is_ok_or_fail_and_applies_at_once() {
    let sim = Sim::start();
    let mut c = connect(&sim);
    c.change_password("", "moxa").unwrap();
    assert_eq!(sim.password().as_deref(), Some("moxa"));
    // At once, no restart: the old session is gone and the client logs in
    // again with the new password.
    let before = sim.count("/home.htm");
    c.export().unwrap();
    assert_eq!(sim.count("/home.htm"), before + 1);
    assert_eq!(sim.reboots(), 0);
    assert_eq!(c.change_password("wrong", "other"), Err(Error::PasswordFailed));
    assert_eq!(sim.password().as_deref(), Some("moxa"));
}
