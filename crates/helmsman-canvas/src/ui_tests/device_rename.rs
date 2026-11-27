//! Renaming a device in the Devices tab's Edit form (Nick, 2026-11-26:
//! renaming the Skipper's `drive` to `motor` was refused with
//! "EtherNet/IP needs a profile or the device's EDS file."). A device in
//! the map is renamed whatever it was set up with; its signals follow it
//! in place, and nothing else in the map moves. A new EtherNet/IP device
//! still needs a profile or an EDS.

use eframe::egui::{self, Event};

use super::{App, Harness};
use crate::device_form::check_settings_for;

/// The bench's io.map: the Skipper's drive (no profile, no EDS) and an
/// E1213 beside it.
const BENCH: &str = "# placeholders\n\
device drive enip-io 192.168.33.1 config=1 o2t=112:36 t2o=100:16 rpi_ms=20\n\
in enabled bool drive in:0.0\n\
in moving bool drive in:0.1  # status word, bit 1\n\
out vmax int drive out:8:u32\n\
device iologik_e1213 enip-io 192.168.33.60 profile=moxa-e1213 rpi_ms=100\n\
in iologik_e1213_di_00 bool iologik_e1213 di:0\n\
out lamp bool sim\n";

const REFUSED: &str = "EtherNet/IP needs a profile or the device's EDS file.";

/// The bench map in a project of the visitor's own, the Devices tab open;
/// `skipper` makes it a Skipper's page.
fn bench(skipper: bool) -> Harness {
    let mut h = Harness::new(App::sim());
    {
        let sim = h.app.sim.as_mut().unwrap();
        sim.proj.create("mine", None);
        sim.proj.add_file(helmsman_project::IO_MAP, BENCH.into());
        if skipper {
            sim.devices_ui.skipper = vec![
                ("drive".into(), "192.168.33.1".into()),
                ("iologik_e1213".into(), "192.168.33.60".into()),
            ];
        }
    }
    h.press("pane devices");
    h.wait(0.3);
    h
}

/// A line's words: the column padding is not what a rename is about.
fn words(l: &str) -> String {
    l.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn map(h: &Harness) -> String {
    h.app.sim.as_ref().unwrap().proj.files[helmsman_project::IO_MAP].clone()
}

/// The form's error, while it is open.
fn error(h: &Harness) -> Option<String> {
    h.app
        .sim
        .as_ref()
        .unwrap()
        .devices_ui
        .adding
        .as_ref()
        .and_then(|d| d.error.clone())
}

/// Edit `from`, type `to` over its name, Save. True when the form closed.
fn rename(h: &mut Harness, from: &str, to: &str) -> bool {
    h.press(&format!("edit device {from}"));
    h.wait(0.3);
    assert!(h.has("device form"), "Edit opens the form for {from}");
    h.press("device name");
    h.frame(vec![Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    }]);
    h.frame(vec![Event::Text(to.into())]);
    h.wait(0.2);
    h.press("device ok");
    h.wait(0.3);
    !h.has("device form")
}

/// The regression: drive → motor on the bench map. The device line and
/// its three signals name `motor` in place, the comment kept; the E1213,
/// its point, the header comment and `lamp` are as they were.
#[test]
fn a_skippers_drive_is_renamed_and_its_signals_follow_in_place() {
    for skipper in [false, true] {
        let mut h = bench(skipper);
        assert!(h.has("edit device drive"), "skipper={skipper}");
        let closed = rename(&mut h, "drive", "motor");
        assert!(closed, "skipper={skipper}: refused with {:?}", error(&h));
        let got = map(&h);
        let lines: Vec<&str> = got.lines().collect();
        let want: Vec<&str> = BENCH.lines().collect();
        assert_eq!(lines.len(), want.len(), "skipper={skipper}: {got}");
        // The device line, where it was, still reached and set up as it was.
        assert!(
            lines[1].starts_with("device motor enip-io 192.168.33.1 "),
            "skipper={skipper}: {got}"
        );
        for p in ["config=1", "o2t=112:36", "t2o=100:16", "rpi_ms=20"] {
            assert!(
                lines[1].split_whitespace().any(|t| t == p),
                "skipper={skipper}: {p} kept: {got}"
            );
        }
        assert!(
            !lines[1].contains("profile=") && !lines[1].contains("eds="),
            "skipper={skipper}: a rename gives it no profile: {got}"
        );
        assert_eq!(
            lines[2..].iter().map(|l| words(l)).collect::<Vec<_>>(),
            [
                "in enabled bool motor in:0.0",
                "in moving bool motor in:0.1 # status word, bit 1",
                "out vmax int motor out:8:u32",
                "device iologik_e1213 enip-io 192.168.33.60 profile=moxa-e1213 rpi_ms=100",
                "in iologik_e1213_di_00 bool iologik_e1213 di:0",
                "out lamp bool sim",
            ],
            "skipper={skipper}"
        );
        assert_eq!(lines[0], "# placeholders");
        assert!(got.ends_with('\n'));
        assert!(!got.contains(" drive"), "no line names drive: {got}");
        assert_eq!(helmsman_project::iomap::skim(&got).odd, 0, "{got}");
        assert!(h.has("device motor") && !h.has("device drive"));
    }
}

/// Save stores spaces as underscores, around the name and inside it.
#[test]
fn a_name_with_spaces_is_saved_with_underscores() {
    let mut h = bench(false);
    assert!(rename(&mut h, "drive", "my drive"), "{:?}", error(&h));
    let got = map(&h);
    assert!(got.contains("\ndevice my_drive enip-io 192.168.33.1 "), "{got}");
    assert!(
        got.lines().any(|l| words(l) == "in enabled bool my_drive in:0.0"),
        "{got}"
    );
    assert!(!got.contains("my drive"), "{got}");
}

