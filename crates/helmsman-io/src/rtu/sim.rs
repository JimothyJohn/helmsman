//! A Modbus RTU unit on an in-memory line: the same tables, cross-wires
//! and request handling as the TCP simulator ([`crate::sim::State`]),
//! framed as RTU, with the serial faults injectable (CRC errors, truncated
//! frames, another unit answering, noise, late replies).
//!
//! As a slave it follows Serial §2.4.2 p.10: a frame that fails its CRC,
//! or is for another unit, gets no reply; a broadcast (unit 0) is carried
//! out and gets no reply (Serial §2.1 p.7).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::pipe::{self, End};
use super::{BROADCAST, Line, MAX_ADU, MAX_UNIT, Timing, decode, encode};
use crate::modbus::SERVER_DEVICE_FAILURE;
use crate::sim::{Fault, Faults, State, lock};

/// A running RTU unit. Stops when dropped.
pub struct RtuSim {
    pub unit: u8,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RtuSim {
    /// Starts unit `unit` (1..=247) serving `state` at `baud`'s timing, and
    /// returns it with the master's end of the line.
    ///
    /// # Panics
    /// `unit` is 0 or over 247.
    #[must_use]
    pub fn start(state: State, unit: u8, baud: u32) -> (Self, End) {
        assert!((1..=MAX_UNIT).contains(&unit), "a unit address is 1..=247");
        let (master, slave) = pipe::pair();
        let state = Arc::new(Mutex::new(state));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp) = (Arc::clone(&state), Arc::clone(&stop));
        let timing = Timing::for_baud(baud);
        let thread = thread::spawn(move || serve(slave, unit, timing, &st, &sp));
        (
            Self {
                unit,
                state,
                stop,
                thread: Some(thread),
            },
            master,
        )
    }

    /// The unit's tables, to set up or inspect.
    pub fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

impl Faults for RtuSim {
    fn inject(&self, fault: Fault) {
        self.state().faults.push(fault);
    }

    fn clear_faults(&self) {
        self.state().faults.clear();
    }

    fn fault_checks(&self) -> Vec<(&'static str, Fault)> {
        vec![
            ("no reply", Fault::Drop),
            ("garbage", Fault::Garbage),
            ("bad crc", Fault::BadCrc),
            ("truncated frame", Fault::Truncate),
            ("wrong unit", Fault::WrongUnit),
            ("device failure", Fault::DeviceFailure),
            ("late reply", Fault::Delay(Duration::from_secs(3))),
        ]
    }
}

impl Drop for RtuSim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

/// One request: bytes until the line has been quiet for t3.5 (Serial
/// §2.5.1.1 p.13). `None` when the simulator stops or the line closes.
fn next_frame(line: &mut End, t35: Duration, stop: &AtomicBool) -> Option<Vec<u8>> {
    let mut buf = [0u8; MAX_ADU];
    let mut frame = Vec::new();
    while frame.is_empty() {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let n = line.recv(&mut buf, Duration::from_millis(20)).ok()?;
        frame.extend_from_slice(&buf[..n]);
    }
    loop {
        let n = line.recv(&mut buf, t35).ok()?;
        if n == 0 {
            return Some(frame);
        }
        frame.extend_from_slice(&buf[..n]);
        if frame.len() > 4 * MAX_ADU {
            // A babbling line: not a frame. Drop it and resynchronise.
            frame.clear();
        }
    }
}

fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) {
    let end = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        thread::sleep(left.min(Duration::from_millis(10)));
    }
}

fn serve(mut line: End, unit: u8, timing: Timing, state: &Mutex<State>, stop: &AtomicBool) {
    // The sim reads requests with at least 1 ms of quiet: at high baud the
    // spec's t3.5 is shorter than a thread wakeup.
    let gap = timing.t35.max(Duration::from_millis(1));
    while let Some(frame) = next_frame(&mut line, gap, stop) {
        let Ok((to, pdu)) = decode(&frame) else {
            continue;
        };
        if to == BROADCAST {
            let mut s = lock(state);
            s.requests += 1;
            let _ = s.handle(pdu);
            continue;
        }
        if to != unit {
            continue;
        }
        let (fault, reply) = {
            let mut s = lock(state);
            let fault = s.next_fault();
            let reply = match fault {
                Some(Fault::DeviceFailure) => vec![pdu[0] | 0x80, SERVER_DEVICE_FAILURE],
                _ => s.handle(pdu),
            };
            (fault, reply)
        };
        let Ok(mut out) = encode(unit, &reply) else {
            continue;
        };
        let sent = match fault {
            Some(Fault::Drop) => Ok(()),
            Some(Fault::Delay(d)) => {
                sleep_unless_stopped(d, stop);
                line.send(&out)
            }
            Some(Fault::Garbage) => line.send(&[0xDE, 0xAD, 0xBE, 0xEF]),
            Some(Fault::BadCrc) => {
                let last = out.len() - 1;
                out[last] ^= 0x01;
                line.send(&out)
            }
            Some(Fault::Truncate) => line.send(&out[..out.len() - 2]),
            Some(Fault::WrongUnit) => {
                let other = if unit == MAX_UNIT { 1 } else { unit + 1 };
                encode(other, &reply).map_or(Ok(()), |f| line.send(&f))
            }
            Some(Fault::Noise) => {
                out.splice(0..0, [0x00, 0xFF, 0x13, unit, 0x37]);
                line.send(&out)
            }
            Some(Fault::Split) => out.iter().try_for_each(|b| line.send(&[*b])),
            Some(Fault::WrongTransaction | Fault::DeviceFailure) | None => line.send(&out),
        };
        if sent.is_err() {
            return;
        }
    }
}
