//! The I/O tab's cards (Nick, 2026-11-16): one per input and output, the
//! same in the simulation and on a box. A card says three things and no
//! more: the point (its control or its live value and name), what it is
//! in the real world (the device wired to it, else the module and channel
//! it is on), and its part in the control scheme, from the diagram alone
//! with no scan needed: the first output an input affects, the first
//! signal that drives an output, and "…" when there are more.
//!
//! Everything else is one gesture away. A double-click on the card (or a
//! click on the "…") opens its details in place: every source in three
//! groups that tell apart at a glance (signals read from outside, state
//! the program keeps from one scan to the next, the function blocks it
//! reads), the wiring in full and how often the name is in the code. The
//! two glyphs at the card's right go to the point: `< >` finds it in the
//! code, and again steps to its next use ([`crate::find::Find::name`]);
//! the diagram glyph lights every block that reads or writes it and
//! frames them ([`crate::st_view::Viewer::show_signal`]). A tap on an
//! output's card lights the path that sets it, as before.
//!
//! The tab shows its cards one to a row, or as tiles (the list and grid
//! glyphs at its top), and Inputs and Outputs each fold away behind the
//! arrow of their heading. Both are remembered ([`crate::editor::Chrome`]).
//!
//! Every width is capped to the column: a resizable panel grows to the
//! widest row it is shown. The tab leaves 12 pt for its scroll bar.

use eframe::egui::{self, Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, Vec2};
use helmsman_diagram::{Source, SourceKind};

use crate::editor::Chrome;
use crate::glyphs::Icon;
use crate::theme::{BODY, theme};
use crate::tips::Tip;

/// What the tab leaves free at its right for the scroll bar, pt.
pub const BAR: f32 = 12.0;
/// The narrowest a tile gets before the grid drops a column, pt.
const TILE: f32 = 96.0;

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

/// The outputs that finally depend on input `name` (the whole variable,
/// or a field or element of it), in the cards' order, as they spell them.
#[must_use]
pub fn affects(cards: &[OutputCard], name: &str) -> Vec<String> {
    let of = |s: &Source| {
        s.kind == SourceKind::Input
            && s.name.len() >= name.len()
            && s.name.is_char_boundary(name.len())
            && s.name[..name.len()].eq_ignore_ascii_case(name)
            && matches!(s.name[name.len()..].chars().next(), None | Some('.' | '['))
    };
    cards
        .iter()
        .filter(|c| c.signals.iter().any(of))
        .map(|c| c.name.clone())
        .collect()
}

/// A point's part in the control scheme, as the diagram tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Links<'a> {
    /// An output and what sets it; `None` when no diagram says.
    Output(Option<&'a OutputCard>),
    /// An input and the outputs that depend on it ([`affects`]); `None`
    /// when no diagram says.
    Input(Option<Vec<String>>),
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

/// A source as a card names it: `start`, `timer t1`.
fn source_name(s: &Source) -> String {
    let n = s.name.to_ascii_lowercase();
    match s.kind {
        SourceKind::Block => format!("{} {n}", block_word(&s.fb)),
        SourceKind::Input | SourceKind::Kept => n,
    }
}

/// An output's sources in the order a card lists them: signals read from
/// outside, then the blocks it reads, then what the program keeps.
fn ordered(card: &OutputCard) -> Vec<&Source> {
    let of = |k: SourceKind| card.signals.iter().filter(move |s| s.kind == k);
    of(SourceKind::Input)
        .chain(of(SourceKind::Block))
        .chain(of(SourceKind::Kept))
        .collect()
}

/// The one line a card says of its point's part in the control scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// `driven by`, `affects`, or the whole of it when nothing is named.
    pub lead: &'static str,
    /// The first name, lower case.
    pub first: Option<String>,
    /// How many more there are behind the "…".
    pub more: usize,
    /// All of it, for the tooltip.
    pub all: String,
}

