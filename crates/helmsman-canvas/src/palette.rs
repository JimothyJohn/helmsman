//! The Palette tab (Nick, 2026-11-12: "drag a pump onto the canvas and
//! the agent writes the I/O and logic for it"): the glyph library's
//! operator controls and program pieces as a grid of pictures. A click
//! on one, or a drag of it onto the diagram, asks the Agent to add that
//! piece to the program, the way a typed instruction does; the answer is
//! checked and waits for Deploy like any other.

use eframe::egui::{self, CornerRadius, Rect, Sense, Vec2};

use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

/// One piece of the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    /// The library glyph (`glyph_lib`).
    pub glyph: &'static str,
    /// What the Agent is asked when it is dropped.
    pub ask: &'static str,
}

/// The operator controls, then the program pieces.
pub const ITEMS: [Item; 12] = [
    Item {
        glyph: "push-button",
        ask: "Add a push button: a BOOL input that is TRUE while it is held, named for what it does, and use it in the program.",
    },
    Item {
        glyph: "e-stop",
        ask: "Add an emergency stop: a BOOL input that is TRUE while the circuit is healthy, so that a broken wire stops the machine, and make it switch every output off when it opens.",
    },
    Item {
        glyph: "selector",
        ask: "Add a selector switch with Manual and Auto positions as a BOOL input, and make the program act on it.",
    },
    Item {
        glyph: "pilot-lamp",
        ask: "Add a pilot lamp: a BOOL output named for what it shows, and drive it from the program's state.",
    },
    Item {
        glyph: "alarm",
        ask: "Add an alarm: a BOOL output that latches when a fault condition of the program holds, and a reset push button input that clears it once the condition has gone.",
    },
    Item {
        glyph: "interlock",
        ask: "Add an interlock: a BOOL input that must be TRUE for the program's outputs to run, and make every output depend on it.",
    },
    Item {
        glyph: "timer",
        ask: "Add a timer (TON) where the program needs a delay, with its preset as a named constant.",
    },
    Item {
        glyph: "counter",
        ask: "Add a counter (CTU) that counts the program's main event, with a reset and its preset as a named constant.",
    },
    Item {
        glyph: "latch",
        ask: "Add a start/stop latch: start sets it, stop resets it, and stop wins when both are pressed.",
    },
    Item {
        glyph: "edge",
        ask: "Add a rising edge detector (R_TRIG) so the program acts once on the moment an input turns on.",
    },
    Item {
        glyph: "step",
        ask: "Turn the program into a sequence of numbered steps (a CASE on a step variable), each step moving to the next when its condition holds.",
    },
    Item {
        glyph: "hmi",
        ask: "Add the values an operator screen would show as outputs: the program's state, its counts and any alarms, each named plainly.",
    },
];

/// A cell's side, points: a tap target and room for the glyph.
const CELL: f32 = TAP * 2.0;

/// The palette in `ui`: a grid of pictures, as many columns as fit, each
/// tipped with what it asks. Returns the item clicked; a drag sets the
/// item as egui's drag payload, and [`dropped`] picks it up over the
/// diagram.
pub fn show(ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) -> Option<Item> {
    let t = theme(ui);
    ui.label(egui::RichText::new("Drag a piece onto the diagram, or click it.").color(t.muted))
        .tip("The Agent adds it to the program; nothing runs until you deploy.");
    ui.add_space(4.0);
    // Columns that fit the panel, so the grid never widens it.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cols = ((ui.available_width() - 12.0) / (CELL + 6.0)).floor().max(1.0) as usize;
    let mut picked = None;
    egui::Grid::new("palette")
        .num_columns(cols)
        .spacing(Vec2::splat(6.0))
        .show(ui, |ui| {
            for row in ITEMS.chunks(cols) {
                for item in row {
                    let (rect, r) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click_and_drag());
                    let hot = r.hovered() || r.dragged();
                    let fill = if hot { t.shade } else { t.rule.gamma_multiply(0.5) };
                    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
                    let ink = if hot { t.accent } else { t.fg };
                    crate::glyph_lib::paint(ui.painter(), rect.shrink(CELL * 0.2), item.glyph, ink);
                    let r = r.on_hover_text(format!("{}: {}", crate::glyph_lib::tip(item.glyph), item.ask));
                    r.dnd_set_drag_payload(*item);
                    hits.push((format!("palette {}", item.glyph), rect));
                    if r.clicked() {
                        picked = Some(*item);
                    }
                }
                ui.end_row();
            }
        });
    // The piece under the pointer while it is dragged.
    if let Some(item) = egui::DragAndDrop::payload::<Item>(ui.ctx())
        && let Some(p) = ui.ctx().pointer_interact_pos()
    {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("palette-drag"));
        let painter = ui.ctx().layer_painter(layer);
        let r = Rect::from_center_size(p, Vec2::splat(TAP * 1.4));
        painter.rect_filled(r, CornerRadius::same(8), t.shade.gamma_multiply(0.9));
        crate::glyph_lib::paint(&painter, r.shrink(TAP * 0.25), item.glyph, t.accent);
    }
    picked
}

/// The palette item released over `diagram` this frame, if one was.
pub fn dropped(ctx: &egui::Context, diagram: Rect) -> Option<Item> {
    let over = ctx
        .input(|i| i.pointer.any_released() && i.pointer.interact_pos().is_some_and(|p| diagram.contains(p)));
    if !over {
        return None;
    }
    egui::DragAndDrop::take_payload::<Item>(ctx).map(|i| *i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_piece_is_a_glyph_of_the_library_and_asks_a_whole_sentence() {
        let mut seen = std::collections::HashSet::new();
        for i in ITEMS {
            assert!(crate::glyph_lib::mask(i.glyph).is_some(), "{}", i.glyph);
            assert!(seen.insert(i.glyph), "{} twice", i.glyph);
            assert!(
                i.ask.starts_with(|c: char| c.is_ascii_uppercase()) && i.ask.ends_with('.'),
                "{}",
                i.ask
            );
            assert!(
                i.ask.len() < 300,
                "{} is short enough to read in a tooltip",
                i.glyph
            );
        }
    }
}
