//! The text a device's modules are written in, on a box's device line
//! (`slots=`, `records=`) and in a node's build settings: one syntax for
//! both, so a line tried on one runs on the other.
//!
//! - Modules are separated by commas; each is `SLOT:MODULE` followed by
//!   one or more `/SUBSLOT:SUBMODULE[:iN][:oN]`: the ident numbers as the
//!   device's GSDML gives them (decimal or `0x` hex), and the bytes of
//!   input and output data. A submodule with neither carries no data.
//! - Records are separated by semicolons; each is
//!   `SLOT.SUBSLOT.INDEX:HEX`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::rt::{Module, Record, Submodule};

/// A number, decimal or `0x` hex.
#[must_use]
pub fn number(s: &str) -> Option<u32> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return (!hex.is_empty() && hex.len() <= 8)
            .then(|| u32::from_str_radix(hex, 16).ok())
            .flatten();
    }
    (!s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// A number up to 65535, decimal or `0x` hex.
#[must_use]
pub fn number16(s: &str) -> Option<u16> {
    number(s).and_then(|n| u16::try_from(n).ok())
}

/// The modules of a `slots=` text.
///
/// # Errors
/// What is wrong with it, in words that name the entry.
pub fn slots(text: &str) -> Result<Vec<Module>, String> {
    let mut modules: Vec<Module> = Vec::new();
    for entry in text.split(',') {
        let bad = |why: &str| format!("slots: {entry:?}: {why}");
        let mut parts = entry.split('/');
        let head = parts.next().unwrap_or_default();
        let (slot, ident) = head
            .split_once(':')
            .ok_or_else(|| bad("want SLOT:MODULE/SUBSLOT:SUBMODULE"))?;
        let slot = number16(slot).ok_or_else(|| bad("the slot is not a number up to 65535"))?;
        let ident = number(ident).ok_or_else(|| bad("the module ident is not a number"))?;
        if modules.iter().any(|m| m.slot == slot) {
            return Err(bad("the slot is listed twice"));
        }
        let mut submodules: Vec<Submodule> = Vec::new();
        for sub in parts {
            let mut fields = sub.split(':');
            let subslot = fields
                .next()
                .and_then(number16)
                .ok_or_else(|| bad("a subslot is not a number up to 65535"))?;
            let ident = fields
                .next()
                .and_then(number)
                .ok_or_else(|| bad("a submodule ident is missing or not a number"))?;
            let (mut input, mut output) = (0, 0);
            for f in fields {
                let len = |n: &str| number16(n).filter(|n| (1..=1440).contains(n));
                match (f.split_at_checked(1), f) {
                    (Some(("i", n)), _) if input == 0 => {
                        input = len(n).ok_or_else(|| bad("iN wants 1..=1440 bytes"))?;
                    }
                    (Some(("o", n)), _) if output == 0 => {
                        output = len(n).ok_or_else(|| bad("oN wants 1..=1440 bytes"))?;
                    }
                    _ => return Err(bad("after the submodule ident come iN and oN, once each")),
                }
            }
            if submodules.iter().any(|s| s.subslot == subslot) {
                return Err(bad("a subslot is listed twice"));
            }
            submodules.push(Submodule {
                subslot,
                ident,
                input,
                output,
                records: Vec::new(),
            });
        }
        if submodules.is_empty() {
            return Err(bad("a module needs at least one /SUBSLOT:SUBMODULE"));
        }
        modules.push(Module {
            api: 0,
            slot,
            ident,
            submodules,
        });
    }
    Ok(modules)
}

/// Adds the records of a `records=` text to `modules`.
///
/// # Errors
/// What is wrong with it, in words that name the entry.
pub fn records(text: &str, modules: &mut [Module]) -> Result<(), String> {
    for entry in text.split(';') {
        let bad = |why: &str| format!("records: {entry:?}: {why}");
        let (place, hex) = entry
            .split_once(':')
            .ok_or_else(|| bad("want SLOT.SUBSLOT.INDEX:HEX"))?;
        let mut parts = place.split('.');
        let (Some(slot), Some(subslot), Some(index), None) = (
            parts.next().and_then(number16),
            parts.next().and_then(number16),
            parts.next().and_then(number16),
            parts.next(),
        ) else {
            return Err(bad("want SLOT.SUBSLOT.INDEX:HEX"));
        };
        if hex.is_empty() || hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad("the data is not whole bytes of hex"));
        }
        let data = (0..hex.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect();
        let sub = modules
            .iter_mut()
            .find(|m| m.slot == slot)
            .and_then(|m| m.submodules.iter_mut().find(|s| s.subslot == subslot))
            .ok_or_else(|| bad("no such slot and subslot in slots="))?;
        sub.records.push(Record { index, data });
    }
    Ok(())
}
