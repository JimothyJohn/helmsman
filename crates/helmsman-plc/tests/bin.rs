//! The `helmsman-plc` binary as a scanner sees it: a Structured Text
//! program, or a ladder project, behind a real CIP adapter on loopback.

use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::time::{Duration, Instant};

use helmsman_plc::store::{ChangeSet, DB_FILE, Meta, ModuleFiles, Store};
use helmsman_proto::cip::class;
use helmsman_proto::scanner::{Input, IoConfig, IoConnection};
use helmsman_proto::session::Session;

struct Running {
    child: Child,
    stderr: BufReader<ChildStderr>,
}

impl Running {
    /// Kills the process and returns what it wrote to stderr after the banner.
    fn finish(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut rest = String::new();
        let _ = self.stderr.read_to_string(&mut rest);
        rest
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn program_file(name: &str, src: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.st"));
    std::fs::write(&path, src).unwrap();
    path
}

fn ladder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../helmsman-iec/tests/plcopen/motor.xml")
}

fn plc() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    // The OPC UA server's default port is shared; tests/opcua.rs covers it.
    c.args(["--opcua", "off"]);
    c
}

/// A fresh, empty state directory for one PLC process.
fn state_dir() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "bin-state-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Starts the PLC on free loopback ports; returns it and its CIP and I/O addresses.
fn spawn(program: &Path, peer_io_port: u16) -> (Running, SocketAddrV4, SocketAddrV4) {
    spawn_with(&[program.as_os_str()], peer_io_port)
}

/// [`spawn`] with any program and `--module` arguments.
fn spawn_with(args: &[&std::ffi::OsStr], peer_io_port: u16) -> (Running, SocketAddrV4, SocketAddrV4) {
    let mut child = plc()
        .args(args)
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
        .args(["--peer-io-port", &peer_io_port.to_string()])
        // Its own API port and token, so parallel tests never share them.
        .args(["--api", "127.0.0.1:0", "--state-dir"])
        .arg(state_dir())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let addrs: Vec<SocketAddrV4> = line
        .split([' ', '(', ')', ','])
        .filter_map(|w| w.parse().ok())
        .collect();
    assert_eq!(addrs.len(), 2, "unexpected banner: {line:?}");
    (Running { child, stderr }, addrs[0], addrs[1])
}

fn wait_for(conn: &IoConnection, what: &str, mut ok: impl FnMut(&Input) -> bool) -> Input {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(input) = conn.latest_input()
            && ok(&input)
        {
            return input;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for {what}; last input {:?}",
        conn.latest_input()
    );
}

