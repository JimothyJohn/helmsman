#!/usr/bin/env python3
"""Write the three rough cuts of the device film as HyperFrames compositions.

    ./build.py        # writes cut-a/, cut-b/ and cut-c/, each a HyperFrames project

Each cut is a table below: shots (footage from footage/shots.py, copied to
assets/footage/), captions and cards on a 120 BPM grid (a beat is 0.5 s), in
the look of ../helmsman-ad-v2. Copy comes from docs/index.html; change it
there first. render.sh runs this, then checks and renders.
"""

from __future__ import annotations

import html
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Measured in the clips (footage/outputs/box/sheets): seconds into the clip.
PLUG_LIGHT = 3.2  # the light bar comes on (the plug's click is at 3.1)
LINE_GREEN = 0.8  # the stack light comes on; the cartons roll a second later
MOUNT_CLICK = 2.94  # the block seats on the rail
RACK_THUD = 2.82
DOOR_CLUNK = 2.4
APP_START = 3.34  # app-latch.mp4: the press on START


def ms(event: float, shot_at: float, land_at: float) -> float:
    """The media offset that lands a clip's event on a beat of the film."""
    return round(event - (land_at - shot_at), 2)


class Film:
    def __init__(self, name: str, title: str, dur: float):
        self.name, self.title, self.dur = name, title, dur
        self.body: list[str] = []
        self.audio: list[str] = []
        self.js: list[str] = []
        self.n = 0
        self.track = 30

    def _id(self, prefix: str) -> str:
        self.n += 1
        return f"{prefix}{self.n}"

    def shot(self, at, dur, clip, media=0.5, old=False, zoom=(1.0, 1.06), sound=0.0):
        """Footage, full frame. `old` is the drained grade of the way things were."""
        i = self._id("v")
        grade = " old" if old else ""
        self.body.append(
            f'<div class="wrap{grade}" id="w{i}"><video id="{i}" class="clip shot" data-start="{at}" '
            f'data-duration="{dur}" data-media-start="{media}" data-track-index="1" '
            f'src="assets/footage/{clip}.mp4" muted playsinline></video></div>'
        )
        self.js.append(
            f'tl.fromTo("#w{i}", {{scale: {zoom[0]}}}, {{scale: {zoom[1]}, duration: {dur}, ease: "none"}}, {at});'
        )
        if sound:
            self.sfx(at, f"real/{clip}.mp3", dur, sound, media=media, fade=0.2)

    def app(self, at, dur, media):
        """The real app's diagram (the recording the earlier ads use)."""
        i = self._id("v")
        self.body.append(
            f'<div class="wrap" id="w{i}"><video id="{i}" class="clip shot app" data-start="{at}" '
            f'data-duration="{dur}" data-media-start="{media}" data-track-index="2" '
            'src="assets/app-latch.mp4" muted playsinline></video></div>'
        )
        self.js.append(f'gsap.set("#w{i}", {{transformOrigin: "0px 0px", scale: 2.0, x: -460, y: -316, clipPath: "inset(250px 830px 495px 430px)"}});')
        self.sfx(round(at + APP_START - media, 2), "sfx/click.mp3", 0.36, 0.8)

    def caps(self, at, dur, lines, kicker=None, step=None, big=False):
        """Lines of copy on label tape, bottom left; a kicker or step top left.
        lines: (seconds after `at`, text, "w" white on black | "y" black on yellow)."""
        g = self._id("c")
        parts = [
            f'<div id="{g}" class="clip full" data-start="{at}" data-duration="{dur}" '
            'data-track-index="3" data-track-kind="graphics" data-layout-ignore>'
        ]
        if kicker:
            parts.append(f'<p id="{g}k" class="kicker cond">{html.escape(kicker)}</p>')
        if step:
            cls = "step big stencil" if big else "step stencil"
            parts.append(f'<p id="{g}s" class="{cls}">{html.escape(step)}</p>')
            self.js.append(f'slam("#{g}s", {at + 0.02}, 1.3); jolt({at + 0.02}, {12 if big else 6});')
        parts.append('<div class="lines">')
        for j, (off, text, tone) in enumerate(lines):
            parts.append(f'<p id="{g}l{j}" class="line cond {tone}">{html.escape(text)}</p>')
            if at + off > 0:  # frame 0 is the thumbnail: its line is already there
                self.js.append(f'slam("#{g}l{j}", {round(at + off + 0.02, 2)}, 1.2); jolt({round(at + off + 0.02, 2)}, 4);')
        parts.append("</div></div>")
        self.body.append("".join(parts))
        self.js.append(f'gsap.set("#{g} .line, #{g} .step", {{transformOrigin: "0% 50%"}});')

    def slash(self, at):
        """The yellow slash: across at `at`, the frame is yellow at at+0.5, gone by at+0.75."""
        g = self._id("s")
        self.body.append(
            f'<div id="{g}" class="clip full" data-start="{at}" data-duration="0.75" data-track-index="5" '
            f'data-track-kind="graphics" data-layout-ignore><div class="slash-wrap"><div class="slash"></div></div>'
            '<div class="flood"></div></div>'
        )
        self.js += [
            f'gsap.set("#{g} .slash-wrap", {{rotation: -6, transformOrigin: "50% 50%"}});',
            f'gsap.set("#{g} .slash", {{transformOrigin: "0% 50%"}});',
            f'tl.fromTo("#{g} .slash", {{scaleX: 0}}, {{scaleX: 1, duration: 0.18, ease: "power3.out"}}, {at});',
            f'tl.to("#{g} .slash", {{scaleY: 20, duration: 0.27, ease: "power3.in"}}, {at + 0.2});',
            f'tl.fromTo("#{g} .flood", {{opacity: 0}}, {{opacity: 1, duration: 0.01}}, {at + 0.47});',
            f'tl.to("#{g} .slash", {{opacity: 0, duration: 0.01}}, {at + 0.5});',
            f'tl.fromTo("#{g} .flood", {{clipPath: "inset(0% 0% 0% 0%)"}}, '
            f'{{clipPath: "inset(0% 0% 0% 100%)", duration: 0.22, ease: "power2.inOut", immediateRender: false}}, {at + 0.5});',
            f"jolt({at + 0.5}, 16);",
        ]
        self.sfx(round(at - 0.1, 2), "sfx/whoosh.mp3", 0.5, 0.6)
        self.sfx(round(at + 0.49, 2), "sfx/impact-bass-1.mp3", 2.0, 0.7, fade=0.6)

    def claim(self, at, dur, words, hot, sub):
        g = self._id("k")
        spans = " ".join(
            "<br />" if w == "/" else f'<span class="w{" hot" if w == hot else ""}">{html.escape(w)}</span>' for w in words
        )
        self.body.append(
            f'<div id="{g}" class="clip full claim" data-start="{at}" data-duration="{dur}" data-track-index="4" '
            f'data-track-kind="graphics"><h1 class="stencil">{spans}</h1><p class="cond sub">{html.escape(sub)}</p></div>'
        )
        k = 0
        for w in words:
            if w == "/":
                continue
            k += 1
            self.js.append(f'slam("#{g} h1 .w:nth-of-type({k})", {round(at + 0.05 + (k - 1) * 0.25, 2)}, 1.6);')
        self.js.append(f"jolt({round(at + 0.05 + (k - 1) * 0.25, 2)}, 10);")
        self.js.append(f'blurIn("#{g} .sub", {round(at + 0.3 + k * 0.25, 2)});')

    def stack(self, at, dur, label, facts):
        """What is built, a line a beat, over the footage dimmed."""
        g = self._id("f")
        rows = "".join(f'<p class="fact cond" style="top: {210 + 130 * i}px"><i></i>{html.escape(f)}</p>' for i, f in enumerate(facts))
        self.body.append(
            f'<div id="{g}" class="clip full stack" data-start="{at}" data-duration="{dur}" data-track-index="6" '
            f'data-track-kind="graphics" data-layout-ignore><p class="built cond">{html.escape(label)}</p>{rows}</div>'
        )
        self.js.append(f'gsap.set("#{g} .fact", {{transformOrigin: "0% 50%"}});')
        for i in range(len(facts)):
            t = round(at + 0.25 + i * 0.5, 2)
            self.js.append(f'slam("#{g} .fact:nth-of-type({i + 2})", {t}, 1.25); jolt({t}, 4);')

    def end(self, at):
        self.body.append(
            f'<div id="end" class="clip full" data-start="{at}" data-duration="{round(self.dur - at, 2)}" data-track-index="7" '
            'data-track-kind="graphics"><div id="lockup"><div id="mark"><img src="assets/helmsman-mark-black.svg" alt="" /></div>'
            '<div><p id="wordmark" class="stencil">Helmsman</p><p id="descriptor" class="cond">Factory automation</p></div></div>'
            '<div id="cta"><p id="cta-btn">Try it in your browser</p><p id="cta-url">helmsman.advin.io</p></div>'
            '<p id="early"><b>Early release</b><span>Open source (Apache 2.0)</span></p></div>'
        )
        self.js += [
            f'tl.fromTo("#mark", {{scale: 3.2, rotation: -120, opacity: 0}}, {{scale: 1, rotation: 0, opacity: 1, duration: 0.18, ease: "power4.in"}}, {at});',
            f"jolt({at + 0.18}, 20);",
            f'tl.fromTo("#wordmark", {{clipPath: "inset(0% 100% 0% 0%)"}}, {{clipPath: "inset(0% 0% 0% 0%)", duration: 0.4, ease: land}}, {at + 0.45});',
            f'tl.fromTo("#descriptor", {{opacity: 0}}, {{opacity: 1, duration: 0.25}}, {at + 0.75});',
            f'tl.fromTo("#cta-btn", {{opacity: 0, scale: 0.9}}, {{opacity: 1, scale: 1, duration: 0.3, ease: "back.out(1.4)"}}, {at + 1});',
            f'tl.fromTo("#cta-url", {{opacity: 0}}, {{opacity: 1, duration: 0.25}}, {at + 1.2});',
            f'tl.fromTo("#early", {{opacity: 0}}, {{opacity: 1, duration: 0.25}}, {at + 1.5});',
        ]
        # The riser's peak and the impact land on the mark, as in the second ad.
        self.sfx(round(at - 3.26, 2), "sfx/riser.mp3", 3.48, 0.5, fade_in=0.8)
        self.sfx(round(at + 0.14, 2), "sfx/impact-bass-1.mp3", 2.1, 0.8, fade=0.8)

    def sfx(self, at, src, dur, vol, media=None, fade=None, fade_in=None):
        self.track += 1
        extra = ""
        if media is not None:
            extra += f' data-media-start="{media}"'
        if fade:
            extra += f' data-fade-out="{fade}"'
        if fade_in:
            extra += f' data-fade-in="{fade_in}"'
        self.audio.append(
            f'<audio id="a{self.track}" class="clip" data-start="{at}" data-duration="{dur}" '
            f'data-track-index="{self.track}" data-volume="{vol}"{extra} src="assets/{src}"></audio>'
        )

    def write(self):
        bed = (
            f'<audio id="bed" class="clip" data-start="0" data-duration="{self.dur}" data-track-index="20" '
            'data-volume="0.8" data-fade-out="1.2" src="assets/music/bed.mp3"></audio>'
        )
        page = PAGE.format(
            title=html.escape(self.title),
            dur=self.dur,
            body="\n        ".join(self.body),
            audio="\n      ".join([bed] + self.audio),
            js="\n      ".join(self.js),
        )
        out = HERE / f"cut-{self.name}"
        out.mkdir(exist_ok=True)
        (out / "index.html").write_text(page)
        for shared in ("assets", "film.css", "hyperframes.json"):  # one project a cut, one set of files
            link = out / shared
            if not link.is_symlink():
                link.symlink_to(f"../{shared}")


