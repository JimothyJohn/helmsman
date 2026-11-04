//! The Devices tab's "On the network" and the device library
//! (`devices-repo`, interview 2026-11-02).
//!
//! - `POST /api/devices/discover` (token or session): one `ListIdentity`
//!   broadcast ([`helmsman_io::resolve::discover`]) and the host's ARP
//!   table read after it; every EtherNet/IP device that answered, with
//!   its address, MAC when known, identity, product name, whether a device
//!   of the running map already binds it, and the library's match when
//!   there is one. Read-only on the network: the one request is all that
//!   is sent. One sweep runs at a time (a second asker gets 409), and the
//!   last result is answered from memory for [`FRESH`] unless `?fresh=1`.
//!   With `?modbus=1` the Modbus identification probe runs beside the
//!   sweep ([`helmsman_io::resolve::probe_modbus`]: Read Device
//!   Identification, FC 43/14, to TCP 502 of the hosts in the ARP table
//!   and no others, bounded in hosts, concurrency and time; read-only), and
//!   the answer gains `modbus`: each device that answered, with its
//!   address, MAC, `VendorName`, `ProductCode`, revision, the
//!   `modbus=VENDOR:PRODUCT` a map may name it by, whether the running map
//!   binds it, and the library's match by `identity.modbus`.
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

use helmsman_io::resolve::{self, Identity, ModbusProbe, ModbusSeen, Resolver, Seen};
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
    /// The Modbus identification probe `?modbus=1` runs; `None`: none
    /// (`?modbus=1` answers 501).
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

/// One scan's result: the `ListIdentity` answers and, when it was asked
/// for, the Modbus probe's.
#[derive(Clone)]
struct Scan {
    at: Instant,
    seen: Vec<Seen>,
    modbus: Option<Vec<ModbusSeen>>,
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

/// One scan, one at a time: the sweep to `target` and, when
/// `want_modbus`, the Modbus probe beside it (the probe's bound,
/// `ModbusProbe::total` plus one host's deadline, is then the wait). Kept
/// as the last result.
///
/// # Errors
/// The status, the reason and the audit line: 409 while another scan
/// runs, 502 when the sweep or the probe failed.
fn scan(st: &State, target: SocketAddrV4, want_modbus: bool) -> Result<Scan, (u16, String, &'static str)> {
    let Ok(_one) = st.running.try_lock() else {
        return Err((
            409,
            "a scan is already running; ask again shortly".into(),
            "discover: busy",
        ));
    };
    let r = st.cfg.resolver(Some(target));
    let (swept, probed) = std::thread::scope(|s| {
        let probe = want_modbus.then(|| s.spawn(|| resolve::probe_modbus(&r)));
        let swept = resolve::discover(&r);
        let probed = probe.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err(std::io::Error::other("the probe failed")))
        });
        (swept, probed)
    });
    let seen = swept.map_err(|e| (502, format!("the scan failed: {e}"), "discover: failed"))?;
    let modbus = probed.transpose().map_err(|e| {
        (
            502,
            format!("the Modbus probe failed: {e}"),
            "discover: modbus failed",
        )
    })?;
    let scan = Scan {
        at: Instant::now(),
        seen,
        modbus,
    };
    *st.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(scan.clone());
    Ok(scan)
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
    let flag = |name: &str| req.query.iter().any(|(k, v)| k == name && v == "1");
    let fresh = flag("fresh");
    let want_modbus = flag("modbus");
    if want_modbus && st.cfg.modbus.is_none() {
        return refused(
            501,
            "this box does not probe for Modbus devices",
            "discover: modbus off",
        );
    }
    let cached = st
        .last
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .filter(|c| !fresh && c.at.elapsed() < FRESH && (!want_modbus || c.modbus.is_some()));
    let Scan { at, seen, modbus } = if let Some(c) = cached {
        c
    } else {
        match scan(st, target, want_modbus) {
            Ok(s) => s,
            Err((status, why, detail)) => return refused(status, &why, detail),
        }
    };
    let map = ctx
        .running()
        .and_then(|cs| cs.io_map)
        .map(|t| iomap::skim(&t))
        .unwrap_or_default();
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
    let mut body = json!({
        "devices": devices,
        "age_ms": u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX),
        "library": match &index { Ok(i) => json!({ "devices": i.devices.len() }), Err(e) => json!({ "unavailable": e }) },
    });
    let mut detail = format!("discover: {} device(s), read-only", seen.len());
    if want_modbus && let Some(found) = &modbus {
        let list: Vec<Json> = found
            .iter()
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
            .collect();
        body["modbus"] = Json::Array(list);
        detail = format!(
            "discover: {} device(s) and {} Modbus (FC 43/14), read-only",
            seen.len(),
            found.len()
        );
    }
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
