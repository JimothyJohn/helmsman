//! The Editor's frame, the same on a box (`box_view`) and in the
//! simulation (`playground_view`), as docs/UI.md lays it out: under the
//! header (the app's one row: menus, the view's actions, the project),
//! the files and program tree on the left, the live diagram in the
//! centre, the ST beside it on demand (View menu), and the right panel's
//! tabs (I/O, Trend, Agent). Both sidebars collapse to a strip;
//! which are open is [`Chrome`], kept per browser by the app.
//!
//! What fills each part is the view's ([`Body`]); where it goes is here.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, TextFormat, Vec2,
    text::LayoutJob, text_edit::TextEditState,
};

use crate::glyphs::Icon;
use crate::st_view::TAP;
use crate::theme::{Theme, theme};
use crate::tips::Tip;

/// Where the app keeps [`Chrome`] in the browser's storage.
pub const CHROME_KEY: &str = "helmsman-canvas/editor";

/// Width of a collapsed sidebar, points.
pub const STRIP: f32 = 48.0;
/// The side of a tab's glyph in a pane's header row, points.
pub const TAB: f32 = TAP * 0.75;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// The tabs a view draws itself ([`Body::tab`]); [`Pane`] places them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Io,
    /// The glyph library's pieces to drag onto the diagram (`palette`).
    Palette,
    Claude,
}

impl Tab {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Io => "I/O",
            Self::Palette => "Palette",
            Self::Claude => "Agent",
        }
    }

    /// The tab button's tooltip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::Io => "I/O: the program's inputs and outputs with their live values.",
            Self::Palette => {
                "Palette: controls and program pieces. Drag one onto the diagram and the Agent adds it."
            }
            Self::Claude => "Agent: ask the agent about the program, or have it propose a change.",
        }
    }
}

/// The bottom panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Bottom {
    #[default]
    Trend,
    Messages,
}

impl Bottom {
    /// Every tab, left to right.
    pub const ALL: [Self; 2] = [Self::Trend, Self::Messages];

    #[must_use]
    pub const fn icon(self) -> Icon {
        match self {
            Self::Trend => Icon::Trend,
            Self::Messages => Icon::Messages,
        }
    }

    /// The tab button's tooltip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::Trend => "Trend: pinned signals over time (Alt+Shift+2).",
            Self::Messages => {
                "Messages: what needs saying now and what happened (connections, deploys, faults and errors), newest last. Red while an error stands or one has come in since you last looked."
            }
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Trend => "trend",
            Self::Messages => "messages",
        }
    }
}

/// A tab of the side panes. Nick, 2026-11-13: two side panels by default
/// was too many; every tab starts in the left pane with the Agent showing,
/// which leaves the diagram as wide as it can be and the focus where it
/// is most of the time. Any tab can be dragged to the right edge to dock
/// it in a right pane (and back), and Alt Shift 5 sends the left pane's
/// tab there and shows the next one on the left. Earlier: Code | Files
/// (2026-11-01), Devices (2026-11-02), History (2026-11-13) on the left,
/// I/O | Palette | Agent on the right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Claude,
    /// The Structured Text editor.
    Code,
    /// The program's inputs and outputs with their live values.
    Io,
    /// The I/O map's devices and their points (`crate::devices`).
    Devices,
    Files,
    /// The open project's revisions, to step through (Nick, 2026-11-13).
    History,
    /// The glyph library's pieces to drag onto the diagram (`palette`).
    Palette,
}

impl Pane {
    /// Every tab, in the left pane's first order.
    pub const ALL: [Self; 7] = [
        Self::Claude,
        Self::Code,
        Self::Io,
        Self::Devices,
        Self::Files,
        Self::History,
        Self::Palette,
    ];

    /// The view's own tab this is, for the three a view draws through
    /// [`Body::tab`].
    #[must_use]
    pub const fn tab(self) -> Option<Tab> {
        match self {
            Self::Claude => Some(Tab::Claude),
            Self::Io => Some(Tab::Io),
            Self::Palette => Some(Tab::Palette),
            Self::Code | Self::Devices | Self::Files | Self::History => None,
        }
    }

    /// The tab button's tooltip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::Files => "Files: the projects and the open one's files (Alt+Shift+0 opens or shuts it).",
            Self::Devices => {
                "Devices: the I/O map, the remote modules and their points (Alt+Shift+4 opens or shuts it)."
            }
            Self::Code => "Code: the Structured Text of the open file (Alt+Shift+1 opens or shuts it).",
            Self::History => {
                "History: every saved version of the open project, newest at the top. Click one, or step with the arrows, to run it."
            }
            Self::Claude | Self::Io | Self::Palette => match self.tab() {
                Some(t) => t.tip(),
                None => "",
            },
        }
    }

    #[must_use]
    pub const fn icon(self) -> Icon {
        match self {
            Self::Files => Icon::Files,
            Self::Devices => Icon::Devices,
            Self::Code => Icon::Code,
            Self::History => Icon::History,
            Self::Claude => Icon::Agent,
            Self::Io => Icon::Io,
            Self::Palette => Icon::Palette,
        }
    }

    /// Its name in the saved layout.
    const fn key(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Devices => "devices",
            Self::Code => "code",
            Self::History => "history",
            Self::Claude => "claude",
            Self::Io => "io",
            Self::Palette => "palette",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    /// Its button's hit name, the same on either side: `pane code` for
    /// the tabs that were the left pane's, `tab claude` for the ones that
    /// were the right panel's (the tests and the tour click by these).
    #[must_use]
    pub fn hit(self) -> String {
        if self.tab().is_some() {
            format!("tab {}", self.key())
        } else {
            format!("pane {}", self.key())
        }
    }
}

/// The left pane's width when first opened, points: one width for every
/// tab (Nick, 2026-11-01); 200 since the third tab (three tab buttons and
/// the hide chevron must fit the row, or the pane grows every frame); 240
/// since 2026-11-05 (Nick: "just a bit wider"); 320 since 2026-11-10 (at
/// 240 the Code tab held 19 characters a row and wrapped a 14-line
/// program to 24 rows; at 320 it holds 27).
pub const LEFT_W: f32 = 320.0;
/// The right panel's width when first opened, points: 236 since
/// 2026-11-05 (the tab buttons and the hide chevron must fit the row, or
/// the panel grows every frame; the Requirements tab is archived since
/// 2026-11-08, the width kept).
pub const RIGHT_W: f32 = 236.0;
/// How many trend colour picks a browser keeps.
pub const MAX_COLORS: usize = 64;

