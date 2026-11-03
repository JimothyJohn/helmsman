# The Agent

The Agent is the way you program by describing the machine. Open the
**Agent** tab in the side panel (<kbd>Alt</kbd> <kbd>3</kbd>, then the
tab), say what you want, and it answers with Structured Text and an
explanation.

## In the simulation

The playground calls the model from your browser with your own API key.
Paste it in the tab; "Remember on this device" keeps it in this browser
only, and the tab says plainly where the key is kept. Running the
playground from your own machine with a local proxy, or natively with
the key in the environment, the tab says the key is preloaded so you are
not left wondering.

<img src="img/agent-tab.png" alt="The Agent tab: the key, the model, the question and Ask." width="240">

Ask for a program ("a conveyor with a photo eye that stops after three
parts"), for a change to the open one ("add a two-second delay before the
belt starts"), or for an explanation ("why does the lamp blink?"). The
answer's code goes into the editor with **Use this code**; nothing runs
until you deploy it, and the editor marks any error first.

## Pages

Every question and its answer is kept as a page of the conversation,
with the project: reload the page or download the project and they are
still there (the file is `agent.md`; the build ignores it). The ‹ and ›
buttons walk the pages, **Delete page** drops the one you are reading,
and **New conversation** drops them all. Every page you keep goes back to
the Agent with your next question, so pruning the pages is how you shape
what it sees. An earlier page's code has its own **Use this code**.

## On a box

On a box the Agent goes through a relay that holds the key, so no box
and no browser holds it. What the Agent proposes becomes a **proposal**
on the box, with a diff and a replay in the simulation, that an approver
signs off under the box's policy before it runs. The Agent tab on a box
is still to come; proposals already arrive and are reviewed from the
Proposals screen (the header's **waiting** badge).

## What it will not do

The Agent writes and explains; it never deploys. It cannot force an
input, change the I/O map on a box, or approve anything. Its code is
checked like yours: an error is marked, a deploy that does not compile
is refused.