FACTS = [
    "Over 1,300 tests on every change",
    "A fixed scan, bounded by a watchdog",
    "Standard IEC 61131-3 Structured Text",
    "EtherNet/IP and Modbus TCP",
    "Changes taken between two scans",
    "No licence keys",
]


def cut_a() -> Film:
    """One cable: commissioning day, from the old way to a closed cabinet door."""
    f = Film("a", "Helmsman device film, cut A: one cable", 32)
    f.shot(0, 2, "oldway", old=True)
    f.caps(0, 2, [(0, "A laptop with the right version", "w"), (1, "A programming cable", "w")], kicker="Commissioning, the old way")
    f.shot(2, 2, "dongles", old=True, sound=0.5)
    f.caps(2, 1.97, [(0, "A licence dongle, in some drawer", "w")], kicker="Commissioning, the old way")
    f.slash(3.5)
    f.shot(4, 2.5, "hand")
    f.caps(4.25, 2.25, [(0.25, "It fits in your hand", "w"), (1.25, "One Ethernet port", "y")], kicker="The Helmsman controller")
    f.shot(6.5, 2.5, "mount", media=ms(MOUNT_CLICK, 6.5, 8.0), sound=0.7)
    f.caps(6.5, 2.5, [(0, "Clip it on the rail", "w")], kicker="Install")
    f.shot(9, 4, "plug", media=ms(PLUG_LIGHT, 9, 11), zoom=(1.0, 1.1), sound=0.9)
    f.caps(9, 4, [(0, "Plug in one cable", "w"), (2, "The controller is up", "y")], kicker="Install")
    f.shot(13, 2.5, "live")
    f.caps(13, 2.5, [(0, "It scans the network", "w"), (1, "and recognises your devices", "w")], kicker="Install")
    f.shot(15.5, 3, "describe")
    f.caps(15.5, 3, [(0, "Open a browser, nothing to install", "w"), (1.5, "Describe the machine in plain words", "w")], kicker="Program")
    f.app(18.5, 3, media=2.6)
    f.caps(18.5, 3, [(0, "Check the program on a live diagram", "w")], kicker="Program")
    f.shot(21.5, 2.5, "linestart", media=ms(LINE_GREEN, 21.5, 22), sound=0.6)
    f.caps(21.5, 2.5, [(0, "Approve it with your PIN", "w"), (0.5, "The line runs", "y")], kicker="Program")
    f.shot(24, 4, "walkaway", media=ms(DOOR_CLUNK, 24, 26), sound=0.6)
    f.caps(24, 4, [(0.5, "No workstation software", "w"), (1.5, "No licence keys", "w"), (2.5, "No programming cable", "w")], kicker="Close the door")
    f.end(28)
    return f


