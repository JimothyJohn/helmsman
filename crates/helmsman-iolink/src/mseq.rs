//! M-sequences (IOL §A.1 and §A.2 p.238-247): a Master message and the
//! Device's reply to it. What the two carry is fixed by the M-sequence
//! type both sides agreed on for the current mode, held here as a
//! [`Layout`], and by the read/write bit of the message's first octet.

use core::fmt;

use crate::checksum;

/// The longest message: MC, CKT, 32 octets of Process Data and 32 of
/// On-request Data in a `TYPE_2_V` write (IOL §A.2.4 p.244-245).
pub const MAX_MESSAGE: usize = 66;

/// IOL Table A.1 p.238: the communication channel of an access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Process,
    Page,
    Diagnosis,
    Isdu,
}

/// The M-sequence control octet (IOL §A.1.2 p.238): read or write, the
/// channel, and the octet offset on it (flow control on the ISDU channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mc {
    read: bool,
    channel: Channel,
    address: u8,
}

impl Mc {
    /// `None` for an address that does not fit the octet's five bits.
    #[must_use]
    pub const fn new(read: bool, channel: Channel, address: u8) -> Option<Self> {
        if address > 0x1F {
            return None;
        }
        Some(Self {
            read,
            channel,
            address,
        })
    }

    /// A read of the Direct Parameter page at `address`.
    #[must_use]
    pub const fn read_page(address: u8) -> Option<Self> {
        Self::new(true, Channel::Page, address)
    }

    /// A write to the Direct Parameter page at `address`.
    #[must_use]
    pub const fn write_page(address: u8) -> Option<Self> {
        Self::new(false, Channel::Page, address)
    }

    /// IOL Table A.2: true is a read (Device to Master).
    #[must_use]
    pub const fn read(self) -> bool {
        self.read
    }

    #[must_use]
    pub const fn channel(self) -> Channel {
        self.channel
    }

    #[must_use]
    pub const fn address(self) -> u8 {
        self.address
    }

    /// IOL Figure A.1: bit 7 R/W, bits 6 to 5 the channel, 4 to 0 the
    /// address.
    #[must_use]
    pub const fn octet(self) -> u8 {
        let channel = match self.channel {
            Channel::Process => 0,
            Channel::Page => 1,
            Channel::Diagnosis => 2,
            Channel::Isdu => 3,
        };
        ((self.read as u8) << 7) | (channel << 5) | self.address
    }

    /// Every octet is a control octet (IOL §A.1.2 p.238: a Device answers
    /// the ones it does not implement with 0 and ignores writes to them).
    #[must_use]
    pub const fn from_octet(octet: u8) -> Self {
        let channel = match (octet >> 5) & 3 {
            0 => Channel::Process,
            1 => Channel::Page,
            2 => Channel::Diagnosis,
            _ => Channel::Isdu,
        };
        Self {
            read: octet & 0x80 != 0,
            channel,
            address: octet & 0x1F,
        }
    }
}

/// Why a message was not built or not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The message, or a part handed to an encoder, is not the length the
    /// layout gives it.
    Length { want: usize, got: usize },
    /// The checksum does not match the octets (IOL §A.1.6).
    Checksum { want: u8, got: u8 },
    /// The check/type octet names another M-sequence type (IOL Table A.3).
    Type { want: u8, got: u8 },
    /// The output buffer is shorter than the message.
    Buffer { want: usize, got: usize },
    /// The Device's Process Data needs the interleaved `TYPE_1_1` /
    /// `TYPE_1_2` mode (IOL Table A.10 NOTE 3), which is not built.
    Interleaved,
    /// No row of IOL Table A.8 or A.10 has this M-sequence code with these
    /// Process Data lengths (octets).
    Capability { code: u8, pd_in: u8, pd_out: u8 },
    /// A Process Data descriptor with a reserved length (IOL Table B.6).
    ProcessData { octet: u8 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Length { want, got } => write!(f, "{got} octets where the M-sequence type has {want}"),
            Self::Checksum { want, got } => {
                write!(f, "checksum 0x{got:02X}, the octets give 0x{want:02X}")
            }
            Self::Type { want, got } => write!(f, "M-sequence type {got} in a type {want} exchange"),
            Self::Buffer { want, got } => write!(f, "a {got}-octet buffer for a {want}-octet message"),
            Self::Interleaved => f.write_str(
                "the device needs interleaved Process Data (TYPE_1_1 / TYPE_1_2), which is not supported yet",
            ),
            Self::Capability { code, pd_in, pd_out } => write!(
                f,
                "no M-sequence type for code {code} with {pd_in} octets in and {pd_out} out"
            ),
            Self::ProcessData { octet } => write!(f, "reserved Process Data length 0x{octet:02X}"),
        }
    }
}

