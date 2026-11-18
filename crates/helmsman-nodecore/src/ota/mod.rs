//! Firmware updates over the network (docs/EDGE.md, "Updating a node
//! without a cable"): the node pulls an image from a URL it is given,
//! writes it into the idle one of two app slots, proves it is the image it
//! was told to fetch, and only then points the bootloader at it. The new
//! image is on trial until it confirms itself; one that never does is
//! replaced by the old one at the fourth boot.
//!
//! Everything here is `no_std` and runs on the host against simulated
//! flash. The firmware supplies the real flash behind two small traits
//! ([`install::Slot`], [`boot::Otadata`]) and the sockets. The ideas are
//! supervictor's `ota/{install,policy,http,manifest}` (the edge firmware
//! next door), ported: the installer buffers whole 4 KiB sectors, hashes
//! as it goes and reads the slot back; the response head is parsed
//! strictly because its body goes straight into flash; the boot policy
//! counts unconfirmed boots. Two things differ on purpose. The trial
//! record lives in its own flash partition ([`record`]) instead of RTC
//! memory, so a power cycle during the trial cannot lose it, and the
//! record is written before the bootloader is pointed at the new slot, so
//! no power cut can leave an image on trial that nothing is counting.

pub mod boot;
pub mod image;
pub mod install;
pub mod record;
pub mod request;
pub mod status;
pub mod wire;

#[cfg(test)]
mod sim;
#[cfg(test)]
mod tests;
