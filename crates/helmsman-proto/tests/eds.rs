//! The EDS reader: an E1213-shaped file resolves to the connections the
//! vendor's EDS declares (fixture: `tests/fixtures/e1213-facts.eds`, our
//! restatement of its facts), an `OpENer`-shaped one to the sample
//! application's, hostile files are refused with a line, and no text at
//! all makes the parser panic.

use std::time::Duration;

use helmsman_proto::eds::{self, AppType, Eds, Field, RealTimeFormat};
use proptest::prelude::*;

const E1213: &str = include_str!("fixtures/e1213-facts.eds");

fn e1213() -> Eds {
    Eds::parse(E1213).expect("fixture parses")
}

#[test]
fn e1213_identity() {
    let id = e1213().identity().unwrap();
    assert_eq!((id.vendor_id, id.device_type, id.product_code), (991, 12, 515));
    assert_eq!((id.major_revision, id.minor_revision), (4, 1));
    assert_eq!(id.product_name.as_deref(), Some("E1213"));
    assert_eq!(id.vendor_name.as_deref(), Some("Moxa"));
}

#[test]
fn adjacent_strings_join_and_comments_vanish() {
    let eds = e1213();
    let icon = eds.section("device").unwrap().get("iconcontents").unwrap();
    assert_eq!(icon.fields, [Field::Str("AAABAAE=".into())]);
    let class = eds
        .section("Device Classification")
        .unwrap()
        .get("Class1")
        .unwrap();
    assert_eq!(class.fields, [Field::Word("EtherNetIP".into())]);
}

#[test]
fn e1213_assemblies() {
    let eds = e1213();
    let a = eds.assemblies().unwrap();
    let got: Vec<_> = a.iter().map(|a| (a.instance, a.name.as_str(), a.size)).collect();
    assert_eq!(
        got,
        [(101, "Input Data", Some(51)), (102, "Output Data", Some(1))]
    );
    assert_eq!(a[0].members.len(), 1);
    assert_eq!(a[0].members[0].bits, Some(408));
}

#[test]
fn e1213_connections() {
    let c = e1213().connections().unwrap();
    assert_eq!(c.len(), 2);

    let owner = &c[0];
    assert_eq!(owner.name, "Exclusive Owner");
    assert_eq!(owner.app_types, [AppType::ExclusiveOwner]);
    assert!(owner.class1 && owner.cyclic);
    assert_eq!(
        (owner.configuration, owner.o_to_t_point, owner.t_to_o_point),
        (100, 102, 101)
    );
    assert_eq!((owner.o_to_t.size, owner.t_to_o.size), (1, 51));
    assert_eq!(owner.o_to_t.format, RealTimeFormat::RunIdle);
    assert_eq!(owner.t_to_o.format, RealTimeFormat::Modeless);
    assert!(owner.o_to_t.point_to_point && owner.t_to_o.point_to_point);
    assert!(!owner.o_to_t.multicast && owner.t_to_o.multicast);
    assert_eq!(owner.o_to_t.rpi.min, Some(Duration::from_millis(100)));
    assert_eq!(owner.o_to_t.rpi.max, Some(Duration::from_secs(3)));
    assert_eq!(owner.t_to_o.rpi.default, Some(Duration::from_millis(100)));
    assert_eq!(owner.config_size, 0);

    let input = &c[1];
    assert_eq!(input.app_types, [AppType::InputOnly]);
    assert_eq!(input.o_to_t_point, 111);
    assert_eq!(input.o_to_t.size, 0);
    assert_eq!(input.o_to_t.format, RealTimeFormat::Heartbeat);
    assert_eq!((input.t_to_o_point, input.t_to_o.size), (101, 51));
    assert!(!c.iter().any(|c| c.is(AppType::ListenOnly)));
}

/// The shape of `OpENer`'s `data/opener_sample_app.eds`: sizes left blank
/// (taken from the assembly), a config assembly with data, 32 members of
/// 8 bits each, a listen-only connection, and the server bit set.
const OPENER_SHAPE: &str = r#"
[Device]
  VendCode = 1; ProdType = 12; ProdCode = 65001; MajRev = 2; MinRev = 3;
  ProdName = "OpENer PC";
[Params]
  Param1 = 0,,,0x0000,0xD1,1,"Input Data","","",,,0,,,,,,,,,;
  Param4 = 0,,,0x0000,0xC8,4,"RPI","","",20000,,30000,,,,,,,,,;
