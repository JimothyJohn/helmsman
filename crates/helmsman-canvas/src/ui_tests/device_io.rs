//! A device's inputs and outputs, added one at a time (`crate::device_io`;
//! Nick, 2026-11-15): the Devices tab lists the points the project has
//! and one row for the rest, + Add I/O narrows and adds them, a point is
//! removed from its row, and a device that holds every channel (as Add
//! wrote them before) offers to remove the ones the program does not use.
//! All of it inside the pane's default width.

use std::fmt::Write as _;

use eframe::egui::Event;

use super::{App, Harness};

const E1213: &str = include_str!("../../../../docs/devices/moxa-iologik-e1213.device");
const DEVICE: &str = "device rio device:moxa-iologik-e1213 cip=991:515:0x00e12130\n";

/// The latch (start, stop, motor) as a project of the visitor's own, with
/// the E1213's profile and `map`.
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

fn map(h: &Harness) -> String {
    h.app.sim.as_ref().unwrap().proj.files[helmsman_project::IO_MAP].clone()
}

fn program(h: &Harness) -> String {
    let sim = h.app.sim.as_ref().unwrap();
    sim.proj
        .files
        .values()
        .find(|t| t.contains("PROGRAM"))
        .unwrap()
        .clone()
}

/// The left pane's right edge: its fold button sits on it (the measure
/// `every_tab_leaves_the_panels_their_width` takes).
fn edge(h: &Harness) -> f32 {
    h.rect("left").right()
}

/// The left pane did not grow past where it was, and `key` sits inside it.
fn inside_the_pane(h: &Harness, key: &str, was: f32) {
    let now = edge(h);
    assert!(
        (now - was).abs() <= 1.0,
        "the pane grew with {key}: {was} -> {now}"
    );
    let r = h.rect(key);
    assert!(
        r.left() >= 0.0 && r.right() <= now + 1.0,
        "{key} {r:?} is outside the pane, which ends at {now}"
    );
}

#[test]
fn a_device_lists_the_points_the_project_has_and_one_row_for_the_rest() {
    let mut h = playground(&format!("{DEVICE}in  part bool rio ch:DI-04\n"));
    let left = edge(&h);
    assert!(
        left <= crate::editor::LEFT_W + 8.0,
        "the pane at its default width: it ends at {left}"
    );
    assert!(h.has("point part"), "the one point the project has");
    assert!(!h.has("point di_00"), "no row for a channel nobody added");
    assert!(h.has("add io rio"));
    inside_the_pane(&h, "add io rio", left);
    // `part` is not a variable of the latch: the row offers to remove it,
    // still inside the pane.
    assert!(h.has("remove unused rio"));
    inside_the_pane(&h, "remove unused rio", left);

    // The list: every channel but the one in the project, narrowed as typed.
    h.press("add io rio");
    h.wait(0.3);
    assert!(h.has("io form"));
    assert!(h.has("io add DI-00") && h.has("io add DO-00"));
    assert!(
        !h.has("io add DI-04"),
        "a channel in the project is not offered again"
    );
    h.press("io search");
    h.frame(vec![Event::Text("do-0 OUT".into())]);
    h.wait(0.2);
    assert!(h.has("io add DO-00") && h.has("io add DO-07"));
    assert!(
        !h.has("io add DI-00") && !h.has("io add DO-00-status"),
        "inputs are narrowed away"
    );
    // A click adds one: its signal after the device's lines, its variable
    // in the program. The list stays for the next.
    h.press("io add DO-00");
    h.wait(0.3);
    assert!(h.has("io form") && !h.has("io add DO-00"));
    assert_eq!(
        map(&h),
        format!("{DEVICE}in  part bool rio ch:DI-04\nout do_00 bool rio ch:DO-00 safe=false\n")
    );
    let st = program(&h);
    assert!(st.contains("  do_00 : BOOL;"), "{st}");
    assert!(helmsman_iec::ast::parse(&st).is_ok(), "{st}");
    // Nothing matches: said, not an empty box.
    h.press("io search");
    h.frame(vec![Event::Text("zzz".into())]);
    h.wait(0.2);
    assert!(!h.has("io add DO-01"));
    h.press("io done");
    h.wait(0.3);
    assert!(!h.has("io form"));
    assert!(h.has("point do_00"), "the added point is under the device");
    inside_the_pane(&h, "point do_00", left);

    // Remove it from its row: asked first, the variable stays, and the
    // channel is back in the list.
    h.right_click(h.rect("point do_00").center());
    h.press("menu remove point");
    h.wait(0.2);
    assert!(h.has("remove io form"));
    h.press("remove io cancel");
    h.wait(0.2);
    assert!(map(&h).contains("out do_00 "), "Cancel keeps it");
    h.right_click(h.rect("point do_00").center());
    h.press("menu remove point");
    h.wait(0.2);
    h.press("remove io ok");
    h.wait(0.3);
    assert_eq!(map(&h), format!("{DEVICE}in  part bool rio ch:DI-04\n"));
    assert!(program(&h).contains("  do_00 : BOOL;"), "the variable stays");
    // Added again, it takes that variable back: nothing is declared twice.
    h.press("add io rio");
    h.wait(0.3);
    h.press("io search");
    h.frame(vec![Event::Text("out".into())]);
    h.wait(0.2);
    h.press("io add DO-00");
    h.wait(0.3);
    h.press("io done");
    h.wait(0.2);
    assert!(
        map(&h).contains("out do_00 bool rio ch:DO-00 safe=false\n"),
        "{} {:?}",
        map(&h),
        h.app.sim.as_ref().unwrap().proj.notice
    );
    let st = program(&h);
    assert_eq!(st.matches("  do_00 : BOOL;").count(), 1, "{st}");
    assert!(helmsman_iec::ast::parse(&st).is_ok(), "{st}");
}

