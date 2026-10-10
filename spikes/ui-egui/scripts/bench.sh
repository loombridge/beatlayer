#!/usr/bin/env bash
# Runs the four benchmark configurations RUNS times each, interleaved, and
# keeps each run's summary and per-frame CSV in OUT (default: bench-results/).
#
# Usage, from spikes/ui-egui after `cargo build --release`:
#   scripts/bench.sh [OUT]
#
# Environment:
#   RUNS           runs per configuration (default 3)
#   BENCH_SECONDS  length of each run (default 30)
#   QUIET_FOR      a pgrep -f pattern for work that would skew the numbers
#                  (another benchmark, a build). Each run waits until nothing
#                  matches, and is repeated if something matched while it ran.
#   READY_CMD      a command that succeeds when the machine is ready (for
#                  example, the display is awake and unlocked); each run waits
#                  for it. A run the spike flags with a WARNING is repeated.
set -euo pipefail

out=${1:-bench-results}
bin=${BIN:-target/release/ui-egui-spike}
seconds=${BENCH_SECONDS:-30}
runs=${RUNS:-3}
mkdir -p "$out"

busy() {
    [[ -n "${QUIET_FOR:-}" ]] && pgrep -f "$QUIET_FOR" >/dev/null
}

ready() {
    [[ -z "${READY_CMD:-}" ]] || $READY_CMD >/dev/null
}

run_once() {
    local name=$1
    shift
    while busy || ! ready; do
        sleep 2
    done
    "$bin" --bench --bench-seconds "$seconds" "$@" --bench-out "$out/$name.csv" \
        >"$out/$name.txt" 2>&1 &
    local pid=$! overlapped=0
    while kill -0 "$pid" 2>/dev/null; do
        if busy || ! ready; then
            overlapped=1
        fi
        sleep 1
    done
    wait "$pid"
    # The spike warns when frames weren't presented or paced by the display.
    if grep -q WARNING "$out/$name.txt"; then
        overlapped=1
    fi
    return "$overlapped"
}

for run in $(seq 1 "$runs"); do
    for tracks in standard all-tracks; do
        for dense in gpu painter; do
            name="$dense-$tracks-$run"
            extra=(--dense "$dense")
            if [[ $tracks == all-tracks ]]; then
                extra+=(--all-tracks)
            fi
            for attempt in 1 2 3 4 5; do
                echo "==> $name (attempt $attempt)"
                if run_once "$name" "${extra[@]}"; then
                    cat "$out/$name.txt"
                    break
                fi
                echo "$name was disturbed (see its WARNING, or QUIET_FOR matched); repeating it"
            done
            sleep 3
        done
    done
done
