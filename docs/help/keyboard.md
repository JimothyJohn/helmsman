# Keyboard

CODESYS's default keys where no browser uses them, and nothing on a key a browser owns (Ctrl D bookmarks the page, Alt 1 is Firefox's first tab, F10 the toolbar), checked against Chrome, Edge, Firefox and Safari. The editor's own conventions stay: Ctrl S, Ctrl Z, Ctrl Shift Z, Ctrl F and Ctrl G, as in every editor on the web. **Ctrl is ⌘ on a Mac.** Every menu item shows its key.

| key | does | CODESYS command on that key |
|---|---|---|
| Ctrl S | Save | Save Project |
| Ctrl Shift Enter | Deploy |  |
| Ctrl Enter | Run or stop the simulation |  |
| Shift F8 | Stop the simulation | Stop |
| F8 | Step one scan (stopped) | Step Into |
| Alt F8 | Sign in (box) | Login |
| Ctrl F8 | Sign out (box) | Logout |
| Ctrl Z | Undo in the Structured Text | Undo |
| Ctrl Shift Z | Redo in the Structured Text |  |
| Ctrl Y (not on a Mac: ⌘Y is the browser's History) | Redo in the Structured Text | Restore |
| Alt Shift D | Go to definition: the picked block's variable, at its VAR line |  |
| Alt Shift X | Cross references: every read and write of it lit |  |
| Shift F9 | Breakpoint on the picked block (simulation): the scan stops before it; F8 runs the rest of the scan, Ctrl Enter goes on to the next stop |  |
| Alt Shift F | Force the picked input: a switch flips, a number's field opens (on a box, an engineer's audited force) |  |
| Alt Shift U | Release every force on the box |  |
| Alt ] | Into the picked block instance's diagram; a stopped simulation steps one scan |  |
| Alt [ | Out to the caller's diagram; a stopped simulation steps one scan |  |
| Ctrl F | Find in the Structured Text (Enter: next, Esc: close) |  |
| Ctrl G | The next match |  |
| Ctrl Shift G | The previous match |  |
| Alt Shift 0 | The left pane's Files tab | Devices |
| Alt Shift 1 | The left pane's Code tab (the Structured Text) | POUs |
| Alt Shift 2 | The bottom panel (Trend, Messages) | Modules |
| Alt Shift 3 | The side panel (I/O, Agent) | Messages |
| Alt Shift 4 | The left pane's Devices tab (the I/O map's devices and points) |  |
| Alt ↑ | Up one level in the diagram | |

Reserved, not built yet: F2 / Shift F2 for the Agent.
