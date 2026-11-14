//! A change too big for one answer, done in steps (Nick, 2026-11-14:
//! "You should proactively know to split up this task and then combine
//! them at the end once they're all working ... I do expect to need to
//! approve if it's iffy before the next major step is taken").
//!
//! - **When**: up front for a big ask ([`big_ask`]), and after any answer
//!   that ran out of room.
//! - **The plan**: the Agent is asked for a numbered list of steps
//!   ([`plan_request`], read back by [`parse`]). A plan of one step is
//!   just the instruction, sent as usual. A plan of more waits for the
//!   engineer to approve it.
//! - **The steps** ([`step_request`]) run one after the other on a
//!   working copy, each checked and retried like any answer. The file
//!   changes once, when every step has passed, as one version in History.
//!   A step that fails its checks, or runs out of room, pauses the plan
//!   for the engineer: try it again, or stop.

/// Most steps a plan may have.
pub const MAX_STEPS: usize = 6;
/// Longest step kept, in characters.
const MAX_STEP: usize = 300;
/// An instruction at least this long is a big ask, in characters.
const BIG_INSTRUCTION: usize = 280;
/// An instruction with at least this many items or sentences is a big ask.
const BIG_PARTS: usize = 4;
/// A file at least this long is a big ask, in lines.
const BIG_FILE: usize = 150;

/// Whether `instruction` on a file of `lines` lines (`None`: a new
/// project) is planned in steps before any code is asked for: a long
/// instruction, one of several parts, or a long file.
#[must_use]
pub fn big_ask(instruction: &str, lines: Option<usize>) -> bool {
    let parts = instruction
        .split(['\n', '.', ';'])
        .filter(|p| p.split_whitespace().count() >= 3)
        .count();
    instruction.chars().count() >= BIG_INSTRUCTION
        || parts >= BIG_PARTS
        || lines.is_some_and(|n| n >= BIG_FILE)
}

/// The words that ask for a plan of `instruction` instead of code;
/// `cut_off` when an answer to it in one go ran out of room.
#[must_use]
pub fn plan_request(instruction: &str, cut_off: bool) -> String {
    let why = if cut_off {
        "The answer to this instruction in one go ran out of room before the code was finished."
    } else {
        "This instruction is big, so it is planned first."
    };
    format!(
        "{why} Do not write any code in this answer. Split the instruction into 2 to {MAX_STEPS} steps, in the order to do them. Each step is one change to the file that can be written whole and checked on its own with a scenario, and builds on the steps before it. Answer with the numbered list only, one step per line, each step one plain sentence (`1. Add the start/stop latch for the motor.`). If the instruction fits in one answer after all, answer `1.` and the instruction itself.\n\nThe instruction: {instruction}"
    )
}

/// The steps of a plan: its numbered lines (`1. `, `2) `), in order, each
/// cut to [`MAX_STEP`] characters, at most [`MAX_STEPS`]. Empty when the
/// answer is not a plan (no numbered line, or it carries code).
#[must_use]
pub fn parse(answer: &str) -> Vec<String> {
    if answer.contains("```") {
        return Vec::new();
    }
    answer
        .lines()
        .filter_map(|l| {
            let l = l.trim().trim_start_matches(['*', '-', ' ']);
            let digits = l.chars().take_while(char::is_ascii_digit).count();
            if digits == 0 || digits > 2 {
                return None;
            }
            let rest = l[digits..].strip_prefix(['.', ')'])?;
            let step = rest.trim().trim_matches('*').trim();
            (!step.is_empty()).then(|| step.chars().take(MAX_STEP).collect())
        })
        .take(MAX_STEPS)
        .collect()
}

/// The words that ask for step `k` (from 0) of `steps`, the plan of
/// `instruction`.
#[must_use]
pub fn step_request(instruction: &str, steps: &[String], k: usize) -> String {
    use std::fmt::Write as _;
    let n = steps.len();
    let mut m = format!(
        "Step {} of {n} of a larger change. The whole change: {instruction}\nThe plan:\n",
        k + 1
    );
    for (i, s) in steps.iter().enumerate() {
        let mark = match i.cmp(&k) {
            std::cmp::Ordering::Less => " (done, already in the file)",
            std::cmp::Ordering::Equal => " (this answer)",
            std::cmp::Ordering::Greater => " (a later answer)",
        };
        let _ = writeln!(m, "{}. {s}{mark}", i + 1);
    }
    let _ = write!(
        m,
        "Do step {} only: {}. Keep what the earlier steps did working, and leave the later steps for later answers.",
        k + 1,
        steps[k].trim_end_matches('.')
    );
    m
}
