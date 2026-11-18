//! What `GET /api/firmware` reports: the running image, its trial, and
//! what a fetch is doing. The firmware keeps one [`Firmware`] in its
//! shared state; the router renders it.

use alloc::string::String;

use serde_json::json;

use super::boot::MAX_PENDING_BOOTS;
use super::request::encode_hex32;

/// Where the running image is in its trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trial {
    /// Confirmed, or never on trial (a USB flash).
    Confirmed,
    /// On trial; boot number `boots` of [`MAX_PENDING_BOOTS`].
    OnTrial { boots: u8 },
}

/// What the fetch task is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    Idle,
    /// Reading the image: `received` of `size` bytes (0 until the server
    /// has said how big).
    Fetching {
        received: u32,
        size: u32,
    },
    /// Written, checked and activated; the node restarts into it.
    Restarting {
        sha256: [u8; 32],
    },
    /// The last attempt, and why it ended. The running image is untouched.
    Failed(String),
}

impl Fetch {
    /// A fetch is running or the node is about to restart: a second
    /// request waits.
    #[must_use]
    pub const fn busy(&self) -> bool {
        matches!(self, Self::Fetching { .. } | Self::Restarting { .. })
    }
}

/// The firmware half of the node's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    /// The running image's version (the crate's).
    pub version: &'static str,
    pub chip: &'static str,
    /// The slot it runs from, 0 or 1; `None` if `otadata` could not be read.
    pub slot: Option<u8>,
    pub trial: Trial,
    pub fetch: Fetch,
}

impl Firmware {
    /// The JSON of `GET /api/firmware`.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        let trial = match self.trial {
            Trial::Confirmed => json!({ "state": "confirmed" }),
            Trial::OnTrial { boots } => {
                json!({ "state": "on_trial", "boots": boots, "max_boots": MAX_PENDING_BOOTS })
            }
        };
        let fetch = match &self.fetch {
            Fetch::Idle => json!({ "state": "idle" }),
            Fetch::Fetching { received, size } => {
                json!({ "state": "fetching", "received": received, "size": size })
            }
            Fetch::Restarting { sha256 } => json!({ "state": "restarting", "sha256": encode_hex32(sha256) }),
            Fetch::Failed(why) => json!({ "state": "failed", "error": why }),
        };
        json!({
            "version": self.version,
            "chip": self.chip,
            "slot": self.slot,
            "trial": trial,
            "fetch": fetch,
        })
    }
}
