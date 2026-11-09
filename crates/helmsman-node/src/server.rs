//! The HTTP server: two connections at a time on the API port (one may be
//! a canvas's values stream), each request read whole, routed by
//! helmsman-nodecore's router against the live state, and the router's
//! effects carried out here: a deploy compiles and is written to flash
//! and handed to the scan loop, a control is handed to the scan loop, a
//! settings save is written to flash and the node restarts.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
use esp_println::println;
use helmsman_iec::ImageSizes;
use helmsman_nodecore::api;
use helmsman_nodecore::http::{self, Read as HttpRead, Response};
use helmsman_nodecore::router::{self, Control, Effect, Outcome};
use helmsman_nodecore::settings::Settings;

use crate::flash::Store;
use crate::state::{self, Pending, Shared};

/// Reads from a peer that stall longer than this end the connection.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the scan loop has to take a deploy or a control.
pub const APPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the values stream looks for a new publication.
pub const STREAM_POLL: Duration = Duration::from_millis(100);

/// The flash store, shared by the two server connections and the boot.
pub type StoreCell = Mutex<CriticalSectionRawMutex, RefCell<Option<Store>>>;

/// What every connection needs.
pub struct Ctx {
    pub shared: &'static Shared,
    pub settings: &'static Settings,
    pub store: &'static StoreCell,
    pub mac: [u8; 6],
    pub board: &'static str,
    pub heap_size: usize,
    pub scan_ms: f64,
    pub sizes: ImageSizes,
}

impl Ctx {
    fn view(&self) -> api::NodeView {
        state::view(self.shared, self.mac, self.board, self.heap_size, self.scan_ms)
    }
}

/// Serves connections on the API port for the life of the node; two run.
#[embassy_executor::task(pool_size = 2)]
pub async fn http_task(stack: Stack<'static>, ctx: &'static Ctx) -> ! {
    let mut rx = vec![0u8; 2048];
    let mut tx = vec![0u8; 2048];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(Duration::from_secs(30)));
    loop {
        if socket.accept(ctx.settings.api_port).await.is_err() {
            Timer::after(Duration::from_millis(100)).await;
            socket.abort();
            continue;
        }
        serve(&mut socket, ctx).await;
        socket.close();
        let _ = with_timeout(Duration::from_millis(500), socket.flush()).await;
        socket.abort();
    }
}

async fn serve(socket: &mut TcpSocket<'_>, ctx: &Ctx) {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    let request = loop {
        match with_timeout(READ_TIMEOUT, socket.read(&mut chunk)).await {
            Ok(Ok(0) | Err(_)) | Err(_) => return,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
        }
        match http::parse(&buf) {
            Ok(HttpRead::Done(req, _)) => break req,
            Ok(HttpRead::Incomplete) => {}
            Err(http::HttpError::HeadTooLong) => {
                return send(socket, &Response::error(431, "request head too long")).await;
            }
            Err(http::HttpError::BodyTooLong) => {
                return send(socket, &Response::error(413, "request body too long")).await;
            }
            Err(http::HttpError::Malformed) => {
                return send(socket, &Response::error(400, "malformed request")).await;
            }
        }
    };
    match router::handle(&request, &ctx.view(), ctx.settings) {
        Outcome::Reply(r) => send(socket, &r).await,
        Outcome::Stream => stream(socket, ctx).await,
        Outcome::Effect(effect) => {
            let reply = perform(effect, ctx).await;
            send(socket, &reply.0).await;
            if reply.1 {
                let _ = with_timeout(Duration::from_millis(500), socket.flush()).await;
                Timer::after(Duration::from_millis(300)).await;
                println!("settings saved; restarting");
                esp_hal::system::software_reset();
            }
        }
    }
}

async fn send(socket: &mut TcpSocket<'_>, r: &Response) {
    let _ = with_timeout(READ_TIMEOUT, socket.write_all(&r.encode())).await;
}

