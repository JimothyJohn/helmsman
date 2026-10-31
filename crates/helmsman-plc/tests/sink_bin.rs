//! The `helmsman-plc` binary with `--sinks`: a module emits by driving a
//! signal, and the core publishes it; a bad sinks file stops the box
//! before the adapter starts.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn plc() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    // No box API here: its default port and state directory are shared.
    c.args(["--api", "off", "--opcua", "off"]);
    c
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sink-bin")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const COUNTER: &str = "PROGRAM counter
VAR_OUTPUT count : INT; END_VAR
  count := count + 1;
END_PROGRAM
";

#[test]
fn a_module_output_is_published_by_the_core() {
    let d = dir("publish");
    // A receiver that hands each request body to the test.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = Vec::new();
            let mut b = [0u8; 4096];
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            while let Ok(n @ 1..) = s.read(&mut b) {
                buf.extend_from_slice(&b[..n]);
                let text = String::from_utf8_lossy(&buf);
                if let Some((_, body)) = text.split_once("\r\n\r\n")
                    && body.ends_with('}')
                {
                    let _ = tx.send(body.to_string());
                    break;
                }
            }
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        }
    });
    let prog = d.join("counter.st");
    std::fs::write(&prog, COUNTER).unwrap();
    let map = d.join("map");
    std::fs::write(&map, "in count int module counter\n").unwrap();
    let sinks = d.join("sinks");
    std::fs::write(
        &sinks,
        format!("sink mes http://127.0.0.1:{port}/in allow_local=true\nemit mes count every_ms=100\n"),
    )
    .unwrap();
    let mut child = plc()
        .arg(&prog)
        .args(["--io-map".as_ref(), map.as_os_str()])
        .args(["--sinks".as_ref(), sinks.as_os_str()])
        .args(["--sink-dir".as_ref(), d.join("state").as_os_str()])
        .args([
            "--bind",
            "127.0.0.1",
            "--encap-port",
            "0",
            "--io-port",
            "0",
            "--scan-ms",
            "2",
        ])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let bodies: Vec<String> = (0..2)
        .map(|_| rx.recv_timeout(Duration::from_secs(10)).expect("an event"))
        .collect();
    let _ = child.kill();
    let _ = child.wait();
    let mut log = String::new();
    while stderr.read_line(&mut log).unwrap_or(0) > 0 {}
    assert!(log.contains("sink mes (http://127.0.0.1:"), "{log}");
    for (i, b) in bodies.iter().enumerate() {
        assert!(
            b.starts_with(&format!(
                "{{\"key\":\"mes-1-{i}\",\"boot\":1,\"seq\":{i},\"signal\":\"count\",\"value\":"
            )),
            "{b}"
        );
        assert!(b.contains("\"trigger\":\"every\""), "{b}");
    }
    assert!(d.join("state").join("mes.queue").exists());
}

#[test]
fn a_bad_sinks_file_is_refused_before_the_adapter_starts() {
    let d = dir("refuse");
    let prog = d.join("counter.st");
    std::fs::write(&prog, COUNTER).unwrap();
    let map = d.join("map");
    std::fs::write(&map, "in count int module counter\n").unwrap();
    let sinks = d.join("sinks");
    std::fs::write(
        &sinks,
        "sink meta http://169.254.169.254/latest/meta-data\nsink loop http://127.0.0.1:9/\nemit meta nope change\n",
    )
    .unwrap();
    let out = plc()
        .arg(&prog)
        .args(["--io-map".as_ref(), map.as_os_str()])
        .args(["--sinks".as_ref(), sinks.as_os_str()])
        .args(["--sink-dir".as_ref(), d.join("state").as_os_str()])
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    for want in [
        "address 169.254.169.254 refused",
        "address 127.0.0.1 refused: loopback",
        "emit of unknown signal nope",
    ] {
        assert!(err.contains(want), "missing {want:?} in:\n{err}");
    }
    assert!(!err.contains("meta-data"), "the URL path was printed:\n{err}");
    assert!(!err.contains("CIP on"), "the adapter started:\n{err}");

    for args in [&["--sinks", "x"][..], &["--sink-dir", "x"]] {
        let out = plc().arg(&prog).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}