def cut_b() -> Film:
    """The change: production wants a change at 2 pm and the line keeps running."""
    f = Film("b", "Helmsman device film, cut B: the change", 32)
    f.shot(0, 2, "running")
    f.caps(0, 2, [(0, "Production wants a change", "w")], kicker="2 pm, the line is running")
    f.shot(2, 2, "oldway", old=True)
    f.caps(2, 1.97, [(0, "That used to mean a stop", "w"), (1, "and a late night at the cabinet", "w")], kicker="2 pm, the line is running")
    f.slash(3.5)
    f.claim(4, 2.5, ["Change", "it", "/", "while", "it", "runs"], "runs", "Helmsman: the controller you program by describing the machine")
    f.shot(6.5, 3, "describe")
    f.caps(6.5, 3, [(0.5, "Say what should change, in plain words", "w")], step="1. Describe")
    f.app(9.5, 4, media=2.3)
    f.caps(9.5, 4, [(0.5, "Watch it on a live diagram", "w"), (2, "Test it with no machine connected", "w")], step="2. Validate")
    f.shot(13.5, 2.5, "approve", media=0.3)
    f.caps(13.5, 2.5, [(0.5, "Sign off with your PIN", "w")], step="3. Approve")
    f.shot(16, 3, "tablet")
    f.caps(16, 3, [(0, "It goes live between two scans", "w"), (1.5, "The line never stopped", "y")])
    f.shot(19, 2.5, "live")
    f.caps(19, 2.5, [(0, "Every version is kept", "w"), (1, "Roll back to any of them", "w")], kicker="In the cabinet")
    f.shot(21.5, 3, "plug", media=ms(PLUG_LIGHT, 21.5, 23), zoom=(1.0, 1.1), sound=0.9)
    f.caps(21.5, 3, [(0, "One controller", "w"), (1.5, "One Ethernet cable", "y")], kicker="In the cabinet")
    f.shot(24.5, 3.5, "coffee")
    f.caps(24.5, 3.5, [(0, "No stop and no late night", "w"), (1.5, "You watch it work", "y")])
    f.end(28)
    return f