impl core::error::Error for Error {}

/// What one M-sequence type carries per cycle: octets of Process Data
/// each way and of On-request Data (one way, by the read/write bit).
///
/// The named types of IOL §A.2 are all one of three shapes on the wire:
/// `TYPE_0`; type 1 with 2, 8 or 32 octets (`TYPE_1_2`, `TYPE_1_V`, and
/// `TYPE_1_1`, which is the same message on the process channel); and
/// type 2 with Process Data beside the On-request Data (`TYPE_2_1` to
/// `TYPE_2_5` are `TYPE_2_V` with one or two octets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    mtype: u8,
    pd_out: u8,
    od: u8,
    pd_in: u8,
}

impl Layout {
    /// IOL §A.2.2 p.241: one octet of On-request Data, mandatory for
    /// every Device and the only type in STARTUP (Table A.7).
    pub const TYPE_0: Self = Self {
        mtype: 0,
        pd_out: 0,
        od: 1,
        pd_in: 0,
    };

    /// IOL §A.2.3 p.242-243: `od` octets read or written per cycle, 2
    /// (`TYPE_1_1`, `TYPE_1_2`), 8 or 32 (`TYPE_1_V`, Tables A.8 and A.10).
    #[must_use]
    pub const fn type1(od: u8) -> Option<Self> {
        if !matches!(od, 2 | 8 | 32) {
            return None;
        }
        Some(Self {
            mtype: 1,
            pd_out: 0,
            od,
            pd_in: 0,
        })
    }

    /// IOL §A.2.4 p.243-245: `pd_out` octets to the Device and `pd_in`
    /// from it every cycle (0 to 32 each, not both 0), beside 1, 2, 8 or
    /// 32 octets of On-request Data.
    #[must_use]
    pub const fn type2(pd_out: u8, od: u8, pd_in: u8) -> Option<Self> {
        if pd_out > 32 || pd_in > 32 || (pd_out == 0 && pd_in == 0) || !matches!(od, 1 | 2 | 8 | 32) {
            return None;
        }
        Some(Self {
            mtype: 2,
            pd_out,
            od,
            pd_in,
        })
    }

    /// The value of the check/type octet's two type bits (IOL Table A.3).
    #[must_use]
    pub const fn mtype(self) -> u8 {
        self.mtype
    }

    #[must_use]
    pub const fn pd_out(self) -> usize {
        self.pd_out as usize
    }

    #[must_use]
    pub const fn pd_in(self) -> usize {
        self.pd_in as usize
    }

    #[must_use]
    pub const fn od(self) -> usize {
        self.od as usize
    }

    /// Octets of the Master message: MC, CKT, the output Process Data,
    /// and the On-request Data when it is a write.
    #[must_use]
    pub const fn master_len(self, read: bool) -> usize {
        2 + self.pd_out() + if read { 0 } else { self.od() }
    }

    /// Octets of the Device reply: the On-request Data when it was a
    /// read, the input Process Data, CKS.
    #[must_use]
    pub const fn reply_len(self, read: bool) -> usize {
        (if read { self.od() } else { 0 }) + self.pd_in() + 1
    }

    /// The type a Device uses in PREOPERATE (IOL Table A.8 p.245), from
    /// the code in its M-sequence capability.
    ///
    /// # Errors
    /// [`Error::Capability`] for a code above 3.
    pub const fn preoperate(code: u8) -> Result<Self, Error> {
        let od = match code {
            0 => return Ok(Self::TYPE_0),
            1 => 2,
            2 => 8,
            3 => 32,
            _ => {
                return Err(Error::Capability {
                    code,
                    pd_in: 0,
                    pd_out: 0,
                });
            }
        };
        Ok(Self {
            mtype: 1,
            pd_out: 0,
            od,
            pd_in: 0,
        })
    }

