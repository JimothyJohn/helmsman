//! The simulate-this-input popup, opened by a double-click on an input.

use helmsman_iec::Value;
use helmsman_sim::{Kind, Mode};

use super::{App, Harness};

const OVEN: &str = "\
PROGRAM oven
VAR_INPUT temp_c : REAL; temp_c_ok : BOOL; END_VAR
VAR_OUTPUT heat_on : BOOL; END_VAR
heat_on := temp_c < 180.0 AND temp_c_ok;
END_PROGRAM
";

#[test]
fn a_double_click_on_an_input_opens_the_popup_and_apply_writes_plant_sim() {
    let mut h = Harness::new(App::sim());
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("oven", None);
        v.proj.files.insert("main.st".into(), OVEN.into());
        v.proj.save();
        v.open_project("oven");
    }
    h.wait(0.2);
    let at = h.rect("input temp_c").center();
    h.click(at);
    h.click(at);
    h.frame(vec![]);
    assert!(h.has("sim popup"), "the popup is open");
    {
        let p = h.app.sim.as_ref().unwrap().sim_popup.as_ref().expect("popup");
        assert_eq!(p.kind, Some(Kind::Heater));
    }
    h.press("sim absent");
    h.press("sim apply");
    h.frame(vec![]);
    let v = h.app.sim.as_ref().unwrap();
    assert!(v.sim_popup.is_none(), "closed on Apply");
    assert_eq!(
        v.proj.files.get("plant.sim").map(String::as_str),
        Some("heater heater1 heat=heat_on temp=temp_c\nmode temp_c absent\n")
    );
    assert_eq!(
        v.session.plant.as_ref().map(|p| p.mode("temp_c")),
        Some(Mode::Absent)
    );
    h.wait(0.2);
    let v = h.app.sim.as_ref().unwrap();
    assert_eq!(v.session.value("temp_c_ok"), Some(Value::Bool(false)));
}
