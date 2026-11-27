//! The per-device wiring walkthrough, "Wire it", on the Devices tab: the
//! inputs the program uses in channel order, what the library says about
//! the device and the sensor on each, and a checklist ticked by hand
//! (simulation) or by watching the box's values (a box page). It never
//! writes, forces or sends anything to the box.

use helmsman_iec::Value;

use super::{App, Harness};
use crate::box_projects::{Buf, OpenProject, PCall};
use crate::box_tests::{Bench, watch};
use crate::remote::{DeviceRow, SignalRow};

const E1213: &str = include_str!("../../../../docs/devices/moxa-iologik-e1213.device");

/// `stop` (DI-01) is written after `start` (DI-05) so that line order
/// and channel order differ; `spare` has no variable in the latch;
/// `rio_do_00` is an output; `other` sits on the second device.
const MAP: &str = "device rio device:moxa-iologik-e1213 cip=991:515:0x00e12130\n\
in  start bool rio ch:DI-05\n\
in  stop  bool rio ch:DI-01\n\
in  spare bool rio ch:DI-03\n\
out motor bool rio ch:DO-00\n\
device rio2 device:moxa-iologik-e1213 cip=991:515:0x00e12131\n\
in  other bool rio2 ch:DI-02\n";

const NOTE: &str = "Wire each DI to the common terminal through the contact.";
const SOURCE: &str = "Moxa ioLogik E1213 manual, wiring chapter";
const WITH_NOTE: &str = r#"{"matches":[{"vendor":"Moxa","model":"ioLogik E1213","wiring":{"note":"Wire each DI to the common terminal through the contact.","source":"Moxa ioLogik E1213 manual, wiring chapter"}}]}"#;
const NO_FACTS: &str = r#"{"matches":[]}"#;
const KQ: &str = r#"{"matches":[{"device":{"vendor":"ifm","model":"KQxxxx","kind":"sensor","draft":true,"signals":[],"connections":[{"type":"Cable","products":["KQ6001","KQ6002"],"wires":[{"n":1,"function":"L+","color":"BN"},{"n":3,"function":"L-","color":"BU"},{"n":4,"function":"C/Q","color":"BK"}]}]}}]}"#;
const KQ_OLD: &str =
    r#"{"matches":[{"device":{"vendor":"ifm","model":"KQxxxx","kind":"sensor","draft":true,"signals":[]}}]}"#;

const NO_DEVICE_FACTS: &str =
    "The device library has no wiring facts for this device. Check the device's manual.";
const NO_SENSOR_FACTS: &str = "The library has no wiring facts for this sensor. Check its manual.";
const SIM_LINE: &str = "Simulation: no hardware here. Tick each step when you have wired it.";
const DOWN_LINE: &str = "The device is not answering, so there is nothing to watch yet.";

/// The latch (start, stop, motor) with the E1213's profile and `map`.
fn playground(map: &str) -> Harness {
    let mut h = Harness::new(App::sim());
    {
        let sim = h.app.sim.as_mut().unwrap();
        let ex = crate::playground::EXAMPLES
            .iter()
            .map(|e| e.src)
            .find(|src| src.contains("PROGRAM latch"))
            .map(|src| crate::sim_projects::example_project(src).0)
            .expect("the latch example");
        sim.open_project(&ex);
        sim.proj.create("mine", Some(&ex));
        sim.proj.add_file("moxa-iologik-e1213.device", E1213.into());
        sim.proj.add_file(helmsman_project::IO_MAP, map.into());
    }
    h.press("pane devices");
    h.wait(0.3);
    h
}

fn sim_ui(h: &mut Harness) -> &mut crate::devices::DevicesUi {
    &mut h.app.sim.as_mut().unwrap().devices_ui
}

fn box_ui(h: &mut Harness) -> &mut crate::devices::DevicesUi {
    &mut h.app.boxv.as_mut().unwrap().devices_ui
}

fn buf(text: &str) -> Buf {
    Buf {
        text: text.into(),
        base: text.into(),
        loaded: true,
        ..Buf::default()
    }
}

fn row(name: &str, value: Option<Value>) -> SignalRow {
    SignalRow {
        name: name.into(),
        dir: "in".into(),
        unit: None,
        value,
        quality: "good".into(),
        point_error: None,
        forced: false,
        sim: false,
    }
}

