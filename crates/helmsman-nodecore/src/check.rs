//! The comms check: before a node takes a new I/O map it asks every
//! device the map names, from its own port, and the map is saved only
//! once each one has answered (Nick, 2026-11-24: "verify that there's not
//! a comms issue before downloading the map"; the download is blocked
//! until all answer).
//!
//! Why the node asks and not the machine that wrote the map: that
//! machine reaching a device says nothing about the node reaching it. A
//! device on another subnet, behind another switch port or holding its
//! one connection for someone else answers a laptop and not the node.
//!
//! This module is the record and the rule; the firmware does the asking
//! (crates/helmsman-node-p4 `check`): a Modbus TCP device is connected to
//! and, when the map names something to read from it, asked for the first
//! of those reads (which proves the unit id and the address as well as
//! the cable); an EtherNet/IP device is connected to and a session is
//! registered and dropped. Nothing is written to a device and no I/O
//! connection is opened, so a check never moves an output and never
//! takes a device from whoever owns it.
//!
//! The rule ([`Check::allows`]): a map is saved when the last check was
//! of exactly that text, every device in it answered, and it finished
//! less than [`FRESH_MS`] ago.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use serde_json::{Value as Json, json};

use crate::iomap::{IoMap, Link};
use crate::settings::Net;

/// How long a passed check lets its map be saved: long enough to read
/// the result and press Save, short enough that the answer is still
/// about the plant as it is.
pub const FRESH_MS: u64 = 10 * 60 * 1000;

/// What one device said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not asked yet.
    Waiting,
    /// It answered, in this many milliseconds.
    Answered { ms: u32 },
    /// It did not, and why.
    Failed(String),
}

/// One device of the checked map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// `a.b.c.d:port`.
    pub address: String,
    /// `modbus-tcp` or `enip-io`.
    pub driver: &'static str,
    pub verdict: Verdict,
}

/// The last check a node ran, or the one it is running.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Check {
    /// The map text that was checked; empty when none ever was.
    pub text: String,
    pub rows: Vec<Row>,
    pub running: bool,
    /// The node's uptime when the check finished.
    pub done_ms: u64,
}

/// Why a map may not be saved yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No check of this text has been run (none at all, or of another
    /// text).
    NotChecked,
    /// The check of this text is still running.
    Running,
    /// These devices did not answer.
    Failed(Vec<String>),
    /// The check passed, but too long ago.
    Stale,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotChecked => f.write_str(
                "this map has not been checked: the node asks every device in it first (POST /api/io_map/check)",
            ),
            Self::Running => f.write_str("the check of this map is still running"),
            Self::Failed(names) => write!(
                f,
                "not saved: {} did not answer this node",
                names.join(", ")
            ),
            Self::Stale => write!(
                f,
                "the check of this map is more than {} minutes old; check it again",
                FRESH_MS / 60_000
            ),
        }
    }
}

fn address(host: [u8; 4], port: u16) -> String {
    format!("{}.{}.{}.{}:{port}", host[0], host[1], host[2], host[3])
}

impl Check {
    /// A check of `map` about to start: every device waiting.
    #[must_use]
    pub fn begin(map: &IoMap) -> Self {
        Self {
            text: map.text.clone(),
            rows: map
                .devices
                .iter()
                .map(|d| Row {
                    name: d.id.clone(),
                    address: address(d.host, d.port),
                    driver: match d.link {
                        Link::Modbus { .. } => "modbus-tcp",
                        Link::Enip(_) => "enip-io",
                    },
                    verdict: Verdict::Waiting,
                })
                .collect(),
            running: true,
            done_ms: 0,
        }
    }

    /// Records what device `k` said.
    pub fn set(&mut self, k: usize, verdict: Verdict) {
        if let Some(row) = self.rows.get_mut(k) {
            row.verdict = verdict;
        }
    }

    /// Ends the check at uptime `now_ms`. A device still waiting was not
    /// asked, which is a failure and not a pass.
    pub fn finish(&mut self, now_ms: u64) {
        for row in &mut self.rows {
            if row.verdict == Verdict::Waiting {
                row.verdict = Verdict::Failed("not asked: the check was cut short".to_string());
            }
        }
        self.running = false;
        self.done_ms = now_ms;
    }

