//! `helmsman-plc bundle ...`: the publisher's side (keys, packing,
//! signing, verifying) and the integrator's (the box's trust store).
//!
//! A subcommand of the box binary rather than a second binary: it is the
//! same code the box verifies with (one build, one version of the rules),
//! and a publisher who has the box's binary has the tool. It never starts
//! the runtime, opens a socket or reads the box's state beyond the trust
//! file it is pointed at.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::sign::{self, SigningKey, TRUST_FILE, TrustStore, write_private};
use super::{
    Bundle, BundleManifest, EXTENSION, MAX_BUNDLE, Trust, capabilities, check_content, check_signatures,
    verify,
};
use crate::Window;
use crate::spec::Defaults;
use crate::store::ChangeSet;

pub const USAGE: &str = "\
usage: helmsman-plc bundle COMMAND ...
  keygen KEYFILE [--name NAME]
        a new Ed25519 publisher key, PKCS#8, mode 0600 (never overwrites);
        prints the trust-store line a box needs to trust it
  pack --module-dir DIR ... [--io-map FILE] [--calendar FILE]
       --name NAME --version V --author TEXT [--description TEXT] OUT
        a bundle of those files, unsigned; its capability manifest is
        computed from them (every module compiled and checked)
  sign --key KEYFILE [--name N] [--version V] [--author T] [--description T] IN OUT
        checks IN, optionally rewrites its manifest (only while unsigned),
        and adds KEYFILE's signature
  verify [--trust FILE] IN
        checks the file, every signature and the content; with --trust (a
        box's trusted-publishers file) also whether a box would take it
  trust [--state-dir DIR] list | add NAME KEY | dev-bundles allow|refuse
        the box's trust store (DIR/trusted-publishers, mode 0600)";

/// The `%M` size and the rest when a module does not say (as the binary's
/// defaults).
#[must_use]
pub const fn defaults() -> Defaults {
    Defaults {
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 1024,
    }
}

/// Flags and positional arguments.
struct Args {
    flags: Vec<(String, String)>,
    multi: Vec<PathBuf>,
    pos: Vec<String>,
}

fn args(a: &[String], with_value: &[&str], multi: &str) -> Result<Args, String> {
    let mut out = Args {
        flags: Vec::new(),
        multi: Vec::new(),
        pos: Vec::new(),
    };
    let mut i = 0;
    while i < a.len() {
        let f = a[i].as_str();
        if f.starts_with("--") {
            let v = a.get(i + 1).ok_or_else(|| format!("{f} needs a value"))?;
            if f == multi {
                out.multi.push(PathBuf::from(v));
            } else if with_value.contains(&f) {
                if out.flags.iter().any(|(k, _)| k == f) {
                    return Err(format!("one {f} only"));
                }
                out.flags.push((f.to_string(), v.clone()));
            } else {
                return Err(format!("unknown flag {f}"));
            }
            i += 2;
        } else {
            out.pos.push(a[i].clone());
            i += 1;
        }
    }
    Ok(out)
}

impl Args {
    fn get(&self, k: &str) -> Option<&str> {
        self.flags.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str())
    }

    fn need(&self, k: &str) -> Result<&str, String> {
        self.get(k).ok_or_else(|| format!("{k} is required"))
    }

    fn positional<const N: usize>(&self, what: &str) -> Result<[&str; N], String> {
        let v: Vec<&str> = self.pos.iter().map(String::as_str).collect();
        v.try_into().map_err(|_| format!("expected {what}"))
    }
}

