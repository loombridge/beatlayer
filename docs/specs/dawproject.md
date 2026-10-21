# DAWproject import and export

- **Status:** draft
- **Work packages:** WP-6.36 (reading a file and opening it as a set: the tracks, mixer and routing, clips, media, the Session View's scenes, tempo and time signature, and the import report: R1 to R22, R24 to R32 and R34 to R43), WP-6.36b (colours and crossfades as they're read, devices and plug-in states, automation, MIDI controllers and note expressions, markers and metadata: R23's reading, R33, R44 to R64), WP-6.37 (writing a set as a file, and the round trip: R23's writing, R65 to R82), WP-6.37b (the Open dialog, the browser, dropping a file on the window, the Export DAWproject menu item and the report window: R83 to R88). The colours a set holds are [themes.md](themes.md)'s (WP-6.30b), and the Arrangement's crossfades between adjacent clips [audio-clips.md](audio-clips.md)'s (WP-6.51)
- **Parity-matrix row:** Files and export: (Beatlayer only) DAWproject import and export
- **Checked against:** the DAWproject format, version 1.0: its repository's README, `Project.xsd`, `MetaData.xsd`, XML reference and reference library (github.com/bitwig/dawproject, MIT licence), read 20 October 2026; the Live 12 reference manual (online edition), chapter 5, read 20 October 2026; the Live 12 release notes, read 20 October 2026; the UI parity reference's M-FILE-002, M-FILE-016 and I-DROP-025

## The behaviour

DAWproject is an open file format for moving a song between music applications: a ZIP archive holding the project as XML (`project.xml`), optional metadata (`metadata.xml`), the plug-ins' saved states, and the audio files, embedded or referenced. It carries the track structure and mixer, the Arrangement's clips and the clip launcher's scenes, notes with their expressions, audio with its warping, tempo and time-signature changes, automation, and plug-in and generic built-in devices. Live doesn't read or write it, so every rule here is Beatlayer's own, resting on the format's published specification and on Beatlayer's own model.

A person opens a `.dawproject` file from the Open dialog, the browser, or by dropping it on the window, and Beatlayer opens what it holds as a new, unsaved set, then shows a report of everything it left out or changed. File ▸ Export DAWproject... writes the open set as a `.dawproject` file, with a report of what the format can't carry. An agent does both through `set.import_dawproject` and `set.export_dawproject`.

The format lets each application write its own structure and leaves the importer to flatten it. Beatlayer reads every structure the schema allows and turns it into the nearest set Beatlayer can hold: nested clips are flattened, alias clips become copies, channels routed to other channels become tracks routed into each other, and devices with no Beatlayer equivalent are left out. Nothing is left out or changed without an entry in the report. A set Beatlayer writes and reads back is the same set, within what the format carries; Beatlayer's own devices and racks travel in Beatlayer's own preset format inside the file, so they survive the round trip, while other applications leave them out.

