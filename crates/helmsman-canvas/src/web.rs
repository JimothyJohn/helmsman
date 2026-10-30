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

/// Whether the page asked for the demo (`?demo`).
#[must_use]
pub fn wants_demo() -> bool {
    wants("demo")
}

/// Whether the page's query string has `key` (`?andon`, `?demo`).
#[must_use]
pub fn wants(key: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .is_some_and(|q| {
            q.trim_start_matches('?')
                .split('&')
                .any(|kv| kv.split('=').next() == Some(key))
        })
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

/// Carries out `req` on the page's origin. The token goes only in the
/// `Authorization` header; nothing is cached or stored.
async fn send(req: ReviewRequest) -> Result<Answer, String> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method(req.method.as_str());
    init.set_cache(web_sys::RequestCache::NoStore);
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let headers = web_sys::Headers::new().map_err(|e| text(&e))?;
    if let Some(t) = &req.bearer {
        headers
            .set("Authorization", &format!("Bearer {t}"))
            .map_err(|e| text(&e))?;
    }
    if let Some(b) = &req.body {
        headers
            .set("Content-Type", "application/json")
            .map_err(|e| text(&e))?;
        init.set_body(&JsValue::from_str(b));
    }
    init.set_headers(&headers);
    let request = web_sys::Request::new_with_str_and_init(&req.url, &init).map_err(|e| text(&e))?;
    drop(req);
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
