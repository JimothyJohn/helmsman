//! The public device library's index (`devices-repo`, interview
//! 2026-11-02): `index.json` at the root of
//! <https://github.com/JimothyJohn/helmsman-devices>, built from every
//! `device.toml` there by its `scripts/build-index.py`. A box fetches it
//! and a browser's simulation fetches it, both through this: parse the
//! file, match a device found on the network by identity first (an
//! EtherNet/IP vendor id and product code, or a Modbus device's Read
//! Device Identification `VendorName` and `ProductCode`), then by vendor and
//! model text.
//! Semantic search is for later, once there are entries enough to need it.
//!
//! ```json
//! { "format": "helmsman-devices-index 1",
//!   "devices": [ { "folder": "devices/moxa/iologik-e1213",
//!                  "vendor": "Moxa", "model": "ioLogik E1213", "kind": "remote-io",
//!                  "profile": "moxa-iologik-e1213.device",
//!                  "identity": { "enip": { "vendor_id": 991, "product_code": 515 },
//!                                "modbus": { "vendor_name": "Moxa", "product_code": "E1213" } },
//!                  "protocols": { "enip": { "driver": "enip-io", "map": "enip.map",
//!                                           "profile": "moxa-e1213",
//!                                           "eds": { "name": "...", "size": 42906, "sha256": "..." } },
//!                                 "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } } ] }
//! ```
//!
//! `profile` names the device's profile file in the folder (`NAME.device`,
//! `helmsman_io::profile`; PROTOCOLS.md step 3): a box copies it into the
//! project when the device is added by click. The older `protocols.*.profile`
//! is a driver's built-in profile name, another thing.
//!
//! A sensor or an actuator that sits on a plain digital channel has a
//! record too (`kind` `sensor` or `actuator`, drafted there from its
//! IODD): no `protocols`, an `identity.iolink` (its IO-Link vendor and
//! device id) and `products`, the order numbers the one record covers.
//! An I/O map line names such a record with `wired=VENDOR/MODEL`
//! (`helmsman_io::text`), and [`Index::by_wired`] finds it.
//!
//! Everything read here is data from a public repository: names are
//! bounded and checked, numbers must be numbers, a folder is relative,
//! under `devices/`, with no `..`, and an index over [`MAX_BYTES`] or
//! [`MAX_DEVICES`] is refused rather than trusted.

use serde_json::Value as Json;

/// Where the library's index is, unless a site hosts its own.
pub const DEFAULT_URL: &str =
    "https://raw.githubusercontent.com/JimothyJohn/helmsman-devices/main/index.json";
/// The format line an index must carry.
pub const FORMAT: &str = "helmsman-devices-index 1";
/// Largest index read, bytes.
pub const MAX_BYTES: usize = 4 << 20;
/// Most devices in one index.
pub const MAX_DEVICES: usize = 10_000;
/// Longest text field kept (vendor, model, kind, driver, map, profile, folder).
pub const MAX_TEXT: usize = 120;
/// Most order numbers one record lists.
pub const MAX_PRODUCTS: usize = 128;

/// A vendor's file the entry points at, by reference only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// One protocol a device speaks, as the library records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protocol {
    /// The Helmsman driver (`enip-io`, `modbus-tcp`, `logix-tag`).
    pub driver: String,
    /// The map lines file in the folder (`enip.map`), with `ADDR` to fill in.
    pub map: Option<String>,
    /// A built-in profile, when Helmsman has one.
    pub profile: Option<String>,
    pub eds: Option<FileRef>,
}

/// One device folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `devices/<vendor>/<model>`, relative, no `..`.
    pub folder: String,
    pub vendor: String,
    pub model: String,
    pub kind: String,
    /// `ListIdentity`'s vendor id and product code, when the device speaks EtherNet/IP.
    pub enip: Option<(u16, u16)>,
    /// Read Device Identification's `VendorName` and `ProductCode`
    /// (`identity.modbus` in device.toml), when the device answers it.
    pub modbus: Option<(String, String)>,
    pub protocols: Vec<Protocol>,
    /// The device's profile file in the folder (`NAME.device`), when the
    /// library has one.
    pub profile: Option<String>,
    /// A sensor's or actuator's IO-Link vendor id and device id
    /// (`identity.iolink`): what its IODD is found by.
    pub iolink: Option<(u16, u32)>,
    /// The order numbers the record covers, as on the label.
    pub products: Vec<String>,
    /// The folder has a `record.json` ([`crate::devrecord`]): a wired
    /// sensor's or actuator's detail.
    pub record: bool,
}

