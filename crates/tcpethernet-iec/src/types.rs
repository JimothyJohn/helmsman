//! Elementary data types, their value ranges and the implicit (widening)
//! conversions between them.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ty {
    Bool,
    SInt,
    Int,
    DInt,
    LInt,
    USInt,
    UInt,
    UDInt,
    ULInt,
    Byte,
    Word,
    DWord,
    LWord,
    Real,
    LReal,
    /// Duration, held as signed 64-bit nanoseconds.
    Time,
}

/// Every type, in the order common supertypes are searched.
const ALL: [Ty; 16] = [
    Ty::Bool,
    Ty::SInt,
    Ty::USInt,
    Ty::Byte,
    Ty::Int,
    Ty::UInt,
    Ty::Word,
    Ty::DInt,
    Ty::UDInt,
    Ty::DWord,
    Ty::Real,
    Ty::LInt,
    Ty::ULInt,
    Ty::LWord,
    Ty::LReal,
    Ty::Time,
];

impl Ty {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter().find(|t| t.name() == name)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "BOOL",
            Self::SInt => "SINT",
            Self::Int => "INT",
            Self::DInt => "DINT",
            Self::LInt => "LINT",
            Self::USInt => "USINT",
            Self::UInt => "UINT",
            Self::UDInt => "UDINT",
            Self::ULInt => "ULINT",
            Self::Byte => "BYTE",
            Self::Word => "WORD",
            Self::DWord => "DWORD",
            Self::LWord => "LWORD",
            Self::Real => "REAL",
            Self::LReal => "LREAL",
            Self::Time => "TIME",
        }
    }

    /// Storage width in bits (BOOL is 1).
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            Self::Bool => 1,
            Self::SInt | Self::USInt | Self::Byte => 8,
            Self::Int | Self::UInt | Self::Word => 16,
            Self::DInt | Self::UDInt | Self::DWord | Self::Real => 32,
            Self::LInt | Self::ULInt | Self::LWord | Self::LReal | Self::Time => 64,
        }
    }

    #[must_use]
    pub const fn is_signed_int(self) -> bool {
        matches!(self, Self::SInt | Self::Int | Self::DInt | Self::LInt)
    }

    #[must_use]
    pub const fn is_unsigned_int(self) -> bool {
        matches!(self, Self::USInt | Self::UInt | Self::UDInt | Self::ULInt)
    }

    /// `ANY_INT`.
    #[must_use]
    pub const fn is_int(self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }

    /// BYTE, WORD, DWORD, LWORD.
    #[must_use]
    pub const fn is_bitstring(self) -> bool {
        matches!(self, Self::Byte | Self::Word | Self::DWord | Self::LWord)
    }

    /// `ANY_BIT`: BOOL and the bit strings.
    #[must_use]
    pub const fn is_bit(self) -> bool {
        matches!(self, Self::Bool) || self.is_bitstring()
    }

    /// `ANY_REAL`.
    #[must_use]
    pub const fn is_real(self) -> bool {
        matches!(self, Self::Real | Self::LReal)
    }

    /// `ANY_NUM`.
    #[must_use]
    pub const fn is_num(self) -> bool {
        self.is_int() || self.is_real()
    }

    /// Integer-represented types: `ANY_INT`, the bit strings and BOOL.
    #[must_use]
    pub const fn is_integral(self) -> bool {
        self.is_int() || self.is_bit()
    }

    /// Signed two's-complement representation: the signed integers and TIME.
    const fn is_signed(self) -> bool {
        self.is_signed_int() || matches!(self, Self::Time)
    }

    /// Inclusive value range of an integral type.
    #[must_use]
    pub fn range(self) -> (i128, i128) {
        let n = self.bits();
        if self.is_signed() {
            (-(1i128 << (n - 1)), (1i128 << (n - 1)) - 1)
        } else {
            (0, (1i128 << n) - 1)
        }
    }

    /// Reduces `v` modulo 2^bits into this type's range (two's complement
    /// for signed types). Identity for values already in range.
    #[must_use]
    pub fn wrap(self, v: i128) -> i128 {
        let n = self.bits();
        let mask = (1i128 << n) - 1;
        let u = v & mask;
        if self.is_signed() && u >= 1i128 << (n - 1) {
            u - (1i128 << n)
        } else {
            u
        }
    }

    /// Implicit conversion `self` -> `to` without loss (reflexive).
    #[must_use]
    pub fn widens_to(self, to: Self) -> bool {
        if self == to {
            return true;
        }
        let (a, b) = (self.bits(), to.bits());
        match (self, to) {
            (s, t) if s.is_signed_int() && t.is_signed_int() => b > a,
            (s, t) if s.is_unsigned_int() && t.is_unsigned_int() => b > a,
            (s, t) if s.is_unsigned_int() && t.is_signed_int() => b > a,
            (s, t) if s.is_bitstring() && t.is_bitstring() => b > a,
            (s, Self::Real) if s.is_int() => a <= 16,
            (s, Self::LReal) if s.is_int() => a <= 32,
            (Self::Real, Self::LReal) => true,
            _ => false,
        }
    }

    /// The narrowest type both widen to, if any.
    #[must_use]
    pub fn common(a: Self, b: Self) -> Option<Self> {
        ALL.into_iter().find(|&t| a.widens_to(t) && b.widens_to(t))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A runtime value. Integral types (integers and bit strings) are
/// `Int`, always within their type's range; REAL is held as an `f64` that is
/// exactly representable as `f32`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i128),
    Real(f64),
}

