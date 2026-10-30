//! Watching signals for events. See the [module docs](super) for the
//! semantics; this is them as code. Runs on the scan thread, so it does
//! no I/O and allocates only for events that fire.

use std::fmt;
use std::time::Duration;

use helmsman_io::{Ty, Value};

use super::config::EmitDecl;
use crate::SignalTable;

/// When an emit fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Change,
    Rise,
    Fall,
    Every(Duration),
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Change => f.write_str("change"),
            Self::Rise => f.write_str("rise"),
            Self::Fall => f.write_str("fall"),
            Self::Every(_) => f.write_str("every"),
        }
    }
}

/// An event that fired, for sink number `sink`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fired {
    pub sink: usize,
    pub signal: String,
    pub value: Value,
    pub trigger: Trigger,
}

/// What a sink's thread receives.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub signal: String,
    pub value: Value,
    pub trigger: Trigger,
    /// Wall-clock ms, `None` while the clock is untrusted.
    pub ts_ms: Option<i64>,
}

impl Fired {
    #[must_use]
    pub fn into_event(self, ts_ms: Option<i64>) -> Event {
        Event {
            signal: self.signal,
            value: self.value,
            trigger: self.trigger,
            ts_ms,
        }
    }
}

#[derive(Debug, Clone)]
struct Watch {
    sink: usize,
    signal: usize,
    name: String,
    trigger: Trigger,
    /// The last value seen; `None` until the first.
    last: Option<Value>,
    /// `Every`: when it is next due.
    due: Duration,
}

/// Every emit of a sinks file, bound to a signal table.
#[derive(Debug, Clone, Default)]
pub struct Emitter {
    watches: Vec<Watch>,
}

fn same(a: Value, b: Value) -> bool {
    match (a, b) {
        // Bitwise, so 0.0 and -0.0 differ and nothing is "equal but changed".
        (Value::Real(x), Value::Real(y)) => x.to_bits() == y.to_bits(),
        _ => a == b,
    }
}

impl Emitter {
    /// Binds each emit to its sink (by name, in `sinks`) and signal.
    ///
    /// # Errors
    /// Every emit naming an unknown sink or signal, `rise`/`fall` on a
    /// signal that is not `bool`, and the same signal emitted twice to one
    /// sink.
    pub fn bind(decls: &[EmitDecl], sinks: &[&str], table: &SignalTable) -> Result<Self, Vec<String>> {
        let mut errs = Vec::new();
        let mut watches: Vec<Watch> = Vec::new();
        for d in decls {
            let Some(sink) = sinks.iter().position(|s| *s == d.sink) else {
                errs.push(format!("line {}: emit to unknown sink {}", d.line, d.sink));
                continue;
            };
            let Some(signal) = table.index(&d.signal) else {
                errs.push(format!("line {}: emit of unknown signal {}", d.line, d.signal));
                continue;
            };
            let ty = table.signal(signal).map(|(_, ty)| ty);
            if matches!(d.trigger, Trigger::Rise | Trigger::Fall) && ty != Some(Ty::Bool) {
                errs.push(format!(
                    "line {}: {} of {}: rise and fall are for bool signals",
                    d.line, d.trigger, d.signal
                ));
                continue;
            }
            if watches.iter().any(|w| w.sink == sink && w.signal == signal) {
                errs.push(format!(
                    "line {}: {} is already emitted to {}",
                    d.line, d.signal, d.sink
                ));
                continue;
            }
            watches.push(Watch {
                sink,
                signal,
                name: d.signal.clone(),
                trigger: d.trigger,
                last: None,
                due: Duration::ZERO,
            });
        }
        if errs.is_empty() {
            Ok(Self { watches })
        } else {
            Err(errs)
        }
    }

    /// Finds each watched signal in `table` (another layout than it was
    /// bound to) by name. A signal that is gone is watched nowhere (its
    /// emit is silent); one whose type changed starts again as if unseen.
    pub fn rebind(&mut self, table: &SignalTable) {
        for w in &mut self.watches {
            let at = table.index(&w.name);
            let ty = at.and_then(|i| table.signal(i)).map(|(_, ty)| ty);
            if w.last.is_some_and(|v| Some(v.ty()) != ty) {
                w.last = None;
            }
            w.signal = at.unwrap_or(usize::MAX);
        }
    }

    /// Looks at every watched signal once, at monotonic time `now`, and
    /// returns what fired, in emit order.
    pub fn observe(&mut self, table: &SignalTable, now: Duration) -> Vec<Fired> {
        let mut out = Vec::new();
        for w in &mut self.watches {
            let Some(v) = table.value(w.signal) else {
                continue;
            };
            let prev = w.last.replace(v);
            let fires = match (w.trigger, prev) {
                (Trigger::Every(period), _) => {
                    if now >= w.due {
                        // Skip missed periods rather than bursting.
                        let late = now.saturating_sub(w.due);
                        let skip = if period.is_zero() {
                            0
                        } else {
                            late.as_nanos() / period.as_nanos()
                        };
                        let steps = u32::try_from(skip + 1).unwrap_or(u32::MAX);
                        w.due = if prev.is_none() {
                            now + period
                        } else {
                            w.due + period * steps
                        };
                        true
                    } else {
                        false
                    }
                }
                (_, None) => false,
                (Trigger::Change, Some(p)) => !same(p, v),
                (Trigger::Rise, Some(p)) => p == Value::Bool(false) && v == Value::Bool(true),
                (Trigger::Fall, Some(p)) => p == Value::Bool(true) && v == Value::Bool(false),
            };
            if fires {
                out.push(Fired {
                    sink: w.sink,
                    signal: w.name.clone(),
                    value: v,
                    trigger: w.trigger,
                });
            }
        }
        out
    }
}
