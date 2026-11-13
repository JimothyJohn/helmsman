//! The lease file: one binding per line, so a block keeps its address
//! across box restarts.
//!
//! ```text
//! # helmsman-dhcp leases 1
//! 00-90-e8-7e-af-be 192.168.3.100 bound 1763042400
//! 00-07-46-01-02-03 192.168.3.101 bound forever
//! 00-07-46-01-02-04 192.168.3.102 released 1763040000
//! ```
//!
//! Offers are not written: one lasts a minute. A file is read whole or not
//! at all ([`parse`] names the first bad line), and the box writes it
//! atomically, as it does the console passwords.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::net::Ipv4Addr;

use crate::message::Mac;
use crate::server::{Binding, FOREVER, State};

/// The first line of every file.
pub const HEADER: &str = "# helmsman-dhcp leases 1";
/// The most lines a file may hold: a /16's worth, far past any device
/// network, so a corrupt or hostile file cannot make the box allocate
/// without bound.
pub const MAX_LINES: usize = 65_536;

/// Why a lease file was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based.
    pub line: usize,
    pub what: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lease file line {}: {}", self.line, self.what)
    }
}

impl core::error::Error for ParseError {}

/// The file's text for these bindings (offers left out), sorted by MAC.
#[must_use]
pub fn write<'a>(bindings: impl IntoIterator<Item = (&'a Mac, &'a Binding)>) -> String {
    let mut lines: Vec<String> = bindings
        .into_iter()
        .filter_map(|(mac, b)| {
            let state = match b.state {
                State::Offered => return None,
                State::Bound => "bound",
                State::Released => "released",
            };
            let until = if b.until == FOREVER {
                String::from("forever")
            } else {
                format!("{}", b.until)
            };
            Some(format!("{mac} {} {state} {until}", b.ip))
        })
        .collect();
    lines.sort();
    let mut out = String::from(HEADER);
    out.push('\n');
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Read a lease file.
///
/// # Errors
///
/// [`ParseError`] for a missing or unknown header, a line that is not
/// `MAC IP STATE UNTIL`, a MAC given twice, or more than [`MAX_LINES`].
pub fn parse(text: &str) -> Result<Vec<(Mac, Binding)>, ParseError> {
    let mut lines = text.lines().enumerate();
    match lines.next() {
        Some((_, h)) if h.trim_end() == HEADER => {}
        _ => {
            return Err(ParseError {
                line: 1,
                what: "not a helmsman-dhcp lease file, version 1",
            });
        }
    }
    let mut out: Vec<(Mac, Binding)> = Vec::new();
    for (i, line) in lines {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if out.len() >= MAX_LINES {
            return Err(ParseError {
                line: n,
                what: "more lines than a lease file holds",
            });
        }
        let err = |what| ParseError { line: n, what };
        let mut f = line.split_ascii_whitespace();
        let (Some(mac), Some(ip), Some(state), Some(until), None) =
            (f.next(), f.next(), f.next(), f.next(), f.next())
        else {
            return Err(err("a line is MAC IP STATE UNTIL"));
        };
        let mac: Mac = mac.parse().map_err(|_| err("not a MAC"))?;
        let ip: Ipv4Addr = ip.parse().map_err(|_| err("not an IPv4 address"))?;
        let state = match state {
            "bound" => State::Bound,
            "released" => State::Released,
            _ => return Err(err("state is bound or released")),
        };
        let until = match until {
            "forever" => FOREVER,
            t => match t.parse::<u64>() {
                Ok(v) if v != FOREVER => v,
                _ => return Err(err("until is seconds since the epoch or forever")),
            },
        };
        if out.iter().any(|(m, _)| *m == mac) {
            return Err(err("a MAC appears twice"));
        }
        out.push((mac, Binding { ip, state, until }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    fn mac(last: u8) -> Mac {
        Mac([0, 7, 0x46, 1, 2, last])
    }

    #[test]
    fn round_trip_leaves_offers_out() {
        let mut b = BTreeMap::new();
        b.insert(
            mac(3),
            Binding {
                ip: Ipv4Addr::new(192, 168, 3, 101),
                state: State::Bound,
                until: FOREVER,
            },
        );
        b.insert(
            mac(4),
            Binding {
                ip: Ipv4Addr::new(192, 168, 3, 102),
                state: State::Released,
                until: 7,
            },
        );
        b.insert(
            mac(5),
            Binding {
                ip: Ipv4Addr::new(192, 168, 3, 103),
                state: State::Offered,
                until: 9,
            },
        );
        let text = write(&b);
        assert_eq!(
            text,
            "# helmsman-dhcp leases 1\n00-07-46-01-02-03 192.168.3.101 bound forever\n00-07-46-01-02-04 192.168.3.102 released 7\n"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], (mac(3), b[&mac(3)]));
        assert_eq!(back[1], (mac(4), b[&mac(4)]));
    }

    #[test]
    fn bad_files_name_their_line() {
        let h = HEADER;
        for (text, line) in [
            (String::new(), 1),
            (String::from("# helmsman-dhcp leases 2\n"), 1),
            (format!("{h}\n00-07-46-01-02-03 192.168.3.101 bound\n"), 2),
            (format!("{h}\n00-07-46-01-02-03 192.168.3.101 bound 1 extra\n"), 2),
            (format!("{h}\n\n00-07-46-01-02 192.168.3.101 bound 1\n"), 3),
            (format!("{h}\n00-07-46-01-02-03 192.168.3.300 bound 1\n"), 2),
            (format!("{h}\n00-07-46-01-02-03 192.168.3.1 offered 1\n"), 2),
            (format!("{h}\n00-07-46-01-02-03 192.168.3.1 bound -1\n"), 2),
            (
                format!("{h}\n00-07-46-01-02-03 192.168.3.1 bound 18446744073709551615\n"),
                2,
            ),
            (
                format!(
                    "{h}\n00-07-46-01-02-03 192.168.3.1 bound 1\n00-07-46-01-02-03 192.168.3.2 bound 1\n"
                ),
                3,
            ),
        ] {
            assert_eq!(parse(&text).unwrap_err().line, line, "{text:?}");
        }
    }
}
