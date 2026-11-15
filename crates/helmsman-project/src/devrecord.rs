//! A wired sensor's or actuator's record from the device library:
//! `record.json` in its folder, built there by `scripts/build-records.py`
//! from the folder's `device.toml` (which was drafted from the device's
//! IODD). It says what the thing on a point is: who makes it, the order
//! numbers one record covers, how each connector is wired, what its
//! signals and their states are called, and the settings it leaves the
//! factory with. A map line names the record with `wired=VENDOR/MODEL`
//! ([`crate::devlib::Index::by_wired`]).
//!
//! ```json
//! { "format": "helmsman-device-record 1", "folder": "devices/ifm/if6123",
//!   "vendor": "ifm", "model": "IF6123, IF6124", "kind": "sensor", "draft": true,
//!   "iolink": { "vendor_id": 310, "device_id": 791 },
//!   "variants": [ { "product_id": "IF6123", "description": "Inductive sensor" } ],
//!   "wired": { "sio": true,
//!              "switching": "SSC1", "source": "data sheet, p. 2",
//!              "connections": [ { "type": "M12-4", "products": ["IF6123"],
//!                                 "wires": [ { "n": 4, "function": "C/Q", "color": "BK" } ] } ] },
//!   "signals": [ { "dir": "in", "name": "SSC1", "type": "bool", "bits": "0",
//!                  "on": "active", "off": "inactive" } ],
//!   "settings": [ { "name": "P-n", "index": 500, "default": "0", "default_name": "PnP",
//!                   "values": { "0": "PnP", "1": "nPn" } } ] }
//! ```
//!
//! `draft` is true until someone has checked a unit. `wired.switching`,
//! which signal the switching wire carries with no IO-Link master, is
//! there only when the library has it from the device's manual, with
//! `source`: an IODD does not say, and neither does this reader.
//!
//! Everything read here is data from a public repository, shown to
//! people and handed to the agent: texts are bounded and printable,
//! numbers are numbers, lists are capped, and a record over
//! [`MAX_BYTES`] or of another format is refused rather than trusted.

use serde_json::{Value as Json, json};

/// The format line a record must carry.
pub const FORMAT: &str = "helmsman-device-record 1";
/// The record's file name in a device's folder.
pub const FILE: &str = "record.json";
/// Largest record read, bytes.
pub const MAX_BYTES: usize = 64 * 1024;
/// Longest text kept.
pub const MAX_TEXT: usize = 160;
/// Most signals or settings in one record.
pub const MAX_ITEMS: usize = 256;
/// Most order numbers.
pub const MAX_VARIANTS: usize = 128;
/// Most connectors.
pub const MAX_CONNECTIONS: usize = 32;
/// Most named values of one signal or setting.
pub const MAX_VALUES: usize = 32;

/// One order number the record covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub product_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// One wire of a connector or cable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wire {
    /// The pin or core, 1 to 9.
    pub n: u8,
    /// `L+`, `L-`, `C/Q`, `NC`, `Other`, ... as the IODD has it.
    pub function: Option<String>,
    /// The core's colour code (`BN`, `WH`, `BU`, `BK`, ...).
    pub color: Option<String>,
    pub name: Option<String>,
}

/// One connector or cable the variants have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// `M5`, `M8`, `M12-4`, `M12-5`, `Cable`, `Other`.
    pub kind: String,
    pub products: Vec<String>,
    pub wires: Vec<Wire>,
}

/// One signal, as the device names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    /// True for a signal from the device.
    pub input: bool,
    pub name: String,
    /// `bool`, `int`, `uint`, `float32`, `string`, `octets`, `time`,
    /// `timespan`.
    pub ty: String,
    /// What the signal's true state is called, when the vendor says.
    pub on: Option<String>,
    pub off: Option<String>,
    pub description: Option<String>,
    /// A number's named values.
    pub values: Vec<(String, String)>,
}

/// One choice the device can be set to, with its factory value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub name: String,
    pub default: String,
    /// What the factory value is called (`PnP`, `High active`).
    pub default_name: Option<String>,
    pub values: Vec<(String, String)>,
}

