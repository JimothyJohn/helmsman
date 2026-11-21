//! An answer given as the lines that change (card agent-edits-not-files):
//! what [`expand`] hands on is the whole file with exactly those lines
//! changed, and a block that does not name one place in the file changes
//! nothing.

use crate::edits::{Edit, MAX_EDITS, apply, expand, is_edit, parse};
use crate::scope::{Files, Scope};
use crate::verify;
use helmsman_iec::spec::Settings;

const LATCH: &str = "PROGRAM latch
VAR_INPUT
  start : BOOL;
  stop : BOOL;
END_VAR
VAR_OUTPUT
  motor : BOOL;
END_VAR
(* Start turns the motor on and it stays on; stop turns it off. *)
motor := (start OR motor) AND NOT stop;
END_PROGRAM
";

const ESTOP_TEST: &str = "(* With estop held, start does not run the motor. *)
PROGRAM test_estop_holds
VAR_INPUT
  motor : BOOL;
END_VAR
VAR_OUTPUT
  start : BOOL;
  estop : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  off_while_held : ASSERT;
END_VAR
n := n + 1;
estop := TRUE;
start := n >= 2 AND n <= 5;
IF n = 20 THEN
  off_while_held(COND := NOT motor);
  done := TRUE;
END_IF;
END_PROGRAM";

fn project() -> Files {
    Files::from([("main.st".to_string(), LATCH.to_string())])
}

fn file() -> Scope {
    Scope::File("main.st".into())
}

fn edit(was: &str, now: &str) -> String {
    format!("```st edit\n{was}\n=======\n{now}\n```")
}

/// The estop change as two edits and a scenario.
fn estop_answer() -> String {
    format!(
        "Why: the motor stays off while estop is on.\n{}\n{}\n```st scenario\n{ESTOP_TEST}\n```",
        edit("  stop : BOOL;", "  stop : BOOL;\n  estop : BOOL;"),
        edit(
            "motor := (start OR motor) AND NOT stop;",
            "motor := (start OR motor) AND NOT stop AND NOT estop;"
        ),
    )
}

/// The file `answer` makes, as the checks will read it.
fn whole(answer: &str) -> String {
    let files = project();
    let expanded = expand(&file(), &files, answer).expect("the edits apply");
    crate::scope::apply(&file(), &files, &expanded).expect("one whole file")["main.st"].clone()
}

#[test]
fn edits_become_the_whole_file_with_only_those_lines_changed() {
    let got = whole(&estop_answer());
    let want = LATCH
        .replace("  stop : BOOL;\n", "  stop : BOOL;\n  estop : BOOL;\n")
        .replace("AND NOT stop;", "AND NOT stop AND NOT estop;");
    assert_eq!(got, want);
    // Every line the edits did not name is there as it was, in order.
    let kept: Vec<&str> = LATCH.lines().filter(|l| !l.starts_with("motor :=")).collect();
    let mut rest = got.lines();
    for line in kept {
        assert!(rest.any(|l| l == line), "{line:?} went missing or moved");
    }
}

#[test]
fn the_expanded_answer_passes_the_same_checks_a_whole_file_would() {
    let files = project();
    let answer = estop_answer();
    let expanded = expand(&file(), &files, &answer).unwrap();
    let check = verify::check(
        &file(),
        &files,
        &expanded,
        Settings::new(crate::playground::SIZES),
    );
    assert!(check.green(), "{:?}", check.problems());
    assert_eq!(check.scenarios.len(), 1, "the scenario came through as it was");
    // The why line and the scenario block are untouched; the edit blocks are gone.
    assert!(expanded.starts_with("Why: the motor stays off while estop is on."));
    assert!(expanded.contains("```st scenario\n"));
    assert!(!expanded.contains("======="));
    assert_eq!(expanded.matches("```st\n").count(), 1);
}

#[test]
fn an_answer_without_an_edit_block_is_left_exactly_as_it_came() {
    let files = project();
    for answer in [
        "The motor already does that.".to_string(),
        format!("Why: nothing.\n```st\n{LATCH}```\n```st scenario\n{ESTOP_TEST}\n```"),
        // An `edit` word in prose or in a scenario's name is not an edit block.
        "I would edit the latch:\n```st scenario replaces test_edit\nPROGRAM test_edit\nEND_PROGRAM\n```"
            .to_string(),
        String::new(),
    ] {
        assert_eq!(expand(&file(), &files, &answer).unwrap(), answer);
    }
    // Cut off inside a block: left for the checks, which say it was cut off.
    let cut = "Why: x.\n```st edit\nmotor := (start OR motor) AND NOT stop;\n=======\nmotor :=";
    assert_eq!(expand(&file(), &files, cut).unwrap(), cut);
}