impl Entry {
    /// Whether a controller reaches the device over a network. A sensor
    /// on a digital channel does not: it has no protocol.
    #[must_use]
    pub fn networked(&self) -> bool {
        !self.protocols.is_empty()
    }

    /// The `wired=` value that names this record: its folder without
    /// `devices/`.
    #[must_use]
    pub fn wired(&self) -> &str {
        self.folder.strip_prefix("devices/").unwrap_or(&self.folder)
    }

    /// The protocol entry for `driver`.
    #[must_use]
    pub fn protocol(&self, driver: &str) -> Option<&Protocol> {
        self.protocols.iter().find(|p| p.driver == driver)
    }

    /// The URL of `file` in this folder under `base` (the index's URL
    /// without its file name).
    #[must_use]
    pub fn file_url(&self, base: &str, file: &str) -> String {
        format!("{}/{}/{}", base.trim_end_matches('/'), self.folder, file)
    }
}

/// The parsed index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    pub devices: Vec<Entry>,
}

fn text(j: &Json, key: &str) -> Result<String, String> {
    let Some(v) = j.get(key) else {
        return Ok(String::new());
    };
    let s = v.as_str().ok_or_else(|| format!("{key}: not text"))?;
    if s.chars().count() > MAX_TEXT || s.chars().any(char::is_control) {
        return Err(format!("{key}: too long or has control characters"));
    }
    Ok(s.to_string())
}

fn folder_ok(f: &str) -> bool {
    f.starts_with("devices/")
        && f.len() <= MAX_TEXT
        && f.split('/').all(|seg| {
            !seg.is_empty()
                && seg != ".."
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        })
        && f.split('/').count() == 3
}

fn file_ok(f: &str) -> bool {
    !f.is_empty()
        && f.len() <= MAX_TEXT
        && !f.contains('/')
        && f != ".."
        && f.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn u16_of(j: &Json, key: &str) -> Result<u16, String> {
    j.get(key)
        .and_then(Json::as_u64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| format!("{key}: not a number 0..=65535"))
}

fn protocol(name: &str, j: &Json) -> Result<Protocol, String> {
    let driver = text(j, "driver")?;
    if !matches!(driver.as_str(), "enip-io" | "modbus-tcp" | "logix-tag") {
        return Err(format!(
            "protocols.{name}.driver: not a Helmsman driver: {driver:?}"
        ));
    }
    let map = text(j, "map")?;
    if !map.is_empty() && !file_ok(&map) {
        return Err(format!("protocols.{name}.map: not a file name"));
    }
    let profile = text(j, "profile")?;
    let eds = match j.get("eds") {
        None => None,
        Some(e) => {
            let fname = text(e, "name")?;
            if !file_ok(&fname) {
                return Err(format!("protocols.{name}.eds.name: not a file name"));
            }
            let sha = text(e, "sha256")?;
            if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("protocols.{name}.eds.sha256: not 64 hex digits"));
            }
            Some(FileRef {
                name: fname,
                size: e.get("size").and_then(Json::as_u64).unwrap_or(0),
                sha256: sha.to_ascii_lowercase(),
            })
        }
    };
    Ok(Protocol {
        driver,
        map: (!map.is_empty()).then_some(map),
        profile: (!profile.is_empty()).then_some(profile),
        eds,
    })
}

