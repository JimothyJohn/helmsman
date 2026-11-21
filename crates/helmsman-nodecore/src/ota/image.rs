//! What an ESP-IDF application image says about itself in its first 24
//! bytes, enough to refuse an image built for another chip or one that is
//! not an image at all. The format is the one `espflash save-image` writes
//! (esptool's "firmware image format": magic 0xE9, segment count, chip id
//! at byte 12, a flag at byte 23 for the SHA-256 appended to the image).

/// Bytes in the fixed image header.
pub const HEADER_LEN: usize = 24;
const HEADER_MIN: u32 = 24;
/// First byte of every ESP-IDF image.
pub const MAGIC: u8 = 0xE9;
/// Most segments an image may declare (esptool's limit).
pub const MAX_SEGMENTS: u8 = 16;

/// The chip ids an image header can carry (`esp_chip_id_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    Esp32,
    Esp32C3,
    Esp32S3,
    Esp32P4,
}

impl Chip {
    /// The value at byte 12 of an image.
    #[must_use]
    pub const fn raw(self) -> u16 {
        match self {
            Self::Esp32 => 0x0000,
            Self::Esp32C3 => 0x0005,
            Self::Esp32S3 => 0x0009,
            Self::Esp32P4 => 0x0012,
        }
    }

    #[must_use]
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0x0000 => Some(Self::Esp32),
            0x0005 => Some(Self::Esp32C3),
            0x0009 => Some(Self::Esp32S3),
            0x0012 => Some(Self::Esp32P4),
            _ => None,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Esp32 => "esp32",
            Self::Esp32C3 => "esp32c3",
            Self::Esp32S3 => "esp32s3",
            Self::Esp32P4 => "esp32p4",
        }
    }
}

/// Why a header was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    TooShort,
    BadMagic,
    BadSegmentCount,
    /// A chip id this node does not know.
    UnknownChip(u16),
}

impl core::fmt::Display for ImageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort => write!(f, "shorter than an image header"),
            Self::BadMagic => write!(f, "not an application image (first byte is not 0xE9)"),
            Self::BadSegmentCount => write!(f, "the header's segment count is not 1 to 16"),
            Self::UnknownChip(id) => write!(
                f,
                "the header names chip id {id:#x}, which this node does not know"
            ),
        }
    }
}

/// What the header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub chip: Chip,
    pub segments: u8,
    /// A SHA-256 follows the last segment (the bootloader checks it on
    /// every boot).
    pub hash_appended: bool,
}

impl Header {
    /// Reads the header at the start of an image.
    ///
    /// # Errors
    /// What is wrong with it.
    pub fn parse(bytes: &[u8]) -> Result<Self, ImageError> {
        if bytes.len() < HEADER_LEN {
            return Err(ImageError::TooShort);
        }
        if bytes[0] != MAGIC {
            return Err(ImageError::BadMagic);
        }
        let segments = bytes[1];
        if segments == 0 || segments > MAX_SEGMENTS {
            return Err(ImageError::BadSegmentCount);
        }
        let raw = u16::from_le_bytes([bytes[12], bytes[13]]);
        let chip = Chip::from_raw(raw).ok_or(ImageError::UnknownChip(raw))?;
        Ok(Self {
            chip,
            segments,
            hash_appended: bytes[23] == 1,
        })
    }
}

/// An image of `size` bytes fits a slot of `capacity`; zero and anything
/// shorter than a header are not images.
#[must_use]
pub const fn fits(size: u32, capacity: u32) -> bool {
    size >= HEADER_MIN && size <= capacity
}
