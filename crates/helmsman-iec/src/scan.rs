//! Structured Text as an editor sees it: every byte of the source in a
//! piece, comments and spacing included, whether or not the text
//! compiles. The lexer drops comments and stops at the first error; a
//! highlighter, a formatter and a fold gutter can do neither.
//!
//! [`scan`] cuts the text into [`Piece`]s that tile it exactly. [`blocks`]
//! reads the block structure off them (a POU, a VAR section, an IF): what
//! a formatter indents by and an editor folds.

#[cfg(not(feature = "std"))]
use crate::prelude::*;

/// What a piece of source is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Spaces and tabs (and a carriage return on its own).
    Space,
    /// One line end: `\n` or `\r\n`.
    Newline,
    /// `(* *)`, `/* */`, `//` to the end of its line, or a `{ }` pragma.
    /// A block comment may hold line ends.
    Comment,
    /// A name or a keyword: letters, digits and `_`.
    Word,
    /// A number, `T#500ms`, `INT#5`, `16#FF`, `Color#Red`.
    Literal,
    /// A directly represented variable: `%IX0.1`.
    Direct,
    /// An operator or a punctuation mark: `:=`, `;`, `(`.
    Op,
    /// Anything else (a quote, a character outside ASCII): never valid,
    /// kept so the pieces still tile the text.
    Other,
}

/// A run of the source: `src[start..end]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
}

impl Piece {
    /// Its text in `src` (the source it was scanned from).
    #[must_use]
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }

    /// Code: not spacing, a line end or a comment.
    #[must_use]
    pub const fn code(&self) -> bool {
        !matches!(self.kind, Kind::Space | Kind::Newline | Kind::Comment)
    }
}

/// What a [`Kind::Word`] is, by its upper-cased text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    Keyword,
    /// An elementary type's name.
    Type,
    /// `TRUE` or `FALSE`.
    Bool,
    Name,
}

const KEYWORDS: &[&str] = &[
    "AND",
    "ARRAY",
    "AT",
    "BY",
    "CASE",
    "CONSTANT",
    "CONTINUE",
    "DO",
    "ELSE",
    "ELSIF",
    "END_CASE",
    "END_FOR",
    "END_FUNCTION",
    "END_FUNCTION_BLOCK",
    "END_IF",
    "END_PROGRAM",
    "END_REPEAT",
    "END_STRUCT",
    "END_TYPE",
    "END_VAR",
    "END_WHILE",
    "EXIT",
    "FOR",
    "FUNCTION",
    "FUNCTION_BLOCK",
    "IF",
    "MOD",
    "NON_RETAIN",
    "NOT",
    "OF",
    "OR",
    "PROGRAM",
    "REPEAT",
    "RETAIN",
    "RETURN",
    "STRUCT",
    "THEN",
    "TO",
    "TYPE",
    "UNTIL",
    "VAR",
    "VAR_ACCESS",
    "VAR_CONFIG",
    "VAR_EXTERNAL",
    "VAR_GLOBAL",
    "VAR_INPUT",
    "VAR_IN_OUT",
    "VAR_OUTPUT",
    "VAR_TEMP",
    "WHILE",
    "XOR",
];

/// What the word `upper` (already upper-cased) is.
#[must_use]
pub fn word(upper: &str) -> Word {
    if upper == "TRUE" || upper == "FALSE" {
        Word::Bool
    } else if KEYWORDS.binary_search(&upper).is_ok() {
        Word::Keyword
    } else if crate::Ty::from_name(upper).is_some() {
        Word::Type
    } else {
        Word::Name
    }
}

const fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `src` in pieces that tile it: the first starts at 0, each starts where
/// the one before ends, the last ends at `src.len()`. Never fails: text
/// the lexer refuses comes out as [`Kind::Other`] or as a comment that
/// runs to the end.
#[must_use]
pub fn scan(src: &str) -> Vec<Piece> {
    let b = src.as_bytes();
    let at = |i: usize| b.get(i).copied();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let c = b[i];
        let kind = match (c, at(i + 1)) {
            (b'\n', _) => {
                i += 1;
                Kind::Newline
            }
            (b'\r', Some(b'\n')) => {
                i += 2;
                Kind::Newline
            }
            (b' ' | b'\t' | b'\r' | 0x0b | 0x0c, _) => {
                while matches!(at(i), Some(b' ' | b'\t' | 0x0b | 0x0c))
                    || (at(i) == Some(b'\r') && at(i + 1) != Some(b'\n'))
                {
                    i += 1;
                }
                Kind::Space
            }
            (b'(', Some(b'*')) | (b'/', Some(b'*')) => {
                let close = if c == b'(' { b')' } else { b'/' };
                i += 2;
                loop {
                    match (at(i), at(i + 1)) {
                        (Some(b'*'), Some(e)) if e == close => {
                            i += 2;
                            break;
                        }
                        (Some(_), _) => i += 1,
                        (None, _) => break,
                    }
                }
                Kind::Comment
            }
            (b'/', Some(b'/')) => {
                while at(i).is_some_and(|c| c != b'\n' && c != b'\r') {
                    i += 1;
                }
                Kind::Comment
            }
            (b'{', _) => {
                while let Some(c) = at(i) {
                    i += 1;
                    if c == b'}' {
                        break;
                    }
                }
                Kind::Comment
            }
            (b'%', Some(n)) if n.is_ascii_alphanumeric() => {
                i += 1;
                while at(i).is_some_and(is_word)
                    || (at(i) == Some(b'.') && at(i + 1).is_some_and(|c| c.is_ascii_alphanumeric()))
                {
                    i += 1;
                }
                Kind::Direct
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                while at(i).is_some_and(is_word) {
                    i += 1;
                }
                if at(i) == Some(b'#') {
                    // A typed literal: TIME#-5s, INT#16#FF, Color#Red.
                    i += 1;
                    if matches!(at(i), Some(b'+' | b'-')) {
                        i += 1;
                    }
                    i = literal_tail(b, i);
                    Kind::Literal
                } else {
                    Kind::Word
                }
            }
            _ if c.is_ascii_digit() => {
                i = literal_tail(b, i);
                Kind::Literal
            }
            (b':', Some(b'=')) | (b'=', Some(b'>')) | (b'.', Some(b'.')) | (b'*', Some(b'*')) => {
                i += 2;
                Kind::Op
            }
            (b'<', Some(b'>' | b'=')) | (b'>', Some(b'=')) => {
                i += 2;
                Kind::Op
            }
            (
                b':' | b';' | b',' | b'.' | b'(' | b')' | b'[' | b']' | b'+' | b'-' | b'*' | b'/' | b'&'
                | b'=' | b'<' | b'>',
                _,
            ) => {
                i += 1;
                Kind::Op
            }
            _ => {
                // One whole character, whatever its width.
                i += 1;
                while i < b.len() && !src.is_char_boundary(i) {
                    i += 1;
                }
                Kind::Other
            }
        };
        out.push(Piece { kind, start, end: i });
    }
    out
}

/// The end of a number or of what follows a `#`: digits, letters and `_`,
/// a `#` (a base), a `.` between two digits, an exponent's sign.
fn literal_tail(b: &[u8], mut i: usize) -> usize {
    let at = |i: usize| b.get(i).copied();
    loop {
        match at(i) {
            Some(c) if is_word(c) => {
                // 1e-3, 1E+5: the sign belongs to the number.
                let exp = matches!(c, b'e' | b'E')
                    && matches!(at(i + 1), Some(b'+' | b'-'))
                    && at(i + 2).is_some_and(|d| d.is_ascii_digit())
                    && i > 0
                    && b[i - 1].is_ascii_digit();
                i += if exp { 2 } else { 1 };
            }
            Some(b'#') if at(i + 1).is_some_and(is_word) => i += 1,
            Some(b'.')
                if i > 0 && b[i - 1].is_ascii_digit() && at(i + 1).is_some_and(|d| d.is_ascii_digit()) =>
            {
                i += 1;
            }
            _ => return i,
        }
    }
}

