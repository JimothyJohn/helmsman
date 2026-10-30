//! A header value that never prints.

use std::fmt;

/// A header value loaded from a file, the environment or the sinks file.
/// `Debug` prints `<redacted>` and there is no `Display`, so a value cannot
/// reach a log line or an error message by accident; [`Secret::expose`] is
/// the one way out, used only to write the request.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(s: String) -> Self {
        Self(s)
    }

    /// The value, for the wire only.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
