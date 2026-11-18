//! The diagram's right-click menu. On the diagram the reader moves blocks,
//! folds, zooms, selects and toggles inputs; the code is written in the
//! editor or by the Agent (Nick, 2026-11-18). So the menu holds no change
//! to the program: a block of a program port has its io.map items
//! ("Assign to device I/O…", "Back to simulated I/O"), and any block has
//! "Chat about this" while the diagram's lines are the text the host's
//! files hold ([`Viewer::chat_ok`]).

use eframe::egui::{self, Pos2, Rect, RichText, Vec2};
use helmsman_diagram::layout::{PORT_ROW, in_port};
use helmsman_diagram::{NodeId, NodeKind};

use crate::editor::Chrome;
use crate::st_view::{Cam, TAP, Viewer, node_near};
use crate::theme::BODY;
use crate::tips::Tip;

/// The longest a pin's pointer target is, in screen points.
const PIN_REACH: f32 = 16.0;
/// The widest a popup is, in points.
const POPUP_W: f32 = 380.0;

/// A right click the host has not taken yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gesture {
    pub at: Pos2,
    pub node: NodeId,
}

impl Viewer {
    /// The block under `p`: the block of a pin within reach of an input
    /// port, else the nearest block.
    pub(crate) fn hit_at(&self, cam: Cam, p: Pos2) -> Option<NodeId> {
        let (x, y) = cam.to_world(p);
        // On the diagram as drawn; a fold (several blocks as one) has
        // nothing to ask about until it is opened, but for a comparison
        // folded with its literal.
        let d = self.shown();
        let reach = (PIN_REACH / cam.zoom).max(PORT_ROW / 2.0);
        let pin = d
            .nodes
            .iter()
            .flat_map(|n| (0..n.ins.len()).map(move |k| (n, k)))
            .map(|(n, k)| {
                let (px, py) = in_port(n, k);
                (n.id, (px - x).hypot(py - y))
            })
            .filter(|(_, dist)| *dist <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((node, _)) = pin {
            return self.base_block(node);
        }
        node_near(d, x, y, TAP / 2.0 / cam.zoom).and_then(|n| self.base_block(n))
    }

    /// Whether screen point `p` is within reach of an input pin of the
    /// diagram as drawn (a press there is the pin's, not empty space).
    pub(crate) fn on_pin(&self, cam: Cam, p: Pos2) -> bool {
        let (x, y) = cam.to_world(p);
        let reach = (PIN_REACH / cam.zoom).max(PORT_ROW / 2.0);
        self.shown()
            .nodes
            .iter()
            .flat_map(|n| (0..n.ins.len()).map(move |k| in_port(n, k)))
            .any(|(px, py)| (px - x).hypot(py - y) <= reach)
    }

    /// What io.map can do with the program port `node` reads (an input's
    /// Read block) or writes (an output's Write block), on the program's
    /// own diagram.
    pub(crate) fn port_block(&self, node: NodeId) -> Option<&crate::assign::PortMenu> {
        use crate::device_form::Dir;
        if !self.scopes[self.scope].prefix.is_empty() {
            return None;
        }
        let (path, dir) = match &self.diagram().node(node).kind {
            NodeKind::Read { path } => (path, Dir::In),
            NodeKind::Write { path } => (path, Dir::Out),
            _ => return None,
        };
        self.assign
            .iter()
            .find(|m| m.dir == dir && m.port.eq_ignore_ascii_case(path.trim()))
    }

    /// A right-click on an on-diagram control (an input's switch or force
    /// button sits over its Read block, so the diagram never sees the
    /// click) is a right click on that block all the same.
    pub(crate) fn secondary_on_controls(&mut self, ui: &egui::Ui, controls: &[(String, Rect)]) {
        let Some(p) = ui.input(|i| {
            i.pointer
                .button_clicked(egui::PointerButton::Secondary)
                .then(|| i.pointer.interact_pos())
                .flatten()
        }) else {
            return;
        };
        if !controls.iter().any(|(_, r)| r.contains(p)) {
            return;
        }
        let cam = self.cams[self.scope];
        if let Some(n) = self.hit_at(cam, p) {
            self.pick = Some(crate::st_view::Pick::Node(n));
            self.gesture_at(p, n);
        }
    }

    /// Takes a right click made at `at` on block `node`: the host opens
    /// the menu when it holds an item for the block.
    pub(crate) fn gesture_at(&mut self, at: Pos2, node: NodeId) {
        if self.port_block(node).is_some() || self.chat_ok {
            self.gesture = Some(Gesture { at, node });
        }
    }
}

/// What the menu holds for one block.
#[derive(Default, Debug)]
struct Entries {
    /// The port this block is, with what io.map offers on it.
    assign: Option<crate::assign::PortMenu>,
    /// The block "Chat about this" is about ([`crate::chat_about`]).
    chat: Option<NodeId>,
}

fn entries(v: &Viewer, node: NodeId) -> Entries {
    Entries {
        assign: v.port_block(node).cloned(),
        chat: v.chat_ok.then_some(node),
    }
}

struct Open {
    at: Pos2,
    entries: Entries,
}

/// The menu a right click opens.
#[derive(Default)]
pub struct Menu {
    open: Option<Open>,
    /// A port's assignment asked for in the menu, for the host to take.
    assign: Option<(String, crate::assign_ui::PortAsk)>,
    /// "Chat about this" chosen in the menu, for the host to take.
    chat: Option<NodeId>,
    /// The Agent tab holds as many chips as it takes: the menu item is
    /// off and says why. The host sets it before [`Self::show`].
    pub chat_full: bool,
}

impl std::fmt::Debug for Menu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Menu")
            .field("open", &self.open.is_some())
            .finish_non_exhaustive()
    }
}

