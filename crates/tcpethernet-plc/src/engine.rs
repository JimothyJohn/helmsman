//! The scan engine: every period, read I from the consumed assembly, run
//! the program, write Q to the produced assembly. The program can be
//! swapped while running; a swap takes effect at the next scan boundary.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tcpethernet_adapter::Device;
use tcpethernet_adapter::device::lock_ref;

use crate::lang::{self, CompileError, Program, Scan, Sizes};

pub const DEFAULT_PROGRAM: &str = "\
# Runs every scan, top to bottom.
#   I = bytes from the scanner (read-only)   Q = bytes to the scanner
#   M = memory                                RUN, SCAN, MS = status/counters
# Echo everything back, so a scanner sees its own output as input:
Q[0..32] = I[0..32]
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Assembly the scanner writes (O->T); the program's I.
    pub input_assembly: u16,
    /// Assembly the scanner reads (T->O); the program's Q.
    pub output_assembly: u16,
    pub memory: usize,
    pub period: Duration,
    /// Where the program is persisted; `None` keeps it in memory only.
    pub program_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub scans: u64,
    pub last_us: u64,
    pub max_us: u64,
    pub faults: u64,
    pub overruns: u64,
}

#[derive(Debug)]
pub struct State {
    pub source: String,
    program: Program,
    pub memory: Vec<u8>,
    pub stats: Stats,
    pub sizes: Sizes,
}

pub struct Engine {
    pub cfg: EngineConfig,
    state: Arc<Mutex<State>>,
    device: Arc<Mutex<Device>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    started: Instant,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

fn lock_state(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_atomic(path: &Path, data: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)
}

impl Engine {
    /// Load the persisted program (or the default), compile it, start scanning.
    ///
    /// # Errors
    /// Unknown assemblies, or a persisted program that no longer compiles.
    pub fn start(device: Arc<Mutex<Device>>, cfg: EngineConfig) -> Result<Self, String> {
        let sizes = {
            let d = lock_ref(&device);
            let i = d
                .assembly(cfg.input_assembly)
                .ok_or("input assembly does not exist")?
                .len();
            let q = d
                .assembly(cfg.output_assembly)
                .ok_or("output assembly does not exist")?
                .len();
            Sizes { i, q, m: cfg.memory }
        };
        let source = match &cfg.program_file {
            Some(p) if p.exists() => fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?,
            _ => DEFAULT_PROGRAM.to_string(),
        };
        let program = lang::compile(&source, sizes).map_err(|errs| {
            let list: Vec<String> = errs.iter().map(ToString::to_string).collect();
            format!("stored program does not compile: {}", list.join("; "))
        })?;
        let state = Arc::new(Mutex::new(State {
            source,
            program,
            memory: vec![0; cfg.memory],
            stats: Stats::default(),
            sizes,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let thread = {
            let (state, device, stop, cfg) = (
                Arc::clone(&state),
                Arc::clone(&device),
                Arc::clone(&stop),
                cfg.clone(),
            );
            thread::Builder::new()
                .name("plc-scan".into())
                .spawn(move || scan_loop(&state, &device, &stop, &cfg, started))
                .map_err(|e| e.to_string())?
        };
        Ok(Self {
            cfg,
            state,
            device,
            stop,
            thread: Some(thread),
            started,
        })
    }

    #[must_use]
    pub fn device(&self) -> Arc<Mutex<Device>> {
        Arc::clone(&self.device)
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        lock_state(&self.state)
    }

    #[must_use]
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Compile `source`; if it compiles, persist it and swap it in.
    ///
    /// # Errors
    /// Compile errors (nothing changes), or a persistence failure (the new
    /// program is not swapped in, so disk and memory agree).
    pub fn set_program(&self, source: &str) -> Result<(), Vec<CompileError>> {
        let sizes = self.state().sizes;
        let program = lang::compile(source, sizes)?;
        if let Some(p) = &self.cfg.program_file {
            write_atomic(p, source).map_err(|e| {
                vec![CompileError {
                    line: 0,
                    col: 0,
                    msg: format!("could not save {}: {e}", p.display()),
                }]
            })?;
        }
        let mut st = self.state();
        st.program = program;
        st.source = source.to_string();
        Ok(())
    }

    /// Write one memory byte (for forcing values from the UI).
    ///
    /// # Errors
    /// Out-of-range address.
    pub fn set_memory(&self, at: usize, value: u8) -> Result<(), String> {
        let mut st = self.state();
        let len = st.memory.len();
        let cell = st
            .memory
            .get_mut(at)
            .ok_or_else(|| format!("M[{at}] is out of range (M has {len} bytes)"))?;
        *cell = value;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn scan_loop(
    state: &Mutex<State>,
    device: &Mutex<Device>,
    stop: &AtomicBool,
    cfg: &EngineConfig,
    started: Instant,
) {
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        let (i, mut q, run) = {
            let d = lock_ref(device);
            (
                d.assembly(cfg.input_assembly).unwrap_or_default().to_vec(),
                d.assembly(cfg.output_assembly).unwrap_or_default().to_vec(),
                d.scanner_running(),
            )
        };
        {
            let mut st = lock_state(state);
            let st = &mut *st;
            let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let faults = st.program.scan(&mut Scan {
                i: &i,
                q: &mut q,
                m: &mut st.memory,
                run,
                scan: st.stats.scans,
                ms,
            });
            st.stats.faults += faults;
            st.stats.scans += 1;
        }
        let _ = lock_ref(device).write_assembly(cfg.output_assembly, &q);
        let took = u64::try_from(t0.elapsed().as_micros()).unwrap_or(u64::MAX);
        {
            let mut st = lock_state(state);
            st.stats.last_us = took;
            st.stats.max_us = st.stats.max_us.max(took);
        }
        next += cfg.period;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            lock_state(state).stats.overruns += 1;
            next = now;
        }
    }
}