/// What the card of a point with these links says; `None` when no
/// diagram says anything of it.
#[must_use]
pub fn summary(links: &Links) -> Option<Summary> {
    let bare = |lead: &'static str, all: &str| Summary {
        lead,
        first: None,
        more: 0,
        all: all.to_string(),
    };
    match links {
        Links::Output(None) | Links::Input(None) => None,
        Links::Output(Some(c)) if !c.driven => Some(bare(
            "nothing sets it",
            "No statement of the program assigns this output.",
        )),
        Links::Output(Some(c)) => {
            let all = ordered(c);
            let Some(first) = all.first() else {
                return Some(bare(
                    "set from constants",
                    "No signal, kept value or block is behind it.",
                ));
            };
            let names: Vec<String> = all.iter().map(|s| source_name(s)).collect();
            Some(Summary {
                lead: "driven by",
                first: Some(source_name(first)),
                more: all.len() - 1,
                all: format!(
                    "Everything this output finally depends on, through every statement that sets it: {}.",
                    names.join(", ")
                ),
            })
        }
        Links::Input(Some(outs)) => {
            let Some(first) = outs.first() else {
                return Some(bare(
                    "no output depends on it",
                    "No output of the program depends on this input.",
                ));
            };
            let names: Vec<String> = outs.iter().map(|n| n.to_ascii_lowercase()).collect();
            Some(Summary {
                lead: "affects",
                first: Some(first.to_ascii_lowercase()),
                more: outs.len() - 1,
                all: format!("The outputs that depend on this input: {}.", names.join(", ")),
            })
        }
    }
}

/// What a card shows now.
#[allow(clippy::struct_excessive_bools)] // independent flags, as `Chrome`'s are
pub struct Row<'a> {
    /// The point's name, for the card's identity and its tooltips.
    pub key: &'a str,
    pub input: bool,
    /// Where the point is, at the end of the card's tooltip.
    pub place: &'a str,
    /// What it is in the real world, and that line's tooltip: the device
    /// wired to it, else the module and channel it is on.
    pub real: Option<(String, String)>,
    pub links: Links<'a>,
    /// The output whose path is lit now, or a tile whose details are open.
    pub selected: bool,
    /// The details are open.
    pub open: bool,
    /// A tile: the first line alone.
    pub tile: bool,
    /// How often the name is in the code, said in the details.
    pub uses: Option<usize>,
    /// The details offer to simulate the input as a device.
    pub simulate: bool,
}

/// What was asked of a card this frame.
#[allow(clippy::struct_excessive_bools)] // independent flags, as `Chrome`'s are
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    /// A click on the card itself.
    pub tapped: bool,
    /// Its details are to open, or to close if they are open.
    pub details: bool,
    /// Find it in the code (again: its next use).
    pub code: bool,
    /// Show it on the diagram.
    pub diagram: bool,
    /// Simulate the input as a device.
    pub simulate: bool,
}

/// An output's first line, or an input's where it has no control: its
/// glyph (a tile has none), and `text` in `color`, cut to the column.
/// `wrap`: a row's line wraps at the column so a long name shows whole;
/// a tile's is cut to its narrow width.
pub fn value(ui: &mut egui::Ui, glyph: Option<&'static str>, color: Color32, text: &str, wrap: bool) {
    ui.horizontal(|ui| {
        if let Some(glyph) = glyph {
            crate::glyph_lib::show(ui, glyph, crate::glyphs::ROW, color, crate::glyph_lib::tip(glyph));
        }
        let label =
            egui::Label::new(RichText::new(text).monospace().size(BODY).color(color)).selectable(false);
        let label = if wrap { label.wrap() } else { label.truncate() };
        ui.add(label).tip(text.to_string());
    });
}

/// A source as a chip of the details.
fn chip(ui: &mut egui::Ui, text: String, fill: Color32, tip: String) {
    let fg = theme(ui).fg;
    ui.label(
        RichText::new(text)
            .monospace()
            .size(BODY)
            .color(fg)
            .background_color(fill),
    )
    .tip(tip);
}

