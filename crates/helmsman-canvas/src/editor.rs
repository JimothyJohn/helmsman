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

use crate::st_view::TAP;
use crate::theme::{Theme, theme};
use crate::tips::Tip;

/// Where the app keeps [`Chrome`] in the browser's storage.
pub const CHROME_KEY: &str = "helmsman-canvas/editor";

/// Width of a collapsed sidebar, points.
pub const STRIP: f32 = 30.0;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// The right panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Io,
    Claude,
}

impl Tab {
    /// Every tab, in the panel's order.
    pub const ALL: [Self; 2] = [Self::Io, Self::Claude];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Io => "I/O",
            Self::Claude => "Agent",
        }
    }

    /// The tab button's tooltip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::Io => "The program's inputs and outputs with their live values.",
            Self::Claude => "Ask the agent about the program, or have it propose a change.",
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Claude => "claude",
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
    const fn key(self) -> &'static str {
        match self {
            Self::Trend => "trend",
            Self::Messages => "messages",
        }
    }
}

/// The left pane's tabs (Nick, 2026-11-01: Code | Files, as the right
/// panel has I/O | Agent; 2026-11-02: Devices between them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Files,
    /// The I/O map's devices and their points (`crate::devices`).
    Devices,
    /// The Structured Text editor.
    Code,
}

impl Pane {
    /// Every tab, left to right.
    pub const ALL: [Self; 3] = [Self::Files, Self::Devices, Self::Code];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Devices => "Devices",
            Self::Code => "Code",
        }
    }

    /// The tab button's tooltip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::Files => "The projects and the open one's files (Alt+Shift+0 opens or shuts it).",
            Self::Devices => {
                "The I/O map: the remote modules and their points (Alt+Shift+4 opens or shuts it)."
            }
            Self::Code => "The Structured Text of the open file (Alt+Shift+1 opens or shuts it).",
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Devices => "devices",
            Self::Code => "code",
        }
    }
}

/// The left pane's width when first opened, points: one width for every
/// tab (Nick, 2026-11-01); 200 since the third tab (three tab buttons and
/// the hide chevron must fit the row, or the pane grows every frame); 240
/// since 2026-11-05 (Nick: "just a bit wider").
pub const LEFT_W: f32 = 240.0;
/// The right panel's width when first opened, points: 236 since
/// 2026-11-05 (the tab buttons and the hide chevron must fit the row, or
/// the panel grows every frame; the Requirements tab is archived since
/// 2026-11-08, the width kept).
pub const RIGHT_W: f32 = 236.0;

/// The Editor's layout and the Monitor's scan detail (kept per browser)
/// and what was drawn where this frame (tests click by it).
// Independent toggles a browser keeps, not states of one thing.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub struct Chrome {
    /// The left pane (Files | Code) is open.
    pub left: bool,
    /// The right panel (tabs) is open.
    pub right: bool,
    /// Which of the left pane's tabs shows.
    pub pane: Pane,
    /// The Trend runs along the bottom, under the diagram (Nick,
    /// 2026-11-01: a long time axis beats a tall one).
    pub trend: bool,
    /// Which of the bottom panel's tabs shows: the Trend or the Messages.
    pub bottom: Bottom,
    pub tab: Tab,
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
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            left: true,
            right: true,
            pane: Pane::Files,
            trend: true,
            bottom: Bottom::Trend,
            tab: Tab::Io,
            scan: false,
            light: false,
            line: None,
            jump: None,
            hits: Vec::new(),
            diagram: Rect::NOTHING,
            find: None,
            st_marks: None,
            wrap: true,
        }
    }
}

impl Chrome {
    /// What is kept per browser, as text.
    #[must_use]
    pub fn saved(&self) -> String {
        format!(
            "left={} right={} pane={} tab={} scan={} trend={} bottom={} light={} wrap={}",
            u8::from(self.left),
            u8::from(self.right),
            self.pane.key(),
            self.tab.key(),
            u8::from(self.scan),
            u8::from(self.trend),
            self.bottom.key(),
            u8::from(self.light),
            u8::from(self.wrap)
        )
    }

