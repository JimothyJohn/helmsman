//! Rung neutral text (RM014 pp.209-221): `XIC(a)[XIO(b),XIC(c)]OTE(d);`.
//! Syntax only: what each instruction and operand means is the
//! converter's business ([`super::convert`]). Bounded: branches nest at
//! most [`MAX_NEST`] deep, a rung holds at most [`MAX_INSTRS`]
//! instructions, and a CPT expression nests at most [`MAX_NEST`] deep, so
//! hostile text cannot exhaust the stack.

/// Deepest branch or expression nesting read.
pub const MAX_NEST: usize = 32;
/// Most instructions on one rung.
pub const MAX_INSTRS: usize = 1000;

/// A rung: its instructions and branches, left to right.
pub type Rung = Vec<Elem>;

#[derive(Debug, Clone, PartialEq)]
pub enum Elem {
    Instr(Instr),
    /// `[leg,leg,...]`: legs top to bottom; a leg may be empty (a wire).
    Branch(Vec<Vec<Elem>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instr {
    /// The mnemonic, upper-cased (`XIC`, `TON`).
    pub name: String,
    /// Operands as written, trimmed, split at top-level commas.
    pub args: Vec<String>,
    /// 1-based column of the mnemonic in the rung text.
    pub col: usize,
}

/// An instruction operand.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// `?`: a pseudo-operand left as the tag holds it.
    Unknown,
    /// An immediate integer, as the controller stores it: 32 bits
    /// (RM003 pp.850-851; other radixes zero-fill).
    Int(i32),
    /// An immediate REAL.
    Real(f32),
    Path(Path),
}

/// A tag reference: `Tag`, `Tag.Member`, `Arr[3]`, `Word.5`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub base: String,
    pub segs: Vec<Seg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Member(String),
    /// `[i]` or `[i,j]`.
    Index(Vec<Index>),
    /// `.n`: bit n of an integer.
    Bit(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Index {
    Const(i64),
    /// A tag as the subscript (indirect addressing).
    Tag(String),
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.base)?;
        for s in &self.segs {
            match s {
                Seg::Member(m) => write!(f, ".{m}")?,
                Seg::Bit(b) => write!(f, ".{b}")?,
                Seg::Index(ix) => {
                    f.write_str("[")?;
                    for (k, i) in ix.iter().enumerate() {
                        if k > 0 {
                            f.write_str(",")?;
                        }
                        match i {
                            Index::Const(n) => write!(f, "{n}")?,
                            Index::Tag(t) => f.write_str(t)?,
                        }
                    }
                    f.write_str("]")?;
                }
            }
        }
        Ok(())
    }
}

