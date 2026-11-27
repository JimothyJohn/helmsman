//! Keyboard shortcuts. CODESYS's defaults where no browser uses the key
//! (verified against the CODESYS Development System help, "Shortcuts",
//! 2026-11-01), and nothing on a chord a browser owns: the page cannot
//! count on keeping one (the browser handles it when the canvas lacks
//! the focus; Nick's Ctrl+D opened the bookmark dialog, 2026-11-08). The
//! browsers' own tables were read on 2026-11-08 (docs/SOURCES.md,
//! "Browser keyboard shortcuts"): Chrome and Edge for Windows and macOS,
//! Safari, and Firefox's keyset source. [`BROWSER`] and [`BROWSER_MAC`]
//! hold their chords in our key space, and a test keeps the table off
//! them. The one exception is the editor's own conventions, Ctrl S,
//! Ctrl Z, Ctrl Shift Z, Ctrl F and Ctrl G: every editor on the web takes
//! these from the browser, and the browser's own default (save the page,
//! find in the page) does no harm if it slips through. Ctrl Y redoes only
//! off a Mac, where ⌘Y is the browser's History.
//!
//! | here | key | CODESYS command on that key |
//! |---|---|---|
//! | Save | Ctrl/⌘ S | Save Project |
//! | Deploy | Ctrl/⌘ Shift Enter | (Download has no default key; Ctrl D bookmarks the page) |
//! | Run / Pause (simulation) | Ctrl/⌘ Enter | (Start is F5, the browser's) |
//! | Stop (simulation: back to initial values) | Shift F8 | Stop |
//! | Step to the next change (simulation, paused or stopped) | F8 | Step Into (Step Over is F10, the toolbar's focus in Chrome and Edge on Windows) |
//! | Sign in (box) | Alt F8 | Login |
//! | Sign out (box) | Ctrl F8 | Logout |
//! | Undo / Redo | Ctrl/⌘ Z, Ctrl/⌘ Shift Z; Ctrl Y off a Mac | Undo / Restore |
//! | Files sidebar | Alt Shift 0 | Devices view (Alt 1 to 9 switch Firefox's tabs on Windows and Linux) |
//! | Structured Text | Alt Shift 1 | POUs view |
//! | Trend along the bottom | Alt Shift 2 | Modules view |
//! | Right pane | Alt Shift 3 | Messages view |
//! | Devices tab | Alt Shift 4 | (a key of our own) |
//! | Move the left pane's tab to the right | Alt Shift 5 | (a key of our own) |
//! | Go to definition (picked block's variable) | Alt Shift D | Go to Definition (Ctrl Shift D bookmarks every tab) |
//! | Cross references (its reads and writes lit) | Alt Shift X | Display Cross References (Ctrl Shift X is Firefox's text direction) |
//! | Breakpoint on the picked block (simulation) | Shift F9 | (Toggle Breakpoint is F9, Edge's reader; Ctrl B is Firefox's bookmarks) |
//! | Force the picked input | Alt Shift F | (a key of our own; Ctrl Shift F is Firefox's full screen on a Mac) |
//! | Release every force (box) | Alt Shift U | (a key of our own; Ctrl Shift U is Edge's Read aloud) |
//! | Into the picked instance's diagram, one scan (stopped) | Alt ] | (a key of our own; Ctrl ] is Forward on a Mac and in Firefox) |
//! | Out to the caller's diagram, one scan (stopped) | Alt [ | (a key of our own; Ctrl [ is Back) |
//! | Find in the Structured Text | Ctrl/⌘ F | (a key of our own) |
//! | Next / previous match | Ctrl/⌘ G, Ctrl/⌘ Shift G | (a key of our own) |
//! | Up one level in the diagram | Alt ↑ | (`st_view`, as before) |
//! | Next / previous function block call, IF, CASE or loop, framed | . , | (`jump`; keys of our own, read by the diagram while the pointer is over it and nothing is typed in) |
//!
//! The shortcuts are read at the end of the frame, after every widget
//! has had its say: a text editor with the focus keeps its own Ctrl+Z.
//! An Alt chord types a character on a Mac (⌥⇧D is Î), so the frame
//! starts by dropping the text one of ours would type ([`swallow_text`]).
//! On the web the page stops the browser's default for these keys
//! ([`wants`], through eframe's `should_prevent_default`).

use eframe::egui::{self, Event, Key, KeyboardShortcut, Modifiers};

/// What a shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Save,
    Deploy,
    RunStop,
    Stop,
    Step,
    SignIn,
    SignOut,
    Undo,
    Redo,
    Files,
    Devices,
    St,
    Panel,
    /// The left pane's tab to the right pane.
    DockRight,
    Trend,
    GoToDefinition,
    CrossReferences,
    Breakpoint,
    Force,
    ReleaseAll,
    StepInto,
    StepOut,
    Find,
    FindNext,
    FindPrevious,
    /// Cold restart of the simulation (a glyph; no key).
    Restart,
    Upload,
    Download,
    /// The Describe tab (the Agent's).
    Describe,
    /// The I/O tab.
    Io,
    /// The Messages, a tab of the bottom panel beside the Trend.
    Messages,
    /// The Editor screen.
    Editor,
    /// The Monitor screen.
    Monitor,
    /// The box's Proposals screen.
    Proposals,
    /// The Describe tab, in its history: the newest instruction and answer.
    History,
}

/// One shortcut: the action, its keys and the CODESYS command the keys
/// come from (empty: a key of our own).
#[derive(Debug, Clone, Copy)]
pub struct Hotkey {
    pub action: Action,
    pub keys: KeyboardShortcut,
    /// Where the key comes from; the module doc's table, as data (the
    /// help page prints it; the browser build never does).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub codesys: &'static str,
    /// Whether the chord holds on a Mac too (Ctrl Y does not: ⌘Y is the
    /// browser's History there).
    pub mac: bool,
}

const fn key(action: Action, modifiers: Modifiers, logical_key: Key, codesys: &'static str) -> Hotkey {
    Hotkey {
        action,
        keys: KeyboardShortcut::new(modifiers, logical_key),
        codesys,
        mac: true,
    }
}

