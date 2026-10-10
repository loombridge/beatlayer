#!/usr/bin/env python3
"""Generates the media decode fixtures in tests/fixtures/media/.

Every fixture is synthesised here: no third-party or recorded audio. The
script writes the source signals as WAV files itself, byte by byte, and then
encodes them into every format Beatlayer imports with the external encoders
named below. It records each file, its source, the encoder, the exact command,
and the encoder's version in tests/fixtures/media/manifest.json, which the
decode tests in crates/media/tests/ read.

Run it from anywhere with Python 3.10 or later. The Vorbis encoder comes
from the `soundfile` package, so run it in a virtual environment that has it:

    python3 -m venv /tmp/gen-media
    /tmp/gen-media/bin/pip install soundfile==0.14.0
    /tmp/gen-media/bin/python tests/fixtures/gen/gen_media.py

The encoders it calls:

- ffmpeg: FLAC, and the RF64 file
- lame: MP3
- libvorbis, through libsndfile in the `soundfile` package: Ogg Vorbis
- afconvert, which ships with macOS: AIFF, AIFF-C, AAC, ALAC, and RIFF with
  a reserved ds64 JUNK chunk

So the full set regenerates only on macOS. The committed fixtures are the
reference; regenerating may change the encoded bytes if an encoder's version
changes, which is why the manifest records the versions.
"""

from __future__ import annotations

import json
import math
import platform
import shutil
import struct
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "media"

# Amplitude of the sine and sweep sources: -6.02 dBFS.
LEVEL = 0.5
# Raised-cosine fade at each end of a sine or sweep, in seconds, so lossy
# encoders don't smear a hard edge across the file.
FADE = 0.005


# Source signals. Each returns the sample at frame `n` of channel `c`, as a
# float in [-1, 1].


def sine(rate: int, frames: int, channels: int):
    # Channel c plays (c + 1) kHz, so a channel swap or a wrong sample rate is
    # visible in the decoded spectrum.
    def sample(n: int, c: int) -> float:
        return LEVEL * fade(n, rate, frames) * math.sin(2 * math.pi * 1000 * (c + 1) * n / rate)

    return sample


def sweep(rate: int, frames: int, channels: int):
    # Exponential sweep from 20 Hz to 20 kHz over the whole file.
    f0, f1 = 20.0, 20000.0
    seconds = frames / rate
    k = math.log(f1 / f0)

    def sample(n: int, c: int) -> float:
        t = n / rate
        phase = 2 * math.pi * f0 * seconds / k * (math.exp(t / seconds * k) - 1)
        return LEVEL * fade(n, rate, frames) * math.sin(phase)

    return sample


def noise(rate: int, frames: int, channels: int):
    # Uniform white noise in [-0.5, 0.5) from xorshift32, seeded per channel,
    # precomputed so the sequence doesn't depend on call order.
    table = []
    for c in range(channels):
        state = 0x9E3779B9 ^ (c * 0x85EBCA6B) & 0xFFFFFFFF
        column = []
        for _ in range(frames):
            state ^= (state << 13) & 0xFFFFFFFF
            state ^= state >> 17
            state ^= (state << 5) & 0xFFFFFFFF
            column.append(state / 2**32 - 0.5)
        table.append(column)
    return lambda n, c: table[c][n]


def silence(rate: int, frames: int, channels: int):
    return lambda n, c: 0.0