/// The values stream: a `values` event whenever the publication count
/// moves, until the peer goes away (a write then fails).
async fn stream(socket: &mut TcpSocket<'_>, ctx: &Ctx) {
    if with_timeout(READ_TIMEOUT, socket.write_all(&http::sse_head()))
        .await
        .is_err()
    {
        return;
    }
    let mut last = None;
    loop {
        let view = ctx.view();
        if last != Some(view.seq) {
            last = Some(view.seq);
            let event = http::sse_event("values", &api::values_event(&view));
            match with_timeout(READ_TIMEOUT, socket.write_all(&event)).await {
                Ok(Ok(())) => {}
                _ => return,
            }
            if with_timeout(READ_TIMEOUT, socket.flush()).await.is_err() {
                return;
            }
        }
        Timer::after(STREAM_POLL).await;
    }
}

/// Carries an effect out; the reply, and whether the node restarts after
/// sending it.
async fn perform(effect: Effect, ctx: &Ctx) -> (Response, bool) {
    match effect {
        Effect::Deploy { name, source } => (deploy(name, source, ctx).await, false),
        Effect::Control { name, action } => (control(&name, action, ctx).await, false),
        Effect::SaveSettings { settings, page } => {
            let written = {
                let store = ctx.store.lock().await;
                let mut store = store.borrow_mut();
                match store.as_mut() {
                    Some(s) => s.write_settings(&settings).map_err(|e| e.to_string()),
                    None => Err(String::from("no flash store")),
                }
            };
            match written {
                Ok(()) => (router::settings_saved(page, true), true),
                Err(why) => (
                    router::settings_not_saved(page, &ctx.view(), ctx.settings, &why),
                    false,
                ),
            }
        }
        Effect::SaveIoMap { text, page } => {
            // Beside the program on flash; the restart makes the I/O task
            // drive the block it names.
            let written = {
                let store = ctx.store.lock().await;
                let mut store = store.borrow_mut();
                match store.as_mut() {
                    Some(s) => s.write_map(&text).map_err(|e| e.to_string()),
                    None => Err(String::from("no flash store")),
                }
            };
            match written {
                Ok(()) => (router::io_map_saved(page, true), true),
                Err(why) => (
                    router::io_map_not_saved(page, &ctx.view(), ctx.settings, &why),
                    false,
                ),
            }
        }
    }
}

async fn deploy(name: String, source: String, ctx: &Ctx) -> Response {
    let bytes = source.len();
    state::with(ctx.shared, |live| {
        live.deployed = None;
        live.pending = Some(Pending::Deploy {
            name: name.clone(),
            source: source.clone(),
        });
    });
    if !wait_applied(ctx).await {
        return Response::error(503, "the scan loop did not take the program in time");
    }
    match state::with(ctx.shared, |live| live.deployed.take()) {
        Some(Ok(())) => {}
        Some(Err(errors)) => return router::deploy_refused(&errors),
        None => return Response::error(500, "the scan loop reported nothing"),
    }
    // It runs; keep it. A program that did not compile never reaches flash.
    let kept = {
        let store = ctx.store.lock().await;
        let mut store = store.borrow_mut();
        match store.as_mut() {
            Some(s) => s.write_program(&name, &source).map_err(|e| e.to_string()),
            None => Err(String::from("no flash store")),
        }
    };
    match kept {
        Ok(()) => router::deployed(&name, bytes),
        Err(why) => router::deployed_not_kept(&name, &why),
    }
}

async fn control(name: &str, action: Control, ctx: &Ctx) -> Response {
    state::with(ctx.shared, |live| {
        live.controlled = None;
        live.pending = Some(Pending::Control(action));
    });
    if !wait_applied(ctx).await {
        return Response::error(503, "the scan loop did not take the control in time");
    }
    let state = state::with(ctx.shared, |live| live.controlled.unwrap_or("unknown"));
    router::controlled(name, action, state)
}

/// Waits for the scan loop to clear the pending slot.
async fn wait_applied(ctx: &Ctx) -> bool {
    let start = embassy_time::Instant::now();
    while start.elapsed() < APPLY_TIMEOUT {
        if state::with(ctx.shared, |live| live.pending.is_none()) {
            return true;
        }
        Timer::after(Duration::from_millis(10)).await;
    }
    false
}
