//! `helmsman-canvas`: the browser app (docs/UI.md). An always-on header
//! over the Editor (a program's Structured Text and its live diagram, the
//! ST beside it on demand, the program tree, I/O, a trend and Claude) or
//! the Monitor (status tiles, phone to wall TV, watch only); and the
//! agent's proposals (diffs, sim replay, approve with a PIN) behind the ⋯
//! menu.
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
mod arrange;
#[cfg(test)]
mod arrange_tests;
mod box_projects;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_projects_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_tests;
mod box_trend;
mod box_view;
mod claude;
mod commit;
mod editor;
mod glyphs;
mod hotkeys;
mod monitor;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod monitor_tests;
mod playground;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod playground_tests;
mod playground_view;
mod projects;
mod remote;
mod review;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod review_tests;
mod review_view;
mod sim_projects;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod sim_projects_tests;
mod st_view;
#[cfg(test)]
mod starter_tests;
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
        // The browser keeps its keys, except the shortcuts of ours.
        let mut options = eframe::WebOptions::default();
        let base = std::mem::replace(&mut options.should_prevent_default, Box::new(|_| false));
        options.should_prevent_default = Box::new(move |e| base(e) || hotkeys::wants(e));
        let started = eframe::WebRunner::new()
            .start(canvas, options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
            .await;
        // The page's loading screen waits for one of these (loading/loading.js).
        match started {
            Ok(()) => announce("helmsman-started", None),
            Err(e) => {
                web_sys::console::error_1(&e);
                announce("helmsman-start-failed", Some(&e));
            }
        }
    });
}

/// Tells the page, which has no other way to know, how starting went.
#[cfg(target_arch = "wasm32")]
fn announce(name: &str, detail: Option<&eframe::wasm_bindgen::JsValue>) {
    let init = web_sys::CustomEventInit::new();
    if let Some(detail) = detail {
        init.set_detail(detail);
    }
    if let (Some(window), Ok(event)) = (
        web_sys::window(),
        web_sys::CustomEvent::new_with_event_init_dict(name, &init),
    ) {
        let _ = window.dispatch_event(&event);
    }
}
