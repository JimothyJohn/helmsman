//! Small grey labels over diagram blocks (the I/O designation, `SIM`, and
//! the diff view's `was ..` / `removed ..` tags) must not be painted over a
//! group's title (`IF ..`, `ELSE`, `CASE ..`) nor over each other, at any
//! zoom, and must stay inside their block's width.

use eframe::egui::{self, Event, Modifiers, MouseWheelUnit, Pos2, Rect, Vec2};

use super::{Harness, sim_view};

const PROGRAM: &str = "\
PROGRAM line
VAR_INPUT
  start : BOOL;
  mode : INT;
  sensor : BOOL;
END_VAR
VAR_OUTPUT
  alpha : BOOL;
  beta : BOOL;
  gamma : BOOL;
  delta : BOOL;
  epsilon : BOOL;
  zeta : BOOL;
END_VAR
IF start THEN
  zeta := sensor;
  alpha := sensor;
  beta := start;
ELSE
  epsilon := sensor;
END_IF;
CASE mode OF
  1:
    gamma := TRUE;
    delta := TRUE;
  2:
    delta := FALSE;
END_CASE;
END_PROGRAM
";

/// A device's name long enough that its label is wider than a block.
const MAP: &str = "\
device a_rather_long_device_name device:moxa-iologik-e1213 cip=991:515:0x00e12130
in  start bool box di:0
out alpha bool a_rather_long_device_name ch:DO-00 safe=false
out beta bool a_rather_long_device_name ch:DO-01 safe=false
out gamma bool a_rather_long_device_name ch:DO-02 safe=false
out delta bool a_rather_long_device_name ch:DO-03 safe=false
out epsilon bool a_rather_long_device_name ch:DO-04 safe=false
out zeta bool a_rather_long_device_name ch:DO-05 safe=false
";

/// The baseline: the same program with other values, so the blocks that
/// differ carry a diff mark in the diagram.
const BEFORE: &str = "\
PROGRAM line
VAR_INPUT
  start : BOOL;
  mode : INT;
  sensor : BOOL;
END_VAR
VAR_OUTPUT
  alpha : BOOL;
  beta : BOOL;
  gamma : BOOL;
  delta : BOOL;
  epsilon : BOOL;
  zeta : BOOL;
END_VAR
IF start THEN
  zeta := start;
  alpha := start;
  beta := sensor;
  epsilon := TRUE;
ELSE
  epsilon := start;
END_IF;
CASE mode OF
  1:
    gamma := FALSE;
    delta := FALSE;
  2:
    delta := TRUE;
END_CASE;
END_PROGRAM
";

struct Seen {
    text: String,
    rect: Rect,
}

/// Every piece of text the last frame drew whose rect lies in `area`.
fn texts(h: &Harness, area: Rect) -> Vec<Seen> {
    fn walk(s: &egui::Shape, area: Rect, out: &mut Vec<Seen>) {
        match s {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            egui::Shape::Text(t) => {
                let r = t.galley.rect.translate(t.pos.to_vec2());
                if area.intersects(r) && !t.galley.text().trim().is_empty() {
                    out.push(Seen {
                        text: t.galley.text().to_string(),
                        rect: r,
                    });
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, area, &mut out);
    }
    out
}

fn is_title(t: &str) -> bool {
    let t = t.to_ascii_uppercase();
    t.starts_with("IF ") || t.starts_with("ELSE") || t.starts_with("CASE ")
}

fn is_label(t: &str) -> bool {
    t == "SIM"
        || t.starts_with("was ")
        || t.starts_with("removed ")
        || t.starts_with("a_rather")
        || t.contains(" · was")
        || t.contains(" · rem")
}

fn project() -> Harness {
    let mut h = Harness::code();
    h.size.y = 1500.0;
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("line", None);
        v.proj.files.insert("main.st".into(), BEFORE.into());
        v.proj.add_file(helmsman_project::IO_MAP, MAP.into());
        v.proj.add_file(
            "moxa-iologik-e1213.device",
            include_str!("../../../../docs/devices/moxa-iologik-e1213.device").into(),
        );
        v.proj.save();
        v.open_project("line");
        // The version after it: the first one is the baseline.
        v.proj.files.insert("main.st".into(), PROGRAM.into());
        v.proj.save();
        v.open_project("line");
        // The Agent's change from BEFORE to PROGRAM: marked on the diagram.
        let src = crate::sim_projects::SimProjects::build_src_of(&{
            let mut f = v.proj.files.clone();
            f.insert("main.st".into(), BEFORE.into());
            f
        });
        let baseline = crate::playground::Build::new(&src)
            .0
            .ok()
            .and_then(|b| b.module)
            .map(std::rc::Rc::new);
        assert!(baseline.is_some(), "the baseline builds");
        v.agent_change = Some(crate::agent_marks::AgentChange::new(
            &v.proj.open,
            "main.st",
            BEFORE,
            PROGRAM.to_string(),
            baseline,
        ));
    }
    h.wait(0.5);
    h
}

fn wheel(y: f32) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, y),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    }
}

