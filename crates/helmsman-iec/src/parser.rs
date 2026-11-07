//! Recursive-descent parser for a compilation unit: `TYPE` blocks and
//! `PROGRAM`, `FUNCTION` and `FUNCTION_BLOCK` units in any order.
//!
//! Operator precedence, tightest first: `( )`, calls, `[ ]` and `.`, then
//! unary `-` `+` `NOT`, `**`, `* / MOD`, `+ -`, `< > <= >=`, `= <>`,
//! `AND &`, `XOR`, `OR`. Binary operators of equal precedence associate
//! left to right. Unary minus binding tighter than `**` (`-2.0 ** 2` is 4)
//! follows ruSTy; see tests/oracle.rs.

use crate::lexer::{LexError, Pos, Tok, lex_spans};
#[cfg(not(feature = "std"))]
use crate::prelude::*;

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
    #[must_use]
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

    #[must_use]
    pub const fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }
}

/// A call argument: positional `e`, input `NAME := e` or output `NAME => v`.
#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    pub name: Option<String>,
    pub output: bool,
    pub value: Expr,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int {
        value: i128,
        prefix: Option<String>,
    },
    Real {
        value: f64,
        prefix: Option<String>,
    },
    Bool(bool),
    Time(i128),
    EnumLit(String, String),
    /// A name: a variable, an enumeration value or a function.
    Var(String),
    Direct(String),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Vec<Expr>),
    /// Partial access `w.3`.
    Bit(Box<Expr>, u8),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Arg>),
}

/// A CASE label bound: an integer or an enumeration value name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelValue {
    Int(i128),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseLabel {
    pub lo: LabelValue,
    pub hi: LabelValue,
    pub pos: Pos,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: Pos,
}

/// One arm of a CASE: its labels and statements.
pub type CaseArm = (Vec<CaseLabel>, Block);

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Assign(Expr, Expr),
    /// A call used as a statement (function or function block instance).
    Call(Expr),
    If(Vec<(Expr, Block)>, Block),
    Case(Expr, Vec<CaseArm>, Block),
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

#[derive(Debug, Clone, PartialEq)]
pub enum TypeRef {
    Named(String, Pos),
    Array {
        dims: Vec<(i128, i128)>,
        elem: Box<TypeRef>,
        pos: Pos,
    },
}

impl TypeRef {
    #[must_use]
    pub const fn pos(&self) -> Pos {
        match self {
            Self::Named(_, p) | Self::Array { pos: p, .. } => *p,
        }
    }
}

/// An initial value.
#[derive(Debug, Clone, PartialEq)]
pub enum Init {
    Expr(Expr),
    /// `[a, b, n(c)]`: elements, each optionally repeated.
    Array(Vec<(usize, Init)>, Pos),
    /// `(x := 1, y := 2)`.
    Struct(Vec<(String, Pos, Init)>, Pos),
}

