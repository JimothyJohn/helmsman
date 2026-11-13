# Investigation: awesome-structured-text against Helmsman

Source: https://github.com/myutzy/awesome-structured-text (README read
2026-11-07, 97 lines, about 45 links). Repo facts below (license, language,
last push) come from `gh api repos/...` on the same day. Helmsman facts come
from `crates/helmsman-iec` and docs/ARCHITECTURE.md at dev 5b83f94.

This file is input for a discussion. It is not a plan and it does not add
cards to TODO.md.

## The short version

Helmsman's bet is that the agent writes the ST and the engineer validates
it. The quality of what ships therefore depends on three things:
**the feedback the agent gets** (checker, tests, oracle), **the language it
is allowed to write**, and **the code it can reuse**. The list touches all
three. Ranked by how much each would change the product:

1. **PLCopen coding-guideline lints in the checker** (from iec-checker).
   The agent's retry loop (`verify::check`) only sees compile errors and
   red tests today. Real lint findings in that loop catch the bugs that
   compile and pass a thin scenario: a timer called inside an `IF`, an
   output written in two places, a variable read before it is set. This is
   the cheapest large win on the list.
2. **A real-world ST corpus as a conformance metric** (OSCAT,
   UniTest, structured-text-utilities, the test suites of ruSTy, matiec,
   blark and iec-checker). It turns "how much of IEC 61131-3 do we
   accept" into one number we can track, as `plcopen_corpus.rs` already
   does for PLCopen XML.
3. **Lift the dialect's biggest gaps: STRING, VAR_IN_OUT, function
   outputs, TOD/DATE.** The playground prompt has to tell the model "do not
   use" these, and the model knows ST from a corpus full of them. Each gap
   is a place where it writes code that does not compile and burns a retry,
   and a place where imported customer code is refused.
4. **A process-control block library** (PID, ramp, hysteresis, scaling,
   filters, in the shape OSCAT uses). Today `library.st` has the IEC
   timers, counters, edges, bistables, `BLINK_PATTERN` and `ASSERT`.
   Without more, the agent writes a new PID every time, and nobody has
   tested that one.
5. **matiec as a second execution oracle.** It is the compiler inside
   OpenPLC and Beremiz, so agreement with it is the same as agreeing with
   the open-source PLC an engineer is most likely to be moving from.

Things that look relevant but are not worth it: tree-sitter, the vendor
IDEs as integration targets, Twinpack, the courses and communities (see the
end).

**Found on the way:** ruSTy **v1.0.8** came out 2026-11-06. Its changelog
says "Backport the typesystem fixes from #2015 (#2016)". That is the fix the
`rusty-bump` card waits for. The `needs:` line on that card may be met
already. Check it with the oracle before believing it (see §5).

## What Helmsman already has

Read this first, so we don't propose things that are already built.

- **Compiler and interpreter** (`helmsman-iec`, about 11k lines, `no_std`
  capable): lexer, parser, checker, interpreter, PLCopen TC6 XML (LD/FBD/ST)
  and L5X ladder lowered to ST.
- **Language covered** (lib.rs doc and the playground's `SYSTEM` prompt in
  crates/helmsman-canvas/src/claude.rs): PROGRAM / FUNCTION /
  FUNCTION_BLOCK, VAR / VAR_TEMP / VAR_INPUT / VAR_OUTPUT / VAR CONSTANT,
  STRUCT / enum / alias, ARRAY, the elementary types up to LREAL and TIME,
  located variables, the full statement set, the standard functions.
- **Language not covered**: STRING / WSTRING, DATE / DT / TOD, VAR_IN_OUT
  (it parses as `Section::InOut`, but nothing downstream supports it),
  VAR_GLOBAL / VAR_EXTERNAL (the parser refuses them), function outputs
  other than the return value, pointers and references, and OOP (METHOD,
  INTERFACE, EXTENDS).
