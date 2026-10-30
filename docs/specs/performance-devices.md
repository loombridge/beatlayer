# Performance devices: Variations, Performer, Arrangement Looper and Prearranger

- **Status:** draft
- **Work packages:** WP-9.42 (what the four share, Variations and Performer: R1 to R18); WP-9.43 (Arrangement Looper and Prearranger: R19 to R30). Each is a bundled extension built with the SDK, reaching the set through the extension API to the set ([extensions](extensions.md) R38 to R45) and drawing through the extension device panels ([extensions](extensions.md) R32 to R37). A rack's own macro variations are [macros](macros.md)'
- **Parity-matrix row:** MIDI effects: Performance Pack (Performer, Variations, Arrangement Looper, Prearranger)
- **Checked against:** Ableton's Performance Pack page, read 30 October 2026

## The behaviour

Four devices help a person perform a set. Variations stores snapshots of the set's parameters, chosen by what to include, and recalls one from a clip, a scene or its own buttons, or morphs between two. Performer is a modular macro device: faders, crossfaders, buttons and dials, added, renamed and arranged by the person, each mapped to parameters across the set with a response curve, and laid out to mirror a hardware controller if wanted. Arrangement Looper loops the Arrangement around where it's playing, at one of four lengths switched on and off while it plays, to stretch a section live. Prearranger lays out where a clip will play before a performance: a source clip recorded once reappears at chosen places later in the set, so the structure fills as the person plays.

They take Live's names, with Beatlayer's own type IDs, look and laws. Everything they change in the set goes through registered commands, so it is undone, read back and reached by agents as a person's edits are.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton's Performance Pack page (ableton.com/en/packs/performance-pack) | Performer: a modular macro device of faders, crossfaders, buttons and dials that controls almost any part of the set and external hardware, whose controls can be added, renamed and rearranged or set up to mirror a MIDI controller, with a Macro Editor for response curves. Variations: snapshots of the set restored later, with chosen parts included or left out, recalled from a clip or a scene. Arrangement Looper: loops the current position in the Arrangement, with four loop lengths switched on and off, to extend a section. Prearranger: a source clip recorded once reappears at set points later in the set, filling the structure during the performance |
| S2 | Spec | [Macros](macros.md) (macro variations, for comparison); [transport](transport.md) (the loop brace and jumps); [session to arrangement](session-to-arrangement.md); [extensions](extensions.md) R38 to R45; [remote mapping](remote-mapping.md) | How Beatlayer keeps variations, loops and the set's structure; how extensions reach the set |

## Rules

### What they share (WP-9.42)

- **R1.** Each device is a bundled extension's component, listed among the MIDI effects (Variations, Performer and Prearranger) or the audio effects (Arrangement Looper, which passes audio through), with Live's name and presets in Beatlayer's own format. (S1; the categories are Beatlayer's own)
- **R2.** A device changes the set only through registered commands from the extension API to the set ([extensions](extensions.md) R38, R40): each change it makes reads back through the query tool, and is or isn't an undo step as each device's rules say. (S2)
- **R3.** Each device's state (its snapshots, controls, mappings and places) saves with the set and in its presets, and reads back headless through the query tool's `device` and `device.display`. (S2)
- **R4.** A target that goes away (a deleted track or device) is dropped from the device's state and listed in its display. (Beatlayer's own)

### Variations (WP-9.42)

