mod common;
use common::*;
use helmsman_drawing::parts::{MAX_DEPTH, MAX_PARTS};
use helmsman_drawing::{Match, Matcher, parts, read};

fn run(f: &[u8]) -> helmsman_drawing::Parts {
    parts(&read(f).unwrap())
}

#[test]
fn the_fixture_gives_its_parts_with_position_rotation_and_attributes() {
    let p = run(&fixture());
    assert_eq!(
        p.parts.len(),
        10,
        "{:#?}",
        p.parts.iter().map(|x| (&x.block, &x.inside)).collect::<Vec<_>>()
    );
    let by = |tag: &str| p.parts.iter().find(|x| x.tag.as_deref() == Some(tag)).unwrap();
    let a1 = by("-A1");
    assert_eq!((a1.x, a1.y, a1.rotation), (100.0, 200.0, 0.0));
    assert_eq!(a1.block, "IOLOGIK_E1213");
    assert_eq!(a1.part_number.as_deref(), Some("ioLogik E1213"));
    let b1 = by("-B1");
    assert_eq!((b1.x, b1.y, b1.rotation), (300.0, 200.0, 90.0));
    assert_eq!(b1.model.as_deref(), Some("IF6123"));
    assert_eq!(b1.part_number, None);
    // The ATTRIB's tag beats the ATTDEF's default of the same tag.
    assert!(b1.attributes.iter().all(|a| !a.from_default || a.tag != "TAG"));
}

#[test]
fn a_nested_insert_is_placed_through_the_ones_around_it() {
    let p = run(&fixture());
    let inner = p
        .parts
        .iter()
        .find(|x| x.block == "SENSOR_M12" && !x.inside.is_empty())
        .unwrap();
    assert_eq!(inner.inside, ["CABINET"]);
    assert_eq!((inner.x, inner.y), (1100.0, 50.0));
    let d = inner.attributes.iter().find(|a| a.tag == "TAG").unwrap();
    assert!(
        d.from_default,
        "no ATTRIB on it, so the block's default shows, marked as one"
    );
}

#[test]
fn rotation_and_scale_compose_through_a_block_with_a_base_point() {
    // CAB at (10,0) rotated 90 degrees, scale 2, base point (1,1); a part at (3,1) inside it.
    let blocks =
        "0\nBLOCK\n2\nCAB\n10\n1\n20\n1\n0\nINSERT\n2\nP\n10\n3\n20\n1\n50\n30\n0\nENDBLK\n".to_string();
    let e = "0\nINSERT\n2\nCAB\n10\n10\n20\n0\n41\n2\n42\n2\n50\n90\n";
    let p = run(&wrap_blocks(&blocks, e));
    let n = &p.parts[1];
    // local offset (2,0) -> scaled (4,0) -> rotated 90 -> (0,4) -> moved to (10,4)
    assert!((n.x - 10.0).abs() < 1e-9 && (n.y - 4.0).abs() < 1e-9, "{n:?}");
    assert!((n.rotation - 120.0).abs() < 1e-9, "{}", n.rotation);
}

#[test]
fn nearby_text_is_found_only_for_a_block_without_attributes() {
    let p = run(&fixture());
    let murr = p.parts.iter().find(|x| x.block == "MURR_8DI8DO").unwrap();
    let texts: Vec<_> = murr.nearby_text.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(
        texts,
        ["Remote I/O station 4", "IMPACT67 E DI8 DO8"],
        "closest first"
    );
    let rail = p.parts.iter().find(|x| x.block == "DINRAIL").unwrap();
    assert_eq!(rail.nearby_text, []);
    let a1 = p.parts.iter().find(|x| x.tag.as_deref() == Some("-A1")).unwrap();
    assert!(a1.nearby_text.is_empty(), "it has attributes");
}

#[test]
fn a_block_that_inserts_itself_terminates() {
    let b = block("LOOP", &insert("LOOP", 1.0, 1.0));
    let p = run(&wrap_blocks(&b, &insert("LOOP", 0.0, 0.0)));
    assert_eq!(p.parts.len(), 2);
    assert!(
        p.notes.iter().any(|n| n.contains("inserts itself")),
        "{:?}",
        p.notes
    );
}

