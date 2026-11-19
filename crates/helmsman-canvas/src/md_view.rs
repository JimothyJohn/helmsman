//! A small Markdown reader for what a design holds (`crate::design_step`):
//! headings, paragraphs, bullet and numbered lists, fenced code blocks, pipe
//! tables, and inside a line bold, italic and inline code. Display only: the
//! text kept in the project is what the model wrote.
//!
//! The reader never fails: anything it does not know stays as the text it
//! was. It is linear in the input (an opener with no closer is looked for
//! once), and bounds what it keeps (list depth, table width and length).
//! The screen part draws code blocks and tables in a horizontal scroll area
//! of their own, so a wide one never widens the window or the panel.

use eframe::egui::{self, Color32, FontId, RichText, TextFormat, text::LayoutJob};

/// Deepest list level kept; deeper lines sit at this level.
const MAX_DEPTH: usize = 8;
/// Most columns of a table kept.
const MAX_COLS: usize = 32;
/// Most rows of a table kept.
const MAX_ROWS: usize = 5000;

/// A run of a line with one style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

/// One block of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Run>),
    Para(Vec<Run>),
    Item {
        depth: usize,
        marker: String,
        text: Vec<Run>,
    },
    Code(String),
    Table {
        head: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
    },
}

/// The runs of one line: `**bold**`, `*italic*`, `` `code` ``. A marker with
/// no closing one is the text it is.
#[must_use]
pub fn inline(line: &str) -> Vec<Run> {
    let chars: Vec<char> = line.chars().collect();
    let mut runs: Vec<Run> = Vec::new();
    let (mut bold, mut italic) = (false, false);
    let mut cur = String::new();
    // A marker looked for to the end once and not found is not looked for
    // again: linear on a line of unclosed markers.
    let (mut no_bold, mut no_italic, mut no_code) = (false, false, false);
    let flush = |cur: &mut String, runs: &mut Vec<Run>, bold: bool, italic: bool, code: bool| {
        if !cur.is_empty() {
            runs.push(Run {
                text: std::mem::take(cur),
                bold,
                italic,
                code,
            });
        }
    };
    let has = |from: usize, pat: &[char]| chars[from..].windows(pat.len()).any(|w| w == pat);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' && !no_code {
            if let Some(len) = chars[i + 1..].iter().position(|x| *x == '`') {
                flush(&mut cur, &mut runs, bold, italic, false);
                let text: String = chars[i + 1..i + 1 + len].iter().collect();
                if !text.is_empty() {
                    runs.push(Run {
                        text,
                        bold,
                        italic,
                        code: true,
                    });
                }
                i += len + 2;
                continue;
            }
            no_code = true;
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if bold || (!no_bold && has(i + 2, &['*', '*'])) {
                flush(&mut cur, &mut runs, bold, italic, false);
                bold = !bold;
                i += 2;
                continue;
            }
            no_bold = true;
        } else if c == '*' {
            let opens = chars.get(i + 1).is_some_and(|n| !n.is_whitespace());
            if italic || (opens && !no_italic && has(i + 1, &['*'])) {
                flush(&mut cur, &mut runs, bold, italic, false);
                italic = !italic;
                i += 1;
                continue;
            }
            no_italic = true;
        }
        cur.push(c);
        i += 1;
    }
    flush(&mut cur, &mut runs, bold, italic, false);
    runs
}

/// `line` is a fence marker (three backticks or more at its start).
fn fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// The cells of a table row, outer pipes dropped.
fn cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|')
        .take(MAX_COLS)
        .map(|c| c.trim().to_string())
        .collect()
}

/// The separator row of a table: cells of dashes with optional colons.
fn separator(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.contains('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// A list item's depth, marker and text.
fn item(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let t = &line[indent..];
    let depth = (indent / 2).min(MAX_DEPTH);
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some((depth, "•".to_string(), rest));
        }
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=4).contains(&digits)
        && let Some(rest) = t[digits..]
            .strip_prefix(". ")
            .or_else(|| t[digits..].strip_prefix(") "))
    {
        return Some((depth, format!("{}.", &t[..digits]), rest));
    }
    None
}

/// Reads `text` into blocks.
#[must_use]
pub fn parse(text: &str) -> Vec<Block> {
    let text = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ");
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let end_para = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(inline(&para.join(" "))));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if fence(line) {
            end_para(&mut para, &mut out);
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !fence(lines[i]) {
                code.push(lines[i]);
                i += 1;
            }
            // The closing fence, if there is one.
            i += 1;
            out.push(Block::Code(code.join("\n")));
            continue;
        }
        if line.trim().is_empty() {
            end_para(&mut para, &mut out);
            i += 1;
            continue;
        }
        if line.trim_start().starts_with('|') && lines.get(i + 1).is_some_and(|l| separator(l)) {
            end_para(&mut para, &mut out);
            let head = cells(line);
            let cols = head.len();
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim_start().starts_with('|') && rows.len() < MAX_ROWS {
                let mut row = cells(lines[i]);
                row.resize(cols, String::new());
                rows.push(row.iter().map(|c| inline(c)).collect());
                i += 1;
            }
            out.push(Block::Table {
                head: head.iter().map(|c| inline(c)).collect(),
                rows,
            });
            continue;
        }
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            end_para(&mut para, &mut out);
            out.push(Block::Heading(hashes as u8, inline(line[hashes..].trim())));
            i += 1;
            continue;
        }
        if let Some((depth, marker, rest)) = item(line) {
            end_para(&mut para, &mut out);
            out.push(Block::Item {
                depth,
                marker,
                text: inline(rest.trim()),
            });
            i += 1;
            continue;
        }
        para.push(line.trim());
        i += 1;
    }
    end_para(&mut para, &mut out);
    out
}