def clip(rate: int, frames: int, channels: int):
    # Clipping edge cases, exact at 16 bits:
    # - the first half is a 220 Hz sine driven to four times full scale and
    #   hard-clipped, so long runs sit at the largest and smallest codes;
    # - the next quarter alternates the largest and smallest codes, a
    #   full-scale square at the Nyquist frequency;
    # - the last quarter steps through 64-sample runs of the largest, smallest,
    #   zero, and one-code values.
    top = 32767 / 32768
    steps = [top, -1.0, 0.0, 1 / 32768, -1 / 32768]

    def sample(n: int, c: int) -> float:
        if n < frames // 2:
            x = 4.0 * math.sin(2 * math.pi * 220 * n / rate + c * math.pi / 2)
            return max(-1.0, min(top, x))
        if n < frames * 3 // 4:
            return top if n % 2 == 0 else -1.0
        return steps[(n // 64) % len(steps)] * (1 if c == 0 else -1)

    return sample


def fade(n: int, rate: int, frames: int) -> float:
    length = int(FADE * rate)
    edge = min(n, frames - 1 - n)
    if edge >= length:
        return 1.0
    return 0.5 - 0.5 * math.cos(math.pi * edge / length)


SIGNALS = {"sine": sine, "sweep": sweep, "noise": noise, "silence": silence, "clip": clip}

# The source WAV files this script writes itself.
#   layout: "pcm" (WAVE_FORMAT_PCM or IEEE float), "extensible"
#   (WAVE_FORMAT_EXTENSIBLE), or "bwf" (a bext chunk before fmt).
SOURCES = [
    dict(name="sine-44k1-s16-2ch", signal="sine", rate=44100, channels=2, fmt="s16", seconds=0.25),
    dict(name="sine-48k-s24-2ch", signal="sine", rate=48000, channels=2, fmt="s24", seconds=0.25),
    dict(name="sine-96k-s24-2ch", signal="sine", rate=96000, channels=2, fmt="s24", seconds=0.125),
    dict(name="sine-44k1-f32-2ch", signal="sine", rate=44100, channels=2, fmt="f32", seconds=0.125),
    dict(name="sine-22k05-u8-1ch", signal="sine", rate=22050, channels=1, fmt="u8", seconds=0.25),
    dict(name="sine-48k-s24-4ch", signal="sine", rate=48000, channels=4, fmt="s24", seconds=0.125, layout="extensible"),
    dict(name="sine-48k-s24-2ch-bwf", signal="sine", rate=48000, channels=2, fmt="s24", seconds=0.125, layout="bwf"),
    dict(name="sweep-48k-s24-1ch", signal="sweep", rate=48000, channels=1, fmt="s24", seconds=0.5),
    dict(name="sweep-44k1-s16-2ch", signal="sweep", rate=44100, channels=2, fmt="s16", seconds=0.5),
    dict(name="noise-44k1-s16-2ch", signal="noise", rate=44100, channels=2, fmt="s16", seconds=0.25),
    dict(name="silence-44k1-s16-2ch", signal="silence", rate=44100, channels=2, fmt="s16", seconds=0.125),
    dict(name="clip-44k1-s16-2ch", signal="clip", rate=44100, channels=2, fmt="s16", seconds=0.125),
]

# Encodings of the sources. `tool` picks the command; `args` are its
# format arguments.
ENCODINGS = [
    # AIFF and AIFF-C, big-endian PCM and float.
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.aiff", tool="afconvert", args=["-f", "AIFF", "-d", "BEI16"], container="aiff", codec="pcm"),
    dict(source="sweep-48k-s24-1ch", out="sweep-48k-s24-1ch.aiff", tool="afconvert", args=["-f", "AIFF", "-d", "BEI24"], container="aiff", codec="pcm"),
    dict(source="clip-44k1-s16-2ch", out="clip-44k1-s16-2ch.aiff", tool="afconvert", args=["-f", "AIFF", "-d", "BEI16"], container="aiff", codec="pcm"),
    dict(source="sine-48k-s24-2ch", out="sine-48k-s24-2ch.aifc", tool="afconvert", args=["-f", "AIFC", "-d", "BEI24"], container="aiff", codec="pcm"),
    dict(source="sine-44k1-f32-2ch", out="sine-44k1-f32-2ch.aifc", tool="afconvert", args=["-f", "AIFC", "-d", "BEF32"], container="aiff", codec="pcm"),
    # RF64 and BW64, the 64-bit WAV variants. ffmpeg writes a real RF64
    # header on request. afconvert's RF64 mode keeps a short file as RIFF and
    # reserves the ds64 space as a JUNK chunk, as EBU Tech 3306 suggests, so
    # its output tests chunk skipping instead. Nothing here writes BW64 for a
    # short file, so this script does.
    dict(source="sine-48k-s24-2ch", out="sine-48k-s24-2ch.rf64.wav", tool="ffmpeg", args=["-c:a", "pcm_s24le", "-rf64", "always"], container="rf64", codec="pcm"),
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.bw64.wav", tool="bw64", args=[], container="rf64", codec="pcm"),
    dict(source="sine-48k-s24-2ch", out="sine-48k-s24-2ch.junk.wav", tool="afconvert", args=["-f", "RF64", "-d", "LEI24"], container="wav", codec="pcm"),
    # FLAC.
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.flac", tool="ffmpeg", args=["-c:a", "flac", "-compression_level", "8"], container="flac", codec="flac"),
    dict(source="sweep-48k-s24-1ch", out="sweep-48k-s24-1ch.flac", tool="ffmpeg", args=["-c:a", "flac", "-compression_level", "8"], container="flac", codec="flac"),
    dict(source="sine-96k-s24-2ch", out="sine-96k-s24-2ch.flac", tool="ffmpeg", args=["-c:a", "flac", "-compression_level", "8"], container="flac", codec="flac"),
    dict(source="sine-48k-s24-4ch", out="sine-48k-s24-4ch.flac", tool="ffmpeg", args=["-c:a", "flac", "-compression_level", "8"], container="flac", codec="flac"),
    dict(source="noise-44k1-s16-2ch", out="noise-44k1-s16-2ch.flac", tool="ffmpeg", args=["-c:a", "flac", "-compression_level", "8"], container="flac", codec="flac"),
    dict(source="silence-44k1-s16-2ch", out="silence-44k1-s16-2ch.flac", tool="ffmpeg", args=["-c:a", "flac", "-compression_level", "8"], container="flac", codec="flac"),
    dict(source="clip-44k1-s16-2ch", out="clip-44k1-s16-2ch.flac", tool="ffmpeg", args=["-c:a", "flac", "-compression_level", "8"], container="flac", codec="flac"),
    # ALAC in M4A.
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.alac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "alac"], container="mp4", codec="alac"),
    dict(source="sweep-48k-s24-1ch", out="sweep-48k-s24-1ch.alac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "alac"], container="mp4", codec="alac"),
    dict(source="clip-44k1-s16-2ch", out="clip-44k1-s16-2ch.alac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "alac"], container="mp4", codec="alac"),
    # MP3, 320 kbit/s CBR, with LAME's gapless (encoder delay and padding) tag.
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.mp3", tool="lame", args=["-b", "320", "--cbr"], container="mp3", codec="mp3"),
    dict(source="sweep-44k1-s16-2ch", out="sweep-44k1-s16-2ch.mp3", tool="lame", args=["-b", "320", "--cbr"], container="mp3", codec="mp3"),
    dict(source="sine-48k-s24-2ch", out="sine-48k-s24-2ch.vbr.mp3", tool="lame", args=["-V", "0"], container="mp3", codec="mp3"),
    dict(source="noise-44k1-s16-2ch", out="noise-44k1-s16-2ch.mp3", tool="lame", args=["-b", "320", "--cbr"], container="mp3", codec="mp3"),
    dict(source="silence-44k1-s16-2ch", out="silence-44k1-s16-2ch.mp3", tool="lame", args=["-b", "320", "--cbr"], container="mp3", codec="mp3"),
    dict(source="clip-44k1-s16-2ch", out="clip-44k1-s16-2ch.mp3", tool="lame", args=["-b", "320", "--cbr"], container="mp3", codec="mp3"),
    # Ogg Vorbis.
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.ogg", tool="soundfile", args=[], container="ogg", codec="vorbis"),
    dict(source="sweep-44k1-s16-2ch", out="sweep-44k1-s16-2ch.ogg", tool="soundfile", args=[], container="ogg", codec="vorbis"),
    dict(source="noise-44k1-s16-2ch", out="noise-44k1-s16-2ch.ogg", tool="soundfile", args=[], container="ogg", codec="vorbis"),
    dict(source="silence-44k1-s16-2ch", out="silence-44k1-s16-2ch.ogg", tool="soundfile", args=[], container="ogg", codec="vorbis"),
    dict(source="clip-44k1-s16-2ch", out="clip-44k1-s16-2ch.ogg", tool="soundfile", args=[], container="ogg", codec="vorbis"),
    # AAC-LC in M4A, and one raw ADTS stream.
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.aac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "aac", "-b", "256000"], container="mp4", codec="aac"),
    dict(source="sweep-44k1-s16-2ch", out="sweep-44k1-s16-2ch.aac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "aac", "-b", "256000"], container="mp4", codec="aac"),
    dict(source="sine-48k-s24-2ch", out="sine-48k-s24-2ch.aac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "aac", "-b", "256000"], container="mp4", codec="aac"),
    dict(source="noise-44k1-s16-2ch", out="noise-44k1-s16-2ch.aac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "aac", "-b", "256000"], container="mp4", codec="aac"),
    dict(source="silence-44k1-s16-2ch", out="silence-44k1-s16-2ch.aac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "aac", "-b", "256000"], container="mp4", codec="aac"),
    dict(source="clip-44k1-s16-2ch", out="clip-44k1-s16-2ch.aac.m4a", tool="afconvert", args=["-f", "m4af", "-d", "aac", "-b", "256000"], container="mp4", codec="aac"),
    dict(source="sine-44k1-s16-2ch", out="sine-44k1-s16-2ch.aac", tool="afconvert", args=["-f", "adts", "-d", "aac", "-b", "256000"], container="adts", codec="aac"),
]


def quantise(x: float, fmt: str) -> bytes:
    if fmt == "s16":
        return struct.pack("<h", max(-32768, min(32767, round(x * 32768))))
    if fmt == "s24":
        v = max(-(1 << 23), min((1 << 23) - 1, round(x * (1 << 23))))
        return struct.pack("<i", v)[:3]
    if fmt == "u8":
        return struct.pack("<B", max(0, min(255, round(x * 128) + 128)))
    if fmt == "f32":
        return struct.pack("<f", x)
    raise ValueError(fmt)


def chunk(fourcc: bytes, body: bytes) -> bytes:
    pad = b"\0" if len(body) % 2 else b""
    return fourcc + struct.pack("<I", len(body)) + body + pad


def write_wav(source: dict) -> Path:
    rate, channels, fmt = source["rate"], source["channels"], source["fmt"]
    frames = round(source["seconds"] * rate)
    signal = SIGNALS[source["signal"]](rate, frames, channels)
    bits = {"s16": 16, "s24": 24, "u8": 8, "f32": 32}[fmt]
    block_align = channels * bits // 8
    layout = source.get("layout", "pcm")

    data = bytearray()
    for n in range(frames):
        for c in range(channels):
            data += quantise(signal(n, c), fmt)

    tag = 3 if fmt == "f32" else 1
    common = struct.pack("<IIHH", rate, rate * block_align, block_align, bits)
    if layout == "extensible":
        # WAVE_FORMAT_EXTENSIBLE, quad speaker mask, KSDATAFORMAT_SUBTYPE_PCM.
        guid = struct.pack("<IHH", tag, 0x0000, 0x0010) + bytes.fromhex("800000aa00389b71")
        fmt_body = struct.pack("<HH", 0xFFFE, channels) + common + struct.pack("<HHI", 22, bits, 0x33) + guid
    elif tag == 3:
        fmt_body = struct.pack("<HH", tag, channels) + common + struct.pack("<H", 0)
    else:
        fmt_body = struct.pack("<HH", tag, channels) + common

    chunks = b""
    if layout == "bwf":
        # A version 2 Broadcast Wave bext chunk, before fmt as BWF writers do.
        bext = (
            b"Beatlayer media fixture".ljust(256, b"\0")
            + b"gen_media.py".ljust(32, b"\0")
            + b"beatlayer".ljust(32, b"\0")
            + b"2026-10-10"
            + b"12:00:00"
            + struct.pack("<Q", 48000 * 3600)
            + struct.pack("<H", 2)
            + bytes(64)
            + struct.pack("<hhhhh", 0x7FFF, 0x7FFF, 0x7FFF, 0x7FFF, 0x7FFF)
            + bytes(180)
        )
        chunks += chunk(b"bext", bext)
    chunks += chunk(b"fmt ", fmt_body)
    if tag == 3 or layout == "extensible":
        chunks += chunk(b"fact", struct.pack("<I", frames))
    chunks += chunk(b"data", bytes(data))

    path = OUT / f"{source['name']}.wav"
    path.write_bytes(b"RIFF" + struct.pack("<I", 4 + len(chunks)) + b"WAVE" + chunks)
    return path


def write_bw64(src: Path, dst: Path) -> None:
    """Rewrites a RIFF WAV file as BW64 (EBU Tech 3306): the 32-bit sizes
    become 0xFFFFFFFF and the real sizes move to a ds64 chunk."""
    riff = src.read_bytes()
    pos, fmt_chunk, data = 12, None, None
    while pos + 8 <= len(riff):
        fourcc, size = riff[pos:pos + 4], struct.unpack("<I", riff[pos + 4:pos + 8])[0]
        body = riff[pos + 8:pos + 8 + size]
        if fourcc == b"fmt ":
            fmt_chunk = chunk(b"fmt ", body)
        elif fourcc == b"data":
            data = body
        pos += 8 + size + size % 2
    block_align = struct.unpack("<H", fmt_chunk[20:22])[0]
    ds64_len = 8 + 28
    riff_size = 4 + ds64_len + len(fmt_chunk) + 8 + len(data) + len(data) % 2
    ds64 = chunk(b"ds64", struct.pack("<QQQI", riff_size, len(data), len(data) // block_align, 0))
    pad = b"\0" if len(data) % 2 else b""
    body = b"WAVE" + ds64 + fmt_chunk + b"data" + struct.pack("<I", 0xFFFFFFFF) + data + pad
    dst.write_bytes(b"BW64" + struct.pack("<I", 0xFFFFFFFF) + body)


def run(cmd: list[str]) -> str:
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def tool_versions() -> dict[str, str]:
    versions = {}
    if shutil.which("ffmpeg"):
        versions["ffmpeg"] = run(["ffmpeg", "-version"]).splitlines()[0].split(" Copyright")[0]
    if shutil.which("lame"):
        versions["lame"] = run(["lame", "--version"]).splitlines()[0]
    if shutil.which("afconvert"):
        versions["afconvert"] = f"afconvert from macOS {platform.mac_ver()[0]}"
    try:
        import soundfile

        versions["soundfile"] = (
            f"libvorbis through libsndfile {soundfile.__libsndfile_version__} "
            f"(python-soundfile {soundfile.__version__})"
        )
    except ImportError:
        pass
    return versions


def write_vorbis(src: Path, dst: Path) -> None:
    """Encodes Ogg Vorbis with libvorbis, through libsndfile."""
    import soundfile

    data, rate = soundfile.read(src, dtype="float32", always_2d=True)
    soundfile.write(dst, data, rate, format="OGG", subtype="VORBIS", compression_level=0.1)


def encode_command(encoding: dict, src: Path, dst: Path) -> list[str]:
    tool, args = encoding["tool"], encoding["args"]
    if tool == "afconvert":
        return ["afconvert", *args, str(src), str(dst)]
    if tool == "ffmpeg":
        return [
            "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
            "-i", str(src), "-map_metadata", "-1", "-fflags", "+bitexact",
            "-flags:a", "+bitexact", *args, str(dst),
        ]
    if tool == "lame":
        return ["lame", "--quiet", "--noreplaygain", *args, str(src), str(dst)]
    raise ValueError(tool)


def main() -> int:
    missing = sorted({
        e["tool"] for e in ENCODINGS
        if e["tool"] not in ("bw64", "soundfile") and not shutil.which(e["tool"])
    })
    try:
        import soundfile  # noqa: F401
    except ImportError:
        missing.append("soundfile (pip install soundfile==0.14.0)")
    if missing:
        print(f"missing encoders: {', '.join(missing)}", file=sys.stderr)
        return 1

    OUT.mkdir(parents=True, exist_ok=True)
    versions = tool_versions()
    by_name = {s["name"]: s for s in SOURCES}
    fixtures = []

    for source in SOURCES:
        path = write_wav(source)
        frames = round(source["seconds"] * source["rate"])
        fixtures.append(dict(
            file=path.name, source=path.name, signal=source["signal"],
            container="wav", codec="pcm", lossless=True,
            rate=source["rate"], channels=source["channels"], frames=frames,
            sample_format=source["fmt"], layout=source.get("layout", "pcm"),
            encoder="gen_media.py", command="written by gen_media.py",
        ))

    for encoding in ENCODINGS:
        source = by_name[encoding["source"]]
        src = OUT / f"{source['name']}.wav"
        dst = OUT / encoding["out"]
        if encoding["tool"] == "bw64":
            write_bw64(src, dst)
            encoder, command = "gen_media.py", "write_bw64(SOURCE, OUTPUT)"
        elif encoding["tool"] == "soundfile":
            write_vorbis(src, dst)
            encoder = versions["soundfile"]
            command = 'soundfile.write(OUTPUT, data, rate, format="OGG", subtype="VORBIS", compression_level=0.1)'
        else:
            cmd = encode_command(encoding, src, dst)
            run(cmd)
            encoder = versions[encoding["tool"]]
            command = " ".join([cmd[0], *[a for a in cmd[1:] if a not in (str(src), str(dst))], "SOURCE", "OUTPUT"])
        fixtures.append(dict(
            file=dst.name, source=src.name, signal=source["signal"],
            container=encoding["container"], codec=encoding["codec"],
            lossless=encoding["codec"] in ("pcm", "flac", "alac"),
            rate=source["rate"], channels=source["channels"],
            frames=round(source["seconds"] * source["rate"]),
            encoder=encoder, command=command,
        ))

    manifest = dict(
        generator="tests/fixtures/gen/gen_media.py",
        note="Generated; don't edit by hand. Rerun the generator instead.",
        encoders=versions,
        fixtures=fixtures,
    )
    (OUT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    total = sum((OUT / f["file"]).stat().st_size for f in fixtures)
    print(f"wrote {len(fixtures)} fixtures, {total / 1024:.0f} KiB, to {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
