mod common;
use common::*;
use helmsman_drawing::dxf::{Entity, MAX_BYTES, MAX_TEXT};
use helmsman_drawing::{DxfError, read};
use proptest::prelude::*;

#[test]
fn the_fixture_reads_and_says_what_it_did_not_read() {
    let d = read(&fixture()).unwrap();
    assert_eq!(d.version.as_deref(), Some("AC1032"));
    assert_eq!(d.units, Some(4));
    assert!(
        d.block("iologik_e1213").is_some(),
        "block names are case-insensitive"
    );
    assert_eq!(d.blocks.len(), 7);
    assert_eq!(d.unsupported.get("CIRCLE"), Some(&2));
    assert_eq!(d.unsupported.get("DIMENSION"), Some(&1));
    assert_eq!(d.sections_not_read, ["TABLES", "OBJECTS"]);
    assert!(!d.latin1);
    let lines = d.entities.iter().filter(|e| matches!(e, Entity::Line(_))).count();
    assert_eq!(lines, 1);
    let mtext = d.entities.iter().find_map(|e| match e {
        Entity::Text(t) if t.multiline => Some(t),
        _ => None,
    });
    assert_eq!(mtext.unwrap().text, "Remote I/O station 4");
}

#[test]
fn binary_dxf_is_refused_by_name() {
    let mut b = b"AutoCAD Binary DXF\r\n\x1a\x00".to_vec();
    b.extend_from_slice(&[0, 83, 69, 67, 84, 73, 79, 78, 0]);
    assert_eq!(read(&b), Err(DxfError::Binary));
}

#[test]
fn input_that_is_not_dxf_is_an_error() {
    assert_eq!(read(b""), Err(DxfError::NotDxf));
    assert_eq!(read(b"\n\n"), Err(DxfError::BadGroupCode { line: 1 }));
    assert_eq!(
        read(b"hello world\nsecond line\n"),
        Err(DxfError::BadGroupCode { line: 1 })
    );
    assert_eq!(read(b"0\nFOO\n0\nEOF\n"), Err(DxfError::NotDxf));
    assert_eq!(read(b"%PDF-1.7\n"), Err(DxfError::BadGroupCode { line: 1 }));
}

#[test]
fn every_proper_prefix_of_a_file_is_an_error() {
    let full = fixture();
    let end = full.len() - 2; // "0\r\nEOF" without its last line break is still whole
    for n in 0..end {
        assert!(
            read(&full[..n]).is_err(),
            "a prefix of {n} bytes read as a whole file"
        );
    }
    assert!(read(&full[..end]).is_ok());
}

#[test]
fn a_trailing_partial_pair_is_truncation() {
    let mut f = wrap(&insert("A", 1.0, 2.0));
    assert!(read(&f).is_ok());
    f.truncate(f.len() - "0\nEOF\n".len());
    f.extend_from_slice(b"0\n");
    assert!(matches!(read(&f), Err(DxfError::Truncated { .. })));
    // a group code with no value line at all
    let g = b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0";
    assert!(matches!(read(g), Err(DxfError::Truncated { .. })));
}

#[test]
fn line_endings_do_not_change_the_reading() {
    let lf = String::from_utf8(fixture()).unwrap().replace("\r\n", "\n");
    let a = read(lf.as_bytes()).unwrap();
    assert_eq!(read(&fixture()).unwrap(), a);
    assert_eq!(read(lf.replace('\n', "\r").as_bytes()).unwrap(), a, "CR only");
    assert_eq!(read(format!("\u{feff}{lf}").as_bytes()).unwrap(), a, "BOM");
    let mixed: String = lf
        .split('\n')
        .enumerate()
        .flat_map(|(i, l)| [l, ["\n", "\r\n"][i % 2]])
        .collect();
    assert_eq!(
        read(mixed.as_bytes()).unwrap(),
        a,
        "LF and CRLF mixed (CR mixed in could swallow an empty value line)"
    );
}

