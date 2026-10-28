//! The PLC language: one assignment per line (or `;`-separated), evaluated
//! top to bottom every scan. Small on purpose: every index is a constant,
//! checked against the real sizes at compile time, so a program that
//! compiles cannot read or write out of bounds.
//!
//! ```text
//! # comments start with # or //
//! Q[0]      = I[0] + 1            # bytes: I = from the scanner (read-only),
//! Q[1].3    = I[1].0 && !I[1].2   #        Q = to the scanner, M = memory
//! QW[2]     = IW[2] * 2           # 16-bit little-endian words at a byte offset
//! M[0]      = RUN ? M[0] + 1 : 0  # RUN = scanner in run mode; SCAN, MS counters
//! Q[4..12]  = I[4..12]            # copy a range (end exclusive)
//! Q[12..32] = 0                   # fill a range
//! ```
//!
//! Operators, loosest first: `?:`, `||`, `&&`, `|`, `^`, `&`, `== !=`,
//! `< <= > >=`, `<< >>`, `+ -`, `* / %`, unary `- ! ~`. Values are 64-bit
//! signed, arithmetic wraps, comparisons and logic give 0/1, division by
//! zero gives 0 and counts a fault. Stores truncate to the target width.

use std::fmt;

/// Data areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// Bytes from the scanner (the O->T assembly). Read-only.
    I,
    /// Bytes to the scanner (the T->O assembly).
    Q,
    /// Retained memory.
    M,
}

impl Area {
    const fn name(self) -> &'static str {
        match self {
            Self::I => "I",
            Self::Q => "Q",
            Self::M => "M",
        }
    }
}

/// Sizes the program is compiled against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    pub i: usize,
    pub q: usize,
    pub m: usize,
}

impl Sizes {
    const fn of(self, a: Area) -> usize {
        match a {
            Area::I => self.i,
            Area::Q => self.q,
            Area::M => self.m,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    And,
    Or,
    Xor,
    LAnd,
    LOr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Num(i64),
    Byte(Area, usize),
    Bit(Area, usize, u8),
    Word(Area, usize),
    Run,
    Scan,
    Ms,
    Unary(Op, Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stmt {
    Byte(Area, usize, Expr),
    Bit(Area, usize, u8, Expr),
    Word(Area, usize, Expr),
    Copy {
        dst: Area,
        at: usize,
        src: Area,
        from: usize,
        len: usize,
    },
    Fill {
        dst: Area,
        at: usize,
        len: usize,
        value: Expr,
    },
}

/// A compiled program. Cheap to clone; run with [`Program::scan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    stmts: Vec<Stmt>,
    sizes: Sizes,
}

/// Inputs and outputs of one scan.
#[derive(Debug)]
pub struct Scan<'a> {
    pub i: &'a [u8],
    pub q: &'a mut [u8],
    pub m: &'a mut [u8],
    pub run: bool,
    pub scan: u64,
    pub ms: u64,
}

const MAX_SOURCE: usize = 64 * 1024;
const MAX_STATEMENTS: usize = 4096;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(i64),
    Sym(&'static str),
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    col: usize,
}

const SYMS: [&str; 31] = [
    ";", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", ":=", "..", "+", "-", "*", "/", "%", "&", "|", "^",
    "~", "!", "<", ">", "=", "(", ")", "[", "]", "?", ":", ".",
];

fn lex(line: &str, lineno: usize) -> Result<Vec<Token>, CompileError> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let err = |col: usize, msg: String| CompileError {
        line: lineno,
        col,
        msg,
    };
    while i < b.len() {
        let c = b[i];
        let col = i + 1;
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
        } else if c == b'#' || (c == b'/' && b.get(i + 1) == Some(&b'/')) {
            break;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let text: String = line[start..i].chars().filter(|c| *c != '_').collect();
            let lower = text.to_ascii_lowercase();
            let parsed = if let Some(h) = lower.strip_prefix("0x") {
                i64::from_str_radix(h, 16)
            } else if let Some(bin) = lower.strip_prefix("0b") {
                i64::from_str_radix(bin, 2)
            } else {
                lower.parse()
            };
            out.push(Token {
                tok: Tok::Num(parsed.map_err(|_| err(col, format!("bad number {text:?}")))?),
                col,
            });
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident(line[start..i].to_ascii_uppercase()),
                col,
            });
        } else if let Some(s) = SYMS.iter().find(|s| line[i..].starts_with(**s)) {
            out.push(Token {
                tok: Tok::Sym(s),
                col,
            });
            i += s.len();
        } else {
            return Err(err(
                col,
                format!(
                    "unexpected character {:?}",
                    line[i..].chars().next().unwrap_or('?')
                ),
            ));
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    line: usize,
    end_col: usize,
    sizes: Sizes,
    depth: usize,
}

