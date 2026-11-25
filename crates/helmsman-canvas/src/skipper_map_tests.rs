//! The map a deploy sends a Skipper ([`crate::skipper_map`]). Every map
//! made here is read back by the Skipper's own parser
//! (`helmsman_nodecore::iomap::parse`) and the box's (`helmsman_io::text`).

use crate::skipper_map::for_skipper;

/// The bench Skipper's list as it ran on 2026-11-24.
const NODE: &str = "# bench 2026-11-24: the Novanta MDrive on EtherNet/IP\ndevice drive enip-io 192.168.33.1 config=1 o2t=112:36 t2o=100:16 rpi_ms=20\nin  enabled  bool drive in:0.0\nin  moving   bool drive in:0.1\nin  position int  drive in:4:i32\nin  fault    int  drive in:8:i32\nout vmax     int  drive out:8:u32\nout enable   int  drive out:17:u8\nout current  int  drive out:18:u8\nout slew     int  drive out:20:i32\n";

/// Nick's project `ass` as Scan and add left it (the deploy that failed:
/// `driver "device:moxa-iologik-e1213": a node drives modbus-tcp and
/// enip-io devices`).
const ASS: &str = "device iologik_e1213 device:moxa-iologik-e1213 cip=991:515:0x2a629eae\nin  iologik_e1213_di_00 bool iologik_e1213 ch:DI-00\n\ndevice liberty_lexium_mdrive_ethernet_lmd_ethernet_ip device:novanta-mdrive cip=243:15362:0x3cad8920\nin  liberty_lexium_mdrive_ethernet_lmd_ether bool liberty_lexium_mdrive_ethernet_lmd_ethernet_ip ch:enabled\n# placeholders: no device has these points yet (set the inputs on the I/O tab)\nin  start bool sim\nin  stop bool sim\nout running bool sim\nout motor_fwd bool sim\nout motor_rev bool sim\nout di0_active bool sim\n";

const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");
const MDRIVE: &str = include_str!("../../../docs/devices/novanta-mdrive.device");

fn profiles(stem: &str) -> Option<String> {
    match stem {
        "moxa-iologik-e1213" => Some(E1213.to_string()),
        "novanta-mdrive" => Some(MDRIVE.to_string()),
        _ => None,
    }
}

/// What the bench scan found.
fn scan(identity: &str) -> Option<String> {
    match identity {
        "cip=991:515:0x2a629eae" => Some("192.168.33.60".into()),
        "cip=243:15362:0x3cad8920" => Some("192.168.33.1".into()),
        _ => None,
    }
}

fn nothing(_: &str) -> Option<String> {
    None
}

/// The Skipper takes it, and so does a box. `sim` lines are left out of
/// the parse: the parsers on this branch predate them (the Skipper's and
/// the box's halves, #630 and #629, add them with their own tests).
fn both_take(map: &str) {
    let mut kept = String::new();
    for l in map.lines().filter(|l| l.split_whitespace().nth(3) != Some("sim")) {
        kept.push_str(l);
        kept.push('\n');
    }
    let map = kept.as_str();
    if let Err(e) = helmsman_nodecore::iomap::parse(map) {
        panic!("the Skipper refuses it: {e:?}\n{map}");
    }
    if let Err(e) = helmsman_io::text::parse(map) {
        panic!("a box refuses it: {e:?}\n{map}");
    }
}

#[test]
fn the_failed_deploy_becomes_a_map_the_skipper_runs() {
    let m = for_skipper(Some(ASS), NODE, &profiles, &scan);
    assert!(m.refused.is_empty(), "{:?}", m.refused);
    both_take(&m.map);
    // The Skipper's own list is kept whole, first.
    assert!(m.map.starts_with(NODE.trim_end()), "{}", m.map);
    // The E1213 on EtherNet/IP (its profile's first transport), at the
    // address the scan saw, with the params a Skipper reads; DI-00 is di:0.
    assert!(
        m.map
            .contains("device iologik_e1213 enip-io 192.168.33.60 profile=moxa-e1213 rpi_ms=100\n"),
        "{}",
        m.map
    );
    assert!(
        m.map
            .contains("in  iologik_e1213_di_00 bool iologik_e1213 di:0\n"),
        "{}",
        m.map
    );
    assert!(!m.map.contains("cycle_ms"), "a box's param: {}", m.map);
    // The MDrive is the Skipper's drive (the same address): no second
    // connection, and its point is left to the Skipper's.
    assert!(!m.map.contains("liberty_lexium"), "{}", m.map);
    assert_eq!(m.map.matches("192.168.33.1 ").count(), 1, "{}", m.map);
    assert!(
        m.notes
            .iter()
            .any(|n| n.contains("is the Skipper's own drive (192.168.33.1)")),
        "{:?}",
        m.notes
    );
    // Its point is the MDrive's `enabled` channel, which the Skipper has
    // as `enabled` (in:0.0): the program must use that name.
    assert_eq!(
        m.same,
        [(
            "liberty_lexium_mdrive_ethernet_lmd_ether".to_string(),
            "enabled".to_string()
        )]
    );
    // The placeholders stay, under their one header.
    for p in ["in  start bool sim", "out di0_active bool sim"] {
        assert!(m.map.contains(p), "{p}: {}", m.map);
    }
    assert_eq!(m.map.matches(crate::placeholders::HEADER).count(), 1, "{}", m.map);
}

