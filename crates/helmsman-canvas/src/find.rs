//! Find in the Structured Text (Ctrl F, Ctrl G next, Ctrl Shift G
//! previous): the box over the ST pane, the matches marked like a
//! diagnostic, the current one selected in the editor and scrolled to.
//! Letters match whatever their case (ST is case-insensitive).

use std::ops::Range;

use eframe::egui::{
    self, Color32, Key, RichText,
    text::{ByteIndex, LayoutJob},
};

use crate::tips::Tip;

/// The find box's state: what is looked for and which match is current.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Find {
    pub query: String,
    /// The current match, an index into [`matches`].
    pub current: usize,
    /// Matches to move by when next drawn (Ctrl G, Ctrl Shift G).
    step: isize,
    /// The box takes the keyboard when next drawn (Ctrl F).
    focus: bool,
    /// The current match is to be selected and scrolled to.
    reveal: bool,
}

impl Find {
    /// Ctrl F: the box opens (or, open, takes the keyboard again).
    pub fn open(find: &mut Option<Self>) {
        let f = find.get_or_insert_with(Self::default);
        f.focus = true;
        f.reveal = true;
    }

    /// Ctrl G (`by` 1) or Ctrl Shift G (`by` -1); opens the box if shut.
    pub fn step(find: &mut Option<Self>, by: isize) {
        match find {
            Some(f) => {
                f.step += by;
                f.reveal = true;
            }
            None => Self::open(find),
        }
    }
}

/// Where `query` occurs in `text`, as byte ranges, left to right, none
/// overlapping. ASCII letters match in either case; anything else
/// exactly. An empty query matches nothing.
#[must_use]
pub fn matches(text: &str, query: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let n = query.len();
    let mut i = 0;
    while i + n <= text.len() {
        if text.is_char_boundary(i)
            && text.is_char_boundary(i + n)
            && text.as_bytes()[i..i + n].eq_ignore_ascii_case(query.as_bytes())
        {
            out.push(i..i + n);
            i += n;
        } else {
            i += 1;
        }
    }
    out
}

