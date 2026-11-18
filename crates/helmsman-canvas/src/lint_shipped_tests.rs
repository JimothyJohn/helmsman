//! The ST lints (`helmsman_iec::lint`) over every program the page ships:
//! the playground's examples, the front-door machine and the pump-station
//! fixture. A shipped program holds no blocking finding (`st-lints`): one
//! that has to would carry an override pragma with its reason.

use helmsman_iec::lint::{Lint, lints};

use crate::playground::EXAMPLES;
use crate::test_programs::PUMP_STATION;

fn shipped() -> Vec<(String, &'static str)> {
    let mut all: Vec<(String, &'static str)> = EXAMPLES
        .iter()
        .map(|e| (format!("example {}", e.name), e.src))
        .collect();
    all.push(("front-door main.st".into(), crate::front::MAIN));
    for (name, text) in PUMP_STATION {
        if std::path::Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("st"))
        {
            all.push((format!("pump-station {name}"), text));
        }
    }
    all
}

fn show(l: &Lint) -> String {
    format!(
        "{}:{} {} {}",
        l.pos.line,
        l.pos.col,
        l.id,
        l.msg.chars().take(70).collect::<String>()
    )
}

#[test]
fn shipped_programs_hold_no_blocking_finding() {
    let mut bad = Vec::new();
    for (what, src) in shipped() {
        for l in lints(src) {
            eprintln!("{what}: {}", show(&l));
            if l.blocking {
                bad.push(format!("{what}: {}", show(&l)));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "blocking findings in shipped programs:\n{}",
        bad.join("\n")
    );
}
