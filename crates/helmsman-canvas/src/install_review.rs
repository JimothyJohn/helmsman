//! What the review screen knows about installing a bundle from a
//! marketplace and about the Agent's review of it, free of I/O like
//! [`crate::review`] (its requests go through that module's transport).
//!
//! The box does the work: `POST /api/market/install` fetches the bundle,
//! checks it, has the Agent read it, and either makes a proposal (the
//! approval below is unchanged) or stops with a reason. This module only
//! reads the box's answer ([`BundleReview::parse`], [`Install`]); nothing
//! here decides whether a bundle is allowed, and the Agent's words are
//! shown as plain text, never as markup.
//!
//! The box's answers, and what the screen shows for each:
//!
//! - `201`: a proposal, with `review.state` `reviewed` (the Agent found
//!   nothing to refuse; its summary is shown above the approval, and it is
//!   not an approval), `not_reviewed` (an engineer went on without a
//!   review) or `not_asked` (a file imported by hand).
//! - `422` with `review.state` `refused`: the Agent advises against
//!   installing; the reason is shown and nothing was installed.
//! - `409` with `review.state` `not_reviewed`: the Agent could not be
//!   asked or its answer could not be read; the way on is "Install
//!   without a review", for an engineer signed in.

use serde_json::Value as Json;

/// Longest piece of the Agent's text drawn (the box bounds it far below
/// this; this is the page's own limit).
const MAX_TEXT: usize = 900;

/// Where the review stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The Agent read it and did not refuse.
    Reviewed,
    /// The Agent advises against installing it.
    Refused,
    /// No review: why, and whether somebody went on.
    NotReviewed,
    /// The bundle came from a file: no review was asked for.
    NotAsked,
}

/// The review as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleReview {
    pub state: State,
    /// What the bundle does, in plain words.
    pub summary: String,
    /// What it touches: heading and entries, in a fixed order (an empty
    /// list is the Agent saying there is none).
    pub lists: Vec<(&'static str, Vec<String>)>,
    /// `low`, `medium` or `high`.
    pub risk: Option<String>,
    /// Why it refused, or what to check before approving.
    pub reason: String,
    /// Why there was no review.
    pub why: String,
    /// Who went on without one.
    pub went_on_by: Option<String>,
}

/// The Agent's text as one displayable string: control and invisible
/// characters as spaces, cut at [`MAX_TEXT`] characters. It is only ever
/// drawn as text.
#[must_use]
pub fn plain(s: &str) -> String {
    let mut out: String = s
        .chars()
        .take(MAX_TEXT)
        .map(|c| {
            let bidi = matches!(
                c,
                '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
            );
            if bidi || (c.is_control() && c != '\n') {
                ' '
            } else {
                c
            }
        })
        .collect();
    if s.chars().count() > MAX_TEXT {
        out.push('…');
    }
    out.trim().to_string()
}

fn text(j: &Json) -> String {
    plain(j.as_str().unwrap_or_default())
}

fn list(j: &Json) -> Vec<String> {
    j.as_array()
        .map(|a| a.iter().filter_map(Json::as_str).map(plain).take(24).collect())
        .unwrap_or_default()
}

impl BundleReview {
    /// The box's `review` object, or `None` when there is none or it is
    /// not one the screen knows.
    #[must_use]
    pub fn parse(j: &Json) -> Option<Self> {
        let state = match j["state"].as_str()? {
            "reviewed" => State::Reviewed,
            "refused" => State::Refused,
            "not_reviewed" => State::NotReviewed,
            "not_asked" => State::NotAsked,
            _ => return None,
        };
        let v = &j["verdict"];
        if matches!(state, State::Reviewed | State::Refused) && !v.is_object() {
            return None;
        }
        Some(Self {
            state,
            summary: text(&v["summary"]),
            lists: vec![
                ("Reads", list(&v["reads"])),
                ("Drives", list(&v["writes"])),
                ("Devices it names", list(&v["devices"])),
                ("Network or file access it asks for", list(&v["access"])),
                ("Runs at start", list(&v["at_start"])),
                ("Does more than its description says", list(&v["undeclared"])),
            ],
            risk: v["risk"].as_str().map(plain),
            reason: text(&v["reason"]),
            why: text(&j["why"]),
            went_on_by: j["went_on_by"].as_str().map(plain),
        })
    }

    /// The one line over the review: what it came to.
    #[must_use]
    pub fn headline(&self) -> String {
        match self.state {
            State::Reviewed => "The Agent read this bundle and found nothing to refuse. The approval below still applies in full.".into(),
            State::Refused => "The Agent advises against installing this bundle. Nothing was installed.".into(),
            State::NotReviewed => match &self.went_on_by {
                Some(by) => format!("This bundle was not reviewed ({}). {by} went on without a review.", self.why),
                None => format!("This bundle was not reviewed: {}.", self.why),
            },
            State::NotAsked => "No review: this bundle was imported from a file.".into(),
        }
    }

