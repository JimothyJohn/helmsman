//! The features manifest, `docs/FEATURES.md`: every surface a user
//! reaches (an API route, a canvas tab, an agent profile the box asks
//! the relay for) under the feature it belongs to, with the journey that
//! proves the feature works (docs/PARITY.md). The parity run walks the
//! journeys on dev and live; the coverage tests (helmsman-plc
//! `tests/features.rs`, helmsman-canvas `features_tests.rs`) hold the
//! manifest to what the code has, both ways, so a new surface cannot
//! ship without a journey and a removed one cannot linger.
//!
//! A feature is one row of the manifest's table:
//!
//! ```text
//! | Feature | Journey | Surfaces |
//! |---|---|---|
//! | Sign in | `sign-in` | `GET /api/login`, `POST /api/login`, `tab:files` |
//! ```
//!
//! A surface is `METHOD PATH` (the path exactly as the box's `GET /api`
//! index writes it, query included), `tab:KEY` (the key a pane or bottom
//! tab saves in a layout) or `profile:ROLE` (a `--relay-ROLE-agent` flag
//! of the box; `--relay-agent` is `chat`). Rows outside the table, and
//! any line that is not a table row, are prose.

use std::collections::BTreeSet;
use std::fmt;

/// One surface a user reaches.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Surface {
    Route { method: String, path: String },
    Tab(String),
    Profile(String),
}

impl fmt::Display for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Route { method, path } => write!(f, "{method} {path}"),
            Self::Tab(k) => write!(f, "tab:{k}"),
            Self::Profile(r) => write!(f, "profile:{r}"),
        }
    }
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH"];

impl Surface {
    /// One surface as the manifest writes it, without its backticks.
    ///
    /// # Errors
    /// The text is no surface.
    pub fn parse(s: &str) -> Result<Self, String> {
        let key = |k: &str| {
            !k.is_empty()
                && k.len() <= 32
                && k.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if let Some(k) = s.strip_prefix("tab:") {
            return if key(k) {
                Ok(Self::Tab(k.into()))
            } else {
                Err(format!("`{s}`: a tab key is 1 to 32 of a-z 0-9 -"))
            };
        }
        if let Some(r) = s.strip_prefix("profile:") {
            return if key(r) {
                Ok(Self::Profile(r.into()))
            } else {
                Err(format!("`{s}`: a profile role is 1 to 32 of a-z 0-9 -"))
            };
        }
        let Some((method, path)) = s.split_once(' ') else {
            return Err(format!("`{s}`: not METHOD PATH, tab:KEY or profile:ROLE"));
        };
        if !METHODS.contains(&method) {
            return Err(format!("`{s}`: {method} is not one of {}", METHODS.join(" ")));
        }
        if !(path.starts_with("/api") || path == "/mcp")
            || path.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(format!(
                "`{s}`: the path starts /api (or is /mcp) and has no spaces"
            ));
        }
        Ok(Self::Route {
            method: method.into(),
            path: path.into(),
        })
    }

    /// The role a `--relay-*-agent` flag names: `--relay-agent` is the
    /// chat profile, `--relay-explain-agent` is `explain`.
    #[must_use]
    pub fn profile_of_flag(flag: &str) -> Option<Self> {
        let mid = flag.strip_prefix("--relay")?.strip_suffix("-agent")?;
        match mid {
            "" => Some(Self::Profile("chat".into())),
            m => Some(Self::Profile(m.strip_prefix('-')?.into())),
        }
    }
}

/// One row of the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feature {
    pub name: String,
    pub journey: String,
    pub surfaces: Vec<Surface>,
    /// Its line in the file, for messages.
    pub line: usize,
}

/// The parsed manifest.
#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub features: Vec<Feature>,
}

/// The cells of a table row: `| a | b |` is `["a", "b"]`.
fn cells(line: &str) -> Option<Vec<&str>> {
    let t = line.trim();
    let inner = t.strip_prefix('|')?.strip_suffix('|')?;
    Some(inner.split('|').map(str::trim).collect())
}

/// The text of one backticked span, or the cell when it has none.
fn ticked(cell: &str) -> Result<Vec<&str>, String> {
    let mut out = Vec::new();
    let mut rest = cell;
    while let Some(i) = rest.find('`') {
        let after = &rest[i + 1..];
        let Some(j) = after.find('`') else {
            return Err(format!("an unclosed ` in `{cell}`"));
        };
        out.push(&after[..j]);
        rest = &after[j + 1..];
    }
    Ok(out)
}

