#!/usr/bin/env python3
"""Generates the warp-mode measurement fixtures in tests/fixtures/warp/ (WP-4.19, warp-modes R34).

Mono, 48 kHz, 24-bit PCM WAV files, synthesised here from sines and xorshift32
noise, so no recorded or third-party audio is involved and the same script
always writes the same bytes. It writes:

- sine-440.wav: a 440 Hz sine at -6 dBFS peak, 8 seconds;
- saw-220.wav: a 220 Hz sawtooth band-limited to harmonics up to 10 kHz, at
  -6 dBFS peak, 8 seconds;
- pink-noise.wav: pink noise from 20 Hz up at -18 dBFS RMS, 8 seconds;
- vowel.wav: a 150 Hz pulse train through resonators at 700, 1,220 and 2,600 Hz,
  at -6 dBFS peak, 8 seconds;
- sweep.wav: a logarithmic sine sweep from 20 Hz to 20 kHz at -6 dBFS peak,
  10 seconds, with a 5 ms raised-cosine fade at each end.

The 120 BPM drum loop and its hit times are tests/fixtures/drums/, which
gen_drums.py writes and this script runs too. Run it from anywhere with Python
3.10 or later and no packages:

    python3 tests/fixtures/gen/gen_warp.py
"""

from __future__ import annotations

import math
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "warp"
RATE = 48_000
SECONDS = 8.0
SWEEP_SECONDS = 10.0
# -6 dBFS as a linear amplitude.
MINUS_6_DB = 10 ** (-6 / 20)
SWEEP_FADE = 0.005


def frames(seconds: float) -> int:
    return round(seconds * RATE)


def sine() -> list[float]:
    return [
        MINUS_6_DB * math.sin(2 * math.pi * 440 * n / RATE)
        for n in range(frames(SECONDS))
    ]


def sawtooth() -> list[float]:
    """The first harmonics of a 220 Hz sawtooth up to 10 kHz, scaled to peak -6 dBFS."""
    harmonics = int(10_000 // 220)
    out = [0.0] * frames(SECONDS)
    for k in range(1, harmonics + 1):
        sign = 1.0 if k % 2 else -1.0
        step = 2 * math.pi * 220 * k / RATE
        for n in range(len(out)):
            out[n] += sign / k * math.sin(step * n)
    peak = max(abs(x) for x in out)
    return [x * MINUS_6_DB / peak for x in out]


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


def high_pass(samples: list[float], hz: float, q: float) -> list[float]:
    """One second-order Butterworth-section high-pass (the RBJ biquad)."""
    w = 2 * math.pi * hz / RATE
    alpha = math.sin(w) / (2 * q)
    a0 = 1 + alpha
    b0 = (1 + math.cos(w)) / 2 / a0
    b1 = -(1 + math.cos(w)) / a0
    a1 = -2 * math.cos(w) / a0
    a2 = (1 - alpha) / a0
    x1 = x2 = y1 = y2 = 0.0
    out = []
    for x in samples:
        y = b0 * x + b1 * x1 + b0 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, x, y1, y
        out.append(y)
    return out


def pink_noise() -> list[float]:
    """White noise through Paul Kellet's refined pink filter and a fourth-order
    20 Hz high-pass (pink noise is the audio band's, 20 Hz up), at -18 dBFS RMS."""
    noise = Noise(0x2545F491)
    b = [0.0] * 7
    out = []
    for _ in range(frames(SECONDS) + RATE // 4):
        white = noise.next()
        b[0] = 0.99886 * b[0] + white * 0.0555179
        b[1] = 0.99332 * b[1] + white * 0.0750759
        b[2] = 0.96900 * b[2] + white * 0.1538520
        b[3] = 0.86650 * b[3] + white * 0.3104856
        b[4] = 0.55000 * b[4] + white * 0.5329522
        b[5] = -0.7616 * b[5] - white * 0.0168980
        out.append(b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.5362)
        b[6] = white * 0.115926
    # Drop the filters' start-up, which has no pink spectrum yet.
    for q in (0.5411961, 1.3065630):
        out = high_pass(out, 20.0, q)
    out = out[RATE // 4 :]
    rms = math.sqrt(sum(x * x for x in out) / len(out))
    gain = 10 ** (-18 / 20) / rms
    return [x * gain for x in out]


def vowel() -> list[float]:
    """A 150 Hz pulse train through resonators at 700, 1,220 and 2,600 Hz."""
    out = [0.0] * frames(SECONDS)
    period = RATE // 150
    for n in range(len(out)):
        if n % period < 4:
            out[n] = 1.0
    for freq, bandwidth in [(700.0, 80.0), (1220.0, 90.0), (2600.0, 120.0)]:
        r = math.exp(-math.pi * bandwidth / RATE)
        two_cos = 2 * r * math.cos(2 * math.pi * freq / RATE)
        a1 = a2 = 0.0
        for n, x in enumerate(out):
            y = (1 - r) * x + two_cos * a1 - r * r * a2
            a2, a1 = a1, y
            out[n] = y
    peak = max(abs(x) for x in out)
    return [x * MINUS_6_DB / peak for x in out]


def sweep() -> list[float]:
    """A logarithmic sine sweep from 20 Hz to 20 kHz: phase 2*pi*f0*T/ln(k) * (k^(t/T) - 1)."""
    f0, f1 = 20.0, 20_000.0
    ratio = f1 / f0
    total = frames(SWEEP_SECONDS)
    coefficient = 2 * math.pi * f0 * SWEEP_SECONDS / math.log(ratio)
    fade = round(SWEEP_FADE * RATE)
    out = []
    for n in range(total):
        phase = coefficient * (ratio ** (n / RATE / SWEEP_SECONDS) - 1)
        edge = min(n, total - 1 - n)
        gain = 0.5 - 0.5 * math.cos(math.pi * edge / fade) if edge < fade else 1.0
        out.append(MINUS_6_DB * gain * math.sin(phase))
    return out


def write_wav(path: Path, samples: list[float]) -> None:
    """A mono 24-bit PCM WAV file."""
    pcm = bytearray()
    for x in samples:
        value = max(-8_388_608, min(8_388_607, round(x * 8_388_608)))
        pcm += struct.pack("<i", value)[:3]
    fmt = struct.pack("<HHIIHH", 1, 1, RATE, RATE * 3, 3, 24)
    body = (
        b"WAVE"
        + b"fmt "
        + struct.pack("<I", len(fmt))
        + fmt
        + b"data"
        + struct.pack("<I", len(pcm))
        + bytes(pcm)
    )
    path.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for name, make in [
        ("sine-440", sine),
        ("saw-220", sawtooth),
        ("pink-noise", pink_noise),
        ("vowel", vowel),
        ("sweep", sweep),
    ]:
        write_wav(OUT / f"{name}.wav", make())
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(HERE))
    import gen_drums

    gen_drums.main()


if __name__ == "__main__":
    main()
