"""Writes a sine wave to a WAV file: sine.py SEED OUT [INPUT].

The test manifest's generating script. The seed is the frequency in hertz,
so the same seed gives the same bytes.
"""
import math
import struct
import sys
import wave

seed, out = int(sys.argv[1]), sys.argv[2]
rate = 8000
with wave.open(out, "wb") as w:
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(rate)
    w.writeframes(
        b"".join(
            struct.pack("<h", int(8000 * math.sin(2 * math.pi * seed * i / rate)))
            for i in range(rate // 4)
        )
    )