/// Every run's text of `blocks`, for tests.
#[cfg(test)]
pub fn texts(blocks: &[Block]) -> Vec<String> {
    let runs = |r: &[Run]| r.iter().map(|x| x.text.as_str()).collect::<String>();
    let mut out = Vec::new();
    for b in blocks {
        match b {
            Block::Heading(_, r) | Block::Para(r) | Block::Item { text: r, .. } => out.push(runs(r)),
            Block::Code(c) => out.push(c.clone()),
            Block::Table { head, rows } => {
                out.extend(head.iter().map(|c| runs(c)));
                out.extend(rows.iter().flatten().map(|c| runs(c)));
            }
        }
    }
    out
}

/// The runs as one laid-out text of `size` points, `color` for plain text.
fn job(runs: &[Run], size: f32, color: Color32, strong: Color32, code_bg: Color32, wrap: f32) -> LayoutJob {
    let mut j = LayoutJob::default();
    j.wrap.max_width = wrap;
    for r in runs {
        let fmt = TextFormat {
            font_id: if r.code {
                FontId::monospace(size - 1.0)
            } else {
                FontId::proportional(size)
            },
            color: if r.bold { strong } else { color },
            italics: r.italic,
            background: if r.code { code_bg } else { Color32::TRANSPARENT },
            ..TextFormat::default()
        };
        j.append(&r.text, 0.0, fmt);
    }
    j
}

/// One line of runs as a wrapped label.
pub fn runs_ui(ui: &mut egui::Ui, runs: &[Run], size: f32) -> egui::Response {
    let t = crate::theme::theme(ui);
    let strong = ui.visuals().strong_text_color();
    let wrap = ui.available_width().max(40.0);
    ui.add(egui::Label::new(job(runs, size, t.fg, strong, ui.visuals().faint_bg_color, wrap)).wrap())
}

/// Inline Markdown `text` as a wrapped label.
pub fn label(ui: &mut egui::Ui, text: &str, size: f32) -> egui::Response {
    runs_ui(ui, &inline(text), size)
}

/// Draws `blocks` at `size`. `salt` tells this document's scroll areas from
/// another's.
pub fn show(ui: &mut egui::Ui, blocks: &[Block], size: f32, salt: &str) {
    let t = crate::theme::theme(ui);
    for (k, b) in blocks.iter().enumerate() {
        match b {
            Block::Heading(level, runs) => {
                ui.add_space(6.0);
                let extra = f32::from(7u8.saturating_sub(*level)) * 0.5;
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                ui.add(egui::Label::new(RichText::new(text).strong().size(size + extra).color(t.fg)).wrap());
            }
            Block::Para(runs) => {
                runs_ui(ui, runs, size);
                ui.add_space(4.0);
            }
            Block::Item { depth, marker, text } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(14.0 * *depth as f32 + 4.0);
                    ui.label(RichText::new(marker).size(size).color(t.muted));
                    runs_ui(ui, text, size);
                });
            }
            Block::Code(code) => {
                egui::Frame::new()
                    .fill(ui.visuals().faint_bg_color)
                    .inner_margin(4.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::horizontal()
                            .id_salt(format!("{salt}-code-{k}"))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(code).monospace().size(size - 1.0).color(t.fg),
                                    )
                                    .extend(),
                                );
                            });
                    });
                ui.add_space(4.0);
            }
            Block::Table { head, rows } => {
                let cols = head.len().max(1);
                let col_w = (ui.available_width() / cols as f32).clamp(110.0, 280.0);
                let strong = ui.visuals().strong_text_color();
                egui::ScrollArea::horizontal()
                    .id_salt(format!("{salt}-table-{k}"))
                    .show(ui, |ui| {
                        egui::Grid::new(format!("{salt}-grid-{k}"))
                            .striped(true)
                            .show(ui, |ui| {
                                for c in head {
                                    cell(ui, c, size, col_w, true, strong);
                                }
                                ui.end_row();
                                for row in rows {
                                    for c in row {
                                        cell(ui, c, size, col_w, false, strong);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(4.0);
            }
        }
    }
}

fn cell(ui: &mut egui::Ui, runs: &[Run], size: f32, width: f32, head: bool, strong: Color32) {
    let t = crate::theme::theme(ui);
    ui.vertical(|ui| {
        ui.set_width(width);
        let color = if head { strong } else { t.fg };
        let bg = ui.visuals().faint_bg_color;
        ui.add(egui::Label::new(job(runs, size, color, strong, bg, width)).wrap());
    });
}

#[cfg(test)]
#[path = "md_view_tests.rs"]
mod tests;