[Assembly]
  Assem100 = "Input Assembly","",4,0x0000,,,8,Param1,8,Param1,8,Param1,8,Param1;
  Assem150 = "Output Assembly","",4,0x0001,,,8,Param1,8,Param1,8,Param1,8,Param1;
  Assem151 = "Config Assembly","",10,0x0001,,,80,Param1;
[Connection Manager]
  Connection1 = 0x84010002, 0x44640405, Param4,,Assem150, Param4,,Assem100,
     ,, ,Assem151, "Exlusive Owner", "", "20 04 24 97 2C 96 2C 64";
  Connection3 = 0x01010002, 0x44240305, Param4,0,, Param4,,Assem100,
     ,, ,, "Listen Only", "", "20 04 24 97 2C 99 2C 64";
"#;

#[test]
fn opener_shape() {
    let eds = Eds::parse(OPENER_SHAPE).unwrap();
    let c = eds.connections().unwrap();
    assert_eq!(c[0].app_types, [AppType::ExclusiveOwner]);
    assert_eq!(c[0].trigger_transport >> 31, 1);
    assert_eq!(
        (c[0].configuration, c[0].o_to_t_point, c[0].t_to_o_point),
        (151, 150, 100)
    );
    assert_eq!((c[0].o_to_t.size, c[0].t_to_o.size, c[0].config_size), (4, 4, 10));
    assert_eq!(c[0].o_to_t.rpi.max, None);
    assert_eq!(c[1].app_types, [AppType::ListenOnly]);
    assert_eq!((c[1].o_to_t_point, c[1].o_to_t.size), (153, 0));
}

fn refused(text: &str) -> eds::Error {
    match Eds::parse(text).map(|e| (e.connections(), e.identity(), e.assemblies())) {
        Err(e) | Ok((Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e))) => e,
        Ok(_) => panic!("accepted:\n{text}"),
    }
}

/// The E1213 fixture with one substitution.
fn edited(from: &str, to: &str) -> String {
    assert!(E1213.contains(from), "fixture lacks {from:?}");
    E1213.replacen(from, to, 1)
}

#[test]
fn structural_errors_name_the_line() {
    for (text, line, what) in [
        ("Key = 1;", 1, "outside any [section]"),
        ("[A]\nKey = 1", 2, "no ;"),
        ("[A]\nKey = 1;\nKey = 2;", 3, "given twice"),
        ("[A]\nkey = 1;\nKEY = 2;", 3, "given twice"),
        ("[A]\n[a]", 2, "given twice"),
        ("[A\nKey = 1;", 1, "without ]"),
        ("[]", 1, "empty section"),
        ("[A] junk", 1, "text after"),
        ("[A]\nKey = \"open\n\";", 2, "past the end of its line"),
        ("[A]\nKey = 1 2\n3;", 3, "expected , or ;"),
        ("[A]\nKey = \"a\" b;", 2, "expected , or ;"),
        ("[A]\n= 1;", 2, "expected a keyword"),
        ("[A]\nKey 1;", 2, "expected ="),
        ("[A]\nKey = 1;\x00", 2, "control character"),
    ] {
        let e = Eds::parse(text).expect_err(text);
        assert_eq!(e.line, line, "{text:?}: {e}");
        assert!(e.what.contains(what), "{text:?}: {e}");
    }
}

