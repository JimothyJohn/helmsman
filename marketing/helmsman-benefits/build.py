#!/usr/bin/env python3
"""Write the benefit films as HyperFrames compositions.

    ./build.py        # writes loop-<name>/ for each benefit and reel/, each a HyperFrames project

One short loop per benefit Nick names in his 2026-11-25 demo recordings,
and a reel of all of them, in the look and on the 120 BPM grid of
../helmsman-ad-v2 (a beat is 0.5 s). Every app picture is the real
playground, recorded by tools/record.py into assets/app/<take>.mp4 with
its events in <take>.json; the cursor, its click rings and the click and
typing sounds are drawn from those events, so a new recording moves them
with it. The bench shots are Nick's own footage, cut by tools/bench.sh.
Copy comes from docs/index.html; change it there first. render.sh runs
this, then checks and renders.
"""

from __future__ import annotations

import html
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
APP = HERE / "assets" / "app"
PX = 1.5  # recorded points to frame pixels (1280x720 points in 1920x1080)
REST = (640, 12)

CURSOR_SVG = (
    '<svg class="cursor" viewBox="0 0 32 32" aria-hidden="true"><path d="M4 2 L4 26 L10.5 20 L15 30 L19 28 '
    'L14.5 18.5 L23 18.5 Z" fill="#ffffff" stroke="#141414" stroke-width="2" stroke-linejoin="round" /></svg>'
)


def events(take: str) -> list[dict]:
    return json.loads((APP / f"{take}.json").read_text())["events"]


def view(s: float, x: float, y: float) -> dict:
    """The camera on the app: its scale and the frame pixel at the top-left corner."""
    return {"scale": s, "x": round(-x * s, 1), "y": round(-y * s, 1)}


FULL = (1.0, 0, 0)


def js_obj(d: dict) -> str:
    return "{" + ", ".join(f"{k}: {v}" for k, v in d.items()) + "}"


