//! Random bytes from the operating system, for tokens, salts and session
//! ids. Unix reads `/dev/urandom`; Windows has no such file and uses
//! `ring`'s system generator (`BCryptGenRandom`), a dependency the box already
//! has for its signatures and TLS.

use std::io;

/// Fills `buf` from the OS.
///
/// # Errors
/// The OS random source cannot be read.
pub fn fill(buf: &mut [u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Read as _;
        std::fs::File::open("/dev/urandom")?.read_exact(buf)
    }
    #[cfg(not(unix))]
    {
        use ring::rand::SecureRandom as _;
        ring::rand::SystemRandom::new()
            .fill(buf)
            .map_err(|_| io::Error::other("the OS random source failed"))
    }
}
