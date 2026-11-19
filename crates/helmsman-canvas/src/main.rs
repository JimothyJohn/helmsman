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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod agent_ladder_tests;
mod agent_marks;
mod app;
mod arrange;
#[cfg(test)]
mod arrange_tests;
mod ask;
#[cfg(test)]
mod ask_tests;
mod assign;
mod assign_ui;
mod box_agent;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_agent_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_design_tests;
mod box_popup;
mod box_projects;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_projects_tests;
mod box_spec;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod box_tests;
mod box_trend;
mod box_view;
mod chat_about;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod chat_about_tests;
mod claude;
mod commit;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod debugger_tests;
mod design;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod design_page_tests;
mod design_step;
mod design_ui;
mod desk;
mod device_config;
mod device_form;
mod device_form_ui;
mod device_io;
mod device_proposal;
mod device_vendor;
mod devices;
mod diagram_menu;
mod diff_ui;
mod editor;
mod edits;
#[cfg(test)]
mod edits_tests;
mod find;
mod front;
#[cfg(test)]
mod front_tests;
mod glyph_lib;
mod glyphs;
mod help;
mod history_ui;
mod hotkeys;
mod icons;
mod instruct;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod instruct_tests;
mod io_cards;
mod io_names;
mod jump;
mod lamps;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod link_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod lint_shipped_tests;
mod md_view;
mod messages;
mod monitor;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod monitor_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod node_api_tests;
mod node_token;
mod pane_layout;
#[cfg(test)]
mod pane_layout_tests;
mod plant;
mod playground;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod playground_tests;
mod playground_view;
mod projects;
mod remote;
mod requirements;
mod review;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod review_tests;
mod review_view;
mod scope;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod scope_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod sim_plant_tests;
mod sim_popup;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod sim_popup_tests;
mod sim_projects;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod sim_projects_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod slow_link_tests;
mod spec_mode;
mod st_fold;
mod st_hints;
mod st_syntax;
mod st_view;
mod stamp;
mod start_state;
#[cfg(test)]
mod start_state_tests;
mod steps;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod steps_tests;
#[cfg(test)]
mod test_programs;
mod theme;
mod tips;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tips_tests;
mod tour;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tour_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod ui_tests;
mod verify;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod verify_tests;
mod watch;
#[cfg(target_arch = "wasm32")]
mod web;
mod wired;

/// `helmsman-canvas --help-tables keyboard|starters`: the help site's
/// generated pages, as Markdown on stdout (scripts/build-help.sh).
#[cfg(not(target_arch = "wasm32"))]
fn help_tables(which: &str) -> Option<String> {
    use std::fmt::Write as _;
    match which {
        "keyboard" => Some(hotkeys::markdown()),
        "starters" => {
            let mut out = String::from(
                "# Examples\n\n\
                 Short programs that teach one thing each, the first chapters of a \
                 Structured Text book. Pick one and name it: it becomes a program of \
                 your own to change.\n\n",
            );
            for e in &playground::EXAMPLES {
                let _ = writeln!(out, "- **{}**", e.name);
            }
            Some(out)
        }
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--help-tables") {
        match args.get(1).and_then(|w| help_tables(w)) {
            Some(md) => print!("{md}"),
            None => eprintln!("usage: helmsman-canvas --help-tables keyboard|starters"),
        }
        return Ok(());
    }
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
