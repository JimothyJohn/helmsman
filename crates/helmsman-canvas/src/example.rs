//! The project a fresh canvas opens with: a start/stop motor whose run
//! command is debounced, counted and blinked, built from blocks three deep.

use helmsman_graph::{Kind, Node, NodeId, Project, Scope, Ty, Wire};

fn add(p: &mut Project, s: Scope, kind: Kind, x: f32, y: f32, label: &str) -> NodeId {
    let mut n = Node::new(kind, [x, y]);
    n.label = label.into();
    p.add_node(s, n).expect("example nodes are valid")
}

fn wire(p: &mut Project, s: Scope, from: NodeId, from_port: u32, to: NodeId, to_port: u32) {
    p.connect(
        s,
        Wire {
            from,
            from_port,
            to,
            to_port,
        },
    )
    .expect("example wires are valid");
}

fn pin_in(p: &mut Project, s: Scope, name: &str, x: f32, y: f32) -> NodeId {
    add(
        p,
        s,
        Kind::PinIn {
            name: name.into(),
            ty: Ty::Bool,
        },
        x,
        y,
        "",
    )
}

fn pin_out(p: &mut Project, s: Scope, name: &str, ty: Ty, x: f32, y: f32) -> NodeId {
    add(
        p,
        s,
        Kind::PinOut {
            name: name.into(),
            ty,
        },
        x,
        y,
        "",
    )
}

