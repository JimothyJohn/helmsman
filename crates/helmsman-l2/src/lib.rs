//! Raw Ethernet frames of one `EtherType` on one interface.
//!
//! - **macOS**: a BPF device (`/dev/bpfN`) bound to the interface, with a
//!   filter that passes the `EtherType` (plain or behind one 802.1Q tag),
//!   immediate delivery, complete headers on send (the caller writes the
//!   source address) and no echo of what it sends. `bpf(4)`; ioctl
//!   numbers and `struct bpf_hdr` from the SDK's `net/bpf.h`, checked by
//!   compiling them (docs/SOURCES.md).
//! - **Linux**: an `AF_PACKET` / `SOCK_RAW` socket bound to the
//!   interface and the `EtherType`, `packet(7)`.
//!
//! Both need a privilege: read and write access to `/dev/bpf*` on macOS
//! (root, or the `access_bpf` group once the devices are opened to it),
//! `CAP_NET_RAW` on Linux. Opening without it is a
//! `PermissionDenied` whose text says which.
//!
//! [`parse_bpf`] (the walk over a BPF read buffer) and [`wanted`] are
//! plain functions, tested without a device.

use std::io;
use std::time::Duration;

/// A MAC address.
pub type Mac = [u8; 6];

const ETHERTYPE_VLAN: u16 = 0x8100;

/// Whether `frame` carries `ethertype`, plain or behind one 802.1Q tag,
/// and was not sent by `own` (a packet socket hands a sender its own
/// frames back).
#[must_use]
pub fn wanted(frame: &[u8], ethertype: u16, own: Mac) -> bool {
    let word = |at: usize| frame.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    if frame.get(6..12) == Some(&own[..]) {
        return false;
    }
    match word(12) {
        Some(t) if t == ethertype => true,
        Some(ETHERTYPE_VLAN) => word(16) == Some(ethertype),
        _ => false,
    }
}

/// The frames in one BPF read: each is a `bpf_hdr` (a 32-bit timeval,
/// the captured length at byte 8, the header's own length at byte 16),
/// the frame, and padding to a 4-byte boundary. A record that runs past
/// the buffer ends the walk.
#[must_use]
pub fn parse_bpf(buf: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut at = 0usize;
    while let Some(header) = buf.get(at..at + 18) {
        let caplen = u32::from_ne_bytes([header[8], header[9], header[10], header[11]]) as usize;
        let hdrlen = usize::from(u16::from_ne_bytes([header[16], header[17]]));
        if hdrlen < 18 {
            break;
        }
        let Some(frame) = buf.get(at + hdrlen..at + hdrlen + caplen) else {
            break;
        };
        frames.push(frame);
        // BPF_WORDALIGN.
        at += (hdrlen + caplen + 3) & !3;
    }
    frames
}

/// One instruction of a classic BPF program (`struct bpf_insn`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BpfInsn {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

/// `BPF_LD | BPF_H | BPF_ABS`: load the 16-bit word at offset `k`.
pub const BPF_LOAD_HALF: u16 = 0x28;
/// `BPF_JMP | BPF_JEQ | BPF_K`: skip `jt` instructions if the word
/// equals `k`, else `jf`.
pub const BPF_JUMP_EQUAL: u16 = 0x15;
/// `BPF_RET | BPF_K`: accept `k` bytes of the frame (0 drops it).
pub const BPF_RETURN: u16 = 0x06;

/// The filter a BPF device is given: accept a frame whose type at byte
/// 12 is `ethertype`, or 0x8100 there and `ethertype` at byte 16; drop
/// everything else.
#[must_use]
pub fn bpf_filter(ethertype: u16) -> [BpfInsn; 7] {
    let insn = |code, jt, jf, k| BpfInsn { code, jt, jf, k };
    let ethertype = u32::from(ethertype);
    [
        insn(BPF_LOAD_HALF, 0, 0, 12),
        insn(BPF_JUMP_EQUAL, 3, 0, ethertype),
        insn(BPF_JUMP_EQUAL, 0, 3, u32::from(ETHERTYPE_VLAN)),
        insn(BPF_LOAD_HALF, 0, 0, 16),
        insn(BPF_JUMP_EQUAL, 0, 1, ethertype),
        insn(BPF_RETURN, 0, 0, 0xFFFF),
        insn(BPF_RETURN, 0, 0, 0),
    ]
}

#[cfg(unix)]
mod sys;

#[cfg(unix)]
pub use sys::{RawSocket, addresses};

/// What an interface is on the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Addresses {
    pub mac: Mac,
    /// Its first IPv4 address, if it has one.
    pub ipv4: Option<[u8; 4]>,
}

/// How long a `recv` waits when the caller gives no time at all: a
/// timeout of zero would mean "for ever" to both kernels.
const SHORTEST_WAIT: Duration = Duration::from_millis(1);

fn timeval(d: Duration) -> (i64, i64) {
    let d = d.max(SHORTEST_WAIT);
    (
        i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        i64::from(d.subsec_micros()),
    )
}

