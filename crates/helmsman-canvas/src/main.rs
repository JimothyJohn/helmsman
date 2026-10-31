//! `helmsman-canvas`: the operator's live view of a module's Structured
//! Text, drawn as a diagram with a traced scan on it every frame (opens on
//! it), the review screen for the agent's proposals (diffs, sim replay,
//! approve with a PIN), and a lab node editor for building logic out of
//! inputs, outputs, timers, counters and latches, grouped into nested
//! function blocks.
//!
//! Native: `cargo run -p helmsman-canvas` (the demo). Web: built by
//! `make canvas-web` and served by the box (`helmsman-plc --ui`), it
//! watches that box over its API; `?demo` (or `make canvas-serve`, with
//! no box behind it) runs the demo in the page instead, `?andon` is
//! the box's floor screen, full screen, and `?site` lists every box of its
//! site (`--site`), each a link to its own canvas.

// Screen geometry is f32 throughout; counts and sim values convert into it.
// Drawing code reads best with short geometry names (r, z, p, x, y).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names
)]

mod andon;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod andon_tests;
mod app;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_tests;
mod box_view;
mod canvas;
mod example;
mod look;
mod remote;
mod review;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod review_tests;
mod review_view;
mod site;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod site_tests;
mod st_view;
#[cfg(test)]
mod ui_tests;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 860.0])
            .with_title("Helmsman canvas"),
        ..Default::default()
    };
    eframe::run_native(
        "helmsman-canvas",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("index.html has a <canvas id=\"canvas\">");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
            )
            .await;
        if let Err(e) = started {
            web_sys::console::error_1(&e);
        }
    });
}