const BOX_PROGRAM: &str = "PROGRAM p\nVAR_INPUT\n  start : BOOL;\n  stop : BOOL;\n  lvl : REAL;\n  cnt : INT;\nEND_VAR\nVAR_OUTPUT\n  motor : BOOL;\nEND_VAR\nmotor := start AND NOT stop;\nEND_PROGRAM\n";
/// On the box page the device has no profile: points carry addresses.
const BOX_MAP: &str = "device rio modbus-tcp 10.0.0.57:502 unit=1\n\
in start bool rio di:5\nin stop bool rio di:1\nin lvl real rio hr:0\nin cnt int rio hr:1\nin spare bool rio di:3\nout motor bool rio do:0\n\
device rio2 modbus-tcp 10.0.0.58:502 unit=1\nin other bool rio2 di:2\n";

/// A box page with the project open and every signal's value in.
fn boxpage() -> Harness {
    let bench = Bench::new();
    let (mut state, _) = watch(&bench);
    state.io_map = Some(BOX_MAP.into());
    state.signals = vec![
        row("start", Some(Value::Bool(false))),
        row("stop", Some(Value::Bool(false))),
        row("lvl", Some(Value::Real(10.0))),
        row("cnt", Some(Value::Int(3))),
        row("spare", Some(Value::Bool(false))),
        row("other", Some(Value::Bool(false))),
    ];
    state.devices = ["rio", "rio2"]
        .map(|n| DeviceRow {
            name: n.into(),
            state: "up".into(),
            error: None,
            address: None,
            transport: None,
            health: None,
        })
        .to_vec();
    let mut h = Harness::new(App::boxed());
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    let v = h.app.boxv.as_mut().unwrap();
    v.state = state;
    v.projects.available = Some(true);
    let mut o = OpenProject {
        name: "demo".into(),
        ..OpenProject::default()
    };
    o.bufs.insert("main.st".into(), buf(BOX_PROGRAM));
    o.bufs.insert("io.map".into(), buf(BOX_MAP));
    v.projects.open = Some(o);
    h.frame(vec![]);
    h.press("pane devices");
    h.wait(0.3);
    // What the page's own start-up queued is not the walkthrough's.
    let v = h.app.boxv.as_mut().unwrap();
    v.sent.clear();
    v.psent.clear();
    h
}

fn set(h: &mut Harness, name: &str, v: Value) {
    let st = &mut h.app.boxv.as_mut().unwrap().state;
    st.signals.iter_mut().find(|s| s.name == name).unwrap().value = Some(v);
    h.frame(vec![]);
    h.frame(vec![]);
}

/// Opens the walkthrough on `device`. Its button is off until the
/// walkthrough works (Nick, 2026-11-26), so the window is opened as the
/// button did; the window itself is what these tests hold.
fn open(h: &mut Harness, device: &str) {
    if h.app.boxv.is_some() {
        box_ui(h).open_wire_it(device);
    } else {
        sim_ui(h).open_wire_it(device);
    }
    h.wait(0.3);
    assert!(h.has("wire form"), "the window opens");
}

fn y(h: &Harness, signal: &str) -> f32 {
    h.rect(&format!("wire step {signal}")).top()
}

/// Every length of text the last frame drew, longest first.
fn longest_text(h: &Harness) -> usize {
    fn walk(shape: &eframe::egui::Shape, best: &mut usize) {
        match shape {
            eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, best)),
            eframe::egui::Shape::Text(t) => *best = (*best).max(t.galley.job.text.chars().count()),
            _ => {}
        }
    }
    let mut best = 0;
    for c in &h.shapes {
        walk(&c.shape, &mut best);
    }
    best
}

fn drew_control(h: &Harness) -> bool {
    ['\u{0}', '\u{7}', '\u{1b}', '\u{8}']
        .iter()
        .any(|c| h.drew(&c.to_string()))
}

// ---- the button -------------------------------------------------------

