#![allow(dead_code)]
use helmsman_drawing::LibraryRecord;

/// The hand-made fixture (not a vendor file: see tests/fixtures/README.md).
pub fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/panel-layout-handmade.dxf"
    ))
    .unwrap()
}

/// Pairs as text, `code value` per entry, LF endings.
pub fn pairs(p: &[(&str, &str)]) -> String {
    p.iter().flat_map(|(c, v)| [*c, "\n", *v, "\n"]).collect()
}

/// A file with `entities` (already pairs text) in its ENTITIES section.
pub fn wrap(entities: &str) -> Vec<u8> {
    wrap_blocks("", entities)
}

/// A file with `blocks` (pairs text of BLOCK..ENDBLK) and `entities`.
pub fn wrap_blocks(blocks: &str, entities: &str) -> Vec<u8> {
    let mut s = String::from("0\nSECTION\n2\nBLOCKS\n");
    s.push_str(blocks);
    s.push_str("0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
    s.push_str(entities);
    s.push_str("0\nENDSEC\n0\nEOF\n");
    s.into_bytes()
}

pub fn block(name: &str, body: &str) -> String {
    format!("0\nBLOCK\n2\n{name}\n10\n0\n20\n0\n{body}0\nENDBLK\n")
}

pub fn insert(name: &str, x: f64, y: f64) -> String {
    format!("0\nINSERT\n2\n{name}\n10\n{x}\n20\n{y}\n")
}

/// What the library's index holds for the parts the brief names, as the
/// matcher takes it (vendor, model text and order numbers as in index.json).
pub fn library() -> Vec<LibraryRecord> {
    let r = |id: &str, vendor: &str, model: &str, products: &[&str]| LibraryRecord {
        id: id.into(),
        vendor: vendor.into(),
        model: model.into(),
        products: products.iter().map(|s| (*s).into()).collect(),
    };
    vec![
        r("moxa/iologik-e1210", "Moxa", "ioLogik E1210", &[]),
        r("moxa/iologik-e1212", "Moxa", "ioLogik E1212", &[]),
        r("moxa/iologik-e1213", "Moxa", "ioLogik E1213", &[]),
        r(
            "murrelektronik/impact67-e-di16",
            "Murrelektronik",
            "IMPACT67 E DI16",
            &[],
        ),
        r(
            "murrelektronik/impact67-e-di8-do8",
            "Murrelektronik",
            "IMPACT67 E DI8 DO8",
            &[],
        ),
        r(
            "murrelektronik/impact67-e-dio8-dio8",
            "Murrelektronik",
            "IMPACT67 E DIO8 (DIO8)",
            &[],
        ),
        r(
            "ifm/if6123",
            "ifm",
            "IF6123, IF6124, IG6615, IG6616, II5973, II5974, IM5172, IM5173",
            &[
                "IF6123", "IF6124", "IG6615", "IG6616", "II5973", "II5974", "IM5172", "IM5173",
            ],
        ),
        r("ifm/kq6005", "ifm", "KQxxxx", &[]),
        r(
            "ifm/o5d100",
            "ifm",
            "O5D100/O5D102/O5D150/O5D152/O5D159",
            &["O5D100", "O5D102"],
        ),
        r(
            "ifm/pn7092",
            "ifm",
            "(PN7072, PN7092, PN7572, PN7592) Status B",
            &["PN7092"],
        ),
        r("sick/wtb4f-did8389194", "SICK", "WTB4F_DID8389194", &[]),
    ]
}
