//! `tcpethernet-canvas`: a node canvas for building PLC logic out of inputs,
//! outputs, timers, counters and latches, grouped into function blocks that
//! nest without limit. Simulates in the page; not wired to a PLC yet.
//!
//! Native: `cargo run -p tcpethernet-canvas`. Web: `make canvas-serve`.

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
mod canvas;
mod example;
mod look;
#[cfg(test)]
mod ui_tests;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 860.0])
            .with_title("tcpethernet canvas"),
        ..Default::default()
    };
    eframe::run_native(
        "tcpethernet-canvas",
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
