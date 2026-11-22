mod common;
use common::*;
use helmsman_drawing::parts::{Attribute, NearText};
use helmsman_drawing::{LibraryRecord, Match, Matcher, Part};
use proptest::prelude::*;

fn part(block: &str, pn: Option<&str>, model: Option<&str>, near: &[&str]) -> Part {
    Part {
        block: block.into(),
        layer: String::new(),
        x: 0.0,
        y: 0.0,
        rotation: 0.0,
        tag: None,
        part_number: pn.map(Into::into),
        model: model.map(Into::into),
        attributes: Vec::new(),
        nearby_text: near
            .iter()
            .map(|t| NearText {
                text: (*t).into(),
                distance: 1.0,
            })
            .collect(),
        inside: Vec::new(),
        line: 0,
    }
}

fn ids(m: &Match<'_>) -> Vec<String> {
    match m {
        Match::Matched(r) => vec![r.id.clone()],
        Match::Ambiguous(v) => v.iter().map(|r| r.id.clone()).collect(),
        Match::Unknown => vec![],
    }
}

#[test]
fn an_exact_identifier_is_matched_however_it_is_typed() {
    let lib = library();
    let m = Matcher::new(&lib);
    for t in [
        "ioLogik E1213",
        "IOLOGIK-E1213",
        "iologik  e1213",
        "Moxa ioLogik E1213",
        "MOXA/IOLOGIK E1213",
    ] {
        assert!(
            matches!(m.match_text(t), Match::Matched(r) if r.id == "moxa/iologik-e1213"),
            "{t}"
        );
    }
    for (t, id) in [
        ("IF6123", "ifm/if6123"),
        ("im5173", "ifm/if6123"),
        ("O5D152", "ifm/o5d100"),
        ("O5D100", "ifm/o5d100"),
        ("PN7092", "ifm/pn7092"),
        ("PN7572", "ifm/pn7092"),
        ("WTB4F_DID8389194", "sick/wtb4f-did8389194"),
        ("IMPACT67 E DI16", "murrelektronik/impact67-e-di16"),
    ] {
        assert!(
            matches!(m.match_text(t), Match::Matched(r) if r.id == id),
            "{t}: {:?}",
            m.match_text(t)
        );
    }
}

#[test]
fn a_model_is_never_guessed() {
    let lib = library();
    let m = Matcher::new(&lib);
    // not in the library at all
    for t in [
        "",
        " ",
        "E1299",
        "55086",
        "IF9999",
        "ioLogik",
        "Remote I/O station 4",
        "-A1",
        "DI8",
        "1234567",
        "\0\u{fffd}",
    ] {
        assert_eq!(m.match_text(t), Match::Unknown, "{t:?}");
    }
    // a family written with x is no identifier: KQ6005 is not matched to "KQxxxx"
    assert_eq!(m.match_text("KQxxxx"), Match::Unknown);
    assert_eq!(m.match_text("KQ6005"), Match::Unknown);
    // part of an identifier: a person decides, and it names every candidate
    let near = ids(&m.match_text("E121"));
    assert!(
        m.match_text("E121") != Match::Unknown && near.len() == 3,
        "{near:?}"
    );
    // an identifier inside longer text
    assert_eq!(
        ids(&m.match_text("ioLogik E1213-T wide temperature")),
        ["moxa/iologik-e1213"]
    );
    assert!(matches!(
        m.match_text("ioLogik E1213-T wide temperature"),
        Match::Ambiguous(_)
    ));
    // a wrong digit is not close enough to count
    assert_eq!(m.match_text("ioLogik E1214x"), Match::Unknown);
}

#[test]
fn two_records_with_one_identifier_are_ambiguous() {
    let mut lib = library();
    lib.push(LibraryRecord {
        id: "other/e1213-copy".into(),
        vendor: "Other".into(),
        model: "E1213 rev B".into(),
        products: vec!["ioLogik E1213".into()],
    });
    let m = Matcher::new(&lib);
    let r = m.match_text("ioLogik E1213");
    assert_eq!(ids(&r), ["moxa/iologik-e1213", "other/e1213-copy"]);
    assert!(matches!(r, Match::Ambiguous(_)));
}

