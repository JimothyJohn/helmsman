//! A skim of an I/O map's text (`helmsman_io::text`'s format): the
//! `device` lines and the `in`/`out` lines, as written, with no checking.
//! For listing what a project's map names (the canvas's Devices tab, the
//! agent's `list_devices`), not for running it: the box's `helmsman_io`
//! parser validates. Lines that are neither are counted and skipped, so a
//! half-typed map still lists what it has.

/// One `device ID DRIVER ENDPOINT [KEY=VALUE ...]` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapDevice {
    pub name: String,
    pub driver: String,
    pub endpoint: String,
    /// The driver's params, in order, as `KEY=VALUE` tokens.
    pub params: Vec<(String, String)>,
    /// Every token after the endpoint, as written (the `KEY=VALUE` ones
    /// among them are `params`), so an edit can keep what it does not know.
    pub settings: Vec<String>,
}

/// One `in|out NAME TYPE DEVICE POINT [KEY=VALUE ...]` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapSignal {
    /// `in` or `out`.
    pub dir: String,
    pub name: String,
    pub ty: String,
    pub device: String,
    pub point: String,
    pub unit: Option<String>,
    /// Every token after the point, as written (`safe=true`, `unit=mm`,
    /// `poll_ms=500`, ...), so an edit can keep what it does not know.
    pub tail: Vec<String>,
}

/// What a map's text names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapText {
    pub devices: Vec<MapDevice>,
    pub signals: Vec<MapSignal>,
    /// Non-blank lines that were neither (comments aside).
    pub odd: usize,
}

/// The device ids the format reserves: not devices anyone declares.
pub const RESERVED: [&str; 3] = ["box", "module", "screen"];

fn params(tokens: &[&str]) -> Vec<(String, String)> {
    tokens
        .iter()
        .filter_map(|t| t.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Skims `text`. Never fails: what does not parse is counted in `odd`.
#[must_use]
pub fn skim(text: &str) -> MapText {
    let mut out = MapText::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.as_slice() {
            ["device", name, driver, endpoint, rest @ ..] => out.devices.push(MapDevice {
                name: (*name).to_string(),
                driver: (*driver).to_string(),
                endpoint: (*endpoint).to_string(),
                params: params(rest),
                settings: rest.iter().map(|t| (*t).to_string()).collect(),
            }),
            [dir @ ("in" | "out"), name, ty, device, point, rest @ ..] => out.signals.push(MapSignal {
                dir: (*dir).to_string(),
                name: (*name).to_string(),
                ty: (*ty).to_string(),
                device: (*device).to_string(),
                point: (*point).to_string(),
                unit: params(rest)
                    .into_iter()
                    .find(|(k, _)| k == "unit")
                    .map(|(_, v)| v),
                tail: rest.iter().map(|t| (*t).to_string()).collect(),
            }),
            _ => out.odd += 1,
        }
    }
    out
}

/// Whether `name` can be a device id: an identifier, not a reserved one.
#[must_use]
pub fn device_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 64
        && !RESERVED.contains(&name)
}

/// Whether `endpoint` is an address `driver` can dial: for the network
/// drivers `host[:port]` with a port that is a `u16` above zero, a host
/// that is a valid IPv4 address when it looks like one (digits and dots)
/// and a hostname otherwise, or `[IPv6]` with an optional port. An
/// identity (`mac=...`, `cip=...`, which the driver resolves) and the
/// other drivers' addresses (a serial port's path) are theirs to check.
///
/// # Errors
/// What is wrong with it, in words.
pub fn endpoint_ok(driver: &str, endpoint: &str) -> Result<(), String> {
    if !matches!(driver, "modbus-tcp" | "enip-io")
        || endpoint.starts_with("mac=")
        || endpoint.starts_with("cip=")
    {
        return Ok(());
    }
    let (host, port) = if let Some(inner) = endpoint.strip_prefix('[') {
        let (h, rest) = inner
            .split_once(']')
            .ok_or("an IPv6 address is written [::1]:502")?;
        if h.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(format!("{h:?} is not an IPv6 address"));
        }
        let port = match rest {
            "" => None,
            r => Some(r.strip_prefix(':').ok_or("after ] comes :PORT")?),
        };
        return port_ok(port);
    } else {
        let mut parts = endpoint.splitn(3, ':');
        let (host, port, more) = (parts.next().unwrap_or(""), parts.next(), parts.next());
        if more.is_some() {
            return Err("the address is HOST or HOST:PORT (an IPv6 address goes in [ ])".into());
        }
        (host, port)
    };
    port_ok(port)?;
    let numeric = !host.is_empty() && host.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    if numeric {
        if host.parse::<std::net::Ipv4Addr>().is_err() {
            return Err(format!("{host:?} is not an IPv4 address"));
        }
    } else if host.is_empty()
        || host.len() > 253
        || host.starts_with(['.', '-'])
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(format!("{host:?} is not a host name or an IP address"));
    }
    Ok(())
}

fn port_ok(port: Option<&str>) -> Result<(), String> {
    match port {
        Some(p) if !(p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0)) => {
            Err(format!("the port is a number from 1 to 65535, not {p:?}"))
        }
        _ => Ok(()),
    }
}

