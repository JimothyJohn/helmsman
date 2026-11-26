//! The diagram's , and . keys: the view jumps to the previous or next
//! part of the program that carries the logic, framed with some room
//! around it (Nick, 2026-11-15; E and Q until 2026-11-26, when E became
//! the Editor's key).
//!
//! A stop is a function block call with the blocks that feed it (a timer,
//! a counter, an instance of a block of the project's own) or a top-level
//! IF, CASE or loop with everything in it. Plain assignments are not
//! stops. They come in program order, which the layout draws left to
//! right in rows.
//!
//! Where a key goes: from the stop the last jump framed, to its neighbour
//! in that order, round the ends. Once the view has been moved by hand,
//! to the nearest stop right (.) or left (,) of the view's centre, and
//! round to the first or last when there is none on that side.

use helmsman_diagram::{Diagram, NodeKind, Rect};

use crate::st_view::Pick;

/// One place , and . go to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    /// What the jump picks: the function block's block, or the first arm
    /// of the IF, CASE or loop.
    pub pick: Pick,
    /// What the jump frames, in diagram units.
    pub rect: Rect,
}

/// The box around `rects`; `None` for none.
fn bounds(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects.fold(None, |all, r| {
        let Some(a) = all else {
            return Some(r);
        };
        let (x, y) = (a.x.min(r.x), a.y.min(r.y));
        Some(Rect {
            x,
            y,
            w: a.right().max(r.right()) - x,
            h: a.bottom().max(r.bottom()) - y,
        })
    })
}

/// The stops of `d`, in program order.
#[must_use]
pub fn stops(d: &Diagram) -> Vec<Stop> {
    let mut out = Vec::new();
    for s in &d.stmts {
        // An IF, CASE or loop at the top of the body: its arms and the
        // conditions in front of them. One nested in another is inside
        // the outer one's frame already.
        if s.region.is_none()
            && let Some(&first) = s.arms.first()
        {
            let arms = s.arms.iter().map(|&r| d.region(r).rect);
            let heads = s
                .roots
                .iter()
                .map(|&n| d.node(n))
                .filter(|n| n.stmt == s.id)
                .map(|n| n.rect);
            if let Some(rect) = bounds(arms.chain(heads)) {
                out.push(Stop {
                    pick: Pick::Region(first),
                    rect,
                });
            }
        }
        // A function block call, wherever it is, with its arguments.
        for &n in &s.roots {
            // A condition that reads a block's output (`IF t.Q`) has the
            // earlier call's block as its root: that is not a call here.
            if d.node(n).stmt != s.id || !matches!(d.node(n).kind, NodeKind::Fb { .. }) {
                continue;
            }
            let call = d.nodes.iter().filter(|m| m.stmt == s.id).map(|m| m.rect);
            out.push(Stop {
                pick: Pick::Node(n),
                rect: bounds(call).unwrap_or(d.node(n).rect),
            });
        }
    }
    out
}

/// Which of `stops` a key goes to: `at`'s neighbour when the view still
/// frames stop `at`, else the nearest one on the key's side of `centre`
/// (diagram units). Round the ends either way; `None` with no stops.
#[must_use]
pub fn next(stops: &[Stop], at: Option<usize>, centre: (f32, f32), forward: bool) -> Option<usize> {
    let n = stops.len();
    if n == 0 {
        return None;
    }
    if let Some(at) = at.filter(|&k| k < n) {
        return Some(if forward { (at + 1) % n } else { (at + n - 1) % n });
    }
    let mid = |r: &Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let ahead = stops
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let x = mid(&s.rect).0;
            if forward {
                x > centre.0 + 1.0
            } else {
                x < centre.0 - 1.0
            }
        })
        .min_by(|(_, a), (_, b)| {
            let far = |s: &Stop| {
                let (x, y) = mid(&s.rect);
                (x - centre.0).hypot(y - centre.1)
            };
            far(a).total_cmp(&far(b))
        })
        .map(|(k, _)| k);
    Some(ahead.unwrap_or(if forward { 0 } else { n - 1 }))
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
    use helmsman_diagram::NodeKind;

    use super::{Stop, next, stops};
    use crate::playground::Session;
    use crate::st_view::{FOCUS_MAX, GLYPH_BAND, Pick, Viewer};

    /// Two function block calls and an IF among plain assignments.
    const SRC: &str = "PROGRAM p
