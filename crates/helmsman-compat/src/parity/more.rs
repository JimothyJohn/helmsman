//! The rest of the box: proposals, history, bundles, audit, forces, trends, layouts, drivers, the site view and MCP.

use super::Journey;

/// This area's journeys; `None` is pending.
pub const JOURNEYS: &[(&str, Option<Journey>)] = &[
    ("proposals", None),
    ("history", None),
    ("bundles", None),
    ("audit", None),
    ("forces", None),
    ("trends", None),
    ("layout", None),
    ("drivers", None),
    ("site", None),
    ("mcp", None),
];
