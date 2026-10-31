//! Modbus/TCP as a `helmsman-driver/v1` guest (helmsman-io's `wasm`
//! module): the native driver (`helmsman_io::modbus`) ported to the
//! sandbox, as an agent would write one. Same point syntax, same request
//! PDUs, same reply checks, the same errors by kind; no dependencies, no
//! allocator. Its only way out is the host's socket to its own device.
//!
//! References (docs/SOURCES.md): MODBUS Application Protocol
//! Specification V1.1b3 ("App"), MODBUS Messaging on TCP/IP
//! Implementation Guide V1.0b ("TCP").
//!
//! Rebuild with `scripts/build-wasm-drivers.sh`.

#![no_std]

use core::cell::UnsafeCell;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU8, AtomicU16, Ordering::Relaxed};

#[link(wasm_import_module = "helmsman-driver/v1")]
unsafe extern "C" {
    fn send(ptr: *const u8, len: usize) -> i32;
    fn recv(ptr: *mut u8, len: usize) -> i32;
    fn close();
    fn point(ty: i32, writable: i32);
    fn value(ty: i32, bits: i64);
    fn fail(kind: i32, code: i32, ptr: *const u8, len: usize);
}

const BOOL: i32 = 0;
const INT: i32 = 1;
const REAL: i32 = 2;

const ADDRESS: i32 = 1;
const TYPE: i32 = 2;
const PROTOCOL: i32 = 3;
const EXCEPTION: i32 = 4;
const READ_ONLY: i32 = 5;
const POINT: i32 = 6;
const IO: i32 = 7;

/// App §4.1 p.3: the PDU is at most 253 bytes.
const MAX_PDU: usize = 253;
/// TCP §3.1.3 p.5: transaction id, protocol id, length, unit id.
const MBAP_LEN: usize = 7;

const READ_COILS: u8 = 0x01;
const READ_DISCRETE_INPUTS: u8 = 0x02;
const READ_HOLDING_REGISTERS: u8 = 0x03;
const READ_INPUT_REGISTERS: u8 = 0x04;
const WRITE_SINGLE_COIL: u8 = 0x05;
const WRITE_SINGLE_REGISTER: u8 = 0x06;
const WRITE_MULTIPLE_REGISTERS: u8 = 0x10;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

/// A message built without an allocator.
struct Msg {
    buf: [u8; 64],
    len: usize,
}

impl Msg {
    fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl core::fmt::Write for Msg {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

enum Error {
    /// Names the address it is about.
    Address,
    /// The type the point wants.
    Type(i32),
    Protocol(&'static str),
    Exception(u8),
    ReadOnly,
    Point(Msg),
    /// The host holds the socket error itself.
    Io,
}

fn report(e: &Error, subject: &[u8]) {
    let (kind, code, msg): (i32, i32, &[u8]) = match e {
        Error::Address => (ADDRESS, 0, subject),
        Error::Type(t) => (TYPE, *t, b""),
        Error::Protocol(m) => (PROTOCOL, 0, m.as_bytes()),
        Error::Exception(c) => (EXCEPTION, i32::from(*c), b""),
        Error::ReadOnly => (READ_ONLY, 0, subject),
        Error::Point(m) => (POINT, 0, m.bytes()),
        Error::Io => (IO, 0, b""),
    };
    // SAFETY: an import of the host's; `msg` is valid for its length.
    unsafe { fail(kind, code, msg.as_ptr(), msg.len()) }
}

// ---- the input buffer ----

const BUF: usize = 16 * 1024;

struct Input(UnsafeCell<[u8; BUF]>);

// SAFETY: a wasm32 guest has one thread.
unsafe impl Sync for Input {}

static INPUT: Input = Input(UnsafeCell::new([0; BUF]));

/// Where the host puts a call's input; null (refused) past 16 KiB.
#[unsafe(no_mangle)]
pub extern "C" fn buffer(len: usize) -> *mut u8 {
    if len > BUF {
        core::ptr::null_mut()
    } else {
        INPUT.0.get().cast()
    }
}

fn input(ptr: *const u8, len: usize) -> &'static [u8] {
    if ptr != INPUT.0.get().cast_const().cast() || len > BUF {
        return &[];
    }
    // SAFETY: the host wrote `len` bytes at `ptr`, which `buffer` gave it.
    unsafe { core::slice::from_raw_parts(ptr, len) }
}

// ---- points (App §4.4: 0-based PDU addresses) ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Table {
    Coil,
    Discrete,
    Holding,
    Input,
}

#[derive(Clone, Copy)]
enum Format {
    Bit,
    U16,
    I16,
    U32(bool),
    I32(bool),
    F32(bool),
}

impl Format {
    const fn words(self) -> u16 {
        match self {
            Self::Bit | Self::U16 | Self::I16 => 1,
            _ => 2,
        }
    }

