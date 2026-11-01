//! Where the diagram's blocks sit, kept (card `arrangement-persist`: "both:
//! shared default + personal tweaks").
//!
//! - **The shared default** is the project's `diagram.layout`
//!   ([`helmsman_project::layout`]): an engineer arranges the diagram and
//!   saves it as the default, and everyone who opens it starts there. On a
//!   box it lives in the running project's folder (so it travels in the
//!   project's `.zip`); in the simulation, in the browser-stored project.
//! - **Personal tweaks** are anyone's further moves, kept in this
//!   browser's storage ([`Layers`], every access wrapped) as offsets *on
//!   top of* the shared default, so a new default from the engineer still
//!   reaches a screen that has tweaks, with the tweaks on it.
//! - **Reset layout** drops the tweaks (back to the shared default); a
//!   second step goes to the automatic layout (kept as a tweak too, so it
//!   survives a reload).
//!
//! Blocks are named by POU, label, text and which of the same
//! ([`Arrangement`]), so an edit that renumbers them keeps both layers. A
//! POU's diagram is the same in every module that uses it, so one
//! arrangement per POU serves them all.

use eframe::egui;
use serde_json::{Value as Json, json};

use crate::editor::Chrome;
use crate::sim_projects::Storage;
use crate::st_view::{Arrangement, Viewer};

/// The format the tweaks are stored in.
const FORMAT: &str = "helmsman-arrangements 1";
/// Most views (projects) whose tweaks one browser keeps; the least
/// recently changed goes first.
pub const MAX_VIEWS: usize = 32;
/// Longest view key kept.
const MAX_KEY: usize = 200;

/// The two layers of one diagram, and the browser's store of tweaks.
pub struct Layers {
    store: Box<dyn Storage>,
    /// Every view's tweaks as `diagram.layout` text, most recent last.
    tweaks: Vec<(String, String)>,
    /// Why this browser does not keep tweaks, if it does not.
    pub problem: Option<String>,
    /// The view shown: whose tweaks apply.
    key: Option<String>,
    /// The shared default as last applied, and what it reads as.
    shared_text: String,
    shared: Arrangement,
    /// The shared default's moves of the blocks the diagram has.
    applied: Arrangement,
    /// The viewer's edit count after the last apply or track.
    seen: u64,
}

impl std::fmt::Debug for Layers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layers")
            .field("key", &self.key)
            .field("views", &self.tweaks.len())
            .field("shared", &self.shared.len())
            .finish_non_exhaustive()
    }
}

fn parse(text: &str) -> Option<Vec<(String, String)>> {
    let j: Json = serde_json::from_str(text).ok()?;
    if j["format"] != FORMAT {
        return None;
    }
    Some(
        j["views"]
            .as_array()?
            .iter()
            .filter_map(|v| {
                let key = v[0].as_str()?;
                let text = v[1].as_str()?;
                (key.len() <= MAX_KEY && Arrangement::from_text(text).is_ok())
                    .then(|| (key.to_string(), text.to_string()))
            })
            .take(MAX_VIEWS)
            .collect(),
    )
}

impl Layers {
    /// The tweaks `store` holds. A store that fails or holds something
    /// unreadable is noted, and the tweaks last until the page closes.
    pub fn load(store: Box<dyn Storage>) -> Self {
        let mut s = Self {
            store,
            tweaks: Vec::new(),
            problem: None,
            key: None,
            shared_text: String::new(),
            shared: Arrangement::default(),
            applied: Arrangement::default(),
            seen: 0,
        };
        match s.store.load() {
            Ok(Some(text)) => match parse(&text) {
                Some(t) => s.tweaks = t,
                None => {
                    s.problem = Some("the stored diagram layouts are not readable; starting afresh".into());
                }
            },
            Ok(None) => {}
            Err(e) => {
                s.problem = Some(format!(
                    "this browser does not keep diagram layouts ({e}): moves last until the page closes"
                ));
            }
        }
        s
    }

