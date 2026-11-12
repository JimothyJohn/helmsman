//! The Devices tab's "On the network" and the device library
//! (`devices-repo`, interview 2026-11-02).
//!
//! - `POST /api/devices/discover` (token or session): one scan, every
//!   protocol the box speaks, read-only on the network. One `ListIdentity`
//!   broadcast ([`helmsman_io::resolve::discover`]) and the host's ARP
//!   table read after it; the Modbus identification probe beside it
//!   ([`helmsman_io::resolve::probe_modbus`]: Read Device Identification,
//!   FC 43/14, to TCP 502 of the hosts in the ARP table and no others,
//!   bounded in hosts, concurrency and time); and, in a build with
//!   `profinet-io` when the running map names an interface (`if=`), one
//!   PROFINET DCP Identify on it. One sweep runs at a time (a second asker
//!   gets 409), and the last result is answered from memory for [`FRESH`]
//!   unless `?fresh=1`. (`?modbus=1`, which used to switch the probe on, is
//!   accepted and ignored.)
//!
//!   The answer's `found` is the list to show: one entry per device, the
//!   answers that share a MAC joined (an entry without a MAC is keyed by
//!   its address), each with every identity it answered (`identities`:
//!   `protocol`, `address`, the vendor and product fields), whether a
//!   device of the running map already binds it (`mapped`) and the
//!   library's match. `protocols` says per protocol whether it was looked
//!   for and, when not, why in words. The per-protocol lists stay for one
//!   release: `devices` (EtherNet/IP) and `modbus`.
//! - `GET /api/devices/library?vendor=V&product=P[&address=A]`, or
//!   `?q=text`: the library's matches, each with the map lines of its
//!   `enip.map` (or the one map it has) with `ADDR` replaced by `address`
//!   when given. The index and the map files come from the library's URL
//!   ([`Discovery::library`]; a site hosts its own with
//!   `--devices-index`), fetched over HTTPS and kept for [`INDEX_FRESH`];
//!   a box without internet has no matches and says so.
//!
//! Everything from the library is data from a public repository: parsed
//! and bounded by [`helmsman_project::devlib`], never run.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use helmsman_io::resolve::{self, Identity, ModbusProbe, ModbusSeen, ProfinetSeen, Resolver, Seen};
use helmsman_project::devlib::{self, Entry, Index};
use helmsman_project::iomap;
use serde_json::{Value as Json, json};

use super::http::{Request, Response};
use super::{Ctx, audit, audit_full};

/// How long a sweep's result answers for, without `?fresh=1`.
pub const FRESH: Duration = Duration::from_secs(60);
/// How long the library's index is kept before it is fetched again.
pub const INDEX_FRESH: Duration = Duration::from_secs(24 * 3600);
/// Largest map file taken from the library.
const MAX_MAP: usize = 64 * 1024;

/// Where a scan reads the ARP table from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Arp {
    /// The host's ([`resolve::host_arp_table`]).
    #[default]
    Host,
    /// This text, in either format (a test's table).
    Fixed(String),
}

/// How the box scans and where its library is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// Where the `ListIdentity` goes: the broadcast address and port.
    /// `None`: no scanning (`/api/devices/discover` answers 501).
    pub sweep: Option<SocketAddrV4>,
    /// How long replies are gathered.
    pub window: Duration,
    /// The library index's URL; `None`: no library.
    pub library: Option<String>,
    /// The Modbus identification probe a scan runs; `None`: none (the
    /// answer says Modbus was not looked for).
    pub modbus: Option<ModbusProbe>,
    /// The ARP table the scan reads MACs from and the probe takes its
    /// hosts from.
    pub arp: Arp,
}

impl Default for Discovery {
    /// Off: no sweep, no probe, no library. The binary turns them on
    /// ([`Discovery::lan`]); a test that never asked for a scan must not
    /// broadcast on whatever network it runs on.
    fn default() -> Self {
        Self {
            sweep: None,
            window: Duration::from_millis(700),
            library: None,
            modbus: None,
            arp: Arp::Host,
        }
    }
}

