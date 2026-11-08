// Archived 2026-11-08 (Nick: the Requirements tab is out of the UI for now; the
// tests the Agent writes with each change take its place). Not compiled; see
// README.md in this directory.

// From crates/helmsman-canvas/src/playground_view.rs (impl PlaygroundView).
// Fields removed: req_ui: ReqUi, adding: Option<Adding>, req_seq: u64 (1 << 40);
// the link's drain gave each Incoming to requirement_answer first. The
// Body::tab arm for Tab::Requirements follows the methods.

    /// Adds a requirement: the Agent writes its test when it can be
    /// reached (the visitor's key or the local proxy); else it is written
    /// as "no test yet".
    pub fn add_requirement(&mut self, text: &str, ctx: &egui::Context) {
        use crate::requirements::{Adding, Phase, name_for, page_request};
        if self.proj.is_example() || crate::requirements::refusal(text).is_some() {
            return;
        }
        let taken: Vec<String> = self.proj.files.keys().cloned().collect();
        let name = name_for(text, &taken);
        self.req_seq += 1;
        let seq = self.req_seq;
        let mut a = Adding::in_page(text, &name, seq);
        match self.chat.route() {
            Ok(route) => {
                let programs: Vec<(String, String)> = self
                    .proj
                    .files
                    .iter()
                    .filter(|(f, _)| {
                        helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St)
                            && !helmsman_iec::spec::is_test(f)
                    })
                    .map(|(f, t)| (f.clone(), t.clone()))
                    .collect();
                let req = page_request(&route, self.chat.model, text, &name, &programs);
                self.link.send(seq, req, ctx);
            }
            Err(why) if matches!(self.chat.proxy, crate::claude::Proxy::Checking) => {
                a.phase = Phase::Failed(why.into());
            }
            Err(_) => {
                let file = self.write_test(&name, &helmsman_iec::spec::placeholder(&name, text));
                a.phase = Phase::Untested {
                    file,
                    why: "No agent here: paste an API key in the Agent tab, or open the row and write the test yourself."
                        .into(),
                };
            }
        }
        self.adding = Some(a);
    }

    /// Writes `test_<name>.st` into the open project, saved, and runs the
    /// tests; the file's name.
    fn write_test(&mut self, name: &str, text: &str) -> String {
        let file = format!("test_{name}.st");
        self.proj.files.insert(file.clone(), text.to_string());
        self.proj.save();
        self.run_tests();
        file
    }

    /// An answer for the requirement being added: taken (`true`) or not
    /// one of its pieces.
    fn requirement_answer(&mut self, m: &crate::claude::Incoming) -> bool {
        use crate::claude::Incoming;
        use crate::requirements::{Phase, State};
        let Some(a) = self.adding.as_mut() else {
            return false;
        };
        let seq = match m {
            Incoming::Status { seq, .. }
            | Incoming::Chunk { seq, .. }
            | Incoming::End { seq }
            | Incoming::Unreachable { seq, .. } => *seq,
            Incoming::Proxy(_) => return false,
        };
        if seq != a.seq {
            return seq > 1 << 40;
        }
        match m {
            Incoming::Chunk { text, .. } => a.chunk(text),
            Incoming::Status { status, body, .. } => {
                a.phase = Phase::Failed(format!(
                    "the Agent answered {status}: {}",
                    body.chars().take(200).collect::<String>()
                ));
            }
            Incoming::Unreachable { why, .. } => a.end(Some(why)),
            Incoming::End { .. } => {
                a.end(None);
                if a.busy() {
                    let (name, requirement) = (a.name.clone(), a.requirement.clone());
                    let program = a.page_program();
                    let text = match &program {
                        Some(p) => format!("{}{}", helmsman_iec::spec::header(&requirement), p.trim_start()),
                        None => helmsman_iec::spec::placeholder(&name, &requirement),
                    };
                    let file = self.write_test(&name, &text);
                    let row = self.tests.rows.iter().find(|r| r.file == file).cloned();
                    let a = self.adding.as_mut().expect("still adding");
                    a.phase = match (program, row) {
                        (None, _) => Phase::Untested {
                            file,
                            why: "the Agent's answer held no test; open the row to write one".into(),
                        },
                        (Some(_), Some(r)) if r.state == State::Untested => Phase::Untested {
                            file,
                            why: String::new(),
                        },
                        (Some(_), Some(r)) => Phase::Written {
                            file,
                            state: r.state,
                            why: r.why,
                        },
                        (Some(_), None) => Phase::Written {
                            file,
                            state: State::Failed,
                            why: "it did not run".into(),
                        },
                    };
                }
            }
            Incoming::Proxy(_) => {}
        }
        true
    }


// ---- Body::tab arm ----
            Tab::Requirements => {
                let ctx = ui.ctx().clone();
                let act = egui::ScrollArea::vertical()
                    .id_salt("pg-tests")
                    .show(ui, |ui| {
                        let files = &self.proj.files;
                        let source = |f: &str| files.get(f).cloned();
                        let cannot_add = self.proj.is_example().then(|| {
                            "Make this example your own first: click it in Files and name it.".to_string()
                        });
                        let view = crate::requirements::View {
                            results: &self.tests,
                            adding: self.adding.as_ref(),
                            busy: false,
                            cannot_add,
                            source: &source,
                            agent: "the Agent (your API key or the local proxy)",
                        };
                        crate::requirements::ui(ui, chrome, &view, &mut self.req_ui)
                    })
                    .inner;
                match act {
                    Some(crate::requirements::Act::Add(text)) => self.add_requirement(&text, &ctx),
                    Some(crate::requirements::Act::Run) => self.run_tests(),
                    Some(crate::requirements::Act::Edit(f)) => {
                        self.proj.open_file(&f);
                        self.reload();
                        chrome.show_code();
                    }
                    Some(crate::requirements::Act::Delete(f)) => {
                        self.proj.delete_file(&f);
                        self.req_ui.open = None;
                        self.run_tests();
                    }
                    Some(crate::requirements::Act::Reword { file, text }) => {
                        self.proj.files.insert(file, text);
                        self.proj.save();
                        self.run_tests();
                    }
                    Some(crate::requirements::Act::Rewrite { file, requirement }) => {
                        self.proj.delete_file(&file);
                        self.req_ui.open = None;
                        self.add_requirement(&requirement, &ctx);
                    }
                    None => {}
                }
            }

// ---- claude.rs: the one-message request page_request used ----
/// A one-message request with its own system prompt (the Requirements
/// tab's test writer), on the same route and with the same headers as the
/// Agent's.
#[must_use]
pub fn one_shot(route: &Route, model: Model, system: &str, user: &str) -> Request {
    let body = json!({
        "model": model.id(),
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "output_config": {"effort": "medium"},
        "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
        "messages": [{"role": "user", "content": user}],
    });
    finish(route, model, body)
}


// ---- claude.rs: the <box-data> envelope page_request wrapped the program in ----
/// `data` ready to go inside a `<box-data>` envelope: a closing tag in it
/// cannot end the envelope early.
#[must_use]
pub fn neutralize_data(data: &str) -> String {
    neutralize(data, "box-data")
}