#[test]
fn program_drives_class1_io() {
    let prog = program_file(
        "drives_io",
        "PROGRAM io_test
         VAR
           a AT %ID0 : UDINT;  r AT %QD0 : UDINT;
           n : UINT;           count AT %QW3 : UINT;
         END_VAR
           r := a + 1;
           %QX4.0 := NOT %IX4.0;
           n := n + 1;
           count := n;
         END_PROGRAM",
    );
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, encap, io) = spawn(&prog, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    assert_eq!(
        s.get_attribute_single(class::IDENTITY, 1, 7).unwrap()[1..],
        *b"Helmsman PLC"
    );
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();

    let mut out = [0u8; 32];
    out[..4].copy_from_slice(&41u32.to_le_bytes());
    conn.set_output(&out).unwrap();
    let first = wait_for(&conn, "r = a + 1 and the inverted bit", |i| {
        i.data[..4] == 42u32.to_le_bytes() && i.data[4] & 1 == 1
    });
    let count = |i: &Input| u16::from_le_bytes([i.data[6], i.data[7]]);
    let c0 = count(&first);
    wait_for(&conn, "the scan counter to advance", |i| count(i) > c0);

    out[4] = 1;
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the bit to follow its input", |i| i.data[4] & 1 == 0);
    assert_eq!(conn.packets_ignored(), 0);
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn a_fault_zeroes_outputs_and_keeps_the_adapter_up() {
    let prog = program_file(
        "faults",
        "PROGRAM faulty
         VAR trigger AT %IX0.0 : BOOL; d AT %IB2 : USINT; q AT %QB1 : USINT; END_VAR
           %QB0 := 16#FF;
           IF trigger THEN
             q := 10 / d;
           END_IF;
         END_PROGRAM",
    );
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (plc, encap, io) = spawn(&prog, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    conn.set_output(&[0; 32]).unwrap();
    wait_for(&conn, "the program to run", |i| i.data[0] == 0xFF);

    let mut out = [0u8; 32];
    out[0] = 1; // trigger, with d = 0
    conn.set_output(&out).unwrap();
    wait_for(&conn, "outputs zeroed after the fault", |i| {
        i.data.iter().all(|&b| b == 0)
    });

    // Still on the network: explicit messaging answers.
    assert_eq!(
        s.get_attribute_single(class::IDENTITY, 1, 1).unwrap(),
        0xFFFFu16.to_le_bytes()
    );
    conn.close(&mut s).unwrap();
    s.close();
    let log = plc.finish();
    assert!(
        log.contains("STOP, outputs zeroed") && log.contains("faults.st: line 5: integer division by zero"),
        "{log}"
    );
}

#[test]
fn modules_share_the_image_and_a_fault_stops_only_its_module() {
    // `counter` owns output byte 0 and counts; `faulty` owns bytes 1..3 and
    // divides by its input byte 2 when bit 0.0 is set. Both read all inputs.
    let counter = program_file(
        "mm_counter",
        "PROGRAM counter
         VAR n : USINT; o AT %QB0 : USINT; END_VAR
           n := n + 1;
           o := n;
         END_PROGRAM",
    );
    let faulty = program_file(
        "mm_faulty",
        "PROGRAM faulty
         VAR trigger AT %IX0.0 : BOOL; d AT %IB2 : USINT; q AT %QB1 : USINT; END_VAR
           %QB0 := 16#FF;
           IF trigger THEN
             q := 10 / d;
           END_IF;
         END_PROGRAM",
    );
    let counter_spec = format!("{},scan=2,q=0+1", counter.display());
    let faulty_spec = format!("{},name=bad,scan=5,q=1+2,m=16", faulty.display());
    let args = ["--module", &counter_spec, "--module", &faulty_spec].map(std::ffi::OsStr::new);
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (plc, encap, io) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    conn.set_output(&[0; 32]).unwrap();
    let first = wait_for(&conn, "both modules to run", |i| {
        i.data[1] == 0xFF && i.data[0] > 0
    });
    assert!(
        first.data[3..].iter().all(|&b| b == 0),
        "unowned bytes: {:?}",
        first.data
    );

    let mut out = [0u8; 32];
    out[0] = 1; // trigger, with d = 0
    conn.set_output(&out).unwrap();
    let after = wait_for(&conn, "bad's outputs zeroed", |i| i.data[1..3] == [0, 0]);
    let c0 = after.data[0];
    wait_for(&conn, "counter to keep counting", |i| i.data[0] != c0);

    conn.close(&mut s).unwrap();
    s.close();
    let log = plc.finish();
    assert!(
        log.contains("STOP, outputs zeroed")
            && log.contains("mm_faulty.st: line 5: integer division by zero (module bad)"),
        "{log}"
    );
    assert!(!log.contains("module mm_counter"), "only bad stopped: {log}");
}

#[test]
fn modules_fighting_for_an_output_are_refused_before_the_adapter_starts() {
    let prog = program_file(
        "fight",
        "PROGRAM p VAR o AT %QB0 : BYTE; END_VAR o := 1; END_PROGRAM",
    );
    // Two bare programs both default to the whole output image.
    let out = plc()
        .arg(&prog)
        .arg(&prog)
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("module \"fight\" defined twice"), "{err}");
    assert!(err.contains("fight and fight drive the same %Q bytes"), "{err}");
    assert!(
        !err.contains("CIP on"),
        "refused before going on the network: {err}"
    );

    let a = format!("{},name=a,q=0+4", prog.display());
    let b = format!("{},name=b,q=3+4", prog.display());
    let out = plc()
        .args([
            "--module",
            &a,
            "--module",
            &b,
            "--encap-port",
            "0",
            "--io-port",
            "0",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("a and b drive the same %Q bytes"), "{err}");

    for (spec, why) in [
        (
            format!("{},q=30+4", prog.display()),
            "%Q window reaches past the image",
        ),
        (format!("{},q=0+1,bogus=1", prog.display()), "unknown key"),
        (format!("{},name=a\nb", prog.display()), "--module name"),
        (format!("{},scan=0", prog.display()), "--module scan"),
        (",q=0+1".into(), "needs a path first"),
    ] {
        let out = plc()
            .args(["--module", &spec, "--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{spec}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(why), "{spec}: {err}");
    }
}

fn text_file(name: &str, text: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn wait_until(what: &str, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_io_map_lets_a_module_drive_a_modbus_device_by_signal_name() {
    use helmsman_io::sim::{Fault, Sim, State};
    let mut st = State::new(8);
    st.wires.push((0, 0));
    let sim = Sim::start(st).unwrap();
    let map = text_file(
        "bench.iomap",
        &format!(
            "device rio modbus-tcp {} timeout_ms=200 poll_ms=5
in  start bool rio di:1
in  fb    bool rio di:0   # wired from coil 0
out motor bool rio coil:0
out lamp  bool box do:0 safe=true
",
            sim.endpoint()
        ),
    );
    let prog = program_file(
        "io_line",
        "PROGRAM line
         VAR_INPUT start : BOOL; fb : BOOL; END_VAR
         VAR_OUTPUT motor : BOOL; lamp : BOOL; END_VAR
           motor := start;
           lamp := fb;
         END_PROGRAM",
    );
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let args = [prog.as_os_str(), "--io-map".as_ref(), map.as_os_str()];
    let (plc, _, _) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());

    wait_until("the module to run", || sim.state().requests > 3);
    assert!(!sim.state().coils[0]);
    sim.state().discrete[1] = true;
    wait_until("coil 0 to follow start", || sim.state().coils[0]);

    for _ in 0..20 {
        sim.inject(Fault::Drop);
    }
    wait_until("the drops to be used up", || sim.state().faults.is_empty());
    wait_until("coil 0 back after the drop", || sim.state().coils[0]);

    let log = plc.finish();
    for want in [
        "box channels (do:N, di:N, i2c:...) are in memory only",
        "device rio up",
        "device box up",
        "device rio down: io:",
        "HOLD, outputs safe: a signal module io_line reads has no value",
        "module io_line running again",
    ] {
        assert!(log.contains(want), "missing {want:?} in:\n{log}");
    }
}

/// The box owned by a scanner and mastering an E1213 over `enip-io` at the
/// same time, on its one I/O port: no `io_port` on the link, nothing moved
/// apart. The scanner's bit goes through the module to the E1213's DO-00,
/// back on its wired DI-00, through the module again and out to the
/// scanner. (`--io-port 0` only so parallel test runs don't fight over
/// 2222; adapter and link share whatever port it is.) Only with the
/// `enip-io` feature (`scripts/lint.sh` builds it; out of the box build).
#[cfg(feature = "enip-io")]
#[test]
fn the_adapter_and_an_enip_io_link_share_one_io_port() {
    use helmsman_io::enip_sim::E1213Sim;
    let sim = E1213Sim::start().unwrap();
    let mut params = sim.params();
    params.remove("io_port");
    let params: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let map = text_file(
        "shared_port.iomap",
        &format!(
            "device rio enip-io {} {}
out clamp    bool rio do:0 safe=false
in  clamp_fb bool rio di:0
",
            sim.endpoint(),
            params.join(" ")
        ),
    );
    let prog = program_file(
        "shared_port",
        "PROGRAM line
         VAR_INPUT clamp_fb : BOOL; END_VAR
         VAR_OUTPUT clamp : BOOL; END_VAR
         VAR go AT %IX0.0 : BOOL; fb AT %QX0.0 : BOOL; END_VAR
           clamp := go;
           fb := clamp_fb;
         END_PROGRAM",
    );
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let args = [prog.as_os_str(), "--io-map".as_ref(), map.as_os_str()];
    let (plc, encap, io) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();

    wait_until("the link to the E1213", || !sim.o_to_t_sources().is_empty());
    // The box's O->T to the E1213 leaves from the port the scanner talks to.
    assert_eq!(sim.o_to_t_sources(), vec![io]);
    let mut out = [0u8; 32];
    out[0] = 1;
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the scanner's bit back through the E1213", |i| {
        i.data[0] & 1 == 1
    });
    out[0] = 0;
    conn.set_output(&out).unwrap();
    wait_for(&conn, "and off again", |i| i.data[0] & 1 == 0);
    assert_eq!(conn.packets_ignored(), 0);
    conn.close(&mut s).unwrap();
    s.close();
    let log = plc.finish();
    assert!(log.contains("device rio up"), "{log}");
    assert!(!log.contains("address in use"), "{log}");
}

/// The box build leaves `enip-io` out (TODO.md `enip-in-box`): a map that
/// names it is refused before the box goes on the network, saying so,
/// even when no signal uses the device. The E1213 runs over Modbus/TCP.
#[cfg(not(feature = "enip-io"))]
#[test]
fn a_map_naming_enip_io_is_refused_as_not_in_this_build() {
    let prog = program_file(
        "enip_out",
        "PROGRAM p VAR_INPUT start : BOOL; END_VAR VAR_OUTPUT lamp : BOOL; END_VAR lamp := start; END_PROGRAM",
    );
    for (name, text) in [
        (
            "enip_used.iomap",
            "device rio enip-io 127.0.0.1:44818\nin start bool rio di:0\nout lamp bool box do:0\n",
        ),
        (
            "enip_unused.iomap",
            "device rio enip-io 127.0.0.1:44818\nin start bool box di:0\nout lamp bool box do:0\n",
        ),
    ] {
        let map = text_file(name, text);
        let out = plc()
            .arg(&prog)
            .arg("--io-map")
            .arg(&map)
            .args(["--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{name}: {err}");
        assert!(
            err.contains("device rio: unknown driver \"enip-io: not in this build"),
            "{name}: {err}"
        );
        assert!(
            err.contains("modbus-tcp"),
            "{name}: names the way that works: {err}"
        );
        assert!(!err.contains("CIP on"), "{name}: {err}");
    }
}

#[test]
fn a_bad_io_map_is_refused_before_the_adapter_starts() {
    let prog = program_file(
        "io_ports",
        "PROGRAM p VAR_INPUT start : BOOL; END_VAR VAR_OUTPUT lamp : BOOL; END_VAR lamp := start; END_PROGRAM",
    );
    for (name, text, want) in [
        (
            "syntax.iomap",
            "in start bool\nout lamp bool box do:0 safe=maybe\n",
            "syntax.iomap:1: in: too few words",
        ),
        (
            "invalid.iomap",
            "in start bool nowhere di:0\nout lamp bool box do:0\n",
            "invalid.iomap: start: no device \"nowhere\"",
        ),
        (
            "unbound.iomap",
            "in begin bool box di:0\nout lamp bool box do:0\n",
            "io_ports: port START: no signal of that name",
        ),
        (
            "driver.iomap",
            "device d nosuch h:1\nin start bool d x\nout lamp bool box do:0\n",
            "unknown driver",
        ),
    ] {
        let map = text_file(name, text);
        let out = plc()
            .arg(&prog)
            .arg("--io-map")
            .arg(&map)
            .args(["--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{name}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{name}: {err}");
        assert!(
            !err.contains("CIP on"),
            "{name}: refused before going on the network: {err}"
        );
    }
    let out = plc()
        .arg(&prog)
        .args(["--io-map", "/nonexistent/x.iomap"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let out = plc()
        .arg(&prog)
        .args(["--io-map", "a", "--io-map", "b"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "one --io-map only");
}

#[test]
fn a_gpio_chip_that_is_not_there_is_refused_before_the_adapter_starts() {
    let prog = program_file(
        "gpio_ports",
        "PROGRAM p VAR_INPUT start : BOOL; END_VAR VAR_OUTPUT lamp : BOOL; END_VAR lamp := start; END_PROGRAM",
    );
    let map = text_file(
        "gpio.iomap",
        "in start bool box di:4\nout lamp bool box do:17\nin temp int box i2c:1:0x48:0:i16\n",
    );
    let out = plc()
        .arg(&prog)
        .arg("--io-map")
        .arg(&map)
        .args(["--gpio-chip", "/nonexistent/gpiochip9"])
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("--gpio-chip"), "{err}");
    if cfg!(not(target_os = "linux")) {
        assert!(err.contains("Linux only"), "{err}");
    }
    assert!(
        !err.contains("CIP on"),
        "refused before going on the network: {err}"
    );
    let out = plc()
        .arg(&prog)
        .args(["--gpio-chip", "a", "--gpio-chip", "b"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "one --gpio-chip only");
}

#[test]
fn compile_errors_exit_1_with_file_line_col() {
    let prog = program_file(
        "bad",
        "PROGRAM bad\nVAR x : SINT; END_VAR\n  x := 300;\n  y := 1;\nEND_PROGRAM",
    );
    let out = plc()
        .arg(&prog)
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bad.st:3:8: 300 does not fit SINT"), "{err}");
    assert!(err.contains("bad.st:4:3: undeclared variable Y"), "{err}");
}

#[test]
fn bad_arguments_exit_2_with_usage() {
    for args in [
        &[][..],
        &["--module"],
        &["a.st", "--scan-ms", "0"],
        &["a.st", "--serial", "xyz"],
        &["a.st", "--memory", "999999"],
        &["a.st", "--bogus", "1"],
    ] {
        let out = plc().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("usage:"),
            "{args:?}"
        );
    }
    let out = plc().arg("/nonexistent/x.st").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn shipped_echo_program_echoes() {
    let prog = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st");
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, encap, io) = spawn(&prog, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    let pattern: Vec<u8> = (1..=32).collect();
    conn.set_output(&pattern).unwrap();
    conn.wait_for_input_eq(&pattern, Duration::from_secs(3))
        .expect("echo.st did not echo all 32 bytes");
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn ladder_project_drives_class1_io() {
    // motor.xml: Start = %IX0.0, Stop = %IX0.1, Lamp = %QX0.0 (sealed in).
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, encap, io) = spawn(&ladder(), scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    let mut out = [0u8; 32];
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the lamp off", |i| i.data[0] & 1 == 0);
    out[0] = 0b01; // Start
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the lamp on", |i| i.data[0] & 1 == 1);
    out[0] = 0;
    conn.set_output(&out).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    wait_for(&conn, "the lamp sealed in", |i| i.data[0] & 1 == 1);
    out[0] = 0b10; // Stop
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the lamp off after Stop", |i| i.data[0] & 1 == 0);
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn emit_st_prints_the_lowered_project() {
    let out = plc().arg(ladder()).arg("--emit-st").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let st = String::from_utf8_lossy(&out.stdout);
    assert!(st.starts_with("PROGRAM motor\n"), "{st}");
    assert!(st.contains("delay(IN := Run, PT := T#2s);"), "{st}");

    let st_prog = program_file("not_xml", "PROGRAM p END_PROGRAM");
    let out = plc().arg(&st_prog).arg("--emit-st").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn project_errors_exit_1_with_xml_positions() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bad.xml");
    let xml = std::fs::read_to_string(ladder())
        .unwrap()
        .replace("<variable>Stop</variable>", "<variable>Stopp</variable>");
    std::fs::write(&path, &xml).unwrap();
    let line = xml
        .lines()
        .position(|l| l.contains("<contact localId=\"4\""))
        .unwrap()
        + 1;
    let out = plc()
        .arg(&path)
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("bad.xml:{line}:13: undeclared variable STOPP")),
        "{err}"
    );
}

#[test]
fn a_module_sees_the_shift_calendar_through_the_scan() {
    let cal = text_file("allday.cal", "zone UTC\nshift 7 daily 00:00-24:00\n");
    let prog = program_file(
        "cal_reader",
        "PROGRAM shifts
         VAR_INPUT in_shift : BOOL; shift_id : INT; END_VAR
         VAR on AT %QX0.0 : BOOL; id AT %QW1 : INT; END_VAR
           on := in_shift;
           id := shift_id;
         END_PROGRAM",
    );
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let args = [prog.as_os_str(), "--calendar".as_ref(), cal.as_os_str()];
    let (plc, encap, io) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    wait_for(&conn, "in shift 7 all day", |i| {
        i.data[0] & 1 == 1 && i.data[2..4] == 7u16.to_le_bytes()
    });
    conn.close(&mut s).unwrap();
    s.close();
    let log = plc.finish();
    let want = "calendar: 1 shift pattern(s), 0 drive(s); clock trusted";
    assert!(log.contains(want), "missing {want:?} in:\n{log}");
}

#[test]
fn a_bad_calendar_is_refused_before_the_adapter_starts() {
    let prog = program_file(
        "cal_lamp",
        "PROGRAM p VAR_INPUT in_shift : BOOL; END_VAR VAR_OUTPUT lamp : BOOL; END_VAR lamp := in_shift; END_PROGRAM",
    );
    let map = text_file(
        "cal_lamp.iomap",
        "out lamp bool box do:0\nout horn bool box do:1\n",
    );
    for (name, text, want) in [
        (
            "syntax.cal",
            "zone UTC\nshift 1 mon 6:00-14:00\n",
            "syntax.cal: line 2: \"6:00-14:00\"",
        ),
        ("tz.cal", "zone Europe/Berlin\n", "time zone database"),
        (
            "overlap.cal",
            "zone UTC\nshift 1 mon 06:00-14:00\nshift 2 mon 13:00-22:00\n",
            "shifts 1 and 2 overlap",
        ),
        (
            "drive.cal",
            "drive lamp in_shift\n",
            "calendar and cal_lamp both drive signal LAMP",
        ),
        (
            "drive_none.cal",
            "drive nothing in_shift\n",
            "drive nothing: no signal of that name",
        ),
    ] {
        let cal = text_file(name, text);
        let out = plc()
            .arg(&prog)
            .arg("--io-map")
            .arg(&map)
            .arg("--calendar")
            .arg(&cal)
            .args(["--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{name}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{name}: {err}");
        assert!(
            !err.contains("CIP on"),
            "{name}: refused before going on the network: {err}"
        );
    }
    let out = plc()
        .arg(&prog)
        .args(["--calendar", "a", "--calendar", "b"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "one --calendar only");
}

fn module_dir(name: &str, manifest: &str, source: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bin-modules")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("module.manifest"), manifest).unwrap();
    std::fs::write(dir.join("module.st"), source).unwrap();
    dir
}

#[test]
fn a_module_directory_runs_with_its_param_beside_a_module_flag() {
    // The directory's module owns output byte 0 and writes its param there;
    // a --module owns byte 1. Both behind one adapter.
    let dir = module_dir(
        "offset",
        "name offset\nauthor test\nscan_ms 2\nq 0+1\nreads\ndrives\n\
         param base int 40 min=0 max=200\n",
        "PROGRAM offset
         VAR_INPUT base : USINT := 0; END_VAR
         VAR o AT %QB0 : USINT; END_VAR
           o := base + 2;
         END_PROGRAM",
    );
    let other = program_file(
        "beside_dir",
        "PROGRAM p VAR o AT %QB0 : USINT; END_VAR o := 7; END_PROGRAM",
    );
    let spec = format!("{},scan=2,q=1+1", other.display());
    let args = [
        std::ffi::OsStr::new("--module-dir"),
        dir.as_os_str(),
        std::ffi::OsStr::new("--module"),
        std::ffi::OsStr::new(&spec),
    ];
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, encap, io) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    conn.set_output(&[0; 32]).unwrap();
    wait_for(&conn, "the param through the directory's module", |i| {
        i.data[..2] == [42, 7]
    });
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn a_module_directory_whose_manifest_lies_is_refused_before_the_adapter_starts() {
    let src = "PROGRAM p VAR_INPUT a : BOOL; END_VAR VAR_OUTPUT b : BOOL; END_VAR b := a; END_PROGRAM";
    for (name, manifest, why) in [
        (
            "lies",
            "name lies\nauthor t\nscan_ms 10\nreads a\ndrives\n",
            "VAR_OUTPUT B is not in the manifest",
        ),
        (
            "badline",
            "name badline\nauthor t\nscan_ms 10\nreads a\ndrives b\nbogus 1\n",
            "module.manifest:6: unknown key",
        ),
        (
            "outofrange",
            "name r\nauthor t\nscan_ms 10\nreads\ndrives b\nparam a int 9 min=0 max=1\n",
            "param a: 9 is outside 0..=1",
        ),
    ] {
        let dir = module_dir(name, manifest, src);
        let out = plc()
            .args([std::ffi::OsStr::new("--module-dir"), dir.as_os_str()])
            .args(["--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{name}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(why), "{name}: {err}");
        assert!(!err.contains("CIP on"), "{name}: {err}");
    }
}

#[test]
fn a_wasm_module_directory_runs_and_one_whose_manifest_lies_is_refused() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bin-modules")
        .join("blink-wasm");
    std::fs::create_dir_all(&dir).unwrap();
    let wasm = wat::parse_str(
        r#"(module (import "helmsman/v1" "now_ms" (func $now (result i64)))
                   (global $t (export "t") (mut i64) (i64.const 0))
                   (func (export "scan") (global.set $t (call $now))))"#,
    )
    .unwrap();
    std::fs::write(dir.join("module.wasm"), &wasm).unwrap();
    std::fs::write(
        dir.join("module.manifest"),
        "name blink\nauthor t\nscan_ms 10\nreads\ndrives\n",
    )
    .unwrap();
    let args = [std::ffi::OsStr::new("--module-dir"), dir.as_os_str()];
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, _, _) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());

    std::fs::write(
        dir.join("module.manifest"),
        "name blink\nauthor t\nscan_ms 10\nreads\ndrives lamp\n",
    )
    .unwrap();
    let out = plc()
        .args([std::ffi::OsStr::new("--module-dir"), dir.as_os_str()])
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("does not match module.wasm"), "{err}");
    assert!(!err.contains("CIP on"), "{err}");
}

// ---- --store -----------------------------------------------------------------

fn store_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bin-stores")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// One module that writes `base + 2` to output byte 0 while the stored
/// calendar says it is in shift (always), with an I/O map beside it.
fn offset_set(base: u8) -> ChangeSet {
    let module = ModuleFiles {
        manifest: format!(
            "name offset\nauthor test\nscan_ms 2\nq 0+1\nreads in_shift\ndrives\n\
             param base int {base} min=0 max=200\n"
        ),
        code: "PROGRAM offset
                 VAR_INPUT base : USINT := 0; in_shift : BOOL; END_VAR
                 VAR o AT %QB0 : USINT; END_VAR
                   IF in_shift THEN o := base + 2; ELSE o := 1; END_IF;
                 END_PROGRAM"
            .into(),
    };
    ChangeSet {
        modules: [("offset".to_string(), module)].into_iter().collect(),
        io_map: Some("out lamp bool box do:0\n".into()),
        calendar: Some("zone UTC\nshift 7 daily 00:00-24:00\n".into()),
        drivers: std::collections::BTreeMap::new(),
    }
}

fn store_meta(author: &str) -> Meta {
    Meta {
        author: author.into(),
        prompt: String::new(),
        approver: None,
        time_ms: 0,
    }
}

#[test]
fn the_store_runs_its_head_snapshot_after_a_rollback() {
    let dir = store_dir("rollback");
    let mut store = Store::open(&dir).unwrap();
    let a = store.commit(&offset_set(40), &store_meta("agent")).unwrap();
    store.commit(&offset_set(60), &store_meta("agent")).unwrap();
    store.rollback(&a, &store_meta("Nick")).unwrap();
    drop(store);
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let args = [std::ffi::OsStr::new("--store"), dir.as_os_str()];
    let (plc, encap, io) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    conn.set_output(&[0; 32]).unwrap();
    wait_for(
        &conn,
        "the rolled-back param, in shift by the stored calendar",
        |i| i.data[0] == 42,
    );
    conn.close(&mut s).unwrap();
    s.close();
    let log = plc.finish();
    let want = format!("snapshot {a} (entry 3, rolled back to {a} by Nick)");
    assert!(log.contains(&want), "missing {want:?} in:\n{log}");
}

#[test]
fn a_store_that_cannot_run_is_refused_before_the_adapter_starts() {
    let empty = store_dir("empty");
    Store::open(&empty).unwrap();
    let newer = store_dir("newer");
    Store::open(&newer).unwrap();
    rusqlite::Connection::open(newer.join(DB_FILE))
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let foreign = store_dir("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join(DB_FILE), [0x5a; 4096]).unwrap();
    let bad_module = store_dir("bad-module");
    let mut set = offset_set(40);
    set.modules.get_mut("offset").unwrap().code = "PROGRAM offset END_PROGRAM".into();
    Store::open(&bad_module)
        .unwrap()
        .commit(&set, &store_meta("agent"))
        .unwrap();
    for (dir, why) in [
        (&empty, "has no snapshot yet"),
        (&newer, "newer than this build"),
        (&foreign, "snapshot store"),
        (&bad_module, "does not match module.st"),
    ] {
        let out = plc()
            .args([std::ffi::OsStr::new("--store"), dir.as_os_str()])
            .args(["--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{why}: {err}");
        assert!(err.contains(why), "{why}: {err}");
        assert!(!err.contains("CIP on"), "{why}: {err}");
    }
    let out = plc()
        .args([std::ffi::OsStr::new("--store"), empty.as_os_str()])
        .args(["--module-dir", "x"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "--store takes no other module source");
}

/// Starts the PLC with its API on a free loopback port and `state` as its
/// state directory; returns it, the API address and the token line.
fn spawn_api(state: &Path) -> (Running, std::net::SocketAddr, String) {
    let echo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st");
    let mut child = plc()
        .arg(&echo)
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--api", "127.0.0.1:0", "--state-dir"])
        .arg(state)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let (mut api, mut token) = (None, None);
    while api.is_none() || token.is_none() {
        let mut line = String::new();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "PLC exited");
        if let Some(rest) = line.strip_prefix("helmsman-plc: API on http://") {
            api = rest.split('/').next().and_then(|a| a.parse().ok());
        } else if line.starts_with("helmsman-plc: API token") {
            token = Some(line.trim_end().to_string());
        }
    }
    (Running { child, stderr }, api.unwrap(), token.unwrap())
}

fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    use std::io::Write as _;
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: box\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn the_api_serves_the_running_box_and_its_token_is_printed_once() {
    let state = state_dir();
    let (plc, api, line) = spawn_api(&state);
    let token_file = state.join("api-token");
    let token = std::fs::read_to_string(&token_file).unwrap().trim().to_string();
    assert_eq!(token.len(), 64);
    assert!(
        line.contains("new, shown once") && line.ends_with(&token),
        "{line}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut modules = http_get(api, "/api/modules");
    while !modules.contains("\"state\":\"running\"") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        modules = http_get(api, "/api/modules");
    }
    assert!(modules.starts_with("HTTP/1.1 200 OK"), "{modules}");
    assert!(
        modules.contains("\"name\":\"echo\"") && modules.contains("\"state\":\"running\""),
        "{modules}"
    );
    drop(plc);

    let (plc, _, line) = spawn_api(&state);
    assert!(
        !line.contains(&token),
        "the token is printed only when made: {line}"
    );
    assert!(line.contains("API token in"), "{line}");
    let rest = plc.finish();
    assert!(!rest.contains(&token), "{rest}");
}

#[test]
fn a_taken_api_port_stops_the_plc_with_a_message() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let echo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st");
    let out = plc()
        .arg(&echo)
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--api", &addr, "--state-dir"])
        .arg(state_dir())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("--api {addr}")), "{err}");
    for bad in ["8080", "localhost:8080", "on", ""] {
        let out = plc().arg(&echo).args(["--api", bad]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "--api {bad:?}");
    }
}

fn set_pin(dir: &Path, role: &str, pin: &str) -> std::process::Output {
    use std::io::Write as _;
    let mut child = plc()
        .args(["--set-pin", role, "--state-dir"])
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{pin}\n").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn set_pin_stores_a_slow_salted_private_hash_and_never_prints_the_pin() {
    use helmsman_plc::policy::pin::{self, Pins};
    let dir = state_dir();
    let out = set_pin(&dir, "engineer", "eng-135797");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let out = set_pin(&dir, "operator", "op-246810");
    assert_eq!(out.status.code(), Some(0));
    let text = std::fs::read_to_string(dir.join(pin::FILE)).unwrap();
    let shown = format!("{err}{}{text}", String::from_utf8_lossy(&out.stderr));
    assert!(!shown.contains("135797") && !shown.contains("246810"), "{shown}");
    assert!(
        text.contains(&format!("engineer pbkdf2-sha256 {} ", pin::ITERATIONS)),
        "{text}"
    );
    let salts: Vec<&str> = text.lines().skip(1).filter_map(|l| l.split(' ').nth(3)).collect();
    assert_eq!(salts.len(), 2);
    assert_ne!(salts[0], salts[1], "a salt each");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(pin::FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let pins = Pins::load(&dir, pin::MIN_ITERATIONS).unwrap();
    assert!(pins.has(helmsman_plc::policy::Role::Engineer) && pins.has(helmsman_plc::policy::Role::Operator));
    for (role, weak) in [
        ("engineer", "123"),
        ("operator", "has space"),
        ("root", "eng-135797"),
    ] {
        let out = set_pin(&dir, role, weak);
        assert_ne!(out.status.code(), Some(0), "{role} {weak:?}");
    }
    assert_eq!(
        std::fs::read_to_string(dir.join(pin::FILE)).unwrap(),
        text,
        "refusals change nothing"
    );
}

/// `helmsman-plc account ARGS... --state-dir DIR`, `stdin` on its stdin.
fn account(dir: &Path, args: &[&str], stdin: &str) -> (Option<i32>, String) {
    use std::io::Write as _;
    // Not plc(): a subcommand must be the first argument, and the account
    // CLI never starts the OPC UA server anyway.
    let mut child = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .arg("account")
        .args(args)
        .arg("--state-dir")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn account_cli_reads_passwords_from_stdin_only_and_keeps_the_file_private() {
    use helmsman_plc::policy::account::{self, Accounts};
    use helmsman_plc::policy::pin;
    let dir = state_dir();
    let (code, out) = account(&dir, &["list"], "");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("role PINs"), "{out}");
    let (code, out) = account(&dir, &["add", "alice", "engineer"], "alice-secret-9\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("role PINs are refused"), "the switch is said: {out}");
    let mut shown = out;
    // A password on the command line is a usage error, never stored.
    let (code, out) = account(&dir, &["add", "bob", "operator", "bob-secret-77"], "");
    assert_ne!(code, Some(0), "{out}");
    shown.push_str(&out);
    for (args, stdin) in [
        (vec!["add", "alice", "operator"], "another-pass-1\n"),
        (vec!["add", "Bob", "operator"], "bob-secret-77\n"),
        (vec!["add", "bob", "root"], "bob-secret-77\n"),
        (vec!["add", "bob", "operator"], "short\n"),
        (vec!["passwd", "nobody"], "whatever-pass\n"),
        (vec!["disable", "nobody"], ""),
    ] {
        let (code, out) = account(&dir, &args, stdin);
        assert_ne!(code, Some(0), "{args:?}: {out}");
        shown.push_str(&out);
    }
    let (code, out) = account(&dir, &["disable", "alice"], "");
    assert_eq!(code, Some(0), "{out}");
    let (code, out) = account(&dir, &["list"], "");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(out, "alice engineer disabled\n");
    let text = std::fs::read_to_string(dir.join(account::FILE)).unwrap();
    shown.push_str(&text);
    for secret in ["alice-secret-9", "bob-secret-77", "another-pass-1"] {
        assert!(!shown.contains(secret), "{secret}: {shown}");
    }
    assert!(
        text.contains(&format!(
            "alice engineer disabled pbkdf2-sha256 {} ",
            pin::ITERATIONS
        )),
        "{text}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(account::FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let a = Accounts::load(&dir, pin::MIN_ITERATIONS).unwrap().unwrap();
    assert_eq!(a.len(), 1);
    // An accounts file others can read, or one that does not parse, stops
    // the box before it is on the network.
    let echo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join(account::FILE), std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let bad = state_dir();
    std::fs::create_dir_all(&bad).unwrap();
    Accounts::default().save(&bad).unwrap();
    std::fs::write(
        bad.join(account::FILE),
        "helmsman-accounts 1\nmallory integrator enabled\n",
    )
    .unwrap();
    for (state, want) in [(&dir, "readable by others"), (&bad, ":2: expected")] {
        if cfg!(not(unix)) && want == "readable by others" {
            continue;
        }
        let out = plc()
            .arg(&echo)
            .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
            .args(["--api", "127.0.0.1:0", "--state-dir"])
            .arg(state)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{want}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{want}: {err}");
        assert!(
            !err.contains("CIP on"),
            "refused before the adapter started: {err}"
        );
    }
}

#[test]
fn a_policy_that_loosens_the_floor_stops_the_box_before_it_is_on_the_network() {
    let echo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st");
    for (text, want) in [
        (
            "helmsman-policy 1\noutput-affecting operator replay=no\n",
            ":2: output-affecting: role operator is below the floor",
        ),
        ("helmsman-policy 1\nlockout 50 1\n", ":2: lockout"),
        ("allow all\n", ":1: expected"),
    ] {
        let policy = text_file("loose.policy", text);
        let out = plc()
            .arg(&echo)
            .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
            .args(["--api", "127.0.0.1:0", "--state-dir"])
            .arg(state_dir())
            .arg("--policy")
            .arg(&policy)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{text}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{text}: {err}");
        assert!(
            !err.contains("CIP on"),
            "refused before the adapter started: {err}"
        );
    }
}

#[test]
fn a_bad_screen_layout_is_refused_before_the_adapter_starts() {
    let prog = program_file(
        "screen_line",
        "PROGRAM p VAR_OUTPUT line1_state : INT; END_VAR line1_state := 1; END_PROGRAM",
    );
    let map = text_file("screen_line.iomap", "out line1_state int screen line1 safe=0\n");
    let good = "helmsman-screen 1\ngrid 1 1\ncode s 1 green Running\ntile line1 0,0 s line1_state Line 1\n";
    for (name, text, want) in [
        (
            "syntax.screen",
            "helmsman-screen 1\ngrid 1 1\ntile\n",
            "syntax.screen: line 3: tile",
        ),
        (
            "unfit.screen",
            "helmsman-screen 1\ngrid 1 1\ncode s 1 green Running\ntile line1 0,0 s nosuch Line 1\n",
            "unfit.screen: tile line1: no signal nosuch in the I/O map",
        ),
    ] {
        let screen = text_file(name, text);
        let out = plc()
            .arg(&prog)
            .arg("--io-map")
            .arg(&map)
            .arg("--screen")
            .arg(&screen)
            .args(["--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{name}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{name}: {err}");
        assert!(!err.contains("CIP on"), "{name}: {err}");
    }
    // Screen tiles in the map need their layout while the API runs.
    let out = plc()
        .arg(&prog)
        .arg("--io-map")
        .arg(&map)
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--screen FILE"));
    let screen = text_file("good.screen", good);
    let out = plc()
        .arg(&prog)
        .arg("--screen")
        .arg(&screen)
        .args(["--api", "off"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "--screen needs the API");
}

/// The relay serves agent v3 (the drop-in tools) and explain-v1 (read-only
/// tools, floor language); a box asks for them unless told otherwise.
#[test]
fn the_default_agent_versions_are_the_ones_the_relay_serves() {
    let out = plc().arg("--help").output().unwrap();
    let usage = String::from_utf8_lossy(&out.stdout);
    assert!(usage.contains("(default v3)"), "{usage}");
    assert!(usage.contains("default explain-v1"), "{usage}");
}
