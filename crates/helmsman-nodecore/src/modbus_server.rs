//! The Skipper's own Modbus TCP server, with no socket in it: what a
//! request frame asks and what the reply is, over the node's shared
//! values (the map's `share` lines, [`crate::iomap::Share`]). The firmware
//! (crates/helmsman-node-p4, `share_server.rs`) accepts the connections, cuts
//! the byte stream into frames with [`head`] and calls [`serve`]; docs/EDGE.md
//! "Sharing values" is the user's side of this.
//!
//! Modbus Application Protocol V1.1b3 and the TCP guide are the references
//! (docs/SOURCES.md); the frame and PDU limits are `helmsman-modbus`'s.
//!
//! # What is served
//!
//! - **Reads**, every shared address: FC 1 (coils) and 2 (discrete inputs)
//!   for bool shares, FC 3 (holding) and 4 (input registers) for int and
//!   real shares. A request reaches only addresses some share holds; an
//!   address no share holds answers exception 02 for the whole request.
//! - **Writes**, FC 5 and 15 (coils), FC 6 and 16 (holding registers), only
//!   where the share is a `VAR_INPUT` of the running program (the other
//!   controller drives that input) and in the `coil` or `hr` table. Any
//!   other address, shared or not, answers exception 02 (illegal data
//!   address) and nothing is written: a request is all or nothing.
//! - A write must cover each 32-bit share it touches whole (FC 16 with both
//!   registers; FC 6 cannot write one): a part of a value answers
//!   exception 03, as does a REAL written as NaN or infinity.
//! - Any other function code answers exception 01. Quantities outside the
//!   spec's limits and a PDU of the wrong length answer exception 03.
//! - The unit id is not checked: any unit answers, and the reply carries
//!   the request's.
//!
//! A frame that is not Modbus TCP (protocol id other than 0, a length
//! outside 2 to 254) is [`Head::Refuse`]d: the connection is closed, as a
//! peer that cannot frame cannot be answered.
//!
//! [`serve`] works on a copy of the bank and returns the writes it
//! approved; [`ShareBank::apply`] commits them. That keeps the critical
//! section the scan loop and this server share down to a copy and a few
//! stores.

use alloc::vec;
use alloc::vec::Vec;

use helmsman_modbus::{
    COIL_OFF, COIL_ON, ILLEGAL_DATA_ADDRESS, ILLEGAL_DATA_VALUE, ILLEGAL_FUNCTION, MAX_PDU, MAX_READ_BITS,
    MAX_READ_REGS, MAX_WRITE_COILS, MAX_WRITE_REGS, MBAP_LEN, READ_COILS, READ_DISCRETE_INPUTS,
    READ_HOLDING_REGISTERS, READ_INPUT_REGISTERS, WRITE_MULTIPLE_COILS, WRITE_MULTIPLE_REGISTERS,
    WRITE_SINGLE_COIL, WRITE_SINGLE_REGISTER, mbap,
};

use crate::iomap::{Fmt, SHARE_BYTES, Share, Table};
use crate::ioplan::{bit_of, set_bit};

/// The port the server listens on: Modbus TCP's.
pub const PORT: u16 = 502;
/// The longest frame: the header and a PDU of 253 bytes.
pub const MAX_FRAME: usize = MBAP_LEN + MAX_PDU;

/// What the bytes read so far of a connection amount to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    /// Not a whole frame yet: this many bytes in all are wanted.
    Need(usize),
    /// A whole frame of this many bytes starts the buffer.
    Frame(usize),
    /// Not a Modbus TCP frame; close the connection.
    Refuse(&'static str),
}

/// Reads the MBAP header at the start of `buf`: how long the first frame
/// is, or that the stream is not Modbus TCP. The length is judged as soon
/// as the header is in, so a hostile length never makes the caller wait
/// for, or buffer, more than [`MAX_FRAME`] bytes.
#[must_use]
pub fn head(buf: &[u8]) -> Head {
    let Some(h) = buf.get(..MBAP_LEN) else {
        return Head::Need(MBAP_LEN);
    };
    if h[2] != 0 || h[3] != 0 {
        return Head::Refuse("protocol id is not 0");
    }
    let len = usize::from(u16::from_be_bytes([h[4], h[5]]));
    if !(2..=MAX_PDU + 1).contains(&len) {
        return Head::Refuse("length field outside 2 to 254");
    }
    let total = 6 + len;
    if buf.len() >= total {
        Head::Frame(total)
    } else {
        Head::Need(total)
    }
}

