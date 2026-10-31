//! The canvas ships to browsers as wasm32 (`make canvas-web`), so nothing it
//! pulls in may need a C toolchain or OS entropy that wasm32-unknown-unknown
//! lacks. The snapshot store (SQLite) and https sinks (ring) live behind
//! helmsman-plc's `box` / `https` features for exactly this reason (the
//! canvas runs on helmsman-iec and helmsman-diagram; helmsman-plc is only
//! a native dev-dependency). This asks cargo for the resolved wasm
//! graph rather than trusting a manifest, so a dependency that drags them
//! back in fails here, not in a browser.

use std::process::Command;

#[test]
fn the_wasm_build_pulls_in_no_native_only_crates() {
    let out = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "-p",
            "helmsman-canvas",
            "--target",
            "wasm32-unknown-unknown",
            "-e",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        tree.contains("helmsman-iec"),
        "the canvas runs the simulation on the interpreter:\n{tree}"
    );
    for native in [
        "ring ",
        "rusqlite ",
        "libsqlite3-sys ",
        "rustls ",
        "rustix ",
        "gpiocdev ",
        "i2cdev ",
        "nix ",
    ] {
        assert!(
            !tree.lines().any(|l| l.starts_with(native)),
            "{} is in the canvas's wasm build:\n{tree}",
            native.trim()
        );
    }
}
