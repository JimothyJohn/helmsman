//! The zoom ladder on the canvas: the cards follow the zoom alone, sit
//! where their blocks are, and nothing they do reaches the layout.

use eframe::egui::{self, Event, PointerButton, Pos2, Rect, Vec2};
use helmsman_diagram::fold::ladder::{HINT, Rung};

use super::super::{Cam, Scan, Viewer};
use super::Card;
use crate::playground::Session;

const SRC: &str = "PROGRAM plant
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  other AT %IX0.2 : BOOL;
  motor AT %QX0.0 : BOOL;
  lamp AT %QX0.1 : BOOL;
  count : INT;
  level : REAL;
  level_pct : REAL;
END_VAR
(* Motor *)
motor := (start OR motor) AND NOT stop;
IF other THEN
  count := count + 1;
  lamp := TRUE;
ELSE
  lamp := FALSE;
  count := 0;
END_IF;
(* Level *)
level_pct := LIMIT(0.0, level / 2.0 * 100.0, 100.0);
count := count + 1;
END_PROGRAM
";

struct Harness {
    ctx: egui::Context,
    session: Session,
    view: Viewer,
    time: f64,
    size: Vec2,
}

impl Harness {
    fn new(src: &str) -> Self {
        let mut session = Session::new(src);
        assert!(session.diags.is_empty(), "{:?}", session.diag_lines());
        session.step();
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        let mut h = Self {
            ctx: egui::Context::default(),
            session,
            view: Viewer::new(src, module),
            time: 0.0,
            size: Vec2::new(1200.0, 800.0),
        };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        let view = &mut self.view;
        let mut out = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::no_frame().show(ui, |ui| view.canvas(ui, scan));
        });
        out.textures_delta.clear();
    }

    fn cam(&self) -> Cam {
        self.view.cams[self.view.scope]
    }

    fn cards(&self) -> Vec<Card> {
        self.view.ladder_cards(self.cam())
    }

    /// Zooms the way the wheel does, about `at`, to `to`.
    fn zoom_to(&mut self, to: f32, at: Pos2) {
        for _ in 0..400 {
            let z = self.cam().zoom;
            if (z - to).abs() < 1e-3 * to {
                break;
            }
            let step = (to / z).clamp(0.9, 1.1);
            self.frame(vec![Event::PointerMoved(at), Event::Zoom(step)]);
        }
        self.frame(vec![Event::PointerMoved(at)]);
    }

    fn centre() -> Pos2 {
        Pos2::new(600.0, 400.0)
    }

    fn click(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        self.frame(vec![]);
    }

    /// Runs frames until a glide has ended.
    fn settle(&mut self) {
        for _ in 0..120 {
            self.frame(vec![]);
        }
    }

    fn rects(&self) -> Vec<helmsman_diagram::Rect> {
        self.view.shown().nodes.iter().map(|n| n.rect).collect()
    }
}

#[test]
fn a_fitted_view_is_the_wiring_with_its_statements_faintly_outlined() {
    let h = Harness::new(SRC);
    assert_eq!(h.view.rung(), Rung::Wiring);
    let cards = h.cards();
    assert_ne!(cards, []);
    for c in &cards {
        assert_eq!(c.rung, Rung::Statements, "only the next step out: {c:?}");
        assert!(!c.solid() && c.formed <= 0.0 && c.code <= 0.0, "{c:?}");
        assert!(c.outline > 0.0 && c.outline <= HINT + 0.2, "faint: {c:?}");
    }
    assert!(cards.iter().any(|c| c.title == "level_pct"));
}

#[test]
fn zooming_out_closes_statements_then_sections_then_the_program_where_they_are() {
    let mut h = Harness::new(SRC);
    let before = h.rects();
    let size = h.view.shown().size;
    let mut seen = vec![h.view.rung()];
    for (zoom, rung, titles) in [
        (
            Rung::Statements.rest(),
            Rung::Statements,
            vec!["motor", "level_pct"],
        ),
        (Rung::Sections.rest(), Rung::Sections, vec!["Motor", "Level"]),
        (Rung::Program.rest(), Rung::Program, vec!["plant"]),
    ] {
        h.zoom_to(zoom, Harness::centre());
        assert_eq!(h.view.rung(), rung, "at {zoom}");
        seen.push(rung);
        let solid: Vec<Card> = h.cards().into_iter().filter(Card::solid).collect();
        assert!(solid.iter().all(|c| c.rung == rung), "{solid:?}");
        for t in titles {
            assert!(solid.iter().any(|c| c.title == t), "{t} at {rung:?}: {solid:?}");
        }
        // Each card is over its own blocks, as they were laid out.
        let cam = h.cam();
        for c in &solid {
            assert!(c.rect.width() > 0.0 && c.rect.height() > 0.0);
            assert_eq!(c.rect, cam.rect(&c.world));
        }
        assert_eq!(h.rects(), before, "nothing is laid out again at {rung:?}");
        assert_eq!(h.view.shown().size, size);
    }
    assert_eq!(
        seen,
        [Rung::Wiring, Rung::Statements, Rung::Sections, Rung::Program]
    );
    // And back in: the same blocks, in the same places, and no edit.
    h.zoom_to(1.0, Harness::centre());
    assert_eq!(h.view.rung(), Rung::Wiring);
    assert_eq!(h.rects(), before);
    assert_eq!(h.view.edits(), 0);
    assert!(!h.view.rearranged());
    assert!(h.view.arrangement().is_empty());
}

