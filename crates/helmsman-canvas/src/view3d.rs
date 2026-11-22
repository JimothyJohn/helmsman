//! The playground's diagram in 3D, behind a toggle (`canvas-3d-mvp`,
//! `canvas-3d-nav`): the scene of `scene3d`, an orbit camera (drag to
//! orbit, right-drag or Shift-drag to pan, the wheel and a pinch to zoom
//! toward the pointer), flights between views (a dive into a POU's
//! blocks, a climb back, a click on a block), picking by ray, and the
//! labels of `labels3d` drawn by egui over the GL pass. 2D stays the
//! default and is not touched: this view replaces the diagram's canvas
//! only while it is on.
//!
//! [`Nav`] is the part with no egui in it (focus, the camera, the
//! flight); the tests read it (`view3d_tests.rs`).

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::egui_glow::CallbackFn;
use helmsman_diagram::{Diagram, Live, Scan, annotate};

use crate::anim3d::{Glows, pulse_phase};
use crate::batch3d::{self, Focus};
use crate::cam3d::{Aabb, Camera, Limits, Orbit, OrbitFlight, PITCH0, Pose, YAW0, add, sub};
use crate::gl3d::{Frame, Shared};
use crate::labels3d::{self, Kind, Label, Opts, Tone};
use crate::scene3d::{self, Item, Scene, ScopeIn};
use crate::st_view::brand::{GREEN, RED, SELECT, YELLOW};
use crate::st_view::{Pick, SCROLL_ZOOM, Viewer};
use crate::theme::{Theme, theme};

/// Seconds of pulse travel a second: the wires' pulses run this many
/// wavelengths in one.
const PULSE_SPEED: f64 = 0.8;

/// Where the camera is, and how it gets elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct Nav {
    pub focus: Focus,
    orbit: Orbit,
    flight: Option<OrbitFlight>,
    /// The camera stands where its focus frames: a resize reframes it.
    framed: bool,
    /// What `framed` becomes when the flight lands.
    framed_after: bool,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            focus: Focus::Overview,
            orbit: Orbit::default(),
            flight: None,
            framed: true,
            framed_after: true,
        }
    }
}

impl Nav {
    /// The points of what `focus` shows, to be framed.
    #[must_use]
    pub fn points(&self, scene: &Scene) -> Vec<crate::cam3d::V3> {
        match self.focus {
            Focus::Overview => scene.frame_points(None),
            Focus::Dive(s) => scene.frame_points(Some(s)),
        }
    }

    /// The limits the pointer and the keys move the camera within.
    #[must_use]
    pub fn limits(scene: &Scene) -> Limits {
        Limits::of(&scene.bounds())
    }

    /// The camera at page time `now`: mid-flight, or where it stands (and
    /// reframed to the focus while it has not been moved).
    pub fn orbit(&mut self, scene: &Scene, aspect: f32, now: f64) -> Orbit {
        if let Some(f) = self.flight {
            let (o, done) = f.at(now);
            self.orbit = o;
            if done {
                self.flight = None;
                self.framed = self.framed_after;
            }
        } else if self.framed {
            self.orbit = self.orbit.framing_points(&self.points(scene), aspect);
        }
        self.orbit
    }

    /// The camera as it stands, with no flight stepped (a test's look).
    #[cfg(test)]
    #[must_use]
    pub fn orbit_now_for_test(&self) -> Orbit {
        self.orbit
    }

    /// The pose of [`Self::orbit`].
    #[cfg(test)]
    pub fn pose(&mut self, scene: &Scene, aspect: f32, now: f64) -> Pose {
        self.orbit(scene, aspect, now).pose()
    }

    /// The camera as the pointer or a key moved it: no flight goes on.
    pub fn moved(&mut self, o: Orbit) {
        self.flight = None;
        self.framed = false;
        self.orbit = o.fixed();
    }

    /// Whether a flight is under way (the frame wants another).
    #[must_use]
    pub const fn flying(&self) -> bool {
        self.flight.is_some()
    }

