// Archived 2026-11-08 (Nick: the Requirements tab is out of the UI for now; the
// tests the Agent writes with each change take its place). Not compiled; see
// README.md in this directory.

// From crates/helmsman-canvas/src/box_view.rs (impl BoxView) and box_projects.rs
// (impl BoxProjects: reread). Fields removed from BoxView: req_ui: ReqUi,
// adding: Option<Adding>, req_seq: u64, req_http: web::ChatHttp (wasm32),
// req_sent: Vec<AddRequest> (native); chat_inbox drained req_http into
// requirement_answer; the desk kept req_ui.draft as Desk::requirement.

    /// The Requirements tab: the box ran the tests (POST
    /// /api/projects/{p}/tests) when the project opened and after each
    /// save; a new requirement goes to the box's agent.
    fn requirements_tab(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::requirements::{Act, Adding, View, name_for};
        let cannot_add = if !self.has_projects() || self.projects.open.is_none() {
            Some("Open a program first: its requirements are kept with it.".to_string())
        } else {
            is_engineer(self.session.as_ref()).err()
        };
        let act = egui::ScrollArea::vertical()
            .id_salt("box-tests")
            .show(ui, |ui| {
                let open = self.projects.open.as_ref();
                let source = |f: &str| {
                    open.and_then(|o| o.bufs.get(f))
                        .filter(|b| b.loaded)
                        .map(|b| b.text.clone())
                };
                let view = View {
                    results: &self.projects.tests,
                    adding: self.adding.as_ref(),
                    busy: self.projects.testing,
                    cannot_add: cannot_add.clone(),
                    source: &source,
                    agent: "the agent on this box",
                };
                crate::requirements::ui(ui, chrome, &view, &mut self.req_ui)
            })
            .inner;
        let act = match act {
            Some(Act::Rewrite { file, requirement }) if cannot_add.is_none() => {
                self.projects.delete_file(&file);
                self.req_ui.open = None;
                Some(Act::Add(requirement))
            }
            a => a,
        };
        match act {
            Some(Act::Add(text)) if cannot_add.is_none() => {
                let Some(o) = &self.projects.open else { return };
                let taken: Vec<String> = o.bufs.keys().cloned().collect();
                let name = name_for(&text, &taken);
                self.req_seq += 1;
                let (a, req) = Adding::to_box(&o.name, &text, &name, self.req_seq);
                self.adding = Some(a);
                #[cfg(target_arch = "wasm32")]
                self.req_http.send(
                    crate::box_agent::Request {
                        seq: req.seq,
                        url: req.url,
                        body: req.body,
                    },
                    ui.ctx(),
                );
                #[cfg(not(target_arch = "wasm32"))]
                self.req_sent.push(req);
            }
            Some(Act::Run) => self.projects.run_tests(),
            Some(Act::Edit(f)) => {
                self.projects.open_file(&f);
                chrome.show_code();
            }
            Some(Act::Delete(f)) if cannot_add.is_none() => {
                self.projects.delete_file(&f);
                self.req_ui.open = None;
            }
            Some(Act::Reword { file, text }) if cannot_add.is_none() => {
                if let Some(b) = self.projects.open.as_mut().and_then(|o| o.bufs.get_mut(&file)) {
                    b.text = text;
                }
                self.save();
            }
            _ => {}
        }
    }

    /// A piece of the box's answer to the requirement being added.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn requirement_answer(&mut self, inc: crate::box_agent::Incoming) {
        use crate::box_agent::Incoming;
        let Some(a) = self.adding.as_mut() else { return };
        let seq = match &inc {
            Incoming::Status { seq, .. }
            | Incoming::Chunk { seq, .. }
            | Incoming::End { seq }
            | Incoming::Failed { seq, .. } => *seq,
        };
        if seq != a.seq {
            return;
        }
        match inc {
            Incoming::Status { status, body, .. } => a.refused(status, &body),
            Incoming::Chunk { text, .. } => a.chunk(&text),
            Incoming::End { .. } => {
                a.end(None);
                // The new test file is on the box: read the list again, which
                // runs the tests, keeping any unsaved edit (Nick, 2026-11-07).
                self.projects.reread();
            }
            Incoming::Failed { why, .. } => a.end(Some(&why)),
        }
    }


    /// Reads the open project's list again without dropping anything: an
    /// unsaved buffer keeps its text, a new file comes in, a changed one
    /// is read if it has no edits (the `PCall::Project` answer merges).
    /// [`open_project`](Self::open_project) starts over instead.
    pub fn reread(&mut self) {
        if let Some(name) = self.open.as_ref().map(|o| o.name.clone()) {
            self.queue
                .push(PReq::get(PCall::Project(name.clone()), project_url(&name)));
            self.read_history(&name);
        }
    }
