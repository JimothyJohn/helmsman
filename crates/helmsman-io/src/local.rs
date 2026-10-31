//! The box's own channels as a [`Driver`]: `do:N` outputs, `di:N` inputs
//! and `i2c:BUS:ADDR:REG[:FMT]` sensor registers, the addresses a
//! [`crate::Binding::Local`] names (syntax in [`crate::board`]). This
//! backend keeps them in memory, which is what a dev machine has;
//! [`crate::board::Board`] is the Pi's GPIO and I2C behind the same
//! contract.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::board::Channel;
use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Ty, Value};

#[derive(Debug, Default)]
struct State {
    outputs: BTreeMap<u16, bool>,
    inputs: BTreeMap<u16, bool>,
    /// What a read of `(bus, addr, reg)` returns, byte for byte.
    i2c: BTreeMap<(u8, u8, u8), Vec<u8>>,
}

/// In-memory box channels. Clones share state, so a test (or a simulated
/// panel) can hold one handle while the runtime drives another.
#[derive(Clone, Debug, Default)]
pub struct Channels {
    state: Arc<Mutex<State>>,
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

    /// Sets the bytes a read of register `reg` of the I2C device at `addr`
    /// on bus `bus` returns (zeros until set; missing bytes read as zero).
    pub fn set_i2c(&self, bus: u8, addr: u8, reg: u8, bytes: &[u8]) {
        self.lock().i2c.insert((bus, addr, reg), bytes.to_vec());
    }
}

impl Driver for Channels {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        Channel::parse(addr).map(Channel::info)
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let chans = addrs
            .iter()
            .map(|a| Channel::parse(a))
            .collect::<Result<Vec<_>, _>>()?;
        let s = self.lock();
        Ok(chans
            .into_iter()
            .map(|c| match c {
                Channel::Out(n) => Value::Bool(s.outputs.get(&n).copied().unwrap_or(false)),
                Channel::In(n) => Value::Bool(s.inputs.get(&n).copied().unwrap_or(false)),
                Channel::I2c(p) => {
                    let mut buf = [0u8; 2];
                    if let Some(b) = s.i2c.get(&(p.bus, p.addr, p.reg)) {
                        for (slot, v) in buf.iter_mut().zip(b) {
                            *slot = *v;
                        }
                    }
                    Value::Int(p.format.decode(&buf[..p.format.bytes()]))
                }
            })
            .collect())
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        let Channel::Out(n) = Channel::parse(addr)? else {
            return Err(Error::ReadOnly(addr.into()));
        };
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
