// Archived 2026-11-08 (Nick: the Requirements tab is out of the UI for now; the
// tests the Agent writes with each change take its place). Not compiled; see
// README.md in this directory.

// From crates/helmsman-canvas/src/ui_tests.rs (mod polish / simulated; uses its
// Harness). The tour's last card also had a Requirements button
// (Button::Requirements -> Effect::Requirements, hit "tour requirements").

    /// Nick, 2026-11-05: "I need to be able to edit and delete
    /// requirements as well". Each row has Edit and Delete; Edit with the
    /// rewrite off changes only the words in the test's first comment,
    /// with it on the old test goes and the requirement is added afresh;
    /// Delete asks first.
    #[test]
    fn a_requirement_is_reworded_rewritten_and_deleted_from_its_row() {
        let mut h = Harness::new(App::sim());
        let ctx = h.ctx.clone();
        {
            let sim = h.app.sim.as_mut().unwrap();
            sim.proj.create("mine", Some("latch"));
            sim.open_project("mine");
            sim.add_requirement("The motor runs once start is pressed", &ctx);
        }
        let file = "test_the_motor_runs_once_start_is_pressed.st";
        assert!(
            h.app.sim.as_ref().unwrap().proj.files.contains_key(file),
            "no agent here: a placeholder"
        );
        h.press("tab tests");
        h.wait(0.3);
        let name = "test_the_motor_runs_once_start_is_pressed";
        assert!(
            h.has(&format!("req edit {name}")) && h.has(&format!("req delete {name}")),
            "on the row, not behind it"
        );
        // Reword, the test kept.
        h.press(&format!("req edit {name}"));
        h.wait(0.2);
        assert!(h.has("req edit form"));
        h.press("req edit rewrite");
        h.press("req edit text");
        h.frame(vec![Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }]);
        h.frame(vec![Event::Text("The motor runs within 1 s of start".into())]);
        h.wait(0.2);
        h.press("req edit ok");
        h.wait(0.3);
        assert!(!h.has("req edit form"));
        {
            let sim = h.app.sim.as_ref().unwrap();
            let text = &sim.proj.files[file];
            assert!(
                text.starts_with("(* The motor runs within 1 s of start *)\n"),
                "{text}"
            );
            let row = sim.tests.rows.iter().find(|r| r.file == file).expect("its row");
            assert_eq!(row.requirement, "The motor runs within 1 s of start");
        }
        // Rewrite: the old file goes, the requirement is added afresh.
        h.press(&format!("req edit {name}"));
        h.wait(0.2);
        h.press("req edit text");
        h.frame(vec![Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }]);
        h.frame(vec![Event::Text("The lamp is on while the motor runs".into())]);
        h.wait(0.2);
        h.press("req edit ok");
        h.wait(0.3);
        let new_file = "test_the_lamp_is_on_while_the_motor_runs.st";
        {
            let sim = h.app.sim.as_ref().unwrap();
            assert!(!sim.proj.files.contains_key(file), "the old test is gone");
            assert!(
                sim.proj.files.contains_key(new_file),
                "{:?}",
                sim.proj.files.keys().collect::<Vec<_>>()
            );
        }
        // Delete: asks, Cancel keeps it, Delete removes it.
        let new_name = "test_the_lamp_is_on_while_the_motor_runs";
        h.press(&format!("req delete {new_name}"));
        h.wait(0.2);
        assert!(h.has("req delete form"));
        h.press("req delete cancel");
        h.wait(0.2);
        assert!(
            h.app.sim.as_ref().unwrap().proj.files.contains_key(new_file),
            "Cancel keeps it"
        );
        h.press(&format!("req delete {new_name}"));
        h.wait(0.2);
        h.press("req delete ok");
        h.wait(0.3);
        assert!(!h.app.sim.as_ref().unwrap().proj.files.contains_key(new_file));
        assert!(!h.has(&format!("req edit {new_name}")));
    }

    /// Nick, 2026-11-05: the Requirements tab lists requirements in plain
    /// words with the test behind each row; a row opens to its test, and
    /// the test file stays out of the Files list.
    #[test]
    fn a_requirement_is_a_row_with_its_test_behind_it_not_a_file() {
        let mut h = Harness::new(App::sim());
        {
            let v = h.app.sim.as_mut().unwrap();
            v.proj.create("mine", Some("latch"));
            v.open_project("mine");
        }
        h.wait(0.2);
        let ctx = h.ctx.clone();
        h.app
            .sim
            .as_mut()
            .unwrap()
            .add_requirement("The motor runs once start is pressed", &ctx);
        h.press("tab tests");
        h.wait(0.2);
        assert!(h.has("req draft") && h.has("req add"), "the field to add one");
        let row = "test test_the_motor_runs_once_start_is_pressed";
        assert!(h.has(row), "its row");
        h.press(row);
        h.wait(0.2);
        assert!(
            h.has("test edit test_the_motor_runs_once_start_is_pressed"),
            "opened: Edit"
        );
        h.press("pane files");
        h.wait(0.2);
        assert!(h.has("file latch.st"));
        assert!(
            !h.has("file test_the_motor_runs_once_start_is_pressed.st"),
            "the test lives behind its row"
        );
    }

// ---- from requirement_tests_show_on_the_box_and_a_red_one_refuses_the_deploy
// (box_projects_tests.rs): the tab drawn ----
    // The tab draws the row red and the refusal shows under the header.
    let ctx = egui::Context::default();
    v.step_all(&ctx, vec![], vec![], 0.0);
    let mut chrome = crate::editor::Chrome::default();
    chrome.tab = crate::editor::Tab::Requirements;
    let mut out = ctx.run_ui(RawInput::default(), |ui| {
        crate::editor::Body::tab(&mut v, ui, crate::editor::Tab::Requirements, &mut chrome);
    });
    out.textures_delta.clear();
    assert!(chrome.rect("test test_lamp").is_some(), "{:?}", chrome.hits);
    assert!(chrome.rect("tests run").is_some());

