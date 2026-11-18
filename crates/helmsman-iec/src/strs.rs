//! `STRING[n]` storage and the string functions.
//!
//! A string variable is a fixed region of the slot array: `slots(n)`
//! consecutive slots holding, as 16 bytes each, a length byte and then the
//! characters, everything past the length zero. So a string moves with the
//! rest of a frame (a struct or array copy, a by-value argument, a function
//! block instance, a hot swap) and two equal strings are equal slot for
//! slot. Nothing here allocates: operations work in stack buffers of
//! [`MAX`] bytes, and a result that does not fit its destination is cut at
//! the destination's capacity.
//!
//! Positions are 1-based and counts are clamped, never faults: a count
//! below 1 selects nothing, one past the end selects to the end (docs on
//! each function). Comparison is by byte value, a prefix sorting first.

#[cfg(not(feature = "std"))]
use crate::prelude::*;

use core::cmp::Ordering;
use core::fmt::Write;

use crate::parser::BinOp;
use crate::types::{Ty, Value, round_real};

/// The most characters a string holds.
pub const MAX: usize = crate::lexer::MAX_STRING;

/// What a bare `STRING` holds.
pub const DEFAULT_CAP: u16 = 80;

/// Slots a string of capacity `cap` takes.
#[must_use]
pub const fn slots(cap: usize) -> usize {
    (cap + 1).div_ceil(16)
}

/// A stack buffer for one string's bytes.
pub type Buf = [u8; MAX];

#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn word(v: Value) -> u128 {
    match v {
        Value::Int(i) => i as u128,
        _ => 0,
    }
}

/// The text at `at` in `mem`: its length, with the bytes in `out`.
pub fn get(mem: &[Value], at: usize, out: &mut Buf) -> usize {
    let first = mem.get(at).copied().map_or(0, word);
    #[allow(clippy::cast_possible_truncation)]
    let len = (first & 0xFF) as usize;
    for (k, b) in out.iter_mut().take(len).enumerate() {
        let i = k + 1;
        let w = mem.get(at + i / 16).copied().map_or(0, word);
        #[allow(clippy::cast_possible_truncation)]
        {
            *b = (w >> ((i % 16) * 8)) as u8;
        }
    }
    len
}

/// The initial slots of a string of capacity `cap` holding `text` (cut at
/// `cap`).
#[must_use]
pub fn encode(cap: usize, text: &[u8]) -> Vec<Value> {
    let text = &text[..text.len().min(cap).min(MAX)];
    #[allow(clippy::cast_possible_wrap)]
    (0..slots(cap))
        .map(|s| Value::Int(word_at(text, s) as i128))
        .collect()
}

/// Slot `s` of a string holding `text` (already cut at its capacity).
fn word_at(text: &[u8], s: usize) -> u128 {
    let mut w = 0u128;
    for j in 0..16 {
        let i = s * 16 + j;
        let byte = if i == 0 {
            text.len() as u128
        } else {
            text.get(i - 1).copied().map_or(0, u128::from)
        };
        w |= byte << (j * 8);
    }
    w
}

/// Writes `text` (cut at `cap`) into the region at `at`. Allocates nothing.
pub fn put(mem: &mut [Value], at: usize, cap: usize, text: &[u8]) {
    let text = &text[..text.len().min(cap).min(MAX)];
    for s in 0..slots(cap) {
        if let Some(dst) = mem.get_mut(at + s) {
            #[allow(clippy::cast_possible_wrap)]
            {
                *dst = Value::Int(word_at(text, s) as i128);
            }
        }
    }
}

/// The text at `at`, as an owned byte vector.
#[must_use]
pub fn read(mem: &[Value], at: usize) -> Vec<u8> {
    let mut b = [0u8; MAX];
    let n = get(mem, at, &mut b);
    b[..n].to_vec()
}

/// Whether `slots` could be a string of capacity `cap`: a length within
/// the capacity and nothing but zero after the text.
#[must_use]
pub fn is_valid(slots_of: &[Value], cap: usize) -> bool {
    if slots_of.len() != slots(cap) || slots_of.iter().any(|v| !matches!(v, Value::Int(_))) {
        return false;
    }
    let mut b = [0u8; MAX];
    let n = get(slots_of, 0, &mut b);
    n <= cap && encode(cap, &b[..n]) == slots_of
}