    fn fly(&mut self, to: Orbit, framed_after: bool, now: f64) {
        self.flight = Some(OrbitFlight::new(self.orbit, to, now));
        self.framed = false;
        self.framed_after = framed_after;
    }

    /// Flies down to `scope`'s blocks; `false` if the scene has no such scope.
    pub fn dive(&mut self, scene: &Scene, scope: usize, aspect: f32, now: f64) -> bool {
        if scope >= scene.boards.len() {
            return false;
        }
        if self.focus != Focus::Dive(scope) || !self.framed {
            self.focus = Focus::Dive(scope);
            // Down among the blocks from an angle they can be read at.
            let from = Orbit {
                pitch: self.orbit.pitch.clamp(0.5, 1.25),
                ..self.orbit
            };
            let to = from.framing_points(&scene.frame_points(Some(scope)), aspect);
            self.fly(to, true, now);
        }
        true
    }

    /// Flies back up to the whole stack; `false` if it is already there.
    pub fn up(&mut self, scene: &Scene, aspect: f32, now: f64) -> bool {
        if self.focus == Focus::Overview {
            return false;
        }
        self.frame_all(scene, aspect, now);
        true
    }

    /// Frames everything, keeping the angle of view.
    pub fn frame_all(&mut self, scene: &Scene, aspect: f32, now: f64) {
        self.focus = Focus::Overview;
        let to = self.orbit.framing_points(&scene.frame_points(None), aspect);
        self.fly(to, true, now);
    }

    /// The opening view of the whole stack: its angles too.
    pub fn reset(&mut self, scene: &Scene, aspect: f32, now: f64) {
        self.focus = Focus::Overview;
        let from = Orbit {
            yaw: YAW0,
            pitch: PITCH0,
            ..self.orbit
        };
        let to = from.framing_points(&scene.frame_points(None), aspect);
        self.fly(to, true, now);
    }

    /// Flies to look at `b` (a block, a POU node) from where it is, with
    /// a few units round it.
    pub fn look_at(&mut self, scene: &Scene, b: &Aabb, aspect: f32, now: f64) {
        let pad = [3.5, 2.5, 3.5];
        let big = Aabb::new(sub(b.min, pad), add(b.max, pad));
        let to = self.orbit.framing(&big, aspect).limited(&Self::limits(scene));
        self.fly(to, false, now);
    }

    /// Dives into the scope `step` away from the one in focus (or from
    /// `from` in the overview), wrapping round.
    pub fn step_scope(
        &mut self,
        scene: &Scene,
        from: usize,
        step: isize,
        aspect: f32,
        now: f64,
    ) -> Option<usize> {
        let n = scene.boards.len();
        if n == 0 {
            return None;
        }
        let at = match self.focus {
            Focus::Dive(s) => s,
            Focus::Overview => from.min(n - 1),
        };
        let to = (at.cast_signed() + step)
            .rem_euclid(n.cast_signed())
            .cast_unsigned();
        self.dive(scene, to, aspect, now).then_some(to)
    }
}

/// The 3D view's state, kept with the playground's.
#[derive(Default)]
pub struct View3d {
    /// The toggle.
    pub on: bool,
    pub nav: Nav,
    /// The item under the pointer, and the one clicked.
    pub hover: Option<Item>,
    pub picked: Option<Item>,
    glows: Glows,
    shared: Arc<Shared>,
}

/// The scene of a viewer's scopes with `scan` on them, and its labels.
pub struct Built {
    pub scene: Scene,
    pub labels: Vec<Label>,
}

/// The scope each scope is called from, by the prefixes (`M.` is called
/// from the scope whose prefix is `""`, `M.N.` from `M.`).
#[must_use]
pub fn parents(prefixes: &[&str]) -> Vec<Option<usize>> {
    prefixes
        .iter()
        .map(|p| {
            let up = p.trim_end_matches('.').rfind('.').map_or("", |i| &p[..=i]);
            if p.is_empty() {
                None
            } else {
                prefixes.iter().position(|q| *q == up)
            }
        })
        .collect()
}