/// A `device` line for the map, or why not: the name an identifier, the
/// endpoint one token, every setting `KEY=VALUE`.
///
/// # Errors
/// Why the line cannot be written, in words.
pub fn device_line(name: &str, driver: &str, endpoint: &str, params: &str) -> Result<String, String> {
    let name = name.trim();
    if !device_name_ok(name) {
        return Err(format!(
            "a device name is letters, digits and _ (not {:?}); {name:?} will not do",
            RESERVED.join(", ")
        ));
    }
    let endpoint = endpoint.trim();
    if endpoint.is_empty() || endpoint.split_whitespace().count() != 1 {
        return Err("the address is one word, like 192.168.1.50:502".into());
    }
    if driver.split_whitespace().count() != 1 {
        return Err("the protocol is one word".into());
    }
    if [driver, endpoint, params].iter().any(|t| t.contains('#')) {
        return Err(
            "a # starts a comment in io.map: the protocol, address and settings cannot hold one".into(),
        );
    }
    endpoint_ok(driver, endpoint)?;
    let mut line = format!("device {name} {driver} {endpoint}");
    for p in params.split_whitespace() {
        match p.split_once('=') {
            Some((k, v)) if !k.is_empty() && !v.is_empty() => {
                line.push(' ');
                line.push_str(p);
            }
            _ => return Err(format!("a setting is KEY=VALUE, not {p:?}")),
        }
    }
    line.push('\n');
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skims_devices_and_signals_and_counts_the_rest() {
        let m = skim(
            "# a comment\n\
             device rio modbus-tcp 10.0.0.57:502 unit=1 timeout_ms=300  # trailing\n\
             in  di0 bool rio di:0\n\
             out lamp bool box do:0 safe=false\n\
             in level real rio hr:5:i16 unit=mm poll_ms=500\n\
             \tdevice\tplc\tlogix-tag\t10.0.0.9\n\
             in short bool\n\
             nonsense\n\
             device onlyname\n",
        );
        assert_eq!(m.devices.len(), 2);
        assert_eq!(m.devices[0].name, "rio");
        assert_eq!(
            m.devices[0].params,
            vec![("unit".into(), "1".into()), ("timeout_ms".into(), "300".into())]
        );
        assert_eq!(m.devices[1].driver, "logix-tag");
        assert_eq!(m.signals.len(), 3);
        assert_eq!(m.signals[2].unit.as_deref(), Some("mm"));
        assert_eq!(m.signals[1].device, "box");
        assert_eq!(m.odd, 3);
        // What follows the endpoint or the point is kept as written.
        assert_eq!(m.devices[0].settings, ["unit=1", "timeout_ms=300"]);
        assert_eq!(m.signals[1].tail, ["safe=false"]);
        assert_eq!(m.signals[2].tail, ["unit=mm", "poll_ms=500"]);
    }

    #[test]
    fn garbage_never_panics() {
        for text in [
            "",
            "\n\n",
            "#",
            "device",
            "in in in in in in in",
            "\u{0}\u{1}device x y z",
            &"device a b c ".repeat(10_000),
            &"x".repeat(1 << 16),
            "device a=b c=d e=f g=h",
            "in \u{feff}name bool box di:0",
        ] {
            let _ = skim(text);
        }
    }

    #[test]
    fn a_device_line_is_checked_before_it_is_written() {
        assert_eq!(
            device_line(" rio ", "modbus-tcp", "10.0.0.57:502", "unit=1 timeout_ms=300").unwrap(),
            "device rio modbus-tcp 10.0.0.57:502 unit=1 timeout_ms=300\n"
        );
        assert_eq!(
            device_line("e1", "enip-io", "10.0.0.57", "").unwrap(),
            "device e1 enip-io 10.0.0.57\n"
        );
        for bad in [
            "",
            "1rio",
            "box",
            "screen",
            "module",
            "a b",
            "r-i-o",
            "ñ",
            &"a".repeat(65),
        ] {
            assert!(device_line(bad, "modbus-tcp", "h:502", "").is_err(), "{bad:?}");
        }
        assert!(device_line("rio", "modbus-tcp", "", "").is_err());
        assert!(device_line("rio", "modbus-tcp", "a b", "").is_err());
        assert!(device_line("rio", "modbus tcp", "a", "").is_err());
        assert!(device_line("rio", "modbus-tcp", "a", "unit").is_err());
        assert!(device_line("rio", "modbus-tcp", "a", "=1").is_err());
        // The address is dialled: a host and a port that exist.
        for bad in [
            "999.1.1.1:99999",
            "10.0.0.57:0",
            "10.0.0.57:65536",
            "host:abc",
            "host:",
            ":502",
            "300.1.1.1",
            "1.2.3",
            "a:b:c",
            "[::1",
            "[zz]:502",
            "ho st",
            "ho$t:502",
            "host#:502",
        ] {
            assert!(device_line("rio", "modbus-tcp", bad, "").is_err(), "{bad:?}");
        }
        for good in [
            "10.0.0.57",
            "10.0.0.57:502",
            "plc-1.local:44818",
            "bench_pc",
            "[::1]:502",
            "[fe80::1]",
        ] {
            assert!(device_line("rio", "enip-io", good, "").is_ok(), "{good:?}");
        }
        // An identity is the driver's to resolve, a #  never fits a line.
        assert!(device_line("rio", "enip-io", "cip=991:515", "").is_ok());
        assert!(device_line("rio", "enip-io", "mac=00-90-e8-7e-af-be", "").is_ok());
        assert!(device_line("rio", "modbus-tcp", "a", "unit=1#x").is_err());
        assert!(device_line("rio", "modbus#tcp", "a", "").is_err());
        assert!(device_line("rio", "modbus-tcp", "a", "unit=").is_err());
        // A line written reads back as one device.
        let line = device_line("rio", "modbus-tcp", "h:502", "unit=1").unwrap();
        let m = skim(&line);
        assert_eq!(m.devices.len(), 1);
        assert_eq!(m.odd, 0);
    }
}