    const fn ty(self) -> i32 {
        match self {
            Self::Bit => BOOL,
            Self::F32(_) => REAL,
            _ => INT,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Bit => "bit",
            Self::U16 => "u16",
            Self::I16 => "i16",
            Self::U32(_) => "u32",
            Self::I32(_) => "i32",
            Self::F32(_) => "f32",
        }
    }
}

#[derive(Clone, Copy)]
struct Point {
    table: Table,
    addr: u16,
    format: Format,
}

impl Point {
    /// `table:addr[:format]`, exactly as the native driver takes it.
    fn parse(s: &[u8]) -> Option<Self> {
        let mut parts = s.split(|&b| b == b':');
        let table = match parts.next()? {
            b"coil" => Table::Coil,
            b"di" => Table::Discrete,
            b"hr" => Table::Holding,
            b"ir" => Table::Input,
            _ => return None,
        };
        let digits = parts.next()?;
        if digits.is_empty() {
            return None;
        }
        let mut addr: u32 = 0;
        for &b in digits {
            if !b.is_ascii_digit() {
                return None;
            }
            addr = addr * 10 + u32::from(b - b'0');
            if addr > u32::from(u16::MAX) {
                return None;
            }
        }
        let addr = u16::try_from(addr).ok()?;
        let bits = matches!(table, Table::Coil | Table::Discrete);
        let format = match (bits, parts.next()) {
            (true, None) => Format::Bit,
            (true, Some(_)) => return None,
            (false, None | Some(b"u16")) => Format::U16,
            (false, Some(b"i16")) => Format::I16,
            (false, Some(b"u32")) => Format::U32(false),
            (false, Some(b"u32sw")) => Format::U32(true),
            (false, Some(b"i32")) => Format::I32(false),
            (false, Some(b"i32sw")) => Format::I32(true),
            (false, Some(b"f32")) => Format::F32(false),
            (false, Some(b"f32sw")) => Format::F32(true),
            _ => return None,
        };
        if parts.next().is_some() || addr.checked_add(format.words() - 1).is_none() {
            return None;
        }
        Some(Self { table, addr, format })
    }

