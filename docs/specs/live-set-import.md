# Live set import: `.als` sets, `.adv` and `.adg` presets and `.agr` grooves

- **Status:** draft
- **Work packages:** WP-9.14 (reading the file, the set's structure, tracks, groups, returns, names, colours, tempo, signatures, locators, the report and refusals: R1 to R16); WP-9.15 (the mixer and routing: R17 to R24); WP-9.16 (MIDI clips, Session slots and scenes, launch settings and follow actions: R25 to R32); WP-9.17 (audio clips, their samples, warping, fades, clip envelopes and grooves: R33 to R42); WP-9.18 (device chains, plug-ins, racks, Drum Racks and the placeholder: R43 to R51); WP-9.21 (Arrangement automation, take lanes and comps, linked tracks: R52 to R57); WP-9.22 (`.adv`, `.adg` and `.agr` files and the app's import paths: R58 to R65); WP-9.25 (video clips: R66). What each of Live's devices becomes is [Live device mapping](live-device-mapping.md)'s; grooves in the pool are [grooves](grooves.md)'; the report window is [DAWproject](dawproject.md) R87's
- **Parity-matrix row:** Files and export: Live set import (`.als`); Live device and rack preset import (`.adv`, `.adg`)
- **Checked against:** the Live 12 reference manual (online edition), chapter 5 "Managing Files and Sets", read 30 October 2026

## The behaviour

A person opens a Live set (`.als`) from the Open dialog, the browser, or by dropping it on the window, and Beatlayer opens what it holds as a new, unsaved set, then shows a report of everything it left out or changed. An agent does the same through `set.import_live_set`. Live's device presets (`.adv`), rack presets (`.adg`) and grooves (`.agr`) load the same way into the open set: a preset onto the selected track as a device or rack, a groove into the Groove Pool.