    /// The names of the devices that did not answer.
    #[must_use]
    pub fn failed(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| !matches!(r.verdict, Verdict::Answered { .. }))
            .map(|r| r.name.clone())
            .collect()
    }

    /// Every device answered, and a map was checked: a map of sim points
    /// alone has no device to ask and passes at once.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.running && !self.text.is_empty() && self.failed().is_empty()
    }

    /// Whether `text` may be saved at uptime `now_ms`.
    ///
    /// # Errors
    /// Why not.
    pub fn allows(&self, text: &str, now_ms: u64) -> Result<(), Refusal> {
        if self.text.is_empty() || self.text != text {
            return Err(Refusal::NotChecked);
        }
        if self.running {
            return Err(Refusal::Running);
        }
        if !self.passed() {
            return Err(Refusal::Failed(self.failed()));
        }
        // A clock that reads earlier than the check's end is a restart's;
        // the record does not outlive one, so this cannot pass by wrapping.
        match now_ms.checked_sub(self.done_ms) {
            Some(age) if age < FRESH_MS => Ok(()),
            _ => Err(Refusal::Stale),
        }
    }

    /// `GET /api/io_map/check`.
    #[must_use]
    pub fn json(&self, now_ms: u64) -> Json {
        let state = if self.text.is_empty() {
            "none"
        } else if self.running {
            "running"
        } else {
            "done"
        };
        json!({
            "state": state,
            "passed": self.passed(),
            "may_save": self.allows(&self.text, now_ms).is_ok(),
            "age_ms": if state == "done" { Json::from(now_ms.saturating_sub(self.done_ms)) } else { Json::Null },
            "text": self.text,
            "devices": self.rows.iter().map(|r| {
                let (state, ms, error) = match &r.verdict {
                    Verdict::Waiting => ("waiting", Json::Null, Json::Null),
                    Verdict::Answered { ms } => ("answered", Json::from(*ms), Json::Null),
                    Verdict::Failed(why) => ("failed", Json::Null, Json::from(why.as_str())),
                };
                json!({
                    "name": r.name,
                    "address": r.address,
                    "driver": r.driver,
                    "state": state,
                    "ms": ms,
                    "error": error,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

/// Why `host` cannot be reached from a node with these network settings
/// at `ip`, if it cannot: it is on another subnet and the node has no
/// gateway. `None` when the node may as well try (the same subnet, a
/// gateway set, or an address by DHCP, whose prefix this does not know).
#[must_use]
pub fn off_subnet(net: &Net, host: [u8; 4]) -> Option<String> {
    let Net::Static { ip, prefix, gateway } = net else {
        return None;
    };
    if *gateway != [0; 4] {
        return None;
    }
    let mask = match prefix {
        0 => 0,
        p if *p >= 32 => u32::MAX,
        p => u32::MAX << (32 - u32::from(*p)),
    };
    let (node, device) = (u32::from_be_bytes(*ip), u32::from_be_bytes(host));
    if node & mask == device & mask {
        return None;
    }
    Some(format!(
        "{}.{}.{}.{} is not on this node's network ({}.{}.{}.{}/{prefix}, no gateway): give the device an address there",
        host[0], host[1], host[2], host[3], ip[0], ip[1], ip[2], ip[3]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iomap::parse;

    const TWO: &str = "device rio modbus-tcp 192.168.3.60\ndevice drive enip-io 192.168.33.1 config=1 o2t=112:36 t2o=100:16\nin estop bool rio di:0\n";

    fn checked(verdicts: &[Verdict], done_ms: u64) -> Check {
        let mut c = Check::begin(&parse(TWO).unwrap());
        for (k, v) in verdicts.iter().enumerate() {
            c.set(k, v.clone());
        }
        c.finish(done_ms);
        c
    }

    #[test]
    fn a_check_begins_with_every_device_waiting_and_names_them_as_the_map_does() {
        let c = Check::begin(&parse(TWO).unwrap());
        assert!(c.running);
        assert_eq!(c.text, TWO);
        assert_eq!(
            c.rows
                .iter()
                .map(|r| (r.name.as_str(), r.address.as_str(), r.driver))
                .collect::<Vec<_>>(),
            [
                ("rio", "192.168.3.60:502", "modbus-tcp"),
                ("drive", "192.168.33.1:44818", "enip-io")
            ]
        );
        assert!(c.rows.iter().all(|r| r.verdict == Verdict::Waiting));
        assert_eq!(c.allows(TWO, 0), Err(Refusal::Running));
        assert!(!c.passed());
    }

    #[test]
    fn a_map_is_saved_only_after_every_device_of_that_exact_text_answered() {
        let ok = Verdict::Answered { ms: 12 };
        let pass = checked(&[ok.clone(), ok.clone()], 1_000);
        assert!(pass.passed());
        assert_eq!(pass.allows(TWO, 1_000), Ok(()));
        assert_eq!(pass.allows(TWO, 1_000 + FRESH_MS - 1), Ok(()));
        // One byte different is another map: a comment, a space, a newline.
        for other in [
            format!("{TWO}# later\n"),
            TWO.replace("di:0", "di:1"),
            TWO.trim_end().to_string(),
            format!(" {TWO}"),
            String::new(),
        ] {
            assert_eq!(pass.allows(&other, 1_000), Err(Refusal::NotChecked), "{other:?}");
        }
        // Too old, and a clock that went backwards.
        assert_eq!(pass.allows(TWO, 1_000 + FRESH_MS), Err(Refusal::Stale));
        assert_eq!(pass.allows(TWO, u64::MAX), Err(Refusal::Stale));
        assert_eq!(pass.allows(TWO, 999), Err(Refusal::Stale));
    }

    #[test]
    fn one_silent_device_refuses_the_map_and_is_named() {
        let ok = Verdict::Answered { ms: 3 };
        let one = checked(
            &[ok.clone(), Verdict::Failed("link: connect timed out".into())],
            5,
        );
        assert!(!one.passed());
        assert_eq!(one.allows(TWO, 5), Err(Refusal::Failed(vec!["drive".into()])));
        assert!(
            one.allows(TWO, 5)
                .unwrap_err()
                .to_string()
                .contains("drive did not answer")
        );
        // Cut short: a device nobody asked is not a pass.
        let cut = checked(&[ok], 5);
        assert_eq!(cut.allows(TWO, 5), Err(Refusal::Failed(vec!["drive".into()])));
        assert!(matches!(&cut.rows[1].verdict, Verdict::Failed(w) if w.contains("cut short")));
        // An index past the map changes nothing.
        let mut c = Check::begin(&parse(TWO).unwrap());
        c.set(9, Verdict::Answered { ms: 1 });
        assert!(c.rows.iter().all(|r| r.verdict == Verdict::Waiting));
    }

    #[test]
    fn no_check_at_all_allows_nothing_not_even_an_empty_text() {
        let none = Check::default();
        assert!(!none.passed());
        assert_eq!(none.allows("", 0), Err(Refusal::NotChecked));
        assert_eq!(none.allows(TWO, 0), Err(Refusal::NotChecked));
        assert_eq!(none.json(0)["state"], "none");
        assert_eq!(none.json(0)["may_save"], false);
    }

    #[test]
    fn the_json_says_who_answered_and_whether_the_map_may_be_saved() {
        let c = checked(
            &[
                Verdict::Answered { ms: 7 },
                Verdict::Failed("192.168.33.1 is not on this node's network".into()),
            ],
            2_000,
        );
        let j = c.json(2_500);
        assert_eq!(j["state"], "done");
        assert_eq!(j["passed"], false);
        assert_eq!(j["may_save"], false);
        assert_eq!(j["age_ms"], 500);
        assert_eq!(j["text"], TWO);
        assert_eq!(j["devices"][0]["state"], "answered");
        assert_eq!(j["devices"][0]["ms"], 7);
        assert_eq!(j["devices"][1]["state"], "failed");
        assert!(
            j["devices"][1]["error"]
                .as_str()
                .unwrap()
                .contains("not on this node's network")
        );
        let ok = Verdict::Answered { ms: 1 };
        let pass = checked(&[ok.clone(), ok], 0);
        assert_eq!(pass.json(10)["may_save"], true);
        assert_eq!(pass.json(FRESH_MS)["may_save"], false, "stale");
        assert_eq!(pass.json(FRESH_MS)["passed"], true);
        assert_eq!(Check::begin(&parse(TWO).unwrap()).json(0)["state"], "running");
    }

    #[test]
    fn a_device_on_another_subnet_is_told_apart_from_one_that_is_silent() {
        let bench = Net::Static {
            ip: [192, 168, 3, 56],
            prefix: 24,
            gateway: [0; 4],
        };
        assert_eq!(off_subnet(&bench, [192, 168, 3, 60]), None);
        let why = off_subnet(&bench, [192, 168, 33, 1]).unwrap();
        assert!(
            why.contains("192.168.33.1") && why.contains("192.168.3.56/24"),
            "{why}"
        );
        // A gateway, or DHCP, and the node tries.
        let routed = Net::Static {
            ip: [192, 168, 3, 56],
            prefix: 24,
            gateway: [192, 168, 3, 1],
        };
        assert_eq!(off_subnet(&routed, [10, 0, 0, 73]), None);
        assert_eq!(off_subnet(&Net::Dhcp, [10, 0, 0, 73]), None);
        // The edges of the prefix.
        let p = |prefix| Net::Static {
            ip: [10, 1, 2, 3],
            prefix,
            gateway: [0; 4],
        };
        assert_eq!(off_subnet(&p(0), [200, 9, 9, 9]), None);
        assert_eq!(off_subnet(&p(32), [10, 1, 2, 3]), None);
        assert!(off_subnet(&p(32), [10, 1, 2, 4]).is_some());
        assert!(
            off_subnet(&p(40), [10, 1, 2, 4]).is_some(),
            "a prefix past 32 is 32"
        );
        assert_eq!(off_subnet(&p(8), [10, 200, 0, 1]), None);
        assert!(off_subnet(&p(16), [10, 2, 0, 1]).is_some());
    }

    #[test]
    fn a_map_of_sim_points_has_no_device_to_ask_and_passes_once_finished() {
        let text = "in go bool sim\nout lamp bool sim\n";
        let mut c = Check::begin(&parse(text).unwrap());
        assert!(!c.passed(), "running");
        assert_eq!(c.allows(text, 0), Err(Refusal::Running));
        c.finish(100);
        assert!(c.passed());
        assert_eq!(c.allows(text, 150), Ok(()));
        // Only that text, and not a stale or an empty record.
        assert_eq!(c.allows("in go bool sim\n", 150), Err(Refusal::NotChecked));
        assert!(c.allows(text, 100 + FRESH_MS).is_err());
        assert!(!Check::default().passed());
    }
}
