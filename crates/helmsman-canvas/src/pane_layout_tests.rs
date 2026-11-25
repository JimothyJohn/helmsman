//! The pane layouts' rules, apart from the screen: where a tab goes when
//! it is placed or hidden, what a file may hold, and what a hostile one
//! is held to.

use crate::editor::{Chrome, LEFT_W, Pane};
use crate::pane_layout::{
    FILE, FORMAT, Layouts, MAX_FILE, MAX_LAYOUTS, MAX_NAME, MAX_SIZE, MIN_SIZE, PaneLayout, Sizes,
    clean_name, parse, pick_refusal, presets,
};

/// Every tab exactly once across the two panes and the hidden ones.
fn whole(l: &PaneLayout) {
    let mut all: Vec<Pane> = l
        .lefts
        .iter()
        .chain(&l.rights)
        .chain(&l.hidden)
        .copied()
        .collect();
    assert_eq!(all.len(), Pane::ALL.len(), "{l:?}");
    for p in Pane::ALL {
        assert!(all.contains(&p), "{p:?} is nowhere in {l:?}");
    }
    all.dedup();
    assert!(l.lefts.is_empty() || l.lefts.contains(&l.pane), "{l:?}");
    assert!(l.rights.is_empty() || l.rights.contains(&l.side), "{l:?}");
}

#[test]
fn a_tab_placed_in_its_own_pane_moves_and_the_shown_one_stays() {
    use Pane::{Claude, Code, Devices, Files, Io};
    let mut c = Chrome::default();
    assert_eq!(c.lefts, [Claude, Code, Io, Devices, Files]);
    c.place(Files, false, Some(Claude));
    assert_eq!(c.lefts, [Files, Claude, Code, Io, Devices], "to the front");
    c.place(Files, false, None);
    assert_eq!(c.lefts, [Claude, Code, Io, Devices, Files], "to the end");
    c.place(Code, false, Some(Files));
    assert_eq!(c.lefts, [Claude, Io, Devices, Code, Files], "rightwards");
    c.place(Files, false, Some(Io));
    assert_eq!(c.lefts, [Claude, Files, Io, Devices, Code], "leftwards");
    // Onto itself, or before the tab it is already before: nothing moves.
    let was = c.lefts.clone();
    c.place(Io, false, Some(Io));
    c.place(Files, false, Some(Io));
    assert_eq!(c.lefts, was);
    assert_eq!(c.pane, Claude, "a reorder does not change the shown tab");
    assert!(c.rights.is_empty() && c.hidden.is_empty());
}

#[test]
fn a_tab_placed_in_the_other_pane_docks_at_that_place() {
    use Pane::{Claude, Code, Devices, Io};
    let mut c = Chrome::default();
    c.dock(Io, true);
    c.dock(Devices, true);
    c.place(Code, true, Some(Devices));
    assert_eq!(c.rights, [Io, Code, Devices]);
    assert_eq!((c.pane, c.side), (Claude, Code), "shown where it landed");
    // A tab the other pane lacks as the place: the end.
    c.place(Claude, true, Some(Pane::Files));
    assert_eq!(c.rights.last(), Some(&Claude));
    whole(&c.layout());
}

#[test]
fn a_hidden_tab_leaves_its_pane_and_comes_back_when_shown() {
    use Pane::{Claude, Code, Files};
    let mut c = Chrome::default();
    c.hide(Claude);
    assert_eq!(c.hidden, [Claude]);
    assert_eq!(c.pane, Code, "the tab that was under it shows");
    assert!(!c.showing(Claude));
    c.hide(Claude);
    assert_eq!(c.hidden, [Claude], "hidden once");
    // Placing or docking a hidden tab does nothing: it is not in a pane.
    c.dock(Claude, true);
    c.place(Claude, false, None);
    assert!(c.rights.is_empty() && !c.lefts.contains(&Claude));
    // Anything that opens the tab brings it back, at the end.
    c.show(Claude);
    assert!(c.hidden.is_empty() && c.showing(Claude));
    assert_eq!(c.lefts.last(), Some(&Claude));
    // The last tab of a pane hidden: the one before shows.
    c.pane = Claude;
    c.hide(Claude);
    assert_eq!(c.pane, Files);
    // Every tab hidden: nothing to show, and a toggle still brings one back.
    for p in Pane::ALL {
        c.hide(p);
    }
    assert!(c.lefts.is_empty() && c.rights.is_empty());
    whole(&c.layout());
    c.toggle_code();
    assert!(c.code_open());
}