class Film:
    def __init__(self, name: str, title: str, dur: float):
        self.name, self.title, self.dur = name, title, dur
        self.body: list[str] = []
        self.audio: list[str] = []
        self.js: list[str] = []
        self.beds: list[tuple[float, float, float, float]] = []
        self.n = 0
        self.track = 30
        self.app_track = 0

    def _id(self, prefix: str) -> str:
        self.n += 1
        return f"{prefix}{self.n}"

    # ---- pictures -------------------------------------------------------------------

    def app(self, at, dur, take, media, views=((0, *FULL),)):
        """A stretch of a recorded take: media..media+dur of it at film time `at`.
        views: (seconds after `at`, scale, x, y) camera moves, the first one set at `at`.
        The cursor follows the take's own events."""
        i = self._id("a")
        # Two app clips back to back alternate tracks, so a cut never overlaps itself.
        self.app_track = 1 - self.app_track
        vt, ct = 1 + 2 * self.app_track, 2 + 2 * self.app_track
        self.body.append(
            f'<div class="cam" id="cam{i}" data-layout-allow-overflow>'
            f'<video id="v{i}" class="clip app" data-start="{at}" data-duration="{dur}" data-media-start="{media}" '
            f'data-track-index="{vt}" src="assets/app/{take}.mp4" muted playsinline></video>'
            f'<div id="k{i}" class="clip full" data-start="{at}" data-duration="{dur}" data-track-index="{ct}" '
            f'data-track-kind="graphics" data-layout-ignore><div class="ptr" id="p{i}" style="position:absolute;left:0;top:0">'
            f'{CURSOR_SVG}<div class="ring"></div></div></div></div>'
        )
        first, *rest = views
        self.js.append(f'gsap.set("#cam{i}", {js_obj(view(*first[1:]))});')
        for off, s, x, y in rest:
            self.js.append(
                f'tl.to("#cam{i}", {{...{js_obj(view(s, x, y))}, duration: 0.5, ease: "power2.inOut"}}, {round(at + off, 3)});'
            )
        self._cursor(i, at, dur, take, media)

    def _cursor(self, i, at, dur, take, media):
        evs = events(take)
        pos = REST
        for e in evs:
            if e["t"] < media and e["x"] is not None:
                pos = (e["x"], e["y"])
        self.js.append(f'gsap.set("#p{i}", {{x: {pos[0] * PX}, y: {pos[1] * PX}}});')
        root = lambda t: round(at + t - media, 3)  # noqa: E731
        down_at = None
        for e in evs:
            t, kind = e["t"], e["action"]
            if not (media <= t < media + dur):
                continue
            if kind == "move":
                d = max(0.18, round(e["end"] - t, 3))
                self.js.append(
                    f'tl.to("#p{i}", {{x: {e["x"] * PX}, y: {e["y"] * PX}, duration: {d}, ease: "power2.inOut"}}, {root(t)});'
                )
            elif kind in ("click", "dbl", "upload", "down"):
                self.js.append(f'gsap.set("#p{i} .cursor", {{transformOrigin: "12% 6%"}});')
                self.js.append(f'tl.to("#p{i} .cursor", {{scale: 0.8, duration: 0.06}}, {root(t) - 0.03});')
                if kind != "down":
                    self.js.append(f'tl.to("#p{i} .cursor", {{scale: 1, duration: 0.12}}, {root(t) + 0.1});')
                else:
                    down_at = t
                self.js.append(
                    f'tl.fromTo("#p{i} .ring", {{scale: 0.3, opacity: 1}}, {{scale: 1.5, opacity: 0, duration: 0.4, ease: "power2.out", immediateRender: false}}, {root(t)});'
                )
                self.sfx(root(t), "sfx/click.mp3", 0.36, 0.8)
                if kind == "dbl":
                    self.sfx(root(t) + 0.12, "sfx/click.mp3", 0.36, 0.6)
            elif kind == "up" and down_at is not None:
                self.js.append(f'tl.to("#p{i} .cursor", {{scale: 1, duration: 0.12}}, {root(t)});')
            elif kind == "type":
                left = round(min(e["end"] - t, media + dur - t), 2)
                k = 0
                while left > 0.1:  # the effect is 1.5 s long: repeat it under long typing
                    self.sfx(root(t) + 1.4 * k, "sfx/typing.mp3", round(min(left, 1.5), 2), 0.5, fade=0.15)
                    left, k = round(left - 1.4, 2), k + 1
            elif kind == "key":
                self.sfx(root(t), "sfx/click.mp3", 0.36, 0.7)

    def bench(self, at, dur, clip, media=0.0, sound=0.0, zoom=(1.0, 1.08)):
        """Nick's bench footage, full frame, a slow push in."""
        i = self._id("b")
        self.app_track = 1 - self.app_track
        vt = 1 + 2 * self.app_track
        self.body.append(
            f'<div class="wrap" id="w{i}"><video id="{i}" class="clip shot" data-start="{at}" data-duration="{dur}" '
            f'data-media-start="{media}" data-track-index="{vt}" src="assets/bench/{clip}.mp4" muted playsinline></video></div>'
        )
        self.js.append(f'tl.fromTo("#w{i}", {{scale: {zoom[0]}}}, {{scale: {zoom[1]}, duration: {dur}, ease: "none"}}, {at});')
        if sound:
            self.sfx(at, f"bench/{clip}.wav", dur, sound, media=media, fade=0.3)

    # ---- copy -----------------------------------------------------------------------

    def caps(self, at, dur, lines, kicker=None, right=False):
        """Lines of copy on label tape, bottom left (or right); a kicker top left.
        lines: (seconds after `at`, text, "w" white on black | "y" black on yellow)."""
        g = self._id("c")
        style = ' style="left:auto;right:96px;align-items:flex-end"' if right else ""
        parts = [f'<div id="{g}" class="clip full" data-start="{at}" data-duration="{dur}" data-track-index="10" data-track-kind="graphics" data-layout-ignore>']
        if kicker:
            parts.append(f'<p id="{g}k" class="kicker cond">{html.escape(kicker)}</p>')
            self.js.append(f'blurIn("#{g}k", {at});')
        parts.append(f'<div class="lines"{style}>')
        for j, (off, text, tone) in enumerate(lines):
            parts.append(f'<p id="{g}l{j}" class="line cond {tone}">{html.escape(text)}</p>')
            t = round(at + off + 0.02, 2)
            self.js.append(f'slam("#{g}l{j}", {t}, 1.2); jolt({t}, 4);')
        parts.append("</div></div>")
        self.body.append("".join(parts))
        origin = "100% 50%" if right else "0% 50%"
        self.js.append(f'gsap.set("#{g} .line", {{transformOrigin: "{origin}"}});')

    def later(self, at, dur, text):
        """Where real time was cut, say so."""
        g = self._id("t")
        self.body.append(
            f'<div id="{g}" class="clip full" data-start="{at}" data-duration="{dur}" data-track-index="11" '
            f'data-track-kind="graphics" data-layout-ignore><p class="later">{html.escape(text)}</p></div>'
        )
        self.js.append(f'blurIn("#{g} .later", {at});')

    def before(self, at, gripe):
        """The complaint from the site's "From the field", struck out: 2 s, the next picture from at+1.5."""
        g = self._id("g")
        self.body.append(
            f'<div id="{g}" class="clip full before" data-start="{at}" data-duration="1.5" data-track-index="6" '
            f'data-track-kind="graphics"><p class="tag cond">Before</p><p class="gripe cond">{html.escape(gripe)}</p></div>'
        )
        self.js.append(f'gsap.set("#{g} .gripe", {{transformOrigin: "0% 50%"}});')
        if at > 0:  # frame 0 is the thumbnail: its complaint is already there
            self.js.append(f'slam("#{g} .gripe", {at + 0.02}, 1.25); jolt({at + 0.02}, 6);')
        self.slash(at + 1.0)

    def gripes(self, at, dur, label, lines):
        """The reel's opener: complaints stacked a beat apart, then one slash through them."""
        g = self._id("g")
        rows = "".join(
            f'<p class="gripe cond" style="position:absolute;left:160px;top:{270 + 150 * k}px;font-size:104px;color:var(--ink);white-space:nowrap">'
            f"{html.escape(t)}</p>"
            for k, t in enumerate(lines)
        )
        self.body.append(
            f'<div id="{g}" class="clip full" data-start="{at}" data-duration="{dur}" data-track-index="6" data-track-kind="graphics">'
            f'<p class="cond" style="position:absolute;left:160px;top:130px;color:var(--yellow);font-size:44px;letter-spacing:0.32em">'
            f"{html.escape(label)}</p>{rows}</div>"
        )
        self.js.append(f'gsap.set("#{g} .gripe", {{transformOrigin: "0% 50%"}});')
        for k in range(1, len(lines)):
            t = round(at + 0.5 * k, 2)
            self.js.append(f'slam("#{g} .gripe:nth-of-type({k + 2})", {t}, 1.25); jolt({t}, 5);')
        self.js.append(f'tl.to("#{g} p", {{opacity: 0.45, duration: 0.15}}, {at + dur - 0.7});')
        self.slash(round(at + dur - 0.7, 2))

    def slash(self, at):
        """The yellow slash: across at `at`, the frame is yellow at at+0.5, gone by at+0.75."""
        g = self._id("s")
        self.body.append(
            f'<div id="{g}" class="clip full" data-start="{at}" data-duration="0.75" data-track-index="12" '
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
            f'<div id="{g}" class="clip full claim" data-start="{at}" data-duration="{dur}" data-track-index="7" '
            f'data-track-kind="graphics"><h1 class="stencil" data-layout-allow-overflow>{spans}</h1><p class="cond sub">{html.escape(sub)}</p></div>'
        )
        k = 0
        for w in words:
            if w == "/":
                continue
            k += 1
            if at == 0 and k == 1:
                continue  # frame 0 is the thumbnail
            self.js.append(f'slam("#{g} h1 .w:nth-of-type({k})", {round(at + 0.05 + (k - 1) * 0.25, 2)}, 1.6);')
        self.js.append(f"jolt({round(at + 0.05 + (k - 1) * 0.25, 2)}, 10);")
        self.js.append(f'blurIn("#{g} .sub", {round(at + 0.3 + k * 0.25, 2)});')
        self.js.append(f'tl.to("#{g}", {{x: -260, opacity: 0, filter: "blur(14px)", duration: 0.2, ease: "power3.in"}}, {round(at + dur - 0.2, 2)});')
        self.sfx(round(at + dur - 0.3, 2), "sfx/whoosh-short.mp3", 0.5, 0.45)

    def stack(self, at, dur, label, facts):
        g = self._id("f")
        rows = "".join(f'<p class="fact cond" style="top: {230 + 125 * i}px"><i></i>{html.escape(f)}</p>' for i, f in enumerate(facts))
        self.body.append(
            f'<div id="{g}" class="clip full stack" data-start="{at}" data-duration="{dur}" data-track-index="8" '
            f'data-track-kind="graphics"><p class="built cond">{html.escape(label)}</p>{rows}</div>'
        )
        self.js.append(f'gsap.set("#{g} .fact", {{transformOrigin: "0% 50%"}});')
        for i in range(len(facts)):
            t = round(at + 0.25 + i * 0.5, 2)
            self.js.append(f'slam("#{g} .fact:nth-of-type({i + 1})", {t}, 1.25); jolt({t}, 4);')

    def end(self, at):
        g = self._id("e")
        self.body.append(
            f'<div id="{g}" class="clip full end" data-start="{at}" data-duration="{round(self.dur - at, 2)}" data-track-index="9" '
            'data-track-kind="graphics"><div class="lockup"><div class="mark"><img src="assets/helmsman-mark-black.svg" alt="" /></div>'
            '<div><p class="wordmark stencil">Helmsman</p><p class="descriptor cond">Factory automation</p></div></div>'
            '<div class="cta"><p class="cta-btn">Try it in your browser</p><p class="cta-url">helmsman.advin.io</p></div>'
            '<p class="licence">Open source (Apache 2.0)</p></div>'
        )
        self.js += [
            f'tl.fromTo("#{g} .mark", {{scale: 3.2, rotation: -120, opacity: 0}}, {{scale: 1, rotation: 0, opacity: 1, duration: 0.18, ease: "power4.in"}}, {at});',
            f"jolt({at + 0.18}, 20);",
            f'tl.fromTo("#{g} .wordmark", {{clipPath: "inset(0% 100% 0% 0%)"}}, {{clipPath: "inset(0% 0% 0% 0%)", duration: 0.4, ease: land}}, {at + 0.45});',
            f'tl.fromTo("#{g} .descriptor", {{opacity: 0}}, {{opacity: 1, duration: 0.25}}, {at + 0.75});',
            f'tl.fromTo("#{g} .cta-btn", {{opacity: 0, scale: 0.9}}, {{opacity: 1, scale: 1, duration: 0.3, ease: "back.out(1.4)"}}, {at + 1});',
            f'tl.fromTo("#{g} .cta-url", {{opacity: 0}}, {{opacity: 1, duration: 0.25}}, {at + 1.2});',
            f'tl.fromTo("#{g} .licence", {{opacity: 0}}, {{opacity: 1, duration: 0.25}}, {at + 1.3});',
        ]
        self.sfx(round(at + 0.14, 2), "sfx/impact-bass-1.mp3", 2.1, 0.8, fade=0.8)

    # ---- sound ----------------------------------------------------------------------

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
            f'<audio id="s{self.track}" class="clip" data-start="{round(at, 3)}" data-duration="{dur}" '
            f'data-track-index="{self.track}" data-volume="{vol}"{extra} src="assets/{src}"></audio>'
        )

    def bed(self, at, dur, media, vol, fade=None):
        """A stretch of the music bed; several of them duck it or loop it on a bar line."""
        self.beds.append((at, dur, media, vol, fade))

    def write(self):
        if not self.beds:
            self.bed(0, self.dur, 0, 0.8, fade=1.2)
        beds = []
        for k, (at, dur, media, vol, fade) in enumerate(self.beds):
            f = f' data-fade-out="{fade}"' if fade else ""
            beds.append(
                f'<audio id="bed{k}" class="clip" data-start="{at}" data-duration="{round(dur, 3)}" data-media-start="{media}" '
                f'data-track-index="{20 + k % 2}" data-volume="{vol}"{f} src="assets/music/bed.mp3"></audio>'
            )
        page = PAGE.format(
            title=html.escape(self.title),
            dur=self.dur,
            body="\n        ".join(self.body),
            audio="\n      ".join(beds + self.audio),
            js="\n      ".join(self.js),
        )
        out = HERE / self.name
        out.mkdir(exist_ok=True)
        (out / "index.html").write_text(page)
        for shared in ("assets", "film.css", "hyperframes.json"):  # one project a film, one set of files
            link = out / shared
            if not link.is_symlink():
                link.symlink_to(f"../{shared}")