/// A wired device's record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// `devices/<vendor>/<model>`.
    pub folder: String,
    pub vendor: String,
    pub model: String,
    /// `sensor`, `actuator` or `other`.
    pub kind: String,
    /// Nobody has checked a unit yet.
    pub draft: bool,
    pub family: Option<String>,
    pub iolink: Option<(u16, u32)>,
    pub variants: Vec<Variant>,
    /// Whether the device switches a wire with no IO-Link master.
    pub sio: Option<bool>,
    /// The signal the switching wire carries with no master, and where
    /// the library read that.
    pub switching: Option<(String, String)>,
    pub connections: Vec<Connection>,
    pub signals: Vec<Signal>,
    pub settings: Vec<Setting>,
}

fn text(j: &Json, key: &str, what: &str) -> Result<String, String> {
    opt(j, key, what)?.ok_or_else(|| format!("{what}.{key}: missing"))
}

fn opt(j: &Json, key: &str, what: &str) -> Result<Option<String>, String> {
    let Some(v) = j.get(key) else {
        return Ok(None);
    };
    let s = v.as_str().ok_or_else(|| format!("{what}.{key}: not text"))?;
    if s.is_empty() || s.chars().count() > MAX_TEXT || s.chars().any(char::is_control) {
        return Err(format!(
            "{what}.{key}: a printable text of 1..={MAX_TEXT} characters"
        ));
    }
    Ok(Some(s.to_string()))
}

fn list<'a>(j: &'a Json, key: &str, what: &str, most: usize) -> Result<&'a [Json], String> {
    match j.get(key) {
        None => Ok(&[]),
        Some(v) => {
            let l = v.as_array().ok_or_else(|| format!("{what}.{key}: not a list"))?;
            if l.len() > most {
                return Err(format!("{what}.{key}: {} entries, more than {most}", l.len()));
            }
            Ok(l)
        }
    }
}

fn number(j: &Json, key: &str, what: &str, low: u64, top: u64) -> Result<u64, String> {
    j.get(key)
        .and_then(Json::as_u64)
        .filter(|n| (low..=top).contains(n))
        .ok_or_else(|| format!("{what}.{key}: not a number {low}..={top}"))
}

fn values(j: &Json, what: &str) -> Result<Vec<(String, String)>, String> {
    let Some(v) = j.get("values") else {
        return Ok(Vec::new());
    };
    let obj = v
        .as_object()
        .ok_or_else(|| format!("{what}.values: not an object"))?;
    if obj.len() > MAX_VALUES {
        return Err(format!("{what}.values: more than {MAX_VALUES}"));
    }
    obj.iter()
        .map(|(k, name)| {
            let name = name
                .as_str()
                .filter(|n| {
                    !n.is_empty() && n.chars().count() <= MAX_TEXT && !n.chars().any(char::is_control)
                })
                .ok_or_else(|| format!("{what}.values: a name is a printable text"))?;
            if k.is_empty() || k.len() > 32 || k.chars().any(char::is_control) {
                return Err(format!("{what}.values: a value is a short printable text"));
            }
            Ok((k.clone(), name.to_string()))
        })
        .collect()
}

fn texts(j: &Json, key: &str, what: &str, most: usize) -> Result<Vec<String>, String> {
    list(j, key, what, most)?
        .iter()
        .map(|p| {
            p.as_str()
                .filter(|s| {
                    !s.is_empty() && s.chars().count() <= MAX_TEXT && !s.chars().any(char::is_control)
                })
                .map(str::to_string)
                .ok_or_else(|| format!("{what}.{key}: a printable text"))
        })
        .collect()
}

fn connection(j: &Json, what: &str) -> Result<Connection, String> {
    let mut wires = Vec::new();
    for (k, w) in list(j, "wires", what, 9)?.iter().enumerate() {
        let ww = format!("{what}.wires[{k}]");
        wires.push(Wire {
            n: u8::try_from(number(w, "n", &ww, 1, 9)?).unwrap_or(0),
            function: opt(w, "function", &ww)?,
            color: opt(w, "color", &ww)?,
            name: opt(w, "name", &ww)?,
        });
    }
    Ok(Connection {
        kind: text(j, "type", what)?,
        products: texts(j, "products", what, MAX_VARIANTS)?,
        wires,
    })
}