/// The shared values as the server and the scan loop hold them: the
/// share bank (the layout of [`crate::iomap::Share::at`]) and which shares
/// another controller may write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareBank {
    pub image: [u8; SHARE_BYTES],
    /// Bit `k` set: share `k` (in map order) is a `VAR_INPUT` of the
    /// running program in a `coil` or `hr` table, so it may be written.
    /// Set by the scan loop whenever a program is loaded.
    pub writable: u32,
    /// Bit `k` set: share `k` has a variable of its name in the running
    /// program. Shown on `GET /api/io_map`; the server does not use it.
    pub bound: u32,
}

impl Default for ShareBank {
    fn default() -> Self {
        Self {
            image: [0; SHARE_BYTES],
            writable: 0,
            bound: 0,
        }
    }
}

/// One approved write, for [`ShareBank::apply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareWrite {
    Bit {
        share: usize,
        on: bool,
    },
    /// The slot's bytes, little-endian as the bank keeps them.
    Slot {
        share: usize,
        raw: [u8; 4],
    },
}

/// What [`serve`] makes of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The whole reply frame, header included.
    pub frame: Vec<u8>,
    /// What the request wrote, if it was accepted.
    pub writes: Vec<ShareWrite>,
}

impl ShareBank {
    fn may_write(&self, share: usize) -> bool {
        share < 32 && self.writable >> share & 1 == 1
    }

    /// Commits writes [`serve`] approved.
    pub fn apply(&mut self, shares: &[Share], writes: &[ShareWrite]) {
        for w in writes {
            match *w {
                ShareWrite::Bit { share, on } => {
                    if let Some(s) = shares.get(share) {
                        set_bit(&mut self.image, s.at, on);
                    }
                }
                ShareWrite::Slot { share, raw } => {
                    let Some(s) = shares.get(share) else { continue };
                    let from = usize::from(s.at.byte);
                    let n = usize::from(s.at.bytes);
                    if let Some(dst) = self.image.get_mut(from..from + n) {
                        dst.copy_from_slice(&raw[..n]);
                    }
                }
            }
        }
    }

    /// [`serve`] and [`ShareBank::apply`] in one, for a caller that holds
    /// the bank whole (the host tests). `None` where [`serve`] says so.
    pub fn handle(&mut self, shares: &[Share], request: &[u8]) -> Option<Vec<u8>> {
        let reply = serve(shares, self, request)?;
        self.apply(shares, &reply.writes);
        Some(reply.frame)
    }
}

/// Answers one whole request frame (as [`head`] cut it). `None` if it is
/// not a frame after all (a bad header, or a length that is not the
/// bytes given): the caller closes the connection.
#[must_use]
pub fn serve(shares: &[Share], bank: &ShareBank, request: &[u8]) -> Option<Reply> {
    let Head::Frame(total) = head(request) else {
        return None;
    };
    if total != request.len() {
        return None;
    }
    let (tid, unit) = (u16::from_be_bytes([request[0], request[1]]), request[6]);
    let pdu = &request[MBAP_LEN..];
    let fc = pdu[0];
    let (body, writes) = match dispatch(shares, bank, fc, pdu) {
        Ok(done) => done,
        Err(code) => (vec![fc | 0x80, code], Vec::new()),
    };
    let frame = mbap::frame(tid, unit, &body).ok()?;
    Some(Reply { frame, writes })
}

type Done = (Vec<u8>, Vec<ShareWrite>);

fn word(pdu: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([pdu[at], pdu[at + 1]])
}

