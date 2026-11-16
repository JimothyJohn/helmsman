//! One question while the Agent works (TODO.md `agent-wait-questions`).
//!
//! The wait for a change is the worst part of the Agent tab, so it is
//! used: when an instruction goes out, a small fast model (the relay's
//! `ask` agent) gets the instruction and the program and comes back,
//! in about a second, with at most one question about how the machine
//! should behave and two or three answers to click, or with nothing.
//! Most instructions draw nothing.
//!
//! What a click does depends on when it comes (Nick, 2026-11-15):
//!
//! - before the Agent has written anything, the instruction is sent
//!   again with the answer in it ([`with_answer`]);
//! - later, the change the Agent made is compared with the answer once
//!   it has passed its own checks (the relay's `check` agent,
//!   [`check_message`], [`verdict`]): one that agrees is written and the
//!   card says so; one that differs goes back to the Agent once more with
//!   the answer ([`differs`]), and nothing is written until that passes.
//!
//! An answer guides this one change and is kept on its History page; it
//! makes no requirement test of its own. Nothing here is a chat: a
//! question is a card with buttons, answered or ignored.
//!
//! This module is the words and the state; the page sends the requests
//! (`crate::playground_view`).

use crate::claude::{Segment, segments};

/// The relay's agent that asks.
pub const AGENT: &str = "ask";
/// The relay's agent that compares an answer with the result.
pub const CHECK_AGENT: &str = "check";
/// Fewest and most answers a question offers.
pub const ANSWERS: std::ops::RangeInclusive<usize> = 2..=3;
/// Longest question and longest answer shown, in characters.
pub const MAX_QUESTION: usize = 200;
pub const MAX_ANSWER: usize = 100;
/// How long the page waits for the comparison before it writes the
/// change anyway, in seconds: the check must never be what the engineer
/// waits for.
pub const CHECK_PATIENCE: f64 = 8.0;

/// A question and its answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub text: String,
    pub answers: Vec<String>,
}

/// The tags the two messages use.
const TAGS: [&str; 5] = ["instruction", "current_program", "question", "answer", "result_program"];

/// `text` between `<tag>` and `</tag>`. A closing tag of any of [`TAGS`]
/// inside it is defanged (`<\/tag>`), white space inside the tag and
/// any case included, so nothing in a program, an instruction or a
/// model's question ends an envelope early or opens the next.
fn tagged(tag: &str, text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(k) = rest.find('<') {
        safe.push_str(&rest[..k]);
        let after = &rest[k + 1..];
        let slash = after.trim_start();
        let closing = slash.strip_prefix('/').is_some_and(|r| {
            let name: String = r
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            TAGS.iter().any(|t| t.eq_ignore_ascii_case(&name))
        });
        if closing {
            safe.push_str("<\\");
            rest = slash;
        } else {
            safe.push('<');
            rest = after;
        }
    }
    safe.push_str(rest);
    format!("<{tag}>\n{}\n</{tag}>\n", safe.trim_end())
}

/// One line of at most `n` characters: control characters and line
/// breaks become spaces, runs of white space one space.
fn one_line(text: &str, n: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    words.join(" ").chars().take(n).collect()
}

/// The message that asks for a question about `instruction` on `program`.
#[must_use]
pub fn message(instruction: &str, program: &str) -> String {
    format!(
        "{}{}",
        tagged("instruction", instruction),
        tagged("current_program", program)
    )
}

/// The question in the `ask` agent's answer: its last `Q:` line and the
/// `A:` lines after it. `None` when it asked nothing (`NONE`), when the
/// answer is not in that form, or when it offers fewer than two or more
/// than three distinct answers: a card the engineer cannot answer with
/// one click is not shown.
#[must_use]
pub fn parse(answer: &str) -> Option<Question> {
    // The model sometimes thinks aloud first; what counts is how it ends.
    let prose: Vec<String> = segments(answer)
        .into_iter()
        .filter_map(|s| match s {
            Segment::Prose(p) => Some(p),
            Segment::Code { .. } => None,
        })
        .collect();
    let lines: Vec<&str> = prose
        .iter()
        .flat_map(|p| p.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.last().is_none_or(|l| l.eq_ignore_ascii_case("none")) {
        return None;
    }
    let field = |line: &str, key: &str| -> Option<String> {
        let head = line.get(..key.len())?;
        head.eq_ignore_ascii_case(key)
            .then(|| line[key.len()..].trim().to_string())
    };
    let at = lines.iter().rposition(|l| field(l, "Q:").is_some())?;
    let text = one_line(&field(lines[at], "Q:")?, MAX_QUESTION);
    let mut answers: Vec<String> = Vec::new();
    for line in &lines[at + 1..] {
        // Anything after the answers that is not one ends the card.
        let Some(a) = field(line, "A:") else { return None };
        let a = one_line(&a, MAX_ANSWER);
        if a.is_empty() || answers.iter().any(|b| b.eq_ignore_ascii_case(&a)) {
            return None;
        }
        answers.push(a);
    }
    (!text.is_empty() && ANSWERS.contains(&answers.len())).then_some(Question { text, answers })
}

/// `instruction` with the engineer's answer in it, for the Agent.
#[must_use]
pub fn with_answer(instruction: &str, question: &Question, picked: usize) -> String {
    format!(
        "{}\n\nAsked \"{}\", the engineer answered: \"{}\". The change must do that.",
        instruction.trim_end(),
        question.text,
        question.answers[picked]
    )
}

/// The message that asks whether `program`, the change the Agent made
/// for `instruction`, does what the engineer picked.
#[must_use]
pub fn check_message(instruction: &str, question: &Question, picked: usize, program: &str) -> String {
    format!(
        "{}{}{}{}",
        tagged("instruction", instruction),
        tagged("question", &question.text),
        tagged("answer", &question.answers[picked]),
        tagged("result_program", program)
    )
}

/// What the `check` agent found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The change does what the engineer picked.
    Agrees,
    /// It does something else.
    Differs,
}