impl Discovery {
    /// A box on a LAN: the subnet-wide `ListIdentity` on 44818, the
    /// Modbus probe's defaults ([`ModbusProbe::default`]) and the library
    /// at `library` (`None`: no library).
    #[must_use]
    pub fn lan(library: Option<String>) -> Self {
        Self {
            sweep: Some(SocketAddrV4::new(Ipv4Addr::BROADCAST, 44818)),
            window: Duration::from_millis(700),
            library,
            modbus: Some(ModbusProbe::default()),
            arp: Arp::Host,
        }
    }

    fn resolver(&self, sweep: Option<SocketAddrV4>) -> Resolver {
        let arp: Box<dyn Fn() -> std::io::Result<String> + Send + Sync> = match &self.arp {
            Arp::Host => Box::new(resolve::host_arp_table),
            Arp::Fixed(t) => {
                let t = t.clone();
                Box::new(move || Ok(t.clone()))
            }
        };
        Resolver {
            arp,
            sweep,
            window: self.window,
            modbus: self.modbus.clone(),
        }
    }
}

/// One scan's result: what each protocol answered (`None`: not looked
/// for) and the words for each protocol saying whether it was.
#[derive(Clone)]
struct Scan {
    at: Instant,
    seen: Vec<Seen>,
    modbus: Option<Vec<ModbusSeen>>,
    profinet: Option<Vec<ProfinetSeen>>,
    protocols: Vec<Json>,
}

/// What the API keeps between calls.
pub(super) struct State {
    cfg: Discovery,
    last: Mutex<Option<Scan>>,
    /// Held while a sweep runs.
    running: Mutex<()>,
    /// The index, or why it could not be had, and when that was.
    index: Mutex<Option<(Instant, Result<Index, String>)>>,
    /// Map files fetched, by URL.
    maps: Mutex<Vec<(String, Instant, String)>>,
}

impl State {
    pub(super) fn new(cfg: Discovery) -> Self {
        Self {
            cfg,
            last: Mutex::new(None),
            running: Mutex::new(()),
            index: Mutex::new(None),
            maps: Mutex::new(Vec::new()),
        }
    }

    /// The library's base URL (the index's without its file name).
    fn base(&self) -> Option<String> {
        let url = self.cfg.library.as_deref()?;
        Some(url.rsplit_once('/').map_or(url, |(b, _)| b).to_string())
    }

    /// The index, fetched when missing or stale.
    fn index(&self) -> Result<Index, String> {
        let Some(url) = self.cfg.library.as_deref() else {
            return Err("no device library (started with --devices-index off)".into());
        };
        let mut slot = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, r)) = slot.as_ref()
            && at.elapsed() < INDEX_FRESH
            && (r.is_ok() || at.elapsed() < Duration::from_secs(60))
        {
            return r.clone();
        }
        let r = fetch_text(url, devlib::MAX_BYTES).and_then(|t| Index::parse(&t));
        *slot = Some((Instant::now(), r.clone()));
        r
    }

    /// A map file of the library, fetched when missing or stale.
    fn map_text(&self, url: &str) -> Result<String, String> {
        let mut maps = self.maps.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, at, text)) = maps.iter().find(|(u, _, _)| u == url)
            && at.elapsed() < INDEX_FRESH
        {
            return Ok(text.clone());
        }
        let text = fetch_text(url, MAX_MAP)?;
        maps.retain(|(u, _, _)| u != url);
        if maps.len() >= 64 {
            maps.remove(0);
        }
        maps.push((url.to_string(), Instant::now(), text.clone()));
        Ok(text)
    }
}

