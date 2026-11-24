//! Elementary data types, their value ranges and the implicit (widening)
//! conversions between them.

use core::fmt;

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
    /// Calendar date: nanoseconds since 1970-01-01, at midnight (see `dt`).
    Date,
    /// Time of day: nanoseconds since midnight, 0 up to one day.
    Tod,
    /// Date and time of day: nanoseconds since 1970-01-01T00:00:00.
    Dt,
}

/// Every type, in the order common supertypes are searched.
const ALL: [Ty; 19] = [
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
    Ty::Date,
    Ty::Tod,
    Ty::Dt,
];

impl Ty {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "TIME_OF_DAY" => return Some(Self::Tod),
            "DATE_AND_TIME" => return Some(Self::Dt),
            _ => {}
        }
        ALL.into_iter().find(|t| t.name() == name)
    }

    /// The names of every elementary type.
    pub fn names() -> impl Iterator<Item = &'static str> {
        ALL.into_iter().map(Self::name)
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
            Self::Date => "DATE",
            Self::Tod => "TOD",
            Self::Dt => "DT",
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
            Self::LInt
            | Self::ULInt
            | Self::LWord
            | Self::LReal
            | Self::Time
            | Self::Date
            | Self::Tod
            | Self::Dt => 64,
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

    /// DATE, TOD and DT: calendar values, held like TIME as 64-bit
    /// nanoseconds but not numbers (no conversion to or from one).
    #[must_use]
    pub const fn is_calendar(self) -> bool {
        matches!(self, Self::Date | Self::Tod | Self::Dt)
    }

    /// Signed two's-complement representation: the signed integers, TIME
    /// and the calendar types.
    const fn is_signed(self) -> bool {
        self.is_signed_int() || matches!(self, Self::Time) || self.is_calendar()
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

    /// The width binary integer arithmetic on this type is carried out in:
    /// SINT and INT as DINT, USINT and UINT as UDINT, others as themselves.
    #[must_use]
    pub const fn arith(self) -> Self {
        match self {
            Self::SInt | Self::Int => Self::DInt,
            Self::USInt | Self::UInt => Self::UDInt,
            t => t,
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

impl Value {
    /// The value as a read-out: a REAL at three significant figures
    /// ([`shown_real`]), everything else as `Display` prints it. Display
    /// only: what is written, sent or compared uses `Display`, which
    /// round-trips.
    #[must_use]
    pub fn shown(&self) -> String {
        match self {
            Self::Real(r) => shown_real(*r),
            v => v.to_string(),
        }
    }
}

/// A REAL or LREAL for a person to read: three significant figures with
/// the trailing zeros kept, so the text keeps its width as the value moves
/// (49.873512 reads 49.9, 50 reads 50.0, 5 reads 5.00). Whole digits are
/// never cut (12420.4 reads 12420) and a rounding that carries stays at
/// three figures (9.996 reads 10.0). Below 1e-4 and from 1e9 up it is
/// e-notation with three figures; NaN and the infinities are words.
#[must_use]
pub fn shown_real(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf" } else { "inf" }.into();
    }
    let a = x.abs();
    if a == 0.0 {
        return "0.00".into();
    }
    if !(1e-4..1e9).contains(&a) {
        return format!("{x:.2e}");
    }
    // The exponent after rounding, so a carry (9.996) counts as 10.0.
    let sci = format!("{a:.2e}");
    let exp: i32 = sci.rsplit('e').next().and_then(|e| e.parse().ok()).unwrap_or(0);
    let decimals = usize::try_from((2 - exp).max(0)).unwrap_or(0);
    format!("{x:.decimals$}")
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(b) => f.write_str(if *b { "TRUE" } else { "FALSE" }),
            Self::Int(i) => write!(f, "{i}"),
            Self::Real(r) => write_real(f, *r),
        }
    }
}