fn dispatch(shares: &[Share], bank: &ShareBank, fc: u8, pdu: &[u8]) -> Result<Done, u8> {
    match fc {
        READ_COILS | READ_DISCRETE_INPUTS | READ_HOLDING_REGISTERS | READ_INPUT_REGISTERS => {
            if pdu.len() != 5 {
                return Err(ILLEGAL_DATA_VALUE);
            }
            let (start, qty) = (word(pdu, 1), word(pdu, 3));
            let table = match fc {
                READ_COILS => Table::Coil,
                READ_DISCRETE_INPUTS => Table::Discrete,
                READ_HOLDING_REGISTERS => Table::Holding,
                _ => Table::Input,
            };
            let body = if fc <= READ_DISCRETE_INPUTS {
                read_bits(shares, bank, table, start, qty)?
            } else {
                read_regs(shares, bank, table, start, qty)?
            };
            let mut reply = vec![fc];
            reply.extend(body);
            Ok((reply, Vec::new()))
        }
        WRITE_SINGLE_COIL => {
            if pdu.len() != 5 {
                return Err(ILLEGAL_DATA_VALUE);
            }
            let on = match word(pdu, 3) {
                COIL_ON => true,
                COIL_OFF => false,
                _ => return Err(ILLEGAL_DATA_VALUE),
            };
            let writes = write_coils(shares, bank, word(pdu, 1), &[on])?;
            Ok((pdu.to_vec(), writes))
        }
        WRITE_SINGLE_REGISTER => {
            if pdu.len() != 5 {
                return Err(ILLEGAL_DATA_VALUE);
            }
            let writes = write_regs(shares, bank, word(pdu, 1), &[word(pdu, 3)])?;
            Ok((pdu.to_vec(), writes))
        }
        WRITE_MULTIPLE_COILS => {
            if pdu.len() < 6 {
                return Err(ILLEGAL_DATA_VALUE);
            }
            let (start, qty, count) = (word(pdu, 1), usize::from(word(pdu, 3)), usize::from(pdu[5]));
            if !(1..=usize::from(MAX_WRITE_COILS)).contains(&qty)
                || count != qty.div_ceil(8)
                || pdu.len() != 6 + count
            {
                return Err(ILLEGAL_DATA_VALUE);
            }
            let bits: Vec<bool> = (0..qty).map(|i| pdu[6 + i / 8] >> (i % 8) & 1 == 1).collect();
            let writes = write_coils(shares, bank, start, &bits)?;
            Ok((pdu[..5].to_vec(), writes))
        }
        WRITE_MULTIPLE_REGISTERS => {
            if pdu.len() < 6 {
                return Err(ILLEGAL_DATA_VALUE);
            }
            let (start, qty, count) = (word(pdu, 1), usize::from(word(pdu, 3)), usize::from(pdu[5]));
            if !(1..=usize::from(MAX_WRITE_REGS)).contains(&qty) || count != qty * 2 || pdu.len() != 6 + count
            {
                return Err(ILLEGAL_DATA_VALUE);
            }
            let regs: Vec<u16> = (0..qty).map(|i| word(pdu, 6 + 2 * i)).collect();
            let writes = write_regs(shares, bank, start, &regs)?;
            Ok((pdu[..5].to_vec(), writes))
        }
        _ => Err(ILLEGAL_FUNCTION),
    }
}

/// `[byte count, data...]` of a bit read.
fn read_bits(shares: &[Share], bank: &ShareBank, table: Table, start: u16, qty: u16) -> Result<Vec<u8>, u8> {
    if !(1..=MAX_READ_BITS).contains(&qty) {
        return Err(ILLEGAL_DATA_VALUE);
    }
    let end = u32::from(start) + u32::from(qty);
    if end > 0x1_0000 {
        return Err(ILLEGAL_DATA_ADDRESS);
    }
    let mut out = vec![0u8; 1 + usize::from(qty).div_ceil(8)];
    out[0] = u8::try_from(out.len() - 1).unwrap_or(u8::MAX);
    let mut covered = 0u32;
    for s in shares.iter().filter(|s| s.table == table) {
        if (u32::from(start)..end).contains(&u32::from(s.addr)) {
            covered += 1;
            if bit_of(&bank.image, s.at) {
                let i = usize::from(s.addr - start);
                out[1 + i / 8] |= 1 << (i % 8);
            }
        }
    }
    if covered != u32::from(qty) {
        return Err(ILLEGAL_DATA_ADDRESS);
    }
    Ok(out)
}

/// A share's registers as they go on the wire: one, or two with the high
/// word first unless the format is `sw`.
fn wire_words(share: &Share, image: &[u8]) -> [u16; 2] {
    let from = usize::from(share.at.byte);
    let raw = image.get(from..from + usize::from(share.at.bytes)).unwrap_or(&[]);
    match *raw {
        [lo, hi] => [u16::from_le_bytes([lo, hi]), 0],
        [b0, b1, b2, b3] => {
            let v = u32::from_le_bytes([b0, b1, b2, b3]);
            let (hi, lo) = (
                u16::try_from(v >> 16).unwrap_or(0),
                u16::try_from(v & 0xFFFF).unwrap_or(0),
            );
            if share.swapped { [lo, hi] } else { [hi, lo] }
        }
        _ => [0, 0],
    }
}

