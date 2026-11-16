# Helmsman device film, three rough cuts (1920x1080, 60 fps)

The second ad (`../helmsman-ad-v2`) with the controller as an object in a
real plant: it is clipped onto a rail, one Ethernet cable goes in and its
light comes on. Same palette, faces, 120 BPM grid, slash and end card; the
pictures are footage instead of type on a ground, in one look of their own:
painterly realism, a plant in graphite grays with the brand yellow as the
only saturated colour. The device and that look are specified in
`docs/brand/index.html#device`.

    ./render.sh              # build, check and render all three
    ./render.sh draft b      # a fast pass of one cut while editing
    ./render.sh preview b    # live preview of one cut in a browser

Renders land in `marketing/helmsman-device-{a,b,c}.mp4` (not committed).
Needs Node 22+, FFmpeg and python3.

## The cuts

| Cut | Length | Story |
| --- | ------ | ----- |
| A, one cable | 32 s | Commissioning day. The old way (laptop, programming cable, dongle), then the device: in a hand, on the rail, cable in, light on, devices found, program described and checked, line running, door closed. |
| B, the change | 32 s | Production wants a change while the line runs. Describe, validate, approve; it goes live between two scans and the line never stops. The device appears as where all of it runs. |
| C, out and in | 28 s | The fastest. What leaves the cabinet (rack, dongles, vendor laptop), what goes in (the device), what it does, then the six claims of the second ad over the plant. |

Each cut is a table in `build.py` (shots, captions, cards, all in seconds on
the half-second beat), which writes `cut-a/`, `cut-b/` and `cut-c/` as
HyperFrames projects sharing `film.css` and `assets/`. Edit the table, not
the generated `index.html`. Copy comes from `docs/index.html`; change it
there first.

## The footage

Every shot is generated in three steps through the `storyboard` CLI in
`~/github/storyboard`, and `footage/shots.py` holds every prompt:

    footage/shots.py stills [name ...]   # a composed photographic still, about $0.04
    footage/shots.py paint  [name ...]   # that still repainted in the film's look, $0.04
    footage/shots.py clips  [name ...]   # the painting animated, about $0.02 a second
    footage/shots.py sheet               # every painted still on one page

The still fixes what is in the frame, the repaint fixes the look, and the
painting is the first frame of a 4 to 6 s clip (MiniMax H3, the 768P draft
model). The device stays the same object from shot to shot because every
device still is composed from the same three reference pictures
(`docs/brand/helmsman-device*.jpg`).

Look at every still and clip before it goes in a cut. The first plug-in
take grew a second socket for the cable, and a repaint prompt that names
something to leave out paints it in: say what is there, never what is not.

The take that is kept is copied from `footage/outputs/box/clip-<name>.mp4`
to `footage/clips/<name>.mp4` and committed. The model renders 24 fps and
the film is 60, so `footage/shots.py install` interpolates each clip
(FFmpeg `minterpolate`, about nine minutes for all fifteen) into
`assets/footage/`; `render.sh` runs it when that folder is empty. It holds
up because every move in the clips is slow.

These are drafts at 1344x768, scaled up. A cut that is kept gets its shots
rendered once more at 2K on the full model (`--resolution 2K` in
`shots.py`, about $0.13 a second, near $11 for all fifteen) before it
ships.

The moments the cuts land on a beat were measured in the clips and are the
constants at the top of `build.py`: the light bar coming on, the stack
light changing, the block seated on the rail, the rack's thud, the door's
clunk. New takes need new numbers.

## Sound

The bed and the effects are the second ad's. Six clips also play their own
generated sound under it (`assets/real/`: the plug's click, the rail, the
rack, the dongles, the line starting, the cabinet door). Levels were
measured, not listened to.

## What the pictures claim

The film shows hardware that is not built: the enclosure is a concept, and
power over the Ethernet cable has not been proven on a bench (TODO.md). The
copy about the software is from the site and true of what is built; "fits
in your hand", "clip it on the rail" and "one cable" describe the design.

## Third-party files

Fonts, effects and the bed as in `../helmsman-ad-v2/README.md`.
`assets/app-latch.mp4` is that ad's recording of the real playground.
