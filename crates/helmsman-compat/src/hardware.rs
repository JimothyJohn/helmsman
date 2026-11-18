//! helmsman ↔ helmsman-hardware. The node board (node-p4-poe) takes the
//! ESP32-P4's RMII and management pins that the node firmware
//! (crates/helmsman-node-p4) already drives on the Waveshare ESP32-P4-ETH,
//! so one image needs only a `board.rs` for the switch in place of the PHY
//! (helmsman-hardware README, docs/design.md "Firmware"). Here the
//! firmware's own wiring, each `field: peripherals.GPIOn` in its
//! binaries, is held to the board's exported netlist
//! (`hardware/node-p4-poe/node-p4-poe.net`, which that repository's tests
//! hold to its generator): the module pin named `GPIOn` sits on the net
//! the field is for. And GPIO54, which would release the module's radio
//! from reset if driven high, is never named by the firmware.

use std::collections::BTreeMap;
use std::path::Path;

use crate::report::Finding;

/// The netlist the board's repository exports.
pub const NETLIST: &str = "hardware/node-p4-poe/node-p4-poe.net";
/// The module's reference designator on the board.
pub const MODULE: &str = "M1";

/// Each firmware field and the board net it must reach. The reset line
/// is the net named for its pin (GPIO51), which reaches the switch's
/// reset through a diode, checked by [`reaches`].
pub const EXPECTED: &[(&str, &str)] = &[
    ("ref_clk", "REF_CLK"),
    ("rxd0", "RMII_RXD0"),
    ("rxd1", "RMII_RXD1"),
    ("rx_dv", "RMII_CRS_DV"),
    ("txd0", "RMII_TXD0"),
    ("txd1", "RMII_TXD1"),
    ("tx_en", "RMII_TX_EN"),
    ("mdc", "MDC"),
    ("mdio", "MDIO"),
    ("phy_reset", "GPIO51"),
];

/// The net the switch's reset input is on.
pub const SWITCH_RESET: &str = "SW_RSTN";
/// The pin the firmware must leave alone.
pub const FORBIDDEN_GPIO: u32 = 54;

/// One node of a net: (reference, pin, pin function).
pub type Node = (String, String, String);

/// Net name (leading `/` dropped) → its nodes, from a `KiCad` netlist.
///
/// # Errors
/// No `(nets` section.
pub fn parse_netlist(text: &str) -> Result<BTreeMap<String, Vec<Node>>, String> {
    let start = text.find("(nets").ok_or("no (nets section in the netlist")?;
    let mut nets: BTreeMap<String, Vec<Node>> = BTreeMap::new();
    let mut net: Option<String> = None;
    let mut node: Option<(String, String, String)> = None;
    let mut in_node = false;
    let quoted = |line: &str, key: &str| -> Option<String> {
        let rest = line.trim().strip_prefix(&format!("({key} \""))?;
        Some(rest.split('"').next()?.to_string())
    };
    for line in text[start..].lines() {
        let t = line.trim();
        if t == "(net" {
            if let (Some(n), Some(nd)) = (&net, node.take()) {
                nets.entry(n.clone()).or_default().push(nd);
            }
            net = None;
            in_node = false;
        } else if t == "(node" {
            if let (Some(n), Some(nd)) = (&net, node.take()) {
                nets.entry(n.clone()).or_default().push(nd);
            }
            node = Some((String::new(), String::new(), String::new()));
            in_node = true;
        } else if let Some(name) = quoted(t, "name")
            && !in_node
        {
            let name = name.trim_start_matches('/').to_string();
            nets.entry(name.clone()).or_default();
            net = Some(name);
        } else if let Some(nd) = node.as_mut() {
            if let Some(r) = quoted(t, "ref") {
                nd.0 = r;
            } else if let Some(p) = quoted(t, "pin") {
                nd.1 = p;
            } else if let Some(f) = quoted(t, "pinfunction") {
                nd.2 = f;
            }
        }
    }
    if let (Some(n), Some(nd)) = (&net, node.take()) {
        nets.entry(n.clone()).or_default().push(nd);
    }
    Ok(nets)
}

/// The net the module pin named `GPIOn` is on (pin functions read
/// `GPIOn_<pad>`).
#[must_use]
pub fn net_of_gpio(nets: &BTreeMap<String, Vec<Node>>, gpio: u32) -> Option<&str> {
    let want = format!("GPIO{gpio}_");
    nets.iter()
        .find(|(_, nodes)| nodes.iter().any(|(r, _, f)| r == MODULE && f.starts_with(&want)))
        .map(|(n, _)| n.as_str())
}

/// Whether net `from` reaches net `to` through one two-pin part.
#[must_use]
pub fn reaches(nets: &BTreeMap<String, Vec<Node>>, from: &str, to: &str) -> bool {
    let parts: Vec<&str> = nets
        .get(from)
        .map(|n| n.iter().map(|(r, _, _)| r.as_str()).collect())
        .unwrap_or_default();
    nets.get(to)
        .is_some_and(|n| n.iter().any(|(r, _, _)| parts.contains(&r.as_str())))
}