impl Init {
    #[must_use]
    pub const fn pos(&self) -> Pos {
        match self {
            Self::Expr(e) => e.pos,
            Self::Array(_, p) | Self::Struct(_, p) => *p,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// `VAR`: keeps its value between scans (programs) or calls (FBs).
    Var,
    /// `VAR_TEMP`: re-initialised on every entry.
    Temp,
    Input,
    Output,
    InOut,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub name: String,
    pub pos: Pos,
    pub section: Section,
    pub constant: bool,
    /// Declared in a `RETAIN` block: kept across a restart of the box, not
    /// only across a deploy. `NON_RETAIN` (and no qualifier) is false.
    pub retain: bool,
    pub ty: TypeRef,
    pub at: Option<(String, Pos)>,
    pub init: Option<Init>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PouKind {
    Program,
    Function(TypeRef),
    FunctionBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pou {
    pub name: String,
    pub pos: Pos,
    pub kind: PouKind,
    pub vars: Vec<VarDecl>,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub pos: Pos,
    pub ty: TypeRef,
    pub init: Option<Init>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDef {
    Alias(TypeRef, Option<Init>),
    Struct(Vec<Field>),
    /// Values with optional explicit integers.
    Enum(Vec<(String, Pos, Option<i128>)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub pos: Pos,
    pub def: TypeDef,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Unit {
    pub types: Vec<TypeDecl>,
    pub pous: Vec<Pou>,
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

/// A declaration list that runs into the statements.
const MISSING_END_VAR: &str = "missing END_VAR: add END_VAR before this line to end the declarations";

/// Words that start a compilation unit's next POU or TYPE block, so end
/// whatever is still open before them.
const POU_START: &[&str] = &["PROGRAM", "FUNCTION", "FUNCTION_BLOCK", "TYPE"];

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
    "END_FUNCTION",
    "END_FUNCTION_BLOCK",
];

/// Whether an upper-cased name is a reserved word.
pub(crate) fn is_reserved(upper: &str) -> bool {
    RESERVED.contains(&upper)
}

/// Reserved words that cannot be names.
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
    "STRUCT",
    "END_STRUCT",
    "ARRAY",
];

/// Deepest nesting of expressions and statements accepted (parentheses
/// weigh [`EXPR_COST`]; operators chained into a left-deep tree, `NOT`/`-`
/// chains, member and index chains and nested blocks weigh 1). The parser,
/// checker and interpreter walk these trees recursively; the limit keeps
/// hostile input off the end of a 2 MiB thread stack, in a debug build.
pub const MAX_NESTING: usize = 200;

/// What entering a (parenthesised) expression counts against
/// [`MAX_NESTING`]: the parser descends through every precedence level
/// for it.
const EXPR_COST: usize = 3;

/// Words a statement can start with or a block ends with: what a
/// misspelled statement keyword is checked against.
const STMT_KEYWORDS: &[&str] = &[
    "IF",
    "THEN",
    "ELSIF",
    "ELSE",
    "END_IF",
    "CASE",
    "END_CASE",
    "FOR",
    "END_FOR",
    "WHILE",
    "END_WHILE",
    "REPEAT",
    "UNTIL",
    "END_REPEAT",
    "EXIT",
    "CONTINUE",
    "RETURN",
    "END_PROGRAM",
    "END_FUNCTION",
    "END_FUNCTION_BLOCK",
];

/// Whether `word` ends or continues the block `opener` started.
fn owns(opener: &str, word: &str) -> bool {
    matches!(
        (opener, word),
        ("IF", "END_IF" | "ELSIF" | "ELSE")
            | ("CASE", "END_CASE" | "ELSE")
            | ("FOR", "END_FOR")
            | ("WHILE", "END_WHILE")
            | ("REPEAT", "UNTIL" | "END_REPEAT")
            | ("PROGRAM", "END_PROGRAM")
            | ("FUNCTION", "END_FUNCTION")
            | ("FUNCTION_BLOCK", "END_FUNCTION_BLOCK")
    )
}

/// What closes a block, as the fix tells the user to write it.
fn closing(opener: &str) -> &'static str {
    match opener {
        "IF" => "END_IF; after its last statement",
        "CASE" => "END_CASE; after its last statement",
        "FOR" => "END_FOR; after its last statement",
        "WHILE" => "END_WHILE; after its last statement",
        "REPEAT" => "UNTIL <condition> END_REPEAT; after its last statement",
        "FUNCTION" => "END_FUNCTION at its end",
        "FUNCTION_BLOCK" => "END_FUNCTION_BLOCK at its end",
        _ => "END_PROGRAM at its end",
    }
}

/// The block a stray closing word belongs to.
fn opener_of(word: &str) -> &'static str {
    match word {
        "END_IF" | "ELSIF" => "IF",
        "ELSE" => "IF or CASE",
        "END_CASE" => "CASE",
        "END_FOR" => "FOR",
        "END_WHILE" => "WHILE",
        "UNTIL" | "END_REPEAT" => "REPEAT",
        "END_FUNCTION" => "FUNCTION",
        "END_FUNCTION_BLOCK" => "FUNCTION_BLOCK",
        _ => "PROGRAM",
    }
}

/// The error for a block that is never closed, reported at its opener.
fn never_closed(opener: &str) -> String {
    format!("this {opener} is never closed: add {}", closing(opener))
}

struct Parser {
    toks: Vec<(Tok, Pos)>,
    /// Where each token ends.
    ends: Vec<Pos>,
    i: usize,
    /// Current nesting, against [`MAX_NESTING`].
    depth: usize,
    /// The blocks open around the current token, innermost last: the
    /// opening word and where it is.
    open: Vec<(&'static str, Pos)>,
}

impl Parser {
    fn new(src: &str, system: bool) -> R<Self> {
        let (toks, ends) = lex_spans(src, system)?;
        Ok(Self {
            toks,
            ends,
            i: 0,
            depth: 0,
            open: Vec::new(),
        })
    }

    /// Where the previous token ends: where a missing `;` or `THEN` goes.
    fn prev_end(&self) -> Pos {
        if self.i == 0 {
            self.pos()
        } else {
            self.ends[self.i - 1]
        }
    }

    /// Whether the current token starts a line after the previous one.
    fn on_new_line(&self) -> bool {
        self.i > 0 && self.pos().line > self.ends[self.i - 1].line
    }

    fn err_at<T>(pos: Pos, msg: impl Into<String>) -> R<T> {
        Err(ParseError { pos, msg: msg.into() })
    }

    /// The `;` that ends a statement or declaration (`what`). Missing at
    /// the end of a line, it is reported there, not at the next line.
    fn expect_semi(&mut self, what: &str) -> R<()> {
        if self.eat(&Tok::Semi) {
            return Ok(());
        }
        if *self.tok() == Tok::RParen {
            return self.err("unmatched ): remove it, or add the ( it closes");
        }
        if self.on_new_line() || (*self.tok() == Tok::Eof && self.i > 0) {
            let msg = match &self.toks[self.i - 1].0 {
                Tok::Ident(kw) if kw.starts_with("END_") => format!("missing ; after {kw}"),
                _ => format!("missing ; at the end of this {what}"),
            };
            return Self::err_at(self.prev_end(), msg);
        }
        self.err(format!("expected ;, found {}", self.tok()))
    }

    /// `THEN`, `DO` or `OF` after `what` (the condition, the FOR range).
    fn expect_kw_after(&mut self, kw: &str, what: &str) -> R<()> {
        if self.eat_kw(kw) {
            return Ok(());
        }
        if self.on_new_line() {
            return Self::err_at(self.prev_end(), format!("missing {kw} after {what}"));
        }
        if *self.tok() == Tok::Assign {
            return self.err(format!(
                "expected {kw}, found :=; write = to compare (:= assigns)"
            ));
        }
        self.err(format!("expected {kw}, found {}", self.tok()))
    }

    /// Opens a block at `pos`, for [`Self::close`].
    fn open(&mut self, opener: &'static str, pos: Pos) {
        self.open.push((opener, pos));
    }

    /// The word `end` that closes the innermost open block. Without it,
    /// the error is at the block that is never closed when what follows
    /// belongs to a block around it (or is the end of the file), and at
    /// the stray word otherwise.
    fn close(&mut self, end: &str) -> R<()> {
        let (opener, at) = self.open.pop().unwrap_or(("PROGRAM", self.pos()));
        if self.eat_kw(end) {
            return Ok(());
        }
        let word = match self.tok() {
            Tok::Eof => return Self::err_at(at, never_closed(opener)),
            Tok::Ident(w) if POU_START.contains(&w.as_str()) => {
                return Self::err_at(at, never_closed(opener));
            }
            Tok::Ident(w) if BLOCK_END.contains(&w.as_str()) => w.clone(),
            t => return self.err(format!("expected {end}, found {t}")),
        };
        if self.open.iter().any(|(o, _)| owns(o, &word)) {
            return Self::err_at(at, never_closed(opener));
        }
        if owns(opener, &word) {
            return self.err(match word.as_str() {
                "ELSIF" => "ELSIF after ELSE: move this branch before the ELSE",
                "ELSE" => "a second ELSE: there is one, and it comes last",
                _ => "missing UNTIL <condition> before END_REPEAT",
            });
        }
        self.err(format!(
            "{word} without a matching {}: remove it",
            opener_of(&word)
        ))
    }

    /// Enters `n` more levels of nesting.
    fn deeper(&mut self, n: usize) -> R<()> {
        self.depth += n;
        if self.depth > MAX_NESTING {
            return self.err(format!("nested deeper than {MAX_NESTING} levels"));
        }
        Ok(())
    }

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

    /// " (did you mean X?)" for a misspelled keyword among `keywords`.
    fn keyword_hint(name: &str, keywords: &[&'static str]) -> String {
        crate::suggest::hint(name, keywords.iter().copied())
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

    fn signed_int(&mut self) -> R<i128> {
        let neg = self.eat(&Tok::Minus);
        match self.tok().clone() {
            Tok::Int { value, prefix: None } => {
                self.advance();
                Ok(if neg { -value } else { value })
            }
            t => self.err(format!("expected an integer, found {t}")),
        }
    }

    // ---- units ----

    fn unit(&mut self) -> R<Unit> {
        let mut u = Unit::default();
        loop {
            if *self.tok() == Tok::Eof {
                return Ok(u);
            }
            if self.eat_kw("TYPE") {
                while !self.eat_kw("END_TYPE") {
                    u.types.push(self.type_decl()?);
                }
                continue;
            }
            let pos = self.pos();
            let (kind, end) = if self.eat_kw("PROGRAM") {
                (None, "END_PROGRAM")
            } else if self.eat_kw("FUNCTION_BLOCK") {
                (Some(None), "END_FUNCTION_BLOCK")
            } else if self.eat_kw("FUNCTION") {
                (Some(Some(())), "END_FUNCTION")
            } else {
                let hint = match self.tok() {
                    Tok::Ident(w) => {
                        Self::keyword_hint(w, &["PROGRAM", "FUNCTION", "FUNCTION_BLOCK", "TYPE"])
                    }
                    _ => String::new(),
                };
                return self.err(format!(
                    "expected PROGRAM, FUNCTION, FUNCTION_BLOCK or TYPE, found {}{hint}",
                    self.tok()
                ));
            };
            self.open(
                match kind {
                    None => "PROGRAM",
                    Some(None) => "FUNCTION_BLOCK",
                    Some(Some(())) => "FUNCTION",
                },
                pos,
            );
            let (name, _) = self.ident()?;
            let kind = match kind {
                None => PouKind::Program,
                Some(None) => PouKind::FunctionBlock,
                Some(Some(())) => {
                    self.expect(&Tok::Colon)?;
                    PouKind::Function(self.type_ref()?)
                }
            };
            let vars = self.var_sections()?;
            let body = self.block()?;
            self.close(end)?;
            u.pous.push(Pou {
                name,
                pos,
                kind,
                vars,
                body,
            });
        }
    }

    fn type_decl(&mut self) -> R<TypeDecl> {
        let (name, pos) = self.ident()?;
        self.expect(&Tok::Colon)?;
        let def = if self.eat_kw("STRUCT") {
            let mut fields = Vec::new();
            while !self.eat_kw("END_STRUCT") {
                let (fname, fpos) = self.ident()?;
                self.expect(&Tok::Colon)?;
                let ty = self.type_ref()?;
                let init = if self.eat(&Tok::Assign) {
                    Some(self.init()?)
                } else {
                    None
                };
                self.expect(&Tok::Semi)?;
                fields.push(Field {
                    name: fname,
                    pos: fpos,
                    ty,
                    init,
                });
            }
            TypeDef::Struct(fields)
        } else if self.eat(&Tok::LParen) {
            let mut values = Vec::new();
            loop {
                let (v, vpos) = self.ident()?;
                let explicit = if self.eat(&Tok::Assign) {
                    Some(self.signed_int()?)
                } else {
                    None
                };
                values.push((v, vpos, explicit));
                if self.eat(&Tok::RParen) {
                    break;
                }
                self.expect(&Tok::Comma)?;
            }
            TypeDef::Enum(values)
        } else {
            let t = self.type_ref()?;
            let init = if self.eat(&Tok::Assign) {
                Some(self.init()?)
            } else {
                None
            };
            TypeDef::Alias(t, init)
        };
        self.expect(&Tok::Semi)?;
        Ok(TypeDecl { name, pos, def })
    }

    fn type_ref(&mut self) -> R<TypeRef> {
        let pos = self.pos();
        if self.eat_kw("ARRAY") {
            self.expect(&Tok::LBracket)?;
            let mut dims = Vec::new();
            loop {
                let rpos = self.pos();
                let lo = self.signed_int()?;
                self.expect(&Tok::DotDot)?;
                let hi = self.signed_int()?;
                if hi < lo {
                    return Err(ParseError {
                        pos: rpos,
                        msg: format!("empty array range {lo}..{hi}"),
                    });
                }
                dims.push((lo, hi));
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RBracket)?;
            self.expect_kw("OF")?;
            let elem = Box::new(self.type_ref()?);
            return Ok(TypeRef::Array { dims, elem, pos });
        }
        match self.advance().0 {
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => Ok(TypeRef::Named(s, pos)),
            t => Err(ParseError {
                pos,
                msg: format!("expected a type, found {t}"),
            }),
        }
    }

    fn init(&mut self) -> R<Init> {
        self.deeper(1)?;
        let i = self.init_inner()?;
        self.depth -= 1;
        Ok(i)
    }

    fn init_inner(&mut self) -> R<Init> {
        let pos = self.pos();
        if self.eat(&Tok::LBracket) {
            let mut items = Vec::new();
            loop {
                // `n(x)` repeats x n times.
                let item = if matches!(self.tok(), Tok::Int { prefix: None, .. })
                    && *self.tok_at(1) == Tok::LParen
                {
                    let n = self.signed_int()?;
                    let n = usize::try_from(n).map_err(|_| ParseError {
                        pos,
                        msg: "bad repeat count".into(),
                    })?;
                    self.expect(&Tok::LParen)?;
                    let v = self.init()?;
                    self.expect(&Tok::RParen)?;
                    (n, v)
                } else {
                    (1, self.init()?)
                };
                items.push(item);
                if self.eat(&Tok::RBracket) {
                    break;
                }
                self.expect(&Tok::Comma)?;
            }
            return Ok(Init::Array(items, pos));
        }
        if *self.tok() == Tok::LParen
            && matches!(self.tok_at(1), Tok::Ident(_))
            && *self.tok_at(2) == Tok::Assign
        {
            self.advance();
            let mut fields = Vec::new();
            loop {
                let (name, fpos) = self.ident()?;
                self.expect(&Tok::Assign)?;
                fields.push((name, fpos, self.init()?));
                if self.eat(&Tok::RParen) {
                    break;
                }
                self.expect(&Tok::Comma)?;
            }
            return Ok(Init::Struct(fields, pos));
        }
        Ok(Init::Expr(self.expr()?))
    }

    fn var_sections(&mut self) -> R<Vec<VarDecl>> {
        let mut vars = Vec::new();
        loop {
            let section = match self.tok() {
                Tok::Ident(s) => match s.as_str() {
                    "VAR" => Section::Var,
                    "VAR_TEMP" => Section::Temp,
                    "VAR_INPUT" => Section::Input,
                    "VAR_OUTPUT" => Section::Output,
                    "VAR_IN_OUT" => Section::InOut,
                    "VAR_GLOBAL" | "VAR_EXTERNAL" | "VAR_ACCESS" | "VAR_CONFIG" => {
                        return self.err(format!("{s} is not supported yet"));
                    }
                    _ => return Ok(vars),
                },
                _ => return Ok(vars),
            };
            self.advance();
            self.var_block(section, &mut vars)?;
        }
    }

    fn var_block(&mut self, section: Section, out: &mut Vec<VarDecl>) -> R<()> {
        let (mut constant, mut retain, mut non_retain) = (false, false, false);
        loop {
            let pos = self.pos();
            if self.eat_kw("CONSTANT") {
                constant = true;
            } else if self.eat_kw("RETAIN") {
                retain = true;
            } else if self.eat_kw("NON_RETAIN") {
                non_retain = true;
            } else {
                break;
            }
            if retain && non_retain {
                return Err(ParseError {
                    pos,
                    msg: "a block is RETAIN or NON_RETAIN, not both".into(),
                });
            }
        }
        while !self.eat_kw("END_VAR") {
            if self.statement_starts_here() {
                return self.err(MISSING_END_VAR);
            }
            let mut names = vec![self.ident()?];
            while self.eat(&Tok::Comma) {
                names.push(self.ident()?);
            }
            if names.len() == 1 && !matches!(self.tok(), Tok::Colon) && !self.is_kw("AT") {
                let (name, at) = &names[0];
                if matches!(self.tok(), Tok::Assign | Tok::LParen | Tok::Dot | Tok::LBracket) {
                    return Self::err_at(*at, MISSING_END_VAR);
                }
                let hint = Self::keyword_hint(name, &["END_VAR"]);
                if !hint.is_empty() {
                    return Self::err_at(*at, format!("{name} is not a keyword{hint}"));
                }
                if let Tok::Ident(ty) = self.tok()
                    && !self.on_new_line()
                {
                    return self.err(format!(
                        "expected : between the name and its type: write {name} : {ty}"
                    ));
                }
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
            let ty = self.type_ref()?;
            let init = if self.eat(&Tok::Assign) {
                Some(self.init()?)
            } else {
                None
            };
            self.expect_semi("declaration")?;
            for (name, pos) in names {
                out.push(VarDecl {
                    name,
                    pos,
                    section,
                    constant,
                    retain,
                    ty: ty.clone(),
                    at: at.clone(),
                    init: init.clone(),
                });
            }
        }
        Ok(())
    }

    /// Whether the current token starts a statement or ends the POU, so
    /// a declaration list it stands in is missing its `END_VAR`. A reserved
    /// word followed by `:` or `,` is a declaration with a reserved name.
    fn statement_starts_here(&self) -> bool {
        match self.tok() {
            Tok::Eof => true,
            Tok::Ident(w) => {
                w != "END_VAR"
                    && RESERVED.contains(&w.as_str())
                    && !matches!(self.tok_at(1), Tok::Colon | Tok::Comma)
            }
            _ => false,
        }
    }

    // ---- statements ----

    fn block(&mut self) -> R<Block> {
        self.block_until(false)
    }

    /// Statements up to a block-ending keyword (not consumed) or, inside a
    /// CASE, the start of the next label.
    fn block_until(&mut self, in_case: bool) -> R<Block> {
        let mut out = Vec::new();
        loop {
            let label_start = in_case
                && match self.tok() {
                    Tok::Int { .. } => true,
                    Tok::Minus => matches!(self.tok_at(1), Tok::Int { .. }),
                    Tok::Ident(s) => {
                        !RESERVED.contains(&s.as_str())
                            && matches!(self.tok_at(1), Tok::Colon | Tok::Comma | Tok::DotDot)
                    }
                    _ => false,
                };
            match self.tok() {
                Tok::Eof => return Ok(out),
                Tok::Ident(s) if BLOCK_END.contains(&s.as_str()) || POU_START.contains(&s.as_str()) => {
                    return Ok(out);
                }
                _ if label_start => return Ok(out),
                _ => out.push(self.stmt()?),
            }
        }
    }

    fn stmt(&mut self) -> R<Stmt> {
        // A nested statement costs the checker and interpreter more stack
        // than an operator does.
        self.deeper(EXPR_COST)?;
        let s = self.stmt_inner()?;
        self.depth -= EXPR_COST;
        Ok(s)
    }

    fn stmt_inner(&mut self) -> R<Stmt> {
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
                    self.open("WHILE", pos);
                    let c = self.expr()?;
                    self.expect_kw_after("DO", "the condition")?;
                    let body = self.block()?;
                    self.close("END_WHILE")?;
                    StmtKind::While(c, body)
                }
                "REPEAT" => {
                    self.advance();
                    self.open("REPEAT", pos);
                    let body = self.block()?;
                    self.close("UNTIL")?;
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
                _ => self.assign_or_call()?,
            },
            Tok::Direct(_) => self.assign_or_call()?,
            t => return self.err(format!("expected a statement, found {t}")),
        };
        self.expect_semi("statement")?;
        Ok(Stmt { kind, pos })
    }

    fn assign_or_call(&mut self) -> R<StmtKind> {
        let pos = self.pos();
        let target = self.postfix()?;
        if self.eat(&Tok::Assign) {
            if !matches!(
                target.kind,
                ExprKind::Var(_)
                    | ExprKind::Direct(_)
                    | ExprKind::Member(..)
                    | ExprKind::Index(..)
                    | ExprKind::Bit(..)
            ) {
                return Err(ParseError {
                    pos,
                    msg: "cannot assign to this".into(),
                });
            }
            return Ok(StmtKind::Assign(target, self.expr()?));
        }
        if matches!(target.kind, ExprKind::Call(..)) {
            return Ok(StmtKind::Call(target));
        }
        if let ExprKind::Var(name) = &target.kind {
            let hint = Self::keyword_hint(name, STMT_KEYWORDS);
            if !hint.is_empty() {
                return Self::err_at(pos, format!("{name} is not a keyword{hint}"));
            }
        }
        if *self.tok() == Tok::Eq {
            return self.err("expected := or a call, found =; write := to assign (= compares)");
        }
        self.err(format!("expected := or a call, found {}", self.tok()))
    }

    fn if_stmt(&mut self) -> R<StmtKind> {
        let pos = self.pos();
        self.expect_kw("IF")?;
        self.open("IF", pos);
        let mut arms = Vec::new();
        loop {
            let c = self.expr()?;
            self.expect_kw_after("THEN", "the condition")?;
            arms.push((c, self.block()?));
            if !self.eat_kw("ELSIF") {
                break;
            }
        }
        // `ELSE IF` nests a second IF, which needs its own END_IF; meant
        // as ELSIF, the missing END_IF is this IF's, and the fix is here.
        let mut else_if = None;
        let other = if self.is_kw("ELSE") {
            let at = self.pos();
            self.advance();
            if self.is_kw("IF") {
                else_if = Some(at);
            }
            self.block()?
        } else {
            Vec::new()
        };
        match (self.close("END_IF"), else_if) {
            (Err(e), Some(at)) if e.pos == pos => Self::err_at(
                at,
                "ELSE IF opens a second IF, which needs its own END_IF: write ELSIF to continue this IF",
            ),
            (r, _) => r.map(|()| StmtKind::If(arms, other)),
        }
    }

    fn case_value(&mut self) -> R<LabelValue> {
        if let Tok::Ident(s) = self.tok().clone()
            && !RESERVED.contains(&s.as_str())
        {
            self.advance();
            return Ok(LabelValue::Name(s));
        }
        if let Tok::EnumLit(_, v) = self.tok().clone() {
            self.advance();
            return Ok(LabelValue::Name(v));
        }
        match self.signed_int() {
            Ok(v) => Ok(LabelValue::Int(v)),
            Err(_) => self.err(format!("expected a CASE label, found {}", self.tok())),
        }
    }

    fn case_stmt(&mut self) -> R<StmtKind> {
        let pos = self.pos();
        self.expect_kw("CASE")?;
        self.open("CASE", pos);
        let sel = self.expr()?;
        self.expect_kw_after("OF", "the CASE selector")?;
        let mut arms = Vec::new();
        while !self.is_kw("ELSE") && !self.is_kw("END_CASE") {
            if matches!(self.tok(), Tok::Ident(w) if BLOCK_END.contains(&w.as_str()))
                || *self.tok() == Tok::Eof
            {
                break;
            }
            let mut labels = Vec::new();
            loop {
                let pos = self.pos();
                let lo = self.case_value()?;
                let hi = if self.eat(&Tok::DotDot) {
                    self.case_value()?
                } else {
                    lo.clone()
                };
                if let (LabelValue::Int(a), LabelValue::Int(b)) = (&lo, &hi)
                    && b < a
                {
                    return Err(ParseError {
                        pos,
                        msg: format!("empty CASE range {a}..{b}"),
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
        self.close("END_CASE")?;
        Ok(StmtKind::Case(sel, arms, other))
    }

    fn for_stmt(&mut self) -> R<StmtKind> {
        let pos = self.pos();
        self.expect_kw("FOR")?;
        self.open("FOR", pos);
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
        self.expect_kw_after("DO", "the FOR range")?;
        let body = self.block()?;
        self.close("END_FOR")?;
        Ok(StmtKind::For {
            var,
            from,
            to,
            by,
            body,
        })
    }

    // ---- expressions ----

    pub fn expr(&mut self) -> R<Expr> {
        self.deeper(EXPR_COST)?;
        let e = self.binary(0)?;
        self.depth -= EXPR_COST;
        Ok(e)
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
            return self.power();
        }
        let mut lhs = self.binary(level + 1)?;
        let mut chain = 0;
        while let Some(op) = self.binop(level) {
            let pos = self.pos();
            if self.tok_at(1) == self.tok() {
                match self.tok() {
                    Tok::Eq => return self.err("write = to compare, not =="),
                    Tok::Amp => return self.err("write AND (or a single &), not &&"),
                    _ => {}
                }
            }
            self.advance();
            self.deeper(1)?;
            chain += 1;
            let rhs = self.binary(level + 1)?;
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                pos,
            };
        }
        self.depth -= chain;
        Ok(lhs)
    }

    fn unary(&mut self) -> R<Expr> {
        let pos = self.pos();
        if self.eat(&Tok::Minus) {
            // Fold `-5` into a literal so `-128` fits SINT.
            match self.tok().clone() {
                Tok::Int { value, prefix: None } => {
                    self.advance();
                    return Ok(Expr {
                        kind: ExprKind::Int {
                            value: -value,
                            prefix: None,
                        },
                        pos,
                    });
                }
                Tok::Real { value, prefix: None } => {
                    self.advance();
                    return Ok(Expr {
                        kind: ExprKind::Real {
                            value: -value,
                            prefix: None,
                        },
                        pos,
                    });
                }
                _ => {}
            }
            self.deeper(1)?;
            let e = self.unary()?;
            self.depth -= 1;
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Neg, Box::new(e)),
                pos,
            });
        }
        if self.eat(&Tok::Plus) {
            self.deeper(1)?;
            let e = self.unary()?;
            self.depth -= 1;
            return Ok(e);
        }
        if self.eat_kw("NOT") {
            self.deeper(1)?;
            let e = self.unary()?;
            self.depth -= 1;
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Not, Box::new(e)),
                pos,
            });
        }
        self.postfix()
    }

    fn power(&mut self) -> R<Expr> {
        let mut lhs = self.unary()?;
        let mut chain = 0;
        while *self.tok() == Tok::Power {
            let pos = self.pos();
            self.advance();
            self.deeper(1)?;
            chain += 1;
            let rhs = self.unary()?;
            lhs = Expr {
                kind: ExprKind::Binary(BinOp::Pow, Box::new(lhs), Box::new(rhs)),
                pos,
            };
        }
        self.depth -= chain;
        Ok(lhs)
    }

    fn args(&mut self) -> R<Vec<Arg>> {
        let mut args = Vec::new();
        if self.eat(&Tok::RParen) {
            return Ok(args);
        }
        loop {
            let pos = self.pos();
            let named =
                matches!(self.tok(), Tok::Ident(_)) && matches!(self.tok_at(1), Tok::Assign | Tok::Arrow);
            let arg = if named {
                let (name, _) = self.ident()?;
                let output = self.advance().0 == Tok::Arrow;
                let value = if output { self.postfix()? } else { self.expr()? };
                Arg {
                    name: Some(name),
                    output,
                    value,
                    pos,
                }
            } else {
                Arg {
                    name: None,
                    output: false,
                    value: self.expr()?,
                    pos,
                }
            };
            args.push(arg);
            if self.eat(&Tok::RParen) {
                return Ok(args);
            }
            self.expect(&Tok::Comma)?;
        }
    }

    /// A primary followed by `.member`, `.bit`, `[index]` and `(args)`.
    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        let mut chain = 0;
        loop {
            let pos = self.pos();
            if matches!(self.tok(), Tok::Dot | Tok::LBracket | Tok::LParen) {
                self.deeper(1)?;
                chain += 1;
            }
            match self.tok().clone() {
                Tok::Dot => {
                    self.advance();
                    match self.advance().0 {
                        Tok::Ident(m) => {
                            e = Expr {
                                kind: ExprKind::Member(Box::new(e), m),
                                pos,
                            }
                        }
                        Tok::Int { value, prefix: None } if (0..64).contains(&value) => {
                            let bit = u8::try_from(value).unwrap_or(0);
                            e = Expr {
                                kind: ExprKind::Bit(Box::new(e), bit),
                                pos,
                            };
                        }
                        t => {
                            return Err(ParseError {
                                pos,
                                msg: format!("expected a member name or bit number, found {t}"),
                            });
                        }
                    }
                }
                Tok::LBracket => {
                    self.advance();
                    let mut idx = vec![self.expr()?];
                    while self.eat(&Tok::Comma) {
                        idx.push(self.expr()?);
                    }
                    self.expect(&Tok::RBracket)?;
                    e = Expr {
                        kind: ExprKind::Index(Box::new(e), idx),
                        pos,
                    };
                }
                Tok::LParen
                    if matches!(
                        e.kind,
                        ExprKind::Var(_) | ExprKind::Member(..) | ExprKind::Index(..)
                    ) =>
                {
                    self.advance();
                    let args = self.args()?;
                    e = Expr {
                        kind: ExprKind::Call(Box::new(e), args),
                        pos,
                    };
                }
                _ => {
                    self.depth -= chain;
                    return Ok(e);
                }
            }
        }
    }

    fn primary(&mut self) -> R<Expr> {
        let pos = self.pos();
        let kind = match self.advance().0 {
            Tok::Int { value, prefix } => ExprKind::Int { value, prefix },
            Tok::Real { value, prefix } => ExprKind::Real { value, prefix },
            Tok::Bool(b) => ExprKind::Bool(b),
            Tok::Time(ns) => ExprKind::Time(ns),
            Tok::EnumLit(t, v) => ExprKind::EnumLit(t, v),
            Tok::Direct(d) => ExprKind::Direct(d),
            Tok::LParen => {
                let e = self.expr()?;
                if !self.eat(&Tok::RParen) {
                    let at = if self.on_new_line() {
                        self.prev_end()
                    } else {
                        self.pos()
                    };
                    return Self::err_at(at, format!("missing ): the ( at {pos} is never closed"));
                }
                return Ok(e);
            }
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => ExprKind::Var(s),
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

/// Parses a compilation unit. `system` admits `__` names.
pub fn parse(src: &str, system: bool) -> R<Unit> {
    Parser::new(src, system)?.unit()
}

/// Parses `src` as exactly one expression.
pub fn parse_expr(src: &str) -> R<Expr> {
    let mut p = Parser::new(src, false)?;
    let e = p.expr()?;
    if *p.tok() != Tok::Eof {
        return p.err(format!("expected the end of the expression, found {}", p.tok()));
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(src: &str) -> Expr {
        let mut p = Parser::new(src, false).unwrap();
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
            ExprKind::Time(ns) => format!("T#{ns}ns"),
            ExprKind::EnumLit(t, v) => format!("{t}#{v}"),
            ExprKind::Var(v) => v.clone(),
            ExprKind::Direct(d) => format!("%{d}"),
            ExprKind::Member(b, m) => format!("{}.{m}", show(b)),
            ExprKind::Bit(b, n) => format!("{}.{n}", show(b)),
            ExprKind::Index(b, i) => format!(
                "{}[{}]",
                show(b),
                i.iter().map(show).collect::<Vec<_>>().join(",")
            ),
            ExprKind::Unary(UnOp::Neg, a) => format!("(-{})", show(a)),
            ExprKind::Unary(UnOp::Not, a) => format!("(NOT {})", show(a)),
            ExprKind::Binary(op, a, b) => format!("({} {} {})", show(a), op.symbol(), show(b)),
            ExprKind::Call(f, args) => {
                let a: Vec<String> = args
                    .iter()
                    .map(|a| match &a.name {
                        Some(n) => format!("{n}{}{}", if a.output { "=>" } else { ":=" }, show(&a.value)),
                        None => show(&a.value),
                    })
                    .collect();
                format!("{}({})", show(f), a.join(", "))
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
            ("-2 ** 2", "(-2 ** 2)"),
            ("2.0 ** 3 ** 2", "((2.0 ** 3) ** 2)"),
            ("2.0 ** -1", "(2.0 ** -1)"),
            ("NOT a AND b", "((NOT A) AND B)"),
            ("a & b", "(A AND B)"),
            ("-x", "(-X)"),
            ("-(1 + 2)", "(-(1 + 2))"),
            ("INT_TO_REAL(a + 1, b)", "INT_TO_REAL((A + 1), B)"),
            ("a[i + 1, 2].b.c[3]", "A[(I + 1),2].B.C[3]"),
            ("w.3 OR t.Q", "(W.3 OR T.Q)"),
            ("-a.b ** 2", "((-A.B) ** 2)"),
            ("NOT a ** b", "((NOT A) ** B)"),
            ("f(x := 1, y => z)", "F(X:=1, Y=>Z)"),
            ("T#1s + t", "(T#1000000000ns + T)"),
            ("c = Color#Red", "(C = COLOR#RED)"),
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
    fn units_and_types() {
        let u = parse(
            "TYPE
               Point : STRUCT x : INT := 1; y : REAL; END_STRUCT;
               Color : (Red, Green := 5, Blue);
               Speed : INT := 10;
               Row : ARRAY[0..3] OF Point;
             END_TYPE
             FUNCTION add : INT VAR_INPUT a, b : INT; END_VAR add := a + b; END_FUNCTION
             FUNCTION_BLOCK counter
               VAR_INPUT up : BOOL; END_VAR VAR_OUTPUT n : DINT; END_VAR
               IF up THEN n := n + 1; END_IF;
             END_FUNCTION_BLOCK
             PROGRAM main
               VAR c : counter; grid : ARRAY[1..2, 1..3] OF INT := [1, 2, 4(0)]; p : Point := (x := 3); END_VAR
               c(up := TRUE);
               grid[1, 2] := add(1, b := 2);
               CASE Color#Red OF Red: ; Green, Blue: ; END_CASE;
             END_PROGRAM",
            false,
        )
        .unwrap();
        assert_eq!(u.types.len(), 4);
        assert!(matches!(&u.types[1].def, TypeDef::Enum(v) if v[1].2 == Some(5)));
        assert_eq!(u.pous.len(), 3);
        assert!(matches!(u.pous[0].kind, PouKind::Function(_)));
        assert_eq!(u.pous[1].vars[0].section, Section::Input);
        let main = &u.pous[2];
        assert!(matches!(&main.vars[1].ty, TypeRef::Array { dims, .. } if dims == &[(1, 2), (1, 3)]));
        assert!(matches!(&main.vars[1].init, Some(Init::Array(items, _)) if items[2].0 == 4));
        assert!(matches!(&main.vars[2].init, Some(Init::Struct(f, _)) if f[0].0 == "X"));
        assert!(matches!(main.body[0].kind, StmtKind::Call(_)));
        let StmtKind::Case(_, arms, _) = &main.body[2].kind else {
            panic!()
        };
        assert_eq!(arms[1].0.len(), 2);
    }

    #[test]
    fn statements() {
        let u = parse(
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
            false,
        )
        .unwrap();
        let p = &u.pous[0];
        assert_eq!(p.name, "MAIN");
        assert_eq!(p.vars.len(), 5);
        assert!(p.vars[0].constant);
        assert_eq!(p.vars[3].at.as_ref().unwrap().0, "IX0.1");
        assert_eq!(p.vars[4].section, Section::Temp);
        assert_eq!(p.body.len(), 9);
        assert_eq!(p.body[1].kind, StmtKind::Empty);
        let StmtKind::Case(_, arms, other) = &p.body[3].kind else {
            panic!()
        };
        assert_eq!(arms[0].0[1].hi, LabelValue::Int(5));
        assert_eq!(arms[1].0[0].lo, LabelValue::Int(-2));
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn errors_point_at_the_problem() {
        let e = |src: &str| parse(src, false).unwrap_err();
        assert_eq!(e("PROGRAM p a := 1 END_PROGRAM").pos.col, 18);
        assert!(
            e("PROGRAM p VAR_GLOBAL a : INT; END_VAR END_PROGRAM")
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
        assert!(e("PROGRAM p END_PROGRAM x").msg.contains("expected PROGRAM"));
        assert!(
            e("PROGRAM p a := (1 + ; END_PROGRAM")
                .msg
                .contains("expected an expression")
        );
        assert!(e("PROGRAM p IF a THEN END_PROGRAM").msg.contains("END_IF"));
        assert!(
            e("PROGRAM p a; END_PROGRAM")
                .msg
                .contains("expected := or a call")
        );
        assert!(e("PROGRAM p (a) := 1; END_PROGRAM").msg.contains("statement"));
        assert!(
            e("PROGRAM p VAR a : ARRAY[3..1] OF INT; END_VAR END_PROGRAM")
                .msg
                .contains("empty array range")
        );
        assert!(
            e("FUNCTION f VAR_INPUT a : INT; END_VAR END_FUNCTION")
                .msg
                .contains("expected :")
        );
    }
}
