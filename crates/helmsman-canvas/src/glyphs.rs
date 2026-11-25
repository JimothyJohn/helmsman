//! The glyph buttons over the diagram's top-right corner (Nick,
//! 2026-11-01): run/stop, step and restart for the simulation, then save,
//! upload, download and help. Drawn as strokes in the brand colours, no
//! font or emoji, so they look the same on every machine.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, Vec2};

use crate::st_view::TAP;
use crate::st_view::brand::{GREEN, RED, YELLOW};
use crate::theme::theme;

/// What a glyph button shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// ▶, green: start.
    Run,
    /// ■, red: stop.
    Stop,
    /// Two upright bars: pause, the state held.
    Pause,
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
    /// A head and shoulders: who is signed in, whatever the role (Nick,
    /// 2026-11-25: "the standard user glyph, not a glyph based on the
    /// role").
    User,
    /// < > : the Agent changes the program (its mode toggle, Nick,
    /// 2026-11-08); [`Icon::Help`] is the other side.
    Code,
    /// → : deploy the open project, its latest edits (Nick, 2026-11-08).
    /// Beside the running project in Files and the Code header.
    Deploy,
    /// A magnifier: find (and replace) in the open file.
    Find,
    /// A line turning back under itself: word wrap in the Code tab
    /// (Nick, 2026-11-08), lit while on.
    Wrap,
    /// A folder: the Files tab.
    Files,
    /// A module with terminals down both sides: the Devices tab.
    Devices,
    /// An arrow in over an arrow out: the I/O tab.
    Io,
    /// A speech bubble: the Agent tab.
    Agent,
    /// A list of lines, each behind its mark: the Messages tab.
    Messages,
    /// A paper plane: send to the Agent (Nick, 2026-11-13: no words).
    Send,
    /// An arrow hooked back to the left: undo the Agent's last change.
    Undo,
    /// A screen on a stand: the Monitor.
    Monitor,
    /// A clock face: the History tab (Nick, 2026-11-13).
    History,
    /// i in a ring: an info message (Nick, 2026-11-13: levels as icons).
    Info,
    /// ! in a triangle: a warning.
    Warning,
    /// × in a ring: an error.
    Error,
    /// ← : back to the revision before the one shown (the Agent tab's
    /// step through its changes; Nick, 2026-11-14: arrows, not words).
    Back,
    /// → , thin: on to the revision after the one shown.
    Forward,
    /// Two blocks joined by a wire: where a signal is on the diagram
    /// (a card of the I/O tab; Nick, 2026-11-16).
    Diagram,
    /// Three full-width lines: the I/O tab's cards one to a row.
    List,
    /// Four squares: the I/O tab's cards as tiles.
    Grid,
    /// Two sheets, one behind the other: copy (the Messages tab).
    Copy,
    /// A bin: empty the Messages.
    Clear,
    /// Arrows out to four sides: pan the diagram (the corner controls;
    /// a hand it was, which no handful of strokes draws well small).
    Pan,
    /// A magnifier with a plus: zoom in.
    ZoomIn,
    /// A magnifier with a minus: zoom out.
    ZoomOut,
}

/// An action button of a panel (Send, Stop, Undo; Nick, 2026-11-12: the
/// app should speak in pictures) at the size the worded buttons had, its
/// glyph in place of the word: `primary` fills it in the brand yellow
/// with black ink, as the worded Send was. The caller tips it with what
/// it does.
pub fn action(ui: &mut egui::Ui, icon: Icon, enabled: bool, primary: bool) -> Response {
    action_wide(ui, icon, enabled, primary, ACTION_W)
}

/// How wide an [`action`] button is, points.
pub const ACTION_W: f32 = TAP * 1.5;

