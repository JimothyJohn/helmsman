//! Help with the words of an instruction (Nick, 2026-11-22: "seems to
//! have a lot of programming issues so I might need some prompting
//! assistance", then picked help in the tab over a prompt rework): the
//! I/O names an instruction spells another way, each with a fix one
//! click applies. All of it is worked out here: nothing is sent. (The
//! example requests it also offered went on 2026-11-23: Nick found them
//! confusing in the tab.)

/// One input or output of the program, spelled as its source writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Io {
    pub name: String,
    pub input: bool,
    /// BOOL (a button, a switch, a lamp); else a number or a time.
    pub boolean: bool,
}

/// A name the instruction spells another way: `from`, as typed, is
/// `to`, as declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub from: String,
    pub to: String,
}

/// At most this many fixes are offered at once.
pub const FIXES: usize = 3;

/// The names `draft` spells another way than `io` declares them: a
/// declared `GreenLamp` or `green_lamp` typed as "green lamp", and a
/// typed identifier (`Motr_1`) that is no declared name but is close to
/// one. ST names ignore case, so case alone is never a fix.
#[must_use]
pub fn fixes(draft: &str, io: &[Io]) -> Vec<Fix> {
    let mut out: Vec<Fix> = Vec::new();
    let mut push = |f: Fix| {
        if !out.iter().any(|g| g.from == f.from) {
            out.push(f);
        }
    };
    let words = spans(draft, |c| c.is_ascii_alphanumeric());
    for p in io {
        let parts = parts(&p.name);
        if parts.len() < 2 {
            continue;
        }
        for w in words.windows(parts.len()) {
            if w.iter()
                .zip(&parts)
                .all(|(s, part)| draft[s.0..s.1].eq_ignore_ascii_case(part))
            {
                let typed = &draft[w[0].0..w[parts.len() - 1].1];
                // Only spaces or hyphens between: "green lamp", not
                // "green, lamp" or "green and lamp".
                let between = w.windows(2).all(|g| {
                    let gap = &draft[g[0].1..g[1].0];
                    !gap.is_empty() && gap.chars().all(|c| c == ' ' || c == '-' || c == '_')
                });
                if between && !typed.eq_ignore_ascii_case(&p.name) {
                    push(Fix {
                        from: typed.to_string(),
                        to: p.name.clone(),
                    });
                }
            }
        }
    }
    for s in spans(draft, |c| c.is_ascii_alphanumeric() || c == '_') {
        let typed = &draft[s.0..s.1];
        if !looks_like_a_name(typed) || io.iter().any(|p| p.name.eq_ignore_ascii_case(typed)) {
            continue;
        }
        let key = squash(typed);
        let near = io
            .iter()
            .map(|p| (distance(&key, &squash(&p.name)), p))
            .filter(|(d, _)| *d <= 2 && *d * 3 <= key.len())
            .min_by_key(|(d, _)| *d);
        if let Some((_, p)) = near {
            push(Fix {
                from: typed.to_string(),
                to: p.name.clone(),
            });
        }
    }
    out.truncate(FIXES);
    out
}

