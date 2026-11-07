//! Card build-match: the canvas and the box it watches say when they are
//! different builds. The box serves the canvas from `--ui DIR`, built
//! separately, so after an update a page left open (or a stale `--ui`)
//! can be an older editor talking to a newer controller. Both carry the
//! stamp `crates/build_stamp.rs` bakes in; the box sends its own on every
//! reply ([`HEADER`]), and the header shows a banner when they differ.

/// This canvas's build stamp (`git describe` when it was built, or
/// [`UNKNOWN`]).
pub const CANVAS: &str = env!("HELMSMAN_BUILD");

/// What a build without git or `HELMSMAN_BUILD` is stamped.
pub const UNKNOWN: &str = "unknown";

/// The response header the box's stamp comes in.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const HEADER: &str = "X-Helmsman-Build";

/// Whether the box's stamp says it is a different build from the canvas.
/// Nothing to compare is not a mismatch: no stamp yet (or a box from
/// before the header), or [`UNKNOWN`] on either side, shows no banner, so
/// a build from a tarball never cries wolf.
#[must_use]
pub fn differs(canvas: &str, controller: Option<&str>) -> bool {
    let known = |s: &str| !s.is_empty() && s != UNKNOWN;
    controller.is_some_and(|c| known(c) && known(canvas) && c != canvas)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{CANVAS, UNKNOWN, differs};

    #[test]
    fn only_two_known_different_stamps_differ() {
        assert!(differs("abc123", Some("def456")));
        assert!(!differs("abc123", Some("abc123")), "the same build");
        assert!(!differs("abc123", None), "no stamp heard yet");
        assert!(!differs("abc123", Some(UNKNOWN)), "a box built without git");
        assert!(!differs(UNKNOWN, Some("abc123")), "a canvas built without git");
        assert!(!differs(UNKNOWN, Some(UNKNOWN)));
        assert!(!differs("abc123", Some("")), "an empty header");
    }

    #[test]
    fn the_canvas_and_the_box_of_one_checkout_carry_one_stamp() {
        assert_ne!(CANVAS, "");
        assert_eq!(CANVAS, helmsman_plc::BUILD, "one build script, one checkout");
    }
}