# ---- the films --------------------------------------------------------------------------
# Take times (seconds into each recording), from assets/app/<take>.json and the frames:
#   describe: typing 1.39-4.98, Enter 5.99, a question at 7.0, "Done" at 9.0; input on 43.15, off 46.37
#   visualize: LAMP picked 0.99, cleared 4.39, zoom 5.39 and 6.19
#   mary: upload 1.2, run 3.4, input on 5.0, trend window dragged 9.0-10.2
#   history: typing 1.42-8.61, Enter 8.61, the answer lands at 18; back 30.41, forward 33.41, the list 36.42
#   code: INT picked 1.0, "IT" typed 1.6, Deploy 4.6
#   scan: Scan 1.0, Add 3.83, Add in the box 5.2
#   front: loaded at 0.5, the program written by 5, all five proofs by 15; high switch failed 18.0; editor 29.5
# bench (tools/bench.sh): tune tones from 0.5 s.


def loop_describe() -> Film:
    f = Film("loop-describe", "Helmsman: say what it should do", 21.0)
    f.claim(0, 3.0, ["Say", "what", "it", "/", "should", "do"], "Say", "In your own words")
    # The answer lands at 11.3 (take 9.0): held to 13.5, the camera pulls back to show the diagram it drew.
    f.app(3.0, 10.5, "describe", 0.7, views=[(0, *FULL), (0.3, 1.9, 0, 40), (8.2, 1.25, 0, 60)])
    f.caps(3.5, 4.5, [(0, "Describe the machine", "w")], right=True)
    f.caps(11.4, 2.1, [(0, "It writes the program", "y")], right=True)
    f.app(13.5, 4.0, "describe", 42.6, views=[(0, 1.25, 400, 160)])
    f.caps(13.5, 4.0, [(0.3, "Flip the input", "w"), (1.0, "The lamp follows", "y")])
    f.end(17.5)
    return f


