#!/usr/bin/env python3
"""The footage for the device film: one composed still per shot (Seedream),
repainted in the film's style, then that painting animated as the first frame of a short clip (MiniMax H3).

    ./shots.py stills [name ...]     # outputs/box/<name>.jpg, about $0.04 each
    ./shots.py paint  [name ...]     # outputs/box/<name>-fine.jpg, the still in the film's look
    ./shots.py clips  [name ...]     # outputs/box/clip-<name>.mp4, a 768P draft, $0.02 a second
    ./shots.py finals [name ...]     # outputs/box/final-<name>.mp4, 2K on the full model, $0.13 a second
    ./shots.py sheet                 # outputs/box/sheet.jpg, every painted still on one page
    ./shots.py install [name ...]    # clips/<name>.mp4 into ../assets/, interpolated to 60 fps

It drives the storyboard CLI (~/github/storyboard), which bounds-checks each
request, retries, and writes a costed record per run under outputs/box/runs/.
768P goes to the draft model; that routing is on storyboard's dev branch, so
STORYBOARD points at a checkout of it. Needs REPLICATE_API_TOKEN, uv, ffmpeg, and
H3_ASSET_BUCKET with the aws CLI for any run more than two hours after its stills.
Media is not committed: this table is the record of how each shot was made.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "outputs" / "box"
STORYBOARD = os.environ.get(
    "STORYBOARD", str(Path.home() / "github/storyboard/.claude/worktrees/dev-run")
)
SB = ["uv", "run", "--project", STORYBOARD, "storyboard"]
PARALLEL = 4  # the hosted API takes about five predictions at once

STYLE = (
    "Documentary photograph, unretouched, muted low-contrast grade, fine film grain, "
    "natural skin texture, no HDR glow, no beauty smoothing."
)
PLANT = (
    "a real mid-sized packaging plant: sealed gray concrete floor with worn yellow safety "
    "lines, powder-coated machine guards, roller conveyors carrying plain brown cardboard "
    "cartons, overhead cable trays, cool LED high-bay light"
)
# The device, as the image model needs it said. docs/brand/index.html#device is the spec.
DEVICE = (
    "the exact same small matte black industrial controller as in the first reference image: "
    "an identical flat rectangular bead-blasted anodised aluminium block the size of a deck "
    "of cards with softly radiused edges, one narrow face carrying exactly one RJ45 Ethernet "
    "socket in a thin dark metal surround with one thin horizontal light bar beneath it, "
    "every other face blank, no text, no logo, no screws, no other ports"
)
NO_TEXT = "No readable text, labels or logos anywhere."
SUBJECT = (
    "the small matte black controller exactly as it appears in the first frame: a flat "
    "rectangular bead-blasted aluminium block the size of a deck of cards with one RJ45 "
    "Ethernet socket and one thin light bar on a narrow face and nothing else on it"
)
KEEP = (
    "its shape, proportions, matte black finish, single Ethernet socket and light bar stay "
    "exactly as in the first frame; it never gains text, a logo, a screen, buttons or ports"
)

# The film's look: painterly realism in graphite and one yellow. The composed still
# fixes what is in the frame; this repaints it, and the painting is the first frame.
PAINT = (
    "Redraw the reference image exactly: the same composition, camera angle, objects, people, "
    "poses and proportions, with exactly the objects the reference has. Change "
    "only the rendering style: painterly realism. At first glance it reads as a film still with "
    "real lens depth of field, real materials and dramatic directional light with deep shadows, "
    "but every surface is hand-painted in thick oil paint with visible, confident brushstrokes "
    "and palette-knife highlights, edges slightly simplified. The palette is strictly limited and "
    "moody: deep charcoal, graphite and steel grays with warm off-white highlights for "
    "everything, and one single saturated colour, a strong industrial yellow: whatever is yellow "
    "in the reference stays yellow, and whatever is pink, blue, orange, red or green there "
    "becomes gray or that yellow. Skin is a natural muted tone. Machine guards and "
    "fences are yellow. Paint it at very high detail, with fine brushwork that resolves small "
    "things: the sandy grain of blasted metal, weld seams and bolt heads, roller bearings, "
    "cable ties, single wire strands and ferrules, the weave of fabric, pores and knuckle "
    "creases in skin, tape and print on cartons, reflections in glass. The plant is clean, "
    "modern and well kept. The subject is in crisp focus."
)
# Said only for the shots the device is in: naming it elsewhere paints one in.
PAINT_DEVICE = (
    "The small black block keeps its exact shape, with its single Ethernet socket and single "
    "light bar; it is brand new, its matte black finish even and flawless, with a soft clean "
    "highlight along its edges."
)
LOOK = (
    "painterly realism: it moves like live-action film with real depth of field and light, and "
    "every surface is hand-painted in thick oil paint with visible brushstrokes, in graphite "
    "grays with industrial yellow as the only saturated colour, exactly as in the first frame; "
    "the painted texture stays stable and never turns photographic. Fine detail holds in every "
    "frame: the grain of metal, weld seams and bolt heads, fabric weave, skin creases, single "
    "wires, dust in the light; light and reflections shift subtly as things move, and nothing "
    "smears, flickers or loses its edges"
)

# name: refs (earlier stills this one is composed from), still, then the clip:
# seconds, subject, what stays fixed, summary, description, sound.
SHOTS: dict[str, dict] = {
    # ---- the device ----
    "hand": {
        "refs": ["root-c2"],
        "still": f"{STYLE} Close shot on a factory floor: a controls engineer's bare, slightly "
        f"work-worn hand holds {DEVICE} flat on the open palm at chest height, the narrow port "
        "face toward the camera, the light bar dark. The block is clearly smaller than the "
        f"palm. Behind, far out of focus, {PLANT}. {NO_TEXT}",
        "paint": "The light bar under the socket is dark and unlit.",
        "seconds": 5,
        "summary": "on a factory floor: an engineer's open hand holds <Subject 1> while the "
        "camera slowly pushes in.",
        "desc": "an engineer's bare hand holds <Subject 1> flat on the open palm at chest "
        "height, a packaging line far out of focus behind. The hand slowly tilts the block a "
        "few degrees toward the camera and back, as if weighing it, the raking light sliding "
        "across its sandy matte finish. The camera slowly pushes in with small amplitude and "
        "holds on the Ethernet socket.",
        "sound": "the steady hum of a packaging line in the distance, rollers turning, a faint "
        "pneumatic hiss.",
    },
    "bench": {
        "refs": ["root-c2"],
        "still": f"{STYLE} {DEVICE.capitalize()} sits alone in the middle of a scarred steel "
        "workbench in a plant maintenance shop, seen from a low three-quarter angle with the "
        "port face toward the camera, the light bar dark. Beside it one neatly coiled yellow "
        "Ethernet patch cable. A single work lamp rakes warm light from the left; behind, out "
        f"of focus, {PLANT}. {NO_TEXT}",
        "paint": "The light bar under the socket is dark and unlit.",
        "seconds": 5,
        "summary": "in a plant maintenance shop: <Subject 1> sits on a steel workbench while "
        "the camera slowly slides past it.",
        "desc": "<Subject 1> sits alone on a scarred steel workbench beside a coiled yellow "
        "Ethernet cable, lit by one warm work lamp. Nothing on the bench moves. The camera "
        "slowly slides from left to right at bench height with small amplitude, the highlight "
        "travelling along the block's top edge, the plant behind staying out of focus.",
        "sound": "quiet shop room tone, a distant conveyor, the faint buzz of the work lamp.",
    },
    "mount": {
        "refs": ["root-c2", "rail-c1"],
        "still": f"{STYLE} Inside the open light gray steel control cabinet from the second "
        "reference image, close on the horizontal steel DIN rail: gray screw terminal blocks on "
        "the left, three small circuit breakers on the right, and an empty gap of bare rail "
        f"between them. A hand in a thin gray work glove holds {DEVICE} upright a few "
        "centimetres in front of that gap, narrow port face outward toward the camera, light "
        f"bar dark, about to clip it onto the rail. Cool overhead shop light. {NO_TEXT}",
        "paint": "The light bar under the socket is dark and unlit.",
        "seconds": 5,
        "summary": "inside an open control cabinet: a gloved hand slowly clips <Subject 1> onto "
        "the DIN rail and lets go.",
        "desc": "a hand in a thin gray work glove holds <Subject 1> upright just in front of "
        "an empty gap on the steel DIN rail, between gray terminal blocks and small circuit "
        "breakers. Over the first three seconds the hand slowly presses the block straight "
        "back onto the rail until it seats with one firm click and sits square and still. In "
        "the last two seconds the hand slowly opens and withdraws out of the frame to the "
        "right, leaving the block alone on the rail. The camera holds still.",
        "sound": "cabinet room tone and a distant line; one crisp plastic-on-steel click as "
        "the block seats on the rail; the soft rustle of the glove.",
    },
    "plug": {
        "refs": ["root-c2", "rail-c1"],
        "still": f"{STYLE} Extreme close-up inside the control cabinet from the second "
        f"reference image: {DEVICE} is clipped upright on the DIN rail, its narrow port face "
        "filling the middle of the frame, the Ethernet socket empty and the light bar beneath "
        "it dark. That socket is the only opening anywhere on the block. From the right edge "
        "of the frame, at exactly the height of the socket, a bare hand holds the clear plastic "
        "RJ45 plug of a bright yellow Ethernet patch cable between thumb and forefinger; the "
        "tip of the plug is one centimetre in front of the socket, pointing straight into it, "
        "the cable running back horizontally out of the frame. Gray terminal blocks soft in the "
        f"background. {NO_TEXT}",
        "paint": "The light bar under the socket is dark and unlit, and the socket is empty.",
        "seconds": 6,
        "summary": "in extreme close-up inside a control cabinet: fingers slowly push a yellow "
        "Ethernet cable into <Subject 1> and its light bar comes on.",
        "desc": "the narrow port face of <Subject 1> fills the frame, its socket empty and its "
        "light bar dark, while a thumb and forefinger hold the clear plug of a yellow Ethernet "
        "cable just in front of that socket, the only socket on the block. Over the first two "
        "and a half seconds the fingers slowly push the plug straight into that one socket in "
        "the middle of the face until it latches with a click; no other opening ever appears "
        "on the block. At about "
        "three seconds the thin light bar beneath the socket fades up from dark to a steady "
        "warm yellow glow, and a tiny green link light inside the socket begins to blink "
        "slowly. In the last two seconds the fingers slowly let go and withdraw, the yellow "
        "cable hanging in a gentle curve. The camera holds still.",
        "sound": "close cabinet room tone; the sharp plastic click of the plug latching; a very "
        "soft rising electronic tone as the light comes on; a distant conveyor.",
    },
    "live": {
        "refs": ["root-c2", "rail-c1", "port-lit"],
        "still": f"{STYLE} Inside the open light gray steel control cabinet from the second "
        f"reference image, a shallow three-quarter view at eye level: {DEVICE} is clipped "
        "upright on the DIN rail between gray terminal blocks and three small circuit "
        "breakers. One bright yellow Ethernet patch cable is plugged into its socket and "
        "curves down into the slotted gray wire duct below. The thin light bar beneath the "
        "socket glows a steady warm yellow as in the third reference image, the only coloured "
        f"light in the cabinet. Cool overhead shop light. {NO_TEXT}",
        "paint": "The light bar under the socket glows steady yellow.",
        "seconds": 5,
        "summary": "inside an open control cabinet: <Subject 1> runs on the DIN rail with one "
        "yellow cable in it while the camera slowly pushes in.",
        "desc": "<Subject 1> sits clipped on the DIN rail with one yellow Ethernet cable "
        "plugged in and its light bar glowing a steady warm yellow. Nothing in the cabinet "
        "moves except a tiny green link light inside the socket, blinking slowly and "
        "irregularly. The camera slowly pushes in toward the port face with small amplitude "
        "and holds.",
        "sound": "the low hum of a control cabinet, a contactor pulling in once somewhere "
        "below, the packaging line running steadily beyond the door.",
    },
    "walkaway": {
        "refs": ["root-c2", "rail-c1"],
        "still": f"{STYLE} Wide shot on the floor of {PLANT}. In the left third of the frame "
        "stands an open light gray steel control cabinet; inside it, small but clear, "
        f"{DEVICE} on the DIN rail with one yellow cable plugged in and its light bar glowing "
        "yellow. Beside the cabinet a woman controls engineer in her thirties, dark polo "
        "shirt, safety glasses, hair tied back, stands with one hand on the cabinet door, her "
        "other hand empty, looking toward the conveyor line running on the right. No laptop, "
        f"no cart, no tools on the floor. {NO_TEXT}",
        "paint": "The light bar under the socket glows steady yellow.",
        "seconds": 5,
        "summary": "on a plant floor: an engineer slowly swings a control cabinet door shut on "
        "<Subject 1> and walks away empty-handed toward the running line.",
        "desc": "an engineer in a dark polo shirt and safety glasses stands at an open control "
        "cabinet with one hand on its door; inside, <Subject 1> glows yellow on the rail. Over "
        "the first three seconds she slowly swings the door closed until it latches. Then she "
        "turns and walks unhurriedly away from the camera toward the conveyor line, both "
        "hands empty and relaxed at her sides. Cartons keep gliding along the conveyor on the "
        "right throughout. The camera holds still.",
        "sound": "the steady rumble of rollers and cartons, the solid clunk of the cabinet "
        "door latching, slow footsteps on concrete.",
    },
    # ---- the way it is today ----
    "oldway": {
        "refs": [],
        "still": f"{STYLE} Night shift in a dim factory. A tired controls engineer in his "
        "forties, stubble, hi-vis vest over a work shirt, sits on an upturned plastic bucket in "
        "front of an open control cabinet. The cabinet is crowded by a large old beige-gray "
        "modular controller rack with rows of worn modules and a thick loom of wires. A heavy, "
        "scuffed old laptop is balanced on his knees, a gray serial cable running from it into "
        "the cabinet and a small USB dongle sticking out of its side; its screen faces away "
        "from the camera and lights his face cold blue. He presses finger and thumb into his "
        f"closed eyes. A paper cup of coffee on the floor. {NO_TEXT}",
        "seconds": 5,
        "subject": "the tired engineer in a hi-vis vest sitting at the open cabinet in the "
        "first frame, a heavy old laptop on his knees",
        "keep": "his face, vest, the laptop, the cabinet and the old controller rack stay "
        "exactly as in the first frame; only his posture changes",
        "summary": "in a dim factory at night: <Subject 1> sits at an open control cabinet "
        "with a laptop on his knees, rubs his eyes and waits.",
        "desc": "<Subject 1> sits on an upturned bucket in front of a crowded control cabinet, "
        "a heavy laptop on his knees lighting his face cold blue, a serial cable running into "
        "the cabinet. He slowly lowers his hand from his eyes, exhales through his nose, looks "
        "back down at the screen and slowly drums two fingers on the edge of the laptop, "
        "waiting. The camera slowly drifts in with small amplitude.",
        "sound": "an almost silent plant at night, a ventilation fan, the laptop's fan "
        "whining, a slow tired exhale, fingers drumming on plastic.",
    },
    "dongles": {
        "refs": [],
        "still": f"{STYLE} Top-down close-up of an open metal desk drawer in a plant "
        "engineering office, full of the clutter of years: a tangle of small USB dongles on "
        "key rings, gray serial and adapter cables, an old external disc drive, loose "
        "memory sticks, a worn lanyard. A man's hand reaches into the tangle from the bottom "
        f"of the frame, searching. Flat fluorescent office light. {NO_TEXT}",
        "seconds": 4,
        "subject": "the desk drawer full of tangled USB dongles, key rings and gray adapter "
        "cables in the first frame",
        "keep": "the drawer and its clutter stay as in the first frame; only the hand and the "
        "items it touches move",
        "summary": "top-down on an office desk drawer: a hand slowly searches through "
        "<Subject 1> for the right dongle.",
        "desc": "seen from directly above, a man's hand slowly sifts through <Subject 1>, "
        "pushing aside a knot of gray cables, lifting one small dongle by its key ring, "
        "turning it over, and slowly dropping it back to pick at another. The camera holds "
        "still.",
        "sound": "the plastic clatter and jingle of small dongles and key rings in a metal "
        "drawer, a quiet office, a faint sigh.",
    },
    "rackout": {
        "refs": [],
        "still": f"{STYLE} In a plant maintenance area, a technician's two gloved hands lower "
        "a bulky old beige-gray modular controller rack, its many modules yellowed with age "
        "and a trail of cut multicoloured wires hanging from it, into a steel scrap bin "
        "already holding old cable offcuts. Seen from a low angle beside the bin; behind, out "
        f"of focus, {PLANT}. {NO_TEXT}",
        "seconds": 5,
        "subject": "the bulky old beige-gray modular controller rack with yellowed modules and "
        "trailing cut wires held over a scrap bin in the first frame",
        "keep": "the rack stays exactly as in the first frame; only its position changes",
        "summary": "in a plant maintenance area: gloved hands slowly lower <Subject 1> into a "
        "steel scrap bin and let go.",
        "desc": "two gloved hands hold <Subject 1> over a steel scrap bin. Over the first "
        "three seconds they slowly lower it into the bin until it settles on the cable "
        "offcuts with a dull thud, its cut wires folding over the rim. In the last two seconds "
        "the hands slowly let go and lift out of the frame. The camera holds still.",
        "sound": "a heavy dull metallic thud as the rack settles in the bin, loose wires "
        "rattling against steel, the plant running in the distance.",
    },
    # ---- the way it is with it ----
    "describe": {
        "refs": [],
        "still": f"{STYLE} A daylit engineering office with a long window overlooking {PLANT}. "
        "A woman controls engineer in her thirties, dark polo shirt, hair tied back, safety "
        "glasses pushed up on her head, sits at a plain desk typing on a slim laptop, seen "
        "from the side and slightly behind the screen so the screen itself is not visible. A "
        "coffee mug and one rolled engineering drawing on the desk. She looks relaxed and "
        f"focused. {NO_TEXT}",
        "seconds": 5,
        "subject": "the woman engineer in a dark polo shirt typing at a slim laptop by the "
        "office window in the first frame",
        "keep": "her face, hair, clothing, the desk and the window stay exactly as in the "
        "first frame; only her hands and gaze move",
        "summary": "in a daylit office above a plant floor: <Subject 1> types a few unhurried "
        "sentences and glances out at the line.",
        "desc": "<Subject 1> sits at a plain desk by a window over the plant floor, typing "
        "unhurriedly on a slim laptop whose screen faces away from the camera. She types for "
        "three seconds, pauses, slowly looks up and out of the window at the line below, then "
        "looks back to the screen with a small satisfied nod. Cartons move slowly on the "
        "conveyors beyond the glass. The camera slowly drifts in with small amplitude.",
        "sound": "soft unhurried laptop keystrokes, a quiet office, the plant muffled through "
        "the glass.",
    },
    "tablet": {
        "refs": [],
        "still": f"{STYLE} On the floor of {PLANT}, medium shot: a woman controls engineer in "
        "her thirties, dark polo shirt, hair tied back, safety glasses on, stands beside a "
        "running roller conveyor holding a tablet in both hands at waist height, the back of "
        "the tablet toward the camera. She is looking up from it at the cartons moving past, "
        f"calm, a slight smile. {NO_TEXT}",
        "seconds": 5,
        "subject": "the woman engineer in a dark polo shirt and safety glasses holding a "
        "tablet beside the conveyor in the first frame",
        "keep": "her face, hair, clothing and the tablet stay exactly as in the first frame; "
        "only her gaze and one hand move",
        "summary": "on a plant floor: <Subject 1> watches cartons run past, taps her tablet "
        "once and looks up again.",
        "desc": "<Subject 1> stands beside a running roller conveyor holding a tablet with its "
        "back to the camera. She watches the cartons glide past for two seconds, slowly looks "
        "down and taps the tablet once with her forefinger, then slowly looks up again at the "
        "line and nods slightly. Cartons keep moving steadily behind her throughout. The "
        "camera holds still.",
        "sound": "rollers turning and cartons rumbling past close by, a pneumatic hiss, the "
        "plant's steady hum.",
    },
    "linestart": {
        "refs": [],
        "still": f"{STYLE} Low angle along a stopped roller conveyor in {PLANT}. Three plain "
        "brown cardboard cartons sit motionless on the rollers in the foreground. On a post at "
        "the left edge of the frame a three-tier industrial stack light, its red tier lit, "
        f"its amber and green tiers dark. Nobody in the frame. {NO_TEXT}",
        "paint": "Only the top tier of the stack light is lit, glowing yellow; the other tiers are dark gray.",
        "seconds": 5,
        "subject": "the roller conveyor with three brown cartons and the stack light on a post "
        "in the first frame",
        "keep": "the conveyor, the cartons and the stack light stay exactly as in the first "
        "frame; only the lit tier and the cartons' position change",
        "summary": "low along a stopped conveyor: the stack light on <Subject 1> changes "
        "from its top tier to its bottom tier and the cartons slowly start to roll.",
        "desc": "three brown cartons sit still on a stopped roller conveyor and the top tier "
        "of the stack light on the post is lit. At about one second the top tier goes dark and "
        "the bottom tier lights bright yellow. At about two seconds the rollers begin to turn and the three "
        "cartons slowly start to glide away from the camera along the conveyor, one after "
        "another, at an even pace. The camera holds still.",
        "sound": "a quiet stopped line; one soft relay click; a motor winding up; rollers "
        "beginning to rumble under the cartons.",
    },
    "running": {
        "refs": [],
        "still": f"{STYLE} High wide establishing shot from a mezzanine over {PLANT} in "
        "morning light from long skylights: two conveyor lines carrying cartons, a guarded "
        "robot palletiser stacking cartons on a pallet at the far end, an orange forklift "
        f"parked, two workers small in the distance. Clean, orderly, running. {NO_TEXT}",
        "seconds": 5,
        "subject": "the packaging plant floor seen from the mezzanine in the first frame",
        "keep": "the layout, machines and light stay exactly as in the first frame",
        "summary": "from a mezzanine: <Subject 1> runs steadily in morning light while the "
        "camera slowly glides along the rail.",
        "desc": "seen from above, cartons glide steadily along two conveyor lines and a "
        "robot palletiser at the far end slowly lifts a carton and places it on a pallet. "
        "Morning light falls in long bars from the skylights. The camera slowly glides to the "
        "right along the mezzanine with small amplitude.",
        "sound": "the broad steady hum of a running plant: rollers, a robot's servo whine, a "
        "distant forklift beeper.",
    },
    "approve": {
        "refs": [],
        "still": f"{STYLE} Close over-the-shoulder shot on a plant floor: a woman engineer's "
        "hands hold a phone upright, her thumb hovering over its softly glowing screen; the "
        "screen is angled away and blurred so nothing on it can be read. Beyond the phone, in "
        "soft focus, a roller conveyor with cartons moving. Dark polo sleeve at the edge of "
        f"the frame. {NO_TEXT}",
        "seconds": 4,
        "subject": "the hands holding a softly glowing phone in the first frame",
        "keep": "the hands, sleeve and phone stay exactly as in the first frame; the screen "
        "stays an unreadable soft glow",
        "summary": "over the shoulder on a plant floor: a thumb slowly taps <Subject 1> four "
        "times while the line runs on behind.",
        "desc": "two hands hold a softly glowing phone whose screen stays blurred and "
        "unreadable. The thumb slowly taps the screen four times in an even rhythm, then "
        "rests. Behind, in soft focus, cartons keep gliding along the conveyor without "
        "pausing. The camera holds still.",
        "sound": "four soft key tones, then one short confirming tone; the line running "
        "steadily behind.",
    },
    "coffee": {
        "refs": [],
        "still": f"{STYLE} Golden late-afternoon light in {PLANT}. A controls engineer in his "
        "forties, stubble, clean work shirt, safety glasses, leans with his forearms on a "
        "yellow guard rail holding a paper cup of coffee, watching a conveyor line of cartons "
        "run past below him, relaxed, the trace of a smile. No laptop, no tools. Medium shot "
        f"from the side. {NO_TEXT}",
        "seconds": 5,
        "subject": "the engineer in a work shirt leaning on the yellow guard rail with a paper "
        "cup of coffee in the first frame",
        "keep": "his face, clothing, the cup and the rail stay exactly as in the first frame; "
        "only his arm and gaze move",
        "summary": "in late-afternoon light on a plant floor: <Subject 1> leans on a guard "
        "rail, sips his coffee and watches the line run.",
        "desc": "<Subject 1> leans on a yellow guard rail watching cartons run past below. He "
        "slowly raises the paper cup, takes an unhurried sip, lowers it and keeps watching "
        "the line with a relaxed half-smile. Cartons move steadily along the conveyor. The "
        "camera slowly drifts in with small amplitude.",
        "sound": "the even rumble of the line, a quiet sip, a contented breath out.",
    },
}

TEMPLATE = """subject_definitions:
<Subject 1> is {subject}.

