//! What a module runs: a compiled ST program, or (the `wasm` feature) a
//! WASM module ([`crate::wasm`]). The runtime treats both alike: ports
//! bound to signals by name, inputs set before a scan, outputs read after
//! it, faults stop the module, state carries on a swap.

use std::time::Duration;

use helmsman_iec::{Carry, Fault, Image, ImageSizes, Port, Program, SetError, Value, trace::Trace};

#[cfg(feature = "wasm")]
use crate::wasm::WasmProgram;

/// A module's logic.
#[derive(Debug, Clone)]
#[allow(
    clippy::large_enum_variant,
    reason = "ST is the common case and is scanned inline; the WASM store is boxed"
)]
pub enum Logic {
    /// Structured Text, compiled against the module's windows.
    St(Program),
    /// WebAssembly under the `helmsman/v1` interface; no windows.
    #[cfg(feature = "wasm")]
    Wasm(Box<WasmProgram>),
}

impl From<Program> for Logic {
    fn from(p: Program) -> Self {
        Self::St(p)
    }
}

#[cfg(feature = "wasm")]
impl From<WasmProgram> for Logic {
    fn from(p: WasmProgram) -> Self {
        Self::Wasm(Box::new(p))
    }
}

impl Logic {
    /// The ST program, if this is one.
    #[must_use]
    pub const fn st(&self) -> Option<&Program> {
        match self {
            Self::St(p) => Some(p),
            #[cfg(feature = "wasm")]
            Self::Wasm(_) => None,
        }
    }

    /// The ST program, mutably, if this is one.
    pub const fn st_mut(&mut self) -> Option<&mut Program> {
        match self {
            Self::St(p) => Some(p),
            #[cfg(feature = "wasm")]
            Self::Wasm(_) => None,
        }
    }

    /// `"st"` or `"wasm"`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::St(_) => "st",
            #[cfg(feature = "wasm")]
            Self::Wasm(_) => "wasm",
        }
    }

    /// The image sizes it was compiled for; a WASM module has none.
    #[must_use]
    pub fn sizes(&self) -> ImageSizes {
        match self {
            Self::St(p) => p.sizes(),
            #[cfg(feature = "wasm")]
            Self::Wasm(_) => ImageSizes { i: 0, q: 0, m: 0 },
        }
    }

    /// Its `VAR_INPUT`s and `VAR_OUTPUT`s (or WASM ports), names upper-cased.
    #[must_use]
    pub fn ports(&self) -> Vec<Port> {
        match self {
            Self::St(p) => p.ports(),
            #[cfg(feature = "wasm")]
            Self::Wasm(w) => w.ports(),
        }
    }

    /// A variable's or port's current value.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Value> {
        match self {
            Self::St(p) => p.get(name),
            #[cfg(feature = "wasm")]
            Self::Wasm(w) => w.get(name),
        }
    }

    /// Sets an input before a scan.
    ///
    /// # Errors
    /// No such input, or a value it cannot hold.
    pub fn set_input(&mut self, name: &str, v: Value) -> Result<(), SetError> {
        match self {
            Self::St(p) => p.set_input(name, v),
            #[cfg(feature = "wasm")]
            Self::Wasm(w) => w.set_input(name, v),
        }
    }

    /// Whether [`Logic::set_input`] would take `v`, without setting it.
    #[must_use]
    pub fn accepts(&self, name: &str, v: Value) -> bool {
        match self {
            Self::St(p) => p.clone().set_input(name, v).is_ok(),
            #[cfg(feature = "wasm")]
            Self::Wasm(w) => w.accepts(name, v),
        }
    }

    /// A cold restart.
    pub fn reset(&mut self) {
        match self {
            Self::St(p) => p.reset(),
            #[cfg(feature = "wasm")]
            Self::Wasm(w) => w.reset(),
        }
    }

    /// Whether a scan can be traced (ST only).
    #[must_use]
    pub const fn traceable(&self) -> bool {
        self.st().is_some()
    }

    /// One scan against `image` at `now`.
    ///
    /// # Errors
    /// The fault that stopped it.
    pub fn scan_at(&mut self, image: &mut Image, now: Duration) -> Result<(), Fault> {
        match self {
            Self::St(p) => p.scan_at(image, now),
            #[cfg(feature = "wasm")]
            Self::Wasm(w) => w.scan_at(now),
        }
    }

    /// [`Logic::scan_at`], traced when it is ST.
    pub fn scan_traced(&mut self, image: &mut Image, now: Duration) -> (Result<(), Fault>, Option<Trace>) {
        match self {
            Self::St(p) => {
                let (r, t) = p.scan_traced(image, now);
                (r, Some(t))
            }
            #[cfg(feature = "wasm")]
            Self::Wasm(w) => (w.scan_at(now), None),
        }
    }

    /// Carries `old`'s state into this one by name and type (see
    /// [`Program::restore`] and [`WasmProgram::restore`]); across kinds
    /// nothing carries.
    pub fn restore(&mut self, old: &Self) -> Carry {
        match (self, old) {
            (Self::St(p), Self::St(o)) => p.restore(&o.state()),
            #[cfg(feature = "wasm")]
            (Self::Wasm(w), Self::Wasm(o)) => w.restore(o),
            #[cfg(feature = "wasm")]
            (new, old) => Carry {
                kept: Vec::new(),
                reset: new.state_names(),
                dropped: old.state_names(),
            },
        }
    }

    #[cfg(feature = "wasm")]
    fn state_names(&self) -> Vec<String> {
        match self {
            Self::St(p) => p.state().names().map(str::to_string).collect(),
            Self::Wasm(w) => w.state_names(),
        }
    }
}
