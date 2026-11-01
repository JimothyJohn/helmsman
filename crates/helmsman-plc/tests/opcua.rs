//! The OPC UA server against a real, independent client (`async-opcua`, a
//! dev-dependency only): browse, read, refused writes, subscriptions; and
//! the `helmsman-plc` binary serving a running module's output.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use helmsman_io::{Dir, Ty, Value};
use helmsman_plc::opcua::feed::{ModShape, ModState, SigShape};
use helmsman_plc::opcua::{Config, Feed, Server, Shape, Snapshot};
use opcua::client::{ClientBuilder, DataChangeCallback, IdentityToken, Session};
use opcua::types::{
    AttributeId, BrowseDescription, BrowseDirection, DataValue, EUInformation, EndpointDescription,
    MessageSecurityMode, MonitoredItemCreateRequest, NodeId, ReadValueId, ReferenceTypeId, StatusCode,
    TimestampsToReturn, UserTokenPolicy, Variant, WriteValue,
};

fn shape() -> Arc<Shape> {
    Arc::new(Shape {
        signals: vec![
            SigShape {
                name: "level".into(),
                ty: Ty::Real,
                dir: Dir::In,
                unit: Some("mm".into()),
                source: None,
                device: true,
            },
            SigShape {
                name: "pump".into(),
                ty: Ty::Bool,
                dir: Dir::Out,
                unit: None,
                source: None,
                device: true,
            },
            SigShape {
                name: "ghost".into(),
                ty: Ty::Int,
                dir: Dir::In,
                unit: None,
                source: None,
                device: true,
            },
        ],
        modules: vec![ModShape {
            name: "fill".into(),
            outputs: vec![("pump".into(), 1)],
        }],
    })
}

fn snapshot(shape: &Arc<Shape>, seq: u64, level: f64) -> Arc<Snapshot> {
    Arc::new(Snapshot {
        seq,
        shape: Arc::clone(shape),
        values: vec![Some(Value::Real(level)), Some(Value::Bool(true)), None],
        errors: vec![false, false, false],
        changed: vec![Some(1_800_000_000_000); 3],
        modules: vec![ModState::Running],
    })
}

/// A fresh directory of its own: the pid and a counter keep parallel
/// copies of this suite apart.
fn tmp(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "opcua-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A connected, activated client session.
async fn connect(addr: SocketAddr, pki: &str) -> Arc<Session> {
    let mut client = ClientBuilder::new()
        .application_name("helmsman test client")
        .application_uri("urn:helmsman:test")
        .create_sample_keypair(false)
        .trust_server_certs(true)
        .pki_dir(tmp(pki))
        .session_retry_limit(0)
        .client()
        .unwrap();
    let url = format!("opc.tcp://{addr}/");
    let ep: EndpointDescription = (
        url.as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy::anonymous(),
    )
        .into();
    let (session, event_loop) = client
        .connect_to_endpoint_directly(ep, IdentityToken::Anonymous)
        .unwrap();
    event_loop.spawn();
    let up = tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
        .await
        .expect("connect timed out");
    assert!(up, "session did not connect");
    session
}

fn node(s: &str) -> NodeId {
    NodeId::new(1, s)
}

async fn read(session: &Session, id: NodeId) -> DataValue {
    let mut r = session
        .read(&[ReadValueId::from(id)], TimestampsToReturn::Both, 0.0)
        .await
        .unwrap();
    assert_eq!(r.len(), 1, "{r:?}");
    r.remove(0)
}

async fn browse(session: &Session, id: NodeId) -> Vec<String> {
    let r = session
        .browse(
            &[BrowseDescription {
                node_id: id,
                browse_direction: BrowseDirection::Forward,
                reference_type_id: ReferenceTypeId::HierarchicalReferences.into(),
                include_subtypes: true,
                node_class_mask: 0,
                result_mask: 0x3F,
            }],
            0,
            None,
        )
        .await
        .unwrap();
    assert_eq!(r[0].status_code, StatusCode::Good, "{r:?}");
    let mut names: Vec<String> = r[0]
        .references
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|d| d.browse_name.name.as_ref().to_string())
        .collect();
    names.sort();
    names
}

