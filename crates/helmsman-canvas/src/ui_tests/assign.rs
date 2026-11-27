//! Assigning a program's input or output to a device channel
//! (`crate::assign`), driven through egui in the simulation: from the I/O
//! tab, from the diagram's block, and from the Devices tab's point row,
//! each leaving the project's io.map as text a person can read, plus the
//! places the items are off and say why, inside the panels' default width.

use eframe::egui::Event;
use helmsman_diagram::NodeKind;

use super::{Harness, sim_view};

const PROGRAM: &str = "\
PROGRAM line
VAR_INPUT
  start : BOOL;
  stop : BOOL;
  level : INT;
  spare : BOOL;
END_VAR
VAR_OUTPUT
  motor : BOOL;
  speed : INT;
END_VAR
motor := start AND NOT stop;
speed := level;
END_PROGRAM
";

/// The profile of the device the map names, as the project holds it.
const PROFILE: &str = include_str!("../../../../docs/devices/moxa-iologik-e1213.device");

const MAP: &str = "\
# the line's wiring
device rio device:moxa-iologik-e1213 cip=991:515:0x00e12130
in  rio_di_04 bool rio ch:DI-04 poll_ms=20  # the E-stop contact
in  rio_di_05 bool rio ch:DI-05
out rio_do_00 bool rio ch:DO-00 safe=true
in  rio_cnt   int  rio ch:DI-00-count unit=mm

in  start bool box di:0  # the button
in  stop bool box di:1
in  spare bool box di:2
out motor bool box do:0 safe=false
out speed int screen speed safe=0
";

/// The simulation with a project of one's own holding `PROGRAM` and `map`,
/// saved and running, the I/O tab docked in the right panel.
fn project(map: &str) -> Harness {
    let mut h = Harness::code();
    // Every simulated card carries its SIM line now: tall enough that the
    // outputs are on screen under the four inputs.
    h.size.y = 1500.0;
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("line", None);
        v.proj.files.insert("main.st".into(), PROGRAM.into());
        v.proj.add_file("moxa-iologik-e1213.device", PROFILE.into());
        if !map.is_empty() {
            v.proj.add_file(helmsman_project::IO_MAP, map.into());
        }
        v.proj.save();
        v.open_project("line");
    }
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.wait(0.4);
    h.press("tab io");
    h.wait(0.3);
    h
}

fn map(h: &Harness) -> String {
    h.app.sim.as_ref().unwrap().proj.files[helmsman_project::IO_MAP].clone()
}

fn dirty(h: &Harness) -> bool {
    h.app.sim.as_ref().unwrap().proj.dirty()
}

fn note(h: &Harness) -> Option<(String, bool)> {
    h.app.sim.as_ref().unwrap().proj.notice.clone()
}

/// Where a right click on the card of row `key` lands: its first line,
/// clear of the glyphs, which in a narrow card sit on a row of their own
/// under it (2026-11-26) where the card's middle used to be blank.
fn on_card(h: &Harness, key: &str) -> eframe::egui::Pos2 {
    h.rect(key).left_top() + eframe::egui::vec2(12.0, 12.0)
}

/// Right-click the row `key`, press the menu item `item`.
fn menu(h: &mut Harness, key: &str, item: &str) {
    h.right_click(on_card(h, key));
    h.press(item);
    h.wait(0.2);
}

/// The map with the line `from` written as `to`.
fn with(from: &str, to: &str) -> String {
    assert!(MAP.contains(from), "{from}");
    MAP.replace(from, to)
}

/// The tooltip text under the pointer at `key` is drawn.
fn says(h: &mut Harness, key: &str, text: &str) -> bool {
    h.frame(vec![Event::PointerMoved(h.rect(key).center())]);
    h.wait(1.5);
    h.drew(text)
}

#[test]
fn an_io_row_has_rename_which_opens_the_question_on_its_name() {
    let mut h = project(MAP);
    menu(&mut h, "io row spare", "menu rename io");
    assert!(h.has("rename form"), "the question opens");
    assert_eq!(
        h.app
            .sim
            .as_ref()
            .unwrap()
            .devices_ui
            .renaming
            .as_ref()
            .map(|r| r.0.as_str()),
        Some("spare")
    );
    assert!(!dirty(&h), "nothing written until Rename is pressed");
}