#[test]
fn bad_group_codes_and_numbers_are_errors_with_a_line() {
    let e = |b: &[u8]| read(b).unwrap_err();
    assert_eq!(
        e(b"0\nSECTION\n2\nENTITIES\nxx\nENDSEC\n"),
        DxfError::BadGroupCode { line: 5 }
    );
    assert_eq!(
        e(b"0\nSECTION\n2\nENTITIES\n2000\nA\n"),
        DxfError::BadGroupCode { line: 5 }
    );
    assert_eq!(
        e(b"0\nSECTION\n2\nENTITIES\n-1\nA\n"),
        DxfError::BadGroupCode { line: 5 }
    );
    for bad in ["1e999", "NaN", "inf", "-inf", "1e16", "1e300", "abc", ""] {
        let f = wrap(&format!("0\nINSERT\n2\nA\n10\n{bad}\n"));
        assert!(matches!(read(&f), Err(DxfError::BadNumber { .. })), "{bad:?}");
    }
    // the largest coordinate is still read
    assert!(read(&wrap("0\nINSERT\n2\nA\n10\n1e15\n20\n-1e15\n")).is_ok());
}

#[test]
fn structure_errors_are_named() {
    let m = |b: Vec<u8>| matches!(read(&b), Err(DxfError::Malformed { .. }));
    assert!(
        m(b"0\nSECTION\n0\nENDSEC\n0\nEOF\n".to_vec()),
        "a section with no name"
    );
    assert!(
        m(b"0\nSECTION\n2\nENTITIES\n8\n0\n".to_vec()),
        "a pair that is no entity"
    );
    assert!(
        m(b"0\nSECTION\n2\nENTITIES\n0\nEOF\n".to_vec()),
        "EOF inside a section"
    );
    assert!(m(wrap("0\nLWPOLYLINE\n20\n5\n")), "a vertex's y with no x");
    assert!(
        m(b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nFOO\n0\nEOF\n".to_vec()),
        "an entity between sections"
    );
    assert!(
        m(b"0\nSECTION\n2\nBLOCKS\n0\nLINE\n0\nENDSEC\n0\nEOF\n".to_vec()),
        "an entity outside a block"
    );
}

#[test]
fn non_utf8_is_read_as_latin1_and_says_so() {
    let mut f = wrap("0\nINSERT\n2\nA\n66\n1\n0\nATTRIB\n2\nTAG\n1\nM\u{fffd}ller\n0\nSEQEND\n");
    let at = f.windows(3).position(|w| w == [0xEF, 0xBF, 0xBD]).unwrap();
    f.splice(at..at + 3, [0xFC]);
    assert!(std::str::from_utf8(&f).is_err());
    let d = read(&f).unwrap();
    assert!(d.latin1);
    let Entity::Insert(i) = &d.entities[0] else {
        panic!()
    };
    assert_eq!(i.attribs[0].value, "M\u{fc}ller");
    let mut junk = wrap("");
    junk.splice(0..0, [0xFF, 0xFE, 0x00]);
    assert!(read(&junk).is_err());
}

#[test]
fn limits_are_errors_not_exhaustion() {
    assert_eq!(read(&vec![b'0'; MAX_BYTES + 1]), Err(DxfError::TooLarge));
    let many = "0\nCIRCLE\n".repeat(1_000_001);
    assert_eq!(read(&wrap(&many)), Err(DxfError::TooMany("entities")));
    let ok = "0\nCIRCLE\n".repeat(1000);
    assert_eq!(read(&wrap(&ok)).unwrap().unsupported["CIRCLE"], 1000);
}

#[test]
fn a_giant_text_is_cut_and_noted() {
    let big = "A".repeat(5_000_000);
    let d = read(&wrap(&format!("0\nTEXT\n1\n{big}\n10\n0\n20\n0\n"))).unwrap();
    let Entity::Text(t) = &d.entities[0] else { panic!() };
    assert_eq!(t.text.len(), MAX_TEXT);
    assert!(d.notes.iter().any(|n| n.contains("cut")));
}

#[test]
fn an_unsupported_entity_is_named_never_dropped_quietly() {
    let f = wrap(&pairs(&[
        ("0", "ARC"),
        ("0", "SPLINE"),
        ("0", "ARC"),
        ("0", "HATCH"),
        ("0", "POLYLINE"),
        ("0", "VERTEX"),
        ("0", "VERTEX"),
        ("0", "SEQEND"),
    ]));
    let d = read(&f).unwrap();
    let got: Vec<_> = d.unsupported.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_eq!(got, [("ARC", 2), ("HATCH", 1), ("POLYLINE", 1), ("SPLINE", 1)]);
    assert_eq!(d.entities, []);
}

#[test]
fn an_orphan_attrib_and_an_array_insert_are_noted() {
    let d = read(&wrap("0\nATTRIB\n2\nT\n1\nv\n0\nINSERT\n2\nA\n70\n3\n71\n2\n")).unwrap();
    assert_eq!(d.unsupported["ATTRIB (outside an INSERT)"], 1);
    assert!(d.notes.iter().any(|n| n.contains("array insert")));
}

#[test]
fn attribs_attach_to_their_insert_only() {
    let f = wrap(&pairs(&[
        ("0", "INSERT"),
        ("2", "A"),
        ("66", "1"),
        ("0", "ATTRIB"),
        ("2", "TAG"),
        ("1", "-A1"),
        ("0", "ATTRIB"),
        ("2", "PARTNO"),
        ("1", "X"),
        ("0", "SEQEND"),
        ("0", "ATTRIB"),
        ("2", "STRAY"),
        ("1", "no"),
        ("0", "INSERT"),
        ("2", "B"),
    ]));
    let d = read(&f).unwrap();
    let ins: Vec<_> = d
        .entities
        .iter()
        .filter_map(|e| if let Entity::Insert(i) = e { Some(i) } else { None })
        .collect();
    assert_eq!(ins.len(), 2);
    assert_eq!(ins[0].attribs.len(), 2);
    assert_eq!(ins[1].attribs, []);
    assert_eq!(d.unsupported["ATTRIB (outside an INSERT)"], 1);
}

#[test]
fn duplicate_blocks_and_a_block_cap() {
    let two = block("A", "") + &block("a", "0\nLINE\n");
    let d = read(&wrap_blocks(&two, "")).unwrap();
    assert_eq!(d.blocks.len(), 1);
    assert_eq!(
        d.block("A").unwrap().entities.len(),
        1,
        "the later definition wins"
    );
    assert!(d.notes.iter().any(|n| n.contains("defined twice")));
    let many: String = (0..20_001).map(|i| block(&format!("B{i}"), "")).collect();
    assert_eq!(read(&wrap_blocks(&many, "")), Err(DxfError::TooMany("blocks")));
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(b in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = read(&b);
    }

    #[test]
    fn arbitrary_pair_soup_never_panics(toks in proptest::collection::vec(
        prop::sample::select(vec![
            "0", "2", "10", "20", "11", "21", "41", "50", "66", "70", "71", "1", "3", "8", "999", "-5", "99999",
            "SECTION", "ENDSEC", "BLOCKS", "ENTITIES", "HEADER", "BLOCK", "ENDBLK", "INSERT", "ATTRIB", "ATTDEF",
            "SEQEND", "TEXT", "MTEXT", "LINE", "LWPOLYLINE", "POLYLINE", "VERTEX", "EOF", "A", "1e5", "-0", "1e999",
            "\\P{\\f", "", " ", "\r", "\u{fffd}",
        ]), 0..120)) {
        let s = toks.join("\n");
        if let Ok(d) = read(s.as_bytes()) {
            let p = helmsman_drawing::parts(&d);
            let lib = library();
            let m = helmsman_drawing::Matcher::new(&lib);
            for part in &p.parts { let _ = m.match_part(part); }
        }
    }

    #[test]
    fn a_wrapped_pair_soup_reaches_the_entity_code(toks in proptest::collection::vec(
        prop::sample::select(vec![
            "0\nINSERT\n", "0\nATTRIB\n", "0\nSEQEND\n", "0\nTEXT\n", "0\nMTEXT\n", "0\nLINE\n", "0\nLWPOLYLINE\n",
            "0\nATTDEF\n", "2\nA\n", "2\nB\n", "66\n1\n", "1\nx\n", "3\n\\P\\H2;\n", "10\n5\n", "20\n6\n", "11\n1\n",
            "21\n1\n", "41\n0\n", "42\n-2\n", "50\n1e14\n", "70\n9\n", "71\n-4\n", "72\n1\n", "40\n2\n",
        ]), 0..60)) {
        let blocks = block("A", &toks.concat()) + &block("B", &insert("A", 1.0, 1.0));
        let f = wrap_blocks(&blocks, &(toks.concat() + &insert("B", 2.0, 2.0) + &insert("A", 0.0, 0.0)));
        if let Ok(d) = read(&f) {
            let p = helmsman_drawing::parts(&d);
            for part in &p.parts {
                prop_assert!(part.x.is_finite() && part.y.is_finite() && part.rotation.is_finite());
                prop_assert!((0.0..360.0).contains(&part.rotation));
            }
        }
    }
}