/// Every source of an output, or every output of an input, as chips.
fn links_ui(ui: &mut egui::Ui, links: &Links) {
    let t = theme(ui);
    let word = |ui: &mut egui::Ui, w: &str, tip: &str| {
        ui.label(RichText::new(w).size(BODY).color(t.muted))
            .tip(tip.to_string());
    };
    match links {
        Links::Output(None) | Links::Input(None) => {}
        Links::Input(Some(outs)) if outs.is_empty() => {
            word(
                ui,
                "no output depends on it",
                "No output of the program depends on this input.",
            );
        }
        Links::Input(Some(outs)) => {
            word(ui, "affects", "The outputs that depend on this input.");
            for o in outs {
                let n = o.to_ascii_lowercase();
                chip(
                    ui,
                    n.clone(),
                    Color32::TRANSPARENT,
                    format!("{n}: an output that depends on this input."),
                );
            }
        }
        Links::Output(Some(c)) if !c.driven => {
            word(
                ui,
                "nothing sets it",
                "No statement of the program assigns this output.",
            );
        }
        Links::Output(Some(c)) if c.signals.is_empty() => {
            word(
                ui,
                "set from constants",
                "No signal, kept value or block is behind it.",
            );
        }
        Links::Output(Some(c)) => {
            let of = |k: SourceKind| c.signals.iter().filter(move |s| s.kind == k);
            if of(SourceKind::Input).next().is_some() {
                word(
                    ui,
                    "driven by",
                    "The signals read from outside that this output finally depends on, through every statement that sets it.",
                );
                for s in of(SourceKind::Input) {
                    let n = s.name.to_ascii_lowercase();
                    chip(
                        ui,
                        n.clone(),
                        Color32::TRANSPARENT,
                        format!("{n}: read from outside the program (an input or a device)."),
                    );
                }
            }
            for s in of(SourceKind::Block) {
                let n = s.name.to_ascii_lowercase();
                let w = block_word(&s.fb);
                chip(
                    ui,
                    format!("{w} {n}"),
                    t.shade,
                    format!(
                        "{n}: a {w} ({}) the program calls. Its output is read on the way to this one.",
                        s.fb.to_ascii_lowercase()
                    ),
                );
            }
            if of(SourceKind::Kept).next().is_some() {
                word(
                    ui,
                    "kept",
                    "Values the program keeps from one scan to the next: what it set last scan decides this one.",
                );
                for s in of(SourceKind::Kept) {
                    let n = s.name.to_ascii_lowercase();
                    chip(
                        ui,
                        n.clone(),
                        t.rule,
                        format!(
                            "{n}: kept by the program. It holds what was set in an earlier scan, a step or a latch."
                        ),
                    );
                }
            }
        }
    }
}

/// The one line of [`summary`]: the lead, the first name, and "…" when
/// there are more. True when the "…" was clicked.
fn summary_ui(ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, s: &Summary, width: f32) -> bool {
    let t = theme(ui);
    let mut more = false;
    ui.horizontal(|ui| {
        let dots = if s.more > 0 { 22.0 } else { 0.0 };
        let room = (width - dots - ui.spacing().item_spacing.x).max(1.0);
        let mut job = egui::text::LayoutJob::default();
        let font = egui::FontId::proportional(BODY);
        job.append(s.lead, 0.0, egui::TextFormat::simple(font, t.muted));
        if let Some(first) = &s.first {
            job.append(
                first,
                6.0,
                egui::TextFormat::simple(egui::FontId::monospace(BODY), t.fg),
            );
        }
        ui.allocate_ui_with_layout(Vec2::new(room, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_max_width(room);
            ui.add(egui::Label::new(job).wrap().selectable(false))
                .tip(s.all.clone())
        });
        if s.more > 0 {
            let r = ui
                .add(
                    egui::Label::new(RichText::new("…").size(BODY).strong().color(t.accent))
                        .sense(Sense::click()),
                )
                .tip(format!(
                    "{} more. Click, or double-click the card, for all of them.",
                    s.more
                ));
            chrome.hit(format!("io more {}", key.to_ascii_lowercase()), r.rect);
            more = r.clicked();
        }
    });
    more
}