/// `[byte count, data...]` of a register read.
fn read_regs(shares: &[Share], bank: &ShareBank, table: Table, start: u16, qty: u16) -> Result<Vec<u8>, u8> {
    if !(1..=MAX_READ_REGS).contains(&qty) {
        return Err(ILLEGAL_DATA_VALUE);
    }
    let end = u32::from(start) + u32::from(qty);
    if end > 0x1_0000 {
        return Err(ILLEGAL_DATA_ADDRESS);
    }
    let mut regs: Vec<Option<u16>> = vec![None; usize::from(qty)];
    for s in shares.iter().filter(|s| s.table == table) {
        let words = wire_words(s, &bank.image);
        for k in 0..u32::from(s.words()) {
            let a = u32::from(s.addr) + k;
            if (u32::from(start)..end).contains(&a)
                && let Some(slot) = regs.get_mut(usize::try_from(a - u32::from(start)).unwrap_or(0))
            {
                *slot = Some(words[usize::try_from(k).unwrap_or(0)]);
            }
        }
    }
    let mut out = Vec::with_capacity(1 + 2 * regs.len());
    out.push(u8::try_from(2 * regs.len()).unwrap_or(u8::MAX));
    for r in regs {
        out.extend_from_slice(&r.ok_or(ILLEGAL_DATA_ADDRESS)?.to_be_bytes());
    }
    Ok(out)
}

fn write_coils(
    shares: &[Share],
    bank: &ShareBank,
    start: u16,
    values: &[bool],
) -> Result<Vec<ShareWrite>, u8> {
    let end = u32::from(start) + u32::try_from(values.len()).unwrap_or(u32::MAX);
    if end > 0x1_0000 {
        return Err(ILLEGAL_DATA_ADDRESS);
    }
    let mut writes = Vec::new();
    for (k, s) in shares.iter().enumerate().filter(|(_, s)| s.table == Table::Coil) {
        if (u32::from(start)..end).contains(&u32::from(s.addr)) {
            if !bank.may_write(k) {
                return Err(ILLEGAL_DATA_ADDRESS);
            }
            writes.push(ShareWrite::Bit {
                share: k,
                on: values[usize::from(s.addr - start)],
            });
        }
    }
    // One share to an address, so every address was met once or is not shared.
    if writes.len() != values.len() {
        return Err(ILLEGAL_DATA_ADDRESS);
    }
    Ok(writes)
}

fn write_regs(shares: &[Share], bank: &ShareBank, start: u16, values: &[u16]) -> Result<Vec<ShareWrite>, u8> {
    let (first_addr, end) = (
        u32::from(start),
        u32::from(start) + u32::try_from(values.len()).unwrap_or(u32::MAX),
    );
    if end > 0x1_0000 {
        return Err(ILLEGAL_DATA_ADDRESS);
    }
    // Every address first (02), then the shape of what is written (03).
    let mut covered = 0usize;
    for (k, s) in shares
        .iter()
        .enumerate()
        .filter(|(_, s)| s.table == Table::Holding)
    {
        let (from, to) = (u32::from(s.addr), u32::from(s.addr) + u32::from(s.words()));
        if from < end && first_addr < to {
            if !bank.may_write(k) {
                return Err(ILLEGAL_DATA_ADDRESS);
            }
            covered += usize::try_from(to.min(end) - from.max(first_addr)).unwrap_or(0);
        }
    }
    if covered != values.len() {
        return Err(ILLEGAL_DATA_ADDRESS);
    }
    let mut writes = Vec::new();
    for (k, s) in shares
        .iter()
        .enumerate()
        .filter(|(_, s)| s.table == Table::Holding)
    {
        let (from, to) = (u32::from(s.addr), u32::from(s.addr) + u32::from(s.words()));
        if !(from < end && first_addr < to) {
            continue;
        }
        if from < first_addr || to > end {
            return Err(ILLEGAL_DATA_VALUE);
        }
        let first = usize::try_from(from - first_addr).unwrap_or(0);
        let mut raw = [0u8; 4];
        if s.words() == 2 {
            let (w0, w1) = (values[first], values[first + 1]);
            let (hi, lo) = if s.swapped { (w1, w0) } else { (w0, w1) };
            let v = (u32::from(hi) << 16) | u32::from(lo);
            if s.fmt == Some(Fmt::F32) && !f32::from_bits(v).is_finite() {
                return Err(ILLEGAL_DATA_VALUE);
            }
            raw = v.to_le_bytes();
        } else {
            raw[..2].copy_from_slice(&values[first].to_le_bytes());
        }
        writes.push(ShareWrite::Slot { share: k, raw });
    }
    Ok(writes)
}

#[cfg(test)]
mod tests;
