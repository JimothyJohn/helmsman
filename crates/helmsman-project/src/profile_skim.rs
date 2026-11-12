//! A device profile's channels, read for a listing (the canvas's add by
//! click, PROTOCOLS.md step 3): `helmsman-device 1`, the vendor and
//! model, the transports in order, and each `in` / `out` channel with its
//! type, unit and safe value. Nothing here checks a profile; the box
//! holds it to `helmsman_io::profile::Profile::parse` when the map loads,
//! and that is what a profile is accepted through. This reads only what a
//! person is shown before they add the device, so the canvas, which
//! ships without helmsman-io, can show it.

/// The first line of a profile.
pub const HEADER: &str = "helmsman-device 1";

/// One channel, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// `in` or `out`.
    pub dir: String,
    pub name: String,
    /// `bool`, `int` or `real`.
    pub ty: String,
    pub unit: Option<String>,
    /// An output's value when the device is lost, as written.
    pub safe: Option<String>,
}

/// What a profile says of a device, for a listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Skim {
    pub vendor: String,
    pub model: String,
    pub kind: String,
    /// The drivers that reach it, best first.
    pub transports: Vec<String>,
    pub channels: Vec<Channel>,
}

/// The signal a channel becomes in a map: the device's id, an underscore,
/// and the channel's name in lower case with `-` as `_` (`rio` and
/// `DI-04-count` give `rio_di_04_count`); the same rule as
/// `helmsman_io::profile::signal_name`.
#[must_use]
pub fn signal_name(device: &str, channel: &str) -> String {
    format!("{device}_{}", channel.to_ascii_lowercase().replace('-', "_"))
}

/// Reads the listing out of a profile's text.
///
/// # Errors
/// Not a profile (its first line), or a channel line too short to list;
/// anything else is left to the box's check.
pub fn skim(text: &str) -> Result<Skim, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some(HEADER) {
        return Err(format!("not a device profile: the first line is not {HEADER:?}"));
    }
    let mut s = Skim::default();
    for (n, raw) in lines.enumerate() {
        let line = raw.split('#').next().unwrap_or_default();
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["vendor", rest @ ..] => s.vendor = rest.join(" "),
            ["model", rest @ ..] => s.model = rest.join(" "),
            ["kind", k] => s.kind = (*k).to_string(),
            ["transport", driver, ..] => s.transports.push((*driver).to_string()),
            [dir @ ("in" | "out"), name, ty, rest @ ..] => {
                let value = |key: &str| {
                    rest.iter()
                        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
                        .map(str::to_string)
                };
                s.channels.push(Channel {
                    dir: (*dir).to_string(),
                    name: (*name).to_string(),
                    ty: (*ty).to_string(),
                    unit: value("unit"),
                    safe: value("safe"),
                });
            }
            [w, ..] if matches!(*w, "in" | "out") => {
                return Err(format!("line {}: {w}: too few words for a channel", n + 2));
            }
            _ => {}
        }
    }
    if s.channels.is_empty() {
        return Err("no channels".into());
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");

    #[test]
    fn the_e1213_lists_forty_channels_and_two_transports() {
        let s = skim(E1213).unwrap();
        assert_eq!(
            (s.vendor.as_str(), s.model.as_str(), s.kind.as_str()),
            ("Moxa", "ioLogik E1213", "remote-io")
        );
        assert_eq!(s.transports, ["enip-io", "modbus-tcp"]);
        assert_eq!(s.channels.len(), 40);
        let c = &s.channels[0];
        assert_eq!(
            (c.dir.as_str(), c.name.as_str(), c.ty.as_str()),
            ("in", "DI-00", "bool")
        );
        let o = s.channels.iter().find(|c| c.name == "DO-03").unwrap();
        assert_eq!((o.dir.as_str(), o.safe.as_deref()), ("out", Some("false")));
        assert_eq!(signal_name("rio", "DI-04-count"), "rio_di_04_count");
    }

    #[test]
    fn what_is_not_a_profile_is_refused_in_words() {
        assert!(skim("vendor X\n").unwrap_err().contains("first line"));
        assert!(
            skim("helmsman-device 1\nvendor X\n")
                .unwrap_err()
                .contains("no channels")
        );
        assert!(
            skim("helmsman-device 1\nin DI-00\n")
                .unwrap_err()
                .contains("line 2: in: too few words")
        );
    }
}