#[test]
/// Nick, 2026-11-26: the Devices tab's Wire it button is off until the
/// walkthrough works, on the simulation and on a box page alike.
fn no_device_has_a_wire_it_button_while_the_walkthrough_is_off() {
    let mut h = playground(MAP);
    assert!(
        h.has("device rio") && h.has("device rio2"),
        "the devices are listed"
    );
    for d in ["rio", "rio2"] {
        assert!(!h.has(&format!("wire it {d}")), "no Wire it for {d}");
    }
    assert!(!h.drew("Wire it"), "no Wire it anywhere on the tab");
    assert!(!h.has("wire form"), "nothing opens it");
    // The window, opened as the button did, still closes and leaves the
    // map alone.
    let map = h
        .app
        .sim
        .as_ref()
        .unwrap()
        .proj
        .files
        .get(helmsman_project::IO_MAP)
        .cloned();
    open(&mut h, "rio");
    h.press("wire done");
    h.wait(0.2);
    assert!(!h.has("wire form"));
    assert_eq!(
        h.app
            .sim
            .as_ref()
            .unwrap()
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned(),
        map
    );
    let b = boxpage();
    assert!(b.has("device rio"), "the box page lists the device");
    assert!(
        !b.has("wire it rio") && !b.has("wire it rio2"),
        "none on a box page"
    );
}

// ---- which steps, in what order ---------------------------------------

#[test]
fn the_window_lists_only_the_inputs_the_program_uses_in_channel_order() {
    let mut h = playground(MAP);
    open(&mut h, "rio");
    sim_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    h.wait(0.2);
    assert!(h.has("wire step stop") && h.has("wire step start"));
    assert!(
        y(&h, "stop") < y(&h, "start"),
        "stop is DI-01 and start DI-05: channel order, not line order"
    );
    assert!(!h.has("wire step spare"), "no variable in the program");
    assert!(!h.has("wire step motor"), "an output is never listed");
    assert!(!h.has("wire step other"), "another device's input");
    assert!(
        h.drew("DI-01") && h.drew("DI-05"),
        "each channel as the device labels it"
    );
    // The second device lists its own, and none of the first's.
    h.press("wire done");
    h.wait(0.2);
    open(&mut h, "rio2");
    assert!(!h.has("wire step stop") && !h.has("wire step start"));
}

// ---- what the library says --------------------------------------------

#[test]
fn the_library_note_and_source_show_above_the_steps() {
    let mut h = playground(MAP);
    open(&mut h, "rio");
    sim_ui(&mut h).wiring_answer("rio", 200, WITH_NOTE);
    h.wait(0.2);
    assert!(h.drew(NOTE), "the note");
    assert!(h.drew(SOURCE), "and where it came from");
    assert!(!h.drew(NO_DEVICE_FACTS));
}

#[test]
fn a_sensors_wires_show_pin_colour_and_function_for_its_step() {
    let map = MAP.replace("ch:DI-05", "ch:DI-05 wired=ifm/kq");
    let mut h = playground(&map);
    open(&mut h, "rio");
    sim_ui(&mut h).wiring_answer("rio", 200, WITH_NOTE);
    sim_ui(&mut h).wired_answer("ifm/kq", 200, KQ);
    h.wait(0.3);
    for w in ["pin 1 BN L+", "pin 3 BU L-", "pin 4 BK C/Q"] {
        assert!(h.drew(w), "{w}");
    }
    assert!(h.drew("Cable"), "the connector");
    assert!(!h.drew(NO_SENSOR_FACTS), "this sensor has wires");
}