/// [`key`] for a chord the browser owns on a Mac alone.
const fn key_off_mac(
    action: Action,
    modifiers: Modifiers,
    logical_key: Key,
    codesys: &'static str,
) -> Hotkey {
    Hotkey {
        action,
        keys: KeyboardShortcut::new(modifiers, logical_key),
        codesys,
        mac: false,
    }
}

/// Every shortcut, in the order they are tried.
pub const TABLE: &[Hotkey] = &[
    key(Action::Save, Modifiers::COMMAND, Key::S, "Save Project"),
    key(
        Action::Deploy,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::Enter,
        "",
    ),
    key(Action::RunStop, Modifiers::COMMAND, Key::Enter, ""),
    key(Action::Stop, Modifiers::SHIFT, Key::F8, "Stop"),
    key(Action::Step, Modifiers::NONE, Key::F8, "Step Into"),
    key(Action::SignIn, Modifiers::ALT, Key::F8, "Login"),
    key(Action::SignOut, Modifiers::CTRL, Key::F8, "Logout"),
    key(Action::Undo, Modifiers::COMMAND, Key::Z, "Undo"),
    key(
        Action::Redo,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::Z,
        "",
    ),
    key_off_mac(Action::Redo, Modifiers::COMMAND, Key::Y, "Restore"),
    key(
        Action::GoToDefinition,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::D,
        "",
    ),
    key(
        Action::CrossReferences,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::X,
        "",
    ),
    key(Action::Breakpoint, Modifiers::SHIFT, Key::F9, ""),
    key(Action::Force, Modifiers::ALT.plus(Modifiers::SHIFT), Key::F, ""),
    key(
        Action::ReleaseAll,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::U,
        "",
    ),
    key(Action::StepInto, Modifiers::ALT, Key::CloseBracket, ""),
    key(Action::StepOut, Modifiers::ALT, Key::OpenBracket, ""),
    key(Action::Find, Modifiers::COMMAND, Key::F, ""),
    key(Action::FindNext, Modifiers::COMMAND, Key::G, ""),
    key(
        Action::FindPrevious,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::G,
        "",
    ),
    key(
        Action::Files,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::Num0,
        "Devices",
    ),
    key(
        Action::St,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::Num1,
        "POUs",
    ),
    key(
        Action::Trend,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::Num2,
        "Modules",
    ),
    key(
        Action::Panel,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::Num3,
        "Messages",
    ),
    key(
        Action::Devices,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::Num4,
        "",
    ),
    key(
        Action::DockRight,
        Modifiers::ALT.plus(Modifiers::SHIFT),
        Key::Num5,
        "",
    ),
    // One plain letter for every view, the first of its name (see
    // [`VIEWS`]); read only while nothing is taking text ([`pressed_views`]).
    view(Action::Editor, Key::E),
    view(Action::Monitor, Key::M),
    view(Action::Proposals, Key::P),
    view(Action::Describe, Key::D),
    // Nick, 2026-11-26: A for the Agent tab too, its field ready to type in.
    view(Action::Describe, Key::A),
    view(Action::St, Key::S),
    view(Action::Io, Key::I),
    view(Action::Devices, Key::V),
    view(Action::Files, Key::F),
    view(Action::Messages, Key::G),
    view(Action::Trend, Key::T),
    // Not a view: the Describe tab's history (H for history).
    view(Action::History, Key::H),
];

/// Every view a user switches to, in the order the letters were given out
/// (Nick, 2026-11-25: every view's key is its name's first letter, unless
/// typing in a field): the screens, then the left pane's tabs in their
/// row, then the Messages and the Trend. A name whose first letter an
/// earlier view holds takes the next free letter of its own name:
/// Devices (D is Describe's) takes V, since E is the Editor's, and
/// Messages (M is the Monitor's) takes G, since E and S are taken and A
/// went to Describe as the Agent tab (Nick, 2026-11-26: A opens it with
/// its field ready to type in; D still does too). The keys are in
/// [`TABLE`] as plain letters; a test keeps the two in step.
#[cfg_attr(not(test), allow(dead_code))]
pub const VIEWS: &[(Action, &str)] = &[
    (Action::Editor, "Editor"),
    (Action::Monitor, "Monitor"),
    (Action::Proposals, "Proposals"),
    (Action::Describe, "Describe"),
    (Action::St, "Structured Text"),
    (Action::Io, "I/O"),
    (Action::Devices, "Devices"),
    (Action::Files, "Files"),
    (Action::Messages, "Messages"),
    (Action::Trend, "Trend"),
];

const fn view(action: Action, logical_key: Key) -> Hotkey {
    Hotkey {
        action,
        keys: KeyboardShortcut::new(Modifiers::NONE, logical_key),
        codesys: "",
        mac: true,
    }
}

impl Hotkey {
    /// A bare letter: a view's key, which a text field takes first.
    #[must_use]
    pub fn plain(&self) -> bool {
        self.keys.modifiers == Modifiers::NONE && self.keys.logical_key.name().len() == 1
    }
}

/// The chords of [`TABLE`], without the plain letters.
fn chords() -> impl Iterator<Item = &'static Hotkey> {
    TABLE.iter().filter(|h| !h.plain())
}

