//! Help › Show hotkeys (a superscript over every control that has a
//! shortcut) and the one plain letter every view has (E M P D S I V F A T),
//! which a text field takes first. Driven the way a visitor does: the Help
//! menu, the keyboard, the pointer.

// The asserts carry a message saying what was expected; assert_eq! on a
// whole list would drown it.
#![allow(clippy::assert_is_empty)]

use eframe::egui::{Event, Key, Modifiers, Pos2, Rect};

use super::{App, Harness, Screen};
use crate::editor::{Chrome, Pane};
use crate::glyphs;
use crate::hotkeys::{self, Action};
use crate::review_tests::Bench;

/// The text a badge drawn this frame for `action` has, and where.
fn badge_of(h: &Harness, action: Action) -> Vec<(String, Rect)> {
    glyphs::badges(&h.ctx)
        .into_iter()
        .filter(|(a, _, _)| *a == action)
        .map(|(_, t, r)| (t, r))
        .collect()
}

/// Whether some badge of `action` sits on the top-right corner of `control`.
fn on_corner(h: &Harness, action: Action, control: Rect) -> bool {
    let want = hotkeys::short(&h.ctx, action);
    !want.is_empty()
        && badge_of(h, action)
            .iter()
            .any(|(t, r)| *t == want && r.intersects(control) && (r.right() - control.right()).abs() <= 4.0)
}

