#!/bin/sh
# Runs the 30-second scripted bench and reports its frame statistics. The
# spike activates its own window; keep it in front and leave the machine
# otherwise quiet while it runs. Per-frame data goes to <prefix>.csv.
#
# Usage: scripts/bench.sh [prefix] [extra spike options]
#   e.g. scripts/bench.sh results/bench-default
#        scripts/bench.sh results/bench-a11y --a11y-active
set -eu
cd "$(dirname "$0")/.."
prefix=${1:-results/bench}
[ $# -gt 0 ] && shift
mkdir -p "$(dirname "$prefix")"
bin=target/release/bl-spike-ui-gpui
[ -x "$bin" ] || cargo build --release
"$bin" --bench --bench-out "$prefix" "$@"
echo "resident memory is not sampled here; see scripts/idle.sh"
