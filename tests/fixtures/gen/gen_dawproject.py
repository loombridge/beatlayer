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
) -> str:
    """A track element."""
    return (
        f'    <Track contentType="{content_type}" loaded="{loaded}" id="{ident}" name="{name}">\n'
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


def write(name: str, xml: str, report: dict, files: dict[str, bytes] | None = None) -> None:
    """The fixture `name`: its container and the report it should give."""
    OUT.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(OUT / f"{name}.dawproject", "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("project.xml", xml)
        for path, data in (files or {}).items():
            archive.writestr(path, data)
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
    """Clips: a loop, a disabled clip, an overlap, a note out of range and
    notes on three channels."""
    body = (
        transport()
        + "  <Structure>\n"
        + track("t1", "Lead", "notes", channel("c1", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Arrangement id=\"arr\">\n"
        '    <Lanes timeUnit="beats" id="lanes">\n'
        '      <Lanes track="t1" id="lane1">\n'
        '        <Clips id="clips1">\n'
        '          <Clip time="0" duration="4" loopStart="0" loopEnd="2" name="Loop">\n'
        + notes(
            "notes1",
            note("0", "1", 60)
            + note("1", "1", 62, 1)
            + note("2", "1", 64, 2)
            + note("3", "1", 200),
        )
        + "          </Clip>\n"
        '          <Clip time="2" duration="4" name="Over">\n'
        + notes("notes2", note("0", "2", 67))
        + "          </Clip>\n"
        '          <Clip time="8" duration="2" enable="false" name="Off">\n'
        + notes("notes3", note("0", "1", 48))
        + "          </Clip>\n"
        "        </Clips>\n"
        "      </Lanes>\n"
        "    </Lanes>\n"
        "  </Arrangement>\n"
        "  <Scenes/>\n"
    )
    write(
        "clips",
        project(body),
        report(
            [
                entry(
                    "a note that can't be played",
                    'the track "Lead"',
                    1,
                    "left out",
                ),
                entry(
                    "notes on more than one channel",
                    'the track "Lead"',
                    1,
                    "left out; the notes play on one channel",
                ),
                entry("a clip another clip covers", "the track", 1, "shortened"),
            ]
        ),
    )


def media() -> None:
    """Media: an embedded file and one the container doesn't hold."""
    body = (
        transport()
        + "  <Structure>\n"
        + track("t1", "Drums", "audio", channel("c1", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Arrangement id=\"arr\">\n"
        '    <Lanes timeUnit="beats" id="lanes">\n'
        '      <Lanes track="t1" id="lane1">\n'
        '        <Clips id="clips1">\n'
        '          <Clip time="0" duration="4" contentTimeUnit="seconds" name="Embedded">\n'
        '            <Audio channels="2" duration="1" sampleRate="44100" id="audio1">\n'
        '              <File path="audio/drum.wav"/>\n'
        "            </Audio>\n"
        "          </Clip>\n"
        '          <Clip time="4" duration="4" contentTimeUnit="seconds" name="Missing">\n'
        '            <Audio channels="2" duration="1" sampleRate="44100" id="audio2">\n'
        '              <File path="audio/nowhere.wav"/>\n'
        "            </Audio>\n"
        "          </Clip>\n"
        "        </Clips>\n"
        "      </Lanes>\n"
        "    </Lanes>\n"
        "  </Arrangement>\n"
        "  <Scenes/>\n"
    )
    write(
        "media",
        project(body),
        report(
            [
                entry(
                    "the file `audio/nowhere.wav`",
                    'the track "Drums"',
                    1,
                    "left out; the container doesn't hold it",
                ),
            ]
        ),
        {"audio/drum.wav": wav_bytes()},
    )


def session() -> None:
    """The Session View: two scenes, an empty slot, a slot with no Stop
    button and a slot for a group."""
    body = (
        transport()
        + "  <Structure>\n"
        + track("t1", "Lead", "notes", channel("c1", destination="cm"))
        + track(
            "tg",
            "Group",
            "tracks",
            channel("cg", destination="cm"),
            children=track("t2", "Inside", "notes", channel("c2", destination="cg")),
        )
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Scenes>\n"
        '    <Scene id="sc1" name="One">\n'
        '      <ClipSlot track="t1" id="slot1">\n'
        '        <Clip time="0" duration="4" name="One">\n'
        + notes("notes1", note("0", "1", 60))
        + "        </Clip>\n"
        "      </ClipSlot>\n"
        '      <ClipSlot track="tg" id="slot2" hasStop="false"/>\n'
        "    </Scene>\n"
        '    <Scene id="sc2" name="Two"/>\n'
        "  </Scenes>\n"
    )
    write(
        "session",
        project(body),
        report(
            [
                entry(
                    "a slot for a track that can't hold clips",
                    "the Session View",
                    1,
                    "left out",
                ),
            ]
        ),
    )


def bitwig_shaped() -> None:
    """A file shaped as Bitwig Studio writes one: the master last, a group of
    two tracks, a hybrid track, a CLAP plug-in and a nested audio clip."""
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
                devices=(
                    "        <Devices>\n"
                    '          <ClapPlugin deviceID="org.surge-synth-team.surge-xt" '
                    'deviceName="Surge XT" deviceRole="instrument" loaded="true" id="plug" '
                    'name="Surge XT">\n'
                    "            <Parameters/>\n"
                    '            <Enabled value="true" id="plugon" name="On/Off"/>\n'
                    '            <State path="plugins/state.clap-preset"/>\n'
                    "          </ClapPlugin>\n"
                    "        </Devices>\n"
                ),
            ),
        )
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Arrangement id=\"arr\">\n"
        '    <Lanes timeUnit="beats" id="lanes">\n'
        '      <Lanes track="t1" id="lane1">\n'
        '        <Clips id="clips1">\n'
        '          <Clip time="0" duration="4" name="Kick">\n'
        + notes("notes1", note("0", "1", 36))
        + "          </Clip>\n"
        "        </Clips>\n"
        "      </Lanes>\n"
        '      <Lanes track="th" id="lane2">\n'
        '        <Clips id="clips2">\n'
        '          <Clip time="0" duration="4" contentTimeUnit="beats" name="Loop">\n'
        '            <Clips id="clips3">\n'
        '              <Clip time="0" duration="4" contentTimeUnit="beats">\n'
        '                <Warps contentTimeUnit="seconds" timeUnit="beats" id="warps">\n'
        '                  <Audio algorithm="stretch" channels="2" duration="1" sampleRate="44100" '
        'id="audio1">\n'
        '                    <File path="audio/loop.wav"/>\n'
        "                  </Audio>\n"
        '                  <Warp time="0" contentTime="0"/>\n'
        '                  <Warp time="4" contentTime="1"/>\n'
        "                </Warps>\n"
        "              </Clip>\n"
        "            </Clips>\n"
        "          </Clip>\n"
        "        </Clips>\n"
        "      </Lanes>\n"
        "    </Lanes>\n"
        "  </Arrangement>\n"
        "  <Scenes/>\n"
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
                    'the track "Hybrid Audio"',
                    1,
                    "changed to the default warp mode",
                ),
                entry("a nested clip", 'the track "Hybrid Audio"', 1, "flattened"),
            ]
        ),
        {"audio/loop.wav": wav_bytes(220.0)},
    )


