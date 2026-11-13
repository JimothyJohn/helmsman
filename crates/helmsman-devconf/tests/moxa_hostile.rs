//! A console (or something standing in for one) that answers badly: every
//! case ends in an error, none in a hang or a panic, and no error carries
//! what the peer sent. Each server speaks to up to two connections (a read
//! is retried once) and never to a real device.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use helmsman_devconf::moxa_e1200::{Config, Console, Error};

/// Answers every connection with `reply` after reading the request head,
/// then closes, or holds the socket open for 2 s.
fn canned(reply: Vec<u8>, hold: bool) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for s in l.incoming().take(2) {
            let Ok(mut s) = s else { continue };
            let reply = reply.clone();
            thread::spawn(move || {
                let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
                let mut buf = [0u8; 4096];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
                        _ => return,
                    }
                }
                let _ = s.write_all(&reply);
                let _ = s.flush();
                if hold {
                    thread::sleep(Duration::from_secs(2));
                }
            });
        }
    });
    addr
}

fn cfg() -> Config {
    Config {
        gap: Duration::ZERO,
        retry_wait: Duration::ZERO,
        request: Duration::from_millis(400),
        ..Config::default()
    }
}

fn error_of(reply: &[u8], hold: bool) -> Error {
    match Console::connect(&canned(reply.to_vec(), hold), cfg()) {
        Err(e) => e,
        Ok(_) => panic!(
            "a hostile reply was accepted: {:?}",
            String::from_utf8_lossy(&reply[..reply.len().min(60)])
        ),
    }
}

#[test]
fn a_head_past_the_cap_is_refused() {
    let mut r = b"HTTP/1.1 200 OK\r\nX-Pad: ".to_vec();
    r.extend(vec![b'a'; 20_000]);
    r.extend_from_slice(b"\r\n\r\n");
    assert_eq!(error_of(&r, false), Error::Protocol("response head too large"));
}

#[test]
fn a_head_that_never_ends_is_refused() {
    let r = [b"HTTP/1.1 200 OK\r\nX: ".to_vec(), vec![b'a'; 1 << 20]].concat();
    assert_eq!(error_of(&r, false), Error::Protocol("response head too large"));
}

#[test]
fn a_declared_body_past_the_cap_is_refused_unread() {
    assert_eq!(
        error_of(b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999\r\n\r\nabc", true),
        Error::TooLarge
    );
}

#[test]
fn a_body_with_no_length_past_the_cap_is_refused() {
    let r = [b"HTTP/1.1 200 OK\r\n\r\n".to_vec(), vec![b'a'; 3 << 20]].concat();
    assert_eq!(error_of(&r, false), Error::TooLarge);
}

#[test]
fn a_body_with_no_length_and_no_end_times_out() {
    assert_eq!(error_of(b"HTTP/1.1 200 OK\r\n\r\npartial", true), Error::Timeout);
}

#[test]
fn a_body_shorter_than_its_length_is_refused() {
    assert_eq!(
        error_of(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort", false),
        Error::Protocol("body shorter than its Content-Length")
    );
}

#[test]
fn a_slow_drip_hits_the_one_deadline() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for s in l.incoming().take(2) {
            let Ok(mut s) = s else { continue };
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nX: ");
                for _ in 0..60 {
                    thread::sleep(Duration::from_millis(50));
                    if s.write_all(b"a").is_err() {
                        return;
                    }
                }
            });
        }
    });
    // Each byte arrives inside any idle timeout; the whole exchange does not.
    assert_eq!(Console::connect(&addr, cfg()).err(), Some(Error::Timeout));
}

#[test]
fn garbage_is_not_a_response() {
    for r in [
        &b"\x00\x01\x02garbage\r\n\r\n"[..],
        b"SSH-2.0-OpenSSH_9.0\r\n\r\n",
        b"HTTP/1.1 abc\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nno colon here\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: nope\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
        b"\xff\xfe\xfd\r\n\r\n",
    ] {
        assert!(
            matches!(error_of(r, false), Error::Protocol(_)),
            "{:?}",
            String::from_utf8_lossy(r)
        );
    }
}

#[test]
fn a_close_with_no_answer_is_no_response() {
    assert_eq!(error_of(b"", false), Error::NoResponse);
}

#[test]
fn a_head_cut_short_is_refused() {
    assert!(matches!(
        error_of(b"HTTP/1.1 200 OK\r\nA: b", false),
        Error::Protocol(_)
    ));
}

#[test]
fn redirects_chunking_and_statuses_are_errors() {
    assert_eq!(
        error_of(
            b"HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/\r\nContent-Length: 0\r\n\r\n",
            false
        ),
        Error::Redirect(302)
    );
    assert!(matches!(
        error_of(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
            false
        ),
        Error::Protocol(_)
    ));
    assert_eq!(
        error_of(b"HTTP/1.1 500 Oops\r\nContent-Length: 0\r\n\r\n", false),
        Error::Status(500)
    );
    assert_eq!(
        error_of(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n", false),
        Error::Status(404)
    );
}

#[test]
fn errors_do_not_echo_what_the_peer_sent() {
    for r in [
        &b"HTTP/1.1 500 x\r\nSet-Cookie: ChallID=hunter2\r\nX-Secret: hunter2\r\nContent-Length: 7\r\n\r\nhunter2"[..],
        b"HTTP/1.1 200 OK\r\nX-Secret hunter2\r\n\r\n",
        b"hunter2 hunter2\r\n\r\n",
        b"HTTP/1.1 302 x\r\nLocation: http://hunter2/\r\n\r\n",
    ] {
        let e = error_of(r, false).to_string();
        assert!(!e.contains("hunter2"), "{e}");
    }
}

#[test]
fn a_flood_of_cookies_is_bounded_and_the_session_still_opens() {
    let mut r = b"HTTP/1.1 200 OK\r\n".to_vec();
    for i in 0..500 {
        r.extend_from_slice(format!("Set-Cookie: c{i}=v\r\n").as_bytes());
    }
    r.extend_from_slice(b"Content-Length: 2\r\n\r\nok");
    assert!(Console::connect(&canned(r, false), cfg()).is_ok());
}

#[test]
fn nothing_listening_is_unreachable() {
    let addr = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    };
    assert!(matches!(
        Console::connect(&addr, cfg()),
        Err(Error::Unreachable(_))
    ));
}
