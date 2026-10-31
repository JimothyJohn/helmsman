//! The browser transport for [`crate::remote`]: `fetch` for one-off
//! reads, `EventSource` for the two streams. Callbacks only push
//! [`Msg`]s into an inbox and wake egui; all decisions are
//! [`crate::remote::BoxState`]'s.
//!
//! A stream that errors is closed at once and reported as
//! [`Msg::Failed`]: the browser's own retry is switched off so the
//! canvas's backoff (and its "reconnecting" / "unreachable" status) is the
//! only one.

use std::cell::RefCell;
use std::rc::Rc;

use eframe::egui;
use eframe::wasm_bindgen::closure::Closure;
use eframe::wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::box_projects::{PCall, PReq};
use crate::claude::{Incoming, KeyStore, PROXY_HEALTH, PROXY_HELLO, Request as ClaudeRequest};
use crate::remote::{Cmd, Msg, Stream};
use crate::review::{Answer, Call, Request as ReviewRequest};

type Inbox = Rc<RefCell<Vec<Msg>>>;

/// An open `EventSource` and the callbacks it holds; closed when dropped.
struct Source {
    es: web_sys::EventSource,
    event: &'static str,
    on_open: Closure<dyn FnMut(web_sys::Event)>,
    on_error: Closure<dyn FnMut(web_sys::Event)>,
    on_event: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl Drop for Source {
    fn drop(&mut self) {
        self.es.close();
        self.es.set_onopen(None);
        self.es.set_onerror(None);
        let _ = self
            .es
            .remove_event_listener_with_callback(self.event, self.on_event.as_ref().unchecked_ref());
        // Keep the closures alive until here: `close` stops further calls.
        let _ = (&self.on_open, &self.on_error);
    }
}

#[derive(Default)]
pub struct Driver {
    inbox: Inbox,
    values: Option<Source>,
    trace: Option<Source>,
}

impl Driver {
    /// What arrived since the last call.
    pub fn drain(&mut self) -> Vec<Msg> {
        std::mem::take(&mut *self.inbox.borrow_mut())
    }

    pub fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        match cmd {
            Cmd::Get(what, url) => {
                let (inbox, ctx) = (Rc::clone(&self.inbox), ctx.clone());
                wasm_bindgen_futures::spawn_local(async move {
                    let r = get(&url).await;
                    inbox.borrow_mut().push(Msg::Got(what, r));
                    ctx.request_repaint();
                });
            }
            Cmd::Listen(stream, url) => {
                let source = self.listen(stream, &url, ctx);
                if source.is_none() {
                    self.inbox.borrow_mut().push(Msg::Failed(stream));
                }
                *self.slot(stream) = source;
            }
            Cmd::Close(stream) => *self.slot(stream) = None,
        }
    }

    fn slot(&mut self, stream: Stream) -> &mut Option<Source> {
        match stream {
            Stream::Values => &mut self.values,
            Stream::Trace => &mut self.trace,
        }
    }

    fn listen(&mut self, stream: Stream, url: &str, ctx: &egui::Context) -> Option<Source> {
        // The old one first, so its late callbacks cannot follow the new one's.
        *self.slot(stream) = None;
        let es = web_sys::EventSource::new(url).ok()?;
        let push = {
            let (inbox, ctx) = (Rc::clone(&self.inbox), ctx.clone());
            move |m: Msg| {
                inbox.borrow_mut().push(m);
                ctx.request_repaint();
            }
        };
        let on_open = {
            let push = push.clone();
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| push(Msg::Opened(stream)))
        };
        let on_error = {
            let (push, es) = (push.clone(), es.clone());
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                es.close();
                push(Msg::Failed(stream));
            })
        };
        let event = stream.event();
        let on_event = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if let Some(data) = e.data().as_string() {
                push(Msg::Event(stream, event.to_string(), data));
            }
        });
        es.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        es.add_event_listener_with_callback(event, on_event.as_ref().unchecked_ref())
            .ok()?;
        Some(Source {
            es,
            event,
            on_open,
            on_error,
            on_event,
        })
    }
}

