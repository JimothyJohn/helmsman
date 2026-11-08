//! A line diff drawn as GitHub draws one (Nick, 2026-11-08): an added
//! line is a `+` on a green tint, a removed one a `−` on a red tint, each
//! row the width of the pane, the sign in its colour and the text in the
//! page's own. Context lines stay plain. The review's source diff
//! ([`crate::review_view`]) and a revision's diff ([`crate::history_ui`])
//! both draw their lines with [`row`].

use eframe::egui::{self, Color32, RichText};

use crate::st_view::brand::{GREEN, RED};
use crate::theme::theme;

/// What a line of a diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// In both versions: context.
    Same,
    /// Only in the new version.
    Added,
    /// Only in the old version.
    Removed,
}

impl Kind {
    /// The sign in the gutter: `+`, `−`, or a blank for context.
    #[must_use]
    pub const fn sign(self) -> &'static str {
        match self {
            Self::Same => " ",
            Self::Added => "+",
            Self::Removed => "−",
        }
    }

    /// The tint under the row: faint green or red, none for context.
    #[must_use]
    pub fn tint(self) -> Option<Color32> {
        match self {
            Self::Same => None,
            Self::Added => Some(GREEN.gamma_multiply(0.18)),
            Self::Removed => Some(RED.gamma_multiply(0.18)),
        }
    }

    /// The sign's colour (context: the muted one of `fallback`).
    #[must_use]
    pub const fn sign_color(self, fallback: Color32) -> Color32 {
        match self {
            Self::Same => fallback,
            Self::Added => GREEN,
            Self::Removed => RED,
        }
    }
}

/// One line: `gutter` (a line number, or nothing) muted, the sign, then
/// the text, truncated to the pane; a changed line is tinted edge to edge.
pub fn row(ui: &mut egui::Ui, kind: Kind, gutter: &str, text: &str) {
    let t = theme(ui);
    let mut frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 1));
    if let Some(fill) = kind.tint() {
        frame = frame.fill(fill);
    }
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if !gutter.is_empty() {
                ui.label(RichText::new(format!("{gutter} ")).monospace().color(t.muted));
            }
            ui.label(
                RichText::new(format!("{} ", kind.sign()))
                    .monospace()
                    .strong()
                    .color(kind.sign_color(t.muted)),
            );
            let fg = if kind == Kind::Same { t.muted } else { t.fg };
            ui.add(egui::Label::new(RichText::new(text).monospace().color(fg)).truncate());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::Kind;

    #[test]
    fn only_a_change_is_signed_and_tinted() {
        assert_eq!(Kind::Added.sign(), "+");
        assert_eq!(Kind::Removed.sign(), "−");
        assert_eq!(Kind::Same.sign(), " ");
        assert!(Kind::Same.tint().is_none());
        let (a, r) = (Kind::Added.tint().unwrap(), Kind::Removed.tint().unwrap());
        assert!(a.g() > a.r() && r.r() > r.g(), "green for added, red for removed");
        assert!(a.a() < 128, "a tint, not a block of colour");
    }
}