/// [`action`], `width` points wide: a row of several in a narrow pane
/// shares the pane's width instead of running off it.
pub fn action_wide(ui: &mut egui::Ui, icon: Icon, enabled: bool, primary: bool, width: f32) -> Response {
    let size = Vec2::new(width, TAP * 0.75);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, r) = ui.allocate_exact_size(size, sense);
    let t = theme(ui);
    let painter = ui.painter();
    let hot = enabled && r.hovered();
    let (fill, ink) = match (enabled, primary) {
        (false, _) => (t.rule.gamma_multiply(0.5), t.muted.gamma_multiply(0.5)),
        (true, true) => (
            if hot { YELLOW.gamma_multiply(0.85) } else { YELLOW },
            crate::st_view::brand::BLACK,
        ),
        (true, false) => (
            if hot {
                ui.style().interact(&r).bg_fill
            } else {
                t.rule
            },
            t.fg,
        ),
    };
    painter.rect_filled(rect, CornerRadius::same(5), fill);
    let ink = if enabled && icon == Icon::Stop {
        t.ink(RED)
    } else {
        ink
    };
    let side = rect.height() * 0.56;
    draw(
        painter,
        Rect::from_center_size(rect.center(), Vec2::splat(side)),
        icon,
        ink,
    );
    r
}

/// How wide the Deploy button is, points.
pub const DEPLOY_W: f32 = ROW * 1.7;

/// Deploy as a button that reads as one (Nick, 2026-11-23) without
/// outgrowing its row (Nick, 2026-11-24: "too big, green, and awkward"):
/// the height of the glyphs beside it, filled in the brand yellow with a
/// black arrow, as Send is. The caller tips it.
pub fn deploy(ui: &mut egui::Ui, enabled: bool) -> Response {
    let size = Vec2::new(DEPLOY_W, ROW);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, r) = ui.allocate_exact_size(size, sense);
    let t = theme(ui);
    let hot = enabled && r.hovered();
    let (fill, ink) = if enabled {
        (
            if hot { YELLOW.gamma_multiply(0.85) } else { YELLOW },
            crate::st_view::brand::BLACK,
        )
    } else {
        (t.rule.gamma_multiply(0.5), t.muted.gamma_multiply(0.5))
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(5), fill);
    let side = rect.height() * 0.5;
    draw(
        painter,
        Rect::from_center_size(rect.center(), Vec2::new(side * 1.3, side)),
        Icon::Deploy,
        ink,
    );
    r
}

/// The side of a glyph button, points.
pub const SIZE: f32 = TAP * 0.8;
/// The side of a glyph in a text row (a file's, a header's), points.
pub const ROW: f32 = TAP * 0.6;

/// One glyph button; `on` tints it as active.
pub fn button(ui: &mut egui::Ui, icon: Icon, enabled: bool, on: bool, tip: &str) -> Response {
    button_sized(ui, icon, SIZE, enabled, on, tip)
}

/// [`button`] at `size` points a side: a glyph in a text row takes the
/// row's height, not the toolbar's.
pub fn button_sized(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    enabled: bool,
    on: bool,
    tip: &str,
) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let r = r.on_hover_text(tip);
    let t = theme(ui);
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
        (false, _) => t.muted.gamma_multiply(0.5),
        (true, Icon::Run | Icon::Deploy) => crate::theme::theme(ui).ink(GREEN),
        (true, Icon::Stop) => crate::theme::theme(ui).ink(RED),
        (true, _) if hot => t.accent,
        (true, _) => t.fg,
    };
    draw(painter, rect.shrink(size * 0.26), icon, color);
    r
}

/// A tab as a glyph (Nick, 2026-11-11: glyphs for the tab titles, the
/// tooltip says what each is): `size` points a side, muted until it is
/// the one on, which gets the 2 pt accent bar under it. `alarm` (0 to 1)
/// puts a red ground of that strength behind it and draws it red: the
/// Messages tab with an error on it. The caller tips it.
pub fn tab(ui: &mut egui::Ui, icon: Icon, size: f32, on: bool, alarm: Option<f32>) -> Response {
    tab_sensed(ui, icon, size, on, alarm, Sense::click())
}

