//! The trial: activating a new image, counting its unconfirmed boots,
//! confirming it, and putting the old one back when it never confirms.
//! The bootloader the board ships selects a slot from `otadata` but does
//! not roll back on its own, so the firmware does (supervictor's
//! `ota/policy.rs`, the same idea).
//!
//! Order is what makes a power cut harmless. Activating writes the trial
//! record first and `otadata` second; reverting switches `otadata` first
//! and clears the record second. In both orders, a cut between the two
//! steps leaves a record that names a slot the board is not running, which
//! [`decide`] reads as stale and clears. No cut leaves an image on trial
//! with nothing counting it, and none leaves a counted trial that cannot
//! end.

use super::install::Slot;
use super::record::{self, Found, StoreError, Trial};

/// Unconfirmed boots tolerated before the old image comes back: the new
/// image may boot this many times, and the next boot reverts.
pub const MAX_PENDING_BOOTS: u8 = 3;

/// The bootloader's choice of slot (`otadata`).
pub trait Otadata {
    type Error: core::fmt::Debug;
    /// The slot, 0 or 1, the bootloader will boot next. A board whose
    /// `otadata` is erased boots slot 0 and says 0.
    ///
    /// # Errors
    /// The backend's.
    fn selected(&mut self) -> Result<u8, Self::Error>;
    /// Make the bootloader boot `slot` next.
    ///
    /// # Errors
    /// The backend's.
    fn select(&mut self, slot: u8) -> Result<(), Self::Error>;
}

/// What a boot has to do about the trial record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing is on trial.
    Normal,
    /// The record names a slot that is not running: leftover of a cut
    /// power or a USB flash; clear it.
    Stale,
    /// The running image is on trial and has `boots` boots including this.
    Count { boots: u8 },
    /// The budget is spent: go back to `previous`.
    Revert { previous: u8 },
}

/// The policy: a pure function of the record and the running slot.
#[must_use]
pub fn decide(trial: Trial, running: u8) -> Step {
    match trial {
        Trial::None => Step::Normal,
        Trial::On { target, .. } if target != running => Step::Stale,
        Trial::On { previous, boots, .. } => {
            let boots = boots.saturating_add(1);
            if boots > MAX_PENDING_BOOTS {
                Step::Revert { previous }
            } else {
                Step::Count { boots }
            }
        }
    }
}

/// What this boot is doing, for the console and `GET /api/firmware`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Normal,
    /// On trial, boot number `boots` of [`MAX_PENDING_BOOTS`].
    Confirming {
        boots: u8,
    },
    /// The trial failed; `otadata` points at `to` again and the node must
    /// reset to run it.
    Reverted {
        to: u8,
    },
}

/// What [`Boot::confirm`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmed {
    /// The running image was on trial and no longer is.
    Yes,
    /// It was not on trial: nothing changed.
    NothingToConfirm,
}

/// Why a step failed.
#[derive(Debug, PartialEq, Eq)]
pub enum Error<R, O> {
    Record(StoreError<R>),
    Read(R),
    Otadata(O),
    /// An update to the slot that is running.
    SameSlot,
}

/// The trial record and `otadata` together.
pub struct Boot<R: Slot, O: Otadata> {
    rec: R,
    ota: O,
    found: Option<Found>,
}

impl<R: Slot, O: Otadata> Boot<R, O> {
    /// Reads the record.
    ///
    /// # Errors
    /// The record region failing to read.
    pub fn open(mut rec: R, ota: O) -> Result<Self, Error<R::Error, O::Error>> {
        let found = record::load(&mut rec).map_err(Error::Read)?;
        Ok(Self { rec, ota, found })
    }

    /// What the record says now.
    #[must_use]
    pub fn trial(&self) -> Trial {
        self.found.map_or(Trial::None, |f| f.trial)
    }

    /// The slot the bootloader will boot.
    ///
    /// # Errors
    /// `otadata` failing to read.
    pub fn selected(&mut self) -> Result<u8, Error<R::Error, O::Error>> {
        self.ota.selected().map_err(Error::Otadata)
    }

    fn save(&mut self, trial: Trial) -> Result<(), Error<R::Error, O::Error>> {
        self.found = Some(record::store(&mut self.rec, self.found.as_ref(), trial).map_err(Error::Record)?);
        Ok(())
    }

    /// The first thing a boot does: count it against a trial, or end the
    /// trial. A failure leaves the node running; the next boot tries
    /// again.
    ///
    /// # Errors
    /// Flash failing while the record or `otadata` is written.
    pub fn on_boot(&mut self) -> Result<Decision, Error<R::Error, O::Error>> {
        let running = self.selected()?;
        match decide(self.trial(), running) {
            Step::Normal => Ok(Decision::Normal),
            Step::Stale => {
                self.save(Trial::None)?;
                Ok(Decision::Normal)
            }
            Step::Count { boots } => {
                let Trial::On { target, previous, .. } = self.trial() else {
                    return Ok(Decision::Normal);
                };
                self.save(Trial::On {
                    target,
                    previous,
                    boots,
                })?;
                Ok(Decision::Confirming { boots })
            }
            Step::Revert { previous } => {
                self.ota.select(previous).map_err(Error::Otadata)?;
                self.save(Trial::None)?;
                Ok(Decision::Reverted { to: previous })
            }
        }
    }

    /// An image has been installed and verified in `target`: put it on
    /// trial and make the bootloader boot it. The caller resets next.
    ///
    /// # Errors
    /// `target` is the running slot, or flash failed (the old image stays
    /// selected unless the failure came after `otadata` was written).
    pub fn start_trial(&mut self, target: u8) -> Result<(), Error<R::Error, O::Error>> {
        let previous = self.selected()?;
        if target == previous || target > 1 {
            return Err(Error::SameSlot);
        }
        self.save(Trial::On {
            target,
            previous,
            boots: 0,
        })?;
        self.ota.select(target).map_err(Error::Otadata)
    }

    /// The running image works: end its trial.
    ///
    /// # Errors
    /// Flash failing while the record is written.
    pub fn confirm(&mut self) -> Result<Confirmed, Error<R::Error, O::Error>> {
        let running = self.selected()?;
        match self.trial() {
            Trial::On { target, .. } if target == running => {
                self.save(Trial::None)?;
                Ok(Confirmed::Yes)
            }
            _ => Ok(Confirmed::NothingToConfirm),
        }
    }
}
