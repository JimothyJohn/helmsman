//! Tooltips on everything (Nick, 2026-11-05): the explanation of a control
//! lives in its tooltip, not in a note beside it, and a disabled control
//! says why it is disabled. [`Tip::tip`] sets both hover texts at once;
//! `tips_tests` reads the canvas sources and refuses a control without one.

use eframe::egui::{Response, WidgetText};

pub trait Tip {
    /// The same tooltip whether the control is enabled or not. Not
    /// `must_use`: a label's response is dropped once it has its tooltip,
    /// as egui's own `on_hover_text`'s is.
    fn tip(self, text: impl Into<WidgetText>) -> Self;
}

/// A text field built with `.show(ui)` (for its cursor) gets its tooltip too.
impl Tip for eframe::egui::text_edit::TextEditOutput {
    fn tip(mut self, text: impl Into<WidgetText>) -> Self {
        self.response.response = self.response.response.tip(text);
        self
    }
}

impl Tip for Response {
    fn tip(self, text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        self.on_hover_text(text.clone()).on_disabled_hover_text(text)
    }
}