#[test]
fn an_input_row_assigns_to_a_free_channel_of_its_direction_and_type() {
    let mut h = project(MAP);
    // The program reads the others on the diagram; this one only the tab lists.
    assert!(h.has("io row spare"), "the input's row");
    assert!(says(
        &mut h,
        "io row spare",
        "Simulated: not assigned to a device yet"
    ));
    menu(&mut h, "io row spare", "menu assign port");
    assert!(h.has("assign form"), "the question opens");
    assert!(h.has("assign channel rio ch:DI-04") && h.has("assign channel rio ch:DI-05"));
    assert!(
        !h.has("assign channel rio ch:DO-00"),
        "an output is not offered an input"
    );
    assert!(
        !h.has("assign channel rio ch:DI-00-count"),
        "an int is not offered a bool"
    );
    h.press("assign channel rio ch:DI-05");
    h.wait(0.3);
    assert!(!h.has("assign form"));
    let want = with(
        "in  rio_di_05 bool rio ch:DI-05\n",
        "in  spare bool rio ch:DI-05\n",
    )
    .replace("in  spare bool box di:2\n", "");
    assert_eq!(map(&h), want);
    crate::assign::testing::loads_with(&map(&h), PROFILE);
    assert!(dirty(&h), "the project has an edit to save");
    let (said, is_error) = note(&h).expect("a note");
    assert!(!is_error && said.contains("spare is now rio, DI-05"), "{said}");
    // The row says where the point is now.
    assert!(says(&mut h, "io row spare", "On rio, DI-05"));
}

#[test]
fn an_output_row_assigns_and_goes_back_to_simulated_io() {
    let mut h = project(MAP);
    menu(&mut h, "io row motor", "menu assign port");
    assert!(h.has("assign channel rio ch:DO-00"));
    assert!(
        !h.has("assign channel rio ch:DI-04"),
        "an input is not offered an output"
    );
    h.press("assign channel rio ch:DO-00");
    h.wait(0.3);
    let moved = with(
        "out rio_do_00 bool rio ch:DO-00 safe=true\n",
        "out motor bool rio ch:DO-00 safe=true\n",
    )
    .replace("out motor bool box do:0 safe=false\n", "");
    assert_eq!(map(&h), moved);
    // On a device now: assigning again says to put it back first, and back is offered.
    h.right_click(on_card(&h, "io row motor"));
    assert!(h.has("menu assign port") && h.has("menu back to simulated"));
    h.press("menu assign port");
    h.wait(0.2);
    assert!(!h.has("assign form"), "off: it is on a device already");
    h.right_click(on_card(&h, "io row motor"));
    h.press("menu back to simulated");
    h.wait(0.3);
    let back = map(&h);
    assert!(
        !back.contains("ch:DO-00"),
        "the line goes, as Remove takes it: {back}"
    );
    assert!(back.ends_with("out motor bool box do:0 safe=false\n"), "{back}");
    crate::assign::testing::loads_with(&back, PROFILE);
    let (said, _) = note(&h).unwrap();
    assert!(said.contains("motor is back on simulated I/O"), "{said}");
}

#[test]
fn with_no_free_channel_the_item_is_off_and_says_to_add_a_device() {
    let mut h =
        project("in  start bool box di:0\nout motor bool box do:0 safe=false\nin spare bool box di:1\n");
    let before = map(&h);
    h.right_click(on_card(&h, "io row spare"));
    assert!(h.has("menu assign port"));
    assert!(says(
        &mut h,
        "menu assign port",
        "Add a device in the Devices tab first."
    ));
    h.press("menu assign port");
    h.wait(0.2);
    assert!(!h.has("assign form"), "nothing opens");
    assert_eq!(map(&h), before);
    assert!(!dirty(&h));
}

#[test]
fn an_example_is_read_only_and_the_item_says_so() {
    let mut h = Harness::code();
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.wait(0.3);
    h.press("tab io");
    h.wait(0.3);
    let key = h
        .app
        .chrome
        .hits
        .iter()
        .map(|(k, _)| k.clone())
        .find(|k| k.starts_with("io row "))
        .expect("an output row");
    h.right_click(on_card(&h, &key));
    assert!(h.has("menu assign port"));
    assert!(says(&mut h, "menu assign port", "The examples are read-only"));
    h.press("menu assign port");
    h.wait(0.2);
    assert!(!h.has("assign form"));
}

