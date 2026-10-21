#!/usr/bin/env python3
"""Generates the DAWproject fixtures in tests/fixtures/dawproject/.

Every fixture is written here: the container is a ZIP the script builds with
the standard library's `zipfile`, the audio is a sine the script writes with
`wave`, and no file another application exported is used. Each fixture's
expected import report is written beside it as JSON, and the import tests in
crates/interop/tests/dawproject.rs read them.

Run it from anywhere with Python 3.10 or later:

    python3 tests/fixtures/gen/gen_dawproject.py
"""

from __future__ import annotations

import io
import json
import math
import struct
import wave
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "dawproject"

# The sample rate and length of the audio the fixtures hold.
RATE = 44100
FRAMES = RATE  # One second.

# The application the fixtures say made them.
APPLICATION = "Fixture DAW"
VERSION = "1.0"


def wav_bytes(frequency: float = 440.0, rate: int = RATE, frames: int = FRAMES) -> bytes:
    """A sine of `frequency` Hz, as a 16-bit stereo WAV."""
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as writer:
        writer.setnchannels(2)
        writer.setsampwidth(2)
        writer.setframerate(rate)
        data = bytearray()
        for n in range(frames):
            value = int(0.5 * 32767 * math.sin(2 * math.pi * frequency * n / rate))
            data += struct.pack("<hh", value, value)
        writer.writeframes(bytes(data))
    return buffer.getvalue()


def project(body: str, version: str = "1.0") -> str:
    """The whole `project.xml`, with the header and the application."""
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        f'<Project version="{version}">\n'
        f'  <Application name="{APPLICATION}" version="{VERSION}"/>\n'
        f"{body}"
        "</Project>\n"
    )


def channel(
    ident: str,
    role: str = "regular",
    destination: str | None = None,
    volume: str = ' unit="linear" min="0" max="2" value="1"',
    pan: str = ' unit="normalized" min="0" max="1" value="0.5"',
    mute: str = "false",
    solo: str = "false",
    sends: str = "",
    devices: str = "",
) -> str:
    """A channel element."""
    destination_attribute = f' destination="{destination}"' if destination else ""
    return (
        f'      <Channel audioChannels="2"{destination_attribute} role="{role}" solo="{solo}" '
        f'id="{ident}">\n'
        f"{devices}"
        f'        <Mute value="{mute}" id="{ident}m" name="Mute"/>\n'
        f'        <Pan{pan} id="{ident}p" name="Pan"/>\n'
        f"{sends}"
        f'        <Volume{volume} id="{ident}v" name="Volume"/>\n'
        "      </Channel>\n"
    )


def track(
    ident: str,
    name: str,
    content_type: str,
    body: str = "",
    loaded: str = "true",
    children: str = "",
    color: str = "",
) -> str:
    """A track element."""
    color_attribute = f' color="{color}"' if color else ""
    return (
        f'    <Track contentType="{content_type}" loaded="{loaded}" id="{ident}" '
        f'name="{name}"{color_attribute}>\n'
        f"{body}"
        f"{children}"
        "    </Track>\n"
    )


def notes(ident: str, notes_body: str) -> str:
    """A notes timeline."""
    return f'            <Notes id="{ident}">\n{notes_body}            </Notes>\n'


def note(time: str, duration: str, key: int, channel_number: int = 0, vel: str = "0.8") -> str:
    """A note."""
    return (
        f'              <Note time="{time}" duration="{duration}" channel="{channel_number}" '
        f'key="{key}" vel="{vel}" rel="0.5"/>\n'
    )


_ids = {"n": 0}


def add(archive: zipfile.ZipFile, name: str, data: bytes | str, compression: int | None = None) -> None:
    """Adds an entry with a fixed date, so a rerun writes the same bytes."""
    entry_info = zipfile.ZipInfo(name, date_time=(2026, 10, 21, 0, 0, 0))
    entry_info.compress_type = archive.compression if compression is None else compression
    archive.writestr(entry_info, data)


def uid(prefix: str = "x") -> str:
    """An `id` no other element of the file has."""
    _ids["n"] += 1
    return f"{prefix}{_ids['n']}"


def audio_clip(
    time: str,
    duration: str,
    name: str,
    path: str,
    attributes: str = "",
    sample_rate: int = RATE,
    external: bool = False,
    ident: str | None = None,
) -> str:
    """An Arrangement clip playing a file as it is, in seconds."""
    external_attribute = ' external="true"' if external else ""
    return (
        f'          <Clip time="{time}" duration="{duration}" contentTimeUnit="seconds" '
        f'name="{name}"{attributes}>\n'
        f'            <Audio channels="2" duration="1" sampleRate="{sample_rate}" '
        f'id="{ident or uid("audio")}">\n'
        f'              <File path="{path}"{external_attribute}/>\n'
        "            </Audio>\n"
        "          </Clip>\n"
    )


