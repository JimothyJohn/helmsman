//! Simulated plant devices (Nick, 2026-11-14: "The simulator needs to be
//! killer since that's where everything will be verified").
//!
//! A project's `plant.sim` file ([`text`]) lists the devices the machine
//! has: a heater that its program's `heat_on` drives toward a temperature,
//! a tank that fills and drains, a motor that ramps, a valve that travels
//! between its limit switches, a photo-eye that parts pass, the buttons
//! and switches an operator works, signals that run by themselves (ramp,
//! exponential, geometric, pulse, step, sine, random walk) and the
//! transmitters that scale a quantity to 4-20 mA, 0-10 V or counts.
//!
//! Each input a device drives behaves normally, abnormally (stuck,
//! drifting, noisy, over or under its range, slow, chattering) or is
//! absent: its device is lost, the input holds its last value as a box's
//! does while a link is down, and its `NAME_ok` input reads FALSE. A
//! `mode` line of the file says which; a requirement test or the popup
//! can change it while running ([`Plant::set_mode`]).
//!
//! [`Plant::step`] runs it: once a scan, before the program reads its
//! inputs, with the outputs the last scan left. No I/O, no clock of its
//! own and no dependency, so the browser's simulation and the box run the
//! same plant, and a seeded run is the same run every time.

pub mod plant;
pub mod text;

pub use plant::{Feed, Plant, Val};
pub use text::{Device, Key, Kind, Mode, ModeLine, Problem, Role, Spec, parse, set_device, set_mode};

/// The file's name in a project.
pub const FILE: &str = "plant.sim";

#[cfg(test)]
mod plant_tests;
#[cfg(test)]
mod text_tests;