/// A REAL is an f32 held widened in an f64, so `{:?}` of it prints noise
/// (23.700000762939453 for 23.7). A value that is exactly an f32 prints
/// as that f32, its shortest text that reads back the same; any other
/// (an LREAL) keeps every digit it has.
#[allow(clippy::cast_possible_truncation)]
fn write_real(f: &mut fmt::Formatter<'_>, r: f64) -> fmt::Result {
    let narrow = r as f32;
    if r.is_finite() && f64::from(narrow).to_bits() == r.to_bits() {
        write!(f, "{narrow:?}")
    } else {
        write!(f, "{r:?}")
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
    fn a_real_is_shown_at_three_figures() {
        let t = [
            (49.873_512, "49.9"),
            (50.0, "50.0"),
            (5.0, "5.00"),
            (0.0, "0.00"),
            (0.012_345, "0.0123"),
            (-3.1449, "-3.14"),
            (12420.4, "12420"),
            (123.4, "123"),
            (9.996, "10.0"),
            (99.96, "100"),
            (999.6, "1000"),
            (-9.996, "-10.0"),
            (0.000_123_45, "0.000123"),
            (0.000_099_96, "1.00e-4"),
            (1.5e-7, "1.50e-7"),
            (1.234e9, "1.23e9"),
            (-2.5e12, "-2.50e12"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::from(23.7f32), "23.7"),
        ];
        for (x, want) in t {
            assert_eq!(shown_real(x), want, "{x}");
        }
        assert_eq!(Value::Real(49.873_512).shown(), "49.9");
        assert_eq!(Value::Int(12345).shown(), "12345");
        assert_eq!(Value::Bool(true).shown(), "TRUE");
        assert_eq!(
            Value::Real(f64::from(49.873_512_f32)).to_string(),
            "49.873512",
            "Display still round-trips"
        );
    }

    #[test]
    fn a_shown_real_is_within_half_a_unit_of_its_last_place() {
        let mut x = 1.1e-4_f64;
        while x < 1e9 {
            for v in [x, -x] {
                let s = shown_real(v);
                let back: f64 = s.parse().unwrap();
                let dec = s.split_once('.').map_or(0, |(_, f)| f.len());
                let half = 0.5 * 10f64.powi(-i32::try_from(dec).unwrap());
                assert!((back - v).abs() <= half * 1.000_001, "{v} shown {s}");
            }
            x *= 1.173;
        }
    }

    #[test]
    fn a_shown_real_keeps_its_width_across_a_sweep() {
        let mut x = 49.5;
        while x < 50.5 {
            assert_eq!(shown_real(x).len(), 4, "{x}");
            x += 0.0137;
        }
    }

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

    /// A REAL is an f32 held widened in an f64; it reads as the f32 it is
    /// (23.7, not 23.700000762939453).
    #[test]
    fn a_real_reads_as_its_shortest_f32() {
        let real = |x: f32| Value::Real(f64::from(x)).to_string();
        assert_eq!(real(0.1), "0.1");
        assert_eq!(real(23.7), "23.7");
        assert_eq!(real(-23.7), "-23.7");
        assert_eq!(real(1e30), "1e30");
        assert_eq!(real(3.0), "3.0");
        assert_eq!(real(0.0), "0.0");
        assert_eq!(real(f32::NAN), "NaN");
        assert_eq!(real(f32::INFINITY), "inf");
        assert_eq!(real(f32::NEG_INFINITY), "-inf");
        // What is not an f32 keeps every digit it has.
        assert_eq!(Value::Real(0.1 + 0.2).to_string(), "0.30000000000000004");
        assert_eq!(Value::Real(1e300).to_string(), "1e300");
        // And the rest print as before.
        assert_eq!(Value::Int(-7).to_string(), "-7");
        assert_eq!(Value::Bool(true).to_string(), "TRUE");
        // The text reads back as the same REAL.
        for x in [0.1f32, 23.7, 1e-7, 16_777_217.0, f32::MAX, f32::MIN_POSITIVE] {
            assert_eq!(real(x).parse::<f32>().unwrap(), x, "{x}");
        }
    }

    #[test]
    fn real_rounds_through_f32() {
        assert_eq!(round_real(Ty::Real, 0.1), f64::from(0.1f32));
        assert_eq!(round_real(Ty::LReal, 0.1), 0.1);
    }
}