/// The Editor's layout and the Monitor's scan detail (kept per browser)
/// and what was drawn where this frame (tests click by it).
// Independent toggles a browser keeps, not states of one thing.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub struct Chrome {
    /// The left pane is open (not its strip).
    pub left: bool,
    /// The right pane is open, when a tab is docked there.
    pub right: bool,
    /// The left pane's tabs, in order: every one by default.
    pub lefts: Vec<Pane>,
    /// The tabs docked in the right pane, in order: none by default, and
    /// then the right pane is not drawn at all.
    pub rights: Vec<Pane>,
    /// Which of the left pane's tabs shows.
    pub pane: Pane,
    /// Which of the right pane's tabs shows.
    pub side: Pane,
    /// The Trend runs along the bottom, under the diagram (Nick,
    /// 2026-11-01: a long time axis beats a tall one).
    pub trend: bool,
    /// Which of the bottom panel's tabs shows: the Trend or the Messages.
    pub bottom: Bottom,
    /// The Monitor's status tiles show the scan period and counts: an
    /// engineer's choice, and only shown while one is signed in.
    pub scan: bool,
    /// View › Light mode: the light page instead of the dark one.
    pub light: bool,
    /// The source line a block click sent the ST editor to.
    pub line: Option<u32>,
    /// [`Self::line`] not scrolled to yet.
    jump: Option<u32>,
    /// Controls drawn this frame, by name.
    pub hits: Vec<(String, Rect)>,
    /// Where the diagram was drawn this frame (the glyph buttons sit in
    /// its top-right corner).
    pub diagram: Rect,
    /// The find box over the ST pane, while open (Ctrl F).
    pub find: Option<crate::find::Find>,
    /// What the Agent's last change added and removed in the open file,
    /// set by the workspace each frame; drawn only while the editor's text
    /// is what the Agent wrote ([`crate::agent_marks`]).
    pub st_marks: Option<crate::agent_marks::StMarks>,
    /// Word wrap in the Code tab: long lines wrap at the pane's width
    /// (on, the default) or run on behind a sideways scroll bar (Nick,
    /// 2026-11-08). The toggle sits in the Code header.
    pub wrap: bool,
    /// The colours picked for trend signals (Nick, 2026-11-11): a
    /// signal's name, lower-cased, and its place in the trend's palette.
    /// A signal not here takes its lane's colour.
    pub colors: Vec<(String, usize)>,
    /// The style hints of the file in the Code tab (`crate::st_hints`).
    pub hints: crate::st_hints::Hints,
    /// The blocks folded in the Code tab (`crate::st_fold`).
    pub folds: crate::st_fold::Folds,
    /// The Devices tab's Advanced switch (PROTOCOLS.md step 3): today's
    /// New device form, Import device… and Use this device, with their
    /// protocols, addresses and typed points. Off, a device is added by
    /// a click on what the scan found. Remembered per browser.
    pub advanced: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            left: true,
            right: true,
            lefts: Pane::ALL.to_vec(),
            rights: Vec::new(),
            pane: Pane::Claude,
            side: Pane::Claude,
            trend: true,
            bottom: Bottom::Trend,
            scan: false,
            light: false,
            line: None,
            jump: None,
            hits: Vec::new(),
            diagram: Rect::NOTHING,
            find: None,
            st_marks: None,
            wrap: true,
            colors: Vec::new(),
            hints: crate::st_hints::Hints::default(),
            folds: crate::st_fold::Folds::default(),
            advanced: false,
        }
    }
}

impl Chrome {
    /// What is kept per browser, as text. `dock=2` marks the layout of
    /// 2026-11-13 (every tab in one left pane); a text without it is
    /// from before, and its panes are not read back.
    #[must_use]
    pub fn saved(&self) -> String {
        let colors = if self.colors.is_empty() {
            String::new()
        } else {
            let all: Vec<String> = self.colors.iter().map(|(n, i)| format!("{n}:{i}")).collect();
            format!(" colors={}", all.join(","))
        };
        let list = |v: &[Pane]| v.iter().map(|p| p.key()).collect::<Vec<_>>().join(",");
        let rights = if self.rights.is_empty() {
            String::new()
        } else {
            format!(" rights={} side={}", list(&self.rights), self.side.key())
        };
        format!(
            "dock=2 left={} right={} lefts={} pane={}{rights} scan={} trend={} bottom={} light={} wrap={} advanced={}{colors}",
            u8::from(self.left),
            u8::from(self.right),
            list(&self.lefts),
            self.pane.key(),
            u8::from(self.scan),
            u8::from(self.trend),
            self.bottom.key(),
            u8::from(self.light),
            u8::from(self.wrap),
            u8::from(self.advanced)
        )
    }

    /// [`Self::saved`] read back; anything unknown keeps its default.
    #[must_use]
    pub fn restored(text: &str) -> Self {
        let mut c = Self::default();
        let docked = text.split_whitespace().any(|kv| kv == "dock=2");
        let list = |v: &str| v.split(',').filter_map(Pane::from_key).collect::<Vec<_>>();
        for kv in text.split_whitespace() {
            match kv.split_once('=') {
                Some(("left", v)) if docked => c.left = v == "1",
                Some(("right", v)) if docked => c.right = v == "1",
                Some(("lefts", v)) if docked => c.lefts = list(v),
                Some(("rights", v)) if docked => c.rights = list(v),
                Some(("pane", v)) if docked => c.pane = Pane::from_key(v).unwrap_or_default(),
                Some(("side", v)) if docked => c.side = Pane::from_key(v).unwrap_or_default(),
                Some(("scan", v)) => c.scan = v == "1",
                Some(("trend", v)) => c.trend = v == "1",
                Some(("light", v)) => c.light = v == "1",
                Some(("wrap", v)) => c.wrap = v == "1",
                Some(("advanced", v)) => c.advanced = v == "1",
                Some(("colors", v)) => {
                    c.colors = v
                        .split(',')
                        .filter_map(|e| e.rsplit_once(':'))
                        .filter_map(|(n, i)| Some((n.to_string(), i.parse().ok()?)))
                        .filter(|(n, _)| !n.is_empty())
                        .take(MAX_COLORS)
                        .collect();
                }
                Some(("bottom", "messages")) => c.bottom = Bottom::Messages,
                Some(("bottom", "trend")) => c.bottom = Bottom::Trend,
                _ => {}
            }
        }
        c.tidy();
        c
    }

    /// Every tab in exactly one pane, the shown ones among their pane's:
    /// a tab twice keeps its first place, a tab in neither (one a later
    /// release added) goes to the end of the left pane.
    fn tidy(&mut self) {
        let mut seen = Vec::new();
        for list in [&mut self.lefts, &mut self.rights] {
            list.retain(|p| {
                let fresh = !seen.contains(p);
                seen.push(*p);
                fresh
            });
        }
        for p in Pane::ALL {
            if !seen.contains(&p) {
                self.lefts.push(p);
            }
        }
        if !self.lefts.contains(&self.pane) {
            self.pane = self.lefts.first().copied().unwrap_or_default();
        }
        if !self.rights.contains(&self.side) {
            self.side = self.rights.first().copied().unwrap_or_default();
        }
    }