/// A CPT expression (RM003 p.403), the subset converted: `+ - * / MOD`,
/// negation and parentheses over tags and immediates.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i32),
    Real(f32),
    Path(Path),
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    instrs: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn at(&self) -> String {
        format!("column {}", self.i + 1)
    }

    /// Elements up to (not including) `,`, `]` or `;`.
    fn elems(&mut self, depth: usize) -> Result<Vec<Elem>, String> {
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => return Err("the rung does not end with ';'".into()),
                Some(b',' | b']' | b';') => return Ok(out),
                Some(b'[') => {
                    if depth >= MAX_NEST {
                        return Err(format!("branches nested deeper than {MAX_NEST}"));
                    }
                    self.i += 1;
                    let mut legs = vec![self.elems(depth + 1)?];
                    loop {
                        match self.peek() {
                            Some(b',') => {
                                self.i += 1;
                                legs.push(self.elems(depth + 1)?);
                            }
                            Some(b']') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(format!("a branch is not closed with ']' ({})", self.at())),
                        }
                    }
                    out.push(Elem::Branch(legs));
                }
                Some(c) if c.is_ascii_alphabetic() || c == b'_' => out.push(Elem::Instr(self.instr()?)),
                Some(c) => {
                    return Err(format!("unexpected {:?} at {}", char::from(c), self.at()));
                }
            }
        }
    }

    fn instr(&mut self) -> Result<Instr, String> {
        self.instrs += 1;
        if self.instrs > MAX_INSTRS {
            return Err(format!("more than {MAX_INSTRS} instructions on one rung"));
        }
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.i += 1;
        }
        let name = String::from_utf8_lossy(&self.s[start..self.i]).to_ascii_uppercase();
        if self.s.get(self.i) != Some(&b'(') {
            return Err(format!("{name} at column {} has no '(' after it", start + 1));
        }
        self.i += 1;
        // Operands: split at commas outside () and [].
        let mut args = Vec::new();
        let mut cur = self.i;
        let mut depth = 0usize;
        loop {
            match self.s.get(self.i) {
                None | Some(b';') => {
                    return Err(format!(
                        "{name} at column {}: operands not closed with ')'",
                        start + 1
                    ));
                }
                Some(b'(' | b'[') => depth += 1,
                Some(b')') if depth == 0 => {
                    args.push(&self.s[cur..self.i]);
                    self.i += 1;
                    break;
                }
                Some(b')' | b']') => depth = depth.saturating_sub(1),
                Some(b',') if depth == 0 => {
                    args.push(&self.s[cur..self.i]);
                    cur = self.i + 1;
                }
                _ => {}
            }
            self.i += 1;
        }
        let mut args: Vec<String> = args
            .into_iter()
            .map(|a| String::from_utf8_lossy(a).trim().to_string())
            .collect();
        if args.len() == 1 && args[0].is_empty() {
            args.clear();
        }
        Ok(Instr {
            name,
            args,
            col: start + 1,
        })
    }
}

