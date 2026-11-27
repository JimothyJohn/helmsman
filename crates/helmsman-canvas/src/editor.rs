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
    Claude,
    /// The Help tab (Nick, 2026-11-26): a question about an error or
    /// about how to do something, kept apart from the Describe tab.
    Help,
}

impl Tab {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Io => "I/O",
            Self::Claude => "Describe",
            Self::Help => "Help",
        }
    }

    /// The tab button's tooltip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::Io => "I/O: the program's inputs and outputs with their live values.",
            Self::Claude => "Describe: say what the program should do, and the change is made and checked.",
            Self::Help => {
                "Help: ask what an error means or how to do something. It reads the Log and the devices and changes nothing."
            }
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
/// I/O | Agent on the right; a Palette of pieces to drag onto the
/// diagram from 2026-11-12 to 2026-11-14 (Nick: it goes against the
/// agent-first design; it may come back for fine tuning).
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
    /// Ask what an error means or how to do something (`crate::help`).
    Help,
}

impl Pane {
    /// Every tab, in the left pane's first order.
    pub const ALL: [Self; 6] = [
        Self::Claude,
        Self::Code,
        Self::Io,
        Self::Devices,
        Self::Files,
        Self::Help,
    ];

    /// The view's own tab this is, for the two a view draws through
    /// [`Body::tab`].
    #[must_use]
    pub const fn tab(self) -> Option<Tab> {
        match self {
            Self::Claude => Some(Tab::Claude),
            Self::Io => Some(Tab::Io),
            Self::Help => Some(Tab::Help),
            Self::Code | Self::Devices | Self::Files => None,
        }
    }

    /// The tab button's tooltip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::Files => "Alt+Shift+0  Files: the projects and their files.",
            Self::Devices => "Alt+Shift+4  Devices: the I/O map and its modules.",
            Self::Code => "Alt+Shift+1  Code: the open file's Structured Text.",
            Self::Claude | Self::Io | Self::Help => match self.tab() {
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
            Self::Claude => Icon::Agent,
            Self::Io => Icon::Io,
            Self::Help => Icon::Help,
        }
    }

    /// The tab's name, as the View menu has it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Devices => "Devices",
            Self::Code => "Structured Text",
            Self::Claude => Tab::Claude.label(),
            Self::Io => Tab::Io.label(),
            Self::Help => Tab::Help.label(),
        }
    }

    /// Its name in the saved layout.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Devices => "devices",
            Self::Code => "code",
            Self::Claude => "claude",
            Self::Io => "io",
            Self::Help => "help",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
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
/// program to 24 rows; at 320 it holds 27); 384 since 2026-11-26 (Nick:
/// "20% wider"); 307 the same day (Nick: "20% narrower", 384 x 0.8).
pub const LEFT_W: f32 = 307.0;
/// The right panel's width when first opened, points: 236 since
/// 2026-11-05 (the tab buttons and the hide chevron must fit the row, or
/// the panel grows every frame; the Requirements tab is archived since
/// 2026-11-08, the width kept).
pub const RIGHT_W: f32 = 236.0;
/// The bottom panel's height when first opened, points (Nick, 2026-11-23:
/// the trend is the whole bottom and taller): the signals' row and the
/// slider take about 80 pt, which leaves six lanes about 40 pt each, and
/// at 1280x800 the diagram keeps about 430 pt above it.
pub const BOTTOM_H: f32 = 288.0;
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
    /// The tabs in neither pane (a tab's own menu hides it; the View
    /// menu and anything that opens the tab bring it back): none by
    /// default.
    pub hidden: Vec<Pane>,
    /// The panes' sizes as last drawn open (`crate::pane_layout`).
    pub sizes: crate::pane_layout::Sizes,
    /// Sizes a layout asked for, given to the panes on the next frame.
    pub resize: Option<crate::pane_layout::Sizes>,
    /// Which of the left pane's tabs shows.
    pub pane: Pane,
    /// Which of the right pane's tabs shows.
    pub side: Pane,
    /// The Trend is the bottom strip from the left pane's edge, under the
    /// right pane (Nick, 2026-11-01: a long time axis beats a tall one;
    /// 2026-11-23: nothing else in it; 2026-11-24: the left pane keeps
    /// its whole height, so the strip is the whole bottom only while
    /// that pane is shut).
    pub trend: bool,
    /// The bottom panel shows its Messages tab, not the Trend (Nick,
    /// 2026-11-26). Remembered.
    pub msgs: bool,
    /// The Monitor's status tiles show the scan period and counts: an
    /// engineer's choice, and only shown while one is signed in.
    pub scan: bool,
    /// View › Light mode: the light page instead of the dark one.
    pub light: bool,
    /// The Describe tab's key was pressed: its instruction field takes the
    /// keyboard the next time it is drawn (Nick, 2026-11-26: A, then type,
    /// then Enter). Not remembered.
    pub focus_describe: bool,
    /// The Help tab's key was pressed: its question field takes the
    /// keyboard the next time it is drawn. Not remembered.
    pub focus_help: bool,
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
    /// The Code tab's versions list is open under its header (the clock
    /// beside the arrows; Nick, 2026-11-25).
    pub history: bool,
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
    /// The I/O tab shows its points as tiles, not one to a row
    /// ([`crate::io_cards`]). Remembered.
    pub io_grid: bool,
    /// The I/O tab's Inputs are folded away. Remembered.
    pub io_fold_in: bool,
    /// The I/O tab's Outputs are folded away. Remembered.
    pub io_fold_out: bool,
    /// The Files tab's My Files section is folded away. Remembered.
    pub files_fold_mine: bool,
    /// The Files tab's Examples section is folded away. Remembered.
    pub files_fold_examples: bool,
    /// Help › Show hotkeys: every control with a shortcut wears it as a
    /// superscript, to teach it. Remembered.
    pub hotkeys: bool,
    /// An error the person clicked, to put to the Help tab: the view
    /// takes it ([`Self::ask_help`]); never saved.
    pub help_ask: Option<String>,
    /// The view has a Help tab this frame, so an error is a link to it
    /// ([`crate::messages::error_link`]).
    pub help_on: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            left: true,
            right: true,
            lefts: Pane::ALL.to_vec(),
            rights: Vec::new(),
            hidden: Vec::new(),
            sizes: crate::pane_layout::Sizes::default(),
            resize: None,
            pane: Pane::Claude,
            side: Pane::Claude,
            trend: true,
            msgs: false,
            scan: false,
            light: false,
            focus_describe: false,
            focus_help: false,
            line: None,
            jump: None,
            hits: Vec::new(),
            diagram: Rect::NOTHING,
            find: None,
            st_marks: None,
            wrap: true,
            history: false,
            colors: Vec::new(),
            hints: crate::st_hints::Hints::default(),
            folds: crate::st_fold::Folds::default(),
            advanced: false,
            io_grid: false,
            io_fold_in: false,
            io_fold_out: false,
            files_fold_mine: false,
            files_fold_examples: false,
            hotkeys: false,
            help_ask: None,
            help_on: false,
        }
    }
}

