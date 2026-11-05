//! Every control the canvas draws has a tooltip: read from the sources,
//! since egui keeps no list of the widgets a frame drew. A control is a
//! button, a check box, a selectable, a combo box, a text field, a slider
//! or a drag value; its statement (or the response it is bound to, within
//! the block that follows) must reach `.tip(`, `on_hover_text` or
//! `on_hover_ui`. A tooltip only while disabled is not enough: the
//! enabled control explains itself too.

use std::path::Path;

const CONTROLS: &[&str] = &[
    ".button(",
    ".small_button(",
    "Button::new(",
    "Button::image",
    "Button::selectable(",
    ".checkbox(",
    "Checkbox::new(",
    ".selectable_value(",
    ".selectable_label(",
    "ComboBox::",
    "TextEdit::",
    ".text_edit_singleline(",
    ".text_edit_multiline(",
    "Slider::new(",
    "DragValue::new(",
    ".radio_value(",
    ".toggle_value(",
    ".link(",
];
const TIPS: &[&str] = &[".tip(", ".on_hover_text(", ".on_hover_ui("];

/// The statement that starts at `at`: up to the `;` (or the closing
/// bracket) at its own depth.
fn statement(src: &str, at: usize) -> &str {
    let mut depth = 0i32;
    for (k, c) in src[at..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth < 0 {
                    return &src[at..at + k];
                }
            }
            ';' if depth == 0 => return &src[at..at + k],
            _ => {}
        }
    }
    &src[at..]
}

/// Where the statement holding `at` starts: after the last `;`, `{` or
/// `}` before it at a lower or equal depth.
fn statement_start(src: &str, at: usize) -> usize {
    let mut depth = 0i32;
    for (k, c) in src[..at].char_indices().rev() {
        match c {
            ')' | ']' => depth += 1,
            '(' | '[' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
            }
            ';' | '{' | '}' if depth == 0 => return k + 1,
            _ => {}
        }
    }
    0
}

/// The name a `let NAME = ...` statement binds, if any.
fn bound(stmt: &str) -> Option<&str> {
    // Past the comment lines a statement may start with.
    let mut rest = stmt.trim_start();
    while rest.starts_with("//") {
        rest = rest.split_once('\n').map_or("", |(_, r)| r).trim_start();
    }
    let rest = rest.strip_prefix("let ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let name: &str = rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next()?;
    (!name.is_empty()).then_some(name)
}

fn untipped(src: &str) -> Vec<(usize, String)> {
    // Test modules are not drawn.
    let code = src.split("#[cfg(test)]\nmod tests").next().unwrap_or(src);
    let mut out = Vec::new();
    for pat in CONTROLS {
        for (at, _) in code.match_indices(pat) {
            let line = code[..at].matches('\n').count() + 1;
            let lstart = code[..at].rfind('\n').map_or(0, |k| k + 1);
            let this_line = &code[lstart..code[at..].find('\n').map_or(code.len(), |k| at + k)];
            if this_line.trim_start().starts_with("//") || this_line.contains("tip: exempt") {
                continue;
            }
            // `state.link()`, a getter, is no hyperlink.
            if *pat == ".link(" && code[at + pat.len()..].starts_with(')') {
                continue;
            }
            let start = statement_start(code, at);
            let stmt = statement(code, start);
            if TIPS.iter().any(|t| stmt.contains(t)) {
                continue;
            }
            // `let r = ui.button(..);` then `r.tip(..)` (or `r\n.on_hover..`)
            // further down the block.
            if let Some(name) = bound(stmt) {
                let after = &code[start + stmt.len()..];
                let block = &after[..after.len().min(4000)];
                // Up to the end of the statement that binds the name again
                // (its right side may still use the old one): a later
                // `let r` is another control.
                let block = [format!("let {name} "), format!("let mut {name} ")]
                    .iter()
                    .filter_map(|l| block.find(l.as_str()))
                    .min()
                    .map_or(block, |k| &block[..k + statement(block, k).len()]);
                let used = TIPS.iter().any(|t| {
                    let t = t.trim_start_matches('.');
                    block.match_indices(name).any(|(k, _)| {
                        let before_ok = block[..k]
                            .chars()
                            .next_back()
                            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                        let rest = block[k + name.len()..].trim_start();
                        let rest = rest.strip_prefix(".clone()").unwrap_or(rest).trim_start();
                        before_ok
                            && rest
                                .strip_prefix('.')
                                .is_some_and(|r| r.trim_start().starts_with(t))
                    })
                });
                // `let b = Button::new(..);` then `ui.add(b).tip(..)`: the
                // statement that uses the name carries the tooltip.
                let added = block.match_indices(name).any(|(k, _)| {
                    let word = block[..k]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                        && block[k + name.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c == ')' || c == ',');
                    let abs = start + stmt.len() + k;
                    let s = statement_start(code, abs);
                    word && TIPS.iter().any(|t| statement(code, s).contains(t))
                });
                if used || added {
                    continue;
                }
            }
            let shown: String = this_line.trim().chars().take(90).collect();
            out.push((line, shown));
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn every_control_has_a_tooltip() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut missing = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let rs = path.extension().is_some_and(|e| e == "rs");
        if !rs || name.ends_with("_tests.rs") || name == "web.rs" {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        for (line, text) in untipped(&src) {
            missing.push(format!("{name}:{line}: {text}"));
        }
    }
    assert_eq!(
        missing,
        [] as [String; 0],
        "{} controls without a tooltip:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn the_scanner_sees_a_bare_button_and_its_tooltip() {
    assert_eq!(untipped("fn f(ui: &mut Ui) { ui.button(\"Go\"); }").len(), 1);
    assert_eq!(
        untipped("fn f(ui: &mut Ui) { ui.button(\"Go\").tip(\"go\"); }"),
        []
    );
    assert_eq!(
        untipped("fn f(ui: &mut Ui) { let r = ui.button(\"Go\");\n if r.tip(\"go\").clicked() {} }"),
        []
    );
    assert_eq!(
        untipped(
            "fn f(ui: &mut Ui) { if ui.add(egui::Button::new(\"Go\")).on_hover_text(\"go\").clicked() {} }"
        ),
        []
    );
    assert_eq!(
        untipped("fn f(ui: &mut Ui) { if ui.add(egui::Button::new(\"Go\")).clicked() {} }").len(),
        1
    );
    assert_eq!(
        untipped("fn f(ui: &mut Ui) { let b = egui::Button::new(\"Go\");\n let r = ui.add(b).tip(\"go\"); }"),
        []
    );
    assert_eq!(
        untipped("fn f(ui: &mut Ui) { let b = egui::Button::new(\"Go\");\n let r = ui.add(b); }").len(),
        1
    );
    assert_eq!(
        untipped(
            "fn f(ui: &mut Ui) { let r = ui.button(\"A\");\n r.clicked();\n let r = ui.button(\"B\").tip(\"b\");\n r.tip(\"b\"); }"
        )
        .len(),
        1
    );
    assert_eq!(
        untipped(
            "fn f(ui: &mut Ui) {\n // a note (with brackets)\n let b = egui::Button::new(\"Go\");\n let r = ui.add(b).tip(\"go\"); }"
        ),
        []
    );
    assert_eq!(
        untipped(
            "fn f(ui: &mut Ui) { let r = ui.button(\"Go\");\n let r = match x { Some(f) => r.tip(f), None => r.tip(\"go\") }; }"
        ),
        []
    );
    assert_eq!(untipped("fn f(s: &S) { if s.link() == L {} }"), []);
    assert_eq!(untipped("fn f(ui: &mut Ui) { ui.link(\"site\"); }").len(), 1);
}