#[test]
fn the_browser_keeps_hidden_tabs_and_a_text_from_before_reads_as_none() {
    let mut c = Chrome::default();
    c.dock(Pane::Io, true);
    c.hide(Pane::Code);
    c.hide(Pane::Files);
    c.place(Pane::Devices, false, Some(Pane::Claude));
    let back = Chrome::restored(&c.saved());
    assert_eq!(back.layout(), c.layout());
    // Without the word (every text saved before today): nothing hidden.
    let old = Chrome::default().saved();
    assert!(!old.contains("hidden"), "{old}");
    assert_eq!(Chrome::restored(&old).hidden, []);
    // A tab named twice keeps its first place; hostile words are dropped.
    let odd = Chrome::restored("dock=2 lefts=code,code,nope hidden=code,io,../etc pane=io");
    assert_eq!(odd.hidden, [Pane::Io]);
    assert_eq!(odd.lefts.iter().filter(|p| **p == Pane::Code).count(), 1);
    whole(&odd.layout());
}

#[test]
fn applying_a_layout_arranges_the_panes_and_asks_for_its_sizes() {
    let mut c = Chrome::default();
    c.light = true;
    c.wrap = false;
    let p = presets();
    let programming = &p.iter().find(|p| p.name == "Programming").unwrap().layout;
    c.apply(programming);
    assert_eq!(c.layout().lefts, programming.lefts);
    assert_eq!(c.side, Pane::Code);
    assert!(c.msgs, "the Messages stay as they were");
    assert_eq!(
        c.resize,
        Some(Sizes {
            left: Some(LEFT_W),
            right: Some(420.0),
            bottom: Some(crate::editor::BOTTOM_H)
        }),
        "a size the layout does not give is the pane's first one"
    );
    assert!(c.light && !c.wrap, "preferences are not a layout's");
}

#[test]
fn every_ready_made_layout_is_whole_and_default_is_the_first_page() {
    let p = presets();
    assert_eq!(
        p.iter().map(|p| p.name).collect::<Vec<_>>(),
        ["Default", "Programming", "Commissioning", "Monitoring"]
    );
    for preset in &p {
        whole(&preset.layout);
        assert!(preset.layout.hidden.is_empty(), "{}", preset.name);
        assert_ne!(preset.tip, "");
    }
    assert_eq!(p[0].layout, Chrome::default().layout());
}

#[test]
fn layouts_go_to_a_file_and_come_back_the_same() {
    let mut c = Chrome::default();
    c.dock(Pane::Io, true);
    c.hide(Pane::Files);
    c.left = false;
    c.msgs = false;
    c.sizes = Sizes {
        left: Some(400.0),
        right: Some(300.0),
        bottom: None,
    };
    let mut mine = Layouts::default();
    assert_eq!(
        mine.save("  Line 3 start-up ", c.layout()).unwrap(),
        "Line 3 start-up"
    );
    mine.save("Wide code", presets()[1].layout.clone()).unwrap();
    let file = mine.export();
    let mut other = Layouts::default();
    assert_eq!(other.import(FILE, &file), Ok(2));
    assert_eq!(other.mine, mine.mine);
    assert_eq!(
        other.get("line 3 START-UP"),
        Some(&c.layout()),
        "names match in any case"
    );
    // What the browser stores is the same text.
    assert_eq!(Layouts::restored(&mine.saved()).mine, mine.mine);
    assert_eq!(Layouts::restored("not json").mine, []);
    assert_eq!(Layouts::restored("").mine, []);
}