/// `job` with `ranges` (byte ranges of its text) on `ground`: sections
/// split where a range starts or ends, the text unchanged.
pub fn mark(job: &mut LayoutJob, ranges: &[(Range<usize>, Color32)]) {
    if ranges.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(job.sections.len() + 2 * ranges.len());
    for s in std::mem::take(&mut job.sections) {
        let (lo, hi) = (s.byte_range.start.0, s.byte_range.end.0);
        let mut cuts = vec![lo, hi];
        for (r, _) in ranges {
            for b in [r.start, r.end] {
                if lo < b && b < hi && job.text.is_char_boundary(b) {
                    cuts.push(b);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for (k, w) in cuts.windows(2).enumerate() {
            let mut piece = s.clone();
            piece.byte_range = ByteIndex(w[0])..ByteIndex(w[1]);
            if k > 0 {
                piece.leading_space = 0.0;
            }
            if let Some((_, ground)) = ranges.iter().find(|(r, _)| r.start <= w[0] && w[1] <= r.end) {
                piece.format.background = *ground;
            }
            out.push(piece);
        }
    }
    job.sections = out;
}

/// Selects byte range `r` of `text` in the editor `id`; `caret_first`
/// leaves the caret at its start (all an unfocused editor keeps).
pub fn select(ctx: &egui::Context, id: egui::Id, text: &str, r: Range<usize>, caret_first: bool) {
    let a = egui::text::CCursor::new(text[..r.start].chars().count());
    let b = egui::text::CCursor::new(text[..r.end].chars().count());
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let range = if caret_first {
        egui::text::CCursorRange::two(b, a)
    } else {
        egui::text::CCursorRange::two(a, b)
    };
    state.cursor.set_char_range(Some(range));
    state.store(ctx, id);
}

/// What the find row asks of the editor this frame.
pub struct Found {
    /// Every match and the current one's index.
    pub ranges: Vec<Range<usize>>,
    pub current: Option<usize>,
    /// Select and scroll to the current match now.
    pub reveal: bool,
}

/// The find row over the ST pane, for `text`; `None` once closed (Esc
/// or ✕). `id` is the ST editor's.
pub fn row(ui: &mut egui::Ui, find: &mut Option<Find>, id: egui::Id, text: &str) -> Option<Found> {
    let f = find.as_mut()?;
    let box_id = id.with("find");
    let mut close = false;
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        let width = (ui.available_width() - 190.0).clamp(80.0, 260.0);
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut f.query)
                    .id(box_id)
                    .hint_text("find")
                    .desired_width(width),
            )
            .tip("Finds this text in the Structured Text, whatever the case. Enter or Ctrl G: next; Shift Enter or Ctrl Shift G: previous; Esc closes.");
        if std::mem::take(&mut f.focus) {
            r.request_focus();
        }
        if r.changed() {
            changed = true;
        }
        if r.lost_focus() {
            let (enter, shift, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
            if esc {
                close = true;
            } else if enter {
                f.step += if shift { -1 } else { 1 };
                f.reveal = true;
                r.request_focus();
            }
        } else if r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        let ranges = matches(text, &f.query);
        let n = ranges.len();
        if changed {
            f.current = 0;
            f.reveal = true;
        }
        if n > 0 {
            #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
            {
                f.current = (f.current as isize + f.step).rem_euclid(n as isize) as usize;
            }
        }
        f.step = 0;
        let count = match n {
            0 if f.query.is_empty() => String::new(),
            0 => "no match".to_string(),
            n => format!("{} of {n}", f.current.min(n - 1) + 1),
        };
        ui.label(RichText::new(count).monospace())
            .tip("Which match is selected, of how many.");
        if ui
            .add_enabled(n > 0, egui::Button::new("▲"))
            .tip("The previous match (Ctrl Shift G).")
            .clicked()
        {
            f.step = -1;
            f.reveal = true;
        }
        if ui
            .add_enabled(n > 0, egui::Button::new("▼"))
            .tip("The next match (Ctrl G).")
            .clicked()
        {
            f.step = 1;
            f.reveal = true;
        }
        if ui.button("✕").tip("Closes the find box (Esc).").clicked() {
            close = true;
        }
    });
    if close {
        // The editor takes the keyboard with the current match selected,
        // so typing replaces it (an unfocused editor keeps only a caret).
        let ranges = matches(text, &f.query);
        if let Some(r) = ranges.get(f.current.min(ranges.len().saturating_sub(1))) {
            select(ui.ctx(), id, text, r.clone(), false);
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }
        *find = None;
        return None;
    }
    let f = find.as_mut()?;
    // A button's step lands on the next frame's count; resolve it now.
    let ranges = matches(text, &f.query);
    let n = ranges.len();
    if n > 0 && f.step != 0 {
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
        {
            f.current = (f.current as isize + f.step).rem_euclid(n as isize) as usize;
        }
        f.step = 0;
    }
    let current = (n > 0).then(|| f.current.min(n - 1));
    let reveal = std::mem::take(&mut f.reveal) && current.is_some();
    Some(Found {
        ranges,
        current,
        reveal,
    })
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};
    use proptest::prelude::*;

    use super::{Find, mark, matches};

    #[test]
    fn matches_ignore_ascii_case_and_never_overlap() {
        let t = "Motor := MOTOR AND motor_ok; aaaa";
        let m = matches(t, "motor");
        assert_eq!(
            m.iter().map(|r| &t[r.clone()]).collect::<Vec<_>>(),
            ["Motor", "MOTOR", "motor"]
        );
        assert_eq!(matches(t, "aa").len(), 2, "aaaa holds two, not three");
        assert_eq!(matches(t, "").len(), 0);
        assert_eq!(matches("", "x").len(), 0);
        assert_eq!(
            matches("größe := Größe;", "größe").len(),
            2,
            "the ASCII G in either case"
        );
        assert_eq!(
            matches("äb := Äb;", "äb").len(),
            1,
            "a non-ASCII letter matches exactly"
        );
    }

    #[test]
    fn ctrl_g_on_a_shut_box_opens_it() {
        let mut f = None;
        Find::step(&mut f, 1);
        assert!(f.is_some());
        Find::step(&mut f, -1);
        Find::step(&mut f, -1);
        assert_eq!(f.unwrap().step, -2);
    }

    fn job(text: &str, cuts: &[usize]) -> LayoutJob {
        let mut j = LayoutJob::default();
        let mut at = 0;
        for &c in cuts.iter().chain(std::iter::once(&text.len())) {
            if c > at && text.is_char_boundary(c) {
                j.append(
                    &text[at..c],
                    0.0,
                    TextFormat::simple(FontId::monospace(12.0), Color32::WHITE),
                );
                at = c;
            }
        }
        j
    }

    proptest! {
        /// Every match is the query in some case, on char boundaries.
        #[test]
        fn every_match_is_the_query(text in "[a-cA-Cé ]{0,40}", query in "[a-cA-Cé]{1,3}") {
            for r in matches(&text, &query) {
                prop_assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
                prop_assert!(text[r].eq_ignore_ascii_case(&query));
            }
        }

        /// Marking keeps the text and its order, covers every byte once,
        /// and grounds exactly the marked bytes.
        #[test]
        fn marking_keeps_the_text(
            text in "[a-z é\n]{0,40}",
            cuts in proptest::collection::vec(0usize..40, 0..5),
            query in "[a-z]{1,2}",
        ) {
            let mut cuts = cuts;
            cuts.sort_unstable();
            let mut j = job(&text, &cuts);
            let ground = Color32::RED;
            let found = matches(&text, &query);
            let ranges: Vec<_> = found.iter().map(|r| (r.clone(), ground)).collect();
            mark(&mut j, &ranges);
            let mut at = 0;
            for s in &j.sections {
                let (lo, hi) = (s.byte_range.start.0, s.byte_range.end.0);
                prop_assert_eq!(lo, at);
                prop_assert!(hi > lo);
                let inside = found.iter().any(|r| r.start <= lo && hi <= r.end);
                prop_assert_eq!(s.format.background == ground, inside);
                at = hi;
            }
            prop_assert_eq!(at, text.len());
        }
    }
}