#[test]
fn a_device_holding_every_channel_offers_to_remove_the_ones_the_program_does_not_use() {
    // As Add wrote it before: a signal a channel, forty of them; the
    // program's start is on one.
    let mut text = String::from("in  stop bool box di:1\n");
    text.push_str(DEVICE);
    let skim = helmsman_project::profile_skim::skim(E1213).unwrap();
    for c in &skim.channels {
        let name = if c.name == "DI-00" {
            "start".to_string()
        } else {
            helmsman_project::profile_skim::signal_name("rio", &c.name)
        };
        let _ = writeln!(text, "{:<3} {name} {} rio ch:{}", c.dir, c.ty, c.name);
    }
    let mut h = playground(&text);
    let left = edge(&h);
    assert!(h.has("remove unused rio"));
    inside_the_pane(&h, "remove unused rio", left);
    // Every channel is in the project: nothing is left to add.
    h.press("add io rio");
    h.wait(0.2);
    assert!(!h.has("io form"), "+ Add I/O is off when no channel is left");
    h.press("remove unused rio");
    h.wait(0.2);
    assert!(h.has("remove io form"), "it asks first");
    h.press("remove io cancel");
    h.wait(0.2);
    assert_eq!(map(&h), text, "Cancel changes nothing");
    h.press("remove unused rio");
    h.wait(0.2);
    h.press("remove io ok");
    h.wait(0.3);
    assert_eq!(
        map(&h),
        format!("in  stop bool box di:1\n{DEVICE}in  start bool rio ch:DI-00\n"),
        "the program's point and every other line stay"
    );
    assert!(!h.has("remove unused rio"), "nothing unused is left");
    assert!(h.has("point start") && !h.has("point rio_di_01"));
    inside_the_pane(&h, "add io rio", left);
    // The thirty-nine are back in the list.
    h.press("add io rio");
    h.wait(0.3);
    assert!(h.has("io add DI-01") && h.has("io add DO-07-status"));
    assert!(!h.has("io add DI-00"));
}