    /// The palette place picked for trend signal `name`, if one was.
    #[must_use]
    pub fn color(&self, name: &str) -> Option<usize> {
        self.colors
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, i)| *i)
    }

    /// Gives trend signal `name` palette place `index`. The oldest pick
    /// goes once [`MAX_COLORS`] are kept.
    pub fn set_color(&mut self, name: &str, index: usize) {
        // What the saved text cannot hold is not a signal's name.
        if name.is_empty() || name.contains([' ', ',', '=']) {
            return;
        }
        self.colors.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.colors.push((name.to_ascii_lowercase(), index));
        if self.colors.len() > MAX_COLORS {
            self.colors.remove(0);
        }
    }

    /// Notes that `key` was drawn at `rect`.
    pub fn hit(&mut self, key: impl Into<String>, rect: Rect) {
        self.hits.push((key.into(), rect));
    }

    /// Where `key` was drawn this frame.
    #[cfg(test)]
    #[must_use]
    pub fn rect(&self, key: &str) -> Option<Rect> {
        self.hits.iter().rev().find(|(k, _)| k == key).map(|(_, r)| *r)
    }

    /// Whether tab `p` is on screen, on either side.
    #[must_use]
    pub fn showing(&self, p: Pane) -> bool {
        (self.left && self.pane == p && self.lefts.contains(&p))
            || (self.right && self.side == p && self.rights.contains(&p))
    }

    /// Opens tab `p` in the pane it is docked in.
    pub fn show(&mut self, p: Pane) {
        if self.rights.contains(&p) {
            self.right = true;
            self.side = p;
        } else {
            self.left = true;
            self.pane = p;
        }
    }

    /// Tab `p`, or its pane shut when that is what shows.
    pub fn toggle(&mut self, p: Pane) {
        if !self.showing(p) {
            self.show(p);
        } else if self.rights.contains(&p) {
            self.right = false;
        } else {
            self.left = false;
        }
    }

    /// Docks tab `p` in the right pane (`right`) or the left one, at the
    /// end, showing it there. When it was the tab its old pane showed,
    /// that pane shows the one that was under it, or the one before when
    /// it was the last.
    pub fn dock(&mut self, p: Pane, right: bool) {
        let (from, to) = if right {
            (&mut self.lefts, &mut self.rights)
        } else {
            (&mut self.rights, &mut self.lefts)
        };
        let Some(at) = from.iter().position(|&q| q == p) else {
            return;
        };
        from.remove(at);
        let next = from.get(at).or_else(|| from.last()).copied();
        to.push(p);
        let next = next.unwrap_or_default();
        if right {
            if self.pane == p {
                self.pane = next;
            }
            self.right = true;
            self.side = p;
        } else {
            if self.side == p {
                self.side = next;
            }
            self.left = true;
            self.pane = p;
        }
    }

    /// Alt Shift 5: the left pane's tab to the right pane, the next one
    /// shown on the left.
    pub fn send_right(&mut self) {
        if self.left && self.lefts.contains(&self.pane) {
            self.dock(self.pane, true);
        }
    }

    /// Whether the Code tab (the ST editor) is on screen.
    #[must_use]
    pub fn code_open(&self) -> bool {
        self.showing(Pane::Code)
    }

    /// Whether the Files tab is on screen.
    #[must_use]
    pub fn files_open(&self) -> bool {
        self.showing(Pane::Files)
    }

    /// Opens the ST editor.
    pub fn show_code(&mut self) {
        self.show(Pane::Code);
    }

    /// Opens the files.
    pub fn show_files(&mut self) {
        self.show(Pane::Files);
    }

    /// Whether the Devices tab is on screen.
    #[must_use]
    pub fn devices_open(&self) -> bool {
        self.showing(Pane::Devices)
    }

    /// Alt Shift 4, View › Devices.
    pub fn toggle_devices(&mut self) {
        self.toggle(Pane::Devices);
    }

    /// Alt Shift 1, View › Structured Text.
    pub fn toggle_code(&mut self) {
        self.toggle(Pane::Code);
    }

    /// Alt Shift 0, View › Files.
    pub fn toggle_files(&mut self) {
        self.toggle(Pane::Files);
    }

    /// Sends the ST editor to `line` (it scrolls there when next drawn).
    pub fn goto(&mut self, line: u32) {
        if line > 0 {
            self.line = Some(line);
            self.jump = Some(line);
        }
    }
}

/// A menu item: a full-width button that closes its menu when clicked,
/// with `tip` on hover (and when disabled: say why). Whether it was.
pub fn menu_item(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    key: &str,
    text: impl Into<RichText>,
    on: bool,
    tip: &str,
) -> bool {
    menu_item_keyed(ui, chrome, key, text, on, "", tip)
}

/// [`menu_item`] with its keyboard shortcut printed at the right.
pub fn menu_item_keyed(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    key: &str,
    text: impl Into<RichText>,
    on: bool,
    shortcut: &str,
    tip: &str,
) -> bool {
    // No `available_width()` here: a menu measures itself against the
    // style's default area (600 pt) before it knows its size, and an item
    // that wide would be remembered. The menu's justified layout fills
    // every item to the widest one instead.
    let mut b = egui::Button::selectable(on, text.into()).min_size(Vec2::new(74.0, TAP * 0.7));
    if !shortcut.is_empty() {
        b = b.shortcut_text(RichText::new(shortcut).weak());
    }
    let r = ui.add(b).tip(tip);
    chrome.hit(key, r.rect);
    if r.clicked() {
        ui.close();
        return true;
    }
    false
}

/// The View menu's part that is the Editor's frame: the tabs, the right
/// pane and the move to it, the bottom panel.
pub fn view_menu(ui: &mut egui::Ui, chrome: &mut Chrome, tabs: &[Tab]) {
    use crate::hotkeys::{Action, label};
    let (code, files, right) = (chrome.code_open(), chrome.files_open(), chrome.right);
    let docked = !chrome.rights.is_empty();
    let ctx = ui.ctx().clone();
    let k = |a| label(&ctx, a);
    if menu_item_keyed(
        ui,
        chrome,
        "view st",
        "Structured Text",
        code,
        &k(Action::St),
        "Shows or hides the code pane.",
    ) {
        chrome.toggle_code();
    }
    if menu_item_keyed(
        ui,
        chrome,
        "view files",
        "Files",
        files,
        &k(Action::Files),
        "Shows or hides the projects and their files.",
    ) {
        chrome.toggle_files();
    }
    let devices = chrome.devices_open();
    if menu_item_keyed(
        ui,
        chrome,
        "view devices",
        "Devices",
        devices,
        &k(Action::Devices),
        "Shows or hides the I/O map's devices.",
    ) {
        chrome.toggle_devices();
    }
    if menu_item_keyed(
        ui,
        chrome,
        "view panel",
        "Right pane",
        right && docked,
        &k(Action::Panel),
        "Shows or hides the pane on the right, once a tab is docked there (drag a tab to the right edge, or Alt+Shift+5).",
    ) && docked
    {
        chrome.right = !right;
    }
    if menu_item_keyed(
        ui,
        chrome,
        "view dock right",
        "Tab to the right",
        false,
        &k(Action::DockRight),
        "Docks the left pane's tab in a pane on the right and shows the next one on the left. Drag a tab back to the left pane to return it.",
    ) {
        chrome.send_right();
    }
    let trend = chrome.trend;
    if menu_item_keyed(
        ui,
        chrome,
        "view trend",
        "Trend",
        trend,
        &k(Action::Trend),
        "Shows or hides the trend and messages along the bottom.",
    ) {
        chrome.trend = !trend;
    }
    for p in Pane::ALL {
        let Some(t) = p.tab().filter(|t| tabs.contains(t)) else {
            continue;
        };
        let on = chrome.showing(p);
        if menu_item(
            ui,
            chrome,
            &format!("view tab {}", p.key()),
            format!("    {}", t.label()),
            on,
            t.tip(),
        ) {
            chrome.show(p);
        }
    }
}

/// The characters selected in the ST editor `id` (either order), when
/// any are.
#[must_use]
pub fn selection(ctx: &egui::Context, id: egui::Id) -> Option<(usize, usize)> {
    let r = TextEditState::load(ctx, id)?.cursor.char_range()?;
    (r.primary.index != r.secondary.index).then_some((r.secondary.index.into(), r.primary.index.into()))
}

/// Drops the ST editor `id`'s selection, leaving the caret where it was.
pub fn deselect(ctx: &egui::Context, id: egui::Id) {
    if let Some(mut state) = TextEditState::load(ctx, id)
        && let Some(r) = state.cursor.char_range()
    {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(r.primary)));
        state.store(ctx, id);
    }
}

/// Selects characters `a..b` of the ST editor `id` (the tests' mouse).
#[cfg(test)]
pub fn select(ctx: &egui::Context, id: egui::Id, a: usize, b: usize) {
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
        egui::text::CCursor::new(a),
        egui::text::CCursor::new(b),
    )));
    state.store(ctx, id);
}

/// The id of the ST editor showing `file` of `project` in `view`. One id
/// per file: egui keeps the undo history, caret and scroll under it, and
/// a shared id would put one file's text into another on Undo.
#[must_use]
pub fn source_id(view: &str, project: &str, file: &str) -> egui::Id {
    egui::Id::unique((view, project, file))
}

/// Undoes (or, `redo`, redoes) the last edit of the ST editor `id` on
/// `text`, through the editor's own undo stack; whether anything changed.
pub fn undo(ctx: &egui::Context, id: egui::Id, text: &mut String, redo: bool) -> bool {
    let Some(mut state) = TextEditState::load(ctx, id) else {
        return false;
    };
    let mut undoer = state.undoer();
    let current = (state.cursor.char_range().unwrap_or_default(), text.clone());
    let next = if redo {
        undoer.redo(&current)
    } else {
        undoer.undo(&current)
    }
    .cloned();
    let Some((cursor, t)) = next else {
        return false;
    };
    let changed = t != *text;
    *text = t;
    state.cursor.set_char_range(Some(cursor));
    state.set_undoer(undoer);
    state.store(ctx, id);
    changed
}

