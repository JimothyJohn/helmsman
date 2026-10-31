//! A simulated Moxa ioLogik E1213 on loopback, for the conformance harness
//! and the `enip-io` tests: `helmsman-adapter`'s E1213 profile (the
//! connections and sizes its EDS declares) with the E1213 application
//! behind it (`helmsman_adapter::e1213`: DO-00..03 wired to DI-00..03 like
//! the bench, DI counters, DO status), and the faults that mean something
//! on a Class 1 connection.

use std::collections::BTreeMap;
use std::io;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helmsman_adapter::device::lock_ref;
use helmsman_adapter::e1213::Application;
use helmsman_adapter::{Adapter, Device, DeviceConfig, Misbehave, ServerConfig};

use crate::sim::{Fault, Faults};

/// A running simulated E1213. Dropping it stops it.
pub struct E1213Sim {
    adapter: Adapter,
    app: Arc<Mutex<Application>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for E1213Sim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E1213Sim")
            .field("endpoint", &self.endpoint())
            .finish_non_exhaustive()
    }
}

impl E1213Sim {
    /// The bench E1213: DO-00..03 wired to DI-00..03.
    ///
    /// # Errors
    /// A socket that won't bind.
    pub fn start() -> io::Result<Self> {
        Self::start_with(DeviceConfig::moxa_e1213(), Application::bench())
    }

    /// Any layout and application, on 127.0.0.1 at free ports, answering
    /// each scanner at the port its O->T packets come from.
    ///
    /// # Errors
    /// A socket that won't bind.
    pub fn start_with(config: DeviceConfig, app: Application) -> io::Result<Self> {
        let adapter = Adapter::start(
            Device::new(config),
            &ServerConfig {
                bind: Ipv4Addr::LOCALHOST,
                encap_port: 0,
                io_port: 0,
                peer_io_port: 0,
                ..ServerConfig::default()
            },
        )?;
        let app = Arc::new(Mutex::new(app));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (dev, app, stop) = (adapter.device(), Arc::clone(&app), Arc::clone(&stop));
            thread::Builder::new().name("e1213-sim".into()).spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    {
                        let mut a = app.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        a.step(&mut lock_ref(&dev));
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            })?
        };
        Ok(Self {
            adapter,
            app,
            stop,
            thread: Some(thread),
        })
    }

    /// `127.0.0.1:PORT`, the encapsulation endpoint.
    #[must_use]
    pub fn endpoint(&self) -> String {
        self.adapter.encap_addr().to_string()
    }

    /// The `enip-io` params that reach this simulator with the E1213
    /// profile: its I/O port, a local port of the OS's choosing, the
    /// EDS's fastest RPI.
    #[must_use]
    pub fn params(&self) -> BTreeMap<String, String> {
        [
            ("profile", "moxa-e1213".to_string()),
            ("device_io_port", self.adapter.io_addr().port().to_string()),
            ("io_port", "0".to_string()),
            ("rpi_ms", "100".to_string()),
            ("timeout_ms", "1000".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    /// The device, to look at its connections or poke its assemblies.
    #[must_use]
    pub fn device(&self) -> Arc<Mutex<Device>> {
        self.adapter.device()
    }

    /// Hold DI-`n` on (or let it follow its wire again).
    pub fn force_di(&self, n: u8, on: bool) {
        let mut a = self.app.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if n < 16 {
            if on {
                a.forced |= 1 << n;
            } else {
                a.forced &= !(1 << n);
            }
        }
    }
}

impl Drop for E1213Sim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.adapter.stop();
    }
}

impl Faults for E1213Sim {
    fn inject(&self, fault: Fault) {
        let dev = self.adapter.device();
        let mut d = lock_ref(&dev);
        match fault {
            Fault::Silent => d.set_misbehave(Some(Misbehave::Silent)),
            Fault::ShortInput => d.set_misbehave(Some(Misbehave::ShortInput)),
            Fault::Garbage => d.set_misbehave(Some(Misbehave::Garbage)),
            Fault::Reboot | Fault::Drop => d.drop_connections(),
            // Request/reply faults: nothing to do on a cyclic connection.
            Fault::Delay(_)
            | Fault::WrongTransaction
            | Fault::DeviceFailure
            | Fault::BadCrc
            | Fault::Truncate
            | Fault::WrongUnit
            | Fault::Noise
            | Fault::Split => {}
        }
    }

    fn clear_faults(&self) {
        lock_ref(&self.adapter.device()).set_misbehave(None);
    }

    fn fault_checks(&self) -> Vec<(&'static str, Fault)> {
        vec![
            ("silent target", Fault::Silent),
            ("short input", Fault::ShortInput),
            ("garbage input", Fault::Garbage),
            ("target reboots", Fault::Reboot),
        ]
    }

    /// The E1213 profile's fastest RPI (100 ms) times 2^(1+2), the
    /// driver's default timeout, plus slack.
    fn fault_window(&self) -> Duration {
        Duration::from_secs(3)
    }
}