#[test]
fn saving_holds_names_and_the_count_to_their_rules() {
    let l = PaneLayout::default();
    let mut mine = Layouts::default();
    assert!(mine.save("   ", l.clone()).is_err(), "no name");
    assert!(
        mine.save("\u{7}\n", l.clone()).is_err(),
        "control characters only"
    );
    assert!(
        mine.save("default", l.clone())
            .unwrap_err()
            .contains("ready-made")
    );
    assert!(mine.save("MONITORING", l.clone()).is_err());
    let long = "x".repeat(200);
    assert_eq!(mine.save(&long, l.clone()).unwrap().chars().count(), MAX_NAME);
    assert_eq!(clean_name("a\u{0}b\tc\r\n"), "abc");
    // The same name again replaces, in place.
    mine.save("A", l.clone()).unwrap();
    let mut other = l.clone();
    other.left = false;
    mine.save("a", other.clone()).unwrap();
    assert_eq!(mine.mine.len(), 2);
    assert_eq!(mine.get("A"), Some(&other));
    for k in mine.mine.len()..MAX_LAYOUTS {
        mine.save(&format!("n{k}"), l.clone()).unwrap();
    }
    assert!(
        mine.save("one too many", l.clone())
            .unwrap_err()
            .contains("delete one")
    );
    assert!(mine.save("A", l).is_ok(), "replacing still works when full");
    assert!(mine.remove("a") && !mine.remove("a"));
    assert_eq!(mine.mine.len(), MAX_LAYOUTS - 1);
}