#[test]
fn a_device_with_no_facts_and_a_sensor_with_no_wires_say_so() {
    let map = MAP.replace("ch:DI-05", "ch:DI-05 wired=ifm/kq");
    let mut h = playground(&map);
    open(&mut h, "rio");
    sim_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    sim_ui(&mut h).wired_answer("ifm/kq", 200, KQ_OLD);
    h.wait(0.3);
    assert!(h.drew(NO_DEVICE_FACTS), "no match");
    assert!(h.drew(NO_SENSOR_FACTS), "a brief without connections");
    assert!(!h.drew("pin 1"));
    // A match with no `wiring` is the same line; so is a refusal.
    sim_ui(&mut h).wiring_answer(
        "rio",
        200,
        r#"{"matches":[{"vendor":"Moxa","model":"ioLogik E1213"}]}"#,
    );
    h.wait(0.2);
    assert!(h.drew(NO_DEVICE_FACTS));
    sim_ui(&mut h).wiring_answer("rio", 502, r#"{"error":"down"}"#);
    h.wait(0.2);
    assert!(
        h.has("wire form"),
        "an error answer does not close or break the window"
    );
    // A step with no wired= says nothing about a sensor.
    h.press("wire done");
    let h2 = playground(MAP);
    assert!(!h2.drew(NO_SENSOR_FACTS));
}

// ---- simulation ---------------------------------------------------------

#[test]
fn in_simulation_steps_are_ticked_or_skipped_by_hand_and_nothing_is_written() {
    let mut h = playground(MAP);
    let (map_before, files_before) = {
        let p = &h.app.sim.as_ref().unwrap().proj;
        (p.files[helmsman_project::IO_MAP].clone(), p.files.clone())
    };
    open(&mut h, "rio");
    sim_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    h.wait(0.2);
    assert!(h.drew(SIM_LINE));
    assert!(!h.drew("Seen:") && !h.drew("changed instead"), "no live text");
    assert!(h.has("wire tick stop") && h.has("wire skip stop"));
    assert!(h.has("wire tick start") && h.has("wire skip start"));
    h.press("wire tick stop");
    h.wait(0.2);
    assert!(h.drew("Ticked by hand"));
    h.press("wire skip start");
    h.wait(0.2);
    assert!(h.drew("Skipped"));
    assert!(!h.drew("Seen:"));
    // Close and open again: the ticks stay.
    h.press("wire done");
    h.wait(0.2);
    open(&mut h, "rio");
    assert!(
        h.drew("Ticked by hand") && h.drew("Skipped"),
        "kept for the session"
    );
    // Nothing of the project changed.
    let p = &h.app.sim.as_ref().unwrap().proj;
    assert_eq!(p.files[helmsman_project::IO_MAP], map_before);
    assert_eq!(p.files, files_before);
}

// ---- a box ---------------------------------------------------------------

/// Everything a box page sent so far: only library lookups by GET.
fn only_lookups(h: &mut Harness) -> usize {
    let v = h.app.boxv.as_mut().unwrap();
    assert!(
        v.sent.is_empty(),
        "the walkthrough sent a command: {:?}",
        v.sent.len()
    );
    let reqs = std::mem::take(&mut v.psent);
    for r in &reqs {
        assert_eq!(r.method, "GET", "{} {}", r.method, r.url);
        assert!(r.url.starts_with("/api/devices/library?"), "{}", r.url);
    }
    reqs.iter()
        .filter(|r| r.call == PCall::FindWired || r.url.contains("wiring=1"))
        .count()
}

#[test]
fn on_a_box_a_change_of_the_waiting_steps_signal_ticks_it_with_what_was_seen() {
    let mut h = boxpage();
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    h.wait(0.2);
    assert!(!h.drew(SIM_LINE), "a box is not the simulation");
    assert!(!h.drew("Seen:"), "nothing seen yet");
    assert!(y(&h, "stop") < y(&h, "start"));
    // stop is DI-01: it waits first. It goes on.
    set(&mut h, "stop", Value::Bool(true));
    assert!(h.drew("Seen: went on"), "ticked with what was seen");
    assert!(!h.drew("then off"));
    set(&mut h, "stop", Value::Bool(false));
    assert!(h.drew("Seen: went on, then off"));
    // The next one waits now.
    set(&mut h, "start", Value::Bool(true));
    assert!(h.drew("Seen: went on"));
    assert!(!h.drew("changed instead"));
    only_lookups(&mut h);
}

#[test]
fn a_change_of_another_input_of_the_device_is_named_and_does_not_tick() {
    let mut h = boxpage();
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    h.wait(0.2);
    // The waiting step is stop; start is another input of the same device.
    set(&mut h, "start", Value::Bool(true));
    assert!(h.drew("start changed instead: wired to the wrong terminal?"));
    assert!(!h.drew("Seen:"), "nothing was ticked by it");
    // The right one still ticks afterwards.
    set(&mut h, "stop", Value::Bool(true));
    assert!(h.drew("Seen: went on"));
    // An input of another device, or one the program does not use, is not reported.
    let mut h = boxpage();
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    set(&mut h, "other", Value::Bool(true));
    set(&mut h, "spare", Value::Bool(true));
    assert!(!h.drew("other changed instead") && !h.drew("spare changed instead"));
    assert!(!h.drew("Seen:"));
}

#[test]
fn analog_inputs_tick_on_a_change_beyond_noise_only() {
    let mut h = boxpage();
    // Skip the two bool steps so that lvl (hr:0) and cnt (hr:1) wait in turn.
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    for s in ["stop", "start"] {
        h.press(&format!("wire skip {s}"));
        h.wait(0.2);
    }
    let first_analog = if y(&h, "lvl") < y(&h, "cnt") { "lvl" } else { "cnt" };
    assert_eq!(first_analog, "lvl", "channel order by address: hr:0 before hr:1");
    set(&mut h, "lvl", Value::Real(10.001));
    assert!(!h.drew("Seen:"), "a change of 0.001 is noise");
    set(&mut h, "lvl", Value::Real(11.5));
    assert!(h.drew("Seen:"), "a change of at least 1.0 ticks");
    // An integer ticks on any change.
    let before = h.drew("Seen: ");
    assert!(before);
    set(&mut h, "cnt", Value::Int(4));
    assert!(h.drew("Seen:"));
    only_lookups(&mut h);
}

#[test]
fn a_down_device_says_it_is_not_answering_and_does_not_tick() {
    let mut h = boxpage();
    h.app.boxv.as_mut().unwrap().state.devices[0].state = "down".into();
    h.frame(vec![]);
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    h.wait(0.2);
    assert!(h.drew(DOWN_LINE));
    set(&mut h, "stop", Value::Bool(true));
    assert!(!h.drew("Seen:"), "no watching while the link is down");
    assert!(!h.drew("changed instead"));
    // The other device is not affected.
    h.press("wire done");
    h.wait(0.2);
    open(&mut h, "rio2");
    assert!(!h.drew(DOWN_LINE));
}

#[test]
fn on_a_box_ticks_by_hand_and_skips_exist_and_survive_closing() {
    let mut h = boxpage();
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    h.wait(0.2);
    h.press("wire tick stop");
    h.wait(0.2);
    assert!(h.drew("Ticked by hand"));
    h.press("wire skip start");
    h.wait(0.2);
    assert!(h.drew("Skipped"));
    h.press("wire done");
    h.wait(0.2);
    assert!(!h.has("wire form"));
    set(&mut h, "lvl", Value::Real(20.0));
    open(&mut h, "rio");
    assert!(
        h.drew("Ticked by hand") && h.drew("Skipped"),
        "the ticks are kept"
    );
    // A watched tick is kept too.
    let mut h = boxpage();
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, NO_FACTS);
    set(&mut h, "stop", Value::Bool(true));
    h.press("wire done");
    h.wait(0.2);
    open(&mut h, "rio");
    assert!(h.drew("Seen: went on"));
}

