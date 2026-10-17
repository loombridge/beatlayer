#!/usr/bin/env python3
"""Generates the audio the M2 song test plays, in tests/fixtures/song/.

Every file is synthesised here from nothing: no third-party or recorded
audio. The script needs only Python 3.10 or later and its standard library,
and it is deterministic (its noise is a seeded xorshift32), so running it
again writes the same bytes:

    python3 tests/fixtures/gen/gen_song.py

The files, all mono 16-bit WAV:

- kick.wav: a kick drum, 48 kHz, 0.5 s. The song's Kick track plays it, and
  the sidechain compressor listens to that track.
- kit.wav: four hits of 0.25 s each, 48 kHz, 1 s in all: a snare, a closed
  hat, an open hat and a clap. One sampler cuts it into four equal slices.
- voice.wav: a voice-like phrase, 24 kHz, 2 s (one bar at 120 BPM): four
  sung notes of an "ah" vowel made by formant synthesis. The test plays it
  into the audio input and records it.
- pluck.wav: four plucked-string notes, 24 kHz, 2 s (one bar at 120 BPM),
  which the audio parts play through the CLAP and VST3 plug-ins.
"""

from __future__ import annotations

import math
import struct
import wave
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "song"


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


def write(name: str, rate: int, samples: list[float], peak: float) -> None:
    """Scales `samples` so the loudest is `peak`, and writes them."""
    loudest = max(abs(s) for s in samples)
    scale = peak / loudest
    OUT.mkdir(parents=True, exist_ok=True)
    with wave.open(str(OUT / name), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(rate)
        file.writeframes(
            b"".join(struct.pack("<h", round(s * scale * 32767)) for s in samples)
        )


def kick(rate: int) -> list[float]:
    frames = int(0.5 * rate)
    phase = 0.0
    out = []
    for n in range(frames):
        t = n / rate
        # A sine that falls from about 150 Hz to 45 Hz, decaying.
        phase += 2 * math.pi * (45 + 105 * math.exp(-t / 0.03)) / rate
        sample = math.sin(phase) * math.exp(-t / 0.15)
        # A two-millisecond click on the attack.
        if t < 0.002:
            sample += 0.5 * (1 - t / 0.002)
        # A short fade so the file ends on zero.
        sample *= min(1.0, (frames - n) / (0.01 * rate))
        out.append(sample)
    return out


def hit(rate: int, seconds: float, shape) -> list[float]:
    frames = int(seconds * rate)
    return [shape(n / rate, n) for n in range(frames)]


def kit(rate: int) -> list[float]:
    noise = Noise(0x2545F491)
    last = [0.0, 0.0]

    def snare(t: float, n: int) -> float:
        return 0.7 * noise.next() * math.exp(-t / 0.06) + 0.5 * math.sin(
            2 * math.pi * 190 * t
        ) * math.exp(-t / 0.04)

    def high_passed(decay: float):
        # The noise minus its own average, twice: a crude high-pass.
        def shape(t: float, n: int) -> float:
            x = noise.next()
            y = x - last[0]
            last[0] = x
            z = y - last[1]
            last[1] = y
            return z * math.exp(-t / decay)

        return shape

    def clap(t: float, n: int) -> float:
        burst = 0.0
        for start in (0.0, 0.01, 0.02):
            if t >= start:
                burst += math.exp(-(t - start) / 0.008)
        return noise.next() * (burst * 0.4 + math.exp(-t / 0.07))

    hits = [
        hit(rate, 0.25, snare),
        hit(rate, 0.25, high_passed(0.02)),
        hit(rate, 0.25, high_passed(0.1)),
        hit(rate, 0.25, clap),
    ]
    out = []
    for samples in hits:
        loudest = max(abs(s) for s in samples)
        frames = len(samples)
        # Every hit is as loud as the others, and fades to zero at its end
        # so the slices don't click.
        out += [
            s / loudest * min(1.0, (frames - n) / (0.01 * rate))
            for n, s in enumerate(samples)
        ]
    return out


def voice(rate: int) -> list[float]:
    # Four sung notes of 0.5 s each, with a little vibrato.
    melody = [196.0, 220.0, 246.9, 220.0]
    formants = [(800.0, 80.0), (1200.0, 90.0), (2800.0, 120.0)]
    noise = Noise(0x1F123BB5)
    frames = 2 * rate
    # One two-pole resonator per formant, in parallel.
    states = [[0.0, 0.0] for _ in formants]
    coefficients = []
    for centre, width in formants:
        r = math.exp(-math.pi * width / rate)
        coefficients.append((2 * r * math.cos(2 * math.pi * centre / rate), -r * r))
    out = []
    phase = 0.0
    for n in range(frames):
        t = n / rate
        note = min(int(t / 0.5), 3)
        into = t - note * 0.5
        hz = melody[note] * 2 ** (0.3 * math.sin(2 * math.pi * 5.5 * t) / 12)
        phase = (phase + hz / rate) % 1.0
        # A glottal-like pulse: a falling ramp, with a little breath.
        source = (1.0 - 2.0 * phase) + 0.03 * noise.next()
        envelope = min(1.0, into / 0.03) * min(1.0, (0.5 - into) / 0.08)
        sample = 0.0
        for state, (a1, a2), (_, _) in zip(states, coefficients, formants):
            value = source * 0.1 + a1 * state[0] + a2 * state[1]
            state[1] = state[0]
            state[0] = value
            sample += value
        out.append(sample * envelope)
    return out


def pluck(rate: int) -> list[float]:
    # Karplus-Strong strings: four notes of 0.5 s, each ringing on into the
    # next.
    noise = Noise(0x6D2B79F5)
    frames = 2 * rate
    out = [0.0] * frames
    for index, hz in enumerate([110.0, 130.8, 164.8, 196.0]):
        length = round(rate / hz)
        line = [noise.next() for _ in range(length)]
        start = index * rate // 2
        position = 0
        for n in range(start, frames):
            out[n] += line[position]
            following = line[(position + 1) % length]
            line[position] = 0.996 * 0.5 * (line[position] + following)
            position = (position + 1) % length
    return out


def main() -> None:
    write("kick.wav", 48_000, kick(48_000), 0.95)
    write("kit.wav", 48_000, kit(48_000), 0.9)
    write("voice.wav", 24_000, voice(24_000), 0.8)
    write("pluck.wav", 24_000, pluck(24_000), 0.8)


if __name__ == "__main__":
    main()
