//! The box's channels behind the driver contract.

use tcpethernet_io::local::Channels;
use tcpethernet_io::{Device, Driver, Error, Ty, Value, open};

#[test]
fn outputs_write_and_read_back_inputs_follow_the_panel() {
    let panel = Channels::new();
    let mut d = panel.clone();
    assert_eq!(
        d.read(&["do:0", "di:3"]).unwrap(),
        [Value::Bool(false), Value::Bool(false)],
        "everything starts off"
    );
    d.write("do:0", Value::Bool(true)).unwrap();
    panel.set_input(3, true);
    assert!(panel.output(0));
    assert!(!panel.output(1));
    assert_eq!(
        d.read(&["do:0", "di:3"]).unwrap(),
        [Value::Bool(true), Value::Bool(true)]
    );
}

#[test]
fn the_contract_refuses_what_it_should() {
    let mut d = Channels::new();
    assert!(matches!(
        d.write("di:0", Value::Bool(true)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(
        d.write("do:0", Value::Int(1)),
        Err(Error::Type {
            want: Ty::Bool,
            got: Ty::Int
        })
    ));
    for bad in [
        "", "do", "do:", "do:x", "do:-1", "do:65536", "ao:0", "coil:0", "do:0:1",
    ] {
        assert!(matches!(d.parse(bad), Err(Error::Address(_))), "{bad:?}");
        assert!(d.read(&["do:0", bad]).is_err(), "{bad:?}: all or nothing");
        assert!(d.write(bad, Value::Bool(true)).is_err(), "{bad:?}");
    }
    assert!(!Channels::new().output(0), "a refused write changes nothing");
}

#[test]
fn open_knows_modbus_and_nothing_else() {
    let dev = |driver: &str, params: &[(&str, &str)]| Device {
        id: "d".into(),
        driver: driver.into(),
        endpoint: "127.0.0.1:1".into(),
        params: params.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    };
    assert!(
        open(&dev("modbus-tcp", &[("unit", "3")])).is_ok(),
        "connects lazily"
    );
    assert!(matches!(
        open(&dev("modbus-tcp", &[("unit", "300")])),
        Err(Error::Address(_))
    ));
    assert!(matches!(
        open(&dev("profinet", &[])),
        Err(Error::UnknownDriver(_))
    ));
}