impl Chrome {
    /// Puts `error`, as shown, to the Help tab: the view opens it and asks.
    pub fn ask_help(&mut self, error: &str) {
        self.help_ask = Some(error.to_string());
    }

    /// Whether the Files tab's section for `g` is folded away.
    #[must_use]
    pub const fn files_folded(&self, g: crate::projects::Group) -> bool {
        match g {
            crate::projects::Group::Mine => self.files_fold_mine,
            crate::projects::Group::FirstSteps => self.files_fold_examples,
        }
    }

    pub(crate) const fn set_files_folded(&mut self, g: crate::projects::Group, folded: bool) {
        match g {
            crate::projects::Group::Mine => self.files_fold_mine = folded,
            crate::projects::Group::FirstSteps => self.files_fold_examples = folded,
        }
    }

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
        let hidden = if self.hidden.is_empty() {
            String::new()
        } else {
            format!(" hidden={}", list(&self.hidden))
        };
        format!(
            "dock=2 left={} right={} lefts={} pane={}{rights}{hidden} scan={} trend={} msgs={} light={} wrap={} advanced={} iogrid={} iofold={}{} filefold={}{} hotkeys={}{colors}",
            u8::from(self.left),
            u8::from(self.right),
            list(&self.lefts),
            self.pane.key(),
            u8::from(self.scan),
            u8::from(self.trend),
            u8::from(self.msgs),
            u8::from(self.light),
            u8::from(self.wrap),
            u8::from(self.advanced),
            u8::from(self.io_grid),
            u8::from(self.io_fold_in),
            u8::from(self.io_fold_out),
            u8::from(self.files_fold_mine),
            u8::from(self.files_fold_examples),
            u8::from(self.hotkeys)
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
                Some(("hidden", v)) if docked => c.hidden = list(v),
                Some(("pane", v)) if docked => c.pane = Pane::from_key(v).unwrap_or_default(),
                Some(("side", v)) if docked => c.side = Pane::from_key(v).unwrap_or_default(),
                Some(("scan", v)) => c.scan = v == "1",
                Some(("trend", v)) => c.trend = v == "1",
                Some(("light", v)) => c.light = v == "1",
                Some(("wrap", v)) => c.wrap = v == "1",
                Some(("advanced", v)) => c.advanced = v == "1",
                Some(("hotkeys", v)) => c.hotkeys = v == "1",
                Some(("iogrid", v)) => c.io_grid = v == "1",
                Some(("iofold", v)) => {
                    c.io_fold_in = v.starts_with('1');
                    c.io_fold_out = v.ends_with('1') && v.len() == 2;
                }
                Some(("filefold", v)) => {
                    c.files_fold_mine = v.starts_with('1');
                    c.files_fold_examples = v.ends_with('1') && v.len() == 2;
                }
                Some(("colors", v)) => {
                    c.colors = v
                        .split(',')
                        .filter_map(|e| e.rsplit_once(':'))
                        .filter_map(|(n, i)| Some((n.to_string(), i.parse().ok()?)))
                        .filter(|(n, _)| !n.is_empty())
                        .take(MAX_COLORS)
                        .collect();
                }
                Some(("msgs", v)) => c.msgs = v == "1",
                _ => {}
            }
        }
        c.tidy();
        c
    }

    /// Every tab in exactly one place (a pane, or hidden), the shown ones
    /// among their pane's: a tab twice keeps its first place, a tab in
    /// none (one a later release added) goes to the end of the left pane.
    pub(crate) fn tidy(&mut self) {
        let mut seen = Vec::new();
        for list in [&mut self.lefts, &mut self.rights, &mut self.hidden] {
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

    /// Opens the bottom panel on its Messages tab.
    pub fn show_messages(&mut self) {
        self.msgs = true;
        self.trend = true;
    }

    /// The Messages' key and menu item: shuts the bottom panel when it
    /// shows them, else opens it on them.
    pub fn toggle_messages(&mut self) {
        if self.trend && self.msgs {
            self.trend = false;
        } else {
            self.show_messages();
        }
    }

    /// The Trend's key and menu item: shuts the bottom panel when it
    /// shows the Trend, else opens it on the Trend.
    pub fn toggle_trend(&mut self) {
        if self.trend && !self.msgs {
            self.trend = false;
        } else {
            self.trend = true;
            self.msgs = false;
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

    /// Opens tab `p` in the pane it is docked in; a hidden one comes
    /// back at the end of the left pane.
    pub fn show(&mut self, p: Pane) {
        if let Some(at) = self.hidden.iter().position(|&q| q == p) {
            self.hidden.remove(at);
            self.lefts.push(p);
        }
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

    /// Takes tab `p` out of its pane. When it was the tab the pane
    /// showed, the pane shows the one that was under it, or the one
    /// before when it was the last.
    pub fn hide(&mut self, p: Pane) {
        for (list, shown) in [
            (&mut self.lefts, &mut self.pane),
            (&mut self.rights, &mut self.side),
        ] {
            if let Some(at) = list.iter().position(|&q| q == p) {
                list.remove(at);
                if *shown == p {
                    *shown = list.get(at).or_else(|| list.last()).copied().unwrap_or_default();
                }
            }
        }
        if !self.hidden.contains(&p) {
            self.hidden.push(p);
        }
    }

    /// Docks tab `p` in the right pane (`right`) or the left one, at the
    /// end, showing it there. When it was the tab its old pane showed,
    /// that pane shows the one that was under it, or the one before when
    /// it was the last.
    pub fn dock(&mut self, p: Pane, right: bool) {
        self.place(p, right, None);
    }

    /// Puts tab `p` in the right pane (`right`) or the left one, before
    /// tab `before` there, or at the end. Within its own pane that is a
    /// reorder and the shown tab stays; from the other pane it is
    /// [`Self::dock`] at that place. A hidden tab stays hidden.
    pub fn place(&mut self, p: Pane, right: bool, before: Option<Pane>) {
        if before == Some(p) {
            return;
        }
        let slot = |list: &[Pane]| {
            before
                .and_then(|b| list.iter().position(|&q| q == b))
                .unwrap_or(list.len())
        };
        let own = if right { &mut self.rights } else { &mut self.lefts };
        if let Some(at) = own.iter().position(|&q| q == p) {
            own.remove(at);
            let to = slot(own);
            own.insert(to, p);
            return;
        }
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
        let k = slot(to);
        to.insert(k, p);
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
        "Shows or hides the right pane, once a tab is docked there.",
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
        "Docks the left pane's tab on the right.\nDrag it back to return it.",
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
    /// stale file with its Reload): the first rows of the Messages,
    /// only while there is one (Nick, 2026-11-11: not a line under the
    /// header).
    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// Whether one of them is an error that still stands (a fault, a
    /// refused save or deploy): the Messages header stays red while it does.
    fn alarm(&self) -> bool;
    /// The left pane's Files tab: the projects, the open one's files under
    /// it.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's Devices tab: the I/O map's devices and their
    /// points, live (`crate::devices`).
    fn devices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The left pane's Code tab: the ST editor (read-only on a box).
    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The Trend along the bottom: pins and the plot.
    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The Messages, a tab of the bottom panel: the view's log (`messages::Log`).
    fn messages(&mut self) -> &mut crate::messages::Log;
    /// The right panel's tabs this view has, in order.
    fn tabs(&self) -> &'static [Tab];
    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab, chrome: &mut Chrome);
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
    let mid = ui.max_rect().center().y;
    let mut picked = None;
    let side = ui.available_width().clamp(STRIP - 16.0, TAB);
    for (k, t) in tabs.iter().enumerate() {
        let r =
            crate::glyphs::tab(ui, t.icon, side, t.on, None).tip(format!("{}\nOpens the pane on it.", t.tip));
        hits.push((t.key.clone(), r.rect));
        if r.clicked() {
            picked = Some(k);
        }
    }
    (picked, strip_toggle(ui, left, hover, mid))
}

/// The rest of a collapsed sidebar's strip as one button: the fold glyph
/// for showing the pane, tinted under the pointer. `left`: the pane is the
/// right one, so showing it moves it left (the strip is on the right).
/// The glyph sits at height `mid`, the pane's middle, where the open
/// pane's ribbon button is, unless the tabs reach past it.
fn strip_toggle(ui: &mut egui::Ui, left: bool, hover: &str, mid: f32) -> egui::Response {
    let size = Vec2::new(
        ui.available_width().max(STRIP - 8.0),
        ui.available_height().max(TAP),
    );
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let r = r.tip(hover);
    if r.hovered() {
        let tint = ui.style().interact(&r).bg_fill;
        ui.painter().rect_filled(rect, CornerRadius::same(4), tint);
    }
    let y = mid.clamp(
        rect.top() + TAP * 0.5,
        (rect.bottom() - TAP * 0.5).max(rect.top() + TAP * 0.5),
    );
    fold_glyph(ui, Pos2::new(rect.center().x, y), !left, false, r.hovered(), 1.0);
    r
}

/// Width of an open sidebar's ribbon, points.
const RIBBON: f32 = 20.0;

/// An open sidebar's hide button (Nick, 2026-11-26): a thin ribbon along
/// the pane's inner edge, `full` the pane's rect, the button at its
/// vertical middle, the very spot the collapsed strip's button takes.
/// `left`: the pane is the left one (its ribbon is on its right).
fn ribbon(ui: &mut egui::Ui, full: Rect, left: bool, hover: &str) -> egui::Response {
    let rib_x = if left {
        full.right() - RIBBON / 2.0
    } else {
        full.left() + RIBBON / 2.0
    };
    let btn = Rect::from_center_size(Pos2::new(rib_x, full.center().y), Vec2::new(RIBBON, TAP * 0.9));
    let r = ui
        .interact(btn, ui.scope_id().with(("ribbon", left)), Sense::click())
        .tip(hover);
    // No ground of its own: a full-height grey strip beside the pane read as
    // a code gutter (Nick, 2026-11-27). The button alone, on the pane.
    let tint = crate::theme::eased_fill(ui, &r, Color32::TRANSPARENT);
    ui.painter().rect_filled(btn, CornerRadius::same(4), tint);
    crate::theme::focus_ring(ui, &r, btn, 4);
    fold_glyph(ui, btn.center(), left, true, r.hovered(), 0.8);
    r
}

/// The fold glyph (Nick, 2026-11-23: the chevron was not intuitive): a
/// window outline with the pane's side filled, and an arrow in the rest
/// of it pointing the way the pane will move, into its edge to hide it,
/// out of it to show it. `pane_left`: the pane is on the left.
fn fold_glyph(ui: &egui::Ui, c: Pos2, pane_left: bool, hiding: bool, hot: bool, scale: f32) {
    let t = theme(ui);
    let ink = if hot { t.accent } else { t.muted };
    let (w, h, slab) = (22.0 * scale, 18.0 * scale, 7.0 * scale);
    let frame = Rect::from_center_size(c, Vec2::new(w, h));
    let p = ui.painter();
    p.rect_stroke(
        frame,
        CornerRadius::same(2),
        Stroke::new(2.0, ink),
        egui::StrokeKind::Inside,
    );
    let side = if pane_left { -1.0 } else { 1.0 };
    let edge = if pane_left { frame.left() } else { frame.right() };
    let slab_rect = Rect::from_x_y_ranges(
        if pane_left {
            edge..=edge + slab
        } else {
            edge - slab..=edge
        },
        frame.top()..=frame.bottom(),
    );
    p.rect_filled(slab_rect, CornerRadius::same(1), ink);
    // The arrow's head points toward the pane's edge to hide, away to show.
    let dir = if hiding { side } else { -side };
    let mid = Pos2::new(edge - side * (slab + (w - slab) / 2.0), c.y);
    let reach = 3.0 * scale;
    let tip = Pos2::new(mid.x + dir * reach, c.y);
    let back = mid.x - dir * reach;
    let stroke = Stroke::new(2.0, ink);
    p.line_segment([Pos2::new(back, c.y - 4.0 * scale), tip], stroke);
    p.line_segment([Pos2::new(back, c.y + 4.0 * scale), tip], stroke);
}

/// How far from the window's right edge a tab dragged out of the left
/// pane docks on the right, points.
pub const DOCK_ZONE: f32 = 96.0;

/// A tab's content in whichever pane it is docked.
fn pane_body(ui: &mut egui::Ui, p: Pane, body: &mut dyn Body, chrome: &mut Chrome) {
    let scroll = |salt: &str| egui::ScrollArea::vertical().id_salt(salt);
    match p {
        Pane::Files => {
            scroll("ed-files-scroll").show(ui, |ui| body.files(ui, chrome));
        }
        Pane::Devices => {
            scroll("ed-devices-scroll").show(ui, |ui| body.devices(ui, chrome));
        }
        Pane::Code => body.st(ui, chrome),
        Pane::Claude => body.tab(ui, Tab::Claude, chrome),
        Pane::Io => body.tab(ui, Tab::Io, chrome),
        Pane::Help => body.tab(ui, Tab::Help, chrome),
    }
}

/// What the Messages' header shows of the log.
struct Cue {
    /// How red the header is, 0 to 1 (`messages::Log::alarm`).
    alarm: Option<f32>,
    /// Errors in the log.
    errors: usize,
}

/// The notices that stand (a fault, a deploy under way or refused, a
/// browser that cannot keep the work), above either tab of the bottom
/// panel: they are what is true now, so the Trend does not hide them.
fn notices_row(ui: &mut egui::Ui, body: &mut dyn Body, chrome: &mut Chrome) {
    if body.has_notices() {
        // They scroll in at most 40% of the panel, and the log keeps the
        // rest.
        egui::ScrollArea::vertical()
            .id_salt("ed-msgs-notices")
            .max_height(ui.available_height() * 0.4)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| body.notices(ui, chrome));
            });
        ui.separator();
    }
}

/// A side pane as drawn: where, and its tabs' buttons along the top
/// while it is open (the places a dragged tab can be dropped between).
struct Side {
    rect: Rect,
    tabs: Vec<(Pane, Rect)>,
}

/// One side pane, `right` or left: its tabs along the top, each one
/// draggable along the row or to the other side and hidden from its own
/// menu, the hide chevron, and the shown tab; shut, its strip.
fn side_pane(
    ui: &mut egui::Ui,
    right: bool,
    tabs: &[Pane],
    body: &mut dyn Body,
    chrome: &mut Chrome,
    hits: &mut Vec<(String, Rect)>,
) -> Side {
    let mut row_tabs = Vec::new();
    let mut hidden = None;
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
            egui::Panel::right(RIGHT_ID)
                .frame(crate::theme::float_frame(ui))
                .resizable(true)
                .default_size(RIGHT_W),
        )
    } else {
        (
            egui::Panel::left("ed-files-strip")
                .exact_size(STRIP)
                .resizable(false),
            egui::Panel::left(LEFT_ID)
                .frame(crate::theme::float_frame(ui))
                .resizable(true)
                .default_size(LEFT_W)
                // Never past the page less 40 pt, every frame: opened at
                // 384 on a wide page, it overfilled one narrowed to a
                // phone's 360 (what 320 happened to leave, it leaves).
                .max_size((ui.ctx().content_rect().width() - 40.0).max(120.0))
                .min_size(120.0),
        )
    };
    let (other, chevron_tip) = if right {
        (
            "the left pane",
            format!(
                "{}  Hide the pane.",
                crate::hotkeys::label(ui.ctx(), crate::hotkeys::Action::Panel)
            ),
        )
    } else {
        ("the window's right edge", "Hide the pane.".to_string())
    };
    let shown = egui::Panel::show_switched(ui, &mut open, strip_panel, panel, |ui, is_open| {
        let toggle = if is_open {
            let full = ui.max_rect();
            let content = if right {
                Rect::from_min_max(Pos2::new(full.left() + RIBBON, full.top()), full.max)
            } else {
                Rect::from_min_max(full.min, Pos2::new(full.right() - RIBBON, full.bottom()))
            };
            let t = ribbon(ui, full, !right, &chevron_tip);
            ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                // The tabs shrink to share one row (seven at full size are
                // wider than the pane's 320 pt), down to 24 pt,
                // and wrap below that: a row that overflows widens the pane
                // every frame. The row's width is read here, before it starts:
                // inside a wrapped row `available_width` is not what is left.
                let row = ui.available_width();
                let gap_x = ui.spacing().item_spacing.x;
                #[allow(clippy::cast_precision_loss)]
                let n = tabs.len() as f32;
                let size = ((row - gap_x * (n + 1.0)) / n.max(1.0)).clamp(24.0, TAB);
                ui.horizontal_wrapped(|ui| {
                    for &p in tabs {
                        let r = crate::glyphs::tab_sensed(
                            ui,
                            p.icon(),
                            size,
                            on == p,
                            None,
                            Sense::click_and_drag(),
                        )
                        .tip(format!(
                            "{}\nDrag to reorder or to {other}; right-click hides the tab.",
                            p.tip()
                        ));
                        r.dnd_set_drag_payload(p);
                        row_tabs.push((p, r.rect));
                        r.context_menu(|ui| {
                            let b = ui
                                .add(egui::Button::new("Hide tab").min_size(Vec2::new(74.0, TAP * 0.7)))
                                .tip(format!(
                                    "Takes {} out of the pane. View › {} brings it back.",
                                    p.label(),
                                    p.label()
                                ));
                            hits.push((format!("hide {}", p.key()), b.rect));
                            if b.clicked() {
                                hidden = Some(p);
                                ui.close();
                            }
                        });
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
                });
                ui.separator();
                pane_body(ui, on, body, chrome);
            });
            ui.expand_to_include_rect(full);
            Some(t)
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
                format!(
                    "{}  Show the pane.",
                    crate::hotkeys::label(ui.ctx(), crate::hotkeys::Action::Panel)
                )
            } else {
                "Show the pane.".to_string()
            };
            let (picked, t) = strip(ui, &strip_tabs, right, &hover, hits);
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
    if let Some(p) = hidden {
        chrome.hide(p);
    }
    // The size a saved layout keeps: the open pane's, not its strip's.
    if open0
        && open
        && let Some(state) = egui::containers::panel::PanelState::load(ui.ctx(), pane_id(ui, right))
    {
        let w = Some(state.size().x.round());
        if right {
            chrome.sizes.right = w;
        } else {
            chrome.sizes.left = w;
        }
    }
    Side {
        rect: shown.response.rect,
        tabs: row_tabs,
    }
}