def loop_visualize() -> Film:
    f = Film("loop-visualize", "Helmsman: watch it run", 16.0)
    f.claim(0, 3.0, ["Watch", "it", "run"], "run", "A live diagram of the program")
    f.app(3.0, 9.5, "visualize", 0.3, views=[(0, *FULL), (0.4, 1.6, 470, 110)])
    f.caps(3.4, 3.6, [(0.2, "Click an output", "w"), (0.9, "See what drives it", "y")])
    f.caps(7.3, 5.2, [(0.3, "Running against", "w"), (0.9, "simulated I/O", "y")])
    f.end(12.5)
    return f


def loop_test() -> Film:
    f = Film("loop-test", "Helmsman: test it before the hardware arrives", 20.5)
    f.before(0, "Testing needs the machine")
    f.app(1.5, 10.0, "mary", 0.6, views=[(0, *FULL), (3.6, 1.4, 460, 300)])
    f.caps(1.9, 3.4, [(0.2, "Upload the bench program", "w")], right=True)
    f.caps(5.6, 5.9, [(0.3, "It runs in the browser", "w"), (1.5, "No machine connected", "y")], right=True)
    f.bench(11.5, 5.5, "tune", media=0.4, sound=1.0)
    f.caps(11.5, 5.5, [(0.5, "Then the real motor plays it", "y")], kicker="On the bench")
    f.bed(0, 11.5, 0, 0.8)
    f.bed(11.5, 5.5, 11.5, 0.18)
    f.bed(17.0, 3.5, 17.0, 0.8, fade=1.0)
    f.end(17.0)
    return f