fn start() -> (Server, Arc<Feed>, Arc<Shape>) {
    let feed = Arc::new(Feed::default());
    let shape = shape();
    assert!(feed.offer(snapshot(&shape, 1, 12.5)));
    let server = Server::start(Config::new("127.0.0.1:0".parse().unwrap()), Arc::clone(&feed)).unwrap();
    (server, feed, shape)
}

#[test]
fn a_client_browses_the_box_and_reads_values_quality_and_units() {
    let (server, _feed, _shape) = start();
    runtime().block_on(async {
        let s = connect(server.addr(), "browse").await;
        let objects = browse(&s, NodeId::new(0, 85u32)).await;
        for want in ["Modules", "Server", "Signals"] {
            assert!(objects.iter().any(|n| n == want), "{objects:?}");
        }
        assert_eq!(browse(&s, node("signals")).await, ["ghost", "level", "pump"]);
        assert_eq!(browse(&s, node("modules")).await, ["fill"]);
        assert_eq!(browse(&s, node("modules/fill")).await, ["Outputs", "Status"]);
        assert_eq!(browse(&s, node("modules/fill/Outputs")).await, ["pump"]);
        assert_eq!(browse(&s, node("signals/level")).await, ["EngineeringUnits"]);

        let level = read(&s, node("signals/level")).await;
        assert_eq!(level.value, Some(Variant::Double(12.5)), "{level:?}");
        assert!(level.status.is_none_or(|c| c.is_good()), "{level:?}");
        assert!(level.source_timestamp.is_some(), "{level:?}");
        assert!(level.server_timestamp.is_some(), "{level:?}");

        // A signal with no value is Bad and carries none.
        let ghost = read(&s, node("signals/ghost")).await;
        assert_eq!(ghost.value, None, "{ghost:?}");
        assert_eq!(ghost.status, Some(StatusCode::BadNoCommunication), "{ghost:?}");

        let pump = read(&s, node("modules/fill/Outputs/pump")).await;
        assert_eq!(pump.value, Some(Variant::Boolean(true)), "{pump:?}");
        let status = read(&s, node("modules/fill/Status")).await;
        assert_eq!(status.value, Some(Variant::from("running")), "{status:?}");

        let eu = read(&s, node("signals/level/EngineeringUnits")).await;
        let Some(Variant::ExtensionObject(ext)) = eu.value else {
            panic!("EngineeringUnits is not a structure: {eu:?}");
        };
        let eu = ext.inner_as::<EUInformation>().expect("an EUInformation");
        assert_eq!(eu.display_name.text.as_ref(), "mm");
        assert_eq!(eu.unit_id, -1);

        let unknown = read(&s, node("signals/nope")).await;
        assert_eq!(unknown.status, Some(StatusCode::BadNodeIdUnknown), "{unknown:?}");
        let _ = s.disconnect().await;
    });
}

#[test]
fn writes_are_refused_and_change_nothing() {
    let (server, _feed, _shape) = start();
    runtime().block_on(async {
        let s = connect(server.addr(), "write").await;
        let write = |id: NodeId, v: Variant| WriteValue {
            node_id: id,
            attribute_id: AttributeId::Value as u32,
            index_range: opcua::types::NumericRange::default(),
            value: DataValue {
                value: Some(v),
                ..Default::default()
            },
        };
        let r = s
            .write(&[
                write(node("signals/pump"), Variant::Boolean(false)),
                write(node("modules/fill/Outputs/pump"), Variant::Boolean(false)),
                write(node("signals/level"), Variant::Double(99.0)),
                write(node("signals/nope"), Variant::Boolean(false)),
            ])
            .await
            .unwrap();
        assert_eq!(
            r,
            [
                StatusCode::BadNotWritable,
                StatusCode::BadNotWritable,
                StatusCode::BadNotWritable,
                StatusCode::BadNodeIdUnknown
            ]
        );
        let pump = read(&s, node("signals/pump")).await;
        assert_eq!(pump.value, Some(Variant::Boolean(true)), "{pump:?}");
        // And the variables say so up front.
        let mut r = s
            .read(
                &[ReadValueId {
                    node_id: node("signals/pump"),
                    attribute_id: AttributeId::UserAccessLevel as u32,
                    ..Default::default()
                }],
                TimestampsToReturn::Neither,
                0.0,
            )
            .await
            .unwrap();
        assert_eq!(r.remove(0).value, Some(Variant::Byte(1)));
        let _ = s.disconnect().await;
    });
}