const LEFT_ID: &str = "ed-files";
const RIGHT_ID: &str = "ed-side";
const BOTTOM_ID: &str = "ed-trend";

/// The id egui keeps a side pane's size under.
fn pane_id(ui: &egui::Ui, right: bool) -> egui::Id {
    ui.scope_id().with(if right { RIGHT_ID } else { LEFT_ID })
}

/// Gives the panes the sizes a layout asked for, before they are drawn:
/// egui sizes a panel from the rectangle it kept of the last frame.
fn resize(ui: &egui::Ui, sizes: crate::pane_layout::Sizes) {
    use egui::containers::panel::PanelState;
    let put = |id: egui::Id, size: Option<f32>, across: bool| {
        let Some(size) = size else { return };
        let was = PanelState::load(ui.ctx(), id)
            .map_or(Rect::from_min_size(Pos2::ZERO, Vec2::ZERO), |s| s.outer_rect);
        let size = if across {
            Vec2::new(size, was.height())
        } else {
            Vec2::new(was.width(), size)
        };
        let outer_rect = Rect::from_min_size(was.min, size);
        ui.ctx()
            .data_mut(|d| d.insert_persisted(id, PanelState { outer_rect }));
    };
    put(pane_id(ui, false), sizes.left, true);
    put(pane_id(ui, true), sizes.right, true);
    put(ui.scope_id().with(BOTTOM_ID), sizes.bottom, false);
}