def loop_history() -> Film:
    f = Film("loop-history", "Helmsman: every change, kept", 20.0)
    f.before(0, "Project files you can't compare")
    f.app(1.5, 2.5, "history", 6.3, views=[(0, 1.9, 0, 40)])
    f.caps(1.5, 2.5, [(0.2, "Describe a change", "w")], right=True)
    f.app(4.0, 3.5, "history", 17.5, views=[(0, 1.25, 0, 60)])
    f.later(4.0, 1.5, "9 s later")
    f.caps(4.0, 3.5, [(0.5, "A new version", "y")], right=True)
    f.app(7.5, 9.0, "history", 29.6, views=[(0, 1.25, 0, 60), (6.4, 1.9, 0, 40)])
    f.caps(7.9, 3.3, [(0.3, "Step back a version", "w"), (1.0, "See what changed", "y")], right=True)
    f.caps(11.3, 2.9, [(0.1, "Step forward again", "w")], right=True)
    f.caps(14.3, 2.2, [(0.1, "Every version, listed", "y")], right=True)
    f.end(16.5)
    return f


def loop_typo() -> Film:
    f = Film("loop-typo", "Helmsman: one typo is one error", 15.0)
    f.before(0, "One typo, twenty errors")
    f.app(1.5, 8.7, "code", 0.3, views=[(0, 1.45, 0, 250), (4.2, 1.3, 0, 0)])
    f.caps(2.0, 3.8, [(0.9, "A typo is named on its line", "w")], right=True)
    f.caps(5.9, 4.3, [(0.2, "Deploy refuses it", "y"), (1.2, "It never reaches the machine", "w")], right=True)
    f.end(11.5)
    return f


