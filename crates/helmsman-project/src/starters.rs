//! The worked machines and templates a project can start from, in one
//! place so the simulation and a box offer the same text. The sources are
//! real files under `starters/`: a machine is its `main.st` (controls),
//! `plant_model.st` (a simulation of the machine, deleted when real I/O is
//! wired), `io.map` and the shared blocks it uses, which are the same files
//! the templates hand out.

use std::collections::BTreeMap;

use crate::Files;

/// Which list a starter is under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A worked machine: read it, run it, copy it.
    Machine,
    /// A starting point meant to be copied and edited.
    Template,
}

/// A project to start from.
#[derive(Debug, Clone, Copy)]
pub struct Starter {
    /// The project's name.
    pub name: &'static str,
    /// What a person calls it.
    pub title: &'static str,
    /// One line on what it shows.
    pub about: &'static str,
    pub kind: Kind,
    /// File name and text.
    pub files: &'static [(&'static str, &'static str)],
}

impl Starter {
    /// The project's files.
    #[must_use]
    pub fn project(&self) -> Files {
        Files(
            self.files
                .iter()
                .map(|(n, t)| ((*n).to_string(), (*t).to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
    }
}

macro_rules! block {
    ($name:literal) => {
        (
            concat!($name, ".st"),
            include_str!(concat!("../starters/blocks/", $name, ".st")),
        )
    };
}

macro_rules! own {
    ($dir:literal, $file:literal) => {
        ($file, include_str!(concat!("../starters/", $dir, "/", $file)))
    };
}

/// Every starter: the machines, then the templates.
pub const STARTERS: &[Starter] = &[
    Starter {
        name: "conveyor-sorter",
        title: "Conveyor sorter",
        about: "Measures parts at a photo eye and diverts the long ones, with belt tracking and a jam alarm",
        kind: Kind::Machine,
        files: &[
            own!("conveyor-sorter", "main.st"),
            own!("conveyor-sorter", "plant_model.st"),
            own!("conveyor-sorter", "io.map"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("solenoid_valve"),
            block!("stack_light"),
        ],
    },
    Starter {
        name: "pump-station",
        title: "Pump station",
        about: "Two pumps on a wet well, duty and standby, run from a level transmitter",
        kind: Kind::Machine,
        files: &[
            own!("pump-station", "main.st"),
            own!("pump-station", "plant_model.st"),
            own!("pump-station", "io.map"),
            block!("analog_scale"),
            block!("duty_standby"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("stack_light"),
        ],
    },
    Starter {
        name: "batch-mixer",
        title: "Batch mixing tank",
        about: "Fill, heat while agitating, hold and drain, with interlocks on the heater",
        kind: Kind::Machine,
        files: &[
            own!("batch-mixer", "main.st"),
            own!("batch-mixer", "plant_model.st"),
            own!("batch-mixer", "io.map"),
            block!("analog_scale"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("solenoid_valve"),
            block!("stack_light"),
            block!("step_sequencer"),
        ],
    },
    Starter {
        name: "pick-and-place",
        title: "Pick and place",
        about: "A three-cylinder pneumatic arm with end switches and travel timeouts",
        kind: Kind::Machine,
        files: &[
            own!("pick-and-place", "main.st"),
            own!("pick-and-place", "plant_model.st"),
            own!("pick-and-place", "io.map"),
            block!("latched_alarm"),
            block!("solenoid_valve"),
            block!("stack_light"),
            block!("step_sequencer"),
        ],
    },
    Starter {
        name: "bottle-filler",
        title: "Bottle filler and capper",
        about: "An indexing conveyor with a timed fill head and a cap press",
        kind: Kind::Machine,
        files: &[
            own!("bottle-filler", "main.st"),
            own!("bottle-filler", "plant_model.st"),
            own!("bottle-filler", "io.map"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("solenoid_valve"),
            block!("stack_light"),
            block!("step_sequencer"),
        ],
    },
    Starter {
        name: "air-compressors",
        title: "Air compressor staging",
        about: "Three compressors staged on header pressure, with a rotating lead",
        kind: Kind::Machine,
        files: &[
            own!("air-compressors", "main.st"),
            own!("air-compressors", "plant_model.st"),
            own!("air-compressors", "io.map"),
            block!("analog_scale"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("stack_light"),
        ],
    },
    Starter {
        name: "air-handler",
        title: "Air handling unit",
        about: "Damper, fan with airflow proof, PID on the heating coil, freeze stat",
        kind: Kind::Machine,
        files: &[
            own!("air-handler", "main.st"),
            own!("air-handler", "plant_model.st"),
            own!("air-handler", "io.map"),
            block!("analog_scale"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("pid_loop"),
            block!("solenoid_valve"),
            block!("stack_light"),
        ],
    },
    Starter {
        name: "discrete-template",
        title: "Discrete machine",
        about: "A motor, a cylinder and a step sequence, ready to rename and extend",
        kind: Kind::Template,
        files: &[
            own!("discrete-template", "main.st"),
            own!("discrete-template", "plant_model.st"),
            own!("discrete-template", "io.map"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("solenoid_valve"),
            block!("stack_light"),
            block!("step_sequencer"),
        ],
    },
    Starter {
        name: "process-template",
        title: "Process loop",
        about: "A scaled measurement, a PID loop and two pumps sharing the duty",
        kind: Kind::Template,
        files: &[
            own!("process-template", "main.st"),
            own!("process-template", "plant_model.st"),
            own!("process-template", "io.map"),
            block!("analog_scale"),
            block!("duty_standby"),
            block!("latched_alarm"),
            block!("motor_starter"),
            block!("pid_loop"),
            block!("stack_light"),
        ],
    },
];

/// The starter called `name`.
#[must_use]
pub fn starter(name: &str) -> Option<&'static Starter> {
    STARTERS.iter().find(|s| s.name == name)
}
