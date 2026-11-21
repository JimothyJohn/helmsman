//! The agent routes: explain, requirements, chat, design and help, each a turn through the relay (the stub model on the candidate, one short real turn on live; docs/PARITY.md).

use super::Journey;

/// This area's journeys; `None` is pending.
pub const JOURNEYS: &[(&str, Option<Journey>)] = &[
    ("explain", None),
    ("requirements", None),
    ("chat", None),
    ("design", None),
    ("help", None),
];
