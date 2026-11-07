//! Bakes the build stamp into the box (`HELMSMAN_BUILD`, sent on every API
//! reply as `X-Helmsman-Build`); `crates/build_stamp.rs` says how.

include!("../build_stamp.rs");
