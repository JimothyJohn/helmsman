//! The tank (`crate::front`), the front door's machine until the front door
//! went (Nick, 2026-11-27), stays a built-in example of the simulation: it
//! is listed with the examples, and opened in the editor it runs against
//! its own plant with its five tests green. Its proofs are checked without
//! a screen in `sim_projects_tests.rs`.

use super::{App, Harness};
use crate::front::{PLANT, PROJECT, TESTS};

#[test]
fn the_tank_is_listed_with_the_examples() {
    let h = Harness::new(App::sim());
    let v = h.app.sim.as_ref().expect("the simulation");
    let rows = v.proj.rows();
    assert!(rows.iter().any(|r| r.name == PROJECT && r.example()), "{rows:?}");
}

#[test]
fn the_tank_opens_running_against_its_plant_with_its_five_tests_green() {
    let mut h = Harness::new(App::sim());
    h.app.sim.as_mut().expect("the simulation").open_project(PROJECT);
    h.wait(1.0);
    let v = h.app.sim.as_mut().expect("the simulation");
    assert_eq!(v.title(), "tank");
    assert!(v.session.running, "running, as an example opens");
    assert_eq!(v.session.diags, Vec::new(), "the program compiles");
    assert!(v.session.plant.is_some(), "its plant is up: {PLANT}");
    assert_eq!(v.session.plant_problems, Vec::new());
    assert_eq!(
        v.session.plant_unbound(),
        Vec::new(),
        "the plant names only what the program has"
    );
    v.run_tests();
    assert_eq!(v.tests.rows.len(), TESTS.len());
    assert_eq!(v.tests.red(), Vec::<String>::new());
    h.header_is_up();
}
