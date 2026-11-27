//! The I/O tab's cards (Nick, 2026-11-16): one per input and output, the
//! same in the simulation and on a box. A card says three things and no
//! more: the point (its control or its live value and name), what it is
//! in the real world (the device wired to it, else the module and channel
//! it is on), and its part in the control scheme, from the diagram alone
//! with no scan needed: the first output an input affects, the first
//! signal that drives an output, and "…" when there are more.
//!
//! Everything else is one gesture away. A click on the card (or on the
//! "…") opens its details in place, and another closes them (Nick,
//! 2026-11-26; a double-click before): every source in three groups that
//! tell apart at a glance (signals read from outside, state the program
//! keeps from one scan to the next, the function blocks it reads), the
//! wiring in full and how often the name is in the code. The three glyphs
//! at the card's right go to the point: `< >` finds it in the code, and
//! again steps to its next use ([`crate::find::Find::name`]); the diagram
//! glyph lights every block that reads or writes it, frames the first
//! use, and again the next ([`crate::st_view::Viewer::show_signal`]); the
//! graph glyph puts it on the Trend. A click on an output's card also
//! lights the path that sets it, and on an input's, where it is.
//!
//! The tab shows its cards one to a row, or as tiles (the list and grid
//! glyphs at its top), and Inputs and Outputs each fold away behind the
//! arrow of their heading. Both are remembered ([`crate::editor::Chrome`]).
//!
//! Every width is capped to the column: a resizable panel grows to the
//! widest row it is shown. The tab leaves 12 pt for its scroll bar.

use eframe::egui::{self, Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, Vec2};
use helmsman_diagram::{Source, SourceKind};

use crate::assign::Designation;
use crate::editor::Chrome;
use crate::glyphs::Icon;
use crate::theme::{BODY, theme};
use crate::tips::Tip;

/// What the tab leaves free at its right for the scroll bar, pt.
pub const BAR: f32 = 12.0;
/// The narrowest a tile gets before the grid drops a column, pt: 88 since
/// the side panes' ribbon (2026-11-26) left the right pane 185 pt, where
/// two tiles of 96 no longer fit and the grid went to one a row.
const TILE: f32 = 88.0;

/// The width of a card's glyph strip, pt: three glyphs and their gaps. An
/// input's control leaves this much free at its right end, so the glyphs
/// that show over it never cover a click target.
#[must_use]
pub fn strip_width() -> f32 {
    3.0 * crate::glyphs::ROW * 0.85 + 2.0 * 2.0
}

fn reserve_id() -> egui::Id {
    egui::Id::NULL.with("io card head reserve")
}

/// A head's controls at its right end, `w` wide: the hover glyphs are drawn
/// left of them, over the name, never over a control.
pub fn reserve_right(ui: &egui::Ui, w: f32) {
    ui.data_mut(|d| d.insert_temp(reserve_id(), w));
}

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
    /// Still simulated (a SIM badge and a dashed outline) or on a device
    /// (solid, naming it); `None` when the point is neither.
    pub designation: Option<Designation>,
    /// It is on the Trend: its trend glyph shows on.
    pub trended: bool,
    /// The program's diagram has a block that reads or writes it: the
    /// diagram glyph has something to show (off, and saying why, when not).
    pub on_diagram: bool,
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
    /// Put it on the Trend, or take it off.
    pub trend: bool,
}

/// An output's first line, or an input's where it has no control: its
/// glyph (a tile has none), and `text` in `color`, cut to the column.
/// `wrap`: a row's line wraps at the column so a long name shows whole;
/// a tile's is cut to its narrow width.
pub fn value(ui: &mut egui::Ui, glyph: Option<&'static str>, color: Color32, text: &str, on: Option<bool>) {
    value_with(ui, glyph, color, color, text, on);
}

/// A BOOL's glyph looks on or off (Nick, 2026-11-27): lit on a soft disc
/// of `on_color` when on, `off_color` dimmed to 35% when off. A point that
/// is no BOOL (`on` is `None`) is drawn in `on_color` as it is.
pub fn state_glyph(
    ui: &mut egui::Ui,
    glyph: &'static str,
    size: f32,
    on_color: Color32,
    off_color: Color32,
    on: Option<bool>,
) {
    let ink = match on {
        Some(false) => off_color.gamma_multiply(0.35),
        _ => on_color,
    };
    let r = crate::glyph_lib::show(ui, glyph, size, ink, crate::glyph_lib::tip(glyph));
    if on == Some(true) {
        ui.painter()
            .circle_filled(r.rect.center(), size * 0.62, on_color.gamma_multiply(0.18));
        crate::glyph_lib::paint(ui.painter(), r.rect, glyph, ink);
    }
}

