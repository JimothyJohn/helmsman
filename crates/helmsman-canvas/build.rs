//! Bakes the build stamp into the canvas (`HELMSMAN_BUILD`, for Help ›
//! About and the different-build banner; `crates/build_stamp.rs` says
//! how), and the glyph library: every `assets/glyphs/*.svg` rasterized to
//! a square alpha mask (`glyph_lib::SIDE` a side) and listed in
//! `$OUT_DIR/glyphs.rs`, which `glyph_lib` includes. The SVG stays the
//! source an engineer redraws; the page carries only the masks and no
//! SVG code (resvg is a build dependency, never in the wasm graph).

use std::fmt::Write as _;
use std::path::Path;

mod stamp {
    include!("../build_stamp.rs");
}

/// A glyph mask's side, pixels: a glyph is drawn at most a tap target
/// (28 pt) on a 2x display.
const SIDE: u32 = 64;

fn glyphs() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/glyphs");
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("glyphs");
    std::fs::create_dir_all(&out).expect("the glyph output directory");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("assets/glyphs")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "svg"))
        .collect();
    files.sort();
    let mut list = String::from("/// Every glyph: its name and its alpha mask, `SIDE` by `SIDE`.\n");
    list.push_str("pub static GLYPHS: &[(&str, &[u8])] = &[\n");
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "glyph file names are lower-case words joined by '-': {name}"
        );
        let svg = std::fs::read(&path).expect("read the glyph");
        let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let size = tree.size();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(SIDE, SIDE).expect("a pixmap");
        #[allow(clippy::cast_precision_loss)]
        let scale =
            resvg::tiny_skia::Transform::from_scale(SIDE as f32 / size.width(), SIDE as f32 / size.height());
        resvg::render(&tree, scale, &mut pixmap.as_mut());
        // Premultiplied RGBA to alpha alone: the colour is the app's.
        let alpha: Vec<u8> = pixmap.data().as_chunks::<4>().0.iter().map(|px| px[3]).collect();
        assert!(alpha.iter().any(|&a| a > 0), "{}: draws nothing", path.display());
        let mask = out.join(format!("{name}.a8"));
        std::fs::write(&mask, &alpha).expect("write the mask");
        let _ = writeln!(list, "    ({name:?}, include_bytes!({:?})),", mask.display());
    }
    list.push_str("];\n");
    std::fs::write(out.with_extension("rs"), list).expect("write glyphs.rs");
}

fn main() {
    stamp::main();
    glyphs();
}