#[test]
fn lines_that_are_not_in_the_file_change_nothing_and_say_which_block() {
    let files = project();
    let answer = format!(
        "Why: x.\n{}\n{}",
        edit("  stop : BOOL;", "  stop : BOOL;\n  estop : BOOL;"),
        edit(
            "motor := start AND NOT stop;",
            "motor := start AND NOT stop AND NOT estop;"
        ),
    );
    let why = expand(&file(), &files, &answer).unwrap_err();
    assert!(why.starts_with("Edit block 2 of 2"), "{why}");
    assert!(why.contains("`motor := start AND NOT stop;`"), "{why}");
    assert!(why.contains("not in the file"), "{why}");
}

#[test]
fn lines_that_stand_in_the_file_twice_are_refused_never_guessed() {
    let files = project();
    // `END_VAR` closes both declaration blocks.
    let why = expand(&file(), &files, &edit("END_VAR", "  estop : BOOL;\nEND_VAR")).unwrap_err();
    assert!(why.contains("2 times"), "{why}");
    // With a neighbouring line it names one place.
    let got = whole(&edit(
        "  stop : BOOL;\nEND_VAR",
        "  stop : BOOL;\n  estop : BOOL;\nEND_VAR",
    ));
    assert!(
        got.contains("  stop : BOOL;\n  estop : BOOL;\nEND_VAR\nVAR_OUTPUT"),
        "{got}"
    );
}

#[test]
fn a_later_block_sees_the_file_as_the_earlier_ones_left_it() {
    // The second block names a line the first one wrote.
    let got = whole(&format!(
        "{}\n{}",
        edit("  motor : BOOL;", "  motor : BOOL;\n  horn : BOOL;"),
        edit("  horn : BOOL;", "  horn : BOOL;\n  lamp : BOOL;"),
    ));
    assert!(
        got.contains("  motor : BOOL;\n  horn : BOOL;\n  lamp : BOOL;\nEND_VAR"),
        "{got}"
    );
    // And a block that names what an earlier one removed fails.
    let files = project();
    let gone = format!(
        "{}\n{}",
        edit("  stop : BOOL;\nEND_VAR", "END_VAR"),
        edit("  stop : BOOL;", "  halt : BOOL;"),
    );
    assert!(
        expand(&file(), &files, &gone)
            .unwrap_err()
            .starts_with("Edit block 2 of 2")
    );
}

#[test]
fn white_space_at_a_line_end_and_wrong_indentation_still_find_the_line() {
    let got = whole(&edit(
        "motor := (start OR motor) AND NOT stop;   ",
        "motor := start AND NOT stop;",
    ));
    assert!(
        got.contains("\nmotor := start AND NOT stop;\nEND_PROGRAM"),
        "{got}"
    );
    let got = whole(&edit("stop : BOOL;", "  halt : BOOL;"));
    assert!(got.contains("  start : BOOL;\n  halt : BOOL;\nEND_VAR"), "{got}");
    // Wrong indentation never makes two places out of one exact match.
    let twice =
        "PROGRAM p\nVAR\n  a : BOOL;\nEND_VAR\nIF a THEN\n  a := FALSE;\nEND_IF;\na := FALSE;\nEND_PROGRAM\n";
    let out = apply(
        twice,
        &[Edit {
            was: vec!["a := FALSE;".into()],
            now: vec!["a := TRUE;".into()],
        }],
    )
    .unwrap();
    assert!(out.contains("  a := FALSE;\nEND_IF;\na := TRUE;\n"), "{out}");
}

#[test]
fn lines_can_be_removed_and_a_crlf_answer_reads_the_same() {
    let got = whole(&edit(
        "(* Start turns the motor on and it stays on; stop turns it off. *)\nmotor := (start OR motor) AND NOT stop;",
        "motor := (start OR motor) AND NOT stop;",
    ));
    assert!(!got.contains("(*"), "{got}");
    let crlf = estop_answer().replace('\n', "\r\n");
    assert_eq!(whole(&crlf), whole(&estop_answer()));
}

