// Archived 2026-11-08 (Nick: the Requirements tab is out of the UI for now; the
// tests the Agent writes with each change take its place). Not compiled; see
// README.md in this directory.

// From crates/helmsman-canvas/src/box_projects_tests.rs (uses its Bench, page,
// make, settle and unchunk helpers).

/// Nick, 2026-11-05: a requirement typed on a box goes to the box's
/// requirements route; with no relay the box writes it as "no test yet",
/// and the list shows it grey, never refusing a deploy.
#[test]
fn a_requirement_added_on_a_box_without_an_agent_is_listed_with_no_test_yet() {
    use crate::requirements::{Adding, Phase, State};
    let b = Bench::build(true);
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS), ("io.map", MAP)],
    );
    settle(&b, &jar, &mut v.projects);

    let (mut a, req) = Adding::to_box("line", "The conveyor runs after start", "conveyor_runs", 1);
    assert_eq!(req.url, "/api/projects/line/requirements");
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: box\r\nX-Helmsman-Request: 1\r\nOrigin: http://box\r\nCookie: helmsman_session={}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        req.url,
        jar.get().unwrap(),
        req.body.len()
    );
    let (status, _, body) = b.raw(&head, req.body.as_bytes());
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    a.chunk(&unchunk(&body));
    a.end(None);
    assert_eq!(
        a.phase,
        Phase::Untested {
            file: "test_conveyor_runs.st".into(),
            why: String::new()
        }
    );

    // The page reads the project again, which runs its tests.
    v.projects.open_project("line");
    settle(&b, &jar, &mut v.projects);
    v.projects.run_tests();
    settle(&b, &jar, &mut v.projects);
    let row = v
        .projects
        .tests
        .rows
        .iter()
        .find(|r| r.file == "test_conveyor_runs.st")
        .expect("its row");
    assert_eq!(row.state, State::Untested, "{:?}", v.projects.tests);
    assert_eq!(row.requirement, "The conveyor runs after start");
    assert_eq!(v.projects.tests.refusal(), None);

    // Signed out, the box refuses it.
    let (mut a, req) = Adding::to_box("line", "The belt stops on stop", "belt_stops", 2);
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: box\r\nX-Helmsman-Request: 1\r\nOrigin: http://box\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        req.url,
        req.body.len()
    );
    let (status, _, body) = b.raw(&head, req.body.as_bytes());
    assert_eq!(status, 401);
    a.refused(status, &String::from_utf8_lossy(&body));
    assert!(
        matches!(&a.phase, Phase::Failed(w) if w.starts_with("Sign in")),
        "{:?}",
        a.phase
    );
}

/// Nick, 2026-11-07: inputs must not disappear on a state change. A
/// requirement the box finished adding reads the project again for the
/// new test file; that read used to replace every buffer, so an unsaved
/// edit to another file was lost. Now the edit stays and the new file
/// comes in beside it.
#[test]
fn a_finished_requirement_keeps_unsaved_edits() {
    use crate::box_agent::Incoming;
    use crate::requirements::Adding;
    let b = Bench::build(true);
    let jar = Jar::default();
    let mut v = page(&b, &jar, Some(("alice", ALICE)));
    make(
        &b,
        &jar,
        &mut v.projects,
        "line",
        &[("main.st", MAIN), ("blocks.st", BLOCKS), ("io.map", MAP)],
    );
    settle(&b, &jar, &mut v.projects);
    let edited = format!("{MAIN}\n(* not saved yet *)\n");
    v.projects
        .open
        .as_mut()
        .unwrap()
        .bufs
        .get_mut("main.st")
        .unwrap()
        .text = edited.clone();

    // The box adds the test file, as the requirement's POST would.
    let (a, req) = Adding::to_box("line", "The conveyor runs after start", "conveyor_runs", 7);
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: box\r\nX-Helmsman-Request: 1\r\nOrigin: http://box\r\nCookie: helmsman_session={}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        req.url,
        jar.get().unwrap(),
        req.body.len()
    );
    let (status, _, body) = b.raw(&head, req.body.as_bytes());
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    v.adding = Some(a);
    v.requirement_answer(Incoming::Chunk {
        seq: 7,
        text: unchunk(&body),
    });
    v.requirement_answer(Incoming::End { seq: 7 });
    settle(&b, &jar, &mut v.projects);

    let o = v.projects.open.as_ref().unwrap();
    assert_eq!(o.bufs["main.st"].text, edited, "the unsaved edit stays");
    assert!(o.bufs["main.st"].dirty());
    assert!(
        o.bufs.contains_key("test_conveyor_runs.st"),
        "{:?}",
        o.bufs.keys()
    );
}

// ---- the helper these used (box_agent_tests.rs keeps its own copy) ----
/// Undoes HTTP chunked framing (a streamed answer).
fn unchunk(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let mut rest: &str = &s;
    let mut out = String::new();
    while let Some((size, after)) = rest.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if n == 0 || after.len() < n {
            break;
        }
        out.push_str(&after[..n]);
        rest = after[n..].strip_prefix("\r\n").unwrap_or(&after[n..]);
    }
    if out.is_empty() { s.into_owned() } else { out }
}