fn text(e: &JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// `GET url` (same origin): the body of a 2xx, or why not.
async fn get(url: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| text(&e))?
        .dyn_into()
        .map_err(|e| text(&e))?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = JsFuture::from(resp.text().map_err(|e| text(&e))?)
        .await
        .map_err(|e| text(&e))?;
    body.as_string().ok_or_else(|| "not text".into())
}

/// Whether this page runs the simulation rather than watching the box
/// that served it ([`crate::app::wants_sim`] on the page's query string
/// and its canvas element's `data-mode`).
#[must_use]
pub fn wants_sim() -> bool {
    let window = web_sys::window();
    let query = window
        .as_ref()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    let mode = window
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("canvas"))
        .and_then(|e| e.get_attribute("data-mode"));
    crate::app::wants_sim(&query, mode.as_deref())
}

/// The playground's link to Claude: one answer at a time, abortable.
/// Callbacks only push [`Incoming`]s; [`crate::claude::Chat`] decides.
#[derive(Default)]
pub struct ClaudeLink {
    inbox: Rc<RefCell<Vec<Incoming>>>,
    abort: Option<web_sys::AbortController>,
}

impl ClaudeLink {
    pub fn drain(&mut self) -> Vec<Incoming> {
        std::mem::take(&mut *self.inbox.borrow_mut())
    }

    /// Asks the page's origin whether the local proxy is there; only a
    /// page served from this machine can have one (a public host is not
    /// even asked).
    pub fn check_proxy(&mut self, ctx: &egui::Context) {
        let local = web_sys::window()
            .and_then(|w| w.location().hostname().ok())
            .is_some_and(|h| matches!(h.as_str(), "127.0.0.1" | "localhost" | "[::1]"));
        let (inbox, ctx) = (Rc::clone(&self.inbox), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let yes = local && get(PROXY_HEALTH).await.is_ok_and(|b| b.trim() == PROXY_HELLO);
            inbox.borrow_mut().push(Incoming::Proxy(yes));
            ctx.request_repaint();
        });
    }

    /// Stops the answer in flight.
    pub fn abort(&mut self) {
        if let Some(a) = self.abort.take() {
            a.abort();
        }
    }

    /// Sends `req` and streams its answer back as `seq`.
    pub fn send(&mut self, seq: u64, req: ClaudeRequest, ctx: &egui::Context) {
        self.abort();
        let controller = web_sys::AbortController::new().ok();
        let signal = controller.as_ref().map(web_sys::AbortController::signal);
        self.abort = controller;
        let (inbox, ctx) = (Rc::clone(&self.inbox), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let push = |m: Incoming| {
                inbox.borrow_mut().push(m);
                ctx.request_repaint();
            };
            if let Err(why) = claude(seq, &req, signal.as_ref(), &push).await {
                push(Incoming::Unreachable { seq, why });
            }
        });
    }
}

/// `POST` the Messages API request; the key (direct route) only ever in
/// its `x-api-key` header, to [`crate::claude::ANTHROPIC_URL`]. Errors
/// carry the browser's words, never a header.
async fn claude(
    seq: u64,
    req: &ClaudeRequest,
    signal: Option<&web_sys::AbortSignal>,
    push: &dyn Fn(Incoming),
) -> Result<(), String> {
    use web_sys::js_sys::{Reflect, Uint8Array};

    let window = web_sys::window().ok_or("no window")?;
    let headers = web_sys::Headers::new().map_err(|e| text(&e))?;
    for (n, v) in req.headers() {
        headers.set(n, v).map_err(|_| format!("bad header {n}"))?;
    }
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_headers(&headers);
    init.set_body(&JsValue::from_str(&req.body));
    init.set_cache(web_sys::RequestCache::NoStore);
    init.set_credentials(web_sys::RequestCredentials::Omit);
    init.set_signal(signal);
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(req.url, &init))
        .await
        .map_err(|e| text(&e))?
        .dyn_into()
        .map_err(|e| text(&e))?;
    if resp.status() != 200 {
        let b = JsFuture::from(resp.text().map_err(|e| text(&e))?)
            .await
            .map_err(|e| text(&e))?;
        let mut body = b.as_string().unwrap_or_default();
        body.truncate(crate::claude::MAX_ANSWER);
        push(Incoming::Status {
            seq,
            status: resp.status(),
            body,
        });
        return Ok(());
    }
    let stream = resp.body().ok_or("no body")?;
    let reader: web_sys::ReadableStreamDefaultReader =
        stream.get_reader().dyn_into().map_err(|e| text(&e))?;
    let decoder = web_sys::TextDecoder::new().map_err(|e| text(&e))?;
    let opts = web_sys::TextDecodeOptions::new();
    opts.set_stream(true);
    loop {
        let r = JsFuture::from(reader.read()).await.map_err(|e| text(&e))?;
        let done = Reflect::get(&r, &JsValue::from_str("done"))
            .map_err(|e| text(&e))?
            .as_bool()
            .unwrap_or(true);
        if done {
            push(Incoming::End { seq });
            return Ok(());
        }
        let value: Uint8Array = Reflect::get(&r, &JsValue::from_str("value"))
            .map_err(|e| text(&e))?
            .dyn_into()
            .map_err(|e| text(&e))?;
        let chunk = decoder
            .decode_with_js_u8_array_and_options(&value, &opts)
            .map_err(|e| text(&e))?;
        push(Incoming::Chunk { seq, text: chunk });
    }
}

