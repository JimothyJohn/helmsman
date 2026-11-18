//! Network firmware update on the P4 (card node-ota; helmsman-nodecore's
//! `ota` module is the logic, this is the flash and the sockets).
//!
//! The partition table (`partitions.csv`) has two app slots, `otadata`
//! and a two-sector `bootrec` partition for the trial record. A request
//! (`POST /api/firmware {url, sha256}`) wakes [`fetch_task`], which reads
//! the image over a plain TCP connection into the slot that is not
//! running, checks it against the digest in the request, puts it on trial
//! and resets. The new image counts its boots in `bootrec` ([`on_boot`]);
//! [`confirm_task`] ends the trial once the node has run healthily for
//! [`CONFIRM_AFTER`], and `POST /api/firmware/confirm` ends it at once. An
//! image that never confirms is replaced by the old one at its fourth boot.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::Write;
use esp_println::println;
use helmsman_nodecore::ota::boot::{Boot, Confirmed, Decision, Error as BootError, Otadata};
use helmsman_nodecore::ota::image::Chip;
use helmsman_nodecore::ota::install::{Image, Installer, SECTOR, Slot};
use helmsman_nodecore::ota::record::Trial as Record;
use helmsman_nodecore::ota::request::Fetch;
use helmsman_nodecore::ota::status;
use helmsman_nodecore::ota::wire::{self, HeadError};

use crate::flash::{Layout, Store, StoreError};
use crate::server::Ctx;
use crate::state;

/// What the build says about itself (`NODE_BUILD_TAG` at build time).
pub const BUILD: &str = match option_env!("NODE_BUILD_TAG") {
    Some(t) => t,
    None => "",
};
/// A bench build that panics a few seconds after it has started, so it
/// never confirms and the node falls back (`NODE_OTA_BREAK=1`).
pub const BREAK: bool = option_env!("NODE_OTA_BREAK").is_some();
/// How long a node on trial must have run, with its link up and its scan
/// loop publishing, before it confirms itself.
pub const CONFIRM_AFTER: Duration = Duration::from_secs(30);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A read that stalls this long ends the fetch.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// The whole fetch gets this long.
const FETCH_DEADLINE: Duration = Duration::from_secs(300);

/// The request the server hands the fetch task.
pub static FETCH: Signal<CriticalSectionRawMutex, Fetch> = Signal::new();

/// The flash store as the server shares it, without its lock.
pub type Cell = RefCell<Option<Store>>;

fn with_store<R>(cell: &Cell, f: impl FnOnce(&mut Store) -> Result<R, StoreError>) -> Result<R, StoreError> {
    match cell.borrow_mut().as_mut() {
        Some(store) => f(store),
        None => Err(StoreError::NoPartition),
    }
}

/// A flash partition as a [`Slot`]: an app slot, or the trial record.
pub struct Part<'a> {
    cell: &'a Cell,
    span: (u32, u32),
}

impl Slot for Part<'_> {
    type Error = StoreError;

    fn capacity(&self) -> u32 {
        self.span.1
    }

    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), StoreError> {
        with_store(self.cell, |s| s.part_write(self.span, offset, data))
    }

    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), StoreError> {
        with_store(self.cell, |s| s.part_read(self.span, offset, buf))
    }
}

/// `otadata` as [`Otadata`].
pub struct Bootloader<'a> {
    cell: &'a Cell,
}

impl Otadata for Bootloader<'_> {
    type Error = StoreError;

    fn selected(&mut self) -> Result<u8, StoreError> {
        with_store(self.cell, Store::selected_slot)
    }

    fn select(&mut self, slot: u8) -> Result<(), StoreError> {
        with_store(self.cell, |s| s.select_slot(slot))
    }
}

type Trial<'a> = Boot<Part<'a>, Bootloader<'a>>;
type Failure = BootError<StoreError, StoreError>;

fn trial_of(cell: &Cell, layout: Layout) -> Result<Trial<'_>, Failure> {
    Boot::open(
        Part {
            cell,
            span: layout.record,
        },
        Bootloader { cell },
    )
}

fn describe(e: &Failure) -> String {
    alloc::format!("{e:?}")
}

