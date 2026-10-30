//! A Modbus TCP device simulator for tests and the bench harness: four tables,
//! cross-wires (a coil driving a discrete input, as a wire from an output
//! terminal to an input terminal would), and injectable faults. It answers
//! the function codes [`crate::modbus`] uses and refuses the rest with the
//! spec's exceptions.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::modbus::{
    COIL_OFF, COIL_ON, ILLEGAL_DATA_ADDRESS, ILLEGAL_DATA_VALUE, ILLEGAL_FUNCTION, MAX_PDU, MAX_READ_BITS,
    MAX_READ_REGS, MAX_WRITE_REGS, MBAP_LEN, READ_COILS, READ_DISCRETE_INPUTS, READ_HOLDING_REGISTERS,
    READ_INPUT_REGISTERS, SERVER_DEVICE_FAILURE, WRITE_MULTIPLE_REGISTERS, WRITE_SINGLE_COIL,
    WRITE_SINGLE_REGISTER,
};

/// A misbehaviour for the next request(s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Answer late.
    Delay(Duration),
    /// Close the connection without answering.
    Drop,
    /// Answer with bytes that are not a Modbus reply.
    Garbage,
    /// Answer with the wrong transaction id.
    WrongTransaction,
    /// Answer with exception 04 (server device failure).
    DeviceFailure,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub coils: Vec<bool>,
    pub discrete: Vec<bool>,
    pub holding: Vec<u16>,
    pub input: Vec<u16>,
    /// `(coil, discrete input)`: writing the coil sets the input.
    pub wires: Vec<(u16, u16)>,
    /// Applied to the next requests, one per request, front first.
    pub faults: Vec<Fault>,
    /// Requests answered, faults included.
    pub requests: u64,
}

impl State {
    #[must_use]
    pub fn new(size: u16) -> Self {
        let n = usize::from(size);
        Self {
            coils: vec![false; n],
            discrete: vec![false; n],
            holding: vec![0; n],
            input: vec![0; n],
            ..Self::default()
        }
    }

    fn apply_wires(&mut self) {
        for &(c, d) in &self.wires {
            if let (Some(&v), Some(slot)) = (
                self.coils.get(usize::from(c)),
                self.discrete.get_mut(usize::from(d)),
            ) {
                *slot = v;
            }
        }
    }