fn read_bundle(path: &Path) -> Result<Vec<u8>, String> {
    let len = fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if len > MAX_BUNDLE as u64 {
        return Err(format!("{}: larger than a bundle may be", path.display()));
    }
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn summary(b: &Bundle) -> String {
    let m = &b.manifest;
    let list = |v: &std::collections::BTreeSet<String>| {
        if v.is_empty() {
            "(none)".to_string()
        } else {
            v.iter().cloned().collect::<Vec<_>>().join(" ")
        }
    };
    format!(
        "bundle {} {} by {}\n  id       {}\n  modules  {}\n  reads    {}\n  drives   {}\n  q-bytes  {}\n  devices  {}\n  drivers  {}\n",
        m.name,
        m.version,
        m.author,
        b.id(),
        list(&m.caps.modules),
        list(&m.caps.reads),
        list(&m.caps.drives),
        m.caps.q_bytes,
        list(&m.caps.devices),
        list(&m.caps.drivers),
    )
}

/// Runs `helmsman-plc bundle ARGS`; what to print on success.
///
/// # Errors
/// Why it failed, for stderr.
pub fn run(a: &[String]) -> Result<String, String> {
    let Some((cmd, rest)) = a.split_first() else {
        return Err(USAGE.into());
    };
    match cmd.as_str() {
        "keygen" => keygen(rest),
        "pack" => pack(rest),
        "sign" => sign_cmd(rest),
        "verify" => verify_cmd(rest),
        "trust" => trust(rest),
        "help" | "--help" | "-h" => Ok(format!("{USAGE}\n")),
        other => Err(format!("unknown bundle command {other:?}\n\n{USAGE}")),
    }
}

fn keygen(a: &[String]) -> Result<String, String> {
    let a = args(a, &["--name"], "")?;
    let [path] = a.positional::<1>("KEYFILE")?;
    let name = a.get("--name").unwrap_or("NAME");
    let (key, der) = SigningKey::generate()?;
    write_private(Path::new(path), &der, true)
        .map_err(|e| format!("{path}: {e} (keygen never overwrites a key)"))?;
    Ok(format!(
        "wrote {path} (mode 0600; keep it off boxes and out of repositories)\n\
         to trust it on a box: helmsman-plc bundle trust add {name} {}\n",
        sign::hex(&key.public())
    ))
}

fn manifest_from(a: &Args, base: Option<&BundleManifest>, cs: &ChangeSet) -> Result<BundleManifest, String> {
    let caps = capabilities(cs, &defaults())
        .map_err(|e| format!("the content is refused:\n  {}", e.join("\n  ")))?;
    let pick = |k: &str, old: Option<&str>| -> Result<String, String> {
        a.get(k)
            .or(old)
            .map(str::to_string)
            .ok_or_else(|| format!("{k} is required"))
    };
    let m = BundleManifest {
        name: pick("--name", base.map(|m| m.name.as_str()))?,
        version: pick("--version", base.map(|m| m.version.as_str()))?,
        author: pick("--author", base.map(|m| m.author.as_str()))?,
        description: a
            .get("--description")
            .map(str::to_string)
            .or_else(|| base.and_then(|m| m.description.clone())),
        caps,
    };
    BundleManifest::parse(&m.render()).map_err(|e| format!("the manifest is refused:\n  {}", e.join("\n  ")))
}

fn pack(a: &[String]) -> Result<String, String> {
    let a = args(
        a,
        &[
            "--io-map",
            "--calendar",
            "--name",
            "--version",
            "--author",
            "--description",
        ],
        "--module-dir",
    )?;
    let [out] = a.positional::<1>("OUT")?;
    let cs = ChangeSet::read(
        &a.multi,
        a.get("--io-map").map(Path::new),
        a.get("--calendar").map(Path::new),
    )?;
    let b = Bundle::new(manifest_from(&a, None, &cs)?, cs);
    fs::write(out, b.encode()).map_err(|e| format!("{out}: {e}"))?;
    Ok(format!(
        "{}wrote {out} (unsigned; sign it with `bundle sign`)\n",
        summary(&b)
    ))
}

fn sign_cmd(a: &[String]) -> Result<String, String> {
    let a = args(
        a,
        &["--key", "--name", "--version", "--author", "--description"],
        "",
    )?;
    let [input, out] = a.positional::<2>("IN OUT")?;
    let key = SigningKey::load(Path::new(a.need("--key")?))?;
    let mut b = Bundle::decode(&read_bundle(Path::new(input))?).map_err(|e| e.to_string())?;
    check_signatures(&b).map_err(|e| e.to_string())?;
    let rewrite = ["--name", "--version", "--author", "--description"]
        .iter()
        .any(|k| a.get(k).is_some());
    if rewrite {
        if !b.signatures.is_empty() {
            return Err(format!(
                "{input} is already signed {} time(s); changing its manifest would void those signatures",
                b.signatures.len()
            ));
        }
        b.manifest = manifest_from(&a, Some(&b.manifest), &b.content)?;
    }
    check_content(&b, &defaults()).map_err(|e| e.to_string())?;
    b.sign(&key);
    fs::write(out, b.encode()).map_err(|e| format!("{out}: {e}"))?;
    Ok(format!(
        "{}signed by {}\nwrote {out}\n",
        summary(&b),
        sign::hex(&key.public())
    ))
}

fn verify_cmd(a: &[String]) -> Result<String, String> {
    let a = args(a, &["--trust"], "")?;
    let [input] = a.positional::<1>("IN")?;
    let bytes = read_bundle(Path::new(input))?;
    let Some(trust_file) = a.get("--trust") else {
        let b = Bundle::decode(&bytes).map_err(|e| e.to_string())?;
        check_signatures(&b).map_err(|e| e.to_string())?;
        check_content(&b, &defaults()).map_err(|e| e.to_string())?;
        let mut s = summary(&b);
        if b.signatures.is_empty() {
            s.push_str("  unsigned\n");
        }
        for sig in &b.signatures {
            let _ = writeln!(s, "  signature by {} verifies", sign::hex(&sig.key));
        }
        s.push_str("ok (no trust store given: whether a box trusts the signer is not checked)\n");
        return Ok(s);
    };
    let path = Path::new(trust_file);
    let src = fs::read_to_string(path).map_err(|e| format!("{trust_file}: {e}"))?;
    let trust = TrustStore::parse(&src).map_err(|errs| {
        let lines: Vec<String> = errs
            .iter()
            .map(|(n, e)| format!("\n  {trust_file}:{n}: {e}"))
            .collect();
        format!("{trust_file} does not parse:{}", lines.concat())
    })?;
    let v = verify(&bytes, &trust, &defaults()).map_err(|e| e.to_string())?;
    let how = match &v.trust {
        Trust::Publisher { .. } => v.trust.describe(),
        Trust::Dev { .. } => format!("{} (that box allows dev bundles)", v.trust.describe()),
    };
    Ok(format!("{}ok: {how}\n", summary(&v.bundle)))
}

/// `--state-dir`, `$STATE_DIRECTORY`, or `./helmsman-state`, as the box.
fn state_dir(a: &Args) -> PathBuf {
    a.get("--state-dir").map_or_else(
        || {
            std::env::var_os("STATE_DIRECTORY")
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from("helmsman-state"), PathBuf::from)
        },
        PathBuf::from,
    )
}