This spec covers reading and writing the file and what each element becomes. What the set's parts do once imported is in their own specs: [midi-clips.md](midi-clips.md), [audio-clips.md](audio-clips.md), [warping.md](warping.md), [automation.md](automation.md), [routing.md](routing.md), [mixer.md](mixer.md), [session-view.md](session-view.md), [device-chains.md](device-chains.md), [clap-hosting.md](clap-hosting.md), [vst3-hosting.md](vst3-hosting.md), [au-hosting.md](au-hosting.md) and [file-management.md](file-management.md). Opening a file isn't an undo step, as opening a set isn't (R11).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Format | DAWproject's README (github.com/bitwig/dawproject, version 1.0, MIT licence) | A `.dawproject` file is a ZIP container holding `project.xml` and `metadata.xml`, in UTF-8; the exporter chooses the folders for media and plug-in files; plug-in states are always embedded and audio is embedded or referenced; an exporter writes its own track and timeline structure and the importer flattens it as it needs; view settings and preferences aren't carried. Its example shows a master track last, volume as a linear gain up to 2, pan normalised with 0.5 as the centre, a CLAP plug-in's state in a `.clap-preset` file, and an audio clip that holds a timeline of clips, each warping a file from seconds onto beats |
| S2 | Format | DAWproject's `Project.xsd`, version 1.0 | The elements and attributes, which are required, and the enumerations: units (`linear`, `normalized`, `percent`, `decibel`, `hertz`, `semitones`, `seconds`, `beats`, `bpm`), time units (`beats`, `seconds`), device roles (`instrument`, `noteFX`, `audioFX`, `analyzer`), mixer roles (`regular`, `master`, `effect`, `submix`, `vca`), content types (`audio`, `automation`, `notes`, `video`, `markers`, `tracks`), interpolation (`hold`, `linear`), expression types (`gain`, `pan`, `transpose`, `timbre`, `formant`, `pressure`, `channelController`, `channelPressure`, `polyPressure`, `pitchBend`, `programChange`), EQ band types and send types |
| S3 | Format | DAWproject's `MetaData.xsd`, version 1.0 | `metadata.xml` holds optional text fields: title, artist, album, original artist, composer, songwriter, producer, arranger, year, genre, copyright, website and comment |
| S4 | Format | DAWproject's XML reference (`Reference.html`), version 1.0 | What each element means: a file reference's path is inside the container unless `external` is true, then relative to the `.dawproject` file or absolute; a timeline's time unit is inherited and is beats when none is given; a clip's time and duration are in its parent's unit and its play and loop points in its content's unit, its duration inferred from the play points when absent, a negative fade-in making a crossfade, and `reference` naming the timeline of a linked (alias) clip; an audio timeline's duration is its file's length in seconds; a warp timeline needs two warp events or more, joined by straight lines; a real point's interpolation is `hold` unless given; real values may be `inf` and `-inf`; a device's state file must be embedded, a VST3 state in `.vstpreset` form, a CLAP state in `.clap-preset` form, a VST2 state in FXB or FXP form; a device ID is a UUID in 8-4-4-4-12 form (VST3), a decimal number (VST2) or text (CLAP); a parameter's `parameterID` is VST2's index or VST3's parameter ID; automated parameters are listed in a device's `Parameters`; the generic compressor, gate, limiter and equaliser and their parameters' units |
| S5 | Format | DAWproject's reference library (`DawProject.java`, `DawProjectTest.java`), version 1.0 | The reference reader takes `project.xml` and `metadata.xml` from the container's root and accepts a byte-order mark for UTF-8 or UTF-16; the writer writes `metadata.xml`, `project.xml`, then the embedded files. Its examples put MIDI controller and pitch-bend automation both on a track's lane and inside clips, in normalised units, and reference audio by absolute and by relative path |
| S6 | Specification | The VST3 SDK (MIT licence since version 3.8): its preset file format | A `.vstpreset` file starts with `VST3`, a version, the class ID as 32 ASCII characters and the offset of its chunk list; the list names each chunk (`Comp`, the component's state; `Cont`, the controller's; `Info`, metadata) with its offset and size, all little-endian |
| S7 | Specification | Apple's Audio Unit documentation: the class info property list | An Audio Unit preset (`.aupreset`) is a property list of the unit's class info, which names the unit's type, subtype and manufacturer and carries its state |
| S8 | Specification | The CLAP specification's state extension | A CLAP plug-in saves and loads its whole state as one opaque stream of bytes |
| S9 | Manual | Live 12 reference manual, chapter 5 "Managing Files and Sets", section 5.4.1 "Creating, Opening and Saving Sets" | A set opens from the File menu's Open command or from the browser, by a double-click or Enter |
| S10 | Release notes | Live 12 release notes, 12.0 to 12.4 | No Live 12 release reads or writes DAWproject |
| S11 | Beatlayer | [The UI parity reference](../ui-parity/README.md): [menus.md](../ui-parity/menus.md) M-FILE-002 (Open Live Set..., Beatlayer's Open Beatlayer Set...) and M-FILE-016 (Export MIDI Clip..., the File menu's last item); [interactions.md](../ui-parity/interactions.md) I-DROP-025 (a `.dawproject` dropped on the main window); [the improvements](../05-improvements.md), rules 1 and 3 | Where the File menu's open and export items sit, and where a dropped file imports; an addition never moves what a Live user knows and goes where Live users would look |
| S12 | Beatlayer | [The architecture](../02-architecture.md): "Project files and interop", "The document", "Musical time", "Agent control", design rules D10 and D17 | Interop lives in `bl-interop`; a set is a project folder with its media by content hash; musical time is whole ticks, 705,600 to the quarter note; Beatlayer hosts no VST2; every action is a registered command with a permission class, and what an agent sets it can read back |
| S13 | Beatlayer | The specs of what an import makes: [mixer.md](mixer.md) R2, R3, R7, R8; [routing.md](routing.md) R3, R10, R11; [device-chains.md](device-chains.md) R2, R3; [warping.md](warping.md) R1, R2, R4; [audio-clips.md](audio-clips.md) R3, R4, R6; [arrangement-editing.md](arrangement-editing.md) R6; [midi-clips.md](midi-clips.md) R17; [midi-controller-data.md](midi-controller-data.md) R1, R2; [session-view.md](session-view.md) R1, R2; [session-automation.md](session-automation.md) R3; [audio-clip-editing.md](audio-clip-editing.md) R5; [file-management.md](file-management.md) R4; [audio-import.md](audio-import.md) R1; [browser.md](browser.md) R6; [clap-hosting.md](clap-hosting.md) R9, R12; [vst3-hosting.md](vst3-hosting.md) R9, R12; [au-hosting.md](au-hosting.md) R1, R12, R14; [themes.md](themes.md) R14, R15, R19, R20 (the colours of tracks, clips and scenes, WP-6.30b); [audio-clips.md](audio-clips.md)'s crossfades between adjacent clips (WP-6.51) | What each part of a set holds, its ranges, and how a missing plug-in or file is kept |

## Rules

**Reading a file (WP-6.36)**

- **R1.** A DAWproject file is a ZIP file whose root holds `project.xml`, and may hold `metadata.xml`. Beatlayer reads both from the root by those exact names; a container without `project.xml` is refused, the message saying so, and a file that isn't a ZIP file is refused as not a DAWproject file. (S1, S5)
- **R2.** `project.xml` and `metadata.xml` are read as UTF-8 with or without a byte-order mark, or as UTF-16 with one; any other encoding is refused, naming the part. (S1, S5)
- **R3.** A project whose `version` has the major number 1 is read; another major number is refused, naming the version. A later 1.x version is read like 1.0, what this spec doesn't know going to the report (R4). (S1, S2)
- **R4.** A file that breaks the schema is read as far as it can be, never crashing Beatlayer: an element or attribute Beatlayer doesn't know is skipped, and each name is listed once in the report with how many times it occurs; an element missing something the schema requires (a clip without its `time`, a note without its `key`) is left out with what it holds, and listed. (S2; Beatlayer's own)
- **R5.** The container is read safely: an entry whose name is absolute, starts with a drive letter, or has a `..` component is never read or written; an entry compressed by anything but storing or deflate is left out and listed; no entry is read past the size its header gives; `project.xml` and `metadata.xml` are refused when either is larger than 256 MiB unpacked; a container of more than 100,000 entries is refused. Before any embedded file is written to disk, Beatlayer checks the destination has room for all the files the project uses, by their headers' sizes, and refuses the import, writing nothing, when it hasn't. The limits are Beatlayer's own. (S1; Beatlayer's own)
- **R6.** The XML is read without expanding any entity but XML's five predefined ones and character references, without reading a document type declaration's contents or anything outside the container, and with elements nested at most 256 deep; deeper elements are left out and listed. (Beatlayer's own)
- **R7.** Every `id` names one element. A second element with an `id` already used is read without it, and listed. A reference (`destination`, `track`, `parameter`, `reference`) to an `id` the file doesn't hold, or to an element of a kind that can't be its target, is dropped and listed, and what holds it is read as if it had none. (S2, S4)
- **R8.** Numbers are read in the XML schema's decimal and double forms, and `inf` and `-inf` where a real value is allowed; booleans as `true`, `false`, `1` or `0`. A value that doesn't read, or isn't finite where a finite value is needed, is treated as missing (R4). (S2, S4)
- **R9.** A timeline's times are in its `timeUnit`, else its parent timeline's, else beats; a clip's `time` and `duration` are in its parent timeline's unit, and its `playStart`, `playStop`, `loopStart`, `loopEnd` and content in its `contentTimeUnit`, else its parent's unit. A beat is a quarter note: 705,600 ticks. A time in beats becomes the nearest tick. A time in seconds is converted at its place in the song: it becomes the tick the imported tempo map (R13) reaches that many seconds after the start of the timeline it's in, and a length in seconds becomes ticks the same way from where it starts. (S2, S4, S12)

**Opening it as a set (WP-6.36)**

- **R10.** `set.import_dawproject` reads the file at `path` and opens what it holds as a new set in place of the open one, as `set.open` does: it's refused while the open set has unsaved changes, and it's in the `external` class, since it reads a file outside the project. The reading, the extracting (R39) and the building run on a worker thread, with the Status Bar showing the progress; the set changes only when the new set is complete, and a failure leaves the open set as it was, with the reason in the result and the Status Bar. An agent's call returns when the new set is open. (S9, S12, S13)
- **R11.** The new set starts empty, not from the default set: no tracks, Main with no devices, every setting at a new set's value, then what the file holds. It's unsaved and its undo history is empty, as a set just opened has; until it's first saved its name is the file's name without `.dawproject`, and the first save asks where to save it, as a new set's does. (S9; Beatlayer's own)
- **R12.** The import report lists, in the file's order, each kind of thing left out or changed in each place it happens: what it was (a VST2 plug-in, a send's pan, a note's channel), where (the track by name, a clip by name and its position in bars, beats and sixteenths, a device by name), how many, and what Beatlayer did (left out, or changed to what). It names the application and version the file says made it. `set.import_dawproject` returns it, and `set.import_report` returns it again while the set the import made is open, and nothing after another set opens. A file Beatlayer reads with nothing lost or changed gives an empty report. (Beatlayer's own)