fn with_hotkeys(app: App) -> Harness {
    let mut h = Harness::new(app);
    h.app.chrome.hotkeys = true;
    h.wait(0.2);
    h
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

/// Every view's letter, with the pane or flag it opens.
const LETTERS: [(Key, &str); 10] = [
    (Key::E, "E"),
    (Key::M, "M"),
    (Key::P, "P"),
    (Key::D, "D"),
    (Key::S, "S"),
    (Key::I, "I"),
    (Key::V, "V"),
    (Key::F, "F"),
    (Key::A, "A"),
    (Key::T, "T"),
];

/// What the chrome and the screen are, for "nothing changed".
fn state(h: &Harness) -> (String, Screen) {
    (h.app.chrome.saved(), h.app.screen)
}

// ---- Help > Show hotkeys -------------------------------------------------

#[test]
fn the_help_menu_has_show_hotkeys_and_it_toggles_off_by_default() {
    let mut h = Harness::new(App::sim());
    assert!(!h.app.chrome.hotkeys, "off by default");
    assert!(glyphs::badges(&h.ctx).is_empty(), "no badge while off");
    h.press("bar help");
    assert!(h.has("help hotkeys"), "Help has the item");
    h.press("help hotkeys");
    h.wait(0.2);
    assert!(h.app.chrome.hotkeys, "a click turns it on");
    assert!(!glyphs::badges(&h.ctx).is_empty(), "and the badges come");
    h.press("bar help");
    h.press("help hotkeys");
    h.wait(0.2);
    assert!(!h.app.chrome.hotkeys, "a second click turns it off");
    assert!(glyphs::badges(&h.ctx).is_empty(), "and they go");
}

#[test]
fn with_it_on_run_and_save_wear_their_key_on_the_top_right_corner() {
    for light in [false, true] {
        let mut h = Harness::new(App::sim());
        h.app.chrome.light = light;
        h.app.chrome.hotkeys = true;
        h.wait(0.2);
        for (hit, action) in [
            ("run", Action::RunStop),
            ("save", Action::Save),
            ("step", Action::Step),
        ] {
            if !h.has(hit) {
                continue;
            }
            let r = h.rect(hit);
            assert!(
                on_corner(&h, action, r),
                "light={light}: {hit} {r:?}: {:?}",
                badge_of(&h, action)
            );
        }
        for hit in ["run", "save"] {
            assert!(h.has(hit), "the sim page has {hit}");
        }
        let run = badge_of(&h, Action::RunStop);
        assert_eq!(run[0].0, hotkeys::short(&h.ctx, Action::RunStop));
        assert!(!run[0].0.is_empty());
    }
}

#[test]
fn a_control_with_no_shortcut_has_no_badge() {
    let h = with_hotkeys(App::sim());
    for hit in ["upload", "download", "restart", "help"] {
        if !h.has(hit) {
            continue;
        }
        let r = h.rect(hit);
        let touching: Vec<_> = glyphs::badges(&h.ctx)
            .into_iter()
            .filter(|(_, _, b)| b.intersects(r) && (b.right() - r.right()).abs() <= 4.0)
            .collect();
        assert!(touching.is_empty(), "{hit} at {r:?} wears {touching:?}");
    }
    assert!(h.has("upload") || h.has("restart") || h.has("help"));
}

#[test]
fn with_it_off_there_are_no_badges_anywhere() {
    let mut h = Harness::code();
    h.messages();
    h.wait(0.2);
    assert!(!h.app.chrome.hotkeys);
    assert!(glyphs::badges(&h.ctx).is_empty());
    h.app.chrome.hotkeys = true;
    h.wait(0.2);
    assert!(!glyphs::badges(&h.ctx).is_empty());
    h.app.chrome.hotkeys = false;
    h.wait(0.2);
    assert!(glyphs::badges(&h.ctx).is_empty(), "off again clears last frame's");
}

#[test]
fn showing_the_keys_moves_no_control_and_no_panel_edge() {
    for light in [false, true] {
        let mut h = Harness::code();
        h.app.chrome.light = light;
        h.messages();
        h.wait(0.3);
        let before = h.app.chrome.hits.clone();
        let (left, right, diagram) = (
            h.app.chrome.rect("left"),
            h.app.chrome.rect("right"),
            h.app.chrome.diagram,
        );
        assert!(!before.is_empty());
        h.app.chrome.hotkeys = true;
        h.wait(0.3);
        assert!(!glyphs::badges(&h.ctx).is_empty());
        assert_eq!(h.app.chrome.hits, before, "light={light}: a control moved");
        assert_eq!(h.app.chrome.rect("left"), left);
        assert_eq!(h.app.chrome.rect("right"), right);
        assert_eq!(h.app.chrome.diagram, diagram);
    }
}

#[test]
fn a_badge_does_not_eat_the_click_on_the_control_under_it() {
    let mut h = with_hotkeys(App::sim());
    assert!(h.app.sim.as_ref().unwrap().session.running, "opens running");
    let run = h.rect("run");
    let badge = badge_of(&h, Action::RunStop)[0].1;
    // Inside both the chip and the control, at the corner.
    let at = Pos2::new(run.right() - 2.0, run.top() + 2.0);
    assert!(badge.contains(at) && run.contains(at), "{badge:?} {run:?}");
    h.click(at);
    h.wait(0.1);
    assert!(
        !h.app.sim.as_ref().unwrap().session.running,
        "the corner click ran/stopped it"
    );
}

#[test]
fn the_setting_is_remembered_and_an_older_saved_text_restores_it_off() {
    let mut h = Harness::new(App::sim());
    assert!(
        h.app.chrome.saved().contains("hotkeys=0"),
        "{}",
        h.app.chrome.saved()
    );
    h.app.chrome.hotkeys = true;
    let saved = h.app.chrome.saved();
    assert!(saved.contains("hotkeys=1"), "{saved}");
    assert!(Chrome::restored(&saved).hotkeys);
    assert!(!Chrome::restored("left=1 right=0").hotkeys, "absent: off");
    assert!(!Chrome::restored("dock=2 left=1 hotkeys=0").hotkeys);
    assert!(!Chrome::restored("hotkeys=junk").hotkeys);
    // The whole layout survives with it.
    let again = Chrome::restored(&saved).saved();
    assert_eq!(again, saved);
    // A browser that kept it on opens with badges on its first frames.
    let mut app = App::sim();
    app.chrome = Chrome::restored(&saved);
    let h = Harness::new(app);
    assert!(!glyphs::badges(&h.ctx).is_empty());
}

#[test]
fn a_tab_wears_the_letter_its_key_has() {
    let mut h = with_hotkeys(App::sim());
    h.messages();
    for (hit, action, text) in [
        ("pane files", Action::Files, "F"),
        ("pane devices", Action::Devices, "V"),
        ("pane code", Action::St, "S"),
        ("tab claude", Action::Describe, "D"),
        ("bottom messages", Action::Messages, "A"),
    ] {
        // A tab on either side of the page has its own hit name.
        let r = if h.has(hit) {
            h.rect(hit)
        } else if hit == "tab claude" && h.has("pane claude") {
            h.rect("pane claude")
        } else {
            panic!("no {hit}");
        };
        assert_eq!(hotkeys::short(&h.ctx, action), text);
        assert!(
            on_corner(&h, action, r),
            "{hit} {r:?}: {:?}",
            badge_of(&h, action)
        );
    }
    let mut seen = glyphs::badges(&h.ctx);
    seen.retain(|(a, _, _)| *a == Action::Io);
    if h.has("tab io") || h.has("pane io") {
        let r = if h.has("tab io") {
            h.rect("tab io")
        } else {
            h.rect("pane io")
        };
        assert!(on_corner(&h, Action::Io, r), "the I/O tab wears I");
    }
}

#[test]
fn the_screen_switch_wears_e_and_m_on_the_editor_and_on_the_monitor() {
    for screen in [Screen::Editor, Screen::Monitor] {
        let mut h = Harness::new(App::sim());
        h.app.screen = screen;
        h.app.chrome.hotkeys = true;
        h.wait(0.2);
        assert!(h.has("editor") && h.has("monitor"));
        assert!(on_corner(&h, Action::Editor, h.rect("editor")), "{screen:?}");
        assert!(on_corner(&h, Action::Monitor, h.rect("monitor")), "{screen:?}");
        assert_eq!(hotkeys::short(&h.ctx, Action::Editor), "E");
        assert_eq!(hotkeys::short(&h.ctx, Action::Monitor), "M");
    }
}

#[test]
fn on_a_box_run_and_restart_show_no_key_but_save_and_the_tabs_do() {
    let mut app = App::boxed();
    app.chrome.hotkeys = true;
    let mut h = Harness::new(app);
    h.app.chrome.hotkeys = true;
    h.wait(0.2);
    for hit in ["run", "restart"] {
        if h.has(hit) {
            let r = h.rect(hit);
            let on: Vec<_> = glyphs::badges(&h.ctx)
                .into_iter()
                .filter(|(_, _, b)| b.intersects(r) && (b.right() - r.right()).abs() <= 4.0)
                .collect();
            assert!(on.is_empty(), "a box takes no key for {hit}: {on:?}");
        }
    }
    assert!(
        !glyphs::badges(&h.ctx).is_empty(),
        "other controls still show theirs"
    );
    for light in [false, true] {
        h.app.chrome.light = light;
        h.wait(0.2);
        assert!(!glyphs::badges(&h.ctx).is_empty(), "light={light}");
    }
}

// ---- the plain letters ---------------------------------------------------

#[test]
fn each_pane_letter_switches_to_that_tab_from_another() {
    for (key, pane, from) in [
        (Key::D, Pane::Claude, Pane::Files),
        (Key::S, Pane::Code, Pane::Files),
        (Key::I, Pane::Io, Pane::Files),
        (Key::V, Pane::Devices, Pane::Files),
        (Key::F, Pane::Files, Pane::Code),
    ] {
        let mut h = Harness::new(App::sim());
        h.app.chrome.show(from);
        h.wait(0.2);
        assert!(h.app.chrome.showing(from) && !h.app.chrome.showing(pane));
        h.key(Modifiers::NONE, key);
        h.wait(0.1);
        assert!(h.app.chrome.showing(pane), "{key:?} should show {pane:?}");
        assert!(!h.app.chrome.showing(from), "{key:?} left {from:?} up");
        assert_eq!(h.app.screen, Screen::Editor);
    }
}

#[test]
fn a_letter_does_what_a_click_on_its_tab_does() {
    for (key, hit, target) in [
        (Key::D, "tab claude", Some(Pane::Claude)),
        (Key::S, "pane code", Some(Pane::Code)),
        (Key::V, "pane devices", Some(Pane::Devices)),
        (Key::F, "pane files", Some(Pane::Files)),
        (Key::A, "bottom messages", None),
    ] {
        // From the tab already shown a click keeps it and a key toggles it
        // (as the Alt+Shift chords do): compared only from another tab.
        for start in [Pane::Files, Pane::Code, Pane::Claude] {
            if Some(start) == target {
                continue;
            }
            let (mut k, mut c) = (Harness::new(App::sim()), Harness::new(App::sim()));
            for h in [&mut k, &mut c] {
                h.app.chrome.show(start);
                // The bottom panel open on the Trend: A and a click on
                // its Messages tab both bring the Messages up.
                h.app.chrome.trend = true;
                h.app.chrome.msgs = false;
                h.wait(0.3);
            }
            let hit = if c.has(hit) { hit } else { "pane claude" };
            c.press(hit);
            c.wait(0.2);
            k.key(Modifiers::NONE, key);
            k.wait(0.2);
            assert_eq!(
                k.app.chrome.saved(),
                c.app.chrome.saved(),
                "{key:?} vs a click on {hit}, from {start:?}"
            );
        }
    }
}

#[test]
/// Nick, 2026-11-26: the Messages and the Trend are the two tabs of the
/// bottom panel (`chrome.trend`: the panel is open; `chrome.msgs`: it shows
/// the Messages). A and T each open the panel on their tab, or shut it
/// when their tab is the one showing.
fn a_and_t_toggle_the_messages_and_the_trend() {
    let mut h = Harness::new(App::sim());
    h.app.chrome.msgs = false;
    h.app.chrome.trend = false;
    h.wait(0.2);
    let open = |h: &Harness| (h.app.chrome.trend, h.app.chrome.msgs);
    h.key(Modifiers::NONE, Key::A);
    assert_eq!(open(&h), (true, true), "A opens the shut panel on the Messages");
    h.key(Modifiers::NONE, Key::A);
    assert!(!h.app.chrome.trend, "A again shuts it");
    h.key(Modifiers::NONE, Key::T);
    assert_eq!(open(&h), (true, false), "T opens the shut panel on the Trend");
    h.key(Modifiers::NONE, Key::A);
    assert_eq!(open(&h), (true, true), "A over the Trend shows the Messages");
    h.key(Modifiers::NONE, Key::T);
    assert_eq!(open(&h), (true, false), "T over the Messages shows the Trend");
    h.key(Modifiers::NONE, Key::T);
    assert!(!h.app.chrome.trend, "T again shuts it");
    // What the keys did is what the panel draws.
    h.key(Modifiers::NONE, Key::A);
    h.wait(0.2);
    assert!(h.has("bottom trend") && h.has("bottom messages"));
}

#[test]
fn e_and_m_flip_between_the_editor_and_the_monitor() {
    let mut h = Harness::new(App::sim());
    assert_eq!(h.app.screen, Screen::Editor);
    h.key(Modifiers::NONE, Key::M);
    assert_eq!(h.app.screen, Screen::Monitor);
    h.key(Modifiers::NONE, Key::E);
    assert_eq!(h.app.screen, Screen::Editor);
    h.key(Modifiers::NONE, Key::M);
    h.key(Modifiers::NONE, Key::M);
    assert_eq!(
        h.app.screen,
        Screen::Editor,
        "either key flips, as a click on the switch does"
    );
    // The same as a click on the switch.
    let mut c = Harness::new(App::sim());
    c.press("monitor");
    assert_eq!(c.app.screen, Screen::Monitor);
}

#[test]
fn on_the_monitor_only_the_screen_keys_act() {
    let mut h = Harness::new(App::sim());
    h.key(Modifiers::NONE, Key::M);
    assert_eq!(h.app.screen, Screen::Monitor);
    let before = state(&h);
    for (key, _) in LETTERS {
        if matches!(key, Key::E | Key::M | Key::P) {
            continue;
        }
        h.key(Modifiers::NONE, key);
        assert_eq!(state(&h), before, "{key:?} on the Monitor changed something");
    }
    h.key(Modifiers::NONE, Key::E);
    assert_eq!(h.app.screen, Screen::Editor, "E leaves the Monitor");
}

#[test]
fn p_does_nothing_on_the_simulation_which_has_no_proposals() {
    let mut h = Harness::new(App::sim());
    let before = state(&h);
    h.key(Modifiers::NONE, Key::P);
    assert_eq!(state(&h), before);
}

fn rv(h: &mut Harness) -> &mut crate::review_view::ReviewView {
    h.app.review.as_mut().unwrap()
}

fn pump(h: &mut Harness, b: &Bench) {
    for _ in 0..20 {
        h.frame(vec![]);
        let sent = std::mem::take(&mut rv(h).sent);
        if sent.is_empty() {
            return;
        }
        let answers: Vec<_> = sent.iter().map(|q| (q.call.clone(), b.send(q))).collect();
        let (ctx, t) = (h.ctx.clone(), h.time);
        rv(h).step(&ctx, answers, t);
        h.frame(vec![]);
    }
    panic!("the screen never went quiet");
}

#[test]
fn p_opens_the_proposals_on_a_box_with_a_proposal_waiting() {
    let b = Bench::new();
    let mut h = Harness::new(App::boxed());
    pump(&mut h, &b);
    rv(&mut h)
        .state
        .login_pin("operator", crate::review_tests::OPERATOR.into());
    pump(&mut h, &b);
    assert_eq!(h.app.waiting(), Some(0));
    // With nothing waiting the spec is silent (the header has no Proposals
    // badge, the menu has the item): not asserted either way.
    b.horn();
    h.time += 31.0;
    pump(&mut h, &b);
    assert_eq!(h.app.waiting(), Some(1));
    h.key(Modifiers::NONE, Key::P);
    assert_eq!(h.app.screen, Screen::Proposals);
    // From the Proposals, E goes back to the Editor, as the switch does.
    h.key(Modifiers::NONE, Key::E);
    assert_eq!(h.app.screen, Screen::Editor);
}

#[test]
fn a_letter_typed_into_the_describe_box_is_typed_and_switches_nothing() {
    for (key, _) in LETTERS {
        let mut h = Harness::new(App::sim());
        h.app.chrome.show(Pane::Claude);
        h.wait(0.3);
        h.click(h.rect("agent instruction").center());
        h.frame(vec![]);
        assert!(h.ctx.egui_wants_keyboard_input(), "the box has the keyboard");
        let before = state(&h);
        h.frame(vec![down(Modifiers::NONE, key), Event::Text("qzxj".into())]);
        h.frame(vec![]);
        assert_eq!(state(&h), before, "{key:?} typed in Describe switched something");
        assert!(h.drew("qzxj"), "{key:?}: the text is in the box");
    }
}

#[test]
fn a_letter_typed_into_the_code_editor_is_typed_and_switches_nothing() {
    for (key, _) in LETTERS {
        let mut h = Harness::code();
        h.wait(0.3);
        let editor = h.editor_id();
        h.ctx.memory_mut(|m| m.request_focus(editor));
        h.frame(vec![]);
        h.frame(vec![]);
        assert!(h.ctx.egui_wants_keyboard_input(), "the editor has the keyboard");
        let before = state(&h);
        let src = h.app.sim.as_ref().unwrap().session.src.clone();
        h.frame(vec![down(Modifiers::NONE, key), Event::Text("qzxj".into())]);
        h.frame(vec![]);
        assert_eq!(state(&h), before, "{key:?} typed in the code switched something");
        let now = h.app.sim.as_ref().unwrap().session.src.clone();
        assert!(
            now.contains("qzxj") && now.len() == src.len() + 4,
            "{key:?}: typed into the source"
        );
    }
}

#[test]
fn a_letter_typed_into_the_find_bar_is_typed_and_switches_nothing() {
    for (key, _) in LETTERS {
        let mut h = Harness::blink();
        h.press("pane code");
        h.wait(0.3);
        h.key(Modifiers::COMMAND, Key::F);
        h.frame(vec![]);
        assert!(h.app.chrome.find.is_some());
        let before = state(&h);
        h.frame(vec![down(Modifiers::NONE, key), Event::Text("zq".into())]);
        h.frame(vec![]);
        assert_eq!(
            state(&h),
            before,
            "{key:?} typed in the find bar switched something"
        );
        assert_eq!(
            h.app.chrome.find.as_ref().unwrap().query,
            "zq",
            "{key:?}: the find bar took the text"
        );
    }
}

#[test]
fn a_letter_with_ctrl_cmd_alt_or_shift_switches_nothing() {
    for m in [
        Modifiers::CTRL,
        Modifiers::COMMAND,
        Modifiers::MAC_CMD,
        Modifiers::ALT,
        Modifiers::SHIFT,
    ] {
        for (key, _) in LETTERS {
            // Ctrl F is Find and Ctrl S Save: chords of their own, which
            // do not touch the screen or the layout.
            let mut h = Harness::code();
            h.wait(0.1);
            let before = state(&h);
            h.key(m, key);
            h.wait(0.1);
            assert_eq!(state(&h), before, "{key:?} with {m:?} switched something");
        }
    }
}

#[test]
/// Nick, 2026-11-26: over the diagram only . and , are its own (they
/// jump along it); every view letter works there as anywhere else, the
/// old pan and jump keys W A S D E Q among them.
fn over_the_diagram_only_period_and_comma_stay_its_own_and_every_letter_works() {
    let mut h = Harness::new(App::sim());
    h.app.chrome.show(Pane::Code);
    h.app.chrome.trend = true;
    h.app.chrome.msgs = false;
    h.wait(0.3);
    let d = h.app.chrome.diagram;
    assert!(d.is_positive());
    h.frame(vec![Event::PointerMoved(d.center())]);
    h.frame(vec![]);
    let before = state(&h);
    let pick = |h: &Harness| super::sim_view(h).pick;
    assert_eq!(pick(&h), None);
    h.key(Modifiers::NONE, Key::Period);
    h.wait(0.5);
    assert!(pick(&h).is_some(), ". over the diagram jumps to a stop");
    h.key(Modifiers::NONE, Key::Comma);
    h.wait(0.5);
    assert!(pick(&h).is_some(), ", over the diagram jumps to a stop");
    assert_eq!(state(&h), before, ". and , switch no view");
    h.key(Modifiers::NONE, Key::D);
    assert!(h.app.chrome.showing(Pane::Claude), "D works over the diagram");
    h.key(Modifiers::NONE, Key::S);
    assert!(h.app.chrome.showing(Pane::Code), "S works over the diagram");
    h.key(Modifiers::NONE, Key::A);
    assert!(
        h.app.chrome.trend && h.app.chrome.msgs,
        "A works over the diagram"
    );
    h.key(Modifiers::NONE, Key::T);
    assert!(
        h.app.chrome.trend && !h.app.chrome.msgs,
        "T works over the diagram"
    );
    h.key(Modifiers::NONE, Key::F);
    assert!(h.app.chrome.showing(Pane::Files), "F works over the diagram");
    h.frame(vec![Event::PointerMoved(h.app.chrome.diagram.center())]);
    h.frame(vec![]);
    h.key(Modifiers::NONE, Key::E);
    assert_eq!(
        h.app.screen,
        Screen::Monitor,
        "E works over the diagram (on the Editor it flips to the Monitor)"
    );
}

#[test]
fn the_old_alt_shift_chords_still_switch() {
    let mut h = Harness::new(App::sim());
    h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::Num1);
    assert!(h.app.chrome.showing(Pane::Code));
    h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::Num0);
    assert!(h.app.chrome.showing(Pane::Files));
}

