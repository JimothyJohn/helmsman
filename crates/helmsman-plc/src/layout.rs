//! What an I/O map file and a calendar file load into, without starting
//! anything: the signal table (map signals in map order, then the
//! calendar's), the calendar bound to it, and the map. The box builds one
//! at startup and again for every change set that brings another map or
//! calendar, to check it (and replay against it) before anything runs it.

use helmsman_io::IoMap;

use crate::calendar::{self, Source};
use crate::signals::SignalTable;

/// A map and calendar, loaded and checked against each other.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub table: SignalTable,
    pub calendar: Option<Source>,
    /// `None`: no map (no signals but the calendar's).
    pub map: Option<IoMap>,
}

impl Layout {
    /// Loads `io_map` (the `helmsman_io::text` format) and `calendar` (the
    /// `calendar::text` format), as the box does at startup.
    ///
    /// # Errors
    /// Every problem, each line naming the file it is in (`io.map`,
    /// `calendar`).
    pub fn build(io_map: Option<&str>, calendar: Option<&str>) -> Result<Self, Vec<String>> {
        let (mut table, map) = match io_map {
            Some(src) => {
                let parsed = helmsman_io::text::parse(src).map_err(|errs| {
                    errs.iter()
                        .map(|(n, e)| format!("io.map:{n}: {e}"))
                        .collect::<Vec<_>>()
                })?;
                let table = SignalTable::new(&parsed.map, &parsed.safe)
                    .map_err(|errs| errs.iter().map(|e| format!("io.map: {e}")).collect::<Vec<_>>())?;
                (table, Some(parsed.map))
            }
            None => (SignalTable::default(), None),
        };
        let calendar = calendar
            .map(|src| {
                let cal = calendar::load(src)
                    .map_err(|errs| errs.iter().map(|e| format!("calendar: {e}")).collect::<Vec<_>>())?;
                Source::attach(cal, &mut table).map_err(|errs| {
                    errs.iter()
                        .map(|e| format!("calendar: does not fit the I/O map: {e}"))
                        .collect::<Vec<_>>()
                })
            })
            .transpose()?;
        Ok(Self { table, calendar, map })
    }
}
