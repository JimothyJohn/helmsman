//! `PLCopen` TC6 XML import: ladder, FBD and ST projects run like the
//! equivalent Structured Text; bad projects fail with positions in the XML.

use std::fmt::Write as _;
use std::time::Duration;

use helmsman_iec::{Image, ImageSizes, Pos, Program, Value, compile_plcopen, plcopen};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/plcopen/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn build(xml: &str) -> Program {
    compile_plcopen(xml, SIZES).unwrap_or_else(|d| {
        let st = plcopen::lower(xml)
            .map(|l| l.st().to_string())
            .unwrap_or_default();
        panic!("{d:#?}\n{st}")
    })
}

fn errors(xml: &str) -> String {
    match compile_plcopen(xml, SIZES) {
        Ok(_) => panic!("compiled:\n{xml}"),
        Err(d) => d.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
    }
}

fn boolean(p: &Program, path: &str) -> bool {
    match p.get(path) {
        Some(Value::Bool(b)) => b,
        other => panic!("{path} = {other:?}"),
    }
}

fn int(p: &Program, path: &str) -> i128 {
    match p.get(path) {
        Some(Value::Int(i)) => i,
        other => panic!("{path} = {other:?}"),
    }
}

fn set(img: &mut Image, bit: usize, on: bool) {
    if on {
        img.i[bit / 8] |= 1 << (bit % 8);
    } else {
        img.i[bit / 8] &= !(1 << (bit % 8));
    }
}

#[test]
fn ladder_seal_in_and_delayed_fan() {
    let mut p = build(&fixture("motor.xml"));
    let mut img = Image::new(SIZES);
    let at = |p: &mut Program, img: &mut Image, ms| p.scan_at(img, Duration::from_millis(ms)).unwrap();

    at(&mut p, &mut img, 0);
    assert!(!boolean(&p, "Run"));
    set(&mut img, 0, true); // Start
    at(&mut p, &mut img, 10);
    assert!(boolean(&p, "Run"));
    assert_eq!(img.q[0] & 1, 1, "Lamp follows Run");
    set(&mut img, 0, false);
    at(&mut p, &mut img, 20);
    assert!(boolean(&p, "Run"), "sealed in");
    at(&mut p, &mut img, 2000);
    assert!(!boolean(&p, "Fan"), "1990 ms after Run");
    at(&mut p, &mut img, 2010);
    assert!(boolean(&p, "Fan"), "2 s after Run");
    set(&mut img, 1, true); // Stop
    at(&mut p, &mut img, 2020);
    assert!(!boolean(&p, "Run"));
    assert!(!boolean(&p, "Fan"));
    assert_eq!(img.q[0] & 1, 0);
}

#[test]
fn fbd_counter_through_a_connector() {
    let mut p = build(&fixture("counter_fbd.xml"));
    let mut img = Image::new(SIZES);
    let pulse = |p: &mut Program, img: &mut Image| {
        set(img, 2, true);
        p.scan(img).unwrap();
        set(img, 2, false);
        p.scan(img).unwrap();
    };
    p.scan(&mut img).unwrap();
    assert!(boolean(&p, "AtMostOne"));
    pulse(&mut p, &mut img);
    assert_eq!(int(&p, "count"), 1);
    assert!(boolean(&p, "AtMostOne"));
    pulse(&mut p, &mut img);
    assert!(!boolean(&p, "AtMostOne"));
    assert!(!boolean(&p, "Done"));
    pulse(&mut p, &mut img);
    assert_eq!(int(&p, "count"), 3);
    assert!(boolean(&p, "Done"), "reached Limit");
    set(&mut img, 3, true); // Reset
    p.scan(&mut img).unwrap();
    assert_eq!(int(&p, "count"), 0);
    assert!(!boolean(&p, "Done"));
}