/// Parses a rung's neutral text.
///
/// # Errors
/// Why the text is not a rung, in words.
pub fn parse(text: &str) -> Result<Rung, String> {
    if !text.is_ascii() {
        return Err("the rung text has characters outside ASCII".into());
    }
    let mut p = P {
        s: text.as_bytes(),
        i: 0,
        instrs: 0,
    };
    let elems = p.elems(0)?;
    match p.peek() {
        Some(b';') => p.i += 1,
        Some(c) => return Err(format!("unexpected {:?} at {}", char::from(c), p.at())),
        None => return Err("the rung does not end with ';'".into()),
    }
    if p.peek().is_some() {
        return Err(format!("text after the rung's ';' at {}", p.at()));
    }
    Ok(elems)
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// An immediate number: decimal (`-5`, `1_000`), radix (`16#FF`,
/// `2#1010`; at most 32 bits, zero-filled), or REAL (`1.5`, `2.0e-3`).
fn number(s: &str) -> Option<Operand> {
    let t: String = s.chars().filter(|c| *c != '_').collect();
    if let Some((radix, digits)) = t.split_once('#') {
        let r = match radix {
            "2" => 2,
            "8" => 8,
            "16" => 16,
            _ => return None,
        };
        let v = u64::from_str_radix(digits, r).ok()?;
        let v = u32::try_from(v).ok()?;
        return Some(Operand::Int(v.cast_signed()));
    }
    let body = t.strip_prefix(['-', '+']).unwrap_or(&t);
    if body.is_empty() || !body.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    if body.bytes().all(|c| c.is_ascii_digit()) {
        let v: i64 = t.parse().ok()?;
        return i32::try_from(v).ok().map(Operand::Int);
    }
    if !body
        .bytes()
        .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let v = t.parse::<f64>().ok()? as f32;
    v.is_finite().then_some(Operand::Real(v))
}

/// Reads a tag path starting at `s[*i]`.
fn path_at(s: &[u8], i: &mut usize) -> Result<Path, String> {
    let ident = |s: &[u8], i: &mut usize| -> Option<String> {
        if !s.get(*i).copied().is_some_and(is_ident_start) {
            return None;
        }
        let st = *i;
        while s.get(*i).copied().is_some_and(is_ident) {
            *i += 1;
        }
        Some(String::from_utf8_lossy(&s[st..*i]).into_owned())
    };
    let Some(base) = ident(s, i) else {
        return Err("expected a tag name".into());
    };
    let mut segs = Vec::new();
    loop {
        match s.get(*i) {
            Some(b'.') => {
                *i += 1;
                if s.get(*i).is_some_and(u8::is_ascii_digit) {
                    let st = *i;
                    while s.get(*i).is_some_and(u8::is_ascii_digit) {
                        *i += 1;
                    }
                    let n: u32 = std::str::from_utf8(&s[st..*i])
                        .ok()
                        .and_then(|t| t.parse().ok())
                        .ok_or("a bit number out of range")?;
                    segs.push(Seg::Bit(n));
                } else {
                    let m = ident(s, i).ok_or("expected a member name after '.'")?;
                    segs.push(Seg::Member(m));
                }
            }
            Some(b'[') => {
                *i += 1;
                let mut ix = Vec::new();
                loop {
                    while s.get(*i).is_some_and(u8::is_ascii_whitespace) {
                        *i += 1;
                    }
                    let st = *i;
                    while s
                        .get(*i)
                        .is_some_and(|c| is_ident(*c) || *c == b'-' || *c == b'#')
                    {
                        *i += 1;
                    }
                    let t = std::str::from_utf8(&s[st..*i]).unwrap_or("");
                    if t.is_empty() {
                        return Err("an array subscript that is not a number or a tag".into());
                    }
                    if t.as_bytes()[0].is_ascii_digit() || t.starts_with('-') {
                        match number(t) {
                            Some(Operand::Int(n)) => ix.push(Index::Const(i64::from(n))),
                            _ => return Err(format!("array subscript {t} is not an integer")),
                        }
                    } else if t.bytes().all(is_ident) {
                        ix.push(Index::Tag(t.to_string()));
                    } else {
                        return Err(format!("array subscript {t} is not a number or a tag"));
                    }
                    while s.get(*i).is_some_and(u8::is_ascii_whitespace) {
                        *i += 1;
                    }
                    match s.get(*i) {
                        Some(b',') => *i += 1,
                        Some(b']') => {
                            *i += 1;
                            break;
                        }
                        _ => return Err("an array subscript that is an expression".into()),
                    }
                }
                segs.push(Seg::Index(ix));
            }
            _ => return Ok(Path { base, segs }),
        }
    }
}

/// Reads one operand.
///
/// # Errors
/// Why it is not a tag, an immediate or `?`.
pub fn operand(s: &str) -> Result<Operand, String> {
    let s = s.trim();
    if s == "?" {
        return Ok(Operand::Unknown);
    }
    if s.is_empty() {
        return Err("an empty operand".into());
    }
    let b = s.as_bytes();
    if b[0].is_ascii_digit() || b[0] == b'-' || b[0] == b'+' || b[0] == b'.' {
        return number(s).ok_or_else(|| format!("{s} is not a number this converter reads"));
    }
    if s.len() > 2 && s[..2].eq_ignore_ascii_case("S:") {
        return Err(format!(
            "{s} is a controller status flag; status flags are not converted"
        ));
    }
    if s.contains(':') {
        return Err(format!(
            "{s} is an I/O module tag (or a program-qualified name), which is not converted"
        ));
    }
    let mut i = 0;
    let p = path_at(b, &mut i)?;
    if i != b.len() {
        return Err(format!("{s} is not a tag path"));
    }
    Ok(Operand::Path(p))
}

/// Parses a CPT expression.
///
/// # Errors
/// Why it is not one in the converted subset.
pub fn expr(s: &str) -> Result<Expr, String> {
    let mut e = E {
        s: s.as_bytes(),
        i: 0,
        depth: 0,
    };
    let x = e.sum()?;
    e.ws();
    if e.i != e.s.len() {
        return Err(format!("unexpected text at column {} of the expression", e.i + 1));
    }
    Ok(x)
}

struct E<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl E<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn word(&self) -> &str {
        let mut j = self.i;
        while self.s.get(j).copied().is_some_and(is_ident) {
            j += 1;
        }
        std::str::from_utf8(&self.s[self.i..j]).unwrap_or("")
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_NEST {
            return Err(format!("the expression nests deeper than {MAX_NEST}"));
        }
        Ok(())
    }

    fn sum(&mut self) -> Result<Expr, String> {
        self.deeper()?;
        let mut x = self.term()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) {
                Some(b'+') => Op::Add,
                Some(b'-') => Op::Sub,
                _ => break,
            };
            self.i += 1;
            let y = self.term()?;
            x = Expr::Bin(op, Box::new(x), Box::new(y));
        }
        self.depth -= 1;
        Ok(x)
    }

    fn term(&mut self) -> Result<Expr, String> {
        let mut x = self.unary()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) {
                Some(b'*') if self.s.get(self.i + 1) == Some(&b'*') => {
                    return Err("** (power) is not converted".into());
                }
                Some(b'*') => Op::Mul,
                Some(b'/') => Op::Div,
                Some(c) if c.is_ascii_alphabetic() && self.word().eq_ignore_ascii_case("MOD") => {
                    self.i += 2;
                    Op::Mod
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    let w = self.word().to_ascii_uppercase();
                    return Err(format!("the operator {w} is not converted"));
                }
                _ => break,
            };
            self.i += 1;
            let y = self.unary()?;
            x = Expr::Bin(op, Box::new(x), Box::new(y));
        }
        Ok(x)
    }

    /// An immediate; `neg` when a '-' came just before it (so
    /// -2147483648 reads).
    fn num(&mut self, neg: bool) -> Result<Expr, String> {
        let st = self.i;
        while self.s.get(self.i).is_some_and(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, b'.' | b'_' | b'#')
                || ((*c == b'+' || *c == b'-')
                    && matches!(self.s.get(self.i.wrapping_sub(1)), Some(b'e' | b'E'))
                    && !self.s[st..self.i].contains(&b'#'))
        }) {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[st..self.i]).unwrap_or("");
        let t = if neg { format!("-{t}") } else { t.to_string() };
        match number(&t) {
            Some(Operand::Int(n)) => Ok(Expr::Int(n)),
            Some(Operand::Real(r)) => Ok(Expr::Real(r)),
            _ => Err(format!("{t} is not a number this converter reads")),
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        self.ws();
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
            if self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                return self.num(true);
            }
            self.deeper()?;
            let x = self.unary()?;
            self.depth -= 1;
            return Ok(Expr::Neg(Box::new(x)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, String> {
        self.ws();
        match self.s.get(self.i) {
            None => Err("the expression ends early".into()),
            Some(b'(') => {
                self.i += 1;
                let x = self.sum()?;
                self.ws();
                if self.s.get(self.i) != Some(&b')') {
                    return Err("a '(' is not closed in the expression".into());
                }
                self.i += 1;
                Ok(x)
            }
            Some(c) if c.is_ascii_digit() || *c == b'.' => self.num(false),
            Some(c) if is_ident_start(*c) => {
                let w = self.word().to_ascii_uppercase();
                let mut j = self.i + w.len();
                while self.s.get(j).is_some_and(u8::is_ascii_whitespace) {
                    j += 1;
                }
                if self.s.get(j) == Some(&b'(') || matches!(w.as_str(), "NOT" | "AND" | "OR" | "XOR") {
                    return Err(format!("{w} is not converted in an expression"));
                }
                let p = path_at(self.s, &mut self.i)?;
                if self.s.get(self.i) == Some(&b':') {
                    return Err("an I/O module tag in an expression".into());
                }
                Ok(Expr::Path(p))
            }
            Some(c) => Err(format!("unexpected {:?} in the expression", char::from(*c))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(name: &str, args: &[&str]) -> Elem {
        Elem::Instr(Instr {
            name: name.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            col: 0,
        })
    }

    fn strip(r: Rung) -> Rung {
        r.into_iter()
            .map(|e| match e {
                Elem::Instr(mut x) => {
                    x.col = 0;
                    Elem::Instr(x)
                }
                Elem::Branch(legs) => Elem::Branch(legs.into_iter().map(strip).collect()),
            })
            .collect()
    }

    #[test]
    fn rm014_examples_parse() {
        // RM014 p.211: a single branch with an empty leg, and two legs.
        let r = parse("XIC(conveyor_a)[,XIC(input_1) XIO(input_2) ]OTE(light_1);").unwrap();
        assert_eq!(
            strip(r),
            vec![
                i("XIC", &["conveyor_a"]),
                Elem::Branch(vec![vec![], vec![i("XIC", &["input_1"]), i("XIO", &["input_2"])]]),
                i("OTE", &["light_1"]),
            ]
        );
        let r = parse("XIC(input1)XIC(input2)OTE(output1)OTE(output2);").unwrap();
        assert_eq!(r.len(), 4);
        let r = parse("CPT(Dest,(a + b[1,2]) * 2);TON(T,?,?)").unwrap_err();
        assert!(r.contains("after the rung"), "{r}");
        let r = parse(" ; ").unwrap();
        assert_eq!(r, Vec::<Elem>::new());
    }

    #[test]
    fn operands() {
        assert_eq!(operand("?"), Ok(Operand::Unknown));
        assert_eq!(operand("-5"), Ok(Operand::Int(-5)));
        assert_eq!(operand("16#FFFF_FFFF"), Ok(Operand::Int(-1)));
        assert_eq!(operand("16#ffff"), Ok(Operand::Int(65535)));
        assert_eq!(operand("8#1234"), Ok(Operand::Int(668)));
        assert_eq!(operand("2#1010"), Ok(Operand::Int(10)));
        assert_eq!(operand("1.5e+002"), Ok(Operand::Real(150.0)));
        assert!(operand("2147483648").is_err());
        assert!(operand("16#1_0000_0000").is_err());
        assert_eq!(
            operand("T1.DN"),
            Ok(Operand::Path(Path {
                base: "T1".into(),
                segs: vec![Seg::Member("DN".into())]
            }))
        );
        assert_eq!(
            operand("a[1,i].5"),
            Ok(Operand::Path(Path {
                base: "a".into(),
                segs: vec![
                    Seg::Index(vec![Index::Const(1), Index::Tag("i".into())]),
                    Seg::Bit(5)
                ]
            }))
        );
        assert!(operand("Local:1:I.Data.3").unwrap_err().contains("I/O module"));
        assert!(operand("a[i+1]").is_err());
        assert!(operand("a b").is_err());
    }

    #[test]
    fn expressions() {
        let e = expr("a + b * -2 MOD 3").unwrap();
        assert!(matches!(e, Expr::Bin(Op::Add, _, _)));
        assert!(expr("a ** 2").is_err());
        assert!(expr("ABS(a)").is_err());
        assert!(expr("a AND b").is_err());
        assert!(expr("(a").is_err());
        assert!(expr(&"(".repeat(100)).unwrap_err().contains("deeper"));
        assert_eq!(expr("1.0e-3"), Ok(Expr::Real(1.0e-3)));
    }

    #[test]
    fn hostile_rungs_are_bounded() {
        let deep = "[".repeat(10_000) + ";";
        assert!(parse(&deep).unwrap_err().contains("deeper"));
        let many = "XIC(a)".repeat(MAX_INSTRS + 1) + ";";
        assert!(parse(&many).unwrap_err().contains("instructions"));
        assert!(parse("XIC(a").is_err());
        assert!(parse("XIC a;").is_err());
        assert!(parse("[XIC(a);").is_err());
        assert!(parse("XIC(a)]").is_err());
        assert!(parse("XIC(é);").is_err());
    }
}
