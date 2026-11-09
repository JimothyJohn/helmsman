//! The settings record and the program slots in the `nvs` partition
//! (helmsman-nodecore's `settings` and `program` lay them out: 24 KiB,
//! exactly the partition espflash writes by default). The flash driver is
//! esp-storage's, borrowed per call; the partition table is read once.
//! supervictor's `settings/device.rs` is the shape this follows.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use esp_bootloader_esp_idf::partitions::{
    self, DataPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionType,
};
use esp_println::println;
use esp_storage::FlashStorage;
use helmsman_nodecore::program::{self, Stored};
use helmsman_nodecore::settings::{self, Found, Settings};

/// Why the partition could not be read or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// No `nvs` partition, or an unreadable table.
    NoPartition,
    /// The partition is smaller than the four slots need.
    TooSmall,
    /// The flash driver failed, or a write did not read back.
    Flash,
    /// The program does not fit a slot.
    TooBig,
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPartition => write!(f, "no nvs partition"),
            Self::TooSmall => write!(f, "the nvs partition is smaller than 24 KiB"),
            Self::Flash => write!(f, "the flash driver failed"),
            Self::TooBig => write!(f, "the program does not fit its flash slot"),
        }
    }
}

/// The partition, its contents as last read, and the flash driver.
pub struct Store {
    flash: FlashStorage<'static>,
    table: Vec<u8>,
    settings: Option<Found>,
    program: Option<Stored>,
}

impl Store {
    /// Reads the partition. Both `None`s mean nothing valid is there.
    ///
    /// # Errors
    /// No usable partition or a flash read failing.
    pub fn open(mut flash: FlashStorage<'static>) -> Result<Self, StoreError> {
        let mut table = vec![0u8; PARTITION_TABLE_MAX_LEN];
        let (settings, program) = {
            let mut region = region(&mut flash, &mut table)?;
            let mut a = [0u8; settings::LEN];
            let mut b = [0u8; settings::LEN];
            region
                .read(settings::Slot::A.offset(), &mut a)
                .map_err(|_| StoreError::Flash)?;
            region
                .read(settings::Slot::B.offset(), &mut b)
                .map_err(|_| StoreError::Flash)?;
            let mut pa = vec![0u8; program::SLOT_LEN];
            let mut pb = vec![0u8; program::SLOT_LEN];
            region
                .read(program::offset(settings::Slot::A), &mut pa)
                .map_err(|_| StoreError::Flash)?;
            region
                .read(program::offset(settings::Slot::B), &mut pb)
                .map_err(|_| StoreError::Flash)?;
            (settings::choose(&a, &b), program::choose(&pa, &pb))
        };
        Ok(Self {
            flash,
            table,
            settings,
            program,
        })
    }

    #[must_use]
    pub fn settings(&self) -> Option<&Settings> {
        self.settings.as_ref().map(|f| &f.settings)
    }

    #[must_use]
    pub fn program(&self) -> Option<&Stored> {
        self.program.as_ref()
    }

    /// Writes `settings` as the next record into the other slot, read back.
    ///
    /// # Errors
    /// The flash failing or the read-back differing.
    pub fn write_settings(&mut self, settings: &Settings) -> Result<(), StoreError> {
        let (slot, seq) = settings::next(self.settings.as_ref());
        let mut frame = [0u8; settings::LEN];
        settings::encode(settings, seq, &mut frame);
        {
            let mut region = region(&mut self.flash, &mut self.table)?;
            region
                .write(slot.offset(), &frame)
                .map_err(|_| StoreError::Flash)?;
            let mut check = [0u8; settings::LEN];
            region
                .read(slot.offset(), &mut check)
                .map_err(|_| StoreError::Flash)?;
            if check != frame {
                return Err(StoreError::Flash);
            }
        }
        self.settings = Some(Found {
            slot,
            seq,
            settings: settings.clone(),
        });
        Ok(())
    }

    /// Writes the program as the next record into the other slot, read back.
    ///
    /// # Errors
    /// A program too big for a slot, the flash failing or the read-back
    /// differing.
    pub fn write_program(&mut self, name: &str, source: &str) -> Result<(), StoreError> {
        let (slot, seq) = program::next(self.program.as_ref());
        let mut frame = vec![0xFFu8; program::SLOT_LEN];
        let used = program::encode(name, source, seq, &mut frame).map_err(|_| StoreError::TooBig)?;
        {
            let mut region = region(&mut self.flash, &mut self.table)?;
            region
                .write(program::offset(slot), &frame[..used])
                .map_err(|_| StoreError::Flash)?;
            let mut check = vec![0u8; used];
            region
                .read(program::offset(slot), &mut check)
                .map_err(|_| StoreError::Flash)?;
            if check != frame[..used] {
                return Err(StoreError::Flash);
            }
        }
        self.program = Some(Stored {
            slot,
            seq,
            name: String::from(name),
            source: String::from(source),
        });
        Ok(())
    }
}

fn region<'a>(
    flash: &'a mut FlashStorage<'static>,
    table: &'a mut [u8],
) -> Result<partitions::FlashRegion<'a, 'static>, StoreError> {
    let buf: &mut [u8; PARTITION_TABLE_MAX_LEN] = table.try_into().map_err(|_| StoreError::NoPartition)?;
    let table = partitions::read_partition_table(flash, buf).map_err(|_| StoreError::NoPartition)?;
    let entry = table
        .find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
        .map_err(|_| StoreError::NoPartition)?
        .ok_or(StoreError::NoPartition)?;
    if entry.len() < program::SPAN {
        return Err(StoreError::TooSmall);
    }
    Ok(entry.as_flash_region(flash))
}

/// What boot runs on: the record on flash, else the factory settings,
/// written there so the next boot does not depend on the image.
pub fn boot_settings(store: &mut Store) -> Settings {
    if let Some(found) = store.settings.as_ref() {
        println!(
            "settings: record {} from slot {:?} (name {:?}, site {:?}, {} net, token {})",
            found.seq,
            found.slot,
            found.settings.name,
            found.settings.site,
            match found.settings.net {
                settings::Net::Dhcp => "dhcp",
                settings::Net::Static { .. } => "static",
            },
            if found.settings.token.is_empty() {
                "unset"
            } else {
                "set"
            }
        );
        return found.settings.clone();
    }
    let factory = Settings::factory();
    match store.write_settings(&factory) {
        Ok(()) => println!(
            "settings: nothing on flash; factory record written (static {}.{}.{}.{}/{})",
            settings::FACTORY_IP[0],
            settings::FACTORY_IP[1],
            settings::FACTORY_IP[2],
            settings::FACTORY_IP[3],
            settings::FACTORY_PREFIX
        ),
        Err(e) => println!(
            "settings: nothing on flash and the factory record did not write ({e}); running on it anyway"
        ),
    }
    factory
}