#[test]
fn the_devices_tab_row_feeds_an_unassigned_port_and_the_reverse_is_there_too() {
    let mut h = project(MAP);
    h.press("pane devices");
    h.wait(0.3);
    menu(&mut h, "point rio_di_04", "menu assign channel");
    assert!(h.has("assign form"));
    assert!(h.has("assign port start") && h.has("assign port stop") && h.has("assign port spare"));
    assert!(!h.has("assign port level"), "an int does not take a bool channel");
    assert!(
        !h.has("assign port motor"),
        "an output does not take an input channel"
    );
    h.press("assign port start");
    h.wait(0.3);
    let want = with(
        "in  rio_di_04 bool rio ch:DI-04 poll_ms=20  # the E-stop contact\n",
        "in  start bool rio ch:DI-04 poll_ms=20  # the E-stop contact\n",
    )
    .replace("in  start bool box di:0  # the button\n", "");
    assert_eq!(map(&h), want, "options and comment kept, the old line gone");
    assert!(dirty(&h));
    // The row is `start` now, and feeds a port: it cannot take another.
    h.right_click(h.rect("point start").center());
    assert!(h.has("menu assign channel") && h.has("menu back channel"));
    assert!(says(&mut h, "menu assign channel", "already feeds start"));
    h.press("menu back channel");
    h.wait(0.3);
    let back = map(&h);
    assert!(
        !back.contains("ch:DI-04"),
        "the line goes, as Remove takes it: {back}"
    );
    assert!(
        back.contains("in  start bool box di:0"),
        "the next free box channel: {back}"
    );
}

#[test]
fn a_channel_with_no_port_waiting_says_what_to_do() {
    // Every int input is assigned already: the analogue channel has nobody.
    let mut h = project(&MAP.replace(
        "in  rio_cnt   int  rio ch:DI-00-count unit=mm\n",
        "in  level int rio ch:DI-00-count unit=mm\nin  rio_ai_1 int rio ch:DI-01-count\n",
    ));
    h.press("pane devices");
    h.wait(0.3);
    h.right_click(h.rect("point rio_ai_1").center());
    assert!(says(
        &mut h,
        "menu assign channel",
        "No program input of type int is waiting for a device."
    ));
}

/// The screen centre of the block reading or writing `name`.
fn block(h: &Harness, name: &str) -> eframe::egui::Pos2 {
    let v = sim_view(h);
    let n = v
        .diagram()
        .nodes
        .iter()
        .find(|n| match &n.kind {
            NodeKind::Read { path } | NodeKind::Write { path } => path.eq_ignore_ascii_case(name),
            _ => false,
        })
        .unwrap_or_else(|| panic!("no block for {name}"));
    v.cams[v.scope].to_screen(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0)
}

#[test]
fn the_diagram_blocks_of_a_port_assign_it_with_the_code_menu_on() {
    let mut h = project(MAP);
    h.right_click(block(&h, "start"));
    assert!(h.has("diagram menu assign"), "an input's Read block");
    assert!(!h.has("diagram menu back to simulated"));
    h.press("diagram menu assign");
    h.wait(0.2);
    assert!(h.has("assign form"));
    assert!(!h.has("assign channel rio ch:DO-00"));
    h.press("assign channel rio ch:DI-04");
    h.wait(0.3);
    assert!(map(&h).contains("in  start bool rio ch:DI-04 poll_ms=20  # the E-stop contact"));
    assert!(!map(&h).contains("box di:0"));
    assert!(dirty(&h));
    // Its output block, and the Write block's own menu.
    h.right_click(block(&h, "motor"));
    h.press("diagram menu assign");
    h.wait(0.2);
    h.press("assign channel rio ch:DO-00");
    h.wait(0.3);
    assert!(map(&h).contains("out motor bool rio ch:DO-00 safe=true"));
    // Back, from the block: the menu offers it now.
    h.right_click(block(&h, "start"));
    assert!(h.has("diagram menu back to simulated"));
    h.press("diagram menu back to simulated");
    h.wait(0.3);
    assert!(!map(&h).contains("ch:DI-04"), "{}", map(&h));
    crate::assign::testing::loads_with(&map(&h), PROFILE);
}