def loop_connect() -> Film:
    f = Film("loop-connect", "Helmsman: plug in and go", 16.5)
    f.claim(0, 3.0, ["Plug", "in", "and", "go"], "go", "Get connected")
    f.bench(3.0, 1.5, "skipper", media=0.1)
    f.caps(3.0, 1.5, [(0, "A $30 controller", "y")], kicker="The Skipper")
    f.app(4.5, 8.5, "scan", 0.5, views=[(0, 1.4, 0, 150)])
    f.caps(4.5, 8.5, [(0.6, "Scan the network", "w"), (3.2, "It knows the device", "w"), (4.8, "Wired in one click", "y")], right=True)
    f.end(13.0)
    return f


def loop_install() -> Film:
    f = Film("loop-install", "Helmsman: nothing to install", 22.0)
    f.before(0, "A virtual machine for every version")
    f.app(1.5, 5.0, "front", 0.3, views=[(0, *FULL)])
    f.caps(1.8, 4.7, [(0.2, "Open a browser", "w"), (1.2, "Nothing to install", "y")], right=True)
    f.app(6.5, 3.5, "front", 11.5, views=[(0, 1.6, 720, 220)])
    f.caps(6.5, 3.5, [(0.3, "It proves the program first", "y")])
    f.app(10.0, 5.5, "front", 17.6, views=[(0, 1.35, 380, 280)])
    f.caps(10.0, 5.5, [(0.6, "Break it on purpose", "w"), (2.4, "It keeps the tank safe", "y")], right=True)
    f.app(15.5, 3.0, "front", 29.2, views=[(0, *FULL)])
    f.caps(15.5, 3.0, [(0.5, "Then open the editor", "w")], right=True)
    f.end(18.5)
    return f