/// `GET url`, the body as UTF-8 text of at most `cap` bytes.
fn fetch_text(url: &str, cap: usize) -> Result<String, String> {
    use helmsman_agent::http::{self, Request as Req, Timeouts, Url};
    let full = Url::parse(url)?;
    let path = if full.prefix.is_empty() {
        "/".to_string()
    } else {
        full.prefix.clone()
    };
    let base = Url {
        prefix: String::new(),
        ..full
    };
    let req = Req {
        method: "GET",
        path: &path,
        headers: &[("Accept", "application/json, text/plain")],
        json: None,
        raw: None,
    };
    let t = Timeouts {
        connect: Duration::from_secs(5),
        idle: Duration::from_secs(10),
        total: Duration::from_secs(20),
    };
    let resp = http::send(&base, &req, t).map_err(|e| format!("{url}: {e:?}"))?;
    if resp.status != 200 {
        return Err(format!("{url}: HTTP {}", resp.status));
    }
    let bytes = resp.read_body(cap).map_err(|e| format!("{url}: {e:?}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{url}: not UTF-8"))
}

/// A device a scan found, as [`binds`] matches it: where, its MAC, and
/// whichever identities it answered with.
struct Found<'a> {
    address: Ipv4Addr,
    mac: Option<[u8; 6]>,
    /// `ListIdentity`'s vendor id, product code, serial.
    cip: Option<(u16, u16, u32)>,
    /// Read Device Identification's `VendorName`, `ProductCode`.
    modbus: Option<(&'a str, &'a str)>,
}

impl<'a> From<&'a Seen> for Found<'a> {
    fn from(s: &'a Seen) -> Self {
        Self {
            address: s.address,
            mac: s.mac,
            cip: Some((s.vendor_id, s.product_code, s.serial)),
            modbus: None,
        }
    }
}

impl<'a> From<&'a ModbusSeen> for Found<'a> {
    fn from(s: &'a ModbusSeen) -> Self {
        Self {
            address: s.address,
            mac: s.mac,
            cip: None,
            modbus: Some((&s.identification.vendor_name, &s.identification.product_code)),
        }
    }
}

/// Whether a map `device` line binds the device `found`: by the host it
/// names, its `mac=`, or its `cip=` or `modbus=` identity.
fn binds(d: &iomap::MapDevice, found: &Found<'_>, address_of: &dyn Fn(&str) -> Option<String>) -> bool {
    match resolve::parse(&d.endpoint) {
        Ok(resolve::Endpoint::Identity(Identity::Mac(m))) => found.mac == Some(m),
        Ok(resolve::Endpoint::Identity(Identity::Cip {
            vendor,
            product,
            serial,
        })) => found
            .cip
            .is_some_and(|(v, p, s)| vendor == v && product == p && serial.is_none_or(|want| want == s)),
        Ok(resolve::Endpoint::Identity(Identity::Modbus { vendor, product })) => {
            found.modbus.is_some_and(|(v, p)| {
                resolve::modbus_text_eq(&vendor, v) && resolve::modbus_text_eq(&product, p)
            })
        }
        Ok(resolve::Endpoint::Host(h)) => {
            let host = h.rsplit_once(':').map_or(h.as_str(), |(a, _)| a);
            host.parse::<Ipv4Addr>() == Ok(found.address)
                || address_of(&d.name).is_some_and(|a| {
                    a.rsplit_once(':')
                        .map_or(a.as_str(), |(x, _)| x)
                        .parse::<Ipv4Addr>()
                        == Ok(found.address)
                })
        }
        Err(_) => false,
    }
}

fn modbus_json(s: &ModbusSeen, mapped: Option<&str>, library: Option<&Entry>) -> Json {
    json!({
        "address": s.address.to_string(),
        "mac": s.mac_text().map(|m| m.trim_start_matches("mac=").to_string()),
        "vendor_name": s.identification.vendor_name,
        "product_code": s.identification.product_code,
        "revision": s.identification.revision,
        "identity": s.identity().to_string(),
        "mapped": mapped,
        "library": library.map(|e| entry_json(e, "modbus-tcp")),
    })
}

