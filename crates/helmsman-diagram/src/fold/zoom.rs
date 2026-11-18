//! Which folds the zoom closes: semantic zoom, as pure functions.
//!
//! The camera's zoom picks one of three [`Level`]s of detail:
//!
//! - [`Level::Far`]: every section and arm is one block (its pins carry the
//!   live values and its lit state), and what is left outside them is one
//!   line per statement.
//! - [`Level::Middle`]: one line per statement ([`FoldKind::Statement`],
//!   titled with its text); sections are outlines around their lines.
//! - [`Level::Near`]: the blocks, as the program is drawn at rest.
//!
//! [`level`] picks the level for a zoom, with a dead band at each threshold
//! so a zoom that rests near one never flips between two levels. [`closed`]
//! says which folds the level closes, given what the reader did by hand:
//! a fold the reader opened or folded stays as they left it, whatever the
//! zoom, and the zoom never closes a fold that hides one the reader opened.
//! The caller keeps the hand state and writes none of this anywhere: the
//! zoom's folds are a function of the camera and are not part of a
//! project's layout.

use std::collections::BTreeSet;

use super::{Fold, FoldKind};

/// How much of the program is drawn as blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Sections and arms as one block each.
    Far,
    /// One line per statement.
    Middle,
    /// The blocks.
    #[default]
    Near,
}

/// Going out, [`Level::Middle`] gives way to [`Level::Far`] below this zoom.
pub const FAR_BELOW: f32 = 0.30;
/// Going in, [`Level::Far`] gives way to [`Level::Middle`] above this zoom.
pub const FAR_ABOVE: f32 = 0.36;
/// Going out, [`Level::Near`] gives way to [`Level::Middle`] below this zoom.
pub const NEAR_BELOW: f32 = 0.60;
/// Going in, [`Level::Middle`] gives way to [`Level::Near`] above this zoom.
/// Under the closest a fit or a jump frames at (0.75), so a view framed on
/// a block is always at [`Level::Near`] unless the jump is made at a
/// farther level (see [`Level::reach`]).
pub const NEAR_ABOVE: f32 = 0.72;

/// The level for `zoom` when the view was at `prev`: `prev` while the zoom
/// is inside the dead band of its thresholds, else the level the zoom is
/// in. A zoom that is not a number changes nothing.
#[must_use]
pub fn level(zoom: f32, prev: Level) -> Level {
    let mut now = prev;
    // At most two steps: a jump from one end to the other.
    for _ in 0..2 {
        now = match now {
            Level::Far if zoom > FAR_ABOVE => Level::Middle,
            Level::Middle if zoom > NEAR_ABOVE => Level::Near,
            Level::Middle if zoom < FAR_BELOW => Level::Far,
            Level::Near if zoom < NEAR_BELOW => Level::Middle,
            same => same,
        };
    }
    now
}

impl Level {
    /// The closest a jump framed at this level goes: a stop is framed
    /// without leaving the level, so a Q or E at [`Level::Far`] shows the
    /// next block, not what is inside it. `None` at [`Level::Near`]: the
    /// usual limit.
    #[must_use]
    pub const fn reach(self) -> Option<f32> {
        match self {
            Self::Far => Some(FAR_BELOW),
            Self::Middle => Some(f32::midpoint(NEAR_BELOW, NEAR_ABOVE) - 0.01),
            Self::Near => None,
        }
    }
}

/// The size, in screen points, a far block's title is drawn at whatever the
/// camera's zoom: the repo's body size (UI rule: 18 pt, never under 16).
pub const FAR_FLOOR_PT: f32 = 16.0;
/// The smallest zoom the camera goes to. The far level is laid out so the
/// title fits its block here, which is the worst case.
pub const FAR_MIN_ZOOM: f32 = 0.15;
/// A monospace character's advance, in ems (a little over, so a title that
/// is budgeted to fit does).
pub const FAR_CHAR_EM: f32 = 0.62;
/// The longest title a far block draws (more is elided).
pub const FAR_TITLE_CHARS: usize = 32;
/// Room kept each side of a far title, in screen points.
pub const FAR_PAD_PT: f32 = 8.0;

