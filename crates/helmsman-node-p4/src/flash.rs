//! The settings record and the program slots in the `nvs` partition
//! (helmsman-nodecore's `settings` and `program` lay them out: 24 KiB,
//! exactly the partition espflash writes by default). The flash driver is
//! esp-storage's, borrowed per call; the partition table is read once.
//! supervictor's `settings/device.rs` is the shape this follows.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use esp_bootloader_esp_idf::ota_updater::OtaUpdater;
use esp_bootloader_esp_idf::partitions::{
    self, AppPartitionSubType, DataPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionType,
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

/// Where the two app slots and the trial record sit in flash (the
/// partition table, `partitions.csv`): `(offset, length)` of each, absolute.
/// Read once at boot; `None` on a board flashed with a table that has one
/// app partition, which then has no network update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub slots: [(u32, u32); 2],
    pub record: (u32, u32),
}

/// The partition, its contents as last read, and the flash driver.
pub struct Store {
    flash: FlashStorage<'static>,
    table: Vec<u8>,
    layout: Option<Layout>,
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
        let layout = find_layout(&mut flash, &mut table);
        Ok(Self {
            flash,
            table,
            layout,
            settings,
            program,
        })
    }

    /// Where the OTA slots are, if this board's table has them.
    #[must_use]
    pub fn layout(&self) -> Option<Layout> {
        self.layout
    }

    /// Reads `buf.len()` bytes at `offset` inside `part` (`(offset,
    /// length)`, absolute).
    ///
    /// # Errors
    /// Outside the partition, or the flash failing.
    pub fn part_read(&mut self, part: (u32, u32), offset: u32, buf: &mut [u8]) -> Result<(), StoreError> {
        check_span(part, offset, buf.len())?;
        self.flash
            .read(part.0 + offset, buf)
            .map_err(|_| StoreError::Flash)
    }

    /// Erases and programs the sector `data` starts in, at `offset` inside
    /// `part`. esp-storage reads, erases and rewrites each sector it
    /// touches, so what `data` does not cover keeps what it had.
    ///
    /// # Errors
    /// Outside the partition, or the flash failing.
    pub fn part_write(&mut self, part: (u32, u32), offset: u32, data: &[u8]) -> Result<(), StoreError> {
        check_span(part, offset, data.len())?;
        self.flash
            .write(part.0 + offset, data)
            .map_err(|_| StoreError::Flash)
    }

    /// The slot, 0 or 1, the bootloader boots next. A table whose `otadata`
    /// is erased (a fresh USB flash) boots the first app partition; this
    /// writes that choice down, because the library's step from "nothing
    /// chosen" to the second slot starts its sequence at zero.
    ///
    /// # Errors
    /// No `otadata`, or the flash failing.
    pub fn selected_slot(&mut self) -> Result<u8, StoreError> {
        let buf = table_buf(&mut self.table)?;
        let mut updater = OtaUpdater::new(&mut self.flash, buf).map_err(|_| StoreError::NoPartition)?;
        match updater.selected_partition().map_err(|_| StoreError::Flash)? {
            AppPartitionSubType::Ota0 => Ok(0),
            AppPartitionSubType::Ota1 => Ok(1),
            AppPartitionSubType::Factory => {
                updater
                    .ota_data()
                    .and_then(|mut o| o.set_current_app_partition(AppPartitionSubType::Ota0))
                    .map_err(|_| StoreError::Flash)?;
                Ok(0)
            }
            _ => Err(StoreError::NoPartition),
        }
    }

    /// Makes the bootloader boot `slot` next.
    ///
    /// # Errors
    /// No `otadata`, a slot other than 0 or 1, or the flash failing.
    pub fn select_slot(&mut self, slot: u8) -> Result<(), StoreError> {
        let app = match slot {
            0 => AppPartitionSubType::Ota0,
            1 => AppPartitionSubType::Ota1,
            _ => return Err(StoreError::NoPartition),
        };
        let buf = table_buf(&mut self.table)?;
        let mut updater = OtaUpdater::new(&mut self.flash, buf).map_err(|_| StoreError::NoPartition)?;
        updater
            .ota_data()
            .and_then(|mut o| o.set_current_app_partition(app))
            .map_err(|_| StoreError::Flash)
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
        // The map rides along unchanged (card node-io-map).
        let map = self.program.as_ref().map(|p| p.map.clone()).unwrap_or_default();
        self.write_slot(name, source, &map)
    }

    /// Writes the I/O map text beside the program (kept as it is) as the
    /// next record into the other slot, read back.
    ///
    /// # Errors
    /// A map too big for the slot beside the program, the flash failing or
    /// the read-back differing.
    pub fn write_map(&mut self, map: &str) -> Result<(), StoreError> {
        let (name, source) = self
            .program
            .as_ref()
            .map(|p| (p.name.clone(), p.source.clone()))
            .unwrap_or_default();
        self.write_slot(&name, &source, map)
    }

    fn write_slot(&mut self, name: &str, source: &str, map: &str) -> Result<(), StoreError> {
        let (slot, seq) = program::next(self.program.as_ref());
        let mut frame = vec![0xFFu8; program::SLOT_LEN];
        let used = program::encode(name, source, map, seq, &mut frame).map_err(|_| StoreError::TooBig)?;
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
            map: String::from(map),
        });
        Ok(())
    }
}

fn check_span(part: (u32, u32), offset: u32, len: usize) -> Result<(), StoreError> {
    if u64::from(offset) + len as u64 > u64::from(part.1) {
        return Err(StoreError::TooBig);
    }
    Ok(())
}

fn table_buf(table: &mut [u8]) -> Result<&mut [u8; PARTITION_TABLE_MAX_LEN], StoreError> {
    table.try_into().map_err(|_| StoreError::NoPartition)
}

/// The two OTA slots and the trial record from the partition table. The
/// record is the data partition labelled `bootrec`. `None` unless all
/// three are there and the record holds its two sectors.
fn find_layout(flash: &mut FlashStorage<'static>, table: &mut [u8]) -> Option<Layout> {
    let buf = table_buf(table).ok()?;
    let table = partitions::read_partition_table(flash, buf).ok()?;
    let mut slots = [None; 2];
    let mut record = None;
    for entry in table.iter() {
        match entry.partition_type() {
            PartitionType::App(AppPartitionSubType::Ota0) => slots[0] = Some((entry.offset(), entry.len())),
            PartitionType::App(AppPartitionSubType::Ota1) => slots[1] = Some((entry.offset(), entry.len())),
            PartitionType::Data(DataPartitionSubType::Undefined) if entry.label_as_str() == "bootrec" => {
                record = Some((entry.offset(), entry.len()));
            }
            _ => {}
        }
    }
    let record = record.filter(|r| r.1 >= helmsman_nodecore::ota::record::SPAN)?;
    Some(Layout {
        slots: [slots[0]?, slots[1]?],
        record,
    })
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