fn entry(j: &Json) -> Result<Entry, String> {
    let folder = text(j, "folder")?;
    if !folder_ok(&folder) {
        return Err(format!("folder: not devices/<vendor>/<model>: {folder:?}"));
    }
    let vendor = text(j, "vendor")?;
    let model = text(j, "model")?;
    if vendor.is_empty() || model.is_empty() {
        return Err(format!("{folder}: vendor and model are required"));
    }
    let enip = match j.get("identity").and_then(|i| i.get("enip")) {
        None => None,
        Some(e) => Some((u16_of(e, "vendor_id")?, u16_of(e, "product_code")?)),
    };
    let modbus = match j.get("identity").and_then(|i| i.get("modbus")) {
        None => None,
        Some(m) => {
            let (v, p) = (text(m, "vendor_name")?, text(m, "product_code")?);
            if v.trim().is_empty() || p.trim().is_empty() {
                return Err(format!(
                    "{folder}: identity.modbus needs vendor_name and product_code"
                ));
            }
            Some((v, p))
        }
    };
    let mut protocols = Vec::new();
    if let Some(p) = j.get("protocols") {
        let obj = p.as_object().ok_or("protocols: not an object")?;
        for (name, v) in obj {
            protocols.push(protocol(name, v)?);
        }
    }
    let profile = text(j, "profile")?;
    if !profile.is_empty() && crate::file_name(&profile) != Some(crate::FileKind::Device) {
        return Err(format!(
            "profile: not a profile file name (NAME.device, NAME 1 to {} of a-z 0-9 _ -)",
            crate::NAME_LEN
        ));
    }
    let iolink = match j.get("identity").and_then(|i| i.get("iolink")) {
        None => None,
        Some(l) => {
            let vendor = u16_of(l, "vendor_id")?;
            let device = l
                .get("device_id")
                .and_then(Json::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=0xFF_FFFF).contains(n))
                .ok_or("device_id: not a number 1..=16777215")?;
            if vendor == 0 {
                return Err(format!("{folder}: identity.iolink.vendor_id: 1..=65535"));
            }
            Some((vendor, device))
        }
    };
    let mut products = Vec::new();
    if let Some(p) = j.get("products") {
        let list = p.as_array().ok_or("products: not a list")?;
        if list.len() > MAX_PRODUCTS {
            return Err(format!(
                "{folder}: {} products, more than {MAX_PRODUCTS}",
                list.len()
            ));
        }
        for item in list {
            let name = item.as_str().ok_or("products: not text")?;
            if name.is_empty() || name.len() > MAX_TEXT || name.chars().any(char::is_control) {
                return Err(format!("{folder}: products: a printable text of 1..={MAX_TEXT}"));
            }
            products.push(name.to_string());
        }
    }
    let record = match j.get("record") {
        None => false,
        Some(r) if r.as_str() == Some(crate::devrecord::FILE) => true,
        Some(_) => return Err(format!("{folder}: record: not {}", crate::devrecord::FILE)),
    };
    Ok(Entry {
        folder,
        vendor,
        model,
        kind: text(j, "kind")?,
        enip,
        modbus,
        protocols,
        profile: (!profile.is_empty()).then_some(profile),
        iolink,
        products,
        record,
    })
}