/// What a view puts in the Editor's frame.
pub trait Body {
    /// The view's actions in the header row, after the menus: run
    /// controls and a short status in the simulation, the link's status
    /// on a box. Short: the row is one line with the project on its right.
    fn toolbar(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// Whether [`Self::notices`] has anything to say.
    fn has_notices(&self) -> bool;
    /// What needs saying now (a fault, a deploy under way or refused, a
    /// stale file with its Reload): the first rows of the Messages tab,
    /// only while there is one (Nick, 2026-11-11: not a line under the
    /// header).
    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// Whether one of them is an error that still stands (a fault, a
    /// refused save or deploy): the Messages tab stays red while it does.
    fn alarm(&self) -> bool;
    /// The left pane's Files tab: the projects, the open one's files under
    /// it.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's Devices tab: the I/O map's devices and their
    /// points, live (`crate::devices`).
    fn devices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's Code tab: the ST editor (read-only on a box).
    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's History tab: the open project's revisions, newest
    /// first, each a short description of its change.
    fn history(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The Trend along the bottom: pins and the plot.
    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The Messages tab beside it: the view's log (`messages::Log`).
    fn messages(&mut self) -> &mut crate::messages::Log;
    /// The right panel's tabs this view has, in order.
    fn tabs(&self) -> &'static [Tab];
    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab, chrome: &mut Chrome);
    /// Asks the Agent `text` as if it had been typed in the Agent tab
    /// and sent (a palette piece clicked or dropped).
    fn ask_agent(&mut self, text: &str, ctx: &egui::Context);
    /// The diagram with its breadcrumbs; a block click calls
    /// [`Chrome::goto`].
    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
}

/// A tab of a collapsed sidebar, as [`strip`] draws it.
struct StripTab {
    icon: Icon,
    /// Its hit's name, as in the open pane's header.
    key: String,
    tip: &'static str,
    on: bool,
}

/// A collapsed sidebar's strip (Nick, 2026-11-11): the pane's tabs as
/// glyphs down it, each opening the pane on that tab, and under them the
/// rest of the strip as one button that opens it where it was. Returns
/// the tab clicked, if one was, and the button.
fn strip(
    ui: &mut egui::Ui,
    tabs: &[StripTab],
    left: bool,
    hover: &str,
    hits: &mut Vec<(String, Rect)>,
) -> (Option<usize>, egui::Response) {
    let mut picked = None;
    let side = ui.available_width().clamp(STRIP - 16.0, TAB);
    for (k, t) in tabs.iter().enumerate() {
        let r =
            crate::glyphs::tab(ui, t.icon, side, t.on, None).tip(format!("{} Opens the pane on it.", t.tip));
        hits.push((t.key.clone(), r.rect));
        if r.clicked() {
            picked = Some(k);
        }
    }
    (picked, strip_toggle(ui, left, hover))
}

/// The rest of a collapsed sidebar's strip as one button: a chevron
/// pointing `left` or right, tinted under the pointer.
fn strip_toggle(ui: &mut egui::Ui, left: bool, hover: &str) -> egui::Response {
    let size = Vec2::new(
        ui.available_width().max(STRIP - 8.0),
        ui.available_height().max(TAP),
    );
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let r = r.on_hover_text(hover);
    if r.hovered() {
        let tint = ui.style().interact(&r).bg_fill;
        ui.painter().rect_filled(rect, CornerRadius::same(4), tint);
    }
    chevron(ui, rect.center(), left, r.hovered());
    r
}

/// An open sidebar's hide button, the same chevron in the header row.
fn header_toggle(ui: &mut egui::Ui, left: bool, hover: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(TAP * 0.75), Sense::click());
    let r = r.on_hover_text(hover);
    if r.hovered() {
        let tint = ui.style().interact(&r).bg_fill;
        ui.painter().rect_filled(rect, CornerRadius::same(4), tint);
    }
    chevron(ui, rect.center(), left, r.hovered());
    r
}

/// A 16 pt stroked chevron at `c`.
fn chevron(ui: &egui::Ui, c: Pos2, left: bool, hot: bool) {
    let s = 8.0;
    let dir = if left { -1.0 } else { 1.0 };
    let tip = Pos2::new(c.x + dir * s * 0.5, c.y);
    let back = c.x - dir * s * 0.5;
    let t = theme(ui);
    let stroke = Stroke::new(2.5, if hot { t.accent } else { t.muted });
    ui.painter().line_segment([Pos2::new(back, c.y - s), tip], stroke);
    ui.painter().line_segment([Pos2::new(back, c.y + s), tip], stroke);
}

/// How far from the window's right edge a tab dragged out of the left
/// pane docks on the right, points.
pub const DOCK_ZONE: f32 = 96.0;

/// A tab's content in whichever pane it is docked.
fn pane_body(
    ui: &mut egui::Ui,
    p: Pane,
    body: &mut dyn Body,
    chrome: &mut Chrome,
    hits: &mut Vec<(String, Rect)>,
    asked: &mut Option<crate::palette::Item>,
) {
    let scroll = |salt: &str| egui::ScrollArea::vertical().id_salt(salt);
    match p {
        Pane::Files => {
            scroll("ed-files-scroll").show(ui, |ui| body.files(ui, chrome));
        }
        Pane::Devices => {
            scroll("ed-devices-scroll").show(ui, |ui| body.devices(ui, chrome));
        }
        Pane::Code => body.st(ui, chrome),
        Pane::History => {
            scroll("ed-history-scroll").show(ui, |ui| body.history(ui, chrome));
        }
        Pane::Palette => *asked = crate::palette::show(ui, hits),
        Pane::Claude => body.tab(ui, Tab::Claude, chrome),
        Pane::Io => body.tab(ui, Tab::Io, chrome),
    }
}

