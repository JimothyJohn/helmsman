//! Semantic zoom on the viewer: the camera's zoom picks how much of the
//! program is drawn as blocks ([`helmsman_diagram::fold::zoom`]).
//!
//! What the zoom closes is a function of the zoom level and of what the
//! reader did by hand, held apart from both: [`ZoomFolds::shut`] is
//! recomputed whenever the folds are drawn again, and nothing here reaches
//! `opened`, `collapsed` or the viewer's edit count, so the zoom never
//! writes to `diagram.layout`. The one thing kept besides the level is
//! `kept_open`: a fold the zoom had closed and the reader opened, which is
//! theirs from then on for this session (what a project's layout holds is
//! only what the reader folded: a fold left as its blocks is no layer).
//!
//! A child module of `st_view`, which it reaches into, so the viewer's own
//! file holds only the hooks.

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{Pos2, Rect};
use helmsman_diagram::NodeId;
use helmsman_diagram::fold::zoom::{self, Hand, Level};
use helmsman_diagram::fold::{Fold, FoldKind};

use super::{Cam, Viewer, on_request};

/// What the zoom and the reader's hand leave of the folds.
#[derive(Debug, Default)]
pub(super) struct ZoomFolds {
    /// The level the view is at.
    pub level: Level,
    /// Folds the zoom closes that the reader opened, as (POU, fold key).
    kept_open: BTreeSet<(String, String)>,
    /// The keys of the folds the zoom closes now, for each POU.
    shut: Vec<BTreeSet<String>>,
}

impl ZoomFolds {
    /// Whether the zoom closes fold `key` of POU number `pou`.
    pub fn is_shut(&self, pou: usize, key: &str) -> bool {
        self.shut.get(pou).is_some_and(|s| s.contains(key))
    }

    /// Whether the reader opened that fold over the zoom.
    pub fn kept(&self, pou: &str, key: &str) -> bool {
        self.kept_open.contains(&(pou.to_string(), key.to_string()))
    }

    /// Forgets what the reader opened (every fold as the program starts it).
    pub fn forget(&mut self) {
        self.kept_open.clear();
    }
}

/// A point of the diagram on screen, named by the statements under it so it
/// can be found again after the blocks are drawn differently.
struct Anchor {
    /// The blocks of the diagram (not of the view) of the top-level
    /// statements the thing under the point belongs to.
    nodes: Vec<NodeId>,
    /// Where the point is in what is drawn for them, as fractions of its size.
    at: (f32, f32),
    screen: Pos2,
}

impl Viewer {
    /// The level of detail the view draws now.
    #[cfg(test)]
    #[must_use]
    pub fn zoom_level(&self) -> Level {
        self.zoom.level
    }

    /// What the reader did to fold `f` of the POU named `pou`, if anything.
    fn hand_of(&self, pou: &str, f: &Fold) -> Option<Hand> {
        let entry = (pou.to_string(), f.key.clone());
        if on_request(f.kind) {
            if self.collapsed.contains(&entry) {
                Some(Hand::Closed)
            } else {
                self.zoom.kept_open.contains(&entry).then_some(Hand::Open)
            }
        } else {
            self.opened.contains(&entry).then_some(Hand::Open)
        }
    }

    /// Works out the folds the zoom closes, for every POU.
    pub(super) fn refresh_zoom_folds(&mut self) {
        let shut: Vec<BTreeSet<String>> = self
            .module
            .pous
            .iter()
            .zip(&self.folds)
            .map(|(p, folds)| zoom::closed(folds, self.zoom.level, &|f| self.hand_of(&p.name, f)))
            .collect();
        self.zoom.shut = shut;
    }

    /// Called as fold `key` of the POU shown is about to be drawn as its
    /// blocks (`open`) or as one block: a fold the zoom had closed and the
    /// reader opens stays open as the zoom changes.
    pub(super) fn zoom_note_hand(&mut self, key: &str, open: bool) {
        let pou = self.scopes[self.scope].pou;
        let Some(f) = self.folds[pou].iter().find(|f| f.key == key) else {
            return;
        };
        if !zoom::managed(f.kind) {
            return;
        }
        let entry = (self.module.pous[pou].name.clone(), key.to_string());
        if !open {
            self.zoom.kept_open.remove(&entry);
        } else if self.is_closed(pou, f) {
            self.zoom.kept_open.insert(entry);
        }
    }

    /// Whether the tab over an opened fold is drawn: a statement is a plain
    /// part of the diagram at the near level, outlined only once the reader
    /// opened it.
    pub(super) fn zoom_tab_wanted(&self, pou: usize, f: &Fold) -> bool {
        f.kind != FoldKind::Statement || self.zoom.kept(&self.module.pous[pou].name, &f.key)
    }

    /// Frames `rect` in `below` for a Q or E jump without leaving the level
    /// the view is at: the stop is shown, not what is inside it.
    pub(super) fn zoom_frame(&self, to: &mut Cam, below: Rect, rect: &helmsman_diagram::Rect) {
        match self.zoom.level.reach() {
            Some(max) => {
                to.frame_within(below, rect, max);
            }
            None => to.frame(below, rect),
        }
    }

