//! The node contract: what `helmsman-nodecore` renders is read by
//! [`crate::remote`] exactly as a box's API is, so a node on the LAN is a
//! box to this canvas. If a field name drifts on either side, this is
//! where it shows.

use helmsman_nodecore::api::{
    self, DeviceView, ModuleView, NodeView, SignalValue, SignalView, Spread, State, Timing,
};
use helmsman_nodecore::settings::Settings;

use crate::remote::{BoxState, Fetch, Msg, Stream};

/// Card canvas-node-token: the header the canvas sends for the token the
/// person typed is the one a node's router takes for a write, and a
/// write without it is refused.
#[test]
fn the_canvas_token_header_opens_a_nodes_writes() {
    use helmsman_nodecore::http::parse;
    use helmsman_nodecore::router::{Effect, Outcome, handle};
    let v = view();
    let settings = Settings {
        token: "bench-token-2026".into(),
        ..Settings::default()
    };
    let body = "PROGRAM p END_PROGRAM";
    let request = |auth: &str| {
        let raw = format!(
            "PUT /api/modules/p HTTP/1.1\r\nHost: node\r\n{auth}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        match parse(raw.as_bytes()).expect("a whole request") {
            helmsman_nodecore::http::Read::Done(r, _) => r,
            helmsman_nodecore::http::Read::Incomplete => panic!("incomplete"),
        }
    };
    let (name, value) = crate::node_token::header(&crate::node_token::clean(" bench-token-2026 ").unwrap());
    let with = request(&format!("{name}: {value}\r\n"));
    assert!(
        matches!(
            handle(&with, &v, &settings),
            Outcome::Effect(Effect::Deploy { .. })
        ),
        "the node takes the canvas's header as its token"
    );
    let without = request("");
    let Outcome::Reply(r) = handle(&without, &v, &settings) else {
        panic!("a write without the token is refused")
    };
    assert_eq!(r.status, 401);
    let wrong = request("Authorization: Bearer bench-token-2025\r\n");
    let Outcome::Reply(r) = handle(&wrong, &v, &settings) else {
        panic!("a wrong token is refused")
    };
    assert_eq!(r.status, 401);
}

pub(crate) fn view() -> NodeView {
    NodeView {
        module: Some(ModuleView {
            name: "chase".into(),
            scan_ms: 10.0,
            state: State::Running,
            scans: 500,
            overruns: 1,
            last_overrun_ms_ago: Some(2500),
            timing: Some(Timing {
                window: 500,
                exec: Spread {
                    min: 90.0,
                    mean: 120.0,
                    max: 300.0,
                    p99: 250.0,
                },
                late: Spread {
                    min: 0.0,
                    mean: 5.0,
                    max: 40.0,
                    p99: 30.0,
                },
            }),
            source: "PROGRAM chase\nVAR x : BOOL; END_VAR\nEND_PROGRAM\n".into(),
        }),
        signals: vec![
            SignalView {
                name: "di0".into(),
                dir: "in",
                value: Some(SignalValue::Bool(true)),
                quality: "good",
                point_error: None,
            },
            SignalView {
                name: "counter".into(),
                dir: "in",
                value: Some(SignalValue::Int(42)),
                quality: "stale",
                point_error: Some("link down".into()),
            },
        ],
        devices: vec![DeviceView {
            name: "rio".into(),
            state: "down",
            error: Some("connect timed out".into()),
            address: Some("192.168.3.60:502".into()),
        }],
        seq: 7,
        at_ms: 70_000,
        uptime_ms: 90_000,
        heap_used: 1,
        heap_peak: 2,
        heap_size: 3,
        heap_external: None,
        mac: [1, 2, 3, 4, 5, 6],
        ip: Some([10, 0, 0, 50]),
        link_up: true,
        version: helmsman_nodecore::VERSION,
        board: "test",
        io_map: "device rio modbus-tcp 192.168.3.60\n".into(),
        one_block: false,
        check: None,
        firmware: None,
    }
}

