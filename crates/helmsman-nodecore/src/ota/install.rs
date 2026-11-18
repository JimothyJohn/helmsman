//! Stream an image into a slot and prove it is the one the request named
//! before anyone boots it.
//!
//! The installer buffers to whole 4 KiB sectors (one erase and program
//! each), hashes as it goes, checks the ESP-IDF header as soon as it has
//! one, and on [`Installer::finish`] compares the size and the digest,
//! then reads the slot back and hashes it again: a flash write that lied
//! is caught here, not by the bootloader refusing to boot. Ported from
//! supervictor's `ota/install.rs` without its resume record (a node
//! fetches again from the start; an image is under 2 MiB).
//!
//! Generic over [`Slot`], so all of it runs on the host against a RAM slot
//! with faults injected.

use sha2::{Digest, Sha256};

use super::image::{self, Chip, Header, ImageError};

/// Flash sector: the erase granularity and the write unit used here.
pub const SECTOR: usize = 4096;
/// The same, for offsets.
pub const SECTOR_U32: u32 = 4096;

/// Where bytes go. Offsets are relative to the slot.
pub trait Slot {
    type Error: core::fmt::Debug;
    /// Bytes the slot can hold.
    fn capacity(&self) -> u32;
    /// Erase the sector at `offset` and program `data` into it. `offset`
    /// is sector aligned; `data` is at most one sector, and the bytes of
    /// the sector after it are left erased.
    ///
    /// # Errors
    /// The backend's.
    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Self::Error>;
    /// Read `buf.len()` bytes from `offset`.
    ///
    /// # Errors
    /// The backend's.
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), Self::Error>;
}

/// What a fetch is expected to deliver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    /// Bytes, from the server's `Content-Length`.
    pub size: u32,
    /// The digest the operator gave in the request.
    pub sha256: [u8; 32],
    /// The chip this node is; an image for another is refused.
    pub chip: Chip,
}

/// Why an install failed.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallError<E> {
    /// The image does not fit the slot (or is shorter than a header).
    TooLarge,
    /// More bytes arrived than were announced.
    Overrun,
    /// `finish` came before `size` bytes had.
    Truncated { got: u32, want: u32 },
    /// The first bytes are not an image header.
    BadImage(ImageError),
    /// The header names another chip.
    ChipMismatch { found: Chip },
    /// The bytes hash to something other than the digest in the request.
    DigestMismatch,
    /// The flash, read back, hashes differently from what was streamed.
    ReadbackMismatch,
    /// The backend failed.
    Slot(E),
}

impl<E> From<E> for InstallError<E> {
    fn from(e: E) -> Self {
        Self::Slot(e)
    }
}

impl<E: core::fmt::Debug> core::fmt::Display for InstallError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "the image does not fit the slot"),
            Self::Overrun => write!(f, "the server sent more bytes than it announced"),
            Self::Truncated { got, want } => write!(f, "the image stopped at {got} of {want} bytes"),
            Self::BadImage(e) => write!(f, "{e}"),
            Self::ChipMismatch { found } => {
                write!(f, "the image is for the {}, not this chip", found.label())
            }
            Self::DigestMismatch => write!(f, "the image does not match the sha256 in the request"),
            Self::ReadbackMismatch => write!(f, "the flash does not hold what was written"),
            Self::Slot(e) => write!(f, "flash: {e:?}"),
        }
    }
}

/// Proof that a slot holds the requested image, byte for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    pub size: u32,
    pub sha256: [u8; 32],
}

/// Streaming writer for one image into one slot.
pub struct Installer<'a, S: Slot> {
    slot: S,
    buf: &'a mut [u8; SECTOR],
    buffered: usize,
    written: u32,
    want: Image,
    header_checked: bool,
    hasher: Sha256,
}

impl<'a, S: Slot> Installer<'a, S> {
    /// Starts an install of `want` into `slot`.
    ///
    /// # Errors
    /// [`InstallError::TooLarge`] when it cannot fit.
    pub fn new(slot: S, buf: &'a mut [u8; SECTOR], want: Image) -> Result<Self, InstallError<S::Error>> {
        if !image::fits(want.size, slot.capacity()) {
            return Err(InstallError::TooLarge);
        }
        Ok(Self {
            slot,
            buf,
            buffered: 0,
            written: 0,
            want,
            header_checked: false,
            hasher: Sha256::new(),
        })
    }

    /// Bytes received so far, committed or buffered.
    #[must_use]
    pub fn received(&self) -> u32 {
        self.written + u32::try_from(self.buffered).unwrap_or(u32::MAX)
    }

    /// Bytes still expected.
    #[must_use]
    pub fn remaining(&self) -> u32 {
        self.want.size - self.received()
    }

    fn check_header(&mut self) -> Result<(), InstallError<S::Error>> {
        let header = Header::parse(&self.buf[..image::HEADER_LEN]).map_err(InstallError::BadImage)?;
        if header.chip != self.want.chip {
            return Err(InstallError::ChipMismatch { found: header.chip });
        }
        self.header_checked = true;
        Ok(())
    }

    /// Takes the next bytes of the image, in any chunking.
    ///
    /// # Errors
    /// More than announced, a bad header, or the flash failing.
    pub fn feed(&mut self, mut data: &[u8]) -> Result<(), InstallError<S::Error>> {
        if data.len() as u64 > u64::from(self.remaining()) {
            return Err(InstallError::Overrun);
        }
        while !data.is_empty() {
            let room = SECTOR - self.buffered;
            let n = data.len().min(room);
            self.buf[self.buffered..self.buffered + n].copy_from_slice(&data[..n]);
            self.buffered += n;
            data = &data[n..];
            if !self.header_checked && self.buffered >= image::HEADER_LEN {
                self.check_header()?;
            }
            if self.buffered == SECTOR {
                self.flush_sector()?;
            }
        }
        Ok(())
    }

    fn flush_sector(&mut self) -> Result<(), InstallError<S::Error>> {
        let n = self.buffered;
        self.slot.write(self.written, &self.buf[..n])?;
        self.hasher.update(&self.buf[..n]);
        self.written += u32::try_from(n).unwrap_or(u32::MAX);
        self.buffered = 0;
        Ok(())
    }

    /// All bytes have arrived: writes the tail, compares size and digest,
    /// then reads the whole slot back and hashes it again.
    ///
    /// # Errors
    /// Any of the ways the image is not the one asked for.
    pub fn finish(mut self) -> Result<Verified, InstallError<S::Error>> {
        if self.received() != self.want.size {
            return Err(InstallError::Truncated {
                got: self.received(),
                want: self.want.size,
            });
        }
        if !self.header_checked {
            self.check_header()?;
        }
        if self.buffered > 0 {
            self.flush_sector()?;
        }
        let streamed: [u8; 32] = core::mem::take(&mut self.hasher).finalize().into();
        if streamed != self.want.sha256 {
            return Err(InstallError::DigestMismatch);
        }
        let mut hasher = Sha256::new();
        let mut pos = 0u32;
        while pos < self.want.size {
            let n = (self.want.size - pos).min(SECTOR_U32) as usize;
            self.slot.read(pos, &mut self.buf[..n])?;
            hasher.update(&self.buf[..n]);
            pos += u32::try_from(n).unwrap_or(u32::MAX);
        }
        let on_flash: [u8; 32] = hasher.finalize().into();
        if on_flash != self.want.sha256 {
            return Err(InstallError::ReadbackMismatch);
        }
        Ok(Verified {
            size: self.want.size,
            sha256: self.want.sha256,
        })
    }
}