/// `draft` with every `fix.from` made `fix.to`, where it stands as a
/// whole word (never inside a longer one).
#[must_use]
pub fn apply(draft: &str, fix: &Fix) -> String {
    let mut out = String::with_capacity(draft.len());
    let mut rest = draft;
    while let Some(at) = rest.find(fix.from.as_str()) {
        let end = at + fix.from.len();
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let before = rest[..at].chars().next_back().is_some_and(word);
        let after = rest[end..].chars().next().is_some_and(word);
        out.push_str(&rest[..at]);
        out.push_str(if before || after { &fix.from } else { &fix.to });
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `upper` (a name as the parser holds it, upper case) as `src` first
/// spells it, else as it is.
#[must_use]
pub fn spelled(src: &str, upper: &str) -> String {
    spans(src, |c| c.is_ascii_alphanumeric() || c == '_')
        .into_iter()
        .map(|(a, b)| &src[a..b])
        .find(|w| w.eq_ignore_ascii_case(upper))
        .unwrap_or(upper)
        .to_string()
}

/// The byte spans of the runs of `keep` in `text`.
fn spans(text: &str, keep: impl Fn(char) -> bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = None;
    for (i, c) in text.char_indices() {
        match (keep(c), from) {
            (true, None) => from = Some(i),
            (false, Some(a)) => {
                out.push((a, i));
                from = None;
            }
            _ => {}
        }
    }
    if let Some(a) = from {
        out.push((a, text.len()));
    }
    out
}

/// The words of an identifier, lower case: `GreenLamp`, `green_lamp`
/// and `GREEN_LAMP` are green, lamp; `StartPB` start, pb; `Motor2`
/// motor, 2.
fn parts(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = name.chars().collect();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = i.checked_sub(1).map(|k| chars[k]);
        let next = chars.get(i + 1);
        let boundary = prev.is_some_and(|p| {
            (p.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (p.is_ascii_digit() != c.is_ascii_digit() && p != '_')
                || (p.is_ascii_uppercase()
                    && c.is_ascii_uppercase()
                    && next.is_some_and(char::is_ascii_lowercase))
        });
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A typed word that reads as a variable's name: an underscore, a digit
/// after a letter, or a capital after a small letter.
fn looks_like_a_name(w: &str) -> bool {
    let b = w.as_bytes();
    w.len() >= 3
        && b[0].is_ascii_alphabetic()
        && (w.contains('_')
            || b.windows(2).any(|p| {
                (p[0].is_ascii_alphabetic() && p[1].is_ascii_digit())
                    || (p[0].is_ascii_lowercase() && p[1].is_ascii_uppercase())
            }))
}

/// Lower case, no underscores: what two spellings of a name share.
fn squash(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Edits from `a` to `b` (Levenshtein).
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io(name: &str, input: bool, boolean: bool) -> Io {
        Io {
            name: name.into(),
            input,
            boolean,
        }
    }

    fn lamp() -> Vec<Io> {
        vec![
            io("StartPB", true, true),
            io("stop_pb", true, true),
            io("Speed", true, false),
            io("GreenLamp", false, true),
            io("RED_LAMP", false, true),
        ]
    }

    #[test]
    fn a_name_typed_as_words_is_offered_as_declared() {
        let f = fixes("turn the green lamp on when start pb is pressed", &lamp());
        assert_eq!(
            f,
            [
                Fix {
                    from: "start pb".into(),
                    to: "StartPB".into()
                },
                Fix {
                    from: "green lamp".into(),
                    to: "GreenLamp".into()
                },
            ]
        );
        let f = fixes("Red-Lamp flashes", &lamp());
        assert_eq!(f[0].to, "RED_LAMP");
    }

    #[test]
    fn case_alone_or_the_name_itself_is_never_a_fix() {
        for draft in [
            "turn greenlamp on",
            "turn GREENLAMP on",
            "RED_LAMP on",
            "red_lamp on",
            "speed above 5",
            "",
        ] {
            assert_eq!(fixes(draft, &lamp()), [], "{draft:?}");
        }
    }

    #[test]
    fn words_apart_are_not_a_name() {
        for draft in ["green, lamp", "green and lamp", "the lamp is green"] {
            assert_eq!(fixes(draft, &lamp()), [], "{draft:?}");
        }
    }

    #[test]
    fn a_misspelt_identifier_is_offered_its_nearest_name() {
        let f = fixes("when StratPB is pressed set Green_Lmp", &lamp());
        assert_eq!(
            f,
            [
                Fix {
                    from: "StratPB".into(),
                    to: "StartPB".into()
                },
                Fix {
                    from: "Green_Lmp".into(),
                    to: "GreenLamp".into()
                },
            ]
        );
        // Far from every name: perhaps a new variable, left alone.
        assert_eq!(fixes("add a Counter_1 for parts", &lamp()), []);
        // Plain words are not names, however close.
        assert_eq!(fixes("speeds", &lamp()), []);
    }

    #[test]
    fn hostile_drafts_do_not_panic_and_offer_few_fixes() {
        let many = "green lamp ".repeat(500);
        assert_eq!(fixes(&many, &lamp()).len(), 1);
        let drafts = [
            "é green lamp ü",
            "____",
            "a_",
            "\u{0}\u{7f}",
            "绿 lamp",
            "green\u{a0}lamp",
        ];
        for d in drafts {
            let _ = fixes(d, &lamp());
        }
        let names: Vec<Io> = (0..20)
            .map(|k| io(&format!("Out_Lamp_{k}"), false, true))
            .collect();
        assert!(fixes("Out_Lmp_1 Out_Lmp_2 Out_Lmp_3 Out_Lmp_4 Out_Lmp_5", &names).len() <= FIXES);
    }

    #[test]
    fn a_fix_replaces_whole_words_only() {
        let fix = Fix {
            from: "green lamp".into(),
            to: "GreenLamp".into(),
        };
        assert_eq!(
            apply("the green lamp, then green lamps; green lamp", &fix),
            "the GreenLamp, then green lamps; GreenLamp"
        );
        let fix = Fix {
            from: "StratPB".into(),
            to: "StartPB".into(),
        };
        assert_eq!(apply("StratPB or xStratPB", &fix), "StartPB or xStratPB");
        assert_eq!(apply("", &fix), "");
    }

    #[test]
    fn a_name_is_spelled_as_the_source_first_writes_it() {
        let src = "PROGRAM p\nVAR_INPUT StartPB : BOOL; END_VAR\nstartpb := TRUE;\n";
        assert_eq!(spelled(src, "STARTPB"), "StartPB");
        assert_eq!(spelled(src, "GONE"), "GONE");
        assert_eq!(spelled("XSTARTPB", "STARTPB"), "STARTPB", "whole words only");
    }

    #[test]
    fn an_identifier_splits_into_its_words() {
        assert_eq!(parts("GreenLamp"), ["green", "lamp"]);
        assert_eq!(parts("GREEN_LAMP"), ["green", "lamp"]);
        assert_eq!(parts("StartPB"), ["start", "pb"]);
        assert_eq!(parts("PBStart"), ["pb", "start"]);
        assert_eq!(parts("Motor2"), ["motor", "2"]);
        assert_eq!(parts("__x__"), ["x"]);
        assert_eq!(parts(""), Vec::<String>::new());
    }
}