    /// The type a revision 1.1 Device uses in OPERATE (IOL Table A.10
    /// p.247), from the code in its M-sequence capability and its Process
    /// Data lengths in octets on the wire (a length in bits rounded up).
    ///
    /// # Errors
    /// [`Error::Interleaved`] for the rows that need the interleaved
    /// mode, [`Error::Capability`] for a combination the table does not
    /// have.
    pub const fn operate(code: u8, pd_in: u8, pd_out: u8) -> Result<Self, Error> {
        let refused = Err(Error::Capability { code, pd_in, pd_out });
        if pd_in > 32 || pd_out > 32 {
            return refused;
        }
        let none = pd_in == 0 && pd_out == 0;
        // Table A.10 splits at 3 octets: below it the lengths are given in
        // bits (1 to 16), from it in octets (IOL Table B.6).
        let wide = pd_in > 2 || pd_out > 2;
        let od = match (code, none, wide) {
            (0, true, _) => return Ok(Self::TYPE_0),
            (1, true, _) => return Self::preoperate(1),
            (6, true, _) => return Self::preoperate(2),
            (7, true, _) => return Self::preoperate(3),
            (0, false, true) => return Err(Error::Interleaved),
            (0, false, false) | (4, false, true) => 1,
            (5, false, _) => 2,
            (6, false, _) => 8,
            (7, false, _) => 32,
            _ => return refused,
        };
        Ok(Self {
            mtype: 2,
            pd_out,
            od,
            pd_in,
        })
    }
}

/// A Master message, decoded (IOL §A.2, the "Master read message" and
/// "Master write message" rows of Figures A.5 to A.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterMessage<'a> {
    pub mc: Mc,
    /// The output Process Data, sent on reads and writes alike.
    pub pd_out: &'a [u8],
    /// The On-request Data of a write; empty in a read.
    pub od: &'a [u8],
}

/// A Device reply (IOL §A.2, the "Device reply message" rows, and the
/// two flags of the check/status octet, §A.1.5 p.239-240).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply<'a> {
    /// The On-request Data of a read; empty after a write.
    pub od: &'a [u8],
    pub pd_in: &'a [u8],
    /// IOL Table A.5: false makes the whole cycle's input Process Data
    /// invalid.
    pub pd_valid: bool,
    /// IOL Table A.6: the Device has an Event for the Master to fetch.
    pub event: bool,
}

const CKS_PD_INVALID: u8 = 0x40;
const CKS_EVENT: u8 = 0x80;

fn part(want: usize, got: &[u8]) -> Result<(), Error> {
    if got.len() == want {
        Ok(())
    } else {
        Err(Error::Length { want, got: got.len() })
    }
}

fn room(want: usize, out: &[u8]) -> Result<(), Error> {
    if out.len() >= want {
        Ok(())
    } else {
        Err(Error::Buffer { want, got: out.len() })
    }
}

fn checked(message: &[u8], check: usize) -> Result<(), Error> {
    let want = checksum::of(message, check);
    let got = message[check] & checksum::MASK;
    if want == got {
        Ok(())
    } else {
        Err(Error::Checksum { want, got })
    }
}

impl MasterMessage<'_> {
    /// Writes the message into `out` and returns its length: MC, CKT,
    /// the Process Data, then the On-request Data of a write.
    ///
    /// # Errors
    /// [`Error::Length`] when `pd_out` or `od` is not what `layout` and
    /// the read/write bit give it (`od` is empty for a read),
    /// [`Error::Buffer`] when `out` is too short.
    pub fn encode(&self, layout: Layout, out: &mut [u8]) -> Result<usize, Error> {
        let read = self.mc.read();
        part(layout.pd_out(), self.pd_out)?;
        part(if read { 0 } else { layout.od() }, self.od)?;
        let len = layout.master_len(read);
        room(len, out)?;
        let out = &mut out[..len];
        out[0] = self.mc.octet();
        out[1] = layout.mtype() << 6;
        let (pd, od) = out[2..].split_at_mut(layout.pd_out());
        pd.copy_from_slice(self.pd_out);
        od.copy_from_slice(self.od);
        out[1] |= checksum::of(out, 1);
        Ok(len)
    }
}