impl Menu {
    /// Opens the menu for a gesture of `v`'s diagram.
    pub fn open(&mut self, v: &Viewer, g: Gesture) {
        self.open = Some(Open {
            at: g.at,
            entries: entries(v, g.node),
        });
    }

    /// Whether the menu is open (it takes Escape).
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The block the menu asked to chat about, once; the menu is closed
    /// then.
    pub fn take_chat(&mut self) -> Option<NodeId> {
        self.chat.take()
    }

    /// The port the menu asked to assign (or put back on simulated I/O),
    /// once; the menu is closed then.
    pub fn take_assign(&mut self) -> Option<(String, crate::assign_ui::PortAsk)> {
        self.assign.take()
    }

    /// Draws the open menu inside `area`.
    pub fn show(&mut self, ctx: &egui::Context, chrome: &mut Chrome, area: Rect) {
        let Some(o) = self.open.take() else {
            return;
        };
        let width = (area.width() - 24.0).clamp(120.0, POPUP_W);
        let mut assign: Option<(String, crate::assign_ui::PortAsk)> = None;
        let mut chat: Option<NodeId> = None;
        let chat_full = self.chat_full;
        let r = egui::Area::new(egui::Id::unique("diagram-menu-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(o.at)
            .constrain_to(area)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(width);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let e = &o.entries;
                    if let Some(m) = &e.assign {
                        if row_on(
                            ui,
                            chrome,
                            "diagram menu assign",
                            "Assign to device I/O…",
                            m.assign.is_ok(),
                            m.assign.as_ref().err().map_or(
                                "Pick a free channel of a device, of the same direction and type, added to the project or not yet. Its line in io.map is named for this variable.",
                                String::as_str,
                            ),
                        ) {
                            assign = Some((m.port.clone(), crate::assign_ui::PortAsk::Assign));
                        }
                        if let Some(back) = &m.back
                            && row_on(
                                ui,
                                chrome,
                                "diagram menu back to simulated",
                                "Back to simulated I/O",
                                back.is_ok(),
                                back.as_ref().err().map_or(
                                    "Puts this back on a simulated input or output. The device channel goes back under + Add I/O, as Remove leaves it.",
                                    String::as_str,
                                ),
                            )
                        {
                            assign = Some((m.port.clone(), crate::assign_ui::PortAsk::Back));
                        }
                    }
                    if let Some(n) = e.chat
                        && row_on(
                            ui,
                            chrome,
                            "diagram menu chat",
                            "Chat about this",
                            !chat_full,
                            if chat_full {
                                crate::chat_about::FULL
                            } else {
                                "Puts this block and its statement in the Agent tab, with what drives it and what it drives. Nothing is sent until you type what to ask and send it."
                            },
                        )
                    {
                        chat = Some(n);
                    }
                });
            });
        chrome.hit("diagram popup", r.response.rect);
        if assign.is_some() {
            self.assign = assign;
            return;
        }
        if chat.is_some() {
            self.chat = chat;
            return;
        }
        if crate::theme::dismissed(ctx, "diagram-menu-popup", &[r.response.rect]) {
            return;
        }
        self.open = Some(o);
    }
}

/// `row`, off when `on` is false: a full-width row of a menu, big enough
/// for a finger.
fn row_on(ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, on: bool, tip: &str) -> bool {
    let w = ui.available_width();
    let r = ui
        .add_enabled(
            on,
            egui::Button::new(RichText::new(text).size(BODY)).min_size(Vec2::new(w, TAP)),
        )
        .tip(tip);
    chrome.hit(key, r.rect);
    r.clicked()
}
