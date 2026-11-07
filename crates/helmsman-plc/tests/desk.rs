//! The desk on the box (`api::desk`): kept per person in the state
//! directory's `desks/` folder across a restart, within its bounds, and
//! never anything but the desk's own fields.

use helmsman_plc::api::desk::{Desk, Desks, MAX_DESK, MAX_OWNERS, Refused};

fn folder(what: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "helmsman-desk-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn desk(saved_at: u64, agent: &str) -> Desk {
    Desk {
        saved_at,
        agent: agent.into(),
        ..Desk::default()
    }
}

#[test]
fn desks_survive_a_restart_one_file_a_person_named_in_hex() {
    let dir = folder("restart").join("desks");
    {
        let mut d = Desks::in_dir(dir.clone());
        assert_eq!(d.get("user:alice"), None);
        d.set("user:alice", desk(1, "a")).unwrap();
        d.set("role:operator", desk(2, "o")).unwrap();
        d.set("user:../../etc/passwd", desk(3, "x")).unwrap();
    }
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    for n in &names {
        let stem = n.strip_suffix(".json").unwrap();
        assert!(stem.bytes().all(|b| b.is_ascii_hexdigit()), "{n}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "only the box reads the desks: {mode:o}");
    }
    let mut d = Desks::in_dir(dir);
    assert_eq!(d.get("user:alice"), Some(desk(1, "a")));
    assert_eq!(d.get("role:operator"), Some(desk(2, "o")));
    assert_eq!(d.get("user:bob"), None);
}

#[test]
fn a_file_that_is_not_a_desk_reads_as_none() {
    let dir = folder("garbage");
    let mut d = Desks::in_dir(dir.clone());
    d.set("user:alice", desk(1, "a")).unwrap();
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    for bad in [&b"{not json"[..], b"[]", b"null", br#"{"agent":7}"#] {
        std::fs::write(&file, bad).unwrap();
        assert_eq!(Desks::in_dir(dir.clone()).get("user:alice"), None, "{bad:?}");
    }
    std::fs::write(&file, vec![b' '; MAX_DESK + 10]).unwrap();
    assert_eq!(Desks::in_dir(dir).get("user:alice"), None, "past the bound");
}

#[test]
fn an_older_desk_is_refused_and_the_newer_stands() {
    let mut d = Desks::default();
    d.set("user:alice", desk(10, "new")).unwrap();
    assert_eq!(d.set("user:alice", desk(9, "old")), Err(Refused::Older(10)));
    assert_eq!(d.get("user:alice"), Some(desk(10, "new")));
    d.set("user:alice", desk(10, "same time")).unwrap();
    assert_eq!(d.get("user:alice").unwrap().agent, "same time");
}

#[test]
fn the_box_keeps_at_most_max_owners_desks_and_refuses_the_next() {
    let dir = folder("owners");
    let mut d = Desks::in_dir(dir.clone());
    for k in 0..MAX_OWNERS {
        d.set(&format!("user:u{k}"), desk(1, "w")).unwrap();
    }
    assert_eq!(d.set("user:late", desk(1, "w")), Err(Refused::Full));
    assert_eq!(d.get("user:late"), None);
    // One already kept still updates; after a restart the count holds.
    d.set("user:u0", desk(2, "again")).unwrap();
    let mut d = Desks::in_dir(dir);
    assert_eq!(d.set("user:late", desk(1, "w")), Err(Refused::Full));
}

#[test]
fn only_the_desks_fields_are_read() {
    let d = Desk::read(br#"{"saved_at":3,"agent":"a","password":"hunter2","pin":"1234"}"#).unwrap();
    assert_eq!(d, desk(3, "a"));
    let text = serde_json::to_string(&d).unwrap();
    assert!(!text.contains("hunter2") && !text.contains("1234"), "{text}");
    for bad in [&b""[..], b"[]", b"7", b"\"s\"", br#"{"drafts":[1]}"#] {
        assert_eq!(Desk::read(bad), None, "{bad:?}");
    }
    assert_eq!(Desk::read(&vec![b' '; MAX_DESK + 1]), None);
}

/// Card `desk-agent-and-forms`: the Agent's conversation and the New
/// device form are desk fields, read into their own shape too, so a field
/// the page's desk does not have (a password beside the form's settings,
/// a token on a turn) is dropped there as well.
#[test]
fn the_conversation_and_the_device_form_are_kept_and_nothing_else_inside_them() {
    use helmsman_plc::api::desk::{Catalog, Dir, End, Item, OutcomeKind, Use};
    let body = br#"{"saved_at":4,
        "conversation":{"turns":[{"user":"blink","text":"Looking","token":"tok-1",
            "items":[{"tool":{"id":"t1","name":"read_file","result":[true,"ok"]}},
                     {"proposal":{"ids":[3],"summary":"speed"}},{"retrying":"busy"},
                     {"fallback":{"proposed":null,"kind":"param"}}],
            "end":{"failed":"the page was reloaded"}}],
          "outcome":{"kind":"failed","status":"Main.st is unchanged","why":null,
            "problems":[],"undo":null,"review":[3]},"interrupted":"main.st"},
        "device":{"name":"rio","protocol":1,"endpoint":"192.168.1.60",
          "params":"eds=e1213.eds rpi_ms=100","eds":"e1213.eds","password":"hunter2",
          "points":[{"on":true,"dir":"in","name":"di0","ty":"bool","point":"di:0",
            "use":{"existing":"start"},"fixed":true,"pin":"1234"}],
          "catalog":"e1213","editing":null}}"#;
    let d = Desk::read(body).expect("a desk");
    let c = d.conversation.as_ref().unwrap();
    assert_eq!(c.turns[0].items.len(), 4);
    assert!(matches!(&c.turns[0].items[1], Item::Proposal { ids, .. } if ids == &[3]));
    assert_eq!(c.turns[0].end, Some(End::Failed("the page was reloaded".into())));
    assert_eq!(c.outcome.as_ref().unwrap().kind, OutcomeKind::Failed);
    let f = d.device.as_ref().unwrap();
    assert_eq!(f.catalog, Some(Catalog::E1213));
    assert_eq!(f.points[0].dir, Dir::In);
    assert_eq!(f.points[0].use_, Use::Existing("start".into()));
    let text = serde_json::to_string(&d).unwrap();
    for secret in ["hunter2", "1234", "tok-1"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    assert_eq!(Desk::read(text.as_bytes()), Some(d), "written back as read");
    // A wrong shape inside them is no desk.
    for bad in [
        &br#"{"conversation":{"turns":[{"end":{"exploded":1}}]}}"#[..],
        br#"{"device":{"points":[{"dir":"sideways"}]}}"#,
    ] {
        assert_eq!(Desk::read(bad), None);
    }
}
