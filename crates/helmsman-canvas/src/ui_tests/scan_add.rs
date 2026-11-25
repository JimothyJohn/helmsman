//! Scan and add (Nick, 2026-11-24: "Add an option to Scan and add all
//! devices vs just scanning what's on a network"): one press scans and
//! adds every found device the library knows, each named after its
//! model, with no form to fill; what it could not add is said.

use super::{App, Harness};
use crate::devices::{AddAll, DeviceAct, DevicesUi, Found, LibraryHit, Scan};

#[test]
fn scan_and_add_puts_every_known_device_in_the_project_without_a_form() {
    let mut h = Harness::new(App::sim());
    h.app.sim.as_mut().unwrap().proj.create("mine", None);
    h.press("pane devices");
    h.wait(0.3);
    assert!(h.has("devices scan add"), "beside Scan");
    h.press("devices scan add");
    for _ in 0..10 {
        h.wait(0.1);
        assert!(!h.has("quick form"), "no form while it adds");
    }
    let sim = h.app.sim.as_ref().unwrap();
    assert_eq!(sim.devices_ui.scan, Scan::Done);
    assert!(sim.devices_ui.add_all.is_none(), "finished");
    let map = sim.proj.files["io.map"].clone();
    assert!(
        map.contains("device iologik_e1213 device:moxa-iologik-e1213 cip=991:515:0x00e12130\n"),
        "{map}"
    );
    assert!(sim.proj.files.contains_key("moxa-iologik-e1213.device"));
    let note = sim.devices_ui.add_note.clone().unwrap();
    assert!(note.starts_with("Scan and add: added iologik_e1213."), "{note}");
    assert!(note.contains("+ Add I/O") && note.contains("Agent"), "{note}");
    assert!(h.has("devices add note"));

    // Again: it is in the map already, so nothing more.
    h.press("devices scan add");
    for _ in 0..10 {
        h.wait(0.1);
    }
    let sim = h.app.sim.as_ref().unwrap();
    assert_eq!(
        sim.proj.files["io.map"].matches("device iologik_e1213").count(),
        1
    );
    assert_eq!(
        sim.devices_ui.add_note.as_deref(),
        Some("Scan and add: nothing was added.")
    );
}

fn found(address: &str, product: u16, library: Option<&str>) -> Found {
    Found {
        address: address.into(),
        mac: None,
        vendor_id: 991,
        product_code: product,
        revision: "3.1".into(),
        serial: "1".into(),
        product_name: "E1213".into(),
        cip: format!("cip=991:{product}:1"),
        identity: format!("cip=991:{product}:{address}"),
        mapped: None,
        library: library.map(|f| LibraryHit {
            folder: "moxa/e1213".into(),
            vendor: "Moxa".into(),
            model: "ioLogik E1213".into(),
            driver: None,
            profile: None,
            profile_file: Some(f.into()),
        }),
        simulated: false,
        reported: None,
    }
}

#[test]
fn an_unknown_device_is_said_and_a_taken_name_gets_a_number() {
    let mut d = DevicesUi::default();
    d.map = helmsman_project::iomap::skim("device iologik_e1213 device:moxa-iologik-e1213 cip=991:515:9\n");
    d.add_all = Some(AddAll::default());
    d.found = vec![
        found("192.168.3.61", 515, Some("moxa-iologik-e1213.device")),
        found("192.168.3.62", 999, None),
    ];
    d.found_to_add_all();
    let Some(DeviceAct::Library {
        vendor: 991,
        product: 515,
    }) = d.add_all_step()
    else {
        panic!("the known one's profile asked for")
    };
    assert_eq!(
        d.quick.as_ref().map(|q| q.name.as_str()),
        Some("iologik_e1213_2"),
        "the map has iologik_e1213"
    );
    // The library could not give it: passed over, said, and done.
    d.quick.as_mut().unwrap().profile = Some(Err("the library has no profile for it".into()));
    assert_eq!(d.add_next, None);
    assert_eq!(d.add_all_step(), None);
    assert_eq!(d.add_all_step(), None);
    assert!(d.add_all.is_none());
    let note = d.add_note.unwrap();
    assert!(
        note.starts_with("Scan and add: nothing was added. Not added: "),
        "{note}"
    );
    assert!(
        note.contains("E1213 at 192.168.3.62, not in the device library yet"),
        "{note}"
    );
    assert!(
        note.contains("at 192.168.3.61: the library has no profile for it"),
        "{note}"
    );
}

/// On a Skipper's page a device the Skipper already runs is passed over
/// (Nick's drive was added a second time beside the Skipper's `drive`).
#[test]
fn a_device_the_skipper_runs_already_is_passed_over() {
    let mut d = DevicesUi::default();
    d.add_all = Some(AddAll::default());
    d.skipper = vec![("drive".into(), "192.168.33.1".into())];
    d.found = vec![found("192.168.33.1", 515, Some("moxa-iologik-e1213.device"))];
    d.found_to_add_all();
    assert_eq!(d.add_all_step(), None);
    assert!(d.add_all.is_none(), "nothing to add");
    let note = d.add_note.unwrap();
    assert!(
        note.contains("ioLogik E1213 at 192.168.33.1, the Skipper's own drive"),
        "{note}"
    );
}

/// On a Skipper's page Add names the device by the address it answered
/// at: a Skipper does not look a device up by its identity.
#[test]
fn on_a_skippers_page_a_device_is_added_at_its_address() {
    let mut d = DevicesUi::default();
    d.add_all = Some(AddAll::default());
    d.skipper = vec![("drive".into(), "192.168.33.1".into())];
    d.found = vec![found("192.168.33.60", 515, Some("moxa-iologik-e1213.device"))];
    d.found_to_add_all();
    assert!(matches!(d.add_all_step(), Some(DeviceAct::Library { .. })));
    let text = include_str!("../../../../docs/devices/moxa-iologik-e1213.device").to_string();
    let skim = helmsman_project::profile_skim::skim(&text).unwrap();
    d.quick.as_mut().unwrap().profile = Some(Ok((text, skim)));
    assert_eq!(d.add_all_step(), None);
    let add = d.add_next.take().expect("handed on as Add");
    assert_eq!(
        add.wiring.device_line,
        "device iologik_e1213 device:moxa-iologik-e1213 192.168.33.60"
    );
}