impl Index {
    /// Parses an index. Refuses the wrong format, anything over the
    /// limits, and any entry that does not read; one bad entry fails the
    /// whole index, so a box never half-trusts it.
    ///
    /// # Errors
    /// Why, in words.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_BYTES {
            return Err(format!(
                "the index is {} bytes, more than {MAX_BYTES}",
                text.len()
            ));
        }
        let j: Json = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        if j.get("format").and_then(Json::as_str) != Some(FORMAT) {
            return Err(format!("not a {FORMAT:?} file"));
        }
        let list = j
            .get("devices")
            .and_then(Json::as_array)
            .ok_or("devices: not a list")?;
        if list.len() > MAX_DEVICES {
            return Err(format!("{} devices, more than {MAX_DEVICES}", list.len()));
        }
        let mut devices = Vec::with_capacity(list.len());
        for (k, d) in list.iter().enumerate() {
            let e = entry(d).map_err(|e| format!("devices[{k}]: {e}"))?;
            if devices.iter().any(|x: &Entry| x.folder == e.folder) {
                return Err(format!("devices[{k}]: folder {} twice", e.folder));
            }
            devices.push(e);
        }
        Ok(Self { devices })
    }

    /// The entries whose EtherNet/IP identity is `vendor_id`,
    /// `product_code`: the first thing a lookup tries.
    #[must_use]
    pub fn by_identity(&self, vendor_id: u16, product_code: u16) -> Vec<&Entry> {
        self.devices
            .iter()
            .filter(|e| e.enip == Some((vendor_id, product_code)))
            .collect()
    }

    /// The entries whose Modbus identity (`identity.modbus`) is
    /// `vendor_name`, `product_code`, trimmed and case-blind as a
    /// `modbus=` endpoint matches.
    #[must_use]
    pub fn by_modbus(&self, vendor_name: &str, product_code: &str) -> Vec<&Entry> {
        let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
        self.devices
            .iter()
            .filter(|e| {
                e.modbus
                    .as_ref()
                    .is_some_and(|(v, p)| eq(v, vendor_name) && eq(p, product_code))
            })
            .collect()
    }

    /// A device found by the Modbus identification probe: its identity
    /// first, then vendor and product code as text.
    #[must_use]
    pub fn lookup_modbus(&self, vendor_name: &str, product_code: &str) -> Option<&Entry> {
        self.by_modbus(vendor_name, product_code)
            .into_iter()
            .next()
            .or_else(|| {
                self.search(&format!("{vendor_name} {product_code}"))
                    .into_iter()
                    .find(|e| e.networked())
            })
    }

    /// The record a map line's `wired=VENDOR/MODEL` names.
    #[must_use]
    pub fn by_wired(&self, wired: &str) -> Option<&Entry> {
        self.devices.iter().find(|e| e.wired() == wired)
    }

    /// The records for the IO-Link device `vendor_id`, `device_id`.
    #[must_use]
    pub fn by_iolink(&self, vendor_id: u16, device_id: u32) -> Vec<&Entry> {
        self.devices
            .iter()
            .filter(|e| e.iolink == Some((vendor_id, device_id)))
            .collect()
    }

    /// The entries whose vendor, model and order numbers together
    /// contain every word of `query`, case-blind: the second thing a
    /// lookup tries, and how a sensor is found by the number on its
    /// label. An empty query matches nothing.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let words: Vec<String> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        self.devices
            .iter()
            .filter(|e| {
                let hay = format!("{} {} {}", e.vendor, e.model, e.products.join(" ")).to_ascii_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }

    /// Identity first, then the product name as text: what a device
    /// found on the network is looked up by. A record with no protocol
    /// (a sensor on a digital channel) is never the answer by text.
    #[must_use]
    pub fn lookup(&self, vendor_id: u16, product_code: u16, product_name: &str) -> Option<&Entry> {
        self.by_identity(vendor_id, product_code)
            .into_iter()
            .next()
            .or_else(|| self.search(product_name).into_iter().find(|e| e.networked()))
    }
}

