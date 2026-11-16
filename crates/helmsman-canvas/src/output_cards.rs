//! The outputs as cards (Nick, 2026-11-16): one per output of the
//! program with its name and live value, and under them the signals it
//! finally depends on as chips. A tap on a card lights the path that sets
//! it on the diagram ([`crate::st_view::Viewer::show_output`]).
//!
//! The card is the same in the simulation's I/O tab and a box's. Rows
//! wrap and every width is capped to the column: a resizable panel grows
//! to the widest row it is shown.

use eframe::egui::{self, Color32, RichText, Vec2};

use crate::theme::{MIN_TEXT, theme};
use crate::tips::Tip;

/// One output of the program and what sets it, from the diagram alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputCard {
    /// The program path, upper-case as the diagram spells it.
    pub name: String,
    /// The signals it finally depends on ([`helmsman_diagram::signals`]).
    pub signals: Vec<String>,
    /// Some statement of the program assigns it.
    pub driven: bool,
}

/// What a card shows now.
pub struct Row<'a> {
    /// The value line: lamp and name, or name and value.
    pub text: String,
    pub color: Color32,
    pub glyph: &'static str,
    /// Under the tap's own words: where the point is wired.
    pub place: &'a str,
    /// The output whose path is lit now.
    pub selected: bool,
    pub card: Option<&'a OutputCard>,
}

/// Draws the card; true when it was tapped. Returns the tap target's
/// response too, for a menu to hang from.
pub fn show(ui: &mut egui::Ui, row: &Row) -> (bool, egui::Response) {
    let t = theme(ui);
    ui.vertical(|ui| {
        // Room for the scroll bar a long list brings.
        ui.set_max_width((ui.available_width() - 12.0).max(1.0));
        let tap = ui
            .horizontal(|ui| {
                crate::glyph_lib::show(
                    ui,
                    row.glyph,
                    crate::glyphs::ROW,
                    row.color,
                    crate::glyph_lib::tip(row.glyph),
                );
                let b = egui::Button::selectable(
                    row.selected,
                    RichText::new(&row.text)
                        .monospace()
                        .size(MIN_TEXT)
                        .color(row.color),
                )
                .wrap_mode(egui::TextWrapMode::Truncate)
                .min_size(Vec2::new(ui.available_width(), crate::st_view::TAP * 0.8));
                let r = ui.add(b).tip(format!(
                    "Click to light what sets it in the diagram, and go there.\n{}",
                    row.place
                ));
                (r.clicked(), r)
            })
            .inner;
        if let Some(card) = row.card {
            ui.horizontal_wrapped(|ui| {
                if card.driven {
                    ui.label(RichText::new("driven by").size(MIN_TEXT).color(t.muted))
                        .tip("The signals this output finally depends on, through every statement that sets it.");
                } else {
                    ui.label(RichText::new("nothing sets it").size(MIN_TEXT).color(t.muted))
                        .tip("No statement of the program assigns this output.");
                }
                if card.driven && card.signals.is_empty() {
                    ui.label(RichText::new("no signals").size(MIN_TEXT).color(t.muted))
                        .tip("It is set from constants alone.");
                }
                for s in &card.signals {
                    ui.add(
                        egui::Label::new(
                            RichText::new(s.to_ascii_lowercase())
                                .monospace()
                                .size(MIN_TEXT)
                                .color(t.fg),
                        )
                        .truncate(),
                    )
                    .tip(format!("{} is read on the way to this output.", s.to_ascii_lowercase()));
                }
            });
        }
        tap
    })
    .inner
}