/// The chords the browsers own in our key space, read on 2026-11-08 from
/// Chrome's and Edge's shortcut pages (Windows and macOS), Safari's, and
/// Firefox's keyset source (docs/SOURCES.md, "Browser keyboard
/// shortcuts"). `COMMAND` is Ctrl on Windows and Linux, ⌘ on a Mac. The
/// table stays off these, bar the editor conventions named in the module
/// doc; `no_shortcut_is_a_browsers` checks.
#[cfg_attr(not(test), allow(dead_code))]
pub const BROWSER: &[(Modifiers, Key, &str)] = &[
    (
        Modifiers::COMMAND,
        Key::D,
        "bookmark the page: Chrome, Edge, Firefox, Safari",
    ),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::D,
        "bookmark every tab: Chrome, Edge, Firefox; Safari's Reading List",
    ),
    (Modifiers::COMMAND, Key::B, "Firefox: the bookmarks sidebar"),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::B,
        "the bookmarks bar: Chrome, Edge, Firefox",
    ),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::X,
        "Firefox: switch the text direction",
    ),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::F,
        "Firefox on a Mac: full screen",
    ),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::U,
        "Edge: Read aloud",
    ),
    (Modifiers::CTRL, Key::U, "Firefox: the tabs sidebar"),
    (
        Modifiers::COMMAND,
        Key::OpenBracket,
        "Back: Firefox; Chrome, Edge and Safari on a Mac",
    ),
    (
        Modifiers::COMMAND,
        Key::CloseBracket,
        "Forward: Firefox; Chrome, Edge and Safari on a Mac",
    ),
    (Modifiers::COMMAND, Key::S, "save the page: every browser"),
    (Modifiers::COMMAND, Key::F, "find in the page: every browser"),
    (
        Modifiers::COMMAND,
        Key::G,
        "the next match: Chrome, Edge, Firefox",
    ),
    (
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::G,
        "the previous match: Chrome, Edge, Firefox",
    ),
    (Modifiers::COMMAND, Key::R, "reload: every browser"),
    (Modifiers::COMMAND, Key::P, "print: every browser"),
    (Modifiers::COMMAND, Key::O, "open a file: every browser"),
    (Modifiers::COMMAND, Key::N, "a new window: every browser"),
    (Modifiers::COMMAND, Key::T, "a new tab: every browser"),
    (Modifiers::COMMAND, Key::W, "close the tab: every browser"),
    (Modifiers::COMMAND, Key::L, "the address bar: every browser"),
    (
        Modifiers::COMMAND,
        Key::H,
        "History on Windows: Chrome, Edge; Hide on a Mac",
    ),
    (
        Modifiers::COMMAND,
        Key::J,
        "Downloads on Windows: Chrome, Edge, Firefox",
    ),
    (
        Modifiers::COMMAND,
        Key::K,
        "search from the address bar: Chrome, Edge, Firefox",
    ),
    (Modifiers::COMMAND, Key::E, "search: Chrome, Edge, Firefox"),
    (
        Modifiers::COMMAND,
        Key::U,
        "the page source: Chrome, Edge, Firefox",
    ),
    (
        Modifiers::COMMAND,
        Key::M,
        "mute the tab: Edge; minimize on a Mac",
    ),
    (
        Modifiers::ALT,
        Key::D,
        "the address bar: Chrome, Edge, Firefox on Windows",
    ),
    (
        Modifiers::ALT,
        Key::E,
        "the browser menu: Chrome, Edge on Windows",
    ),
    (
        Modifiers::ALT,
        Key::F,
        "the browser menu: Chrome, Edge on Windows",
    ),
    (
        Modifiers::ALT,
        Key::Num1,
        "Firefox on Windows and Linux: the first tab",
    ),
    (
        Modifiers::ALT,
        Key::Num2,
        "Firefox on Windows and Linux: the second tab",
    ),
    (
        Modifiers::ALT,
        Key::Num3,
        "Firefox on Windows and Linux: the third tab",
    ),
    (
        Modifiers::ALT,
        Key::Num4,
        "Firefox on Windows and Linux: the fourth tab",
    ),
    (
        Modifiers::ALT,
        Key::Num9,
        "Firefox on Windows and Linux: the last tab",
    ),
    (
        Modifiers::ALT,
        Key::ArrowLeft,
        "Back: Chrome, Edge, Firefox on Windows",
    ),
    (
        Modifiers::ALT,
        Key::ArrowRight,
        "Forward: Chrome, Edge, Firefox on Windows",
    ),
    (Modifiers::ALT, Key::F4, "close the window: Windows"),
    (Modifiers::NONE, Key::F1, "Help: Chrome, Edge"),
    (Modifiers::NONE, Key::F3, "find: Chrome, Edge, Firefox"),
    (Modifiers::SHIFT, Key::F3, "the previous match: Firefox"),
    (Modifiers::NONE, Key::F4, "the address bar: Edge"),
    (Modifiers::CTRL, Key::F4, "close the tab: Chrome, Edge on Windows"),
    (Modifiers::NONE, Key::F5, "reload: Chrome, Edge, Firefox"),
    (Modifiers::SHIFT, Key::F5, "reload past the cache: Chrome, Edge"),
    (
        Modifiers::CTRL,
        Key::F5,
        "reload past the cache: Firefox; the address bar: Chrome, Edge",
    ),
    (
        Modifiers::NONE,
        Key::F6,
        "the address bar or the next pane: Chrome, Edge",
    ),
    (Modifiers::SHIFT, Key::F6, "the previous pane: Edge"),
    (Modifiers::CTRL, Key::F6, "the web content: Chrome, Edge"),
    (Modifiers::NONE, Key::F7, "caret browsing: Chrome, Edge"),
    (Modifiers::NONE, Key::F9, "Immersive Reader: Edge"),
    (
        Modifiers::NONE,
        Key::F10,
        "the toolbar's focus: Chrome, Edge on Windows",
    ),
    (Modifiers::SHIFT, Key::F10, "the context menu: Edge on Windows"),
    (Modifiers::NONE, Key::F11, "full screen: Chrome, Edge, Firefox"),
    (Modifiers::NONE, Key::F12, "Developer Tools: Chrome, Edge"),
];

/// [`BROWSER`]'s Mac-only chords (⌘ on a Mac, nothing on Windows or
/// Linux): a hotkey marked off the Mac may sit on one.
#[cfg_attr(not(test), allow(dead_code))]
pub const BROWSER_MAC: &[(Modifiers, Key, &str)] = &[
    (
        Modifiers::COMMAND,
        Key::Y,
        "History: Chrome, Edge, Firefox on a Mac",
    ),
    (
        Modifiers::COMMAND,
        Key::Comma,
        "Settings: Chrome, Edge, Safari, Firefox on a Mac",
    ),
    (Modifiers::COMMAND, Key::Q, "quit: every browser on a Mac"),
];