FACTS = [
    "No licence fees",
    "A $30 controller",
    "Nothing to install",
    "IEC 61131-3 Structured Text",
    "EtherNet/IP and Modbus TCP",
    "Every change recorded",
]


def reel() -> Film:
    f = Film("reel", "Helmsman: the benefits, shown in the app itself", 55.0)
    f.gripes(0, 4.2, "What engineers are tired of", [
        "A virtual machine per version",
        "Testing needs the machine",
        "One typo, twenty errors",
        "Project files you can't compare",
    ])
    f.claim(4.2, 2.8, ["Say", "what", "it", "/", "should", "do"], "Say", "See it work")
    # Describe: the last words typed, the answer, then the input.
    f.app(7.0, 3.0, "describe", 3.0, views=[(0, 1.9, 0, 40)])
    f.caps(7.0, 3.0, [(0.1, "Describe it", "y")], right=True)
    f.app(10.0, 2.0, "describe", 8.6, views=[(0, 1.25, 0, 60)])
    f.app(12.0, 2.5, "describe", 42.8, views=[(0, 1.25, 400, 160)])
    f.caps(10.0, 4.5, [(0.1, "It writes the program", "w"), (2.4, "The lamp follows", "y")])
    # Visualize
    f.app(14.5, 4.0, "visualize", 0.5, views=[(0, 1.6, 470, 110)])
    f.caps(14.5, 4.0, [(0.1, "Watch it run", "y"), (1.0, "See what drives it", "w")])
    # Test with no machine, then the bench
    f.app(18.5, 4.5, "mary", 2.8, views=[(0, *FULL), (1.6, 1.4, 460, 300)])
    f.caps(18.5, 4.5, [(0.1, "Test with no machine", "y")], right=True)
    f.bench(23.0, 4.5, "tune", media=0.4, sound=1.0)
    f.caps(23.0, 4.5, [(0.3, "Then the real motor plays it", "y")], kicker="On the bench")
    # History
    f.app(27.5, 4.0, "history", 29.9, views=[(0, 1.25, 0, 60)])
    f.caps(27.5, 4.0, [(0.1, "Every change, kept", "y"), (1.0, "See what changed", "w")], right=True)
    # Typo
    f.app(31.5, 5.0, "code", 1.4, views=[(0, 1.45, 0, 250), (3.1, 1.3, 0, 0)])
    f.caps(31.5, 5.0, [(0.3, "A typo is one error", "w"), (3.3, "Deploy refuses it", "y")], right=True)
    # Connect
    f.bench(36.5, 1.0, "skipper", media=0.2)
    f.caps(36.5, 1.0, [(0, "A $30 controller", "y")])
    f.app(37.5, 3.0, "scan", 2.9, views=[(0, 1.4, 0, 150)])
    f.caps(37.5, 3.0, [(0.1, "Plug in and go", "y")], right=True)
    # Proofs
    f.app(40.5, 1.5, "front", 12.5, views=[(0, 1.6, 720, 220)])
    f.app(42.0, 3.0, "front", 17.7, views=[(0, 1.35, 380, 280)])
    f.caps(40.5, 4.5, [(0.1, "It proves itself first", "y"), (1.6, "Break it on purpose", "w")], right=True)
    f.stack(45.0, 4.5, "What is built", FACTS)
    f.end(49.5)
    # The bed is 32.5 s with its first beat on frame 0: it ducks under the motor and starts again on bar 14.
    f.bed(0, 23.0, 0, 0.8)
    f.bed(23.0, 4.5, 23.0, 0.18)
    f.bed(27.5, 0.5, 27.5, 0.8)
    f.bed(28.0, 27.0, 0, 0.8, fade=1.2)
    return f


FILMS = (loop_describe, loop_visualize, loop_test, loop_history, loop_typo, loop_connect, loop_install, reel)


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
    <!-- Written by build.py: edit the film's table there, not this file. -->
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
    import sys

    names = sys.argv[1:]
    for film in FILMS:
        f = film()
        if not names or f.name in names or f.name.removeprefix("loop-") in names:
            f.write()
            print("wrote", f.name)
