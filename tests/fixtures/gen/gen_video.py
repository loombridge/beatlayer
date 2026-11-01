#!/usr/bin/env python3
"""Generates the video fixtures in tests/fixtures/video/.

Every fixture is synthesised here: no third-party footage or audio. Each
frame is a 64 by 48 picture whose top row of eight 8 by 8 cells holds the
frame's index in binary (white is 1, black is 0, cell 0 the lowest bit), with
a red square at the bottom right and a green one at the bottom left, so a
wrong frame, a swapped colour channel and a flipped picture all show. Flat
cells survive any codec, so the tests read the index back from a decoded
frame.

The Motion JPEG movies are written byte by byte by this script: the JPEG
frames (every 8 by 8 block holds only its DC coefficient), the `moov` box,
the sample tables and the edit lists. They need no external tool:

    python3 tests/fixtures/gen/gen_video.py

The H.264, HEVC and ProRes movies come from ffmpeg's libx264, libx265 and
prores_ks encoders and are written only if ffmpeg is on the path; the
committed files are the reference. The manifest records ffmpeg's version.

The fixtures:

- mjpeg-24f.mov: 24 frames at 30000/1001 per second, and one second of a
  22.05 kHz mono 16-bit sine as `sowt` audio; mjpeg-24f.wav holds the same
  samples as a WAV file, which the audio test compares against.
- mjpeg-gaps.mov: the same media under edit lists with gaps. Video: frames 0
  to 7, four frames of nothing, frames 8 to 15; audio: 0.1 s of silence, the
  first 0.2 s of the sine, 0.1 s of silence, then 0.3 s of the sine from 0.2 s.
  The audio runs longer than the video.
- mjpeg-reordered.mov: 12 frames stored in an order unlike their display
  order (pairs swapped), with a composition-offset table and an edit list.
  Each picture holds its display index.
- h264.mov, hevc.mov, prores.mov: the 24 frames (H.264 with B-frames).
"""

from __future__ import annotations

import json
import math
import shutil
import struct
import subprocess
import wave
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "video"

W, H = 64, 48
TIMESCALE = 30000  # the video's media time scale and the movie's
FRAME = 1001  # one frame, in those units: 30000/1001 frames a second
RATE = 22050  # the audio's sample rate


# The pictures.


def picture(index: int):
    """Rows of (r, g, b) pixels for frame `index`."""
    rows = [[(128, 128, 128)] * W for _ in range(H)]
    for bit in range(8):
        colour = (255, 255, 255) if index >> bit & 1 else (0, 0, 0)
        for y in range(8):
            for x in range(8 * bit, 8 * bit + 8):
                rows[y][x] = colour
    for y in range(H - 16, H):
        for x in range(16):
            rows[y][x] = (0, 255, 0)
        for x in range(W - 16, W):
            rows[y][x] = (255, 0, 0)
    return rows


# A baseline JPEG whose blocks hold only a DC coefficient.

DC_COUNTS = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0]
DC_VALUES = list(range(12))
AC_COUNTS = [1] + [0] * 15
AC_VALUES = [0x00]  # end of block, the only symbol a DC-only block uses
QUANT = 8  # DC / 8, so a block's quantised DC is its level minus 128


def huffman_codes(counts, values):
    codes, code, k = {}, 0, 0
    for length in range(1, 17):
        for _ in range(counts[length - 1]):
            codes[values[k]] = (code, length)
            code += 1
            k += 1
        code <<= 1
    return codes


DC_CODES = huffman_codes(DC_COUNTS, DC_VALUES)
AC_CODES = huffman_codes(AC_COUNTS, AC_VALUES)


class Bits:
    def __init__(self):
        self.out = bytearray()
        self.acc = 0
        self.n = 0

    def put(self, code: int, length: int):
        self.acc = self.acc << length | code
        self.n += length
        while self.n >= 8:
            byte = self.acc >> (self.n - 8) & 0xFF
            self.out.append(byte)
            if byte == 0xFF:
                self.out.append(0)
            self.n -= 8
        self.acc &= (1 << self.n) - 1

    def finish(self) -> bytes:
        if self.n:
            self.put((1 << (8 - self.n)) - 1, 8 - self.n)
        return bytes(self.out)


def ycc(r: int, g: int, b: int):
    y = 0.299 * r + 0.587 * g + 0.114 * b
    cb = 128 - 0.168736 * r - 0.331264 * g + 0.5 * b
    cr = 128 + 0.5 * r - 0.418688 * g - 0.081312 * b
    return [max(0, min(255, round(v))) for v in (y, cb, cr)]


def segment(marker: int, body: bytes) -> bytes:
    return struct.pack(">BBH", 0xFF, marker, len(body) + 2) + body