#[test]
fn hostile_values_are_refused() {
    for (from, to, what) in [
        // Sizes that disagree with the assembly, or can't be.
        (
            "Param3, Param2, Assem101,       $ T->O",
            "Param3, 52, Assem101,       $ T->O",
            "assembly is 51",
        ),
        ("51, 51, 51,", "70000, 70000, 70000,", "out of range"),
        (
            "Assem101 = \"Input Data\", , 51,",
            "Assem101 = \"Input Data\", , 50,",
            "add up",
        ),
        (
            "Assem101 = \"Input Data\", , 51,",
            "Assem101 = \"Input Data\", , 4294967296,",
            "not a number up to 65535",
        ),
        ("Assem102 =", "Assem99999999999 =", "no Assem102"),
        // References to things that aren't there, or aren't references.
        (
            "Param3, Param1, Assem102,",
            "Param3, Param9, Assem102,",
            "no Param9",
        ),
        (
            "Param3, Param1, Assem102,",
            "Param3, Param1, Assem7,",
            "no Assem7",
        ),
        (
            "Param3, Param1, Assem102,",
            "Param3, Param1, Param1,",
            "is not an AssemN",
        ),
        (
            "Param3, Param1, Assem102,",
            "Bogus, Param1, Assem102,",
            "not a ParamN",
        ),
        (
            "Param3, Param1, Assem102,",
            "Param3, Param2, Assem101,",
            "path's point",
        ),
        ("Param3, 0, ,", "Param3, 4, ,", "heartbeat but carries 4"),
        (
            "100000, 3000000, 100000,",
            "100000, 3000, 100000,",
            "min 100000 > max 3000",
        ),
        // Words that are the wrong type.
        ("0x04010002,", "\"0x04010002\",", "not a 32-bit word"),
        ("0x04010002,", "0x104010002,", "not a 32-bit word"),
        ("VendCode = 991;", "VendCode = 65536;", "VendCode"),
        ("VendCode = 991;", "VendCode = -1;", "VendCode"),
        ("MajRev = 4;", "MajRev = 4, 5;", "one value"),
        // Paths.
        (
            "\"20 04 24 64 2C 66 2C 65\";      $ class",
            "\"20 04 24 [Param1] 2C 66 2C 65\";      $ class",
            "literal hex",
        ),
        (
            "\"20 04 24 64 2C 66 2C 65\";      $ class",
            "\"20 04 24 64 2C 66 2C\";      $ class",
            "even count",
        ),
        (
            "\"20 04 24 64 2C 66 2C 65\";      $ class",
            "\"20 05 24 64 2C 66 2C 65\";      $ class",
            "not the Assembly",
        ),
        (
            "\"20 04 24 64 2C 66 2C 65\";      $ class",
            "\"20 04 24 64 2C 66 00 00\";      $ class",
            "unsupported segment",
        ),
        (
            "\"20 04 24 64 2C 66 2C 65\";      $ class",
            "\"20 04 2C 66\";      $ class",
            "two connection points",
        ),
        (
            "\"20 04 24 64 2C 66 2C 65\";      $ class",
            "\"20 04 24 64 2C 66 2C 65 2C 01\";      $ class",
            "more than two",
        ),
    ] {
        let e = refused(&edited(from, to));
        assert!(e.what.contains(what), "{from:?} -> {to:?}: {e}");
    }
}

#[test]
fn packet_over_a_forward_open_is_refused() {
    let big = edited("51, 51, 51,", "600, 600, 600,").replacen(
        "Assem101 = \"Input Data\", , 51, 0x0001, ,, 408,",
        "Assem101 = \"Input Data\", , 600, 0x0001, ,, 4800,",
        1,
    );
    let e = refused(&big);
    assert!(e.what.contains("over what a Forward_Open"), "{e}");
}

#[test]
fn too_many_fields_and_too_much_text() {
    let many = format!("[A]\nK = {};", "1,".repeat(eds::MAX_FIELDS));
    assert!(Eds::parse(&many).unwrap_err().what.contains("more than"));
    let huge = format!("[A]\n$ {}", "x".repeat(eds::MAX_TEXT));
    assert!(Eds::parse(&huge).unwrap_err().what.contains("larger than"));
}

#[test]
fn escapes_and_empties() {
    let eds = Eds::parse("[A]\nK = \"a\\\"b\\\\c\\n\\q\",,\"\" ,;\nE = ;").unwrap();
    let a = eds.section("A").unwrap();
    assert_eq!(
        a.get("K").unwrap().fields,
        [
            Field::Str("a\"b\\c\n\\q".into()),
            Field::Empty,
            Field::Str(String::new()),
            Field::Empty
        ]
    );
    assert_eq!(a.get("E").unwrap().fields, []);
}

#[test]
fn numbers() {
    let w = |s: &str| Field::Word(s.into());
    assert_eq!(w("0x1F").uint(), Some(31));
    assert_eq!(w("0X1f").uint(), Some(31));
    assert_eq!(w("-12").int(), Some(-12));
    assert_eq!(w("0x").uint(), None);
    assert_eq!(w("1.5").uint(), None);
    assert_eq!(w("+3").uint(), None);
    assert_eq!(w("99999999999999999999").uint(), None);
    assert_eq!(Field::Str("12".into()).uint(), None);
    assert_eq!(w("Param007").reference("param"), Some(7));
    assert_eq!(w("Param").reference("Param"), None);
    assert_eq!(w("Param-1").reference("Param"), None);
    assert_eq!(w("Param99999999999").reference("Param"), None);
}