/// A library entry, with the protocol for `driver` (or its first).
fn entry_json(e: &Entry, driver: &str) -> Json {
    let p = e.protocol(driver).or_else(|| e.protocols.first());
    json!({
        "folder": e.folder, "vendor": e.vendor, "model": e.model, "kind": e.kind,
        "driver": p.map(|p| p.driver.clone()),
        "profile": p.and_then(|p| p.profile.clone()),
        "map": p.and_then(|p| p.map.clone()),
        "eds": p.and_then(|p| p.eds.as_ref()).map(|f| json!({ "name": f.name, "size": f.size, "sha256": f.sha256 })),
    })
}

fn seen_json(s: &Seen, mapped: Option<&str>, library: Option<&Entry>) -> Json {
    json!({
        "address": s.address.to_string(),
        "mac": s.mac_text().map(|m| m.trim_start_matches("mac=").to_string()),
        "vendor_id": s.vendor_id, "product_code": s.product_code, "device_type": s.device_type,
        "revision": format!("{}.{}", s.revision.0, s.revision.1),
        "serial": format!("{:#010x}", s.serial),
        "product_name": s.product_name,
        "cip": s.cip(),
        "mapped": mapped,
        "library": library.map(|e| entry_json(e, "enip-io")),
    })
}

/// How long a PROFINET DCP Identify listens at least: stations spread
/// their answers over up to a second.
#[cfg(feature = "profinet-io")]
const PROFINET_WINDOW: Duration = Duration::from_millis(1500);

/// One PROFINET DCP Identify on `interface`, or why there was none.
#[cfg(feature = "profinet-io")]
fn profinet_probe(interface: Option<&str>, window: Duration) -> Result<Vec<ProfinetSeen>, String> {
    let Some(interface) = interface else {
        return Err(
            "no network interface is known (a profinet-io device line in the map names one with if=)".into(),
        );
    };
    resolve::probe_profinet(interface, window.max(PROFINET_WINDOW)).map_err(|e| e.to_string())
}

/// Without `profinet-io` the build cannot send the frame.
#[cfg(not(feature = "profinet-io"))]
fn profinet_probe(_interface: Option<&str>, _window: Duration) -> Result<Vec<ProfinetSeen>, String> {
    Err("this build cannot send raw Ethernet frames (it is built without profinet-io)".into())
}

/// The interface the running map's PROFINET devices name (`if=`), the
/// first one; the simulated ones (`if=sim`) are on none.
fn profinet_interface(devices: &[iomap::MapDevice]) -> Option<String> {
    devices
        .iter()
        .filter(|d| d.driver == "profinet-io")
        .find_map(|d| {
            d.params
                .iter()
                .find(|(k, v)| k == "if" && v != "sim")
                .map(|(_, v)| v.clone())
        })
}

/// What a scan says about one protocol: looked for, or not and why.
fn protocol_json(protocol: &str, looked: Result<(), String>) -> Json {
    match looked {
        Ok(()) => json!({ "protocol": protocol, "looked": true }),
        Err(why) => json!({ "protocol": protocol, "looked": false, "why": why }),
    }
}

