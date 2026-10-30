//! The API token: 32 random bytes as hex, in `api-token` in the state
//! directory with mode 0600. Made on first start and printed once (to the
//! journal under systemd); later starts read it and say only where it is.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// The token file's name in the state directory.
pub const FILE: &str = "api-token";

/// Hex digits in a token.
pub const LEN: usize = 64;

fn random() -> io::Result<String> {
    let mut b = [0u8; LEN / 2];
    fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().fold(String::with_capacity(LEN), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    }))
}

fn valid(t: &str) -> bool {
    t.len() == LEN && t.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(unix)]
fn private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn private(_: &Path) -> io::Result<()> {
    Ok(())
}

fn create(path: &Path, token: &str) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(token.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()
}

/// A token and where it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub token: String,
    pub path: PathBuf,
    /// Made by this call (print it once), or read from an earlier start.
    pub new: bool,
}

/// Reads the token from `dir`, or makes one. A file another user could
/// read is made private again; one that does not hold a token is replaced.
///
/// # Errors
/// The directory cannot be created, or the file cannot be read or written.
pub fn load_or_create(dir: &Path) -> io::Result<Token> {
    fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    for _ in 0..2 {
        match fs::read_to_string(&path) {
            Ok(t) if valid(t.trim()) => {
                private(&path)?;
                return Ok(Token {
                    token: t.trim().into(),
                    path,
                    new: false,
                });
            }
            Ok(_) => fs::remove_file(&path)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let token = random()?;
        match create(&path, &token) {
            Ok(()) => {
                return Ok(Token {
                    token,
                    path,
                    new: true,
                });
            }
            // Another process made it between our read and create: read it.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "{}: could not settle on a token",
        path.display()
    )))
}

/// Compares in time independent of where the strings differ.
#[must_use]
pub fn eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