fn trust(a: &[String]) -> Result<String, String> {
    let a = args(a, &["--state-dir"], "")?;
    let dir = state_dir(&a);
    let file = dir.join(TRUST_FILE);
    let mut t = TrustStore::load(&dir)?;
    let pos: Vec<&str> = a.pos.iter().map(String::as_str).collect();
    let said = match pos.as_slice() {
        ["list"] => {
            let mut s = String::new();
            for (n, k) in t.publishers() {
                let _ = writeln!(s, "publisher {n} ed25519 {}", sign::hex(k));
            }
            s.push_str(if t.dev_bundles {
                "dev-bundles allow\n"
            } else {
                "dev-bundles refuse\n"
            });
            return Ok(s);
        }
        ["add", name, key] => {
            let k =
                sign::parse_key(key).ok_or("KEY must be 64 lowercase hex digits (as keygen prints it)")?;
            t.add(name, k)?;
            format!("trusting {name}")
        }
        ["dev-bundles", v @ ("allow" | "refuse")] => {
            t.dev_bundles = *v == "allow";
            format!("dev-bundles {v}")
        }
        _ => {
            return Err(format!(
                "expected list, add NAME KEY, or dev-bundles allow|refuse\n\n{USAGE}"
            ));
        }
    };
    t.save(&dir).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(format!(
        "{said}: wrote {} (restart the box to load it)\n",
        file.display()
    ))
}

/// A file name for a bundle of snapshot `id`.
#[must_use]
pub fn file_name(id: &str) -> String {
    format!("snapshot-{}.{EXTENSION}", &id[..id.len().min(12)])
}