impl Manifest {
    /// Read the manifest's table: the first one whose header is
    /// `Feature | Journey | Surfaces`.
    ///
    /// # Errors
    /// Every problem, each with its line: a row with the wrong number of
    /// cells, a journey that is not one backticked id, a surface that does
    /// not parse, a journey or surface named twice, a feature with no
    /// surface, no table at all.
    pub fn parse(text: &str) -> Result<Self, Vec<String>> {
        let mut problems = Vec::new();
        let mut features = Vec::new();
        let mut journeys = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut in_table = false;
        let mut found = false;
        for (i, line) in text.lines().enumerate() {
            let n = i + 1;
            let Some(row) = cells(line) else {
                if in_table {
                    break;
                }
                continue;
            };
            if !in_table {
                if row == ["Feature", "Journey", "Surfaces"] {
                    in_table = true;
                    found = true;
                }
                continue;
            }
            if row
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':')))
            {
                continue;
            }
            let [name, journey, surfaces] = row.as_slice() else {
                problems.push(format!("line {n}: a row has 3 cells, this one {}", row.len()));
                continue;
            };
            let journey = match ticked(journey).as_deref() {
                Ok([j]) if Surface::parse(&format!("tab:{j}")).is_ok() => (*j).to_string(),
                _ => {
                    problems.push(format!(
                        "line {n}: the journey is one `id` of a-z 0-9 -, not `{journey}`"
                    ));
                    continue;
                }
            };
            if !journeys.insert(journey.clone()) {
                problems.push(format!("line {n}: journey `{journey}` is named twice"));
            }
            let mut list = Vec::new();
            match ticked(surfaces) {
                Err(e) => problems.push(format!("line {n}: {e}")),
                Ok(spans) => {
                    for s in spans {
                        match Surface::parse(s) {
                            Ok(sf) if !seen.insert(sf.clone()) => {
                                problems.push(format!("line {n}: `{sf}` is in two features"));
                            }
                            Ok(sf) => list.push(sf),
                            Err(e) => problems.push(format!("line {n}: {e}")),
                        }
                    }
                }
            }
            if list.is_empty() {
                problems.push(format!("line {n}: feature {name} names no surface"));
            }
            features.push(Feature {
                name: (*name).to_string(),
                journey,
                surfaces: list,
                line: n,
            });
        }
        if !found {
            problems.push("no table with the header | Feature | Journey | Surfaces |".into());
        }
        if problems.is_empty() {
            Ok(Self { features })
        } else {
            Err(problems)
        }
    }

    /// Every surface the manifest names.
    #[must_use]
    pub fn surfaces(&self) -> BTreeSet<Surface> {
        self.features
            .iter()
            .flat_map(|f| f.surfaces.iter().cloned())
            .collect()
    }

    /// The manifest held to what the code has, for the surfaces `kind`
    /// picks: each surface the code has and the manifest lacks, then each
    /// the manifest names and the code lacks. Both empty is covered.
    #[must_use]
    pub fn coverage(
        &self,
        have: &BTreeSet<Surface>,
        kind: impl Fn(&Surface) -> bool,
    ) -> (Vec<Surface>, Vec<Surface>) {
        let named: BTreeSet<Surface> = self.surfaces().into_iter().filter(&kind).collect();
        let have: BTreeSet<Surface> = have.iter().filter(|s| kind(s)).cloned().collect();
        (
            have.difference(&named).cloned().collect(),
            named.difference(&have).cloned().collect(),
        )
    }

    /// The feature a surface is under.
    #[must_use]
    pub fn feature_of(&self, s: &Surface) -> Option<&Feature> {
        self.features.iter().find(|f| f.surfaces.contains(s))
    }
}