#[test]
fn mutual_recursion_terminates() {
    let b = block("A", &insert("B", 1.0, 0.0)) + &block("B", &insert("A", 1.0, 0.0));
    let p = run(&wrap_blocks(&b, &insert("A", 0.0, 0.0)));
    assert!(p.parts.len() <= 4);
    assert!(p.notes.iter().any(|n| n.contains("inserts itself")));
}

#[test]
fn a_deep_chain_stops_at_the_depth_cap() {
    let b: String = (0..30)
        .map(|i| block(&format!("L{i}"), &insert(&format!("L{}", i + 1), 1.0, 0.0)))
        .collect();
    let p = run(&wrap_blocks(&b, &insert("L0", 0.0, 0.0)));
    assert_eq!(p.parts.len(), MAX_DEPTH + 1);
    assert!(p.notes.iter().any(|n| n.contains("deeper")));
}

#[test]
fn an_exploding_tree_is_capped_in_parts_and_in_time() {
    let blocks: String = (0..7)
        .map(|i| {
            block(
                &format!("L{i}"),
                &insert(&format!("L{}", i + 1), 1.0, 0.0).repeat(20),
            )
        })
        .collect();
    let start = std::time::Instant::now();
    let p = run(&wrap_blocks(&blocks, &insert("L0", 0.0, 0.0)));
    assert_eq!(p.parts.len(), MAX_PARTS);
    assert!(p.notes.iter().any(|n| n.contains("more than")));
    assert!(start.elapsed().as_secs() < 20);
}

#[test]
fn huge_scales_and_coordinates_stay_finite() {
    let b: String = (0..7)
        .map(|i| {
            block(
                &format!("L{i}"),
                &format!(
                    "0\nINSERT\n2\nL{}\n10\n1e15\n20\n1e15\n41\n1e15\n42\n1e15\n50\n1e15\n",
                    i + 1
                ),
            )
        })
        .collect();
    let p = run(&wrap_blocks(
        &b,
        "0\nINSERT\n2\nL0\n10\n1e15\n20\n-1e15\n41\n1e15\n42\n1e15\n",
    ));
    assert!(p.parts.len() > 3);
    for x in &p.parts {
        assert!(x.x.is_finite() && x.y.is_finite() && (0.0..360.0).contains(&x.rotation));
    }
}

#[test]
fn an_insert_of_a_block_the_file_lacks_is_still_a_part() {
    let p = run(&wrap("0\nINSERT\n2\nGHOST\n10\n5\n20\n6\n"));
    assert_eq!(p.parts.len(), 1);
    assert_eq!(p.parts[0].block, "GHOST");
}

#[test]
fn the_fixture_matches_as_the_library_says() {
    let lib = library();
    let m = Matcher::new(&lib);
    let p = run(&fixture());
    let of = |f: &dyn Fn(&&helmsman_drawing::Part) -> bool| m.match_part(p.parts.iter().find(f).unwrap());
    assert!(
        matches!(of(&|x| x.tag.as_deref() == Some("-A1")), Match::Matched(r) if r.id == "moxa/iologik-e1213")
    );
    assert!(matches!(of(&|x| x.tag.as_deref() == Some("-B1")), Match::Matched(r) if r.id == "ifm/if6123"));
    assert!(matches!(of(&|x| x.tag.as_deref() == Some("-B2")), Match::Matched(r) if r.id == "ifm/o5d100"));
    // The Murrelektronik module has only a label beside it: a person confirms.
    assert!(
        matches!(of(&|x| x.block == "MURR_8DI8DO"), Match::Ambiguous(v) if v.len() == 1 && v[0].id == "murrelektronik/impact67-e-di8-do8")
    );
    assert_eq!(of(&|x| x.tag.as_deref() == Some("-X9")), Match::Unknown);
    assert_eq!(of(&|x| x.block == "DINRAIL"), Match::Unknown);
}