#[test]
fn set_reset_edges_and_enable() {
    let mut p = build(&fixture("coils.xml"));
    let mut img = Image::new(SIZES);
    p.scan(&mut img).unwrap();
    assert!(!boolean(&p, "Latched"));
    assert_eq!(int(&p, "Edges"), 0);

    set(&mut img, 4, true); // A rises
    p.scan(&mut img).unwrap();
    assert!(boolean(&p, "Latched"));
    assert!(boolean(&p, "Pulsed"), "ENO follows EN");
    assert_eq!(int(&p, "Edges"), 1);
    assert!(!boolean(&p, "Fell"));

    p.scan(&mut img).unwrap(); // A held
    assert!(!boolean(&p, "Pulsed"));
    assert_eq!(int(&p, "Edges"), 1, "ADD runs only when enabled");

    set(&mut img, 4, false); // A falls
    p.scan(&mut img).unwrap();
    assert!(boolean(&p, "Fell"));
    assert!(boolean(&p, "Latched"), "set coils hold");
    p.scan(&mut img).unwrap();
    assert!(!boolean(&p, "Fell"));

    set(&mut img, 5, true); // B
    p.scan(&mut img).unwrap();
    assert!(!boolean(&p, "Latched"));

    set(&mut img, 5, false);
    for _ in 0..3 {
        set(&mut img, 4, true);
        p.scan(&mut img).unwrap();
        set(&mut img, 4, false);
        p.scan(&mut img).unwrap();
    }
    assert_eq!(int(&p, "Edges"), 4);
}

#[test]
fn data_types_and_st_bodies() {
    let mut p = build(&fixture("types_st.xml"));
    let mut img = Image::new(SIZES);
    p.scan(&mut img).unwrap();
    // Twice(1 + 4) + 10 + 2 + 7
    assert_eq!(int(&p, "sum"), 29);
    assert_eq!(int(&p, "m"), 0);
    p.scan(&mut img).unwrap();
    assert_eq!(int(&p, "m"), 5, "Mode#Running");
}

/// FBD helpers: an input variable, a block, an output variable.
fn invar(id: u32, expr: &str) -> String {
    format!("<inVariable localId=\"{id}\"><connectionPointOut/><expression>{expr}</expression></inVariable>")
}

fn block(id: u32, ty: &str, ins: &[(&str, u32, &str)], outs: &[&str]) -> String {
    let ins = ins.iter().fold(String::new(), |mut s, (f, from, port)| {
        let port = if port.is_empty() {
            String::new()
        } else {
            format!(" formalParameter=\"{port}\"")
        };
        write!(
            s,
            "<variable formalParameter=\"{f}\"><connectionPointIn>\
             <connection refLocalId=\"{from}\"{port}/></connectionPointIn></variable>"
        )
        .unwrap();
        s
    });
    let outs = outs.iter().fold(String::new(), |mut s, f| {
        write!(
            s,
            "<variable formalParameter=\"{f}\"><connectionPointOut/></variable>"
        )
        .unwrap();
        s
    });
    format!(
        "<block localId=\"{id}\" typeName=\"{ty}\"><inputVariables>{ins}</inputVariables>\
         <inOutVariables/><outputVariables>{outs}</outputVariables></block>"
    )
}

fn outvar(id: u32, from: u32, port: &str, expr: &str) -> String {
    format!(
        "<outVariable localId=\"{id}\"><connectionPointIn><connection refLocalId=\"{from}\" \
         formalParameter=\"{port}\"/></connectionPointIn><expression>{expr}</expression></outVariable>"
    )
}

#[test]
fn chained_comparisons_and_stacked_enables() {
    // ok := a > b > c; y := (a + b) * c, written only while both e1 and e2.
    let body = [
        invar(1, "a"),
        invar(2, "b"),
        invar(3, "c"),
        invar(4, "e1"),
        invar(5, "e2"),
        block(
            6,
            "GT",
            &[("IN1", 1, ""), ("IN2", 2, ""), ("IN3", 3, "")],
            &["OUT"],
        ),
        outvar(7, 6, "OUT", "ok"),
        block(
            8,
            "ADD",
            &[("EN", 4, ""), ("IN1", 1, ""), ("IN2", 2, "")],
            &["ENO", "OUT"],
        ),
        block(
            9,
            "MUL",
            &[("EN", 5, ""), ("IN1", 8, "OUT"), ("IN2", 3, "")],
            &["ENO", "OUT"],
        ),
        outvar(10, 9, "OUT", "y"),
    ]
    .concat();
    let vars = [
        ("a", "INT"),
        ("b", "INT"),
        ("c", "INT"),
        ("e1", "BOOL"),
        ("e2", "BOOL"),
        ("ok", "BOOL"),
        ("y", "INT"),
    ];
    let xml = project(&vars, &format!("<FBD>{body}</FBD>"));
    let src = plcopen::lower(&xml).unwrap().st().to_string();
    for (a, b, c, e1, e2, ok, y) in [
        (3, 2, 1, true, true, true, 5),
        (3, 2, 2, true, true, false, 10),
        (1, 2, 0, true, true, false, 0),
        (3, 2, 1, true, false, true, -1),
        (3, 2, 1, false, true, true, -1),
    ] {
        let init = src
            .replace("  a : INT;", &format!("  a : INT := {a};"))
            .replace("  b : INT;", &format!("  b : INT := {b};"))
            .replace("  c : INT;", &format!("  c : INT := {c};"))
            .replace("  e1 : BOOL;", &format!("  e1 : BOOL := {e1};"))
            .replace("  e2 : BOOL;", &format!("  e2 : BOOL := {e2};"))
            .replace("  y : INT;", "  y : INT := -1;");
        let mut p = helmsman_iec::compile(&init, SIZES).unwrap();
        p.scan(&mut Image::new(SIZES)).unwrap();
        assert_eq!(boolean(&p, "ok"), ok, "{a} > {b} > {c}");
        assert_eq!(int(&p, "y"), y, "a={a} b={b} c={c} e1={e1} e2={e2}");
    }
}