/// One side pane, `right` or left: its tabs along the top, each one
/// draggable to the other side, the hide chevron, and the shown tab;
/// shut, its strip. Where it was drawn.
fn side_pane(
    ui: &mut egui::Ui,
    right: bool,
    tabs: &[Pane],
    body: &mut dyn Body,
    chrome: &mut Chrome,
    hits: &mut Vec<(String, Rect)>,
    asked: &mut Option<crate::palette::Item>,
) -> Rect {
    let (mut open, mut on) = if right {
        (chrome.right, chrome.side)
    } else {
        (chrome.left, chrome.pane)
    };
    let (open0, on0) = (open, on);
    let mut hide = false;
    let (strip_panel, panel) = if right {
        (
            egui::Panel::right("ed-side-strip")
                .exact_size(STRIP)
                .resizable(false),
            egui::Panel::right("ed-side")
                .resizable(true)
                .default_size(RIGHT_W),
        )
    } else {
        (
            egui::Panel::left("ed-files-strip")
                .exact_size(STRIP)
                .resizable(false),
            egui::Panel::left("ed-files")
                .resizable(true)
                .default_size(LEFT_W)
                .min_size(120.0),
        )
    };
    let (other, chevron_tip) = if right {
        ("the left pane", "Hides the pane (Alt+Shift+3).")
    } else {
        ("the window's right edge", "Hides the pane.")
    };
    let shown = egui::Panel::show_switched(ui, &mut open, strip_panel, panel, |ui, is_open| {
        let toggle = if is_open {
            let mut t = None;
            // The tabs shrink to share one row with the chevron (seven at
            // full size are wider than the pane's 320 pt), down to 24 pt,
            // and wrap below that: a row that overflows widens the pane
            // every frame. The row's width is read here, before it starts:
            // inside a wrapped row `available_width` is not what is left.
            let row = ui.available_width();
            let gap_x = ui.spacing().item_spacing.x;
            #[allow(clippy::cast_precision_loss)]
            let n = tabs.len() as f32;
            let size = ((row - TAP * 0.75 - gap_x * (n + 1.0)) / n.max(1.0)).clamp(24.0, TAB);
            let fill = row - n * (size + gap_x) - TAP * 0.75 - gap_x;
            ui.horizontal_wrapped(|ui| {
                if right {
                    t = Some(header_toggle(ui, false, chevron_tip));
                }
                for &p in tabs {
                    let r =
                        crate::glyphs::tab_sensed(ui, p.icon(), size, on == p, None, Sense::click_and_drag())
                            .tip(format!(
                                "{} Drag it to {other} to dock it there; double-click hides the pane.",
                                p.tip()
                            ));
                    r.dnd_set_drag_payload(p);
                    // A tab is a control first: the finger, not the grab
                    // hand a draggable thing gets (`theme::pointer_cursor`
                    // leaves the dragged ones alone).
                    if r.hovered() && !r.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    hits.push((p.hit(), r.rect));
                    if r.clicked() {
                        on = p;
                    }
                    if r.double_clicked() {
                        hide = true;
                    }
                }
                if !right {
                    // The chevron at the right edge when the row has room.
                    if fill > 0.0 {
                        ui.add_space(fill);
                    }
                    t = Some(header_toggle(ui, true, chevron_tip));
                }
            });
            ui.separator();
            pane_body(ui, on, body, chrome, hits, asked);
            t
        } else {
            let strip_tabs: Vec<StripTab> = tabs
                .iter()
                .map(|&p| StripTab {
                    icon: p.icon(),
                    key: p.hit(),
                    tip: p.tip(),
                    on: on == p,
                })
                .collect();
            let hover = if right {
                "Shows the pane (Alt+Shift+3)."
            } else {
                "Shows the pane."
            };
            let (picked, t) = strip(ui, &strip_tabs, right, hover, hits);
            if let Some(k) = picked {
                on = tabs[k];
                hide = true;
            }
            Some(t)
        };
        if let Some(t) = toggle {
            hits.push((if right { "right" } else { "left" }.to_string(), t.rect));
            t.clicked() || hide
        } else {
            false
        }
    });
    if shown.inner {
        open = !open;
    }
    // The body may have moved the pane itself (a double click on a file
    // opens the Code tab); the tabs' copy wins only when it did not.
    let (open_now, on_now) = if right {
        (&mut chrome.right, &mut chrome.side)
    } else {
        (&mut chrome.left, &mut chrome.pane)
    };
    if *open_now == open0 {
        *open_now = open;
    }
    if *on_now == on0 {
        *on_now = on;
    }
    shown.response.rect
}

/// A tab dragged out of its pane: where it would dock lit, the tab under
/// the pointer, and on release over that place, the tab docked there.
fn dock_drop(ui: &egui::Ui, chrome: &mut Chrome, left: Option<Rect>, right: Option<Rect>) {
    let ctx = ui.ctx();
    let Some(p) = egui::DragAndDrop::payload::<Pane>(ctx).map(|p| *p) else {
        return;
    };
    let screen = ctx.content_rect();
    let to_right = !chrome.rights.contains(&p);
    let zone = if to_right {
        let edge = Rect::from_min_max(
            Pos2::new(screen.right() - DOCK_ZONE, screen.top()),
            screen.right_bottom(),
        );
        right.map_or(edge, |r| r.union(edge))
    } else {
        let edge = Rect::from_min_max(
            screen.left_top(),
            Pos2::new(screen.left() + DOCK_ZONE, screen.bottom()),
        );
        left.map_or(edge, |r| r.union(edge))
    };
    chrome.hit("dock zone", zone);
    let at = ctx.pointer_interact_pos();
    let over = at.is_some_and(|a| zone.contains(a));
    let t = theme(ui);
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("dock-zone"));
    let painter = ctx.layer_painter(layer);
    painter.rect_filled(
        zone,
        CornerRadius::same(6),
        t.accent.gamma_multiply(if over { 0.22 } else { 0.08 }),
    );
    painter.rect_stroke(
        zone.shrink(1.0),
        CornerRadius::same(6),
        Stroke::new(2.0, t.accent.gamma_multiply(if over { 0.9 } else { 0.4 })),
        egui::StrokeKind::Inside,
    );
    if let Some(a) = at {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("dock-drag"));
        let painter = ctx.layer_painter(layer);
        let r = Rect::from_center_size(a, Vec2::splat(TAB));
        painter.rect_filled(r, CornerRadius::same(5), t.shade.gamma_multiply(0.9));
        crate::glyphs::paint(&painter, r.shrink(TAB * 0.24), p.icon(), t.fg);
    }
    if over && ctx.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ctx);
        chrome.dock(p, to_right);
    }
}

/// One frame of the Editor.
pub fn show(ui: &mut egui::Ui, chrome: &mut Chrome, body: &mut dyn Body) {
    let mut hits = Vec::new();
    // A palette piece clicked or dropped on the diagram: the Agent's.
    let mut asked: Option<crate::palette::Item> = None;
    let have = body.tabs();
    let has = |p: &Pane| p.tab().is_none_or(|t| have.contains(&t));
    let lefts: Vec<Pane> = chrome.lefts.iter().copied().filter(has).collect();
    let rights: Vec<Pane> = chrome.rights.iter().copied().filter(has).collect();
    if !lefts.contains(&chrome.pane)
        && let Some(&p) = lefts.first()
    {
        chrome.pane = p;
    }
    if !rights.contains(&chrome.side)
        && let Some(&p) = rights.first()
    {
        chrome.side = p;
    }
    let left = (!lefts.is_empty()).then(|| side_pane(ui, false, &lefts, body, chrome, &mut hits, &mut asked));
    let right =
        (!rights.is_empty()).then(|| side_pane(ui, true, &rights, body, chrome, &mut hits, &mut asked));
    chrome.hits.extend(hits);
    dock_drop(ui, chrome, left, right);

    // The Messages tab is where every notice and error is (Nick,
    // 2026-11-11): red while an error stands or has come in unread, and
    // pulsing for a few seconds after a new one. A new error opens the
    // panel if it was shut, so it is never out of sight.
    let now = ui.input(|i| i.time);
    let standing = body.alarm();
    let alarm = body.messages().alarm(now, standing);
    if body.messages().take_fresh() && !chrome.trend {
        chrome.trend = true;
        chrome.bottom = Bottom::Messages;
    }
    if body.messages().flashing(now) {
        ui.ctx().request_repaint();
    }
    let errors = body.messages().errors();
    let mut trend = chrome.trend;
    egui::Panel::bottom("ed-trend")
        .resizable(true)
        // Tabs, the signals' row and the slider take about 110 pt: at
        // 200 the plot under them had 50.
        .default_size(280.0)
        .show_collapsible(ui, &mut trend, |ui| {
            ui.horizontal(|ui| {
                for b in Bottom::ALL {
                    let alarm = alarm.filter(|_| b == Bottom::Messages);
                    let r = crate::glyphs::tab(ui, b.icon(), TAB, chrome.bottom == b, alarm).tip(b.tip());
                    chrome.hit(format!("bottom {}", b.key()), r.rect);
                    if r.clicked() {
                        chrome.bottom = b;
                    }
                }
                if errors > 0 {
                    let red = theme(ui).ink(crate::st_view::brand::RED);
                    let r = ui
                        .label(egui::RichText::new(errors.to_string()).strong().color(red))
                        .tip(if errors == 1 {
                            "One error in the Messages.".to_string()
                        } else {
                            format!("{errors} errors in the Messages.")
                        });
                    chrome.hit("bottom errors", r.rect);
                }
            });
            ui.separator();
            match chrome.bottom {
                Bottom::Trend => body.trend(ui, chrome),
                Bottom::Messages => {
                    if body.has_notices() {
                        ui.horizontal_wrapped(|ui| body.notices(ui, chrome));
                        ui.separator();
                    }
                    body.messages().ui(ui, chrome);
                }
            }
        });
    chrome.trend = trend;
    // The open file's style hints, once the typing has settled.
    if let Some(line) = chrome.hints.settled(now) {
        body.messages().info(now, line);
    }

    egui::CentralPanel::no_frame().show(ui, |ui| {
        chrome.diagram = ui.available_rect_before_wrap();
        body.diagram(ui, chrome);
    });
    if let Some(item) = asked.or_else(|| crate::palette::dropped(ui.ctx(), chrome.diagram)) {
        chrome.show(Pane::Claude);
        body.ask_agent(item.ask, ui.ctx());
    }
}

