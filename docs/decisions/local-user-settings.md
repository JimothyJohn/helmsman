# Local user settings and sign-in on a Skipper (proposal, 2026-11-23)

A proposal for Nick to read and decide on. Nothing here is built. It
answers the card `local-user-settings`: "Skippers run on-device, so user
preferences must persist locally on the device. This implies a lightweight
local user/auth model. Propose a design before building it."

## The short version

Most of the model exists already. A box has local accounts, sessions and
a per-person store on the device. What it lacks is small:

1. **Preferences are kept in the browser only.** Light mode, word wrap,
   the panes, the colours, "reset after a described change" live in one
   `localStorage` string per browser. Open the same Skipper from another
   screen and they are gone.
2. **There is nowhere in the UI to see or change them.** They are
   scattered over the View menu and remembered silently.
3. **The first account can only be made at a shell** (`helmsman-plc
   account add`). A Skipper that arrives preloaded has no way to make one
   from its own screen.

The proposal is to add a typed settings record beside what the box
already keeps per person, a Settings window that is the one place they
are shown, and a first-run step that makes the first account on the
device. No new sign-in mechanism.

## What exists today (read from the code, 2026-11-23)

| Piece | Where | What it does |
|---|---|---|
| Local accounts | `crates/helmsman-plc/src/policy/account.rs` | One login per person in `<state-dir>/accounts` (mode 0600): name, role (operator, engineer, integrator), enabled, PBKDF2-SHA256 hash. Edited with `helmsman-plc account add\|passwd\|disable\|enable\|list`; a password is only read from stdin. |
| Sessions | same | `POST /api/login` gives a 32-byte token held server-side as its SHA-256; ends after 1 h, after 15 min unused, on logout, or when the account changes. Wrong passwords lock the account and then the box. |
| Role PINs | `policy/pin.rs` | A box without an accounts file takes a role's PIN instead; with the file, PINs are refused. |
| The desk, per person, on the box | `api/desk.rs`, `crates/helmsman-canvas/src/desk.rs` | `GET`/`PUT /api/desk`: the open project and file, unsaved edits, half-typed inputs, kept in `<state-dir>/desks/`, one file per person, at most 512 KiB each. The browser's copy is the fallback when signed out. |
| Pane layouts, per box | `api/pane_layouts.rs` | Named layouts in `<state-dir>/pane-layouts.json`; an engineer may write them. |
| Preferences, per browser | `Chrome::saved()` in `crates/helmsman-canvas/src/editor.rs` | One line of `key=value` in `localStorage`: `light`, `wrap`, `reset`, `colors`, the panes and folds, `advanced`, `iogrid`. Not sent to the box. |
| The playground | `sim_projects.rs` | No box, no accounts: projects and preferences in the browser. |

So "a lightweight local user/auth model" is `account.rs` as it stands. It
is local to the device, needs no cloud, and already names who did what in
the audit log.

## Proposal

### 1. Settings are a typed record, kept where the desk is kept

A `Settings` struct in the canvas with named, typed fields and defaults,
serialized as JSON with unknown fields ignored and missing ones
defaulted, so an older page and a newer box (or the reverse) never refuse
each other. First fields, all of which exist as preferences today:

| Setting | Type | Default | Today |
|---|---|---|---|
| Light mode | bool | off | `light=` |
| Word wrap in the code | bool | on | `wrap=` |
| Trend colours per signal | map | none | `colors=` |
| I/O tab as tiles or rows | enum | rows | `iogrid=` |
| Advanced fields in forms | bool | off | `advanced=` |

The layout of the panes (which tab is where, the sizes, what is folded)
stays out of Settings: it is the arrangement of one screen, and a wall
display and a laptop want different ones. It stays per browser, with the
named pane layouts on the box as the way to carry one across.

Where a record lives:

- **Signed in on a Skipper:** on the device, per person, beside the desk
  (`<state-dir>/settings/<person>.json`, a few hundred bytes, same owner
  rule as the desk: the account, or the role for a role-PIN session).
  `GET`/`PUT /api/settings`, any signed-in session for its own record,
  bounded like the desk (a small byte cap, a cap on people). The browser
  keeps a copy and uses it until the box has answered, so the page does
  not flash from dark to light.
