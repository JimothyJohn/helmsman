//! The zoom ladder on the viewer ([`helmsman_diagram::fold::ladder`]):
//! the farther out the camera, the more of the program one card stands
//! for, and all the way in a statement is the text that wrote it.
//!
//! Nothing is laid out again and nothing is kept: each frame the cards
//! are the bounds of their blocks as drawn, painted over them as strongly
//! as the zoom says. A group is outlined faintly before its card forms,
//! so it shows what will close together. A card that has formed takes the
//! pointer from the blocks under it: a click flies in to what it holds.
//!
//! A child module of `st_view`, which it reaches into.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};
use helmsman_diagram::fold::ladder::{self, Group, Rung};
use helmsman_diagram::{Diagram, NodeKind};
use helmsman_iec::Value;

use super::{Cam, GLYPH_BAND, Glide, Viewer, fitted, show, wire_color};
use crate::theme::Theme;

/// Room between a group's blocks and its card's edge, diagram units.
const PAD: f32 = 8.0;
/// A card at least this formed takes the pointer.
const SOLID: f32 = 0.5;
/// The size a card's title is drawn at, screen points, whatever the zoom.
const TITLE_PT: f32 = crate::theme::MIN_TEXT;
/// The largest a statement's text is drawn, diagram units.
const CODE_MAX: f32 = 14.0;
/// A monospace character's advance, in ems (a little over).
const CHAR_EM: f32 = 0.62;
/// How wide the rung's name is in the corner controls, points.
pub(super) const NAME_W: f32 = 124.0;

/// The groups of one POU, by rung.
#[derive(Debug, Default, Clone)]
pub(super) struct Groups {
    statements: Vec<Group>,
    sections: Vec<Group>,
    program: Vec<Group>,
}

impl Groups {
    pub fn of(d: &Diagram, src: &str) -> Self {
        Self {
            statements: ladder::groups(d, src, Rung::Statements),
            sections: ladder::groups(d, src, Rung::Sections),
            program: ladder::groups(d, src, Rung::Program),
        }
    }

    fn at(&self, rung: Rung) -> &[Group] {
        match rung {
            Rung::Code | Rung::Statements => &self.statements,
            Rung::Sections => &self.sections,
            Rung::Program => &self.program,
            Rung::Wiring => &[],
        }
    }
}

/// One group as a frame draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub rung: Rung,
    /// Its place among its rung's groups.
    pub index: usize,
    pub title: String,
    /// Its blocks' bounds, in diagram units.
    pub world: helmsman_diagram::Rect,
    /// The same on screen.
    pub rect: Rect,
    /// How strongly it is outlined, 0 to 1.
    pub outline: f32,
    /// How formed the card is, 0 to 1.
    pub card: f32,
    /// How formed its text is ([`Rung::Code`] only), 0 to 1.
    pub code: f32,
}

impl Card {
    /// Whether the card takes the pointer from the blocks under it.
    #[must_use]
    pub fn solid(&self) -> bool {
        self.card >= SOLID || self.code >= SOLID
    }
}

impl Viewer {
    /// The rung the view is on.
    #[must_use]
    pub fn rung(&self) -> Rung {
        ladder::rung_at(self.cams[self.scope].zoom)
    }

    /// The bounds of `g`'s blocks as drawn, padded.
    fn group_bounds(&self, g: &Group) -> Option<helmsman_diagram::Rect> {
        let d = self.shown();
        let r = g
            .nodes
            .iter()
            .filter_map(|&n| self.to_shown(n))
            .filter_map(|s| d.nodes.get(s.0 as usize))
            .map(|n| n.rect)
            .reduce(|p, q| {
                let (x, y) = (p.x.min(q.x), p.y.min(q.y));
                helmsman_diagram::Rect {
                    x,
                    y,
                    w: p.right().max(q.right()) - x,
                    h: p.bottom().max(q.bottom()) - y,
                }
            })?;
        Some(helmsman_diagram::Rect {
            x: r.x - PAD,
            y: r.y - PAD,
            w: r.w + 2.0 * PAD,
            h: r.h + 2.0 * PAD,
        })
    }

