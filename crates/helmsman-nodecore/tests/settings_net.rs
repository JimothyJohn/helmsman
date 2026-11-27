//! A node's address settings (Nick, 2026-11-26: "New skippers should
//! always default to 255.255.0.0 subnet but that same IP address is
//! fine"): a fresh node is 192.168.127.250/16; a static setting saved
//! without a prefix, through the settings form or `PUT /api/settings`,
//! gets /16; a prefix given is kept; one past 32 or not a number is
//! refused. Read through the public API and the router a node runs.

use helmsman_nodecore::api::{NodeView, settings_from_pairs, settings_json, settings_pairs_from_json};
use helmsman_nodecore::http::{self, Read, Request};
use helmsman_nodecore::router::{Effect, Outcome, handle};
use helmsman_nodecore::settings::{self, FACTORY_IP, FACTORY_PREFIX, LEN, Net, Settings};
use proptest::prelude::*;
use serde_json::json;

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, x)| ((*k).to_string(), (*x).to_string()))
        .collect()
}

fn prefix_of(s: &Settings) -> u8 {
    match s.net {
        Net::Static { prefix, .. } => prefix,
        Net::Dhcp => panic!("static wanted, got DHCP: {s:?}"),
    }
}

fn view() -> NodeView {
    NodeView {
        module: None,
        signals: Vec::new(),
        devices: Vec::new(),
        seq: 0,
        at_ms: 0,
        uptime_ms: 0,
        heap_used: 0,
        heap_peak: 0,
        heap_size: 0,
        heap_external: None,
        mac: [0x02, 0, 0, 0, 0, 1],
        ip: Some([192, 168, 127, 250]),
        link_up: true,
        version: "test",
        board: "test",
        io_map: String::new(),
        shares: Vec::new(),
        one_block: false,
        check: None,
        firmware: None,
    }
}

fn req(text: &str) -> Request {
    match http::parse(text.as_bytes()) {
        Ok(Read::Done(r, _)) => r,
        other => panic!("{other:?}"),
    }
}

/// A node with a token, on DHCP: what a PUT or the form turns it into.
fn node() -> Settings {
    Settings {
        token: "tok".into(),
        ..Settings::default()
    }
}

