//! `helmsman-canvas`: the browser app (docs/UI.md). An always-on header
//! over the Editor: a program's Structured Text and its live diagram, the
//! ST beside it on demand, the program tree, I/O, a trend and Claude; and
//! the agent's proposals (diffs, sim replay, approve with a PIN) behind
//! the ⋯ menu.
//!
//! Where the program runs is a setting, not a mode. Served by a box
//! (`make canvas-web`, `helmsman-plc --ui`), the page watches that box
//! over its API. Opened from the public site (`make playground`, into
//! docs/playground/), or with the old `?playground` and `?demo` links, or
//! natively (`cargo run -p helmsman-canvas`), it runs a simulated box in
//! the page against virtual I/O, with Claude on the visitor's own key.

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

mod app;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_tests;
mod box_view;
mod claude;
mod editor;
mod monitor;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod monitor_tests;
mod playground;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod playground_tests;
mod playground_view;
mod remote;
mod review;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod review_tests;
mod review_view;
mod st_view;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod ui_tests;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 860.0])
            .with_title("Helmsman"),
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
