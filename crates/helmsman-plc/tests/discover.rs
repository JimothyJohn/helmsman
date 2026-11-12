//! `devices-repo`: the box scans its network and looks what it finds up in
//! the device library. Real sockets throughout: the simulated E1213
//! answers the `ListIdentity`, a static server on loopback serves the
//! library's `index.json` and map file, the API runs on its own listener.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helmsman_io::enip_sim::E1213Sim;
use helmsman_io::resolve::ModbusProbe;
use helmsman_io::sim::{Identification, Sim, State};
use helmsman_plc::SignalTable;
use helmsman_plc::api::discover::{Arp, Discovery};
use helmsman_plc::api::{
    Api, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, More, Shared,
};
use helmsman_plc::store::ChangeSet;
use serde_json::Value as Json;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const INDEX: &str = r#"{ "format": "helmsman-devices-index 1", "devices": [
  { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213", "kind": "remote-io",
    "identity": { "enip": { "vendor_id": 991, "product_code": 515 } },
    "protocols": { "enip": { "driver": "enip-io", "map": "enip.map", "profile": "moxa-e1213" },
                   "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } } ] }"#;
const ENIP_MAP: &str =
    "# the E1213\ndevice e1213 enip-io ADDR profile=moxa-e1213 rpi_ms=100\nin di0 bool e1213 di:0\n";

/// A static file server on loopback: `path -> body`, anything else 404.
struct Files {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Files {
    fn serve(files: BTreeMap<&'static str, String>) -> Self {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = l.local_addr().unwrap();
        l.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                let Ok((mut c, _)) = l.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // An accepted socket inherits the listener's non-blocking
                // mode on macOS: a read before the request arrives would
                // return at once and serve `/` (404).
                c.set_nonblocking(false).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut buf = [0u8; 4096];
                let n = c.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/");
                let (status, body) = match files.get(path) {
                    Some(b) => ("200 OK", b.clone()),
                    None => ("404 Not Found", String::new()),
                };
                let _ = write!(
                    c,
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            addr,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Json {
        serde_json::from_str(&self.body).unwrap_or(Json::Null)
    }
}

fn request(addr: SocketAddr, method: &str, path: &str, bearer: bool) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let auth = if bearer {
        format!("Authorization: Bearer {TOKEN}\r\n")
    } else {
        String::new()
    };
    let body = if method == "POST" { "{}" } else { "" };
    let ctype = if method == "POST" {
        "Content-Type: application/json\r\n"
    } else {
        ""
    };
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: box\r\n{auth}{ctype}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    Resp {
        status: head[9..12].parse().unwrap(),
        body: body.to_string(),
    }
}

/// A box whose running map names the simulated E1213 by `cip=`, scanning
/// at the simulator's address, with the library at `library`.
fn box_with(sim: &E1213Sim, library: Option<String>, map: &str) -> Api {
    box_probing(sim, library, map, None, Arp::Host)
}

/// [`box_with`], with the Modbus probe and the ARP table given.
fn box_probing(
    sim: &E1213Sim,
    library: Option<String>,
    map: &str,
    modbus: Option<ModbusProbe>,
    arp: Arp,
) -> Api {
    let parsed = helmsman_io::text::parse("in ready bool box di:0\n").unwrap();
    let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
    let catalog = Catalog {
        signals: Catalog::signals(&table, Some(&parsed.map), &[]),
        modules: Vec::new(),
    };
    let cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        token: TOKEN.into(),
        limits: Limits::default(),
        stream_every: Duration::from_millis(50),
        memory: 0,
        log: Arc::new(|_: &str| {}),
    };
    let shared = Shared {
        catalog,
        live: Arc::new(Live::default()),
        devices: Arc::new(Devices::new(["box".to_string(), "e1213".to_string()])),
        audit: Box::new(MemAudit::new(100)),
        proposals: Box::new(MemProposals::new(10)),
        running: Some(ChangeSet {
            modules: BTreeMap::new(),
            io_map: Some(map.into()),
            calendar: None,
        }),
        snapshots: None,
        approval: None,
        chat: None,
    };
    let encap: SocketAddrV4 = sim.endpoint().parse().unwrap();
    let more = More {
        discovery: Discovery {
            sweep: Some(encap),
            window: Duration::from_millis(300),
            library,
            modbus,
            arp,
        },
        ..More::default()
    };
    Api::start_box(cfg, shared, Extras::default(), None, more).unwrap()
}

fn library() -> Files {
    Files::serve(BTreeMap::from([
        ("/index.json", INDEX.to_string()),
        ("/devices/moxa/iologik-e1213/enip.map", ENIP_MAP.to_string()),
    ]))
}

#[test]
fn a_scan_lists_the_simulated_e1213_mapped_and_known_to_the_library() {
    let sim = E1213Sim::start().unwrap();
    let lib = library();
    let api = box_with(
        &sim,
        Some(lib.url("/index.json")),
        "device e1213 enip-io cip=991:515 profile=moxa-e1213 rpi_ms=100\nin di0 bool e1213 di:0\n",
    );
    // A scan needs the token (or a session): reads are open, this sends a packet.
    assert_eq!(
        request(api.addr(), "POST", "/api/devices/discover", false).status,
        401
    );
    let scan = request(api.addr(), "POST", "/api/devices/discover", true);
    assert_eq!(scan.status, 200, "{}", scan.body);
    let answer = scan.json();
    let devices = answer["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1, "{answer}");
    let dev = &devices[0];
    assert_eq!(dev["vendor_id"], 991);
    assert_eq!(dev["product_code"], 515);
    assert_eq!(dev["address"], "127.0.0.1");
    assert_eq!(dev["product_name"], "E1213");
    assert_eq!(dev["cip"], "cip=991:515:0x00e12130");
    assert_eq!(dev["mapped"], "e1213", "the map names it by identity");
    assert_eq!(dev["library"]["model"], "ioLogik E1213");
    assert_eq!(dev["library"]["profile"], "moxa-e1213");
    assert_eq!(answer["library"]["devices"], 1);
    // The second ask within a minute answers from memory; `fresh=1` scans again.
    let again = request(api.addr(), "POST", "/api/devices/discover", true).json();
    assert!(again["age_ms"].as_u64().unwrap() >= 1 || again["devices"] == answer["devices"]);
    let fresh = request(api.addr(), "POST", "/api/devices/discover?fresh=1", true).json();
    assert_eq!(fresh["devices"].as_array().unwrap().len(), 1);
    // Every scan is audited as a read-only act.
    let audit = request(api.addr(), "GET", "/api/audit?limit=100", true).json();
    assert!(
        audit.as_array().unwrap().iter().any(|e| e["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("discover: 1 device(s), 1 EtherNet/IP"))),
        "{audit}"
    );
    // The library route: the map lines with the address in.
    let lib = request(
        api.addr(),
        "GET",
        "/api/devices/library?vendor=991&product=515&address=10.0.0.57",
        true,
    );
    assert_eq!(lib.status, 200);
    let lib = lib.json();
    assert_eq!(lib["matches"].as_array().unwrap().len(), 1, "{lib}");
    let lines = lib["matches"][0]["lines"].as_str().unwrap();
    assert!(
        lines.contains("device e1213 enip-io 10.0.0.57 profile=moxa-e1213 rpi_ms=100\n"),
        "{lines}"
    );
    assert!(lines.contains("in di0 bool e1213 di:0"), "{lines}");
    // By text, and a bad ask.
    let by_text = request(api.addr(), "GET", "/api/devices/library?q=moxa%20e1213", true).json();
    assert_eq!(by_text["matches"].as_array().unwrap().len(), 1);
    let none = request(api.addr(), "GET", "/api/devices/library?q=nothing%20here", true).json();
    assert_eq!(none["matches"].as_array().unwrap().len(), 0);
    assert_eq!(
        request(api.addr(), "GET", "/api/devices/library?vendor=x", true).status,
        400
    );
    assert_eq!(
        request(api.addr(), "GET", "/api/devices/library", true).status,
        400
    );
}

#[test]
fn an_unmapped_device_says_so_and_a_hostile_library_is_refused_whole() {
    let sim = E1213Sim::start().unwrap();
    let lib = Files::serve(BTreeMap::from([(
        "/index.json",
        format!(
            r#"{{ "format": "helmsman-devices-index 1", "devices": [ {{ "folder": "../../etc", "vendor": "x", "model": "y" }} ], "pad": "{}" }}"#,
            "p".repeat(100)
        ),
    )]));
    let api = box_with(&sim, Some(lib.url("/index.json")), "in ready bool box di:0\n");
    let j = request(api.addr(), "POST", "/api/devices/discover", true).json();
    assert_eq!(j["devices"][0]["mapped"], Json::Null, "{j}");
    assert_eq!(j["devices"][0]["library"], Json::Null);
    let why = j["library"]["unavailable"].as_str().unwrap();
    assert!(why.contains("folder"), "{why}");
    let m = request(
        api.addr(),
        "GET",
        "/api/devices/library?vendor=991&product=515",
        true,
    )
    .json();
    assert_eq!(m["matches"].as_array().unwrap().len(), 0);
    assert!(m["unavailable"].as_str().unwrap().contains("folder"), "{m}");
}

#[test]
fn a_box_without_scanning_or_library_says_so() {
    let sim = E1213Sim::start().unwrap();
    let api = {
        let parsed = helmsman_io::text::parse("in ready bool box di:0\n").unwrap();
        let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
        let catalog = Catalog {
            signals: Catalog::signals(&table, Some(&parsed.map), &[]),
            modules: Vec::new(),
        };
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            limits: Limits::default(),
            stream_every: Duration::from_millis(50),
            memory: 0,
            log: Arc::new(|_: &str| {}),
        };
        let shared = Shared {
            catalog,
            live: Arc::new(Live::default()),
            devices: Arc::new(Devices::new(["box".to_string()])),
            audit: Box::new(MemAudit::new(100)),
            proposals: Box::new(MemProposals::new(10)),
            running: None,
            snapshots: None,
            approval: None,
            chat: None,
        };
        Api::start_with(cfg, shared, Extras::default()).unwrap()
    };
    drop(sim);
    let r = request(api.addr(), "POST", "/api/devices/discover", true);
    assert_eq!(r.status, 501, "{}", r.body);
    let m = request(api.addr(), "GET", "/api/devices/library?q=moxa", true).json();
    assert_eq!(m["matches"].as_array().unwrap().len(), 0);
    assert!(
        m["unavailable"].as_str().unwrap().contains("devices-index off"),
        "{m}"
    );
}

const MODBUS_INDEX: &str = r#"{ "format": "helmsman-devices-index 1", "devices": [
  { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213", "kind": "remote-io",
    "identity": { "enip": { "vendor_id": 991, "product_code": 515 },
                  "modbus": { "vendor_name": "Moxa", "product_code": "E1213" } },
    "protocols": { "enip": { "driver": "enip-io", "map": "enip.map", "profile": "moxa-e1213" },
                   "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } } ] }"#;

/// A box probing the ARP table (the simulated Modbus device on loopback,
/// whose MAC is the E1213's) under `probe`.
fn probing_box(sim: &E1213Sim, rio: &Sim, probe: impl FnOnce(ModbusProbe) -> ModbusProbe) -> (Api, Files) {
    let lib = Files::serve(BTreeMap::from([("/index.json", MODBUS_INDEX.to_string())]));
    let probe = probe(ModbusProbe {
        port: rio.addr.port(),
        per_host: Duration::from_millis(500),
        total: Duration::from_secs(2),
        ..ModbusProbe::default()
    });
    let arp = Arp::Fixed("? (127.0.0.1) at 0:90:e8:7e:af:be on lo0 ifscope [ethernet]\n".into());
    let api = box_probing(
        sim,
        Some(lib.url("/index.json")),
        "device rio modbus-tcp modbus=moxa:e1213 unit=1\nin di0 bool rio di:0\n",
        Some(probe),
        arp,
    );
    (api, lib)
}

fn modbus_sim() -> Sim {
    let mut st = State::new(8);
    st.identification = Some(Identification::basic("Moxa", "E1213", "V3.1 Build19110615"));
    Sim::start(st).unwrap()
}

/// `one-scan`: a plain scan, no flag, probes the ARP table's hosts with
/// FC 43/14 (here the simulator, on loopback), matches the device to the
/// map line naming it by `modbus=` and to the library's `identity.modbus`,
/// and the unit that answered on EtherNet/IP and on Modbus is ONE entry
/// of `found`, its two identities side by side.
#[test]
fn one_scan_finds_the_device_once_with_both_identities() {
    let sim = E1213Sim::start().unwrap();
    let rio = modbus_sim();
    let (api, _lib) = probing_box(&sim, &rio, |p| p);
    let scan = request(api.addr(), "POST", "/api/devices/discover", true);
    assert_eq!(scan.status, 200, "{}", scan.body);
    let j = scan.json();
    // The one list: a device, two identities, the map's and the library's match.
    let found = j["found"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{j}");
    let d = &found[0];
    assert_eq!(d["mac"], "00-90-e8-7e-af-be");
    assert_eq!(d["address"], "127.0.0.1");
    assert_eq!(d["mapped"], "rio", "the map names it by modbus=");
    assert_eq!(d["library"]["model"], "ioLogik E1213");
    let ids = d["identities"].as_array().unwrap();
    let protocols: Vec<&str> = ids.iter().map(|i| i["protocol"].as_str().unwrap()).collect();
    assert_eq!(protocols, ["ethernet-ip", "modbus-tcp"], "{d}");
    assert_eq!(ids[0]["vendor_id"], 991);
    assert_eq!(ids[0]["product_name"], "E1213");
    assert_eq!(ids[1]["vendor_name"], "Moxa");
    assert_eq!(ids[1]["identity"], "modbus=Moxa:E1213");
    // PROFINET was not looked for, and the answer says so in words.
    let says = j["protocols"].as_array().unwrap();
    assert_eq!(says.len(), 3, "{j}");
    assert_eq!(says[0]["looked"], true);
    assert_eq!(says[1]["looked"], true);
    assert_eq!(says[2]["protocol"], "profinet");
    assert_eq!(says[2]["looked"], false);
    assert!(
        says[2]["why"]
            .as_str()
            .unwrap()
            .starts_with("PROFINET was not looked for"),
        "{j}"
    );
    // The old per-protocol fields are still there.
    assert_eq!(j["devices"].as_array().unwrap().len(), 1, "{j}");
    let old = &j["modbus"].as_array().unwrap()[0];
    assert_eq!(old["address"], "127.0.0.1");
    assert_eq!(old["mac"], "00-90-e8-7e-af-be");
    assert_eq!(old["vendor_name"], "Moxa");
    assert_eq!(old["product_code"], "E1213");
    assert_eq!(old["revision"], "V3.1 Build19110615");
    assert_eq!(old["identity"], "modbus=Moxa:E1213");
    assert_eq!(old["mapped"], "rio", "the map names it by modbus=, case-blind");
    assert_eq!(old["library"]["driver"], "modbus-tcp");
    assert_eq!(old["library"]["map"], "modbus.map");
    // The probe was read-only: identification requests only.
    assert!(rio.state().requests >= 1);
    assert_eq!(rio.state().coils, vec![false; 8]);
    assert_eq!(rio.state().holding, vec![0; 8]);
    // Answered from memory within a minute, probe included.
    let before = rio.state().requests;
    let again = request(api.addr(), "POST", "/api/devices/discover", true).json();
    assert_eq!(again["found"], j["found"]);
    assert_eq!(rio.state().requests, before);
    // `?modbus=1`, the old switch, is accepted and changes nothing.
    let old_client = request(api.addr(), "POST", "/api/devices/discover?modbus=1", true);
    assert_eq!(old_client.status, 200, "{}", old_client.body);
    assert_eq!(old_client.json()["found"], j["found"]);
    let audit = request(api.addr(), "GET", "/api/audit?limit=100", true).json();
    assert!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["detail"].as_str().is_some_and(|d| {
                d.contains("discover: 1 device(s), 1 EtherNet/IP, 1 Modbus (FC 43/14), 0 PROFINET")
            })),
        "{audit}"
    );
}

/// The probe stays bounded: with room for no host it asks nobody, and the
/// device is still found, by its EtherNet/IP answer alone.
#[test]
fn the_probe_keeps_its_bounds() {
    let sim = E1213Sim::start().unwrap();
    let rio = modbus_sim();
    let (api, _lib) = probing_box(&sim, &rio, |p| ModbusProbe { max_hosts: 0, ..p });
    let j = request(api.addr(), "POST", "/api/devices/discover", true).json();
    assert_eq!(rio.state().requests, 0, "no host was in the probe's bound");
    assert_eq!(j["modbus"].as_array().unwrap().len(), 0, "{j}");
    let found = j["found"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{j}");
    assert_eq!(found[0]["identities"].as_array().unwrap().len(), 1, "{j}");
}

#[test]
fn a_box_without_the_probe_says_modbus_was_not_looked_for() {
    let sim = E1213Sim::start().unwrap();
    let api = box_with(&sim, None, "in ready bool box di:0\n");
    let r = request(api.addr(), "POST", "/api/devices/discover", true);
    assert_eq!(r.status, 200, "{}", r.body);
    let j = r.json();
    assert_eq!(j["found"].as_array().unwrap().len(), 1, "{j}");
    assert!(j.get("modbus").is_none(), "{j}");
    let says = &j["protocols"][1];
    assert_eq!(says["protocol"], "modbus-tcp");
    assert_eq!(says["looked"], false);
    assert!(says["why"].as_str().unwrap().contains("Modbus"), "{j}");
}