/// The room, in screen points, a far block's title of `chars` characters
/// needs: width, then height.
#[must_use]
pub fn far_room(chars: usize) -> (f32, f32) {
    let n = chars.clamp(1, FAR_TITLE_CHARS) as f32;
    (
        n * FAR_FLOOR_PT * FAR_CHAR_EM + 2.0 * FAR_PAD_PT,
        FAR_FLOOR_PT * 2.0,
    )
}

/// The characters of a title that fit `width` screen points at the floor
/// size, at most [`FAR_TITLE_CHARS`].
#[must_use]
pub fn far_chars(width: f32) -> usize {
    let room = (width - 2.0 * FAR_PAD_PT).max(0.0);
    // `room` is not negative and the count is capped: the cast is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = (room / (FAR_FLOOR_PT * FAR_CHAR_EM))
        .floor()
        .min(FAR_TITLE_CHARS as f32) as usize;
    n
}

/// How much the far level's layout is magnified, across and down, so every
/// title fits its block at [`FAR_MIN_ZOOM`]: the whole laid-out diagram is
/// scaled by it (gaps with the blocks, so nothing starts to touch). `blocks`
/// gives, per block that carries a title, its characters and its width and
/// height in diagram units as laid out. `(1, 1)` for none.
#[must_use]
pub fn far_scale(blocks: impl IntoIterator<Item = (usize, f32, f32)>) -> (f32, f32) {
    let (mut sx, mut sy) = (1.0_f32, 1.0_f32);
    for (chars, w, h) in blocks {
        let (need_w, need_h) = far_room(chars);
        if w > 0.0 {
            sx = sx.max(need_w / FAR_MIN_ZOOM / w);
        }
        if h > 0.0 {
            sy = sy.max(need_h / FAR_MIN_ZOOM / h);
        }
    }
    // A hair over, against rounding.
    let hair = |s: f32| if s > 1.0 { s * 1.01 } else { s };
    (hair(sx), hair(sy))
}

/// What the reader did to a fold by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    /// Drawn as its blocks.
    Open,
    /// Drawn as one block.
    Closed,
}

/// Whether the zoom manages folds of `kind`: the author's own structure,
/// which starts as its blocks and folds on request. The standard circuits
/// (a latch, a table, a comparison) are the same at every zoom.
#[must_use]
pub const fn managed(kind: FoldKind) -> bool {
    matches!(kind, FoldKind::Section | FoldKind::Arm | FoldKind::Statement)
}

/// Whether `level` closes folds of `kind`.
const fn closes(level: Level, kind: FoldKind) -> bool {
    match level {
        Level::Far => managed(kind),
        Level::Middle => matches!(kind, FoldKind::Statement),
        Level::Near => false,
    }
}

/// The keys of the folds `level` closes. `hand` says what the reader did
/// to a fold, if anything: such a fold is never closed by the zoom, and a
/// fold that holds one the reader opened is not either (it would hide
/// it). The order of `folds` does not matter here; it is
/// [`super::view`]'s to resolve folds that overlap.
#[must_use]
pub fn closed(folds: &[Fold], level: Level, hand: &dyn Fn(&Fold) -> Option<Hand>) -> BTreeSet<String> {
    if level == Level::Near {
        return BTreeSet::new();
    }
    let opened: Vec<&Fold> = folds.iter().filter(|f| hand(f) == Some(Hand::Open)).collect();
    folds
        .iter()
        .filter(|f| closes(level, f.kind) && hand(f).is_none())
        .filter(|f| {
            !opened
                .iter()
                .any(|h| h.key != f.key && !h.nodes.is_empty() && h.nodes.is_subset(&f.nodes))
        })
        .map(|f| f.key.clone())
        .collect()
}
