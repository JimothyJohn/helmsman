//! Tokens of Structured Text. Keywords and identifiers are case-insensitive
//! (identifiers are folded to upper case here); comments are `(* *)`,
//! `/* */` and `//`; pragmas `{ }` are skipped.

use std::fmt;

/// 1-based source position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// Identifier or keyword, upper-cased. Keywords are recognised by the
    /// parser, so `END_IF` and `x` are both `Ident`.
    Ident(String),
    /// Integer literal; `prefix` is a type name for `INT#5`. Only typed
    /// literals (`INT#-5`) can be negative here; `-5` is unary minus.
    Int {
        value: i128,
        prefix: Option<String>,
    },
    Real {
        value: f64,
        prefix: Option<String>,
    },
    /// `TRUE`/`FALSE`, with an optional `BOOL#` prefix.
    Bool(bool),
    /// Directly represented variable: `%IX0.1`, `%QW2`, `%MD4`.
    Direct(String),
    Assign, // :=
    Colon,
    Semi,
    Comma,
    Dot,
    DotDot,
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Power, // **
    Slash,
    Amp, // &
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ident(s) => write!(f, "{s}"),
            Self::Int { value, .. } => write!(f, "{value}"),
            Self::Real { value, .. } => write!(f, "{value}"),
            Self::Bool(b) => write!(f, "{}", if *b { "TRUE" } else { "FALSE" }),
            Self::Direct(d) => write!(f, "%{d}"),
            Self::Eof => write!(f, "end of input"),
            other => {
                let s = match other {
                    Self::Assign => ":=",
                    Self::Colon => ":",
                    Self::Semi => ";",
                    Self::Comma => ",",
                    Self::Dot => ".",
                    Self::DotDot => "..",
                    Self::LParen => "(",
                    Self::RParen => ")",
                    Self::Plus => "+",
                    Self::Minus => "-",
                    Self::Star => "*",
                    Self::Power => "**",
                    Self::Slash => "/",
                    Self::Amp => "&",
                    Self::Eq => "=",
                    Self::Ne => "<>",
                    Self::Lt => "<",
                    Self::Le => "<=",
                    Self::Gt => ">",
                    _ => ">=",
                };
                write!(f, "{s}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub pos: Pos,
    pub msg: String,
}

struct Lexer<'a> {
    src: &'a [u8],
    i: usize,
    pos: Pos,
}