fn check(h: &Harness, what: &str) -> usize {
    let area = h.app.chrome.diagram;
    let all = texts(h, area);
    let titles: Vec<&Seen> = all.iter().filter(|s| is_title(&s.text)).collect();
    let labels: Vec<&Seen> = all.iter().filter(|s| is_label(&s.text)).collect();
    let dump = || {
        all.iter()
            .map(|s| format!("{:?} {:?}", s.text, s.rect))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let up = |s: &&Seen| s.text.to_ascii_uppercase();
    assert!(
        what == "zoomed in" || titles.iter().any(|s| up(s).starts_with("IF ")),
        "{what}: the IF title drawn: {}",
        dump()
    );
    assert!(
        titles.iter().any(|s| up(s) == "ELSE"),
        "{what}: the ELSE title drawn: {}",
        dump()
    );
    for (i, l) in labels.iter().enumerate() {
        for t in &titles {
            assert!(
                !l.rect.intersects(t.rect),
                "{what}: label {:?} {:?} overprints the title {:?} {:?}",
                l.text,
                l.rect,
                t.text,
                t.rect
            );
        }
        for m in labels.iter().skip(i + 1) {
            assert!(
                !l.rect.intersects(m.rect),
                "{what}: label {:?} {:?} overprints label {:?} {:?}",
                l.text,
                l.rect,
                m.text,
                m.rect
            );
        }
    }
    labels.len()
}

#[test]
fn labels_over_blocks_never_overprint_group_titles_or_each_other() {
    let mut h = project();
    let _ = sim_view(&h);
    let area = h.app.chrome.diagram;
    let centre = Pos2::new(area.center().x, area.center().y);
    let mut zooms = Vec::new();
    for (name, events) in [
        ("default", vec![]),
        ("zoomed in", vec![Event::PointerMoved(centre), wheel(60.0)]),
        ("zoomed out", vec![wheel(-200.0)]),
    ] {
        h.frame(events);
        h.wait(0.4);
        zooms.push(sim_view(&h).cams[0].zoom);
        let n = check(&h, name);
        let all = texts(&h, h.app.chrome.diagram);
        let have = |p: &dyn Fn(&str) -> bool| all.iter().any(|s| p(&s.text));
        assert!(have(&|t| t == "SIM"), "{name}: a SIM label is drawn");
        assert!(
            have(&|t| {
                t.starts_with("was ")
                    || t.starts_with("removed ")
                    || t.contains(" · was")
                    || t.contains(" · rem")
            }),
            "{name}: a diff tag is drawn"
        );
        assert!(
            have(&|t| t.starts_with("a_rather")),
            "{name}: a device label is drawn: {:?}",
            all.iter().map(|s| s.text.clone()).collect::<Vec<_>>()
        );
        assert!(
            n >= if name == "zoomed in" { 3 } else { 8 },
            "{name}: only {n} labels found"
        );
        // Hostile: a label wider than its block stays inside it.
        let v = sim_view(&h);
        for var in ["ALPHA", "BETA", "EPSILON", "ZETA"] {
            let n = v
                .diagram()
                .nodes
                .iter()
                .find(|n| super::write_to(var)(&n.kind))
                .expect("the write");
            let block = v.block_on_screen(n.id).expect("drawn");
            let l = texts(&h, h.app.chrome.diagram)
                .into_iter()
                .find(|s| s.text.starts_with("a_rather") && (s.rect.left() - block.left()).abs() < 2.0)
                .unwrap_or_else(|| panic!("{name}: the label of {var}"));
            assert!(
                l.rect.right() <= block.right() + 1.0,
                "{name}: label {:?} {:?} runs past its block {block:?}",
                l.text,
                l.rect
            );
        }
    }
    assert!(
        zooms[1] > zooms[0] && zooms[2] < zooms[0],
        "three different zooms: {zooms:?}"
    );
}
