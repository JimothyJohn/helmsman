//! The approval policy: what it takes to change a running box.
//!
//! Every change is a step from the change set the box runs to another
//! ([`crate::store::ChangeSet`]), and [`classify`] puts the step in a
//! [`Tier`] from the two change sets alone, never from what a proposal
//! says about itself. The [`Policy`] then names, per tier, the least
//! [`Role`] whose PIN applies it and whether a sim replay of exactly that
//! step must be attached first. [`pin`] keeps the PINs and the lockout.
//!
//! **The floor.** The product's rules are enforced here in code:
//!
//! | tier | least role | sim replay |
//! | --- | --- | --- |
//! | `param` | operator | no (applies live; undo is one rollback) |
//! | `logic` | engineer | yes (the preview) |
//! | `output-affecting` | engineer | yes |
//!
//! plus a PIN lockout after at most 5 wrong PINs for at least 30 s. A
//! policy file ([`Policy::parse`]) may only make these stricter; a file
//! that tries to loosen any of them is refused at load, naming the line.
//!
//! **Policy file** (`helmsman-policy 1`), one rule per line, `#` comments:
//!
//! ```text
//! helmsman-policy 1
//! # tier            least role   replay
//! param             engineer     replay=no
//! logic             engineer     replay=yes
//! output-affecting  integrator   replay=yes
//! lockout           3 60         # wrong PINs before lockout, first lockout in seconds
//! min-replay        120          # a replay must cover at least this many seconds
//! ```
//!
//! Every line is optional and may appear once; what is left out is the
//! floor. A higher tier may not ask less than a lower one (the file is
//! refused, not reinterpreted).

pub mod account;
pub mod classify;
pub mod pin;

use std::fmt;
use std::time::Duration;

pub use classify::{Class, classify, classify_with};

/// How far a change reaches, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Only param values changed, each within the range its manifest
    /// declares.
    Param,
    /// Programs or manifests changed, but what can drive an output did
    /// not: the same output signals, `%Q` bytes and safe values.
    Logic,
    /// Anything that can move an output.
    OutputAffecting,
}

impl Tier {
    pub const ALL: [Self; 3] = [Self::Param, Self::Logic, Self::OutputAffecting];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Param => "param",
            Self::Logic => "logic",
            Self::OutputAffecting => "output-affecting",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who is at the box, from least to most trusted. A role may do whatever
/// a lesser one may.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Views the box and asks the agent.
    Operator,
    /// Approves changes and edits ST.
    Engineer,
    /// Installs modules and sets the policy.
    Integrator,
}

impl Role {
    pub const ALL: [Self; 3] = [Self::Operator, Self::Engineer, Self::Integrator];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Engineer => "engineer",
            Self::Integrator => "integrator",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one tier takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The least role whose PIN applies it.
    pub role: Role,
    /// A sim replay of exactly this step, over the recorded window, must be
    /// attached.
    pub replay: bool,
}

impl Rule {
    /// Whether `self` asks at least what `other` does.
    #[must_use]
    pub fn at_least(self, other: Self) -> bool {
        self.role >= other.role && (self.replay || !other.replay)
    }
}

/// Wrong PINs before the API stops checking them, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lockout {
    /// Consecutive wrong PINs that lock entry.
    pub attempts: u32,
    /// The first lockout; each further wrong PIN doubles it, up to
    /// [`MAX_LOCKOUT`].
    pub first: Duration,
}

/// The longest a lockout grows to.
pub const MAX_LOCKOUT: Duration = Duration::from_secs(60 * 60);

/// What a box enforces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub param: Rule,
    pub logic: Rule,
    pub output: Rule,
    pub lockout: Lockout,
    /// The least window a replay must cover to count.
    pub min_replay: Duration,
}

/// The product's rules; no policy is looser.
pub const FLOOR: Policy = Policy {
    param: Rule {
        role: Role::Operator,
        replay: false,
    },
    logic: Rule {
        role: Role::Engineer,
        replay: true,
    },
    output: Rule {
        role: Role::Engineer,
        replay: true,
    },
    lockout: Lockout {
        attempts: 5,
        first: Duration::from_secs(30),
    },
    min_replay: Duration::ZERO,
};

impl Default for Policy {
    fn default() -> Self {
        FLOOR
    }
}

/// The first line of a policy file.
pub const HEADER: &str = "helmsman-policy 1";

/// Largest policy file, in bytes.
pub const MAX_FILE: usize = 64 * 1024;

impl Policy {
    /// The rule for `tier`.
    #[must_use]
    pub const fn rule(&self, tier: Tier) -> Rule {
        match tier {
            Tier::Param => self.param,
            Tier::Logic => self.logic,
            Tier::OutputAffecting => self.output,
        }
    }

    fn rule_mut(&mut self, tier: Tier) -> &mut Rule {
        match tier {
            Tier::Param => &mut self.param,
            Tier::Logic => &mut self.logic,
            Tier::OutputAffecting => &mut self.output,
        }
    }