#[test]
fn a_device_nobody_knows_takes_a_point_by_its_address() {
    let mut h = playground("device plc modbus-tcp 10.0.0.9:502 unit=1\n");
    let left = edge(&h);
    assert!(h.has("add io plc"));
    inside_the_pane(&h, "add io plc", left);
    h.press("add io plc");
    h.wait(0.3);
    assert!(h.has("io form") && h.has("io custom point"));
    assert!(!h.has("io search"), "no list to search");
    // A point that is not Modbus is refused in words, and nothing is written.
    h.press("io custom point");
    h.frame(vec![Event::Text("di:0 # x".into())]);
    h.wait(0.2);
    h.press("io custom add");
    h.wait(0.3);
    assert_eq!(map(&h), "device plc modbus-tcp 10.0.0.9:502 unit=1\n");
    let err = h
        .app
        .sim
        .as_ref()
        .unwrap()
        .devices_ui
        .adding_io
        .as_ref()
        .unwrap()
        .error
        .clone();
    assert!(err.is_some(), "the refusal is shown");
    // A good one goes in as an output of its type.
    h.app
        .sim
        .as_mut()
        .unwrap()
        .devices_ui
        .adding_io
        .as_mut()
        .unwrap()
        .custom
        .as_mut()
        .unwrap()
        .point = "coil:3".into();
    h.press("io custom out");
    h.wait(0.2);
    h.press("io custom add");
    h.wait(0.3);
    assert_eq!(
        map(&h),
        "device plc modbus-tcp 10.0.0.9:502 unit=1\nout coil_3 bool plc coil:3 safe=false\n"
    );
    assert!(program(&h).contains("  coil_3 : BOOL;"));
}

/// Nick 2026-11-25: "another cleanup button on the Devices tab that gets
/// rid of all unused I/O at the same time for all devices." Two devices
/// each with points the program does not use: one Clean up asks once and
/// takes them all out, the program's points and the other lines stay.
#[test]
fn clean_up_takes_every_devices_unused_points_out_at_once() {
    let second = "device rio2 device:moxa-iologik-e1213 cip=991:515:0x00e12131\n";
    let text = format!(
        "in  stop bool box di:1\n{DEVICE}in  start bool rio ch:DI-00\nin  rio_di_01 bool rio ch:DI-01\n{second}in  rio2_di_02 bool rio2 ch:DI-02\nout rio2_do_00 bool rio2 ch:DO-00\n"
    );
    let mut h = playground(&text);
    let left = edge(&h);
    assert!(h.has("devices clean up"), "one button for every device");
    inside_the_pane(&h, "devices clean up", left);
    h.press("devices clean up");
    h.wait(0.2);
    assert!(h.has("remove io form"), "it asks first");
    h.press("remove io cancel");
    h.wait(0.2);
    assert_eq!(map(&h), text, "Cancel changes nothing");
    h.press("devices clean up");
    h.wait(0.2);
    h.press("remove io ok");
    h.wait(0.3);
    assert_eq!(
        map(&h),
        format!("in  stop bool box di:1\n{DEVICE}in  start bool rio ch:DI-00\n{second}"),
        "both devices' unused points go; the program's and the devices stay"
    );
    assert!(!h.has("devices clean up"), "nothing unused is left");
}

/// Nick, 2026-11-25: "Remove N unused point(s)" was drawn over the list
/// with the rows showing through. The question is opaque from the first
/// frame it is painted (no fade-in), and the pane keeps its width.
#[test]
fn the_remove_question_is_opaque_over_the_list() {
    use eframe::egui::{Rect, Shape};
    /// (the question's background, if painted; whether its text is faded)
    fn painted(s: &Shape, form: Rect, out: &mut (Option<u8>, bool)) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| painted(s, form, out)),
            Shape::Rect(r)
                if r.rect.width() > 300.0
                    && r.rect.width() <= form.width() + 1.0
                    && r.rect.intersects(form)
                    && r.fill.a() > 0 =>
            {
                out.0 = Some(r.fill.a());
            }
            Shape::Text(t) if form.contains(t.pos) && t.opacity_factor < 1.0 => out.1 = true,
            _ => {}
        }
    }
    let mut h = playground(&format!("{DEVICE}in  part bool rio ch:DI-04\n"));
    let left = edge(&h);
    let at = h.rect("remove unused rio").center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Harness::button(at, true)]);
    h.frame(vec![Harness::button(at, false)]);
    // The first frame it is painted (the one before it only measures).
    let mut seen = (None, false);
    for _ in 0..8 {
        h.frame(vec![]);
        if h.has("remove io form") {
            let form = h.rect("remove io form");
            seen = (None, false);
            for c in &h.shapes {
                painted(&c.shape, form, &mut seen);
            }
            if seen.0.is_some() {
                break;
            }
        }
    }
    assert_eq!(seen.0, Some(255), "an opaque background behind the question");
    assert!(!seen.1, "its text is not faded over the rows");
    inside_the_pane(&h, "remove unused rio", left);
}

