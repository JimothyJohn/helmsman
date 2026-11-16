//! Spec mode on a box's page (Nick, 2026-11-15: "in both" modes; the
//! simulation's half is `crate::spec_mode`). The same window and review
//! ([`crate::spec_mode::form_ui`], [`crate::spec_mode::review_ui`]) and the
//! same checks (`crate::instruct`, `crate::verify`), with the box's ends:
//!
//! - **Documents.** Text and the pasted text go in the message to the
//!   box's agent (`POST /api/chat`). A PDF is kept on the box first as a
//!   drop-in (`POST /api/dropins`, [`crate::device_vendor::upload_request`])
//!   and the message names its id; the box puts the PDF in the message
//!   itself. The Agent is asked only once every PDF is kept.
//! - **The run.** [`crate::instruct::BOX_NEW_RULES`] then the
//!   specification's rules: a new project, with or without a project open.
//!   It is checked against the image sizes and scan period the box reports
//!   for a project made now (`GET /api/projects` `tests`).
//! - **Accept.** The project is one zip in one request
//!   (`POST /api/projects/{name}/zip`: the one write of many files that is
//!   atomic). A taken name is a 409 and nothing is written. The box
//!   records the revision as an upload; the route carries no label.
//! - **Inputs.** A number input has no line in `io.map` (the box refuses a
//!   screen tile as an input), so the review lists it as needing a device.
//!
//! State only; `box_view.rs` carries the requests.

use crate::box_projects::{PCall, PReq};
use crate::instruct::Apply;
use crate::spec_mode::{Doc, Draft, PdfRecord, Review, Spec};

/// What the window and the run have, on a box's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSpec {
    pub spec: Spec,
    /// The draft on review.
    pub review: Option<Review>,
    /// The checked answer the draft was read from, until Accept or Discard.
    pub apply: Option<Apply>,
    /// The documents of the run under way, for its draft.
    docs: Option<(Vec<Doc>, Vec<PdfRecord>)>,
    /// The PDFs are going to the box.
    pub uploading: bool,
    /// The project is going to the box.
    pub accepting: bool,
}

impl Default for BoxSpec {
    fn default() -> Self {
        Self::new()
    }
}

/// A project name that is free among `taken`, starting from `want`.
fn free_name(want: &str, taken: &[String]) -> String {
    let base: String = want.chars().take(44).collect();
    let is_taken = |n: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if !is_taken(&base) {
        return base;
    }
    (2..=400)
        .map(|k| format!("{base}-{k}"))
        .find(|n| !is_taken(n))
        .unwrap_or(base)
}

impl BoxSpec {
    #[must_use]
    pub fn new() -> Self {
        Self {
            spec: Spec {
                on_box: true,
                ..Spec::default()
            },
            review: None,
            apply: None,
            docs: None,
            uploading: false,
            accepting: false,
        }
    }

    /// The drop-in ids of the PDFs, in the order added.
    #[must_use]
    pub fn dropins(&self) -> Vec<u64> {
        self.spec.pdfs.iter().filter_map(|p| p.id).collect()
    }

    /// The requests that keep the PDFs not kept yet on the box, each
    /// marked as on its way.
    pub fn upload_requests(&mut self) -> Vec<PReq> {
        let mut out = Vec::new();
        for p in &mut self.spec.pdfs {
            if p.id.is_some() || p.sending {
                continue;
            }
            p.refused = None;
            p.sending = true;
            out.push(crate::device_vendor::upload_request(
                PCall::SpecPdf(p.name.clone()),
                &p.name,
                p.bytes.clone(),
                crate::device_vendor::File::Pdf,
            ));
        }
        self.uploading = !out.is_empty();
        out
    }

