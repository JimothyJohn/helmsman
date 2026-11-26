#!/usr/bin/env bash
# Cut the bench shots for the benefit films from Nick's OBS recording of the
# 2026-11-25 demo: the webcam inset in its bottom right corner (584x588 at
# 1336,492 of the 1920x1080 screen), squared up to 1080x1080.
#   tools/bench.sh [recording.mp4]
# Writes assets/bench/{skipper,spin,tune}.mp4. The tune keeps its sound:
# the motor playing Mary had a little lamb (tones from 590.3 s to 599.7 s
# of the recording, read off a spectrogram).
set -euo pipefail
cd "$(dirname "$0")/.."
src="${1:-$HOME/Movies/OBS/processed/2026-11-25/clean/2026-11-25 12-54-54 clean.mp4}"
[ -f "$src" ] || { echo "no recording at $src" >&2; exit 1; }
mkdir -p assets/bench
inset="crop=584:588:1336:492,scale=1080:1080:flags=lanczos,unsharp=5:5:0.6,fps=30,format=yuv420p"
cut() { # name start seconds [audio]
    local audio=(-an)
    [ "${4:-}" = audio ] && audio=(-c:a aac -b:a 192k -af "loudnorm=I=-16:TP=-1.5")
    ffmpeg -v error -y -ss "$2" -t "$3" -i "$src" -vf "$inset" -c:v libx264 -crf 16 "${audio[@]}" "assets/bench/$1.mp4"
    echo "assets/bench/$1.mp4"
}
cut skipper 218.9 1.8         # the controller board in hand, 3:39
cut spin 505.0 4.0            # the motor turning at one revolution a second, 8:25
cut tune 589.8 10.4 audio     # the motor playing the tune, 9:50
# The tune's sound on its own, for the films' audio track.
ffmpeg -v error -y -i assets/bench/tune.mp4 -vn -c:a pcm_s16le assets/bench/tune.wav
echo assets/bench/tune.wav
