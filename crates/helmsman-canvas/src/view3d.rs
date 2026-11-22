//! The playground's diagram in 3D, behind a toggle (`canvas-3d-mvp`):
//! the scene of `scene3d`, a camera that dives into a POU's blocks and
//! climbs back, picking by ray, and labels drawn by egui over the GL pass.
//! 2D stays the default and is not touched: this view replaces the
//! diagram's canvas only while it is on.
//!
//! [`Nav`] is the part with no egui in it (focus, the camera's pose, the
//! flight between two); the tests read it (`view3d_tests.rs`).

use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::egui_glow::CallbackFn;
use helmsman_diagram::{Diagram, Live, Scan, annotate};

use crate::batch3d::{self, Focus};
use crate::cam3d::{Camera, Flight, Pose, framing, view_dir};
use crate::gl3d::{Frame, Shared};
use crate::scene3d::{self, Item, Level, Scene, ScopeIn};
use crate::st_view::brand::{SELECT, YELLOW};
use crate::st_view::{Pick, Viewer};
use crate::theme::{Theme, theme};

/// Where the camera is, and how it gets elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct Nav {
    pub focus: Focus,
    flight: Option<Flight>,
    /// The pose last given out.
    pose: Pose,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            focus: Focus::Overview,
            flight: None,
            pose: Pose::default(),
        }
    }
}

impl Nav {
    /// The pose that frames what `focus` shows.
    #[must_use]
    pub fn target(&self, scene: &Scene, aspect: f32) -> Pose {
        let b = match self.focus {
            Focus::Overview => scene.bounds(),
            Focus::Dive(s) => scene.boards.get(s).copied().unwrap_or_else(|| scene.bounds()),
        };
        framing(&b, aspect, view_dir())
    }

    /// The camera now, at page time `now`: mid-flight, or where the focus
    /// frames (so a resize or a new scene needs no handling).
    pub fn pose(&mut self, scene: &Scene, aspect: f32, now: f64) -> Pose {
        let mut end = None;
        if let Some(f) = self.flight {
            let (p, done) = f.at(now);
            if done {
                self.flight = None;
            } else {
                end = Some(p);
            }
        }
        self.pose = end.unwrap_or_else(|| self.target(scene, aspect));
        self.pose
    }

    /// Whether a flight is under way (the frame wants another).
    #[must_use]
    pub const fn flying(&self) -> bool {
        self.flight.is_some()
    }

    fn fly_to(&mut self, focus: Focus, scene: &Scene, aspect: f32, now: f64) {
        let from = self.pose;
        self.focus = focus;
        let to = self.target(scene, aspect);
        self.flight = Some(Flight::new(from, to, now));
    }

    /// Flies down to `scope`'s blocks; `false` if the scene has no such scope.
    pub fn dive(&mut self, scene: &Scene, scope: usize, aspect: f32, now: f64) -> bool {
        if scope >= scene.boards.len() {
            return false;
        }
        if self.focus != Focus::Dive(scope) {
            self.fly_to(Focus::Dive(scope), scene, aspect, now);
        }
        true
    }

