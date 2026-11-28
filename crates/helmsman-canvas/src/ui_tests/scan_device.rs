//! The Devices tab's Scan for Device (Nick, 2026-11-26): under the list it
//! asks the network and lists what answered, by vendor and model where the
//! device library knows it; a click on one asks only its name and adds it
//! (docs/help/editor.md, Devices). The simulation answers with its
//! simulated E1213. Driven the way a visitor does: the pointer and the
//! keyboard.

use eframe::egui::{Event, Key, Modifiers};

use super::{App, Harness};

/// The simulation with an empty project of its own, the Devices tab shown.
fn devices() -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.sim.as_mut().unwrap().proj.create("mine", None);
    h.press("pane devices");
    h.wait(0.3);
    h
}

fn io_map(h: &Harness) -> String {
    h.app
        .sim
        .as_ref()
        .unwrap()
        .proj
        .files
        .get(helmsman_project::IO_MAP)
        .cloned()
        .unwrap_or_default()
}

/// The hit of the one device the scan found.
fn found(h: &Harness) -> String {
    let rows: Vec<String> = h
        .app
        .chrome
        .hits
        .iter()
        .map(|(k, _)| k.clone())
        .filter(|k| k.starts_with("network "))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "one device answered the simulation's scan: {rows:?}"
    );
    rows[0].clone()
}

/// Puts `name` in the name field, replacing what is there.
fn type_name(h: &mut Harness, name: &str) {
    h.press("quick name");
    h.frame(vec![Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]);
    h.frame(vec![Event::Text(name.into())]);
    h.wait(0.2);
}

#[test]
fn scan_for_device_lists_the_simulated_e1213_by_vendor_and_model() {
    let mut h = devices();
    assert!(h.has("devices scan"), "Scan for Device under the list");
    assert!(h.drew("Scan for Device"));
    let scan = h.rect("devices scan");
    let left = h.rect("tabs left").left();
    assert!(scan.left() >= left - 1.0, "inside the pane: {scan:?}");
    assert!(
        !h.app.chrome.hits.iter().any(|(k, _)| k.starts_with("network ")),
        "nothing listed before a scan"
    );
    h.press("devices scan");
    h.wait(2.0);
    let row = found(&h);
    assert!(h.drew("Moxa ioLogik E1213"), "named by vendor and model");
    let r = h.rect(&row);
    assert!(r.top() > scan.top(), "listed under the button: {r:?} vs {scan:?}");
    assert_eq!(io_map(&h), "", "a scan writes nothing");
}

#[test]
fn a_click_on_the_found_device_asks_only_its_name_and_adds_it_by_that_name() {
    let mut h = devices();
    h.press("devices scan");
    h.wait(2.0);
    let row = found(&h);
    h.press(&row);
    h.wait(0.3);
    assert!(h.has("quick form") && h.has("quick name"), "the small form opens");
    // One field: the name. No protocol, address or timing is asked.
    for asked in [
        "device endpoint",
        "device enip-io",
        "device modbus-tcp",
        "device form",
    ] {
        assert!(!h.has(asked), "{asked} is not asked");
    }
    assert_eq!(io_map(&h), "", "nothing written until Add");
    type_name(&mut h, "rio");
    h.press("quick ok");
    h.wait(0.3);
    assert!(!h.has("quick form"), "the form closes");
    let map = io_map(&h);
    let devices: Vec<&str> = map.lines().filter(|l| l.starts_with("device ")).collect();
    assert_eq!(devices.len(), 1, "one device line: {map}");
    assert!(
        devices[0].starts_with("device rio "),
        "added by the name typed: {map}"
    );
    assert!(
        h.has("device rio"),
        "the new device's row: {:?}",
        h.app.chrome.hits
    );
    // A device is added alone: no signal lines until + Add I/O.
    assert!(
        !map.lines().any(|l| l.starts_with("in ") || l.starts_with("out ")),
        "no points added with it: {map}"
    );
}

#[test]
fn cancel_adds_nothing_and_a_name_with_spaces_is_saved_with_underscores() {
    let mut h = devices();
    h.press("devices scan");
    h.wait(2.0);
    let row = found(&h);
    h.press(&row);
    h.wait(0.3);
    h.press("quick cancel");
    h.wait(0.2);
    assert!(!h.has("quick form"));
    assert_eq!(io_map(&h), "", "Cancel writes nothing");
    h.press(&row);
    h.wait(0.3);
    type_name(&mut h, "line 3 rio");
    h.press("quick ok");
    h.wait(0.3);
    let map = io_map(&h);
    let devices: Vec<&str> = map.lines().filter(|l| l.starts_with("device ")).collect();
    assert_eq!(devices.len(), 1, "one device, the Cancel added none: {map}");
    assert!(
        devices[0].starts_with("device line_3_rio "),
        "spaces become underscores: {map}"
    );
}