def warped_event(time: str, duration: str, end_beat: str, end_second: str, path: str) -> str:
    """An audio event of a nested clip: a file warped so that `end_beat` of
    its contents are `end_second` of the file."""
    return (
        f'              <Clip time="{time}" duration="{duration}" contentTimeUnit="beats">\n'
        f'                <Warps contentTimeUnit="seconds" timeUnit="beats" id="{uid("warps")}">\n'
        '                  <Audio algorithm="stretch" channels="2" duration="1" '
        f'sampleRate="44100" id="{uid("audio")}">\n'
        f'                    <File path="{path}"/>\n'
        "                  </Audio>\n"
        '                  <Warp time="0" contentTime="0"/>\n'
        f'                  <Warp time="{end_beat}" contentTime="{end_second}"/>\n'
        "                </Warps>\n"
        "              </Clip>\n"
    )


def lane(track_id: str, content: str, attributes: str = "") -> str:
    """The lanes of one track."""
    return f'      <Lanes track="{track_id}"{attributes} id="{uid("lane")}">\n{content}      </Lanes>\n'


def clips_timeline(content: str, name: str | None = None) -> str:
    """A `Clips` timeline."""
    name_attribute = f' name="{name}"' if name else ""
    return f'        <Clips id="{uid("clips")}"{name_attribute}>\n{content}        </Clips>\n'


def send(destination: str, kind: str, volume: str, enabled: str = "true") -> str:
    """A send."""
    ident = uid("send")
    return (
        f'          <Send destination="{destination}" type="{kind}" id="{ident}">\n'
        f'            <Enable value="{enabled}" id="{ident}e" name="On/Off"/>\n'
        '            <Pan unit="normalized" min="0" max="1" value="0.5" '
        f'id="{ident}p" name="Pan"/>\n'
        f'            <Volume unit="linear" min="0" max="2" value="{volume}" '
        f'id="{ident}v" name="Volume"/>\n'
        "          </Send>\n"
    )


def sends(*each: str) -> str:
    """A channel's sends."""
    return "        <Sends>\n" + "".join(each) + "        </Sends>\n"


def devices(*each: str) -> str:
    """A channel's devices."""
    return "        <Devices>\n" + "".join(each) + "        </Devices>\n"


def plugin(element: str, role: str, name: str, state: str) -> str:
    """A plug-in with its saved state in the container."""
    ident = uid("plug")
    return (
        f'          <{element} deviceID="{ident}" deviceName="{name}" deviceRole="{role}" '
        f'loaded="true" id="{ident}x" name="{name}">\n'
        "            <Parameters/>\n"
        f'            <Enabled value="true" id="{ident}e" name="On/Off"/>\n'
        f'            <State path="{state}"/>\n'
        f"          </{element}>\n"
    )


def generic(element: str, name: str) -> str:
    """One of the four generic devices."""
    return f'          <{element} deviceRole="audioFX" loaded="true" id="{uid("gen")}" name="{name}"/>\n'


def arrangement(lanes: str) -> str:
    """The Arrangement around some lanes."""
    return (
        '  <Arrangement id="arr">\n'
        f'    <Lanes timeUnit="beats" id="{uid("lanes")}">\n'
        f"{lanes}"
        "    </Lanes>\n"
        "  </Arrangement>\n"
    )



