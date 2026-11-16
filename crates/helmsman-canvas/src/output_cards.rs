//! The outputs as cards (Nick, 2026-11-16): one per output of the
//! program, one to a row at the panel's width. The first line is the live
//! value and the name; the second says what the output finally depends on
//! as chips, in three groups that tell apart at a glance: signals read
//! from outside, state the program keeps from one scan to the next, and
//! the function blocks (timers, counters) it reads. The whole card is the
//! tap target: a tap lights the path that sets the output on the diagram
//! ([`crate::st_view::Viewer::show_output`]), and the card stays selected
//! while that path is lit.
//!
//! The card is the same in the simulation's I/O tab and a box's. Every
//! width is capped to the column and 12 pt is left for the scroll bar: a
//! resizable panel grows to the widest row it is shown.

use eframe::egui::{self, Color32, Margin, RichText, Sense, Stroke};
use helmsman_diagram::{Source, SourceKind};

use crate::theme::{BODY, theme};
use crate::tips::Tip;

/// One output of the program and what sets it, from the diagram alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputCard {
    /// The program path, upper-case as the diagram spells it.
    pub name: String,
    /// What it finally depends on ([`helmsman_diagram::signals`]).
    pub signals: Vec<Source>,
    /// Some statement of the program assigns it.
    pub driven: bool,
}

/// What a card shows now.
pub struct Row<'a> {
    /// The point's name, for the card's identity.
    pub key: &'a str,
    /// The value line: lamp and name, or name and value.
    pub text: String,
    pub color: Color32,
    pub glyph: &'static str,
    /// Where the point is wired, at the end of the tooltip.
    pub place: &'a str,
    /// The output whose path is lit now.
    pub selected: bool,
    pub card: Option<&'a OutputCard>,
}

/// The word for a block of type `fb`.
fn block_word(fb: &str) -> &'static str {
    match fb {
        "TON" | "TOF" | "TP" => "timer",
        "CTU" | "CTD" | "CTUD" => "counter",
        "R_TRIG" | "F_TRIG" => "edge detector",
        _ => "block",
    }
}

/// Draws the card; true when it was tapped. The response is the card's,
/// for a menu to hang from.
pub fn show(ui: &mut egui::Ui, row: &Row) -> (bool, egui::Response) {
    let t = theme(ui);
    let margin = 8.0;
    let inner = (ui.available_width() - 12.0 - 2.0 * margin - 2.0).max(1.0);
    // What a hover over each part says; the card's own where none is.
    let mut parts: Vec<(egui::Rect, String)> = Vec::new();
    let frame = egui::Frame::new()
        .fill(if row.selected { t.shade } else { t.panel })
        .stroke(Stroke::new(
            if row.selected { 2.0 } else { 1.0 },
            if row.selected { t.accent } else { t.rule },
        ))
        .corner_radius(6)
        .inner_margin(Margin::same(8));
    let out = frame.show(ui, |ui| {
        ui.set_width(inner);
        ui.horizontal(|ui| {
            let g = crate::glyph_lib::show(
                ui,
                row.glyph,
                crate::glyphs::ROW,
                row.color,
                crate::glyph_lib::tip(row.glyph),
            );
            parts.push((g.rect, crate::glyph_lib::tip(row.glyph).to_string()));
            ui.add(
                egui::Label::new(RichText::new(&row.text).monospace().size(BODY).color(row.color))
                    .truncate(),
            );
        });
        let Some(card) = row.card else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            let word = |ui: &mut egui::Ui, w: &str, tip: &str| {
                let r = ui.label(RichText::new(w).size(BODY).color(t.muted));
                (r.rect, tip.to_string())
            };
            if !card.driven {
                parts.push(word(ui, "nothing sets it", "No statement of the program assigns this output."));
                return;
            }
            let of = |k: SourceKind| card.signals.iter().filter(move |s| s.kind == k);
            if of(SourceKind::Input).next().is_none()
                && of(SourceKind::Kept).next().is_none()
                && of(SourceKind::Block).next().is_none()
            {
                parts.push(word(ui, "set from constants", "No signal, kept value or block is behind it."));
                return;
            }
            let chip = |ui: &mut egui::Ui, text: String, fill: Color32, tip: String| {
                let r = ui.label(
                    RichText::new(text)
                        .monospace()
                        .size(BODY)
                        .color(t.fg)
                        .background_color(fill),
                );
                (r.rect, tip)
            };
            if of(SourceKind::Input).next().is_some() {
                let r = ui.label(RichText::new("driven by").size(BODY).color(t.muted));
                parts.push((
                    r.rect,
                    "The signals read from outside that this output finally depends on, through every statement that sets it.".into(),
                ));
                for s in of(SourceKind::Input) {
                    let n = s.name.to_ascii_lowercase();
                    parts.push(chip(
                        ui,
                        n.clone(),
                        Color32::TRANSPARENT,
                        format!("{n}: read from outside the program (an input or a device)."),
                    ));
                }
            }
            if of(SourceKind::Kept).next().is_some() {
                let r = ui.label(RichText::new("kept").size(BODY).color(t.muted));
                parts.push((
                    r.rect,
                    "Values the program keeps from one scan to the next: what it set last scan decides this one."
                        .into(),
                ));
                for s in of(SourceKind::Kept) {
                    let n = s.name.to_ascii_lowercase();
                    parts.push(chip(
                        ui,
                        n.clone(),
                        t.rule,
                        format!("{n}: kept by the program. It holds what was set in an earlier scan, a step or a latch."),
                    ));
                }
            }
            for s in of(SourceKind::Block) {
                let n = s.name.to_ascii_lowercase();
                let w = block_word(&s.fb);
                parts.push(chip(
                    ui,
                    format!("{w} {n}"),
                    t.shade,
                    format!("{n}: a {w} ({}) the program calls. Its output is read on the way to this one.", s.fb.to_ascii_lowercase()),
                ));
            }
        });
    });
    let rect = out.response.rect;
    let id = ui.scope_id().with(("output card", row.key));
    let resp = ui.interact(rect, id, Sense::click());
    let tap = format!(
        "Show what drives {} on the diagram.\n{}",
        row.key.to_ascii_lowercase(),
        row.place
    );
    let at = ui.input(|i| i.pointer.hover_pos());
    let tip = at
        .and_then(|p| parts.iter().find(|(r, _)| r.contains(p)))
        .map_or(tap, |(_, tip)| tip.clone());
    let resp = resp.tip(tip);
    ui.add_space(4.0);
    (resp.clicked(), resp)
}
