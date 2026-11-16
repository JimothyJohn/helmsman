//! The box's pane layouts (`api::pane_layouts`): kept in the state
//! directory's `pane-layouts.json` across a restart, within their bounds,
//! and never anything but a layout's own fields. The routes, the engineer
//! gate and the audit are in tests/accounts.rs.

use helmsman_plc::api::pane_layouts::{FORMAT, Kept, Layout, MAX_BODY, MAX_LAYOUTS, MAX_NAME, Refused};
use serde_json::{Value as Json, json};

fn folder(what: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "helmsman-pane-layouts-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn body(name: &str) -> Json {
    json!({
        "name": name,
        "left": ["devices", "files", "claude", "code"],
        "right": ["io"],
        "hidden": ["history"],
        "left_shows": "devices",
        "right_shows": "io",
        "left_open": true,
        "right_open": true,
        "bottom_open": false,
        "bottom_shows": "messages",
        "left_width": 400.0,
    })
}

fn layout(name: &str) -> Layout {
    Layout::read(body(name).to_string().as_bytes()).unwrap()
}

fn names(k: &mut Kept) -> Vec<String> {
    k.list().into_iter().map(|l| l.name).collect()
}

#[test]
fn layouts_survive_a_restart_in_the_export_files_shape() {
    let file = folder("restart").join("state").join("pane-layouts.json");
    {
        let mut k = Kept::in_file(file.clone());
        assert_eq!(k.list(), []);
        k.set(layout("Commissioning line 3")).unwrap();
        k.set(layout("Wall screen")).unwrap();
        // The same name in another case replaces, in place.
        let mut again = layout("commissioning LINE 3");
        again.bottom_open = true;
        k.set(again).unwrap();
        assert_eq!(names(&mut k), ["commissioning LINE 3", "Wall screen"]);
    }
    let text = std::fs::read_to_string(&file).unwrap();
    let j: Json = serde_json::from_str(&text).unwrap();
    assert_eq!(j["format"], FORMAT);
    assert_eq!(j["layouts"][0]["left_width"], 400.0);
    assert!(
        j["layouts"][0].get("right_width").is_none(),
        "a size not given is not written: {text}"
    );
    let mut k = Kept::in_file(file.clone());
    assert_eq!(names(&mut k), ["commissioning LINE 3", "Wall screen"]);
    assert!(k.list()[0].bottom_open);
    assert_eq!(k.remove("WALL SCREEN"), Ok(true));
    assert_eq!(k.remove("Wall screen"), Ok(false));
    assert_eq!(names(&mut Kept::in_file(file)), ["commissioning LINE 3"]);
}

#[test]
fn a_file_that_is_not_the_list_reads_as_none_and_bad_entries_are_dropped() {
    let dir = folder("garbage");
    for (k, text) in [
        "",
        "not json",
        "[]",
        r#"{"format":"helmsman-pane-layouts 2","layouts":[]}"#,
        "{\"layouts\":7}",
    ]
    .into_iter()
    .enumerate()
    {
        let file = dir.join(format!("{k}.json"));
        std::fs::write(&file, text).unwrap();
        assert_eq!(Kept::in_file(file).list(), [], "{text:?}");
    }
    // A file someone edited: the entries that pass the rules, the first of
    // a name, and no more than the box keeps.
    let mut entries = vec![
        json!(7),
        json!({ "name": "" }),
        json!({ "name": "no bottom" }),
        body("Good"),
        body("good"),
        json!({ "name": "odd tab", "left": ["../../etc/passwd"], "bottom_shows": "trend" }),
    ];
    entries.extend((0..MAX_LAYOUTS + 4).map(|k| body(&format!("n{k}"))));
    let file = dir.join("edited.json");
    std::fs::write(&file, json!({ "format": FORMAT, "layouts": entries }).to_string()).unwrap();
    let mut k = Kept::in_file(file);
    let got = names(&mut k);
    assert_eq!(got.len(), MAX_LAYOUTS);
    assert_eq!(got[..2], ["Good", "n0"]);
}