    /// What the ladder draws with the camera at `cam`, the nearest rung's
    /// cards first: every group that is outlined or formed at all.
    #[must_use]
    pub fn ladder_cards(&self, cam: Cam) -> Vec<Card> {
        let Some(groups) = self.ladder.get(self.scopes[self.scope].pou) else {
            return Vec::new();
        };
        let z = cam.zoom;
        let mut out = Vec::new();
        for rung in [Rung::Statements, Rung::Sections, Rung::Program] {
            // A statement is the next step both ways: its outline firms up
            // toward its text going in and toward its card going out.
            let code = if rung == Rung::Statements {
                ladder::card(Rung::Code, z)
            } else {
                0.0
            };
            let outline = if rung == Rung::Statements {
                ladder::outline(rung, z).max(ladder::outline(Rung::Code, z))
            } else {
                ladder::outline(rung, z)
            };
            let card = ladder::card(rung, z);
            if outline <= 0.0 && card <= 0.0 && code <= 0.0 {
                continue;
            }
            // Under a rung that has wholly formed nothing of this one shows.
            if rung.farther().is_some_and(|f| ladder::card(f, z) >= 1.0) {
                continue;
            }
            for (index, g) in groups.at(rung).iter().enumerate() {
                let Some(world) = self.group_bounds(g) else {
                    continue;
                };
                out.push(Card {
                    rung: if code > 0.0 { Rung::Code } else { rung },
                    index,
                    title: g.title.clone(),
                    world,
                    rect: cam.rect(&world),
                    outline,
                    card,
                    code,
                });
            }
        }
        out
    }

    /// The formed card under screen point `p`: the farthest rung's.
    pub(super) fn ladder_at(&self, cam: Cam, p: Pos2) -> Option<Card> {
        self.ladder_cards(cam)
            .into_iter()
            .rev()
            .find(|c| c.solid() && c.rect.contains(p))
    }

    fn ladder_group(&self, c: &Card) -> Option<&Group> {
        self.ladder
            .get(self.scopes[self.scope].pou)?
            .at(c.rung)
            .get(c.index)
    }

    /// A click on formed card `c`: a statement's text selects its blocks,
    /// any other card flies in one rung to what it holds.
    pub(super) fn ladder_click(&mut self, c: &Card, area: Rect, now: f64) {
        if c.rung == Rung::Code {
            if let Some(nodes) = self.ladder_group(c).map(|g| g.nodes.clone()) {
                self.select(nodes, false);
            }
            return;
        }
        let Some(to) = c.rung.closer() else { return };
        self.ladder_fly(to, Some(&c.world), area, now);
    }