// ---- errors ----

/// A project holding one program with `vars` (`name: TYPE` pairs, all
/// local) and `body` (the inside of `<body>`).
fn project(vars: &[(&str, &str)], body: &str) -> String {
    let mut s = String::from(
        "<?xml version='1.0' encoding='utf-8'?>\n\
         <project xmlns=\"http://www.plcopen.org/xml/tc6_0201\" xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">\n\
         <types><dataTypes/><pous>\n\
         <pou name=\"p\" pouType=\"program\">\n<interface><localVars>\n",
    );
    for (n, t) in vars {
        writeln!(s, "<variable name=\"{n}\"><type><{t}/></type></variable>").unwrap();
    }
    write!(
        s,
        "</localVars></interface>\n<body>\n{body}\n</body>\n</pou>\n</pous></types>\n</project>\n"
    )
    .unwrap();
    s
}

fn st(text: &str) -> String {
    format!("<ST><xhtml:p><![CDATA[{text}]]></xhtml:p></ST>")
}

fn line_of(xml: &str, needle: &str) -> u32 {
    let k = xml.lines().position(|l| l.contains(needle)).expect(needle);
    u32::try_from(k).unwrap() + 1
}

#[test]
fn st_body_errors_point_into_the_xml() {
    let xml = project(&[("x", "INT")], &st("x := 1;\nx := y;"));
    let line = line_of(&xml, "x := y;");
    let col = u32::try_from(xml.lines().nth(line as usize - 1).unwrap().find('y').unwrap()).unwrap() + 1;
    let msg = errors(&xml);
    assert!(msg.starts_with(&format!("{line}:{col}:")), "{msg}");
    assert!(msg.contains('Y'), "{msg}");
    // First line of the CDATA: the column is shifted past `<![CDATA[`.
    let xml = project(&[("x", "INT")], &st("x := q;"));
    let l = xml.lines().find(|l| l.contains("x := q")).unwrap();
    let col = u32::try_from(l.find('q').unwrap()).unwrap() + 1;
    assert!(errors(&xml).starts_with(&format!("{}:{col}:", line_of(&xml, "x := q"))));
}

#[test]
fn graphical_errors_point_at_the_element() {
    let body = "<LD>\n\
        <leftPowerRail localId=\"1\"><position x=\"0\" y=\"0\"/><connectionPointOut formalParameter=\"\"/></leftPowerRail>\n\
        <contact localId=\"2\"><position x=\"10\" y=\"0\"/><connectionPointIn><connection refLocalId=\"1\"/></connectionPointIn><variable>nosuch</variable></contact>\n\
        <coil localId=\"3\"><position x=\"20\" y=\"0\"/><connectionPointIn><connection refLocalId=\"2\"/></connectionPointIn><variable>out</variable></coil>\n\
        </LD>";
    let xml = project(&[("out", "BOOL")], body);
    let msg = errors(&xml);
    assert!(msg.starts_with(&format!("{}:", line_of(&xml, "<coil"))), "{msg}");
    assert!(msg.contains("NOSUCH"), "{msg}");
}

fn ld(body: &str) -> String {
    project(&[("a", "BOOL"), ("b", "BOOL")], &format!("<LD>{body}</LD>"))
}

fn conns(from: &[u32]) -> String {
    from.iter().fold(String::new(), |mut s, f| {
        write!(s, "<connection refLocalId=\"{f}\"/>").unwrap();
        s
    })
}

const RAIL: &str = "<leftPowerRail localId=\"1\"><connectionPointOut formalParameter=\"\"/></leftPowerRail>";

