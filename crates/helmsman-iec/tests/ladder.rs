//! The L5X ladder converter's contract: a hand-written export in RM014's
//! shape converts to Structured Text that compiles and runs as the ladder
//! would; anything outside the subset is refused as a whole, each problem
//! with its program, routine and rung; hostile files are refused without
//! harm. Behaviour over random rungs is `ladder_equiv.rs`.

use std::fmt::Write as _;
use std::time::Duration;

use helmsman_iec::ladder::{self, MAX_PROBLEMS, Refused};
use helmsman_iec::{Image, ImageSizes, Value};
use proptest::prelude::*;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/l5x/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// An export: controller tags, programs (name, tags, routines as
/// (name, type, rungs)), tasks (name, type, programs).
struct X<'a> {
    ctl_tags: &'a str,
    programs: Vec<(&'a str, &'a str, Vec<(&'a str, &'a str, Vec<&'a str>)>)>,
    tasks: Vec<(&'a str, &'a str, Vec<&'a str>)>,
}

impl X<'_> {
    fn xml(&self) -> String {
        let mut progs = String::new();
        for (name, tags, routines) in &self.programs {
            let main = routines.first().map_or("", |r| r.0);
            let _ = write!(
                progs,
                "<Program Name=\"{name}\" MainRoutineName=\"{main}\"><Tags>{tags}</Tags><Routines>"
            );
            for (rn, ty, rungs) in routines {
                let _ = write!(progs, "<Routine Name=\"{rn}\" Type=\"{ty}\">");
                if *ty == "RLL" {
                    progs.push_str("<RLLContent>");
                    for (k, t) in rungs.iter().enumerate() {
                        let _ = write!(
                            progs,
                            "<Rung Number=\"{k}\" Type=\"N\"><Text><![CDATA[{t}]]></Text></Rung>"
                        );
                    }
                    progs.push_str("</RLLContent>");
                } else if *ty == "ST" {
                    progs.push_str("<STContent><Line Number=\"0\"><![CDATA[x := 1;]]></Line></STContent>");
                }
                progs.push_str("</Routine>");
            }
            progs.push_str("</Routines></Program>");
        }
        let mut tasks = String::new();
        for (name, ty, ps) in &self.tasks {
            let _ = write!(
                tasks,
                "<Task Name=\"{name}\" Type=\"{ty}\" Rate=\"10\"><ScheduledPrograms>"
            );
            for p in ps {
                let _ = write!(tasks, "<ScheduledProgram Name=\"{p}\"/>");
            }
            tasks.push_str("</ScheduledPrograms></Task>");
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RSLogix5000Content SchemaRevision=\"1.0\" SoftwareRevision=\"33.00\" TargetName=\"C\" TargetType=\"Controller\">\n<Controller Use=\"Target\" Name=\"C\">\n<Tags>{}</Tags>\n<Programs>{progs}</Programs>\n<Tasks>{tasks}</Tasks>\n</Controller>\n</RSLogix5000Content>\n",
            self.ctl_tags
        )
    }
}

fn tag(name: &str, ty: &str) -> String {
    format!("<Tag Name=\"{name}\" TagType=\"Base\" DataType=\"{ty}\"/>")
}

/// One program `P` in task `T` with `tags` and one ladder routine.
fn one(tags: &str, rungs: &[&str]) -> String {
    X {
        ctl_tags: "",
        programs: vec![("P", tags, vec![("Main", "RLL", rungs.to_vec())])],
        tasks: vec![("T", "CONTINUOUS", vec!["P"])],
    }
    .xml()
}

fn refused(src: &str) -> Refused {
    match ladder::convert(src) {
        Ok(c) => panic!("converted:\n{}", c.st),
        Err(r) => r,
    }
}

fn has(r: &Refused, at: &str, msg: &str) {
    assert!(
        r.problems
            .iter()
            .any(|p| p.at.contains(at) && p.msg.contains(msg)),
        "no problem at {at:?} saying {msg:?} in: {r}"
    );
}

fn bools() -> String {
    ["A", "B", "C", "Q"].iter().map(|n| tag(n, "BOOL")).collect()
}

#[test]
fn the_fixture_converts_compiles_and_runs_as_ladder() {
    let c = ladder::convert(&fixture("ladder.L5X")).unwrap();
    assert_eq!(c.programs, ["MainTask"]);
    assert!(
        c.notes.iter().any(|n| n.contains("periodic every 10 ms")),
        "{:?}",
        c.notes
    );
    assert!(c.notes.iter().any(|n| n.contains("Log_tag")), "{:?}", c.notes);
    assert!(c.st.contains("VAR_INPUT"), "{}", c.st);
    let sizes = ImageSizes { i: 0, q: 0, m: 0 };
    let mut p = helmsman_iec::compile(&c.st, sizes).unwrap();
    let ports: Vec<String> = p.ports().into_iter().map(|p| p.name).collect();
    assert_eq!(ports, ["START", "STOP", "PART_EYE", "MOTOR"]);
    // Initial values from decorated data and pseudo-operands.
    assert_eq!(p.get("Delay.PRE"), Some(Value::Int(2000)));
    assert_eq!(p.get("Parts.PRE"), Some(Value::Int(3)));
    assert_eq!(p.get("Speeds[3]"), Some(Value::Real(-4.25)));
    assert_eq!(p.get("Gain"), Some(Value::Int(7)));

    let mut img = Image::new(sizes);
    let mut scan =
        |p: &mut helmsman_iec::Program, ms: u64| p.scan_at(&mut img, Duration::from_millis(ms)).unwrap();
    // Start seals in; the motor waits for the 2 s delay.
    p.set_input("Start", Value::Bool(true)).unwrap();
    scan(&mut p, 0);
    p.set_input("Start", Value::Bool(false)).unwrap();
    scan(&mut p, 10);
    assert_eq!(p.get("Running"), Some(Value::Bool(true)));
    assert_eq!(p.get("Motor"), Some(Value::Bool(false)));
    scan(&mut p, 1500);
    assert_eq!(p.get("Motor"), Some(Value::Bool(false)));
    scan(&mut p, 2010);
    assert_eq!(p.get("Motor"), Some(Value::Bool(true)));
    assert_eq!(
        p.get("Run_Lamp"),
        Some(Value::Bool(true)),
        "the alias Lamp is Run_Lamp"
    );
    // Three parts: the ONS counts edges, not scans; the third resets the
    // counter and runs the subroutine.
    let mut t = 2020;
    for _ in 0..3 {
        for eye in [true, true, false] {
            p.set_input("Part_Eye", Value::Bool(eye)).unwrap();
            scan(&mut p, t);
            t += 10;
        }
    }
    assert_eq!(p.get("Total"), Some(Value::Int(3)));
    assert_eq!(p.get("Parts.ACC"), Some(Value::Int(0)), "RES on DN");
    // Stats: CPT over REALs; MUL into an INT; LIM 10..20 on 3 is false.
    assert_eq!(
        p.get("Avg"),
        Some(Value::Real(f64::from((1.5f32 + 2.5 + -4.25) / 3.0)))
    );
    assert_eq!(p.get("Status"), Some(Value::Int(21)));
    assert_eq!(p.get("Log_tag[0]"), Some(Value::Int(3)));
    // Stop drops it all.
    p.set_input("Stop", Value::Bool(true)).unwrap();
    scan(&mut p, t);
    assert_eq!(p.get("Motor"), Some(Value::Bool(false)));
    assert_eq!(p.get("Delay.ACC"), Some(Value::Int(0)));
}

#[test]
fn the_inventory_fixture_is_refused_with_every_reason() {
    let r = refused(&fixture("line3.L5X"));
    has(&r, "routine MainRoutine, rung 1", "Local:1:I.Data.3");
    has(&r, "routine Calc", "Structured Text routine");
    has(&r, "routine Seq", "sequential function chart");
    assert_eq!(r.total, r.problems.len());
}

#[test]
fn unknown_instructions_are_refused_with_their_rung() {
    let r = refused(&one(
        &bools(),
        &[
            "XIC(A)OTE(Q);",
            "XIC(A)BTD(A,0,B,0,1);",
            "AFI()OTE(B);",
            "XIC(A)AOI_X(A);",
        ],
    ));
    has(
        &r,
        "program P, routine Main, rung 1",
        "BTD is not an instruction this converter translates",
    );
    has(&r, "rung 2", "AFI");
    has(&r, "rung 3", "AOI_X");
    assert_eq!(r.total, 3, "{r}");
}

#[test]
fn malformed_rungs_are_refused() {
    for bad in [
        "XIC(A",
        "XIC(A)OTE(Q)",
        "[XIC(A),XIC(B)OTE(Q);",
        "XIC(A)]OTE(Q);",
        "XIC A;",
        "XIC(A);OTE(Q);",
    ] {
        let r = refused(&one(&bools(), &[bad]));
        has(&r, "rung 0", "does not read as ladder");
    }
}

#[test]
fn operand_problems_are_refused_with_the_instruction() {
    let tags = format!(
        "{}{}{}{}{}<Tag Name=\"K\" TagType=\"Base\" DataType=\"DINT\" Constant=\"true\"/><Tag Name=\"Arr\" TagType=\"Base\" DataType=\"DINT\" Dimensions=\"4\"/>{}{}",
        bools(),
        tag("D", "DINT"),
        tag("R", "REAL"),
        tag("T1", "TIMER"),
        tag("M1", "UDT_Motor"),
        tag("L", "LINT"),
        tag("I", "DINT")
    );
    let rungs = [
        "XIC(Nope)OTE(Q);",
        "XIC(D)OTE(Q);",
        "OTE(T1);",
        "MOV(1,K);",
        "XIC(M1.Run)OTE(Q);",
        "MOV(Arr[I],D);",
        "MOV(Arr[4],D);",
        "MOV(L,D);",
        "TON(D,100,0);",
        "COP(Arr[0],R,1);",
        "COP(Arr[2],Arr[0],3);",
        "XIC(A)JSR(Sub,1,A);",
        "[XIC(A)JSR(Sub),XIC(B)];",
        "JSR(Sub)OTE(Q);",
        "CPT(D,D ** 2);",
        "CPT(D,ABS(D));",
        "ADD(1,2,3);",
        "XIC(D.32)OTE(Q);",
        "MOV(R.1,D);",
        "TON(T1,1.5,0);",
        "XIC(Local:1:I.Data.0)OTE(Q);",
    ];
    let r = refused(&one(&tags, &rungs));
    let at = |n: usize| format!("rung {n}");
    has(&r, &at(0), "Nope is not a tag");
    has(&r, &at(1), "is a DINT, not a BOOL");
    has(&r, &at(2), "is a TIMER, not a BOOL");
    has(&r, &at(3), "constant tag");
    has(&r, &at(4), "type UDT_Motor, which is not converted");
    has(&r, &at(5), "indirect addressing");
    has(&r, &at(6), "outside Arr[4]");
    has(&r, &at(7), "type LINT");
    has(&r, &at(8), "not a TIMER");
    has(&r, &at(9), "COP copies bytes between types");
    has(&r, &at(10), "runs past the end");
    has(&r, &at(11), "JSR with parameters");
    has(&r, &at(12), "last instruction");
    has(&r, &at(13), "last instruction");
    has(&r, &at(14), "** (power)");
    has(&r, &at(15), "ABS");
    has(&r, &at(16), "immediate where a tag is needed");
    has(&r, &at(17), "bit 32 of a DINT");
    has(&r, &at(18), "does not name a part of a REAL");
    has(&r, &at(19), "not an integer immediate");
    has(&r, &at(20), "I/O module tag");
    assert_eq!(r.total, rungs.len(), "{r}");
}

#[test]
fn routines_tasks_and_programs_outside_the_subset_are_refused() {
    let x = X {
        ctl_tags: "",
        programs: vec![
            (
                "P",
                "",
                vec![
                    ("Main", "RLL", vec!["JSR(Sub);"]),
                    ("Sub", "FBD", vec![]),
                    ("St", "ST", vec![]),
                ],
            ),
            ("Lonely", "", vec![("Main", "RLL", vec![])]),
            ("Ev", "", vec![("Main", "RLL", vec![])]),
        ],
        tasks: vec![("T", "CONTINUOUS", vec!["P"]), ("E", "EVENT", vec!["Ev"])],
    };
    let r = refused(&x.xml());
    has(&r, "routine Sub", "function block diagram");
    has(&r, "routine St", "Structured Text routine");
    has(&r, "program Lonely", "scheduled in no task");
    has(&r, "task E", "event tasks");
}

#[test]
fn jsr_recursion_and_calls_to_main_are_refused() {
    let x = X {
        ctl_tags: "",
        programs: vec![(
            "P",
            "",
            vec![
                ("Main", "RLL", vec!["JSR(A);"]),
                ("A", "RLL", vec!["JSR(B);"]),
                ("B", "RLL", vec!["JSR(A);", "JSR(Main);", "JSR(Nowhere);"]),
            ],
        )],
        tasks: vec![("T", "CONTINUOUS", vec!["P"])],
    };
    let r = refused(&x.xml());
    has(&r, "routine B, rung 0", "recursion (Main -> A -> B -> A)");
    has(&r, "routine B, rung 1", "main routine");
    has(&r, "routine B, rung 2", "Nowhere is not a routine");
}

#[test]
fn deep_jsr_chains_are_refused() {
    let names: Vec<String> = (0..30).map(|k| format!("R{k}")).collect();
    let calls: Vec<String> = (0..30).map(|k| format!("JSR(R{});", k + 1)).collect();
    let mut routines = vec![("Main", "RLL", vec!["JSR(R0);"])];
    for k in 0..30 {
        let rung = if k < 29 { calls[k].as_str() } else { "" };
        routines.push((
            names[k].as_str(),
            "RLL",
            if rung.is_empty() { vec![] } else { vec![rung] },
        ));
    }
    let x = X {
        ctl_tags: "",
        programs: vec![("P", "", routines)],
        tasks: vec![("T", "CONTINUOUS", vec!["P"])],
    };
    let r = refused(&x.xml());
    has(&r, "", "nest deeper than 25");
}

#[test]
fn inlining_that_explodes_is_refused() {
    // Each routine calls the next twice through two rungs: 2^20 copies.
    let mut routines = Vec::new();
    let calls: Vec<String> = (0..21).map(|k| format!("JSR(R{k});")).collect();
    let names: Vec<String> = (0..21).map(|k| format!("R{k}")).collect();
    routines.push(("Main", "RLL", vec![calls[0].as_str(), calls[0].as_str()]));
    for k in 0..20 {
        routines.push((
            names[k].as_str(),
            "RLL",
            vec![calls[k + 1].as_str(), calls[k + 1].as_str()],
        ));
    }
    routines.push((names[20].as_str(), "RLL", vec!["XIC(A)OTE(A);"]));
    let tags = tag("A", "BOOL");
    let x = X {
        ctl_tags: "",
        programs: vec![("P", &tags, routines)],
        tasks: vec![("T", "CONTINUOUS", vec!["P"])],
    };
    let r = refused(&x.xml());
    has(&r, "PROGRAM T", "MiB of Structured Text");
}

#[test]
fn programs_of_one_task_share_its_controller_tags() {
    let ctl = tag("Shared", "DINT");
    let pa = format!("{}{}", tag("Step", "DINT"), tag("Shared", "BOOL"));
    let pb = tag("Step", "DINT");
    let x = X {
        ctl_tags: &ctl,
        programs: vec![
            (
                "A",
                &pa,
                vec![("Main", "RLL", vec!["ADD(Step,1,Step)XIC(Shared)OTE(Shared);"])],
            ),
            (
                "B",
                &pb,
                vec![("Main", "RLL", vec!["ADD(Shared,1,Shared)MOV(Shared,Step);"])],
            ),
        ],
        tasks: vec![("T", "CONTINUOUS", vec!["A", "B"])],
    };
    let c = ladder::convert(&x.xml()).unwrap();
    // A's Shared shadows the controller's; both programs' Step coexist.
    for decl in ["Shared : DINT", "Step : DINT", "B_Step : DINT", "A_Shared : BOOL"] {
        assert!(c.st.contains(decl), "{decl} in\n{}", c.st);
    }
    let sizes = ImageSizes { i: 0, q: 0, m: 0 };
    let mut p = helmsman_iec::compile(&c.st, sizes).unwrap();
    let mut img = Image::new(sizes);
    for ms in 0..3 {
        p.scan_at(&mut img, Duration::from_millis(ms)).unwrap();
    }
    assert_eq!(p.get("Step"), Some(Value::Int(3)));
    assert_eq!(p.get("Shared"), Some(Value::Int(3)));
    assert_eq!(p.get("B_Step"), Some(Value::Int(3)));
}

#[test]
fn a_controller_tag_two_tasks_use_is_refused() {
    let ctl = tag("Shared", "BOOL");
    let x = X {
        ctl_tags: &ctl,
        programs: vec![
            ("A", "", vec![("Main", "RLL", vec!["OTE(Shared);"])]),
            ("B", "", vec![("Main", "RLL", vec!["XIC(Shared)OTU(Shared);"])]),
        ],
        tasks: vec![("Fast", "PERIODIC", vec!["A"]), ("Slow", "PERIODIC", vec!["B"])],
    };
    let r = refused(&x.xml());
    has(&r, "controller tag Shared", "used by tasks Fast and Slow");
    // Without the shared tag the two tasks are two PROGRAMs.
    let x = X {
        ctl_tags: "",
        programs: vec![
            ("A", "", vec![("Main", "RLL", vec![])]),
            ("B", "", vec![("Main", "RLL", vec![])]),
        ],
        tasks: vec![("Fast", "PERIODIC", vec!["A"]), ("Slow", "PERIODIC", vec!["B"])],
    };
    let c = ladder::convert(&x.xml()).unwrap();
    assert_eq!(c.programs, ["Fast", "Slow"]);
}

#[test]
fn names_structured_text_keeps_are_renamed_or_refused() {
    let tags = format!(
        "{}{}{}",
        tag("Max", "DINT"),
        tag("Limit", "DINT"),
        tag("LOGIX_X", "BOOL")
    );
    let r = refused(&one(&tags, &["MOV(Max,Limit);"]));
    has(&r, "tag LOGIX_X", "LOGIX_");
    let tags = format!("{}{}", tag("Max", "DINT"), tag("Limit", "DINT"));
    let c = ladder::convert(&one(&tags, &["MOV(Max,Limit);"])).unwrap();
    assert!(c.st.contains("Limit_tag := Max_tag"), "{}", c.st);
}

#[test]
fn conflicting_pseudo_operands_are_refused() {
    let r = refused(&one(&tag("T1", "TIMER"), &["TON(T1,100,0);", "TON(T1,200,0);"]));
    has(&r, "rung 1", "differ from those at");
}

#[test]
fn hostile_files_are_refused_without_harm() {
    // Entity expansion and external entities: no DTD is read at all.
    let laughs = "<?xml version=\"1.0\"?><!DOCTYPE l [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]><RSLogix5000Content>&b;</RSLogix5000Content>";
    has(&refused(laughs), "line 1", "DOCTYPE");
    let xxe = "<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY e SYSTEM \"file:///etc/passwd\">]><RSLogix5000Content><Controller Name=\"C\">&e;</Controller></RSLogix5000Content>";
    has(&refused(xxe), "line 1", "DOCTYPE");
    let inner_entity = one("", &[]).replace("<Tags></Tags>", "<Tags>&secret;</Tags>");
    has(&refused(&inner_entity), "", "unknown entity");
    // Deep nesting and huge files.
    let deep = format!("<RSLogix5000Content>{}", "<a>".repeat(100_000));
    has(&refused(&deep), "", "nested deeper");
    let huge = format!(
        "<RSLogix5000Content>{}</RSLogix5000Content>",
        " ".repeat(helmsman_iec::l5x::MAX_L5X)
    );
    has(&refused(&huge), "", "larger than");
    // Rungs that try to escape the comment they are copied into cannot
    // parse as ladder, so they never reach it.
    let r = refused(&one(&bools(), &["XIC(A) *) END_PROGRAM PROGRAM evil (* OTE(Q);"]));
    has(&r, "rung 0", "does not read as ladder");
    // A flood of problems is capped and counted.
    let many: Vec<String> = (0..500).map(|k| format!("XIC(Nope{k})OTE(Q);")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let r = refused(&one(&bools(), &refs));
    assert_eq!(r.problems.len(), MAX_PROBLEMS);
    assert_eq!(r.total, 500);
    assert!(
        r.to_string().ends_with("; and 300 more"),
        "{}",
        &r.to_string()[r.to_string().len() - 40..]
    );
}

#[test]
fn a_program_export_without_tasks_is_one_program() {
    let tags = bools();
    let x = X {
        ctl_tags: "",
        programs: vec![("P", &tags, vec![("Main", "RLL", vec!["XIC(A)OTE(Q);"])])],
        tasks: vec![],
    };
    let c = ladder::convert(&x.xml()).unwrap();
    assert_eq!(c.programs, ["C"]);
}

fn rung_text() -> impl Strategy<Value = String> {
    let atom = prop::sample::select(vec![
        "XIC(A)",
        "XIO(B)",
        "OTE(Q)",
        "ONS(C)",
        "TON(T,?,?)",
        "CPT(D,D+1)",
        "MOV(D,D)",
        "[",
        "]",
        ",",
        ";",
        "(",
        ")",
        "JSR(P)",
        "XIC(",
        "16#",
        "*)",
        " ",
        "é",
        "\u{0}",
        "[[",
        "LIM(1,D,2)",
        "COP(D,D,1)",
        "D.",
        "D[",
    ]);
    prop::collection::vec(atom, 0..40).prop_map(|v| v.concat())
}

proptest! {
    /// Whatever the rung text, the converter answers (converted or
    /// refused) without panicking, and what it converts compiles.
    #[test]
    fn arbitrary_rungs_never_panic(t in rung_text(), u in "\\PC{0,80}") {
        let tags = format!("{}{}{}", bools(), tag("D", "DINT"), tag("T", "TIMER"));
        for text in [t.as_str(), u.as_str()] {
            let src = one(&tags, &[text]);
            if let Ok(c) = ladder::convert(&src) {
                let sizes = ImageSizes { i: 0, q: 0, m: 0 };
                prop_assert!(helmsman_iec::compile(&c.st, sizes).is_ok());
            }
        }
    }
}