/// Every `field: peripherals.GPIOn` in a firmware source text.
#[must_use]
pub fn wiring(src: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        let Some((field, rest)) = t.split_once(':') else {
            continue;
        };
        let Some(n) = rest.trim().strip_prefix("peripherals.GPIO") else {
            continue;
        };
        let digits: String = n.chars().take_while(char::is_ascii_digit).collect();
        if let (Ok(n), true) = (
            digits.parse(),
            field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        ) {
            out.push((field.to_string(), n));
        }
    }
    out
}

/// Whether a source text names `GPIOn` as a whole word.
#[must_use]
pub fn names_gpio(src: &str, n: u32) -> bool {
    let want = format!("GPIO{n}");
    src.match_indices(&want).any(|(i, _)| {
        let after = src[i + want.len()..].chars().next();
        !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// Holds the firmware sources (`name`, text) to the netlist.
#[must_use]
pub fn judge(netlist: &BTreeMap<String, Vec<Node>>, sources: &[(String, String)]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut wired: BTreeMap<String, (u32, String)> = BTreeMap::new();
    for (name, src) in sources {
        for (field, n) in wiring(src) {
            if let Some((was, at)) = wired.get(&field)
                && *was != n
            {
                out.push(Finding::fail(
                    format!("firmware {field}"),
                    format!("{at} wires it to GPIO{was}, {name} to GPIO{n}"),
                ));
            }
            wired.entry(field).or_insert((n, name.clone()));
        }
        if names_gpio(src, FORBIDDEN_GPIO) {
            out.push(Finding::fail(
                format!("firmware GPIO{FORBIDDEN_GPIO}"),
                format!("{name} names GPIO{FORBIDDEN_GPIO}: on node-p4-poe it holds the module's radio in reset and must stay an input or low"),
            ));
        }
    }
    for (field, net) in EXPECTED {
        let check = format!("pin {field}");
        let Some((n, at)) = wired.get(*field) else {
            out.push(Finding::fail(
                check,
                format!("the firmware does not wire {field} (expected on net {net})"),
            ));
            continue;
        };
        match net_of_gpio(netlist, *n) {
            Some(got) if got == *net => {
                if *field == "phy_reset" && !reaches(netlist, got, SWITCH_RESET) {
                    out.push(Finding::fail(
                        check,
                        format!("GPIO{n} is on {got}, which does not reach {SWITCH_RESET}"),
                    ));
                } else {
                    out.push(Finding::pass(check, format!("GPIO{n} ({at}) is on {got}")));
                }
            }
            Some(got) => out.push(Finding::fail(
                check,
                format!("GPIO{n} ({at}) is on board net {got}, expected {net}"),
            )),
            None => out.push(Finding::fail(
                check,
                format!("the board's {MODULE} has no pin GPIO{n} on any net"),
            )),
        }
    }
    if !out.iter().any(|f| f.check.starts_with("firmware GPIO")) {
        out.push(Finding::pass(
            format!("firmware GPIO{FORBIDDEN_GPIO}"),
            "never named",
        ));
    }
    out
}

/// Runs the contract on two checkouts.
///
/// # Errors
/// The netlist or the firmware sources cannot be read.
pub fn check(helmsman: &Path, hardware: &Path) -> Result<Vec<Finding>, String> {
    let path = hardware.join(NETLIST);
    if !hardware.join("hardware/node-p4-poe").exists() {
        // A board ref from before the KiCad project (the design package
        // alone) has no wiring to hold the firmware to. Once the project
        // exists a missing netlist fails below.
        return Ok(vec![Finding::pass(
            "board netlist",
            "n/a: no KiCad project (hardware/node-p4-poe) at this ref, so no pin contract yet",
        )]);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{NETLIST}: {e}"))?;
    let nets = parse_netlist(&text)?;
    let fw = helmsman.join("crates/helmsman-node-p4/src");
    let mut sources = Vec::new();
    collect_rs(&fw, &fw, &mut sources)?;
    if sources.is_empty() {
        return Err(format!("no firmware sources under {}", fw.display()));
    }
    Ok(judge(&nets, &sources))
}

fn collect_rs(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_rs(root, &p, out)?;
        } else if p.extension().is_some_and(|x| x == "rs") {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p.strip_prefix(root).unwrap_or(&p).display().to_string();
            out.push((name, text));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// A netlist in the `KiCad` layout with the nets the check reads.
    fn netlist(rxd0_pin_function: &str) -> String {
        let mut s = String::from("(export\n\t(components\n\t\t(comp (ref \"M1\"))\n\t)\n\t(nets\n");
        let mut net = |name: &str, nodes: &[(&str, &str, &str)]| {
            let _ = write!(
                s,
                "\t\t(net\n\t\t\t(code \"1\")\n\t\t\t(name \"/{name}\")\n\t\t\t(class \"Default\")\n"
            );
            for (r, p, f) in nodes {
                let _ = write!(
                    s,
                    "\t\t\t(node\n\t\t\t\t(ref \"{r}\")\n\t\t\t\t(pin \"{p}\")\n\t\t\t\t(pinfunction \"{f}\")\n\t\t\t\t(pintype \"passive\")\n\t\t\t)\n"
                );
            }
            s.push_str("\t\t)\n");
        };
        net(
            "REF_CLK",
            &[("M1", "78", "GPIO50_78"), ("U3", "23", "REFCLKI_23")],
        );
        net(
            "RMII_RXD0",
            &[("M1", "56", rxd0_pin_function), ("U3", "29", "SMRXD30_29")],
        );
        net("RMII_RXD1", &[("M1", "57", "GPIO30_57")]);
        net("RMII_CRS_DV", &[("M1", "55", "GPIO28_55")]);
        net("RMII_TXD0", &[("M1", "61", "GPIO34_61")]);
        net("RMII_TXD1", &[("M1", "62", "GPIO35_62")]);
        net("RMII_TX_EN", &[("M1", "77", "GPIO49_77")]);
        net("MDC", &[("M1", "58", "GPIO31_58")]);
        net("MDIO", &[("M1", "80", "GPIO52_80")]);
        net("GPIO51", &[("M1", "79", "GPIO51_79"), ("D6", "1", "K")]);
        net("SW_RSTN", &[("D6", "2", "A"), ("U3", "34", "RSTN_34")]);
        s.push_str("\t)\n)\n");
        s
    }

    const FIRMWARE: &str = "    let pins = Pins {
        ref_clk: peripherals.GPIO50,
        rxd0: peripherals.GPIO29,
        rxd1: peripherals.GPIO30,
        rx_dv: peripherals.GPIO28,
        txd0: peripherals.GPIO34,
        txd1: peripherals.GPIO35,
        tx_en: peripherals.GPIO49,
        mdc: peripherals.GPIO31,
        mdio: peripherals.GPIO52,
        phy_reset: peripherals.GPIO51,
    };";

    fn src(text: &str) -> Vec<(String, String)> {
        vec![("bin/node.rs".into(), text.into())]
    }

    #[test]
    fn the_netlist_reads_nets_and_their_module_pins() {
        let nets = parse_netlist(&netlist("GPIO29_56")).unwrap();
        assert_eq!(net_of_gpio(&nets, 29), Some("RMII_RXD0"));
        assert_eq!(net_of_gpio(&nets, 2), None);
        assert!(reaches(&nets, "GPIO51", "SW_RSTN"));
        assert!(!reaches(&nets, "MDC", "SW_RSTN"));
        assert!(parse_netlist("(export)").is_err());
    }

    #[test]
    fn matching_wiring_passes_every_pin() {
        let f = judge(&parse_netlist(&netlist("GPIO29_56")).unwrap(), &src(FIRMWARE));
        assert!(f.iter().all(|f| f.ok), "{f:?}");
        assert_eq!(f.len(), EXPECTED.len() + 1);
    }

    #[test]
    fn a_pin_moved_on_the_board_fails_its_field() {
        // The board puts RMII_RXD0 on GPIO33: the firmware's GPIO29 is
        // on no RXD0 net any more.
        let f = judge(&parse_netlist(&netlist("GPIO33_56")).unwrap(), &src(FIRMWARE));
        let failed: Vec<&str> = f.iter().filter(|f| !f.ok).map(|f| f.check.as_str()).collect();
        assert_eq!(failed, ["pin rxd0"], "{f:?}");
    }

    #[test]
    fn a_pin_moved_in_the_firmware_fails_its_field() {
        let fw = FIRMWARE.replace("mdc: peripherals.GPIO31", "mdc: peripherals.GPIO52");
        let f = judge(&parse_netlist(&netlist("GPIO29_56")).unwrap(), &src(&fw));
        let bad = f.iter().find(|f| f.check == "pin mdc").unwrap();
        assert!(!bad.ok && bad.detail.contains("board net MDIO"), "{bad:?}");
    }

    #[test]
    fn two_binaries_that_disagree_fail() {
        let other = FIRMWARE.replace("txd0: peripherals.GPIO34", "txd0: peripherals.GPIO36");
        let sources = vec![
            ("bin/node.rs".into(), FIRMWARE.into()),
            ("bin/profinet.rs".into(), other),
        ];
        let f = judge(&parse_netlist(&netlist("GPIO29_56")).unwrap(), &sources);
        assert!(f.iter().any(|f| !f.ok && f.check == "firmware txd0"), "{f:?}");
    }

    #[test]
    fn naming_the_radio_reset_pin_fails_but_gpio540_does_not() {
        let nets = parse_netlist(&netlist("GPIO29_56")).unwrap();
        let f = judge(&nets, &src(&format!("{FIRMWARE}\nlet led = peripherals.GPIO54;")));
        assert!(f.iter().any(|f| !f.ok && f.check == "firmware GPIO54"), "{f:?}");
        assert!(!names_gpio("GPIO540 GPIO5", 54));
    }

    #[test]
    fn a_missing_field_fails() {
        let fw = FIRMWARE.replace("        mdio: peripherals.GPIO52,\n", "");
        let f = judge(&parse_netlist(&netlist("GPIO29_56")).unwrap(), &src(&fw));
        assert!(f.iter().any(|f| !f.ok && f.check == "pin mdio"), "{f:?}");
    }
}