#[test]
fn the_diagram_menu_assigns_while_the_code_is_ahead_of_the_diagram_and_offers_nothing_on_other_blocks() {
    let mut h = project(MAP);
    // The code moves on; the diagram keeps what was deployed.
    h.app
        .sim
        .as_mut()
        .unwrap()
        .proj
        .files
        .get_mut("main.st")
        .unwrap()
        .push_str("\n(* a note not deployed yet *)\n");
    h.wait(0.3);
    h.right_click(block(&h, "start"));
    assert!(h.has("diagram menu assign"), "the io.map item is there anyway");
    h.press("diagram menu assign");
    h.wait(0.2);
    h.press("assign channel rio ch:DI-05");
    h.wait(0.3);
    assert!(map(&h).contains("in  start bool rio ch:DI-05"));
    // Any other block offers nothing: no menu opens.
    let and = {
        let v = sim_view(&h);
        let n = v
            .diagram()
            .nodes
            .iter()
            .find(|n| matches!(&n.kind, NodeKind::Op { op } if op == "AND"))
            .unwrap();
        v.cams[v.scope].to_screen(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0)
    };
    h.right_click(and);
    assert!(!h.has("diagram menu assign") && !h.has("diagram menu chat"));
    assert!(!h.app.sim.as_ref().unwrap().dmenu.is_open(), "no menu opened");
}

#[test]
fn the_io_rows_and_the_devices_rows_stay_inside_the_panels_default_width() {
    let mut h = project(MAP);
    let tab = h.rect("tab io");
    let edge = h.size.x - crate::editor::RIGHT_W;
    let inside = |h: &Harness, key: &str| {
        let r = h.rect(key);
        assert!(
            r.left() >= edge - 8.0 && r.right() <= h.size.x,
            "{key} {r:?} stays in the pane's {} pt default width",
            crate::editor::RIGHT_W
        );
    };
    inside(&h, "io row spare");
    inside(&h, "io row motor");
    // Assigned, the rows are the same width: the place is in a tooltip.
    menu(&mut h, "io row spare", "menu assign port");
    let form = h.rect("assign form");
    assert!(
        form.right() <= h.size.x && form.left() >= 0.0,
        "{form:?} on the screen"
    );
    h.press("assign channel rio ch:DI-05");
    h.wait(0.3);
    menu(&mut h, "io row motor", "menu assign port");
    h.press("assign channel rio ch:DO-00");
    h.wait(0.3);
    inside(&h, "io row spare");
    inside(&h, "io row motor");
    assert!(
        (h.rect("tab io").left() - tab.left()).abs() <= 1.0,
        "the pane grew"
    );
    // The Cancel of the question changes nothing.
    h.right_click(on_card(&h, "io row spare"));
    h.press("menu back to simulated");
    h.wait(0.3);
    let before = map(&h);
    menu(&mut h, "io row spare", "menu assign port");
    assert!(h.has("assign form"));
    h.press("assign cancel");
    h.wait(0.2);
    assert!(!h.has("assign form"));
    assert_eq!(map(&h), before);
    // And the Devices tab's rows and menu, in the other pane.
    h.press("pane devices");
    h.wait(0.3);
    let left = h.rect("pane devices");
    h.right_click(h.rect("point rio_di_04").center());
    assert!(h.has("menu assign channel"));
    let item = h.rect("menu assign channel");
    assert!(item.right() <= h.size.x, "{item:?}");
    h.press("menu assign channel");
    h.wait(0.2);
    h.press("assign cancel");
    h.wait(0.2);
    assert!(
        (h.rect("pane devices").left() - left.left()).abs() <= 1.0,
        "the pane moved"
    );
}