- **Feedback loop**: the checker's errors, the requirement tests
  (`test_NAME.st` with `ASSERT`, run in lockstep by `helmsman_iec::spec`),
  and the ruSTy differential oracle (tests/oracle.rs, run through docker,
  dev-only, executed and never linked).
- **Corpus**: `scripts/plcopen-corpus.sh` fetches Beremiz's and OpenPLC
  Editor's `plc.xml` files by hash and runs `tests/plcopen_corpus.rs` on
  them. The licenses keep them out of the repo.
- **Editor**: a hand-written highlighter in the egui canvas
  (`editor::highlight`), with diagnostics inline.
- **Sharing**: bundles and a marketplace skeleton.
- **No source for the language itself.** docs/SOURCES.md has entries for
  CIP, Modbus, Logix and ZIP. It has none for IEC 61131-3. Semantics are
  pinned by agreeing with ruSTy, not by citing the standard.

## The list, item by item

Each item says what it is, what it would change for Helmsman, and where I
stand on it.

### 1. Static analysis: iec-checker

- **What**: `iec-checker/iec-checker` (it moved from jubnzv). OCaml,
  LGPL-3.0, 110 stars, pushed 2026-04. The README claims 26 checks from the
  PLCopen Software Construction Guidelines (IDs such as `PLCOPEN-CP3`
  "initialized before use" and `PLCOPEN-CP13` "no recursion"), plus
  unreachable code and unused variables. Input is ST, PLCopen XML or SEL
  XML. Output can be JSON. Its dialect is matiec-compatible.
