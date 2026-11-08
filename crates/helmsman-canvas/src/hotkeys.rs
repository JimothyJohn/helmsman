//! Keyboard shortcuts. CODESYS's defaults where the browser leaves the
//! key alone (verified against the CODESYS Development System help,
//! "Shortcuts", 2026-11-01), and nothing on a key the browser owns: F5
//! and Ctrl+R reload, F11 goes full screen, Ctrl+F5 reloads hard, so
//! Start, Generate Code and Single Cycle have no key of CODESYS's here.
//!
//! | here | key | CODESYS command on that key |
//! |---|---|---|
//! | Save | Ctrl/⌘ S | Save Project |
//! | Deploy | Ctrl/⌘ D | (Download has no default key) |
//! | Run / Stop (simulation) | Ctrl/⌘ Enter | (Start is F5, the browser's) |
//! | Stop (simulation) | Shift F8 | Stop |
//! | Step one scan (simulation, stopped) | F10 | Step Over |
//! | Sign in (box) | Alt F8 | Login |
//! | Sign out (box) | Ctrl F8 | Logout |
//! | Undo / Redo | Ctrl/⌘ Z, Ctrl/⌘ Y | Undo / Restore |
//! | Files sidebar | Alt 0 | Devices view |
//! | Structured Text | Alt 1 | POUs view |
//! | Trend along the bottom | Alt 2 | Modules view |
//! | Side panel | Alt 3 | Messages view |
//! | Devices tab | Alt 4 | (a key of our own) |
//! | Go to definition (picked block's variable) | Ctrl/⌘ Shift D | Go to Definition |
//! | Cross references (its reads and writes lit) | Ctrl/⌘ Shift X | Display Cross References |
//! | Breakpoint on the picked block (simulation) | Ctrl/⌘ B | (Toggle Breakpoint is F9 there) |
//! | Force the picked input | Ctrl/⌘ Shift F | (a key of our own) |
//! | Release every force (box) | Ctrl/⌘ Shift U | (a key of our own) |
//! | Into the picked instance's diagram, one scan (stopped) | Ctrl/⌘ ] | (a key of our own) |
//! | Out to the caller's diagram, one scan (stopped) | Ctrl/⌘ [ | (a key of our own) |
//! | Find in the Structured Text | Ctrl/⌘ F | (a key of our own) |
//! | Next / previous match | Ctrl/⌘ G, Ctrl/⌘ Shift G | (a key of our own) |
//! | Up one level in the diagram | Alt ↑ | (`st_view`, as before) |
//!
//! The shortcuts are read at the end of the frame, after every widget
//! has had its say: a text editor with the focus keeps its own Ctrl+Z.
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
}

const fn key(action: Action, modifiers: Modifiers, logical_key: Key, codesys: &'static str) -> Hotkey {
    Hotkey {
        action,
        keys: KeyboardShortcut::new(modifiers, logical_key),
        codesys,
    }
}