/// The place in a row of tab buttons the pointer `at` is over: the tab
/// a dropped one goes before (`None`: the end), and the edge to mark.
fn slot(tabs: &[(Pane, Rect)], at: Pos2) -> Option<(Option<Pane>, f32, egui::Rangef)> {
    let band = tabs.iter().map(|(_, r)| *r).reduce(Rect::union)?;
    if !band.expand(6.0).contains(at) {
        return None;
    }
    let (k, (_, r)) = tabs
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.1.distance_to_pos(at).total_cmp(&b.1.1.distance_to_pos(at)))?;
    if at.x > r.center().x {
        Some((tabs.get(k + 1).map(|t| t.0), r.right() + 2.0, r.y_range()))
    } else {
        Some((Some(tabs[k].0), r.left() - 2.0, r.y_range()))
    }
}

/// A tab being dragged: over a pane's row of tabs, the place it would
/// take marked, and on release the tab put there (a reorder in its own
/// pane, a dock at that place in the other); elsewhere, where it would
/// dock lit, and on release over that place, the tab docked there. The
/// tab is drawn under the pointer all the while.
fn dock_drop(ui: &egui::Ui, chrome: &mut Chrome, left: Option<&Side>, right: Option<&Side>) {
    let ctx = ui.ctx();
    for (key, side) in [("tabs left", left), ("tabs right", right)] {
        if let Some(row) = side.and_then(|s| s.tabs.iter().map(|(_, r)| *r).reduce(Rect::union)) {
            chrome.hit(key, row);
        }
    }
    let Some(p) = egui::DragAndDrop::payload::<Pane>(ctx).map(|p| *p) else {
        return;
    };
    let at = ctx.pointer_interact_pos();
    let t = theme(ui);
    let in_row = [(false, left), (true, right)]
        .into_iter()
        .find_map(|(to_right, side)| {
            let (before, x, ys) = slot(&side?.tabs, at?)?;
            Some((to_right, before, x, ys))
        });
    if let Some((to_right, before, x, ys)) = in_row {
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("dock-slot"));
        ctx.layer_painter(layer).vline(x, ys, Stroke::new(3.0, t.accent));
        drag_ghost(ctx, at, p, &t);
        if ctx.input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(ctx);
            chrome.place(p, to_right, before);
        }
        return;
    }
    let (left, right) = (left.map(|s| s.rect), right.map(|s| s.rect));
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
    let over = at.is_some_and(|a| zone.contains(a));
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
    drag_ghost(ctx, at, p, &t);
    if over && ctx.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ctx);
        chrome.dock(p, to_right);
    }
}