    /// Fold `f` as it is drawn when closed: one the zoom closes keeps no
    /// variable as a block (the overview is one block per section, its
    /// pins carrying the values), one the reader folded is as they made it.
    pub(super) fn zoom_shape(&self, pou: usize, f: &Fold) -> Fold {
        let mut f = f.clone();
        if f.blocks && self.zoom.is_shut(pou, &f.key) {
            f.blocks = false;
        }
        f
    }

    /// Takes the zoom of the current scope's camera to its level, and when
    /// that changes the level draws the folds again and keeps the diagram
    /// under `pointer` (the middle of `area` without one) where it was.
    /// Returns the camera as it is after. Does nothing while a jump eases
    /// over (it ends at the level it began at) or before the first fit.
    pub(super) fn follow_zoom(&mut self, pointer: Option<Pos2>, area: Rect) -> Cam {
        let cam = self.cams[self.scope];
        if self.glide.is_some() || !cam.fitted {
            return cam;
        }
        let level = zoom::level(cam.zoom, self.zoom.level);
        if level == self.zoom.level {
            return cam;
        }
        let screen = pointer
            .filter(|p| area.contains(*p))
            .unwrap_or_else(|| area.center());
        let anchor = self.anchor_at(cam, screen);
        self.zoom.level = level;
        self.reshow();
        if let Some(a) = anchor {
            self.keep_anchor(&a, area);
        }
        self.cams[self.scope]
    }

    /// What is at screen point `p` on the diagram as drawn: the block under
    /// it, or the one nearest.
    fn anchor_at(&self, cam: Cam, p: Pos2) -> Option<Anchor> {
        let (x, y) = cam.to_world(p);
        let d = self.shown();
        let dist = |n: &helmsman_diagram::Node| {
            let r = &n.rect;
            let dx = (r.x - x).max(x - r.right()).max(0.0);
            let dy = (r.y - y).max(y - r.bottom()).max(0.0);
            dx * dx + dy * dy
        };
        let near = d.nodes.iter().min_by(|a, b| dist(a).total_cmp(&dist(b)))?;
        let held: Vec<NodeId> = match self.fold_shown(near.id) {
            Some(f) => f.nodes.iter().copied().collect(),
            None => self.to_base(near.id).into_iter().collect(),
        };
        // The point's place is kept in the band of the top-level statements
        // the block belongs to: whatever the zoom draws for a statement is
        // drawn within that band, a block, a line or its fold's block.
        let base = self.diagram();
        let top = top_statements(base);
        let stmt_of: HashMap<_, _> = base.stmts.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        let band: BTreeSet<usize> = held
            .iter()
            .filter_map(|&n| stmt_of.get(&base.node(n).stmt).map(|&s| top[s]))
            .collect();
        let nodes: Vec<NodeId> = base
            .nodes
            .iter()
            .filter(|n| stmt_of.get(&n.stmt).is_some_and(|&s| band.contains(&top[s])))
            .map(|n| n.id)
            .collect();
        let r = self.drawn_bounds(&nodes)?;
        Some(Anchor {
            nodes,
            at: ((x - r.x) / r.w.max(1.0), (y - r.y) / r.h.max(1.0)),
            screen: p,
        })
    }

    /// Moves the camera so the point `a` names is at the screen point it
    /// was at, on the diagram as it is drawn now.
    fn keep_anchor(&mut self, a: &Anchor, area: Rect) {
        let Some(b) = self.drawn_bounds(&a.nodes) else {
            return;
        };
        let size = self.shown().size;
        let (wx, wy) = (b.x + a.at.0 * b.w, b.y + a.at.1 * b.h);
        let cam = &mut self.cams[self.scope];
        cam.offset = a.screen - cam.origin - eframe::egui::Vec2::new(wx, wy) * cam.zoom;
        cam.clamp(area, size);
    }

    /// The bounds of what is drawn for blocks `nodes` of the diagram.
    fn drawn_bounds(&self, nodes: &[NodeId]) -> Option<helmsman_diagram::Rect> {
        let d = self.shown();
        nodes
            .iter()
            .filter_map(|&n| self.to_shown(n))
            .map(|s| d.node(s).rect)
            .reduce(|p, q| {
                let (x, y) = (p.x.min(q.x), p.y.min(q.y));
                helmsman_diagram::Rect {
                    x,
                    y,
                    w: p.right().max(q.right()) - x,
                    h: p.bottom().max(q.bottom()) - y,
                }
            })
    }
}

/// For each statement of `d`, the index of the top-level statement it is in
/// (itself, for one at the top).
fn top_statements(d: &helmsman_diagram::Diagram) -> Vec<usize> {
    let at: HashMap<_, _> = d.stmts.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    (0..d.stmts.len())
        .map(|mut s| {
            // The chain is as long as the nesting is deep, so this ends.
            for _ in 0..d.stmts.len() {
                let Some(&parent) = d.stmts[s].region.and_then(|r| at.get(&d.region(r).stmt)) else {
                    break;
                };
                s = parent;
            }
            s
        })
        .collect()
}

#[cfg(test)]
#[path = "st_zoom_tests.rs"]
mod tests;
