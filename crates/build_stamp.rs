// The build stamp, shared by the build scripts that `include!` it
// (helmsman-plc's and helmsman-canvas's): `HELMSMAN_BUILD`, the
// checkout's `git describe --always --abbrev=9`, so the box and the
// canvas it serves can tell they come from different builds (the box
// sends it as `X-Helmsman-Build`; the canvas shows a banner on a
// mismatch). `HELMSMAN_BUILD` set in the environment wins (a build from a
// tarball, a release script); without either it is "unknown", which never
// counts as a mismatch.
//
// No `--dirty`: the script reruns when HEAD moves, not on every edit, so a
// dirty mark would go stale. A tree built twice from one commit stamps the
// same either way.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// What a header value and a label can carry: letters, digits and
/// `.-+_`, at most 64 of them; anything else is "unknown".
fn clean(s: &str) -> String {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
    if ok { s.to_string() } else { "unknown".into() }
}

/// Reruns when HEAD moves in this checkout (a worktree's own HEAD, its
/// reflog, the branch's ref or the packed refs), and only then: a path
/// that does not exist would rerun the script on every build.
fn watch_git() {
    // Relative to the script's directory (the crate's) when git says so.
    let here = std::env::current_dir().unwrap_or_default();
    let (Some(dir), Some(common)) = (
        git(&["rev-parse", "--git-dir"]),
        git(&["rev-parse", "--git-common-dir"]),
    ) else {
        return;
    };
    let (dir, common) = (here.join(dir), here.join(common));
    let mut paths = vec![dir.join("HEAD"), dir.join("logs").join("HEAD")];
    // The branch's loose ref, or the packed refs while it has none (a
    // branch moves under them only by being written loose again).
    let loose = git(&["symbolic-ref", "-q", "HEAD"])
        .map(|r| common.join(r))
        .filter(|p| p.exists());
    paths.push(loose.unwrap_or_else(|| common.join("packed-refs")));
    for p in paths.iter().filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

// `pub`: helmsman-canvas's build script includes this file as a module and
// calls it from its own `main` (the glyph library comes after the stamp).
pub fn main() {
    println!("cargo:rerun-if-env-changed=HELMSMAN_BUILD");
    let stamp = match std::env::var("HELMSMAN_BUILD") {
        Ok(s) if !s.trim().is_empty() => clean(s.trim()),
        _ => {
            watch_git();
            git(&["describe", "--always", "--abbrev=9"]).map_or_else(|| "unknown".into(), |s| clean(&s))
        }
    };
    println!("cargo:rustc-env=HELMSMAN_BUILD={stamp}");
}
