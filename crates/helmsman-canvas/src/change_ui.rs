//! Show Changes on screen (Nick, 2026-11-27): the proposed program put on
//! the canvas and in the Code tab, marked against the current one and not
//! running, under a banner that says so. Both Editors draw it from the
//! [`Shown`] they build for the change held in the Agent tab.

use std::rc::Rc;

use eframe::egui::{self, RichText};
use helmsman_iec::ImageSizes;

use crate::agent_marks::StMarks;
use crate::change;
use crate::instruct::{Act, Apply};
use crate::scope::Files;
use crate::st_view::Viewer;
use crate::tips::Tip;

/// The words over the canvas while a change is on show.
pub const BANNER: &str = "Proposed change, not applied: the machine runs the current program";

/// The proposed program, ready to draw.
pub struct Shown {
    /// The file the change writes.
    pub file: String,
    /// That file as proposed.
    pub text: String,
    /// Its lines marked against the current file, as a diff draws.
    pub marks: StMarks,
    /// The proposed program's diagram, marked against the current one
    /// (`baseline`), with no scan on it.
    pub viewer: Viewer,
}

impl Shown {
    /// The proposed version of `old` with `apply` in it; `None` when its
    /// diagram cannot be drawn.
    #[must_use]
    pub fn build(old: &Files, apply: &Apply, sizes: ImageSizes) -> Option<Self> {
        let new = change::proposed(old, apply);
        let (old_src, new_src) = (change::build_src(old), change::build_src(&new));
        let drawn = change::draw(&old_src, &new_src, sizes).ok()?;
        let mut viewer = Viewer::new(&new_src, drawn.new);
        viewer.baseline = drawn.old.map(Rc::new);
        Some(Self {
            file: apply.file.clone(),
            marks: StMarks::of(old.get(&apply.file).map_or("", String::as_str), &apply.text),
            text: apply.text.clone(),
            viewer,
        })
    }
}

/// The banner over the canvas: what is on show, and Show current, Apply
/// and Discard. Apply is off, with its reason in the tip, when the change
/// cannot be applied.
pub fn banner(ui: &mut egui::Ui, chrome: &mut crate::editor::Chrome, refusal: Option<&str>) -> Option<Act> {
    let t = crate::theme::theme(ui);
    let mut act = None;
    egui::Frame::new()
        .fill(t.rule)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let r = ui.add(egui::Label::new(RichText::new(BANNER).strong().color(t.fg)).wrap());
                chrome.hit("changes banner", r.rect);
                let r = crate::glyphs::button(
                    ui,
                    crate::glyphs::Icon::Find,
                    true,
                    true,
                    "Show current: puts the running program back on screen. The change stays held.",
                );
                chrome.hit("changes show current", r.rect);
                if r.clicked() {
                    act = Some(Act::ShowChanges);
                }
                let r = crate::glyphs::button(
                    ui,
                    crate::glyphs::Icon::Apply,
                    refusal.is_none(),
                    false,
                    refusal.unwrap_or("Apply: writes the change into the file."),
                );
                chrome.hit("changes apply", r.rect);
                if r.clicked() && refusal.is_none() {
                    act = Some(Act::ApplyChange);
                }
                let r = crate::glyphs::button(
                    ui,
                    crate::glyphs::Icon::Clear,
                    true,
                    false,
                    "Discard: drops the change. Nothing was written.",
                );
                chrome.hit("changes discard", r.rect);
                if r.clicked() {
                    act = Some(Act::DiscardChange);
                }
            });
        });
    act
}

/// The note over the Code tab's read-only text.
pub fn code_note(ui: &mut egui::Ui, chrome: &mut crate::editor::Chrome) {
    let t = crate::theme::theme(ui);
    let r = ui
        .add(egui::Label::new(RichText::new(BANNER).italics().color(t.muted)).wrap())
        .tip("The file as the change would leave it, marked against the current one. Read-only.");
    chrome.hit("changes code note", r.rect);
}
