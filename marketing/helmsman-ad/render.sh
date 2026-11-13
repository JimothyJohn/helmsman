#!/usr/bin/env bash
# Check and render the ad to marketing/helmsman-ad.mp4.
# Needs Node 22+ and FFmpeg; HyperFrames runs through npx at a pinned
# version, so nothing is installed into the repo.
#   ./render.sh            delivery quality
#   ./render.sh draft      a fast pass while editing
#   ./render.sh preview    live preview in a browser (the HyperFrames studio)
set -euo pipefail
cd "$(dirname "$0")"
export HYPERFRAMES_NO_TELEMETRY=1
HF="npx --yes hyperframes@0.9.37"
case "${1:-delivery}" in
preview)
    exec $HF preview
    ;;
draft | looks | delivery)
    $HF check
    $HF render --quality "${1:-delivery}" -o ../helmsman-ad.mp4
    ffprobe -v error -show_entries format=duration,size -of default=nw=1 ../helmsman-ad.mp4
    ;;
*)
    echo "usage: $0 [delivery|looks|draft|preview]" >&2
    exit 2
    ;;
esac