def studio_one_shaped() -> None:
    """A file shaped as Studio One writes one: a folder track with no
    channel, an FX channel, a send that isn't to a return and an alias
    clip."""
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
                    "Inside",
                    "notes",
                    channel(
                        "c1",
                        destination="cm",
                        sends=(
                            "        <Sends>\n"
                            '          <Send destination="cm" type="post" id="send1">\n'
                            '            <Enable value="true" id="send1on" name="On/Off"/>\n'
                            '            <Pan unit="normalized" min="0" max="1" value="0.5" '
                            'id="send1p" name="Pan"/>\n'
                            '            <Volume unit="linear" min="0" max="2" value="0.5" '
                            'id="send1v" name="Volume"/>\n'
                            "          </Send>\n"
                            "        </Sends>\n"
                        ),
                    ),
                )
            ),
        )
        + track("tx", "FX", "audio", channel("cx", role="effect", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Arrangement id=\"arr\">\n"
        '    <Lanes timeUnit="beats" id="lanes">\n'
        '      <Lanes track="t1" id="lane1">\n'
        '        <Clips id="clips1">\n'
        '          <Clip time="0" duration="4" name="Inside">\n'
        + notes("notes1", note("0", "1", 60))
        + "          </Clip>\n"
        '          <Clip time="4" duration="4" name="Alias" reference="notes1"/>\n'
        "        </Clips>\n"
        "      </Lanes>\n"
        "    </Lanes>\n"
        "  </Arrangement>\n"
        "  <Scenes/>\n"
    )
    write(
        "studio-one-shaped",
        project(body),
        report(
            [
                entry(
                    "a send to a channel that isn't a return",
                    'the track "Inside"',
                    1,
                    "left out",
                ),
                entry("an alias clip", "the clip", 1, "read as a copy"),
            ]
        ),
    )


def cubase_shaped() -> None:
    """A file shaped as Cubase writes one: a VST3 instrument, a disabled
    track, sends that disagree about pre or post and a file whose rate the
    file disagrees with."""
    sends = (
        "        <Sends>\n"
        '          <Send destination="cx" type="pre" id="send1">\n'
        '            <Enable value="true" id="send1on" name="On/Off"/>\n'
        '            <Volume unit="linear" min="0" max="2" value="0.5" id="send1v" '
        'name="Volume"/>\n'
        "          </Send>\n"
        '          <Send destination="cx" type="post" id="send2">\n'
        '            <Enable value="false" id="send2on" name="On/Off"/>\n'
        '            <Volume unit="linear" min="0" max="2" value="0.25" id="send2v" '
        'name="Volume"/>\n'
        "          </Send>\n"
        "        </Sends>\n"
    )
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
                sends=sends,
                devices=(
                    "        <Devices>\n"
                    '          <Vst3Plugin deviceID="1234abcd-1234-1234-1234-1234567890ab" '
                    'deviceName="Test VST3" deviceRole="instrument" loaded="true" id="plug" '
                    'name="Test VST3">\n'
                    "            <Parameters/>\n"
                    '            <Enabled value="true" id="plugon" name="On/Off"/>\n'
                    '            <State path="plugins/state.vstpreset"/>\n'
                    "          </Vst3Plugin>\n"
                    "        </Devices>\n"
                ),
            ),
        )
        + track(
            "t2",
            "Pad",
            "audio",
            channel(
                "c2",
                destination="cm",
                sends=(
                    "        <Sends>\n"
                    '          <Send destination="cx" type="post" id="send3">\n'
                    '            <Enable value="true" id="send3on" name="On/Off"/>\n'
                    '            <Volume unit="linear" min="0" max="2" value="0.5" id="send3v" '
                    'name="Volume"/>\n'
                    "          </Send>\n"
                    "        </Sends>\n"
                ),
            ),
        )
        + track("t3", "Disabled", "audio", channel("c3", destination="cm"), loaded="false")
        + track("tx", "FX", "audio", channel("cx", role="effect", destination="cm"))
        + track("tm", "Master", "audio notes", channel("cm", role="master"))
        + "  </Structure>\n"
        "  <Arrangement id=\"arr\">\n"
        '    <Lanes timeUnit="beats" id="lanes">\n'
        '      <Lanes track="t3" id="lane1">\n'
        '        <Clips id="clips1">\n'
        '          <Clip time="0" duration="4" contentTimeUnit="seconds" name="Sound">\n'
        '            <Audio channels="2" duration="1" sampleRate="48000" id="audio1">\n'
        '              <File path="audio/sound.wav"/>\n'
        "            </Audio>\n"
        "          </Clip>\n"
        "        </Clips>\n"
        "      </Lanes>\n"
        "    </Lanes>\n"
        "  </Arrangement>\n"
        "  <Scenes/>\n"
    )
    write(
        "cubase-shaped",
        project(body),
        report(
            [
                entry(
                    "a track that isn't loaded",
                    'the track "Disabled"',
                    1,
                    "its Track Activator is off",
                ),
                entry("a VST3 plug-in", 'the track "Instrument"', 1, "left out"),
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
                    'the track "Disabled"',
                    1,
                    "the file's own rate, channels and length were used",
                ),
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
        archive.writestr("readme.txt", "no project here\n")
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
                    "left out; the track plays to Main",
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
        archive.writestr("project.xml", project("  <Structure/>\n  <Scenes/>\n"))
        archive.writestr("../escape.wav", b"not a wav")
    (folder / "escape.json").write_text(
        json.dumps(
            report([entry("the entry `../escape.wav`", "in the container", 1, "left out")]),
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
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
        archive.writestr("project.xml", data)
    return buffer.getvalue()


if __name__ == "__main__":
    main()