    /// [`Self::saved`] read back; anything unknown keeps its default.
    #[must_use]
    pub fn restored(text: &str) -> Self {
        let mut c = Self::default();
        for kv in text.split_whitespace() {
            match kv.split_once('=') {
                Some(("left", v)) => c.left = v == "1",
                Some(("right", v)) => c.right = v == "1",
                // `st=1` is from before the tabs, when the ST had a pane of
                // its own.
                Some(("pane", "code") | ("st", "1")) => c.pane = Pane::Code,
                Some(("pane", "files")) => c.pane = Pane::Files,
                Some(("pane", "devices")) => c.pane = Pane::Devices,
                Some(("scan", v)) => c.scan = v == "1",
                Some(("trend", v)) => c.trend = v == "1",
                Some(("light", v)) => c.light = v == "1",
                Some(("wrap", v)) => c.wrap = v == "1",
                Some(("bottom", "messages")) => c.bottom = Bottom::Messages,
                Some(("bottom", "trend")) => c.bottom = Bottom::Trend,
                Some(("tab", v)) => {
                    if let Some(t) = Tab::ALL.into_iter().find(|t| t.key() == v) {
                        c.tab = t;
                    }
                }
                _ => {}
            }
        }
        c
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

    /// Whether the Code tab (the ST editor) is on screen.
    #[must_use]
    pub fn code_open(&self) -> bool {
        self.left && self.pane == Pane::Code
    }

    /// Whether the Files tab is on screen.
    #[must_use]
    pub fn files_open(&self) -> bool {
        self.left && self.pane == Pane::Files
    }

    /// Opens the left pane on the ST editor.
    pub fn show_code(&mut self) {
        self.left = true;
        self.pane = Pane::Code;
    }

    /// Opens the left pane on the files.
    pub fn show_files(&mut self) {
        self.left = true;
        self.pane = Pane::Files;
    }

    /// Whether the Devices tab is on screen.
    #[must_use]
    pub fn devices_open(&self) -> bool {
        self.left && self.pane == Pane::Devices
    }

    /// Opens the left pane on the devices.
    pub fn show_devices(&mut self) {
        self.left = true;
        self.pane = Pane::Devices;
    }

    /// Alt Shift 4, View › Devices: the Devices tab, or the pane shut when that
    /// is what shows.
    pub fn toggle_devices(&mut self) {
        if self.devices_open() {
            self.left = false;
        } else {
            self.show_devices();
        }
    }

    /// Alt Shift 1, View › Structured Text: the Code tab, or the pane shut when
    /// that is what shows.
    pub fn toggle_code(&mut self) {
        if self.code_open() {
            self.left = false;
        } else {
            self.show_code();
        }
    }

    /// Alt Shift 0, View › Files: the Files tab, or the pane shut when that is
    /// what shows.
    pub fn toggle_files(&mut self) {
        if self.files_open() {
            self.left = false;
        } else {
            self.show_files();
        }
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

/// The View menu's part that is the Editor's frame: the left pane's two
/// tabs, the right panel and its tabs, the bottom panel.
pub fn view_menu(ui: &mut egui::Ui, chrome: &mut Chrome, tabs: &[Tab]) {
    use crate::hotkeys::{Action, label};
    let (code, files, right) = (chrome.code_open(), chrome.files_open(), chrome.right);
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
        "Side panel",
        right,
        &k(Action::Panel),
        "Shows or hides the panel on the right: I/O, Agent.",
    ) {
        chrome.right = !right;
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
    for &t in tabs {
        let on = chrome.right && chrome.tab == t;
        if menu_item(
            ui,
            chrome,
            &format!("view tab {}", t.key()),
            format!("    {}", t.label()),
            on,
            t.tip(),
        ) {
            chrome.tab = t;
            chrome.right = true;
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
    /// stale file with its Reload): a line under the header, only while
    /// there is one.
    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's Files tab: the projects, the open one's files and
    /// the program tree.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's Devices tab: the I/O map's devices and their
    /// points, live (`crate::devices`).
    fn devices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's Code tab: the ST editor (read-only on a box).
    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The Trend along the bottom: pins and the plot.
    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The Messages tab beside it: the view's log (`messages::Log`).
    fn messages(&mut self) -> &mut crate::messages::Log;
    /// The right panel's tabs this view has, in order.
    fn tabs(&self) -> &'static [Tab];
    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab, chrome: &mut Chrome);
    /// The diagram with its breadcrumbs; a block click calls
    /// [`Chrome::goto`].
    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
}

/// A collapsed sidebar's strip as one button: a chevron pointing `left`
/// or right, the strip's full height, tinted under the pointer.
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

/// One frame of the Editor.
pub fn show(ui: &mut egui::Ui, chrome: &mut Chrome, body: &mut dyn Body) {
    if body.has_notices() {
        egui::Panel::top("ed-notices").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| body.notices(ui, chrome));
        });
    }
    let mut hits = Vec::new();
    let mut left = chrome.left;
    let mut pane = chrome.pane;
    let (left0, pane0) = (left, pane);
    let mut hide = false;
    let flip = egui::Panel::show_switched(
        ui,
        &mut left,
        egui::Panel::left("ed-files-strip")
            .exact_size(STRIP)
            .resizable(false),
        egui::Panel::left("ed-files")
            .resizable(true)
            .default_size(LEFT_W)
            .min_size(120.0),
        |ui, open| {
            let toggle = if open {
                let mut t = None;
                // Tabs: Files | Devices | Code, the hide chevron at the right.
                // A double click on a tab hides the pane too (Nick, 2026-11-07).
                ui.horizontal(|ui| {
                    for p in Pane::ALL {
                        let r = crate::theme::tab(ui, pane == p, p.label(), Vec2::new(0.0, TAP * 0.75))
                            .tip(format!("{} Double-click hides the pane.", p.tip()));
                        hits.push((format!("pane {}", p.key()), r.rect));
                        if r.clicked() {
                            pane = p;
                        }
                        if r.double_clicked() {
                            hide = true;
                        }
                    }
                    // The chevron at the right edge, or right after the
                    // tabs when they fill the row: a right-to-left layout
                    // with no room left would draw it over the last tab.
                    // The gap leaves the item spacing before the chevron,
                    // or the row overflows and the panel grows every frame.
                    let gap = ui.available_width() - TAP * 0.75 - ui.spacing().item_spacing.x;
                    ui.add_space(gap.max(0.0));
                    t = Some(header_toggle(ui, true, "Hides the pane."));
                });
                ui.separator();
                match pane {
                    Pane::Files => {
                        egui::ScrollArea::vertical()
                            .id_salt("ed-files-scroll")
                            .show(ui, |ui| body.files(ui, chrome));
                    }
                    Pane::Devices => {
                        egui::ScrollArea::vertical()
                            .id_salt("ed-devices-scroll")
                            .show(ui, |ui| body.devices(ui, chrome));
                    }
                    Pane::Code => body.st(ui, chrome),
                }
                t
            } else {
                Some(strip_toggle(ui, false, "Shows the pane."))
            };
            if let Some(t) = toggle {
                hits.push(("left".to_string(), t.rect));
                t.clicked() || hide
            } else {
                false
            }
        },
    )
    .inner;
    if flip {
        left = !left;
    }
    // The body may have moved the pane itself (a double click on a file
    // opens the Code tab); the tabs' copy wins only when it did not.
    if chrome.left == left0 {
        chrome.left = left;
    }
    if chrome.pane == pane0 {
        chrome.pane = pane;
    }

    let tabs = body.tabs();
    if !tabs.contains(&chrome.tab) {
        chrome.tab = tabs.first().copied().unwrap_or_default();
    }
    let mut right = chrome.right;
    let mut tab = chrome.tab;
    let mut hide_right = false;
    let flip = egui::Panel::show_switched(
        ui,
        &mut right,
        egui::Panel::right("ed-side-strip")
            .exact_size(STRIP)
            .resizable(false),
        egui::Panel::right("ed-side")
            .resizable(true)
            .default_size(RIGHT_W),
        |ui, open| {
            let toggle = if open {
                let mut t = None;
                ui.horizontal(|ui| {
                    t = Some(header_toggle(ui, false, "Hides the panel (Alt+Shift+3)."));
                    for &k in tabs {
                        let r = crate::theme::tab(ui, tab == k, k.label(), Vec2::new(0.0, TAP * 0.75))
                            .tip(format!("{} Double-click hides the panel.", k.tip()));
                        hits.push((format!("tab {}", k.key()), r.rect));
                        if r.clicked() {
                            tab = k;
                        }
                        if r.double_clicked() {
                            hide_right = true;
                        }
                    }
                });
                ui.separator();
                body.tab(ui, tab, chrome);
                t
            } else {
                Some(strip_toggle(ui, true, "Shows the panel (Alt+Shift+3)."))
            };
            if let Some(t) = toggle {
                hits.push(("right".to_string(), t.rect));
                t.clicked() || hide_right
            } else {
                false
            }
        },
    )
    .inner;
    if flip {
        right = !right;
    }
    chrome.right = right;
    chrome.tab = tab;
    chrome.hits.extend(hits);

    let mut trend = chrome.trend;
    egui::Panel::bottom("ed-trend")
        .resizable(true)
        // Tabs, the signals' row and the slider take about 110 pt: at
        // 200 the plot under them had 50.
        .default_size(280.0)
        .show_collapsible(ui, &mut trend, |ui| {
            // Tabs: Trend | Messages (errors counted on the tab).
            let label = body.messages().label();
            ui.horizontal(|ui| {
                for (b, text, key, tip) in [
                    (
                        Bottom::Trend,
                        "Trend".to_string(),
                        "bottom trend",
                        "Pinned signals over time (Alt+Shift+2).",
                    ),
                    (
                        Bottom::Messages,
                        label,
                        "bottom messages",
                        "What happened: connections, deploys, faults and errors, newest last.",
                    ),
                ] {
                    let r =
                        crate::theme::tab(ui, chrome.bottom == b, &text, Vec2::new(64.0, TAP * 0.6)).tip(tip);
                    chrome.hit(key, r.rect);
                    if r.clicked() {
                        chrome.bottom = b;
                    }
                }
            });
            ui.separator();
            match chrome.bottom {
                Bottom::Trend => body.trend(ui, chrome),
                Bottom::Messages => body.messages().ui(ui, chrome),
            }
        });
    chrome.trend = trend;

    egui::CentralPanel::no_frame().show(ui, |ui| {
        chrome.diagram = ui.available_rect_before_wrap();
        body.diagram(ui, chrome);
    });
}

/// The ST, with line numbers, diagnostics marked (`(line, column)`), and
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
                // Slack for the editor's own margin and the scroll bar: wider
                // than the pane and a resizable pane grows to fit, every frame.
                let wrap = if wrap_on {
                    (ui.available_width() - gutter_w - ui.spacing().item_spacing.x - 32.0).max(60.0)
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
                let mut job = highlight(text.as_str(), diags, at, &added, &font, edit_wrap, &t);
                crate::find::mark(&mut job, &marks);
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                let mut gutter = LayoutJob::default();
                for (n, number) in numbering(galley.rows.iter().map(|r| r.ends_with_newline)) {
                    let bad = diags.iter().any(|(l, _)| *l as usize == n);
                    let here = at.is_some_and(|l| l as usize == n);
                    let color = if bad {
                        crate::theme::theme(ui).ink(RED)
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
                ui.label(gutter);
                // Wrap off: the editor is as wide as its longest line (plus
                // its margins) and the scroll area scrolls to it; the pane
                // keeps its width because the scroll area clips.
                let desired = if wrap_on { wrap } else { galley.size().x + 16.0 };
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                    let wrap = if wrap_on { wrap } else { f32::INFINITY };
                    ui.data_mut(|d| d.insert_temp(wrap_key, wrap));
                    let mut job = highlight(buf.as_str(), diags, at, &added, &font, wrap, &t);
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

/// The ST as a layout wrapped at `wrap` points: a line with a diagnostic
/// on a red ground, its column to the end of the word underlined; the
/// line `at` (a block click sent the editor there) on a grey ground; a
/// line the Agent's last change added (`added`) on a green ground.
#[must_use]
pub fn highlight(
    text: &str,
    diags: &[(u32, u32)],
    at: Option<u32>,
    added: &[u32],
    font: &FontId,
    wrap: f32,
    t: &Theme,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(font.clone(), t.code);
    let bad_ground = t.bad;
    for (k, line) in text.split_inclusive('\n').enumerate() {
        let n = u32::try_from(k + 1).unwrap_or(u32::MAX);
        let Some(&(_, col)) = diags.iter().find(|(l, _)| *l == n) else {
            let format = if at == Some(n) {
                TextFormat {
                    background: t.rule,
                    color: t.fg,
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
            job.append(line, 0.0, format);
            continue;
        };
        let ground = TextFormat {
            background: bad_ground,
            ..plain.clone()
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
        job.append(&line[..start], 0.0, ground.clone());
        job.append(
            &rest[..word],
            0.0,
            TextFormat {
                underline: Stroke::new(1.5, RED),
                color: t.fg,
                ..ground.clone()
            },
        );
        job.append(&rest[word..], 0.0, ground);
    }
    job.wrap.max_width = wrap;
    job
}
