# The Requirements tab, archived

Archived 2026-11-08 (Nick): "Remove Requirements for now from the UI and
archive it. It's not that useful and creates more issues than it solves.
Realistically the automatically generated tests should solve this issue."

The tab (Nick, 2026-11-05) listed the project's requirement tests as rows in
plain words, green, red or grey, and took a new requirement typed in words
for the agent to write its test. Last whole in the tree at commit dc8f47e
(`git show dc8f47e:crates/helmsman-canvas/src/requirements.rs`).

Nothing in this directory is compiled. What stayed in `src/`:

- `requirements.rs`: `State`, `Row`, `Results` (the test results),
  `refusal` and `name_for` (verify.rs names the Agent's scenario tests with
  them).
- The gate: a red `test_*.st` still refuses Deploy and "Use this code", in
  the simulation (`PlaygroundView::tests_refuse`) and on the box (the box's
  own check). The refusal names the test in the deploy error.
- The box API: `POST /api/projects/{p}/tests` (the box and the CLI runner
  use it) and `POST /api/projects/{p}/requirements` (`api::requirements`,
  now without a caller in the canvas).

Files:

- `requirements_ui.rs`: `Backend`, `Phase`, `AddRequest`, `Adding`,
  `page_request`, `TEST_WRITER`, `ReqUi`, `Act`, `reword`, `View`, `ui`,
  `edit_ui`, `delete_ui`.
- `tests_writer.md`: the page's test-writer prompt (`TEST_WRITER`).
- `box_view_requirements.rs`: `BoxView::requirements_tab`,
  `requirement_answer`, `BoxProjects::reread`.
- `playground_view_requirements.rs`: `PlaygroundView::add_requirement`,
  `write_test`, `requirement_answer`, the `Tab::Requirements` arm,
  `claude::one_shot`.
- `requirements_tests.rs`, `ui_tests_requirements.rs`,
  `box_projects_tests_requirements.rs`: the tests that drove the tab.

To bring it back: `Tab::Requirements` in editor.rs (`ALL` had three tabs,
key `tests`), the fields named at the top of each file, the tour's last-card
button, `Desk::requirement` (canvas and `helmsman-plc/src/api/desk.rs`), and
the docs (docs/UI.md, docs/help/projects.md, docs/index.html,
docs/ARCHITECTURE.md "Writing a requirement").
