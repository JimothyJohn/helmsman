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
//! | Run / Stop (simulation) | Ctrl/⌘ Enter | (Start is F5, the browser's) |
//! | Stop (simulation) | Shift F8 | Stop |
//! | Step one scan (simulation, stopped) | F8 | Step Into (Step Over is F10, the toolbar's focus in Chrome and Edge on Windows) |
//! | Sign in (box) | Alt F8 | Login |
//! | Sign out (box) | Ctrl F8 | Logout |
//! | Undo / Redo | Ctrl/⌘ Z, Ctrl/⌘ Shift Z; Ctrl Y off a Mac | Undo / Restore |
//! | Files sidebar | Alt Shift 0 | Devices view (Alt 1 to 9 switch Firefox's tabs on Windows and Linux) |
//! | Structured Text | Alt Shift 1 | POUs view |
//! | Trend along the bottom | Alt Shift 2 | Modules view |
//! | Side panel | Alt Shift 3 | Messages view |
//! | Devices tab | Alt Shift 4 | (a key of our own) |
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
];

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
        Action::RunStop => "Run or stop the simulation",
        Action::Stop => "Stop the simulation",
        Action::Step => "Step one scan (stopped)",
        Action::SignIn => "Sign in (box)",
        Action::SignOut => "Sign out (box)",
        Action::Undo => "Undo in the Structured Text",
        Action::Redo => "Redo in the Structured Text",
        Action::Files => "The left pane's Files tab",
        Action::Devices => "The left pane's Devices tab (the I/O map's devices and points)",
        Action::St => "The left pane's Code tab (the Structured Text)",
        Action::Trend => "The bottom panel (Trend, Messages)",
        Action::Panel => "The side panel (I/O, Agent)",
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
         web. **Ctrl is ⌘ on a Mac.** Every menu item shows its key.\n\n\
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
    out.push_str("\nReserved, not built yet: F2 / Shift F2 for the Agent.\n");
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
                    && TABLE
                        .iter()
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
    let mut order: Vec<&Hotkey> = TABLE
        .iter()
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
        } => TABLE
            .iter()
            .any(|h| h.keys.logical_key == *key && modifiers.matches_logically(h.keys.modifiers)),
        _ => false,
    }
}

#[cfg(test)]
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
}
