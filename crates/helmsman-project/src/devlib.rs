//! The public device library's index (`devices-repo`, interview
//! 2026-11-02): `index.json` at the root of
//! <https://github.com/JimothyJohn/helmsman-devices>, built from every
//! `device.toml` there by its `scripts/build-index.py`. A box fetches it
//! and a browser's simulation fetches it, both through this: parse the
//! file, match a device found on the network by identity first (an
//! EtherNet/IP vendor id and product code), then by vendor and model text.
//! Semantic search is for later, once there are entries enough to need it.
//!
//! ```json
//! { "format": "helmsman-devices-index 1",
//!   "devices": [ { "folder": "devices/moxa/iologik-e1213",
//!                  "vendor": "Moxa", "model": "ioLogik E1213", "kind": "remote-io",
//!                  "identity": { "enip": { "vendor_id": 991, "product_code": 515 } },
//!                  "protocols": { "enip": { "driver": "enip-io", "map": "enip.map",
//!                                           "profile": "moxa-e1213",
//!                                           "eds": { "name": "...", "size": 42906, "sha256": "..." } },
//!                                 "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } } ] }
//! ```
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
    pub protocols: Vec<Protocol>,
}

impl Entry {
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
    let mut protocols = Vec::new();
    if let Some(p) = j.get("protocols") {
        let obj = p.as_object().ok_or("protocols: not an object")?;
        for (name, v) in obj {
            protocols.push(protocol(name, v)?);
        }
    }
    Ok(Entry {
        folder,
        vendor,
        model,
        kind: text(j, "kind")?,
        enip,
        protocols,
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

    /// The entries whose vendor or model contains every word of `query`,
    /// case-blind: the second thing a lookup tries. An empty query
    /// matches nothing.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let words: Vec<String> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        self.devices
            .iter()
            .filter(|e| {
                let hay = format!("{} {}", e.vendor, e.model).to_ascii_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }

    /// Identity first, then the product name as text: what a device
    /// found on the network is looked up by.
    #[must_use]
    pub fn lookup(&self, vendor_id: u16, product_code: u16, product_name: &str) -> Option<&Entry> {
        self.by_identity(vendor_id, product_code)
            .into_iter()
            .next()
            .or_else(|| self.search(product_name).into_iter().next())
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
        "kind": "remote-io", "identity": { "enip": { "vendor_id": 991, "product_code": 515 } },
        "protocols": { "enip": { "driver": "enip-io", "map": "enip.map", "profile": "moxa-e1213",
          "eds": { "name": "x.eds", "size": 42906, "sha256": "3895fbca8bb036c1b31a9d6b68dea69eca6892fe0a6bce7409ab76311cfd470d" } },
          "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } },
        { "folder": "devices/acme/relay-8", "vendor": "Acme", "model": "Relay 8", "kind": "remote-io",
          "protocols": { "modbus": { "driver": "modbus-tcp", "map": "modbus.map" } } } ] }"#;

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
}