#[test]
fn a_channel_with_no_line_yet_is_offered_and_gets_one_named_for_the_port() {
    // The map has the device and four of its signals; DI-06 and DO-03 have none.
    let mut h = project(MAP);
    menu(&mut h, "io row spare", "menu assign port");
    assert!(
        h.has("assign channel rio ch:DI-06"),
        "a channel of the profile with no line"
    );
    assert!(h.has("assign channel rio ch:DI-04"), "and one that has");
    h.press("assign channel rio ch:DI-06");
    h.wait(0.3);
    let got = map(&h);
    let line = got.lines().find(|l| l.contains("ch:DI-06")).expect("a new line");
    assert!(
        line.starts_with("in") && line.contains(" spare bool rio ch:DI-06"),
        "{line}"
    );
    assert!(
        !got.contains("spare bool box"),
        "the simulated line is gone: {got}"
    );
    let at = |needle: &str| got.find(needle).unwrap();
    assert!(
        at("ch:DI-06") > at("ch:DO-00") && at("ch:DI-06") < at("in  start"),
        "after the device's other lines: {got}"
    );
    crate::assign::testing::loads_with(&got, PROFILE);
    assert!(dirty(&h));
    assert!(says(&mut h, "io row spare", "On rio, DI-06"));
    // An output, from its row: the line carries the profile's safe value.
    menu(&mut h, "io row motor", "menu assign port");
    h.press("assign channel rio ch:DO-03");
    h.wait(0.3);
    let got = map(&h);
    let line = got.lines().find(|l| l.contains("ch:DO-03")).expect("a new line");
    assert!(
        line.starts_with("out") && line.contains(" motor bool rio ch:DO-03") && line.contains("safe=false"),
        "{line}"
    );
    crate::assign::testing::loads_with(&got, PROFILE);
    // Back: the line is gone and the channel is the picker's again.
    h.right_click(on_card(&h, "io row motor"));
    h.press("menu back to simulated");
    h.wait(0.3);
    assert!(!map(&h).contains("ch:DO-03"), "{}", map(&h));
    assert!(map(&h).contains("out motor bool box do:0 safe=false"));
    crate::assign::testing::loads_with(&map(&h), PROFILE);
}

#[test]
fn a_device_added_alone_is_offered_to_the_diagram_and_the_io_tab_and_not_called_missing() {
    // Only the device's line: nothing on it yet.
    let mut h = project(
        "device rio device:moxa-iologik-e1213 cip=991:515:0x00e12130\nin  start bool box di:0\nin  spare bool box di:1\nout motor bool box do:0 safe=false\n",
    );
    h.right_click(block(&h, "start"));
    assert!(h.has("diagram menu assign"));
    h.press("diagram menu assign");
    h.wait(0.2);
    assert!(h.has("assign channel rio ch:DI-00") && h.has("assign channel rio ch:DI-11"));
    h.press("assign channel rio ch:DI-02");
    h.wait(0.3);
    let got = map(&h);
    assert!(got.contains(" start bool rio ch:DI-02"), "{got}");
    assert!(!got.contains("start bool box"), "{got}");
    crate::assign::testing::loads_with(&got, PROFILE);
    // And from the I/O tab's row.
    menu(&mut h, "io row spare", "menu assign port");
    assert!(!h.has("assign channel rio ch:DI-02"), "taken now");
    h.press("assign channel rio ch:DI-03");
    h.wait(0.3);
    let got = map(&h);
    assert!(got.contains(" spare bool rio ch:DI-03"), "{got}");
    crate::assign::testing::loads_with(&got, PROFILE);
}

// ---- simulated or physical, per signal (Nick, 2026-11-24) ----

/// `project(map)` with a plant.sim.
fn with_plant(map: &str, plant: &str) -> Harness {
    let mut h = project(map);
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.add_file(helmsman_project::PLANT, plant.into());
        v.proj.save();
    }
    h.wait(0.3);
    h
}

fn plant(h: &Harness) -> String {
    h.app.sim.as_ref().unwrap().proj.files[helmsman_project::PLANT].clone()
}

#[test]
fn a_simulated_card_has_the_badge_and_a_dashed_outline_and_a_physical_one_is_solid() {
    let mut h = project(MAP);
    for name in ["start", "stop", "spare", "motor", "speed"] {
        assert!(h.has(&format!("io sim {name}")), "{name} says SIM");
        assert!(h.has(&format!("io dashed {name}")), "{name} is dashed");
        assert!(!h.has(&format!("io solid {name}")), "{name} is not solid");
    }
    assert!(h.drew("SIM"));
    menu(&mut h, "io row spare", "menu assign port");
    h.press("assign channel rio ch:DI-05");
    h.wait(0.3);
    assert!(
        !h.has("io sim spare") && !h.has("io dashed spare"),
        "no badge once physical"
    );
    assert!(h.has("io solid spare"), "solid");
    assert!(h.has("io sim stop"), "the others stay simulated");
    // And back: the badge returns.
    h.right_click(on_card(&h, "io row spare"));
    h.press("menu back to simulated");
    h.wait(0.3);
    assert!(h.has("io sim spare") && h.has("io dashed spare") && !h.has("io solid spare"));
}