/// A key the user asked this browser to remember, in `localStorage`.
#[derive(Debug, Default)]
pub struct LocalKeyStore;

const KEY_SLOT: &str = "helmsman-playground/anthropic-api-key";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

impl KeyStore for LocalKeyStore {
    fn load(&self) -> Option<String> {
        local_storage()?.get_item(KEY_SLOT).ok().flatten()
    }

    fn save(&mut self, key: &str) {
        if let Some(s) = local_storage() {
            let _ = s.set_item(KEY_SLOT, key);
        }
    }

    fn forget(&mut self) {
        if let Some(s) = local_storage() {
            let _ = s.remove_item(KEY_SLOT);
        }
    }
}

/// The review screen's transport: one `fetch` per [`ReviewRequest`],
/// answered with its status, `Retry-After` and body whatever the status,
/// so the screen can show the box's refusal verbatim.
#[derive(Default)]
pub struct Http {
    inbox: Rc<RefCell<Vec<(Call, Answer)>>>,
}

impl Http {
    /// What arrived since the last call.
    pub fn drain(&mut self) -> Vec<(Call, Answer)> {
        std::mem::take(&mut *self.inbox.borrow_mut())
    }

    pub fn send(&mut self, req: ReviewRequest, ctx: &egui::Context) {
        let (inbox, ctx) = (Rc::clone(&self.inbox), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let call = req.call.clone();
            let answer = send(req).await.unwrap_or_else(|why| Answer {
                status: 0,
                retry_after: None,
                body: why,
            });
            inbox.borrow_mut().push((call, answer));
            ctx.request_repaint();
        });
    }
}

/// What every call to the box sends: same-origin only, the browser's
/// session cookie with it (`HttpOnly`: this code never sees it), nothing
/// cached, and the page's origin. The box serves its pages with
/// `Referrer-Policy: no-referrer`, under which a browser sends `Origin:
/// null` even to the same origin; `same-origin` here makes it send the
/// real one, which the box checks beside the cookie. A mutating call
/// carries `X-Helmsman-Request: 1`, a header no cross-site form can send.
fn init(method: &str) -> Result<(web_sys::RequestInit, web_sys::Headers), String> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_mode(web_sys::RequestMode::SameOrigin);
    init.set_cache(web_sys::RequestCache::NoStore);
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_referrer_policy(web_sys::ReferrerPolicy::SameOrigin);
    let headers = web_sys::Headers::new().map_err(|e| text(&e))?;
    if method != "GET" {
        headers.set("X-Helmsman-Request", "1").map_err(|e| text(&e))?;
    }
    Ok((init, headers))
}

/// Carries out `req` on the page's origin with the session cookie (see
/// [`init`]); nothing is cached or stored.
async fn send(req: ReviewRequest) -> Result<Answer, String> {
    let body = req.body.as_ref().map(|b| JsValue::from_str(b));
    let ctype = body.as_ref().map(|_| "application/json");
    let (method, url) = (req.method.as_str(), req.url.clone());
    drop(req);
    fetch(method, &url, body, ctype).await
}