def jpeg(rows) -> bytes:
    bits = Bits()
    previous = [0, 0, 0]
    for by in range(0, H, 8):
        for bx in range(0, W, 8):
            levels = ycc(*rows[by][bx])
            for c in range(3):
                dc = levels[c] - 128
                diff, previous[c] = dc - previous[c], dc
                category = abs(diff).bit_length()
                code, length = DC_CODES[category]
                bits.put(code, length)
                if category:
                    bits.put(diff if diff > 0 else diff + (1 << category) - 1, category)
                code, length = AC_CODES[0]
                bits.put(code, length)
    out = b"\xff\xd8"
    out += segment(0xE0, b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00")
    out += segment(0xDB, bytes([0]) + bytes([QUANT]) * 64)
    out += segment(0xC0, struct.pack(">BHHB", 8, H, W, 3) + b"".join(bytes([i, 0x11, 0]) for i in (1, 2, 3)))
    out += segment(0xC4, bytes([0x00]) + bytes(DC_COUNTS) + bytes(DC_VALUES))
    out += segment(0xC4, bytes([0x10]) + bytes(AC_COUNTS) + bytes(AC_VALUES))
    out += segment(0xDA, bytes([3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]))
    return out + bits.finish() + b"\xff\xd9"


# The movie container.


def atom(kind: bytes, body: bytes = b"") -> bytes:
    return struct.pack(">I", len(body) + 8) + kind + body


def full(kind: bytes, body: bytes = b"", version=0, flags=0) -> bytes:
    return atom(kind, struct.pack(">I", version << 24 | flags) + body)


def table(entries, fmt) -> bytes:
    return struct.pack(">I", len(entries)) + b"".join(struct.pack(fmt, *e) if isinstance(e, tuple) else struct.pack(fmt, e) for e in entries)


def elst(edits) -> bytes:
    """edits: (duration in movie units, media time or -1)."""
    body = table([(d, t, 1, 0) for d, t in edits], ">IiHH")
    return atom(b"edts", full(b"elst", body))


MATRIX = struct.pack(">9I", 0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000)


def track(track_id, handler, media_scale, media_duration, movie_duration, edits, stsd, stbl_rest, media_header, width=0, height=0):
    tkhd = full(
        b"tkhd",
        struct.pack(">IIIII8xhhhH", 0, 0, track_id, 0, movie_duration, 0, 0, 0x100 if handler == b"soun" else 0, 0)
        + MATRIX + struct.pack(">II", width << 16, height << 16),
        flags=3,
    )
    mdhd = full(b"mdhd", struct.pack(">IIIIHH", 0, 0, media_scale, media_duration, 0x55C4, 0))
    hdlr = full(b"hdlr", b"\0\0\0\0" + handler + b"\0" * 12 + b"\0")
    dinf = atom(b"dinf", full(b"dref", struct.pack(">I", 1) + full(b"url ", b"", flags=1)))
    stbl = atom(b"stbl", full(b"stsd", struct.pack(">I", 1) + stsd) + stbl_rest)
    minf = atom(b"minf", media_header + dinf + stbl)
    return atom(b"trak", tkhd + (elst(edits) if edits else b"") + atom(b"mdia", mdhd + hdlr + minf))


def video_entry() -> bytes:
    body = b"\0" * 6 + struct.pack(">H", 1) + struct.pack(">HHIII", 0, 0, 0, 0, 0x200)
    body += struct.pack(">HHIIIH", W, H, 0x480000, 0x480000, 0, 1) + bytes([4]) + b"JPEG" + b"\0" * 27
    body += struct.pack(">Hh", 24, -1)
    return atom(b"jpeg", body)


def audio_entry() -> bytes:
    body = b"\0" * 6 + struct.pack(">H", 1) + struct.pack(">HHI", 0, 0, 0)
    body += struct.pack(">HHHHI", 1, 16, 0, 0, RATE << 16)
    return atom(b"sowt", body)


def sine(seconds: float):
    return [round(0.5 * 32767 * math.sin(2 * math.pi * 440 * n / RATE)) for n in range(round(seconds * RATE))]


def movie(path: Path, frames, video_edits, durations=None, offsets=None, audio=None, audio_edits=None):
    """Writes a movie. `frames`: JPEG bytes in decode order. `video_edits` and
    `audio_edits`: (duration in movie units, media time or -1)."""
    durations = durations or [FRAME] * len(frames)
    media_duration = sum(durations)
    mdat = b"".join(frames)
    pcm = b"" if audio is None else struct.pack(f"<{len(audio)}h", *audio)
    ftyp = atom(b"ftyp", b"qt  " + struct.pack(">I", 0x200) + b"qt  ")
    base = len(ftyp) + 8  # the mdat's body starts after its header
    chunk_offsets = [base]
    sample_offset = base + len(mdat)
    stbl = full(b"stts", table(_runs(durations), ">II"))
    if offsets is not None:
        stbl += full(b"ctts", table(_runs(offsets), ">II"), version=1)
    stbl += full(b"stsc", table([(1, len(frames), 1)], ">III"))
    stbl += full(b"stsz", struct.pack(">II", 0, len(frames)) + b"".join(struct.pack(">I", len(f)) for f in frames))
    stbl += full(b"stco", table(chunk_offsets, ">I"))
    movie_duration = sum(d for d, _ in video_edits)
    traks = track(1, b"vide", TIMESCALE, media_duration, movie_duration, video_edits, video_entry(), stbl,
                  full(b"vmhd", b"\0" * 8, flags=1), W, H)
    if audio is not None:
        astbl = full(b"stts", table([(len(audio), 1)], ">II"))
        astbl += full(b"stsc", table([(1, len(audio), 1)], ">III"))
        astbl += full(b"stsz", struct.pack(">II", 2, len(audio)))
        astbl += full(b"stco", table([sample_offset], ">I"))
        audio_total = sum(d for d, _ in audio_edits)
        traks += track(2, b"soun", RATE, len(audio), audio_total, audio_edits, audio_entry(), astbl,
                       full(b"smhd", b"\0" * 4))
        movie_duration = max(movie_duration, audio_total)
    mvhd = full(b"mvhd", struct.pack(">IIII", 0, 0, TIMESCALE, movie_duration)
                + struct.pack(">IhH8x", 0x10000, 0x100, 0) + MATRIX + b"\0" * 24 + struct.pack(">I", 3))
    path.write_bytes(ftyp + atom(b"mdat", mdat + pcm) + atom(b"moov", mvhd + traks))


def _runs(values):
    runs = []
    for v in values:
        if runs and runs[-1][1] == v:
            runs[-1] = (runs[-1][0] + 1, v)
        else:
            runs.append((1, v))
    return runs


def write_wav(path: Path, samples):
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(struct.pack(f"<{len(samples)}h", *samples))


def ffmpeg_movie(name: str, codec_args, count=24):
    raw = b"".join(bytes(c for px in row for c in px) for i in range(count) for row in picture(i))
    cmd = ["ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}",
           "-r", f"{TIMESCALE}/{FRAME}", "-i", "-",
           "-vf", "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p" if "prores" not in name else "scale=out_color_matrix=bt709:out_range=tv,format=yuv422p10le",
           "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv",
           *codec_args, "-an", "-f", "mov", str(OUT / name)]
    subprocess.run(cmd, input=raw, check=True)
    # Recorded with the output's name alone, so no local path is kept.
    return " ".join(cmd[:-1] + [name])


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    manifest = {"width": W, "height": H, "time_scale": TIMESCALE, "frame_duration": FRAME, "sample_rate": RATE, "files": {}}

    jpegs = [jpeg(picture(i)) for i in range(24)]
    audio = sine(1.0)
    write_wav(OUT / "mjpeg-24f.wav", audio)
    movie(OUT / "mjpeg-24f.mov", jpegs, [(24 * FRAME, 0)], audio=audio, audio_edits=[(TIMESCALE, 0)])
    manifest["files"]["mjpeg-24f.mov"] = {"frames": 24, "audio": "mjpeg-24f.wav"}

    movie(OUT / "mjpeg-gaps.mov", jpegs[:16],
          [(8 * FRAME, 0), (4 * FRAME, -1), (8 * FRAME, 8 * FRAME)],
          audio=audio[: round(0.6 * RATE)],
          audio_edits=[(3000, -1), (6000, 0), (3000, -1), (9000, round(0.2 * RATE))])
    manifest["files"]["mjpeg-gaps.mov"] = {"frames": 16, "audio": "mjpeg-24f.wav"}

    # Decode order 0 2 1 4 3 6 5 ...: the sample at decode position k shows
    # at display position order[k]. With one frame of offset added to keep
    # every composition offset above zero, and an edit list taking it away.
    order = [0] + [p for pair in range(1, 11, 2) for p in (pair + 1, pair)] + [11]
    frames = [jpeg(picture(p)) for p in order]
    offsets = [(p - k + 1) * FRAME for k, p in enumerate(order)]
    movie(OUT / "mjpeg-reordered.mov", frames, [(12 * FRAME, FRAME)], offsets=offsets)
    manifest["files"]["mjpeg-reordered.mov"] = {"frames": 12, "decode_order": order}

    if shutil.which("ffmpeg"):
        version = subprocess.run(["ffmpeg", "-version"], capture_output=True, text=True).stdout.splitlines()[0]
        manifest["ffmpeg"] = version
        for name, args, key in [
            ("h264.mov", ["-c:v", "libx264", "-preset", "veryslow", "-crf", "6", "-bf", "2", "-g", "12", "-tag:v", "avc1"], "h264"),
            ("hevc.mov", ["-c:v", "libx265", "-preset", "veryslow", "-crf", "6", "-g", "12", "-tag:v", "hvc1", "-x265-params", "log-level=error"], "hevc"),
            ("prores.mov", ["-c:v", "prores_ks", "-profile:v", "3", "-vendor", "apl0"], "prores"),
        ]:
            manifest["files"][name] = {"frames": 24, "command": ffmpeg_movie(name, args)}
    (OUT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