#[test]
fn a_tile_names_its_device_and_channel_at_the_panels_default_width() {
    let mut h = project(MAP);
    h.app.chrome.io_grid = true;
    h.wait(0.3);
    menu(&mut h, "io row spare", "menu assign port");
    h.press("assign channel rio ch:DI-05");
    h.wait(0.3);
    assert!(h.has("io phys spare"), "the tile names where it is");
    assert!(h.drew("rio DI-05"));
    assert!(h.has("io sim stop"), "a simulated tile says SIM");
    let edge = h.size.x - crate::editor::RIGHT_W;
    for key in ["io card spare", "io card stop", "io card motor"] {
        let r = h.rect(key);
        assert!(
            r.left() >= edge - 8.0 && r.right() <= h.size.x,
            "{key} {r:?} stays in the pane's {} pt default width",
            crate::editor::RIGHT_W
        );
    }
    assert!(h.rect("io phys spare").right() <= h.rect("io card spare").right() + 0.5);
}

#[test]
fn the_plant_line_is_kept_and_switched_off_on_assign_and_back_on_with_the_way_back() {
    const PLANT: &str = "# the bench\nbutton b input=spare\n";
    let mut h = with_plant(MAP, PLANT);
    menu(&mut h, "io row spare", "menu assign port");
    h.press("assign channel rio ch:DI-05");
    h.wait(0.3);
    assert_eq!(plant(&h), format!("{PLANT}off spare\n"), "kept, with an off line");
    let (spec, problems) = helmsman_sim::parse(&plant(&h));
    assert!(problems.is_empty() && spec.is_off("spare") && spec.devices.len() == 1);
    h.right_click(on_card(&h, "io row spare"));
    h.press("menu back to simulated");
    h.wait(0.3);
    assert_eq!(plant(&h), PLANT, "exactly as it was");
    // A port no plant line names gains nothing.
    menu(&mut h, "io row stop", "menu assign port");
    h.press("assign channel rio ch:DI-04");
    h.wait(0.3);
    assert_eq!(plant(&h), PLANT);
}

#[test]
fn the_diagram_marks_a_simulated_block_and_a_physical_one_differently() {
    use crate::assign::Designation;
    let mut h = project(MAP);
    let marks = |h: &Harness| {
        let v = sim_view(h);
        let name_of = |id| match &v.shown().node(id).kind {
            NodeKind::Read { path } | NodeKind::Write { path } => path.to_ascii_lowercase(),
            _ => unreachable!(),
        };
        v.io_marks()
            .into_iter()
            .map(|(id, d)| (name_of(id), d))
            .collect::<Vec<_>>()
    };
    let m = marks(&h);
    assert!(m.len() >= 3, "{m:?}");
    assert!(m.iter().all(|(_, d)| d.is_simulated()), "{m:?}");
    h.right_click(block(&h, "start"));
    h.press("diagram menu assign");
    h.wait(0.2);
    h.press("assign channel rio ch:DI-04");
    h.wait(0.3);
    let m = marks(&h);
    let start = m.iter().find(|(n, _)| n == "start").expect("start");
    assert_eq!(
        start.1,
        Designation::Physical {
            device: "rio".into(),
            channel: "DI-04".into()
        }
    );
    assert!(m.iter().find(|(n, _)| n == "stop").unwrap().1.is_simulated());
}

#[test]
fn the_trend_legend_marks_a_simulated_signal() {
    let mut h = project(MAP);
    assert!(h.has("trend sim spare"), "spare is simulated");
    assert!(h.drew("spare SIM"));
    menu(&mut h, "io row spare", "menu assign port");
    h.press("assign channel rio ch:DI-05");
    h.wait(0.3);
    assert!(
        h.has("trend signal spare") && !h.has("trend sim spare"),
        "physical: no mark"
    );
    assert!(h.has("trend sim stop"));
}