fn contact(id: u32, from: &[u32], var: &str) -> String {
    let conns = conns(from);
    format!(
        "<contact localId=\"{id}\"><connectionPointIn>{conns}</connectionPointIn><variable>{var}</variable></contact>"
    )
}

fn coil(id: u32, from: u32, var: &str, attrs: &str) -> String {
    format!(
        "<coil localId=\"{id}\" {attrs}><connectionPointIn><connection refLocalId=\"{from}\"/></connectionPointIn><variable>{var}</variable></coil>"
    )
}

#[test]
fn structural_errors() {
    let cases: &[(String, &str)] = &[
        (
            ld(&format!("{RAIL}{}", coil(2, 9, "a", ""))),
            "localId 9, which does not exist",
        ),
        (
            ld(&format!(
                "{RAIL}{}{}",
                contact(2, &[1], "a"),
                contact(2, &[1], "b")
            )),
            "used twice",
        ),
        (
            ld(&format!(
                "{}{}{}",
                contact(2, &[3], "a"),
                contact(3, &[2], "b"),
                coil(4, 3, "a", "")
            )),
            "feedback loop",
        ),
        (ld("<jump localId=\"2\" label=\"x\"/>"), "jumps and labels"),
        (ld("<vendorElement localId=\"2\"/>"), "not supported in LD or FBD"),
        (
            ld(&format!(
                "{RAIL}<contact localId=\"2\"><variable>a</variable></contact>{}",
                coil(3, 2, "b", "")
            )),
            "contact is not connected",
        ),
        (
            ld(&format!(
                "{RAIL}{}",
                coil(2, 1, "a", "storage=\"set\" negated=\"true\"")
            )),
            "set or reset coil",
        ),
        (
            ld(&format!("{RAIL}{}", coil(2, 1, "a", "edge=\"sideways\""))),
            "expected none, rising or falling",
        ),
        (
            ld(&format!("{RAIL}{}", coil(2, 1, "a + b", ""))),
            "is not a variable",
        ),
        (project(&[], "<SFC/>"), "SFC bodies"),
        (project(&[], "<IL><xhtml:p>LD a</xhtml:p></IL>"), "IL bodies"),
        (project(&[], "<LD/><FBD/>"), "exactly one language"),
        (project(&[("s", "string")], &st("")), "STRING"),
        ("<plc/>".to_string(), "expected a PLCopen <project>"),
        (
            "<project><types><pous><pou name=\"p\" pouType=\"task\"/></pous></types></project>".into(),
            "pouType",
        ),
        (
            "<project><types><pous><pou name=\"p q\" pouType=\"program\"/></pous></types></project>".into(),
            "not a valid name",
        ),
    ];
    for (xml, want) in cases {
        let msg = errors(xml);
        assert!(msg.contains(want), "want {want:?}, got:\n{msg}\n{xml}");
    }
}

/// An expression or variable text must be exactly one expression: no
/// statements, unterminated comments or declarations sneak through into the
/// generated program.
#[test]
fn injected_text_is_refused() {
    for bad in [
        "a; b := TRUE",
        "a := TRUE",
        "a (*",
        "a END_PROGRAM PROGRAM q",
        "a) OR (b",
        "",
    ] {
        let xml = ld(&format!("{RAIL}{}{}", contact(2, &[1], bad), coil(3, 2, "b", "")));
        let msg = errors(&xml);
        assert!(
            msg.starts_with(&format!("{}:", line_of(&xml, "<contact"))),
            "{bad:?}: {msg}"
        );
    }
    let xml = project(&[("a", "BOOL")], &st("a := TRUE;")).replace(
        "<variable name=\"a\">",
        "<variable name=\"a\" address=\"%QX0.0; x := 1\">",
    );
    assert!(errors(&xml).contains("not a direct address"));
}

#[test]
fn xml_problems_are_reported_with_positions() {
    for (xml, want) in [
        ("<!DOCTYPE p [<!ENTITY a 'b'>]><project/>", "1:1: DOCTYPE"),
        (
            "<project>\n  <types>\n</project>",
            "3:1: </project> closes <types>",
        ),
        ("<project a='&bogus;'/>", "unknown entity"),
    ] {
        let msg = errors(xml);
        assert!(msg.contains(want), "{xml}: {msg}");
    }
}