/// `text` with `ADDR` replaced by `address` on every `device` line: the
/// library's map lines made the device's. Nothing else is touched.
#[must_use]
pub fn with_address(text: &str, address: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("device ") {
            out.push_str(&line.replacen("ADDR", address, 1));
        } else {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{ "format": "helmsman-devices-index 1", "built": "2026-11-02",
      "devices": [ { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213",
        "kind": "remote-io", "identity": { "enip": { "vendor_id": 991, "product_code": 515 },
                                           "modbus": { "vendor_name": "Moxa", "product_code": "E1213" } },
        "protocols": { "enip": { "driver": "enip-io", "map": "enip.map", "profile": "moxa-e1213",
          "eds": { "name": "x.eds", "size": 42906, "sha256": "3895fbca8bb036c1b31a9d6b68dea69eca6892fe0a6bce7409ab76311cfd470d" } },
          "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } },
        { "folder": "devices/acme/relay-8", "vendor": "Acme", "model": "Relay 8", "kind": "remote-io",
          "protocols": { "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } } ] }"#;

    /// The library's own index lines for a sensor on a digital channel
    /// (helmsman-devices, `scripts/build-index.py`).
    const SENSORS: &str = r#"{ "format": "helmsman-devices-index 1",
      "devices": [ { "folder": "devices/acme/relay-8", "vendor": "Acme", "model": "Relay 8", "kind": "remote-io",
          "protocols": { "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } },
        { "folder": "devices/ifm/if6123", "vendor": "ifm",
          "model": "IF6123, IF6124, IG6615, IG6616", "kind": "sensor",
          "identity": { "iolink": { "vendor_id": 310, "device_id": 791 } },
          "products": ["IF6123", "IF6124", "IG6615", "IG6616"], "record": "record.json" },
        { "folder": "devices/acme/relay-sensor", "vendor": "Acme", "model": "Relay 8 sensor", "kind": "sensor",
          "identity": { "iolink": { "vendor_id": 7, "device_id": 16777215 } }, "products": ["RS-8"] } ] }"#;

    #[test]
    fn a_sensor_record_is_found_by_its_folder_its_label_and_its_iolink_identity() {
        let i = Index::parse(SENSORS).unwrap();
        let e = i.by_wired("ifm/if6123").unwrap();
        assert_eq!((e.vendor.as_str(), e.kind.as_str()), ("ifm", "sensor"));
        assert_eq!(e.iolink, Some((310, 791)));
        assert_eq!(e.products.len(), 4);
        assert_eq!(e.wired(), "ifm/if6123");
        assert!(e.record && !i.devices[0].record && !i.devices[2].record);
        assert_eq!(
            e.file_url("https://x/main", crate::devrecord::FILE),
            "https://x/main/devices/ifm/if6123/record.json"
        );
        assert!(
            Index::parse(&SENSORS.replace(r#""record": "record.json""#, r#""record": "../x.json""#)).is_err()
        );
        assert!(!e.networked());
        assert!(i.devices[0].networked());
        assert_eq!((i.devices[0].iolink, i.devices[0].products.len()), (None, 0));
        assert!(i.by_wired("ifm/if6124").is_none());
        assert!(i.by_wired("devices/ifm/if6123").is_none());
        assert!(i.by_wired("").is_none());
        // By the number on the label, which the model text may not hold.
        assert_eq!(i.search("ig6616")[0].folder, "devices/ifm/if6123");
        assert_eq!(i.search("ifm IG6615").len(), 1);
        assert_eq!(i.by_iolink(310, 791).len(), 1);
        assert_eq!(i.by_iolink(310, 792).len(), 0);
    }

    /// A device found on the network is never matched to a sensor's
    /// record by text: the sensor is not on the network.
    #[test]
    fn a_network_lookup_by_text_passes_over_sensor_records() {
        let i = Index::parse(SENSORS).unwrap();
        assert!(i.lookup(1, 2, "IF6123").is_none());
        assert!(i.lookup_modbus("ifm", "IG6615").is_none());
        // Both an I/O block and a sensor match the words: the block wins.
        assert_eq!(i.search("acme relay").len(), 2);
        assert_eq!(i.lookup(1, 2, "relay 8").unwrap().folder, "devices/acme/relay-8");
        assert_eq!(
            i.lookup_modbus("Acme", "Relay").unwrap().folder,
            "devices/acme/relay-8"
        );
    }

    #[test]
    fn a_wrong_sensor_identity_or_product_list_fails_the_index() {
        let cases = [
            (
                r#""vendor_id": 310, "device_id": 791"#,
                r#""vendor_id": 0, "device_id": 791"#,
            ),
            (
                r#""vendor_id": 310, "device_id": 791"#,
                r#""vendor_id": 310, "device_id": 0"#,
            ),
            (
                r#""vendor_id": 310, "device_id": 791"#,
                r#""vendor_id": 310, "device_id": 16777216"#,
            ),
            (
                r#""vendor_id": 310, "device_id": 791"#,
                r#""vendor_id": "310", "device_id": 791"#,
            ),
            (r#""vendor_id": 310, "device_id": 791"#, r#""vendor_id": 310"#),
            (r#"["IF6123", "IF6124", "IG6615", "IG6616"]"#, r#""IF6123""#),
            (r#"["IF6123", "IF6124", "IG6615", "IG6616"]"#, r#"["IF6123", 7]"#),
            (r#"["IF6123", "IF6124", "IG6615", "IG6616"]"#, r#"[""]"#),
            (r#"["IF6123", "IF6124", "IG6615", "IG6616"]"#, r#"["a\u0007b"]"#),
        ];
        for (from, to) in cases {
            assert!(SENSORS.contains(from));
            let e = Index::parse(&SENSORS.replace(from, to)).unwrap_err();
            assert!(e.starts_with("devices[1]: "), "{to}: {e}");
        }
        let many: Vec<String> = (0..=MAX_PRODUCTS).map(|k| format!("\"P{k}\"")).collect();
        let e = Index::parse(&SENSORS.replace(r#"["RS-8"]"#, &format!("[{}]", many.join(",")))).unwrap_err();
        assert!(e.contains("more than 128"), "{e}");
        let long = format!("[\"{}\"]", "x".repeat(MAX_TEXT + 1));
        assert!(Index::parse(&SENSORS.replace(r#"["RS-8"]"#, &long)).is_err());
    }

    #[test]
    fn parses_matches_by_identity_then_text() {
        let i = Index::parse(GOOD).unwrap();
        assert_eq!(i.devices.len(), 2);
        let e = i.lookup(991, 515, "anything").unwrap();
        assert_eq!(e.model, "ioLogik E1213");
        assert_eq!(
            e.protocol("enip-io").unwrap().profile.as_deref(),
            Some("moxa-e1213")
        );
        assert_eq!(e.protocol("enip-io").unwrap().eds.as_ref().unwrap().size, 42906);
        assert_eq!(
            e.file_url("https://x/main", "enip.map"),
            "https://x/main/devices/moxa/iologik-e1213/enip.map"
        );
        assert_eq!(i.lookup(1, 2, "relay 8").unwrap().vendor, "Acme");
        assert_eq!(i.lookup(1, 2, "RELAY").unwrap().vendor, "Acme");
        assert!(i.lookup(1, 2, "").is_none());
        assert!(i.lookup(1, 2, "nothing like it").is_none());
        assert_eq!(i.search("moxa e1213").len(), 1);
        assert_eq!(i.search("").len(), 0);
        // By Modbus identity (case-blind, trimmed), then text.
        assert_eq!(i.by_modbus(" moxa ", "e1213").len(), 1);
        assert_eq!(i.by_modbus("Moxa", "E1214").len(), 0);
        assert_eq!(i.lookup_modbus("MOXA", "E1213").unwrap().model, "ioLogik E1213");
        assert_eq!(
            i.lookup_modbus("Acme", "Relay").unwrap().vendor,
            "Acme",
            "by text"
        );
        assert!(i.lookup_modbus("Nobody", "X1").is_none());
        assert_eq!(i.devices[1].modbus, None);
    }

    #[test]
    fn a_hostile_index_is_refused_whole() {
        let bad = [
            "",
            "null",
            "[]",
            r#"{ "format": "other", "devices": [] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": {} }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "../etc", "vendor": "a", "model": "b" } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b/c", "vendor": "a", "model": "b" } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": 1, "model": "b" } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b", "identity": { "enip": { "vendor_id": 70000, "product_code": 1 } } } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b", "protocols": { "x": { "driver": "ssh" } } } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b", "protocols": { "enip": { "driver": "enip-io", "map": "../io.map" } } } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b", "protocols": { "enip": { "driver": "enip-io", "eds": { "name": "x.eds", "sha256": "nope" } } } } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b" }, { "folder": "devices/a/b", "vendor": "a", "model": "c" } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a\u0000", "model": "b" } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b", "identity": { "modbus": { "vendor_name": "a" } } } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b", "identity": { "modbus": { "vendor_name": 1, "product_code": "x" } } } ] }"#,
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/a/b", "vendor": "a", "model": "b", "identity": { "modbus": { "vendor_name": "a\n", "product_code": "x" } } } ] }"#,
        ];
        for b in bad {
            assert!(Index::parse(b).is_err(), "{b:.80}");
        }
        let long = format!(
            r#"{{ "format": "helmsman-devices-index 1", "devices": [ {{ "folder": "devices/a/b", "vendor": "{}", "model": "b" }} ] }}"#,
            "v".repeat(MAX_TEXT + 1)
        );
        assert!(Index::parse(&long).is_err());
        let huge = format!(
            r#"{{ "format": "helmsman-devices-index 1", "devices": [], "pad": "{}" }}"#,
            "x".repeat(MAX_BYTES)
        );
        assert!(Index::parse(&huge).is_err());
    }

    #[test]
    fn the_address_goes_into_device_lines_only() {
        let t = "# ADDR stays here\ndevice e1213 enip-io ADDR profile=moxa-e1213\nin di0 bool e1213 di:0 # ADDR\n";
        assert_eq!(
            with_address(t, "10.0.0.57"),
            "# ADDR stays here\ndevice e1213 enip-io 10.0.0.57 profile=moxa-e1213\nin di0 bool e1213 di:0 # ADDR\n"
        );
    }

    #[test]
    fn an_entry_may_name_its_profile_file_and_nothing_else_passes_as_one() {
        let ok = Index::parse(
            r#"{ "format": "helmsman-devices-index 1", "devices": [ { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213", "profile": "moxa-iologik-e1213.device", "protocols": { "modbus": { "driver": "modbus-tcp" } } } ] }"#,
        )
        .unwrap();
        assert_eq!(
            ok.devices[0].profile.as_deref(),
            Some("moxa-iologik-e1213.device")
        );
        assert_eq!(
            ok.devices[0].file_url("https://x/lib", "moxa-iologik-e1213.device"),
            "https://x/lib/devices/moxa/iologik-e1213/moxa-iologik-e1213.device"
        );
        for bad in [
            "../x.device",
            "Moxa.device",
            "e1213.eds",
            "a/b.device",
            "x.devicee",
        ] {
            let e = Index::parse(&format!(
                r#"{{ "format": "helmsman-devices-index 1", "devices": [ {{ "folder": "devices/a/b", "vendor": "a", "model": "b", "profile": "{bad}", "protocols": {{ "modbus": {{ "driver": "modbus-tcp" }} }} }} ] }}"#
            ))
            .unwrap_err();
            assert!(e.contains("profile: not a profile file name"), "{bad}: {e}");
        }
    }

    /// The Novanta MDrive's library entry as the library's index will
    /// carry it (helmsman-devices `devices/novanta/lexium-mdrive-ethernet`,
    /// with its profile file), and the scan's identity for the bench unit:
    /// vendor 243, product code 15362, "MDrive", revision 2.36.
    const MDRIVE: &str = r#"{ "format": "helmsman-devices-index 1", "devices": [
      { "folder": "devices/novanta/lexium-mdrive-ethernet", "vendor": "Novanta IMS",
        "model": "Liberty / Lexium MDrive Ethernet (LMD), EtherNet/IP", "kind": "drive",
        "profile": "novanta-mdrive.device",
        "identity": { "enip": { "vendor_id": 243, "product_code": 15362 } },
        "protocols": { "enip": { "driver": "enip-io",
          "eds": { "name": "SEM_MDrivever1_2.eds", "size": 24813,
                   "sha256": "b14f0fe268df5b37e9dcee6bd0288a88e59d428dbe6044b49dba8beaaeb86041" } } } },
      { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213", "kind": "remote-io",
        "identity": { "enip": { "vendor_id": 991, "product_code": 515 } },
        "protocols": { "enip": { "driver": "enip-io", "profile": "moxa-e1213" } } } ] }"#;

    #[test]
    fn the_mdrive_is_found_by_its_identity_and_offers_its_profile() {
        let i = Index::parse(MDRIVE).unwrap();
        let e = i.lookup(243, 15362, "MDrive").unwrap();
        assert_eq!(e.folder, "devices/novanta/lexium-mdrive-ethernet");
        assert_eq!(e.profile.as_deref(), Some("novanta-mdrive.device"));
        assert_eq!(e.protocol("enip-io").unwrap().driver, "enip-io");
        assert_eq!(
            e.file_url("https://x/lib", e.profile.as_deref().unwrap()),
            "https://x/lib/devices/novanta/lexium-mdrive-ethernet/novanta-mdrive.device"
        );
        // The identity is a vendor and a product code, so both units seen
        // (revision 2.22 by the EDS, 2.36 on the bench) are one entry.
        assert_eq!(i.by_identity(243, 15362).len(), 1);
        // Another product code or vendor is another device, by identity.
        assert!(i.by_identity(243, 15363).is_empty());
        assert!(i.by_identity(244, 15362).is_empty());
        assert!(i.by_identity(15362, 243).is_empty());
        assert_ne!(i.by_identity(991, 515)[0].folder, e.folder);
        // And an unrelated name does not match by text.
        assert!(i.lookup(243, 15363, "Something Else").is_none());
    }
}