- **What it changes**: the guidelines name exactly the bugs an agent makes
  in code that compiles and still passes a thin scenario. Some lints
  Helmsman should have, my picks:
  - an FB instance called conditionally (a `TON` inside an `IF` stops
    timing when the branch is not taken; this is the classic ST timer bug);
  - an output written in more than one place. Explain mode and the live
    diagram already ask the agent for "one place that writes each output",
    and this lint would enforce it;
  - a variable read before it is assigned, when it has no initial value;
  - `=` or `<>` on REAL;
  - the loop variable or its bounds changed inside a FOR body;
  - unused variables and unreachable code (check.rs already has "the result
    of X is unused");
  - recursion (CP13), which the instruction budget only catches at run
    time.
- **How**: write them natively in `check.rs` as warnings with stable IDs.
  The canvas is wasm and the box is one binary, so an OCaml tool can't be
  embedded. Use iec-checker the way we use ruSTy: dev-time, in docker, as
  a reference that both tools flag the same corpus the same way. Warnings
  go into the agent's retry message in `verify::next` alongside compile
  errors.
- **Decided** (Nick, 2026-11-07): every lint is shown and goes into the
  agent's retry message. Two block "Use this code" and Deploy the way a
  red requirement test does: an FB instance called inside a conditional,
  and an output written in more than one place. Each blocking lint can be
  overridden on its line by a pragma the engineer adds with a reason,
  logged in the audit trail. The rest stay advisory until the ST corpus
  shows how noisy they are.
- **Needs**: the PLCopen guidelines document itself, for a SOURCES.md entry
  per lint ID. I haven't checked whether plcopen.org needs a login to
  download it.
- **Stand**: **do it first.** It is deterministic, offline, cheap per lint,
  and makes every agent turn better.

### 2. Compilers as oracles: RuSTy, matiec, blark

- **RuSTy** (`PLC-lang/rusty`): Rust, LGPL-3.0, LLVM backend, very active
  (pushed 2026-11-06). Already Helmsman's oracle. Two new facts:
  - v1.0.8 likely carries the #2015 signedness fix (the backport is #2016).
    Bump `docker/rusty/Dockerfile`, flip `rusty_unsigned_compare_bug`, and
    widen the sweep. That is the existing `rusty-bump` card.
  - The 1.0.8 changelog is mostly OOP work: interfaces, vtable dispatch,
    `THIS^`, `REFERENCE TO`. If we ever add METHOD / INTERFACE, the oracle
    already covers them.
  - Don't link it: LGPL plus LLVM would break the wasm canvas, the
    dependency budget and the edge node.
- **matiec** (`beremiz/matiec`): C++, GPL-3.0, pushed 2026-11-07. It
  compiles ST to C and is the compiler inside OpenPLC and Beremiz.
  - **What it changes**: a second, independent execution oracle. Where
    helmsman and ruSTy disagree, a third vote says which one is wrong. And
    "behaves like OpenPLC" is a claim we could make, and test.
  - **How**: the same pattern as ruSTy: a docker image, `HELMSMAN_MATIEC`,
    a harness that wraps the generated C in a scan loop and prints the
    variables. Its output is C, so the harness is more work than ruSTy's
    `stdin → run`.
  - **Stand**: worth it after the lints, mainly for the OpenPLC migration
    story. Half a day to a day for the image and the harness.
- **blark** (`klauer/blark`): Python, GPL-2.0, a Lark (Earley) grammar for
  TwinCAT ST. It is not an oracle, but its grammar and test fixtures are
  full of TwinCAT-flavoured ST (OOP, pragmas, properties) we refuse today,
  which makes it good corpus material (§3). It also parses TwinCAT
  `.TcPOU` files. Those are XML wrapping ST in CDATA, and an importer for
  them is a small job (§8).

### 3. A corpus of real ST

- **Sources** (license decides whether a file can live in the repo or only
  be fetched by hash, the way plcopen-corpus.sh does it):
  - `WengerAG/structured-text-utilities`: MIT. Arrays, numbers, strings,
    date/time.
  - `tkucic/UniTest`: MIT, pure IEC 61131-3 by design ("PLC, IDE and
    manufacturer agnostic"). GitHub labels both MIT repos "Smalltalk",
    which is just the `.st` extension.
  - OSCAT BASIC: from the CODESYS store. **I haven't verified its license
    or whether the source can be downloaded without an account.**
  - The test directories of ruSTy (LGPL), matiec (GPL), iec-checker
    (LGPL) and blark (GPL-2.0): fetch by hash, never vendor.
- **What it changes**: one test, `tests/st_corpus.rs`, reports for each
  file whether it parses, checks and compiles, and a total. The refusals
  get ranked by cause ("STRING: 312 files, VAR_IN_OUT: 140 ..."), and that
  ranking is the roadmap for §4. It is measured instead of guessed.
- **Stand**: **do it second.** It costs little and makes the ordering in
  §4 a data question.

### 4. Lifting the dialect

The guard is the product. Module isolation (`%I/%Q/%M` windows, no shared
memory), the live diagram and explain mode all depend on the language
staying analysable. So each feature gets a position, not a blanket "yes":

| feature | stand | why |
| --- | --- | --- |
| VAR_IN_OUT | yes | the standard way to pass a struct or array into an FB; every library uses it. Inside one module it breaks nothing. |
| function outputs (VAR_OUTPUT on FUNCTION) | yes | cheap, common in libraries |
| STRING (fixed-length, `STRING[n]`) | yes | messages, recipe names, OPC UA and webhook payloads, device IDs. Fixed length keeps the scan bounded and fits no_std. WSTRING later. |
| TOD / DATE / DT | yes | the shift calendar already exists; logic like "after 22:00" is natural to write and today can't be. Needs a clock source in `scan_at` (it has one). |
| VAR_GLOBAL inside one module (CONFIGURATION / RESOURCE scope) | maybe | common in imported code. Across modules it must stay refused; signals and `Binding::Module` are the way modules talk. |
| METHOD / PROPERTY on FUNCTION_BLOCK | later | TwinCAT and CODESYS code (TcOpen, Henneken's blog, Pratt's OOIP talk) is written this way, and models have read a lot of it. The diagram needs a story for methods first. |
| INTERFACE / EXTENDS / polymorphism | no, for now | dynamic dispatch defeats "on because ..." explanations and per-statement tracing. Revisit only if the corpus says imports fail mostly on this. |
| POINTER / REFERENCE | no | it breaks the memory-safety and isolation claims the voice rules lean on |

Each "yes" adds three things: oracle cases (ruSTy accepts all of them),
property tests, and the edge-node build (`no_std`, which matters most for
STRING). Each also changes the playground `SYSTEM` prompt and a relay
profile. That means a new profile id, since a served profile never changes.

- **Decided** (Nick, 2026-11-08): the table as it stands, with VAR_GLOBAL
  allowed inside one module and refused across modules. OOP stays out
  until the corpus ranking says imports fail mostly on it. Cards
  `st-var-in-out`, `st-var-global`, `st-string`, `st-tod-date` in TODO.md,
  in the corpus's order.

### 5. Unit-test frameworks: TcUnit, co-unit, UniTest, zkbuild-action

- **What**: TcUnit (C#/ST, 425 stars, active; GitHub shows its license as
  NOASSERTION, so read the LICENSE file before quoting it), co-unit
  (CODESYS Forge), UniTest (MIT, pure IEC), and zkbuild-action (MIT, a
  GitHub Action that builds and unit-tests TwinCAT projects on Zeugwerk's
  CI server).
- **Helmsman has its own**: requirement tests with one `ASSERT` block. The
  ideas worth taking:
  - **Typed asserts with expected and actual values.** TcUnit's
    `AssertEquals_INT(Expected, Actual, Message)` family. A red test that
    says "speed was 40, expected 50 at 3.2 s" in plain words is better for
    the engineer reading the Requirements tab, and better feedback when the
    agent fixes it. `ASSERT(COND)` only knows it failed. Without generics
    this needs STRING (§4) for the message, or one block per type.
  - **Several test cases in one file, each with its own name and result**
    (TcUnit's `TEST('name') ... TEST_FINISHED()`). Today one file is one
    requirement, which is a fine rule for engineers. Keep it.
  - **A headless test runner with JUnit XML output** (what zkbuild-action
    and TcUnit-Runner produce). Something like `helmsman-plc test
    PROJECT_DIR` that exits non-zero when a test is red. Then an
    engineer's project in git, the `git-on-controller` card, gets CI from
    any runner, with a template workflow. It fits the "robust: CI" benefit
    in the voice rules directly. The subcommands today are `account` and
    `bundle` (main.rs); `test` would sit next to them.
- **Stand**: the CLI runner plus JUnit output is small (about a day) and
  pairs with git on the controller. Typed asserts come after STRING.

### 6. Libraries: OSCAT, structured-text-utilities, TcOpen

- **OSCAT** (BASIC, BUILDING, NETWORK): the de facto open FB library. Its
  BASIC library has control blocks (PID-type controllers, ramps,
  hysteresis, filters), scaling, and time and calendar functions.
  - **What it changes**: a tested process-control library is what makes
    "the agent writes it" safe for analog loops. Without one, every tank
    level loop is a hand-rolled PI with its own windup bugs.
  - **Stand**: don't import OSCAT wholesale (license unverified; it needs
    STRING, VAR_IN_OUT and pointers in places). Write a small Helmsman set
    in `library.st` instead, about 10-15 blocks: `PID` with anti-windup and
    manual/auto bumpless transfer, `RAMP`, `HYST`, `SCALE`
    (raw→engineering, using the signal's `unit`), `LIMIT_ALARM`,
    `FILTER_PT1`, `DEBOUNCE`, `PULSE_COUNT`/rate, `SEQUENCE` helpers.
    Match OSCAT's or the standard's pin names where the model already knows
    them, give each block requirement tests, and mark in the docs which
    blocks are not IEC (as `BLINK_PATTERN` already is). The agent's profile
    gets the list.
  - **Decided** (Nick, 2026-11-08): OSCAT's or the standard's names, no
    aliases; the plain words go in each block's tooltip and help page.
    Card `st-library`.
- **structured-text-utilities**: MIT. Arrays and strings. It is corpus
  material, and once STRING exists, a source for string helpers.
- **TcOpen**: MIT, **archived 2025-02**, TwinCAT 3 plus .NET. Its
  component model is worth reading (a component per physical device, a
  task / sequencer pattern, a standard state machine for the operator).
  That is design input for the "running template of the machine" card, not
  code.

### 7. Editor tooling: tree-sitter grammar, vscode-st, STweep

- **tree-sitter-structured-text** (MIT, 7 stars, last push 2021, made for
  B&R Automation Studio): no. It is C, the canvas is egui and wasm, and the
  hand-written highlighter shares the real lexer, which is better.
- **vscode-st** (MIT, TypeScript, active): it shows what engineers expect
  from an ST editor (snippets, outline, formatting, go to definition). The
  real lever is a **language server built on helmsman-iec**: diagnostics,
  hover types, go to definition, the lints from §1. It is a thin binary
  over the checker, and gives any editor (VS Code, Neovim, Zed) Helmsman's
  dialect. It matters only if engineers edit outside the canvas. With git
  on the controller they might. Medium priority, and an open question for
  Nick.
- **STweep** (commercial formatter): the idea that matters is **one
  canonical format**. With a formatter, proposal review diffs show only
  real changes, and agent output and engineer edits look the same. Two
  catches:
  - the lexer drops comments and folds identifiers to upper case, so a
    formatter needs a lossless token layer (comments and original case
    kept) first;
  - Rule 2 of the agent prompt ("every line outside the section comes back
    byte for byte") means formatting can never run implicitly on save. It
    would be an explicit Format command, or only for agent-written new
    files.
  - Stand: worth it, but after the lints. The lossless layer is the real
    cost.
- **Decided** (Nick, 2026-11-08): neither the language server nor the
  formatter yet; revisit once `version-control` has put a project in git
  and an engineer has cloned one. Card `st-lsp-formatter`, parked behind
  it.

### 8. Platforms and import paths

- **CODESYS, TwinCAT, Beremiz, OpenPLC, PLCnext, Sysmac, Control Expert,
  B&R**: all export PLCopen XML to some degree, and that is already
  imported.
- **TwinCAT `.TcPOU` / `.TcDUT` / `.TcGVL`**: XML with the ST in CDATA,
  which blark parses. An importer for them is small, because the ST parser
  does the real work. It opens Beckhoff users, and their code will hit §4
  (OOP, VAR_GLOBAL) hard, which the corpus will show.
- **Siemens SCL (TIA Portal)**: an ST dialect with `#local`,
  `"DB".member`, `REGION`, its own timer instances (IEC_TIMER) and
  instructions. A source-level SCL→ST converter is real work but follows
  the L5X pattern (convert, refuse what doesn't map, list every problem).
  Siemens is the largest installed base outside North America. This is a
  market question for Nick, not a technical one. **Decided** (Nick,
  2026-11-08): not for the early release; card `scl-import` waits for a
  customer with TIA code.
- **Rockwell Studio 5000**: covered by the L5X import, and ST routines come
  through it too.
- **OpenPLC**: the closest open-source competitor, and it also runs on a
  Pi. matiec agreement (§2) plus the PLCopen corpus that already exists
  are the migration story: "your OpenPLC project runs here unchanged."

### 9. Standards: IEC 61131-3 and PLCopen

- **IEC 61131-3**: Helmsman cites no edition anywhere. The CLAUDE.md rule
  "protocol claims get a SOURCES.md entry" should cover the language too.
  Either buy the standard, or cite free secondary references per behaviour
  (the Fernhill ST reference on the list is detailed) and say they are
  secondary. The current edition is IEC 61131-3:2025 (edition 4.0, May
  2025; the 2013 edition 3.0 before it), sold at webstore.iec.ch
  (publication 68533; price not checked). **Decided** (Nick, 2026-11-08):
  secondary references only, marked as such: the docs/SOURCES.md entry
  "IEC 61131-3, by secondary references" says what each one covers and
  that no clause of the standard is cited from it.
- **PLCopen**: three things matter here: the coding guidelines (§1),
  TC6 XML (already used), and the OPC UA information model for IEC
  61131-3 that PLCopen and the OPC Foundation publish together. The box
  has an OPC UA server (`sink-opcua`). Exposing modules and FB instances
  in that model's shape would let SCADA clients browse them the way they
  browse a CODESYS controller. **I recall that companion spec exists, and
  I haven't verified its number or scope.** Check it before anything gets
  carded.

### 10. Packaging and DevOps: Twinpack, zkbuild-action

- **Twinpack** (GPL-3.0, active): versioned TwinCAT library packages with
  dependencies. Helmsman's bundles and marketplace already cover sharing.
  The one idea to take is **a project declaring the library versions it
  depends on**, so a project built a year ago rebuilds the same. That
  matters once §6 exists and `library.st` changes behaviour between
  releases. Small, and it can wait for the marketplace card.
- **zkbuild-action**: covered in §5. Copy the shape, not the service.

### 11. Learning material, lists, communities

- **Books** (Antonsen's *PLC Controls with Structured Text*, White's
  *Mastering PLC Programming*), Henneken's blog, Sagatowski's TwinCAT
  course, the tutorials: useful as *style* input for the agent's relay
  profile: naming conventions, a state machine with CASE on an enum, how
  to structure an FB. A short "house style" section in the next profile
  could take the good parts. The model already knows ST from these, so
  the gain is consistency, not knowledge.
- **awesome-industrial, twincat-resources**: more lists. Worth one pass
  for EtherNet/IP and Modbus prior art for SOURCES.md. Nothing else.
- **r/PLC, PLCTalk, CODESYS Forge**: where the early-release audience is.
  Marketing, out of scope here. A ready-to-run UniTest or OSCAT-style
  demo would make a better post than a feature list.

## Not worth doing

- Linking ruSTy, matiec or iec-checker into the box (licenses, LLVM, wasm,
  dependency budget).
- The tree-sitter grammar.
- Integrating with any vendor IDE.
- Twinpack itself, TcOpen code, the courses.

## Suggested order, if we agree

Agreed (Nick, 2026-11-08): carded in this order in TODO.md, "The ST plan".
The first and fourth rows shipped on 2026-11-07 (#282, #289).

| step | est | depends |
| --- | --- | --- |
| ruSTy 1.0.8 bump (existing `rusty-bump` card; check that the fix is really in it) | 2h | – |
| ST corpus test and refusal ranking (§3) | 1d | – |
| PLCopen guideline lints, the first 6-8, in `verify::next` (§1) | 3d | the guidelines document |
| `helmsman-plc test` plus JUnit XML (§5) | 1d | – |
| VAR_IN_OUT and function outputs (§4) | 2d | corpus ranking |
| STRING[n] (§4), then typed asserts (§5) | 4d | – |
| Process-control library, 10-15 blocks with tests (§6) | 4d | VAR_IN_OUT |
| matiec oracle (§2) | 1d | – |
| TOD/DATE (§4), TcPOU import (§8), LSP and formatter (§7) | later | discussion |

## Questions for the discussion (all decided)

1. Lints: decided 2026-11-07, see §1 (all shown, two block, a pragma
   with a reason overrides).
2. The dialect: the §4 table, VAR_GLOBAL inside one module only, OOP not
   on the table until the corpus says otherwise (2026-11-08, §4).
3. Process-control library: OSCAT's or the standard's names, plain words
   in the tooltips (2026-11-08, §6).
4. Editing outside the canvas: not yet; revisit after `version-control`
   (2026-11-08, §7).
5. Siemens SCL import: not for the early release (2026-11-08, §8).
6. IEC 61131-3: secondary references only, marked as such (2026-11-08,
   §9).