/// A name another device has is refused, and the map is left as it was;
/// a name of 32 characters passes on a Skipper's page and 33 is refused.
#[test]
fn a_taken_name_or_one_too_long_for_a_skipper_is_refused() {
    let mut h = bench(true);
    assert!(!rename(&mut h, "drive", "iologik_e1213"), "the form stays");
    let e = error(&h).expect("said why");
    assert!(e.contains("iologik_e1213"), "{e}");
    assert!(!e.contains(REFUSED), "{e}");
    h.press("device cancel");
    h.wait(0.2);
    assert_eq!(map(&h), BENCH, "nothing written");

    let long = "m".repeat(crate::skipper_map::NAME_MAX + 1);
    assert!(!rename(&mut h, "drive", &long), "33 characters on a Skipper");
    assert_eq!(
        error(&h).as_deref(),
        Some("A Skipper names a device in at most 32 characters.")
    );
    h.press("device cancel");
    h.wait(0.2);
    assert_eq!(map(&h), BENCH, "nothing written");

    let fits = "m".repeat(crate::skipper_map::NAME_MAX);
    assert!(rename(&mut h, "drive", &fits), "{:?}", error(&h));
    let got = map(&h);
    assert!(got.contains(&format!("\ndevice {fits} enip-io ")), "{got}");
    assert!(
        got.contains(&format!("\nout vmax int {fits} out:8:u32\n")),
        "{got}"
    );
}

/// Only a Skipper limits a device name to 32 characters.
#[test]
fn a_long_name_is_a_skippers_limit_only() {
    let mut h = bench(false);
    let long = "m".repeat(crate::skipper_map::NAME_MAX + 1);
    assert!(rename(&mut h, "drive", &long), "{:?}", error(&h));
    assert!(
        map(&h).contains(&format!("\ndevice {long} enip-io ")),
        "{}",
        map(&h)
    );
}

/// Saving a device's form with its own name unchanged is not "taken".
#[test]
fn saving_a_device_under_its_own_name_changes_nothing_it_names() {
    let mut h = bench(true);
    assert!(rename(&mut h, "drive", "drive"), "{:?}", error(&h));
    let got = map(&h);
    let lines: Vec<&str> = got.lines().collect();
    assert!(
        lines[1].starts_with("device drive enip-io 192.168.33.1 "),
        "{got}"
    );
    assert_eq!(
        got.lines().skip(2).map(words).collect::<Vec<_>>(),
        BENCH.lines().skip(2).map(words).collect::<Vec<_>>()
    );
}

/// A name that is not an identifier never reaches the map as one: the
/// form refuses it, or what it writes still reads as one device line and
/// its signals.
#[test]
fn a_hostile_name_does_not_break_the_map() {
    for bad in [
        "motor\nin evil bool box di:1",
        "motor # x",
        "../motor",
        "1motor",
        "mo;tor",
    ] {
        let mut h = bench(true);
        let closed = rename(&mut h, "drive", bad);
        let got = map(&h);
        if closed {
            let m = helmsman_project::iomap::skim(&got);
            assert_eq!(m.odd, 0, "{bad:?}: {got}");
            assert_eq!(m.devices.len(), 2, "{bad:?}: {got}");
            assert!(!got.contains("evil"), "{bad:?}: {got}");
            assert_eq!(got.lines().count(), BENCH.lines().count(), "{bad:?}: {got}");
        } else {
            assert_eq!(got, BENCH, "{bad:?}: refused, nothing written");
        }
    }
}

/// What a device's settings must hold, new or edited (the check the form
/// runs on Save).
#[test]
fn a_new_ethernet_ip_device_still_needs_a_profile_or_an_eds() {
    let skipper = "config=1 o2t=112:36 t2o=100:16 rpi_ms=20";
    // New: neither is refused; edited, the line it was read from stands.
    assert_eq!(check_settings_for("enip-io", skipper, false), Err(REFUSED.into()));
    assert_eq!(check_settings_for("enip-io", "", false), Err(REFUSED.into()));
    assert_eq!(check_settings_for("enip-io", skipper, true), Ok(()));
    // Either one alone passes, new or edited.
    for editing in [false, true] {
        assert_eq!(
            check_settings_for("enip-io", "profile=moxa-e1213 rpi_ms=100", editing),
            Ok(())
        );
        assert_eq!(check_settings_for("enip-io", "eds=moxa.eds", editing), Ok(()));
        // Both together, an unknown profile, a value out of range: refused
        // whether new or edited.
        assert!(
            check_settings_for("enip-io", "profile=moxa-e1213 eds=a.eds", editing).is_err(),
            "both, editing={editing}"
        );
        assert!(
            check_settings_for("enip-io", "profile=other", editing).is_err(),
            "unknown profile, editing={editing}"
        );
        for bad in ["rpi_ms=abc", "rpi_ms=-1", "rpi_ms=99999999999"] {
            let e = check_settings_for(
                "enip-io",
                &format!("config=1 o2t=112:36 t2o=100:16 {bad}"),
                editing,
            );
            assert!(e.is_err(), "{bad}, editing={editing}");
            assert_ne!(e, Err(REFUSED.into()), "{bad}: refused for the value itself");
        }
        assert!(check_settings_for("modbus-tcp", "unit=0", editing).is_err());
        assert!(check_settings_for("modbus-tcp", "timeout_ms=60001", editing).is_err());
    }
}
