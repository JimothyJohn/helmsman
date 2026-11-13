//! The token a page sends the node (or box) it watches by address
//! (`?box=ORIGIN`, card canvas-node-token). A node guards its writes with
//! the bearer token from its settings record, and a box every call; a
//! page served from elsewhere has no cookie there, so the person types
//! the token once into the sign-in panel and this browser keeps it for
//! that origin (`localStorage`, never the URL). The transport
//! (`crate::web`) then sends it as `Authorization: Bearer` on every call
//! to that origin, and the page counts as an engineer there, so Deploy,
//! Save, Stop and Run work from the UI instead of a curl.

use crate::review::Session;

/// The `localStorage` slot the token for `origin` lives in, one per
/// origin so two nodes on one laptop keep their own. Only the browser
/// build has a store to use it on.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[must_use]
pub fn slot(origin: &str) -> String {
    format!("helmsman-canvas/box-token/{}", origin.trim_end_matches('/'))
}

/// What a person typed, as a token: trimmed, one line, nothing a header
/// cannot carry, not absurdly long. `None` is "not a token" (said in the
/// panel), never sent.
#[must_use]
pub fn clean(input: &str) -> Option<String> {
    let t = input.trim();
    let ok = !t.is_empty() && t.len() <= 256 && t.bytes().all(|b| b.is_ascii_graphic());
    ok.then(|| t.to_string())
}

/// The header every call to the origin carries (the browser build sends
/// it; the native build only tests it).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[must_use]
pub fn header(token: &str) -> (&'static str, String) {
    ("Authorization", format!("Bearer {token}"))
}

/// Who this page is, at an origin it holds a token for: the node knows
/// no people, only its token, which is the engineer's.
#[must_use]
pub fn session() -> Session {
    Session {
        user: Some("token".into()),
        role: "engineer".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_one_clean_line_and_the_slot_is_per_origin() {
        assert_eq!(clean("  bench-token-2026 \n"), Some("bench-token-2026".into()));
        assert_eq!(clean(""), None);
        assert_eq!(clean("   "), None);
        assert_eq!(clean("two words"), None);
        assert_eq!(clean("tab\tbed"), None);
        assert_eq!(clean("crlf\r\ninjected: 1"), None);
        assert_eq!(clean(&"x".repeat(257)), None);
        assert_eq!(clean("é"), None, "ASCII only: a header carries nothing else");
        assert_eq!(
            slot("http://192.168.3.55:8080/"),
            "helmsman-canvas/box-token/http://192.168.3.55:8080"
        );
        assert_ne!(slot("http://a:8080"), slot("http://b:8080"));
        assert_eq!(header("tok"), ("Authorization", "Bearer tok".into()));
        assert_eq!(session().label(), "token (engineer)");
    }
}
