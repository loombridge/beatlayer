#!/usr/bin/env python3
"""Generates the drum loop in tests/fixtures/drums/ (WP-4.3, the warping spec R22).

One bar of 4/4 at 120 BPM, 2 seconds of 48 kHz 16-bit mono, synthesised here
from sines and xorshift32 noise, so no recorded or third-party audio is
involved and the same script always writes the same bytes. It writes:

- drum-loop.wav: the loop;
- drum-loop.hits: the time of every hit in seconds, one a line, in order. Each
  hit starts on a whole sample, so these are exact.

The transient tests in crates/analysis/tests/ read both. Run it from anywhere
with Python 3.10 or later and no packages:

    python3 tests/fixtures/gen/gen_drums.py
"""

from __future__ import annotations

import math
import struct
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "drums"
RATE = 48_000
SECONDS = 2.0

# (time in seconds, drum). Kick on beats 1 and 3 and a late one, snare on
# beats 2 and 4, hats between the beats; the closest two hits are 125 ms apart.
HITS = [
    (0.0, "kick"),
    (0.25, "hat"),
    (0.5, "snare"),
    (0.75, "hat"),
    (1.0, "kick"),
    (1.25, "hat"),
    (1.375, "kick"),
    (1.5, "snare"),
    (1.75, "hat"),
]


class Noise:
    """Uniform noise in [-1, 1) from xorshift32."""

    def __init__(self, seed: int) -> None:
        self.state = seed

    def next(self) -> float:
        s = self.state
        s ^= (s << 13) & 0xFFFFFFFF
        s ^= s >> 17
        s ^= (s << 5) & 0xFFFFFFFF
        self.state = s
        return s / 2**31 - 1.0


def render(drum: str, noise: Noise) -> list[float]:
    """One drum hit from its first sample, a click and then its body."""
    length = {"kick": 0.30, "snare": 0.20, "hat": 0.06}[drum]
    out = []
    phase = 0.0
    previous = 0.0
    for n in range(int(length * RATE)):
        t = n / RATE
        click = noise.next() * math.exp(-t / 0.001) * 0.5
        if drum == "kick":
            # A sine gliding from 160 Hz down to 45 Hz.
            hz = 45.0 + 115.0 * math.exp(-t / 0.03)
            phase += 2 * math.pi * hz / RATE
            body = 0.8 * math.sin(phase) * math.exp(-t / 0.08)
        elif drum == "snare":
            body = 0.5 * noise.next() * math.exp(-t / 0.05)
            body += 0.3 * math.sin(2 * math.pi * 190 * t) * math.exp(-t / 0.04)
        else:
            # Noise with its low end taken out by differencing.
            raw = noise.next()
            body = 0.35 * (raw - previous) * math.exp(-t / 0.012)
            previous = raw
        out.append(click + body)
    return out


def main() -> None:
    frames = int(SECONDS * RATE)
    mix = [0.0] * frames
    noise = Noise(0x9E3779B9)
    for time, drum in HITS:
        start = round(time * RATE)
        for n, x in enumerate(render(drum, noise)):
            if start + n < frames:
                mix[start + n] += x
    peak = max(abs(x) for x in mix)
    scale = 0.8 / peak
    pcm = b"".join(struct.pack("<h", round(x * scale * 32767)) for x in mix)
    fmt = struct.pack("<HHIIHH", 1, 1, RATE, RATE * 2, 2, 16)
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(pcm)) + pcm
    OUT.mkdir(exist_ok=True)
    (OUT / "drum-loop.wav").write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)
    (OUT / "drum-loop.hits").write_text("".join(f"{round(t * RATE) / RATE}\n" for t, _ in HITS))


if __name__ == "__main__":
    main()