const IDENT_MAP: &str = "device rio modbus-tcp 10.0.0.57:502 unit=1 cip=991:515:0x00e12130\n\
in start bool rio di:5\nin stop bool rio di:1\nin lvl real rio hr:0\nin cnt int rio hr:1\nin spare bool rio di:3\nout motor bool rio do:0\n\
device rio2 modbus-tcp 10.0.0.58:502 unit=1\nin other bool rio2 di:2\n";

#[test]
fn the_walkthrough_sends_nothing_but_library_lookups() {
    let mut h = boxpage();
    {
        let v = h.app.boxv.as_mut().unwrap();
        v.state.io_map = Some(IDENT_MAP.into());
        v.projects
            .open
            .as_mut()
            .unwrap()
            .bufs
            .insert("io.map".into(), buf(IDENT_MAP));
    }
    h.frame(vec![]);
    open(&mut h, "rio");
    box_ui(&mut h).wiring_answer("rio", 200, WITH_NOTE);
    set(&mut h, "stop", Value::Bool(true));
    h.press("wire tick start");
    h.press("wire skip lvl");
    h.press("wire done");
    h.wait(0.2);
    open(&mut h, "rio");
    h.press("wire done");
    h.wait(0.2);
    let v = h.app.boxv.as_mut().unwrap();
    assert_eq!(v.sent.len(), 0, "the walkthrough sent a command");
    let reqs = std::mem::take(&mut v.psent);
    for r in &reqs {
        assert_eq!(r.method, "GET", "{} {}", r.method, r.url);
        assert!(r.url.starts_with("/api/devices/library?"), "{}", r.url);
    }
    assert!(
        reqs.iter().any(|r| r.url.contains("vendor=991")
            && r.url.contains("product=515")
            && r.url.contains("wiring=1")),
        "the device's wiring is looked up: {:?}",
        reqs.iter().map(|r| r.url.clone()).collect::<Vec<_>>()
    );
    let o = h.app.boxv.as_ref().unwrap().projects.open.as_ref().unwrap();
    assert_eq!(o.bufs["io.map"].text, IDENT_MAP);
    assert_eq!(o.bufs["main.st"].text, BOX_PROGRAM);
}