#[test]
fn a_malformed_block_is_refused_with_what_it_lacks() {
    let files = project();
    for (answer, says) in [
        (
            "```st edit\nmotor := (start OR motor) AND NOT stop;\n```",
            "exactly one line of seven equals",
        ),
        (
            "```st edit\na\n=======\nb\n=======\nc\n```",
            "exactly one line of seven equals",
        ),
        ("```st edit\n=======\n  estop : BOOL;\n```", "no lines above"),
        (
            "```st edit\n\n   \n=======\n  estop : BOOL;\n```",
            "no lines above",
        ),
        // Fewer than seven equals signs is code, not a divider.
        (
            "```st edit\na\n======\nb\n```",
            "exactly one line of seven equals",
        ),
    ] {
        let why = expand(&file(), &files, answer).unwrap_err();
        assert!(why.contains(says), "{answer:?}: {why}");
    }
    assert!(
        parse("a\n=======\n").unwrap().now.is_empty(),
        "nothing below removes the lines"
    );
}

#[test]
fn edits_beside_a_whole_file_or_without_a_file_are_refused() {
    let files = project();
    let both = format!("{}\n```st\n{LATCH}```", edit("  stop : BOOL;", "  halt : BOOL;"));
    assert!(
        expand(&file(), &files, &both)
            .unwrap_err()
            .contains("one form or the other")
    );
    let e = edit("  stop : BOOL;", "  halt : BOOL;");
    assert!(
        expand(&Scope::NewProject, &files, &e)
            .unwrap_err()
            .contains("no file to edit yet")
    );
    let missing = Scope::File("gone.st".into());
    assert!(
        expand(&missing, &files, &e)
            .unwrap_err()
            .contains("no file to edit yet")
    );
    let many = vec![e; MAX_EDITS + 1].join("\n");
    assert!(
        expand(&file(), &files, &many)
            .unwrap_err()
            .contains("edit blocks")
    );
}

#[test]
fn an_edit_outside_the_selected_lines_is_still_refused_by_the_scope() {
    // Lines 10..=10 is the assignment; the edit touches a declaration.
    let files = project();
    let scope = Scope::Lines {
        file: "main.st".into(),
        from: 10,
        to: 10,
    };
    let inside = expand(
        &scope,
        &files,
        &edit(
            "motor := (start OR motor) AND NOT stop;",
            "motor := start AND NOT stop;",
        ),
    )
    .unwrap();
    assert!(crate::scope::apply(&scope, &files, &inside).is_ok());
    let outside = expand(&scope, &files, &edit("  stop : BOOL;", "  halt : BOOL;")).unwrap();
    assert!(crate::scope::apply(&scope, &files, &outside).is_err());
}

#[test]
fn only_an_edit_fence_is_an_edit() {
    assert!(is_edit("st edit"));
    assert!(is_edit("ST Edit"));
    assert!(is_edit("edit"));
    assert!(!is_edit("st"));
    assert!(!is_edit("st scenario"));
    assert!(!is_edit("st scenario replaces test_edit"));
    assert!(!is_edit("st edited"));
}

#[test]
fn text_inside_a_block_is_never_read_as_markup_or_instructions() {
    // A divider look-alike inside a comment line is code unless the whole
    // line is equals signs; a fence inside the block ends it, as in any
    // other answer.
    let got = whole(&edit(
        "(* Start turns the motor on and it stays on; stop turns it off. *)",
        "(* ======= not a divider: latch ======= *)",
    ));
    assert!(
        got.contains("(* ======= not a divider: latch ======= *)\nmotor :="),
        "{got}"
    );
}

/// A retry asks an agent that answered with edit blocks for edit blocks
/// again, and one that sent the whole file for the whole file.
#[test]
fn a_retry_asks_again_in_the_form_the_answer_came_in() {
    use crate::edits::{has_edits, retry_words};
    use crate::verify::{Check, Next, next};
    let red = Check {
        code: true,
        failures: vec!["The code does not compile: 3:1: x".into()],
        ..Check::default()
    };
    let Next::Retry(text) = next(&red, 0, false) else {
        panic!("a red answer goes back")
    };
    assert!(text.contains("the whole file with the fix made in it"));
    let for_edits = retry_words(&text);
    assert!(!for_edits.contains("whole file"), "{for_edits}");
    assert!(for_edits.contains("```st edit blocks copied from the file as it was sent to you"));
    assert!(
        for_edits.contains("The code does not compile"),
        "what failed is still said"
    );
    let bare = Check {
        code: true,
        ..Check::default()
    };
    if let Next::Retry(text) = next(&bare, 0, false) {
        assert!(retry_words(&text).contains("the same ```st edit blocks,"));
    }
    assert!(has_edits(&estop_answer()));
    assert!(!has_edits(&format!("```st\n{LATCH}```")));
    assert!(!has_edits(
        "I would edit it.\n```st scenario\nPROGRAM test_edit\nEND_PROGRAM\n```"
    ));
}