/// The ST, with line numbers, errors marked (`(line, column)`), warnings
/// marked the same way but lighter (an underline, no ground, and the
/// gutter number in the warning colour), and
/// the line [`Chrome::goto`] sent it to scrolled into view and marked. A
/// `&mut &str` is read-only (on a box); a `String` is edited in place, up
/// to `limit` characters. Lines wrap at the pane's width while
/// [`Chrome::wrap`] is on, a wrapped line numbered once, on its first
/// row; off, each line is one row and the code scrolls sideways.
pub fn st_editor(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut dyn egui::TextBuffer,
    diags: &[(u32, u32)],
    warns: &[(u32, u32)],
    limit: usize,
    chrome: &mut Chrome,
) -> egui::Response {
    let font = FontId::monospace(crate::theme::MONO);
    let row = ui.fonts_mut(|f| f.row_height(&font));
    let found = crate::find::row(ui, &mut chrome.find, id, text.as_str());
    let wrap_on = chrome.wrap;
    let mut scroll = if wrap_on {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    }
    .id_salt(id.with("scroll"));
    if let Some(line) = chrome.jump.take() {
        #[allow(clippy::cast_precision_loss)]
        let y = (line.saturating_sub(4)) as f32 * row;
        scroll = scroll.vertical_scroll_offset(y);
        // The caret to the start of the line, for an editor that has one.
        if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
            let at = text
                .as_str()
                .split_inclusive('\n')
                .take(line.saturating_sub(1) as usize)
                .map(|l| l.chars().count())
                .sum::<usize>();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
            state.store(ui.ctx(), id);
        }
    }
    // The current match: selected and scrolled to.
    if let Some(f) = &found
        && f.reveal
        && let Some(r) = f.current.and_then(|k| f.ranges.get(k))
    {
        let s = text.as_str();
        let line = s[..r.start].matches('\n').count();
        #[allow(clippy::cast_precision_loss)]
        let y = line.saturating_sub(4) as f32 * row;
        scroll = scroll.vertical_scroll_offset(y);
        crate::find::select(ui.ctx(), id, s, r.clone(), true);
    }
    let t = theme(ui);
    let now = ui.input(|i| i.time);
    let hinted: Vec<helmsman_iec::hints::Hint> = chrome.hints.of(text.as_str(), now).to_vec();
    let hints: Vec<(u32, u32)> = hinted.iter().map(|h| (h.pos.line, h.pos.col)).collect();
    // Folding (`crate::st_fold`): a folded block's inner lines are laid
    // out with no height. A fold opens when a line inside it has to be
    // seen: a problem's, the one a block click went to, the caret moving
    // in; and all of them while the find box is open, so every match shows.
    let blocks = crate::st_fold::foldable(text.as_str());
    let caret = TextEditState::load(ui.ctx(), id)
        .and_then(|s| s.cursor.char_range())
        .map(|r| {
            let ends = text
                .as_str()
                .chars()
                .take(r.primary.index.into())
                .filter(|c| *c == '\n')
                .count();
            u32::try_from(ends + 1).unwrap_or(u32::MAX)
        });
    let shown: Vec<u32> = diags
        .iter()
        .chain(warns)
        .map(|(l, _)| *l)
        .chain(chrome.line)
        .collect();
    if found.is_some() {
        chrome.folds.open_all();
    }
    let folded = chrome.folds.settle(id, &blocks, &shown, caret);
    let lines = Lines {
        errors: diags,
        warnings: warns,
        hints: &hints,
        folded: &folded,
    };
    let marks: Vec<(std::ops::Range<usize>, Color32)> = found
        .as_ref()
        .map(|f| {
            f.ranges
                .iter()
                .enumerate()
                .map(|(k, r)| {
                    let strength = if f.current == Some(k) { 0.75 } else { 0.3 };
                    (r.clone(), crate::st_view::brand::YELLOW.gamma_multiply(strength))
                })
                .collect()
        })
        .unwrap_or_default();
    let at = chrome.line;
    let agent = chrome
        .st_marks
        .clone()
        .filter(|m| m.applies(text.as_str()))
        .unwrap_or_default();
    let added = agent.added.clone();
    scroll
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let digit = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
                let gutter_w = digit * 3.0;
                // The fold arrows' column, left of the line numbers.
                let arrow_w = (digit * 1.4).ceil();
                // Slack for the editor's own margin and the scroll bar: wider
                // than the pane and a resizable pane grows to fit, every frame.
                let wrap = if wrap_on {
                    (ui.available_width() - gutter_w - arrow_w - 2.0 * ui.spacing().item_spacing.x - 32.0)
                        .max(60.0)
                } else {
                    f32::INFINITY
                };
                // The width the text edit really wraps at (its margins
                // come off the width it is given) is told by its layouter;
                // the gutter uses the last frame's, or the estimate on the
                // first.
                let wrap_key = id.with("wrap");
                let edit_wrap = if wrap_on {
                    ui.data(|d| d.get_temp::<f32>(wrap_key)).unwrap_or(wrap - 8.0)
                } else {
                    f32::INFINITY
                };
                // The rows the text takes at that width: each source line
                // numbered on its first row, its wrapped rows blank.
                let mut job = highlight(text.as_str(), &lines, at, &added, &font, edit_wrap, &t);
                crate::find::mark(&mut job, &marks);
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                let mut gutter = LayoutJob::default();
                for (n, number) in numbering(galley.rows.iter().map(|r| r.ends_with_newline)) {
                    if crate::st_fold::hidden(&folded, u32::try_from(n).unwrap_or(u32::MAX)) {
                        // A row of no height, as the code beside it has.
                        gutter.append("\n", 0.0, gone(&font));
                        continue;
                    }
                    let bad = diags.iter().any(|(l, _)| *l as usize == n);
                    let here = at.is_some_and(|l| l as usize == n);
                    let warned = warns.iter().any(|(l, _)| *l as usize == n);
                    let color = if bad {
                        crate::theme::theme(ui).ink(RED)
                    } else if warned {
                        crate::theme::theme(ui).ink(crate::st_view::brand::WARN)
                    } else if hints.iter().any(|(l, _)| *l as usize == n) {
                        t.ink(HINT)
                    } else if here {
                        t.accent
                    } else {
                        t.muted
                    };
                    let label = if number {
                        format!("{n:>3}\n")
                    } else {
                        "   \n".to_string()
                    };
                    gutter.append(&label, 0.0, TextFormat::simple(font.clone(), color));
                }
                gutter.wrap.max_width = f32::INFINITY;
                fold_arrows(ui, id, &galley, arrow_w, row, &blocks, &folded, chrome);
                ui.label(gutter);
                // Wrap off: the editor is as wide as its longest line (plus
                // its margins) and the scroll area scrolls to it; the pane
                // keeps its width because the scroll area clips.
                let desired = if wrap_on { wrap } else { galley.size().x + 16.0 };
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                    let wrap = if wrap_on { wrap } else { f32::INFINITY };
                    ui.data_mut(|d| d.insert_temp(wrap_key, wrap));
                    let mut job = highlight(buf.as_str(), &lines, at, &added, &font, wrap, &t);
                    crate::find::mark(&mut job, &marks);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                // A tooltip would sit over the code under the pointer while
                // the engineer types; the pane's tab button has one.
                let out = egui::TextEdit::multiline(text) // tip: exempt
                    .id(id)
                    .code_editor()
                    .desired_width(desired)
                    .char_limit(limit)
                    .desired_rows(24)
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui);
                // The gutter was laid out a frame behind a change of width.
                if wrap_on
                    && ui
                        .data(|d| d.get_temp::<f32>(wrap_key))
                        .is_some_and(|w| (w - edit_wrap).abs() > 0.01)
                {
                    ui.ctx().request_repaint();
                }
                removed_rules(ui, &out, &agent.removed, chrome);
                hint_tips(ui, &out, &hinted, chrome);
                fold_marks(ui, &out, &folded, &font, chrome);
                out.response.response
            })
            .inner
        })
        .inner
}

