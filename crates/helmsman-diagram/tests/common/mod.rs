#![allow(dead_code)]

use std::fmt::Write as _;

use helmsman_diagram::{Diagram, Module, NodeKind, Rect, lower};
use helmsman_iec::{ImageSizes, Program, compile};

pub const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 16 };

pub fn build(src: &str) -> (Program, Module) {
    let p = compile(src, SIZES).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let m = lower(src, &p.trace_map()).unwrap_or_else(|e| panic!("{e}\n{src}"));
    (p, m)
}

/// A PROGRAM `t` with `vars` declared and `body` as its body.
pub fn prog(vars: &str, body: &str) -> String {
    format!("PROGRAM t\nVAR {vars} END_VAR\n{body}\nEND_PROGRAM\n")
}

/// The program's diagram as text: one line per node (`#id kind label
/// [region] <- inputs`), then one per region (`R#id label gate=#node body=n`).
/// Structure only, no positions.
pub fn shape(d: &Diagram) -> String {
    let mut s = String::new();
    for n in &d.nodes {
        let kind = match &n.kind {
            NodeKind::Read { .. } => "read",
            NodeKind::Const { .. } => "const",
            NodeKind::Op { .. } => "op",
            NodeKind::Func { .. } => "func",
            NodeKind::Fb { .. } => "fb",
            NodeKind::Access { .. } => "access",
            NodeKind::Write { .. } => "write",
            NodeKind::For { .. } => "for",
            NodeKind::Jump => "jump",
        };
        let ins: Vec<String> = d
            .inputs(n.id)
            .map(|w| {
                let port = &d.node(w.from).outs[w.out];
                if port.is_empty() {
                    format!("#{}", w.from.0)
                } else {
                    format!("#{}.{port}", w.from.0)
                }
            })
            .collect();
        let region = n.region.map_or(String::new(), |r| format!(" [R{}]", r.0));
        write!(s, "#{} {kind} {}{region}", n.id.0, n.label).unwrap();
        if !ins.is_empty() {
            write!(s, " <- {}", ins.join(" ")).unwrap();
        }
        s.push('\n');
    }
    for r in &d.regions {
        let gate = r.gate.map_or("-".to_string(), |(n, _)| format!("#{}", n.0));
        writeln!(s, "R{} {} gate={gate} body={}", r.id.0, r.label, r.body.len()).unwrap();
    }
    s
}

pub fn program_shape(m: &Module) -> String {
    shape(&m.program().unwrap().diagram)
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

fn inside(a: Rect, outer: Rect) -> bool {
    a.x >= outer.x && a.y >= outer.y && a.right() <= outer.right() && a.bottom() <= outer.bottom()
}

/// The layout invariants every diagram keeps; shared with the property test.
pub fn check(d: &Diagram) {
    for (i, a) in d.nodes.iter().enumerate() {
        assert!(a.rect.w > 0.0 && a.rect.h > 0.0, "{a:?}");
        assert!(a.rect.x >= 0.0 && a.rect.y >= 0.0, "{a:?}");
        assert!(
            a.rect.right() <= d.size.0 + 0.01 && a.rect.bottom() <= d.size.1 + 0.01,
            "{a:?} outside {:?}",
            d.size
        );
        for b in &d.nodes[i + 1..] {
            assert!(!overlap(a.rect, b.rect), "overlap:\n{a:?}\n{b:?}");
        }
        if let Some(r) = a.region {
            assert!(
                inside(a.rect, d.region(r).rect),
                "{a:?} not inside {:?}",
                d.region(r)
            );
        }
    }
    for r in &d.regions {
        if let Some(p) = r.parent {
            assert!(inside(r.rect, d.region(p).rect), "{r:?} not inside its parent");
        }
        // A region's gate is left of it (when the gate is the statement's own:
        // `IF t.Q` is gated by the call above it).
        if let Some((g, _)) = r.gate
            && d.node(g).stmt == r.stmt
        {
            assert!(
                d.node(g).rect.right() <= r.rect.x,
                "gate of {r:?} not on its left"
            );
        }
    }
    // Within a statement part, data flows left to right.
    for w in &d.wires {
        let (a, b) = (d.node(w.from), d.node(w.to));
        if a.stmt == b.stmt && a.region == b.region {
            assert!(a.rect.right() <= b.rect.x, "wire {a:?} -> {b:?} goes backwards");
        }
    }
}