#[test]
fn the_canvas_reads_a_node_as_it_reads_a_box() {
    let v = view();
    let mut s = BoxState::new();
    s.apply(Msg::Got(Fetch::Modules, Ok(api::modules(&v).to_string())), 1.0);
    assert_eq!(s.modules.len(), 1);
    let m = &s.modules[0];
    assert_eq!(m.name, "chase");
    assert_eq!(m.state, "running");
    assert_eq!(m.scans, 500);
    assert_eq!(m.scan_ms, Some(10.0));
    assert_eq!(m.overruns, 1);
    assert_eq!(m.overrun_ago, Some(2.5));
    let t = m.timing.expect("timing parsed");
    assert_eq!(t.window, 500);
    assert_eq!(t.exec.p99, 250.0);
    assert_eq!(t.late.max, 40.0);

    s.apply(Msg::Opened(Stream::Values), 1.1);
    s.apply(
        Msg::Event(Stream::Values, "values".into(), api::values_event(&v).to_string()),
        1.2,
    );
    assert_eq!(s.seq, Some(7));
    assert_eq!(s.values_at_ms, Some(70_000));
    assert_eq!(s.signals.len(), 2);
    assert_eq!(s.signals[0].name, "di0");
    assert_eq!(s.signals[0].dir, "in");
    assert_eq!(s.signals[0].quality, "good");
    assert_eq!(s.signals[1].quality, "stale");
    assert_eq!(s.signals[1].point_error.as_deref(), Some("link down"));
    assert_eq!(s.modules[0].name, "chase", "the event's modules replace the list");

    s.apply(Msg::Got(Fetch::Devices, Ok(api::devices(&v).to_string())), 1.3);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].state, "down");
    assert_eq!(s.devices[0].error.as_deref(), Some("connect timed out"));
    assert_eq!(s.devices[0].address.as_deref(), Some("192.168.3.60:502"));

    s.apply(Msg::Got(Fetch::IoMap, Ok(api::io_map(&v).to_string())), 1.4);
    assert_eq!(s.io_map.as_deref(), Some("device rio modbus-tcp 192.168.3.60\n"));

    let detail = api::module_detail(v.module.as_ref().unwrap());
    s.apply(
        Msg::Got(Fetch::Source("chase".into()), Ok(detail.to_string())),
        1.5,
    );
    // The Monitor's source fetch lands in the watched programs; a node
    // has no traces, so it stays source-only. Parsing it must not fail.
    assert!(s.problem.is_none());
}

#[test]
fn a_faulted_node_module_shows_its_fault() {
    let mut v = view();
    v.module.as_mut().unwrap().state = State::Faulted("division by zero".into());
    let mut s = BoxState::new();
    s.apply(Msg::Got(Fetch::Modules, Ok(api::modules(&v).to_string())), 1.0);
    assert_eq!(s.modules[0].state, "faulted");
    assert_eq!(s.modules[0].fault.as_deref(), Some("division by zero"));
}

#[test]
fn the_beacon_parses_as_the_box_reads_beacons() {
    // The box's parser is in helmsman-plc; its contract test (tests/site.rs)
    // holds the format. Here: six lines, the names in order.
    let settings = Settings {
        name: "press-4".into(),
        site: "line-a".into(),
        ..Settings::default()
    };
    let b = api::beacon(&view(), &settings);
    let keys: Vec<&str> = b.lines().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(keys, ["helmsman-site", "site", "id", "name", "api", "version"]);
    assert!(b.len() <= 512);
}

