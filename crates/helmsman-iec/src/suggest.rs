//! "Did you mean": the known names closest to a misspelled one.

#[cfg(not(feature = "std"))]
use crate::prelude::*;

/// Edits (insert, delete, replace, swap two neighbours) between `a` and `b`.
fn distance(a: &[u8], b: &[u8]) -> usize {
    // Optimal string alignment: three rows of the Levenshtein table.
    let mut before: Vec<usize> = Vec::new();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(before[j - 2] + 1);
            }
        }
        before = core::mem::replace(&mut prev, row);
    }
    prev[b.len()]
}

/// The candidates within a typo of `name` (one edit for a short name, two
/// for one of six letters or more), nearest first, at most three, as
/// " (did you mean X?)" or " (did you mean X or Y?)"; empty when none is
/// near. Names of one or two letters get no suggestion: every other short
/// name is near them.
pub fn hint<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    let max = match name.len() {
        0..=2 => return String::new(),
        3..=5 => 1,
        _ => 2,
    };
    let mut near: Vec<(usize, &str)> = candidates
        .into_iter()
        .filter(|c| *c != name && !c.starts_with("__"))
        .map(|c| (distance(name.as_bytes(), c.as_bytes()), c))
        .filter(|(d, _)| *d <= max)
        .collect();
    near.sort_unstable();
    near.dedup();
    let Some(&(best, _)) = near.first() else {
        return String::new();
    };
    let names: Vec<&str> = near
        .iter()
        .filter(|(d, _)| *d == best)
        .take(3)
        .map(|(_, c)| *c)
        .collect();
    let list = match names.as_slice() {
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        [] => return String::new(),
    };
    format!(" (did you mean {list}?)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        let d = |a: &str, b: &str| distance(a.as_bytes(), b.as_bytes());
        assert_eq!(d("ELSEIF", "ELSIF"), 1);
        assert_eq!(d("WHLIE", "WHILE"), 1);
        assert_eq!(d("TOM", "TON"), 1);
        assert_eq!(d("", "ABC"), 3);
        assert_eq!(d("ABC", "ABC"), 0);
        assert_eq!(d("CA", "ABC"), 3);
    }

    #[test]
    fn hints() {
        assert_eq!(hint("TOM", ["TON", "TOF", "CTU"]), " (did you mean TOF or TON?)");
        assert_eq!(hint("CUONT", ["COUNT", "A"]), " (did you mean COUNT?)");
        assert_eq!(hint("B", ["A"]), "");
        assert_eq!(hint("XYZZY", ["COUNT"]), "");
        assert_eq!(hint("TOM", ["__TOM2", "TOM"]), "");
    }
}