/// The Code header's word-wrap toggle, drawn as "code wrap" for the
/// tests: on, long lines wrap at the pane's width; off, each line is one
/// row and the code scrolls sideways ([`Chrome::wrap`]).
pub fn wrap_toggle(ui: &mut egui::Ui, chrome: &mut Chrome) {
    let tip = if chrome.wrap {
        "Word wrap is on: a long line wraps at the pane's width. Click for one row per line, scrolled sideways."
    } else {
        "Word wrap is off: each line is one row, scrolled sideways. Click to wrap long lines at the pane's width."
    };
    let r = crate::glyphs::button_sized(
        ui,
        crate::glyphs::Icon::Wrap,
        crate::glyphs::ROW,
        true,
        chrome.wrap,
        tip,
    );
    chrome.hit("code wrap", r.rect);
    if r.clicked() {
        chrome.wrap = !chrome.wrap;
    }
}

/// The Code header's name: the open file, truncated to leave the row's
/// glyphs and notes their room, drawn as "code file" for the tests.
pub fn file_header(ui: &mut egui::Ui, chrome: &mut Chrome, name: &str, tip: &str) {
    // Two glyphs, their spacing and a short note stay on the row.
    let room = (ui.available_width() - 2.0 * (crate::glyphs::ROW + 6.0) - 120.0).max(60.0);
    let r = ui
        .scope(|ui| {
            ui.set_max_width(room);
            ui.add(egui::Label::new(RichText::new(name).strong()).truncate())
        })
        .inner
        .tip(tip);
    chrome.hit("code file", r.rect);
}

/// The format of a line a fold hides: laid out, so the text edit still
/// holds it, with no height and no ink.
fn gone(font: &FontId) -> TextFormat {
    TextFormat {
        line_height: Some(0.0),
        ..TextFormat::simple(font.clone(), Color32::TRANSPARENT)
    }
}

/// The top of each source line's first row in `galley`, by line number
/// less one, and the bottom of the last row.
fn line_tops(galley: &egui::Galley) -> (Vec<f32>, f32) {
    let mut tops = Vec::new();
    let mut first = true;
    for r in &galley.rows {
        if first {
            tops.push(r.rect().top());
        }
        first = r.ends_with_newline;
    }
    (tops, galley.rows.last().map_or(0.0, |r| r.rect().bottom()))
}

/// The fold arrows (Nick, 2026-11-11: an arrow in the gutter on every
/// block that folds): a column `width` wide and as tall as the code, an
/// arrow on the first line of each block, pointing down while the block
/// is open and right while it is folded. A click folds or opens it. The
/// hits are `fold <line>`.
#[allow(clippy::too_many_arguments)]
fn fold_arrows(
    ui: &mut egui::Ui,
    id: egui::Id,
    galley: &egui::Galley,
    width: f32,
    row: f32,
    blocks: &[crate::st_fold::Foldable],
    folded: &[crate::st_fold::Foldable],
    chrome: &mut Chrome,
) {
    let (column, _) = ui.allocate_exact_size(Vec2::new(width, galley.size().y), Sense::hover());
    let (tops, _) = line_tops(galley);
    let t = theme(ui);
    for b in blocks {
        // An arrow on a line that is itself folded away has nowhere to be.
        if crate::st_fold::hidden(folded, b.first) {
            continue;
        }
        let Some(top) = tops.get(b.first.saturating_sub(1) as usize) else {
            continue;
        };
        let rect = Rect::from_min_size(
            Pos2::new(column.left(), column.top() + top),
            Vec2::new(width, row),
        );
        let shut = chrome.folds.folded(b.first);
        let tip = if shut {
            format!("Opens this block: {} line(s) are folded away.", b.hides())
        } else {
            format!(
                "Folds this block: its {} line(s) are hidden until you open it again.",
                b.hides()
            )
        };
        let r = ui
            .interact(rect, id.with(("fold", b.first)), Sense::click())
            .on_hover_text(tip);
        chrome.hit(format!("fold {}", b.first), rect);
        let color = if r.hovered() { t.accent } else { t.muted };
        let c = rect.center();
        let s = (width * 0.32).min(row * 0.22);
        let points = if shut {
            vec![
                Pos2::new(c.x - s * 0.6, c.y - s),
                Pos2::new(c.x + s * 0.8, c.y),
                Pos2::new(c.x - s * 0.6, c.y + s),
            ]
        } else {
            vec![
                Pos2::new(c.x - s, c.y - s * 0.6),
                Pos2::new(c.x + s, c.y - s * 0.6),
                Pos2::new(c.x, c.y + s * 0.8),
            ]
        };
        ui.painter()
            .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        if r.clicked() {
            chrome.folds.toggle(b.first);
        }
    }
}

/// After the first line of each folded block: how many lines it hides.
/// The hits are `folded <line>`.
fn fold_marks(
    ui: &egui::Ui,
    out: &egui::text_edit::TextEditOutput,
    folded: &[crate::st_fold::Foldable],
    font: &FontId,
    chrome: &mut Chrome,
) {
    let t = theme(ui);
    for b in folded {
        // Folded inside an outer fold: not on screen.
        if crate::st_fold::hidden(folded, b.first) {
            continue;
        }
        // The last row of the block's first line.
        let mut line = 1;
        let mut end = None;
        for r in &out.galley.rows {
            if line == b.first {
                end = Some(r.rect());
            }
            if r.ends_with_newline {
                line += 1;
            }
            if line > b.first {
                break;
            }
        }
        let Some(end) = end else { continue };
        let at = out.galley_pos + Vec2::new(end.right() + 10.0, end.center().y);
        let rect = ui.painter().text(
            at,
            egui::Align2::LEFT_CENTER,
            if b.hides() == 1 {
                "… 1 line".to_string()
            } else {
                format!("… {} lines", b.hides())
            },
            font.clone(),
            t.muted,
        );
        chrome.hit(format!("folded {}", b.first), rect);
    }
}

/// A style hint's colour: a calm blue, apart from the red of an error
/// and the orange of a warning.
pub const HINT: Color32 = Color32::from_rgb(0x6E, 0xC6, 0xFF);

