# MIDI Monitor, the display-only MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.32. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the display-data path of R35 to R38.
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 33 "Max for Live Devices", section 33.3.3 "MIDI Monitor", read on 18 October 2026

## The behaviour

MIDI Monitor shows what reaches it and changes nothing. Its Note display draws a keyboard with each key that is sounding, its velocity, and the root and chord the sounding notes make. Its Flow display lists the notes and messages — control changes, pitch bend, channel pressure — as they come. Its MPE display lists the per-note expressions — pressure, slide and per-note pitch bend — as they come. Freeze holds the display where it is while the notes carry on; Clear empties it.

The display's data is published after each block through the device framework's path, so the generic UI draws it, a screen reader reads it, and an agent reads the same data headless with `device.display`. What Live names a chord, and how, isn't recorded; Beatlayer's own rule is below.

This spec covers the device only. How a display's data reaches its cell and agents is device-framework R35 to R38.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 33, section 33.3.3 "MIDI Monitor" | A Note display showing incoming notes on a keyboard with their root note and related chords and their velocity; a Flow diagram listing incoming notes and data such as pitch bend and aftertouch as a continuous stream; Freeze and Clear; an MPE display showing incoming note, velocity, slide, pressure and per-note pitch data |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30, R35 to R38 | A MIDI effect forwards what it doesn't change; a device publishes its display data after each block through a lock-free handoff and offers views |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules" | No allocation, lock or blocking call on the audio thread |

## Rules

- **R1.** The Note display draws a keyboard with each of the 128 keys' velocity, zero when the key isn't sounding, and reads the number of sounding keys, their lowest key and the chord they make. (S1)
- **R2.** The monitor changes nothing it passes: every note, per-note expression and channel message goes on unchanged, and it starts no note. (S1, S2)
- **R3.** While Freeze is on, the display holds what it showed and the notes pass on. (S1)
- **R4.** When Clear turns on, the display empties on that frame. (S1)
- **R5.** The Flow display lists each note-on, note-off and channel message that reaches the device, most recent first, with how many it has seen. (S1)
- **R6.** The MPE display lists each per-note pressure, slide and pitch bend that reaches the device, most recent first, with how many it has seen. (S1)
- **R7.** The chord a set of sounding keys makes is read in root position: the lowest key is the root, and the intervals above it name the chord (major, minor, diminished, augmented, sus2, sus4, major 7, dominant 7 and minor 7). An inversion, a single key or anything else has no name. (S1; the manual doesn't record Live's rule, so this is Beatlayer's own.)
- **R8.** A key is named by its pitch class and octave, with middle C (key 60) as C4. (S1)
- **R9.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/MIDI Monitor/`. (S3)
- **R10.** Every parameter is automatable and has a description, and nothing the device does on the audio thread allocates, locks or blocks, at any block size. (S3, S4)

## Defaults, ranges and details

- Freeze and Clear start off.
- The Flow and MPE lists hold their most recent 16 entries; the counts are of everything seen.
- The display offers three views, Note, Flow and MPE, chosen with `device.set_display_view`; the first is the one it starts in. The cell draws the keyboard in every view and lists the readings; Live's three drawings differ more than this (S1).
- A note whose key is still sounding is drawn with its note-on velocity; a key whose note-off has come is zero.

## What the manual leaves open

For `docs/evidence/spec-notes/midi-monitor.md`:

- How Live names a chord, which chords it names, and whether it reads inversions.
- What the Flow diagram shows for each message type, its order and how many it keeps.
- Whether the three displays are separate views or one drawing, and how they're chosen.
- What Freeze holds exactly, and whether Clear also resets the counts.

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/midi_monitor.rs`, `the_display_publishes_the_keyboard_and_the_chord`; `crates/devices/src/midi_monitor.rs`, `the_display_reads_the_keyboard_and_the_chord` | real-time trap, unit |
| R2 | `crates/devices/tests/midi_monitor.rs`, `the_monitor_changes_nothing_it_passes` | real-time trap |
| R3 | `crates/devices/tests/midi_monitor.rs`, `the_display_publishes_the_keyboard_and_the_chord` | real-time trap |
| R4 | `crates/devices/tests/midi_monitor.rs`, `clear_empties_the_display` | real-time trap |
| R5 | `crates/devices/src/midi_monitor.rs`, `the_display_reads_the_keyboard_and_the_chord` | unit |
| R6 | `crates/devices/src/midi_monitor.rs`, `the_display_reads_the_keyboard_and_the_chord` | unit |
| R7 | `crates/devices/tests/midi_monitor.rs`, `chords_and_keys_are_named`; `crates/devices/src/midi_monitor.rs`, `the_chord_name_reads_root_position` | real-time trap, unit |
| R8 | `crates/devices/tests/midi_monitor.rs`, `chords_and_keys_are_named`; `crates/devices/src/midi_monitor.rs`, `a_key_is_named_from_middle_c` | real-time trap, unit |
| R9 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R10 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/midi_monitor.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |
