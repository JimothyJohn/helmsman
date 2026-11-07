# Feedback: what users like and hate about the platforms in INVESTIGATION.md

Companion to INVESTIGATION.md. Researched 2026-11-07 by four parallel
web-research passes, about 70 distinct sources in all. It covers the
commercial IDEs, the open-source stacks, the ST tooling, and what engineers
say about AI writing PLC code.

**Read the limits first.** No pass could open **Reddit** (r/PLC is refused
by the fetcher, and mirrors were blocked) or most of **PLCTalk**, G2,
Capterra and the Siemens and Schneider forums (403). One pass did read
PLCTalk threads in full through the browser. The rest of the voice comes
from mrplc.com, Hacker News (Algolia), CODESYS Forge, vendor community
forums (PLCnext, B&R), GitHub issues, alltwincat.com, DMC,
theautomationblog.com and conference slides.
- Lines marked **(snippet)** come from a search-result summary, not an
  opened page.
- industrialmonitordirect.com shows up a lot in search. It reads like
  machine-written SEO, so it is used here only as a weak signal, and it is
  never the only source for a claim.
- "Recurs" means it came up in several independent sources in this sample.
  It is not a measured frequency.
- Many forum threads are 2011–2021. The complaints are old and still
  current, which is itself a finding.

## The short version

The same eight complaints show up on every platform, commercial or open:

1. **Version lock-in.** A project opens only in the exact IDE version and
   firmware, so engineers keep "500 different VMs". Upgrades are one-way.
2. **Binary or noisy project files.** They don't diff, merge or review.
   Version control is Dropbox and `R1, R2` copies.
3. **Slow, crashing Windows IDEs**, and worst while online. People save
   defensively because they don't trust the tool.
4. **Licensing and paywalls.** Dongles and CodeMeter, demo runtimes that
   stop after 2 hours, ST priced above ladder, Git and testing as paid
   add-ons.
5. **No headless build or test.** The compiler lives inside the GUI, so CI
   means scripting Visual Studio. Testing is toggling bits online, and
   nothing is recorded.
6. **Download and online-change surprises.** STOP to sync, variables
   cleared silently, HALT after an online edit.
7. **Install and toolchain breakage**, which is the top open-source
   complaint: Python and wxPython versions, LLVM, Pi OS releases,
   antivirus.
8. **Insecure defaults** on the open-source runtimes: `openplc:openplc`,
   an unauthenticated API, old vendored Modbus code.

What people consistently **like**, and would hate to lose:
- **Rockwell-style online edits**, with no stop and no surprise.
- **Free tools with a simulator** (CODESYS, the free TwinCAT engineering
  licence).
- **One tool for logic, motion and HMI** (Sysmac, TIA).
- **Text files that work with Git** (TwinCAT in principle, B&R in
  practice).
- **Structure and reusable FBs**, as long as the maintenance tech can still
  read them.

On **AI writing PLC code**, engineers trust it for explaining and
documenting code. They don't trust it for control logic. Their stated
conditions are review at the level of an intern, review that costs less
than writing the code, and machine checking. The bug they fear is the
subtle "close but not quite" one, at an edge case or a scan-timing
boundary, or from what "Bubba on 3rd shift" does to the machine.

## What this means for Helmsman

Each row is a recurring complaint, what Helmsman already answers (from
CLAUDE.md and ARCHITECTURE.md, not re-verified here), and the gap. The gap
column is where design attention should go.

| complaint | Helmsman today | gap / design rule |
| --- | --- | --- |
| Version lock-in, 500 VMs | one binary; the box serves the canvas | **Never make a project version-gated.** A project from any release opens in every later one. Refuse an unknown construct with a message, and never refuse the whole file. Pin `library.st` behaviour per project (INVESTIGATION §10) before §6 changes it. |
| Binary / noisy project files | ST files are plain text | Keep every project artifact (signals, devices, bindings, layout) as stable-ordered text: no GUIDs, no LineIDs, no reorder on save. DMC's worst TwinCAT bug was "variable links silently deleted after a merge", so I/O bindings must not be hidden state. |
| Slow IDE, worst online | egui canvas, native | Keep monitoring cheap over a slow link. PLCnext's online layer "will freeze 99.9% of the time" on VPN. Test the canvas against a throttled relay connection. |
| Licensing / paywalls | none | Stay that way: every feature on every box, no dongle, no demo timer. Users name all of these as betrayals ("pay-to-play entry barrier"). |
| No headless build / test / CI | requirement tests run in `spec` | **`helmsman-plc test` + JUnit XML (INVESTIGATION §5) is the most-requested missing thing on this list.** TcUnit-Runner died on Visual Studio automation, and Sagatowski called CI "so much more challenging than I could ever have imagined". |
| Tests need hardware | tests run against the interpreter | Simulated I/O is built in. Make it obvious: tests never need the remote modules connected, and no one ever "disables IO before running tests". |
| Online-change surprises | approval flow; module stop/run/restart (PR #253) | **Show what a deploy does to state before it happens:** which retained values survive, which reset, whether any module stops. Sysmac users: "The software should warn the user if the variables will be cleared." |
| Install breakage | one static binary | This is the biggest open-source pain, and Helmsman's strongest answer. Say so in the voice (robust, few dependencies). |
| Insecure defaults | accounts, approval, audit | No default password ever. The API is authenticated from first boot, and Modbus/CIP servers are off unless configured. OpenPLC v3's CVE history (§ OpenPLC) is the cautionary tale an engineer will compare against. |
| Compiler error quality | checker with inline diagnostics | matiec's worst complaint is the wrong line and a cascade from one `ELSE IF` typo. **One error at the right line, with the fix in plain words**, and recovery so one typo doesn't produce twenty errors. Worth a test corpus of typical typos. |
| No scan-time / jitter view | fixed scan, watchdog | Show scan time, jitter and overruns on the Monitor. OpenPLC's "There is no straightforward way to do what you want" is the complaint. Show FB instance and STRUCT values live (an OpenPLC v4 regression). |
| Fieldbus edge cases | EtherNet/IP, Modbus TCP | Check against the OpenPLC failures: one TCP connection per host, not per slave (gateways cap at 2–4); unit IDs behind gateways; graceful handling of unsupported function codes; no silent retain truncation (OpenPLC dropped retained variables past the first 128). |
| Dialects break all tooling | one dialect, one parser | Document the dialect as one page (INVESTIGATION §9). That fixed, written target is a selling point to tool builders and to the agent. |
| Abstraction vs the maintenance tech | no OOP, flat ST, explain mode | This supports the INVESTIGATION §4 "no / later" on OOP. Sysmac maintenance staff "hate FBs as well". The live diagram and explain mode are the answer: the technician reads behaviour, not class hierarchies. |
| Windows coupling | runs on Linux; canvas in the browser | Never require Windows anywhere. TwinCAT BSODs after a Windows update, and Control Expert's speed depends on the Windows version. |

**For the agent specifically**, where the AI-trust findings point:

- **Reading the tests must be much cheaper than reading the code.** mk42:
  "It just trades tedious typing for tedious proofreading." The
  Requirements tab in plain words is the right shape. Keep each requirement
  readable by someone who can't read ST.
- **Tests over time and abnormal inputs, not only the happy path.** These
  are the failures engineers expect: sensor stuck, operator presses both
  buttons, power loss mid-sequence. The agent's test-writing profile should
  produce those cases by default.
- **Machine checking raises quality.** LLM4PLC took the success rate from
  47% to 72% with a compiler plus a model checker in the loop (snippet,
  arXiv 2401.05443). That is the case for the INVESTIGATION §1 lints in the
  retry loop. iec-checker's open issues include CP12, "physical outputs
  shall be written once per PLC cycle", the same lint §1 proposes.
- **Answer timing questions.** arpus4KM wants to ask "How many
  milliseconds will this logic take to execute". The instruction budget and
  scan stats could answer that in explain mode.
- **A named human signs off.** The liability fear ("a tort lawyer will
  argue you don't know what you're doing") is met by approval and audit
  naming who verified what. Keep that visible, not buried.
- **No cloud in the control path.** Critical-infrastructure operators ban
  cloud-dependent AI (snippet, arXiv 2511.09122). The box runs without the
  relay. Say so where a buyer will ask.
- **Simple is a virtue to this audience.** "PLCs use IEC languages
  *because* they are limited." Agent output should be flat, boring ST.

---

## Commercial platforms

### Siemens TIA Portal (STEP 7, SCL, WinCC, Openness)

**Liked**
- **Structure and reusable FBs and DBs.** "Personally I much prefer Siemens
  to AB, due to the structure and the ability for re-usable code."
  (https://mrplc.com/forums/topic/18857-siemens-vs-allen-bradley/)
- **PLC and HMI in one tool, with good drag and drop.** "the drag & drop
  support in TIA is really good." "HMI integration is good too."
  (https://forge.codesys.com/forge/talk/Engineering/thread/52490a7e81/)
- **Block-at-a-time downloads, and a free trial with no sales call.**
  "you can grab a Siemens 21 day trial from their website without calling
  anyone"
  (https://theautomationblog.com/siemens-vs-allen-bradley-plcs-one-engineers-perspective/)

**Hated** (worst first)
1. **Slow and bloated.** "What I can do in half a second in Codesys takes
   like 15-20 seconds in TIA." (Forge 52490a7e81) "Each time I navigate
   between the fast an very well built STEP7 and the bloated and heavy
   TIA, I wonder how SIEMENS fell so hard." (snippet,
   https://www.plctalk.net/goto/post?id=720274)
2. **Binary blobs and GUI-only tooling.** "GUI only tooling, binary blobs
   that don't work well with version control"
   (https://news.ycombinator.com/item?id=29905046). "the binary blob that
   is the source code and the exported readable code get out of sync"
   (https://news.ycombinator.com/item?id=38508956)
3. **Weak search and debugging.** "There is no way to search project wide
   for comments, partial variable names or anything else." The same poster
   calls testing and debugging "a nightmare" (Forge 52490a7e81).
4. **One-way upgrades and many installed versions** (weakly sourced; see
   the Rockwell VM quote).
5. **Licensing.** It is painful to share, and WinCC has "a weird price by
   the 'power tag' thing" (theautomationblog).
6. **UI by committee, and churn.** V17 changed the ladder shortcuts, and
   they "can't be changed back" (snippet, Siemens forum post 1142126).

Quote: "TIA portal makes me want to kill myself sometimes, but Siemens AX
is now just vscode with extensions!"
(https://news.ycombinator.com/item?id=41461230). Siemens' own answer to the
complaints is a text-based, VS Code-shaped toolchain.

### Rockwell Studio 5000 Logix Designer and FactoryTalk

**Liked**
- **Online edits.** This is the most consistent praise, even from Siemens
  fans. "I do still prefer Rockwell's Online Edit methodology over the
  partial download between CPU scans of the S7" (theautomationblog).
  "Online editing couldn't be any easier." (snippet, G2)
- **Easy to use.** "the easiest platform I've used." (snippet, G2)
- **Licences shareable across a team; ControlFlash** "makes it really easy
  to upgrade firmware across all devices" (theautomationblog).

**Hated** (worst first)
1. **Cost, with a surcharge for ST.** "10k for ladder and another 10k for
   the other languages."
   (https://news.ycombinator.com/item?id=41465114) "more than $25,000, not
   even counting the subscription fees."
   (https://news.ycombinator.com/item?id=40528589)
2. **Version and firmware lock-in.** "They are not backward compatible so
   the standard thing every control engineer seems to do is have 500
   different VMs." (https://news.ycombinator.com/item?id=31823921)
3. **Crashes.** "if you go a day wihout a crash, then you start to feel
   like checking your files."
   (https://news.ycombinator.com/item?id=30228654)
4. **Paid support and subscriptions.** "I loathe paying $800/year to
   Rockwell for the privilege of reading" firmware notices
   (https://news.ycombinator.com/item?id=32224185).
5. **FactoryTalk View.** "Horribly expensive, laggy and broken." "Don't let
   windows update either" (https://news.ycombinator.com/item?id=30225146).
6. **Windows-only; no online edits inside AOIs** (snippet, G2).

Quote: "This package is part of the pay-to-play entry barrier in my
industrial controls industry." (HN 40528589)

### CODESYS (IDE, runtime, Store, Raspberry Pi, Git, co-unit)

**Liked**
- **Free IDE, widest hardware support, built-in simulator.** "it has the
  most hardware support, and lots of vendors use it under the covers."
  (https://news.ycombinator.com/item?id=40883132)
- **More productive than TIA, and modern** (OOP, test manager, Python
  scripting). "I'm far more productive with it." (Forge 52490a7e81)
- **Git built in, when licensed.** "Git functionality is built right in,
  it's fantastic." (https://news.ycombinator.com/item?id=38520137)

**Hated** (worst first)
1. **IDE hangs that lose work.** "the ide becomes non-responsive forever,
   and everything since the last :Save" is gone
   (https://forge.codesys.com/forge/talk/Engineering/thread/a4801cec06/).
   "5 minutes later, CodeSys crashes."
   (https://forge.codesys.com/forge/talk/Engineering/thread/e8460c92e1/)
2. **Binary project file; Git and SVN cost extra.** "you will have to
   manually merge"
   (https://forge.codesys.com/forge/talk/Engineering/thread/294c752534/).
   Git "in very bad shape" (https://news.ycombinator.com/item?id=38570262).
   Git now comes with the Professional Developer Edition, about $660 per
   user per year (snippet, CODESYS store).
3. **Runtime licensing.** Unlicensed, it "runs for two hours … before shut
   down" (snippet, Pi SL datasheet). "If I was going to pay for Codesys on
   each Pi, anything above 10€ is unreasonable really."
   (https://forge.codesys.com/forge/talk/Runtime/thread/4d2beb5052/)
4. **Testing is a paid plugin "in rough shape"** (HN 38570262). That gap is
   why co-unit exists, per its own page: the "absence of 'good and free'
   IEC61131-3 automated unit-testing solutions" (snippet).
5. **On the Pi.** "the pessimal application of an ARM processor… run it hot
   on a 50Hz (default) PLC loop"
   (https://news.ycombinator.com/item?id=49218043). Pi 4 runtime crashes
   were fixed only by reflashing an older OS image
   (https://forge.codesys.com/forge/talk/Runtime/thread/038e7bdccf/).

Quote: PLC programming is "still stuck in the 1990s software development"
(HN 38570262).

### Beckhoff TwinCAT 3

**Liked**
- **Text project files that work with Git, in principle.** "most direct Git
  merges succeed… especially when splitting the project into individual
  files"
  (https://www.dmcinfo.com/blog/18615/version-control-and-multi-user-development-with-beckhoff-twincat-3/)
- **Free engineering, renewable 7-day runtime trials** (snippet, Beckhoff
  InfoSys).
- **Version pinning**, which "will prevent you from accidentally upgrading
  the project" (DMC). TwinCAT/BSD is respected as a lean real-time base
  (snippet).

**Hated** (worst first)
1. **Merge noise and fragile merges.** "TwinCAT saves its project files
   somewhat arbitrarily, the order of elements is changed seemingly at
   random." (https://twincat-tools.readthedocs.io/latest/pages/tools.html)
   DMC lists LineID conflicts, links silently deleted after merges, and
   "TwinCAT doesn't always notice external Git changes".
2. **Version and library lock-in.** "You cannot install an older version of
   TwinCAT on a machine where a newer version is installed." A library
   mismatch blocks going online "even with otherwise identical code"
   (DMC).
3. **The Visual Studio shell** is heavy, slow and crashes. A localized
   German VS breaks the Automation Interface
   (https://www.github.com/TcOpenGroup/TcOpen/issues/708).
4. **No headless build or test.** "Its not possible with MsBuild, because
   plc projects are build with CodeSYS compiler."
   (https://github.com/tcunit/TcUnit/issues/7)
5. **Windows in the runtime.** Windows 11 24H2 with 4024 "will crash your
   runtime and potentially give you a BSOD"
   (https://alltwincat.com/2025/03/29/twincat-4024-windows-11-24h2-bsod/).

Quote: "There isn't anything mentioned about TDD in the TwinCAT
documentation!"
(https://alltwincat.com/2017/11/02/test-driven-development-in-twincat-part-1/)

### Phoenix Contact PLCnext

**Liked:** openness (Linux, containers, Node-RED), mostly from vendor
material, with little independent praise. "The Long Term Support Version
is much more stable than the newer versions."
(https://forum.plcnext-community.net/t/plcnext-engineer-issues/9949)

**Hated**
1. **The online layer freezes over VPN.** "The remote access to the
   controllers is next to useless." "The program will freeze 99.9% of the
   time." Phoenix said this "results from the current design of the
   'online layer'" and gave "no timeframe"
   (https://forum.plcnext-community.net/t/plcnext-engineer-slow-and-unresponsive-while-in-remote-connection-vpn/11876).
2. **Crashes and silent data loss.** "CSV imports declares success but
   inspection shows that only the Group names are present." (thread 9949)
3. **Upgrade regressions** corrupt the I/O configuration (snippet).
4. **C++ toolchain friction**: Eclipse pinning, and root rights discouraged
   (snippet). **CodeMeter licence errors** (snippet).

### Omron Sysmac Studio

**Liked**
- **One tool for logic, motion, safety and EtherCAT**, which "drastically
  reduces the engineering time"
  (https://mrplc.com/forums/topic/21802-new-plc-nj501-1500-machine-controller/).
- **A 3D motion simulator; symbolic IEC tags.** "a hell of a lot easier
  than learning cryptic memory maps." "Would not go back."
  (https://mrplc.com/forums/topic/32267-move-to-nj-series-worth-it/page/2/)

**Hated**
1. **Slow, especially online.** "It takes literal minutes to open a
   project." "Four seconds for each contact and relay you place in online
   editing mode."
   (https://mrplc.com/forums/topic/40084-sysmac-studio-ide-very-slow/)
   "Trying to type when online is like wading through treacle."
   (https://mrplc.com/forums/topic/27262-sysmac-studio-improvement-request/page/4/)
2. **Sync semantics.** It goes to STOP to sync small changes (snippet,
   Reddit), variables are cleared without warning, and comments are lost
   in transfer.
3. **Half-baked third-party EtherCAT.** "Either Omron complies with the
   ETG… or it doesn't."
4. **Opaque saves; the PLC and HMI can fall out of step.**
5. **Maintenance staff resist abstraction.** "They hate sysmac studio and
   symbolic addressing." "They hate FBs as well." (thread 32267)

Quote: "An instruction that does not work should be illegal." (thread
27262)

### Schneider EcoStruxure Control Expert and Machine Expert

Weakest sourcing: the Schneider community returned 403, and nothing below
comes from an opened page. **No first-hand praise found.**

**Hated** (all snippets)
- "software crashes very often", with "a lot of bugs existing since very
  old versions"
  (https://community.se.com/t5/Industry-Automation-and-Control/Control-Expert-lacks-improvments/m-p/412313).
- **Speed tied to Windows.** v14.1 is slower on Windows 10 than Windows 7,
  and support blamed Microsoft. Startup takes 2 minutes on Windows 11.
- **One-way upgrades.** An updated project can't be opened in an older
  version, and the object code changes, so the project must be retested.
- **HALT after an online modification in RUN** (hotfix notes).
- A blogger on version numbering: "Awfully incremental improvements to be
  justifying major sofware number increments."
  (https://jasonfirth.ca/new-software-release-schneider-unity-pro-9-i-mean-10-i-mean-11/)

### B&R Automation Studio

**Liked** (one strong voice, 2015)
- "really is the best IDE of any PLC platform by a very wide margin". The
  ST editor is "world class". It "works easily with standard version
  control (Subversion, Git, etc)". "The same code will run on any of their
  PLCs." (https://mrplc.com/forums/topic/15012-br-automation-opinions-needed/)

**Hated**
1. **Licensing churn** (Technology Guarding to CodeMeter). Old versions
   stopped activating, and users see SSL/TLS errors
   (https://community.br-automation.com/t/license-key-issue/5711).
2. **Major-version performance regressions.** On AS4.12 to AS6, CPU went
   "from 40% to 60-80%"
   (https://community.br-automation.com/t/as6-performance-of-mappview-visualization-drop-down/4208).
3. **A poor ladder editor (older), a steep learning curve, vague errors.**
   "Need to get error information more detailed way to user" (snippet,
   G2).

---

## Open-source stacks

State checked on GitHub (2026-11-07):
- `thiagoralves/OpenPLC_v3` is **archived**.
- OpenPLC v4 is `Autonomy-Logic/openplc-runtime` (headless, REST, Docker)
  plus `openplc-editor` 4.3.x.
- Beremiz and matiec are both active.

### OpenPLC (v3, Editor, v4)

**Liked**
- **Free, on cheap hardware, no licence keys.** This recurs everywhere.
- **An open runtime you can read**, "for anyone who needs to know exactly
  what the scan is doing"
  (https://ladx.ai/resources/open-source-plc-options).
- **Fast with good I/O.** "loop reaction times in the steady 10-12uS area"
  on a Pi 4B with LinuxCNC
  (https://hackaday.com/2020/08/24/evaluating-raspberry-pi-as-a-programmable-logic-controller).
- **The maintainer answers most threads himself.**

**Hated** (worst first)
1. **Security.** v3 is end of life with an API auth-bypass CVE, reported
   as CVE-2026-28205 at CVSS 9.2
   (https://fieldguide.lutrasecurity.com/CVE-2026-28205). A traversal to RCE
   is made easier by "hardcoded default credentials (openplc:openplc)"
   (snippet, SentinelOne). There is an outdated libmodbus with CVEs
   (https://github.com/thiagoralves/OpenPLC_v3/issues/174). v4 has an
   unauthenticated S7comm crash and connection-exhaustion DoS
   (https://github.com/Autonomy-Logic/openplc-runtime/issues/153). *The CVE
   ids are as the agent reported them, not re-checked here.*
2. **Churn, with no stable releases.** "The development branch was a side
   project that ended up being abandoned."
   (https://github.com/thiagoralves/OpenPLC_v3/issues/224) "For a PLC
   runtime that's quite an important point, I dare say." Editor and
   runtime versions mismatch, with "MD5 verification failed" errors.
3. **Installing.** 47 v3 issues match "install". A Pi Zero 2W freezes, and
   antivirus deletes `xml2st.exe`. Hofer's SFSCON 2025 slides:
   "Complicated installation and management… Docker installation
   cumbersome".
4. **Modbus master.** It opens one TCP connection per slave, so gateways
   capped at 2–4 connections give "dropping connection, reconnecting and
   generally poor performance"
   (https://github.com/thiagoralves/OpenPLC_v3/issues/195). v4 refused
   duplicate host:port combinations until 4.1.5 (#131). Retained variables
   past the first 128 were silently dropped
   (https://github.com/Autonomy-Logic/openplc-runtime/issues/209).
5. **Real time.** In the benchmark, compute was about 1/3 of CODESYS, with
   "Jitter also inconsistent, 3x with same CPU load" and the web server
   sharing the container (SFSCON slides,
   https://www.sfscon.it/wp-content/uploads/2025/11/1762239388197OpenPLC.pdf).
   On measuring scan load: "There is no straightforward way to do what you
   want."
6. **Docs and tone.** "If you don't like it, feel free not to use the
   software" (https://github.com/thiagoralves/OpenPLC_v3/issues/187).
7. **Seen as a hobby tool.** It "definitely isn't suitable for larger
   projects" (snippet, Habr).

### Beremiz

**Liked:** a full open IDE and runtime, with Linux targets working
"out-of-the-box". The maintainer fixes things quickly (Windows breakage
fixed in v1.6.1 within days).

**Hated**
1. **Install and build.** The snap crashes, wxPython builds fail, and the
   Win11 First Steps project fails to build. "I still didn't setup any
   automated test for this damned Windows OS"
   (https://github.com/beremiz/beremiz/issues/89).
2. **Documentation.** "documentation is still really sparse despite of
   almost 20y old project" (maintainer,
   https://github.com/beremiz/beremiz/issues/53).
3. **Plugin rot** (OPC UA, EtherCAT, CANopen), and a monolithic codebase.

### matiec

**Liked:** "the only widely available way to check without a vendor
licence" (ladx.ai). It sits under both OpenPLC and Beremiz.

**Hated**
1. **Error messages.** "The error is on line 23, and matiec outputs what is
   on line 26" (https://github.com/beremiz/matiec/issues/2). On an `ELSE
   IF` typo, "many errors will be shown… the real error is the 'ELSE IF'"
   (https://github.com/beremiz/matiec/issues/13).
2. **Codegen correctness.** Issues cover VAR_TEMP and struct-array init,
   LREAL literal limits, DINT_TO_TIME, and a duration literal 1 ns short
   (#9, #11, #12, #14, #19, #22, #44). This matters for INVESTIGATION §2:
   as an oracle, matiec will disagree for its own bugs too.
3. **Gaps.** No `CONTINUE`; FOR `BY` expressions skipped (#4).

### ruSTy

**Liked:** a modern ST compiler close to the standard. People embed it in
their own runtimes (#1473). Followers say "many Codesys and Beckhoff
programmers… would be tempted to migrate", but only for a "Batteries
included" experience (https://github.com/PLC-lang/rusty/issues/1276).

**Hated:** install pain ("have not been able to install the compiler on any
machine", #985; LLVM versions on Fedora, #450). Panics: "we should
solve/remove all the panicked errors output text!" (#1168). No runtime, IDE
or online debugging, and locals are invisible in the debugger (#1473).

### Pi-as-PLC in general

- "the one thing i hate about it is the reliability(or lack of) the SD
  cards"; "RPI does NOT have the temp range for industrial applications"
  (Hackaday comments).
- An integrator replaced a Pi system with an industrial PLC within six
  months over false triggers, and replies blamed the wiring (snippet,
  https://www.plctalk.net/forums/threads/raspberry-pi-open-plc.135333/).
- Node-RED is fine for monitoring and gateways, not interlocks (snippet,
  vendor-written).

**Why people would or wouldn't run open source in production.**
- **Would:** small, non-critical or research installs, behind remote I/O,
  where cost and a readable scan matter.
- **Wouldn't:**
  - no certification and no support contract;
  - jitter about 3x CODESYS;
  - insecure defaults;
  - no stable release line;
  - Pi hardware (temperature, SD cards).

This is the objection list Helmsman's "early release" faces, and every
item on it except certification is something the design can answer.

---

## Structured Text tooling

### TcUnit and TcUnit-Runner
- **Liked:** it filled a real gap. "I would like to say a huuuge Thank
  you!" (https://alltwincat.com/2018/12/10/tcunit-a-twincat-unit-testing-framework/).
  The Runner "kinda works out-of-the box"
  (https://github.com/tcunit/TcUnit-Runner/issues/2).
- **Hated:**
  - **CI through Visual Studio DTE.** Issue #19, "Error loading VS DTE",
    has 40 comments. There is a BuildSolution hang (#28) and VM timeouts
    (#30). The Runner is now archived.
  - **Hardware gets in the way.** "Make sure you disable IO before running
    TcUnit" (https://github.com/tcunit/TcUnit/issues/264). Heap limits are
    raised by hand-editing `TcRegistry.xml`.
  - **Output and timing bugs:** messages printed several times, tests run
    4 times, wrong durations, FPU exceptions in `AssertEquals_REAL`
    (#248, #235, #226, #279).

### UniTest, co-unit, zkbuild, Twinpack, TcOpen
- **UniTest** claims to be vendor-agnostic but breaks on B&R: "VAR_GLOBAL
  doesnt exist in B&R", "iecUnitTest is too long name for B&R library"
  (https://github.com/tkucic/UniTest/issues). There have been no commits
  since 2021. "Portable IEC" does not survive real toolchains.
- **co-unit**: no user discussion found. The commercial alternative is
  about €1500 (https://www.plctalk.net/forums/threads/unit-testing-frameworks.107699/).
- **zkbuild-action**: four issues, all about project shape (testing a PLC
  project without I/O).
- **Twinpack**: `add` "clears its namespace and drops its library
  parameters" (#183). There is a random IDE error needing a restart (#146),
  and an 18-comment thread on Azure feed login.
- **TcOpen**: version churn ("Make it work with 4026").

### OSCAT
- **Liked:** free, broad, multi-platform. People keep using it despite the
  problems.
- **Hated:**
  - no warranty, volunteer support (snippet, CODESYS store data sheet);
  - an out-of-bounds read in `MONTH_TO_STRING`, VDE-2024-046, fixed in
    Basic 3.3.5 (snippet, certvde.com);
  - "oscat is very big" and leaves unresolved POUs after a failed install;
  - name clashes (`INT_TO_BCD`) and breakage on new CODESYS SPs.
- This supports INVESTIGATION §6: write a small tested set rather than
  import OSCAT. It also warns that STRING functions are where library
  memory bugs live, which matters for §4's STRING[n].

### iec-checker, blark, tree-sitter
- **iec-checker**: "found it very helpful", but the user also hit parser
  limits because "the existing standard ST syntax … is a subset of the
  CODESYS ST syntax" (https://github.com/jubnzv/iec-checker/issues/63).
  Many PLCopen rules, CP12 among them, are open issues, so its coverage is
  partial. Use it as a reference for the lints, not as the spec.
- **blark**: grammar gaps on real TwinCAT code. "Boolean literals are
  parsed as identifiers" (#108); "Variables Containing the word 'Return'
  Incorrectly Identified as RETURN" (#67).
- **tree-sitter-structured-text**: five tiny forks, no discussion.

### vscode-st, STweep
- **vscode-st**: the top issue (26 comments) asks for a linter. The
  maintainer: "In order to implement it LSP has to be created with full
  code parse capability. Lint will be just a byproduct"
  (https://github.com/Serhioromano/vscode-st/issues/41). There is also high
  CPU load (#101) and the formatter adds spaces in the wrong places (#46).
  This bears on INVESTIGATION question 4: demand for an ST language server
  with real diagnostics is visible, and Helmsman's checker is that parser.
- **STweep**: "Multiline condition gets put on 1 line", errors on huge FBs,
  "removes tabs/spaces from empty lines, breaking code collapse hierarchy"
  (https://github.com/Barteling/STweep/issues). A formatter that breaks
  code is worse than none, which supports §7's "lossless token layer
  first".
- Peter Nachtwey: "I think that most PLCs programming software is awful.
  Look at VSCode for starters"
  (https://www.plctalk.net/forums/threads/ia-plcs-who-else-would-you-say-is-using-ai-to-help-with-their-programming.148578/).

---

## AI writing PLC code: what engineers say

Mostly from three PLCTalk threads read in full: 146094 "Copilot structured
text coding", 146383 "How are you using AI…", 143095 "Evaluate idea: AI for
PLC programming". Also 148578, mrplc 43898 and drivesncontrols.com.

**Where it helps, by their account**
- **Explaining code others wrote.** It is "very useful in explaining the
  logic of code written by others" (DonNH01, 146094).
- **Documentation and busywork.** "a really good job at doing a first pass
  on descriptive paperwork" (KenWiegers, 146383).
- **Boilerplate for seniors.** "great for Senior engineers who need an
  efficient boost when doing refactoring or needing to build out
  boilerplate code like an Equipment Driver AOI" (theColonel26, 146383).

**Failures they report**
- **Subtle bugs.** "it often makes very hard to find mistakes" (goghie,
  143095). "it does make some really stupid mistakes" (DonNH01).
- **Ladder output is useless; ST is close but misses vendor details.**
  Ladder output is "completely unintelligible" (DonNH01). A start/stop
  circuit got "Afraid I will not be using it" (parky). "I've tried a few
  times with zero success"
  (https://mrplc.com/forums/topic/43898-using-chatgpt-to-write-ladder-logic/).
- **Thin training data.** "The publicly available code base for IEC61131
  languages is absolutely miniscule" (JLand, 146094).
- **Confidently wrong.** "it 'sounds' authorative when it just doesn't a
  have a clue what it's talking about" (Pete.S., 146383).
- **Outright rejection.** "I'm using AI zero percent. Don't trust it"
  (robertmee).
- **A tester.** Sagatowski tested ChatGPT on PID in ST: some answers
  "excellent", but "there's a lot of limitations in the system"
  (https://drivesncontrols.com/news/fullstory.php/aid/7245/).

**Fears**
- **Liability.** "Would you trust life and limb to AI generated code …?
  It's fine for turning on your sprinkler system" (robertmee). The
  counterpoint: "there must be other personel that does a verification"
  (methadrine).
- **The real plant.** "AI programming will never be able to think of all
  the things people can do to the machine." "Bubba on 3rd shift can't be
  predicted." (143095)
- **Non-experts accept nonsense.** "you're all too likely to accept an AI
  hallucination as a valid control strategy" (Steve Bailey, 148578).
- **Review costs as much as writing.** "It just trades tedious typing for
  tedious proofreading" (mk42, 143095).
- **Juniors stop learning.** "I would rather turn over the drudge work to
  an intern than to an AI. That way, the intern learns something." (Steve
  Bailey)
- **The cloud** is banned by critical-infrastructure operators (snippet,
  arXiv 2511.09122).

**What would earn trust**
- **Intern-level supervision.** "If you trust it about as much as work an
  intern would do, then it might be OK" (mk42).
- **Questions about behaviour and timing**, such as "How many millliseconds
  will this logic take to execute" (arpus4KM).
- **Machine checking.** LLM4PLC (compiler plus model checker) took success
  from 47% to 72% and expert-rated quality from 2.25 to 7.75 out of 10
  (snippet, https://arxiv.org/html/2401.05443v1). LLM-PLC-AS found prompt
  structure mattered more than the model (snippet).
- **Limited languages on purpose.** "PLCs use IEC languages *because* they
  are limited" (idiotsecant, 146094).

**Read for Helmsman:** the forum consensus describes Helmsman's loop almost
exactly: AI drafts, a human verifies against requirements, a machine checks,
and a named person approves. That doesn't make engineers believe it. Their
objection is specific: if verification costs what writing costs, nothing
is gained. Helmsman's pitch has to rest on how cheap checking is: requirements in
plain words, a live diagram, explain mode, tests over abnormal sequences.
How capable the model is matters less.

## Testing and version control in PLC culture

- **Testing is manual and then lost.** Before TcUnit, Sagatowski toggled
  inputs online, which was "a one-time shot" and "not documented
  anywhere".
- **Hardware coupling.** Integrators still want tests "to decrease
  debugging on the machine" (TcUnit-Runner #3). Simulation routines and
  bench wiring are written by hand.
- **Version control is improvised.** "Manually saving project versions is
  better than not doing anything, but just barely." "I have everything on
  Dropbox." "R1, R2, etc.."
  (https://www.plctalk.net/forums/threads/who-is-using-version-control-git-subversion-assetcenter-etc.92781/)
- **Plant tools don't serve OEMs.** "VersionDog and MDT AutoSave are
  completly worthless for OEMs and Machine builders"
  (https://www.plctalk.net/forums/threads/plc-programming-version-control.123604/).
- **What would change it** (the sources' own words, summarized): tests that
  run without hardware and without driving the IDE, projects that diff,
  and CI that works out of the box. Each is in reach for Helmsman: the
  `test` subcommand, text projects, and the git-on-controller card.

## Open threads this raises for the INVESTIGATION discussion

- **Q1 (do lints block?):** the trust findings favour blocking on the
  output-written-twice and conditional-FB lints. Those are the "very hard
  to find mistakes" engineers fear, and CP12 is a published guideline.
- **Q4 (do engineers edit outside the canvas?):** vscode-st's most-wanted
  feature is a real linter from a real parser, and HN's praise for Siemens
  AX is that it's "just vscode". The evidence leans yes for some users.
  It's still not proof that Helmsman's users will.
- **New:** a deploy preview of state effects (retain, reset, which modules
  stop). Nothing in INVESTIGATION covers it, and it answers a complaint
  every vendor has.
- **New:** scan-time and jitter on the Monitor, plus a throttled-link test
  for the canvas.
- **New:** a fieldbus checklist from the OpenPLC Modbus failures, to check
  against Helmsman's drivers.
