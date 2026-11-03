#!/usr/bin/env python3
"""Generates the sound families in tests/fixtures/families/ (WP-9.8, the sound
similarity spec R9).

Five families (kick, snare, hat, bass, pad), eight sounds each, each family
made from one recipe with a random variation per sound (pitch, length, noise).
22.05 kHz 16-bit mono WAV, synthesised here from sines and xorshift32 noise,
so no recorded or third-party audio is involved and the same script always
writes the same bytes. File names are `<family>-<n>.wav`. The test in
crates/analysis/tests/similarity.rs checks that each sound's five nearest
neighbours are of its own family. Run it from anywhere with Python 3.10 or
later and no packages:

    python3 tests/fixtures/gen/gen_families.py
"""

from __future__ import annotations

import math
import struct
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "families"
RATE = 22_050
PER_FAMILY = 8


class Noise:
    """Uniform noise in [-1, 1) from xorshift32."""

    def __init__(self, seed: int) -> None:
        self.state = seed

    def next(self) -> float:
        x = self.state
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        self.state = x
        return x / 2**31 - 1.0

    def between(self, low: float, high: float) -> float:
        return low + (high - low) * (self.next() + 1.0) / 2.0


def frames(seconds: float) -> int:
    return round(seconds * RATE)


def kick(n: Noise) -> list[float]:
    length = n.between(0.30, 0.45)
    start, end = n.between(120, 180), n.between(40, 60)
    phase = 0.0
    out = []
    for i in range(frames(length)):
        t = i / RATE
        hz = end + (start - end) * math.exp(-t * 30)
        phase += 2 * math.pi * hz / RATE
        out.append(math.sin(phase) * math.exp(-t * n.between(7, 9)) + (0.3 * n.next() if i < 40 else 0.0))
    return out


def snare(n: Noise) -> list[float]:
    length = n.between(0.20, 0.30)
    hz = n.between(170, 210)
    out = []
    for i in range(frames(length)):
        t = i / RATE
        tone = math.sin(2 * math.pi * hz * t) * math.exp(-t * 25)
        out.append(0.5 * tone + 0.7 * n.next() * math.exp(-t * n.between(14, 18)))
    return out


def hat(n: Noise) -> list[float]:
    length = n.between(0.06, 0.12)
    decay = n.between(35, 55)
    out = []
    last = 0.0
    for i in range(frames(length)):
        t = i / RATE
        fresh = n.next()
        out.append((fresh - last) * math.exp(-t * decay))  # first difference: high-passed noise
        last = fresh
    return out


def bass(n: Noise) -> list[float]:
    length = n.between(0.5, 0.7)
    hz = n.between(40, 70)
    out = []
    for i in range(frames(length)):
        t = i / RATE
        env = min(1.0, t * 80) * math.exp(-t * 2.5)
        wave = sum(math.sin(2 * math.pi * hz * k * t) / k for k in (1, 2, 3, 4))
        out.append(0.4 * wave * env)
    return out


def pad(n: Noise) -> list[float]:
    length = n.between(0.9, 1.1)
    root = n.between(180, 330)
    out = []
    for i in range(frames(length)):
        t = i / RATE
        env = min(1.0, t * 5) * min(1.0, (length - t) * 5)
        wave = 0.0
        for ratio in (1.0, 1.25, 1.5):
            for detune in (0.997, 1.003):
                hz = root * ratio * detune
                wave += sum(math.sin(2 * math.pi * hz * k * t) / k for k in (1, 2, 3))
        out.append(0.12 * wave * env)
    return out


FAMILIES = {"kick": kick, "snare": snare, "hat": hat, "bass": bass, "pad": pad}


def write(path: Path, samples: list[float]) -> None:
    peak = max(1e-9, max(abs(s) for s in samples))
    data = b"".join(struct.pack("<h", round(0.8 * 32767 * s / peak)) for s in samples)
    header = (
        b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt "
        + struct.pack("<IHHIIHH", 16, 1, 1, RATE, RATE * 2, 2, 16)
        + b"data" + struct.pack("<I", len(data))
    )
    path.write_bytes(header + data)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for index, (name, recipe) in enumerate(FAMILIES.items()):
        for i in range(PER_FAMILY):
            write(OUT / f"{name}-{i + 1}.wav", recipe(Noise(0x9E3779B1 + 1000 * index + i)))


if __name__ == "__main__":
    main()