VAR
  start AT %IX0.0 : BOOL;
  lamp AT %QX0.0 : BOOL;
  horn AT %QX0.1 : BOOL;
  t : TON;
  c : CTU;
  n : INT;
END_VAR
lamp := start;
t(IN := start, PT := T#1s);
horn := start AND NOT lamp;
IF t.Q THEN
  n := n + 1;
ELSE
  n := 0;
END_IF;
lamp := t.Q;
c(CU := start, PV := 3);
horn := c.Q;
END_PROGRAM
";

    fn viewer() -> Viewer {
        let session = Session::new(SRC);
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        Viewer::new(SRC, module)
    }

    /// What each stop is: `t`, `c`, or `IF` for a region.
    fn names(v: &Viewer, stops: &[Stop]) -> Vec<String> {
        stops
            .iter()
            .map(|s| match s.pick {
                Pick::Node(n) => match &v.diagram().node(n).kind {
                    NodeKind::Fb { instance, .. } => instance.to_lowercase(),
                    k => format!("{k:?}"),
                },
                Pick::Region(_) => "IF".into(),
            })
            .collect()
    }

    #[test]
    fn the_stops_are_the_block_calls_and_the_top_level_ifs_in_program_order() {
        let v = viewer();
        let d = v.diagram();
        let all = stops(d);
        assert_eq!(names(&v, &all), ["t", "IF", "c"]);
        // A call is framed with the blocks that feed it; an IF with both
        // arms and its condition.
        for s in &all {
            match s.pick {
                Pick::Node(n) => {
                    let stmt = d.node(n).stmt;
                    let fed = d.nodes.iter().filter(|m| m.stmt == stmt).count();
                    assert!(fed > 1, "the call has arguments");
                    for m in d.nodes.iter().filter(|m| m.stmt == stmt) {
                        assert!(inside(s.rect, m.rect), "{} is in the call's frame", m.label);
                    }
                }
                Pick::Region(_) => {
                    assert_eq!(d.regions.len(), 2, "IF and ELSE");
                    for r in &d.regions {
                        assert!(inside(s.rect, r.rect), "{} is in the IF's frame", r.label);
                    }
                }
            }
        }
        // No stops in a program of plain assignments.
        let plain = "PROGRAM q\nVAR a, b : BOOL; END_VAR\nb := a;\nEND_PROGRAM\n";
        let session = Session::new(plain);
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        assert_eq!(stops(Viewer::new(plain, module).diagram()), []);
    }

    fn inside(outer: helmsman_diagram::Rect, r: helmsman_diagram::Rect) -> bool {
        r.x >= outer.x && r.y >= outer.y && r.right() <= outer.right() && r.bottom() <= outer.bottom()
    }

    #[test]
    fn a_key_steps_from_the_framed_stop_and_otherwise_takes_the_side_of_the_centre() {
        let at = |x: f32, y: f32| Stop {
            pick: Pick::Node(helmsman_diagram::NodeId(0)),
            rect: helmsman_diagram::Rect {
                x: x - 10.0,
                y: y - 10.0,
                w: 20.0,
                h: 20.0,
            },
        };
        // Two rows: 0 and 1 on the first, 2 and 3 on the second.
        let all = [
            at(100.0, 50.0),
            at(600.0, 50.0),
            at(150.0, 400.0),
            at(500.0, 400.0),
        ];
        assert_eq!(next(&[], None, (0.0, 0.0), true), None);
        // From a framed stop: its neighbours in order, round the ends.
        assert_eq!(next(&all, Some(0), (100.0, 50.0), true), Some(1));
        assert_eq!(
            next(&all, Some(1), (600.0, 50.0), true),
            Some(2),
            "on to the next row"
        );
        assert_eq!(next(&all, Some(3), (500.0, 400.0), true), Some(0));
        assert_eq!(next(&all, Some(0), (100.0, 50.0), false), Some(3));
        assert_eq!(next(&all, Some(2), (150.0, 400.0), false), Some(1));
        // A stop that is gone (the program changed) is no place to step from.
        assert_eq!(next(&all, Some(9), (300.0, 60.0), true), Some(1));
        // From a view moved by hand: the nearest on the key's side.
        assert_eq!(next(&all, None, (300.0, 60.0), true), Some(1));
        assert_eq!(next(&all, None, (300.0, 60.0), false), Some(0));
        assert_eq!(next(&all, None, (300.0, 390.0), true), Some(3));
        assert_eq!(next(&all, None, (300.0, 390.0), false), Some(2));
        // The stop under the centre is not on either side of it.
        assert_eq!(next(&all, None, (500.0, 400.0), true), Some(1));
        // Nothing on that side: round to the first, or the last.
        assert_eq!(next(&all, None, (900.0, 50.0), true), Some(0));
        assert_eq!(next(&all, None, (10.0, 50.0), false), Some(3));
    }

    const SIZE: Vec2 = Vec2::new(1200.0, 800.0);

    /// The viewer on a bare page, with a text field above it to type in.
    struct Harness {
        ctx: egui::Context,
        view: Viewer,
        time: f64,
        typing: bool,
        text: String,
    }

    impl Harness {
        fn new() -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                view: viewer(),
                time: 0.0,
                typing: false,
                text: String::new(),
            };
            h.frame(vec![Event::PointerMoved(Pos2::new(600.0, 500.0))]);
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            self.time += 1.0 / 60.0;
            let Self {
                view, typing, text, ..
            } = self;
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::Panel::top("field").show(ui, |ui| {
                    let field = ui.text_edit_singleline(text);
                    if *typing {
                        field.request_focus();
                    } else {
                        field.surrender_focus();
                    }
                });
                egui::CentralPanel::no_frame().show(ui, |ui| view.canvas(ui, None));
            });
            out.textures_delta.clear();
        }

        /// `key` pressed and let go, then frames until the view rests.
        fn press(&mut self, modifiers: Modifiers, key: Key) {
            let ev = |pressed| Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            };
            self.frame(vec![ev(true)]);
            self.frame(vec![ev(false)]);
            let end = self.time + 0.5;
            while self.time < end {
                self.frame(vec![]);
            }
        }

        /// The stop the view frames: the one picked.
        fn framed(&self) -> String {
            let all = self.view.stops();
            let k = all
                .iter()
                .position(|s| Some(s.pick) == self.view.pick)
                .expect("a stop is picked");
            let cam = self.view.cams[self.view.scope];
            let r = all[k].rect;
            let on_screen = Rect::from_min_max(cam.to_screen(r.x, r.y), cam.to_screen(r.right(), r.bottom()));
            let view = Rect::from_min_max(Pos2::new(0.0, GLYPH_BAND), SIZE.to_pos2());
            assert!(
                view.shrink(8.0).contains_rect(on_screen),
                "{on_screen:?} has room around it in {view:?}"
            );
            assert!(
                (on_screen.center().x - view.center().x).abs() < 2.0,
                "{on_screen:?} sits in the middle of {view:?}"
            );
            assert!(cam.zoom <= FOCUS_MAX, "zoom {}", cam.zoom);
            // Cropped to it: one side is as large as the padding lets it
            // be, unless the zoom stopped at its largest first.
            let tight = on_screen.width() > view.width() * 0.6 || on_screen.height() > view.height() * 0.6;
            assert!(
                tight || (cam.zoom - FOCUS_MAX).abs() < 1e-3,
                "{on_screen:?} at zoom {}",
                cam.zoom
            );
            names(&self.view, &all)[k].clone()
        }
    }

    #[test]
    fn period_and_comma_frame_the_next_and_the_previous_stop() {
        let mut h = Harness::new();
        assert_eq!(h.view.pick, None);
        // The fitted program is the whole view: . takes the nearest stop
        // right of its centre, then goes on in program order and round.
        h.press(Modifiers::NONE, Key::Period);
        let first = h.framed();
        let order = ["t", "IF", "c"];
        let k = order.iter().position(|n| *n == first).unwrap();
        h.press(Modifiers::NONE, Key::Period);
        assert_eq!(h.framed(), order[(k + 1) % 3]);
        h.press(Modifiers::NONE, Key::Period);
        assert_eq!(h.framed(), order[(k + 2) % 3]);
        h.press(Modifiers::NONE, Key::Period);
        assert_eq!(h.framed(), first, "round the end");
        h.press(Modifiers::NONE, Key::Comma);
        assert_eq!(h.framed(), order[(k + 2) % 3], ", goes back");
        // Two presses before the view rests still go two stops on.
        let ev = |pressed| Event::Key {
            key: Key::Comma,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        h.frame(vec![ev(true)]);
        h.frame(vec![ev(false)]);
        h.press(Modifiers::NONE, Key::Comma);
        assert_eq!(h.framed(), first);
    }

    #[test]
    fn a_view_moved_by_hand_jumps_by_where_it_is() {
        let mut h = Harness::new();
        h.press(Modifiers::NONE, Key::Period);
        h.press(Modifiers::NONE, Key::Period);
        // Pan until every stop is left of the centre: , takes the nearest
        // one there, . has none on its side and goes round to the first.
        let rightmost = |h: &Harness| {
            let cam = h.view.cams[h.view.scope];
            h.view
                .stops()
                .iter()
                .map(|s| cam.to_screen(s.rect.x + s.rect.w / 2.0, 0.0).x)
                .fold(f32::MIN, f32::max)
        };
        let held = |pressed| Event::Key {
            key: Key::ArrowRight,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        h.frame(vec![held(true)]);
        for _ in 0..600 {
            if rightmost(&h) < SIZE.x / 2.0 - 20.0 {
                break;
            }
            h.frame(vec![]);
        }
        h.frame(vec![held(false)]);
        assert!(rightmost(&h) < SIZE.x / 2.0, "every stop is left of the centre");
        let cam = h.view.cams[h.view.scope];
        let centre = cam.to_world(Pos2::new(SIZE.x / 2.0, f32::midpoint(SIZE.y, GLYPH_BAND)));
        let all = h.view.stops();
        let nearest = next(&all, None, centre, false).unwrap();
        h.press(Modifiers::NONE, Key::Comma);
        assert_eq!(h.view.pick, Some(all[nearest].pick));
        h.framed();
    }

    #[test]
    fn the_keys_are_the_diagrams_only_when_nothing_is_typed_in_and_no_modifier_is_held() {
        let mut h = Harness::new();
        let before = h.view.cams[0];
        for m in [Modifiers::COMMAND, Modifiers::ALT, Modifiers::SHIFT] {
            h.press(m, Key::Period);
            h.press(m, Key::Comma);
        }
        h.typing = true;
        h.frame(vec![]);
        h.press(Modifiers::NONE, Key::Period);
        h.press(Modifiers::NONE, Key::Comma);
        let after = h.view.cams[0];
        assert_eq!(h.view.pick, None);
        assert_eq!((after.offset, after.zoom), (before.offset, before.zoom));
        // The pointer off the diagram (over the field's panel): not its keys.
        h.typing = false;
        h.frame(vec![Event::PointerMoved(Pos2::new(600.0, 4.0))]);
        h.press(Modifiers::NONE, Key::Period);
        assert_eq!(h.view.pick, None);
        h.frame(vec![Event::PointerMoved(Pos2::new(600.0, 500.0))]);
        h.press(Modifiers::NONE, Key::Period);
        assert!(h.view.pick.is_some());
    }

    /// Nick, 2026-11-26: W A S D no longer move the view (the arrow keys
    /// still do), and E and Q no longer jump.
    #[test]
    fn w_a_s_d_do_not_pan_and_e_and_q_do_not_jump_but_the_arrows_pan() {
        let mut h = Harness::new();
        let before = h.view.cams[0];
        for key in [Key::W, Key::A, Key::S, Key::D, Key::E, Key::Q] {
            let ev = |pressed| Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            };
            h.frame(vec![ev(true)]);
            for _ in 0..12 {
                h.frame(vec![]);
            }
            h.frame(vec![ev(false)]);
            h.press(Modifiers::NONE, key);
            let after = h.view.cams[0];
            assert_eq!(
                (after.offset, after.zoom),
                (before.offset, before.zoom),
                "{key:?} moved the view"
            );
            assert_eq!(h.view.pick, None, "{key:?} jumped");
        }
        let ev = |pressed| Event::Key {
            key: Key::ArrowRight,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        h.frame(vec![ev(true)]);
        for _ in 0..12 {
            h.frame(vec![]);
        }
        h.frame(vec![ev(false)]);
        assert_ne!(h.view.cams[0].offset, before.offset, "the right arrow pans");
    }
}