/// The dragged tab `p` drawn under the pointer.
fn drag_ghost(ctx: &egui::Context, at: Option<Pos2>, p: Pane, t: &Theme) {
    let Some(a) = at else { return };
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("dock-drag"));
    let painter = ctx.layer_painter(layer);
    let r = Rect::from_center_size(a, Vec2::splat(TAB));
    painter.rect_filled(r, CornerRadius::same(5), t.shade.gamma_multiply(0.9));
    crate::glyphs::paint(&painter, r.shrink(TAB * 0.24), p.icon(), t.fg);
}

/// The bottom panel across whatever width is left when it is drawn: the
/// Trend and the Messages, a tab each (Nick, 2026-11-26: the Messages are
/// historic and run long, so they sit with the Trend, not under the left
/// pane). Its height is kept. The Messages tab is red while an error
/// stands or came in unseen, with the count of errors beside it. The hits
/// are `bottom trend`, `bottom messages` and `messages error count`.
fn trend_panel(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    body: &mut dyn Body,
    cue: &Cue,
    hits: &mut Vec<(String, Rect)>,
) {
    let mut open = chrome.trend;
    egui::Panel::bottom(BOTTOM_ID)
        .frame(crate::theme::float_frame(ui))
        .resizable(true)
        .default_size(BOTTOM_H)
        .show_collapsible(ui, &mut open, |ui| {
            // The two tabs stacked in a strip at the left: a row of them
            // over the plot took the height the Trend's signal rows need
            // (a long time axis beats a tall one, Nick 2026-11-01).
            egui::Panel::left("ed-bottom-tabs")
                .exact_size(TAB + 8.0)
                .resizable(false)
                .show(ui, |ui| {
                    let r = crate::glyphs::tab(ui, Icon::Trend, TAB, !chrome.msgs, None)
                        .tip("Trend: the signals you pin, over time.");
                    hits.push(("bottom trend".to_string(), r.rect));
                    if r.clicked() {
                        chrome.msgs = false;
                    }
                    let r = crate::glyphs::tab(ui, Icon::Messages, TAB, chrome.msgs, cue.alarm)
                        .tip("Log: what happened, newest last.");
                    hits.push(("bottom messages".to_string(), r.rect));
                    if r.clicked() {
                        chrome.msgs = true;
                    }
                    if cue.errors > 0 {
                        let red = theme(ui).ink(crate::st_view::brand::RED);
                        let r = ui
                            .label(RichText::new(cue.errors.to_string()).strong().color(red))
                            .tip(if cue.errors == 1 {
                                "One error in the Log.".to_string()
                            } else {
                                format!("{} errors in the Log.", cue.errors)
                            });
                        hits.push(("messages error count".to_string(), r.rect));
                    }
                });
            notices_row(ui, body, chrome);
            if chrome.msgs {
                body.messages().ui(ui, chrome);
            } else {
                body.trend(ui, chrome);
            }
        });
    if open
        && chrome.trend
        && let Some(state) =
            egui::containers::panel::PanelState::load(ui.ctx(), ui.scope_id().with(BOTTOM_ID))
    {
        chrome.sizes.bottom = Some(state.size().y.round());
    }
    chrome.trend = open;
}

