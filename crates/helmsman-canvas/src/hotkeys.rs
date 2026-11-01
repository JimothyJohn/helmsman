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
//! | Side panel | Alt 3 | Messages view |
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
    St,
    Panel,
}

/// One shortcut: the action, its keys and the CODESYS command the keys
/// come from (empty: a key of our own).
#[derive(Debug, Clone, Copy)]
pub struct Hotkey {
    pub action: Action,
    pub keys: KeyboardShortcut,
    /// Where the key comes from; the module doc's table, as data.
    #[cfg_attr(not(test), allow(dead_code))]
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
    key(Action::Files, Modifiers::ALT, Key::Num0, "Devices"),
    key(Action::St, Modifiers::ALT, Key::Num1, "POUs"),
    key(Action::Panel, Modifiers::ALT, Key::Num3, "Messages"),
];

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
pub fn pressed(ctx: &egui::Context) -> Vec<Action> {
    ctx.input_mut(|i| {
        TABLE
            .iter()
            .filter(|h| i.consume_shortcut(&h.keys))
            .map(|h| h.action)
            .collect()
    })
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
    use eframe::egui::{Event, Key, Modifiers};

    use super::{Action, TABLE, wants};

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