/// A message for a coverage test that failed: what to add and what to
/// remove.
#[must_use]
pub fn explain(file: &str, missing: &[Surface], stale: &[Surface]) -> String {
    let mut m = String::new();
    if !missing.is_empty() {
        m.push_str(&format!(
            "{file} has no feature for these (add each to a row, with its journey; docs/PARITY.md):\n"
        ));
        for s in missing {
            m.push_str(&format!("  `{s}`\n"));
        }
    }
    if !stale.is_empty() {
        m.push_str(&format!(
            "{file} names these and the code no longer has them (remove them):\n"
        ));
        for s in stale {
            m.push_str(&format!("  `{s}`\n"));
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "Prose first.\n\n| Feature | Journey | Surfaces |\n|---|---|---|\n";

    fn parse(rows: &str) -> Result<Manifest, Vec<String>> {
        Manifest::parse(&format!("{HEAD}{rows}"))
    }

    #[test]
    fn a_row_reads_as_its_surfaces() {
        let m = parse(
            "| Sign in | `sign-in` | `GET /api/login`, `POST /api/login`, `tab:files`, `profile:chat` |\n",
        )
        .unwrap();
        let f = &m.features[0];
        assert_eq!(
            (f.name.as_str(), f.journey.as_str(), f.line),
            ("Sign in", "sign-in", 5)
        );
        assert_eq!(
            f.surfaces,
            vec![
                Surface::Route {
                    method: "GET".into(),
                    path: "/api/login".into()
                },
                Surface::Route {
                    method: "POST".into(),
                    path: "/api/login".into()
                },
                Surface::Tab("files".into()),
                Surface::Profile("chat".into()),
            ]
        );
    }

    #[test]
    fn the_table_ends_at_the_first_line_that_is_no_row() {
        let m = parse("| A | `a` | `GET /api/a` |\n\n| B | `b` | `GET /api/b` |\n").unwrap();
        assert_eq!(m.features.len(), 1);
    }

    #[test]
    fn every_problem_is_reported_with_its_line() {
        let e = parse(concat!(
            "| A | `a` | `GET /api/a` |\n",
            "| B | `a` | `GET /api/a` |\n",
            "| C | c | `GET /api/c` |\n",
            "| D | `d` | `FETCH /api/d`, `GET api/d`, `tab:Dev ices`, `profile:` |\n",
            "| E | `e` | nothing here |\n",
            "| F | `f` |\n",
            "| G | `g` | `GET /api/g |\n",
            "| H | `h` | `GET /api/h\u{1b}[2J` |\n",
        ))
        .unwrap_err();
        let want = [
            "line 6: journey `a` is named twice",
            "line 6: `GET /api/a` is in two features",
            "line 7: the journey",
            "line 8: `FETCH /api/d`",
            "line 8: `GET api/d`",
            "line 8: `tab:Dev ices`",
            "line 8: `profile:`",
            "line 9: feature E names no surface",
            "line 10: a row has 3 cells, this one 2",
            "line 11: an unclosed `",
            "line 12: `GET /api/h",
        ];
        for w in want {
            assert!(
                e.iter().any(|p| p.starts_with(w)),
                "no problem starting {w:?} in {e:#?}"
            );
        }
    }

    #[test]
    fn a_file_without_the_table_is_refused() {
        let e = Manifest::parse("| Name | Journey |\n|---|---|\n| A | `a` |\n").unwrap_err();
        assert_eq!(
            e,
            vec!["no table with the header | Feature | Journey | Surfaces |".to_string()]
        );
        assert!(Manifest::parse("").is_err());
    }

    #[test]
    fn coverage_names_what_is_missing_and_what_is_stale_for_one_kind() {
        let m = parse("| A | `a` | `GET /api/a`, `GET /api/gone`, `tab:io` |\n").unwrap();
        let have: BTreeSet<Surface> = ["GET /api/a", "POST /api/new", "tab:code"]
            .iter()
            .map(|s| Surface::parse(s).unwrap())
            .collect();
        let (missing, stale) = m.coverage(&have, |s| matches!(s, Surface::Route { .. }));
        assert_eq!(missing, vec![Surface::parse("POST /api/new").unwrap()]);
        assert_eq!(stale, vec![Surface::parse("GET /api/gone").unwrap()]);
        let (missing, stale) = m.coverage(&have, |s| matches!(s, Surface::Tab(_)));
        assert_eq!(
            (missing, stale),
            (vec![Surface::Tab("code".into())], vec![Surface::Tab("io".into())])
        );
    }

    #[test]
    fn a_relay_flag_names_its_role() {
        assert_eq!(
            Surface::profile_of_flag("--relay-agent"),
            Some(Surface::Profile("chat".into()))
        );
        assert_eq!(
            Surface::profile_of_flag("--relay-explain-agent"),
            Some(Surface::Profile("explain".into()))
        );
        assert_eq!(Surface::profile_of_flag("--relay-url"), None);
        assert_eq!(Surface::profile_of_flag("--relayx-agent"), None);
    }
}