/// A BOOL's icon as the control of an input: [`state_glyph`]'s look on a
/// rounded ground that eases in on hover and press, as the glyph buttons
/// do. `sense` is a click for a switch, click and drag for a push button.
pub fn state_button(
    ui: &mut egui::Ui,
    glyph: &'static str,
    size: f32,
    on_color: Color32,
    off_color: Color32,
    on: Option<bool>,
    sense: Sense,
) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let ground = crate::theme::eased_fill(ui, &r, Color32::TRANSPARENT);
    ui.painter().rect_filled(rect, 5, ground);
    crate::theme::focus_ring(ui, &r, rect, 5);
    let ink = match on {
        Some(false) => off_color.gamma_multiply(0.35),
        _ => on_color,
    };
    if on == Some(true) {
        ui.painter()
            .circle_filled(rect.center(), size * 0.62, on_color.gamma_multiply(0.18));
    }
    crate::glyph_lib::paint(ui.painter(), rect, glyph, ink);
    r
}

/// [`value`] with its own colour for the glyph while the point is off: a
/// lamp is green on and red off (Nick, 2026-11-27), its name muted.
pub fn value_with(
    ui: &mut egui::Ui,
    glyph: Option<&'static str>,
    color: Color32,
    glyph_off: Color32,
    text: &str,
    on: Option<bool>,
) {
    ui.horizontal(|ui| {
        if let Some(glyph) = glyph {
            state_glyph(ui, glyph, crate::glyphs::ROW * 0.85, color, glyph_off, on);
        }
        // One line, cut at the card's edge with its whole text on hover
        // (Nick, 2026-11-27: the compact view on one line).
        // A reading is its name (Inter, cut to what is left) and, at the
        // card's right end, its value (mono, a slot as wide as its text,
        // never cut): numeric columns are right-aligned. The hover has
        // "name = value" whole.
        if let Some((name, val)) = text.split_once(" = ") {
            let slot = crate::theme::live_slot(ui, val, 0);
            let room = (ui.available_width() - slot - ui.spacing().item_spacing.x).max(20.0);
            ui.allocate_ui_with_layout(
                Vec2::new(room, crate::glyphs::ROW),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(room);
                    let label =
                        egui::Label::new(RichText::new(name).size(BODY).color(color)).selectable(false);
                    ui.add(label.truncate()).tip(text.to_string());
                },
            );
            crate::theme::live(ui, val, color, 0).tip(text.to_string());
        } else {
            let label = egui::Label::new(RichText::new(text).size(BODY).color(color)).selectable(false);
            ui.add(label.truncate()).tip(text.to_string());
        }
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
    let (fill, stroke) = look(row.tile, row.selected, &t);
    let sim = row.designation.as_ref().is_some_and(Designation::is_simulated);
    // A simulated card's outline is dashed, drawn over the frame below.
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(if sim { Stroke::NONE } else { stroke })
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
            let r = head(ui);
            designation_line(ui, chrome, row);
            return r;
        }
        // Glyphs a little under the row's size so name and glyphs share
        // one line in the narrow right pane too (Nick, 2026-11-27).
        let side = crate::glyphs::ROW * 0.85;
        let gap = 2.0;
        // The glyphs show while the pointer is over the card (where it was
        // drawn last frame, so they do not flicker as they appear), while
        // its details are open, and while one has keyboard focus (Nick,
        // 2026-11-27); a closed card at rest is its point alone, using the
        // room the glyphs would take.
        let focus_id = id.with("glyph focus");
        let hovered = was.is_some_and(|w| {
            ui.input(|i| i.pointer.hover_pos())
                .is_some_and(|p| w.contains(p) && ui.clip_rect().contains(p))
        });
        let focused = ui.data(|d| d.get_temp::<bool>(focus_id)).unwrap_or(false);
        let glyphs = row.open || hovered || focused;
        // The card's three glyphs: find in the code, show on the diagram,
        // put on the Trend.
        let mut glyph_row = |ui: &mut egui::Ui| {
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
                row.on_diagram,
                false,
                &if row.on_diagram {
                    format!("Shows {name} on the diagram: every block that reads or writes it. Click again for its next use.")
                } else {
                    format!("{name} is not on the diagram: the program never reads or writes it.")
                },
            );
            chrome.hit(format!("io diagram {name}"), diagram.rect);
            asked.diagram = diagram.clicked();
            // Nick, 2026-11-26: a graph glyph puts the point on the
            // Trend at the bottom (on while it is there).
            let trend = crate::glyphs::button_sized(
                ui,
                Icon::Trend,
                side,
                true,
                row.trended,
                &if row.trended {
                    format!("Takes {name} off the Trend.")
                } else {
                    format!("Adds {name} to the Trend at the bottom.")
                },
            );
            chrome.hit(format!("io trend {name}"), trend.rect);
            asked.trend = trend.clicked();
            let has = code.has_focus() || diagram.has_focus() || trend.has_focus();
            ui.data_mut(|d| d.insert_temp(focus_id, has));
        };
        // The head always has the card's full width, so nothing moves when
        // the pointer enters or leaves (Nick, 2026-11-27). The glyphs are
        // drawn over the right end of that line, on a backdrop of the
        // card's fill with a soft fade at its left edge; they come after
        // the head, so their clicks win over the control under them.
        ui.data_mut(|d| d.insert_temp(reserve_id(), 0.0_f32));
        let head_out = ui.allocate_ui_with_layout(Vec2::new(inner, side), Layout::top_down(Align::Min), |ui| {
            ui.set_width(inner);
            head(ui)
        });
        let r = head_out.inner;
        if glyphs {
            let line = head_out.response.rect;
            let w = strip_width();
            let line_right = line.right() - ui.data(|d| d.get_temp::<f32>(reserve_id())).unwrap_or(0.0);
            let fade = 18.0;
            let h = side.max(line.height().min(side + 6.0));
            let x1 = line_right;
            let x0 = x1 - w;
            let back = Rect::from_min_max(egui::pos2(x0 - fade, line.top()), egui::pos2(x1, line.top() + h));
            let solid = Rect::from_min_max(egui::pos2(x0, back.top()), back.right_bottom());
            let clear = fill.gamma_multiply(0.0);
            let mut mesh = egui::Mesh::default();
            let (l, r_) = (back.left(), x0);
            let (top, bot) = (back.top(), back.bottom());
            mesh.colored_vertex(egui::pos2(l, top), clear);
            mesh.colored_vertex(egui::pos2(r_, top), fill);
            mesh.colored_vertex(egui::pos2(r_, bot), fill);
            mesh.colored_vertex(egui::pos2(l, bot), clear);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            ui.painter().add(egui::Shape::mesh(mesh));
            ui.painter().rect_filled(solid, 0.0, fill);
            let mut g = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(solid)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            g.spacing_mut().item_spacing.x = gap;
            glyph_row(&mut g);
        }
        if row.open {
            // The rest is in the details: the SIM badge (the dashed
            // outline says it on the card), the wiring, the links.
            if sim {
                badge(ui, chrome, row);
            }
            if let Some((text, tip)) = &row.real {
                let label = egui::Label::new(RichText::new(text).size(BODY).color(t.muted)).wrap();
                let l = ui.add(label).tip(tip.clone());
                chrome.hit(format!("wired {}", row.key), l.rect);
            }
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
        }
        r
    });
    let rect = out.response.rect;
    ui.data_mut(|d| d.insert_temp(id, rect));
    chrome.hit(format!("io card {name}"), rect);
    if sim {
        dashed_outline(ui.painter(), rect, stroke);
        chrome.hit(format!("io dashed {name}"), rect);
    } else if row.designation.is_some() {
        chrome.hit(format!("io solid {name}"), rect);
    }
    // One click on the card (not on its glyphs or its control, which are
    // in front of it) opens or closes its details: its dependencies and
    // its device (Nick, 2026-11-26; a double-click before).
    asked.tapped = resp.clicked();
    asked.details |= asked.tapped;
    let what = if row.open {
        "Click to close its details."
    } else {
        "Click for its details: what it drives and its device."
    };
    let tip = if row.input {
        format!("{name}: shows where it is on the diagram. {what}\n{}", row.place)
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

/// `rect`'s outline as dashes, rounded corners cut square.
pub fn dashed_outline(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(&corners, stroke, 5.0, 3.0));
}