/// A string function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrFn {
    /// `LEN(s)`: INT.
    Len,
    /// `LEFT(s, l)`.
    Left,
    /// `RIGHT(s, l)`.
    Right,
    /// `MID(s, l, p)`.
    Mid,
    /// `CONCAT(a, b)`; more arguments nest.
    Concat,
    /// `FIND(s, sub)`: INT.
    Find,
    /// `INSERT(s, ins, p)`.
    Insert,
    /// `DELETE(s, l, p)`.
    Delete,
    /// `REPLACE(s, ins, l, p)`.
    Replace,
    /// `=`, `<>`, `<`, `>`, `<=`, `>=` on two strings: BOOL.
    Cmp(BinOp),
    /// A number or BOOL as text.
    FromNum(Ty),
    /// Text as a number or BOOL.
    ToNum(Ty),
}

impl StrFn {
    /// How many of the arguments, from the first, are strings.
    #[must_use]
    pub const fn strings(self) -> usize {
        match self {
            Self::FromNum(_) => 0,
            Self::Len | Self::Left | Self::Right | Self::Mid | Self::Delete | Self::ToNum(_) => 1,
            Self::Concat | Self::Find | Self::Insert | Self::Replace | Self::Cmp(_) => 2,
        }
    }
}

fn clamp(n: i128) -> usize {
    usize::try_from(n.clamp(0, 1_000_000)).unwrap_or(0)
}

/// `l` leftmost characters.
fn left(s: &[u8], l: i128) -> &[u8] {
    &s[..clamp(l).min(s.len())]
}

fn right(s: &[u8], l: i128) -> &[u8] {
    &s[s.len() - clamp(l).min(s.len())..]
}

/// `l` characters from position `p` (1-based); nothing for `p` below 1 or
/// past the end.
fn mid(s: &[u8], l: i128, p: i128) -> &[u8] {
    if p < 1 || p > i128::try_from(s.len()).unwrap_or(0) {
        return &[];
    }
    let from = clamp(p - 1);
    let to = from.saturating_add(clamp(l)).min(s.len());
    &s[from..to]
}

/// The 1-based position of the first `sub` in `s`, 0 if none or `sub` is
/// empty.
fn find(s: &[u8], sub: &[u8]) -> usize {
    if sub.is_empty() || sub.len() > s.len() {
        return 0;
    }
    s.windows(sub.len()).position(|w| w == sub).map_or(0, |i| i + 1)
}

/// Pushes `part` onto `out` (length `n`), cut at `cap`.
fn push(out: &mut Buf, n: &mut usize, cap: usize, part: &[u8]) {
    let room = cap.min(MAX).saturating_sub(*n);
    let k = part.len().min(room);
    out[*n..*n + k].copy_from_slice(&part[..k]);
    *n += k;
}

/// `ins` placed after position `p` of `s` (0 puts it first, past the end
/// last), with `del` characters of `s` from there dropped first.
fn splice(s: &[u8], ins: &[u8], after: i128, del: usize, cap: usize, out: &mut Buf) -> usize {
    let at = clamp(after).min(s.len());
    let rest = at.saturating_add(del).min(s.len());
    let mut n = 0;
    push(out, &mut n, cap, &s[..at]);
    push(out, &mut n, cap, ins);
    push(out, &mut n, cap, &s[rest..]);
    n
}

fn order(a: &[u8], b: &[u8]) -> Ordering {
    a.cmp(b)
}

/// A decimal or text rendering into a small stack buffer.
struct Text {
    b: [u8; 40],
    n: usize,
}

impl Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

/// The capacity of the text [`StrFn::FromNum`] makes for a `ty`.
#[must_use]
pub const fn num_cap(ty: Ty) -> u16 {
    match ty {
        Ty::Bool => 5,
        t if t.is_real() => 24,
        _ => 21,
    }
}

fn from_num(v: Value, out: &mut Buf) -> usize {
    let mut t = Text { b: [0; 40], n: 0 };
    let _ = write!(t, "{v}");
    out[..t.n].copy_from_slice(&t.b[..t.n]);
    t.n
}