#[test]
fn a_subscription_delivers_changes() {
    let (server, feed, shape) = start();
    runtime().block_on(async {
        let s = connect(server.addr(), "subscribe").await;
        let (tx, rx) = mpsc::channel::<DataValue>();
        let sub = s
            .create_subscription(
                Duration::from_millis(100),
                30,
                10,
                0,
                0,
                true,
                DataChangeCallback::new(move |dv, _| {
                    let _ = tx.send(dv);
                }),
            )
            .await
            .unwrap();
        let r = s
            .create_monitored_items(
                sub,
                TimestampsToReturn::Both,
                vec![MonitoredItemCreateRequest::from(node("signals/level"))],
            )
            .await
            .unwrap();
        assert!(r[0].result.status_code.is_good(), "{r:?}");
        // The initial value, then each change the box publishes.
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut next: u32 = 1;
        while Instant::now() < deadline && !seen.contains(&Variant::Double(3.0)) {
            if let Ok(dv) = rx.recv_timeout(Duration::from_millis(50)) {
                seen.push(dv.value.unwrap_or(Variant::Empty));
            }
            if next <= 3 && seen.len() >= next as usize {
                feed.offer(snapshot(&shape, 1 + u64::from(next), f64::from(next)));
                next += 1;
            }
        }
        assert_eq!(seen.first(), Some(&Variant::Double(12.5)), "{seen:?}");
        assert!(seen.contains(&Variant::Double(3.0)), "{seen:?}");
        let _ = s.disconnect().await;
    });
}

// ---- the binary ---------------------------------------------------------------

struct Plc(Child);

impl Drop for Plc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_box_serves_a_running_modules_output() {
    let d = tmp("bin");
    let prog = d.join("counter.st");
    std::fs::write(
        &prog,
        "PROGRAM counter\nVAR_OUTPUT count : INT; END_VAR\n  count := count + 1;\nEND_PROGRAM\n",
    )
    .unwrap();
    let map = d.join("map");
    std::fs::write(&map, "in count int module counter unit=parts\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .current_dir(&d)
        .arg(&prog)
        .args(["--io-map".as_ref(), map.as_os_str()])
        .args([
            "--api",
            "off",
            "--opcua",
            "127.0.0.1:0",
            "--bind",
            "127.0.0.1",
            "--encap-port",
            "0",
            "--io-port",
            "0",
            "--scan-ms",
            "20",
        ])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let plc = Plc(child);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let addr: SocketAddr = loop {
        let line = rx.recv_timeout(Duration::from_secs(20)).expect("no OPC UA line");
        if let Some(rest) = line.split("OPC UA on opc.tcp://").nth(1) {
            break rest.split('/').next().unwrap().parse().unwrap();
        }
    };
    runtime().block_on(async {
        let s = connect(addr, "bin").await;
        assert_eq!(browse(&s, node("modules/counter/Outputs")).await, ["count"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        let first = loop {
            let v = read(&s, node("signals/count")).await;
            if let Some(Variant::Int64(n)) = v.value {
                break n;
            }
            assert!(Instant::now() < deadline, "no value: {v:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        loop {
            let v = read(&s, node("modules/counter/Outputs/count")).await;
            if matches!(v.value, Some(Variant::Int64(n)) if n > first) {
                break;
            }
            assert!(Instant::now() < deadline, "count never moved past {first}: {v:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = s.disconnect().await;
    });
    drop(plc);
}

#[test]
fn a_taken_port_stops_the_box_with_a_reason() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let d = tmp("taken");
    let prog = d.join("p.st");
    std::fs::write(&prog, "PROGRAM p\nVAR x : INT; END_VAR\n x := 1;\nEND_PROGRAM\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .current_dir(&d)
        .arg(&prog)
        .args([
            "--api",
            "off",
            "--opcua",
            &taken.local_addr().unwrap().to_string(),
        ])
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--opcua") && err.contains("--opcua off"), "{err}");
}
