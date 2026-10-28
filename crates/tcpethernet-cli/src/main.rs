//! `tcpethernet` — command-line originator built on the library.

mod args;

use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use args::{Cmd, IoArgs};
use tcpethernet::cip::class;
use tcpethernet::discover::sweep;
use tcpethernet::encap::{IO_PORT, PORT};
use tcpethernet::scanner::{IoConfig, IoConnection, bind_io_socket};
use tcpethernet::session::Session;

const TIMEOUT: Duration = Duration::from_secs(3);

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 3);
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{x:02x}");
    }
    s
}

fn discover(port: u16, target: Ipv4Addr, window: Duration) -> Result<(), String> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| e.to_string())?;
    sock.set_broadcast(true).map_err(|e| e.to_string())?;
    let r = sweep(&sock, SocketAddrV4::new(target, port), window).map_err(|e| e.to_string())?;
    for f in &r.found {
        let id = &f.identity;
        println!(
            "{}\tvendor={} type=0x{:04X} product={} rev={}.{} serial=0x{:08X}\t{}",
            f.from.ip(),
            id.vendor_id,
            id.device_type,
            id.product_code,
            id.revision_major,
            id.revision_minor,
            id.serial_number,
            id.product_name_lossy()
        );
    }
    for (from, why) in &r.rejected {
        eprintln!("ignored reply from {from}: {why:?}");
    }
    eprintln!("{} device(s)", r.found.len());
    Ok(())
}

fn identity(port: u16, host: Ipv4Addr) -> Result<(), String> {
    let mut s = Session::connect((host, port), TIMEOUT).map_err(|e| e.to_string())?;
    let names = [
        "vendor id",
        "device type",
        "product code",
        "revision",
        "status",
        "serial number",
        "product name",
    ];
    for (attr, name) in (1u16..).zip(names) {
        match s.get_attribute_single(class::IDENTITY, 1, attr) {
            Ok(b) => {
                let shown = match attr {
                    1..=3 | 5 if b.len() == 2 => u16::from_le_bytes([b[0], b[1]]).to_string(),
                    4 if b.len() == 2 => format!("{}.{}", b[0], b[1]),
                    6 if b.len() == 4 => format!("0x{:08X}", u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    7 if !b.is_empty() => String::from_utf8_lossy(&b[1..]).into_owned(),
                    _ => hex(&b),
                };
                println!("{attr}\t{name}\t{shown}");
            }
            Err(e) => println!("{attr}\t{name}\terror: {e}"),
        }
    }
    s.close();
    Ok(())
}

fn get(port: u16, host: Ipv4Addr, class_id: u16, instance: u16, attribute: u16) -> Result<(), String> {
    let mut session = Session::connect((host, port), TIMEOUT).map_err(|e| e.to_string())?;
    let result = session
        .get_attribute_single(class_id, instance, attribute)
        .map_err(|e| e.to_string());
    session.close();
    println!("{}", hex(&result?));
    Ok(())
}

fn io(port: u16, a: &IoArgs) -> Result<(), String> {
    let mut s = Session::connect((a.host, port), TIMEOUT).map_err(|e| e.to_string())?;
    let sock = if a.local_io_port == IO_PORT {
        bind_io_socket(Ipv4Addr::UNSPECIFIED)
    } else {
        UdpSocket::bind((Ipv4Addr::UNSPECIFIED, a.local_io_port))
    }
    .map_err(|e| format!("bind UDP {}: {e}", a.local_io_port))?;
    let cfg = if a.input_only {
        IoConfig::input_only(a.config, a.out, a.input, a.in_len, a.rpi)
    } else {
        IoConfig::exclusive_owner(a.config, a.out, a.input, a.out_len, a.in_len, a.rpi)
    };
    let conn = IoConnection::open(&mut s, &sock, SocketAddrV4::new(a.host, a.io_port), cfg)
        .map_err(|e| e.to_string())?;
    eprintln!(
        "open: O->T 0x{:08X} T->O 0x{:08X} rpi {}/{} us",
        conn.reply().o_to_t_connection_id,
        conn.reply().t_to_o_connection_id,
        conn.reply().o_to_t_api_us,
        conn.reply().t_to_o_api_us
    );
    if let Some(bytes) = &a.set {
        conn.set_output(bytes).map_err(|e| e.to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(a.seconds);
    let mut last: Option<Vec<u8>> = None;
    while Instant::now() < deadline {
        if let Some(input) = conn.wait_for_input(deadline.saturating_duration_since(Instant::now()))
            && last.as_ref() != Some(&input.data)
        {
            println!("{}", hex(&input.data));
            last = Some(input.data);
        }
    }
    let (rx, tx, ign) = (
        conn.packets_received(),
        conn.packets_sent(),
        conn.packets_ignored(),
    );
    let r = conn.close(&mut s).map_err(|e| e.to_string());
    s.close();
    r?;
    eprintln!("closed: {rx} in, {tx} out, {ign} ignored");
    if rx == 0 {
        return Err("no input packets arrived; is UDP 2222 reachable from the target?".into());
    }
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (parsed, port) = match args::parse_with_port(&argv) {
        Ok((cmd, port)) => (Ok(cmd), port),
        Err(e) => (Err(e), PORT),
    };
    let result = match parsed {
        Ok(Cmd::Help) => {
            println!("{}", args::USAGE);
            Ok(())
        }
        Ok(Cmd::Discover { target, window }) => discover(port, target, window),
        Ok(Cmd::Identity { host }) => identity(port, host),
        Ok(Cmd::Get {
            host,
            class,
            instance,
            attribute,
        }) => get(port, host, class, instance, attribute),
        Ok(Cmd::Io(a)) => io(port, &a),
        Err(e) => {
            eprintln!("{e}\n\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