    /// Flies back up to the overview; `false` if it is already there.
    pub fn up(&mut self, scene: &Scene, aspect: f32, now: f64) -> bool {
        if self.focus == Focus::Overview {
            return false;
        }
        self.fly_to(Focus::Overview, scene, aspect, now);
        true
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
    shared: Arc<Shared>,
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

/// The scene of a viewer's scopes with `scan` on them.
#[must_use]
pub fn scene_of(v: &Viewer, scan: Option<&dyn Scan>) -> Scene {
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
    let ins: Vec<ScopeIn> = v
        .scopes
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            Some(ScopeIn {
                label: s.label.clone(),
                diagram: diagrams[i]?,
                live: lives[i].as_ref(),
                parent: parents[i],
            })
        })
        .collect();
    scene3d::build(&ins)
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
            batch: batch3d::Batch::default(),
            bg: [0.0; 3],
            bloom: false,
            ppp: 1.0,
        };
        ui.painter().add(egui::PaintCallback {
            rect: Rect::from_min_size(area.min, Vec2::splat(1.0)),
            callback: Arc::new(CallbackFn::new(move |info, painter| {
                shared.paint(&info, painter, &frame);
            })),
        });
    }

    /// The toggle's button, drawn over the diagram's left edge, clear of its corners (a band is dragged from them).
    pub fn toggle_ui(&mut self, ui: &mut egui::Ui, area: Rect) {
        let t = theme(ui);
        let size = Vec2::new(48.0, 28.0);
        let rect = Rect::from_min_size(Pos2::new(area.left() + 10.0, area.bottom() - size.y - 80.0), size);
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
                "The program in 3D: levels by height, a dive into the blocks"
            });
        if r.clicked() {
            let on = !self.on;
            self.set_on(on);
        }
    }

    /// The diagram in 3D, in the rest of `ui`. A click on a block picks it
    /// (the code's line follows, as in 2D), a double click or Enter dives,
    /// Escape climbs.
    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut Viewer, scan: Option<&dyn Scan>) {
        let t = theme(ui);
        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let now = ui.input(|i| i.time);
        let scene = scene_of(v, scan);
        let size = (area.width(), area.height());
        let aspect = Camera::new(Pose::default(), size).aspect();
        let cam = Camera::new(self.nav.pose(&scene, aspect, now), size);
        let dive = match self.nav.focus {
            Focus::Dive(s) => Some(s),
            Focus::Overview => None,
        };

        // Pointer: what is under it.
        let local = |p: Pos2| p - area.min;
        self.hover = response
            .hover_pos()
            .filter(|_| response.hovered())
            .and_then(|p| cam.ray(local(p).x, local(p).y))
            .and_then(|r| scene.pick(&r, dive));
        if response.clicked() {
            self.picked = self.hover;
            if let Some(Item::Block(s, n)) = self.picked {
                v.scope = s.min(v.scopes.len().saturating_sub(1));
                v.pick = Some(Pick::Node(n));
            }
        }
        let keyboard = response.hovered() && ui.ctx().memory(|m| m.focused().is_none());
        let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let mut dive_to = None;
        if response.double_clicked() {
            dive_to = self.hover.map(Item::scope);
        } else if enter && keyboard {
            dive_to = self.picked.or(self.hover).map(Item::scope);
        }
        if let Some(s) = dive_to
            && self.nav.dive(&scene, s, aspect, now)
        {
            v.scope = s.min(v.scopes.len().saturating_sub(1));
        }
        if esc && !v.menu_open && self.nav.up(&scene, aspect, now) {
            self.picked = None;
        }
        if self.nav.flying() {
            ui.ctx().request_repaint();
        }

        // The GL pass.
        let batch = batch3d::build(&scene, self.nav.focus, &t);
        let frame = Frame {
            vp: cam.view_proj(),
            batch,
            bg: [
                f32::from(t.bg.r()) / 255.0,
                f32::from(t.bg.g()) / 255.0,
                f32::from(t.bg.b()) / 255.0,
            ],
            bloom: true,
            ppp: ui.ctx().pixels_per_point(),
        };
        if let Some(why) = self.shared.error() {
            ui.painter().rect_filled(area, 0.0, t.bg);
            ui.painter().text(
                area.center(),
                egui::Align2::CENTER_CENTER,
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

        self.overlay(ui, &scene, &cam, area, &t, dive);
    }

    /// Labels, and the outlines of what is hovered and picked, projected
    /// from 3D onto the scene.
    fn overlay(
        &self,
        ui: &egui::Ui,
        scene: &Scene,
        cam: &Camera,
        area: Rect,
        t: &Theme,
        dive: Option<usize>,
    ) {
        let painter = ui.painter_at(area);
        let at = |p: [f32; 3]| cam.project(p).map(|(x, y, _)| area.min + Vec2::new(x, y));
        let font = FontId::proportional(13.0);
        for s in &scene.solids {
            let Some(item) = s.item else { continue };
            // Blocks are named while their board is the one dived into;
            // the POU nodes while the view is up there.
            let shown = match item {
                Item::Pou(_) => dive.is_none() || s.level == Level::Middle,
                Item::Block(sc, _) => dive == Some(sc),
            };
            if !shown || s.label.is_empty() {
                continue;
            }
            let ghost = matches!(item, Item::Pou(_)) && dive.is_some();
            let top = s.bounds.center();
            let (Some(p), Some(l), Some(r)) = (
                at([top[0], s.bounds.max[1], top[2]]),
                at([s.bounds.min[0], s.bounds.max[1], top[2]]),
                at([s.bounds.max[0], s.bounds.max[1], top[2]]),
            ) else {
                continue;
            };
            let room = (r.x - l.x).abs();
            if room < 22.0 {
                continue;
            }
            let max_chars = ((room / 7.0) as usize).max(3);
            let text: String = if s.label.chars().count() > max_chars {
                s.label
                    .chars()
                    .take(max_chars.saturating_sub(1))
                    .chain(['…'])
                    .collect()
            } else {
                s.label.clone()
            };
            let ink = if ghost { t.muted.gamma_multiply(0.5) } else { t.fg };
            let galley = painter.layout_no_wrap(text, font.clone(), ink);
            let rect = Rect::from_center_size(p, galley.size() + Vec2::new(8.0, 2.0));
            if !ghost {
                painter.rect_filled(rect, 3.0, t.veil);
            }
            painter.galley(rect.center() - galley.size() / 2.0, galley, ink);
        }
        for (item, color) in [(self.hover, SELECT), (self.picked, YELLOW)] {
            if let Some(b) = item.and_then(|i| scene.solid(i)).map(|s| s.bounds) {
                outline(&painter, &b.corners().map(at), color);
            }
        }
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