#[test]
fn to_text_round_trips_the_fixture() {
    let eds = e1213();
    let again = Eds::parse(&eds.to_text()).unwrap();
    assert_eq!(strip(&again), strip(&eds));
    assert_eq!(again.connections().unwrap(), {
        // Lines differ after rewriting; everything else must not.
        let mut c = eds.connections().unwrap();
        for (a, b) in c.iter_mut().zip(again.connections().unwrap()) {
            a.line = b.line;
        }
        c
    });
}

type Shape = Vec<(String, Vec<(String, Vec<Field>)>)>;

fn strip(eds: &Eds) -> Shape {
    eds.sections
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                s.entries
                    .iter()
                    .map(|e| (e.key.clone(), e.fields.clone()))
                    .collect(),
            )
        })
        .collect()
}

fn field() -> impl Strategy<Value = Field> {
    prop_oneof![
        Just(Field::Empty),
        "[A-Za-z0-9_.:/+-][A-Za-z0-9_.:/+ -]{0,12}[A-Za-z0-9_.:/+-]".prop_map(Field::Word),
        "[A-Za-z0-9]".prop_map(Field::Word),
        "[ -~\t\\n\\\\\"]{0,24}".prop_map(Field::Str),
    ]
}

fn eds_strategy() -> impl Strategy<Value = Shape> {
    let entry = (
        "[A-Za-z_][A-Za-z0-9_]{0,10}",
        proptest::collection::vec(field(), 0..8)
            .prop_filter("a lone empty field reads back as none", |f| f != &[Field::Empty]),
    );
    let section = (
        "[A-Za-z0-9/_][A-Za-z0-9 /_-]{0,14}[A-Za-z0-9/_]",
        proptest::collection::vec(entry, 0..6),
    );
    proptest::collection::vec(section, 0..5)
}

fn build(shape: &Shape) -> Option<Eds> {
    let mut eds = Eds::default();
    for (name, entries) in shape {
        if eds.section(name).is_some() {
            return None;
        }
        let mut s = eds::Section {
            name: name.clone(),
            entries: Vec::new(),
            line: 0,
        };
        for (key, fields) in entries {
            if s.get(key).is_some() {
                return None;
            }
            s.entries.push(eds::Entry {
                key: key.clone(),
                fields: fields.clone(),
                line: 0,
            });
        }
        eds.sections.push(s);
    }
    Some(eds)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn never_panics_on_any_text(s in "\\PC{0,400}") {
        if let Ok(eds) = Eds::parse(&s) {
            let _ = eds.identity();
            let _ = eds.assemblies();
            let _ = eds.connections();
            let _ = eds.to_text();
        }
    }

    #[test]
    fn never_panics_on_eds_like_text(
        parts in proptest::collection::vec(
            prop_oneof![
                prop::sample::select(vec![
                    "[", "]", "=", ",", ";", "$", "\"", "\\", "\n", " ", "0x", "Param1",
                    "Assem101", "[Connection Manager]\n", "Connection1 =", "[Params]\n",
                    "[Assembly]\n", "[Device]\n", "\"20 04 24 64 2C 66 2C 65\"",
                    "4294967295", "-1", "99999999999999999999999",
                ])
                .prop_map(String::from),
                "[a-zA-Z0-9]{1,4}",
            ],
            0..80,
        )
    ) {
        let s: String = parts.concat();
        if let Ok(eds) = Eds::parse(&s) {
            let _ = eds.identity();
            let _ = eds.assemblies();
            let _ = eds.connections();
        }
    }

    /// Any one-byte edit of the real-shaped fixture: no panic.
    #[test]
    fn fixture_mutations_never_panic(at in 0..E1213.len(), b in 0x20u8..0x7f) {
        let mut bytes = E1213.as_bytes().to_vec();
        bytes[at] = b;
        if let Ok(s) = String::from_utf8(bytes)
            && let Ok(eds) = Eds::parse(&s)
        {
            let _ = eds.identity();
            let _ = eds.connections();
        }
    }

    #[test]
    fn to_text_round_trips(shape in eds_strategy()) {
        if let Some(eds) = build(&shape) {
            let text = eds.to_text();
            let back = Eds::parse(&text).map_err(|e| TestCaseError::fail(format!("{e}\n{text}")))?;
            prop_assert_eq!(strip(&back), strip(&eds));
        }
    }
}