/// The verdict in the `check` agent's answer: its last word. `None`
/// when that is neither, and then the change is taken as it is: only a
/// plain `DIFFERS` sends the Agent round again.
#[must_use]
pub fn verdict(answer: &str) -> Option<Verdict> {
    let last = answer
        .split_whitespace()
        .last()?
        .trim_matches(|c: char| !c.is_ascii_alphabetic());
    if last.eq_ignore_ascii_case("agrees") {
        Some(Verdict::Agrees)
    } else if last.eq_ignore_ascii_case("differs") {
        Some(Verdict::Differs)
    } else {
        None
    }
}

/// What the Agent is told when its change differs from the answer.
#[must_use]
pub fn differs(question: &Question, picked: usize) -> String {
    format!(
        "While you worked the engineer was asked \"{}\" and answered: \"{}\". Your change passed its checks but does not do that. Send the change again so that it does, with a scenario that checks it.",
        question.text, question.answers[picked]
    )
}

/// The line the change's History page keeps.
#[must_use]
pub fn record(question: &Question, picked: usize) -> String {
    format!("Asked: {} Answered: {}", question.text, question.answers[picked])
}

/// Where a question stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Asked of the `ask` agent, no answer yet.
    Asking,
    /// Shown, not answered.
    Open,
    /// Answered before the Agent had written anything: the instruction
    /// went out again with the answer in it.
    Sent,
    /// Answered while the Agent was writing: compared once its change
    /// has passed its checks.
    Waiting,
    /// The comparison is under way.
    Checking,
    /// The change does what was picked.
    Covered,
    /// It did not: the Agent has been sent round again with the answer.
    Amending,
}

/// The question that goes with the instruction under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub question: Option<Question>,
    pub picked: Option<usize>,
    pub state: State,
}

impl Default for Card {
    fn default() -> Self {
        Self {
            question: None,
            picked: None,
            state: State::Asking,
        }
    }
}

impl Card {
    /// The `ask` agent's whole answer arrived.
    pub fn asked(&mut self, answer: &str) {
        if self.state == State::Asking {
            self.question = parse(answer);
            self.state = State::Open;
        }
    }

    /// The card to show: the question, while there is one to answer or
    /// one that was answered.
    #[must_use]
    pub fn shown(&self) -> Option<&Question> {
        self.question.as_ref()
    }

    /// The engineer clicked answer `k`. `drafted`: the Agent has written
    /// something already. Returns whether the instruction is to be sent
    /// again with the answer ([`with_answer`]).
    pub fn pick(&mut self, k: usize, drafted: bool) -> bool {
        let Some(q) = &self.question else { return false };
        if self.state != State::Open || k >= q.answers.len() {
            return false;
        }
        self.picked = Some(k);
        if drafted {
            self.state = State::Waiting;
            false
        } else {
            self.state = State::Sent;
            true
        }
    }

    /// The question and the answer picked, when the change is still to be
    /// compared with it.
    #[must_use]
    pub fn to_check(&self) -> Option<(&Question, usize)> {
        (self.state == State::Waiting)
            .then(|| self.question.as_ref().zip(self.picked))
            .flatten()
    }

    /// The question and the answer the change went out with or was
    /// compared with, for its History page ([`record`]).
    #[must_use]
    pub fn answered(&self) -> Option<(&Question, usize)> {
        matches!(self.state, State::Sent | State::Covered | State::Amending)
            .then(|| self.question.as_ref().zip(self.picked))
            .flatten()
    }

    /// One line under the question, once it is answered.
    #[must_use]
    pub fn status(&self) -> Option<&'static str> {
        match self.state {
            State::Asking | State::Open => None,
            State::Sent => Some("The Agent is working with your answer."),
            State::Waiting => Some("Your answer is checked against the change when it is ready."),
            State::Checking => Some("Checking the change against your answer…"),
            State::Covered => Some("The change does what you answered."),
            State::Amending => Some("The change did not do that yet: the Agent is correcting it."),
        }
    }
}
