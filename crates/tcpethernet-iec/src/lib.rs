//! IEC 61131-3 Structured Text: compile a `PROGRAM`, then run it one scan
//! at a time against a process image.
//!
//! ```
//! use tcpethernet_iec::{Image, ImageSizes, compile};
//!
//! let src = "
//! PROGRAM blink
//! VAR count : INT; END_VAR
//!   count := count + 1;
//!   %QX0.0 := count MOD 2 = 0;
//!   %QW1 := INT_TO_WORD(count);
//! END_PROGRAM";
//! let sizes = ImageSizes { i: 32, q: 32, m: 64 };
//! let mut p = compile(src, sizes).unwrap();
//! let mut img = Image::new(sizes);
//! p.scan(&mut img).unwrap();
//! p.scan(&mut img).unwrap();
//! assert_eq!(img.q[0] & 1, 1);
//! assert_eq!(&img.q[2..4], &[2, 0]);
//! ```
//!
//! This slice covers PROGRAM units with `VAR` / `VAR_TEMP` / `VAR CONSTANT`,
//! the elementary types BOOL, SINT..LINT, USINT..ULINT, BYTE..LWORD,
//! REAL, LREAL, located variables (`AT %IX0.1`) and direct addresses, the
//! full ST statement set, and the `*_TO_*` conversions. Functions,
//! function blocks, TIME, STRING and the standard library come next.

mod check;
mod interp;
mod lexer;
mod parser;
mod types;

pub use check::{Area, Diagnostic, ImageSizes};
pub use interp::{Fault, FaultKind, Image};
pub use lexer::Pos;
pub use types::{Ty, Value};

use check::{St, SymKind};
use interp::Machine;

/// Instructions one scan may execute before it faults (see [`FaultKind::Watchdog`]).
pub const DEFAULT_BUDGET: u64 = 1_000_000;

impl Image {
    /// A zeroed image of the given sizes.
    #[must_use]
    pub fn new(sizes: ImageSizes) -> Self {
        Self {
            i: vec![0; sizes.i],
            q: vec![0; sizes.q],
            m: vec![0; sizes.m],
        }
    }
}

/// A compiled program and its variables.
#[derive(Debug, Clone)]
pub struct Program {
    name: String,
    sizes: ImageSizes,
    syms: std::collections::HashMap<String, check::Sym>,
    init: Vec<Value>,
    slots: Vec<Value>,
    temps: Vec<usize>,
    body: Vec<St>,
    budget: u64,
}

/// Compiles one `PROGRAM ... END_PROGRAM` against an image of `sizes`.
/// # Errors
/// Every syntax or type error found, sorted by position.
pub fn compile(src: &str, sizes: ImageSizes) -> Result<Program, Vec<Diagnostic>> {
    let ast = parser::parse(src).map_err(|e| {
        vec![Diagnostic {
            pos: e.pos,
            msg: e.msg,
        }]
    })?;
    let c = check::check(&ast, sizes)?;
    Ok(Program {
        name: ast.name,
        sizes,
        syms: c.syms,
        slots: c.init.clone(),
        init: c.init,
        temps: c.temps,
        body: c.body,
        budget: DEFAULT_BUDGET,
    })
}

impl Program {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn sizes(&self) -> ImageSizes {
        self.sizes
    }

    /// Changes the per-scan instruction budget.
    pub const fn set_budget(&mut self, budget: u64) {
        self.budget = budget;
    }

    /// Runs one scan. On a fault the variables are left as the fault found
    /// them; [`Program::reset`] restores the initial values.
    ///
    /// # Errors
    /// A [`Fault`]: integer division by zero, or the watchdog.
    pub fn scan(&mut self, image: &mut Image) -> Result<(), Fault> {
        for &t in &self.temps {
            self.slots[t] = self.init[t];
        }
        Machine {
            slots: &mut self.slots,
            image,
            budget: self.budget,
        }
        .run(&self.body)
    }

    /// Restores every variable to its initial value (a cold restart).
    pub fn reset(&mut self) {
        self.slots.clone_from(&self.init);
    }

    /// Current value of a variable (case-insensitive name). Located
    /// variables live in the image, so this returns `None` for them.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Value> {
        match self.syms.get(&name.to_ascii_uppercase())?.kind {
            SymKind::Slot(s) => Some(self.slots[s]),
            SymKind::Image(_) => None,
        }
    }

    /// Declared type of a variable.
    #[must_use]
    pub fn ty(&self, name: &str) -> Option<Ty> {
        self.syms.get(&name.to_ascii_uppercase()).map(|s| s.ty)
    }
}