impl<'a> MasterMessage<'a> {
    /// Reads a whole Master message, as a Device receives it.
    ///
    /// # Errors
    /// [`Error::Length`] when `message` is not the length its own
    /// read/write bit gives it under `layout`, [`Error::Type`] for
    /// another M-sequence type, [`Error::Checksum`].
    pub fn decode(layout: Layout, message: &'a [u8]) -> Result<Self, Error> {
        let Some(&first) = message.first() else {
            return Err(Error::Length {
                want: layout.master_len(true),
                got: 0,
            });
        };
        let mc = Mc::from_octet(first);
        part(layout.master_len(mc.read()), message)?;
        let got = message[1] >> 6;
        if got != layout.mtype() {
            return Err(Error::Type {
                want: layout.mtype(),
                got,
            });
        }
        checked(message, 1)?;
        let (pd_out, od) = message[2..].split_at(layout.pd_out());
        Ok(Self { mc, pd_out, od })
    }
}

impl Reply<'_> {
    /// Writes the reply to a read (`read`) or a write into `out` and
    /// returns its length: the On-request Data of a read, the Process
    /// Data, CKS.
    ///
    /// # Errors
    /// [`Error::Length`] when `od` or `pd_in` is not what `layout` gives
    /// it (`od` is empty after a write), [`Error::Buffer`] when `out` is
    /// too short.
    pub fn encode(&self, layout: Layout, read: bool, out: &mut [u8]) -> Result<usize, Error> {
        part(if read { layout.od() } else { 0 }, self.od)?;
        part(layout.pd_in(), self.pd_in)?;
        let len = layout.reply_len(read);
        room(len, out)?;
        let out = &mut out[..len];
        let (od, rest) = out.split_at_mut(self.od.len());
        od.copy_from_slice(self.od);
        rest[..layout.pd_in()].copy_from_slice(self.pd_in);
        let mut cks = 0;
        if !self.pd_valid {
            cks |= CKS_PD_INVALID;
        }
        if self.event {
            cks |= CKS_EVENT;
        }
        out[len - 1] = cks;
        out[len - 1] |= checksum::of(out, len - 1);
        Ok(len)
    }
}