def cut_c() -> Film:
    """Out and in: what leaves the cabinet, what goes in, what it does."""
    f = Film("c", "Helmsman device film, cut C: out and in", 28)
    f.shot(0, 1.5, "rackout", media=ms(RACK_THUD, 0, 1), old=True, sound=0.8)
    f.caps(0, 1.5, [(0, "The rack that fills the cabinet", "w")], step="Out", big=True)
    f.shot(1.5, 1.5, "dongles", old=True, sound=0.5)
    f.caps(1.5, 1.5, [(0, "Licence keys and dongles", "w")], step="Out", big=True)
    f.shot(3, 1.5, "oldway", old=True)
    f.caps(3, 1.47, [(0, "The laptop with one vendor's software", "w")], step="Out", big=True)
    f.slash(4)
    f.shot(4.5, 3, "bench")
    f.caps(4.75, 2.75, [(0.75, "One controller", "w"), (1.75, "One Ethernet port", "w")], step="In", big=True)
    f.shot(7.5, 1.5, "mount", media=ms(MOUNT_CLICK, 7.5, 8.5), sound=0.7)
    f.caps(7.5, 1.5, [(0, "On the rail", "w")])
    f.shot(9, 3, "plug", media=ms(PLUG_LIGHT, 9, 10.5), zoom=(1.0, 1.1), sound=0.9)
    f.caps(9, 3, [(0, "One cable", "w"), (1.5, "Running", "y")])
    f.shot(12, 2, "describe")
    f.caps(12, 2, [(0, "You describe the machine", "w")])
    f.app(14, 2.5, media=2.6)
    f.caps(14, 2.5, [(0, "The agent writes Structured Text", "w"), (1.5, "You check it on a live diagram", "w")])
    f.shot(16.5, 2, "approve", media=0.3)
    f.caps(16.5, 2, [(0, "You approve it with a PIN", "w")])
    f.shot(18.5, 2, "linestart", media=ms(LINE_GREEN, 18.5, 19), sound=0.6)
    f.caps(18.5, 2, [(0, "It drives the I/O you already have", "w"), (0.5, "EtherNet/IP and Modbus TCP", "y")])
    f.shot(20.5, 3.5, "running")
    f.stack(20.5, 3.5, "What is built", FACTS)
    f.end(24)
    return f