- **Signed out on a Skipper:** the device's defaults, which an integrator
  may set (`<state-dir>/settings/default.json`): a plant that wants light
  mode on its floor screens sets it once. The browser's copy only as the
  fallback when the box does not answer.
- **The playground:** the browser, as now. There is no device.

Which wins when both exist: the newer, by a `saved_at` in the record, the
same rule the desk uses. A setting changed while the box is unreachable
is kept in the browser and sent on the next answer.

### 2. One Settings window

A Settings item in the menu (the one hamburger menu, card
`fb-global-chrome`) opens a window with the table above as rows: a name,
a control, a one-line tooltip. It says at the top where they are kept
("On this Skipper, for alice" / "In this browser"). The View menu keeps
only what changes the screen in front of you (panes, layout, fit).

"Reset after a described change" does not move here: the switch is gone
(Nick, 2026-11-23), and a described change runs at once in the simulation.

### 3. The first account, from the device itself

Today the first account needs a shell. For a preloaded Skipper:

- A box with **no accounts file and no PINs set** serves a first-run page
  instead of the app: name, password twice, and the role is integrator
  (the first person sets the box up). It is offered only from the device
  itself or its own subnet's first connection, only until the first
  account exists, and it is in the audit log. This is the part with real
  security weight; see the questions.
- After that, **an integrator manages accounts in the UI**: add a person
  with a role, disable, reset a password. It calls what the CLI calls;
  the CLI stays.
- **A person changes their own password** in the Settings window.

No password recovery by email or cloud: an integrator resets it, and a
box with no integrator left is recovered at its shell
(`helmsman-plc account passwd`), which needs the device in hand.

### 4. What does not change

- Roles, the PIN on apply and rollback, the audit log, lockouts, session
  lifetimes.
- A Skipper never needs the internet to sign in or to keep settings.
- The relay is not involved and learns nothing about accounts or
  settings.

## What I would not do

- **A second, lighter user model for preferences** ("profiles" without
  passwords). It would be a second list of people to keep in step with
  the accounts, and anyone could pick anyone's profile. Signed-out
  screens get the device defaults instead.
- **Sync through the relay or a cloud account.** It contradicts "no cloud
  to run the machine" and makes preferences depend on a service.
- **Putting the pane arrangement in Settings.** See above.

## Questions for Nick (each changes what gets built)

1. **First account on a preloaded Skipper: from its own screen, or with a
   code printed on the device?** A first-run page open to whoever reaches
   the box first is the simplest and is how most network gear works, but
   on a shared plant network the first to connect owns the box. The
   alternative is a one-time setup code that ships with the device (on a
   label, or a file on its boot partition beside the pairing files
   `deploy/helmsman-firstboot.sh` already reads), typed on the first-run
   page. My recommendation is the code: one more line to type,
   and it closes the race.
2. **Do signed-out screens get to change settings at all?** My
   recommendation is no: they show the device's defaults, and changes
   made signed out stay in that browser only. Otherwise the last person
   to touch any screen sets it for all.
3. **Is the session length right for a floor screen?** One hour, or 15
   minutes unused, is right for an engineer's laptop and wrong for an
   operator's wall display, which would sign out all day. A per-role
   session length (an operator's lasting a shift) is a small change in
   `SessionLimits`; I would add it with this work if the answer is yes.
4. **Account management in the UI in the first cut, or settings first?**
   Settings on the device (sections 1 and 2) are about a week and touch
   no security code. The first-run page and account management
   (section 3) are the larger and more sensitive half. My recommendation
   is two steps in that order.

## Size, if the answers are the recommended ones

| Step | What | Size |
|---|---|---|
| A | `Settings` record, `GET`/`PUT /api/settings`, the Settings window, migration from the `Chrome` line, device defaults | about a week |
| B | First-run page with a setup code, accounts in the UI, change own password, per-role session length | about two weeks, with a security review of the first-run path |

Step A needs a new feature row in docs/FEATURES.md (the route) with its
journey; step B needs a docs/security page update.