type PResult<T> = Result<T, CompileError>;

impl Parser<'_> {
    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        let col = self.toks.get(self.pos).map_or(self.end_col, |t| t.col);
        Err(CompileError {
            line: self.line,
            col,
            msg: msg.into(),
        })
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    fn eat(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(x)) if *x == s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> PResult<()> {
        if self.eat(s) {
            Ok(())
        } else {
            self.err(format!("expected '{s}'"))
        }
    }

    fn index(&mut self) -> PResult<usize> {
        match self.peek() {
            Some(Tok::Num(n)) => {
                let n = *n;
                self.pos += 1;
                usize::try_from(n).or_else(|_| self.err("index must not be negative"))
            }
            _ => self.err("index must be a constant number"),
        }
    }

    fn check(&self, area: Area, at: usize, width: usize) -> PResult<()> {
        let size = self.sizes.of(area);
        if at.checked_add(width).is_none_or(|end| end > size) {
            let what = if width == 1 {
                format!("{}[{at}]", area.name())
            } else {
                format!("{}W[{at}]", area.name())
            };
            return self.err(format!(
                "{what} is out of range: {} has {size} bytes",
                area.name()
            ));
        }
        Ok(())
    }

    /// `AREA[n]`, `AREA[n].b`, `AREA[a..b]`, or `AREAW[n]` after the name.
    fn reference(&mut self, name: &str) -> PResult<Ref> {
        let (area, word) = match name {
            "I" => (Area::I, false),
            "Q" => (Area::Q, false),
            "M" => (Area::M, false),
            "IW" => (Area::I, true),
            "QW" => (Area::Q, true),
            "MW" => (Area::M, true),
            _ => {
                return self.err(format!(
                    "unknown name {name}; use I, Q, M, IW, QW, MW, RUN, SCAN or MS"
                ));
            }
        };
        self.expect("[")?;
        let at = self.index()?;
        if !word && self.eat("..") {
            let end = self.index()?;
            self.expect("]")?;
            if end <= at {
                return self.err(format!("range {at}..{end} is empty"));
            }
            self.check(area, at, end - at)?;
            return Ok(Ref::Range(area, at, end - at));
        }
        self.expect("]")?;
        if word {
            self.check(area, at, 2)?;
            return Ok(Ref::Word(area, at));
        }
        self.check(area, at, 1)?;
        if self.eat(".") {
            let bit = self.index()?;
            if bit > 7 {
                return self.err("bit index must be 0..=7");
            }
            return Ok(Ref::Bit(area, at, u8::try_from(bit).unwrap_or(0)));
        }
        Ok(Ref::Byte(area, at))
    }

    fn statement(&mut self) -> PResult<Stmt> {
        let Some(Tok::Ident(name)) = self.peek().cloned() else {
            return self.err("a statement starts with Q[..], QW[..], M[..] or MW[..]");
        };
        self.pos += 1;
        let target = self.reference(&name)?;
        if target.area() == Area::I {
            return self.err("I is read-only: it is the data the scanner sends");
        }
        if !(self.eat("=") || self.eat(":=")) {
            return self.err("expected '=' after the target");
        }
        let stmt = match target {
            Ref::Range(dst, at, len) => {
                let save = self.pos;
                if let Some(Tok::Ident(src_name)) = self.peek().cloned() {
                    self.pos += 1;
                    if let Ok(Ref::Range(src, from, n)) = self.reference(&src_name) {
                        if n != len {
                            return self.err(format!("range lengths differ: {len} = {n}"));
                        }
                        if self.pos == self.toks.len() {
                            return Ok(Stmt::Copy {
                                dst,
                                at,
                                src,
                                from,
                                len,
                            });
                        }
                    }
                    self.pos = save;
                }
                Stmt::Fill {
                    dst,
                    at,
                    len,
                    value: self.expr()?,
                }
            }
            Ref::Byte(a, at) => Stmt::Byte(a, at, self.expr()?),
            Ref::Bit(a, at, bit) => Stmt::Bit(a, at, bit, self.expr()?),
            Ref::Word(a, at) => Stmt::Word(a, at, self.expr()?),
        };
        if self.pos != self.toks.len() {
            return self.err("unexpected text after the statement");
        }
        Ok(stmt)
    }

    fn expr(&mut self) -> PResult<Expr> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.err("expression nested too deeply");
        }
        let cond = self.binary(0)?;
        let out = if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            Expr::Ternary(Box::new(cond), Box::new(a), Box::new(b))
        } else {
            cond
        };
        self.depth -= 1;
        Ok(out)
    }

    fn binary(&mut self, level: usize) -> PResult<Expr> {
        const LEVELS: [&[(&str, Op)]; 10] = [
            &[("||", Op::LOr)],
            &[("&&", Op::LAnd)],
            &[("|", Op::Or)],
            &[("^", Op::Xor)],
            &[("&", Op::And)],
            &[("==", Op::Eq), ("!=", Op::Ne)],
            &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
            &[("<<", Op::Shl), (">>", Op::Shr)],
            &[("+", Op::Add), ("-", Op::Sub)],
            &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        'outer: loop {
            for (s, op) in LEVELS[level] {
                if self.eat(s) {
                    let rhs = self.binary(level + 1)?;
                    lhs = Expr::Binary(*op, Box::new(lhs), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn unary(&mut self) -> PResult<Expr> {
        for (s, op) in [("-", Op::Neg), ("!", Op::Not), ("~", Op::BitNot)] {
            if self.eat(s) {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return self.err("expression nested too deeply");
                }
                let e = self.unary()?;
                self.depth -= 1;
                return Ok(Expr::Unary(op, Box::new(e)));
            }
        }
        self.primary()
    }

    fn primary(&mut self) -> PResult<Expr> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Num(n))
            }
            Some(Tok::Sym("(")) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                match name.as_str() {
                    "RUN" => Ok(Expr::Run),
                    "SCAN" => Ok(Expr::Scan),
                    "MS" => Ok(Expr::Ms),
                    _ => match self.reference(&name)? {
                        Ref::Byte(a, at) => Ok(Expr::Byte(a, at)),
                        Ref::Bit(a, at, b) => Ok(Expr::Bit(a, at, b)),
                        Ref::Word(a, at) => Ok(Expr::Word(a, at)),
                        Ref::Range(..) => self.err("a range can only be copied whole: Q[a..b] = I[c..d]"),
                    },
                }
            }
            _ => self.err("expected a number, a name, or '('"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Ref {
    Byte(Area, usize),
    Bit(Area, usize, u8),
    Word(Area, usize),
    Range(Area, usize, usize),
}

impl Ref {
    const fn area(self) -> Area {
        match self {
            Self::Byte(a, ..) | Self::Bit(a, ..) | Self::Word(a, ..) | Self::Range(a, ..) => a,
        }
    }
}

/// Compile `source` against `sizes`. Errors are collected for every bad
/// statement, not just the first.
///
/// # Errors
/// Every compile error, with line and column (1-based).
pub fn compile(source: &str, sizes: Sizes) -> Result<Program, Vec<CompileError>> {
    if source.len() > MAX_SOURCE {
        return Err(vec![CompileError {
            line: 1,
            col: 1,
            msg: format!("program is {} bytes; the limit is {MAX_SOURCE}", source.len()),
        }]);
    }
    let mut stmts = Vec::new();
    let mut errors = Vec::new();
    for (idx, raw) in source.lines().enumerate() {
        let lineno = idx + 1;
        let toks = match lex(raw, lineno) {
            Ok(t) => t,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        for part in toks.split(|t| t.tok == Tok::Sym(";")) {
            if part.is_empty() {
                continue;
            }
            let mut p = Parser {
                toks: part,
                pos: 0,
                line: lineno,
                end_col: raw.len() + 1,
                sizes,
                depth: 0,
            };
            match p.statement() {
                Ok(s) => stmts.push(s),
                Err(e) => errors.push(e),
            }
        }
    }
    if stmts.len() > MAX_STATEMENTS {
        errors.push(CompileError {
            line: 1,
            col: 1,
            msg: format!("{} statements; the limit is {MAX_STATEMENTS}", stmts.len()),
        });
    }
    if errors.is_empty() {
        Ok(Program { stmts, sizes })
    } else {
        Err(errors)
    }
}

impl Program {
    #[must_use]
    pub const fn sizes(&self) -> Sizes {
        self.sizes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.stmts.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stmts.is_empty()
    }

    /// Run every statement once. Returns the number of faults (division by
    /// zero). Panics never: indices were checked at compile time, and the
    /// buffers must match the compiled sizes.
    ///
    /// # Panics
    /// If the buffers are smaller than the sizes the program was compiled for.
    pub fn scan(&self, s: &mut Scan<'_>) -> u64 {
        assert!(
            s.i.len() >= self.sizes.i && s.q.len() >= self.sizes.q && s.m.len() >= self.sizes.m,
            "scan buffers smaller than compiled sizes"
        );
        let mut faults = 0;
        for st in &self.stmts {
            match st {
                Stmt::Byte(a, at, e) => {
                    let v = eval(e, s, &mut faults);
                    area_mut(s, *a)[*at] = low_byte(v);
                }
                Stmt::Bit(a, at, bit, e) => {
                    let v = eval(e, s, &mut faults) != 0;
                    let byte = &mut area_mut(s, *a)[*at];
                    if v {
                        *byte |= 1 << bit;
                    } else {
                        *byte &= !(1 << bit);
                    }
                }
                Stmt::Word(a, at, e) => {
                    let v = low_word(eval(e, s, &mut faults));
                    area_mut(s, *a)[*at..*at + 2].copy_from_slice(&v);
                }
                Stmt::Copy {
                    dst,
                    at,
                    src,
                    from,
                    len,
                } => {
                    let tmp: Vec<u8> = area(s, *src)[*from..*from + *len].to_vec();
                    area_mut(s, *dst)[*at..*at + *len].copy_from_slice(&tmp);
                }
                Stmt::Fill { dst, at, len, value } => {
                    let v = low_byte(eval(value, s, &mut faults));
                    area_mut(s, *dst)[*at..*at + *len].fill(v);
                }
            }
        }
        faults
    }
}

/// Stores truncate to the target width (two's complement): -1 stores 0xFF.
const fn low_byte(v: i64) -> u8 {
    v.to_le_bytes()[0]
}

const fn low_word(v: i64) -> [u8; 2] {
    let b = v.to_le_bytes();
    [b[0], b[1]]
}

/// Shift counts use the low 6 bits, like the hardware.
const fn shift(y: i64) -> u32 {
    (y.to_le_bytes()[0] & 63) as u32
}

fn area<'a>(s: &'a Scan<'_>, a: Area) -> &'a [u8] {
    match a {
        Area::I => s.i,
        Area::Q => s.q,
        Area::M => s.m,
    }
}

fn area_mut<'a>(s: &'a mut Scan<'_>, a: Area) -> &'a mut [u8] {
    match a {
        Area::Q => s.q,
        Area::M => s.m,
        Area::I => unreachable!("the compiler rejects writes to I"),
    }
}

fn eval(expr: &Expr, ctx: &Scan<'_>, faults: &mut u64) -> i64 {
    match expr {
        Expr::Num(n) => *n,
        Expr::Byte(a, at) => i64::from(area(ctx, *a)[*at]),
        Expr::Bit(a, at, b) => i64::from((area(ctx, *a)[*at] >> b) & 1),
        Expr::Word(a, at) => {
            let d = area(ctx, *a);
            i64::from(u16::from_le_bytes([d[*at], d[*at + 1]]))
        }
        Expr::Run => i64::from(ctx.run),
        Expr::Scan => ctx.scan.cast_signed(),
        Expr::Ms => ctx.ms.cast_signed(),
        Expr::Unary(op, x) => {
            let v = eval(x, ctx, faults);
            match op {
                Op::Neg => v.wrapping_neg(),
                Op::Not => i64::from(v == 0),
                _ => !v,
            }
        }
        Expr::Ternary(cond, yes, no) => {
            if eval(cond, ctx, faults) != 0 {
                eval(yes, ctx, faults)
            } else {
                eval(no, ctx, faults)
            }
        }
        Expr::Binary(Op::LAnd, lhs, rhs) => {
            i64::from(eval(lhs, ctx, faults) != 0 && eval(rhs, ctx, faults) != 0)
        }
        Expr::Binary(Op::LOr, lhs, rhs) => {
            i64::from(eval(lhs, ctx, faults) != 0 || eval(rhs, ctx, faults) != 0)
        }
        Expr::Binary(op, lhs, rhs) => {
            let (x, y) = (eval(lhs, ctx, faults), eval(rhs, ctx, faults));
            match op {
                Op::Add => x.wrapping_add(y),
                Op::Sub => x.wrapping_sub(y),
                Op::Mul => x.wrapping_mul(y),
                Op::Div | Op::Rem if y == 0 => {
                    *faults += 1;
                    0
                }
                Op::Div => x.wrapping_div(y),
                Op::Rem => x.wrapping_rem(y),
                Op::Shl => x.wrapping_shl(shift(y)),
                Op::Shr => x.wrapping_shr(shift(y)),
                Op::And => x & y,
                Op::Or => x | y,
                Op::Xor => x ^ y,
                Op::Eq => i64::from(x == y),
                Op::Ne => i64::from(x != y),
                Op::Lt => i64::from(x < y),
                Op::Le => i64::from(x <= y),
                Op::Gt => i64::from(x > y),
                Op::Ge => i64::from(x >= y),
                _ => 0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: Sizes = Sizes { i: 32, q: 32, m: 16 };

    fn run(src: &str, i: &[u8]) -> (Vec<u8>, Vec<u8>, u64) {
        let p = compile(src, SIZES).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        let mut q = vec![0; 32];
        let mut m = vec![0; 16];
        let mut iv = vec![0; 32];
        iv[..i.len()].copy_from_slice(i);
        let faults = p.scan(&mut Scan {
            i: &iv,
            q: &mut q,
            m: &mut m,
            run: true,
            scan: 7,
            ms: 1000,
        });
        (q, m, faults)
    }

    #[test]
    fn bytes_bits_words_and_precedence() {
        let (q, ..) = run(
            "Q[0] = I[0] + 1\nQ[1].3 = I[1].0 && !I[1].2\nQW[2] = IW[2] * 2; Q[4] = 2 + 3 * 4",
            &[9, 1, 0x34, 0x12],
        );
        assert_eq!(q[0], 10);
        assert_eq!(q[1], 0b1000);
        assert_eq!(u16::from_le_bytes([q[2], q[3]]), 0x2468);
        assert_eq!(q[4], 14);
    }

    #[test]
    fn ranges_ternary_counters_and_comments() {
        let src = "# echo\nQ[0..8] = I[0..8]   // copy\nQ[8..16] = 0xAA\nM[0] = RUN ? SCAN : 0\nMW[2] = MS";
        let (q, m, _) = run(src, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&q[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&q[8..16], &[0xAA; 8]);
        assert_eq!(m[0], 7);
        assert_eq!(u16::from_le_bytes([m[2], m[3]]), 1000);
    }

    #[test]
    fn later_statements_see_earlier_writes() {
        let (q, ..) = run("Q[0] = 5\nQ[1] = Q[0] * 2", &[]);
        assert_eq!(&q[..2], &[5, 10]);
    }

    #[test]
    fn division_by_zero_is_a_fault_not_a_crash() {
        let (q, _, faults) = run("Q[0] = 10 / I[0]\nQ[1] = 10 % 0", &[0]);
        assert_eq!(q[0], 0);
        assert_eq!(faults, 2);
    }

    #[test]
    fn compile_errors_have_positions_and_keep_going() {
        let errs = compile("Q[0] = 1\nI[0] = 2\nQ[40] = 1\nQ[0].9 = 1\nQ[0] = (1\nQW[31] = 1\nQ[0..4] = I[0..5]\nfoo = 1\nQ[0] = 1 $", SIZES)
            .unwrap_err();
        let lines: Vec<usize> = errs.iter().map(|e| e.line).collect();
        assert_eq!(lines, vec![2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(errs[0].msg.contains("read-only"), "{:?}", errs[0]);
        assert!(errs[1].msg.contains("out of range"), "{:?}", errs[1]);
        assert_eq!(errs[7].col, 10);
    }

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        let src = format!("Q[0] = {}1{}", "(".repeat(10_000), ")".repeat(10_000));
        let errs = compile(&src, SIZES).unwrap_err();
        assert!(errs[0].msg.contains("nested too deeply"));
        let src = format!("Q[0] = {}1", "-".repeat(10_000));
        assert!(compile(&src, SIZES).is_err());
    }

    #[test]
    fn stores_truncate_twos_complement() {
        let (q, ..) = run("Q[0] = -1\nQW[1] = 0x12345\nQ[3] = 1 << 65", &[]);
        assert_eq!(&q[..4], &[0xFF, 0x45, 0x23, 2]);
    }

    #[test]
    fn numbers_in_any_base() {
        let (q, ..) = run("Q[0] = 0x1F\nQ[1] = 0b1010\nQ[2] = 1_0", &[]);
        assert_eq!(&q[..3], &[31, 10, 10]);
    }
}
