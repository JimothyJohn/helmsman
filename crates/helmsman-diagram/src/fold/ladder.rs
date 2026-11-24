//! The zoom ladder: how much of a program one block stands for, by how
//! far out the camera is (Nick, 2026-11-23).
//!
//! The rungs, closest first: [`Rung::Code`] (each statement as the text
//! that wrote it), [`Rung::Wiring`] (the blocks and wires, as the diagram
//! is laid out), [`Rung::Statements`] (one card per statement, named by
//! what it sets), [`Rung::Sections`] (one card per section, arm or
//! top-level statement) and [`Rung::Program`] (the POU as one card). The
//! closest is the most concrete and each step out says less about how and
//! more about what.
//!
//! Nothing is laid out again. A rung's [`Group`]s are sets of blocks of the
//! diagram as it is drawn, and a group's card is painted over the bounds
//! of its blocks, where they are. How strongly is a continuous function of
//! the zoom alone ([`card`], [`outline`]): a group is outlined faintly one
//! rung before it closes, the outline firms up as the zoom nears its band,
//! and the card fades in over the band. So there is no state to flip, a
//! zoom that rests anywhere draws the same thing every frame, and the
//! bands are far enough apart ([`STEP`]) that one turn of the wheel never
//! crosses two.

use std::collections::BTreeSet;

use super::structure;
use super::{Fold, FoldKind};
use crate::model::{Diagram, NodeId, NodeKind};

/// One rung of the ladder, closest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rung {
    /// Each statement as its source text.
    Code,
    /// The blocks and wires.
    Wiring,
    /// One card per statement.
    Statements,
    /// One card per section, arm or top-level statement.
    Sections,
    /// The POU as one card.
    Program,
}

/// The least the zoom changes between two rungs' bands, as a factor.
pub const STEP: f32 = 2.2;
/// The smallest zoom the camera goes to: [`Rung::Program`] with room.
pub const MIN_ZOOM: f32 = 0.05;
/// The largest zoom the camera goes to: [`Rung::Code`] with room.
pub const MAX_ZOOM: f32 = 6.0;
/// How strongly a group is outlined one rung before it starts to close.
pub const HINT: f32 = 0.22;

impl Rung {
    /// Every rung, closest first.
    pub const ALL: [Self; 5] = [
        Self::Code,
        Self::Wiring,
        Self::Statements,
        Self::Sections,
        Self::Program,
    ];

    /// What the rung is called on screen.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Wiring => "Wiring",
            Self::Statements => "Statements",
            Self::Sections => "Sections",
            Self::Program => "Program",
        }
    }

    /// The zoom at which the rung's cards start to form and the zoom at
    /// which they are whole. [`Rung::Code`] forms going in (the second is
    /// the larger), the rungs past [`Rung::Wiring`] going out; `None` for
    /// [`Rung::Wiring`], which is the diagram itself.
    #[must_use]
    pub const fn band(self) -> Option<(f32, f32)> {
        match self {
            Self::Code => Some((2.2, 3.0)),
            Self::Wiring => None,
            Self::Statements => Some((0.62, 0.45)),
            Self::Sections => Some((0.28, 0.20)),
            Self::Program => Some((0.125, 0.09)),
        }
    }

    /// A zoom well inside the rung: where a step to it rests.
    #[must_use]
    pub const fn rest(self) -> f32 {
        match self {
            Self::Code => 4.0,
            Self::Wiring => 1.0,
            Self::Statements => 0.36,
            Self::Sections => 0.16,
            Self::Program => 0.07,
        }
    }

    /// The zooms a view framed on something of this rung may rest at:
    /// inside the rung, clear of the bands either side.
    #[must_use]
    pub const fn span(self) -> (f32, f32) {
        match self {
            Self::Code => (3.2, MAX_ZOOM),
            Self::Wiring => (0.75, 2.0),
            Self::Statements => (0.30, 0.42),
            Self::Sections => (0.14, 0.19),
            Self::Program => (MIN_ZOOM, 0.085),
        }
    }

    /// The rung one step in (closer), if any.
    #[must_use]
    pub fn closer(self) -> Option<Self> {
        let k = Self::ALL.iter().position(|&r| r == self)?;
        k.checked_sub(1).map(|k| Self::ALL[k])
    }

    /// The rung one step out (farther), if any.
    #[must_use]
    pub fn farther(self) -> Option<Self> {
        let k = Self::ALL.iter().position(|&r| r == self)?;
        Self::ALL.get(k + 1).copied()
    }
}