#[test]
fn lowering_maps_positions() {
    let xml = fixture("motor.xml");
    let l = plcopen::lower(&xml).unwrap();
    let st = l.st();
    let k = st.lines().position(|s| s.starts_with("delay(")).unwrap();
    let origin = l.origin(Pos {
        line: u32::try_from(k).unwrap() + 1,
        col: 1,
    });
    assert_eq!(origin.line, line_of(&xml, "<block localId=\"9\""));
}

// ---- properties ----

/// A series-parallel contact network.
#[derive(Debug, Clone)]
enum Net {
    Contact { input: usize, negated: bool },
    Series(Vec<Net>),
    Parallel(Vec<Net>),
}

fn net() -> impl Strategy<Value = Net> {
    let leaf = (0..4usize, any::<bool>()).prop_map(|(input, negated)| Net::Contact { input, negated });
    leaf.prop_recursive(4, 24, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..4).prop_map(Net::Series),
            prop::collection::vec(inner, 2..4).prop_map(Net::Parallel),
        ]
    })
}

fn eval(n: &Net, bits: u8) -> bool {
    match n {
        Net::Contact { input, negated } => (bits >> input & 1 == 1) != *negated,
        Net::Series(ns) => ns.iter().all(|n| eval(n, bits)),
        Net::Parallel(ns) => ns.iter().any(|n| eval(n, bits)),
    }
}

/// Renders `n` fed from `from`; returns the ids its power leaves through.
fn render(n: &Net, from: &[u32], next: &mut u32, out: &mut String) -> Vec<u32> {
    match n {
        Net::Contact { input, negated } => {
            *next += 1;
            let id = *next;
            let conns = conns(from);
            writeln!(
                out,
                "<contact localId=\"{id}\" negated=\"{negated}\"><position x=\"{id}\" y=\"0\"/>\
                 <connectionPointIn>{conns}</connectionPointIn><connectionPointOut/>\
                 <variable>%IX0.{input}</variable></contact>"
            )
            .unwrap();
            vec![id]
        }
        Net::Series(ns) => ns.iter().fold(from.to_vec(), |f, n| render(n, &f, next, out)),
        Net::Parallel(ns) => ns.iter().flat_map(|n| render(n, from, next, out)).collect(),
    }
}

fn ladder(n: &Net) -> String {
    let mut body = String::from(RAIL);
    body.push('\n');
    let mut next = 1;
    let ends = render(n, &[1], &mut next, &mut body);
    let conns = conns(&ends);
    write!(
        body,
        "<coil localId=\"{}\"><position x=\"0\" y=\"10\"/><connectionPointIn>{conns}</connectionPointIn>\
         <variable>%QX0.0</variable></coil>\n<coil localId=\"{}\"><position x=\"0\" y=\"20\"/>\
         <connectionPointIn><connection refLocalId=\"{}\"/></connectionPointIn><variable>%QX0.1</variable></coil>",
        next + 1,
        next + 2,
        next + 1,
    )
    .unwrap();
    project(&[], &format!("<LD>{body}</LD>"))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Power flow through any series-parallel network of contacts is the
    /// AND/OR formula it draws, for every input combination; a coil passes
    /// the same power on.
    #[test]
    fn ladder_is_its_boolean_formula(n in net()) {
        let xml = ladder(&n);
        let mut p = build(&xml);
        for bits in 0..16u8 {
            let mut img = Image::new(SIZES);
            img.i[0] = bits;
            p.scan(&mut img).unwrap();
            let want = u8::from(eval(&n, bits));
            prop_assert_eq!(img.q[0] & 1, want, "inputs {:04b}", bits);
            prop_assert_eq!(img.q[0] >> 1 & 1, want, "second coil, inputs {:04b}", bits);
        }
    }

    /// Damaged projects are refused or compiled, never a panic.
    #[test]
    fn damaged_projects_never_panic(
        which in 0..4usize,
        cuts in prop::collection::vec((any::<prop::sample::Index>(), 0..40usize, ".{0,8}"), 1..4),
    ) {
        let names = ["motor.xml", "counter_fbd.xml", "coils.xml", "types_st.xml"];
        let mut s = fixture(names[which]).into_bytes();
        for (at, len, insert) in cuts {
            let i = at.index(s.len());
            let j = (i + len).min(s.len());
            s.splice(i..j, insert.into_bytes());
        }
        let s = String::from_utf8_lossy(&s);
        let _ = compile_plcopen(&s, SIZES);
    }

    #[test]
    fn arbitrary_text_never_panics(s in "\\PC{0,200}") {
        let _ = plcopen::lower(&s);
        let _ = plcopen::lower(&format!("<project>{s}</project>"));
    }
}