#[test]
fn a_hostile_file_is_refused_or_held_to_the_rules() {
    let mut mine = Layouts::default();
    let refused = |mine: &mut Layouts, bytes: &[u8]| {
        let before = mine.mine.clone();
        let why = mine.import("evil.json", bytes).unwrap_err();
        assert_eq!(mine.mine, before, "a refused file changes nothing");
        why
    };
    assert!(refused(&mut mine, b"").contains("not a pane layout file"));
    assert!(refused(&mut mine, &[0xff, 0xfe, 0x00]).contains("not text"));
    assert!(refused(&mut mine, b"[1,2,3]").contains("not a pane layout file"));
    assert!(refused(&mut mine, br#"{"format":"helmsman-layout 9","layouts":[]}"#).contains("not a pane"));
    assert!(
        refused(&mut mine, br#"{"format":"helmsman-pane-layouts 2","layouts":[]}"#).contains("newer version")
    );
    assert!(refused(&mut mine, format!(r#"{{"format":"{FORMAT}"}}"#).as_bytes()).contains("no layouts"));
    assert!(
        refused(
            &mut mine,
            format!(r#"{{"format":"{FORMAT}","layouts":[]}}"#).as_bytes()
        )
        .contains("no layouts")
    );
    // Deeply nested, inside the size limit: refused, not a stack overflow.
    let deep = format!("{}{}", "[".repeat(30_000), "]".repeat(30_000));
    assert!(refused(&mut mine, deep.as_bytes()).contains("not a pane layout file"));
    let big = format!(
        r#"{{"format":"{FORMAT}","layouts":[],"pad":"{}"}}"#,
        "x".repeat(MAX_FILE)
    );
    assert!(refused(&mut mine, big.as_bytes()).contains("at most"));
    assert!(pick_refusal("big.json", 1e9).unwrap().contains("at most"));
    assert!(pick_refusal("fine.json", 2000.0).is_none());
    // A file's own name is shown cleaned.
    let why = mine.import("a\nb\u{1b}[31m.json", b"x").unwrap_err();
    assert!(why.starts_with("ab[31m.json was not read"), "{why}");

    // Entries of the wrong shape are skipped; the rest are held to the rules.
    let file = format!(
        r#"{{"format":"{FORMAT}","layouts":[
            7, null, {{}}, {{"name":""}}, {{"name":17}},
            {{"name":"Odd","left":["code","code","../../etc/passwd",5,"io"],"right":"io",
              "hidden":["io","devices"],"left_shows":"nope","right_shows":"files",
              "left_open":"yes","bottom_shows":"cellar","messages_open":"yes",
              "left_width":1e30,"right_width":-5,"bottom_height":"tall"}},
            {{"name":"Default","left":["files"]}},
            {{"name":"  Odd  ","left":["files"]}},
            {{"name":"NaN","left_width":null}}
        ]}}"#
    );
    assert_eq!(mine.import("mixed.json", file.as_bytes()), Ok(4));
    let names: Vec<&str> = mine.mine.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["Odd", "Default (imported)", "NaN"],
        "the second Odd replaced the first"
    );
    for (_, l) in &mine.mine {
        whole(l);
    }
    // The first Odd, read alone: what each odd field came to.
    let odd = &parse(&file).unwrap()[0].1;
    assert_eq!(
        odd.lefts[..2],
        [Pane::Code, Pane::Io],
        "a tab once, unknown ones dropped"
    );
    assert_eq!(odd.hidden, [Pane::Devices], "io kept its first place");
    assert!(odd.rights.is_empty(), "a list that is not a list is empty");
    assert_eq!(odd.pane, Pane::Claude, "an unknown shown tab: the first page's");
    assert!(odd.left, "a flag that is not a flag keeps the first page's value");
    assert!(
        odd.msgs,
        "a flag that is not a flag: Messages open, as the first page has them"
    );
    assert_eq!(
        odd.sizes,
        Sizes {
            left: Some(MAX_SIZE),
            right: Some(MIN_SIZE),
            bottom: None
        }
    );

    // More than a browser keeps: the first that fit come in, and it says so.
    let many: Vec<String> = (0..MAX_LAYOUTS + 5)
        .map(|k| format!(r#"{{"name":"m{k}"}}"#))
        .collect();
    let file = format!(r#"{{"format":"{FORMAT}","layouts":[{}]}}"#, many.join(","));
    let took = mine.import("many.json", file.as_bytes()).unwrap();
    assert_eq!(took, MAX_LAYOUTS - 3);
    assert_eq!(mine.mine.len(), MAX_LAYOUTS);
    let (note, bad) = mine.note.clone().unwrap();
    assert!(bad && note.contains("did not fit"), "{note}");
    let why = mine.import(
        "many.json",
        br#"{"format":"helmsman-pane-layouts 1","layouts":[{"name":"new"}]}"#,
    );
    assert!(why.unwrap_err().contains("Delete one first"));
}

/// A layout file written when the bottom panel had tabs (`bottom_shows`)
/// still reads: its panes come back as they were and the Messages are
/// open, as the first page has them.
#[test]
fn a_layout_file_from_the_tabbed_bottom_panel_still_reads() {
    let file = format!(
        r#"{{"format":"{FORMAT}","layouts":[
            {{"name":"Old","left":["files","claude"],"right":["code","io","devices","history"],
              "left_shows":"files","right_shows":"code","left_open":true,"right_open":true,
              "bottom_open":false,"bottom_shows":"messages","left_width":300,"bottom_height":210}}
        ]}}"#
    );
    let (name, l) = &parse(&file).unwrap()[0];
    assert_eq!(name, "Old");
    assert_eq!(l.lefts[..2], [Pane::Files, Pane::Claude]);
    assert_eq!(l.pane, Pane::Files);
    assert_eq!(l.side, Pane::Code);
    assert!(l.right && !l.trend && l.msgs);
    assert_eq!((l.sizes.left, l.sizes.bottom), (Some(300.0), Some(210.0)));
    // And one that keeps the Messages folded keeps them so.
    let mut c = Chrome::default();
    c.msgs = false;
    let json = c.layout().to_json("Folded").to_string();
    let file = format!(r#"{{"format":"{FORMAT}","layouts":[{json}]}}"#);
    assert!(!parse(&file).unwrap()[0].1.msgs);
}