#[test]
fn a_device_that_cannot_be_identified_queues_no_lookup() {
    let mut h = boxpage();
    open(&mut h, "rio");
    h.wait(0.3);
    let v = h.app.boxv.as_mut().unwrap();
    assert_eq!(v.sent.len(), 0);
    assert!(
        v.psent.is_empty(),
        "nothing is guessed: {:?}",
        v.psent.iter().map(|r| r.url.clone()).collect::<Vec<_>>()
    );
    assert!(h.drew(NO_DEVICE_FACTS));
}

// ---- hostile inputs -----------------------------------------------------

#[test]
fn a_huge_note_a_thousand_wires_and_control_characters_stay_bounded() {
    let note = format!("\u{0}\u{7}\u{1b}[31m{}\nx\u{8}", "N".repeat(100_000));
    let body = format!(
        r#"{{"matches":[{{"vendor":"Moxa","model":"m","wiring":{{"note":{},"source":{}}}}}]}}"#,
        serde_json::to_string(&note).unwrap(),
        serde_json::to_string(&format!("\u{1b}{}", "S".repeat(50_000))).unwrap()
    );
    let wires: Vec<String> = (1..=1000)
        .map(|n| {
            format!(
                r#"{{"n":{n},"function":"F{n}\u0000{}","color":"C\u001b{n}"}}"#,
                "f".repeat(300)
            )
        })
        .collect();
    let kq = format!(
        r#"{{"matches":[{{"device":{{"vendor":"ifm","model":"KQxxxx","kind":"sensor","draft":true,"signals":[],"connections":[{{"type":"Cable","products":[],"wires":[{}]}}]}}}}]}}"#,
        wires.join(",")
    );
    let map = MAP.replace("ch:DI-05", "ch:DI-05 wired=ifm/kq");
    let mut h = playground(&map);
    let left = h.rect("left").right();
    open(&mut h, "rio");
    sim_ui(&mut h).wiring_answer("rio", 200, &body);
    sim_ui(&mut h).wired_answer("ifm/kq", 200, &kq);
    for _ in 0..3 {
        h.wait(0.3);
    }
    assert!(h.has("wire form"), "still up");
    let f = h.rect("wire form");
    assert!(
        f.left() >= -1.0 && f.right() <= h.size.x + 1.0,
        "{f:?} is outside the screen"
    );
    for s in ["stop", "start"] {
        let r = h.rect(&format!("wire step {s}"));
        assert!(
            r.left() >= f.left() - 1.0 && r.right() <= f.right() + 1.0,
            "{s}: {r:?} outside {f:?}"
        );
    }
    assert!(
        longest_text(&h) < 10_000,
        "a 100 kB note is cut: {}",
        longest_text(&h)
    );
    assert!(!drew_control(&h), "control characters are not drawn");
    assert!(h.rect("left").right() - left <= 8.0, "the pane kept its width");
    h.press("wire tick stop");
    h.wait(0.2);
    h.press("wire done");
    h.wait(0.2);
    assert!(!h.has("wire form"));
}

#[test]
fn the_window_open_leaves_the_panels_their_width() {
    let mut h = playground(MAP);
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.wait(0.3);
    let left = h.rect("left");
    let right = h.rect("right");
    open(&mut h, "rio");
    sim_ui(&mut h).wiring_answer("rio", 200, WITH_NOTE);
    for key in [
        "tab io",
        "pane files",
        "pane devices",
        "tab claude",
        "tab io",
        "pane devices",
    ] {
        h.press(key);
        h.wait(0.3);
        let l = h.rect("left").right() - left.right();
        let r = right.left() - h.rect("right").left();
        assert!((0.0..=8.0).contains(&l), "after {key} the left pane grew {l} pt");
        assert!(
            (0.0..=8.0).contains(&r),
            "after {key} the right panel grew {r} pt"
        );
    }
}
