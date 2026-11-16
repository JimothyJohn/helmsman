# Helmsman device film, three rough cuts (1920x1080, 30 fps)

The second ad (`../helmsman-ad-v2`) with the controller as an object in a
real plant: it is clipped onto a rail, one Ethernet cable goes in and its
light comes on. Same palette, faces, 120 BPM grid, slash and end card; the
pictures are live action instead of type on a ground. The device's look is
specified in `docs/brand/index.html#device`.

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

`assets/footage/*.mp4` are generated: one composed still per shot
(Seedream 4.5), then that still animated as the first frame of a 4 to 6 s
clip (MiniMax H3, the 768P draft model) through the `storyboard` CLI in
`~/github/storyboard`. `footage/shots.py` holds every prompt and is the
record of how each shot was made:

    footage/shots.py stills [name ...]   # about $0.04 a still
    footage/shots.py clips  [name ...]   # about $0.02 a second
    footage/shots.py sheet

then copy `footage/outputs/box/clip-<name>.mp4` to
`assets/footage/<name>.mp4`. The device stays the same object from shot to
shot because every device still is composed from the same three reference
pictures (`docs/brand/helmsman-device*.jpg`). Look at every clip before it
goes in a cut: the first plug-in take grew a second socket for the cable.

These are drafts at 1344x768, scaled up. A cut that is kept gets its shots
rendered once more at 2K on the full model (`--resolution 2K`, about $0.13
a second) before it ships.

The moments the cuts land on a beat were measured in the clips and are the
constants at the top of `build.py`: the light bar coming on, the stack
light turning green, the rail click, the rack's thud, the door's clunk.

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
