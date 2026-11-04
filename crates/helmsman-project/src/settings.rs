//! Writing PROGRAM scan rates into a project's `project.settings` (whose
//! grammar the box's `helmsman_plc::project::settings` parses): what an
//! imported L5X's periodic tasks become, the same in the box and in the
//! browser's simulation.

use std::fmt::Write as _;

/// `settings` with a `program NAME scan_ms=N` for each `(NAME, N)` of
/// `scans`, and a note for each one it left alone.
///
/// - No `program NAME` line (names match case-insensitively, as ST's
///   do): one is added at the end.
/// - A line without `scan_ms`: `scan_ms=N` is added to it, its windows
///   kept.
/// - A line with a `scan_ms` already: kept as the engineer wrote it, with
///   a note when it differs from `N`.
///
/// Every other line is kept as it was. The text is unchanged when there is
/// nothing to do.
#[must_use]
pub fn set_scans(settings: &str, scans: &[(String, u32)]) -> (String, Vec<String>) {
    let mut lines: Vec<String> = settings.lines().map(str::to_string).collect();
    let mut notes = Vec::new();
    let mut changed = false;
    for (name, ms) in scans {
        let at = lines.iter().position(|l| {
            let mut w = l.split_whitespace();
            w.next() == Some("program") && w.next().is_some_and(|n| n.eq_ignore_ascii_case(name))
        });
        match at {
            None => {
                lines.push(format!("program {name} scan_ms={ms}"));
                changed = true;
            }
            Some(k) => {
                let had = lines[k]
                    .split_whitespace()
                    .find_map(|w| w.strip_prefix("scan_ms="))
                    .map(str::to_string);
                match had {
                    None => {
                        let line = lines[k].trim_end().to_string();
                        lines[k] = format!("{line} scan_ms={ms}");
                        changed = true;
                    }
                    Some(v) if v == ms.to_string() => {}
                    Some(v) => notes.push(format!(
                        "project.settings keeps program {name} scan_ms={v}; its Logix task runs every {ms} ms"
                    )),
                }
            }
        }
    }
    if !changed {
        return (settings.to_string(), notes);
    }
    let mut out = String::with_capacity(settings.len() + 32 * scans.len());
    for l in &lines {
        let _ = writeln!(out, "{l}");
    }
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::set_scans;

    fn scans(s: &[(&str, u32)]) -> Vec<(String, u32)> {
        s.iter().map(|(n, ms)| ((*n).to_string(), *ms)).collect()
    }

    #[test]
    fn a_missing_line_is_added_and_a_bare_one_gets_its_scan() {
        let (text, notes) = set_scans(
            "# mine\nauthor Nick\nprogram fast q=0+2\n",
            &scans(&[("Fast", 10), ("Slow", 500)]),
        );
        assert_eq!(
            text,
            "# mine\nauthor Nick\nprogram fast q=0+2 scan_ms=10\nprogram Slow scan_ms=500\n"
        );
        assert!(notes.is_empty(), "{notes:?}");
        let (text, _) = set_scans("", &scans(&[("T", 1)]));
        assert_eq!(text, "program T scan_ms=1\n");
    }

    #[test]
    fn an_engineers_scan_is_kept_and_named() {
        let src = "program Fast scan_ms=20\nprogram Slow  scan_ms=500 i=0+1";
        let (text, notes) = set_scans(src, &scans(&[("FAST", 10), ("slow", 500)]));
        assert_eq!(text, src, "unchanged, byte for byte");
        assert_eq!(
            notes,
            ["project.settings keeps program FAST scan_ms=20; its Logix task runs every 10 ms"]
        );
    }

    #[test]
    fn nothing_to_do_leaves_the_text_alone() {
        let src = "author A\r\nparam p x real 1";
        assert_eq!(set_scans(src, &[]), (src.to_string(), Vec::new()));
        // A param line naming the PROGRAM is not its program line, and a
        // longer name is another PROGRAM.
        let (text, _) = set_scans("param T x real 1\nprogram TT scan_ms=5\n", &scans(&[("T", 7)]));
        assert_eq!(
            text,
            "param T x real 1\nprogram TT scan_ms=5\nprogram T scan_ms=7\n"
        );
    }
}