fn put(settings: &Settings, body: &str) -> Outcome {
    let r = req(&format!(
        "PUT /api/settings HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    handle(&r, &view(), settings)
}

fn form(settings: &Settings, body: &str) -> Outcome {
    let r = req(&format!(
        "POST /settings HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    handle(&r, &view(), settings)
}

fn saved(o: Outcome) -> Settings {
    match o {
        Outcome::Effect(Effect::SaveSettings { settings, .. }) => settings,
        other => panic!("not saved: {other:?}"),
    }
}

fn refused(o: &Outcome) -> bool {
    matches!(o, Outcome::Reply(r) if (400..500).contains(&r.status))
}

#[test]
fn a_fresh_node_is_192_168_127_250_on_a_16_bit_mask() {
    let f = Settings::factory();
    assert_eq!(FACTORY_IP, [192, 168, 127, 250]);
    assert_eq!(FACTORY_PREFIX, 16);
    match f.net {
        Net::Static { ip, prefix, .. } => {
            assert_eq!(ip, [192, 168, 127, 250]);
            assert_eq!(prefix, 16);
        }
        Net::Dhcp => panic!("a fresh node is static: {f:?}"),
    }
    assert_eq!(Settings::mask(prefix_of(&f)), [255, 255, 0, 0]);
    assert_eq!(f.validate(), Ok(()));
    // What the page and GET /api/settings show for it.
    let j = settings_json(&f);
    assert_eq!(j["net"]["mode"], "static");
    assert_eq!(j["net"]["ip"], "192.168.127.250");
    assert_eq!(j["net"]["prefix"], 16);
    // Written to flash and read back, it is still /16.
    let mut frame = [0u8; LEN];
    settings::encode(&f, 1, &mut frame);
    let (_, back) = settings::decode(&frame).expect("decodes");
    assert_eq!(back, f);
}

#[test]
fn a_record_written_at_24_still_reads_24() {
    // Existing nodes keep what they were set to: the default is for a
    // fresh node and a static setting given without a prefix.
    let s = Settings {
        net: Net::Static {
            ip: [192, 168, 3, 56],
            prefix: 24,
            gateway: [192, 168, 3, 1],
        },
        ..Settings::factory()
    };
    let mut frame = [0u8; LEN];
    settings::encode(&s, 7, &mut frame);
    assert_eq!(settings::decode(&frame).map(|(_, s)| prefix_of(&s)), Some(24));
    // A change to something else leaves the mask alone.
    let named = settings_from_pairs(&s, &pairs(&[("name", "press-5")])).unwrap();
    assert_eq!(named.net, s.net);
}

#[test]
fn a_static_setting_without_a_prefix_gets_16_and_a_prefix_given_is_kept() {
    let from_dhcp = Settings::default();
    let s = settings_from_pairs(&from_dhcp, &pairs(&[("mode", "static"), ("ip", "192.168.33.56")])).unwrap();
    assert_eq!(prefix_of(&s), 16, "{s:?}");
    for (given, want) in [
        ("24", 24),
        ("16", 16),
        ("8", 8),
        ("32", 32),
        ("0", 0),
        (" 24 ", 24),
    ] {
        let s = settings_from_pairs(
            &from_dhcp,
            &pairs(&[("mode", "static"), ("ip", "192.168.33.56"), ("prefix", given)]),
        )
        .unwrap();
        assert_eq!(prefix_of(&s), want, "prefix={given:?}");
    }
    // JSON: a number or a string, under `net`.
    for body in [
        json!({"net": {"mode": "static", "ip": "192.168.33.56", "prefix": 24}}),
        json!({"net": {"mode": "static", "ip": "192.168.33.56", "prefix": "24"}}),
    ] {
        let s = settings_from_pairs(&from_dhcp, &settings_pairs_from_json(&body)).unwrap();
        assert_eq!(prefix_of(&s), 24, "{body}");
    }
    let body = json!({"net": {"mode": "static", "ip": "192.168.33.56", "gateway": "0.0.0.0"}});
    let s = settings_from_pairs(&from_dhcp, &settings_pairs_from_json(&body)).unwrap();
    assert_eq!(prefix_of(&s), 16, "{body}");
}

#[test]
fn a_prefix_past_32_or_not_a_number_is_refused() {
    let from_dhcp = Settings::default();
    for bad in [
        "33", "64", "255", "256", "-1", "abc", "16.0", "0x10", "1e1", "２４", "24/24", "16 # x",
    ] {
        let r = settings_from_pairs(
            &from_dhcp,
            &pairs(&[("mode", "static"), ("ip", "192.168.33.56"), ("prefix", bad)]),
        );
        assert!(r.is_err(), "prefix={bad:?} gave {r:?}");
    }
    for bad in [
        json!(33),
        json!(-1),
        json!(16.5),
        json!(1e10),
        json!("abc"),
        json!(true),
        json!([16]),
    ] {
        let body = json!({"net": {"mode": "static", "ip": "192.168.33.56", "prefix": bad}});
        let r = settings_from_pairs(&from_dhcp, &settings_pairs_from_json(&body));
        assert!(r.is_err(), "{body} gave {r:?}");
    }
}

/// The same through the router: what `PUT /api/settings` and the page's
/// form hand the firmware to write.
#[test]
fn put_and_the_form_save_a_static_address_at_16_unless_told() {
    let n = node();
    let s = saved(put(
        &n,
        r#"{"net": {"mode": "static", "ip": "192.168.33.56", "gateway": "0.0.0.0"}}"#,
    ));
    assert_eq!(prefix_of(&s), 16, "{s:?}");
    assert_eq!(s.token, "tok", "the token is kept");
    let s = saved(put(
        &n,
        r#"{"net": {"mode": "static", "ip": "192.168.33.56", "prefix": 24}}"#,
    ));
    assert_eq!(prefix_of(&s), 24);
    for bad in [
        r#"{"net": {"mode": "static", "ip": "192.168.33.56", "prefix": 33}}"#,
        r#"{"net": {"mode": "static", "ip": "192.168.33.56", "prefix": "x"}}"#,
        r#"{"net": {"mode": "static", "ip": "192.168.33.56", "prefix": 300}}"#,
    ] {
        let o = put(&n, bad);
        assert!(refused(&o), "{bad}: {o:?}");
    }
    let s = saved(form(&n, "mode=static&ip=192.168.33.56&gateway=0.0.0.0&auth=tok"));
    assert_eq!(prefix_of(&s), 16, "{s:?}");
    let s = saved(form(
        &n,
        "mode=static&ip=192.168.33.56&prefix=24&gateway=0.0.0.0&auth=tok",
    ));
    assert_eq!(prefix_of(&s), 24);
    let o = form(
        &n,
        "mode=static&ip=192.168.33.56&prefix=33&gateway=0.0.0.0&auth=tok",
    );
    assert!(refused(&o), "{o:?}");
    // Without the token nothing is saved, whatever the prefix.
    let r = req(&format!(
        "PUT /api/settings HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
        r#"{"net": {"mode": "static", "ip": "1.2.3.4"}}"#.len(),
        r#"{"net": {"mode": "static", "ip": "1.2.3.4"}}"#
    ));
    assert!(
        !matches!(handle(&r, &view(), &n), Outcome::Effect(_)),
        "no token, no save"
    );
}

proptest! {
    /// What GET shows, PUT back, is the same address and mask.
    #[test]
    fn a_static_setting_read_and_put_back_is_unchanged(
        ip in any::<[u8; 4]>(),
        prefix in 0u8..=32,
        gateway in any::<[u8; 4]>(),
    ) {
        let s = Settings {
            net: Net::Static { ip, prefix, gateway },
            ..node()
        };
        let back = settings_from_pairs(&node(), &settings_pairs_from_json(&settings_json(&s))).unwrap();
        prop_assert_eq!(back.net, s.net);
    }

    #[test]
    fn every_prefix_past_32_is_refused(prefix in 33u32..=100_000) {
        let r = settings_from_pairs(
            &Settings::default(),
            &pairs(&[("mode", "static"), ("ip", "10.0.0.1"), ("prefix", &prefix.to_string())]),
        );
        prop_assert!(r.is_err());
    }
}