    /// Every way `self` is looser than [`FLOOR`] or a higher tier asks less
    /// than a lower one; empty when it may be enforced.
    #[must_use]
    pub fn below_floor(&self) -> Vec<String> {
        let mut errs = Vec::new();
        for t in Tier::ALL {
            let (r, f) = (self.rule(t), FLOOR.rule(t));
            if r.role < f.role {
                errs.push(format!("{t}: role {} is below the floor ({})", r.role, f.role));
            }
            if f.replay && !r.replay {
                errs.push(format!("{t}: the floor requires a sim replay"));
            }
        }
        for w in Tier::ALL.windows(2) {
            if !self.rule(w[1]).at_least(self.rule(w[0])) {
                errs.push(format!("{} asks less than {}", w[1], w[0]));
            }
        }
        let l = self.lockout;
        if l.attempts == 0 || l.attempts > FLOOR.lockout.attempts {
            errs.push(format!(
                "lockout: {} wrong PINs; the floor allows 1..={}",
                l.attempts, FLOOR.lockout.attempts
            ));
        }
        if l.first < FLOOR.lockout.first || l.first > MAX_LOCKOUT {
            errs.push(format!(
                "lockout: first lockout {} s; the floor allows {}..={} s",
                l.first.as_secs(),
                FLOOR.lockout.first.as_secs(),
                MAX_LOCKOUT.as_secs()
            ));
        }
        errs
    }

    /// A policy file (see the [module docs](self)), held to the floor.
    ///
    /// # Errors
    /// Every problem with its line number (1-based; 0 for the file as a
    /// whole), including each way the file would loosen the floor.
    pub fn parse(src: &str) -> Result<Self, Vec<(usize, String)>> {
        if src.len() > MAX_FILE {
            return Err(vec![(0, format!("larger than {MAX_FILE} bytes"))]);
        }
        let mut p = FLOOR;
        let mut errs = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut header = false;
        for (n, raw) in src.lines().enumerate() {
            let n = n + 1;
            let line = raw.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if !raw.bytes().all(|b| b == b'\t' || (b' '..=b'~').contains(&b)) {
                errs.push((n, "only printable ASCII".into()));
                continue;
            }
            if !header {
                if line != HEADER {
                    errs.push((n, format!("expected {HEADER:?} first")));
                }
                header = true;
                continue;
            }
            let words: Vec<&str> = line.split_whitespace().collect();
            let key = words[0];
            if seen.contains(&key) {
                errs.push((n, format!("{key} given twice")));
                continue;
            }
            seen.push(key);
            if let Err(e) = p.line(key, &words[1..]) {
                errs.push((n, e));
                continue;
            }
            for e in p.below_floor() {
                if e.starts_with(key) {
                    errs.push((n, e));
                }
            }
        }
        if !header {
            errs.push((0, format!("empty; expected {HEADER:?}")));
        }
        if errs.is_empty() {
            // Cross-tier problems name no single line.
            errs.extend(p.below_floor().into_iter().map(|e| (0, e)));
        }
        if errs.is_empty() { Ok(p) } else { Err(errs) }
    }

    fn line(&mut self, key: &str, args: &[&str]) -> Result<(), String> {
        let secs = |s: &str| {
            s.parse::<u64>()
                .ok()
                .filter(|&n| n <= 7 * 24 * 60 * 60)
                .map(Duration::from_secs)
                .ok_or_else(|| format!("{s:?}: expected whole seconds"))
        };
        if let Some(t) = Tier::parse(key) {
            let [role, replay] = args else {
                return Err(format!("expected: {key} ROLE replay=yes|no"));
            };
            let role = Role::parse(role)
                .ok_or_else(|| format!("{role:?}: expected operator, engineer or integrator"))?;
            let replay = match *replay {
                "replay=yes" => true,
                "replay=no" => false,
                r => return Err(format!("{r:?}: expected replay=yes or replay=no")),
            };
            *self.rule_mut(t) = Rule { role, replay };
            return Ok(());
        }
        match (key, args) {
            ("lockout", [attempts, first]) => {
                let attempts = attempts
                    .parse::<u32>()
                    .map_err(|_| format!("{attempts:?}: expected a count"))?;
                self.lockout = Lockout {
                    attempts,
                    first: secs(first)?,
                };
                Ok(())
            }
            ("lockout", _) => Err("expected: lockout ATTEMPTS SECONDS".into()),
            ("min-replay", [s]) => {
                self.min_replay = secs(s)?;
                Ok(())
            }
            ("min-replay", _) => Err("expected: min-replay SECONDS".into()),
            _ => Err(format!(
                "{key:?}: expected param, logic, output-affecting, lockout or min-replay"
            )),
        }
    }

    /// The file [`Policy::parse`] reads back as `self`.
    #[must_use]
    pub fn render(&self) -> String {
        use fmt::Write as _;
        let mut s = format!("{HEADER}\n");
        for t in Tier::ALL {
            let r = self.rule(t);
            let _ = writeln!(
                s,
                "{t:<17} {:<10} replay={}",
                r.role.as_str(),
                if r.replay { "yes" } else { "no" }
            );
        }
        let _ = writeln!(
            s,
            "lockout           {} {}",
            self.lockout.attempts,
            self.lockout.first.as_secs()
        );
        let _ = writeln!(s, "min-replay        {}", self.min_replay.as_secs());
        s
    }
}