/// Text as a value of `ty`: BOOL is `TRUE` or `1` (any case), an integer
/// an optional sign and digits that fit `ty`, a real a plain decimal or
/// exponent form that is finite. Anything else is 0, FALSE or 0.0.
fn to_num(s: &[u8], ty: Ty) -> Value {
    if ty == Ty::Bool {
        return Value::Bool(s == b"1" || s.eq_ignore_ascii_case(b"TRUE"));
    }
    if ty.is_real() {
        let ok = !s.is_empty()
            && s.iter()
                .all(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.' | b'e' | b'E'));
        let x = core::str::from_utf8(s)
            .ok()
            .filter(|_| ok)
            .and_then(|t| t.parse::<f64>().ok())
            .map(|x| round_real(ty, x))
            .filter(|x| x.is_finite());
        return Value::Real(x.unwrap_or(0.0));
    }
    let (neg, digits) = match s.split_first() {
        Some((b'-', d)) => (true, d),
        Some((b'+', d)) => (false, d),
        _ => (false, s),
    };
    if digits.is_empty() || digits.len() > 20 || !digits.iter().all(u8::is_ascii_digit) {
        return Value::Int(0);
    }
    let mut n: i128 = 0;
    for d in digits {
        n = n * 10 + i128::from(d - b'0');
    }
    let n = if neg { -n } else { n };
    let (lo, hi) = ty.range();
    Value::Int(if (lo..=hi).contains(&n) { n } else { 0 })
}

/// What a string function made, and the bytes it handled.
pub enum Made {
    /// A string of this many bytes, in the output buffer.
    Text(usize),
    Num(Value),
}

/// Applies `f`. `a` and `b` are the string arguments, `n` the integer
/// arguments, `v` the number a [`StrFn::FromNum`] renders; a string result
/// goes into `out`, cut at `cap`.
pub fn apply(f: StrFn, a: &[u8], b: &[u8], n: &[Value], v: Value, cap: usize, out: &mut Buf) -> Made {
    let int = |k: usize| match n.get(k) {
        Some(Value::Int(i)) => *i,
        _ => 0,
    };
    let mut len = 0;
    let text: &[u8] = match f {
        StrFn::Len => return Made::Num(Value::Int(i128::try_from(a.len()).unwrap_or(0))),
        StrFn::Find => return Made::Num(Value::Int(i128::try_from(find(a, b)).unwrap_or(0))),
        StrFn::Cmp(op) => {
            let o = order(a, b);
            return Made::Num(Value::Bool(match op {
                BinOp::Eq => o == Ordering::Equal,
                BinOp::Ne => o != Ordering::Equal,
                BinOp::Lt => o == Ordering::Less,
                BinOp::Le => o != Ordering::Greater,
                BinOp::Gt => o == Ordering::Greater,
                _ => o != Ordering::Less,
            }));
        }
        StrFn::ToNum(ty) => return Made::Num(to_num(a, ty)),
        StrFn::FromNum(_) => {
            return Made::Text(from_num(v, out).min(cap));
        }
        StrFn::Left => left(a, int(0)),
        StrFn::Right => right(a, int(0)),
        StrFn::Mid => mid(a, int(0), int(1)),
        StrFn::Concat => {
            push(out, &mut len, cap, a);
            push(out, &mut len, cap, b);
            return Made::Text(len);
        }
        StrFn::Insert => return Made::Text(splice(a, b, int(0), 0, cap, out)),
        StrFn::Delete => {
            let (l, p) = (int(0), int(1));
            let len_a = i128::try_from(a.len()).unwrap_or(0);
            return Made::Text(if p < 1 || p > len_a || l < 1 {
                splice(a, &[], len_a, 0, cap, out)
            } else {
                splice(a, &[], p - 1, clamp(l), cap, out)
            });
        }
        StrFn::Replace => {
            let (l, p) = (int(0), int(1));
            let len_a = i128::try_from(a.len()).unwrap_or(0);
            return Made::Text(if p < 1 || p > len_a + 1 {
                splice(a, &[], len_a, 0, cap, out)
            } else {
                splice(a, b, p - 1, clamp(l), cap, out)
            });
        }
    };
    push(out, &mut len, cap, text);
    Made::Text(len)
}