/// The bottom panel on the Monitor: the Editor's own (`trend_panel`, one
/// `chrome.trend` and `chrome.msgs` between them), drawn before the
/// Monitor's central panel.
pub fn bottom(ui: &mut egui::Ui, chrome: &mut Chrome, body: &mut dyn Body) {
    let now = ui.input(|i| i.time);
    let standing = body.alarm();
    let alarm = body.messages().alarm(now, standing);
    if body.messages().take_fresh() && !chrome.trend {
        chrome.show_messages();
    }
    if body.messages().flashing(now) {
        ui.ctx().request_repaint();
    }
    let cue = Cue {
        alarm,
        errors: body.messages().errors(),
    };
    let mut hits = Vec::new();
    trend_panel(ui, chrome, body, &cue, &mut hits);
    chrome.hits.extend(hits);
}

/// One frame of the Editor.
pub fn show(ui: &mut egui::Ui, chrome: &mut Chrome, body: &mut dyn Body) {
    let mut hits = Vec::new();
    let have = body.tabs();
    chrome.help_on = have.contains(&Tab::Help);
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
    if let Some(sizes) = chrome.resize.take() {
        resize(ui, sizes);
    }
    // The Messages are where every notice and error is (Nick,
    // 2026-11-11): red while an error stands or has come in unread, and
    // pulsing for a few seconds after a new one. They are a tab of the
    // bottom panel (Nick, 2026-11-26). A new error opens the panel on
    // them when it is shut; an open panel keeps its tab (the Messages
    // tab goes red).
    let now = ui.input(|i| i.time);
    let standing = body.alarm();
    let alarm = body.messages().alarm(now, standing);
    if body.messages().take_fresh() && !chrome.trend {
        chrome.show_messages();
    }
    if body.messages().flashing(now) {
        ui.ctx().request_repaint();
    }
    let cue = Cue {
        alarm,
        errors: body.messages().errors(),
    };

    // The left pane has the page's whole height, for its tab and the
    // Messages under it, and the Trend starts at its edge (the whole
    // width while that pane is shut) and runs under the right pane (Nick,
    // 2026-11-24). On a phone the pane leaves no width beside it, so
    // there the Trend keeps the whole bottom, under both panes.
    let phone = ui.ctx().content_rect().width() < crate::monitor::NARROW;
    if phone {
        trend_panel(ui, chrome, body, &cue, &mut hits);
    }
    let left = (!lefts.is_empty()).then(|| side_pane(ui, false, &lefts, body, chrome, &mut hits));
    if !phone {
        trend_panel(ui, chrome, body, &cue, &mut hits);
    }
    let right = (!rights.is_empty()).then(|| side_pane(ui, true, &rights, body, chrome, &mut hits));
    chrome.hits.extend(hits);
    dock_drop(ui, chrome, left.as_ref(), right.as_ref());

    // The open file's style hints, once the typing has settled.
    if let Some(line) = chrome.hints.settled(now) {
        body.messages().info(now, line);
    }

    egui::CentralPanel::no_frame().show(ui, |ui| {
        chrome.diagram = ui.available_rect_before_wrap();
        body.diagram(ui, chrome);
    });
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
    let found = crate::find::row(ui, &mut chrome.find, id, text);
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
                let galley = hang(ui, job, digit);
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
                    hang(ui, job, digit)
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

/// `job` laid out so that a wrapped line continues under its own
/// indentation plus one step (two columns), not at column 0 (Nick,
/// 2026-11-25: `MOD NOTE_COUNT;` flush left read as a new statement).
/// egui wraps every row of a job at one width and has no hanging indent,
/// so each source line is laid out as a galley of its own, the way egui
/// splits paragraphs itself, and the galleys are joined with
/// `Galley::concat` over the whole job: the text edit still gets one
/// galley of the whole text. In a line's job the first section starts
/// `indent` to the left (the row gets that much more room) and the wrap
/// width is `indent` less; every row of the line is then moved right by
/// `indent`, which puts the first row back at column 0 and the rest
/// `indent` in. `digit` is a glyph's width. With wrapping off the job is
/// laid out as it is.
pub(crate) fn hang(ui: &egui::Ui, job: LayoutJob, digit: f32) -> std::sync::Arc<egui::Galley> {
    use egui::text::{ByteIndex, LayoutSection, TextWrapping};

    let wrap = job.wrap.max_width;
    if !wrap.is_finite() || job.text.is_empty() {
        return ui.fonts_mut(|f| f.layout_job(job));
    }
    let job = std::sync::Arc::new(job);
    let len = job.text.len();
    let mut galleys = Vec::new();
    let (mut start, mut first) = (0, 0);
    while start < len {
        let mut end = job.text[start..].find('\n').map_or(len, |i| start + i);
        if end + 1 == len && job.text.ends_with('\n') {
            end += 1;
        }
        let line = &job.text[start..end];
        #[allow(clippy::cast_precision_loss)]
        let columns = line
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .map(|c| if c == '\t' { 2 } else { 1 })
            .sum::<usize>() as f32;
        let indent = if line.trim().is_empty() {
            0.0
        } else {
            ((columns + 2.0) * digit).min(wrap / 2.0).round()
        };
        let mut sections = Vec::new();
        for (k, section) in job.sections.iter().enumerate().skip(first) {
            let r = &section.byte_range;
            if r.end <= ByteIndex(start) {
                first = k + 1;
            } else if ByteIndex(end) < r.start {
                break;
            } else {
                sections.push(LayoutSection {
                    leading_space: if ByteIndex(start) <= r.start {
                        section.leading_space
                    } else {
                        0.0
                    },
                    byte_range: r.start.saturating_sub(start)
                        ..r.end.min(ByteIndex(end)).saturating_sub(start),
                    format: section.format.clone(),
                });
            }
        }
        if indent > 0.0
            && let Some(s) = sections.first_mut()
        {
            s.leading_space = -indent;
        }
        let paragraph = LayoutJob {
            text: line.to_owned(),
            wrap: TextWrapping {
                max_width: wrap - indent,
                ..job.wrap.clone()
            },
            sections,
            break_on_newline: job.break_on_newline,
            halign: job.halign,
            justify: job.justify,
            first_row_min_height: if start == 0 { job.first_row_min_height } else { 0.0 },
            round_output_to_gui: job.round_output_to_gui,
            keep_trailing_whitespace: job.keep_trailing_whitespace,
        };
        let mut galley = std::sync::Arc::unwrap_or_clone(ui.fonts_mut(|f| f.layout_job(paragraph)));
        if indent > 0.0 {
            for row in &mut galley.rows {
                row.pos.x += indent;
            }
        }
        galleys.push(std::sync::Arc::new(galley));
        start = end + 1;
    }
    let mut galley = egui::Galley::concat(job, &galleys, ui.pixels_per_point());
    galley.rect.min.x = 0.0;
    std::sync::Arc::new(galley)
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

/// The Code header's magnifier, drawn as "code find" for the tests: it
/// opens the find row over the code (Find, Replace, Replace all), and
/// shuts it again.
pub fn find_toggle(ui: &mut egui::Ui, chrome: &mut Chrome) {
    let open = chrome.find.is_some();
    let r = crate::glyphs::button_sized(
        ui,
        crate::glyphs::Icon::Find,
        crate::glyphs::ROW,
        true,
        open,
        "Ctrl F: find in this file, and replace.\nCtrl G next, Ctrl Shift G previous, Esc closes.",
    );
    chrome.hit("code find", r.rect);
    if r.clicked() {
        if open {
            chrome.find = None;
        } else {
            crate::find::Find::open(&mut chrome.find);
        }
    }
}

/// What the Code header's right end shows: Deploy, and before it the
/// errors when the code does not compile (nothing while it does: Nick,
/// 2026-11-23).
pub struct Status<'a> {
    /// How many errors, and the first one's words for the popup.
    pub errors: usize,
    pub first: Option<&'a str>,
    /// Whether Deploy may be pressed, or why not.
    pub deploy: &'a Result<(), String>,
    pub deploy_tip: &'a str,
    /// A deploy under way: what it is doing, beside a spinner.
    pub busy: Option<&'a str>,
    /// Back and Forward through the project's revisions (Nick,
    /// 2026-11-25: History in the Code tab), when there are any.
    pub steps: Option<&'a crate::instruct::Steps>,
    /// There are versions to list: the clock is drawn before the arrows.
    pub history: bool,
}

/// A spinner and what a deploy is doing, beside Deploy (in a right-to-left
/// row, so the words come before the spinner on screen).
pub fn busy(ui: &mut egui::Ui, chrome: &mut Chrome, stage: &str, hit: &str) {
    let t = crate::theme::theme(ui);
    let r = ui
        .add(egui::Label::new(RichText::new(stage).color(t.muted)).truncate())
        .tip("Deploy is under way. The Log says how it ends.");
    let s = ui.add(egui::Spinner::new().size(crate::glyphs::ROW * 0.8));
    chrome.hit(hit, r.rect.union(s.rect));
}

/// The Code header's Back, (i) and Forward, points a side: a little under
/// the row's glyphs, so the row still fits the 307 pt pane.
const STEP: f32 = crate::glyphs::ROW * 0.8;

/// What was pressed at the header's right end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pressed {
    pub deploy: bool,
    /// The red ✖: go to the first error.
    pub errors: bool,
    /// Back or Forward: the revision to show.
    pub revision: Option<u32>,
    /// The info button between them: the revision to open in a window.
    pub info: Option<u32>,
}