impl Value {
    #[must_use]
    pub const fn zero(ty: Ty) -> Self {
        match ty {
            Ty::Bool => Self::Bool(false),
            Ty::Real | Ty::LReal => Self::Real(0.0),
            _ => Self::Int(0),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(b) => f.write_str(if *b { "TRUE" } else { "FALSE" }),
            Self::Int(i) => write!(f, "{i}"),
            Self::Real(r) => write!(f, "{r:?}"),
        }
    }
}

/// Rounds to the nearest `f32` when `ty` is REAL.
#[allow(clippy::cast_possible_truncation)]
pub fn round_real(ty: Ty, x: f64) -> f64 {
    if ty == Ty::Real { f64::from(x as f32) } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_wrap() {
        assert_eq!(Ty::SInt.range(), (-128, 127));
        assert_eq!(Ty::ULInt.range(), (0, i128::from(u64::MAX)));
        assert_eq!(Ty::SInt.wrap(128), -128);
        assert_eq!(Ty::SInt.wrap(-129), 127);
        assert_eq!(Ty::Byte.wrap(-1), 255);
        assert_eq!(Ty::LInt.wrap(i128::from(i64::MAX) + 1), i128::from(i64::MIN));
        assert_eq!(Ty::ULInt.wrap(-1), i128::from(u64::MAX));
    }

    #[test]
    fn widening_never_loses_range() {
        for a in ALL {
            for b in ALL {
                if a == b || !a.widens_to(b) || !a.is_integral() || !b.is_integral() {
                    continue;
                }
                let (lo, hi) = a.range();
                let (tlo, thi) = b.range();
                assert!(tlo <= lo && hi <= thi, "{a} -> {b}");
            }
        }
    }

    #[test]
    fn widening_table() {
        assert!(Ty::SInt.widens_to(Ty::LInt));
        assert!(Ty::USInt.widens_to(Ty::Int));
        assert!(!Ty::UInt.widens_to(Ty::Int));
        assert!(!Ty::Int.widens_to(Ty::UDInt));
        assert!(Ty::Int.widens_to(Ty::Real));
        assert!(!Ty::DInt.widens_to(Ty::Real));
        assert!(Ty::DInt.widens_to(Ty::LReal));
        assert!(!Ty::LInt.widens_to(Ty::LReal));
        assert!(!Ty::Byte.widens_to(Ty::Int));
        assert!(!Ty::Bool.widens_to(Ty::Byte));
        assert_eq!(Ty::common(Ty::SInt, Ty::USInt), Some(Ty::Int));
        assert_eq!(Ty::common(Ty::UInt, Ty::SInt), Some(Ty::DInt));
        assert_eq!(Ty::common(Ty::Int, Ty::Real), Some(Ty::Real));
        assert_eq!(Ty::common(Ty::DInt, Ty::Real), Some(Ty::LReal));
        assert_eq!(Ty::common(Ty::ULInt, Ty::LInt), None);
        assert_eq!(Ty::common(Ty::Word, Ty::Int), None);
    }

    #[test]
    fn real_rounds_through_f32() {
        assert_eq!(round_real(Ty::Real, 0.1), f64::from(0.1f32));
        assert_eq!(round_real(Ty::LReal, 0.1), 0.1);
    }
}