/// 0 at `from`, 1 at `to`, eased, whichever of the two is larger. A zoom
/// that is not a number is 0.
fn ease(zoom: f32, from: f32, to: f32) -> f32 {
    if !zoom.is_finite() || (to - from).abs() < f32::EPSILON {
        return 0.0;
    }
    // Zoom is a ratio: ease along its logarithm, so the fade takes the
    // same turn of the wheel at every rung.
    let s = ((zoom.max(1e-6).ln() - from.ln()) / (to.ln() - from.ln())).clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

/// How formed `rung`'s cards are at `zoom`: 0 (the blocks under them show)
/// to 1 (the cards alone). Always 0 for [`Rung::Wiring`].
#[must_use]
pub fn card(rung: Rung, zoom: f32) -> f32 {
    rung.band().map_or(0.0, |(start, whole)| ease(zoom, start, whole))
}

/// How strongly `rung`'s groups are outlined at `zoom`, 0 to 1: nothing
/// until the rung before it has started to form, then [`HINT`], firming up
/// over the approach to its band and whole where its cards are.
#[must_use]
pub fn outline(rung: Rung, zoom: f32) -> f32 {
    let Some((start, whole)) = rung.band() else {
        return 0.0;
    };
    if !zoom.is_finite() {
        return 0.0;
    }
    // The zoom this rung's outlines appear at: where the rung between it
    // and the wiring starts to form (statements are outlined at every
    // zoom between their two bands: they are the next step both ways).
    let shown = match rung {
        Rung::Code | Rung::Statements | Rung::Wiring => 1.0,
        Rung::Sections => ease(zoom, 0.62, 0.62 / 1.15),
        Rung::Program => ease(zoom, 0.28, 0.28 / 1.15),
    };
    // The approach is the last part of the way to the band.
    let approach = if whole > start { start / 1.5 } else { start * 1.5 };
    shown * (HINT + (1.0 - HINT) * ease(zoom, approach, whole))
}

/// The rung a view at `zoom` is on: the farthest whose cards are at least
/// half formed, [`Rung::Wiring`] between the bands.
#[must_use]
pub fn rung_at(zoom: f32) -> Rung {
    if card(Rung::Code, zoom) >= 0.5 {
        return Rung::Code;
    }
    [Rung::Program, Rung::Sections, Rung::Statements]
        .into_iter()
        .find(|&r| card(r, zoom) >= 0.5)
        .unwrap_or(Rung::Wiring)
}

/// Blocks one card stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// Its name across versions of the source (a fold's key, or `program`).
    pub key: String,
    /// What the card says.
    pub title: String,
    /// The blocks it stands for.
    pub nodes: BTreeSet<NodeId>,
    /// The source it stands for, as written: the statement of a
    /// [`Rung::Code`] or [`Rung::Statements`] group. Empty for any other.
    pub code: String,
}

/// The longest card title, in characters.
pub const TITLE: usize = 40;

fn cut(text: &str) -> String {
    if text.chars().count() <= TITLE {
        text.to_string()
    } else {
        text.chars().take(TITLE - 1).chain(Some('…')).collect()
    }
}

/// What a statement is called one rung above its wiring: the variables it
/// sets, or the function block it calls, and its own text when it does
/// neither.
fn says(d: &Diagram, f: &Fold) -> String {
    let mut names: Vec<String> = Vec::new();
    for &n in &f.nodes {
        let name = match &d.node(n).kind {
            NodeKind::Write { path } => path.to_ascii_lowercase(),
            NodeKind::Fb { instance, .. } => instance.to_ascii_lowercase(),
            _ => continue,
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        cut(&f.title)
    } else {
        cut(&names.join(", "))
    }
}

/// The statements of `d` that hold no other statement, as groups.
fn leaves(d: &Diagram, src: &str) -> Vec<Group> {
    let all = structure::statements(d, src);
    let code = structure::sources(d, src);
    all.iter()
        .filter(|f| {
            !all.iter()
                .any(|g| g.key != f.key && g.nodes.is_subset(&f.nodes) && g.nodes.len() < f.nodes.len())
        })
        .map(|f| Group {
            key: f.key.clone(),
            title: says(d, f),
            nodes: f.nodes.clone(),
            code: f.stmts.first().and_then(|s| code.get(s)).cloned().unwrap_or_default(),
        })
        .collect()
}

/// The groups of `rung` for `d`, a POU of the module whose source is
/// `src`. No two share a block. None for [`Rung::Wiring`], and none for a
/// source that does not parse except the program's own.
#[must_use]
pub fn groups(d: &Diagram, src: &str, rung: Rung) -> Vec<Group> {
    match rung {
        Rung::Wiring => Vec::new(),
        Rung::Code | Rung::Statements => leaves(d, src),
        Rung::Sections => {
            let mut out: Vec<Group> = Vec::new();
            let mut taken: BTreeSet<NodeId> = BTreeSet::new();
            let whole: Vec<Fold> = structure::statements(d, src)
                .into_iter()
                .filter(|f| f.kind == FoldKind::Statement && !f.key.contains('/'))
                .collect();
            // The author's sections first, then the arms outside them,
            // then what is left of the top level, statement by statement.
            let folds = structure::sections(d, src)
                .into_iter()
                .chain(structure::arms(d))
                .chain(whole);
            for f in folds {
                if f.nodes.is_empty() || !f.nodes.is_disjoint(&taken) {
                    continue;
                }
                taken.extend(f.nodes.iter().copied());
                let title = if f.kind == FoldKind::Statement {
                    says(d, &f)
                } else {
                    cut(&f.title)
                };
                out.push(Group {
                    key: f.key,
                    title,
                    nodes: f.nodes,
                    code: String::new(),
                });
            }
            // Statements inside an IF whose arms were too small to fold.
            for g in leaves(d, src) {
                if g.nodes.is_disjoint(&taken) {
                    taken.extend(g.nodes.iter().copied());
                    out.push(Group {
                        code: String::new(),
                        ..g
                    });
                }
            }
            out
        }
        Rung::Program => {
            let nodes: BTreeSet<NodeId> = d.nodes.iter().map(|n| n.id).collect();
            if nodes.is_empty() {
                return Vec::new();
            }
            vec![Group {
                key: "program".into(),
                title: cut(&d.pou.to_ascii_lowercase()),
                nodes,
                code: String::new(),
            }]
        }
    }
}