/// What a block is: what opens it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// PROGRAM, FUNCTION_BLOCK or FUNCTION.
    Pou,
    /// Any VAR section.
    Var,
    Type,
    Struct,
    If,
    Case,
    For,
    While,
    Repeat,
}

impl BlockKind {
    /// The block `upper` opens, if it is an opening keyword.
    #[must_use]
    pub fn opened_by(upper: &str) -> Option<Self> {
        Some(match upper {
            "PROGRAM" | "FUNCTION_BLOCK" | "FUNCTION" => Self::Pou,
            "TYPE" => Self::Type,
            "STRUCT" => Self::Struct,
            "IF" => Self::If,
            "CASE" => Self::Case,
            "FOR" => Self::For,
            "WHILE" => Self::While,
            "REPEAT" => Self::Repeat,
            v if v == "VAR" || v.starts_with("VAR_") => Self::Var,
            _ => return None,
        })
    }

    /// The block `upper` closes, if it is a closing keyword.
    #[must_use]
    pub fn closed_by(upper: &str) -> Option<Self> {
        Some(match upper {
            "END_PROGRAM" | "END_FUNCTION_BLOCK" | "END_FUNCTION" => Self::Pou,
            "END_VAR" => Self::Var,
            "END_TYPE" => Self::Type,
            "END_STRUCT" => Self::Struct,
            "END_IF" => Self::If,
            "END_CASE" => Self::Case,
            "END_FOR" => Self::For,
            "END_WHILE" => Self::While,
            "END_REPEAT" => Self::Repeat,
            _ => return None,
        })
    }
}

/// A block of the source, by 1-based lines: the line its opening keyword
/// is on and the line its closing keyword is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub first: u32,
    pub last: u32,
}