/// The scene and the labels of a viewer's scopes with `scan` on them.
#[must_use]
pub fn built_of(v: &Viewer, scan: Option<&dyn Scan>) -> Built {
    let prefixes: Vec<&str> = v.scopes.iter().map(|s| s.prefix.as_str()).collect();
    let parents = parents(&prefixes);
    let diagrams: Vec<Option<&Diagram>> = v
        .scopes
        .iter()
        .map(|s| v.module.pous.get(s.pou).map(|p| &p.diagram))
        .collect();
    let lives: Vec<Option<Live>> = v
        .scopes
        .iter()
        .zip(&diagrams)
        .map(|(s, d)| Some(annotate((*d)?, scan?, &s.prefix)))
        .collect();
    let mut ins: Vec<ScopeIn> = Vec::new();
    let mut vars = Vec::new();
    for (i, s) in v.scopes.iter().enumerate() {
        let Some(d) = diagrams[i] else { continue };
        ins.push(ScopeIn {
            label: s.label.clone(),
            diagram: d,
            live: lives[i].as_ref(),
            parent: parents[i],
        });
        vars.push(v.module.pous.get(s.pou).map(|p| &p.vars));
    }
    let scene = scene3d::build(&ins);
    let labels = labels3d::labels(&ins, &vars, &scene);
    Built { scene, labels }
}

/// The scene of a viewer's scopes with `scan` on them.
#[cfg(test)]
#[must_use]
pub fn scene_of(v: &Viewer, scan: Option<&dyn Scan>) -> Scene {
    built_of(v, scan).scene
}

fn lin3(c: Color32) -> [f32; 3] {
    let [r, g, b, _] = c.to_array();
    [r, g, b].map(|v| (f32::from(v) / 255.0).powf(2.2))
}

fn srgb3(c: Color32) -> [f32; 3] {
    let [r, g, b, _] = c.to_array();
    [r, g, b].map(|v| f32::from(v) / 255.0)
}

impl View3d {
    /// Frees the GL objects (the app's exit).
    pub fn destroy(&self, gl: &eframe::glow::Context) {
        self.shared.destroy(gl);
    }

    /// Switches 3D on or off. Off frees the GL objects on the next paint.
    pub fn set_on(&mut self, on: bool) {
        if self.on && !on {
            self.shared.request_release();
        }
        self.on = on;
    }

    /// Whether a paint callback is wanted to free the GL objects.
    #[must_use]
    pub fn wants_release(&self) -> bool {
        !self.on && self.shared.release_pending()
    }

    /// A callback that does nothing but free the GL objects.
    pub fn release(&self, ui: &egui::Ui, area: Rect) {
        let shared = Arc::clone(&self.shared);
        let frame = Frame {
            vp: [0.0; 16],
            eye: [0.0; 3],
            batch: batch3d::Batch::default(),
            bg: [0.0; 3],
            fog: [0.0; 4],
            time: 0.0,
            bloom: 0.0,
            exposure: 1.0,
            ppp: 1.0,
        };
        ui.painter().add(egui::PaintCallback {
            rect: Rect::from_min_size(area.min, Vec2::splat(1.0)),
            callback: Arc::new(CallbackFn::new(move |info, painter| {
                shared.paint(&info, painter, &frame);
            })),
        });
    }