    /// The box's answer to the upload of PDF `name`. `Some(ready)` once
    /// every upload is answered: `ready` when every PDF is kept.
    pub fn pdf_answer(&mut self, name: &str, status: u16, body: &str) -> Option<bool> {
        let p = self.spec.pdfs.iter_mut().find(|p| p.name == name && p.sending)?;
        p.sending = false;
        let id = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|j| j["id"].as_u64())
            .filter(|_| status == 201);
        match id {
            Some(id) => p.id = Some(id),
            None => {
                p.refused = Some(if status == 201 {
                    format!(
                        "The box kept {name} but did not say where, so the agent was not asked. Nothing was sent to the agent."
                    )
                } else {
                    crate::device_vendor::refusal_words(status, body, name)
                });
            }
        }
        if self.spec.pdfs.iter().any(|p| p.sending) {
            return None;
        }
        self.uploading = false;
        Some(self.spec.pdfs.iter().all(|p| p.id.is_some()))
    }

    /// The run starts: the documents are held for its draft.
    pub fn begin_run(&mut self) {
        self.docs = Some((
            self.spec.documents(),
            self.spec.pdfs.iter().map(crate::spec_mode::Pdf::record).collect(),
        ));
        self.spec.open = false;
        self.spec.error = None;
    }

    /// The run's answer passed its checks: the draft goes on review under
    /// a name the box does not have.
    pub fn drafted(&mut self, apply: Apply, taken: &[String]) -> bool {
        let Some((docs, pdfs)) = self.docs.take() else {
            return false;
        };
        let draft = Draft::read(&apply, docs).for_box(pdfs);
        let name = free_name(&crate::playground_view::answer_name(&apply.text), taken);
        self.review = Some(Review::new(draft, name));
        self.apply = Some(apply);
        true
    }

    /// Accept: the project as a zip and its name, or `None` with the reason
    /// on the review (the name is refused beside the field, nothing is
    /// sent).
    pub fn accept(&mut self, taken: &[String]) -> Option<(String, Vec<u8>)> {
        if self.accepting {
            return None;
        }
        let r = self.review.as_mut()?;
        let name = r.name.trim().to_ascii_lowercase();
        if !helmsman_project::project_name(&name) {
            r.error =
                Some("A project's name is 1 to 48 of a-z 0-9 _ -, starting with a letter or digit.".into());
            return None;
        }
        if taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
            r.error = Some(format!(
                "A project named {name} is already on the box: pick another name."
            ));
            return None;
        }
        let files = match r.draft.files() {
            Ok(f) => f,
            Err(e) => {
                r.refusal = Some(e);
                return None;
            }
        };
        match helmsman_project::Files(files).to_zip() {
            Ok(zip) => {
                r.error = None;
                self.accepting = true;
                Some((name, zip))
            }
            Err(e) => {
                r.refusal = Some(format!("The project could not be packed: {e}"));
                None
            }
        }
    }

    /// The box refused the project (`status`, its `body`): said beside the
    /// name, the draft stays, nothing was written.
    pub fn refused(&mut self, name: &str, status: u16, body: &str) {
        self.accepting = false;
        let Some(r) = self.review.as_mut() else { return };
        let why = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|j| j["error"].as_str().map(str::to_string))
            .unwrap_or_default();
        let why = crate::device_proposal::clean(&why, 200, false);
        r.error = Some(match status {
            409 => format!(
                "A project named {name} is already on the box: pick another name. Nothing was written."
            ),
            401 => "The box no longer knows this page's session. Sign in again. Nothing was written.".into(),
            403 => "Only an engineer makes a project on the box. Nothing was written.".into(),
            0 => format!("The box did not answer. Nothing was written. Press Accept to try again. {why}"),
            s => format!("The box refused the project ({s}): {why}. Nothing was written."),
        });
    }

    /// The box made the project: the run is over; the checked answer and
    /// the review are returned for the outcome.
    pub fn made(&mut self) -> Option<(Apply, Draft)> {
        self.accepting = false;
        let r = self.review.take()?;
        let a = self.apply.take()?;
        self.spec = Spec {
            on_box: true,
            ..Spec::default()
        };
        Some((a, r.draft))
    }

    /// Discard, Stop or a failed run: nothing is held but the window's own
    /// text. `true` when there was a draft.
    pub fn drop_run(&mut self) -> bool {
        self.docs = None;
        self.accepting = false;
        self.apply = None;
        self.review.take().is_some()
    }
}

#[cfg(test)]
#[path = "box_spec_tests.rs"]
mod tests;
