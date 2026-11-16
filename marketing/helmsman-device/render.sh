#!/usr/bin/env bash
# Build, check and render the device film's rough cuts to
# marketing/helmsman-device-{a,b,c}.mp4. Needs Node 22+, FFmpeg and python3;
# HyperFrames runs through npx at a pinned version, as in ../helmsman-ad-v2.
#   ./render.sh              all three, delivery quality
#   ./render.sh draft        all three, a fast pass while editing
#   ./render.sh draft b      one cut
#   ./render.sh preview b    live preview of one cut in a browser
set -euo pipefail
cd "$(dirname "$0")"
export HYPERFRAMES_NO_TELEMETRY=1
HF="npx --yes hyperframes@0.9.37"
quality="${1:-delivery}"
cuts="${2:-a b c}"
python3 build.py
case "$quality" in
preview)
    cd "cut-${2:-a}"
    exec $HF preview
    ;;
draft | looks | delivery)
    for cut in $cuts; do
        (
            cd "cut-$cut"
            $HF check
            $HF render --quality "$quality" -o "../../helmsman-device-$cut.mp4"
            ffprobe -v error -show_entries format=duration,size -of default=nw=1 "../../helmsman-device-$cut.mp4"
        )
    done
    ;;
*)
    echo "usage: $0 [delivery|looks|draft|preview] [a|b|c]" >&2
    exit 2
    ;;
esac
