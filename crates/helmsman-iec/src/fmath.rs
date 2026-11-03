//! The float functions Structured Text needs (SQRT, LN, LOG, EXP, EXPT, the
//! trigonometry, rounding), from std where there is one and from `libm`
//! where there is not: `core` has no `sqrt` or `sin`. One place, so the
//! interpreter reads the same either way and a `no_std` build differs from
//! the std build only in who computes the last bit.

#[cfg(feature = "std")]
macro_rules! unary {
    ($($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name(x: f64) -> f64 {
                f64::$name(x)
            }
        )*
    };
}

#[cfg(not(feature = "std"))]
macro_rules! unary {
    ($($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name(x: f64) -> f64 {
                libm::$name(x)
            }
        )*
    };
}

unary!(sqrt, exp, sin, cos, tan, asin, acos, atan, log10, round, trunc);

#[inline]
pub fn ln(x: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        x.ln()
    }
    #[cfg(not(feature = "std"))]
    {
        libm::log(x)
    }
}

#[inline]
pub fn abs(x: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        x.abs()
    }
    #[cfg(not(feature = "std"))]
    {
        libm::fabs(x)
    }
}

#[inline]
pub fn powf(x: f64, y: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        x.powf(y)
    }
    #[cfg(not(feature = "std"))]
    {
        libm::pow(x, y)
    }
}