/// What each action does, for the help page.
#[cfg(not(target_arch = "wasm32"))]
const fn does(action: Action) -> &'static str {
    match action {
        Action::Save => "Save",
        Action::Deploy => "Deploy",
        Action::RunStop => "Run or pause the simulation",
        Action::Stop => "Stop the simulation: initial values, clock to zero",
        Action::Step => "Step to the next change (paused or stopped)",
        Action::SignIn => "Sign in (box)",
        Action::SignOut => "Sign out (box)",
        Action::Undo => "Undo in the Structured Text",
        Action::Redo => "Redo in the Structured Text",
        Action::Files => "The Files tab",
        Action::Devices => "The Devices tab (the I/O map's devices and points)",
        Action::St => "The Code tab (the Structured Text)",
        Action::Trend => "The trend along the bottom",
        Action::Panel => "The right pane, once a tab is docked there",
        Action::DockRight => "The left pane's tab to a pane on the right, the next one shown on the left",
        Action::GoToDefinition => "Go to definition: the picked block's variable, at its VAR line",
        Action::CrossReferences => "Cross references: every read and write of it lit",
        Action::Breakpoint => {
            "Breakpoint on the picked block (simulation): the scan stops before it; F8 runs the rest \
             of the scan, Ctrl Enter goes on to the next stop"
        }
        Action::Force => {
            "Force the picked input: a switch flips, a number's field opens (on a box, an engineer's \
             audited force)"
        }
        Action::ReleaseAll => "Release every force on the box",
        Action::StepInto => "Into the picked block instance's diagram; a stopped simulation steps one scan",
        Action::StepOut => "Out to the caller's diagram; a stopped simulation steps one scan",
        Action::Find => "Find in the Structured Text (Enter: next, Esc: close)",
        Action::FindNext => "The next match",
        Action::FindPrevious => "The previous match",
        Action::Describe => "The Describe tab",
        Action::Io => "The I/O tab",
        Action::Messages => "The Messages, the bottom panel's tab beside the Trend",
        Action::Editor => "The Editor screen; on the Editor it flips to the Monitor",
        Action::Monitor => "The Monitor screen; on the Monitor it flips to the Editor",
        Action::Proposals => "The box's Proposals screen",
        Action::History => {
            "The Describe tab's history: the newest instruction and its answer, then Back and Forward"
        }
        Action::Restart | Action::Upload | Action::Download => "",
    }
}

/// A key as the help page prints it: `Ctrl S`, `Shift F8`, `Alt 0`.
/// Ctrl stands for ⌘ on a Mac.
#[cfg(not(target_arch = "wasm32"))]
fn key_text(k: KeyboardShortcut) -> String {
    let m = k.modifiers;
    let mut out = Vec::new();
    if m.ctrl || m.command || m.mac_cmd {
        out.push("Ctrl");
    }
    if m.alt {
        out.push("Alt");
    }
    if m.shift {
        out.push("Shift");
    }
    let key = match k.logical_key {
        Key::Enter => "Enter".to_string(),
        Key::ArrowUp => "↑".to_string(),
        k => k.symbol_or_name().to_string(),
    };
    out.push(&key);
    out.join(" ")
}

/// The shortcut table as Markdown (docs/help/keyboard.md is generated
/// from it by scripts/build-help.sh, so the page cannot drift).
#[must_use]
#[cfg(not(target_arch = "wasm32"))]
pub fn markdown() -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "# Keyboard\n\n\
         CODESYS's default keys where no browser uses them, and nothing on a key a browser \
         owns (Ctrl D bookmarks the page, Alt 1 is Firefox's first tab, F10 the toolbar), \
         checked against Chrome, Edge, Firefox and Safari. The editor's own conventions \
         stay: Ctrl S, Ctrl Z, Ctrl Shift Z, Ctrl F and Ctrl G, as in every editor on the \
         web. **Ctrl is ⌘ on a Mac.** Every menu item shows its key. \
         Every view has a key of one letter, the first of its name; it works whenever you are \
         not typing in a field, and with no Ctrl, Alt or ⌘ held. Over the diagram, \
         . and , jump along it. Help › Show hotkeys writes each key beside its \
         button.\n\n\
         | key | does | CODESYS command on that key |\n|---|---|---|\n",
    );
    for h in TABLE {
        let d = does(h.action);
        if d.is_empty() {
            continue;
        }
        let codesys = if h.codesys.is_empty() { "" } else { h.codesys };
        let keys = if h.mac {
            key_text(h.keys)
        } else {
            format!("{} (not on a Mac: ⌘Y is the browser's History)", key_text(h.keys))
        };
        let _ = writeln!(out, "| {keys} | {d} | {codesys} |");
    }
    out.push_str("| Alt ↑ | Up one level in the diagram | |\n");
    out.push_str(
        "| . , | The next or the previous function block call, IF, CASE or loop on the diagram, \
         zoomed in and framed (with the pointer over the diagram) | |\n",
    );
    out.push_str("\nReserved, not built yet: F2 / Shift F2 for the Describe tab.\n");
    out
}

/// The first shortcut of `action` that holds on this OS, as the menus
/// print it (`⌘S` on a Mac, `Ctrl+S` elsewhere).
#[must_use]
pub fn label(ctx: &egui::Context, action: Action) -> String {
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    TABLE
        .iter()
        .find(|h| h.action == action && (h.mac || !mac))
        .map(|h| ctx.format_shortcut(&h.keys))
        .unwrap_or_default()
}

/// What a control's superscript says (Help > Show hotkeys): the view's
/// letter where it has one, else [`label`].
#[must_use]
pub fn short(ctx: &egui::Context, action: Action) -> String {
    TABLE
        .iter()
        .find(|h| h.action == action && h.plain())
        .map_or_else(|| compact(&label(ctx, action)), |h| ctx.format_shortcut(&h.keys))
}

