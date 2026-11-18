//! A small HTTP/1.1 server on loopback for the train's stand-ins: the
//! devices checkout served as the library, and the stub model behind a
//! relay. One thread per connection, one request per connection
//! (`Connection: close`), bodies by `Content-Length` only.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Largest request body read.
const MAX_BODY: usize = 16 << 20;

/// One request as the handler sees it.
#[derive(Debug)]
pub struct Req {
    pub method: String,
    /// The path, query included.
    pub path: String,
    pub body: Vec<u8>,
}

/// One response.
#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Resp {
    #[must_use]
    pub fn json(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.to_string().into_bytes(),
        }
    }

    #[must_use]
    pub fn not_found() -> Self {
        Self {
            status: 404,
            content_type: "text/plain",
            body: b"not found".to_vec(),
        }
    }
}

type Handler = dyn Fn(&Req) -> Resp + Send + Sync;

/// A running server; stops when dropped.
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Listens on a free loopback port.
    ///
    /// # Errors
    /// The listener cannot be made.
    pub fn start(handler: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let flag = Arc::clone(&stop);
        thread::spawn(move || {
            for conn in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(s) = conn else { continue };
                let h = Arc::clone(&handler);
                thread::spawn(move || serve_one(s, &*h));
            }
        });
        Ok(Self { addr, stop })
    }

    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

fn serve_one(s: TcpStream, handler: &Handler) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    let Ok(read_half) = s.try_clone() else { return };
    let mut r = BufReader::new(read_half);
    let Some(req) = read_request(&mut r) else { return };
    let resp = handler(&req);
    let mut w = s;
    let reason = match resp.status {
        200 => "OK",
        404 => "Not Found",
        _ => "Status",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        resp.content_type,
        resp.body.len()
    );
    let _ = w.write_all(head.as_bytes());
    let _ = w.write_all(&resp.body);
    let _ = w.flush();
}

fn read_request(r: &mut impl BufRead) -> Option<Req> {
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 {
            return None;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().ok()?;
        }
    }
    if len > MAX_BODY {
        return None;
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    Some(Req { method, path, body })
}

/// The file under `root` a request path names, or `None` for anything
/// that would leave `root` (`..`, an absolute part) or carries a query.
#[must_use]
pub fn file_under(root: &Path, path: &str) -> Option<PathBuf> {
    let rel = path.split('?').next()?.trim_start_matches('/');
    let rel = Path::new(rel);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let p = root.join(rel);
    p.is_file().then_some(p)
}

/// A static server for the files under `root` (GET only).
///
/// # Errors
/// The listener cannot be made.
pub fn serve_dir(root: PathBuf) -> std::io::Result<Server> {
    Server::start(move |req| {
        if req.method != "GET" {
            return Resp::not_found();
        }
        match file_under(&root, &req.path).and_then(|p| std::fs::read(p).ok()) {
            Some(body) => Resp {
                status: 200,
                content_type: "application/octet-stream",
                body,
            },
            None => Resp::not_found(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_cannot_leave_the_root() {
        let root = std::env::temp_dir();
        for bad in ["/../etc/passwd", "/a/../../b", "//etc/passwd", "/./x"] {
            assert_eq!(file_under(&root, bad), None, "{bad}");
        }
    }

    #[test]
    fn the_dir_server_serves_files_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("compat-web-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("devices")).unwrap();
        std::fs::write(dir.join("index.json"), "{}").unwrap();
        let s = serve_dir(dir.clone()).unwrap();
        let get = |p: &str| {
            let url = helmsman_agent::http::Url::parse(&s.url("")).unwrap();
            crate::client::get(&url, p, None).unwrap().0
        };
        assert_eq!(get("/index.json"), 200);
        assert_eq!(get("/devices"), 404, "a directory is not a file");
        assert_eq!(get("/nope.json"), 404);
        assert_eq!(get("/../index.json"), 404);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