/// [`tab`] with its own sense: a side pane's tab can be dragged too.
pub fn tab_sensed(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    on: bool,
    alarm: Option<f32>,
    sense: Sense,
) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let t = theme(ui);
    let painter = ui.painter();
    let red = t.ink(RED);
    if let Some(a) = alarm {
        painter.rect_filled(rect, CornerRadius::same(5), RED.gamma_multiply(0.15 + 0.55 * a));
    } else if r.hovered() {
        let tint = ui.style().interact(&r).bg_fill;
        painter.rect_filled(rect, CornerRadius::same(5), tint);
    }
    let color = match alarm {
        // On a strong red ground the page's ink reads; on a faint one, red.
        Some(a) if a > 0.5 => t.fg,
        Some(_) => red,
        None if on => t.fg,
        None if r.hovered() => t.accent,
        None => t.muted,
    };
    draw(painter, rect.shrink(size * 0.24), icon, color);
    if on {
        painter.hline(
            (rect.left() + 4.0)..=(rect.right() - 4.0),
            rect.bottom() - 1.0,
            Stroke::new(2.0, if alarm.is_some() { red } else { t.accent }),
        );
    }
    r
}

/// `icon` drawn in `rect` in `color`, outside a button (the header's
/// signed-in role).
pub fn paint(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    draw(p, rect, icon, color);
}