/// Every shortcut, in the order they are tried.
pub const TABLE: &[Hotkey] = &[
    key(Action::Save, Modifiers::COMMAND, Key::S, "Save Project"),
    key(Action::Deploy, Modifiers::COMMAND, Key::D, ""),
    key(Action::RunStop, Modifiers::COMMAND, Key::Enter, ""),
    key(Action::Stop, Modifiers::SHIFT, Key::F8, "Stop"),
    key(Action::Step, Modifiers::NONE, Key::F10, "Step Over"),
    key(Action::SignIn, Modifiers::ALT, Key::F8, "Login"),
    key(Action::SignOut, Modifiers::CTRL, Key::F8, "Logout"),
    key(Action::Undo, Modifiers::COMMAND, Key::Z, "Undo"),
    key(Action::Redo, Modifiers::COMMAND, Key::Y, "Restore"),
    key(
        Action::Redo,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::Z,
        "",
    ),
    key(
        Action::GoToDefinition,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::D,
        "Go to Definition",
    ),
    key(
        Action::CrossReferences,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::X,
        "Display Cross References",
    ),
    key(Action::Breakpoint, Modifiers::COMMAND, Key::B, ""),
    key(
        Action::Force,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::F,
        "",
    ),
    key(
        Action::ReleaseAll,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::U,
        "",
    ),
    key(Action::StepInto, Modifiers::COMMAND, Key::CloseBracket, ""),
    key(Action::StepOut, Modifiers::COMMAND, Key::OpenBracket, ""),
    key(Action::Find, Modifiers::COMMAND, Key::F, ""),
    key(Action::FindNext, Modifiers::COMMAND, Key::G, ""),
    key(
        Action::FindPrevious,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::G,
        "",
    ),
    key(Action::Files, Modifiers::ALT, Key::Num0, "Devices"),
    key(Action::St, Modifiers::ALT, Key::Num1, "POUs"),
    key(Action::Trend, Modifiers::ALT, Key::Num2, "Modules"),
    key(Action::Panel, Modifiers::ALT, Key::Num3, "Messages"),
    key(Action::Devices, Modifiers::ALT, Key::Num4, ""),
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
            "Breakpoint on the picked block (simulation): the scan stops before it; F10 runs the rest \
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
         CODESYS's default keys where the browser leaves them alone, nothing on a key the \
         browser owns (F5 and Ctrl R reload, F11 goes full screen). The page keeps the \
         browser's default off these. **Ctrl is ⌘ on a Mac.** Every menu item shows its key.\n\n\
         | key | does | CODESYS command on that key |\n|---|---|---|\n",
    );
    for h in TABLE {
        let d = does(h.action);
        if d.is_empty() {
            continue;
        }
        let codesys = if h.codesys.is_empty() { "" } else { h.codesys };
        let _ = writeln!(out, "| {} | {d} | {codesys} |", key_text(h.keys));
    }
    out.push_str("| Alt ↑ | Up one level in the diagram | |\n");
    out.push_str("\nReserved, not built yet: F2 / Shift F2 for the Agent.\n");
    out
}

/// The first shortcut of `action`, as the menus print it (`⌘S` on a
/// Mac, `Ctrl+S` elsewhere).
#[must_use]
pub fn label(ctx: &egui::Context, action: Action) -> String {
    TABLE
        .iter()
        .find(|h| h.action == action)
        .map(|h| ctx.format_shortcut(&h.keys))
        .unwrap_or_default()
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
        .memory(|m| m.focused())
        .is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some());
    let mut order: Vec<&Hotkey> = TABLE
        .iter()
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

    use super::{Action, TABLE, pressed, wants};

    /// What one frame with `key` down reports.
    fn frame(modifiers: Modifiers, key: Key) -> Vec<Action> {
        let ctx = egui::Context::default();
        let input = RawInput {
            events: vec![down(modifiers, key)],
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut full = ctx.run_ui(input, |ui| out = pressed(ui.ctx()));
        full.textures_delta.clear();
        out
    }

    #[test]
    fn a_shortcut_with_more_modifiers_is_not_swallowed_by_a_shorter_one() {
        assert_eq!(frame(Modifiers::COMMAND, Key::D), [Action::Deploy]);
        assert_eq!(
            frame(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::D),
            [Action::GoToDefinition]
        );
        assert_eq!(frame(Modifiers::COMMAND, Key::Z), [Action::Undo]);
        assert_eq!(
            frame(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z),
            [Action::Redo]
        );
        assert_eq!(frame(Modifiers::NONE, Key::F10), [Action::Step]);
        assert_eq!(frame(Modifiers::NONE, Key::D), []);
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
        assert!(wants(&down(Modifiers::NONE, Key::F10)));
        // Back and Forward on a Mac: ours while the page has the keyboard.
        assert!(wants(&down(Modifiers::COMMAND, Key::OpenBracket)));
        assert!(wants(&down(Modifiers::COMMAND, Key::CloseBracket)));
        for (m, k) in [
            (Modifiers::NONE, Key::F5),
            (Modifiers::NONE, Key::F11),
            (Modifiers::COMMAND, Key::R),
            (Modifiers::COMMAND, Key::C),
            (Modifiers::COMMAND, Key::F5),
            (Modifiers::NONE, Key::F8),
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
