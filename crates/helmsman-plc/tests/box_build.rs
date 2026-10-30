//! The box build (`cargo build -p helmsman-plc`, what `pi-deploy-plc.sh`
//! ships) must not enable `helmsman-proto`'s `unrestricted` feature: without
//! it the only requests the box can send to a controller it observes are
//! reads (`docs/ARCHITECTURE.md`, trust boundary "box ↔ observed PLCs").
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
}