impl Lexer<'_> {
    fn peek(&self, k: usize) -> Option<u8> {
        self.src.get(self.i + k).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek(0)?;
        self.i += 1;
        if c == b'\n' {
            self.pos.line += 1;
            self.pos.col = 1;
        } else {
            self.pos.col += 1;
        }
        Some(c)
    }

    fn err(pos: Pos, msg: impl Into<String>) -> LexError {
        LexError { pos, msg: msg.into() }
    }

    /// Skips whitespace, comments and pragmas.
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(c), _) if c.is_ascii_whitespace() => {
                    self.bump();
                }
                (Some(b'('), Some(b'*')) => self.block_comment(b')')?,
                (Some(b'/'), Some(b'*')) => self.block_comment(b'/')?,
                (Some(b'/'), Some(b'/')) => {
                    while let Some(c) = self.peek(0) {
                        if c == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                (Some(b'{'), _) => {
                    let start = self.pos;
                    self.bump();
                    loop {
                        match self.bump() {
                            Some(b'}') => break,
                            Some(_) => {}
                            None => return Err(Self::err(start, "unterminated pragma")),
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// `(* ... *)` or `/* ... */`, not nested; `close` is the byte after `*`.
    fn block_comment(&mut self, close: u8) -> Result<(), LexError> {
        let start = self.pos;
        self.bump();
        self.bump();
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b'*'), Some(c)) if c == close => {
                    self.bump();
                    self.bump();
                    return Ok(());
                }
                (Some(_), _) => {
                    self.bump();
                }
                (None, _) => return Err(Self::err(start, "unterminated comment")),
            }
        }
    }

    fn word(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek(0) {
            if c.is_ascii_alphanumeric() || c == b'_' {
                s.push(char::from(c.to_ascii_uppercase()));
                self.bump();
            } else {
                break;
            }
        }
        s
    }

    /// Digits with `_` separators, in `radix`. Returns the digits without
    /// separators; rejects leading, trailing or doubled `_`.
    fn digits(&mut self, radix: u32, start: Pos) -> Result<String, LexError> {
        let mut s = String::new();
        let mut last_us = true;
        while let Some(c) = self.peek(0) {
            if c == b'_' {
                if last_us {
                    return Err(Self::err(start, "misplaced '_' in number"));
                }
                last_us = true;
            } else if char::from(c).is_digit(radix) {
                s.push(char::from(c));
                last_us = false;
            } else {
                break;
            }
            self.bump();
        }
        if s.is_empty() {
            return Err(Self::err(start, "expected digits"));
        }
        if last_us {
            return Err(Self::err(start, "misplaced '_' in number"));
        }
        Ok(s)
    }

    /// A number starting at a digit; `prefix` is the `TYPE#` already read.
    fn number(&mut self, start: Pos, prefix: Option<String>) -> Result<Tok, LexError> {
        let int = self.digits(10, start)?;
        // Based literal: 16#FF, 2#1010, 8#17.
        if self.peek(0) == Some(b'#') {
            let radix = match int.as_str() {
                "2" => 2,
                "8" => 8,
                "16" => 16,
                _ => return Err(Self::err(start, format!("unsupported base {int}#"))),
            };
            self.bump();
            let d = self.digits(radix, start)?;
            let value =
                i128::from_str_radix(&d, radix).map_err(|_| Self::err(start, "integer literal too large"))?;
            return Ok(Tok::Int { value, prefix });
        }
        let is_real = self.peek(0) == Some(b'.') && self.peek(1).is_some_and(|c| c.is_ascii_digit());
        if !is_real && !matches!(self.peek(0), Some(b'e' | b'E')) {
            let value = int
                .parse::<i128>()
                .map_err(|_| Self::err(start, "integer literal too large"))?;
            return Ok(Tok::Int { value, prefix });
        }
        let mut text = int;
        if is_real {
            self.bump();
            text.push('.');
            text.push_str(&self.digits(10, start)?);
        }
        if matches!(self.peek(0), Some(b'e' | b'E')) {
            self.bump();
            text.push('e');
            if let Some(sign @ (b'+' | b'-')) = self.peek(0) {
                self.bump();
                text.push(char::from(sign));
            }
            text.push_str(&self.digits(10, start)?);
        }
        let value: f64 = text.parse().map_err(|_| Self::err(start, "bad real literal"))?;
        if !value.is_finite() {
            return Err(Self::err(start, "real literal out of range"));
        }
        Ok(Tok::Real { value, prefix })
    }

    fn next(&mut self) -> Result<(Tok, Pos), LexError> {
        self.skip_trivia()?;
        let start = self.pos;
        let Some(c) = self.peek(0) else {
            return Ok((Tok::Eof, start));
        };
        if c.is_ascii_alphabetic() || c == b'_' {
            let w = self.word();
            if self.peek(0) == Some(b'#') {
                self.bump();
                return self.typed_literal(w, start).map(|t| (t, start));
            }
            let tok = match w.as_str() {
                "TRUE" => Tok::Bool(true),
                "FALSE" => Tok::Bool(false),
                _ => Tok::Ident(w),
            };
            return Ok((tok, start));
        }
        if c.is_ascii_digit() {
            return self.number(start, None).map(|t| (t, start));
        }
        if c == b'%' {
            self.bump();
            let mut s = String::new();
            while let Some(c) = self.peek(0) {
                if c.is_ascii_alphanumeric()
                    || (c == b'.' && self.peek(1).is_some_and(|d| d.is_ascii_digit()))
                {
                    s.push(char::from(c.to_ascii_uppercase()));
                    self.bump();
                } else {
                    break;
                }
            }
            if s.is_empty() {
                return Err(Self::err(start, "expected an address after '%'"));
            }
            return Ok((Tok::Direct(s), start));
        }
        self.bump();
        let two = |l: &mut Self, next: u8, yes: Tok, no: Tok| {
            if l.peek(0) == Some(next) {
                l.bump();
                yes
            } else {
                no
            }
        };
        let tok = match c {
            b':' => two(self, b'=', Tok::Assign, Tok::Colon),
            b';' => Tok::Semi,
            b',' => Tok::Comma,
            b'.' => two(self, b'.', Tok::DotDot, Tok::Dot),
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => two(self, b'*', Tok::Power, Tok::Star),
            b'/' => Tok::Slash,
            b'&' => Tok::Amp,
            b'=' => Tok::Eq,
            b'<' => match self.peek(0) {
                Some(b'=') => {
                    self.bump();
                    Tok::Le
                }
                Some(b'>') => {
                    self.bump();
                    Tok::Ne
                }
                _ => Tok::Lt,
            },
            b'>' => two(self, b'=', Tok::Ge, Tok::Gt),
            _ => {
                return Err(Self::err(
                    start,
                    format!("unexpected character {:?}", char::from(c)),
                ));
            }
        };
        Ok((tok, start))
    }

    /// After `TYPE#`: a number, or TRUE/FALSE/0/1 for `BOOL#`.
    fn typed_literal(&mut self, ty: String, start: Pos) -> Result<Tok, LexError> {
        let neg = if self.peek(0) == Some(b'-') {
            self.bump();
            true
        } else {
            false
        };
        if ty == "BOOL" && !neg {
            if self.peek(0).is_some_and(|c| c.is_ascii_alphabetic()) {
                return match self.word().as_str() {
                    "TRUE" => Ok(Tok::Bool(true)),
                    "FALSE" => Ok(Tok::Bool(false)),
                    _ => Err(Self::err(start, "BOOL# needs TRUE, FALSE, 0 or 1")),
                };
            }
            return match self.number(start, None)? {
                Tok::Int { value: 0, .. } => Ok(Tok::Bool(false)),
                Tok::Int { value: 1, .. } => Ok(Tok::Bool(true)),
                _ => Err(Self::err(start, "BOOL# needs TRUE, FALSE, 0 or 1")),
            };
        }
        if !self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            return Err(Self::err(start, format!("expected a number after {ty}#")));
        }
        match self.number(start, Some(ty))? {
            Tok::Int { value, prefix } if neg => Ok(Tok::Int {
                value: -value,
                prefix,
            }),
            Tok::Real { value, prefix } if neg => Ok(Tok::Real {
                value: -value,
                prefix,
            }),
            t => Ok(t),
        }
    }
}