#[test]
fn a_card_holds_every_block_it_stands_for() {
    let mut h = Harness::new(SRC);
    h.zoom_to(Rung::Sections.rest(), Harness::centre());
    let d = h.view.shown();
    let cards: Vec<Card> = h.cards().into_iter().filter(Card::solid).collect();
    let program = h
        .view
        .ladder_cards(Cam {
            zoom: Rung::Program.rest(),
            ..h.cam()
        })
        .into_iter()
        .find(|c| c.rung == Rung::Program)
        .expect("the program's card");
    for n in &d.nodes {
        let inside = |w: &helmsman_diagram::Rect| {
            n.rect.x >= w.x && n.rect.y >= w.y && n.rect.right() <= w.right() && n.rect.bottom() <= w.bottom()
        };
        assert!(inside(&program.world), "{} outside the program", n.label);
        assert!(
            cards.iter().filter(|c| inside(&c.world)).count() >= 1,
            "{} in no section's card",
            n.label
        );
    }
}

#[test]
fn all_the_way_in_a_statement_is_its_text() {
    let mut h = Harness::new(SRC);
    h.zoom_to(Rung::Code.rest(), Harness::centre());
    assert_eq!(h.view.rung(), Rung::Code);
    let cards = h.cards();
    assert!(
        cards.iter().all(|c| c.rung == Rung::Code && c.solid()),
        "{cards:?}"
    );
    let pou = h.view.scopes[h.view.scope].pou;
    let code: Vec<&str> = h.view.ladder[pou]
        .at(Rung::Code)
        .iter()
        .map(|g| g.code.as_str())
        .collect();
    assert!(
        code.contains(&"level_pct := LIMIT(0.0, level / 2.0 * 100.0, 100.0);"),
        "{code:?}"
    );
    assert!(
        code.contains(&"motor := (start OR motor) AND NOT stop;"),
        "{code:?}"
    );
}

#[test]
fn a_click_on_a_card_flies_in_one_rung_and_never_reaches_the_blocks_under_it() {
    let mut h = Harness::new(SRC);
    h.zoom_to(Rung::Sections.rest(), Harness::centre());
    let card = h
        .cards()
        .into_iter()
        .find(|c| c.solid() && c.title == "Level")
        .expect("the Level section");
    h.click(card.rect.center());
    h.settle();
    assert_eq!(h.view.rung(), Rung::Statements, "one rung in, not to the wiring");
    assert!(
        h.view.selection().is_empty(),
        "the blocks under a card are not picked"
    );
    assert!(h.view.pick.is_none());
    // What the card held is in the middle of the view.
    let now = h.cam().rect(&card.world);
    let area = Rect::from_min_size(Pos2::ZERO, h.size);
    assert!(area.contains(now.center()), "{now:?}");
    // On the wiring a click is the blocks' again.
    h.zoom_to(1.0, now.center());
    let cam = h.cam();
    let block = h
        .view
        .shown()
        .nodes
        .iter()
        .find(|n| n.label.contains("LIMIT"))
        .or_else(|| h.view.shown().nodes.first())
        .unwrap()
        .rect;
    h.click(cam.rect(&block).center());
    assert_ne!(h.view.selection(), [], "a block is picked on the wiring");
}

#[test]
fn no_input_control_is_put_over_a_formed_card() {
    let mut h = Harness::new(SRC);
    let on_wiring = h.view.reads_on_screen();
    assert!(
        on_wiring.iter().any(|(_, r)| r.is_positive()),
        "the wiring has its controls"
    );
    for rung in [Rung::Statements, Rung::Sections, Rung::Program, Rung::Code] {
        h.zoom_to(rung.rest(), Harness::centre());
        let cards: Vec<Rect> = h
            .cards()
            .into_iter()
            .filter(Card::solid)
            .map(|c| c.rect)
            .collect();
        for (name, r) in h.view.reads_on_screen() {
            assert!(
                !r.is_positive() || !cards.iter().any(|c| c.contains(r.center())),
                "{name} has a control over a card at {rung:?}"
            );
        }
    }
}

#[test]
fn the_corner_names_the_rung() {
    let mut h = Harness::new(SRC);
    let area = Rect::from_min_size(Pos2::ZERO, h.size);
    assert!(area.contains_rect(h.view.ladder_name), "{:?}", h.view.ladder_name);
    assert!(h.view.ladder_name.height() >= 16.0);
    // A press on it is the control's: the diagram under it is not picked.
    h.zoom_to(Rung::Program.rest(), Harness::centre());
    let at = h.view.ladder_name.center();
    h.click(at);
    assert_eq!(h.view.selection(), []);
}
