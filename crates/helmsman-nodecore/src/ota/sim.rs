//! A simulated board for the tests: two app slots, the trial record's
//! partition and the bootloader's choice of slot, all in RAM, with a
//! power cut that can be scheduled at any write, and a flash that can lie.
//! One [`World`] is shared by the handles the code under test is given, so
//! a cut stops all of them at once, the way it would.

extern crate std;

use std::cell::RefCell;
use std::rc::Rc;
use std::vec;
use std::vec::Vec;

use super::boot::Otadata;
use super::install::{SECTOR, Slot};

/// Slot size in the simulation: 16 sectors.
pub const SLOT_CAP: usize = 16 * SECTOR;
pub const SLOT_CAP_U32: u32 = 16 * 4096;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Fault {
    /// Power was cut during or at this operation, or earlier.
    PowerCut,
    /// A scripted failure of one write (the flash said no).
    WriteFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// The operation does not happen (a torn sector for a flash write).
    Before,
    /// The operation happens and the power goes right after.
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Slot0,
    Slot1,
    Record,
}

pub struct World {
    pub slots: [Vec<u8>; 2],
    pub record: Vec<u8>,
    pub selected: u8,
    /// Write operations so far (flash sector writes and `select`s).
    pub ops: usize,
    /// Cut the power at this operation number (1 is the first).
    pub cut_at: Option<(usize, Cut)>,
    pub dead: bool,
    /// Fail the write that covers this offset in this part, once.
    pub fail_write: Option<(Part, u32)>,
    /// Flip a bit in the sector this write programs, once: a flash that lies.
    pub corrupt_write: Option<(Part, u32)>,
}

pub type Shared = Rc<RefCell<World>>;

impl World {
    #[must_use]
    pub fn new() -> Shared {
        Rc::new(RefCell::new(Self {
            slots: [vec![0xFF; SLOT_CAP], vec![0xFF; SLOT_CAP]],
            record: vec![0xFF; 2 * SECTOR],
            selected: 0,
            ops: 0,
            cut_at: None,
            dead: false,
            fail_write: None,
            corrupt_write: None,
        }))
    }

    /// Power comes back: nothing is dead and no cut is pending.
    pub fn power_on(&mut self) {
        self.dead = false;
        self.cut_at = None;
    }

    /// Counts an operation and says what the cut does to it.
    fn tick(&mut self) -> Result<Option<Cut>, Fault> {
        if self.dead {
            return Err(Fault::PowerCut);
        }
        self.ops += 1;
        match self.cut_at {
            Some((n, how)) if n == self.ops => {
                self.dead = true;
                Ok(Some(how))
            }
            _ => Ok(None),
        }
    }
}

pub struct SimPart {
    pub w: Shared,
    pub part: Part,
}

impl SimPart {
    #[must_use]
    pub fn new(w: &Shared, part: Part) -> Self {
        Self {
            w: Rc::clone(w),
            part,
        }
    }
}

impl Slot for SimPart {
    type Error = Fault;

    fn capacity(&self) -> u32 {
        let w = self.w.borrow();
        let len = match self.part {
            Part::Slot0 => w.slots[0].len(),
            Part::Slot1 => w.slots[1].len(),
            Part::Record => w.record.len(),
        };
        u32::try_from(len).unwrap()
    }

    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Fault> {
        assert_eq!(offset as usize % SECTOR, 0, "an unaligned sector write");
        assert!(data.len() <= SECTOR, "more than a sector in one write");
        let mut w = self.w.borrow_mut();
        let cut = w.tick()?;
        let covers = |at: u32| at >= offset && (at as usize) < offset as usize + data.len();
        if w.fail_write.is_some_and(|(p, at)| p == self.part && covers(at)) {
            w.fail_write = None;
            return Err(Fault::WriteFailed);
        }
        let corrupt = w
            .corrupt_write
            .filter(|&(p, at)| p == self.part && covers(at))
            .map(|(_, at)| at);
        if corrupt.is_some() {
            w.corrupt_write = None;
        }
        let part = self.part;
        let mem = match part {
            Part::Slot0 => &mut w.slots[0],
            Part::Slot1 => &mut w.slots[1],
            Part::Record => &mut w.record,
        };
        let sector = &mut mem[offset as usize..offset as usize + SECTOR];
        match cut {
            Some(Cut::Before) => {
                // Erased, then programmed halfway: a torn sector.
                sector.fill(0xFF);
                let half = data.len() / 2;
                sector[..half].copy_from_slice(&data[..half]);
                return Err(Fault::PowerCut);
            }
            Some(Cut::After) | None => {
                sector.fill(0xFF);
                sector[..data.len()].copy_from_slice(data);
                if let Some(at) = corrupt {
                    mem[at as usize] ^= 0x55;
                }
            }
        }
        if cut.is_some() {
            return Err(Fault::PowerCut);
        }
        Ok(())
    }

    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), Fault> {
        let w = self.w.borrow();
        if w.dead {
            return Err(Fault::PowerCut);
        }
        let mem = match self.part {
            Part::Slot0 => &w.slots[0],
            Part::Slot1 => &w.slots[1],
            Part::Record => &w.record,
        };
        let end = offset as usize + buf.len();
        assert!(end <= mem.len(), "a read past the partition");
        buf.copy_from_slice(&mem[offset as usize..end]);
        Ok(())
    }
}

pub struct SimOta {
    pub w: Shared,
}

impl Otadata for SimOta {
    type Error = Fault;

    fn selected(&mut self) -> Result<u8, Fault> {
        let w = self.w.borrow();
        if w.dead {
            return Err(Fault::PowerCut);
        }
        Ok(w.selected)
    }

    fn select(&mut self, slot: u8) -> Result<(), Fault> {
        let mut w = self.w.borrow_mut();
        let cut = w.tick()?;
        match cut {
            Some(Cut::Before) => return Err(Fault::PowerCut),
            Some(Cut::After) => {
                w.selected = slot;
                return Err(Fault::PowerCut);
            }
            None => w.selected = slot,
        }
        Ok(())
    }
}