/// One scan, one at a time: the sweep to `target`, the Modbus probe when
/// the box has one and the PROFINET Identify beside it (the longest
/// bound, the probe's `ModbusProbe::total` plus one host's deadline, is
/// then the wait). A probe that fails is a protocol not looked for, with
/// the reason; only the sweep failing fails the scan. Kept as the last
/// result.
///
/// # Errors
/// The status, the reason and the audit line: 409 while another scan
/// runs, 502 when the sweep failed.
fn scan(
    st: &State,
    target: SocketAddrV4,
    interface: Option<&str>,
) -> Result<Scan, (u16, String, &'static str)> {
    let Ok(_one) = st.running.try_lock() else {
        return Err((
            409,
            "a scan is already running; ask again shortly".into(),
            "discover: busy",
        ));
    };
    let r = st.cfg.resolver(Some(target));
    let (swept, probed, pn) = std::thread::scope(|s| {
        let probe = st
            .cfg
            .modbus
            .is_some()
            .then(|| s.spawn(|| resolve::probe_modbus(&r)));
        let pn = s.spawn(|| profinet_probe(interface, st.cfg.window));
        let swept = resolve::discover(&r);
        let probed = probe.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err(std::io::Error::other("the probe failed")))
        });
        let pn = pn.join().unwrap_or_else(|_| Err("the Identify failed".into()));
        (swept, probed, pn)
    });
    let seen = swept.map_err(|e| (502, format!("the scan failed: {e}"), "discover: failed"))?;
    let (modbus, modbus_says) = match probed {
        Some(Ok(v)) => (Some(v), Ok(())),
        Some(Err(e)) => (
            None,
            Err(format!("Modbus was not looked for: the probe failed: {e}")),
        ),
        None => (
            None,
            Err("Modbus was not looked for: this box does not probe for Modbus devices".into()),
        ),
    };
    let (profinet, profinet_says) = match pn {
        Ok(v) => (Some(v), Ok(())),
        Err(e) => (None, Err(format!("PROFINET was not looked for: {e}"))),
    };
    let scan = Scan {
        at: Instant::now(),
        seen,
        modbus,
        profinet,
        protocols: vec![
            protocol_json("ethernet-ip", Ok(())),
            protocol_json("modbus-tcp", modbus_says),
            protocol_json("profinet", profinet_says),
        ],
    };
    *st.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(scan.clone());
    Ok(scan)
}

/// One protocol's answer from a scan.
#[derive(Clone, Copy)]
enum Item<'a> {
    Enip(&'a Seen),
    Modbus(&'a ModbusSeen),
    Profinet(&'a ProfinetSeen),
}

impl Item<'_> {
    fn mac(self) -> Option<[u8; 6]> {
        match self {
            Item::Enip(s) => s.mac,
            Item::Modbus(s) => s.mac,
            Item::Profinet(s) => Some(s.mac),
        }
    }

    fn address(self) -> Option<Ipv4Addr> {
        match self {
            Item::Enip(s) => Some(s.address),
            Item::Modbus(s) => Some(s.address),
            Item::Profinet(s) => s.address,
        }
    }

    fn json(self) -> Json {
        match self {
            Item::Enip(s) => json!({
                "protocol": "ethernet-ip",
                "address": s.address.to_string(),
                "vendor_id": s.vendor_id, "product_code": s.product_code,
                "device_type": s.device_type,
                "revision": format!("{}.{}", s.revision.0, s.revision.1),
                "serial": format!("{:#010x}", s.serial),
                "product_name": s.product_name,
                "identity": s.cip(),
            }),
            Item::Modbus(s) => json!({
                "protocol": "modbus-tcp",
                "address": s.address.to_string(),
                "vendor_name": s.identification.vendor_name,
                "product_code": s.identification.product_code,
                "revision": s.identification.revision,
                "identity": s.identity().to_string(),
            }),
            Item::Profinet(s) => json!({
                "protocol": "profinet",
                "address": s.address.map(|a| a.to_string()),
                "name": s.name,
                "vendor_id": s.vendor_id, "device_id": s.device_id,
                "type_of_station": s.type_of_station,
                "identity": s.mac_text(),
            }),
        }
    }
}

/// The answers of a scan grouped into devices: answers that share a MAC
/// are one device; one without a MAC joins the device at its address, or
/// is a device of its own keyed by that address. Each group keeps the
/// order EtherNet/IP, Modbus, PROFINET; the groups are in address order
/// (a device without an address last), then MAC.
fn group<'a>(seen: &'a [Seen], modbus: &'a [ModbusSeen], profinet: &'a [ProfinetSeen]) -> Vec<Vec<Item<'a>>> {
    let mut items: Vec<Item<'a>> = seen.iter().map(Item::Enip).collect();
    items.extend(modbus.iter().map(Item::Modbus));
    items.extend(profinet.iter().map(Item::Profinet));
    // With a MAC first, so one without joins the device it belongs to.
    items.sort_by_key(|i| i.mac().is_none());
    let mut groups: Vec<Vec<Item<'a>>> = Vec::new();
    for it in items {
        let at = groups.iter().position(|g| match it.mac() {
            Some(m) => g.iter().any(|x| x.mac() == Some(m)),
            None => it.address().is_some() && g.iter().any(|x| x.address() == it.address()),
        });
        match at {
            Some(i) => groups[i].push(it),
            None => groups.push(vec![it]),
        }
    }
    for g in &mut groups {
        g.sort_by_key(|i| match i {
            Item::Enip(_) => 0,
            Item::Modbus(_) => 1,
            Item::Profinet(_) => 2,
        });
    }
    groups.sort_by_key(|g| {
        (
            g.iter().find_map(|i| i.address()).is_none(),
            g.iter().find_map(|i| i.address()),
            g.iter().find_map(|i| i.mac()),
        )
    });
    groups
}