    /// The tweaks of view `key`.
    fn personal(&self, key: &str) -> Arrangement {
        self.tweaks
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, t)| Arrangement::from_text(t).ok())
            .unwrap_or_default()
    }

    /// The shown view's tweaks.
    #[must_use]
    pub fn tweaks(&self) -> Arrangement {
        self.key.as_deref().map(|k| self.personal(k)).unwrap_or_default()
    }

    /// The shown view has tweaks on its default.
    #[must_use]
    pub fn tweaked(&self) -> bool {
        !self.tweaks().is_empty()
    }

    /// The shown view has a shared default.
    #[must_use]
    pub fn has_default(&self) -> bool {
        !self.shared.is_empty()
    }

    /// Keeps `a` as view `key`'s tweaks, and writes them out.
    fn put(&mut self, key: &str, a: &Arrangement) {
        self.tweaks.retain(|(k, _)| k != key);
        if !a.is_empty() && key.len() <= MAX_KEY && a.len() <= helmsman_project::layout::MAX_MOVES {
            self.tweaks.push((key.to_string(), a.to_text()));
        }
        while self.tweaks.len() > MAX_VIEWS {
            self.tweaks.remove(0);
        }
        let text = json!({ "format": FORMAT, "views": self.tweaks }).to_string();
        if let Err(e) = self.store.save(&text)
            && self.problem.is_none()
        {
            self.problem = Some(format!(
                "this browser does not keep diagram layouts ({e}): moves last until the page closes"
            ));
        }
    }

    /// Lays `v` out as view `key` with shared default `shared`, from the
    /// automatic layout: the default, then the tweaks on top.
    fn apply(&mut self, v: &mut Viewer, key: &str, shared: &str) {
        shared.clone_into(&mut self.shared_text);
        self.shared = Arrangement::from_text(shared).unwrap_or_default();
        self.key = Some(key.to_string());
        v.reset_arrangement();
        v.arrange(&self.shared);
        self.applied = self.shared.within(v);
        let mine = self.personal(key);
        v.arrange(&mine);
        self.seen = v.edits();
    }

    /// Every frame, before drawing: `v` (`fresh`: just built) shown as
    /// view `key`, whose shared default is `shared` (`""`: none). A new
    /// diagram, view or default is laid out again; otherwise a finished
    /// move is kept as a tweak.
    pub fn show(&mut self, v: &mut Viewer, key: &str, shared: &str, fresh: bool) {
        if fresh || self.key.as_deref() != Some(key) || self.shared_text != shared {
            self.apply(v, key, shared);
        } else {
            self.track(v);
        }
    }

    /// Keeps what the operator moved since last time as the view's tweaks
    /// (once a drag is over).
    pub fn track(&mut self, v: &Viewer) {
        if v.dragging() || v.edits() == self.seen {
            return;
        }
        let Some(key) = self.key.clone() else { return };
        let mine = v.arrangement().minus(&self.applied);
        self.put(&key, &mine);
        self.seen = v.edits();
    }

    /// Reset layout: tweaks gone (the shared default); with none, the
    /// automatic layout (kept, so a reload shows it too).
    pub fn reset(&mut self, v: &mut Viewer) {
        let Some(key) = self.key.clone() else { return };
        v.reset_arrangement();
        let mine = if self.tweaked() {
            v.arrange(&self.shared);
            Arrangement::default()
        } else {
            Arrangement::default().minus(&self.applied)
        };
        self.put(&key, &mine);
        self.seen = v.edits();
    }

    /// `text` is now the shared default because this page saved it from
    /// `v` as it stands: nothing moves, and the tweaks are what `v` has
    /// beyond it (none, unless the operator moved on meanwhile).
    pub fn adopt(&mut self, v: &Viewer, text: &str) {
        let Some(key) = self.key.clone() else { return };
        text.clone_into(&mut self.shared_text);
        self.shared = Arrangement::from_text(text).unwrap_or_default();
        self.applied = self.shared.within(v);
        let mine = v.arrangement().minus(&self.applied);
        self.put(&key, &mine);
        self.seen = v.edits();
    }
}

/// The toolbar's layout buttons: Reset layout (in two steps) and, where
/// the page may (`can`), Save as default. Whether Save was clicked.
pub fn buttons(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    layers: &mut Layers,
    view: Option<&mut Viewer>,
    can: Result<(), String>,
) -> bool {
    let Some(v) = view else {
        ui.add_enabled(false, egui::Button::new("Reset layout"));
        return false;
    };
    let (label, tip, on) = match (layers.tweaked(), layers.has_default()) {
        (true, true) => (
            "Reset to default",
            "back to the project's shared layout; your own moves here are dropped",
            true,
        ),
        (false, true) => (
            "Automatic layout",
            "from the shared layout to the automatic one (only on this browser)",
            v.rearranged(),
        ),
        (true, false) => ("Reset layout", "back to the automatic layout", true),
        (false, false) => (
            "Reset layout",
            "drag blocks, or an IF/CASE/loop by its label, to rearrange the diagram",
            false,
        ),
    };
    let r = ui.add_enabled(on, egui::Button::new(label)).on_hover_text(tip);
    chrome.hit("reset layout", r.rect);
    if r.clicked() {
        layers.reset(v);
    }
    let b = egui::Button::new("Save as default");
    let r = match can {
        Ok(()) => ui
            .add_enabled(v.rearranged() || layers.has_default(), b)
            .on_hover_text("everyone who opens this project starts from this arrangement"),
        Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
    };
    chrome.hit("save layout", r.rect);
    r.clicked()
}