#[test]
fn the_letter_of_the_tab_already_showing_keeps_it_as_a_click_does() {
    for (key, pane) in [
        (Key::D, Pane::Claude),
        (Key::S, Pane::Code),
        (Key::I, Pane::Io),
        (Key::V, Pane::Devices),
        (Key::F, Pane::Files),
    ] {
        let mut h = Harness::new(App::sim());
        h.app.chrome.show(pane);
        h.wait(0.3);
        let before = state(&h);
        h.key(Modifiers::NONE, key);
        h.wait(0.2);
        assert!(h.app.chrome.showing(pane), "{key:?} closed {pane:?}");
        assert_eq!(
            state(&h),
            before,
            "{key:?} on the tab already showing changed something"
        );
    }
    // Messages and the Trend are the bottom panel's tabs: the letter of
    // the one showing shuts the panel, and brings it back on that tab.
    for (key, msgs) in [(Key::A, true), (Key::T, false)] {
        let mut h = Harness::new(App::sim());
        h.app.chrome.trend = true;
        h.app.chrome.msgs = msgs;
        h.wait(0.2);
        h.key(Modifiers::NONE, key);
        assert!(!h.app.chrome.trend, "{key:?} on its own tab shuts the panel");
        h.key(Modifiers::NONE, key);
        assert_eq!((h.app.chrome.trend, h.app.chrome.msgs), (true, msgs), "{key:?}");
    }
}
