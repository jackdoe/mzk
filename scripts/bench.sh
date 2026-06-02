#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

TRACK="${TRACK:-tests/fixtures/voyager/30 - Dark Was the Night, Cold Was the Ground}"
FORMATS=(wav flac mp3 opus aac.m4a alac.m4a)
PERF="${PERF:-1}"

speed_of() { awk -v f="$1" '$1==f{print $4}' "$2"; }

if [[ "${1:-}" == "compare" ]]; then
    A="prof/${2:?usage: bench.sh compare A B}/throughput.txt"
    B="prof/${3:?usage: bench.sh compare A B}/throughput.txt"
    printf "%-8s %12s %12s %9s\n" fmt "$2" "$3" delta
    for ext in "${FORMATS[@]}"; do
        sa=$(speed_of "$ext" "$A" 2>/dev/null || true)
        sb=$(speed_of "$ext" "$B" 2>/dev/null || true)
        [[ -z "$sa" || -z "$sb" ]] && continue
        na=${sa%x}; nb=${sb%x}
        d=$(awk -v a="$na" -v b="$nb" 'BEGIN{printf "%+.1f%%", (b/a-1)*100}')
        printf "%-8s %12s %12s %9s\n" "$ext" "$sa" "$sb" "$d"
    done
    exit 0
fi

LABEL="${1:-$(date +%Y%m%d-%H%M%S)}"
OUT="prof/$LABEL"
mkdir -p "$OUT"
REV=$(git rev-parse --short HEAD 2>/dev/null || echo nogit)

echo "building (profiling)…"
cargo build --profile profiling >/dev/null 2>&1
BIN=target/profiling/mzk

TP="$OUT/throughput.txt"
: > "$TP"
echo "== throughput =="
for ext in "${FORMATS[@]}"; do
    f="$TRACK.$ext"
    [[ -f "$f" ]] || { echo "skip $ext (missing)"; continue; }
    row=$("$BIN" --bench "$f" 2>/dev/null | sed -n '2p')
    # shellcheck disable=SC2086
    set -- $row
    printf '%-9s %9s %10s %8s %11s %9s %9s\n' "$ext" "$2" "$3" "$4" "$5" "$6" "$7" | tee -a "$TP"
done

REPORT="$OUT/report.md"
{
    echo "# mzk bench — $LABEL ($REV)"
    echo
    echo "## Throughput (decode to null)"
    echo
    printf '| %s | %s | %s | %s | %s | %s | %s |\n' fmt audio decode speed samples file rss
    echo '|---|--:|--:|--:|--:|--:|--:|'
    while read -r c1 c2 c3 c4 c5 c6 c7; do
        [[ -z "${c1:-}" ]] && continue
        printf '| %s | %s | %s | %s | %s | %s | %s |\n' "$c1" "$c2" "$c3" "$c4" "$c5" "$c6" "$c7"
    done < "$TP"
} > "$REPORT"

if [[ "$PERF" == "1" ]] && command -v perf >/dev/null && command -v inferno-flamegraph >/dev/null; then
    echo "== perf =="
    declare -A REPS=( [opus]=60 [mp3]=60 [aac]=60 [alac]=60 [flac]=12 )
    { echo; echo "## Hotspots (perf self-cost) & flamegraphs"; } >> "$REPORT"
    for ext in "${FORMATS[@]}"; do
        [[ "$ext" == wav ]] && continue
        f="$TRACK.$ext"
        [[ -f "$f" ]] || continue
        name=${ext%.m4a}
        n=${REPS[$name]:-40}
        args=(); for ((i=0;i<n;i++)); do args+=("$f"); done
        perf record -F 1999 -g --call-graph dwarf -o "$OUT/$name.data" -- \
            "$BIN" --bench "${args[@]}" >/dev/null 2>&1 || { echo "perf failed for $name"; continue; }
        perf script -i "$OUT/$name.data" 2>/dev/null | inferno-collapse-perf 2>/dev/null > "$OUT/$name.folded"
        inferno-flamegraph --title "mzk $name decode ($LABEL)" "$OUT/$name.folded" > "$OUT/$name.svg" 2>/dev/null
        echo "  $name → $OUT/$name.svg"
        {
            echo
            echo "### $name  ([$name.svg]($name.svg))"
            echo '```'
            perf report -i "$OUT/$name.data" --stdio --no-children 2>/dev/null \
                | grep -E '^\s+[0-9]+\.[0-9]+%' \
                | grep -iE 'mzk|libm|libc' \
                | head -10 \
                | sed -E 's/^\s+//; s/\[\.\] //; s/ +mzk +/  /; s/ +/ /2g'
            echo '```'
        } >> "$REPORT"
    done
fi

echo
echo "report: $REPORT"