- **R5.** Variations stores up to 16 snapshots. Storing one records the values of every parameter in its scope: the devices, mixers and sends of the tracks it's set to include, with plug-ins, Max-built devices' equivalents and modulation routings each included or left out by a switch. (S1; the count and switches are Beatlayer's own)
- **R6.** Recalling a snapshot sets every stored parameter to its value, as one undo step named after the snapshot. (S1; the undo step is Beatlayer's own)
- **R7.** A snapshot is recalled by its button, from a clip's launch (a clip whose name is `var <n>` on the device's track recalls snapshot *n* on its launch) or from a scene's launch (a scene named `var <n>`). (S1; the naming is Beatlayer's own)
- **R8.** Morph sets every stored parameter between two snapshots by its share, as a hand move does: a morph's movement is one gesture and one undo step. (Beatlayer's own)
- **R9.** A parameter held by a macro or automation follows the same rules a hand move does ([macros](macros.md), [automation](automation.md) R5). (S2)
- **R10.** Its display shows the snapshots as buttons, the last recalled lit, and the morph as a slider between two chosen snapshots. (Beatlayer's own)

### Performer (WP-9.42)

- **R11.** Performer holds up to 32 controls, each a fader, a crossfader, a button (momentary or toggle) or a dial, with a name and a place on a grid of eight columns. Adding, removing, renaming and moving controls are edits of the device, each one undo step. (S1)
- **R12.** Each control maps to up to eight parameters anywhere in the set, each with a minimum, a maximum and a response curve drawn in its Macro Editor (linear, exponential, logarithmic, S-curve, or drawn breakpoints). (S1)
- **R13.** Moving a control moves its targets as a macro moves its mappings: the targets follow at once, and the move is a gesture and one undo step. (S1, S2)
- **R14.** A crossfader's two ends each have their own target values, and its position blends between them. (S1)
- **R15.** Each control is a parameter of the device, automatable and mappable in MIDI Map Mode, so a controller drives it ([remote mapping](remote-mapping.md)). (S1, S2)
- **R16.** A control mapped to an external MIDI port sends a controller on it as it moves. (S1)
- **R17.** Its display draws the controls on their grid; a drag moves one, and in Edit mode places it. (S1)
- **R18.** A layout can mirror a controller template's controls ([control surfaces](control-surfaces.md) R39 to R48), placing one control per control of the template. (S1; Beatlayer's own)

### Arrangement Looper (WP-9.43)

- **R19.** Arrangement Looper has four loop switches, each with its own length (from 1/4 beat to 16 bars, 1 bar, 2 bars, 4 bars and 8 bars on a new device). Switching one on while the Arrangement plays sets the loop brace to that length starting at the last boundary of the length before the playhead, and switches the loop on, at the next quantisation step. (S1; the lengths and the boundary are Beatlayer's own)
- **R20.** Switching it off puts the loop brace and the loop switch back as they were before, at the next quantisation step. With two on, the shorter loops. (S1; Beatlayer's own)
- **R21.** The loop changes go through `transport.set_loop` and are not undo steps, as a performance move. (S2; Beatlayer's own)
- **R22.** Its quantisation is the global launch quantisation, or its own choice. (Beatlayer's own)
- **R23.** The loop lands on its frames at any block size. (S2)
- **R24.** Its display shows the four switches and the current loop against the bar. (Beatlayer's own)

### Prearranger (WP-9.43)

- **R25.** Prearranger holds a list of places in the Arrangement (each a song position) for its track's source clip. Recording into the source clip, when the recording ends, copies the clip to every place, as one undo step named Prearrange. (S1; the trigger is Beatlayer's own)
- **R26.** A place can be set from the playhead, typed, or set from the scenes: each scene the source clip's slot sits in becomes a place in the Arrangement as Session to Arrangement would lay it ([session to arrangement](session-to-arrangement.md)). (S1, S2; Beatlayer's own)
- **R27.** A copy is a linked clip of the source, so later edits to the source reach every place. (Beatlayer's own)
- **R28.** The copies go through `clip.create` and `clip.set_data`; each place that already holds a clip is skipped and listed. (S2)
- **R29.** Its display lists the places on a timeline of the set, the source's lit. (Beatlayer's own)
- **R30.** Clearing the places removes nothing from the set. (Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R18 | to be written in WP-9.42: tests under `extensions/performance/` and `crates/shell/tests/performance_devices.rs` | unit, shell test |
| R19 to R30 | to be written in WP-9.43: engine frames at block sizes 1, 61 and 512 | engine test, shell test |