    /// Answers one request PDU (App §6), or an exception (App §7).
    #[must_use]
    pub fn handle(&mut self, pdu: &[u8]) -> Vec<u8> {
        let Some(&fc) = pdu.first() else {
            return vec![0x80, ILLEGAL_FUNCTION];
        };
        let ex = |code: u8| vec![fc | 0x80, code];
        let word = |i: usize| pdu.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let range = |len: usize, start: u16, qty: u16| {
            let end = usize::from(start) + usize::from(qty);
            (end <= len).then_some(usize::from(start)..end)
        };
        match fc {
            READ_COILS | READ_DISCRETE_INPUTS => {
                let (Some(start), Some(qty), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if qty == 0 || qty > MAX_READ_BITS {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let table = if fc == READ_COILS {
                    &self.coils
                } else {
                    &self.discrete
                };
                let Some(r) = range(table.len(), start, qty) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                let mut bytes = vec![0u8; usize::from(qty).div_ceil(8)];
                for (i, b) in table[r].iter().enumerate() {
                    bytes[i / 8] |= u8::from(*b) << (i % 8);
                }
                let mut out = vec![fc, u8::try_from(bytes.len()).expect("at most 250")];
                out.extend(bytes);
                out
            }
            READ_HOLDING_REGISTERS | READ_INPUT_REGISTERS => {
                let (Some(start), Some(qty), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if qty == 0 || qty > MAX_READ_REGS {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let table = if fc == READ_HOLDING_REGISTERS {
                    &self.holding
                } else {
                    &self.input
                };
                let Some(r) = range(table.len(), start, qty) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                let mut out = vec![fc, u8::try_from(qty * 2).expect("at most 250")];
                for v in &table[r] {
                    out.extend(v.to_be_bytes());
                }
                out
            }
            WRITE_SINGLE_COIL => {
                let (Some(addr), Some(v), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if v != COIL_ON && v != COIL_OFF {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let Some(slot) = self.coils.get_mut(usize::from(addr)) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                *slot = v == COIL_ON;
                self.apply_wires();
                pdu.to_vec()
            }
            WRITE_SINGLE_REGISTER => {
                let (Some(addr), Some(v), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                let Some(slot) = self.holding.get_mut(usize::from(addr)) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                *slot = v;
                pdu.to_vec()
            }
            WRITE_MULTIPLE_REGISTERS => {
                let (Some(start), Some(qty), Some(&n)) = (word(1), word(3), pdu.get(5)) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if qty == 0
                    || qty > MAX_WRITE_REGS
                    || usize::from(n) != usize::from(qty) * 2
                    || pdu.len() != 6 + usize::from(n)
                {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let Some(r) = range(self.holding.len(), start, qty) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                for (slot, c) in self.holding[r].iter_mut().zip(pdu[6..].as_chunks::<2>().0) {
                    *slot = u16::from_be_bytes(*c);
                }
                pdu[..5].to_vec()
            }
            _ => ex(ILLEGAL_FUNCTION),
        }
    }
}

/// A running simulator on 127.0.0.1. Stops when dropped.
pub struct Sim {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Sim {
    /// Starts serving `state` on an ephemeral loopback port.
    ///
    /// # Errors
    /// The listener could not be bound.
    pub fn start(state: State) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(state));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp) = (Arc::clone(&state), Arc::clone(&stop));
        let accept = thread::spawn(move || {
            while !sp.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let (st, sp) = (Arc::clone(&st), Arc::clone(&sp));
                        thread::spawn(move || serve(conn, &st, &sp));
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(2)),
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            addr,
            state,
            stop,
            accept: Some(accept),
        })
    }

    /// The simulator's tables, to set up or inspect.
    pub fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    pub fn inject(&self, fault: Fault) {
        self.state().faults.push(fault);
    }

    pub fn clear_faults(&self) {
        self.state().faults.clear();
    }

    #[must_use]
    pub fn endpoint(&self) -> String {
        self.addr.to_string()
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

/// Reads exactly `buf.len()` bytes, giving up when the simulator stops.
fn read_full(conn: &mut TcpStream, buf: &mut [u8], stop: &AtomicBool) -> bool {
    let mut got = 0;
    while got < buf.len() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        match conn.read(&mut buf[got..]) {
            Ok(0) => return false,
            Ok(n) => got += n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return false,
        }
    }
    true
}

fn serve(mut conn: TcpStream, state: &Mutex<State>, stop: &AtomicBool) {
    let _ = conn.set_read_timeout(Some(Duration::from_millis(20)));
    let _ = conn.set_nodelay(true);
    loop {
        let mut head = [0u8; MBAP_LEN];
        if !read_full(&mut conn, &mut head, stop) {
            return;
        }
        let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
        // TCP §3.1.3 p.5: protocol id 0; length covers unit id + PDU.
        if head[2..4] != [0, 0] || !(2..=MAX_PDU + 1).contains(&len) {
            return;
        }
        let mut pdu = vec![0u8; len - 1];
        if !read_full(&mut conn, &mut pdu, stop) {
            return;
        }
        let (fault, mut reply) = {
            let mut s = lock(state);
            s.requests += 1;
            let fault = if s.faults.is_empty() {
                None
            } else {
                Some(s.faults.remove(0))
            };
            let reply = match fault {
                Some(Fault::DeviceFailure) => vec![pdu[0] | 0x80, SERVER_DEVICE_FAILURE],
                _ => s.handle(&pdu),
            };
            (fault, reply)
        };
        let mut tid = [head[0], head[1]];
        match fault {
            Some(Fault::Drop) => return,
            Some(Fault::Delay(d)) => thread::sleep(d),
            Some(Fault::Garbage) => reply = vec![0xDE, 0xAD],
            Some(Fault::WrongTransaction) => tid = u16::from_be_bytes(tid).wrapping_add(1).to_be_bytes(),
            Some(Fault::DeviceFailure) | None => {}
        }
        let mut frame = Vec::with_capacity(MBAP_LEN + reply.len());
        frame.extend(tid);
        frame.extend([0, 0]);
        frame.extend(u16::try_from(reply.len() + 1).expect("pdu fits").to_be_bytes());
        frame.push(head[6]);
        frame.extend(reply);
        if fault == Some(Fault::Garbage) {
            // Garbage in the header too: protocol id 0xBEEF.
            frame[2..4].copy_from_slice(&[0xBE, 0xEF]);
        }
        if conn.write_all(&frame).is_err() {
            return;
        }
    }
}