#[test]
fn the_box_keeps_at_most_max_layouts_and_refuses_the_next() {
    let mut k = Kept::default();
    for n in 0..MAX_LAYOUTS {
        k.set(layout(&format!("n{n}"))).unwrap();
    }
    assert_eq!(k.set(layout("one more")), Err(Refused::Full));
    assert_eq!(k.list().len(), MAX_LAYOUTS);
    k.set(layout("N3"))
        .expect("a name the box has is replaced when full");
    assert_eq!(k.remove("n0"), Ok(true));
    k.set(layout("one more")).unwrap();
}

#[test]
fn a_folder_that_cannot_be_written_keeps_the_layout_in_memory_and_says_so() {
    let dir = folder("blocked");
    let blocker = dir.join("state");
    std::fs::write(&blocker, "a file where the folder should be").unwrap();
    let mut k = Kept::in_file(blocker.join("pane-layouts.json"));
    assert!(matches!(k.set(layout("A")), Err(Refused::NotKept(_))));
    assert_eq!(names(&mut k), ["A"]);
}

#[test]
fn only_a_layout_that_keeps_every_rule_is_read() {
    let ok = |j: &Json| Layout::read(j.to_string().as_bytes());
    assert!(ok(&body("A")).is_ok());
    // Only the layout's fields are kept.
    let mut extra = body("A");
    extra["password"] = json!("hunter2");
    extra["script"] = json!("<script>alert(1)</script>");
    let kept = serde_json::to_string(&ok(&extra).unwrap()).unwrap();
    assert!(!kept.contains("hunter2") && !kept.contains("script"), "{kept}");
    // Names.
    let long = "x".repeat(MAX_NAME + 1);
    for name in [
        "",
        " ",
        " a",
        "a ",
        "a\nb",
        "a\u{0}",
        "\u{1b}[31mred",
        long.as_str(),
    ] {
        assert!(ok(&body(name)).is_err(), "{name:?}");
    }
    assert!(
        ok(&body(&"é".repeat(MAX_NAME))).is_ok(),
        "the limit is characters"
    );
    assert!(
        ok(&body("<b>Line 3</b> & \"co\"")).is_ok(),
        "a name is text, kept as it is"
    );
    // Each field, broken one at a time.
    let broken: Vec<(&str, Json)> = vec![
        ("left", json!(["nope"])),
        ("left", json!(["../../etc/passwd"])),
        ("left", json!(["devices", "devices"])),
        ("right", json!(["devices"])),
        ("hidden", json!(["io"])),
        ("left", json!("devices")),
        ("left", json!([7])),
        ("left_shows", json!("io")),
        ("right_shows", json!("code")),
        ("right_shows", json!("")),
        ("bottom_shows", json!("cellar")),
        ("bottom_shows", json!(null)),
        ("left_open", json!("yes")),
        ("left_width", json!(1e30)),
        ("left_width", json!(-5)),
        ("right_width", json!(0)),
        ("bottom_height", json!("tall")),
        ("name", json!(7)),
    ];
    for (field, value) in broken {
        let mut j = body("A");
        j[field] = value.clone();
        assert!(ok(&j).is_err(), "{field} = {value}");
    }
    // An empty pane shows nothing, or names a tab the page falls back to.
    let mut j = body("A");
    j["right"] = json!([]);
    j["left"] = json!(["devices", "files", "claude", "code", "io"]);
    assert!(
        ok(&j).is_ok(),
        "an empty pane may name any tab: the page falls back"
    );
    j["right_shows"] = json!("");
    assert!(ok(&j).is_ok());
    j["right_shows"] = json!("nope");
    assert!(ok(&j).is_err());
    // Not an object, not JSON, too long.
    for bad in ["", "{", "[]", "null", "7", "\"A\""] {
        assert!(Layout::read(bad.as_bytes()).is_err(), "{bad:?}");
    }
    let mut big = body("A");
    big["pad"] = json!("x".repeat(MAX_BODY));
    assert!(ok(&big).unwrap_err().contains("at most"));
    let deep = format!("{}{}", "[".repeat(1500), "]".repeat(1500));
    assert!(Layout::read(deep.as_bytes()).is_err());
}
