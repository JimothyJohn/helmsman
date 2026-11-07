//! The commit the box is built from, as `HELMSMAN_COMMIT` (Help mode
//! attaches it to an issue, `api::help`): the environment's when set (a
//! build from a tarball), else `git rev-parse HEAD`, else `unknown`.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(s.trim().to_string())
}

fn commit_ok(c: &str) -> bool {
    (7..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit())
}

fn from_git() -> Option<String> {
    let dir = git(&["rev-parse", "--absolute-git-dir"])?;
    // Built again when HEAD moves: a commit, a checkout, a merge.
    for f in ["HEAD", "logs/HEAD"] {
        let p = Path::new(&dir).join(f);
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
    git(&["rev-parse", "HEAD"]).filter(|c| commit_ok(c))
}

fn main() {
    println!("cargo:rerun-if-env-changed=HELMSMAN_COMMIT");
    let commit = std::env::var("HELMSMAN_COMMIT")
        .ok()
        .filter(|c| commit_ok(c))
        .or_else(from_git)
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=HELMSMAN_COMMIT={commit}");
}