pub fn project() -> Project {
    let mut p = Project::default();

    // DEBOUNCE: IN must stay on for 150 ms.
    let debounce = p.new_block("DEBOUNCE");
    let s = Scope::Block(debounce);
    let i = pin_in(&mut p, s, "IN", 0.0, 40.0);
    let t = add(&mut p, s, Kind::Ton { preset_ms: 150.0 }, 180.0, 20.0, "");
    let o = pin_out(&mut p, s, "Q", Ty::Bool, 400.0, 40.0);
    wire(&mut p, s, i, 0, t, 0);
    wire(&mut p, s, t, 0, o, 0);

    // SEAL_IN: classic start/stop holding circuit, START debounced.
    let seal = p.new_block("SEAL_IN");
    let s = Scope::Block(seal);
    let start = pin_in(&mut p, s, "START", 0.0, 0.0);
    let stop = pin_in(&mut p, s, "STOP", 0.0, 140.0);
    let db = add(&mut p, s, Kind::Block(debounce), 160.0, -20.0, "");
    let or = add(&mut p, s, Kind::Or { inputs: 2 }, 400.0, 0.0, "");
    let not = add(&mut p, s, Kind::Not, 400.0, 140.0, "");
    let and = add(&mut p, s, Kind::And { inputs: 2 }, 620.0, 40.0, "hold");
    let q = pin_out(&mut p, s, "RUN", Ty::Bool, 840.0, 60.0);
    wire(&mut p, s, start, 0, db, i.0);
    wire(&mut p, s, db, o.0, or, 0);
    wire(&mut p, s, and, 0, or, 1);
    wire(&mut p, s, stop, 0, not, 0);
    wire(&mut p, s, or, 0, and, 0);
    wire(&mut p, s, not, 0, and, 1);
    wire(&mut p, s, and, 0, q, 0);

    // MOTOR: seal-in, a start counter, and a blinking "running" light.
    let motor = p.new_block("MOTOR");
    let s = Scope::Block(motor);
    let m_start = pin_in(&mut p, s, "START", 0.0, 0.0);
    let m_stop = pin_in(&mut p, s, "STOP", 0.0, 80.0);
    let m_reset = pin_in(&mut p, s, "RESET", 0.0, 260.0);
    let sl = add(&mut p, s, Kind::Block(seal), 180.0, 0.0, "");
    let clock = add(&mut p, s, Kind::Clock { period_ms: 500.0 }, 180.0, 180.0, "blink");
    let blink = add(&mut p, s, Kind::And { inputs: 2 }, 440.0, 120.0, "");
    let edge = add(&mut p, s, Kind::RTrig, 440.0, 220.0, "");
    let ctu = add(&mut p, s, Kind::Ctu { preset: 5.0 }, 640.0, 220.0, "starts");
    let run = pin_out(&mut p, s, "RUN", Ty::Bool, 680.0, 0.0);
    let lamp = pin_out(&mut p, s, "BLINK", Ty::Bool, 680.0, 120.0);
    let svc = pin_out(&mut p, s, "SERVICE", Ty::Bool, 880.0, 220.0);
    let count = pin_out(&mut p, s, "STARTS", Ty::Num, 880.0, 300.0);
    wire(&mut p, s, m_start, 0, sl, start.0);
    wire(&mut p, s, m_stop, 0, sl, stop.0);
    wire(&mut p, s, sl, q.0, run, 0);
    wire(&mut p, s, sl, q.0, blink, 0);
    wire(&mut p, s, clock, 0, blink, 1);
    wire(&mut p, s, blink, 0, lamp, 0);
    wire(&mut p, s, sl, q.0, edge, 0);
    wire(&mut p, s, edge, 0, ctu, 0);
    wire(&mut p, s, m_reset, 0, ctu, 1);
    wire(&mut p, s, ctu, 0, svc, 0);
    wire(&mut p, s, ctu, 1, count, 0);

    // Root: two motors from the same definition, each with its own state.
    let r = Scope::Root;
    for (k, name) in ["Conveyor", "Pump"].into_iter().enumerate() {
        let y = k as f32 * 320.0;
        let b_start = add(&mut p, r, Kind::Button, 0.0, y, &format!("{name} start"));
        let b_stop = add(&mut p, r, Kind::Button, 0.0, y + 90.0, &format!("{name} stop"));
        let b_reset = add(&mut p, r, Kind::Button, 0.0, y + 180.0, "reset count");
        let m = add(&mut p, r, Kind::Block(motor), 240.0, y + 20.0, name);
        let l_run = add(&mut p, r, Kind::Lamp, 520.0, y - 30.0, "running");
        let l_blink = add(&mut p, r, Kind::Lamp, 520.0, y + 80.0, "beacon");
        let l_svc = add(&mut p, r, Kind::Lamp, 720.0, y - 30.0, "service due");
        let d = add(&mut p, r, Kind::Display, 720.0, y + 80.0, "starts");
        wire(&mut p, r, b_start, 0, m, m_start.0);
        wire(&mut p, r, b_stop, 0, m, m_stop.0);
        wire(&mut p, r, b_reset, 0, m, m_reset.0);
        wire(&mut p, r, m, run.0, l_run, 0);
        wire(&mut p, r, m, lamp.0, l_blink, 0);
        wire(&mut p, r, m, svc.0, l_svc, 0);
        wire(&mut p, r, m, count.0, d, 0);
    }
    add(
        &mut p,
        r,
        Kind::Note {
            text: "Hold a start button (150 ms debounce).\nDouble-click a motor to go inside;\nMOTOR > SEAL_IN > DEBOUNCE.".into(),
        },
        900.0,
        -10.0,
        "",
    );
    p
}

#[cfg(test)]
mod tests {
    use helmsman_graph::{Kind, Sim, Value};

    /// The example must build (every `expect` above) and behave: holding a
    /// start button past the debounce latches that motor and only that one.
    #[test]
    fn example_runs() {
        let p = super::project();
        let buttons: Vec<_> = p
            .root
            .nodes
            .iter()
            .filter(|(_, n)| n.kind == Kind::Button)
            .map(|(id, _)| *id)
            .collect();
        let lamps: Vec<_> = p
            .root
            .nodes
            .iter()
            .filter(|(_, n)| n.label == "running")
            .map(|(id, _)| *id)
            .collect();
        let lit = |sim: &Sim, i: usize| {
            let w = p.root.wire_into(lamps[i], 0).unwrap();
            sim.root.outs.get(&(w.from, w.from_port)) == Some(&Value::Bool(true))
        };
        let mut sim = Sim::default();
        sim.held.insert(vec![buttons[0]]);
        for _ in 0..20 {
            sim.scan(&p, 10.0);
        }
        sim.held.clear();
        for _ in 0..5 {
            sim.scan(&p, 10.0);
        }
        assert!(lit(&sim, 0), "conveyor latched on");
        assert!(!lit(&sim, 1), "pump untouched");
    }
}
