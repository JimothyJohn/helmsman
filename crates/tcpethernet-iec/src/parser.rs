//! Recursive-descent parser for a `PROGRAM ... END_PROGRAM` unit.
//!
//! Operator precedence, tightest first: `( )` and calls, `**`, unary `-` `+`
//! `NOT`, `* / MOD`, `+ -`, `< > <= >=`, `= <>`, `AND &`, `XOR`, `OR`.
//! Binary operators of equal precedence associate left to right.

use crate::lexer::{LexError, Pos, Tok, lex};

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    Xor,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

impl BinOp {
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Or => "OR",
            Self::Xor => "XOR",
            Self::And => "AND",
            Self::Eq => "=",
            Self::Ne => "<>",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Mod => "MOD",
            Self::Pow => "**",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int { value: i128, prefix: Option<String> },
    Real { value: f64, prefix: Option<String> },
    Bool(bool),
    Var(String),
    Direct(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

/// Inclusive range of a CASE label; a single value has `lo == hi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaseLabel {
    pub lo: i128,
    pub hi: i128,
    pub pos: Pos,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Assign(Expr, Expr),
    If(Vec<(Expr, Block)>, Block),
    Case(Expr, Vec<(Vec<CaseLabel>, Block)>, Block),
    For {
        var: String,
        from: Expr,
        to: Expr,
        by: Option<Expr>,
        body: Block,
    },
    While(Expr, Block),
    Repeat(Block, Expr),
    Exit,
    Continue,
    Return,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    /// `VAR`: keeps its value from scan to scan.
    Var,
    /// `VAR_TEMP`: re-initialised at the start of every scan.
    Temp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub name: String,
    pub pos: Pos,
    pub kind: VarKind,
    pub constant: bool,
    pub ty: String,
    pub ty_pos: Pos,
    pub at: Option<(String, Pos)>,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub name: String,
    pub vars: Vec<VarDecl>,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub pos: Pos,
    pub msg: String,
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        Self {
            pos: e.pos,
            msg: e.msg,
        }
    }
}

type R<T> = Result<T, ParseError>;

/// Words that end a statement list.
const BLOCK_END: &[&str] = &[
    "END_IF",
    "ELSIF",
    "ELSE",
    "END_CASE",
    "END_FOR",
    "END_WHILE",
    "UNTIL",
    "END_REPEAT",
    "END_PROGRAM",
];

/// Reserved words that cannot be variable names.
const RESERVED: &[&str] = &[
    "PROGRAM",
    "END_PROGRAM",
    "VAR",
    "VAR_TEMP",
    "VAR_INPUT",
    "VAR_OUTPUT",
    "VAR_IN_OUT",
    "VAR_GLOBAL",
    "VAR_EXTERNAL",
    "END_VAR",
    "CONSTANT",
    "RETAIN",
    "NON_RETAIN",
    "AT",
    "IF",
    "THEN",
    "ELSIF",
    "ELSE",
    "END_IF",
    "CASE",
    "OF",
    "END_CASE",
    "FOR",
    "TO",
    "BY",
    "DO",
    "END_FOR",
    "WHILE",
    "END_WHILE",
    "REPEAT",
    "UNTIL",
    "END_REPEAT",
    "EXIT",
    "CONTINUE",
    "RETURN",
    "AND",
    "OR",
    "XOR",
    "NOT",
    "MOD",
    "FUNCTION",
    "FUNCTION_BLOCK",
    "END_FUNCTION",
    "END_FUNCTION_BLOCK",
    "TYPE",
    "END_TYPE",
];

struct Parser {
    toks: Vec<(Tok, Pos)>,
    i: usize,
}

impl Parser {
    fn tok(&self) -> &Tok {
        &self.toks[self.i].0
    }

