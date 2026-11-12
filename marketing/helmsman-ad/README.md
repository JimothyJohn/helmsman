# Helmsman ad (34.2 s, 1920x1080, 30 fps)

A [HyperFrames](https://github.com/heygen-com/hyperframes) composition: HTML
with a GSAP timeline, rendered frame by frame to `marketing/helmsman-ad.mp4`.

    ./render.sh            # check, then render at delivery quality
    ./render.sh draft      # a fast pass while editing
    ./render.sh preview    # live preview in a browser

Needs Node 22+ and FFmpeg. HyperFrames runs through `npx` at a pinned
version with its telemetry off; nothing is installed into the repo.

## What is in it

| Seconds   | Beat                                                                  |
| --------- | --------------------------------------------------------------------- |
| 0-9.8     | Three complaints from the site's "From the field", each struck out and answered: the editor in a browser, an Apache 2.0 stamp, a plain-text diff |
| 9.3-12.6  | The diff's added line floods yellow: "Write specs, not code"          |
| 12.5-16.5 | 1. Describe: the spec is typed on a card                              |
| 16.5-25.6 | 2. Validate: the card becomes the real app; run, start, stop, with the motor's row on the trend outlined |
| 25.6-29.0 | 3. Approve                                                            |
| 29.0-34.2 | End card: mark, name, "Try it in your browser", helmsman.advin.io     |

`assets/editor.png` is `docs/help/img/editor.png` (the Blink example).
The diff in the third answer is illustrative: an earlier, wrong version of
the latch line against the example's own.

All of it is `index.html`: one clip per beat, the timeline at the bottom in
the same order. Copy is taken from `docs/index.html`; change it there first.

## The app footage is real

`assets/app-latch.mp4` is the published playground (`docs/playground/`)
running the Start/stop latch example, recorded headless:

    python3 -I marketing/helmsman-ad/tools/capture.py docs target/ad-capture
    (cd target/ad-capture && ffmpeg -f concat -safe 0 -i frames.txt -vf fps=30 \
      -c:v libx264 -crf 14 -pix_fmt yuv420p -an ../../marketing/helmsman-ad/assets/app-latch.mp4)

The recorder prints when each press happened. The cursor in the film is
drawn over the footage at those times (`RUN`, `START`, `STOP` and the
`press(...)` calls in `index.html`, offset by the clip's `data-start` minus
its `data-media-start`), so a new recording means new times there. The
spec sentence on the card is the example's own comment; the agent step
itself is not on screen, because the playground needs a key to run it.

## Sound

Effects only, each on a visible action. There is no music bed: add a track
as one more `<audio id=... class="clip">` in `index.html`.

## Third-party files

- `assets/fonts/`: Saira Stencil One and Barlow Condensed 700, the site's
  faces, from Google Fonts under the SIL Open Font License 1.1.
- `assets/sfx/`: from HyperFrames' bundled set, sourced from Pixabay under
  the Pixabay Content License (no attribution required).
