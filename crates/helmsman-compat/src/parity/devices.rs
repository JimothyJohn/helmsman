//! The device journeys, on simulators only (the Moxa `E1200` sim, the `OpENer` container), never a bench device (docs/PARITY.md).

use super::Journey;

/// This area's journeys; `None` is pending.
pub const JOURNEYS: &[(&str, Option<Journey>)] = &[
    ("devices", None),
    ("device-config", None),
    ("device-profile", None),
];
