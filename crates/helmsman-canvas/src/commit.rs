//! A number field that writes only when committed. While someone types,
//! nothing reaches the program: Enter or leaving the field commits what
//! was typed, Escape puts the current value back. The simulation's scan
//! period, the I/O panel's numbers, a number on the diagram and a box's
//! numeric forces all go through here (a slider stays live: a drag is
//! the commit). Clicking in selects the whole value, so typing replaces.

use eframe::egui::{self, Id, Key, Vec2, text::CCursor, text::CCursorRange, text_edit::TextEditState};

/// What a frame of [`number`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Commit {
    /// Nothing committed (idle, or still being typed).
    Nothing,
    /// Enter or focus out with a changed text: what was typed, trimmed.
    Set(String),
}

/// The draft text of the field `id` while it is being edited.
fn draft(ui: &egui::Ui, id: Id) -> Option<String> {
    ui.data(|d| d.get_temp::<String>(id.with("draft")))
}

fn set_draft(ui: &egui::Ui, id: Id, text: Option<String>) {
    ui.data_mut(|d| match text {
        Some(t) => {
            d.insert_temp(id.with("draft"), t);
        }
        None => d.remove::<String>(id.with("draft")),
    });
}

/// A field of `size` showing `value` (as text) with `suffix` after it
/// (`500 ms`; typing it back is optional), editable in place. Returns
/// what the frame committed; the caller parses and bounds it, and leaves
/// the value alone on nonsense.
pub fn number(ui: &mut egui::Ui, id: Id, value: &str, suffix: &str, size: Vec2, hint: &str) -> Commit {
    let editing = draft(ui, id);
    let shown = if suffix.is_empty() {
        value.to_string()
    } else {
        format!("{value} {suffix}")
    };
    let mut text = editing.clone().unwrap_or_else(|| shown.clone());
    let field = egui::TextEdit::singleline(&mut text)
        .id(id)
        .desired_width(size.x)
        .font(egui::TextStyle::Monospace);
    let r = ui.add_sized(size, field).on_hover_text(hint);
    if r.gained_focus()
        && let Some(mut state) = TextEditState::load(ui.ctx(), id)
    {
        let all = CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), id);
    }
    let escaped = ui.input(|i| i.key_pressed(Key::Escape));
    if r.lost_focus() {
        set_draft(ui, id, None);
        let typed = text.trim();
        let typed = if suffix.is_empty() {
            typed
        } else {
            typed.strip_suffix(suffix).map_or(typed, str::trim_end)
        };
        if escaped || typed == value.trim() {
            return Commit::Nothing;
        }
        return Commit::Set(typed.to_string());
    }
    if r.has_focus() {
        set_draft(ui, id, Some(text));
    } else if editing.is_some() {
        set_draft(ui, id, None);
    }
    Commit::Nothing
}

/// A committed integer within `lo..=hi`, or `None` for anything else.
#[must_use]
pub fn int(c: &Commit, lo: i64, hi: i64) -> Option<i64> {
    match c {
        Commit::Set(t) => t.parse::<i64>().ok().map(|v| v.clamp(lo, hi)),
        Commit::Nothing => None,
    }
}

/// A committed finite real, or `None` for anything else.
#[must_use]
pub fn real(c: &Commit) -> Option<f64> {
    match c {
        Commit::Set(t) => t.parse::<f64>().ok().filter(|v| v.is_finite()),
        Commit::Nothing => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Commit, int, real};

    #[test]
    fn parsing_is_bounded_and_refuses_nonsense() {
        assert_eq!(int(&Commit::Set("12".into()), 0, 10), Some(10));
        assert_eq!(int(&Commit::Set("-3".into()), 0, 10), Some(0));
        assert_eq!(int(&Commit::Set("4".into()), 0, 10), Some(4));
        for junk in ["", "x", "1e3", "4.5", "99999999999999999999"] {
            assert_eq!(int(&Commit::Set(junk.into()), 0, 10), None, "{junk:?}");
        }
        assert_eq!(int(&Commit::Nothing, 0, 10), None);
        assert_eq!(real(&Commit::Set("2.5".into())), Some(2.5));
        assert_eq!(real(&Commit::Set("1e3".into())), Some(1000.0));
        for junk in ["", "x", "inf", "NaN", "-inf"] {
            assert_eq!(real(&Commit::Set(junk.into())), None, "{junk:?}");
        }
    }
}