fn signal(j: &Json, what: &str) -> Result<Signal, String> {
    let input = match j.get("dir").and_then(Json::as_str) {
        Some("in") => true,
        Some("out") => false,
        _ => return Err(format!("{what}.dir: in or out")),
    };
    let ty = text(j, "type", what)?;
    if !matches!(
        ty.as_str(),
        "bool" | "int" | "uint" | "float32" | "string" | "octets" | "time" | "timespan"
    ) {
        return Err(format!("{what}.type: not a signal type: {ty:?}"));
    }
    Ok(Signal {
        input,
        name: text(j, "name", what)?,
        ty,
        on: opt(j, "on", what)?,
        off: opt(j, "off", what)?,
        description: opt(j, "description", what)?,
        values: values(j, what)?,
    })
}

impl Record {
    /// Parses a `record.json`.
    ///
    /// # Errors
    /// Why it is not a record this reader takes, in words: the wrong
    /// format, over a limit, or a field that does not read. One bad
    /// field fails the whole record.
    pub fn parse(src: &str) -> Result<Self, String> {
        if src.len() > MAX_BYTES {
            return Err(format!(
                "the record is {} bytes, more than {MAX_BYTES}",
                src.len()
            ));
        }
        let j: Json = serde_json::from_str(src).map_err(|e| format!("not JSON: {e}"))?;
        if j.get("format").and_then(Json::as_str) != Some(FORMAT) {
            return Err(format!("not a {FORMAT:?} file"));
        }
        let folder = text(&j, "folder", "record")?;
        if !folder.starts_with("devices/") || folder.split('/').count() != 3 || folder.contains("..") {
            return Err(format!("folder: not devices/<vendor>/<model>: {folder:?}"));
        }
        let iolink = match j.get("iolink") {
            None => None,
            Some(l) => Some((
                u16::try_from(number(l, "vendor_id", "iolink", 1, 65_535)?).unwrap_or(0),
                u32::try_from(number(l, "device_id", "iolink", 1, 0xFF_FFFF)?).unwrap_or(0),
            )),
        };
        let mut variants = Vec::new();
        for (k, v) in list(&j, "variants", "record", MAX_VARIANTS)?.iter().enumerate() {
            let what = format!("variants[{k}]");
            variants.push(Variant {
                product_id: text(v, "product_id", &what)?,
                name: opt(v, "name", &what)?,
                description: opt(v, "description", &what)?,
            });
        }
        let wired = j.get("wired").ok_or("wired: missing")?;
        let sio = match wired.get("sio") {
            None => None,
            Some(b) => Some(b.as_bool().ok_or("wired.sio: true or false")?),
        };
        // A switching signal without where it was read is not taken.
        let switching = match opt(wired, "switching", "wired")? {
            None => None,
            Some(signal) => Some((signal, text(wired, "source", "wired")?)),
        };
        let mut connections = Vec::new();
        for (k, c) in list(wired, "connections", "wired", MAX_CONNECTIONS)?
            .iter()
            .enumerate()
        {
            connections.push(connection(c, &format!("wired.connections[{k}]"))?);
        }
        let mut signals = Vec::new();
        for (k, s) in list(&j, "signals", "record", MAX_ITEMS)?.iter().enumerate() {
            signals.push(signal(s, &format!("signals[{k}]"))?);
        }
        let mut settings = Vec::new();
        for (k, s) in list(&j, "settings", "record", MAX_ITEMS)?.iter().enumerate() {
            let what = format!("settings[{k}]");
            settings.push(Setting {
                name: text(s, "name", &what)?,
                default: text(s, "default", &what)?,
                default_name: opt(s, "default_name", &what)?,
                values: values(s, &what)?,
            });
        }
        Ok(Self {
            folder,
            vendor: text(&j, "vendor", "record")?,
            model: text(&j, "model", "record")?,
            kind: text(&j, "kind", "record")?,
            draft: j.get("draft").and_then(Json::as_bool).unwrap_or(true),
            family: opt(&j, "family", "record")?,
            iolink,
            variants,
            sio,
            switching,
            connections,
            signals,
            settings,
        })
    }

