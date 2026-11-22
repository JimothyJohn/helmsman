//! The reader over real vendor DXF files, when they are at hand.
//!
//! Vendor files are not committed (their terms do not clearly allow it):
//! put them, one directory per vendor or flat, in a directory and name it in
//! `HELMSMAN_DXF_CACHE`. Without it this test says why it did nothing.
mod common;
use common::*;
use helmsman_drawing::{DxfError, Matcher, parts, read};
use std::path::{Path, PathBuf};

fn dxfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut es: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    es.sort();
    for p in es {
        if p.is_dir() {
            dxfs(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dxf")) {
            out.push(p);
        }
    }
}

#[test]
fn real_vendor_files_read_without_panic_and_with_a_named_outcome() {
    let Some(dir) = std::env::var_os("HELMSMAN_DXF_CACHE") else {
        eprintln!(
            "SKIPPED: HELMSMAN_DXF_CACHE is not set; no vendor DXF files to read (none are committed: see tests/fixtures/README.md)"
        );
        return;
    };
    let mut files = Vec::new();
    dxfs(Path::new(&dir), &mut files);
    if files.is_empty() {
        eprintln!("SKIPPED: no .dxf file under {dir:?}");
        return;
    }
    let lib = library();
    let m = Matcher::new(&lib);
    for f in files {
        let bytes = std::fs::read(&f).unwrap();
        match read(&bytes) {
            Ok(d) => {
                let p = parts(&d);
                let matched = p
                    .parts
                    .iter()
                    .filter(|x| matches!(m.match_part(x), helmsman_drawing::Match::Matched(_)))
                    .count();
                eprintln!(
                    "{}: {} blocks, {} parts ({matched} matched), not read: {:?} {:?}",
                    f.display(),
                    d.blocks.len(),
                    p.parts.len(),
                    d.unsupported,
                    d.sections_not_read
                );
            }
            Err(e @ (DxfError::Binary | DxfError::TooLarge)) => eprintln!("{}: refused: {e}", f.display()),
            Err(e) => panic!("{}: {e}", f.display()),
        }
    }
}