/// The word SIM as a card shows it.
fn badge_text(ui: &egui::Ui) -> RichText {
    RichText::new(Designation::BADGE)
        .strong()
        .size(BODY)
        .color(theme(ui).muted)
}

/// A simulated card's badge: its own line under the first.
fn badge(ui: &mut egui::Ui, chrome: &mut Chrome, row: &Row) {
    let name = row.key.to_ascii_lowercase();
    let r = ui
        .add(egui::Label::new(badge_text(ui)).selectable(false))
        .tip(format!("{name} is simulated: no device is wired to it."));
    chrome.hit(format!("io sim {name}"), r.rect);
}

/// A tile's one line under its value: SIM, or the device and channel.
fn designation_line(ui: &mut egui::Ui, chrome: &mut Chrome, row: &Row) {
    let t = theme(ui);
    match &row.designation {
        Some(Designation::Simulated) => badge(ui, chrome, row),
        Some(d @ Designation::Physical { .. }) => {
            let name = row.key.to_ascii_lowercase();
            let text = RichText::new(d.label()).size(BODY).color(t.muted);
            let r = ui
                .add(egui::Label::new(text).truncate().selectable(false))
                .tip(format!("{name} is on {}.", d.label()));
            chrome.hit(format!("io phys {name}"), r.rect);
        }
        None => {}
    }
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