    /// The `wired=` value that names this record: its folder without
    /// `devices/`.
    #[must_use]
    pub fn wired(&self) -> &str {
        self.folder.strip_prefix("devices/").unwrap_or(&self.folder)
    }

    /// The signal the switching wire carries with no IO-Link master,
    /// when the library has it from the manual.
    #[must_use]
    pub fn switching_signal(&self) -> Option<&Signal> {
        let (name, _) = self.switching.as_ref()?;
        self.signals.iter().find(|s| &s.name == name)
    }

    /// What a person or the agent needs to know about the thing on a
    /// point, in a few lines of JSON: what it is, whether anyone has
    /// checked it, its on/off signals with what each state is called,
    /// which of them the switching wire carries when the library knows,
    /// and the settings it leaves the factory with. Lists are cut to
    /// `most` entries each, with `more` saying how many were left out.
    #[must_use]
    pub fn brief(&self, most: usize) -> Json {
        let switches: Vec<&Signal> = self.signals.iter().filter(|s| s.ty == "bool").collect();
        let mut out = json!({
            "wired": self.wired(),
            "vendor": self.vendor,
            "model": self.model,
            "kind": self.kind,
            "draft": self.draft,
            "products": self.variants.iter().take(most).map(|v| v.product_id.clone()).collect::<Vec<_>>(),
            "switches_without_a_master": self.sio,
            "signals": switches.iter().take(most).map(|s| json!({
                "dir": if s.input { "in" } else { "out" },
                "name": s.name, "on": s.on, "off": s.off, "description": s.description,
            })).collect::<Vec<_>>(),
            "factory_settings": self.settings.iter().take(most).map(|s| json!({
                "name": s.name,
                "value": s.default_name.clone().unwrap_or_else(|| s.default.clone()),
            })).collect::<Vec<_>>(),
        });
        match &self.switching {
            Some((signal, source)) => {
                out["switching_wire_carries"] = json!({ "signal": signal, "source": source });
            }
            None => {
                out["switching_wire_carries"] = json!(
                    "not in the library yet: the device's data sheet says which signal its switching wire carries and whether it is normally open"
                );
            }
        }
        let left_out = self.variants.len().saturating_sub(most)
            + switches.len().saturating_sub(most)
            + self.settings.len().saturating_sub(most);
        if left_out > 0 {
            out["more"] = json!(left_out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As helmsman-devices' `scripts/build-records.py` writes one.
    const GOOD: &str = r#"{
 "format": "helmsman-device-record 1",
 "folder": "devices/ifm/if6123",
 "vendor": "ifm",
 "model": "IF6123, IF6124",
 "kind": "sensor",
 "draft": true,
 "family": "I",
 "iolink": { "vendor_id": 310, "device_id": 791 },
 "variants": [
  { "product_id": "IF6123", "name": "IF6123", "description": "Inductive sensor" },
  { "product_id": "IF6124" }
 ],
 "wired": {
  "sio": true,
  "connections": [
   { "type": "M12-4", "products": ["IF6123"],
     "wires": [ { "n": 1, "function": "L+", "color": "BN" },
                { "n": 2, "function": "Other", "color": "WH", "name": "OUT2" },
                { "n": 4, "function": "C/Q", "color": "BK" } ] }
  ]
 },
 "signals": [
  { "dir": "in", "name": "PDV1", "type": "int", "bits": "4..15", "values": { "8184": "OL" } },
  { "dir": "in", "name": "SSC1", "type": "bool", "bits": "0", "on": "active", "off": "inactive" },
  { "dir": "out", "name": "Teach", "type": "bool", "bits": "0" }
 ],
 "settings": [
  { "name": "SSC1 Config: Logic", "index": 61, "subindex": 1, "default": "0",
    "default_name": "High active", "values": { "0": "High active", "1": "Low active" } },
  { "name": "Keylock", "index": 100, "default": "false" }
 ]
}"#;

    #[test]
    fn a_record_reads_as_what_the_library_wrote() {
        let r = Record::parse(GOOD).unwrap();
        assert_eq!(
            (r.vendor.as_str(), r.kind.as_str(), r.draft),
            ("ifm", "sensor", true)
        );
        assert_eq!(r.wired(), "ifm/if6123");
        assert_eq!(r.iolink, Some((310, 791)));
        assert_eq!(r.variants.len(), 2);
        assert_eq!(r.variants[1].name, None);
        assert_eq!(r.sio, Some(true));
        assert_eq!(r.switching, None);
        assert_eq!(r.connections[0].kind, "M12-4");
        assert_eq!(
            r.connections[0].wires[1],
            Wire {
                n: 2,
                function: Some("Other".into()),
                color: Some("WH".into()),
                name: Some("OUT2".into())
            }
        );
        assert_eq!(r.signals.len(), 3);
        assert_eq!(
            (r.signals[1].on.as_deref(), r.signals[1].off.as_deref()),
            (Some("active"), Some("inactive"))
        );
        assert!(r.signals[1].input && !r.signals[2].input);
        assert_eq!(r.signals[0].values, [("8184".to_string(), "OL".to_string())]);
        assert_eq!(r.settings[0].default_name.as_deref(), Some("High active"));
        assert_eq!(r.settings[1].values, []);
        assert_eq!(r.switching_signal(), None);
    }

    #[test]
    fn the_brief_says_what_is_known_and_what_is_not() {
        let r = Record::parse(GOOD).unwrap();
        let b = r.brief(8);
        assert_eq!(b["wired"], "ifm/if6123");
        assert_eq!(b["draft"], true);
        assert_eq!(b["products"], json!(["IF6123", "IF6124"]));
        assert_eq!(b["switches_without_a_master"], true);
        // Only the on/off signals: a wired point is one bit.
        assert_eq!(b["signals"].as_array().unwrap().len(), 2);
        assert_eq!(b["signals"][0]["on"], "active");
        assert_eq!(
            b["factory_settings"][0],
            json!({"name": "SSC1 Config: Logic", "value": "High active"})
        );
        assert_eq!(
            b["factory_settings"][1],
            json!({"name": "Keylock", "value": "false"})
        );
        assert!(
            b["switching_wire_carries"]
                .as_str()
                .unwrap()
                .starts_with("not in the library yet")
        );
        assert!(b.get("more").is_none());
        // Cut to one entry a list: the rest is counted, not dropped unsaid.
        let cut = r.brief(1);
        assert_eq!(cut["products"], json!(["IF6123"]));
        assert_eq!(cut["more"], 3);

        let known = GOOD.replace(
            r#""sio": true,"#,
            r#""sio": true, "switching": "SSC1", "source": "data sheet, p. 2","#,
        );
        let r = Record::parse(&known).unwrap();
        assert_eq!(r.switching_signal().unwrap().on.as_deref(), Some("active"));
        assert_eq!(
            r.brief(8)["switching_wire_carries"],
            json!({"signal": "SSC1", "source": "data sheet, p. 2"})
        );
    }

    /// The library's own records (not committed here): every
    /// `devices/*/*/record.json` under `HELMSMAN_DEVICES_DIR` reads.
    ///
    /// `HELMSMAN_DEVICES_DIR=~/github/helmsman-devices cargo test -p helmsman-project devrecord -- --ignored --nocapture`
    #[test]
    #[ignore = "needs HELMSMAN_DEVICES_DIR, a checkout of the device library"]
    fn the_librarys_records_read() {
        let root =
            std::path::PathBuf::from(std::env::var("HELMSMAN_DEVICES_DIR").expect("HELMSMAN_DEVICES_DIR"));
        let mut read = 0;
        for vendor in std::fs::read_dir(root.join("devices")).unwrap() {
            let vendor = vendor.unwrap().path();
            if !vendor.is_dir() {
                continue;
            }
            for model in std::fs::read_dir(&vendor).unwrap() {
                let file = model.unwrap().path().join(FILE);
                if !file.is_file() {
                    continue;
                }
                let r = Record::parse(&std::fs::read_to_string(&file).unwrap())
                    .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
                println!(
                    "{}: {} {} ({}), {} signal(s), {} setting(s)",
                    r.wired(),
                    r.vendor,
                    r.model,
                    r.kind,
                    r.signals.len(),
                    r.settings.len()
                );
                assert!(file.ends_with(format!("{}/{FILE}", r.folder)));
                let _ = r.brief(8);
                read += 1;
            }
        }
        assert!(read > 0, "no record.json under {}", root.display());
    }

    #[test]
    fn what_is_not_a_record_is_refused_whole() {
        let cases = [
            (
                r#""helmsman-device-record 1""#,
                r#""helmsman-device-record 2""#,
                "not a",
            ),
            (
                r#""folder": "devices/ifm/if6123""#,
                r#""folder": "../../etc/passwd""#,
                "folder",
            ),
            (
                r#""folder": "devices/ifm/if6123""#,
                r#""folder": "devices/ifm/../x""#,
                "folder",
            ),
            (
                r#""vendor": "ifm","#,
                r#""vendor": 7,"#,
                "record.vendor: not text",
            ),
            (r#""vendor": "ifm","#, "", "record.vendor: missing"),
            (r#""vendor": "ifm","#, r#""vendor": "","#, "record.vendor"),
            (r#""vendor": "ifm","#, r#""vendor": "a\u0007b","#, "record.vendor"),
            (r#""vendor_id": 310"#, r#""vendor_id": 0"#, "iolink.vendor_id"),
            (
                r#""device_id": 791"#,
                r#""device_id": 16777216"#,
                "iolink.device_id",
            ),
            (r#""sio": true,"#, r#""sio": "yes","#, "wired.sio"),
            (
                r#""sio": true,"#,
                r#""sio": true, "switching": "SSC1","#,
                "wired.source: missing",
            ),
            (r#""n": 1,"#, r#""n": 10,"#, "wires[0].n"),
            (r#""dir": "out""#, r#""dir": "both""#, "signals[2].dir"),
            (r#""type": "int""#, r#""type": "decimal""#, "signals[0].type"),
            (r#"{ "8184": "OL" }"#, r#"{ "8184": 5 }"#, "signals[0].values"),
            (r#"{ "8184": "OL" }"#, r#"["OL"]"#, "signals[0].values"),
            (r#""products": ["IF6123"]"#, r#""products": [1]"#, "products"),
            (r#""name": "Keylock""#, r#""name": null"#, "settings[1].name"),
            (r#""variants": ["#, r#""variants": 3, "x": ["#, "variants"),
        ];
        for (from, to, why) in cases {
            assert!(GOOD.contains(from), "{from}");
            let e = Record::parse(&GOOD.replacen(from, to, 1)).unwrap_err();
            assert!(e.contains(why), "{to}: {e}");
            assert!(e.len() < 300, "{e}");
        }
        let long = GOOD.replace(
            r#""vendor": "ifm""#,
            &format!(r#""vendor": "{}""#, "x".repeat(MAX_TEXT + 1)),
        );
        assert!(Record::parse(&long).is_err());
        let many: Vec<String> = (0..=MAX_ITEMS)
            .map(|k| format!(r#"{{"dir":"in","name":"s{k}","type":"bool"}}"#))
            .collect();
        let e = Record::parse(&GOOD.replace(
            r#""signals": ["#,
            &format!(r#""signals": [{}], "x": ["#, many.join(",")),
        ))
        .unwrap_err();
        assert!(e.contains("more than 256"), "{e}");
        assert!(
            Record::parse(&" ".repeat(MAX_BYTES + 1))
                .unwrap_err()
                .contains("bytes")
        );
        assert!(Record::parse("").is_err());
        assert!(Record::parse("[]").is_err());
        assert!(Record::parse(&GOOD.replace(r#" "wired": {"#, r#" "unwired": {"#)).is_err());
    }
}