/// The first thing a boot does about firmware: count it against a trial,
/// or end the trial and reset into the old image. Fills the live state;
/// returns whether this image is on trial, and a line for the log.
pub fn on_boot(cell: &Cell, shared: &state::Shared) -> (bool, String) {
    let Some(layout) = with_store(cell, |s| Ok(s.layout())).ok().flatten() else {
        return (
            false,
            String::from("ota: no A/B partition table on this board; update it over USB"),
        );
    };
    let mut trial = match trial_of(cell, layout) {
        Ok(t) => t,
        Err(e) => {
            return (
                false,
                alloc::format!("ota: the trial record is unreadable ({})", describe(&e)),
            );
        }
    };
    let decision = trial.on_boot();
    let slot = trial.selected().ok();
    let (on_trial, line) = match decision {
        Ok(Decision::Normal) => (
            false,
            alloc::format!("ota: slot {} confirmed, build {:?}", slot.unwrap_or(9), BUILD),
        ),
        Ok(Decision::Confirming { boots }) => (
            true,
            alloc::format!(
                "ota: slot {} on trial, boot {boots} of {}, build {:?}",
                slot.unwrap_or(9),
                helmsman_nodecore::ota::boot::MAX_PENDING_BOOTS,
                BUILD
            ),
        ),
        Ok(Decision::Reverted { to }) => {
            println!("ota: the new image never confirmed; slot {to} selected again, restarting into it");
            esp_hal::delay::Delay::new().delay_millis(250);
            esp_hal::system::software_reset()
        }
        Err(e) => (
            matches!(trial.trial(), Record::On { .. }),
            alloc::format!("ota: the boot step failed ({}); running as is", describe(&e)),
        ),
    };
    let boots = match trial.trial() {
        Record::On { boots, .. } => boots,
        Record::None => 0,
    };
    state::with(shared, |live| {
        live.fw.available = true;
        live.fw.slot = slot;
        live.fw.trial = if on_trial {
            status::Trial::OnTrial { boots }
        } else {
            status::Trial::Confirmed
        };
    });
    (on_trial, line)
}

/// The slot this image booted from, as the boot step read it. Not what
/// `otadata` says now: once an update has been started that already names
/// the new image while this one still runs.
fn running_slot(ctx: &Ctx) -> Result<u8, String> {
    state::with(ctx.shared, |live| live.fw.slot).ok_or_else(|| String::from("the running slot is unknown"))
}

/// Ends the running image's trial. `Ok(false)` if none was on.
///
/// # Errors
/// The record or `otadata` failing, or no A/B table.
pub async fn confirm(ctx: &Ctx) -> Result<bool, String> {
    let guard = ctx.store.lock().await;
    let cell: &Cell = &guard;
    let layout = with_store(cell, |s| Ok(s.layout()))
        .ok()
        .flatten()
        .ok_or_else(|| String::from("no A/B partition table on this board"))?;
    let mut trial = trial_of(cell, layout).map_err(|e| describe(&e))?;
    let ended = trial.confirm(running_slot(ctx)?).map_err(|e| describe(&e))? == Confirmed::Yes;
    state::with(ctx.shared, |live| live.fw.trial = status::Trial::Confirmed);
    Ok(ended)
}

/// Confirms a node on trial once it has run healthily for
/// [`CONFIRM_AFTER`]: link up and the scan loop publishing. Spawned only
/// when the boot found the image on trial.
#[embassy_executor::task]
pub async fn confirm_task(ctx: &'static Ctx) {
    let start = Instant::now();
    loop {
        Timer::after(Duration::from_secs(1)).await;
        if state::with(ctx.shared, |live| live.fw.trial == status::Trial::Confirmed) {
            return;
        }
        if start.elapsed() < CONFIRM_AFTER {
            continue;
        }
        let healthy = state::with(ctx.shared, |live| live.link_up && live.seq > 10);
        if !healthy {
            continue;
        }
        match confirm(ctx).await {
            Ok(_) => println!("ota: confirmed after {} s of running", start.elapsed().as_secs()),
            Err(why) => println!("ota: could not confirm ({why}); will try again"),
        }
        if state::with(ctx.shared, |live| live.fw.trial == status::Trial::Confirmed) {
            return;
        }
    }
}

/// The fetch: waits for a request, reads the image into the idle slot,
/// puts it on trial and resets. On a failure the running image is
/// untouched and the reason is in `GET /api/firmware`.
#[embassy_executor::task]
pub async fn fetch_task(stack: Stack<'static>, ctx: &'static Ctx) -> ! {
    let mut rx = vec![0u8; 8192];
    let mut tx = vec![0u8; 512];
    loop {
        let request = FETCH.wait().await;
        println!("ota: fetching http://{}{}", request.authority(), request.path);
        state::with(ctx.shared, |live| {
            live.fw.fetch = status::Fetch::Fetching { received: 0, size: 0 };
        });
        let started = Instant::now();
        let outcome = with_timeout(FETCH_DEADLINE, run(stack, ctx, &request, &mut rx, &mut tx)).await;
        match outcome {
            Ok(Ok(())) => {
                println!(
                    "ota: image written and verified in {} ms; on trial, restarting",
                    started.elapsed().as_millis()
                );
                state::with(ctx.shared, |live| {
                    live.fw.fetch = status::Fetch::Restarting {
                        sha256: request.sha256,
                    };
                });
                Timer::after(Duration::from_secs(2)).await;
                esp_hal::system::software_reset();
            }
            Ok(Err(why)) => {
                println!("ota: fetch failed: {why}");
                state::with(ctx.shared, |live| live.fw.fetch = status::Fetch::Failed(why));
            }
            Err(_) => {
                println!("ota: fetch took over {} s; given up", FETCH_DEADLINE.as_secs());
                state::with(ctx.shared, |live| {
                    live.fw.fetch = status::Fetch::Failed(String::from("the fetch took too long"));
                });
            }
        }
    }
}