/// A node that drives several devices lists each with its own state and
/// its points with their own types: the canvas reads that as it reads a
/// box's devices and signals (card node-multi-device).
#[test]
fn several_devices_in_mixed_states_and_numeric_points_read_as_a_boxs_do() {
    use helmsman_iec::Value;
    let mut v = view();
    v.io_map = helmsman_nodecore::iomap::IoMap::factory([192, 168, 3, 60], "rio").text;
    v.devices = vec![
        DeviceView {
            name: "rio".into(),
            state: "up",
            error: None,
            address: Some("192.168.3.60:502".into()),
        },
        DeviceView {
            name: "drive".into(),
            state: "down",
            error: Some("refused: 0x0106 ownership conflict".into()),
            address: Some("192.168.33.1:44818".into()),
        },
        DeviceView {
            name: "press".into(),
            state: "unknown",
            error: None,
            address: Some("192.168.3.71:502".into()),
        },
    ];
    v.signals = vec![
        SignalView {
            name: "estop".into(),
            dir: "in",
            value: Some(SignalValue::Bool(true)),
            quality: "good",
            point_error: None,
        },
        SignalView {
            name: "pos".into(),
            dir: "in",
            value: Some(SignalValue::Int(-70_000)),
            quality: "stale",
            point_error: Some("refused: 0x0106 ownership conflict".into()),
        },
        SignalView {
            name: "level".into(),
            dir: "in",
            value: Some(SignalValue::Real(12.25)),
            quality: "good",
            point_error: None,
        },
        SignalView {
            name: "whole".into(),
            dir: "out",
            value: Some(SignalValue::Real(3.0)),
            quality: "good",
            point_error: None,
        },
        SignalView {
            name: "nan".into(),
            dir: "in",
            value: Some(SignalValue::Real(f64::NAN)),
            quality: "good",
            point_error: None,
        },
    ];
    let mut s = BoxState::new();
    s.apply(Msg::Got(Fetch::Devices, Ok(api::devices(&v).to_string())), 1.0);
    let seen: Vec<(&str, &str, Option<&str>)> = s
        .devices
        .iter()
        .map(|d| (d.name.as_str(), d.state.as_str(), d.error.as_deref()))
        .collect();
    assert_eq!(
        seen,
        [
            ("rio", "up", None),
            ("drive", "down", Some("refused: 0x0106 ownership conflict")),
            ("press", "unknown", None)
        ]
    );
    assert_eq!(s.devices[1].address.as_deref(), Some("192.168.33.1:44818"));

    s.apply(Msg::Opened(Stream::Values), 1.1);
    s.apply(
        Msg::Event(Stream::Values, "values".into(), api::values_event(&v).to_string()),
        1.2,
    );
    assert_eq!(s.signals.len(), 5);
    assert_eq!(s.signals[0].value, Some(Value::Bool(true)));
    assert_eq!(s.signals[1].value, Some(Value::Int(-70_000)));
    assert_eq!(s.signals[1].quality, "stale");
    assert_eq!(s.signals[2].value, Some(Value::Real(12.25)));
    assert_eq!(
        s.signals[3].value,
        Some(Value::Real(3.0)),
        "a whole real stays a real"
    );
    assert_eq!(
        s.signals[4].value, None,
        "a real that is not a number is no value"
    );

    // The map reply carries the points' addresses beside the text; a
    // reader that does not know the field takes the text as before.
    let reply = api::io_map(&v);
    assert_eq!(reply["image"][0]["address"], "%IX0.0");
    s.apply(Msg::Got(Fetch::IoMap, Ok(reply.to_string())), 1.3);
    assert_eq!(s.io_map.as_deref(), Some(v.io_map.as_str()));
    assert!(s.problem.is_none());
}