/// Draws the card: `head` draws its first line (the point's control, or
/// [`value`]) in the width it is given. Returns what was asked, the
/// card's response for a menu to hang from, and what `head` returned.
pub fn show<R>(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    row: &Row,
    head: impl FnOnce(&mut egui::Ui) -> R,
) -> (Asked, egui::Response, R) {
    let t = theme(ui);
    let name = row.key.to_ascii_lowercase();
    let margin: i8 = if row.tile { 5 } else { 8 };
    let inner = (ui.available_width() - 2.0 * f32::from(margin) - 2.0).max(1.0);
    // The card takes clicks where it was drawn last frame, so what is
    // drawn on it now (its control, its glyphs) is in front of it.
    let id = ui.scope_id().with(("io card", row.key, row.tile));
    let was: Option<Rect> = ui.data(|d| d.get_temp(id));
    let resp = ui.interact(was.unwrap_or(Rect::NOTHING), id, Sense::click());
    let mut asked = Asked::default();
    let mut glyphs = Rect::NOTHING;
    let (fill, stroke) = look(row.tile, row.selected, &t);
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(6)
        .inner_margin(Margin::same(margin));
    let out = frame.show(ui, |ui| {
        ui.set_width(inner);
        if row.tile {
            // A snapshot: nothing grows under the pointer.
            let w = &mut ui.visuals_mut().widgets;
            for v in [&mut w.hovered, &mut w.active, &mut w.inactive] {
                v.expansion = 0.0;
            }
            return head(ui);
        }
        let side = crate::glyphs::ROW;
        let gap = ui.spacing().item_spacing.x;
        let room = (inner - 2.0 * side - 2.0 * gap).max(1.0);
        let r = ui
            .horizontal_top(|ui| {
                let r = ui
                    .allocate_ui_with_layout(Vec2::new(room, side), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(room);
                        head(ui)
                    })
                    .inner;
                let code = crate::glyphs::button_sized(
                    ui,
                    Icon::Code,
                    side,
                    true,
                    false,
                    &format!("Finds {name} in the code. Click again for its next use."),
                );
                chrome.hit(format!("io code {name}"), code.rect);
                asked.code = code.clicked();
                let diagram = crate::glyphs::button_sized(
                    ui,
                    Icon::Diagram,
                    side,
                    true,
                    false,
                    &format!("Shows {name} on the diagram: every block that reads or writes it."),
                );
                chrome.hit(format!("io diagram {name}"), diagram.rect);
                asked.diagram = diagram.clicked();
                glyphs = code.rect.union(diagram.rect);
                r
            })
            .inner;
        if let Some((text, tip)) = &row.real {
            let label = egui::Label::new(RichText::new(text).size(BODY).color(t.muted)).wrap();
            let l = ui.add(label).tip(tip.clone());
            chrome.hit(format!("wired {}", row.key), l.rect);
        }
        if row.open {
            ui.horizontal_wrapped(|ui| links_ui(ui, &row.links));
            if !row.place.is_empty() {
                ui.add(egui::Label::new(RichText::new(row.place).size(BODY).color(t.muted)).wrap())
                    .tip("Where the program's point is: the device and channel, or the simulation.");
            }
            ui.horizontal_wrapped(|ui| {
                if let Some(n) = row.uses {
                    let text = match n {
                        0 => "not named in the code".to_string(),
                        1 => "named once in the code".to_string(),
                        n => format!("named {n} times in the code"),
                    };
                    ui.label(RichText::new(text).size(BODY).color(t.muted))
                        .tip("How often the program's code names it. The < > glyph steps through them in the open file.");
                }
                if row.simulate {
                    let b = ui
                        .button(RichText::new("Simulate…").size(BODY))
                        .tip("Simulates this input as a device: a sensor that follows an output, a fault, a missing signal.");
                    chrome.hit(format!("io simulate {name}"), b.rect);
                    asked.simulate = b.clicked();
                }
            });
        } else if let Some(s) = summary(&row.links)
            && summary_ui(ui, chrome, row.key, &s, inner)
        {
            asked.details = true;
        }
        r
    });
    let rect = out.response.rect;
    ui.data_mut(|d| d.insert_temp(id, rect));
    chrome.hit(format!("io card {name}"), rect);
    // A double-click anywhere on the card but its two glyphs, its control
    // included (a switch flipped twice is where it was).
    let double = ui.rect_contains_pointer(rect)
        && ui.input(|i| {
            i.pointer.button_double_clicked(egui::PointerButton::Primary)
                && i.pointer.interact_pos().is_some_and(|p| !glyphs.contains(p))
        });
    asked.details |= double;
    asked.tapped = resp.clicked();
    let what = if row.open {
        "Double-click to close its details."
    } else {
        "Double-click for its details."
    };
    let tip = if row.input {
        format!("{name}: {what}\n{}", row.place)
    } else {
        format!(
            "Click to light what drives {name} on the diagram. {what}\n{}",
            row.place
        )
    };
    let resp = resp.tip(tip.trim_end().to_string());
    ui.add_space(4.0);
    (asked, resp, out.inner)
}

/// A card's fill and outline. A tile is a flat snapshot (Nick,
/// 2026-11-23): one thin outline and the same fill selected or not, the
/// accent colour of the outline alone saying its details are open. A row
/// keeps its shaded fill and heavier outline for the output whose path
/// is lit.
#[must_use]
pub fn look(tile: bool, selected: bool, t: &crate::theme::Theme) -> (Color32, Stroke) {
    let colour = if selected { t.accent } else { t.rule };
    if tile {
        return (t.panel, Stroke::new(1.0, colour));
    }
    (
        if selected { t.shade } else { t.panel },
        Stroke::new(if selected { 2.0 } else { 1.0 }, colour),
    )
}

