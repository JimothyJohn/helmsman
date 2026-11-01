//! The glyph buttons over the diagram's top-right corner (Nick,
//! 2026-11-01): run/stop, step and restart for the simulation, then save,
//! upload, download and help. Drawn as strokes in the brand colours, no
//! font or emoji, so they look the same on every machine.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, Vec2};

use crate::st_view::TAP;
use crate::st_view::brand::{GREEN, RED, STEEL, WHITE, YELLOW};

/// What a glyph button shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// ▶, green: start.
    Run,
    /// ■, red: stop.
    Stop,
    /// ▶| : one scan.
    Step,
    /// ↻ : cold restart.
    Restart,
    /// A floppy's outline.
    Save,
    /// An arrow up out of a tray.
    Upload,
    /// An arrow down into a tray.
    Download,
    /// ? in a ring.
    Help,
}

/// The side of a glyph button, points.
pub const SIZE: f32 = TAP * 0.8;

/// One glyph button; `on` tints it as active.
pub fn button(ui: &mut egui::Ui, icon: Icon, enabled: bool, on: bool, tip: &str) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(SIZE), sense);
    let r = r.on_hover_text(tip);
    let painter = ui.painter();
    let hot = enabled && r.hovered();
    if hot || on {
        let tint = if on {
            YELLOW.gamma_multiply(0.2)
        } else {
            ui.style().interact(&r).bg_fill
        };
        painter.rect_filled(rect, CornerRadius::same(5), tint);
    }
    let color = match (enabled, icon) {
        (false, _) => STEEL.gamma_multiply(0.5),
        (true, Icon::Run) => GREEN,
        (true, Icon::Stop) => RED,
        (true, _) if hot => YELLOW,
        (true, _) => WHITE,
    };
    draw(painter, rect.shrink(SIZE * 0.26), icon, color);
    r
}

fn draw(p: &egui::Painter, b: Rect, icon: Icon, color: Color32) {
    let w = (b.width() * 0.13).max(1.5);
    let stroke = Stroke::new(w, color);
    let (l, t, r, bt) = (b.left(), b.top(), b.right(), b.bottom());
    let c = b.center();
    match icon {
        Icon::Run => {
            p.add(egui::Shape::convex_polygon(
                vec![Pos2::new(l, t), Pos2::new(r, c.y), Pos2::new(l, bt)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Stop => {
            p.rect_filled(b.shrink(b.width() * 0.08), CornerRadius::same(2), color);
        }
        Icon::Step => {
            let rr = l + b.width() * 0.68;
            p.add(egui::Shape::convex_polygon(
                vec![Pos2::new(l, t), Pos2::new(rr, c.y), Pos2::new(l, bt)],
                color,
                Stroke::NONE,
            ));
            p.line_segment([Pos2::new(r - w, t), Pos2::new(r - w, bt)], stroke);
        }
        Icon::Restart => {
            // Three quarters of a ring, an arrowhead at its open end.
            let radius = b.width() * 0.42;
            let n = 24;
            let pts: Vec<Pos2> = (0..=n)
                .map(|k| {
                    #[allow(clippy::cast_precision_loss)]
                    let a =
                        -std::f32::consts::FRAC_PI_2 + (k as f32 / n as f32) * std::f32::consts::TAU * 0.78;
                    Pos2::new(c.x + radius * a.cos(), c.y + radius * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(pts, stroke));
            let tip = Pos2::new(c.x, t + w * 0.5);
            p.add(egui::Shape::convex_polygon(
                vec![
                    tip + Vec2::new(-radius * 0.45, -radius * 0.35),
                    tip + Vec2::new(radius * 0.25, 0.0),
                    tip + Vec2::new(-radius * 0.45, radius * 0.35),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Save => {
            p.rect_stroke(b, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            let slot = Rect::from_min_max(
                Pos2::new(l + b.width() * 0.25, t),
                Pos2::new(r - b.width() * 0.25, t + b.height() * 0.3),
            );
            p.rect_stroke(slot, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            let label = Rect::from_min_max(
                Pos2::new(l + b.width() * 0.2, bt - b.height() * 0.35),
                Pos2::new(r - b.width() * 0.2, bt),
            );
            p.rect_filled(label, CornerRadius::ZERO, color);
        }
        Icon::Upload | Icon::Download => {
            let up = icon == Icon::Upload;
            let tray = [
                Pos2::new(l, bt - b.height() * 0.3),
                Pos2::new(l, bt),
                Pos2::new(r, bt),
                Pos2::new(r, bt - b.height() * 0.3),
            ];
            p.add(egui::Shape::line(tray.to_vec(), stroke));
            let (a, z) = if up {
                (Pos2::new(c.x, bt - b.height() * 0.35), Pos2::new(c.x, t))
            } else {
                (Pos2::new(c.x, t), Pos2::new(c.x, bt - b.height() * 0.35))
            };
            p.line_segment([a, z], stroke);
            let head = b.width() * 0.28;
            let dir = if up { -1.0 } else { 1.0 };
            p.line_segment([z, z + Vec2::new(-head, -dir * head)], stroke);
            p.line_segment([z, z + Vec2::new(head, -dir * head)], stroke);
        }
        Icon::Help => {
            p.circle_stroke(c, b.width() * 0.5, stroke);
            p.text(
                c + Vec2::new(0.0, -w * 0.2),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional(b.height() * 0.78),
                color,
            );
        }
    }
}
