#!/usr/bin/env bash
set -euo pipefail

ID=voyager-golden-record-cd-ozma
DIR="$(cd "$(dirname "$0")/.." && pwd)/tests/fixtures/voyager"
CLIP_SECONDS="${CLIP_SECONDS:-30}"
mkdir -p "$DIR"

FILES=(
    "23 - Izlel e Delyu Haydutin.flac"
    "30 - Dark Was the Night, Cold Was the Ground.flac"
)

for f in "${FILES[@]}"; do
    src="$DIR/$f"
    if [[ ! -f "$src" ]]; then
        url="https://archive.org/download/$ID/${f// /%20}"
        echo "fetch  $f"
        curl -fL --retry 3 -o "$src" "$url"
    fi
    base="${src%.flac}"
    echo "encode $f"
    ffmpeg -y -loglevel error -i "$src" -t "$CLIP_SECONDS" -vn -map 0:a:0 -f f32le -acodec pcm_f32le "$base.f32le"
    ffmpeg -y -loglevel error -i "$src" -t "$CLIP_SECONDS" -vn -map 0:a:0 "$base.wav"
    ffmpeg -y -loglevel error -i "$src" -t "$CLIP_SECONDS" -vn -map 0:a:0 -c:a alac "$base.alac.m4a"
    ffmpeg -y -loglevel error -i "$src" -t "$CLIP_SECONDS" -vn -map 0:a:0 -c:a aac "$base.aac.m4a"
    ffmpeg -y -loglevel error -i "$base.aac.m4a" -f f32le -acodec pcm_f32le "$base.aac.f32le"
done

echo "done -> $DIR"
