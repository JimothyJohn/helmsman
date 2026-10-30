//! I/O map validation: every inconsistency is reported, by name.

use std::collections::BTreeMap;

use helmsman_io::{Binding, Device, Dir, IoMap, MapError, Signal, Ty};

fn modbus(id: &str) -> Device {
    Device {
        id: id.into(),
        driver: "modbus-tcp".into(),
        endpoint: "192.168.3.50:502".into(),
        params: BTreeMap::new(),
    }
}

fn sig(name: &str, ty: Ty, dir: Dir, binding: Binding) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir,
        binding,
    }
}

fn on(device: &str, point: &str) -> Binding {
    Binding::Device {
        device: device.into(),
        point: point.into(),
    }
}

fn local(channel: &str) -> Binding {
    Binding::Local {
        channel: channel.into(),
    }
}

#[test]
fn a_sensible_map_validates() {
    let map = IoMap {
        devices: vec![modbus("rio")],
        signals: vec![
            sig("stack_green", Ty::Bool, Dir::Out, local("do:0")),
            sig("door_closed", Ty::Bool, Dir::In, local("di:2")),
            sig("conveyor_run", Ty::Bool, Dir::Out, on("rio", "coil:0")),
            sig("part_present", Ty::Bool, Dir::In, on("rio", "di:4")),
            sig("line_speed", Ty::Real, Dir::In, on("rio", "ir:10:f32")),
            sig("speed_setpoint", Ty::Int, Dir::Out, on("rio", "hr:0")),
            // Reading an output point back is fine.
            sig("conveyor_fb", Ty::Bool, Dir::In, on("rio", "coil:0")),
        ],
    };
    assert_eq!(map.validate(), Ok(()));
}

#[test]
fn every_problem_is_reported_not_just_the_first() {
    let map = IoMap {
        devices: vec![
            modbus("rio"),
            modbus("rio"),
            Device {
                driver: "profibus".into(),
                ..modbus("old")
            },
        ],
        signals: vec![
            sig("1st", Ty::Bool, Dir::In, local("di:0")),
            sig("a", Ty::Bool, Dir::In, local("di:0")),
            sig("a", Ty::Bool, Dir::In, local("di:1")),
            sig("ghost", Ty::Bool, Dir::In, on("nope", "di:0")),
            sig("typo", Ty::Bool, Dir::In, on("rio", "dj:0")),
            sig("legacy", Ty::Bool, Dir::In, on("old", "x")),
            sig("wrong_ty", Ty::Bool, Dir::In, on("rio", "hr:0")),
            sig("into_input", Ty::Bool, Dir::Out, on("rio", "di:0")),
            sig("lamp_a", Ty::Bool, Dir::Out, local("do:3")),
            sig("lamp_b", Ty::Bool, Dir::Out, local("do:3")),
            sig("out_on_in", Ty::Bool, Dir::Out, local("di:3")),
            sig("bad_chan", Ty::Bool, Dir::Out, local("ao:1")),
        ],
    };
    let errs = map.validate().unwrap_err();
    let want = [
        MapError::DuplicateDevice("rio".into()),
        MapError::BadName("1st".into()),
        MapError::DuplicateSignal("a".into()),
        MapError::UnknownDevice {
            signal: "ghost".into(),
            device: "nope".into(),
        },
        MapError::BadPoint {
            signal: "typo".into(),
            why: "bad address \"dj:0\"".into(),
        },
        MapError::BadPoint {
            signal: "legacy".into(),
            why: "unknown driver \"profibus\"".into(),
        },
        MapError::TypeMismatch {
            signal: "wrong_ty".into(),
            signal_ty: Ty::Bool,
            point_ty: Ty::Int,
        },
        MapError::NotWritable("into_input".into()),
        MapError::TwoWriters {
            a: "lamp_a".into(),
            b: "lamp_b".into(),
        },
        MapError::NotWritable("out_on_in".into()),
    ];
    for w in &want {
        assert!(errs.contains(w), "missing {w:?} in {errs:#?}");
    }
    assert!(
        errs.iter()
            .any(|e| matches!(e, MapError::BadPoint { signal, .. } if signal == "bad_chan")),
        "{errs:#?}"
    );
    assert_eq!(errs.len(), want.len() + 1, "{errs:#?}");
}

#[test]
fn two_writers_on_the_same_device_point_conflict_across_devices_only_when_same_device() {
    let map = IoMap {
        devices: vec![modbus("a"), modbus("b")],
        signals: vec![
            sig("x", Ty::Bool, Dir::Out, on("a", "coil:1")),
            sig("y", Ty::Bool, Dir::Out, on("b", "coil:1")),
            sig("z", Ty::Bool, Dir::Out, on("a", "coil:1")),
        ],
    };
    assert_eq!(
        map.validate(),
        Err(vec![MapError::TwoWriters {
            a: "x".into(),
            b: "z".into()
        }])
    );
}
