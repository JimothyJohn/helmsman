//! A bounded server-sent-events reader (the WHATWG event-stream format):
//! `event:` and `data:` fields, `:` comments, events ended by a blank
//! line, `\n`, `\r\n` or `\r` line ends. Lines and events are capped, so
//! a hostile or broken stream costs at most [`MAX_EVENT`] bytes.

use std::fmt;
use std::io::BufRead;

/// Longest line read.
pub const MAX_LINE: usize = 1 << 20;
/// Most `data` bytes in one event.
pub const MAX_EVENT: usize = 2 << 20;

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The `event:` field; `message` when none was given.
    pub name: String,
    /// The `data:` lines, joined with `\n`.
    pub data: String,
}

/// Why the stream cannot be read further.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A read failed (the transport's error, as text).
    Read(crate::http::Error),
    TooLong,
    NotUtf8,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) => write!(f, "{e}"),
            Self::TooLong => f.write_str("an event-stream line or event is too long"),
            Self::NotUtf8 => f.write_str("the event stream is not UTF-8"),
        }
    }
}

/// Reads events from a byte stream.
#[derive(Debug, Default)]
pub struct Reader {
    /// A `\r` ended the last line; skip a `\n` right after it.
    after_cr: bool,
}

impl Reader {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One line without its end; `None` at end of stream (a last line
    /// without an end is dropped, as the format says).
    fn line(&mut self, r: &mut impl BufRead) -> Result<Option<Vec<u8>>, Error> {
        let mut line = Vec::new();
        loop {
            let buf = r.fill_buf().map_err(|e| Error::Read(crate::http::io_error(&e)))?;
            if buf.is_empty() {
                return Ok(None);
            }
            let mut start = 0;
            if self.after_cr {
                self.after_cr = false;
                if buf[0] == b'\n' {
                    start = 1;
                }
            }
            if let Some(k) = buf[start..].iter().position(|&b| b == b'\n' || b == b'\r') {
                let end = start + k;
                line.extend_from_slice(&buf[start..end]);
                self.after_cr = buf[end] == b'\r';
                r.consume(end + 1);
                if line.len() > MAX_LINE {
                    return Err(Error::TooLong);
                }
                return Ok(Some(line));
            }
            let n = buf.len();
            line.extend_from_slice(&buf[start..]);
            r.consume(n);
            if line.len() > MAX_LINE {
                return Err(Error::TooLong);
            }
        }
    }

    /// The next event, or `None` when the stream ends. An event cut off
    /// by the end of the stream is not dispatched.
    ///
    /// # Errors
    /// [`Error`]: the stream is unusable from here on.
    pub fn next(&mut self, r: &mut impl BufRead) -> Result<Option<Event>, Error> {
        let mut name: Option<String> = None;
        let mut data: Option<Vec<String>> = None;
        let mut size = 0usize;
        loop {
            let Some(line) = self.line(r)? else {
                return Ok(None);
            };
            let line = String::from_utf8(line).map_err(|_| Error::NotUtf8)?;
            if line.is_empty() {
                if let Some(lines) = data.take() {
                    return Ok(Some(Event {
                        name: name
                            .take()
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| "message".into()),
                        data: lines.join("\n"),
                    }));
                }
                name = None;
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line.as_str(), ""),
            };
            match field {
                "event" => name = Some(value.to_string()),
                "data" => {
                    size += value.len() + 1;
                    if size > MAX_EVENT {
                        return Err(Error::TooLong);
                    }
                    data.get_or_insert_with(Vec::new).push(value.to_string());
                }
                _ => {}
            }
        }
    }
}