    /// Eases the view to `rung`, framing `on` (the middle of the view
    /// without one) as large as the rung allows.
    pub(super) fn ladder_fly(&mut self, rung: Rung, on: Option<&helmsman_diagram::Rect>, area: Rect, now: f64) {
        let cam = self.cams[self.scope];
        let below = Rect::from_min_max(area.min + Vec2::new(0.0, GLYPH_BAND), area.max);
        let (lo, hi) = rung.span();
        let (zoom, middle) = match on {
            Some(r) => (
                (below.width() / r.w.max(1.0))
                    .min(below.height() / r.h.max(1.0))
                    .clamp(lo, hi),
                Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0),
            ),
            None => {
                let (x, y) = cam.to_world(below.center());
                (rung.rest(), Vec2::new(x, y))
            }
        };
        let mut to = cam;
        to.zoom = zoom;
        to.offset = below.center() - cam.origin - middle * zoom;
        to.clamp(area, self.shown().size);
        self.cams[self.scope].moved = true;
        self.glide = Some(Glide {
            scope: self.scope,
            from: (cam.offset, cam.zoom),
            to: (to.offset, to.zoom),
            t0: now,
        });
    }

    /// The value statement card `c` shows beside its name: what its one
    /// assignment last wrote. `None` for a card of several, or of none.
    fn ladder_value(&self, c: &Card, live: Option<&helmsman_diagram::live::Live>) -> Option<Value> {
        if !matches!(c.rung, Rung::Statements | Rung::Code) {
            return None;
        }
        let g = self.ladder_group(c)?;
        let base = self.diagram();
        let mut writes = g
            .nodes
            .iter()
            .filter(|&&n| matches!(base.node(n).kind, NodeKind::Write { .. }));
        let (one, None) = (writes.next()?, writes.next()) else {
            return None;
        };
        let shown = self.to_shown(*one)?;
        let d = self.shown();
        let w = d.wires.iter().position(|w| w.to == shown)?;
        live?.input(d, w)
    }

    /// Paints the ladder over the diagram: outlines, then cards.
    pub(super) fn ladder_paint(
        &self,
        painter: &egui::Painter,
        t: &Theme,
        cam: Cam,
        live: Option<&helmsman_diagram::live::Live>,
    ) {
        let cards = self.ladder_cards(cam);
        let clip = painter.clip_rect();
        for c in cards.iter().filter(|c| c.rect.intersects(clip)) {
            let round = CornerRadius::same(6);
            if c.card > 0.0 || c.code > 0.0 {
                painter.rect_filled(c.rect, round, t.panel.gamma_multiply(c.card.max(c.code)));
            }
            if c.outline > 0.0 {
                painter.rect_stroke(
                    c.rect,
                    round,
                    Stroke::new(1.0 + 1.5 * c.outline, t.fg.gamma_multiply(0.75 * c.outline)),
                    StrokeKind::Inside,
                );
            }
            if c.code > 0.0 {
                self.paint_code(painter, t, c, cam.zoom);
            }
            if c.card > 0.0 {
                let value = self.ladder_value(c, live);
                paint_title(painter, t, c, value);
            }
        }
    }

    /// A statement's text in its card, as large as fits.
    fn paint_code(&self, painter: &egui::Painter, t: &Theme, c: &Card, z: f32) {
        let Some(code) = self.ladder_group(c).map(|g| g.code.as_str()).filter(|s| !s.is_empty()) else {
            return;
        };
        let lines: Vec<&str> = code.lines().collect();
        let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1).max(1);
        #[allow(clippy::cast_precision_loss)]
        let (rows, cols) = (lines.len().max(1) as f32, cols as f32);
        let room = c.rect.shrink(PAD * z);
        let size = (room.height() / (rows * 1.3))
            .min(room.width() / (cols * CHAR_EM))
            .min(CODE_MAX * z);
        if size < 6.0 {
            return;
        }
        painter.text(
            room.left_center(),
            Align2::LEFT_CENTER,
            code,
            FontId::monospace(size),
            t.fg.gamma_multiply(c.code),
        );
    }
}

/// A card's name, and for a statement what it last wrote, at the size
/// the app's text is held to: nothing when the card has no room for it.
fn paint_title(painter: &egui::Painter, t: &Theme, c: &Card, value: Option<Value>) {
    let room = c.rect.shrink2(Vec2::new(8.0, 2.0));
    if room.height() < TITLE_PT * 1.2 || room.width() < TITLE_PT * 2.0 {
        return;
    }
    let font = FontId::monospace(TITLE_PT);
    let mut width = room.width();
    if let Some(v) = value {
        let (color, _) = wire_color(t, Some(v));
        let g = fitted(painter, &show(Some(v)), font.clone(), color.gamma_multiply(c.card), width / 2.0);
        width -= g.size().x + 8.0;
        painter.galley(
            Pos2::new(room.right() - g.size().x, room.center().y - g.size().y / 2.0),
            g,
            Color32::PLACEHOLDER,
        );
    }
    let g = fitted(painter, &c.title, font, t.fg.gamma_multiply(c.card), width);
    painter.galley(
        Pos2::new(room.left(), room.center().y - g.size().y / 2.0),
        g,
        Color32::PLACEHOLDER,
    );
}

#[cfg(test)]
#[path = "st_ladder_tests.rs"]
mod tests;
