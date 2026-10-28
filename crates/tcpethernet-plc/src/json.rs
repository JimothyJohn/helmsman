//! Minimal JSON writer; the API only ever emits JSON, never parses it.

use std::fmt::Write as _;

#[must_use]
pub fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[must_use]
pub fn bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() * 4 + 2);
    out.push('[');
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{x}");
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_everything_that_breaks_json_or_script() {
        let expected = concat!("\"a\\\"b\\\\c\\nd\\u0001e", "\\u2028\"");
        assert_eq!(super::string("a\"b\\c\nd\u{1}e\u{2028}"), expected);
        assert_eq!(super::bytes(&[1, 255]), "[1,255]");
    }
}