    /// Whether the review has the Agent's reading to show.
    #[must_use]
    pub const fn read(&self) -> bool {
        matches!(self.state, State::Reviewed | State::Refused)
    }
}

/// An install the box stopped, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    /// The bundle's name and version, as the box read them.
    pub bundle: String,
    /// The box's reason, in its words.
    pub error: String,
    pub review: BundleReview,
    /// What was asked, kept so "Install without a review" asks for
    /// exactly the same bundle and not what the fields say by now.
    pub index: String,
    pub name: String,
}

impl Stopped {
    /// Whether "Install without a review" is the way on (no review, not a
    /// refusal).
    #[must_use]
    pub fn can_go_on(&self) -> bool {
        self.review.state == State::NotReviewed
    }
}

/// The install form and what became of the last install.
#[derive(Debug, Default)]
pub struct Install {
    /// The marketplace's index URL.
    pub index: String,
    /// The bundle's name.
    pub name: String,
    /// A request is on its way.
    pub busy: bool,
    pub stopped: Option<Stopped>,
    /// Why nothing was sent or the box refused for another reason.
    pub error: Option<String>,
    /// The proposal the last install made.
    pub installed: Option<u64>,
}

impl Install {
    /// Whether anything about the last install is to be shown.
    #[must_use]
    pub const fn has_result(&self) -> bool {
        self.stopped.is_some() || self.error.is_some() || self.installed.is_some()
    }

    /// Forgets the last result.
    pub fn dismiss(&mut self) {
        self.stopped = None;
        self.error = None;
        self.installed = None;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn verdict() -> Json {
        json!({
            "summary": "Sounds the horn while start is held.",
            "reads": ["start"], "writes": ["horn"], "devices": ["rio"], "access": [],
            "at_start": [], "undeclared": [], "risk": "medium", "refuse": false,
            "reason": "Check the horn's wiring.",
        })
    }

    #[test]
    fn a_review_reads_in_the_boxs_shape_and_each_state_has_its_own_headline() {
        let r = BundleReview::parse(&json!({ "state": "reviewed", "verdict": verdict() })).unwrap();
        assert_eq!(r.state, State::Reviewed);
        assert_eq!(r.risk.as_deref(), Some("medium"));
        assert_eq!(r.lists.len(), 6);
        assert_eq!(r.lists[1], ("Drives", vec!["horn".to_string()]));
        assert!(r.headline().contains("still applies in full"));
        assert!(r.read());
        let r = BundleReview::parse(&json!({ "state": "refused", "verdict": verdict() })).unwrap();
        assert!(r.headline().contains("advises against"));
        let r =
            BundleReview::parse(&json!({ "state": "not_reviewed", "why": "the Agent could not be reached" }))
                .unwrap();
        assert!(!r.read());
        assert!(r.headline().contains("could not be reached"));
        let r = BundleReview::parse(
            &json!({ "state": "not_reviewed", "why": "x", "went_on_by": "engineer (PIN)" }),
        )
        .unwrap();
        assert!(r.headline().contains("engineer (PIN) went on"));
        let r = BundleReview::parse(&json!({ "state": "not_asked", "why": "file" })).unwrap();
        assert!(r.headline().contains("imported from a file"));
    }

    #[test]
    fn a_review_the_screen_does_not_know_is_none() {
        for j in [
            json!(null),
            json!({}),
            json!({ "state": "approved", "verdict": verdict() }),
            json!({ "state": "reviewed" }),
            json!({ "state": "refused", "verdict": "no" }),
            json!({ "state": 3 }),
        ] {
            assert_eq!(BundleReview::parse(&j), None, "{j}");
        }
    }

    #[test]
    fn the_agents_text_is_plain_text_bounded_and_without_hidden_characters() {
        let mut v = verdict();
        v["reason"] = json!("<b>bold</b> **x** [y](javascript:z)\u{202e}evil\u{0}");
        v["summary"] = json!("x".repeat(5000));
        let r = BundleReview::parse(&json!({ "state": "refused", "verdict": v })).unwrap();
        // Markup stays as the characters it is; nothing renders it.
        assert!(
            r.reason.starts_with("<b>bold</b> **x** [y](javascript:z)"),
            "{}",
            r.reason
        );
        assert!(!r.reason.contains('\u{202e}') && !r.reason.contains('\u{0}'));
        assert!(
            r.summary.chars().count() <= MAX_TEXT + 1,
            "{}",
            r.summary.chars().count()
        );
        assert!(r.summary.ends_with('…'));
    }

    #[test]
    fn only_a_missing_review_has_a_way_on() {
        let stop = |state: &str| Stopped {
            bundle: "horn-kit 1.0.0".into(),
            error: "e".into(),
            review: BundleReview::parse(&json!({ "state": state, "why": "w", "verdict": verdict() }))
                .unwrap(),
            index: "i".into(),
            name: "n".into(),
        };
        assert!(stop("not_reviewed").can_go_on());
        assert!(!stop("refused").can_go_on(), "a refusal has no go-on");
    }
}