#[test]
fn text_beside_a_part_never_decides_alone() {
    let lib = library();
    let m = Matcher::new(&lib);
    let r = m.match_part(&part("BOX", None, None, &["ioLogik E1213"]));
    assert!(matches!(&r, Match::Ambiguous(v) if v.len() == 1), "{r:?}");
    // its own attribute decides, and a label that agrees changes nothing
    let r = m.match_part(&part("BOX", Some("ioLogik E1213"), None, &["ioLogik E1213"]));
    assert!(matches!(r, Match::Matched(_)));
    // a label that names another record does
    let r = m.match_part(&part("BOX", Some("ioLogik E1213"), None, &["ioLogik E1212"]));
    assert_eq!(ids(&r), ["moxa/iologik-e1212", "moxa/iologik-e1213"]);
}

#[test]
fn any_attribute_value_and_the_block_name_count_as_the_part_own() {
    let lib = library();
    let m = Matcher::new(&lib);
    let mut p = part("X", None, None, &[]);
    p.attributes.push(Attribute {
        tag: "WHATEVER".into(),
        value: "IF6124".into(),
        from_default: false,
    });
    assert!(matches!(m.match_part(&p), Match::Matched(r) if r.id == "ifm/if6123"));
    assert!(
        matches!(m.match_part(&part("O5D100", None, None, &[])), Match::Matched(r) if r.id == "ifm/o5d100")
    );
    // two own texts naming two records: not Matched
    let mut q = part("IF6123", Some("O5D100"), None, &[]);
    q.attributes.clear();
    assert_eq!(ids(&m.match_part(&q)), ["ifm/if6123", "ifm/o5d100"]);
}

#[test]
fn hostile_library_and_text_do_not_panic() {
    let lib = vec![
        LibraryRecord {
            id: String::new(),
            vendor: String::new(),
            model: String::new(),
            products: vec![],
        },
        LibraryRecord {
            id: "a".into(),
            vendor: "\u{fffd}".repeat(100_000),
            model: "x".repeat(300_000),
            products: vec!["9".repeat(500_000)],
        },
        LibraryRecord {
            id: "b".into(),
            vendor: "V".into(),
            model: "(((,,,///)))".into(),
            products: vec![String::new(); 1000],
        },
    ];
    let m = Matcher::new(&lib);
    let long = "E1213".repeat(200_000);
    assert!(matches!(
        m.match_text(&long),
        Match::Unknown | Match::Ambiguous(_)
    ));
    let _ = m.match_text("9999");
    let empty = Matcher::new(&[]);
    assert_eq!(empty.match_text("ioLogik E1213"), Match::Unknown);
}

proptest! {
    /// Contract: `Matched(r)` only when the part's own text is, normalised, an identifier of `r`.
    #[test]
    fn matched_means_an_exact_identifier(t in "[ -~]{0,24}", pre in prop::sample::select(vec!["", "moxa ", "xx "]), pick in 0usize..14) {
        let lib = library();
        let m = Matcher::new(&lib);
        let seeds = ["ioLogik E1213", "IF6123", "o5d100", "PN7092", "IMPACT67 E DI8 DO8", "WTB4F_DID8389194", "E121", "KQ6005",
                     "55086", "IF6", "", "O5D1", "iologik", "E1213"];
        let text = format!("{pre}{}{t}", seeds[pick]);
        let norm = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect::<String>();
        if let Match::Matched(r) = m.match_part(&part("B", Some(&text), None, &[])) {
            let n = norm(&text);
            let mut idents: Vec<String> = r.products.clone();
            idents.push(r.model.clone());
            idents.extend(r.model.split([',', '/', '(', ')']).map(str::to_owned));
            idents.push(format!("{} {}", r.vendor, r.model));
            prop_assert!(idents.iter().any(|i| norm(i) == n), "{text:?} matched {}", r.id);
        }
    }

    #[test]
    fn no_text_makes_matched_from_nearby_alone(t in "[ -~]{0,30}") {
        let lib = library();
        let m = Matcher::new(&lib);
        let r = m.match_part(&part("B", None, None, &[&t]));
        prop_assert!(!matches!(r, Match::Matched(_)));
    }
}