/// Every block of `src` that closes, outermost first where they start on
/// the same line, otherwise in the order they open. A block left open (a
/// program still being typed) is not one. An `END_` with nothing of its
/// kind open is skipped; one that closes an outer block closes the blocks
/// left open inside it too.
#[must_use]
pub fn blocks(src: &str) -> Vec<Block> {
    let mut open: Vec<(BlockKind, u32, usize)> = Vec::new();
    let mut out: Vec<(usize, Block)> = Vec::new();
    let mut line = 1u32;
    let mut order = 0usize;
    for p in scan(src) {
        match p.kind {
            Kind::Newline => line += 1,
            Kind::Comment => {
                line += u32::try_from(p.text(src).matches('\n').count()).unwrap_or(0);
            }
            Kind::Word => {
                let w = p.text(src).to_ascii_uppercase();
                if let Some(kind) = BlockKind::opened_by(&w) {
                    open.push((kind, line, order));
                    order += 1;
                } else if let Some(kind) = BlockKind::closed_by(&w)
                    && let Some(k) = open.iter().rposition(|(o, _, _)| *o == kind)
                {
                    let (kind, first, n) = open[k];
                    open.truncate(k);
                    out.push((
                        n,
                        Block {
                            kind,
                            first,
                            last: line,
                        },
                    ));
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out.into_iter().map(|(_, b)| b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(Kind, &str)> {
        scan(src)
            .iter()
            .filter(|p| p.kind != Kind::Space)
            .map(|p| (p.kind, p.text(src)))
            .collect()
    }

    #[test]
    fn pieces_tile_the_text_whatever_it_is() {
        for src in [
            "",
            "x := 1;",
            "(* never closed",
            "{ pragma",
            "a\r\nb\rc\n",
            "é ' \" @ ` 🙂 x",
            "T#1h30m 16#FF 1.5e-3 1..10 INT#-5 %IX0.1 %QW2 x.y",
            "/* a */ // b\n(* c\nd *) e",
            "\u{feff}PROGRAM p\nEND_PROGRAM",
        ] {
            let ps = scan(src);
            let mut at = 0;
            for p in &ps {
                assert_eq!(p.start, at, "{src:?}");
                assert!(p.end > p.start, "{src:?}");
                assert!(src.is_char_boundary(p.end), "{src:?}");
                at = p.end;
            }
            assert_eq!(at, src.len(), "{src:?}");
        }
    }

    #[test]
    fn literals_ranges_and_addresses_are_whole_pieces() {
        use Kind::{Comment, Direct, Literal, Newline, Op, Word};
        assert_eq!(
            kinds("t := T#1h30m + TIME#-5s;"),
            [
                (Word, "t"),
                (Op, ":="),
                (Literal, "T#1h30m"),
                (Op, "+"),
                (Literal, "TIME#-5s"),
                (Op, ";")
            ]
        );
        assert_eq!(
            kinds("a[1..10] 1.5e-3 16#FF 2e5 x.y 1.x"),
            [
                (Word, "a"),
                (Op, "["),
                (Literal, "1"),
                (Op, ".."),
                (Literal, "10"),
                (Op, "]"),
                (Literal, "1.5e-3"),
                (Literal, "16#FF"),
                (Literal, "2e5"),
                (Word, "x"),
                (Op, "."),
                (Word, "y"),
                (Literal, "1"),
                (Op, "."),
                (Word, "x"),
            ]
        );
        assert_eq!(
            kinds("m AT %IX0.1 : BOOL; // in\n(* a\nb *)"),
            [
                (Word, "m"),
                (Word, "AT"),
                (Direct, "%IX0.1"),
                (Op, ":"),
                (Word, "BOOL"),
                (Op, ";"),
                (Comment, "// in"),
                (Newline, "\n"),
                (Comment, "(* a\nb *)"),
            ]
        );
        assert_eq!(kinds("a-1 a - -1 x**2 a<>b a<=b c=>d"), {
            let w = |s| (Word, s);
            let o = |s| (Op, s);
            let l = |s| (Literal, s);
            [
                w("a"),
                o("-"),
                l("1"),
                w("a"),
                o("-"),
                o("-"),
                l("1"),
                w("x"),
                o("**"),
                l("2"),
                w("a"),
                o("<>"),
                w("b"),
                w("a"),
                o("<="),
                w("b"),
                w("c"),
                o("=>"),
                w("d"),
            ]
        });
    }

    #[test]
    fn words_are_keywords_types_booleans_or_names() {
        assert_eq!(word("END_IF"), Word::Keyword);
        assert_eq!(word("VAR_IN_OUT"), Word::Keyword);
        assert_eq!(word("LREAL"), Word::Type);
        assert_eq!(word("TIME"), Word::Type);
        assert_eq!(word("TRUE"), Word::Bool);
        assert_eq!(word("TON"), Word::Name);
        assert_eq!(word("LAMP"), Word::Name);
        // The list is searched by halves: it must be sorted.
        assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn blocks_are_the_pous_sections_and_statements_that_close() {
        let src = "PROGRAM p\nVAR\n  x : INT;\nEND_VAR\nIF x > 0 THEN\n  CASE x OF\n    1: x := 2;\n  END_CASE;\nEND_IF;\nEND_PROGRAM\n";
        let b = |kind, first, last| Block { kind, first, last };
        assert_eq!(
            blocks(src),
            [
                b(BlockKind::Pou, 1, 10),
                b(BlockKind::Var, 2, 4),
                b(BlockKind::If, 5, 9),
                b(BlockKind::Case, 6, 8),
            ]
        );
        // Left open, or closed twice: only what closes is a block, and a
        // keyword in a comment is not one.
        assert_eq!(blocks("IF a THEN\n(* END_IF *)\nx := 1;\n"), []);
        assert_eq!(
            blocks("END_IF;\nWHILE a DO\nEND_WHILE;\nEND_WHILE;"),
            [b(BlockKind::While, 2, 3)]
        );
        // An outer END closes what was left open inside it.
        assert_eq!(
            blocks("PROGRAM p\nIF a THEN\nEND_PROGRAM\n"),
            [b(BlockKind::Pou, 1, 3)]
        );
        // A comment's line ends count.
        assert_eq!(
            blocks("(* a\nb *)\nREPEAT\nUNTIL x END_REPEAT;"),
            [b(BlockKind::Repeat, 3, 4)]
        );
    }
}
