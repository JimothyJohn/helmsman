//! Expressions back to source-like text, for labels and explanations.
//! Names come out upper-cased (the lexer folds them); parentheses only
//! where precedence needs them.

use helmsman_iec::ast::{BinOp, CaseLabel, Expr, ExprKind, LabelValue, UnOp};

const fn prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::Xor => 2,
        BinOp::And => 3,
        BinOp::Eq | BinOp::Ne => 4,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 5,
        BinOp::Add | BinOp::Sub => 6,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 7,
        BinOp::Pow => 8,
    }
}

const UNARY: u8 = 9;

fn level(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Binary(op, ..) => prec(*op),
        ExprKind::Unary(..) => UNARY,
        _ => u8::MAX,
    }
}

/// A TIME literal in the largest unit that divides it.
#[must_use]
pub fn time(ns: i128) -> String {
    const UNITS: [(i128, &str); 6] = [
        (3_600_000_000_000, "h"),
        (60_000_000_000, "m"),
        (1_000_000_000, "s"),
        (1_000_000, "ms"),
        (1_000, "us"),
        (1, "ns"),
    ];
    let (k, u) = UNITS
        .iter()
        .find(|(k, _)| ns % k == 0)
        .copied()
        .unwrap_or((1, "ns"));
    format!("T#{}{u}", ns / k)
}

/// Source text of `e`.
#[must_use]
pub fn expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int { value, prefix } => match prefix {
            Some(p) => format!("{p}#{value}"),
            None => value.to_string(),
        },
        ExprKind::Real { value, prefix } => match prefix {
            Some(p) => format!("{p}#{value:?}"),
            None => format!("{value:?}"),
        },
        ExprKind::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).to_string(),
        ExprKind::Time(ns) => time(*ns),
        ExprKind::EnumLit(t, v) => format!("{t}#{v}"),
        ExprKind::Var(v) => v.clone(),
        ExprKind::Direct(d) => format!("%{d}"),
        ExprKind::Member(b, m) => format!("{}.{m}", operand(b, u8::MAX)),
        ExprKind::Bit(b, n) => format!("{}.{n}", operand(b, u8::MAX)),
        ExprKind::Index(b, ix) => format!(
            "{}[{}]",
            operand(b, u8::MAX),
            ix.iter().map(expr).collect::<Vec<_>>().join(",")
        ),
        ExprKind::Unary(UnOp::Neg, a) => format!("-{}", operand(a, UNARY)),
        ExprKind::Unary(UnOp::Not, a) => format!("NOT {}", operand(a, UNARY)),
        ExprKind::Binary(op, a, b) => {
            let p = prec(*op);
            // Left-associative: an equal-precedence right operand needs parentheses.
            format!("{} {} {}", operand(a, p), op.symbol(), operand(b, p + 1))
        }
        ExprKind::Call(f, args) => {
            let a: Vec<String> = args
                .iter()
                .map(|a| match &a.name {
                    Some(n) => format!("{n} {} {}", if a.output { "=>" } else { ":=" }, expr(&a.value)),
                    None => expr(&a.value),
                })
                .collect();
            format!("{}({})", operand(f, u8::MAX), a.join(", "))
        }
    }
}

fn operand(e: &Expr, min: u8) -> String {
    if level(e) < min {
        format!("({})", expr(e))
    } else {
        expr(e)
    }
}

/// A CASE arm's labels: `1, 3..5, RED`.
#[must_use]
pub fn labels(ls: &[CaseLabel]) -> String {
    let one = |v: &LabelValue| match v {
        LabelValue::Int(i) => i.to_string(),
        LabelValue::Name(n) => n.clone(),
    };
    ls.iter()
        .map(|l| {
            if l.lo == l.hi {
                one(&l.lo)
            } else {
                format!("{}..{}", one(&l.lo), one(&l.hi))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