/// One same-origin call (see [`init`]) with an optional body of type
/// `ctype`: its status, `Retry-After` and body whatever the status.
async fn fetch(
    method: &str,
    url: &str,
    body: Option<JsValue>,
    ctype: Option<&str>,
) -> Result<Answer, String> {
    let window = web_sys::window().ok_or("no window")?;
    let (init, headers) = init(method)?;
    if let Some(b) = &body {
        if let Some(t) = ctype {
            headers.set("Content-Type", t).map_err(|e| text(&e))?;
        }
        init.set_body(b);
    }
    init.set_headers(&headers);
    let request = web_sys::Request::new_with_str_and_init(url, &init).map_err(|e| text(&e))?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| text(&e))?
        .dyn_into()
        .map_err(|e| text(&e))?;
    let retry_after = resp
        .headers()
        .get("Retry-After")
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse().ok());
    let body = JsFuture::from(resp.text().map_err(|e| text(&e))?)
        .await
        .map_err(|e| text(&e))?
        .as_string()
        .unwrap_or_default();
    Ok(Answer {
        status: resp.status(),
        retry_after,
        body,
    })
}

/// The project and force routes' transport: one `fetch` per [`PReq`],
/// its body bytes as they are (a `.zip`, a file, JSON).
#[derive(Default)]
pub struct ProjHttp {
    inbox: Rc<RefCell<Vec<(PCall, Answer)>>>,
}

impl ProjHttp {
    /// What arrived since the last call.
    pub fn drain(&mut self) -> Vec<(PCall, Answer)> {
        std::mem::take(&mut *self.inbox.borrow_mut())
    }

    pub fn send(&mut self, req: PReq, ctx: &egui::Context) {
        let (inbox, ctx) = (Rc::clone(&self.inbox), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let body = req
                .body
                .as_ref()
                .map(|b| JsValue::from(web_sys::js_sys::Uint8Array::from(b.as_slice())));
            let ctype = body.as_ref().map(|_| req.ctype);
            let answer = fetch(req.method, &req.url, body, ctype)
                .await
                .unwrap_or_else(|why| Answer {
                    status: 0,
                    retry_after: None,
                    body: why,
                });
            inbox.borrow_mut().push((req.call, answer));
            ctx.request_repaint();
        });
    }
}

thread_local! {
    /// Files the user picked, read, and not taken yet.
    static PICKED: RefCell<Vec<(String, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

/// Opens the browser's file picker (`accept`: `.zip,.st,.xml,.l5x`); the
/// file chosen arrives through [`take_picked`].
pub fn pick_file(accept: &str, ctx: &egui::Context) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(input) = doc
        .create_element("input")
        .ok()
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return;
    };
    input.set_type("file");
    input.set_accept(accept);
    let ctx = ctx.clone();
    let picker = input.clone();
    let on_change = Closure::once_into_js(move || {
        let Some(file) = picker.files().and_then(|l| l.get(0)) else {
            return;
        };
        let name = file.name();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(buf) = JsFuture::from(file.array_buffer()).await {
                let bytes = web_sys::js_sys::Uint8Array::new(&buf).to_vec();
                PICKED.with(|p| p.borrow_mut().push((name, bytes)));
                ctx.request_repaint();
            }
        });
    });
    input.set_onchange(Some(on_change.unchecked_ref()));
    input.click();
}

/// The files picked since the last call: name and bytes.
#[must_use]
pub fn take_picked() -> Vec<(String, Vec<u8>)> {
    PICKED.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// Clicks a hidden link to `href` that downloads as `name`.
fn click_download(href: &str, name: &str) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(a) = doc
        .create_element("a")
        .ok()
        .and_then(|e| e.dyn_into::<web_sys::HtmlAnchorElement>().ok())
    else {
        return;
    };
    a.set_href(href);
    a.set_download(name);
    a.click();
}

/// Downloads a same-origin `url` (the box's `GET .../zip`) as `name`.
pub fn download_url(url: &str, name: &str) {
    click_download(url, name);
}