/// What the page sends for an edited module, as the node's router reads it.
fn routed(q: &crate::box_projects::PReq, token: &str) -> helmsman_nodecore::router::Outcome {
    use helmsman_nodecore::http::{Read, parse};
    let settings = Settings {
        token: "bench-token-2026".into(),
        ..Settings::default()
    };
    let body = q.body.clone().unwrap_or_default();
    let (name, value) = crate::node_token::header(&crate::node_token::clean(token).unwrap());
    let mut raw = format!(
        "{} {} HTTP/1.1\r\nHost: node\r\n{name}: {value}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
        q.method,
        q.url,
        q.ctype,
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(&body);
    match parse(&raw).expect("a whole request") {
        Read::Done(r, _) => {
            // A P4 Skipper: it checks a map's devices before saving it.
            let mut v = view();
            v.check = Some(helmsman_nodecore::check::Check::default());
            helmsman_nodecore::router::handle(&r, &v, &settings)
        }
        Read::Incomplete => panic!("incomplete"),
    }
}

const OLD: &str = "PROGRAM drive\nVAR slew : DINT; END_VAR\nslew := 0;\nEND_PROGRAM\n";
const NEW: &str = "PROGRAM drive\nVAR slew : DINT; END_VAR\nslew := 5000;\nEND_PROGRAM\n";

/// Nick 2026-11-24 ("It's running but I can't make any changes"): a module
/// edited on a Skipper's page reaches the Skipper's own router as exactly
/// that program, under the token, and a 201 makes it what runs here too.
#[test]
fn an_edited_module_reaches_the_skippers_router_and_becomes_what_runs() {
    use crate::box_projects::{Answer, BoxProjects, Deploy, PCall};
    use helmsman_nodecore::router::{Effect, Outcome, deployed};
    let mut p = BoxProjects::default();
    p.available = Some(false);
    p.track_module("drive", OLD);
    p.module.as_mut().unwrap().text = NEW.into();
    assert!(p.module.as_ref().unwrap().dirty());
    p.deploy_module();
    assert_eq!(p.deploy, Deploy::Deploying);
    let reqs = p.poll();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    let q = &reqs[0];
    assert_eq!((q.method, q.url.as_str()), ("PUT", "/api/modules/drive"));
    match routed(q, "bench-token-2026") {
        Outcome::Effect(Effect::Deploy { name, source }) => {
            assert_eq!(name, "drive");
            assert_eq!(source, NEW, "the node compiles the edited text, byte for byte");
        }
        other => panic!("not a deploy: {other:?}"),
    }
    let Outcome::Reply(r) = routed(q, "bench-token-2025") else {
        panic!("a wrong token is refused")
    };
    assert_eq!(r.status, 401);
    // Pressing again while one is on its way sends nothing more.
    p.deploy_module();
    assert_eq!(p.poll().len(), 0);

    let ok = deployed("drive", NEW.len());
    p.answer(
        PCall::DeployModule("drive".into()),
        &Answer {
            status: ok.status,
            retry_after: None,
            body: String::from_utf8(ok.body.clone()).unwrap(),
        },
    );
    assert!(matches!(p.deploy, Deploy::Done(_)), "{:?}", p.deploy);
    assert!(p.deployed, "the module list is read again");
    let m = p.module.as_ref().unwrap();
    assert!(!m.dirty(), "what was deployed is what runs");
    // The node has no traces, so the page may re-read the old source it
    // already had: that never puts the old program back in the editor.
    p.track_module("drive", OLD);
    assert_eq!(p.module.as_ref().unwrap().text, NEW);
}

/// A program the Skipper will not compile is refused with its lines, the
/// old one keeps running, and the edits stay in the editor to fix.
#[test]
fn a_refused_module_keeps_the_edits_and_says_why() {
    use crate::box_projects::{Answer, BoxProjects, Deploy, PCall};
    use helmsman_nodecore::router::deploy_refused;
    let mut p = BoxProjects::default();
    p.available = Some(false);
    p.track_module("drive", OLD);
    p.module.as_mut().unwrap().text = "PROGRAM drive\nslew := ;\nEND_PROGRAM\n".into();
    p.deploy_module();
    let _ = p.poll();
    let no = deploy_refused(&["2:9: expected an expression".into()]);
    p.answer(
        PCall::DeployModule("drive".into()),
        &Answer {
            status: no.status,
            retry_after: None,
            body: String::from_utf8(no.body.clone()).unwrap(),
        },
    );
    let Deploy::Failed(why) = &p.deploy else {
        panic!("{:?}", p.deploy)
    };
    assert!(why.contains("old version keeps running"), "{why}");
    assert!(why.contains("2:9: expected an expression"), "{why}");
    let m = p.module.as_ref().unwrap();
    assert!(m.dirty(), "the edits stay");
    assert_eq!(m.base, OLD);
    // An empty program is not sent at all.
    p.module.as_mut().unwrap().text = "  \n".into();
    p.deploy = Deploy::Idle;
    p.deploy_module();
    assert_eq!(p.poll().len(), 0);
    assert!(matches!(p.deploy, Deploy::Failed(_)));
}

/// Edits survive a change on the box; another module replaces them.
#[test]
fn the_module_editor_follows_the_box_without_losing_edits() {
    use crate::box_projects::BoxProjects;
    let mut p = BoxProjects::default();
    p.track_module("drive", OLD);
    // Unedited: a new source on the box shows.
    p.track_module("drive", NEW);
    assert_eq!(p.module.as_ref().unwrap().text, NEW);
    // Edited: a new source on the box does not overwrite the edits.
    p.module.as_mut().unwrap().text = "PROGRAM drive\nEND_PROGRAM\n".into();
    p.track_module("drive", OLD);
    let m = p.module.as_ref().unwrap();
    assert_eq!(m.text, "PROGRAM drive\nEND_PROGRAM\n");
    assert_eq!(m.base, OLD);
    // Another module is another file.
    p.track_module("chase", "PROGRAM chase\nEND_PROGRAM\n");
    let m = p.module.as_ref().unwrap();
    assert_eq!(
        (m.name.as_str(), m.text.as_str()),
        ("chase", "PROGRAM chase\nEND_PROGRAM\n")
    );
}

/// A project open on a box, as `BoxProjects` holds it after opening.
fn opened(files: &[(&str, &str)]) -> crate::box_projects::OpenProject {
    crate::box_projects::OpenProject {
        name: "skipper_drive".into(),
        bufs: files
            .iter()
            .map(|(n, t)| {
                (
                    (*n).to_string(),
                    crate::box_projects::Buf {
                        text: (*t).into(),
                        base: (*t).into(),
                        loaded: true,
                        ..crate::box_projects::Buf::default()
                    },
                )
            })
            .collect(),
        current: Some("main.st".into()),
        runs: None,
    }
}

fn check_answer(body: &serde_json::Value) -> crate::box_projects::Answer {
    crate::box_projects::Answer {
        status: 200,
        retry_after: None,
        body: body.to_string(),
    }
}

const FB: &str = "FUNCTION_BLOCK ramp\nVAR_INPUT x : DINT; END_VAR\nEND_FUNCTION_BLOCK\n";
const TEST: &str = "PROGRAM test_drive\nEND_PROGRAM\n";

/// Nick 2026-11-24 (the agent on a Skipper page): a page a box served
/// deploys the checked project to the Skipper it watches: the program and
/// its function blocks as one source, never the requirement tests or the
/// map, under the program's name, which the Skipper's router takes.
#[test]
fn a_checked_project_goes_to_the_watched_skipper_as_one_program() {
    use crate::box_projects::{BoxProjects, Deploy, PCall};
    use helmsman_nodecore::router::{Effect, Outcome};
    let mut p = BoxProjects::default();
    p.available = Some(true);
    p.open = Some(opened(&[
        ("main.st", NEW),
        ("ramp.st", FB),
        ("test_drive.st", TEST),
        ("io.map", "device drive enip-io 192.168.33.1\n"),
    ]));
    p.to_node = true;
    // The Skipper runs this I/O list already: only the program goes.
    p.node_map = Some("device drive enip-io 192.168.33.1\n".into());
    p.deploy = Deploy::Checking;
    p.answer(
        PCall::Check("skipper_drive".into()),
        &check_answer(&serde_json::json!({
            "ok": true, "held": [], "diagnostics": [], "warnings": [],
            "modules": [{"name": "drive", "file": "main.st"}],
            "approval": {"needs": {"replay": true}}
        })),
    );
    // The box's policy asks a replay for this change: it runs first, on
    // the box, and nothing has gone to the Skipper yet (Nick, 2026-11-24).
    let reqs = p.poll();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert_eq!(reqs[0].call, PCall::Replay("skipper_drive".into()));
    assert!(!reqs[0].call.on_node());
    p.answer(
        PCall::Replay("skipper_drive".into()),
        &crate::box_projects::Answer {
            status: 201,
            retry_after: None,
            body: r#"{"replay": 7}"#.into(),
        },
    );
    let reqs = p.poll();
    assert_eq!(reqs.len(), 1, "the program, and no box deploy: {reqs:?}");
    let q = &reqs[0];
    assert!(q.call.on_node());
    assert_eq!((q.method, q.url.as_str()), ("PUT", "/api/modules/drive"));
    let Outcome::Effect(Effect::Deploy { name, source }) = routed(q, "bench-token-2026") else {
        panic!("the Skipper's router did not take it")
    };
    assert_eq!(name, "drive");
    assert!(
        source.contains("slew := 5000;") && source.contains("FUNCTION_BLOCK ramp"),
        "{source}"
    );
    assert!(
        !source.contains("test_drive"),
        "requirement tests stay on the box: {source}"
    );
    assert!(
        !source.contains("enip-io"),
        "the map is not program text: {source}"
    );
}

/// What the box's lint holds is not sent; nor is a project of two programs
/// (a Skipper runs one); nothing reaches the Skipper either way.
#[test]
fn a_held_or_two_program_project_is_not_sent_to_a_skipper() {
    use crate::box_projects::{BoxProjects, Deploy, PCall};
    for (body, says) in [
        (
            serde_json::json!({"ok": true, "held": ["main.st:3:5: tick (TON) is called inside an IF"],
                "modules": [{"name": "drive"}], "approval": {"needs": {}}}),
            "called inside an IF",
        ),
        (
            serde_json::json!({"ok": true, "held": [],
                "modules": [{"name": "drive"}, {"name": "other"}], "approval": {"needs": {}}}),
            "runs one program",
        ),
    ] {
        let mut p = BoxProjects::default();
        p.available = Some(true);
        p.open = Some(opened(&[("main.st", NEW)]));
        p.to_node = true;
        p.deploy = Deploy::Checking;
        p.answer(PCall::Check("skipper_drive".into()), &check_answer(&body));
        assert_eq!(p.poll().len(), 0);
        let Deploy::Failed(why) = &p.deploy else {
            panic!("{:?}", p.deploy)
        };
        assert!(why.contains(says), "{why}");
    }
}

/// A Skipper that refuses the token is the token's fault: the box's own
/// session is not ended, and the page says where the token goes.
#[test]
fn a_skipper_refusing_the_token_leaves_the_box_session_alone() {
    use crate::box_projects::{Answer, BoxProjects, Deploy, PCall};
    let mut p = BoxProjects::default();
    p.available = Some(false);
    p.track_module("drive", OLD);
    p.module.as_mut().unwrap().text = NEW.into();
    p.deploy_module();
    let _ = p.poll();
    p.answer(
        PCall::DeployModule("drive".into()),
        &Answer {
            status: 401,
            retry_after: None,
            body: r#"{"error":"a token is needed"}"#.into(),
        },
    );
    assert!(!p.ended, "the box's session is not the Skipper's token");
    let Deploy::Failed(why) = &p.deploy else {
        panic!("{:?}", p.deploy)
    };
    assert!(why.contains("token"), "{why}");
    assert!(p.module.as_ref().unwrap().dirty(), "the edits stay");
    // Only the watched node's calls go there; the box's stay at home.
    assert!(PCall::DeployModule("x".into()).on_node());
    assert!(!PCall::Check("x".into()).on_node());
    assert!(!PCall::List.on_node());
}

/// A replay the box could not run stops a Skipper deploy: nothing is sent.
#[test]
fn a_replay_that_did_not_run_sends_nothing_to_the_skipper() {
    use crate::box_projects::{Answer, BoxProjects, Deploy, PCall};
    let mut p = BoxProjects::default();
    p.available = Some(true);
    p.open = Some(opened(&[("main.st", NEW)]));
    p.to_node = true;
    p.deploy = Deploy::Checking;
    p.answer(
        PCall::Check("skipper_drive".into()),
        &check_answer(&serde_json::json!({"ok": true, "held": [],
            "modules": [{"name": "drive"}], "approval": {"needs": {"replay": true}}})),
    );
    assert_eq!(p.poll().len(), 1, "the replay");
    p.answer(
        PCall::Replay("skipper_drive".into()),
        &Answer {
            status: 422,
            retry_after: None,
            body: r#"{"error":"the replay faulted"}"#.into(),
        },
    );
    assert_eq!(p.poll().len(), 0);
    let Deploy::Failed(why) = &p.deploy else {
        panic!("{:?}", p.deploy)
    };
    assert!(why.contains("replay"), "{why}");
}

/// Nick 2026-11-24: "I'm also still not seeing the output visual diagram
/// of the program anywhere". A node serves no traced scans (its trace
/// stream answers 404): once that is known the program is drawn from its
/// source, compiled here for the statement table; a box that traces is
/// still waited for.
#[test]
fn a_node_without_traces_still_draws_its_program() {
    use crate::remote::Stream;
    let v = view();
    let mut s = BoxState::new();
    s.apply(Msg::Got(Fetch::Modules, Ok(api::modules(&v).to_string())), 1.0);
    let detail = api::module_detail(v.module.as_ref().unwrap());
    s.apply(
        Msg::Got(Fetch::Module("chase".into()), Ok(detail.to_string())),
        1.1,
    );
    assert!(s.lowered.is_none(), "a box may still trace: wait for it");
    s.apply(Msg::Failed(Stream::Trace), 1.2);
    let l = s.lowered.as_ref().expect("drawn from the source");
    assert_eq!(l.src, v.module.as_ref().unwrap().source);
    assert!(s.problem.is_none(), "{:?}", s.problem);
    // A source that does not compile says why and draws nothing.
    let mut s = BoxState::new();
    s.apply(Msg::Got(Fetch::Modules, Ok(api::modules(&v).to_string())), 1.0);
    let bad = serde_json::json!({ "name": "chase", "source": "PROGRAM chase\nx := ;\nEND_PROGRAM\n" });
    s.apply(Msg::Got(Fetch::Module("chase".into()), Ok(bad.to_string())), 1.1);
    s.apply(Msg::Failed(Stream::Trace), 1.2);
    assert!(s.lowered.is_none());
    assert!(
        s.problem
            .as_deref()
            .is_some_and(|p| p.starts_with("cannot draw chase")),
        "{:?}",
        s.problem
    );
}

const MAP_NOW: &str = "device drive enip-io 192.168.33.1 config=1 o2t=112:36 t2o=100:16 rpi_ms=20\nin moving bool drive in:0.1\n";
const MAP_NEW: &str = "device drive enip-io 192.168.33.1 config=1 o2t=112:36 t2o=100:16 rpi_ms=20\nin moving bool drive in:0.1\ndevice moxa modbus-tcp 192.168.33.60 unit=1\nin di0 bool moxa di:0\n";

fn answer(status: u16, body: &serde_json::Value) -> crate::box_projects::Answer {
    crate::box_projects::Answer {
        status,
        retry_after: None,
        body: body.to_string(),
    }
}

/// A project whose I/O list differs from the Skipper's, checked on the box.
fn checked_with_new_map() -> crate::box_projects::BoxProjects {
    use crate::box_projects::{BoxProjects, Deploy, PCall};
    let mut p = BoxProjects::default();
    p.available = Some(true);
    p.open = Some(opened(&[("main.st", NEW), ("io.map", MAP_NEW)]));
    p.to_node = true;
    p.node_map = Some(MAP_NOW.into());
    p.tick(10.0);
    p.deploy = Deploy::Checking;
    p.answer(
        PCall::Check("skipper_drive".into()),
        &check_answer(&serde_json::json!({"ok": true, "held": [],
            "modules": [{"name": "drive"}], "approval": {"needs": {}}})),
    );
    p
}

/// Nick 2026-11-24: the Agent's change to the I/O list never reached the
/// Skipper. Deploy now sends it first: the Skipper checks every device of
/// it, the page saves it once the check passed, waits for the Skipper to
/// restart on it, then sends the program (again, if the Skipper is still
/// restarting).
#[test]
fn a_changed_io_list_is_checked_and_saved_on_the_skipper_before_the_program() {
    use crate::box_projects::{Deploy, MapStep, PCall};
    use helmsman_nodecore::router::{Effect, Outcome};
    let mut p = checked_with_new_map();
    let reqs = p.poll();
    assert_eq!(reqs.len(), 1, "the map first, no program yet: {reqs:?}");
    assert_eq!(
        (reqs[0].method, reqs[0].url.as_str()),
        ("POST", "/api/io_map/check")
    );
    assert!(reqs[0].call.on_node());
    match routed(&reqs[0], "bench-token-2026") {
        Outcome::Effect(Effect::CheckIoMap { text, .. }) => assert_eq!(text, MAP_NEW),
        other => panic!("the Skipper's router did not take the check: {other:?}"),
    }
    assert!(p.deploy.busy());
    p.answer(
        PCall::NodeMapSend(MapStep::Check),
        &answer(202, &serde_json::json!({"checking": true})),
    );
    p.tick(10.5);
    assert!(p.poll().is_empty(), "read a second later, not at once");
    p.tick(11.1);
    let reqs = p.poll();
    assert_eq!(
        (reqs[0].method, reqs[0].url.as_str()),
        ("GET", "/api/io_map/check")
    );
    p.answer(
        PCall::NodeMapSend(MapStep::Poll),
        &answer(200, &serde_json::json!({"state": "running", "text": MAP_NEW})),
    );
    p.tick(12.2);
    let _ = p.poll();
    p.answer(
        PCall::NodeMapSend(MapStep::Poll),
        &answer(200, &serde_json::json!({"state": "done", "may_save": true, "passed": true, "text": MAP_NEW,
            "devices": [{"name": "moxa", "address": "192.168.33.60:502", "state": "answered", "error": null}]})),
    );
    let reqs = p.poll();
    assert_eq!(reqs.len(), 1);
    assert_eq!((reqs[0].method, reqs[0].url.as_str()), ("PUT", "/api/io_map"));
    let body: serde_json::Value = serde_json::from_slice(reqs[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["text"], MAP_NEW);
    p.answer(
        PCall::NodeMapSend(MapStep::Save),
        &answer(200, &serde_json::json!({"saved": true, "restarting": true})),
    );
    p.tick(15.0);
    assert!(p.poll().is_empty(), "the Skipper is restarting");
    p.tick(20.3);
    let reqs = p.poll();
    assert_eq!(
        (reqs[0].method, reqs[0].url.as_str()),
        ("PUT", "/api/modules/drive")
    );
    // Not up yet: tried again shortly.
    p.answer(
        PCall::DeployModule("drive".into()),
        &answer(0, &serde_json::json!(null)),
    );
    assert!(p.deploy.busy());
    p.tick(22.4);
    let reqs = p.poll();
    assert_eq!(
        (reqs[0].method, reqs[0].url.as_str()),
        ("PUT", "/api/modules/drive")
    );
    let ok = helmsman_nodecore::router::deployed("drive", NEW.len());
    p.answer(
        PCall::DeployModule("drive".into()),
        &answer(ok.status, &serde_json::from_slice(&ok.body).unwrap()),
    );
    assert!(matches!(p.deploy, Deploy::Done(_)), "{:?}", p.deploy);
}

/// A device the Skipper cannot reach stops the deploy: the I/O list is not
/// saved, no program is sent, and the page names the device and why.
#[test]
fn a_device_that_does_not_answer_stops_the_deploy_and_is_named() {
    use crate::box_projects::{Deploy, MapStep, PCall};
    let mut p = checked_with_new_map();
    let _ = p.poll();
    p.answer(
        PCall::NodeMapSend(MapStep::Check),
        &answer(202, &serde_json::json!({})),
    );
    p.tick(11.1);
    let _ = p.poll();
    p.answer(
        PCall::NodeMapSend(MapStep::Poll),
        &answer(200, &serde_json::json!({"state": "done", "may_save": false, "passed": false, "text": MAP_NEW,
            "devices": [
                {"name": "drive", "address": "192.168.33.1:44818", "state": "answered", "error": null},
                {"name": "moxa", "address": "192.168.33.60:502", "state": "error", "error": "connect timed out"}]})),
    );
    let Deploy::Failed(why) = &p.deploy else {
        panic!("{:?}", p.deploy)
    };
    assert!(
        why.contains("moxa (192.168.33.60:502): connect timed out"),
        "{why}"
    );
    assert!(!why.contains("drive ("), "{why}");
    for t in [12.0, 20.0, 40.0] {
        p.tick(t);
        assert!(p.poll().is_empty(), "nothing more is sent");
    }
}

/// The Skipper already runs the project's I/O list: only the program goes.
#[test]
fn an_unchanged_io_list_is_not_sent_again() {
    let mut p = crate::box_projects::BoxProjects::default();
    p.available = Some(true);
    p.open = Some(opened(&[("main.st", NEW), ("io.map", MAP_NOW)]));
    p.to_node = true;
    p.node_map = Some(format!("{MAP_NOW}\n"));
    p.deploy = crate::box_projects::Deploy::Checking;
    p.answer(
        crate::box_projects::PCall::Check("skipper_drive".into()),
        &check_answer(&serde_json::json!({"ok": true, "held": [],
            "modules": [{"name": "drive"}], "approval": {"needs": {}}})),
    );
    let reqs = p.poll();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert_eq!(
        (reqs[0].method, reqs[0].url.as_str()),
        ("PUT", "/api/modules/drive")
    );
}

/// Nick 2026-11-24, at 0:54 of his recording: "the replay cannot run:
/// modules do not fit together: drive: port SEEN: no signal of that name".
/// For a Skipper the page says it in plain words and sends nothing.
#[test]
fn a_replay_refused_for_a_port_without_a_signal_says_so_plainly() {
    use crate::box_projects::{BoxProjects, Deploy, PCall};
    let mut p = BoxProjects::default();
    p.available = Some(true);
    p.open = Some(opened(&[("main.st", NEW)]));
    p.to_node = true;
    p.deploy = Deploy::Checking;
    p.answer(
        PCall::Check("skipper_drive".into()),
        &check_answer(&serde_json::json!({"ok": true, "held": [],
            "modules": [{"name": "drive"}], "approval": {"needs": {"replay": true}}})),
    );
    let _ = p.poll();
    p.answer(
        PCall::Replay("skipper_drive".into()),
        &answer(422, &serde_json::json!({"changed": false,
            "error": "the replay cannot run: modules do not fit together: drive: port SEEN: no signal of that name"})),
    );
    let Deploy::Failed(why) = &p.deploy else {
        panic!("{:?}", p.deploy)
    };
    assert_eq!(
        why,
        "Not deployed: seen is not in the I/O list. Add it to the I/O list, or declare it under VAR in the program."
    );
    p.tick(100.0);
    assert_eq!(p.poll().len(), 0);
}
