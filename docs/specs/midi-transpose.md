# The transposing MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.30. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34; where a MIDI effect sits in a chain is [the device chains spec](device-chains.md)
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.5 "Pitch", read on 17 October 2026

## The behaviour

Pitch is a MIDI effect that moves every note that reaches it by a number of semitones, and keeps the result inside a window of keys. A note that would land outside the window is dropped (Block), moved by octaves into it (Fold), or moved to its nearest edge (Limit). The note's velocity, release velocity, channel and timing are untouched, and its per-note expressions (bend, pressure, slide) follow it to its new key. Pitch ends each note on the key it started it on, whatever changes while the note is held.

The device takes Live's name, Pitch; its look is the device framework's generic controls. Its parameters, ranges and units come from the manual's Pitch section. Step Up and Step Down in the manual are two buttons that move Pitch by Step Width; the generic layout has no momentary buttons, so Beatlayer carries Step Width as a parameter and agents and people set Pitch directly. The Use Current Scale switch is carried and does nothing until Phase 6's scales: Pitch then reads in scale degrees. This spec covers the device only.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.5 "Pitch" | Transposes incoming notes by up to 128 semitones either way (30 scale degrees with Use Current Scale); Step Up and Step Down move the value by Step Width, from 1 to 48 semitones; Lowest and Range define a window of keys notes may pass; Mode chooses Block (notes outside are dropped, and a light by Range flashes), Fold (notes outside are moved into the window) or Limit (notes below are raised to the lowest key, notes above lowered to the highest) |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | A native MIDI effect reads the stretch's notes, forwards what it doesn't change, sends notes in order, ends every note it started, and keeps its expressions with their notes |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules" | No allocation, lock, or blocking call on the audio thread; every buffer is made with the device |

## Rules

- **R1.** Every note that reaches Pitch leaves it Pitch semitones away, on the same frame, with the same velocity, channel and note ID, and its note-off leaves on the key its note-on left on, with the note-off's own release velocity. A note moved outside 0 to 127 is outside the window (R3 to R5). (S1, S2)
- **R2.** Pitch runs from −128 to +128 semitones and moves notes down when negative. (S1)
- **R3.** The window runs from Lowest, a key from 0 to 127, up Range keys, to at most 127. In Block mode a note that would land outside it is dropped, and so are its note-off and its expressions; the light that flashes for it is drawn once the engine publishes a device's display state. (S1)
- **R4.** In Fold mode a note outside the window is moved by whole octaves into it; a window too narrow for any octave of the note drops it. (S1)
- **R5.** In Limit mode a note below the window is moved to Lowest and a note above it to the window's highest key. (S1)
- **R6.** Changing Pitch, Lowest, Range or Mode while a note is held doesn't move the note: its note-off leaves on the key its note-on left on, at any block size. A note-off for a note Pitch never saw, such as one started while it was switched off, passes through as it came. (S1, S2)
- **R7.** A per-note expression for a note Pitch moved is sent on to the moved note, with its key and note ID. (S2)
- **R8.** A note Pitch dropped takes its expressions with it. (S2)
- **R9.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Pitch/`. (S3)
- **R10.** Every parameter is automatable and has a description, and nothing Pitch does on the audio thread allocates, locks, or blocks. (S3, S4)
- **R11.** Use Current Scale is a switch that does nothing until Phase 6 brings scales; Step Width is a value that nothing reads until the Step Up and Step Down buttons exist. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/lift.rs`, `a_note_lands_transposed_on_its_frames`; `crates/shell/tests/midi_io.rs`, `the_computer_keyboard_plays_through_a_midi_effect_into_an_instrument` | real-time trap; unit |
| R2 | `crates/devices/tests/lift.rs`, `a_negative_pitch_moves_notes_down` | real-time trap |
| R3 | `crates/devices/tests/lift.rs`, `block_drops_notes_outside_the_window` | real-time trap |
| R4 | `crates/devices/tests/lift.rs`, `fold_moves_notes_by_octaves_into_the_window` | real-time trap |
| R5 | `crates/devices/tests/lift.rs`, `limit_moves_notes_to_the_nearest_edge` | real-time trap |
| R6 | `crates/devices/tests/lift.rs`, `a_held_note_ends_where_it_started` | real-time trap |
| R7 | `crates/devices/tests/lift.rs`, `expressions_follow_the_transposed_note` | real-time trap |
| R8 | `crates/devices/tests/lift.rs`, `a_blocked_note_takes_its_expressions_with_it` | real-time trap |
| R9 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R10 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/lift.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |
| R11 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` (both exist and read back) | MCP script |

## Defaults, ranges and details

- A new Pitch starts at Pitch 0, Step Width 1, Lowest 0, Range 127 (the whole keyboard), Mode Block, Use Current Scale off. The manual doesn't state the defaults; these leave notes alone.
- The window applies to the key after Pitch. Pitch and Step Width are whole numbers of semitones.
- The blocked-note light is not drawn: it needs the engine to publish a device's display state.