def write(name: str, xml: str, report: dict, files: dict[str, bytes] | None = None) -> None:
    """The fixture `name`: its container and the report it should give."""
    OUT.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(OUT / f"{name}.dawproject", "w", zipfile.ZIP_DEFLATED) as archive:
        add(archive, "project.xml", xml)
        for path, data in (files or {}).items():
            add(archive, path, data)
    (OUT / f"{name}.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def entry(what: str, place: str, count: int, did: str) -> dict:
    """One line of an import report."""
    return {"what": what, "where": place, "count": count, "did": did}


def report(entries: list[dict]) -> dict:
    """An import report."""
    return {"application": f"{APPLICATION} {VERSION}", "entries": entries}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    minimal()
    tempo()
    clips()
    media()
    session()
    bitwig_shaped()
    studio_one_shaped()
    cubase_shaped()
    damaged()
    print(f"Wrote the DAWproject fixtures to {OUT}")


def transport(tempo: str = "120") -> str:
    """The Transport element."""
    return (
        "  <Transport>\n"
        f'    <Tempo unit="bpm" value="{tempo}" id="tempo" name="Tempo"/>\n'
        '    <TimeSignature numerator="4" denominator="4" id="sig"/>\n'
        "  </Transport>\n"
    )


def minimal() -> None:
    """The smallest file that imports: one MIDI track and a master."""
    body = (
        transport()
        + "  <Structure>\n"
        + track("t1", "Bass", "notes", channel("c1", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Arrangement id=\"arr\">\n"
        '    <Lanes timeUnit="beats" id="lanes">\n'
        '      <Lanes track="t1" id="lane1">\n'
        '        <Clips id="clips1">\n'
        '          <Clip time="0" duration="4" name="Bass">\n'
        + notes(
            "notes1",
            note("0", "1", 36) + note("1", "1", 36),
        )
        + "          </Clip>\n"
        "        </Clips>\n"
        "      </Lanes>\n"
        "    </Lanes>\n"
        "  </Arrangement>\n"
        "  <Scenes/>\n"
    )
    write("minimal", project(body), report([]))


def tempo() -> None:
    """Tempos and signatures: a ramp, a point in seconds, a clamp and a
    signature Beatlayer can't hold."""
    body = (
        transport("140")
        + "  <Structure>\n"
        + track("t1", "Bass", "notes", channel("c1", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Arrangement id=\"arr\">\n"
        '    <TempoAutomation unit="bpm" id="ta">\n'
        '      <Target parameter="tempo"/>\n'
        '      <RealPoint time="0" value="140" interpolation="linear"/>\n'
        '      <RealPoint time="4" value="160" interpolation="hold"/>\n'
        '      <RealPoint time="8" value="160" interpolation="hold"/>\n'
        '      <RealPoint time="12" value="2000" interpolation="hold"/>\n'
        "    </TempoAutomation>\n"
        '    <TimeSignatureAutomation id="tsa">\n'
        '      <Target parameter="sig"/>\n'
        '      <TimeSignaturePoint time="0" numerator="7" denominator="8"/>\n'
        '      <TimeSignaturePoint time="4" numerator="3" denominator="32"/>\n'
        "    </TimeSignatureAutomation>\n"
        '    <Lanes timeUnit="beats" id="lanes">\n'
        '      <Lanes track="t1" id="lane1">\n'
        '        <Points unit="bpm" timeUnit="seconds" id="points1">\n'
        '          <Target parameter="tempo"/>\n'
        '          <RealPoint time="10" value="150" interpolation="hold"/>\n'
        "        </Points>\n"
        "      </Lanes>\n"
        "    </Lanes>\n"
        "  </Arrangement>\n"
        "  <Scenes/>\n"
    )
    write(
        "tempo",
        project(body),
        report(
            [
                entry("a tempo point", "in the song's tempo", 1, "changed to 1000 BPM"),
                entry(
                    "a time signature Beatlayer can't hold",
                    "in the song's time signature",
                    1,
                    "left out",
                ),
            ]
        ),
    )


def clips() -> None:
    """Clips: loops, play regions, a disabled clip, an overlap, notes out of
    range and on three channels, an unwarped loop, a fade, a take lane, a
    timeline outside any clip and a lane in seconds."""
    lead = (
        '          <Clip time="0" duration="4" loopStart="0" loopEnd="2" name="Loop">\n'
        + notes(
            "notes1",
            note("0", "1", 60)
            + note("1", "1", 62, 1)
            + note("2", "1", 64, 2)
            + note("3", "1", 200)
            + note("-1", "1", 65),
        )
        + "          </Clip>\n"
        '          <Clip time="2" duration="4" name="Over">\n'
        + notes("notes2", note("0", "2", 67))
        + "          </Clip>\n"
        '          <Clip time="8" duration="2" enable="false" name="Off">\n'
        + notes("notes3", note("0", "1", 48))
        + "          </Clip>\n"
        '          <Clip time="12" playStart="1" playStop="3" name="Window">\n'
        + notes("notes4", note("0", "1", 50) + note("1", "1", 52) + note("2", "1", 53))
        + "          </Clip>\n"
    )
    pad = clips_timeline(
        audio_clip("0", "4", "Looped", "audio/loop.wav", ' loopStart="0" loopEnd="0.5"')
        + audio_clip(
            "4", "2", "Faded", "audio/loop.wav", ' fadeInTime="0.01" fadeTimeUnit="seconds"'
        )
        + audio_clip("8", "1", "Window", "audio/loop.wav", ' playStart="0.25" playStop="0.75"')
    ) + clips_timeline(audio_clip("0", "2", "Alt", "audio/loop.wav"), "Take B")
    loose = (
        f'        <Notes id="{uid("loose")}">\n'
        + note("0", "1", 60)
        + note("4", "1", 62)
        + "        </Notes>\n"
    )
    seconds = clips_timeline(
        '          <Clip time="1" duration="2" name="Sec">\n'
        + notes("notes5", note("0", "1", 55))
        + "          </Clip>\n"
    )
    bass = clips_timeline(
        '          <Clip time="0" duration="4" name="One">\n'
        + notes("notes6", note("0", "1", 40))
        + "          </Clip>\n"
    ) + clips_timeline(
        '          <Clip time="0" duration="4" name="Two">\n'
        + notes("notes7", note("0", "1", 41))
        + "          </Clip>\n",
        "Take B",
    )
    body = (
        transport()
        + "  <Structure>\n"
        + track("t1", "Lead", "notes", channel("c1", destination="cm"))
        + track("t2", "Pad", "audio", channel("c2", destination="cm"))
        + track("t3", "Loose", "notes", channel("c3", destination="cm"))
        + track("t4", "Secs", "notes", channel("c4", destination="cm"))
        + track("t5", "Bass", "notes", channel("c5", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        + arrangement(
            lane("t1", clips_timeline(lead))
            + lane("t2", pad)
            + lane("t3", loose)
            + lane("t4", seconds, ' timeUnit="seconds"')
            + lane("t5", bass)
        )
        + "  <Scenes/>\n"
    )
    write(
        "clips",
        project(body),
        report(
            [
                entry("a note that can't be played", 'the clip "Loop" at 1.1.1 on the track "Lead"', 2, "left out"),
                entry("a clip another clip covers", 'the clip "Loop" at 1.1.1 on the track "Lead"', 1, "shortened"),
                entry("notes on more than one channel", 'the track "Lead"', 1, "left out; the notes play on one channel"),
                entry("a loop in an unwarped clip", 'the clip "Looped" at 1.1.1 on the track "Pad"', 1, "unrolled into 4 clips"),
                entry("a clip's fades", 'the clip "Faded" at 2.1.1 on the track "Pad"', 1, "left out"),
                entry("a further lane of clips", 'the track "Bass"', 1, "left out"),
            ]
        ),
        {"audio/loop.wav": wav_bytes(220.0)},
    )


def media() -> None:
    """Media: embedded files by every spelling of a path, files on disk,
    files found by name, and files that can't be used."""
    drum = wav_bytes()
    clips_xml = "".join(
        [
            audio_clip("0", "1", "Embedded", "audio/drum.wav", sample_rate=48000),
            audio_clip("1", "1", "Copy", "audio/copy.wav"),
            audio_clip("2", "1", "Spaced", "audio/Kick \u2013 \u00dcn\u00ef 1.wav"),
            audio_clip("3", "1", "Backslash", "audio\\win.wav"),
            audio_clip("4", "1", "Relative", "media-files/outside.wav", external=True),
            audio_clip("5", "1", "Elsewhere", "/nonexistent-dawproject/found.wav", external=True),
            audio_clip("6", "1", "Missing", "audio/nowhere.wav"),
            audio_clip("7", "1", "Nowhere", "elsewhere/nowhere.wav", external=True),
            audio_clip("8", "1", "Broken", "audio/broken.wav"),
        ]
    )
    body = (
        transport()
        + "  <Structure>\n"
        + track("t1", "Drums", "audio", channel("c1", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        + arrangement(lane("t1", clips_timeline(clips_xml)))
        + "  <Scenes/>\n"
    )
    folder = OUT / "media-files"
    (folder / "sub").mkdir(parents=True, exist_ok=True)
    (folder / "outside.wav").write_bytes(wav_bytes(330.0))
    (folder / "sub" / "found.wav").write_bytes(wav_bytes(550.0))
    place = lambda name, at: f'the clip "{name}" at {at} on the track "Drums"'
    write(
        "media",
        project(body),
        report(
            [
                entry(
                    "the file `audio/drum.wav`",
                    place("Embedded", "1.1.1"),
                    1,
                    "the file's own rate, channels and length were used",
                ),
                entry(
                    "the file `/nonexistent-dawproject/found.wav`",
                    place("Elsewhere", "2.2.1"),
                    1,
                    "found at media-files/sub/found.wav",
                ),
                entry(
                    "the file `audio/nowhere.wav`",
                    place("Missing", "2.3.1"),
                    1,
                    "left out; the container doesn't hold it",
                ),
                entry(
                    "the file `elsewhere/nowhere.wav`",
                    place("Nowhere", "2.4.1"),
                    1,
                    "left out; it wasn't found",
                ),
                entry(
                    "the file `audio/broken.wav`",
                    place("Broken", "3.1.1"),
                    1,
                    "left out; it isn't a file Beatlayer can play",
                ),
            ]
        ),
        {
            "audio/drum.wav": drum,
            "audio/copy.wav": drum,
            "audio/Kick \u2013 \u00dcn\u00ef 1.wav": wav_bytes(660.0),
            "audio/win.wav": wav_bytes(770.0),
            "audio/broken.wav": b"this is not audio",
        },
    )


def session() -> None:
    """The Session View: three scenes, a clip on a MIDI and an audio track,
    an empty slot, slots with no Stop button, a slot for a group and a second
    slot for one track."""
    scene_one = (
        f'      <ClipSlot track="t1" hasStop="true" id="{uid("slot")}">\n'
        '        <Clip time="0" duration="4" name="Idea">\n'
        + notes(uid("notes"), note("0", "1", 60))
        + "        </Clip>\n"
        "      </ClipSlot>\n"
        f'      <ClipSlot track="t2" id="{uid("slot")}">\n'
        '        <Clip time="0" duration="2" contentTimeUnit="seconds" loopStart="0" '
        'loopEnd="0.5" name="Beat">\n'
        f'          <Audio channels="2" duration="1" sampleRate="44100" id="{uid("audio")}">\n'
        '            <File path="audio/beat.wav"/>\n'
        "          </Audio>\n"
        "        </Clip>\n"
        "      </ClipSlot>\n"
        f'      <ClipSlot track="tg" hasStop="false" id="{uid("slot")}"/>\n'
        f'      <ClipSlot track="t1" id="{uid("slot")}"/>\n'
    )
    scene_two = (
        f'      <ClipSlot track="t1" hasStop="false" id="{uid("slot")}"/>\n'
        f'      <ClipSlot track="t3" id="{uid("slot")}">\n'
        '        <Clip time="0" duration="4" name="Inner">\n'
        + notes(uid("notes"), note("0", "1", 62))
        + "        </Clip>\n"
        "      </ClipSlot>\n"
    )
    body = (
        transport()
        + "  <Structure>\n"
        + track("t1", "Lead", "notes", channel("c1", destination="cm"))
        + track("t2", "Drums", "audio", channel("c2", destination="cm"))
        + track(
            "tg",
            "Group",
            "tracks",
            channel("cg", destination="cm"),
            children=track("t3", "Inside", "notes", channel("c3", destination="cg")),
        )
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Scenes>\n"
        f'    <Scene id="sc1" name="One">\n{scene_one}    </Scene>\n'
        f'    <Scene id="sc2" name="Two">\n{scene_two}    </Scene>\n'
        '    <Scene id="sc3"/>\n'
        "  </Scenes>\n"
    )
    write(
        "session",
        project(body),
        report(
            [
                entry("a slot for a track that can't hold clips", 'the scene "One"', 1, "left out"),
                entry("a second slot for one track in one scene", 'the scene "One"', 1, "left out"),
            ]
        ),
        {"audio/beat.wav": wav_bytes(440.0)},
    )


def bitwig_shaped() -> None:
    """A file shaped as Bitwig Studio writes one: the master last, a group of
    two tracks, a hybrid track with a CLAP plug-in and a nested, looped audio
    clip of two warped events with fades, an unwarped loop, a return fed by a
    send, a track routed into another, and two scenes."""
    nested = (
        '          <Clip time="0" duration="8" contentTimeUnit="beats" loopStart="0" '
        'loopEnd="4" fadeInTime="0.5" fadeOutTime="0.5" fadeTimeUnit="beats" name="Loop">\n'
        f'            <Clips id="{uid("clips")}">\n'
        + warped_event("0", "2", "2", "1", "audio/loop.wav")
        + warped_event("2", "2", "4", "1", "audio/loop.wav")
        + "            </Clips>\n"
        "          </Clip>\n"
    )
    scenes = (
        "  <Scenes>\n"
        f'    <Scene id="{uid("scene")}" name="Verse">\n'
        f'      <ClipSlot track="t1" id="{uid("slot")}">\n'
        '        <Clip time="0" duration="4" name="Kick">\n'
        + notes(uid("notes"), note("0", "1", 36))
        + "        </Clip>\n"
        "      </ClipSlot>\n"
        "    </Scene>\n"
        f'    <Scene id="{uid("scene")}" name="Chorus"/>\n'
        "  </Scenes>\n"
    )
    body = (
        transport()
        + "  <Structure>\n"
        + track(
            "tg",
            "Group",
            "tracks",
            channel("cg", destination="cm"),
            children=(
                track("t1", "Kick", "notes", channel("c1", destination="cg"))
                + track("t2", "Snare", "notes", channel("c2", destination="cg"))
            ),
        )
        + track(
            "th",
            "Hybrid",
            "audio notes",
            channel(
                "ch",
                destination="cm",
                devices=devices(
                    plugin("ClapPlugin", "instrument", "Surge XT", "plugins/state.clap-preset")
                ),
            ),
        )
        + track(
            "td",
            "Drums",
            "audio",
            channel("cd", destination="cm", sends=sends(send("cr", "post", "0.5"))),
        )
        + track("ts", "Source", "audio", channel("cs", destination="cb"))
        + track("tb", "Bus", "audio", channel("cb", role="submix", destination="cm"))
        + track("tr", "Reverb", "audio", channel("cr", role="effect", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        + arrangement(
            lane(
                "t1",
                clips_timeline(
                    '          <Clip time="0" duration="4" name="Kick">\n'
                    + notes(uid("notes"), note("0", "1", 36))
                    + "          </Clip>\n"
                ),
            )
            + lane("th", clips_timeline(nested))
            + lane(
                "td",
                clips_timeline(
                    audio_clip("0", "4", "Beat", "audio/drums.wav", ' loopStart="0" loopEnd="0.5"')
                ),
            )
            + lane("ts", clips_timeline(audio_clip("0", "4", "Tone", "audio/source.wav")))
        )
        + scenes
    )
    write(
        "bitwig-shaped",
        project(body),
        report(
            [
                entry(
                    "a track holding both audio and notes",
                    'the track "Hybrid"',
                    1,
                    "split into a MIDI track and an audio track",
                ),
                entry("a CLAP plug-in", 'the track "Hybrid"', 1, "left out"),
                entry(
                    "the warp mode `stretch`",
                    'a clip at 1.1.1 on the track "Hybrid Audio"',
                    1,
                    "changed to the default warp mode",
                ),
                entry(
                    "the warp mode `stretch`",
                    'a clip at 1.3.1 on the track "Hybrid Audio"',
                    1,
                    "changed to the default warp mode",
                ),
                entry(
                    "a nested clip",
                    'the clip "Loop" at 1.1.1 on the track "Hybrid Audio"',
                    2,
                    "flattened",
                ),
                entry(
                    "a clip's fades",
                    'the clip "Loop" at 1.1.1 on the track "Hybrid Audio"',
                    1,
                    "left out",
                ),
                entry(
                    "a loop in an unwarped clip",
                    'the clip "Beat" at 1.1.1 on the track "Drums"',
                    1,
                    "unrolled into 4 clips",
                ),
            ]
        ),
        {
            "audio/loop.wav": wav_bytes(220.0),
            "audio/drums.wav": wav_bytes(330.0),
            "audio/source.wav": wav_bytes(440.0),
        },
    )


def studio_one_shaped() -> None:
    """A file shaped as Studio One writes one: a folder track with no
    channel, an FX channel fed by a send, a bus channel tracks route to, alias
    clips of notes and of audio, Audio Unit and VST2 plug-ins, a video track
    and a marker track."""
    guitar = clips_timeline(
        audio_clip("0", "4", "Riff", "audio/riff.wav", ident="riff")
        + '          <Clip time="4" duration="4" name="Riff 2" reference="riff"/>\n'
    )
    synth = clips_timeline(
        '          <Clip time="0" duration="4" name="Idea">\n'
        + notes("notes1", note("0", "1", 60))
        + "          </Clip>\n"
        '          <Clip time="4" duration="4" name="Idea again" reference="notes1"/>\n'
    )
    body = (
        transport()
        + "  <Structure>\n"
        + track(
            "tf",
            "Folder",
            "tracks",
            children=(
                track(
                    "t1",
                    "Guitar",
                    "audio",
                    channel(
                        "c1",
                        destination="cb",
                        sends=sends(send("cx", "pre", "0.5")),
                        devices=devices(
                            plugin("AuPlugin", "audioFX", "Test AU", "plugins/state.aupreset")
                        ),
                    ),
                )
                + track(
                    "t2",
                    "Synth",
                    "notes",
                    channel(
                        "c2",
                        destination="cm",
                        sends=sends(send("cb", "post", "0.25")),
                        devices=devices(
                            plugin("Vst2Plugin", "instrument", "Test VST2", "plugins/state.fxp")
                        ),
                    ),
                )
            ),
        )
        + track("tb", "Bus", "audio", channel("cb", role="submix", destination="cm"))
        + track("tx", "Delay", "audio", channel("cx", role="effect", destination="cm"))
        + track("tv", "Video", "video")
        + track("tk", "Markers", "markers")
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        + arrangement(lane("t1", guitar) + lane("t2", synth))
        + "  <Scenes/>\n"
    )
    write(
        "studio-one-shaped",
        project(body),
        report(
            [
                entry("a video track", 'the track "Video"', 1, "left out"),
                entry("a track of markers", 'the track "Markers"', 1, "left out"),
                entry("an Audio Unit plug-in", 'the track "Guitar"', 1, "left out"),
                entry("a VST2 plug-in", 'the track "Synth"', 1, "left out"),
                entry(
                    "a send to a channel that isn't a return",
                    'the track "Synth"',
                    1,
                    "left out",
                ),
                entry(
                    "an alias clip",
                    'the clip "Riff 2" at 2.1.1 on the track "Guitar"',
                    1,
                    "read as a copy",
                ),
                entry(
                    "an alias clip",
                    'the clip "Idea again" at 2.1.1 on the track "Synth"',
                    1,
                    "read as a copy",
                ),
            ]
        ),
        {"audio/riff.wav": wav_bytes(262.0)},
    )


def cubase_shaped() -> None:
    """A file shaped as Cubase writes one: a VST3 instrument, a group
    channel tracks route to, a strip of the four generic devices before a
    VST3 insert, an FX channel fed by sends that disagree about pre and post,
    automation, a marker track, a disabled track, a colour, a track routed
    into one that holds clips, and a file whose rate it disagrees with."""
    automation = (
        f'      <Lanes track="t4" id="{uid("lane")}">\n'
        f'        <Points unit="linear" id="{uid("points")}">\n'
        '          <Target parameter="c4v"/>\n'
        '          <RealPoint time="0" value="1" interpolation="linear"/>\n'
        '          <RealPoint time="4" value="0.5" interpolation="linear"/>\n'
        "        </Points>\n"
        f'        <Points unit="normalized" id="{uid("points")}">\n'
        '          <Target parameter="c4p"/>\n'
        '          <RealPoint time="0" value="0.5" interpolation="hold"/>\n'
        "        </Points>\n"
        "      </Lanes>\n"
    )
    pad = channel("c2", destination="cg", sends=sends(send("cx", "post", "0.5")))
    body = (
        transport()
        + "  <Structure>\n"
        + track(
            "t1",
            "Instrument",
            "notes",
            channel(
                "c1",
                destination="cm",
                sends=sends(
                    send("cx", "pre", "0.5"), send("cx", "post", "0.25", enabled="false")
                ),
                devices=devices(
                    plugin("Vst3Plugin", "instrument", "Test VST3", "plugins/state.vstpreset")
                ),
            ),
        )
        + track("tg", "Group", "audio", channel("cg", role="submix", destination="cm"))
        + track("t2", "Pad", "audio", pad, color="#ff8800")
        + track("t3", "Strings", "audio", channel("c3", destination="cg"))
        + track(
            "t4",
            "Vocals",
            "audio",
            channel(
                "c4",
                destination="cm",
                devices=devices(
                    generic("Equalizer", "EQ"),
                    generic("Compressor", "Compressor"),
                    generic("NoiseGate", "Gate"),
                    generic("Limiter", "Limiter"),
                    plugin("Vst3Plugin", "audioFX", "Test Insert", "plugins/insert.vstpreset"),
                ),
            ),
        )
        + track("t6", "Layer", "audio", channel("c6", destination="c3"))
        + track("t5", "Disabled", "audio", channel("c5", destination="cm"), loaded="false")
        + track("tk", "Markers", "markers")
        + track("tx", "FX", "audio", channel("cx", role="effect", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        + arrangement(
            lane(
                "t3",
                clips_timeline(
                    audio_clip("0", "4", "Sound", "audio/sound.wav", sample_rate=48000)
                ),
            )
            + automation
        )
        + "  <Scenes/>\n"
    )
    write(
        "cubase-shaped",
        project(body),
        report(
            [
                entry("a colour", 'the track "Pad"', 1, "left out"),
                entry(
                    "a track that isn't loaded",
                    'the track "Disabled"',
                    1,
                    "its Track Activator is off",
                ),
                entry("a track of markers", 'the track "Markers"', 1, "left out"),
                entry("a VST3 plug-in", 'the track "Instrument"', 1, "left out"),
                entry("a generic equaliser", 'the track "Vocals"', 1, "left out"),
                entry("a generic compressor", 'the track "Vocals"', 1, "left out"),
                entry("a generic gate", 'the track "Vocals"', 1, "left out"),
                entry("a generic limiter", 'the track "Vocals"', 1, "left out"),
                entry("a VST3 plug-in", 'the track "Vocals"', 1, "left out"),
                entry(
                    "a send at -12 dB",
                    'the track "Instrument"',
                    1,
                    "left out; the send is switched off",
                ),
                entry(
                    "sends that disagree about pre or post",
                    'the return "FX"',
                    1,
                    "the return takes them post",
                ),
                entry(
                    "the file `audio/sound.wav`",
                    'the clip "Sound" at 1.1.1 on the track "Strings"',
                    1,
                    "the file's own rate, channels and length were used",
                ),
                entry(
                    "a track that holds clips and monitors an input",
                    'the track "Strings"',
                    1,
                    "the clips are silent while it monitors",
                ),
                entry("a timeline of automation", 'the track "Vocals"', 2, "left out"),
            ]
        ),
        {"audio/sound.wav": wav_bytes(330.0)},
    )


def damaged() -> None:
    """The files a reader has to survive: none of them may crash it."""
    folder = OUT / "damaged"
    if folder.exists():
        for old in folder.iterdir():
            old.unlink()
    folder.mkdir(parents=True, exist_ok=True)

    def damaged(name: str, data: bytes, report_json: dict | None) -> None:
        (folder / f"{name}.dawproject").write_bytes(data)
        if report_json is not None:
            (folder / f"{name}.json").write_text(
                json.dumps(report_json, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )

    # Not a ZIP file at all.
    damaged("not-a-zip", b"this is not a DAWproject file\n", None)

    # A ZIP file with no `project.xml`.
    with zipfile.ZipFile(folder / "no-project-xml.dawproject", "w") as archive:
        add(archive, "readme.txt", "no project here\n")
    (folder / "no-project-xml.json").write_text(
        json.dumps({"refused": "no project.xml"}, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    # A version Beatlayer doesn't read.
    damaged(
        "version-2",
        zip_bytes(project("  <Structure/>\n  <Scenes/>\n", version="2.0")),
        {"refused": "version 2.0"},
    )

    # XML that breaks off in the middle.
    damaged(
        "broken-xml",
        zip_bytes(
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<Project version="1.0">\n'
            f'  <Application name="{APPLICATION}" version="{VERSION}"/>\n'
            "  <Structure>\n"
            '    <Track contentType="notes" id="t1" name="Bass">\n'
        ),
        None,
    )

    # A byte-order mark, and UTF-16.
    damaged(
        "utf-8-bom",
        zip_bytes(b"\xef\xbb\xbf" + project("  <Structure/>\n  <Scenes/>\n").encode()),
        report([]),
    )
    damaged(
        "utf-16",
        zip_bytes(project("  <Structure/>\n  <Scenes/>\n").encode("utf-16")),
        report([]),
    )

    # A document type declaration with an entity in it.
    damaged(
        "doctype",
        zip_bytes(
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<!DOCTYPE Project [<!ENTITY boom "boom">]>\n'
            '<Project version="1.0">\n'
            f'  <Application name="{APPLICATION}" version="{VERSION}"/>\n'
            "  <Structure/>\n"
            "  <Scenes/>\n"
            "</Project>\n"
        ),
        report([]),
    )

    # A reference to an `id` that isn't there.
    damaged(
        "missing-reference",
        zip_bytes(
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<Project version="1.0">\n'
            f'  <Application name="{APPLICATION}" version="{VERSION}"/>\n'
            "  <Structure>\n"
            '    <Track contentType="notes" id="t1" name="Bass">\n'
            '      <Channel destination="nowhere" role="regular" id="c1">\n'
            '        <Volume unit="linear" value="1" id="c1v"/>\n'
            "      </Channel>\n"
            "    </Track>\n"
            "  </Structure>\n"
            "  <Scenes/>\n"
            "</Project>\n"
        ),
        report(
            [
                entry(
                    "a destination the file doesn't hold",
                    'the track "Bass"',
                    1,
                    "left out; the track plays to its default output",
                ),
            ]
        ),
    )

    # An element and an attribute Beatlayer doesn't know.
    damaged(
        "unknown",
        zip_bytes(
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<Project version="1.0">\n'
            f'  <Application name="{APPLICATION}" version="{VERSION}"/>\n'
            "  <Structure>\n"
            '    <Track contentType="notes" id="t1" name="Bass" wobble="3">\n'
            "      <Wobble/>\n"
            "    </Track>\n"
            "  </Structure>\n"
            "  <Scenes/>\n"
            "</Project>\n"
        ),
        report(
            [
                entry("the element `Wobble`", "in the project", 1, "left out"),
                entry("the attribute `wobble`", "in the project", 1, "left out"),
            ]
        ),
    )

    # An entry whose name could escape the folder it is written to.
    with zipfile.ZipFile(folder / "escape.dawproject", "w") as archive:
        add(archive, "project.xml", project("  <Structure/>\n  <Scenes/>\n"))
        add(archive, "../escape.wav", b"not a wav")
    (folder / "escape.json").write_text(
        json.dumps(
            report([entry("the entry `../escape.wav`", "in the container", 1, "left out")]),
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    # Entries the reader never reads or writes: an absolute name, a drive
    # letter, one compressed by bzip2, and a folder entry that holds nothing.
    with zipfile.ZipFile(folder / "unsafe-entries.dawproject", "w") as archive:
        add(archive, "project.xml", project("  <Structure/>\n  <Scenes/>\n"))
        add(archive, "/absolute.wav", b"not a wav")
        add(archive, "C:\\drive.wav", b"not a wav")
        add(archive, "audio/", b"")
        add(archive, "audio/bzip2.wav", b"not a wav", zipfile.ZIP_BZIP2)
    (folder / "unsafe-entries.json").write_text(
        json.dumps(
            report(
                [
                    entry("the entry `/absolute.wav`", "in the container", 1, "left out"),
                    entry("the entry `C:\\drive.wav`", "in the container", 1, "left out"),
                    entry("the entry `audio/bzip2.wav`", "in the container", 1, "left out"),
                ]
            ),
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    # Two elements with one `id`: the second is read without it.
    damaged(
        "duplicate-id",
        zip_bytes(
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<Project version="1.0">\n'
            f'  <Application name="{APPLICATION}" version="{VERSION}"/>\n'
            "  <Structure>\n"
            '    <Track contentType="notes" id="t1" name="Bass"/>\n'
            '    <Track contentType="notes" id="t1" name="Lead"/>\n'
            "  </Structure>\n"
            "  <Scenes/>\n"
            "</Project>\n"
        ),
        report([entry("a repeated `id` (`t1`)", "in the project", 1, "left out")]),
    )

    # Elements nested deeper than the reader's limit.
    deep = "".join(f'<Lanes id="deep{i}">' for i in range(300))
    deep += "</Lanes>" * 300
    damaged(
        "too-deep",
        zip_bytes(
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<Project version="1.0">\n'
            f'  <Application name="{APPLICATION}" version="{VERSION}"/>\n'
            f"  <Structure>{deep}</Structure>\n"
            "  <Scenes/>\n"
            "</Project>\n"
        ),
        None,
    )


def zip_bytes(data: bytes) -> bytes:
    """A container holding one file, as bytes."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        add(archive, "project.xml", data)
    return buffer.getvalue()


if __name__ == "__main__":
    main()