/// The header's right end, right to left: Deploy as a filled yellow
/// button with a black arrow, the row's height ("code deploy"), then a red ✖ with the count
/// when the code has errors ("code errors"), its popup the first error.
pub fn header_right(ui: &mut egui::Ui, chrome: &mut Chrome, s: &Status<'_>) -> Pressed {
    let mut out = Pressed::default();
    // What is left of the row, up to the pane's right edge.
    chrome.hit("code header rest", ui.available_rect_before_wrap());
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // Close together, as the Agent tab's row: with Back, (i) and
        // Forward the default spacing ran the clock over the wrap glyph
        // in the 307 pt pane (2026-11-27).
        ui.spacing_mut().item_spacing.x = 2.0;
        let tip = s.deploy.as_ref().err().map_or(s.deploy_tip, String::as_str);
        let r = crate::glyphs::deploy(ui, s.deploy.is_ok()).tip(tip);
        chrome.hit("code deploy", r.rect);
        out.deploy = r.clicked();
        if let Some(stage) = s.busy {
            busy(ui, chrome, stage, "code deploying");
        }
        if let Some(steps) = s.steps {
            // Right to left: Forward, info, then Back, so they read
            // Back (i) Forward as in the Agent tab.
            for (k, (key, icon, to, end)) in [
                (
                    "code forward",
                    Icon::Forward,
                    &steps.forward,
                    "Forward: this is the newest revision.",
                ),
                ("code info", Icon::Info, &None, ""),
                (
                    "code back",
                    Icon::Back,
                    &steps.back,
                    "Back: this is the first revision.",
                ),
            ]
            .into_iter()
            .enumerate()
            {
                if k == 1 {
                    let tip = if steps.shown.is_some() {
                        "Details: this version's instruction, what changed and the code, in a window, to look at or restore."
                    } else {
                        "Details: step Back to an older version first, then see its details here."
                    };
                    let r = crate::glyphs::button_sized(ui, icon, STEP, steps.shown.is_some(), false, tip);
                    chrome.hit(key, r.rect);
                    if r.clicked() {
                        out.info = steps.shown;
                    }
                    continue;
                }
                let tip = to.as_ref().map_or(end, |(_, t)| t.as_str());
                let r = crate::glyphs::button_sized(ui, icon, STEP, to.is_some(), false, tip);
                chrome.hit(key, r.rect);
                if r.clicked() {
                    out.revision = to.as_ref().map(|(n, _)| *n);
                }
            }
        }
        if s.history {
            let on = chrome.history;
            let r = crate::glyphs::button_sized(
                ui,
                Icon::History,
                crate::glyphs::ROW,
                true,
                on,
                if on {
                    "Versions: shut the list of this project's versions."
                } else {
                    "Versions: every saved version of this project, to search and step to."
                },
            );
            chrome.hit("code versions", r.rect);
            if r.clicked() {
                chrome.history = !on;
            }
        }
        if s.errors > 0 {
            let red = crate::theme::theme(ui).ink(crate::st_view::brand::RED);
            let r = ui
                .add(
                    egui::Label::new(RichText::new(format!("✖ {}", s.errors)).strong().color(red))
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .tip(format!(
                    "{}\nClick to go to its line; the list under the code has each one.",
                    s.first.unwrap_or("The code does not compile.")
                ));
            chrome.hit("code errors", r.rect);
            out.errors = r.clicked();
        }
    });
    out
}