**Tempo and time signature (WP-6.36)**

- **R13.** The Transport's `Tempo` is the song's tempo from 1.1.1, and 120 BPM when there's none. The Arrangement's tempo automation replaces it: each point starts a segment of the tempo map, steady to the next point if its interpolation is `hold` and a straight ramp to the next point's value if it's `linear`; before the first point the first point's value holds. Points targeting the Transport's `Tempo` anywhere in the Arrangement are read the same way. A point in seconds is placed through the map the points before it make. A tempo outside 1 to 1,000 BPM is clamped into that range and listed. (S2, S4, S12)
- **R14.** The Transport's `TimeSignature` is the song's signature from 1.1.1, and 4/4 when there's none; each point of the Arrangement's time-signature automation is a signature change at its place. A signature Beatlayer can't hold (a numerator outside 1 to 99, a denominator not 1, 2, 4, 8 or 16) is left out and listed; of two changes at one tick, the later in the file wins. (S2, S12; [musical time](musical-time.md))

**Tracks, mixer and routing (WP-6.36; R23 WP-6.36b and WP-6.37)**

- **R15.** Each `Track` in `Structure` becomes one Beatlayer track, in the file's order, its child `Track`s the tracks of a group at any depth. Its `contentType` decides its kind: `notes` a MIDI track, `audio` an audio track, `tracks` (or child tracks) a group; with neither audio nor notes, a MIDI track if its channel holds an instrument (R44) and an audio track otherwise. A `Channel` that sits directly in `Structure` is read as a track that holds that channel and no clips. (S2, S4, S13)
- **R16.** A track's channel `role` changes what it becomes: `effect` makes a return track, in the file's order among the returns; `master` makes Main (R18); `submix` with child tracks makes a group, and without them an audio track that other tracks route into (R19); `vca` is left out with its automation and listed, since Beatlayer has no VCA. A return or group in the file that holds clips keeps none of them, listed. (S2, S13)
- **R17.** A track whose `contentType` holds both `audio` and `notes` becomes a MIDI track holding its channel, its devices and its notes, and an audio track straight after it, named "<the track's name> Audio", holding its audio clips, with the same output and no devices; the report lists the split. (S2; Beatlayer's own)
- **R18.** The first `master` channel's devices, volume and pan become Main's; with none, Main stays as a new set's. A master channel's mute, when on, is listed, as Main has no activator; a second `master` channel is read as a `submix` one and listed. (S1, S2, S13)
- **R19.** A channel's `destination` sets its track's output: its own group's channel, or Main's for a track at the top, is the default output; Main's channel from inside a group is Main; another audio track's channel is that track's Track In, and the receiving track's Monitor is set to In and its input to No Input, so it hears what is routed to it ([routing.md](routing.md) R3, R10, R11), a receiving track that holds clips being listed, since they're silent while it monitors. A channel with no `destination` takes the default output, and so, listed, does one naming any other channel (a MIDI track's, a return's, a VCA's) or nothing the import made. (S2, S4, S13)
- **R20.** A track takes its `Track`'s name, else its channel's, else its kind's default name and number; comments aren't kept and are counted in the report. A track whose `Track` has `loaded` false has its Track Activator off, listed. A channel's `audioChannels` isn't kept: Beatlayer's tracks are stereo, and a mono file plays on both sides. (S2, S4, S13)
- **R21.** A channel's `Volume` becomes its track's volume as a linear gain: `linear` as given, `decibel` as 10^(dB/20) with `-inf` as silence, and `normalized` placed on its `min` to `max` (0 to 1 when they're absent) as a linear gain; a gain past the fader's range is clamped to it and listed. `Pan` is placed from its `min` (hard left) to its `max` (hard right), 0 to 1 when they're absent, and 0 to 100 for `percent`, onto −1 to 1. `Mute` on turns the Track Activator off; `solo` true solos the track. (S1, S2, S4, S13)
- **R22.** Each `Send` to a return's channel becomes the track's send to that return, its `Volume` read as R21 reads a volume and clamped to 0 dB, listed when it was above. A send whose `Enable` is off is left out, listed with its level; a send's pan away from its centre is listed, as Beatlayer's sends have no pan. A return's Pre/Post switch is Pre when most of the sends to it are `pre`, Post otherwise, and listed when its sends disagree. Sends from returns or from Main, and sends to a channel that isn't a return, are left out and listed. (S2, S4, S13)
- **R23.** A track's, clip's or scene's `color` (`#rrggbb`) becomes the swatch of the colour palette ([themes.md](themes.md) R14, R15) nearest it, as the theme in use draws the palette, by the distance between the two in the CIELAB colour space; a track, clip or scene with no `color` takes the colour a new one takes ([themes.md](themes.md) R19, R20; a scene none). On export each writes its swatch's `#rrggbb` as the theme in use draws it, and a scene with no colour writes none. (S2, S4; [themes.md](themes.md); Beatlayer's own)

**Clips (WP-6.36; R33 WP-6.36b)**

- **R24.** A track's Arrangement clips are those in the `Clips` timelines of the Arrangement's lanes scoped to it by `track`, at any depth of `Lanes`. A `Notes`, `Audio` or `Warps` timeline that sits in a track's lanes outside any clip becomes one clip from the timeline's start: a MIDI clip as long as its notes, rounded up to the next bar as a MIDI file's clip is ([midi-clips.md](midi-clips.md) R17), or an audio clip as long as its file plays. (S2, S4, S13)
- **R25.** A track's first `Clips` timeline is its Arrangement lane. On an audio track each further `Clips` timeline becomes a take lane under it, in order, named after the timeline's `name` or "Take" and its number; on a MIDI track their clips are left out and listed. (S2; Beatlayer's own)
- **R26.** A track's clips are placed in order of their start, then the file's order, each replacing what it covers of the clips placed before it, as a drop does ([arrangement-editing.md](arrangement-editing.md) R6); each clip shortened, split or removed so is counted in the report. (S13; Beatlayer's own)
- **R27.** A clip starts at its `time` and lasts its `duration`; without one, `playStop` less `playStart`, and without those, its content's length. Its `playStart` is its Start marker and its `playStop` its End marker. It loops when it has both `loopStart` and `loopEnd` with the end after the start, and they are its loop brace; otherwise its loop is off. A clip whose `enable` is false is switched off (its activator), and its `name` is its name. (S2, S4, S13)
- **R28.** A note starts at its `time` and lasts its `duration`, in its timeline's unit, counted from the clip's content start; its `key` is its pitch, its velocity is `vel` × 127 rounded and kept from 1 to 127 (100 without `vel`), and its release velocity `rel` × 127 the same way (64 without `rel`). A note whose key is outside 0 to 127, whose length isn't above 0, or that starts before its content's start is left out and counted. A note's `channel` isn't kept, and the report says so once per track whose notes use more than one channel. (S2, S4, S13)
- **R29.** An audio clip whose content is an `Audio` timeline, with no warping, is unwarped: its Start and End markers and loop brace are seconds in its file. (S2, S4, S13)
- **R30.** An audio clip whose content is a `Warps` timeline with `contentTimeUnit` seconds holding an `Audio` timeline is warped: each `Warp` event becomes a warp marker, its `time` the beat in the clip's contents and its `contentTime` the time in the file ([warping.md](warping.md) R2), and the clip's tempo is the last segment's; its markers and loop brace are beats of its contents. A `Warps` with fewer than two events, or whose events don't rise in both times, is read as an unwarped clip of its `Audio` and listed. (S2, S4, S13)
- **R31.** A warped clip's `algorithm` names its warp mode, compared ignoring case, spaces, hyphens and underscores: `repitch` is Re-Pitch, and `beats`, `tones`, `texture`, `complex` and `complexpro` are those modes. Any other name, or none, takes the Default Warp Mode setting, and each other name is listed once with the mode it took. (S2, S4; [warping.md](warping.md))
- **R32.** An unwarped audio clip in the Arrangement whose loop would play more than once in its length becomes one unlooped clip per pass, back to back over the clip's length: the first from its Start marker to its loop's end, each later one over the loop brace, the last cut at the clip's end. It's listed, since only warped clips loop in the Arrangement. A Session clip keeps its loop. (S4, S13; Beatlayer's own)
- **R33.** A clip's `fadeInTime` and `fadeOutTime`, in its `fadeTimeUnit` (else its content's unit), become its fades in seconds, each limited to the clip's length. A negative `fadeInTime` makes a crossfade with the clip before it on the lane: the clip still starts at its `time`, the clip before it ends there, and the two meet in a crossfade as long as the negative value is large, as the Arrangement's crossfades between adjacent clips are ([audio-clips.md](audio-clips.md), WP-6.51). A MIDI clip's fades are ignored. (S2, S4, S13)
- **R34.** A clip whose content is a `Clips` timeline is flattened. On a MIDI track the inner clips' notes are merged into one MIDI clip with the outer clip's place, markers and loop, each inner clip's notes placed where it starts and cut to its region. On an audio track each inner clip becomes an audio clip of its own, at the place the outer clip plays it, cut to the outer clip's region, and repeated for each pass of the outer clip's loop. Deeper nesting flattens level by level. Each flattened clip is listed. (S1, S4; Beatlayer's own)
- **R35.** A clip with a `reference` plays a copy of the timeline it names, as if that timeline were its own content; the copy is independent of the original, and the report counts the alias clips made copies. A `reference` to something that isn't a timeline leaves the clip out (R7). (S2, S4)
- **R36.** A clip whose content is a `Lanes` takes its notes or its audio from the first `Notes`, `Audio` or `Warps` timeline in it, and its `Points` as automation (R59 to R62); further note or audio timelines in it are left out and listed. `Video` timelines, and tracks whose only content is video, are left out and listed. (S2, S4; Beatlayer's own)
- **R37.** A clip on a track that can't hold clips (a group, a return or Main) is left out. An audio clip on a MIDI track goes to the audio track R17 makes beside it, made for it when there's none; notes on an audio track are left out. Each is listed. (S13; Beatlayer's own)

**Media (WP-6.36)**

- **R38.** A `File`'s `path` is inside the container when `external` is false or absent, with `/` or `\` as the separator. With `external` true it's on disk: absolute when it starts with `/`, `\\` or a drive letter, otherwise relative to the folder that holds the `.dawproject` file. (S2, S4, S5)
- **R39.** Each embedded file the set uses is written to the Temporary Folder ([audio-clip-editing.md](audio-clip-editing.md) R5), under `Imported/<the file's name without .dawproject>/`, keeping its path in the container, and joins the set's media by its content hash; files with the same bytes are one medium. The first save moves each into the project folder's `Samples/Imported/`, under a free name, and points the media table at it, as processed files move. (S1, S13; Beatlayer's own)
- **R40.** A referenced file is used where it is, as an imported sample is, and Collect All and Save copies it into the project ([file-management.md](file-management.md) R4). A referenced file that isn't at its path is looked for by its name in the `.dawproject` file's folder and the folders inside it, the first match by name being used, and listed. A file found nowhere leaves out every clip that plays it, each listed with the path. (S4, S5, S13; Beatlayer's own)
- **R41.** A file Beatlayer imports ([audio-import.md](audio-import.md) R1) plays as it decodes; a file of another format, or one that doesn't decode, leaves out the clips that play it, listed. An `Audio` element's `sampleRate`, `channels` and `duration` aren't used: the file's own are, and where they disagree (a different rate or channel count, or a duration more than 1 ms out) the file is listed once. (S2, S4, S13)

**The Session View (WP-6.36)**

- **R42.** Each `Scene` becomes a scene, in the file's order, with its name. Each `ClipSlot` in it scoped to an audio or MIDI track is that track's slot in the scene, holding its `Clip` read as R27 to R36 read clips; a slot whose `hasStop` is false has its Clip Stop button removed. A slot for a track that can't hold clips, or a second slot for one track in one scene, is left out and listed. A file with no scenes gets the eight empty scenes a new set has ([session-view.md](session-view.md) R1). (S2, S4, S13)
- **R43.** Imported clips take a new clip's launch settings, and imported scenes have no tempo or signature of their own, since the format carries neither. (S2)

**Devices (WP-6.36b)**

- **R44.** A channel's `Devices` become its track's chain in the file's order, each as R46 to R54 say, with its role from `deviceRole`: `noteFX` a MIDI effect, `instrument` an instrument, `audioFX` and `analyzer` audio effects. A chain keeps its MIDI effects first, then one instrument, then its audio effects ([device-chains.md](device-chains.md) R2, R3): a device out of that order moves to its role's place, keeping the file's order within each role; a second instrument, and a MIDI effect or instrument on an audio track, group or return, is left out. Each is listed. (S2, S13)
- **R45.** A device whose `Enabled` is false, or whose `loaded` is false, is switched off, the latter listed. A device's `name` is the name its title bar shows. (S2, S4, S13)
- **R46.** A plug-in is found among the plug-ins scanned on this machine by its format and ID ([clap-hosting.md](clap-hosting.md), [vst3-hosting.md](vst3-hosting.md), [au-hosting.md](au-hosting.md)). One that isn't found becomes a device that keeps its ID and its state, passing its input through, as a set's missing plug-in does ([clap-hosting.md](clap-hosting.md) R12), and is listed. A found plug-in is loaded and given its state as a set's is ([clap-hosting.md](clap-hosting.md) R9, [vst3-hosting.md](vst3-hosting.md) R9). (S2, S4, S13)
- **R47.** A `ClapPlugin`'s `deviceID` is its CLAP ID, and its `State` file's bytes (`.clap-preset`) are the state stream the plug-in saved, handed to it unchanged. (S1, S4, S8)
- **R48.** A `Vst3Plugin`'s `deviceID` is its class ID, compared ignoring case and dashes. Its `State` is a `.vstpreset` file: its `Comp` chunk is the component's state and its `Cont` chunk, where there is one, the controller's. A preset whose class ID isn't the device's, or that doesn't read, leaves the plug-in at its defaults, listed. (S4, S6, S12)
- **R49.** An `AuPlugin`'s `deviceID` names its type, subtype and manufacturer, four characters each, joined by `/` or `:`; its `State` is a property list, XML or binary (`.aupreset`), which becomes the unit's state as the set keeps it. On Windows and Linux the device is kept unhosted, as a set's Audio Unit is there ([au-hosting.md](au-hosting.md) R14). An ID that doesn't read leaves the device out, listed. (S4, S7, S13)
- **R50.** A `Vst2Plugin` is left out and listed, as Beatlayer hosts no VST2 (D10). A `Device` or `BuiltinDevice`, the generic devices of R51 to R54 among them, whose `deviceID` is one of Beatlayer's own device types and whose `State` is a Beatlayer device or rack preset (`.blpreset`) becomes that device or rack, loaded from the preset with its chains and samples as a preset loads, and its generic parameters aren't read. Any other `Device` or `BuiltinDevice` is left out, listed with its `deviceName` and `deviceVendor`. (S2, S4, S12)
- **R51.** A `Compressor` becomes a Compressor: `Threshold` its Threshold; `Ratio` in `percent` the ratio 1 / (1 − p / 100), 100 % the largest ratio, and in any other unit the ratio as given; `Attack` and `Release` its Attack and Release; `OutputGain` its Out; `AutoMakeup` its Makeup. An `InputGain` other than 0 dB is listed, as the Compressor has none. ([compressor.md](compressor.md); S2, S4)
- **R52.** A `NoiseGate` becomes a Gate: `Threshold`, `Attack` and `Release` its own; `Range` its Floor. A `Ratio` under 100 % is listed, as the Gate has none. ([gate.md](gate.md); S2, S4)
- **R53.** A `Limiter` becomes a Limiter: its Gain is `InputGain` plus `OutputGain`, and its Ceiling `Threshold` plus `OutputGain`, in decibels, so it limits the same signal to the same level; `Release` its Release. An `Attack` is listed, as the Limiter has none. ([limiter.md](limiter.md); S2, S4)
- **R54.** An `Equalizer` becomes an EQ: its bands in `order` (else the file's order) take its first eight bands, the rest left out and listed; `highPass` a Low Cut at 12 dB per octave, `lowPass` a High Cut at 12 dB per octave, `lowShelf` a Low Shelf, `highShelf` a High Shelf, `bell` a Bell, `notch` a Notch; each band's `Freq`, `Gain`, `Q` and `Enabled` its own; the bands it doesn't use switched off; `OutputGain` its Gain. A `bandPass` band is switched off and listed, and an `InputGain` other than 0 dB is listed. ([eq.md](eq.md); S2, S4)
- **R55.** A generic device's value is converted from its `unit` to the Beatlayer parameter's (decibels and linear gain, seconds and milliseconds, a `normalized` value placed on the parameter's range) and clamped to its range; a value whose unit doesn't convert leaves the parameter at its default, listed. A plug-in's `Parameters` don't set its values, which its state holds; they name what its automation targets. (S2, S4)

**Automation (WP-6.36b)**

- **R56.** `Points` in the Arrangement's lanes outside clips drive the parameter their `Target` names, whichever lane holds them: a channel's `Volume`, `Pan` or `Mute` its track's mixer control, a send's `Volume` its send level, and a device's parameter that parameter. Points targeting something the import left out are left out with it; points on a target Beatlayer can't automate (a device's `Enabled`, a send's `Pan` or `Enable`) are left out and listed. ([automation.md](automation.md); S2, S4)
- **R57.** Each point's value is converted to the target's own units: a mixer control's as R21 and R22 read them, a generic device's as R55, and a plug-in parameter in `normalized` units through the loaded plug-in's own mapping from its normalized range to its values. A plug-in that didn't load keeps no automation, listed. The `Points`' `unit`, else its target's, is the values' unit. (S2, S4, S13)
- **R58.** A `linear` real point is a breakpoint whose segment runs straight to the next. A `hold` real point, and every boolean, integer and enumeration point, is a breakpoint holding its value, followed by a breakpoint with the same value one tick before the next point's, so the value steps there. Of two points at one tick, the later in the file wins. (S2, S4; [automation.md](automation.md))
- **R59.** `Points` targeting a parameter inside an Arrangement clip drive it over the clip's span on its track, the parts outside the clip left out. Inside a Session slot's clip they become that clip's automation envelope for the parameter ([session-automation.md](session-automation.md) R3). (S2, S4, S13)
- **R60.** `Points` with a `channelController` expression for controllers 1 to 31 and 33 to 119, a `pitchBend` or a `channelPressure` expression become the MIDI clip's controller envelopes ([midi-controller-data.md](midi-controller-data.md) R1): `normalized` values from 0 to 1 become 0 to 127, and pitch bend from 0 to 1 becomes −8,192 to 8,191 with 0.5 at rest. Points outside clips in a MIDI track's lanes are copied into the envelopes of the clips they overlap, each clip starting with the value in force at its start; what falls outside every clip is left out and listed. Other controllers, `programChange`, and values for one controller on a second channel are left out and listed. (S2, S4, S5, S13)
- **R61.** A `polyPressure` expression with a `key` becomes the pressure of the notes of that key it overlaps ([midi-controller-data.md](midi-controller-data.md) R2). Inside a `Note`, a `pressure` expression becomes the note's pressure, from 0 to 1 onto 0 to 127; a `transpose` expression, in semitones, its own pitch bend, and a `timbre` expression its slide (MPE in clips, WP-6.17); `gain`, `pan` and `formant` expressions are left out and listed. (S2, S4, S13)
- **R62.** Inside an audio clip, a `gain` expression with one value sets the clip's gain, and a `transpose` expression with one value sets its Transpose and Detune, rounded to the nearest cent; an expression that changes over the clip, and any other expression, is left out and listed. (S1, S2, S4)

**Markers and metadata (WP-6.36b)**

- **R63.** Each `Marker` of the Arrangement's `Markers`, or of a `Markers` timeline in its lanes, becomes a locator (WP-6.24) at its time with its name; of two at one tick, the first is kept. Markers' colours and comments aren't kept. (S2, S4)
- **R64.** `metadata.xml` isn't kept; the report lists the fields it held. (S3)

**Writing a file (WP-6.37)**

- **R65.** `set.export_dawproject` writes the open set to the file at `path` as DAWproject 1.0, `.dawproject` added when it's missing. It refuses when a file is already at `path` unless `replace` is true, and it's in the `external` class. The plug-ins' states are taken from the running plug-ins, as a save takes them, and the file is written on a worker thread into a hidden file beside the destination, then renamed into place, so the destination holds the whole file or nothing new; the set doesn't change. It returns the file's path and the export report (R81). (S1, S12)
- **R66.** The container holds `metadata.xml`, then `project.xml`, both UTF-8 without a byte-order mark and deflated, then the plug-in states in `plugins/` and the media in `audio/`, each stored without compression. Each element that is referenced has an `id` of `id` and a number, numbered in the order elements are written. Every entry carries the same time stamp, 1 January 1980, so the same set writes the same bytes each time. (S1, S2, S5; Beatlayer's own)
- **R67.** `project.xml` names the format version `1.0` and the application `Beatlayer` with its version. `Structure` holds each top-level track in order, a group as a `Track` with `contentType` `tracks` holding its tracks and a `submix` channel; then the returns, each a `Track` with an `effect` channel; then Main, last, as a `Track` with a `master` channel. An audio track's `contentType` is `audio` and a MIDI track's `notes`. (S1, S2)
- **R68.** Each channel writes its track's volume as `linear` gain (`min` 0, `max` the fader's top), its pan as `normalized` (0 hard left, 1 hard right), `Mute` on while its Track Activator is off, `solo`, and `audioChannels` 2. Its `destination` is its group's channel, Main's, or the channel of the track whose Track In it feeds; an output to Sends Only or Ext. Out writes none, and the export report lists it. (S1, S2, S13)
- **R69.** Each send above −∞ dB writes a `Send` to its return's channel, its level as `linear` gain, `type` `pre` or `post` from the return's Pre/Post switch, enabled. (S2, S13)
- **R70.** A MIDI clip writes a `Clip` with its `time`, `duration`, `playStart`, `playStop` and, while it loops, `loopStart` and `loopEnd`, in beats; `enable` false while it's off; its name; and a `Notes` timeline. Each note writes its `time` and `duration` in beats from the clip's content start, its `key`, `vel` and `rel` as its velocities over 127, and `channel` 0. Muted notes are left out; notes' chance and velocity deviation aren't written; both are counted in the export report. (S2, S4, S13)
- **R71.** An unwarped audio clip writes a `Clip` with `contentTimeUnit` seconds holding its `Audio`; a warped one writes a `Clip` with `contentTimeUnit` beats holding a `Warps` (`contentTimeUnit` seconds) with one `Warp` per warp marker, and a second, from the clip's tempo, when it has one marker. Its warp mode writes the `algorithm` named as R31 reads it (`complex pro` for Complex Pro). Its fades write in seconds, a crossfade with the clip before it writes as a negative `fadeInTime` of the crossfade's length (R33 read in reverse), and a gain other than 0 dB writes a `gain` expression with one point. Its `Audio` names the file with the file's own sample rate, channels and length. (S2, S4, S13)
- **R72.** An audio track's take lanes write as further `Clips` timelines in its lanes, after its Arrangement lane, each with its name. (S2; Beatlayer's own)
- **R73.** Every media file the set uses is embedded in `audio/` under its own name, a clash getting " 2", " 3" and so on before its extension, unless `embed_media` is false, when each is written as an `external` reference by its absolute path. A media file that's missing leaves out the clips that play it, listed. (S1, S4)
- **R74.** Each scene writes a `Scene` with its name, holding for each audio and MIDI track a `ClipSlot` scoped to it, with `hasStop` false where the slot's Clip Stop button is removed and its clip where it holds one, written as R70 and R71 write clips. (S2, S4, S13)
- **R75.** The Transport writes the tempo at 1.1.1 in `bpm` and the signature at 1.1.1. The Arrangement's tempo automation writes a point at each segment's start, `hold` for a steady segment and `linear` for a ramp, with a point holding a ramp's end value at the next segment's start where the tempo jumps there; each signature change writes a time-signature point. (S2, S4, S12)
- **R76.** Each automation envelope writes `Points` in its track's lane targeting the parameter, listed in its device's `Parameters` where the format asks: a breakpoint with a straight segment writes a `linear` point; a curved segment writes as straight segments at every sixteenth note, and more often where the curve moves more than 1 % of the parameter's range between them; two breakpoints one tick apart or less, where the value steps, write as a `hold` point. Mute writes boolean points. A plug-in parameter writes in `normalized` units through the plug-in's own mapping. A Session clip's automation envelopes write inside its clip; its MIDI controller envelopes and its notes' pressure write as the expressions R60 and R61 read. Clip modulation envelopes aren't written, listed. (S2, S4, S13)
- **R77.** A CLAP plug-in writes a `ClapPlugin` with its ID, name, vendor and version and its state as `plugins/<n>.clap-preset`, the state stream unchanged; a VST3 plug-in a `Vst3Plugin` with its class ID as a UUID and its state as a `.vstpreset` holding its `Comp` and `Cont` chunks; an Audio Unit an `AuPlugin` whose ID is its type, subtype and manufacturer joined by `/`, its state as an `.aupreset` property list. An LV2 plug-in isn't written, listed. Each writes its `deviceRole`, `Enabled`, and in `Parameters` the parameters it automates. (S2, S4, S6, S7, S8)
- **R78.** A Compressor, Gate, Limiter or EQ writes the matching generic device, its values mapped as R51 to R54 read them in reverse (a Low Cut or High Cut at 48 dB per octave as `highPass` or `lowPass`, listed), with its own preset (`.blpreset`) as its `State`. Every other native device, and every rack with everything in it, writes a `BuiltinDevice` whose `deviceID` is its type, `deviceName` its name, `deviceVendor` `Beatlayer` and `State` its preset, which R50 reads back; the export report lists each, as other applications leave them out. (S2, S4; Beatlayer's own)
- **R79.** Each locator writes a `Marker` with its time in beats and its name, in the Arrangement's `Markers`. `metadata.xml` writes the set's name as its `Title`. (S2, S3)
- **R80.** What the format can't carry isn't written, and the export report lists it once per kind and place: clips' launch settings and follow actions, scenes' tempo, signature and follow actions, warp modes' settings, clip modulation envelopes, crossfader assignments, track delays, inputs, monitoring and arm, MIDI To, sidechains, and Ext. Out. (S1, S2; Beatlayer's own)
- **R81.** The export report has the form of the import report (R12), and `set.export_dawproject` returns it. (Beatlayer's own)
- **R82.** A set written by `set.export_dawproject` and read back by `set.import_dawproject` is the same set in everything the format carries: tracks, groups, returns and their order, names, mixer values, sends and outputs, clips with their places, Start and End markers, loops, notes, velocities, gains, fades, crossfades and warp maps, the colours of tracks, clips and scenes, take lanes, scenes and slots, tempo and signatures, automation (a curved segment within 1 % of the parameter's range), devices and plug-ins with their states, and locators. What changes is listed in the export report. (S1; Beatlayer's own)

**In the app (WP-6.37b)**

- **R83.** Open Beatlayer Set... (M-FILE-002) lists `.dawproject` files beside Beatlayer sets, and choosing one runs `set.import_dawproject` for it, after asking whether to save changes as opening a set does. (S9, S11; Beatlayer's own)
- **R84.** The browser lists `.dawproject` files beside Beatlayer sets ([browser.md](browser.md) R6). Enter or a double-click on one imports it, as Enter or a double-click on a set opens the set, each asking first whether to save changes. (S9, S13)
- **R85.** A `.dawproject` file dropped anywhere on the main window (I-DROP-025) is imported as R83 says; no view places it as a clip. (Beatlayer's own)
- **R86.** File ▸ Export DAWproject..., straight after Export MIDI Clip... (M-FILE-016), opens a save dialog with the set's name and `.dawproject`, and runs `set.export_dawproject` with the chosen path, with `replace` once the dialog has asked about a file already there. It has no key. (S11; Beatlayer's own)
- **R87.** After an import or export whose report isn't empty, a window titled "DAWproject Import" or "DAWproject Export" lists the report's entries under Left Out and Changed, each with where, what and how many, with a Copy button that puts the report on the clipboard as text and a Close button. Assistive technology reads it as a list. With an empty report no window opens, and the Status Bar says the file was imported or exported. (Beatlayer's own)
- **R88.** `set.import_dawproject` and `set.export_dawproject` are registered commands in the `external` class, and `set.import_report` in the `read` class, each with a schema and a description, run by ID through the agent API. The import's effect reads back in the query tool's `song` (`set.name` and the tracks), and the export names nothing to read back, since it writes a file outside the set. (S12)

## Tests

The fixtures are written by `tests/fixtures/gen/gen_dawproject.py` into `tests/fixtures/dawproject/`, from the script's own XML, its own generated audio and the test plug-ins' own states; none is a file another application exported. Each fixture has the report it should give beside it.

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-6.36 | unit |
| R2 | to be written in WP-6.36 | unit |
| R3 | to be written in WP-6.36 | unit |
| R4 | to be written in WP-6.36 | unit, fuzz |
| R5 | to be written in WP-6.36 | unit, fuzz |
| R6 | to be written in WP-6.36 | unit, fuzz |
| R7 | to be written in WP-6.36 | unit |
| R8 | to be written in WP-6.36 | unit |
| R9 | to be written in WP-6.36 | unit, property |
| R10 | to be written in WP-6.36 | unit, agent test |
| R11 | to be written in WP-6.36 | unit |
| R12 | to be written in WP-6.36 | unit, agent test |
| R13 | to be written in WP-6.36 | unit |
| R14 | to be written in WP-6.36 | unit |
| R15 | to be written in WP-6.36 | unit |
| R16 | to be written in WP-6.36 | unit |
| R17 | to be written in WP-6.36 | unit |
| R18 | to be written in WP-6.36 | unit |
| R19 | to be written in WP-6.36 | unit, golden render |
| R20 | to be written in WP-6.36 | unit |
| R21 | to be written in WP-6.36 | unit |
| R22 | to be written in WP-6.36 | unit |
| R23 | to be written in WP-6.36b (reading) and WP-6.37 (writing) | unit |
| R24 | to be written in WP-6.36 | unit |
| R25 | to be written in WP-6.36 | unit |
| R26 | to be written in WP-6.36 | unit |
| R27 | to be written in WP-6.36 | unit |
| R28 | to be written in WP-6.36 | unit |
| R29 | to be written in WP-6.36 | unit, golden render |
| R30 | to be written in WP-6.36 | unit, golden render |
| R31 | to be written in WP-6.36 | unit |
| R32 | to be written in WP-6.36 | unit, golden render |
| R33 | to be written in WP-6.36b | unit |
| R34 | to be written in WP-6.36 | unit |
| R35 | to be written in WP-6.36 | unit |
| R36 | to be written in WP-6.36 | unit |
| R37 | to be written in WP-6.36 | unit |
| R38 | to be written in WP-6.36 | unit |
| R39 | to be written in WP-6.36 | unit |
| R40 | to be written in WP-6.36 | unit |
| R41 | to be written in WP-6.36 | unit |
| R42 | to be written in WP-6.36 | unit |
| R43 | to be written in WP-6.36 | unit |
| R44 | to be written in WP-6.36b | unit |
| R45 | to be written in WP-6.36b | unit |
| R46 | to be written in WP-6.36b | unit |
| R47 | to be written in WP-6.36b | unit |
| R48 | to be written in WP-6.36b | unit |
| R49 | to be written in WP-6.36b | unit |
| R50 | to be written in WP-6.36b | unit |
| R51 | to be written in WP-6.36b | unit |
| R52 | to be written in WP-6.36b | unit |
| R53 | to be written in WP-6.36b | unit, golden render |
| R54 | to be written in WP-6.36b | unit |
| R55 | to be written in WP-6.36b | unit |
| R56 | to be written in WP-6.36b | unit |
| R57 | to be written in WP-6.36b | unit |
| R58 | to be written in WP-6.36b | unit |
| R59 | to be written in WP-6.36b | unit |
| R60 | to be written in WP-6.36b | unit |
| R61 | to be written in WP-6.36b | unit |
| R62 | to be written in WP-6.36b | unit |
| R63 | to be written in WP-6.36b | unit |
| R64 | to be written in WP-6.36b | unit |
| R65 | to be written in WP-6.37 | unit, agent test |
| R66 | to be written in WP-6.37 | unit |
| R67 | to be written in WP-6.37 | unit |
| R68 | to be written in WP-6.37 | unit |
| R69 | to be written in WP-6.37 | unit |
| R70 | to be written in WP-6.37 | unit |
| R71 | to be written in WP-6.37 | unit |
| R72 | to be written in WP-6.37 | unit |
| R73 | to be written in WP-6.37 | unit |
| R74 | to be written in WP-6.37 | unit |
| R75 | to be written in WP-6.37 | unit |
| R76 | to be written in WP-6.37 | unit |
| R77 | to be written in WP-6.37 | unit |
| R78 | to be written in WP-6.37 | unit |
| R79 | to be written in WP-6.37 | unit |
| R80 | to be written in WP-6.37 | unit |
| R81 | to be written in WP-6.37 | unit, agent test |
| R82 | to be written in WP-6.37 | property, golden render |
| R83 | to be written in WP-6.37b | UI script |
| R84 | to be written in WP-6.37b | UI script |
| R85 | to be written in WP-6.37b | UI script |
| R86 | to be written in WP-6.37b | UI script |
| R87 | to be written in WP-6.37b | UI script |
| R88 | to be written in WP-6.36, WP-6.37 | unit, agent test |