    const fn writable(self) -> bool {
        matches!(self.table, Table::Coil | Table::Holding)
    }
}

// ---- the transport (TCP §3.1.3) ----

static TID: AtomicU16 = AtomicU16::new(0);
static UNIT: AtomicU8 = AtomicU8::new(1);

/// Sends one request PDU, returns the reply PDU's length in `reply`. A
/// header that doesn't match drops the connection, so a late reply is
/// never taken for the next answer.
fn transact(pdu: &[u8], reply: &mut [u8; MAX_PDU]) -> Result<usize, Error> {
    let tid = TID.load(Relaxed).wrapping_add(1);
    TID.store(tid, Relaxed);
    let unit = UNIT.load(Relaxed);
    let n = pdu.len();
    let mut frame = [0u8; MBAP_LEN + MAX_PDU];
    frame[0..2].copy_from_slice(&tid.to_be_bytes());
    frame[4..6].copy_from_slice(
        &u16::try_from(n + 1)
            .map_err(|_| Error::Protocol("request too long"))?
            .to_be_bytes(),
    );
    frame[6] = unit;
    frame[MBAP_LEN..MBAP_LEN + n].copy_from_slice(pdu);
    // SAFETY: host imports; each pointer is valid for its length.
    if unsafe { send(frame.as_ptr(), MBAP_LEN + n) } != 0 {
        return Err(Error::Io);
    }
    let mut head = [0u8; MBAP_LEN];
    if unsafe { recv(head.as_mut_ptr(), MBAP_LEN) } != 0 {
        return Err(Error::Io);
    }
    let got = u16::from_be_bytes([head[0], head[1]]);
    let proto = u16::from_be_bytes([head[2], head[3]]);
    let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
    let bad = if proto != 0 {
        Some("protocol id")
    } else if got != tid {
        Some("transaction id")
    } else if !(2..=MAX_PDU + 1).contains(&len) {
        // Length counts the unit id plus a PDU of 1..=253 bytes.
        Some("length field")
    } else if head[6] != unit {
        Some("unit id")
    } else {
        None
    };
    if let Some(why) = bad {
        unsafe { close() };
        return Err(Error::Protocol(why));
    }
    if unsafe { recv(reply.as_mut_ptr(), len - 1) } != 0 {
        return Err(Error::Io);
    }
    Ok(len - 1)
}

/// Strips the function code; an exception reply (App §7 p.47) is
/// [`Error::Exception`].
fn body(fc: u8, pdu: &[u8]) -> Result<&[u8], Error> {
    match pdu {
        [] => Err(Error::Protocol("empty reply")),
        [f, code] if *f == fc | 0x80 => Err(Error::Exception(*code)),
        [f, ..] if *f == fc | 0x80 => Err(Error::Protocol("malformed exception reply")),
        [f, rest @ ..] if *f == fc => Ok(rest),
        _ => Err(Error::Protocol("reply to another function")),
    }
}

fn request(fc: u8, addr: u16, word: u16) -> [u8; 5] {
    let [a, b] = addr.to_be_bytes();
    let [c, d] = word.to_be_bytes();
    [fc, a, b, c, d]
}

/// FC 0x01/0x02, one bit (App §6.1 p.11).
fn read_bit(fc: u8, addr: u16) -> Result<bool, Error> {
    let mut r = [0u8; MAX_PDU];
    let n = transact(&request(fc, addr, 1), &mut r)?;
    match body(fc, &r[..n])? {
        [1, d] => Ok(d & 1 == 1),
        _ => Err(Error::Protocol("bit reply length")),
    }
}

/// FC 0x03/0x04, one or two registers (App §6.3 p.15).
fn read_registers(fc: u8, addr: u16, qty: u16) -> Result<[u16; 2], Error> {
    let mut r = [0u8; MAX_PDU];
    let n = transact(&request(fc, addr, qty), &mut r)?;
    let b = body(fc, &r[..n])?;
    let want = usize::from(qty) * 2;
    match b {
        [count, data @ ..] if usize::from(*count) == want && data.len() == want => {
            let mut regs = [0u16; 2];
            for (slot, c) in regs.iter_mut().zip(data.as_chunks::<2>().0) {
                *slot = u16::from_be_bytes([c[0], c[1]]);
            }
            Ok(regs)
        }
        _ => Err(Error::Protocol("register reply length")),
    }
}

/// Sends `req` and checks that the reply echoes `req[1..echo]` (App
/// §6.5 p.17, §6.6 p.19, §6.12 p.30).
fn write_echoed(req: &[u8], echo: usize) -> Result<(), Error> {
    let mut r = [0u8; MAX_PDU];
    let n = transact(req, &mut r)?;
    if body(req[0], &r[..n])? == &req[1..echo] {
        Ok(())
    } else {
        Err(Error::Protocol("write reply does not echo the request"))
    }
}

// ---- values ----

fn pair(regs: [u16; 2], swapped: bool) -> u32 {
    let (hi, lo) = if swapped {
        (regs[1], regs[0])
    } else {
        (regs[0], regs[1])
    };
    (u32::from(hi) << 16) | u32::from(lo)
}

/// The value read, as `(ty, bits)`.
fn decode(f: Format, regs: [u16; 2]) -> (i32, i64) {
    match f {
        Format::Bit => (BOOL, i64::from(regs[0] != 0)),
        Format::U16 => (INT, i64::from(regs[0])),
        Format::I16 => (INT, i64::from(regs[0].cast_signed())),
        Format::U32(sw) => (INT, i64::from(pair(regs, sw))),
        Format::I32(sw) => (INT, i64::from(pair(regs, sw).cast_signed())),
        Format::F32(sw) => (
            REAL,
            f64::from(f32::from_bits(pair(regs, sw))).to_bits().cast_signed(),
        ),
    }
}

fn split(x: u32, swapped: bool) -> [u16; 2] {
    let (hi, lo) = ((x >> 16) as u16, (x & 0xFFFF) as u16);
    if swapped { [lo, hi] } else { [hi, lo] }
}

/// Registers to write, in wire order, and how many.
fn encode(f: Format, ty: i32, bits: i64) -> Result<([u16; 2], usize), Error> {
    let range = || {
        let mut m = Msg { buf: [0; 64], len: 0 };
        let _ = write!(m, "{bits} does not fit {}", f.name());
        Error::Point(m)
    };
    let one = |x: u16| ([x, 0], 1);
    match (f, ty) {
        (Format::Bit, BOOL) => Ok(one(u16::from(bits == 1))),
        (Format::U16, INT) => u16::try_from(bits).map(one).map_err(|_| range()),
        (Format::I16, INT) => i16::try_from(bits)
            .map(|x| one(x.cast_unsigned()))
            .map_err(|_| range()),
        (Format::U32(sw), INT) => u32::try_from(bits)
            .map(|x| (split(x, sw), 2))
            .map_err(|_| range()),
        (Format::I32(sw), INT) => i32::try_from(bits)
            .map(|x| (split(x.cast_unsigned(), sw), 2))
            .map_err(|_| range()),
        #[allow(clippy::cast_possible_truncation)] // f32 is the wire format.
        (Format::F32(sw), REAL) => Ok((
            split((f64::from_bits(bits.cast_unsigned()) as f32).to_bits(), sw),
            2,
        )),
        (f, _) => Err(Error::Type(f.ty())),
    }
}

// ---- the exports ----

#[unsafe(no_mangle)]
pub extern "C" fn parse(ptr: *const u8, len: usize) {
    let a = input(ptr, len);
    match Point::parse(a) {
        // SAFETY: a host import.
        Some(p) => unsafe { point(p.format.ty(), i32::from(p.writable())) },
        None => report(&Error::Address, a),
    }
}

/// The next `u16` little-endian length and address in a read's input.
fn entry(rest: &[u8]) -> Option<(&[u8], &[u8])> {
    let (n, tail) = rest.split_first_chunk::<2>()?;
    let n = usize::from(u16::from_le_bytes(*n));
    (tail.len() >= n).then(|| tail.split_at(n))
}

fn read_point(p: Point) -> Result<(i32, i64), Error> {
    match p.table {
        Table::Coil | Table::Discrete => {
            let fc = if p.table == Table::Coil {
                READ_COILS
            } else {
                READ_DISCRETE_INPUTS
            };
            read_bit(fc, p.addr).map(|b| (BOOL, i64::from(b)))
        }
        Table::Holding | Table::Input => {
            let fc = if p.table == Table::Holding {
                READ_HOLDING_REGISTERS
            } else {
                READ_INPUT_REGISTERS
            };
            read_registers(fc, p.addr, p.format.words()).map(|regs| decode(p.format, regs))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn read(ptr: *const u8, len: usize) {
    let all = input(ptr, len);
    // Every address parses before the device is asked anything.
    let mut rest = all;
    while !rest.is_empty() {
        let Some((a, tail)) = entry(rest) else {
            return report(&Error::Address, b"");
        };
        if Point::parse(a).is_none() {
            return report(&Error::Address, a);
        }
        rest = tail;
    }
    let mut rest = all;
    while let Some((a, tail)) = entry(rest) {
        let p = Point::parse(a).expect("parsed above");
        match read_point(p) {
            // SAFETY: a host import. A later failure discards these.
            Ok((ty, bits)) => unsafe { value(ty, bits) },
            Err(e) => return report(&e, a),
        }
        rest = tail;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn write(ptr: *const u8, len: usize, ty: i32, bits: i64) {
    let a = input(ptr, len);
    let r = (|| {
        let p = Point::parse(a).ok_or(Error::Address)?;
        match p.table {
            Table::Discrete | Table::Input => Err(Error::ReadOnly),
            Table::Coil if ty != BOOL => Err(Error::Type(BOOL)),
            Table::Coil => {
                let on = if bits == 1 { 0xFF00 } else { 0x0000 };
                write_echoed(&request(WRITE_SINGLE_COIL, p.addr, on), 5)
            }
            Table::Holding => match encode(p.format, ty, bits)? {
                ([v, _], 1) => write_echoed(&request(WRITE_SINGLE_REGISTER, p.addr, v), 5),
                ([hi, lo], _) => {
                    let [a0, a1] = p.addr.to_be_bytes();
                    let [h0, h1] = hi.to_be_bytes();
                    let [l0, l1] = lo.to_be_bytes();
                    write_echoed(&[WRITE_MULTIPLE_REGISTERS, a0, a1, 0, 2, 4, h0, h1, l0, l1], 5)
                }
            },
        }
    })();
    if let Err(e) = r {
        report(&e, a);
    }
}

/// `unit=N` (1 by default), as the native driver reads it: an optional
/// `+` and the digits of a `u8`.
fn parse_u8(s: &[u8]) -> Option<u8> {
    let digits = s.strip_prefix(b"+").unwrap_or(s);
    if digits.is_empty() {
        return None;
    }
    let mut n: u16 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n * 10 + u16::from(b - b'0');
        if n > 255 {
            return None;
        }
    }
    u8::try_from(n).ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn configure(ptr: *const u8, len: usize) {
    for line in input(ptr, len).split(|&b| b == b'\n') {
        if let Some(v) = line.strip_prefix(b"unit=") {
            match parse_u8(v) {
                Some(u) => UNIT.store(u, Relaxed),
                None => return report(&Error::Address, b"unit"),
            }
        }
    }
}
