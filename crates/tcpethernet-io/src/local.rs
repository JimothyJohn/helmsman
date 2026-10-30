//! The box's own channels as a [`Driver`]: `do:N` outputs and `di:N`
//! inputs, the addresses a [`crate::Binding::Local`] names. This backend
//! keeps them in memory, which is what a dev machine has; a GPIO backend
//! for the Pi implements the same contract.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Ty, Value, local_point};

#[derive(Debug, Default)]
struct State {
    outputs: BTreeMap<u16, bool>,
    inputs: BTreeMap<u16, bool>,
}

/// In-memory box channels. Clones share state, so a test (or a simulated
/// panel) can hold one handle while the runtime drives another.
#[derive(Clone, Debug, Default)]
pub struct Channels {
    state: Arc<Mutex<State>>,
}

fn channel(addr: &str) -> Result<(PointInfo, u16), Error> {
    let info = local_point(addr).map_err(|_| Error::Address(addr.into()))?;
    let n = addr
        .split_once(':')
        .and_then(|(_, n)| n.parse().ok())
        .ok_or_else(|| Error::Address(addr.into()))?;
    Ok((info, n))
}

impl Channels {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the box is driving on `do:n` (off until written).
    #[must_use]
    pub fn output(&self, n: u16) -> bool {
        self.lock().outputs.get(&n).copied().unwrap_or(false)
    }

    /// Sets what `di:n` reads, as a wire to the input terminal would.
    pub fn set_input(&self, n: u16, on: bool) {
        self.lock().inputs.insert(n, on);
    }
}

impl Driver for Channels {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        channel(addr).map(|(info, _)| info)
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let points = addrs.iter().map(|a| channel(a)).collect::<Result<Vec<_>, _>>()?;
        let s = self.lock();
        Ok(points
            .into_iter()
            .map(|(info, n)| {
                let table = if info.writable { &s.outputs } else { &s.inputs };
                Value::Bool(table.get(&n).copied().unwrap_or(false))
            })
            .collect())
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        let (info, n) = channel(addr)?;
        if !info.writable {
            return Err(Error::ReadOnly(addr.into()));
        }
        let Value::Bool(on) = value else {
            return Err(Error::Type {
                want: Ty::Bool,
                got: value.ty(),
            });
        };
        self.lock().outputs.insert(n, on);
        Ok(())
    }
}
