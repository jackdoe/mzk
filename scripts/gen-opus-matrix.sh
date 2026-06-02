#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Regenerates the CELT-config test matrix under tests/fixtures/opus-matrix/.
# Each .opus is a CELT-only Opus (forced via -application lowdelay) at a given
# frame size and bandwidth; the paired .f32le is ffmpeg's own decode, the RMS
# reference for opus::tests::decodes_celt_matrix_within_rms.

SRC="${SRC:-tests/fixtures/voyager/30 - Dark Was the Night, Cold Was the Ground.wav}"
DIR="tests/fixtures/opus-matrix"
SECONDS_CLIP="${SECONDS_CLIP:-5}"

if [[ ! -f "$SRC" ]]; then
    echo "source missing: $SRC (run scripts/gen-fixtures.sh first)" >&2
    exit 1
fi
mkdir -p "$DIR"

# name             frame_ms  cutoff(Hz)  -> expected config
#  fb_20ms          20        20000          31
#  fb_10ms          10        20000          30
#  fb_5ms           5         20000          29
#  wb_20ms          20        8000           23
#  nb_20ms          20        4000           19
gen() {
    local name=$1 fd=$2 cut=$3
    ffmpeg -hide_banner -loglevel error -y -i "$SRC" -t "$SECONDS_CLIP" \
        -c:a libopus -application lowdelay -frame_duration "$fd" -cutoff "$cut" \
        -b:a 128k "$DIR/$name.opus"
    ffmpeg -hide_banner -loglevel error -y -i "$DIR/$name.opus" \
        -f f32le -ac 2 -ar 48000 "$DIR/$name.f32le"
    echo "  $name"
}

gen fb_20ms 20 20000
gen fb_10ms 10 20000
gen fb_5ms  5  20000
gen wb_20ms 20 8000
gen nb_20ms 20 4000
echo "wrote $DIR"