    /// The `3` key, with the pointer over the diagram and nothing typed
    /// in: 3D on or off.
    pub fn key_ui(&mut self, ui: &egui::Ui, area: Rect) {
        let pressed = ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key: Key::Num3, pressed: true, modifiers, .. } if modifiers.is_none())
            })
        });
        if pressed && ui.rect_contains_pointer(area) && ui.ctx().memory(|m| m.focused().is_none()) {
            let on = !self.on;
            self.set_on(on);
        }
    }

    /// The toggle's button, drawn at the top of the diagram's middle, in the
    /// row of the crumbs and the run controls and clear of the blocks and
    /// of the corners (a band is dragged from them).
    pub fn toggle_ui(&mut self, ui: &mut egui::Ui, area: Rect) {
        let t = theme(ui);
        let size = Vec2::new(48.0, 26.0);
        let rect = Rect::from_min_size(Pos2::new(area.center().x - size.x / 2.0, area.top() + 3.0), size);
        let label = if self.on { "2D" } else { "3D" };
        let r = ui
            .put(
                rect,
                egui::Button::new(egui::RichText::new(label).color(t.fg).strong())
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .fill(t.veil)
                    .stroke(Stroke::new(1.0, t.rule)),
            )
            .on_hover_text(if self.on {
                "Back to the flat diagram"
            } else {
                "The program in 3D: levels by height, orbit, a dive into the blocks"
            });
        if r.clicked() {
            let on = !self.on;
            self.set_on(on);
        }
    }

    /// The diagram in 3D, in the rest of `ui`.
    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut Viewer, scan: Option<&dyn Scan>) {
        let t = theme(ui);
        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let now = ui.input(|i| i.time);
        let built = built_of(v, scan);
        let scene = &built.scene;
        let size = (area.width(), area.height());
        let aspect = Camera::new(Pose::default(), size).aspect();
        let limits = Nav::limits(scene);
        let mut orbit = self.nav.orbit(scene, aspect, now);

        // The pointer and the keys move the camera; a flight stops where
        // it is.
        let (shift, scroll, pinch, keys, dt, fingers) = ui.input(|i| {
            let held = |a: Key, b: Key| f32::from(u8::from(i.key_down(a) || i.key_down(b)));
            let mv = if i.modifiers.any() {
                Vec2::ZERO
            } else {
                Vec2::new(
                    held(Key::ArrowRight, Key::D) - held(Key::ArrowLeft, Key::A),
                    held(Key::ArrowDown, Key::S) - held(Key::ArrowUp, Key::W),
                )
            };
            (
                i.modifiers.shift,
                i.smooth_scroll_delta,
                i.zoom_delta(),
                mv,
                i.stable_dt.min(0.1),
                // Two fingers on a trackpad (native; a browser does not say).
                matches!(
                    i.scroll_source(),
                    Some(egui::MouseWheelSource::Trackpad | egui::MouseWheelSource::Momentum)
                ),
            )
        });
        let keyboard = response.hovered() && ui.ctx().memory(|m| m.focused().is_none());
        let local = |p: Pos2| p - area.min;
        let mut moved = false;
        if response.dragged() {
            let d = response.drag_delta();
            let pan = response.dragged_by(egui::PointerButton::Secondary)
                || response.dragged_by(egui::PointerButton::Middle)
                || shift;
            orbit = if pan {
                orbit.panned(d.x, d.y, size.1, &limits)
            } else {
                orbit.orbited(d.x, d.y, &limits)
            };
            moved = d != Vec2::ZERO;
        }
        if response.hovered() {
            let at = response.hover_pos().map_or((size.0 / 2.0, size.1 / 2.0), |p| {
                let l = local(p);
                (l.x, l.y)
            });
            let factor = if (pinch - 1.0).abs() > f32::EPSILON {
                pinch
            } else {
                (scroll.y / (SCROLL_ZOOM * 1.5)).exp()
            };
            if (shift || fingers) && scroll != Vec2::ZERO && (pinch - 1.0).abs() <= f32::EPSILON {
                orbit = orbit.panned(scroll.x, scroll.y, size.1, &limits);
                moved = true;
            } else if (factor - 1.0).abs() > 1e-4 {
                orbit = orbit.zoomed_at(at.0, at.1, size, factor, &limits);
                moved = true;
            } else if scroll.x.abs() > 0.5 {
                orbit = orbit.panned(scroll.x, 0.0, size.1, &limits);
                moved = true;
            }
        }
        if keys != Vec2::ZERO && keyboard {
            let k = 520.0 * dt;
            orbit = orbit.panned(-keys.x * k, -keys.y * k, size.1, &limits);
            moved = true;
        }
        if moved {
            self.nav.moved(orbit);
        }
        let cam = Camera::new(orbit.pose(), size);

        // What is under the pointer; a click picks it and flies to it.
        let dive = match self.nav.focus {
            Focus::Dive(s) => Some(s),
            Focus::Overview => None,
        };
        self.hover = response
            .hover_pos()
            .filter(|_| response.hovered())
            .and_then(|p| cam.ray(local(p).x, local(p).y))
            .and_then(|r| scene.pick(&r, dive));
        if response.clicked()
            && let Some(item) = self.hover
        {
            self.picked = Some(item);
            if let Item::Block(s, n) = item {
                v.scope = s.min(v.scopes.len().saturating_sub(1));
                v.pick = Some(Pick::Node(n));
            }
            let b = scene.solid(item).map(|s| s.bounds);
            if let Some(b) = b
                && !response.double_clicked()
            {
                self.nav.look_at(scene, &b, aspect, now);
            }
        } else if response.clicked() {
            self.picked = None;
        }
        let keys_pressed = ui.input(|i| {
            let pressed = |k: Key| i.events.iter().any(|e| matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if *key == k && modifiers.is_none()));
            (
                pressed(Key::Enter),
                pressed(Key::Escape),
                pressed(Key::F) || pressed(Key::Home),
                pressed(Key::Num1),
                pressed(Key::Num2),
                pressed(Key::R),
                pressed(Key::Q),
                pressed(Key::E),
            )
        });
        let (enter, esc, frame_all, one, two, reset, prev, next) = keys_pressed;
        let here = match self.nav.focus {
            Focus::Dive(s) => s,
            Focus::Overview => self.picked.or(self.hover).map_or(0, Item::scope),
        };
        let mut dive_to = None;
        if response.double_clicked() {
            dive_to = self.hover.map(Item::scope);
        } else if keyboard {
            if enter {
                dive_to = self.picked.or(self.hover).map(Item::scope);
            } else if two {
                dive_to = Some(here);
            } else if prev || next {
                let step = if next { 1 } else { -1 };
                dive_to = self.nav.step_scope(scene, here, step, aspect, now);
                if dive_to.is_some() {
                    self.picked = None;
                }
            }
            if frame_all {
                self.nav.frame_all(scene, aspect, now);
            }
            if one || reset {
                self.nav.reset(scene, aspect, now);
            }
        }
        if let Some(s) = dive_to
            && self.nav.dive(scene, s, aspect, now)
        {
            v.scope = s.min(v.scopes.len().saturating_sub(1));
        }
        if esc && !v.menu_open && self.nav.up(scene, aspect, now) {
            self.picked = None;
        }

        // The glows ease and flare; the frame asks for another while
        // anything moves or a wire carries pulses.
        let solids: Vec<(bool, u64)> = scene.solids.iter().map(|s| (s.lit, s.sig)).collect();
        let segs: Vec<(bool, u64)> = scene.segs.iter().map(|s| (s.lit(), 0)).collect();
        self.glows.update(&solids, &segs, now);
        let pulses = scene.segs.iter().any(scene3d::Seg::lit);
        if self.nav.flying() || self.glows.moving(now) || pulses {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(if self.nav.flying() {
                    8
                } else {
                    30
                }));
        }

        // The GL pass.
        let batch = batch3d::build(scene, self.nav.focus, &t, Some(&self.glows), now);
        let bounds = scene.bounds();
        let r = sub(bounds.max, bounds.min);
        let radius = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt() / 2.0;
        let bg = lin3(t.bg);
        let frame = Frame {
            vp: cam.view_proj(),
            eye: orbit.pose().eye,
            batch,
            bg: srgb3(t.bg),
            fog: [bg[0], bg[1], bg[2], 1.0 / (radius * 3.0).max(1.0)],
            time: pulse_phase(now, PULSE_SPEED),
            bloom: if t.dark { 0.95 } else { 0.5 },
            exposure: 1.0,
            ppp: ui.ctx().pixels_per_point(),
        };
        if let Some(why) = self.shared.error() {
            ui.painter().rect_filled(area, 0.0, t.bg);
            ui.painter().text(
                area.center(),
                Align2::CENTER_CENTER,
                format!("3D is not available here ({why}). The flat diagram is behind the 2D button."),
                FontId::proportional(crate::theme::BODY),
                t.muted,
            );
            return;
        }
        let shared = Arc::clone(&self.shared);
        let frame = Arc::new(frame);
        ui.painter().add(egui::PaintCallback {
            rect: area,
            callback: Arc::new(CallbackFn::new(move |info, painter| {
                shared.paint(&info, painter, &frame);
            })),
        });

        self.overlay(ui, &built, &cam, area, &t, dive, orbit.dist);
    }

    /// Labels, and the outlines of what is hovered and picked, projected
    /// from 3D onto the scene.
    #[allow(clippy::too_many_arguments)]
    fn overlay(
        &self,
        ui: &egui::Ui,
        built: &Built,
        cam: &Camera,
        area: Rect,
        t: &Theme,
        dive: Option<usize>,
        dist: f32,
    ) {
        let painter = ui.painter_at(area);
        let scene = &built.scene;
        let at = |p: [f32; 3]| cam.project(p).map(|(x, y, _)| area.min + Vec2::new(x, y));
        let opts = Opts {
            focus: dive,
            reach: dist * 1.6,
            ..Opts::default()
        };
        for p in labels3d::place(&built.labels, cam, &opts) {
            let l = &built.labels[p.index];
            let ink = match l.tone {
                Tone::Normal => t.fg,
                Tone::Muted => t.muted,
                Tone::True => t.ink(GREEN),
                Tone::False => t.ink(RED),
                Tone::Accent => t.accent,
            };
            let ink = ink.gamma_multiply(p.alpha);
            let rect = Rect::from_min_max(
                area.min + Vec2::new(p.rect[0], p.rect[1]),
                area.min + Vec2::new(p.rect[2], p.rect[3]),
            );
            if matches!(
                l.kind,
                Kind::Node | Kind::Value | Kind::Region | Kind::Pou | Kind::WireValue
            ) {
                painter.rect_filled(rect, 3.0, t.veil.gamma_multiply(0.55 * p.alpha));
            }
            painter.text(
                Pos2::new(rect.left() + 2.0, rect.center().y),
                Align2::LEFT_CENTER,
                &p.text,
                FontId::monospace(p.font),
                ink,
            );
        }
        for (item, color) in [(self.hover, SELECT), (self.picked, YELLOW)] {
            if let Some(b) = item.and_then(|i| scene.solid(i)).map(|s| s.bounds) {
                outline(&painter, &b.corners().map(at), color);
            }
        }
        let hint = if dive.is_some() {
            "drag: orbit   right-drag or Shift-drag: pan   wheel: zoom   F: frame all   Q E: other POU   Esc: up"
        } else {
            "drag: orbit   right-drag or Shift-drag: pan   wheel: zoom   F: frame all   double-click or Enter: dive   Q E: POU"
        };
        painter.text(
            Pos2::new(area.left() + 12.0, area.bottom() - 12.0),
            Align2::LEFT_CENTER,
            hint,
            FontId::proportional(12.0),
            t.muted.gamma_multiply(0.8),
        );
    }
}

fn outline(painter: &egui::Painter, c: &[Option<Pos2>; 8], color: Color32) {
    for (i, j) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        if let (Some(a), Some(b)) = (c[i], c[j]) {
            painter.line_segment([a, b], Stroke::new(2.0, color));
        }
    }
}
