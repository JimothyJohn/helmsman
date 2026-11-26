#!/usr/bin/env bash
# Build, check and render the benefit films to marketing/helmsman-benefits-<film>.mp4
# (1920x1080, 30 fps). Needs Node 22+, FFmpeg and python3; HyperFrames runs
# through npx at a pinned version, as in ../helmsman-ad-v2.
#   ./render.sh                    every film, delivery quality
#   ./render.sh draft              every film, a fast pass while editing
#   ./render.sh draft describe     one film (loop-describe), or reel
#   ./render.sh preview reel       live preview of one film in a browser
# The bench shots come from Nick's OBS recording (tools/bench.sh, once).
set -euo pipefail
cd "$(dirname "$0")"
export HYPERFRAMES_NO_TELEMETRY=1
HF="npx --yes hyperframes@0.9.37"
quality="${1:-delivery}"
shift || true
films=("$@")
[ ${#films[@]} -gt 0 ] || films=(describe visualize test history typo connect install reel)
# Only the test and connect loops use the bench shots; the reel renders without them.
for film in "${films[@]}"; do
    case "$film" in
    test | connect) [ -f assets/bench/tune.wav ] || tools/bench.sh ;;
    esac
done
python3 build.py "${films[@]}"
dir() { [ "$1" = reel ] && echo reel || echo "loop-$1"; }
case "$quality" in
preview)
    cd "$(dir "${films[0]}")"
    exec $HF preview
    ;;
draft | looks | delivery)
    for film in "${films[@]}"; do
        (
            cd "$(dir "$film")"
            $HF check
            $HF render --quality "$quality" -o "../../helmsman-benefits-$film.mp4"
            ffprobe -v error -show_entries format=duration,size -of default=nw=1 "../../helmsman-benefits-$film.mp4"
        )
    done
    ;;
*)
    echo "usage: $0 [delivery|looks|draft|preview] [film ...]" >&2
    exit 2
    ;;
esac
