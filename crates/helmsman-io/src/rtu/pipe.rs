//! An in-memory serial line: two [`End`]s, each one's sends the other's
//! receives. What the RTU tests and the simulator run over instead of a
//! port. Dropping an end closes the line: the other end's `recv` then
//! fails once it has read what was already sent.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::Line;

#[derive(Debug, Default)]
struct Queue {
    bytes: VecDeque<u8>,
    closed: bool,
}

#[derive(Debug, Default)]
struct Chan {
    q: Mutex<Queue>,
    cv: Condvar,
}

impl Chan {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.q.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_all();
    }
}

/// One end of the line.
#[derive(Debug)]
pub struct End {
    tx: Arc<Chan>,
    rx: Arc<Chan>,
}

/// A connected pair of ends.
#[must_use]
pub fn pair() -> (End, End) {
    let (a, b) = (Arc::new(Chan::default()), Arc::new(Chan::default()));
    (
        End {
            tx: Arc::clone(&a),
            rx: Arc::clone(&b),
        },
        End { tx: b, rx: a },
    )
}

impl End {
    /// Bytes sent to this end and not yet received.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.rx.lock().bytes.len()
    }
}

impl Line for End {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let mut q = self.tx.lock();
        if q.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        q.bytes.extend(frame);
        drop(q);
        self.tx.cv.notify_all();
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<usize> {
        let q = self.rx.lock();
        let (mut q, _) = self
            .rx
            .cv
            .wait_timeout_while(q, wait, |q| q.bytes.is_empty() && !q.closed)
            .unwrap_or_else(PoisonError::into_inner);
        if q.bytes.is_empty() {
            return if q.closed {
                Err(io::ErrorKind::UnexpectedEof.into())
            } else {
                Ok(0)
            };
        }
        let n = buf.len().min(q.bytes.len());
        for (slot, b) in buf.iter_mut().zip(q.bytes.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }
}

impl Drop for End {
    fn drop(&mut self) {
        self.tx.close();
        self.rx.close();
    }
}