/// Tokenises `src`. Stops at the first error.
pub fn lex(src: &str) -> Result<Vec<(Tok, Pos)>, LexError> {
    let mut l = Lexer {
        src: src.as_bytes(),
        i: 0,
        pos: Pos { line: 1, col: 1 },
    };
    let mut out = Vec::new();
    loop {
        let (t, p) = l.next()?;
        let eof = t == Tok::Eof;
        out.push((t, p));
        if eof {
            return Ok(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn keywords_fold_and_comments_vanish() {
        assert_eq!(
            toks("if (* c *) x /* d */ then // e\n {pragma} End_If"),
            vec![
                Tok::Ident("IF".into()),
                Tok::Ident("X".into()),
                Tok::Ident("THEN".into()),
                Tok::Ident("END_IF".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn numbers() {
        let int = |v| Tok::Int {
            value: v,
            prefix: None,
        };
        assert_eq!(
            toks("1_000 16#FF 2#1010 8#17"),
            vec![int(1000), int(255), int(10), int(15), Tok::Eof]
        );
        assert_eq!(
            toks("1.5 2.0E3 1e-2"),
            vec![
                Tok::Real {
                    value: 1.5,
                    prefix: None
                },
                Tok::Real {
                    value: 2000.0,
                    prefix: None
                },
                Tok::Real {
                    value: 0.01,
                    prefix: None
                },
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("INT#5 BOOL#1 bool#false"),
            vec![
                Tok::Int {
                    value: 5,
                    prefix: Some("INT".into())
                },
                Tok::Bool(true),
                Tok::Bool(false),
                Tok::Eof
            ]
        );
        // `1..5` is a range, not a real.
        assert_eq!(toks("1..5"), vec![int(1), Tok::DotDot, int(5), Tok::Eof]);
    }

    #[test]
    fn operators_and_addresses() {
        assert_eq!(
            toks(":= ** <> <= >= %IX0.1 %qw2"),
            vec![
                Tok::Assign,
                Tok::Power,
                Tok::Ne,
                Tok::Le,
                Tok::Ge,
                Tok::Direct("IX0.1".into()),
                Tok::Direct("QW2".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn errors_carry_positions() {
        assert_eq!(lex("x := 1__0").unwrap_err().pos, Pos { line: 1, col: 6 });
        assert_eq!(lex("\n  (* open").unwrap_err().pos, Pos { line: 2, col: 3 });
        assert!(lex("x := $").unwrap_err().msg.contains("unexpected character"));
        assert!(lex("3#12").unwrap_err().msg.contains("base"));
        assert!(
            lex("99999999999999999999999999999999999999999")
                .unwrap_err()
                .msg
                .contains("too large")
        );
    }
}