/// How a card is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// One to a row, with its lines; its details open in it.
    Row,
    /// A tile of the grid: the first line alone.
    Tile,
    /// Under the tiles: the open details of one of them.
    Detail,
}

/// Lays out a section's `n` cards: one to a row, or as tiles with the
/// details of each open one (`open(k)`) under them. `card` draws card
/// `k` in the form it is given, in the width it finds.
pub fn lay(
    ui: &mut egui::Ui,
    id: &str,
    n: usize,
    grid: bool,
    open: &dyn Fn(usize) -> bool,
    card: &mut dyn FnMut(&mut egui::Ui, usize, Form),
) {
    let full = (ui.available_width() - BAR).max(1.0);
    let mut one = |ui: &mut egui::Ui, k: usize, form: Form, width: f32| {
        ui.vertical(|ui| {
            ui.set_max_width(width);
            card(ui, k, form);
        });
    };
    if !grid {
        for k in 0..n {
            one(ui, k, Form::Row, full);
        }
        return;
    }
    let gap = ui.spacing().item_spacing.x;
    let (cols, col) = tiles(full, gap);
    egui::Grid::new(id)
        .num_columns(cols)
        .min_col_width(col)
        .max_col_width(col)
        .show(ui, |ui| {
            for k in 0..n {
                one(ui, k, Form::Tile, col);
                if (k + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
    for k in (0..n).filter(|&k| open(k)) {
        one(ui, k, Form::Detail, full);
    }
}

/// What a point is in the real world, with that line's tooltip: what the
/// device library says is wired to it (`wired`: its line and tooltip,
/// [`crate::wired::line`]) and the device and channel it is on. `None`
/// for a point that is only simulated.
#[must_use]
pub fn real(wired: Option<(String, String)>, place: &crate::assign::Place) -> Option<(String, String)> {
    use crate::assign::{Place, point_label};
    let at = match place {
        Place::Device { device, point } => Some(format!("{device} {}", point_label(point))),
        Place::Module(m) => Some(format!("module {m}")),
        Place::Simulated => None,
    };
    match (wired, at) {
        (Some((text, tip)), Some(at)) => Some((format!("{text}, {at}"), tip)),
        (Some(w), None) => Some(w),
        (None, Some(_)) => Some((
            place.describe(),
            "Where the point is: the device and its channel.".to_string(),
        )),
        (None, None) => None,
    }
}

/// How many tiles go side by side in `width`, and each one's width.
#[must_use]
pub fn tiles(width: f32, gap: f32) -> (usize, f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cols = (((width + gap) / (TILE + gap)).floor() as usize).clamp(1, 6);
    #[allow(clippy::cast_precision_loss)]
    let col = (width - gap * (cols as f32 - 1.0)) / cols as f32;
    (cols, col.max(1.0))
}

/// A section's heading: the fold arrow, its name and how many points it
/// has. A click folds or unfolds it. `first` puts the list and grid
/// glyphs at the row's right.
pub fn heading(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    title: &str,
    count: usize,
    input: bool,
    tip: &str,
    first: bool,
) {
    let folded = if input {
        chrome.io_fold_in
    } else {
        chrome.io_fold_out
    };
    let arrow = if folded { "▸" } else { "▾" };
    let width = ui.available_width() - BAR;
    ui.horizontal(|ui| {
        ui.set_width(width.max(1.0));
        let how = if folded { "Click to show them." } else { "Click to fold them away." };
        let r = ui
            .add(
                egui::Label::new(RichText::new(format!("{arrow} {title}  {count}")).strong())
                    .sense(Sense::click()),
            )
            .tip(format!("{tip} {how}"));
        chrome.hit(format!("io head {}", title.to_ascii_lowercase()), r.rect);
        if r.clicked() {
            if input {
                chrome.io_fold_in = !folded;
            } else {
                chrome.io_fold_out = !folded;
            }
        }
        if !first {
            return;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let side = crate::glyphs::ROW;
            let grid = crate::glyphs::button_sized(
                ui,
                Icon::Grid,
                side,
                true,
                chrome.io_grid,
                "Tiles: each point's control or value alone, side by side. Double-click a tile for its details.",
            );
            chrome.hit("io grid", grid.rect);
            let list = crate::glyphs::button_sized(
                ui,
                Icon::List,
                side,
                true,
                !chrome.io_grid,
                "A list: one point to a row, with what is wired to it and what it drives.",
            );
            chrome.hit("io list", list.rect);
            if grid.clicked() {
                chrome.io_grid = true;
            }
            if list.clicked() {
                chrome.io_grid = false;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str, kind: SourceKind, fb: &str) -> Source {
        Source {
            name: name.into(),
            kind,
            fb: fb.into(),
        }
    }

    fn card(name: &str, signals: Vec<Source>, driven: bool) -> OutputCard {
        OutputCard {
            name: name.into(),
            signals,
            driven,
        }
    }

    #[test]
    fn an_input_affects_the_outputs_that_read_it_whole_or_by_a_field() {
        let cards = [
            card("LAMP", vec![src("START", SourceKind::Input, "")], true),
            card("MOTOR", vec![src("START_BUTTON", SourceKind::Input, "")], true),
            card("VALVE", vec![src("START.PRESSED", SourceKind::Input, "")], true),
            card("HORN", vec![src("START", SourceKind::Kept, "")], true),
            card("ARR", vec![src("start[2]", SourceKind::Input, "")], true),
        ];
        assert_eq!(affects(&cards, "start"), ["LAMP", "VALVE", "ARR"]);
        assert_eq!(affects(&cards, "START_BUTTON"), ["MOTOR"]);
        assert_eq!(affects(&cards, ""), [] as [String; 0], "no name is no input");
        assert_eq!(affects(&cards, "stärt"), [] as [String; 0]);
        // A name that ends inside a multi-byte letter of a source.
        let odd = [card("X", vec![src("ÄB", SourceKind::Input, "")], true)];
        assert_eq!(affects(&odd, "a"), [] as [String; 0]);
    }

    #[test]
    fn the_summary_names_the_first_and_counts_the_rest() {
        let c = card(
            "LAMP",
            vec![
                src("LATCH", SourceKind::Kept, ""),
                src("T1", SourceKind::Block, "TON"),
                src("START", SourceKind::Input, ""),
                src("STOP", SourceKind::Input, ""),
            ],
            true,
        );
        let s = summary(&Links::Output(Some(&c))).unwrap();
        assert_eq!(
            (s.lead, s.first.as_deref(), s.more),
            ("driven by", Some("start"), 3)
        );
        assert!(s.all.contains("start, stop, timer t1, latch"), "{}", s.all);
        let only_block = card("L", vec![src("C1", SourceKind::Block, "CTU")], true);
        let s = summary(&Links::Output(Some(&only_block))).unwrap();
        assert_eq!((s.first.as_deref(), s.more), (Some("counter c1"), 0));
        let undriven = card("L", vec![], false);
        let s = summary(&Links::Output(Some(&undriven))).unwrap();
        assert_eq!((s.lead, s.first, s.more), ("nothing sets it", None, 0));
        let constant = card("L", vec![], true);
        assert_eq!(
            summary(&Links::Output(Some(&constant))).unwrap().lead,
            "set from constants"
        );
        let s = summary(&Links::Input(Some(vec!["LAMP".into(), "HORN".into()]))).unwrap();
        assert_eq!((s.lead, s.first.as_deref(), s.more), ("affects", Some("lamp"), 1));
        let s = summary(&Links::Input(Some(vec![]))).unwrap();
        assert_eq!((s.lead, s.first, s.more), ("no output depends on it", None, 0));
        assert_eq!(summary(&Links::Input(None)), None);
        assert_eq!(summary(&Links::Output(None)), None);
    }

    #[test]
    fn tiles_fill_the_width_and_never_fewer_than_one() {
        assert_eq!(tiles(224.0, 8.0).0, 2);
        let (cols, col) = tiles(224.0, 8.0);
        #[allow(clippy::cast_precision_loss)]
        let used = col * cols as f32 + 8.0 * (cols as f32 - 1.0);
        assert!((used - 224.0).abs() < 0.01);
        assert_eq!(tiles(60.0, 8.0), (1, 60.0));
        assert_eq!(tiles(0.0, 8.0).0, 1);
        assert_eq!(tiles(-5.0, 8.0), (1, 1.0));
        assert_eq!(tiles(1.0e6, 8.0).0, 6);
        assert_eq!(tiles(f32::NAN, 8.0).0, 1);
    }
}