A Live set is gzipped XML. Beatlayer reads it into its own model of the parts it understands, then builds a Beatlayer set from that: the tracks with their groups and returns, the mixer and routing, the Arrangement's and the Session View's clips, warping, automation, take lanes, plug-ins with their states, racks and Drum Racks, and each of Live's own devices mapped to its Beatlayer equivalent. Nothing is left out or changed without an entry in the report. The format is learned only from files Live saves, read as its output.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 5 "Managing Files and Sets", sections 5.1 to 5.4 and 5.7 | A Live set holds its tracks, clips, devices and their settings, and refers to samples by location; device presets are `.adv` files, rack presets `.adg` files, grooves `.agr` files; a set whose samples have moved can be relinked with the File Manager's search |
| S2 | Measurement | Sets, presets and grooves saved by Live 12.4.3 on macOS from new sets built for each part of this spec, read as gzipped XML output; the element and attribute names, value forms and units each rule relies on are recorded in the tables of this spec and of [Live device mapping](live-device-mapping.md) | How each part is stored |
| S3 | Spec | [DAWproject](dawproject.md) R4 (reading past what isn't known), R11 (opening isn't an undo step), R83 to R88 (the app's import paths, the report window and the commands); [musical time](musical-time.md) (705,600 ticks a quarter note); [themes](themes.md) (the clip and track swatches); [file management](file-management.md) (relinking) | How an importer behaves in Beatlayer |
| S4 | Beatlayer | The architecture's "Project files and interop" (Live sets read with `flate2` and `quick-xml` into `bl-interop`) and "Testing" (the importer is fuzzed); CONTRIBUTING.md, "Building without Ableton's code"; "Agent control" (D17) | Where the importer lives and how it's tested; that no file made by Live enters the repository |

## Rules

### Reading the file (WP-9.14)

- **R1.** A Live set is a gzip stream holding UTF-8 XML whose root element is `Ableton`. A file that isn't gzip, or whose XML has another root, is refused as not a Live set; a stream cut short is read as far as it goes and the rest is listed as missing in the report. (S2)
- **R2.** The root's version attributes name the Live version that saved the set. Sets saved by Live 10.0 to 12.4 are read; an older set is refused, naming its version; a newer 12.x or later set is read like 12.4, and what isn't known goes to the report. (S2; the range is Beatlayer's own)
- **R3.** The XML is read as a stream, so a large set never needs its whole text in memory twice. An element or attribute Beatlayer doesn't know is skipped and listed once with how many times it occurs; an element missing something a part needs (a clip without its time, a note without its key) is left out with what it holds, and listed. No damaged or truncated file crashes Beatlayer. (S3, S4)
- **R4.** Live's IDs are unique only within a set and within their kind; Beatlayer gives every imported part new IDs of its own and keeps a table from Live's to Beatlayer's while it imports, so references between parts (routings, envelopes' targets, macro mappings, linked clips) survive. (S2; Beatlayer's own)
- **R5.** Times in beats become ticks exactly, at 705,600 ticks a quarter note; a time that isn't a whole number of ticks rounds to the nearest and is listed once with the largest change. Times in seconds stay in seconds where Beatlayer keeps seconds (warp markers' sample times, track delay). (S2, S3)
- **R6.** Opening an imported set isn't an undo step; the set is new and unsaved, named after the file, and closes the open set as opening a set does, after asking whether to save changes. (S3)

### Structure (WP-9.14)

- **R7.** Audio, MIDI, group and return tracks become tracks of the same kind in Live's order, with their names, colours, fold states and heights; a group's tracks go inside it, groups nested to any depth. The Main track becomes Main. (S2)
- **R8.** A colour is set to the swatch of Beatlayer's palette nearest to Live's colour index's colour, by its index where the palettes share one ([themes](themes.md)). (S2, S3)
- **R9.** The set's tempo, its time signature, its tempo and signature changes, the global launch quantisation, the groove amount, the loop brace and its switch, the song's start position and the locators with their names come across as they are. (S2)
- **R10.** The report lists, under Left Out and Changed, every part not imported or imported differently, with where it was (track, clip, device), what, and how many. (S3)
- **R11.** `set.import_live_set` (`external`, as it reads a file outside the set) imports the file at a path; `set.import_report` (`read`) gives the last import's report. The import reads back through the query tool's `song`. (S3, S4)
- **R12.** A Live set is fuzzed: the fuzz target reads arbitrary bytes as a gzipped set and as plain XML, and every input either imports or is refused with a reason. (S4)
- **R13.** Test sets are generated by `tests/fixtures/gen/gen_live_set.py` from Beatlayer's own description of each part, never copied from Live; no file saved by Live is committed. `BL_ALS_CORPUS` names a folder outside the repository whose sets an opt-in test imports, checking only that none crashes. (S4)
- **R14.** Samples, presets and grooves that belong to Live's own libraries are never imported as content: a set using one keeps its name and is listed. (S4)
- **R15.** Import runs off the main thread with a progress window and Cancel; Cancel leaves the open set as it was. (Beatlayer's own)
- **R16.** A set saved by Live as a project keeps its folder: samples are looked for relative to it first (R34). (S1)

### The mixer and routing (WP-9.15)

- **R17.** Each track's volume, pan (or split stereo pan with its two values), mute (from the Track Activator), solo, arm, monitoring (In, Auto, Off), crossfader assignment (A, none, B) and track delay with its unit come across; values are converted by the laws in this spec's table of stored forms, volume from Live's stored linear gain to decibels exactly. (S2)
- **R18.** Each send's level comes across to the matching return, by the order of the returns; each return's send mode, pre or post, comes across. A send to a return that isn't there is dropped and listed. (S2)
- **R19.** Main's volume, pan, crossfader position and the Cue volume, and each return's settings, come across. (S2)
- **R20.** A track's input and output routings come across by kind and name: another track by its imported ID, an audio interface channel by its index, a MIDI port by its name, a sidechain by its source track and tap. A routing to a port or track that isn't there keeps its name in the set, as Beatlayer keeps a missing port, and is listed. (S2)
- **R21.** Values outside Beatlayer's ranges are clamped to the nearest and listed. (Beatlayer's own)
- **R22.** A set of every mixer and routing setting imports to a set that renders the same as the set built by hand in Beatlayer. (Beatlayer's own)
- **R23.** Live's mixer settings Beatlayer doesn't have are listed. (Beatlayer's own)
- **R24.** The volume and pan laws convert so the imported set sounds as Live's set does: the gain at each fader position, and the pan law of [mixer](mixer.md), with an error under 0.01 dB. (S2)

### MIDI clips and the Session View (WP-9.16)

- **R25.** A MIDI clip's notes come across with their key, start, length, velocity, release velocity, chance, velocity deviation, deactivation, and probability groups; per-note expressions (pitch bend, slide and pressure) with their envelopes. Live's note IDs and probability groups become Beatlayer's in one pass. (S2)
- **R26.** A MIDI clip's controller envelopes (CC, pitch bend, channel pressure) come across as clip envelopes. (S2)
- **R27.** A clip's name, colour, loop, markers, signature, groove, Clip Scale and launch settings come across. (S2)
- **R28.** Each track's slots come across in scene order, with their clips and stop buttons; each scene with its name, colour, tempo and signature. (S2)
- **R29.** Launch modes, launch quantisation, Legato, velocity amount and follow actions (both actions, their chance, linked or unlinked time, Jump targets) come across; a scene's follow actions too. (S2)
- **R30.** Imported clips play the same notes on the same frames as the set built by hand in Beatlayer, at any block size. (Beatlayer's own)
- **R31.** Follow actions and launch settings behave as in the set built by hand. (Beatlayer's own)
- **R32.** The Session View's crossfader, the Main column's scene stop button and the clip view options of each clip come across where Beatlayer has them, and are listed where it doesn't. (S2)

### Audio clips (WP-9.17)

- **R33.** An audio clip comes across with its name, colour, gain, transpose, detune, loop and markers, fades (and the Arrangement's crossfades), RAM and HiQ, Warp on or off, warp mode with each mode's settings, warp markers, Lead or Follow, reverse, and clip envelopes on targets Beatlayer maps (R54). (S2)
- **R34.** A sample is found by its absolute path, then its path relative to the set, then the File Manager's search by name and size ([file management](file-management.md)); one still missing is kept as a missing file in the set and listed. (S1, S2, S3)
- **R35.** Warp markers' sample times in seconds and beat times in beats become Beatlayer's warp map exactly ([warping](warping.md) R1 to R11). (S2)
- **R36.** A warped clip in every warp mode renders within its warp mode's golden tolerance of the set built by hand in Beatlayer. (Beatlayer's own)
- **R37.** A set's grooves come into its Groove Pool, each clip keeping its groove ([grooves](grooves.md) R40). (S2)
- **R38.** An audio clip whose sample lies on another drive keeps its absolute path. (S2)
- **R39.** Clip envelopes on a clip's mixer, sends and device parameters come across where the target is mapped; others are listed. (S2)
- **R40.** A Session audio clip's launch settings come across as a MIDI clip's do (R29). (S2)
- **R41.** Sample edits Live keeps only in its analysis files aren't read; Beatlayer analyses each sample itself. (S4)
- **R42.** A clip whose sample is a video comes across as an audio clip until video clips are built, then as R66 says. (S2)

### Devices (WP-9.18)

- **R43.** Each track's devices come across in order with their names, activators and fold states; each of Live's own devices becomes its Beatlayer equivalent by [Live device mapping](live-device-mapping.md). (S2)
- **R44.** A VST3 or Audio Unit plug-in loads by its ID, with its saved state given to it as the plug-in saved it; a missing plug-in keeps its state, as a missing plug-in in Beatlayer does. (S2, S3)
- **R45.** A VST2 plug-in, which Beatlayer doesn't host, stays as a missing device with its name and its data kept, and is listed. (S2)
- **R46.** Instrument, Audio Effect, MIDI Effect and Drum Racks come across with their chains, chain names, colours and mixers, key, velocity and chain-select zones with their fades, Macro Controls with their names, values and mappings (each mapping's target, minimum and maximum), and macro variations. A mapping to a parameter that isn't mapped is kept unmapped and listed. (S2)
- **R47.** A Drum Rack's pads keep their notes, chains, choke groups, sends to its return chains and the return chains themselves. (S2)
- **R48.** One of Live's own devices with no Beatlayer equivalent becomes a placeholder device that keeps its place, Live's name for it and its stored data, passes audio through unchanged (or MIDI, for a MIDI effect), and is listed; a later import of the same set after Beatlayer maps the device maps it. (S2; the placeholder is Beatlayer's own)
- **R49.** A device's activator, its name if renamed and its fold state come across. (S2)
- **R50.** Max for Live devices aren't run; each becomes a placeholder named after it, unless [Live device mapping](live-device-mapping.md) maps it by name, and is listed. (S4)
- **R51.** Test VST3 and Audio Unit plug-ins load with the states the generated set holds. (Beatlayer's own)

### Automation, take lanes and linked tracks (WP-9.21)

- **R52.** Each Arrangement envelope comes across with its breakpoints and their curves, on the target the mixer or [Live device mapping](live-device-mapping.md) maps; an envelope whose target isn't mapped is listed. (S2)
- **R53.** Tempo and signature automation come across. (S2)
- **R54.** A curve's stored shape converts to Beatlayer's curved segment so the envelope's value at every tick is within 0.5 % of the parameter's range of Live's. (S2)
- **R55.** Take lanes come across with their names and clips, and the comp's regions on the main lane. (S2)
- **R56.** Linked tracks stay linked. (S2)
- **R57.** An imported set plays its automation, take lanes and comps as the set built by hand in Beatlayer, at any block size. (Beatlayer's own)

### Presets, grooves and the app (WP-9.22)

- **R58.** A Live device preset (`.adv`) loads as its mapped device with its settings, a rack preset (`.adg`) as its rack with its chains, macros and mapped devices, onto the selected track as a Beatlayer preset loads, one undo step each, and hot-swappable as Beatlayer's own presets are. (S1, S2)
- **R59.** A Live groove (`.agr`) loads into the Groove Pool with its notes' timing and velocities and its Base, Quantize, Timing, Random and Velocity values ([grooves](grooves.md) R39). (S1, S2)
- **R60.** Open Beatlayer Set... lists `.als` files beside Beatlayer sets, and choosing one runs `set.import_live_set`. (S3)
- **R61.** The browser lists `.als`, `.adv`, `.adg` and `.agr` files; Enter or a double-click on a set imports it, and on a preset or groove loads it as R58 and R59 say. (S3)
- **R62.** A `.als` file dropped on the main window imports as R60 says; an `.adv` or `.adg` dropped on a track or device chain loads there; an `.agr` dropped on the Groove Pool loads there. (S3)
- **R63.** After an import whose report isn't empty, the report window titled "Live Set Import" opens as [DAWproject](dawproject.md) R87's does. (S3)
- **R64.** `device.import_live_preset` (`edit`) loads a preset by path onto a track; `groove.import_live` (`edit`) loads a groove. Each names its readback. (S3, S4)
- **R65.** Preset and groove files are read by the same code as a set's devices and grooves. (Beatlayer's own)

### Video clips (WP-9.25)

- **R66.** A clip whose sample is a QuickTime movie comes across as a video clip ([video](video.md) R5) with its warping. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R16 | to be written in WP-9.14: `crates/interop/tests/live_set.rs` per generated fixture; `fuzz/fuzz_targets/live_set.rs`; `crates/agent/tests/live_set.rs` | interop test, fuzz, agent API |
| R17 to R24 | to be written in WP-9.15: interop tests and a render against the hand-built set | interop test, golden render |
| R25 to R32 | to be written in WP-9.16: renders null at block sizes 1, 64 and 512 | interop test, engine render |
| R33 to R42 | to be written in WP-9.17 | interop test, golden render |
| R43 to R51 | to be written in WP-9.18 with the test VST3 and Audio Unit plug-ins | interop test |
| R52 to R57 | to be written in WP-9.21 | interop test, engine render |
| R58 to R65 | to be written in WP-9.22 | shell test, UI script, agent API |
| R66 | to be written in WP-9.25 | interop test |
