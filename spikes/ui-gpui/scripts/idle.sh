#!/bin/sh
# Idle measurement: starts the spike with meters stopped and nothing moving,
# waits for it to settle, then samples the process once a second for 30
# seconds with ps. Prints each sample, the mean %CPU, the CPU time used over
# the window (from ps's cumulative TIME), and resident memory.
#
# Usage: scripts/idle.sh [extra spike options]
set -eu
cd "$(dirname "$0")/.."
bin=target/release/bl-spike-ui-gpui
[ -x "$bin" ] || cargo build --release
log=$(mktemp -t bl-spike-idle)
"$bin" --idle "$@" 2>"$log" &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT
# Let start-up, the plug-in window, and any first-frame work finish.
sleep 5
grep -E '^(data|ready|plug-in)' "$log" || true

secs() { # [[dd-]hh:]mm:ss.cc -> seconds
  echo "$1" | awk -F'[-:]' '{ s = 0; for (i = 1; i <= NF; i++) s = s * 60 + $i; print s }'
}
t0=$(secs "$(ps -o time= -p $pid | tr -d ' ')")
sum=0
n=0
i=1
while [ $i -le 30 ]; do
  sleep 1
  line=$(ps -o %cpu=,rss= -p $pid)
  cpu=$(echo "$line" | awk '{print $1}')
  rss=$(echo "$line" | awk '{print $2}')
  echo "sample $i: cpu ${cpu}% rss ${rss} KiB"
  sum=$(echo "$sum + $cpu" | bc -l)
  n=$((n + 1))
  i=$((i + 1))
done
t1=$(secs "$(ps -o time= -p $pid | tr -d ' ')")
echo "mean %CPU over $n ps samples: $(echo "scale=2; $sum / $n" | bc -l)"
echo "CPU time used in the window: $(echo "scale=3; $t1 - $t0" | bc -l) s of 30 s ($(echo "scale=2; ($t1 - $t0) * 100 / 30" | bc -l)%)"
echo "resident memory at the end: $(ps -o rss= -p $pid | tr -d ' ') KiB"