/// What each hinted line says, on hover (Nick, 2026-11-11: style hints
/// are a lighter level: an underline and a word on request, no list).
/// The hits are `hint <line>`.
fn hint_tips(
    ui: &egui::Ui,
    out: &egui::text_edit::TextEditOutput,
    hints: &[helmsman_iec::hints::Hint],
    chrome: &mut Chrome,
) {
    if hints.is_empty() {
        return;
    }
    // Each source line's rows, top to bottom, by 1-based line number.
    let mut spans: Vec<(f32, f32)> = Vec::new();
    let mut first = true;
    for r in &out.galley.rows {
        let (top, bottom) = (r.rect().top(), r.rect().bottom());
        if first {
            spans.push((top, bottom));
        } else if let Some(s) = spans.last_mut() {
            s.1 = bottom;
        }
        first = r.ends_with_newline;
    }
    let left = out.galley_pos.x;
    let right = out.response.rect.right();
    let mut said: Vec<u32> = Vec::new();
    for h in hints {
        let line = h.pos.line;
        let Some(&(top, bottom)) = spans.get(line.saturating_sub(1) as usize) else {
            continue;
        };
        if said.contains(&line) {
            continue;
        }
        said.push(line);
        let band = Rect::from_min_max(
            Pos2::new(left, out.galley_pos.y + top),
            Pos2::new(right, out.galley_pos.y + bottom),
        );
        let text: Vec<String> = hints
            .iter()
            .filter(|o| o.pos.line == line)
            .map(|o| format!("Hint: {}.", o.msg))
            .collect();
        ui.interact(band, out.response.id.with(("hint", line)), egui::Sense::hover())
            .on_hover_text(format!("{}\nA hint never stops a deploy.", text.join("\n")));
        chrome.hit(format!("hint {line}"), band);
    }
}

/// Where the Agent's change removed lines: a red rule between the rows
/// they stood between, its tooltip the removed text (a text editor holds
/// only the file's own lines, so the gone ones are told, not inserted).
fn removed_rules(
    ui: &egui::Ui,
    out: &egui::text_edit::TextEditOutput,
    removed: &[(u32, Vec<String>)],
    chrome: &mut Chrome,
) {
    if removed.is_empty() {
        return;
    }
    let rows = &out.galley.rows;
    // The top of each source line's first row, by 1-based line number.
    let mut tops: Vec<f32> = Vec::new();
    let mut first = true;
    for r in rows {
        if first {
            tops.push(r.rect().top());
        }
        first = r.ends_with_newline;
    }
    let bottom = rows.last().map_or(0.0, |r| r.rect().bottom());
    let left = out.galley_pos.x;
    let right = out.response.rect.right();
    for (line, gone) in removed {
        let y = out.galley_pos.y
            + tops
                .get(line.saturating_sub(1) as usize)
                .copied()
                .unwrap_or(bottom);
        let band = Rect::from_min_max(Pos2::new(left, y - 3.0), Pos2::new(right, y + 3.0));
        ui.painter().line_segment(
            [Pos2::new(left, y), Pos2::new(right, y)],
            Stroke::new(2.0, crate::st_view::brand::RED.gamma_multiply(0.85)),
        );
        let n = gone.len();
        ui.painter().text(
            Pos2::new(right - 4.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("−{n}"),
            FontId::monospace(crate::theme::MIN_TEXT),
            crate::theme::theme(ui).ink(crate::st_view::brand::RED),
        );
        let r = ui.interact(
            band,
            out.response.id.with(("removed", *line)),
            egui::Sense::hover(),
        );
        let mut tip = format!("The Agent removed {n} line(s) here:");
        for g in gone {
            tip.push('\n');
            tip.push_str(g);
        }
        r.on_hover_text(tip);
        chrome.hit(format!("removed lines {line}"), band);
    }
}

/// The gutter beside laid-out rows (`ends_with_newline` per row): each
/// row's source line, and whether the row is that line's first (the one
/// that shows the number; a wrapped line's other rows stay blank).
#[must_use]
pub fn numbering(rows: impl Iterator<Item = bool>) -> Vec<(usize, bool)> {
    let mut n = 1;
    let mut first = true;
    rows.map(|ends| {
        let out = (n, first);
        first = ends;
        if ends {
            n += 1;
        }
        out
    })
    .collect()
}

/// The lines of a source with a problem, `(line, column)`: errors, and
/// warnings (which never stop the build).
#[derive(Debug, Clone, Copy)]
pub struct Lines<'a> {
    pub errors: &'a [(u32, u32)],
    pub warnings: &'a [(u32, u32)],
    /// Style hints (`helmsman_iec::hints`): the lightest mark, and only
    /// on a line with no error or warning.
    pub hints: &'a [(u32, u32)],
    /// The blocks folded (`crate::st_fold`): their inner lines are laid
    /// out with no height.
    pub folded: &'a [crate::st_fold::Foldable],
}

/// The ST as a layout wrapped at `wrap` points: a line with a diagnostic
/// on a red ground, its column to the end of the word underlined; a line
/// with only a warning (`warns`) has no ground, just that word
/// underlined, thicker and in the warning colour; the
/// line `at` (a block click sent the editor there) on a grey ground; a
/// line the Agent's last change added (`added`) on a green ground.
#[must_use]
pub fn highlight(
    text: &str,
    lines: &Lines,
    at: Option<u32>,
    added: &[u32],
    font: &FontId,
    wrap: f32,
    t: &Theme,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(font.clone(), t.code);
    let bad_ground = t.bad;
    let (diags, warns) = (lines.errors, lines.warnings);
    // Each run of code in its own colour (`crate::st_syntax`); a word a
    // diagnostic underlines keeps the page's ink.
    let runs = crate::st_syntax::tints(text);
    let tinted = |job: &mut LayoutJob, piece: &str, base: usize, format: &TextFormat| {
        let mut at = 0;
        while at < piece.len() {
            let (end, tint) = crate::st_syntax::tint_at(&runs, base + at);
            let to = end.saturating_sub(base).clamp(at + 1, piece.len());
            // A run never ends inside a character, but a clamp may.
            let to = (to..=piece.len())
                .find(|&k| piece.is_char_boundary(k))
                .unwrap_or(piece.len());
            let mut f = format.clone();
            f.color = tint.color(t);
            f.italics = tint == crate::st_syntax::Tint::Comment;
            job.append(&piece[at..to], 0.0, f);
            at = to;
        }
    };
    let mut base = 0;
    for (k, line) in text.split_inclusive('\n').enumerate() {
        let n = u32::try_from(k + 1).unwrap_or(u32::MAX);
        let line_base = base;
        base += line.len();
        if crate::st_fold::hidden(lines.folded, n) {
            job.append(line, 0.0, gone(font));
            continue;
        }
        let error = diags.iter().find(|(l, _)| *l == n);
        let warn = warns.iter().find(|(l, _)| *l == n);
        let hint = lines.hints.iter().find(|(l, _)| *l == n);
        let Some(&(_, col)) = error.or(warn).or(hint) else {
            let format = if at == Some(n) {
                TextFormat {
                    background: t.rule,
                    ..plain.clone()
                }
            } else if added.contains(&n) {
                TextFormat {
                    background: crate::st_view::brand::GREEN.gamma_multiply(0.22),
                    ..plain.clone()
                }
            } else {
                plain.clone()
            };
            tinted(&mut job, line, line_base, &format);
            continue;
        };
        let (ground, line_color, width) = if error.is_some() {
            (
                TextFormat {
                    background: bad_ground,
                    ..plain.clone()
                },
                RED,
                1.5,
            )
        } else if warn.is_some() {
            (plain.clone(), crate::st_view::brand::WARN, 3.0)
        } else {
            (plain.clone(), t.ink(HINT), 1.5)
        };
        // Columns count characters from 1.
        let start = line
            .char_indices()
            .nth(col.saturating_sub(1) as usize)
            .map_or(line.trim_end_matches('\n').len(), |(i, _)| i);
        let rest = &line[start..];
        let word = rest
            .char_indices()
            .find(|(i, c)| *i > 0 && !(c.is_alphanumeric() || *c == '_'))
            .map_or(rest.trim_end_matches('\n').len(), |(i, _)| i)
            .max(
                rest.chars()
                    .next()
                    .filter(|c| *c != '\n')
                    .map_or(0, char::len_utf8),
            );
        tinted(&mut job, &line[..start], line_base, &ground);
        job.append(
            &rest[..word],
            0.0,
            TextFormat {
                underline: Stroke::new(width, line_color),
                color: t.fg,
                ..ground.clone()
            },
        );
        tinted(&mut job, &rest[word..], line_base + start + word, &ground);
    }
    job.wrap.max_width = wrap;
    job
}