summary:
[keyframe completion] A {seconds}-second single shot beginning exactly on the first frame, {summary}

retention_analysis:
<Subject 1> (appears in [Shot 1]): fully_preserved - {keep}.

detailed_description:
The target video is {look}. It is a horizontal frame, one continuous {seconds}-second take with no cuts beginning exactly on the first frame; every movement is slow, smooth and deliberate, nothing fast. No text, captions or logos appear at any point.
[Shot 1] The video begins exactly on the first frame: {desc}

overall_soundscape:
{sound}

non_diegetic_music:
None.
"""


def url_of(name: str) -> str:
    """A URL the models can fetch outputs/box/<name>.jpg from. Replicate's own delivery
    URLs die after about two hours, so with H3_ASSET_BUCKET set (storyboard's reference
    bucket, public read) the file is copied there once and that URL is used."""
    bucket = os.environ.get("H3_ASSET_BUCKET")
    if not bucket:
        record = json.loads((OUT / "runs" / f"{name}.json").read_text())
        return record["urls"][0]
    local = OUT / f"{name}.jpg"
    stamp = f"{int(local.stat().st_mtime)}"
    key = f"helmsman-box/{name}-{stamp}.jpg"
    marker = OUT / "runs" / f"{name}.s3"
    if not marker.exists() or marker.read_text() != key:
        subprocess.run(["aws", "s3", "cp", "--only-show-errors", str(local), f"s3://{bucket}/{key}"], check=True)
        marker.write_text(key)
    region = os.environ.get("AWS_REGION", "us-east-1")
    return f"https://{bucket}.s3.{region}.amazonaws.com/{key}"


def run(args: list[str]) -> int:
    return subprocess.run(SB + args, cwd=HERE).returncode


def still(name: str) -> int:
    shot = SHOTS[name]
    args = ["still", shot["still"], "--project", "box", "--name", name, "--aspect", "16:9", "--size", "2K"]
    for ref in shot["refs"]:
        args += ["--ref", url_of(ref)]
    return run(args)


def paint(name: str) -> int:
    shot = SHOTS[name]
    words = [PAINT, PAINT_DEVICE if shot["refs"] else "", shot.get("paint", "")]
    old = OUT / f"{name}-fine.jpg"
    if old.exists():  # the CLI never overwrites; a new take replaces the old one here
        old.unlink()
    args = ["still", " ".join(w for w in words if w), "--project", "box", "--name", f"{name}-fine"]
    return run(args + ["--aspect", "16:9", "--size", "2K", "--ref", url_of(name)])


def final(name: str) -> int:
    return clip(name, resolution="2K", prefix="final")


def clip(name: str, resolution: str = "768P", prefix: str = "clip") -> int:
    shot = SHOTS[name]
    prompts = HERE / "prompts" / "box"
    prompts.mkdir(parents=True, exist_ok=True)
    text = TEMPLATE.format(
        subject=shot.get("subject", SUBJECT),
        keep=shot.get("keep", KEEP),
        seconds=shot["seconds"],
        summary=shot["summary"],
        desc=shot["desc"],
        sound=shot["sound"],
        look=LOOK,
    )
    prompt = prompts / f"prompt.{name}.txt"
    prompt.write_text(text)
    if run(["validate", str(prompt)]) != 0:
        return 1
    return run(
        ["generate", str(prompts), "--prompt-file", str(prompt), "--first-frame", url_of(f"{name}-fine")]
        + ["--duration", str(shot["seconds"]), "--resolution", resolution, "--ratio", "16:9"]
        + ["--name", f"{prefix}-{name}"]
    )


# The model renders 24 fps; the film is 60. Motion-compensated interpolation holds up
# because every move in these clips is slow.
TO_60 = "minterpolate=fps=60:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1"
REAL_SOUND = ("plug", "mount", "rackout", "walkaway", "dongles", "linestart")


def install(name: str) -> int:
    """clips/<name>.mp4 (the take that was kept, committed) -> ../assets/footage/<name>.mp4
    at 60 fps, and for the shots the cuts play with their own sound, ../assets/real/<name>.mp3."""
    src = HERE / "clips" / f"{name}.mp4"
    assets = HERE.parent / "assets"
    (assets / "footage").mkdir(parents=True, exist_ok=True)
    (assets / "real").mkdir(parents=True, exist_ok=True)
    video = ["ffmpeg", "-v", "error", "-y", "-i", str(src), "-vf", TO_60, "-an", "-c:v", "libx264"]
    video += ["-crf", "14", "-preset", "slow", "-pix_fmt", "yuv420p", str(assets / "footage" / f"{name}.mp4")]
    code = subprocess.run(video).returncode
    if code == 0 and name in REAL_SOUND:
        audio = ["ffmpeg", "-v", "error", "-y", "-i", str(src), "-vn", "-c:a", "libmp3lame", "-q:a", "3"]
        code = subprocess.run(audio + [str(assets / "real" / f"{name}.mp3")]).returncode
    return code


def sheet() -> int:
    names = [f"{n}-fine" for n in SHOTS if (OUT / f"{n}-fine.jpg").exists()]
    cols = 4
    rows = -(-len(names) // cols)
    args = ["ffmpeg", "-v", "error", "-y"]
    for n in names:
        args += ["-i", str(OUT / f"{n}.jpg")]
    scaled = "".join(f"[{i}]scale=640:360[v{i}];" for i in range(len(names)))
    layout = "|".join(f"{(i % cols) * 640}_{(i // cols) * 360}" for i in range(len(names)))
    graph = scaled + "".join(f"[v{i}]" for i in range(len(names)))
    graph += f"xstack=inputs={len(names)}:layout={layout}:fill=black"
    args += ["-filter_complex", graph, str(OUT / "sheet.jpg")]
    print(f"{rows} rows:", " ".join(names))
    return subprocess.run(args).returncode


def main() -> int:
    if len(sys.argv) < 2 or sys.argv[1] not in ("stills", "paint", "clips", "finals", "sheet", "install"):
        print(__doc__)
        return 2
    if sys.argv[1] == "sheet":
        return sheet()
    names = sys.argv[2:] or list(SHOTS)
    unknown = [n for n in names if n not in SHOTS]
    if unknown:
        print("unknown shot:", ", ".join(unknown), file=sys.stderr)
        return 2
    job = {"stills": still, "paint": paint, "clips": clip, "finals": final, "install": install}[sys.argv[1]]
    prompts = HERE / "prompts" / "box"
    if job in (clip, final) and not (prompts / "manifest.json").exists():
        # generate reads a project manifest even when a first frame is the only picture
        prompts.mkdir(parents=True, exist_ok=True)
        if run(["scan", str(prompts)]) != 0:
            return 1
    with ThreadPoolExecutor(PARALLEL) as pool:
        codes = list(pool.map(job, names))
    for name, code in zip(names, codes):
        if code:
            print(f"FAILED {name} (exit {code})", file=sys.stderr)
    return 1 if any(codes) else 0


if __name__ == "__main__":
    sys.exit(main())