/// A chord in the few characters a superscript has room for: the
/// modifiers as their symbols (`⇧⌘↵` for Shift+Cmd+Enter).
fn compact(label: &str) -> String {
    label
        .replace("Ctrl+", "^")
        .replace("Cmd+", "⌘")
        .replace("Shift+", "⇧")
        .replace("Alt+", "⌥")
        .replace("Enter", "↵")
}

/// The views whose plain letter was pressed this frame, consumed. Nothing
/// is returned while a widget has the keyboard (a text field, the code
/// editor, the find bar), and a letter with any modifier held is left to
/// the browser and the chords. `diagram`: the pointer is over the diagram,
/// whose . and , jump along it; those keys stay its.
pub fn pressed_views(ctx: &egui::Context, diagram: bool) -> Vec<Action> {
    if ctx.egui_wants_keyboard_input() {
        return Vec::new();
    }
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = e
            else {
                return true;
            };
            let diagram_key = matches!(key, Key::Period | Key::Comma);
            if !modifiers.is_none() || (diagram && diagram_key) {
                return true;
            }
            match TABLE.iter().find(|h| h.plain() && h.keys.logical_key == *key) {
                Some(h) => {
                    out.push(h.action);
                    false
                }
                None => true,
            }
        });
    });
    out
}

/// Drops the text a chord of ours would type: an Alt chord puts a
/// character into a focused editor on a Mac (⌥⇧D is Î, ⌥] is ‘), so a
/// frame with one of our Alt chords down has no text. Called first in
/// the frame, before any widget reads the input.
pub fn swallow_text(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        let ours = i.events.iter().any(|e| {
            matches!(
                e,
                Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    ..
                } if modifiers.alt
                    && chords()
                        .any(|h| h.keys.logical_key == *key && modifiers.matches_logically(h.keys.modifiers))
            )
        });
        if ours {
            i.events.retain(|e| !matches!(e, Event::Text(_)));
        }
    });
}

/// The actions whose shortcut was pressed this frame, consumed so no
/// later reader sees them. Called once per frame, after the widgets.
/// The most modified shortcuts go first: egui matches a shortcut when
/// its modifiers are held, extra ones too, so Ctrl+D would otherwise
/// swallow Ctrl+Shift+D and Ctrl+Z Ctrl+Shift+Z.
pub fn pressed(ctx: &egui::Context) -> Vec<Action> {
    // A text edit with the keyboard undoes and redoes through its own
    // stack, whether or not it consumed the chord: taking it here too
    // would step the same history a second time.
    let typing = ctx
        .memory(egui::Memory::focused)
        .is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some());
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let mut order: Vec<&Hotkey> = chords()
        .filter(|h| h.mac || !mac)
        .filter(|h| !(typing && matches!(h.action, Action::Undo | Action::Redo)))
        .collect();
    order.sort_by_key(|h| std::cmp::Reverse(modifier_count(h.keys.modifiers)));
    ctx.input_mut(|i| {
        order
            .into_iter()
            .filter(|h| i.consume_shortcut(&h.keys))
            .map(|h| h.action)
            .collect()
    })
}

fn modifier_count(m: Modifiers) -> u8 {
    u8::from(m.alt) + u8::from(m.shift) + u8::from(m.ctrl || m.command || m.mac_cmd)
}

/// Whether the page should keep `e` (a key down) from the browser: it is
/// one of ours. Everything else is the browser's as before.
#[must_use]
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub fn wants(e: &Event) -> bool {
    match e {
        Event::Key {
            key,
            modifiers,
            pressed: true,
            ..
        } => chords().any(|h| h.keys.logical_key == *key && modifiers.matches_logically(h.keys.modifiers)),
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::assert_is_empty)]
mod tests {
    use eframe::egui::{self, Event, Key, Modifiers, RawInput};

    use eframe::egui::os::OperatingSystem;

    use super::{Action, BROWSER, BROWSER_MAC, TABLE, pressed, swallow_text, wants};