#[test]
fn made_twice_it_is_the_same() {
    let once = for_skipper(Some(ASS), NODE, &profiles, &scan);
    let twice = for_skipper(Some(&once.map), NODE, &profiles, &scan);
    assert_eq!(twice.map, once.map);
    assert!(twice.refused.is_empty() && twice.notes.is_empty(), "{twice:?}");
}

#[test]
fn an_identity_the_page_has_no_address_for_is_refused_before_anything_is_sent() {
    let m = for_skipper(Some(ASS), NODE, &profiles, &nothing);
    assert_eq!(m.refused.len(), 2, "{:?}", m.refused);
    assert!(
        m.refused[0].starts_with("iologik_e1213: the network scan did not find it at cip=991:515:0x2a629eae"),
        "{:?}",
        m.refused
    );
    assert_eq!(
        m.unfound,
        ["iologik_e1213", "liberty_lexium_mdrive_ethernet_lmd_ethernet_ip"],
        "a scan may find them"
    );
    let words = m.words().unwrap();
    assert!(
        words.starts_with("Not deployed: the Skipper cannot run iologik_e1213"),
        "{words}"
    );
    // Its point is not sent on a device that is not.
    assert!(!m.map.contains("ch:"), "{}", m.map);
}

#[test]
fn a_device_by_its_profile_at_a_host_needs_no_scan() {
    let p = "device rio device:moxa-iologik-e1213 192.168.33.60 enip-io.rpi_ms=200\nin  go bool rio ch:di-04\nout lamp bool rio ch:DO-01 safe=false\nin  n int rio ch:DI-00-count\n";
    let m = for_skipper(Some(p), NODE, &profiles, &nothing);
    assert!(m.refused.is_empty(), "{:?}", m.refused);
    both_take(&m.map);
    // The line's own enip-io.rpi_ms over the profile's.
    assert!(
        m.map
            .contains("device rio enip-io 192.168.33.60 profile=moxa-e1213 rpi_ms=200\n"),
        "{}",
        m.map
    );
    assert!(
        m.map.contains("in  go bool rio di:4\n"),
        "a channel is case-blind: {}",
        m.map
    );
    assert!(m.map.contains("out lamp bool rio do:1 safe=false\n"), "{}", m.map);
    assert!(m.map.contains("in  n int rio in:2:u32\n"), "{}", m.map);
}

#[test]
fn a_channel_the_profile_lacks_is_refused_by_name() {
    let p = "device rio device:moxa-iologik-e1213 192.168.33.60\nin  x bool rio ch:DI-99\n";
    let m = for_skipper(Some(p), NODE, &profiles, &nothing);
    assert_eq!(m.refused, ["x: rio has no channel DI-99 on enip-io"]);
}

#[test]
fn a_profile_the_project_lacks_or_a_driver_a_skipper_has_not_is_refused() {
    let p = "device r device:acme-widget 10.0.0.9\nin a bool r ch:X\ndevice ur ur-rtde 10.0.0.8\n";
    let m = for_skipper(Some(p), NODE, &profiles, &nothing);
    assert_eq!(m.refused.len(), 2, "{:?}", m.refused);
    assert!(m.refused[0].contains("no acme-widget.device"), "{:?}", m.refused);
    assert!(m.refused[1].contains("ur-rtde is not one"), "{:?}", m.refused);
}

#[test]
fn a_plain_line_keeps_only_the_params_a_skipper_reads() {
    let p = "device rio modbus-tcp 192.168.33.60:502 unit=2 poll_ms=20 eds=x.eds\nin  a bool rio di:0 poll_ms=5\n";
    let m = for_skipper(Some(p), NODE, &profiles, &nothing);
    assert!(m.refused.is_empty(), "{:?}", m.refused);
    both_take(&m.map);
    assert!(
        m.map.contains("device rio modbus-tcp 192.168.33.60:502 unit=2\n"),
        "{}",
        m.map
    );
}

#[test]
fn a_point_the_skipper_names_already_is_the_skippers() {
    // The project once copied the Skipper's list and then added its own.
    let p = format!("{NODE}in  enabled bool sim\nin  start bool sim\n");
    let m = for_skipper(Some(&p), NODE, &profiles, &nothing);
    both_take(&m.map);
    assert_eq!(m.map.matches(" enabled ").count(), 1, "{}", m.map);
    assert!(m.map.contains("in  start bool sim"), "{}", m.map);
}

#[test]
fn no_project_map_is_the_skippers_list() {
    let m = for_skipper(None, NODE, &profiles, &nothing);
    assert_eq!(m.map, NODE.trim_end().to_string() + "\n");
    assert_eq!(m.words(), None);
}

#[test]
fn a_name_longer_than_a_skipper_takes_is_refused_by_name() {
    let long = "a_very_long_placeholder_name_of_40_chars";
    assert_eq!(long.len(), 40);
    let p = format!("in  {long} bool sim\nin  ok bool sim\n");
    let m = for_skipper(Some(&p), NODE, &profiles, &nothing);
    assert_eq!(m.refused.len(), 1, "{:?}", m.refused);
    assert!(
        m.refused[0].starts_with(&format!(
            "{long}: a Skipper names a point in at most 32 characters, and this is 40"
        )),
        "{:?}",
        m.refused
    );
    assert!(m.map.contains("in  ok bool sim"), "{}", m.map);
}