impl<'a> Reply<'a> {
    /// Reads a whole Device reply to a read (`read`) or a write, as the
    /// Master receives it.
    ///
    /// # Errors
    /// [`Error::Length`] when `reply` is not the length `layout` gives
    /// it, [`Error::Checksum`].
    pub fn decode(layout: Layout, read: bool, reply: &'a [u8]) -> Result<Self, Error> {
        let len = layout.reply_len(read);
        part(len, reply)?;
        checked(reply, len - 1)?;
        let cks = reply[len - 1];
        let (od, pd_in) = reply[..len - 1].split_at(if read { layout.od() } else { 0 });
        Ok(Self {
            od,
            pd_in,
            pd_valid: cks & CKS_PD_INVALID == 0,
            event: cks & CKS_EVENT != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(pd_out: u8, od: u8, pd_in: u8) -> Layout {
        Layout::type2(pd_out, od, pd_in).unwrap()
    }

    /// The octets here were worked by hand from IOL Figure A.1 and
    /// equations (A.1): a read of `MinCycleTime` (page address 0x02) is
    /// MC 0xA2, and 0x52 ^ 0xA2 = 0xF0 folds to 0.
    #[test]
    fn a_type_0_page_read_is_two_octets() {
        let mut out = [0; MAX_MESSAGE];
        let message = MasterMessage {
            mc: Mc::read_page(0x02).unwrap(),
            pd_out: &[],
            od: &[],
        };
        let len = message.encode(Layout::TYPE_0, &mut out).unwrap();
        assert_eq!(&out[..len], [0xA2, 0x00]);
        assert_eq!(MasterMessage::decode(Layout::TYPE_0, &out[..len]), Ok(message));
    }

    /// `DevicePreoperate` (0x9A) written to `MasterCommand` (address 0):
    /// 0x52 ^ 0x20 ^ 0x9A = 0xE8, which folds to 0x36.
    #[test]
    fn a_type_0_page_write_carries_its_octet_after_the_check() {
        let mut out = [0; MAX_MESSAGE];
        let message = MasterMessage {
            mc: Mc::write_page(0x00).unwrap(),
            pd_out: &[],
            od: &[0x9A],
        };
        let len = message.encode(Layout::TYPE_0, &mut out).unwrap();
        assert_eq!(&out[..len], [0x20, 0x36, 0x9A]);
    }

    /// A reply of 0x17: 0x52 ^ 0x17 = 0x45 folds to 0x1B; with the Event
    /// flag, 0xC5 folds to 0x33.
    #[test]
    fn a_reply_ends_in_its_flags_and_checksum() {
        let mut out = [0; MAX_MESSAGE];
        let mut reply = Reply {
            od: &[0x17],
            pd_in: &[],
            pd_valid: true,
            event: false,
        };
        let len = reply.encode(Layout::TYPE_0, true, &mut out).unwrap();
        assert_eq!(&out[..len], [0x17, 0x1B]);
        reply.event = true;
        let len = reply.encode(Layout::TYPE_0, true, &mut out).unwrap();
        assert_eq!(&out[..len], [0x17, 0xB3]);
        assert_eq!(Reply::decode(Layout::TYPE_0, true, &out[..len]), Ok(reply));
        // After a write a TYPE_0 reply is the check octet alone.
        let ack = Reply {
            od: &[],
            pd_in: &[],
            pd_valid: true,
            event: false,
        };
        assert_eq!(ack.encode(Layout::TYPE_0, false, &mut out), Ok(1));
    }

    /// IOL Figure A.14: Process Data out before the On-request Data in
    /// the Master message, On-request Data before Process Data in the
    /// reply.
    #[test]
    fn type_2_orders_its_parts_as_figure_a14() {
        let l = layout(2, 2, 3);
        let mut out = [0; MAX_MESSAGE];
        let write = MasterMessage {
            mc: Mc::new(false, Channel::Isdu, 1).unwrap(),
            pd_out: &[0xAA, 0xBB],
            od: &[0x01, 0x02],
        };
        let len = write.encode(l, &mut out).unwrap();
        assert_eq!(out[0], 0x61);
        assert_eq!(out[1] >> 6, 2);
        assert_eq!(&out[2..len], [0xAA, 0xBB, 0x01, 0x02]);
        let reply = Reply {
            od: &[0x01, 0x02],
            pd_in: &[0x10, 0x20, 0x30],
            pd_valid: false,
            event: false,
        };
        let len = reply.encode(l, true, &mut out).unwrap();
        assert_eq!(&out[..len - 1], [0x01, 0x02, 0x10, 0x20, 0x30]);
        assert_eq!(out[len - 1] & 0xC0, 0x40);
        assert_eq!(l.reply_len(false), 4);
        assert_eq!(l.master_len(true), 4);
    }

    #[test]
    fn a_message_of_another_type_or_length_is_refused() {
        let mut out = [0; MAX_MESSAGE];
        let read = MasterMessage {
            mc: Mc::read_page(0x02).unwrap(),
            pd_out: &[],
            od: &[],
        };
        let len = read.encode(Layout::TYPE_0, &mut out).unwrap();
        assert_eq!(
            MasterMessage::decode(Layout::type1(2).unwrap(), &out[..len]),
            Err(Error::Type { want: 1, got: 0 })
        );
        assert_eq!(
            MasterMessage::decode(Layout::TYPE_0, &out[..1]),
            Err(Error::Length { want: 2, got: 1 })
        );
        assert_eq!(
            MasterMessage::decode(Layout::TYPE_0, &[]),
            Err(Error::Length { want: 2, got: 0 })
        );
        assert_eq!(
            Reply::decode(Layout::TYPE_0, true, &[0x17, 0x1A]),
            Err(Error::Checksum {
                want: 0x1B,
                got: 0x1A
            })
        );
        assert_eq!(
            read.encode(Layout::TYPE_0, &mut out[..1]),
            Err(Error::Buffer { want: 2, got: 1 })
        );
        let stray = MasterMessage { od: &[1], ..read };
        assert_eq!(
            stray.encode(Layout::TYPE_0, &mut out),
            Err(Error::Length { want: 0, got: 1 })
        );
    }

    #[test]
    fn layouts_take_only_the_lengths_the_types_have() {
        assert!(Layout::type1(1).is_none());
        assert!(Layout::type1(8).is_some());
        assert!(Layout::type2(0, 1, 0).is_none());
        assert!(Layout::type2(33, 1, 0).is_none());
        assert!(Layout::type2(0, 4, 1).is_none());
        assert!(Mc::new(true, Channel::Page, 0x20).is_none());
    }

    /// IOL Table A.8.
    #[test]
    fn preoperate_follows_table_a8() {
        assert_eq!(Layout::preoperate(0), Ok(Layout::TYPE_0));
        assert_eq!(Layout::preoperate(1), Ok(Layout::type1(2).unwrap()));
        assert_eq!(Layout::preoperate(2), Ok(Layout::type1(8).unwrap()));
        assert_eq!(Layout::preoperate(3), Ok(Layout::type1(32).unwrap()));
        assert!(Layout::preoperate(4).is_err());
    }

    /// IOL Table A.10, row by row (lengths in octets on the wire).
    #[test]
    fn operate_follows_table_a10() {
        let op = Layout::operate;
        // No Process Data.
        assert_eq!(op(0, 0, 0), Ok(Layout::TYPE_0));
        assert_eq!(op(1, 0, 0), Ok(Layout::type1(2).unwrap()));
        assert_eq!(op(6, 0, 0), Ok(Layout::type1(8).unwrap()));
        assert_eq!(op(7, 0, 0), Ok(Layout::type1(32).unwrap()));
        // Code 0 with up to 16 bits each way: TYPE_2_1 to TYPE_2_5, then
        // TYPE_2_V.
        assert_eq!(op(0, 1, 0), Ok(layout(0, 1, 1)));
        assert_eq!(op(0, 2, 0), Ok(layout(0, 1, 2)));
        assert_eq!(op(0, 0, 1), Ok(layout(1, 1, 0)));
        assert_eq!(op(0, 0, 2), Ok(layout(2, 1, 0)));
        assert_eq!(op(0, 1, 1), Ok(layout(1, 1, 1)));
        assert_eq!(op(0, 2, 1), Ok(layout(1, 1, 2)));
        assert_eq!(op(0, 1, 2), Ok(layout(2, 1, 1)));
        assert_eq!(op(0, 2, 2), Ok(layout(2, 1, 2)));
        // Code 0 from 3 octets: interleaved.
        assert_eq!(op(0, 3, 0), Err(Error::Interleaved));
        assert_eq!(op(0, 0, 32), Err(Error::Interleaved));
        // Code 4: TYPE_2_V with one octet of On-request Data, from 3
        // octets either way.
        assert_eq!(op(4, 3, 0), Ok(layout(0, 1, 3)));
        assert_eq!(op(4, 1, 32), Ok(layout(32, 1, 1)));
        assert!(op(4, 2, 2).is_err());
        assert!(op(4, 0, 0).is_err());
        // Codes 5 to 7: TYPE_2_V with 2, 8, 32 octets of On-request Data.
        assert_eq!(op(5, 1, 0), Ok(layout(0, 2, 1)));
        assert_eq!(op(6, 0, 4), Ok(layout(4, 8, 0)));
        assert_eq!(op(7, 32, 32), Ok(layout(32, 32, 32)));
        assert!(op(5, 0, 0).is_err());
        // Not in the table.
        assert!(op(1, 1, 0).is_err());
        assert!(op(2, 0, 0).is_err());
        assert!(op(3, 1, 1).is_err());
        assert!(op(8, 0, 0).is_err());
        assert!(op(7, 33, 0).is_err());
    }
}