    fn tok_at(&self, k: usize) -> &Tok {
        &self.toks[(self.i + k).min(self.toks.len() - 1)].0
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].1
    }

    fn advance(&mut self) -> (Tok, Pos) {
        let t = self.toks[self.i].clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(ParseError {
            pos: self.pos(),
            msg: msg.into(),
        })
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.tok(), Tok::Ident(s) if s == kw)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> R<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            self.err(format!("expected {kw}, found {}", self.tok()))
        }
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.tok() == t {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok) -> R<()> {
        if self.eat(t) {
            Ok(())
        } else {
            self.err(format!("expected {t}, found {}", self.tok()))
        }
    }

    fn ident(&mut self) -> R<(String, Pos)> {
        match self.tok().clone() {
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => {
                let p = self.pos();
                self.advance();
                Ok((s, p))
            }
            t => self.err(format!("expected a name, found {t}")),
        }
    }

    fn program(&mut self) -> R<Program> {
        self.expect_kw("PROGRAM")?;
        let (name, _) = self.ident()?;
        let mut vars = Vec::new();
        loop {
            let kind = if self.eat_kw("VAR") {
                VarKind::Var
            } else if self.eat_kw("VAR_TEMP") {
                VarKind::Temp
            } else if let Tok::Ident(s) = self.tok()
                && s.starts_with("VAR_")
            {
                return self.err(format!("{s} is not supported in a PROGRAM yet"));
            } else {
                break;
            };
            self.var_block(kind, &mut vars)?;
        }
        let body = self.block()?;
        self.expect_kw("END_PROGRAM")?;
        if *self.tok() != Tok::Eof {
            return self.err(format!(
                "expected end of input after END_PROGRAM, found {}",
                self.tok()
            ));
        }
        Ok(Program { name, vars, body })
    }

    fn var_block(&mut self, kind: VarKind, out: &mut Vec<VarDecl>) -> R<()> {
        let mut constant = false;
        loop {
            if self.eat_kw("CONSTANT") {
                constant = true;
            } else if !(self.eat_kw("RETAIN") || self.eat_kw("NON_RETAIN")) {
                break;
            }
        }
        while !self.eat_kw("END_VAR") {
            let mut names = vec![self.ident()?];
            while self.eat(&Tok::Comma) {
                names.push(self.ident()?);
            }
            let at = if self.eat_kw("AT") {
                let p = self.pos();
                match self.advance().0 {
                    Tok::Direct(d) => Some((d, p)),
                    t => {
                        return Err(ParseError {
                            pos: p,
                            msg: format!("expected an address after AT, found {t}"),
                        });
                    }
                }
            } else {
                None
            };
            if at.is_some() && names.len() > 1 {
                return Err(ParseError {
                    pos: names[1].1,
                    msg: "AT takes a single variable".into(),
                });
            }
            self.expect(&Tok::Colon)?;
            let ty_pos = self.pos();
            let ty = match self.advance().0 {
                Tok::Ident(s) => s,
                t => {
                    return Err(ParseError {
                        pos: ty_pos,
                        msg: format!("expected a type name, found {t}"),
                    });
                }
            };
            let init = if self.eat(&Tok::Assign) {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect(&Tok::Semi)?;
            for (name, pos) in names {
                out.push(VarDecl {
                    name,
                    pos,
                    kind,
                    constant,
                    ty: ty.clone(),
                    ty_pos,
                    at: at.clone(),
                    init: init.clone(),
                });
            }
        }
        Ok(())
    }

    /// Statements up to a block-ending keyword (not consumed) or, inside a
    /// CASE, the start of the next label.
    fn block(&mut self) -> R<Block> {
        self.block_until(false)
    }

    fn block_until(&mut self, in_case: bool) -> R<Block> {
        let mut out = Vec::new();
        loop {
            match self.tok() {
                Tok::Eof => return Ok(out),
                Tok::Ident(s) if BLOCK_END.contains(&s.as_str()) => return Ok(out),
                Tok::Int { .. } if in_case => return Ok(out),
                Tok::Minus if in_case && matches!(self.tok_at(1), Tok::Int { .. }) => return Ok(out),
                _ => out.push(self.stmt()?),
            }
        }
    }

    fn stmt(&mut self) -> R<Stmt> {
        let pos = self.pos();
        let kind = match self.tok().clone() {
            Tok::Semi => {
                self.advance();
                return Ok(Stmt {
                    kind: StmtKind::Empty,
                    pos,
                });
            }
            Tok::Ident(kw) => match kw.as_str() {
                "IF" => self.if_stmt()?,
                "CASE" => self.case_stmt()?,
                "FOR" => self.for_stmt()?,
                "WHILE" => {
                    self.advance();
                    let c = self.expr()?;
                    self.expect_kw("DO")?;
                    let body = self.block()?;
                    self.expect_kw("END_WHILE")?;
                    StmtKind::While(c, body)
                }
                "REPEAT" => {
                    self.advance();
                    let body = self.block()?;
                    self.expect_kw("UNTIL")?;
                    let c = self.expr()?;
                    self.expect_kw("END_REPEAT")?;
                    StmtKind::Repeat(body, c)
                }
                "EXIT" => {
                    self.advance();
                    StmtKind::Exit
                }
                "CONTINUE" => {
                    self.advance();
                    StmtKind::Continue
                }
                "RETURN" => {
                    self.advance();
                    StmtKind::Return
                }
                _ => self.assign()?,
            },
            Tok::Direct(_) => self.assign()?,
            t => return self.err(format!("expected a statement, found {t}")),
        };
        self.expect(&Tok::Semi)?;
        Ok(Stmt { kind, pos })
    }

    fn assign(&mut self) -> R<StmtKind> {
        let pos = self.pos();
        let target = match self.advance().0 {
            Tok::Direct(d) => Expr {
                kind: ExprKind::Direct(d),
                pos,
            },
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => Expr {
                kind: ExprKind::Var(s),
                pos,
            },
            t => {
                return Err(ParseError {
                    pos,
                    msg: format!("expected a statement, found {t}"),
                });
            }
        };
        self.expect(&Tok::Assign)?;
        Ok(StmtKind::Assign(target, self.expr()?))
    }

    fn if_stmt(&mut self) -> R<StmtKind> {
        self.expect_kw("IF")?;
        let mut arms = Vec::new();
        loop {
            let c = self.expr()?;
            self.expect_kw("THEN")?;
            arms.push((c, self.block()?));
            if !self.eat_kw("ELSIF") {
                break;
            }
        }
        let other = if self.eat_kw("ELSE") {
            self.block()?
        } else {
            Vec::new()
        };
        self.expect_kw("END_IF")?;
        Ok(StmtKind::If(arms, other))
    }

    fn case_value(&mut self) -> R<i128> {
        let neg = self.eat(&Tok::Minus);
        match self.tok().clone() {
            Tok::Int { value, prefix: None } => {
                self.advance();
                Ok(if neg { -value } else { value })
            }
            t => self.err(format!("expected an integer CASE label, found {t}")),
        }
    }

    fn case_stmt(&mut self) -> R<StmtKind> {
        self.expect_kw("CASE")?;
        let sel = self.expr()?;
        self.expect_kw("OF")?;
        let mut arms = Vec::new();
        while !self.is_kw("ELSE") && !self.is_kw("END_CASE") {
            let mut labels = Vec::new();
            loop {
                let pos = self.pos();
                let lo = self.case_value()?;
                let hi = if self.eat(&Tok::DotDot) {
                    self.case_value()?
                } else {
                    lo
                };
                if hi < lo {
                    return Err(ParseError {
                        pos,
                        msg: format!("empty CASE range {lo}..{hi}"),
                    });
                }
                labels.push(CaseLabel { lo, hi, pos });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::Colon)?;
            arms.push((labels, self.block_until(true)?));
        }
        let other = if self.eat_kw("ELSE") {
            self.block()?
        } else {
            Vec::new()
        };
        self.expect_kw("END_CASE")?;
        Ok(StmtKind::Case(sel, arms, other))
    }

    fn for_stmt(&mut self) -> R<StmtKind> {
        self.expect_kw("FOR")?;
        let (var, _) = self.ident()?;
        self.expect(&Tok::Assign)?;
        let from = self.expr()?;
        self.expect_kw("TO")?;
        let to = self.expr()?;
        let by = if self.eat_kw("BY") {
            Some(self.expr()?)
        } else {
            None
        };
        self.expect_kw("DO")?;
        let body = self.block()?;
        self.expect_kw("END_FOR")?;
        Ok(StmtKind::For {
            var,
            from,
            to,
            by,
            body,
        })
    }

    pub fn expr(&mut self) -> R<Expr> {
        self.binary(0)
    }

    /// Binary operator at precedence `level` (0 = loosest), if the current
    /// token is one.
    fn binop(&self, level: usize) -> Option<BinOp> {
        let op = match (level, self.tok()) {
            (0, Tok::Ident(s)) if s == "OR" => BinOp::Or,
            (1, Tok::Ident(s)) if s == "XOR" => BinOp::Xor,
            (2, Tok::Ident(s)) if s == "AND" => BinOp::And,
            (2, Tok::Amp) => BinOp::And,
            (3, Tok::Eq) => BinOp::Eq,
            (3, Tok::Ne) => BinOp::Ne,
            (4, Tok::Lt) => BinOp::Lt,
            (4, Tok::Le) => BinOp::Le,
            (4, Tok::Gt) => BinOp::Gt,
            (4, Tok::Ge) => BinOp::Ge,
            (5, Tok::Plus) => BinOp::Add,
            (5, Tok::Minus) => BinOp::Sub,
            (6, Tok::Star) => BinOp::Mul,
            (6, Tok::Slash) => BinOp::Div,
            (6, Tok::Ident(s)) if s == "MOD" => BinOp::Mod,
            _ => return None,
        };
        Some(op)
    }

    fn binary(&mut self, level: usize) -> R<Expr> {
        if level > 6 {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        while let Some(op) = self.binop(level) {
            let pos = self.pos();
            self.advance();
            let rhs = self.binary(level + 1)?;
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                pos,
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> R<Expr> {
        let pos = self.pos();
        if self.eat(&Tok::Minus) {
            // Fold `-5` into a literal so `-128` fits SINT, unless `**`
            // follows: `-2**2` is `-(2**2)`.
            if let Tok::Int { value, prefix: None } = self.tok().clone()
                && *self.tok_at(1) != Tok::Power
            {
                self.advance();
                return Ok(Expr {
                    kind: ExprKind::Int {
                        value: -value,
                        prefix: None,
                    },
                    pos,
                });
            }
            if let Tok::Real { value, prefix: None } = self.tok().clone()
                && *self.tok_at(1) != Tok::Power
            {
                self.advance();
                return Ok(Expr {
                    kind: ExprKind::Real {
                        value: -value,
                        prefix: None,
                    },
                    pos,
                });
            }
            let e = self.unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Neg, Box::new(e)),
                pos,
            });
        }
        if self.eat(&Tok::Plus) {
            return self.unary();
        }
        if self.eat_kw("NOT") {
            let e = self.unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Not, Box::new(e)),
                pos,
            });
        }
        self.power()
    }

    fn power(&mut self) -> R<Expr> {
        let mut lhs = self.primary()?;
        while *self.tok() == Tok::Power {
            let pos = self.pos();
            self.advance();
            // The exponent may carry its own sign: `2.0 ** -1`.
            let rhs = if matches!(self.tok(), Tok::Minus | Tok::Plus) || self.is_kw("NOT") {
                self.unary()?
            } else {
                self.primary()?
            };
            lhs = Expr {
                kind: ExprKind::Binary(BinOp::Pow, Box::new(lhs), Box::new(rhs)),
                pos,
            };
        }
        Ok(lhs)
    }

    fn primary(&mut self) -> R<Expr> {
        let pos = self.pos();
        let kind = match self.advance().0 {
            Tok::Int { value, prefix } => ExprKind::Int { value, prefix },
            Tok::Real { value, prefix } => ExprKind::Real { value, prefix },
            Tok::Bool(b) => ExprKind::Bool(b),
            Tok::Direct(d) => ExprKind::Direct(d),
            Tok::LParen => {
                let e = self.expr()?;
                self.expect(&Tok::RParen)?;
                return Ok(e);
            }
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => {
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            args.push(self.expr()?);
                            if self.eat(&Tok::RParen) {
                                break;
                            }
                            self.expect(&Tok::Comma)?;
                        }
                    }
                    ExprKind::Call(s, args)
                } else {
                    ExprKind::Var(s)
                }
            }
            t => {
                return Err(ParseError {
                    pos,
                    msg: format!("expected an expression, found {t}"),
                });
            }
        };
        Ok(Expr { kind, pos })
    }
}

