#!/usr/bin/env python3
"""Generates the user wavetable fixtures in tests/fixtures/wavetables/.

Every file is synthesised here from sine waves: no third-party or recorded
audio, and no copy of a factory table. The script writes the WAV and AIFF
files byte by byte, so it needs only Python 3.10 or later:

    python3 tests/fixtures/gen/gen_wavetables.py

The files (the wavetable spec, R6):

- plain-3-frames.wav: 3 frames of 2,048 samples with no `clm ` chunk, each a
  sine of its own harmonic riding on a DC offset, so Raw's clean-up shows.
- clm-4-frames.wav: 4 frames of 1,024 samples with a `clm ` chunk naming
  that frame size.
- short-300.wav: one cycle in 300 samples, shorter than a frame.
- empty.wav: a WAV file with no samples.
- plain-2-frames.aif: an AIFF file of 2 frames and a part of a third.
"""

from __future__ import annotations

import math
import struct
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "wavetables"
RATE = 48_000
DC = 0.2


def cycle(size: int, harmonic: int, dc: float = 0.0) -> list[float]:
    return [
        0.5 * math.sin(2 * math.pi * harmonic * n / size) + dc for n in range(size)
    ]


def pcm16(samples: list[float]) -> list[int]:
    return [round(max(-1.0, min(1.0, x)) * 32767) for x in samples]


def chunk(tag: bytes, data: bytes) -> bytes:
    pad = b"\0" if len(data) % 2 else b""
    return tag + struct.pack("<I", len(data)) + data + pad


def wav(samples: list[float], extra: bytes = b"") -> bytes:
    fmt = struct.pack("<HHIIHH", 1, 1, RATE, RATE * 2, 2, 16)
    data = struct.pack(f"<{len(samples)}h", *pcm16(samples))
    body = b"WAVE" + chunk(b"fmt ", fmt) + extra + chunk(b"data", data)
    return b"RIFF" + struct.pack("<I", len(body)) + body


def extended(rate: int) -> bytes:
    """An 80-bit IEEE extended float, as an AIFF `COMM` chunk holds its rate."""
    exponent = rate.bit_length() - 1
    mantissa = rate << (63 - exponent)
    return struct.pack(">HQ", 0x3FFF + exponent, mantissa)


def aiff(samples: list[float]) -> bytes:
    comm = struct.pack(">hIh", 1, len(samples), 16) + extended(RATE)
    sound = struct.pack(">II", 0, 0) + struct.pack(f">{len(samples)}h", *pcm16(samples))

    def be_chunk(tag: bytes, data: bytes) -> bytes:
        pad = b"\0" if len(data) % 2 else b""
        return tag + struct.pack(">I", len(data)) + data + pad

    body = b"AIFF" + be_chunk(b"COMM", comm) + be_chunk(b"SSND", sound)
    return b"FORM" + struct.pack(">I", len(body)) + body


def main() -> None:
    OUT.mkdir(exist_ok=True)
    plain = [x for h in (1, 2, 3) for x in cycle(2048, h, DC)]
    (OUT / "plain-3-frames.wav").write_bytes(wav(plain))

    clm = b"<!>1024 10000000 beatlayer fixtures"
    four = [x for h in (1, 2, 3, 4) for x in cycle(1024, h)]
    (OUT / "clm-4-frames.wav").write_bytes(wav(four, chunk(b"clm ", clm)))

    (OUT / "short-300.wav").write_bytes(wav(cycle(300, 1)))
    (OUT / "empty.wav").write_bytes(wav([]))

    two_and_a_part = [x for h in (1, 2) for x in cycle(2048, h)] + cycle(1000, 3)
    (OUT / "plain-2-frames.aif").write_bytes(aiff(two_and_a_part))


if __name__ == "__main__":
    main()