/// The Code tab's list of versions, under its header while the clock is
/// on: the Agent tab's own list ([`crate::instruct::versions_ui`], search
/// box and all), in a frame that scrolls. A click is the revision to show.
pub fn versions_panel(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    s: &mut crate::instruct::Instruct,
    versions: &crate::instruct::Versions,
) -> Option<crate::instruct::Act> {
    let t = theme(ui);
    let max = (ui.available_height() * 0.45).clamp(120.0, 260.0);
    let mut act = None;
    let r = egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(Stroke::new(1.0, t.muted.gamma_multiply(0.4)))
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            act = egui::ScrollArea::vertical()
                .id_salt("code-versions-scroll")
                .max_height(max)
                .show(ui, |ui| {
                    // Room for the scroll bar.
                    ui.set_max_width(ui.available_width() - 12.0);
                    crate::instruct::versions_ui(ui, chrome, s, versions)
                })
                .inner;
        })
        .response;
    chrome.hit("code versions list", r.rect);
    act
}

/// The Code header's name: the open file, truncated to leave the row's
/// glyphs and notes their room, drawn as "code file" for the tests.
pub fn file_header(ui: &mut egui::Ui, chrome: &mut Chrome, name: &str, tip: &str) {
    // Three glyphs (find, wrap, versions), their spacing, the errors'
    // count and Deploy stay on the row.
    let room =
        (ui.available_width() - 3.0 * (crate::glyphs::ROW + 6.0) - crate::glyphs::DEPLOY_W - 80.0).max(60.0);
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
        let mut tip = format!("The change removed {n} line(s) here:");
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