/// An open arrowhead at `tip` pointing along `dir` (a unit vector), its
/// two strokes `size` long on each axis: one line, so the point is a
/// clean corner and not two butted ends.
fn head(p: &egui::Painter, tip: Pos2, dir: Vec2, size: f32, stroke: Stroke) {
    let side = Vec2::new(-dir.y, dir.x);
    p.add(egui::Shape::line(
        vec![
            tip - dir * size + side * size,
            tip,
            tip - dir * size - side * size,
        ],
        stroke,
    ));
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
        Icon::Pause => {
            let bar = b.width() * 0.3;
            for x in [l + b.width() * 0.08, r - b.width() * 0.08 - bar] {
                let rect = Rect::from_min_max(Pos2::new(x, t), Pos2::new(x + bar, bt));
                p.rect_filled(rect, CornerRadius::same(1), color);
            }
        }
        Icon::Files => {
            // One outline: the tab at the top left, its slope down to the
            // body's top edge, the body.
            let h = w * 0.5;
            let (top, tab) = (t + b.height() * 0.3, t + b.height() * 0.1);
            p.add(egui::Shape::closed_line(
                vec![
                    Pos2::new(l + h, bt - b.height() * 0.08),
                    Pos2::new(l + h, tab),
                    Pos2::new(l + b.width() * 0.38, tab),
                    Pos2::new(l + b.width() * 0.52, top),
                    Pos2::new(r - h, top),
                    Pos2::new(r - h, bt - b.height() * 0.08),
                ],
                stroke,
            ));
        }
        Icon::Devices => {
            let body = Rect::from_min_max(
                Pos2::new(l + b.width() * 0.26, t),
                Pos2::new(r - b.width() * 0.26, bt),
            );
            p.rect_stroke(body, CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
            for k in [0.22, 0.5, 0.78] {
                let y = t + b.height() * k;
                p.line_segment([Pos2::new(l, y), Pos2::new(body.left(), y)], stroke);
                p.line_segment([Pos2::new(body.right(), y), Pos2::new(r, y)], stroke);
            }
        }
        Icon::Io => {
            let size = b.width() * 0.22;
            let inset = w * 0.7;
            let (y1, y2) = (t + b.height() * 0.27, bt - b.height() * 0.27);
            p.line_segment([Pos2::new(l, y1), Pos2::new(r - inset, y1)], stroke);
            head(p, Pos2::new(r - inset, y1), Vec2::new(1.0, 0.0), size, stroke);
            p.line_segment([Pos2::new(l + inset, y2), Pos2::new(r, y2)], stroke);
            head(p, Pos2::new(l + inset, y2), Vec2::new(-1.0, 0.0), size, stroke);
        }
        Icon::Agent => {
            // A bubble with its tail at the bottom left, two lines in it.
            let base = bt - b.height() * 0.24;
            let bubble = Rect::from_min_max(Pos2::new(l, t), Pos2::new(r, base));
            p.rect_stroke(bubble, CornerRadius::same(3), stroke, egui::StrokeKind::Inside);
            let tail = [
                Pos2::new(l + b.width() * 0.2, base - w * 0.5),
                Pos2::new(l + b.width() * 0.2, bt),
                Pos2::new(l + b.width() * 0.48, base - w * 0.5),
            ];
            p.add(egui::Shape::line(tail.to_vec(), stroke));
            for k in [0.36, 0.64] {
                let y = t + (base - t) * k;
                let end = if k < 0.5 { 0.24 } else { 0.42 };
                p.line_segment(
                    [
                        Pos2::new(l + b.width() * 0.24, y),
                        Pos2::new(r - b.width() * end, y),
                    ],
                    Stroke::new(w * 0.8, color),
                );
            }
        }
        Icon::Send => {
            // A paper plane pointing right: its outline, and the fold.
            let tip = Pos2::new(r, c.y);
            let fold = Pos2::new(l + b.width() * 0.32, c.y);
            p.add(egui::Shape::closed_line(
                vec![Pos2::new(l, t + w), tip, Pos2::new(l, bt - w), fold],
                stroke,
            ));
            p.line_segment([fold, tip], stroke);
        }
        Icon::Undo => {
            // From the right, a half ring down and back left, ending in an
            // arrowhead pointing left at the top.
            let radius = b.height() * 0.3;
            let top = t + b.height() * 0.3;
            let centre = Pos2::new(r - radius - w * 0.5, top + radius);
            let n = 16;
            let mut pts: Vec<Pos2> = (0..=n)
                .map(|k| {
                    #[allow(clippy::cast_precision_loss)]
                    let a = -std::f32::consts::FRAC_PI_2 + (k as f32 / n as f32) * std::f32::consts::PI;
                    Pos2::new(centre.x + radius * a.cos(), centre.y + radius * a.sin())
                })
                .collect();
            pts.insert(0, Pos2::new(l + b.width() * 0.25, top));
            pts.push(Pos2::new(l + b.width() * 0.3, top + 2.0 * radius));
            p.add(egui::Shape::line(pts, stroke));
            let head = Pos2::new(l, top);
            p.add(egui::Shape::convex_polygon(
                vec![
                    head,
                    Pos2::new(l + b.width() * 0.32, top - b.height() * 0.22),
                    Pos2::new(l + b.width() * 0.32, top + b.height() * 0.22),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::History => {
            let radius = b.width().min(b.height()) * 0.45;
            p.circle_stroke(c, radius, stroke);
            p.line_segment([c, Pos2::new(c.x, c.y - radius * 0.62)], stroke);
            p.line_segment([c, Pos2::new(c.x + radius * 0.5, c.y + radius * 0.18)], stroke);
        }
        Icon::Monitor => {
            let screen = Rect::from_min_max(Pos2::new(l, t), Pos2::new(r, bt - b.height() * 0.28));
            p.rect_stroke(screen, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            p.line_segment(
                [Pos2::new(c.x, screen.bottom()), Pos2::new(c.x, bt - w * 0.5)],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(l + b.width() * 0.25, bt - w * 0.5),
                    Pos2::new(r - b.width() * 0.25, bt - w * 0.5),
                ],
                stroke,
            );
        }
        Icon::Messages => {
            for k in [0.14, 0.5, 0.86] {
                let y = t + b.height() * k;
                p.circle_filled(Pos2::new(l + w, y), w, color);
                p.line_segment([Pos2::new(l + b.width() * 0.3, y), Pos2::new(r, y)], stroke);
            }
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
                (
                    Pos2::new(c.x, bt - b.height() * 0.35),
                    Pos2::new(c.x, t + w * 0.7),
                )
            } else {
                (Pos2::new(c.x, t), Pos2::new(c.x, bt - b.height() * 0.35))
            };
            p.line_segment([a, z], stroke);
            let dir = Vec2::new(0.0, if up { -1.0 } else { 1.0 });
            // The point's corner reaches a little past the tip.
            head(p, z, dir, b.width() * 0.26, stroke);
        }
        Icon::Diagram => {
            // A block low on the left, one high on the right, a wire
            // stepping between them.
            let side = b.width() * 0.36;
            let a = Rect::from_min_size(Pos2::new(l, bt - side), Vec2::splat(side));
            let z = Rect::from_min_size(Pos2::new(r - side, t), Vec2::splat(side));
            for q in [a, z] {
                p.rect_stroke(q, CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
            }
            let mid = c.x;
            p.add(egui::Shape::line(
                vec![
                    Pos2::new(a.right(), a.center().y),
                    Pos2::new(mid, a.center().y),
                    Pos2::new(mid, z.center().y),
                    Pos2::new(z.left(), z.center().y),
                ],
                stroke,
            ));
        }
        Icon::List => {
            for k in [0.14, 0.5, 0.86] {
                let y = t + b.height() * k;
                p.line_segment([Pos2::new(l, y), Pos2::new(r, y)], stroke);
            }
        }
        Icon::Grid => {
            let side = b.width() * 0.4;
            for (x, y) in [(l, t), (r - side, t), (l, bt - side), (r - side, bt - side)] {
                let q = Rect::from_min_size(Pos2::new(x, y), Vec2::splat(side));
                p.rect_stroke(q, CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
            }
        }
        Icon::Pan => {
            // A cross of two shafts, an arrowhead at each of its four ends.
            let inset = w * 0.7;
            let size = b.width() * 0.17;
            p.line_segment([Pos2::new(l + inset, c.y), Pos2::new(r - inset, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, t + inset), Pos2::new(c.x, bt - inset)], stroke);
            head(p, Pos2::new(r - inset, c.y), Vec2::new(1.0, 0.0), size, stroke);
            head(p, Pos2::new(l + inset, c.y), Vec2::new(-1.0, 0.0), size, stroke);
            head(p, Pos2::new(c.x, t + inset), Vec2::new(0.0, -1.0), size, stroke);
            head(p, Pos2::new(c.x, bt - inset), Vec2::new(0.0, 1.0), size, stroke);
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            // A lens up and to the left, its handle down and to the right.
            let radius = b.width() * 0.34;
            let lens = Pos2::new(l + radius, t + radius);
            p.circle_stroke(lens, radius, stroke);
            let reach = radius * std::f32::consts::FRAC_1_SQRT_2;
            p.line_segment([lens + Vec2::splat(reach), Pos2::new(r, bt)], stroke);
            let bar = radius * 0.55;
            p.line_segment([lens - Vec2::new(bar, 0.0), lens + Vec2::new(bar, 0.0)], stroke);
            if icon == Icon::ZoomIn {
                p.line_segment([lens - Vec2::new(0.0, bar), lens + Vec2::new(0.0, bar)], stroke);
            }
        }
        Icon::Back | Icon::Forward => {
            // A shaft across the middle, an open arrowhead at one end.
            let inset = w * 0.7;
            let (from, to, dir) = if icon == Icon::Back {
                (r, l + inset, -1.0)
            } else {
                (l, r - inset, 1.0)
            };
            p.line_segment([Pos2::new(from, c.y), Pos2::new(to, c.y)], stroke);
            head(
                p,
                Pos2::new(to, c.y),
                Vec2::new(dir, 0.0),
                b.width() * 0.36,
                stroke,
            );
        }
        Icon::Find => {
            // A lens and its handle, down to the right.
            let rad = b.width() * 0.32;
            let at = Pos2::new(l + rad + w * 0.5, t + rad + w * 0.5);
            p.circle_stroke(at, rad, stroke);
            let d = rad * std::f32::consts::FRAC_1_SQRT_2;
            p.line_segment(
                [Pos2::new(at.x + d, at.y + d), Pos2::new(r, bt)],
                Stroke::new(w * 1.3, color),
            );
        }
        Icon::Deploy => {
            // A shaft from the left edge, an arrowhead at the right.
            let shaft = Stroke::new(w * 1.3, color);
            let tip = Pos2::new(r - w * 0.9, c.y);
            p.line_segment([Pos2::new(l, c.y), tip], shaft);
            head(
                p,
                tip,
                Vec2::new(1.0, 0.0),
                b.height().min(b.width()) * 0.36,
                shaft,
            );
        }
        Icon::Wrap => {
            // A full line, a second that turns down and back with an
            // arrowhead, and the short start of a third.
            let (y1, y2, y3) = (t + b.height() * 0.12, c.y, bt - b.height() * 0.12);
            let turn = r - w * 0.5;
            let back = c.x + w * 0.7;
            p.line_segment([Pos2::new(l, y1), Pos2::new(r, y1)], stroke);
            p.add(egui::Shape::line(
                vec![
                    Pos2::new(l, y2),
                    Pos2::new(turn, y2),
                    Pos2::new(turn, y3),
                    Pos2::new(back, y3),
                ],
                stroke,
            ));
            head(
                p,
                Pos2::new(back, y3),
                Vec2::new(-1.0, 0.0),
                b.width() * 0.16,
                stroke,
            );
            p.line_segment([Pos2::new(l, y3), Pos2::new(l + b.width() * 0.26, y3)], stroke);
        }
        Icon::User => {
            // A round head over rounded shoulders.
            let head = b.width() * 0.2;
            p.circle_filled(Pos2::new(c.x, t + b.height() * 0.3), head, color);
            let radius = b.width() * 0.36;
            let base = bt - b.height() * 0.04;
            let n = 16;
            let mut body: Vec<Pos2> = (0..=n)
                .map(|k| {
                    #[allow(clippy::cast_precision_loss)]
                    let a = std::f32::consts::PI * (k as f32 / n as f32);
                    Pos2::new(c.x - radius * a.cos(), base - radius * 0.9 * a.sin())
                })
                .collect();
            body.push(Pos2::new(c.x + radius, base));
            p.add(egui::Shape::convex_polygon(body, color, Stroke::NONE));
        }
        Icon::Code => {
            // Two angle brackets, each a chevron a third of the width.
            // Apart, or the pair reads as a diamond.
            let dx = b.width() * 0.26;
            let dy = b.height() * 0.38;
            let la = l + w * 0.7;
            let ra = r - w * 0.7;
            p.add(egui::Shape::line(
                vec![
                    Pos2::new(la + dx, c.y - dy),
                    Pos2::new(la, c.y),
                    Pos2::new(la + dx, c.y + dy),
                ],
                stroke,
            ));
            p.add(egui::Shape::line(
                vec![
                    Pos2::new(ra - dx, c.y - dy),
                    Pos2::new(ra, c.y),
                    Pos2::new(ra - dx, c.y + dy),
                ],
                stroke,
            ));
        }
        Icon::Info => {
            p.circle_stroke(c, b.width() * 0.5 - w * 0.5, stroke);
            p.circle_filled(Pos2::new(c.x, t + b.height() * 0.28), w * 0.75, color);
            p.line_segment(
                [
                    Pos2::new(c.x, t + b.height() * 0.45),
                    Pos2::new(c.x, bt - b.height() * 0.22),
                ],
                stroke,
            );
        }
        Icon::Warning => {
            let inset = w * 0.6;
            p.add(egui::Shape::closed_line(
                vec![
                    Pos2::new(c.x, t + inset),
                    Pos2::new(r - inset, bt - inset),
                    Pos2::new(l + inset, bt - inset),
                ],
                stroke,
            ));
            p.line_segment(
                [
                    Pos2::new(c.x, t + b.height() * 0.38),
                    Pos2::new(c.x, t + b.height() * 0.66),
                ],
                stroke,
            );
            p.circle_filled(Pos2::new(c.x, t + b.height() * 0.8), w * 0.7, color);
        }
        Icon::Copy => {
            let k = b.width() * 0.62;
            let front = Rect::from_min_size(Pos2::new(r - k, bt - k), Vec2::splat(k));
            // The sheet behind shows only what the front one leaves.
            p.add(egui::Shape::line(
                vec![
                    Pos2::new(l + k, bt - k),
                    Pos2::new(l + k, t),
                    Pos2::new(l, t),
                    Pos2::new(l, t + k),
                    Pos2::new(r - k, t + k),
                ],
                stroke,
            ));
            p.rect_stroke(front, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
        }
        Icon::Clear => {
            // A bin: the lid and its handle over a tapering body.
            let lid = t + b.height() * 0.22;
            p.line_segment([Pos2::new(l, lid), Pos2::new(r, lid)], stroke);
            p.line_segment(
                [
                    Pos2::new(c.x - b.width() * 0.18, t + w * 0.5),
                    Pos2::new(c.x + b.width() * 0.18, t + w * 0.5),
                ],
                stroke,
            );
            p.add(egui::Shape::line(
                vec![
                    Pos2::new(l + b.width() * 0.14, lid),
                    Pos2::new(l + b.width() * 0.22, bt - w * 0.5),
                    Pos2::new(r - b.width() * 0.22, bt - w * 0.5),
                    Pos2::new(r - b.width() * 0.14, lid),
                ],
                stroke,
            ));
        }
        Icon::Error => {
            p.circle_stroke(c, b.width() * 0.5 - w * 0.5, stroke);
            let k = b.width() * 0.2;
            p.line_segment([c + Vec2::new(-k, -k), c + Vec2::new(k, k)], stroke);
            p.line_segment([c + Vec2::new(-k, k), c + Vec2::new(k, -k)], stroke);
        }
        Icon::Help => {
            p.circle_stroke(c, b.width() * 0.5 - w * 0.5, stroke);
            p.text(
                c + Vec2::new(0.0, -w * 0.2),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional((b.height() * 0.78).max(crate::theme::MIN_TEXT)),
                color,
            );
        }
    }
}

/// The brand's stencil encoder mark (docs/brand, the `se` symbol: a rim cut
/// by eight bridges, a hub ring cut by four, the dot, eight detached teeth)
/// drawn in `rect`'s square in `color`. The SVG masks the cuts out; here the
/// rim and the hub ring are arcs that stop short of them.
pub fn paint_mark(p: &egui::Painter, rect: Rect, color: Color32) {
    let u = rect.width().min(rect.height()) / 96.0;
    let c = rect.center();
    let at = |deg: f32, radius: f32| {
        let a = deg.to_radians();
        c + Vec2::new(a.sin(), -a.cos()) * radius * u
    };
    // Arcs of a ring at `radius`, one between each pair of cuts: the cuts
    // sit at `first` degrees and every `step` after, and an arc stops
    // `gap` degrees short of each.
    let ring = |radius: f32, width: f32, first: f32, step: f32, gap: f32| {
        let mut from = first + gap;
        while from + step - 2.0 * gap <= 360.0 + first {
            let to = from + step - 2.0 * gap;
            let pts: Vec<Pos2> = (0..=6u8)
                .map(|i| at(from + (to - from) * f32::from(i) / 6.0, radius))
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(width * u, color)));
            from += step;
        }
    };
    ring(26.5, 9.0, 22.5, 45.0, 4.0);
    ring(11.0, 4.0, 45.0, 90.0, 7.0);
    p.circle_filled(c, 4.0 * u, color);
    for k in 0..8u8 {
        let a = (45.0 * f32::from(k)).to_radians();
        let tooth: Vec<Pos2> = [(-5.0, 33.0), (5.0, 33.0), (5.0, 43.0), (-5.0, 43.0)]
            .iter()
            .map(|&(x, y): &(f32, f32)| {
                // (x, -y) turned clockwise by the tooth's angle.
                c + Vec2::new(x * a.cos() + y * a.sin(), x * a.sin() - y * a.cos()) * u
            })
            .collect();
        p.add(egui::Shape::convex_polygon(tooth, color, Stroke::NONE));
    }
}