/// One device of the answer's `found`.
fn found_json(
    g: &[Item<'_>],
    map: &[iomap::MapDevice],
    address_of: &dyn Fn(&str) -> Option<String>,
    index: Option<&Index>,
) -> Json {
    let address = g.iter().find_map(|i| i.address());
    let mac = g.iter().find_map(|i| i.mac());
    let found = Found {
        address: address.unwrap_or(Ipv4Addr::UNSPECIFIED),
        mac,
        cip: g.iter().find_map(|i| match i {
            Item::Enip(s) => Some((s.vendor_id, s.product_code, s.serial)),
            _ => None,
        }),
        modbus: g.iter().find_map(|i| match i {
            Item::Modbus(s) => Some((
                s.identification.vendor_name.as_str(),
                s.identification.product_code.as_str(),
            )),
            _ => None,
        }),
    };
    let mapped = map
        .iter()
        .find(|d| binds(d, &found, address_of))
        .map(|d| d.name.as_str());
    let library = index.and_then(|ix| {
        g.iter().find_map(|i| match i {
            Item::Enip(s) => ix
                .lookup(s.vendor_id, s.product_code, &s.product_name)
                .map(|e| entry_json(e, "enip-io")),
            Item::Modbus(s) => ix
                .lookup_modbus(&s.identification.vendor_name, &s.identification.product_code)
                .map(|e| entry_json(e, "modbus-tcp")),
            Item::Profinet(_) => None,
        })
    });
    json!({
        "mac": mac.map(|m| Identity::Mac(m).to_string().trim_start_matches("mac=").to_string()),
        "address": address.map(|a| a.to_string()),
        "identities": g.iter().map(|i| i.json()).collect::<Vec<_>>(),
        "mapped": mapped,
        "library": library,
    })
}

/// `POST /api/devices/discover`.
pub(super) fn post(req: &Request, ctx: &Ctx) -> Response {
    let refused = |status: u16, why: &str, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::error(status, why),
        None => audit_full(),
    };
    let st = &ctx.discovery;
    let Some(target) = st.cfg.sweep else {
        return refused(501, "this box does not scan its network", "discover: off");
    };
    // `?modbus=1` is accepted and ignored: the probe always runs now.
    let fresh = req.query.iter().any(|(k, v)| k == "fresh" && v == "1");
    let map = ctx
        .running()
        .and_then(|cs| cs.io_map)
        .map(|t| iomap::skim(&t))
        .unwrap_or_default();
    let cached = st
        .last
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .filter(|c| !fresh && c.at.elapsed() < FRESH);
    let Scan {
        at,
        seen,
        modbus,
        profinet,
        protocols,
    } = if let Some(c) = cached {
        c
    } else {
        match scan(st, target, profinet_interface(&map.devices).as_deref()) {
            Ok(s) => s,
            Err((status, why, detail)) => return refused(status, &why, detail),
        }
    };
    let address_of = |name: &str| ctx.box_.devices.address(name);
    let index = st.index();
    let devices: Vec<Json> = seen
        .iter()
        .map(|s| {
            let found = Found::from(s);
            let mapped = map
                .devices
                .iter()
                .find(|d| binds(d, &found, &address_of))
                .map(|d| d.name.as_str());
            let hit = index
                .as_ref()
                .ok()
                .and_then(|i| i.lookup(s.vendor_id, s.product_code, &s.product_name));
            seen_json(s, mapped, hit)
        })
        .collect();
    let modbus_list = modbus.as_deref().unwrap_or_default();
    let pn_list = profinet.as_deref().unwrap_or_default();
    let found: Vec<Json> = group(&seen, modbus_list, pn_list)
        .iter()
        .map(|g| found_json(g, &map.devices, &address_of, index.as_ref().ok()))
        .collect();
    let mut body = json!({
        "found": found,
        "protocols": protocols,
        "devices": devices,
        "age_ms": u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX),
        "library": match &index { Ok(i) => json!({ "devices": i.devices.len() }), Err(e) => json!({ "unavailable": e }) },
    });
    if let Some(list) = &modbus {
        body["modbus"] = Json::Array(
            list.iter()
                .map(|s| {
                    let f = Found::from(s);
                    let mapped = map
                        .devices
                        .iter()
                        .find(|d| binds(d, &f, &address_of))
                        .map(|d| d.name.as_str());
                    let hit = index.as_ref().ok().and_then(|i| {
                        i.lookup_modbus(&s.identification.vendor_name, &s.identification.product_code)
                    });
                    modbus_json(s, mapped, hit)
                })
                .collect(),
        );
    }
    let detail = format!(
        "discover: {} device(s), {} EtherNet/IP, {} Modbus (FC 43/14), {} PROFINET, read-only",
        found.len(),
        seen.len(),
        modbus_list.len(),
        pn_list.len()
    );
    match audit(ctx, req, 200, &detail) {
        Some(_) => Response::json(200, &body),
        None => audit_full(),
    }
}