/// Parses one `PROGRAM` unit.
pub fn parse(src: &str) -> R<Program> {
    let toks = lex(src)?;
    Parser { toks, i: 0 }.program()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(src: &str) -> Expr {
        let toks = lex(src).unwrap();
        let mut p = Parser { toks, i: 0 };
        let e = p.expr().unwrap();
        assert_eq!(*p.tok(), Tok::Eof, "trailing input in {src:?}");
        e
    }

    /// Fully parenthesised rendering, to check precedence.
    fn show(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int { value, .. } => value.to_string(),
            ExprKind::Real { value, .. } => format!("{value:?}"),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Var(v) => v.clone(),
            ExprKind::Direct(d) => format!("%{d}"),
            ExprKind::Unary(UnOp::Neg, a) => format!("(-{})", show(a)),
            ExprKind::Unary(UnOp::Not, a) => format!("(NOT {})", show(a)),
            ExprKind::Binary(op, a, b) => format!("({} {} {})", show(a), op.symbol(), show(b)),
            ExprKind::Call(f, args) => {
                format!("{f}({})", args.iter().map(show).collect::<Vec<_>>().join(", "))
            }
        }
    }

    #[test]
    fn precedence_and_associativity() {
        let cases = [
            ("a OR b AND c", "(A OR (B AND C))"),
            ("a XOR b OR c", "((A XOR B) OR C)"),
            ("a = b AND c <> d", "((A = B) AND (C <> D))"),
            ("a < b = c", "((A < B) = C)"),
            ("1 + 2 * 3", "(1 + (2 * 3))"),
            ("10 - 3 - 2", "((10 - 3) - 2)"),
            ("a MOD b * c", "((A MOD B) * C)"),
            ("-2 ** 2", "(-(2 ** 2))"),
            ("2.0 ** 3 ** 2", "((2.0 ** 3) ** 2)"),
            ("2.0 ** -1", "(2.0 ** -1)"),
            ("NOT a AND b", "((NOT A) AND B)"),
            ("a & b", "(A AND B)"),
            ("-x", "(-X)"),
            ("-(1 + 2)", "(-(1 + 2))"),
            ("INT_TO_REAL(a + 1, b)", "INT_TO_REAL((A + 1), B)"),
        ];
        for (src, want) in cases {
            assert_eq!(show(&expr(src)), want, "{src}");
        }
    }

    #[test]
    fn negative_literals_fold() {
        assert_eq!(
            expr("-128").kind,
            ExprKind::Int {
                value: -128,
                prefix: None
            }
        );
        assert_eq!(
            expr("-1.5").kind,
            ExprKind::Real {
                value: -1.5,
                prefix: None
            }
        );
    }

    #[test]
    fn statements() {
        let p = parse(
            "PROGRAM main
             VAR CONSTANT k : INT := 3; END_VAR
             VAR a, b : INT; x AT %IX0.1 : BOOL; END_VAR
             VAR_TEMP t : DINT; END_VAR
               a := 1;;
               IF x THEN a := 2; ELSIF b > 1 THEN a := 3; ELSE a := 4; END_IF;
               CASE a OF 1, 3..5: b := 1; -2: b := 2; ELSE b := 0; END_CASE;
               FOR t := 1 TO 10 BY 2 DO EXIT; END_FOR;
               WHILE a < 10 DO a := a + 1; CONTINUE; END_WHILE;
               REPEAT a := a - 1; UNTIL a = 0 END_REPEAT;
               %QX0.0 := x;
               RETURN;
             END_PROGRAM",
        )
        .unwrap();
        assert_eq!(p.name, "MAIN");
        assert_eq!(p.vars.len(), 5);
        assert!(p.vars[0].constant);
        assert_eq!(p.vars[3].at.as_ref().unwrap().0, "IX0.1");
        assert_eq!(p.vars[4].kind, VarKind::Temp);
        assert_eq!(p.body.len(), 9);
        assert_eq!(p.body[1].kind, StmtKind::Empty);
        let StmtKind::Case(_, arms, other) = &p.body[3].kind else {
            panic!()
        };
        assert_eq!(
            arms[0].0.iter().map(|l| (l.lo, l.hi)).collect::<Vec<_>>(),
            vec![(1, 1), (3, 5)]
        );
        assert_eq!((arms[1].0[0].lo, arms[1].0[0].hi), (-2, -2));
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn errors_point_at_the_problem() {
        let e = |src: &str| parse(src).unwrap_err();
        assert_eq!(e("PROGRAM p a := 1 END_PROGRAM").pos.col, 18);
        assert!(
            e("PROGRAM p VAR_INPUT a : INT; END_VAR END_PROGRAM")
                .msg
                .contains("not supported")
        );
        assert!(
            e("PROGRAM p VAR a, b AT %IX0.0 : BOOL; END_VAR END_PROGRAM")
                .msg
                .contains("single")
        );
        assert!(
            e("PROGRAM p VAR if : INT; END_VAR END_PROGRAM")
                .msg
                .contains("expected a name")
        );
        assert!(
            e("PROGRAM p CASE a OF 5..1: ; END_CASE; END_PROGRAM")
                .msg
                .contains("empty CASE range")
        );
        assert!(e("PROGRAM p END_PROGRAM x").msg.contains("end of input"));
        assert!(
            e("PROGRAM p a := (1 + ; END_PROGRAM")
                .msg
                .contains("expected an expression")
        );
        assert!(e("PROGRAM p IF a THEN END_PROGRAM").msg.contains("END_IF"));
    }
}