    /// What one frame with `key` down reports, on `os`.
    fn frame_on(os: OperatingSystem, modifiers: Modifiers, key: Key) -> Vec<Action> {
        let ctx = egui::Context::default();
        ctx.set_os(os);
        let input = RawInput {
            events: vec![down(modifiers, key)],
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut full = ctx.run_ui(input, |ui| out = pressed(ui.ctx()));
        full.textures_delta.clear();
        out
    }

    /// [`frame_on`] on Windows.
    fn frame(modifiers: Modifiers, key: Key) -> Vec<Action> {
        frame_on(OperatingSystem::Windows, modifiers, key)
    }

    #[test]
    fn a_shortcut_with_more_modifiers_is_not_swallowed_by_a_shorter_one() {
        assert_eq!(frame(Modifiers::COMMAND, Key::Enter), [Action::RunStop]);
        assert_eq!(
            frame(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Enter),
            [Action::Deploy]
        );
        assert_eq!(frame(Modifiers::COMMAND, Key::Z), [Action::Undo]);
        assert_eq!(
            frame(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z),
            [Action::Redo]
        );
        assert_eq!(frame(Modifiers::NONE, Key::F8), [Action::Step]);
        assert_eq!(frame(Modifiers::SHIFT, Key::F8), [Action::Stop]);
        assert_eq!(frame(Modifiers::ALT, Key::F8), [Action::SignIn]);
        assert_eq!(
            frame(Modifiers::ALT.plus(Modifiers::SHIFT), Key::D),
            [Action::GoToDefinition]
        );
        assert_eq!(
            frame(Modifiers::ALT.plus(Modifiers::SHIFT), Key::Num1),
            [Action::St]
        );
        assert_eq!(frame(Modifiers::NONE, Key::D), []);
        // The old chords are the browser's now.
        assert_eq!(frame(Modifiers::COMMAND, Key::D), []);
        assert_eq!(frame(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::D), []);
        assert_eq!(frame(Modifiers::NONE, Key::F10), []);
        assert_eq!(frame(Modifiers::ALT, Key::Num1), []);
        assert_eq!(frame(Modifiers::COMMAND, Key::B), []);
    }

    /// Nick, 2026-11-08: Ctrl+D opened the browser's bookmark dialog. No
    /// chord of ours is one a browser owns, bar the editor's conventions.
    #[test]
    fn no_shortcut_is_a_browsers() {
        let editor = |a: Action| {
            matches!(
                a,
                Action::Save
                    | Action::Undo
                    | Action::Redo
                    | Action::Find
                    | Action::FindNext
                    | Action::FindPrevious
            )
        };
        for h in TABLE {
            if editor(h.action) {
                continue;
            }
            let taken = BROWSER
                .iter()
                .chain(if h.mac { BROWSER_MAC } else { &[] })
                .find(|(m, k, _)| *k == h.keys.logical_key && *m == h.keys.modifiers);
            assert!(
                taken.is_none(),
                "{:?} on {:?} is the browser's: {}",
                h.action,
                h.keys,
                taken.map_or("", |t| t.2)
            );
        }
        // Save is the exception, and Ctrl Y the Mac's History.
        assert!(
            BROWSER
                .iter()
                .any(|(m, k, _)| *m == Modifiers::COMMAND && *k == Key::S)
        );
        let y = TABLE
            .iter()
            .find(|h| h.keys.logical_key == Key::Y)
            .expect("Ctrl Y");
        assert!(!y.mac);
    }

    #[test]
    fn ctrl_y_redoes_on_windows_and_linux_but_not_on_a_mac() {
        assert_eq!(
            frame_on(OperatingSystem::Windows, Modifiers::COMMAND, Key::Y),
            [Action::Redo]
        );
        assert_eq!(
            frame_on(OperatingSystem::Nix, Modifiers::COMMAND, Key::Y),
            [Action::Redo]
        );
        assert_eq!(frame_on(OperatingSystem::Mac, Modifiers::COMMAND, Key::Y), []);
        assert_eq!(
            frame_on(
                OperatingSystem::Mac,
                Modifiers::COMMAND.plus(Modifiers::SHIFT),
                Key::Z
            ),
            [Action::Redo]
        );
        let ctx = egui::Context::default();
        let mut full = ctx.run_ui(RawInput::default(), |_| {});
        full.textures_delta.clear();
        // The menu prints the chord that holds everywhere, never Ctrl Y.
        for os in [
            OperatingSystem::Mac,
            OperatingSystem::Windows,
            OperatingSystem::Nix,
        ] {
            ctx.set_os(os);
            let label = super::label(&ctx, Action::Redo);
            assert!(label.ends_with('Z') && !label.contains('Y'), "{os:?}: {label}");
        }
    }

    /// ⌥⇧D on a Mac keys Î into a focused editor as well as the chord.
    #[test]
    fn a_frame_with_an_alt_chord_of_ours_types_nothing() {
        let ctx = egui::Context::default();
        let typed = |events: Vec<Event>| {
            let mut left = 0;
            let mut full = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    swallow_text(ui.ctx());
                    left = ui.input(|i| i.events.iter().filter(|e| matches!(e, Event::Text(_))).count());
                },
            );
            full.textures_delta.clear();
            left
        };
        let alt_shift_d = down(Modifiers::ALT.plus(Modifiers::SHIFT), Key::D);
        assert_eq!(typed(vec![alt_shift_d.clone(), Event::Text("Î".into())]), 0);
        assert_eq!(
            typed(vec![
                down(Modifiers::ALT, Key::CloseBracket),
                Event::Text("‘".into())
            ]),
            0
        );
        // Plain typing, and a chord of ours without Alt, keep their text.
        assert_eq!(typed(vec![Event::Text("x".into())]), 1);
        assert_eq!(
            typed(vec![down(Modifiers::COMMAND, Key::S), Event::Text("s".into())]),
            1
        );
        // Alt on a key that is not ours types as before (⌥e is a dead key).
        assert_eq!(
            typed(vec![down(Modifiers::ALT, Key::E), Event::Text("´".into())]),
            1
        );
    }

    fn down(modifiers: Modifiers, key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn the_page_keeps_our_keys_and_leaves_the_browser_its_own() {
        assert!(wants(&down(Modifiers::COMMAND, Key::S)));
        assert!(wants(&down(Modifiers::SHIFT, Key::F8)));
        assert!(wants(&down(Modifiers::ALT, Key::F8)));
        assert!(wants(&down(Modifiers::NONE, Key::F8)));
        assert!(wants(&down(Modifiers::ALT.plus(Modifiers::SHIFT), Key::Num1)));
        assert!(wants(&down(Modifiers::ALT, Key::OpenBracket)));
        assert!(wants(&down(Modifiers::ALT, Key::CloseBracket)));
        for (m, k) in [
            (Modifiers::NONE, Key::F5),
            (Modifiers::NONE, Key::F11),
            (Modifiers::COMMAND, Key::R),
            (Modifiers::COMMAND, Key::C),
            (Modifiers::COMMAND, Key::F5),
            (Modifiers::NONE, Key::F10),
            (Modifiers::COMMAND, Key::D),
            (Modifiers::COMMAND, Key::OpenBracket),
            (Modifiers::ALT, Key::Num1),
        ] {
            assert!(!wants(&down(m, k)), "{k:?} with {m:?} is the browser's");
        }
        assert!(!wants(&Event::Key {
            key: Key::S,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }));
    }

    #[test]
    fn no_two_actions_share_a_key() {
        for (i, a) in TABLE.iter().enumerate() {
            for b in &TABLE[i + 1..] {
                assert!(
                    a.keys != b.keys || a.action == b.action,
                    "{:?} and {:?} both on {:?}",
                    a.action,
                    b.action,
                    a.keys
                );
            }
        }
        assert!(
            TABLE
                .iter()
                .any(|h| h.action == Action::Save && h.codesys == "Save Project")
        );
    }

    // ---- the plain letter of every view -------------------------------

    use super::{VIEWS, pressed_views, short};
    use crate::app::Screen;
    use crate::editor::Pane;

    /// The view action each pane opens. Exhaustive: a new pane does not
    /// compile until it is given a key here, and the test below then
    /// holds it to [`VIEWS`].
    const fn pane_action(p: Pane) -> Action {
        match p {
            Pane::Claude => Action::Describe,
            Pane::Code => Action::St,
            Pane::Io => Action::Io,
            Pane::Devices => Action::Devices,
            Pane::Files => Action::Files,
        }
    }

    const fn screen_action(s: Screen) -> Action {
        match s {
            Screen::Editor => Action::Editor,
            Screen::Monitor => Action::Monitor,
            Screen::Proposals => Action::Proposals,
        }
    }

    fn plain_keys(action: Action) -> Vec<Key> {
        TABLE
            .iter()
            .filter(|h| h.action == action && h.keys.modifiers == Modifiers::NONE)
            .map(|h| h.keys.logical_key)
            .collect()
    }

    #[test]
    fn every_view_has_a_plain_letter() {
        let mut views: Vec<(Action, String)> = Pane::ALL
            .iter()
            .map(|p| (pane_action(*p), p.label().to_string()))
            .collect();
        for s in [Screen::Editor, Screen::Monitor, Screen::Proposals] {
            views.push((screen_action(s), format!("{s:?}")));
        }
        views.push((Action::Messages, "Messages".into()));
        views.push((Action::Trend, "Trend".into()));
        for (a, name) in &views {
            let entry = VIEWS.iter().find(|(v, _)| v == a);
            let Some((_, vname)) = entry else {
                panic!("view {name} ({a:?}) has no entry in VIEWS");
            };
            assert!(
                vname.eq_ignore_ascii_case(name),
                "{a:?} is {vname} in VIEWS but {name} in the app"
            );
            assert_eq!(plain_keys(*a).len(), 1, "{name} needs exactly one plain key");
        }
        assert_eq!(VIEWS.len(), views.len(), "VIEWS has a view the app lacks");
    }

    #[test]
    fn views_and_plain_table_entries_match_one_to_one() {
        for (a, name) in VIEWS {
            assert_eq!(VIEWS.iter().filter(|(b, _)| b == a).count(), 1, "{name} twice");
            let keys = plain_keys(*a);
            assert_eq!(keys.len(), 1, "{name}: {keys:?}");
            let letter = keys[0].name();
            assert_eq!(letter.chars().count(), 1);
            assert!(
                name.to_ascii_lowercase().contains(&letter.to_ascii_lowercase()),
                "{name}'s key {letter} is not a letter of its name"
            );
        }
        let plain: Vec<_> = TABLE.iter().filter(|h| h.plain()).collect();
        assert_eq!(plain.len(), VIEWS.len(), "a plain key that is no view");
        for h in plain {
            assert!(
                VIEWS.iter().any(|(a, _)| *a == h.action),
                "{:?} is plain but no view",
                h.action
            );
            assert_eq!(h.keys.modifiers, Modifiers::NONE);
        }
        let mut letters: Vec<_> = VIEWS.iter().map(|(a, _)| plain_keys(*a)[0]).collect();
        letters.sort_by_key(|k| k.name());
        let n = letters.len();
        letters.dedup();
        assert_eq!(letters.len(), n, "two views share a letter");
    }

    #[test]
    fn the_letters_are_the_documented_ones() {
        let want = [
            (Action::Editor, Key::E),
            (Action::Monitor, Key::M),
            (Action::Proposals, Key::P),
            (Action::Describe, Key::D),
            (Action::St, Key::S),
            (Action::Io, Key::I),
            (Action::Devices, Key::V),
            (Action::Files, Key::F),
            (Action::Messages, Key::A),
            (Action::Trend, Key::T),
        ];
        for (a, k) in want {
            assert_eq!(plain_keys(a), [k], "{a:?}");
        }
    }

    /// What `pressed_views` reports for one frame of `events`, and the
    /// events that were left for everyone else.
    fn views_in(events: Vec<Event>, diagram: bool) -> (Vec<Action>, usize) {
        let ctx = egui::Context::default();
        let (mut out, mut left) = (Vec::new(), 0);
        let mut full = ctx.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                out = pressed_views(ui.ctx(), diagram);
                left = ui.input(|i| i.events.iter().filter(|e| matches!(e, Event::Key { .. })).count());
            },
        );
        full.textures_delta.clear();
        (out, left)
    }

    #[test]
    fn each_view_letter_reports_its_view_once_and_is_consumed() {
        for (a, name) in VIEWS {
            let key = plain_keys(*a)[0];
            assert_eq!(
                views_in(vec![down(Modifiers::NONE, key)], false),
                (vec![*a], 0),
                "{name}"
            );
        }
    }

    #[test]
    fn a_modified_released_or_repeated_letter_is_no_view() {
        for m in [
            Modifiers::CTRL,
            Modifiers::ALT,
            Modifiers::SHIFT,
            Modifiers::COMMAND,
            Modifiers::MAC_CMD,
        ] {
            for (a, name) in VIEWS {
                let key = plain_keys(*a)[0];
                assert_eq!(
                    views_in(vec![down(m, key)], false),
                    (vec![], 1),
                    "{name} with {m:?}"
                );
            }
        }
        let up = Event::Key {
            key: Key::F,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        assert!(views_in(vec![up], false).0.is_empty(), "a release is no press");
        let rep = Event::Key {
            key: Key::F,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: Modifiers::NONE,
        };
        // A held key reports its first press only: egui calls a press
        // of a key already down a repeat.
        let ctx = egui::Context::default();
        let mut got = Vec::new();
        for events in [vec![down(Modifiers::NONE, Key::F)], vec![rep.clone()], vec![rep]] {
            let mut full = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| got.push(pressed_views(ui.ctx(), false)),
            );
            full.textures_delta.clear();
        }
        assert_eq!(
            got,
            [vec![Action::Files], vec![], vec![]],
            "a held key does not repeat the switch"
        );
        assert!(
            views_in(vec![down(Modifiers::NONE, Key::Z)], false).0.is_empty(),
            "Z is no view"
        );
        assert!(views_in(vec![down(Modifiers::NONE, Key::F8)], false).0.is_empty());
    }

    #[test]
    fn over_the_diagram_the_diagrams_keys_are_left_alone() {
        // Nick, 2026-11-26: . and , jump along the diagram; W A S D no
        // longer pan it, so no letter is the diagram's any more.
        for key in [Key::Period, Key::Comma] {
            assert_eq!(
                views_in(vec![down(Modifiers::NONE, key)], true),
                (vec![], 1),
                "{key:?} over the diagram stays the diagram's"
            );
            assert_eq!(
                views_in(vec![down(Modifiers::NONE, key)], false),
                (vec![], 1),
                "{key:?} is no view anywhere"
            );
        }
        // Every view letter works over the diagram as off it, E (once a
        // jump key) and A, S, D (once a pan) among them.
        for (a, _) in VIEWS {
            let key = plain_keys(*a)[0];
            assert_eq!(
                views_in(vec![down(Modifiers::NONE, key)], true),
                (vec![*a], 0),
                "{key:?} over the diagram"
            );
        }
        for key in [Key::W, Key::Q] {
            assert_eq!(
                views_in(vec![down(Modifiers::NONE, key)], true),
                (vec![], 1),
                "{key:?} is nobody's"
            );
        }
    }

    #[test]
    fn nothing_is_read_while_a_widget_has_the_keyboard() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let id = egui::Id::unique("field");
        let mut run = |events: Vec<Event>, focus: bool| {
            let mut out = Vec::new();
            let mut full = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut text).id(id))
                        .on_hover_text("a field");
                    if focus {
                        r.request_focus();
                    }
                    out = pressed_views(ui.ctx(), false);
                },
            );
            full.textures_delta.clear();
            out
        };
        run(vec![], true);
        run(vec![], false);
        assert!(ctx.egui_wants_keyboard_input());
        for (a, _) in VIEWS {
            let key = plain_keys(*a)[0];
            assert!(
                run(vec![down(Modifiers::NONE, key)], false).is_empty(),
                "{a:?} while typing"
            );
            run(
                vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                false,
            );
        }
        // Focus gone, the letter is a view again.
        ctx.memory_mut(|m| m.surrender_focus(id));
        run(vec![], false);
        assert_eq!(run(vec![down(Modifiers::NONE, Key::F)], false), [Action::Files]);
    }

    #[test]
    fn a_plain_letter_is_never_the_pages_to_prevent_default_on() {
        for (a, name) in VIEWS {
            let key = plain_keys(*a)[0];
            assert!(
                !wants(&down(Modifiers::NONE, key)),
                "{name}: typing must reach the page"
            );
        }
        // And pressed() (the chords) does not report a bare letter.
        for (a, _) in VIEWS {
            let key = plain_keys(*a)[0];
            assert_eq!(frame(Modifiers::NONE, key), [], "{a:?}");
        }
    }

    #[test]
    fn an_alt_view_letter_still_types_its_text() {
        let ctx = egui::Context::default();
        for (a, name) in VIEWS {
            let key = plain_keys(*a)[0];
            let mut left = 0;
            let mut full = ctx.run_ui(
                RawInput {
                    events: vec![down(Modifiers::ALT, key), Event::Text("\u{b4}".into())],
                    ..Default::default()
                },
                |ui| {
                    swallow_text(ui.ctx());
                    left = ui.input(|i| i.events.iter().filter(|e| matches!(e, Event::Text(_))).count());
                },
            );
            full.textures_delta.clear();
            assert_eq!(left, 1, "{name}: Alt+{key:?} lost its text");
        }
    }

    #[test]
    fn short_is_the_letter_for_a_view_and_the_chord_otherwise() {
        let ctx = egui::Context::default();
        let mut full = ctx.run_ui(RawInput::default(), |_| {});
        full.textures_delta.clear();
        for (a, name) in VIEWS {
            let key = plain_keys(*a)[0];
            assert_eq!(short(&ctx, *a), key.name().to_ascii_uppercase(), "{name}");
        }
        assert_eq!(short(&ctx, Action::Files), "F");
        for a in [Action::RunStop, Action::Save, Action::Step, Action::Deploy] {
            // A chord's badge is its label, compacted (Ctrl+ is ^, and so on).
            let (s, l) = (short(&ctx, a), super::label(&ctx, a));
            assert!(
                !s.is_empty() && s.chars().count() <= l.chars().count(),
                "{a:?}: {s} / {l}"
            );
            for long in ["Ctrl+", "Cmd+", "Shift+", "Alt+", "Enter"] {
                assert!(!s.contains(long), "{a:?}: {s} still says {long}");
            }
        }
        for a in [Action::Restart, Action::Upload, Action::Download] {
            assert_eq!(short(&ctx, a), "", "{a:?} has no key");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_keyboard_page_lists_every_view_letter() {
        let md = super::markdown();
        for (a, name) in VIEWS {
            let key = plain_keys(*a)[0];
            let letter = key.name();
            assert!(
                md.lines()
                    .any(|l| l.starts_with('|') && l.contains(&format!("| {letter} |"))
                        || l.contains(&format!("`{letter}`"))),
                "no row for {name}'s {letter}:\n{md}"
            );
        }
    }

    /// docs/help/keyboard.md is what scripts/build-help.sh writes from
    /// [`super::markdown`]: the page a user reads says what the keys do
    /// (it said E and Q after they moved to . and , on 2026-11-26).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_shipped_keyboard_page_is_the_generated_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/help/keyboard.md");
        let shipped = std::fs::read_to_string(&path).unwrap();
        let md = super::markdown();
        assert_eq!(
            shipped.trim_end(),
            md.trim_end(),
            "docs/help/keyboard.md is stale: run scripts/build-help.sh"
        );
        assert!(md.contains("| . , |"), "the jump keys' row:\n{md}");
        assert!(!md.contains("| E, Q |"), "E and Q no longer jump:\n{md}");
    }
}