/// `GET /api/devices/library`.
pub(super) fn library(req: &Request, ctx: &Ctx) -> Response {
    let st = &ctx.discovery;
    let q = |k: &str| req.query.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let index = match st.index() {
        Ok(i) => i,
        Err(e) => return Response::json(200, &json!({ "matches": [], "unavailable": e })),
    };
    let address = q("address").filter(|a| a.parse::<Ipv4Addr>().is_ok());
    let matches: Vec<&Entry> = match (
        q("vendor").and_then(|v| v.parse::<u16>().ok()),
        q("product").and_then(|p| p.parse::<u16>().ok()),
        q("q"),
    ) {
        (Some(v), Some(p), _) => index.by_identity(v, p),
        (_, _, Some(text)) if text.chars().count() <= 120 => index.search(text),
        _ => return Response::error(400, "give vendor and product (numbers), or q (text)"),
    };
    let base = st.base().unwrap_or_default();
    let out: Vec<Json> = matches
        .iter()
        .take(20)
        .map(|e| {
            let mut j = entry_json(e, "enip-io");
            let p = e.protocol("enip-io").or_else(|| e.protocols.first());
            if let Some(file) = p.and_then(|p| p.map.as_deref()) {
                let url = e.file_url(&base, file);
                match st.map_text(&url) {
                    Ok(text) => {
                        let lines = address.map_or(text.clone(), |a| devlib::with_address(&text, a));
                        j["lines"] = json!(lines);
                    }
                    Err(why) => j["lines_unavailable"] = json!(why),
                }
            }
            j
        })
        .collect();
    Response::json(200, &json!({ "matches": out }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use helmsman_io::modbus::DeviceIdentification;

    const A: [u8; 6] = [0, 0x90, 0xe8, 0, 0, 0xa];
    const B: [u8; 6] = [0, 0x90, 0xe8, 0, 0, 0xb];

    fn enip(last: u8, mac: Option<[u8; 6]>) -> Seen {
        Seen {
            address: Ipv4Addr::new(10, 0, 0, last),
            mac,
            vendor_id: 991,
            product_code: 515,
            device_type: 7,
            revision: (3, 1),
            serial: u32::from(last),
            product_name: "E1213".into(),
        }
    }

    fn modbus(last: u8, mac: Option<[u8; 6]>) -> ModbusSeen {
        ModbusSeen {
            address: Ipv4Addr::new(10, 0, 0, last),
            mac,
            identification: DeviceIdentification {
                vendor_name: "Moxa".into(),
                product_code: "E1213".into(),
                revision: "V3.1".into(),
                conformity: 1,
            },
        }
    }

    fn pn(last: Option<u8>, mac: [u8; 6]) -> ProfinetSeen {
        ProfinetSeen {
            mac,
            address: last.map(|l| Ipv4Addr::new(10, 0, 0, l)),
            name: "rio".into(),
            vendor_id: 1,
            device_id: 2,
            type_of_station: "t".into(),
        }
    }

    fn shape(g: &[Vec<Item<'_>>]) -> Vec<Vec<&'static str>> {
        g.iter()
            .map(|g| {
                g.iter()
                    .map(|i| match i {
                        Item::Enip(_) => "enip",
                        Item::Modbus(_) => "modbus",
                        Item::Profinet(_) => "profinet",
                    })
                    .collect()
            })
            .collect()
    }

    /// Answers that share a MAC are one device; two MACs are two devices.
    #[test]
    fn a_shared_mac_is_one_device_and_two_macs_are_two() {
        let e = [enip(9, Some(B)), enip(5, Some(A))];
        let m = [modbus(5, Some(A)), modbus(9, Some(B))];
        let p = [pn(Some(5), A)];
        let g = group(&e, &m, &p);
        assert_eq!(
            shape(&g),
            [vec!["enip", "modbus", "profinet"], vec!["enip", "modbus"]]
        );
        // Address order: .5 (A) before .9 (B).
        assert_eq!(g[0][0].address(), Some(Ipv4Addr::new(10, 0, 0, 5)));
        assert_eq!(g[1][0].mac(), Some(B));
    }

    /// Different MACs on the same pair of protocols never merge, and an
    /// answer without a MAC is keyed by its address: it joins the device
    /// at that address and otherwise stands alone.
    #[test]
    fn an_answer_without_a_mac_is_keyed_by_its_address() {
        let e = [enip(5, Some(A)), enip(7, None)];
        let m = [modbus(5, None), modbus(7, None), modbus(8, None)];
        let g = group(&e, &m, &[]);
        assert_eq!(
            shape(&g),
            [vec!["enip", "modbus"], vec!["enip", "modbus"], vec!["modbus"]]
        );
        assert_eq!(g[0][0].mac(), Some(A));
        assert_eq!(g[1][0].mac(), None);
        // The same address with two different MACs is two devices.
        let e = [enip(5, Some(A)), enip(5, Some(B))];
        assert_eq!(group(&e, &[], &[]).len(), 2);
    }

    /// A PROFINET station that has no IP address yet is still a device
    /// (last), and the answer has nothing for it to hide.
    #[test]
    fn a_station_without_an_address_is_listed_last() {
        let e = [enip(5, Some(A))];
        let p = [pn(None, B)];
        let g = group(&e, &[], &p);
        assert_eq!(shape(&g), [vec!["enip"], vec!["profinet"]]);
        assert_eq!(g[1][0].json()["address"], Json::Null);
    }

    #[test]
    fn the_interface_comes_from_a_real_profinet_device_line() {
        let dev = |driver: &str, params: &[(&str, &str)]| iomap::MapDevice {
            name: "d".into(),
            driver: driver.into(),
            endpoint: "x".into(),
            params: params.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
            settings: Vec::new(),
        };
        assert_eq!(profinet_interface(&[]), None);
        assert_eq!(profinet_interface(&[dev("enip-io", &[("if", "en0")])]), None);
        assert_eq!(profinet_interface(&[dev("profinet-io", &[("if", "sim")])]), None);
        assert_eq!(
            profinet_interface(&[
                dev("profinet-io", &[("if", "sim")]),
                dev("profinet-io", &[("if", "en7")])
            ]),
            Some("en7".into())
        );
    }
}
