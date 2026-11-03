//! The box build (`cargo build -p helmsman-plc`, what `pi-deploy-plc.sh`
//! ships) must not enable `helmsman-proto`'s `unrestricted` feature:
//! without it the only requests the box can send to a controller it
//! observes are reads (`docs/ARCHITECTURE.md`, trust boundary "box ↔
//! observed PLCs"). Its Class 1 `scanner` (the `enip-io` driver) is in,
//! and brings nothing wider.
//! Feature unification is a property of the whole dependency graph, so this
//! asks cargo for the resolved features rather than trusting one manifest.

use std::process::Command;

#[test]
fn box_build_cannot_send_arbitrary_services() {
    let out = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            "helmsman-plc",
            "-e",
            "normal,features",
            "-i",
            "helmsman-proto",
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
        tree.contains("helmsman-proto"),
        "proto not in the box build?\n{tree}"
    );
    assert!(
        !tree.contains("\"unrestricted\""),
        "the box build enables helmsman-proto/unrestricted:\n{tree}"
    );
    // The Class 1 scanner (helmsman-io's enip-io) is in the box since the
    // E1213 bench passed (TODO.md `e1213-bench`).
    assert!(
        tree.contains("\"scanner\""),
        "the box build lacks helmsman-proto/scanner (enip-io):\n{tree}"
    );
}

/// The `enip-io` feature on its own brings in `scanner`, and still not
/// `unrestricted`.
#[test]
fn the_enip_io_feature_brings_the_scanner_and_nothing_wider() {
    let out = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            "helmsman-plc",
            "--features",
            "enip-io",
            "-e",
            "normal,features",
            "-i",
            "helmsman-proto",
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
    assert!(tree.contains("\"scanner\""), "{tree}");
    assert!(!tree.contains("\"unrestricted\""), "{tree}");
}

/// The OPC UA tests drive the server with `async-opcua`'s client, a
/// dev-dependency only. It must never reach the box: it pulls tokio and
/// some 150 crates, and its crypto (`rsa`) carries RUSTSEC-2023-0071, an
/// unpatched timing side channel (docs/ARCHITECTURE.md "OPC UA server").
#[test]
fn box_build_carries_no_opcua_crate_or_rsa() {
    let out = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            "helmsman-plc",
            "-e",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(tree.contains("helmsman-plc"), "{tree}");
    for line in tree.lines() {
        let name = line.split_whitespace().next().unwrap_or("");
        assert!(
            !(name.starts_with("async-opcua") || name == "rsa" || name == "tokio"),
            "the box build depends on {name}:\n{tree}"
        );
    }
}