async fn run(
    stack: Stack<'static>,
    ctx: &Ctx,
    request: &Fetch,
    rx: &mut [u8],
    tx: &mut [u8],
) -> Result<(), String> {
    let mut socket = TcpSocket::new(stack, rx, tx);
    socket.set_timeout(Some(READ_TIMEOUT));
    let remote = IpEndpoint::new(
        IpAddress::Ipv4(Ipv4Address::new(
            request.host[0],
            request.host[1],
            request.host[2],
            request.host[3],
        )),
        request.port,
    );
    match with_timeout(CONNECT_TIMEOUT, socket.connect(remote)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return Err(alloc::format!(
                "could not connect to {}: {e:?}",
                request.authority()
            ));
        }
        Err(_) => {
            return Err(alloc::format!(
                "no answer from {} in {} s",
                request.authority(),
                CONNECT_TIMEOUT.as_secs()
            ));
        }
    }
    socket
        .write_all(wire::image_request(request).as_bytes())
        .await
        .map_err(|e| alloc::format!("the request did not send: {e:?}"))?;

    let guard = ctx.store.lock().await;
    let cell: &Cell = &guard;
    let layout = with_store(cell, |s| Ok(s.layout()))
        .ok()
        .flatten()
        .ok_or_else(|| String::from("no A/B partition table on this board; flash it once over USB"))?;
    let running = running_slot(ctx)?;
    let idle = 1 - running;
    let span = layout.slots[usize::from(idle)];

    // The response head first, then the body in whatever pieces it comes.
    let mut chunk = vec![0u8; 4096];
    let (parsed, first, body_at) = read_head(&mut socket, &mut chunk, span.1).await?;
    if parsed.status != 200 {
        return Err(alloc::format!(
            "the server answered {} (200 is the file)",
            parsed.status
        ));
    }
    let size = parsed
        .content_length
        .ok_or_else(|| String::from("the server did not say how long the file is (no Content-Length)"))?;

    let mut buf = Box::new([0u8; SECTOR]);
    let mut installer = Installer::new(
        Part { cell, span },
        &mut buf,
        Image {
            size,
            sha256: request.sha256,
            chip: Chip::Esp32P4,
        },
    )
    .map_err(|e| alloc::format!("{e}"))?;
    println!(
        "ota: {size} bytes into slot {idle} ({} KiB free there)",
        span.1 / 1024
    );
    installer
        .feed(&first[body_at..])
        .map_err(|e| alloc::format!("{e}"))?;
    drop(first);
    let mut shown = 0u32;
    while installer.remaining() > 0 {
        let n = read(&mut socket, &mut chunk).await?;
        if n == 0 {
            break;
        }
        installer.feed(&chunk[..n]).map_err(|e| alloc::format!("{e}"))?;
        if installer.received() - shown >= 32 * 1024 {
            shown = installer.received();
            let received = shown;
            state::with(ctx.shared, |live| {
                live.fw.fetch = status::Fetch::Fetching { received, size }
            });
        }
    }
    socket.abort();
    let verified = installer.finish().map_err(|e| alloc::format!("{e}"))?;
    println!(
        "ota: {} bytes verified against the request's sha256, read back from slot {idle}",
        verified.size
    );

    let mut trial = trial_of(cell, layout).map_err(|e| describe(&e))?;
    trial.start_trial(running, idle).map_err(|e| describe(&e))?;
    Ok(())
}

/// Reads until the response head ends; returns it, the bytes read so far
/// (the body starts at the returned offset) and that offset.
async fn read_head(
    socket: &mut TcpSocket<'_>,
    chunk: &mut [u8],
    limit: u32,
) -> Result<(wire::Head, Vec<u8>, usize), String> {
    let mut seen: Vec<u8> = Vec::with_capacity(512);
    loop {
        match wire::parse_head(&seen, limit) {
            Ok((parsed, at)) => return Ok((parsed, seen, at)),
            Err(HeadError::Incomplete) if seen.len() < wire::MAX_HEAD_LEN => {}
            Err(HeadError::Incomplete) => return Err(String::from("the response head is longer than 1 KiB")),
            Err(e) => return Err(alloc::format!("{e}")),
        }
        let n = read(socket, chunk).await?;
        if n == 0 {
            return Err(String::from(
                "the server closed the connection before the response head ended",
            ));
        }
        seen.extend_from_slice(&chunk[..n]);
    }
}

async fn read(socket: &mut TcpSocket<'_>, chunk: &mut [u8]) -> Result<usize, String> {
    match with_timeout(READ_TIMEOUT, socket.read(chunk)).await {
        Ok(Ok(n)) => Ok(n),
        Ok(Err(e)) => Err(alloc::format!("the connection broke: {e:?}")),
        Err(_) => Err(alloc::format!(
            "the server stalled for {} s",
            READ_TIMEOUT.as_secs()
        )),
    }
}

/// What the report line says about the firmware.
#[must_use]
pub fn report_text(live: &state::Live) -> String {
    let trial = match live.fw.trial {
        status::Trial::Confirmed => "confirmed".to_string(),
        status::Trial::OnTrial { boots } => alloc::format!("on trial {boots}"),
    };
    alloc::format!("fw {BUILD:?} slot {} {trial}", live.fw.slot.unwrap_or(9))
}
