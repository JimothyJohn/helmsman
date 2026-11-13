//! The three values the console's `07.htm` script computes before it
//! submits a password change (md5.js and its `Xor()`, as described in the
//! devices write-up). The vendor's own outputs, computed by running that
//! script under node, are the fixtures below.
//!
//! - `ChallID` = hex MD5 of the `ChallID` cookie's string.
//! - `OldPwd` = hex MD5 of the old password.
//! - `NewPwd` = the 16 bytes of MD5(cookie string), the first `len`
//!   combined byte by byte (XOR) with the new password's ASCII codes, as hex.

use super::Error;
use crate::md5::{hex, md5};

/// Longest password the page's `Xor()` can carry.
pub const MAX_PASSWORD: usize = 16;

/// Printable ASCII (0x20..=0x7e), at most [`MAX_PASSWORD`] bytes.
///
/// # Errors
/// [`Error::Refused`], without echoing the password.
pub fn check(password: &str) -> Result<(), Error> {
    if password.len() > MAX_PASSWORD {
        return Err(Error::Refused(format!(
            "a password is at most {MAX_PASSWORD} characters"
        )));
    }
    if !password.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return Err(Error::Refused(
            "a password is printable ASCII (space to ~)".into(),
        ));
    }
    Ok(())
}

/// `ChallID` field: hex MD5 of the cookie string.
#[must_use]
pub fn chall_id(cookie: &str) -> String {
    hex(&md5(cookie.as_bytes()))
}

/// `OldPwd` field: hex MD5 of the old password.
#[must_use]
pub fn old_pwd(password: &str) -> String {
    hex(&md5(password.as_bytes()))
}

/// `NewPwd` field. The caller has run [`check`].
#[must_use]
pub fn new_pwd(cookie: &str, password: &str) -> String {
    let mut key = md5(cookie.as_bytes());
    for (k, p) in key.iter_mut().zip(password.bytes()) {
        *k ^= p;
    }
    hex(&key)
}

/// What a console does with `NewPwd`: undo the XOR with the stored
/// challenge. Unchanged bytes XOR to 0, and a printable character is never
/// 0, so the length is the last nonzero position. `None` for anything that
/// is not 32 hex characters or does not decode to printable ASCII.
#[cfg(any(test, feature = "sim"))]
#[must_use]
pub fn unmask(cookie: &str, new_pwd: &str) -> Option<String> {
    if new_pwd.len() != 32 || !new_pwd.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let key = md5(cookie.as_bytes());
    let mut out = Vec::new();
    for (i, k) in key.iter().enumerate() {
        let b = u8::from_str_radix(&new_pwd[2 * i..2 * i + 2], 16).ok()?;
        out.push(b ^ k);
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    if out.iter().all(|b| (0x20..=0x7e).contains(b)) {
        String::from_utf8(out).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (challenge, password, md5(challenge), md5(password), newpwd): the
    /// vendor's own `md5.js` and `Xor()` run under node.
    const VENDOR: [(&str, &str, &str, &str, &str); 6] = [
        (
            "",
            "moxa",
            "d41d8cd98f00b204e9800998ecf8427e",
            "d3bff8450106a52369b8b4dbca7b1b16",
            "b972f4b88f00b204e9800998ecf8427e",
        ),
        (
            "4739",
            "",
            "f3507289cfdc8c9ae93f4098111a13f9",
            "d41d8cd98f00b204e9800998ecf8427e",
            "f3507289cfdc8c9ae93f4098111a13f9",
        ),
        (
            "4739",
            "moxa",
            "f3507289cfdc8c9ae93f4098111a13f9",
            "d3bff8450106a52369b8b4dbca7b1b16",
            "9e3f0ae8cfdc8c9ae93f4098111a13f9",
        ),
        (
            "12345",
            "Abc!~ 09",
            "827ccb0eea8a706c4c34a16891f84e7b",
            "54832070cac8fae13e9debcb61ec8ecf",
            "c31ea82f94aa40554c34a16891f84e7b",
        ),
        (
            "65535",
            "sixteen-chars-xx",
            "3c9b5f1cd6a030bcb7bed9eac80589fb",
            "db5dbdd94d28c99f28055567d9500bd9",
            "4ff22768b3c55e91d4d6b898bb28f183",
        ),
        (
            "0",
            "0123456789",
            "cfcd208495d565ef66e7dff9f98764da",
            "781e5e245d69b566979b86e28d23f2c7",
            "fffc12b7a1e053d85ededff9f98764da",
        ),
    ];

    #[test]
    fn vendor_vectors() {
        for (chall, pw, want_chall, want_old, want_new) in VENDOR {
            assert_eq!(chall_id(chall), want_chall, "ChallID({chall:?})");
            assert_eq!(old_pwd(pw), want_old, "OldPwd({pw:?})");
            assert_eq!(new_pwd(chall, pw), want_new, "NewPwd({chall:?}, {pw:?})");
        }
    }

    #[test]
    fn unmask_inverts_every_vendor_vector() {
        for (chall, pw, _, _, new) in VENDOR {
            assert_eq!(unmask(chall, new).as_deref(), Some(pw), "({chall:?}, {pw:?})");
        }
    }

    #[test]
    fn password_rules() {
        assert!(check("").is_ok());
        assert!(check("sixteen-chars-xx").is_ok());
        assert!(check("seventeen-chars-x").is_err());
        assert!(check("tab\there").is_err());
        assert!(check("caf\u{e9}").is_err());
        assert!(check("nul\0").is_err());
        let e = check("secret\u{e9}").unwrap_err().to_string();
        assert!(!e.contains("secret"), "the error echoed the password: {e}");
    }

    #[test]
    fn unmask_refuses_what_a_console_would_not_decode() {
        assert_eq!(unmask("1", "zz"), None);
        assert_eq!(unmask("1", &"g".repeat(32)), None);
        assert_eq!(
            unmask("1", &"A".repeat(32)),
            None,
            "uppercase hex is not what the page sends"
        );
    }
}
