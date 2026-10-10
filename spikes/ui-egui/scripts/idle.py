#!/usr/bin/env python3
"""Idle measurement for the egui spike (macOS and Linux).

Starts the spike with its meters stopped and nothing moving, waits for it to
settle, then samples the process with `ps` once a second. Reports the mean of
ps's %CPU column, the CPU time used over the window divided by wall time (the
more exact figure: macOS's %CPU is a decaying average), and resident memory.

Usage: scripts/idle.py [--seconds 30] [--settle 10] [-- spike options...]
Run it from spikes/ui-egui after `cargo build --release`.
"""

import argparse
import subprocess
import sys
import time


def cpu_seconds(text: str) -> float:
    """Parses ps's cputime column: [[dd-]hh:]mm:ss[.cc]."""
    text = text.strip()
    days = 0
    if "-" in text:
        d, text = text.split("-", 1)
        days = int(d)
    parts = [float(p) for p in text.split(":")]
    seconds = 0.0
    for p in parts:
        seconds = seconds * 60 + p
    return days * 86400 + seconds


def sample(pid: int):
    out = subprocess.run(
        ["ps", "-o", "%cpu=,rss=,time=", "-p", str(pid)],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    return float(out[0]), int(out[1]), cpu_seconds(out[2])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--settle", type=float, default=10.0)
    parser.add_argument("--binary", default="target/release/ui-egui-spike")
    parser.add_argument("spike_args", nargs="*")
    args = parser.parse_args()

    command = [args.binary, "--meters-off", "--on-top", *args.spike_args]
    app = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        time.sleep(args.settle)
        if app.poll() is not None:
            print(app.stdout.read())
            print("the spike exited early", file=sys.stderr)
            return 1

        start_wall = time.monotonic()
        _, _, start_cpu = sample(app.pid)
        rows = []
        for i in range(args.seconds):
            # Sample on whole seconds from the start, so ps itself doesn't drift.
            time.sleep(max(0.0, start_wall + i + 1 - time.monotonic()))
            rows.append((time.monotonic() - start_wall, *sample(app.pid)))
        wall = rows[-1][0]
        cpu_used = rows[-1][3] - start_cpu

        print(f"command: {' '.join(command)}")
        print("t_s   %cpu  rss_kb   cputime_s")
        for t, pct, rss, cpu in rows:
            print(f"{t:5.1f} {pct:5.1f} {rss:8d} {cpu:9.2f}")
        mean_pct = sum(r[1] for r in rows) / len(rows)
        rss = [r[2] for r in rows]
        print(f"samples                {len(rows)} over {wall:.1f} s")
        print(f"mean of ps %cpu        {mean_pct:.2f}%")
        print(f"cpu time / wall time   {100 * cpu_used / wall:.2f}% ({cpu_used:.2f} s in {wall:.1f} s)")
        print(f"resident memory        mean {sum(rss) / len(rss) / 1024:.1f} MiB, max {max(rss) / 1024:.1f} MiB")
    finally:
        app.terminate()
        try:
            out, _ = app.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            app.kill()
            out, _ = app.communicate()
        print("--- spike output ---")
        print(out.strip())
    return 0


if __name__ == "__main__":
    sys.exit(main())
