//! What an I/O map file and a calendar file load into, without starting
//! anything: the signal table (map signals in map order, then the
//! calendar's), the calendar bound to it, and the map. The box builds one
//! at startup and again for every change set that brings another map or
//! calendar, to check it (and replay against it) before anything runs it.

use helmsman_io::IoMap;

use crate::calendar::{self, Source};
use crate::drivers::{self, Drivers, Points};
use crate::signals::SignalTable;

/// A map and calendar, loaded and checked against each other and against
/// the WASM drivers the map may name.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub table: SignalTable,
    pub calendar: Option<Source>,
    /// `None`: no map (no signals but the calendar's).
    pub map: Option<IoMap>,
    /// The approved WASM drivers the map was checked against, which its
    /// links open on (`wasm:NAME`).
    pub drivers: Drivers,
}

impl Layout {
    /// Loads `io_map` (the `helmsman_io::text` format) and `calendar` (the
    /// `calendar::text` format), as the box does at startup, with no WASM
    /// drivers.
    ///
    /// # Errors
    /// Every problem, each line naming the file it is in (`io.map`,
    /// `calendar`).
    pub fn build(io_map: Option<&str>, calendar: Option<&str>) -> Result<Self, Vec<String>> {
        Self::build_with(io_map, calendar, &Drivers::new())
    }

    /// [`Layout::build`] with the approved WASM `drivers`: a device naming
    /// `wasm:NAME` must name one of them, and its points are parsed by
    /// that guest.
    ///
    /// # Errors
    /// As [`Layout::build`], plus every device naming a driver that is not
    /// in `drivers`.
    pub fn build_with(
        io_map: Option<&str>,
        calendar: Option<&str>,
        drivers: &Drivers,
    ) -> Result<Self, Vec<String>> {
        let (mut table, map) = match io_map {
            Some(src) => {
                let parsed = helmsman_io::text::parse(src).map_err(|errs| {
                    errs.iter()
                        .map(|(n, e)| format!("io.map:{n}: {e}"))
                        .collect::<Vec<_>>()
                })?;
                let mut points = Points::new(drivers);
                let mut errs: Vec<String> = drivers::unknown(&parsed.map, drivers)
                    .into_iter()
                    .map(|e| format!("io.map: {e}"))
                    .collect();
                let table =
                    SignalTable::with_points(&parsed.map, &parsed.safe, &mut |d, a| points.parse(d, a));
                let table = match table {
                    Ok(t) if errs.is_empty() => t,
                    Ok(_) => return Err(errs),
                    Err(e) => {
                        errs.extend(e.iter().map(|e| format!("io.map: {e}")));
                        return Err(errs);
                    }
                };
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
        Ok(Self {
            table,
            calendar,
            map,
            drivers: drivers.clone(),
        })
    }
}