fn permission(e: io::Error, what: &str) -> io::Error {
    if e.kind() == io::ErrorKind::PermissionDenied {
        io::Error::new(io::ErrorKind::PermissionDenied, format!("{what}: {e}"))
    } else {
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: Mac = [2, 0, 0, 0, 0, 1];

    fn frame(src: Mac, tail: &[u8]) -> Vec<u8> {
        let mut f = vec![0xFF; 6];
        f.extend_from_slice(&src);
        f.extend_from_slice(tail);
        f
    }

    #[test]
    fn the_ethertype_is_wanted_plain_or_tagged_and_never_from_ourselves() {
        let other = [2, 0, 0, 0, 0, 2];
        assert!(wanted(&frame(other, &[0x88, 0x92, 1, 2]), 0x8892, OWN));
        assert!(wanted(
            &frame(other, &[0x81, 0x00, 0xC0, 0x00, 0x88, 0x92]),
            0x8892,
            OWN
        ));
        assert!(!wanted(&frame(other, &[0x08, 0x00]), 0x8892, OWN));
        assert!(!wanted(
            &frame(other, &[0x81, 0x00, 0xC0, 0x00, 0x08, 0x00]),
            0x8892,
            OWN
        ));
        assert!(!wanted(&frame(OWN, &[0x88, 0x92]), 0x8892, OWN));
        assert!(!wanted(&[0; 13], 0x8892, OWN));
        assert!(!wanted(&[], 0x8892, OWN));
    }

    /// One BPF record: an 18-byte header padded to 20, then the frame.
    fn record(frame: &[u8]) -> Vec<u8> {
        let mut r = vec![0u8; 8];
        let len = u32::try_from(frame.len()).unwrap();
        r.extend_from_slice(&len.to_ne_bytes());
        r.extend_from_slice(&len.to_ne_bytes());
        r.extend_from_slice(&20u16.to_ne_bytes());
        r.extend_from_slice(&[0, 0]);
        r.extend_from_slice(frame);
        while !r.len().is_multiple_of(4) {
            r.push(0);
        }
        r
    }

    #[test]
    fn a_bpf_buffer_holds_several_frames_each_word_aligned() {
        let a = [1u8; 61];
        let b = [2u8; 64];
        let c = [3u8; 3];
        let mut buf = record(&a);
        buf.extend(record(&b));
        buf.extend(record(&c));
        let frames = parse_bpf(&buf);
        assert_eq!(frames, [&a[..], &b[..], &c[..]]);
    }

    #[test]
    fn a_truncated_or_corrupt_bpf_buffer_ends_the_walk_without_a_panic() {
        let mut buf = record(&[9u8; 60]);
        let whole = buf.len();
        buf.extend(record(&[8u8; 60]));
        assert_eq!(parse_bpf(&buf[..whole + 30]).len(), 1);
        assert_eq!(parse_bpf(&buf[..10]).len(), 0);
        // A header length of 0 would loop for ever.
        let mut bad = record(&[7u8; 8]);
        bad[16] = 0;
        bad[17] = 0;
        assert_eq!(parse_bpf(&bad), Vec::<&[u8]>::new());
        // A captured length past the buffer.
        let mut long = record(&[7u8; 8]);
        long[8..12].copy_from_slice(&1000u32.to_ne_bytes());
        assert_eq!(parse_bpf(&long), Vec::<&[u8]>::new());
    }

    /// Runs a filter the way the kernel does, for the three
    /// instructions [`bpf_filter`] uses: how many bytes it accepts.
    fn run_filter(program: &[BpfInsn], frame: &[u8]) -> u32 {
        let (mut pc, mut acc) = (0usize, 0u32);
        loop {
            let i = program[pc];
            pc += 1;
            match i.code {
                BPF_LOAD_HALF => {
                    let at = i.k as usize;
                    // A load past the frame drops it, as in the kernel.
                    let Some(b) = frame.get(at..at + 2) else { return 0 };
                    acc = u32::from(u16::from_be_bytes([b[0], b[1]]));
                }
                BPF_JUMP_EQUAL => pc += usize::from(if acc == i.k { i.jt } else { i.jf }),
                BPF_RETURN => return i.k,
                other => panic!("instruction {other:#x} is not one the filter uses"),
            }
        }
    }

    #[test]
    fn the_bpf_filter_agrees_with_wanted_on_every_shape_of_frame() {
        let other = [2, 0, 0, 0, 0, 2];
        let program = bpf_filter(0x8892);
        let tails: [&[u8]; 8] = [
            &[0x88, 0x92, 1, 2],
            &[0x81, 0x00, 0xC0, 0x00, 0x88, 0x92, 9],
            &[0x08, 0x00, 0x88, 0x92],
            &[0x81, 0x00, 0xC0, 0x00, 0x08, 0x00],
            &[0x81, 0x00, 0xC0],
            &[0x88],
            &[],
            &[0x88, 0x93],
        ];
        for tail in tails {
            let f = frame(other, tail);
            assert_eq!(
                run_filter(&program, &f) != 0,
                wanted(&f, 0x8892, OWN),
                "{tail:02x?}"
            );
        }
    }

    #[test]
    fn a_zero_timeout_becomes_the_shortest_wait() {
        assert_eq!(timeval(Duration::ZERO), (0, 1000));
        assert_eq!(timeval(Duration::from_millis(1500)), (1, 500_000));
    }
}