/// Nick, 2026-11-26: a point is named by its channel alone, and the
/// device goes in front only when that name is taken. Two E1213s in one
/// project, DI-00 added on each from the Devices tab: the first is
/// `di_00`, the second `cell_b_di_00`, each declared once in the program.
#[test]
fn two_devices_with_one_channel_name_get_the_short_name_and_the_prefixed_one() {
    let second = "device cell_b device:moxa-iologik-e1213 cip=991:515:0x00e12131\n";
    let mut h = playground(&format!("{DEVICE}{second}"));
    for dev in ["rio", "cell_b"] {
        h.press(&format!("add io {dev}"));
        h.wait(0.3);
        assert!(h.has("io add DI-00"), "{dev}: DI-00 is offered");
        h.press("io add DI-00");
        h.wait(0.3);
        h.press("io done");
        h.wait(0.3);
        assert!(!h.has("io form"));
    }
    assert_eq!(
        map(&h),
        format!("{DEVICE}in  di_00 bool rio ch:DI-00\n{second}in  cell_b_di_00 bool cell_b ch:DI-00\n")
    );
    let st = program(&h);
    assert_eq!(st.matches("  di_00 : BOOL;").count(), 1, "{st}");
    assert_eq!(st.matches("  cell_b_di_00 : BOOL;").count(), 1, "{st}");
    assert!(helmsman_iec::ast::parse(&st).is_ok(), "{st}");
    assert!(h.has("point di_00") && h.has("point cell_b_di_00"));
}

/// A profile's channel names are the vendor's words. Named by the
/// channel alone (Nick, 2026-11-26), a channel called `AND`, `If`, `END_VAR` or
/// `Program` would be a keyword in the program: the add is refused or
/// the name made safe, and the program still parses either way. A
/// channel name that fits a Skipper's 32 characters is kept whole
/// ("a long device name is cut, never the channel").
#[test]
fn a_channel_named_like_a_keyword_leaves_the_program_whole_and_one_that_fits_is_kept() {
    let long = "Conveyor_Infeed_Photo_Eye_Sensor";
    assert_eq!(long.len(), crate::skipper_map::NAME_MAX);
    let profile = E1213
        .replace("in  DI-05        bool", "in  AND          bool")
        .replace("in  DI-06        bool", "in  If           bool")
        .replace("in  DI-07        bool", "in  END_VAR      bool")
        .replace("in  DI-08        bool", "in  Program      bool")
        .replace("in  DI-09        bool", &format!("in  {long} bool"));
    let mut h = playground("device rio device:acme-rio cip=991:515:0x00e12130\n");
    h.app
        .sim
        .as_mut()
        .unwrap()
        .proj
        .add_file("acme-rio.device", profile);
    h.press("pane devices");
    h.wait(0.3);
    for label in ["AND", "If", "END_VAR", "Program", long] {
        h.press("add io rio");
        h.wait(0.3);
        h.press("io search");
        h.frame(vec![Event::Text(label.into())]);
        h.wait(0.2);
        assert!(h.has(&format!("io add {label}")), "{label} is offered");
        h.press(&format!("io add {label}"));
        h.wait(0.3);
        h.press("io done");
        h.wait(0.3);
        let st = program(&h);
        assert!(
            helmsman_iec::ast::parse(&st).is_ok(),
            "after adding {label}:\n{}\n{st}",
            map(&h)
        );
    }
    assert!(
        map(&h).contains(&format!("in  {} bool rio ch:{long}\n", long.to_ascii_lowercase())),
        "{}",
        map(&h)
    );
}