PAGE = """<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=1920, height=1080" />
    <title>{title}</title>
    <script src="https://cdn.jsdelivr.net/npm/gsap@3.15.0/dist/gsap.min.js"></script>
    <link rel="stylesheet" href="film.css" />
  </head>
  <body>
    <!-- Written by build.py: edit the cut's table there, not this file. -->
    <div id="root" data-composition-id="main" data-start="0" data-duration="{dur}" data-width="1920" data-height="1080" data-motion-blur="on">
      <div id="shake">
        {body}
      </div>
      {audio}
    </div>
    <script>
      const tl = gsap.timeline({{ paused: true }});
      const land = "expo.out";
      // A short decaying shake of the whole frame, the same every render.
      const jolt = (at, amp) => {{
        [[amp, -amp * 0.6], [-amp * 0.7, amp * 0.5], [amp * 0.4, -amp * 0.3], [-amp * 0.15, amp * 0.1], [0, 0]].forEach(([x, y], i) =>
          tl.to("#shake", {{ x, y, duration: 0.033, ease: "none" }}, at + i * 0.033),
        );
      }};
      const slam = (sel, at, from = 1.35) =>
        tl.fromTo(sel, {{ opacity: 0, scale: from, filter: "blur(12px)" }}, {{ opacity: 1, scale: 1, filter: "blur(0px)", duration: 0.2, ease: land }}, at);
      const blurIn = (sel, at) =>
        tl.fromTo(sel, {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.14, ease: "power2.out" }}, at);
      {js}
      window.__timelines = window.__timelines || {{}};
      window.__timelines["main"] = tl;
      tl.seek(0);
    </script>
  </body>
</html>
"""

if __name__ == "__main__":
    for cut in (cut_a, cut_b, cut_c):
        cut().write()
