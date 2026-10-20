# The scale-map MIDI effect

- **Status:** partial (R1 to R9 have passing tests; R10 to R12, the scale names and Use Current Scale, are WP-6.8's)
- **Work packages:** WP-5.30 (R1 to R9); WP-6.8 (the Scale Name chooser and Use Current Scale: R10 to R12; under a tuning the switch follows [tunings.md](tunings.md) R16). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.7 "Scale", read on 17 October 2026; for the scale names and Use Current Scale, the same section and the manual's chapter 3, section 3.13 "Scale Awareness", read 20 October 2026, and the Live 12 release notes, 12.0 and 12.1.5

## The behaviour

Scale sends each note of an octave to a note of your choosing. A grid has a column for each note that can come in, counted from Base, and a row for each note that can go out; a column sends its note to its row, in every octave, and a column with no row sends its note nowhere, so it plays nothing. Transpose then moves every note that comes out. Fold keeps a mapped note within half an octave of the note that came in. Lowest and Range limit which notes are remapped; the rest pass through.

The device takes Live's name, Scale; its look is the device framework's generic controls, with each column of the grid a parameter. A Scale Name chooser picks User, the grid a person edits, or a named scale from the list ([scales.md](scales.md)), whose grid sends each note to the scale note nearest it. Use Current Scale, a switch in its title bar, takes Base and Scale Name from the device's current scale and shows them. This spec covers the device only.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.7 "Scale" | A 13 by 13 note matrix maps incoming notes (columns) to outgoing notes (rows), starting from the root at the bottom left; Base and Scale Name choosers set the root and the scale, and are unavailable with Use Current Scale; the User scale lets squares be added and removed, and a removed note is no longer output; Transpose moves notes by up to 36 semitones; Fold, for the User scale, maps a note that would land more than six semitones from the original to the nearer octave; Lowest and Range limit which notes are remapped |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | As in the transposing spec |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description; a choice of one entry can't change, so the framework's rule G2 test refuses it |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules" | No allocation, lock, or blocking call on the audio thread |
| S5 | Manual | Live 12 reference manual, chapter 3, section 3.13 "Scale Awareness" | Scale is scale-aware: its Use Current Scale switch, in its title bar, applies the clip's scale |
| S6 | Release notes | Live 12 release notes, 12.0 (Scale follows the clip's root note and scale name and shows the scale in effect in its choosers) and 12.1.5 (Scale's Scale and Root parameters can be mapped to a rack's macros) | The choosers show what the device follows; Base and Scale Name are parameters |
| S7 | Beatlayer | [scales.md](scales.md) R1, R2 and R15 to R17 | The scale list, the nearest scale note, a device's current scale, its Use Current Scale switch, held notes |

## Rules

- **R1.** With the grid as a new device has it, every column sends its note to the same note, and no note changes. (S1)
- **R2.** A note's column is its distance in semitones above Base, modulo 12; a column set to a row sends the note to that many semitones above the base of the note's own octave. Notes of every octave follow their column. (S1)
- **R3.** A column set to −1 (no note) plays nothing: the note is dropped, with its note-off and its expressions. (S1)
- **R4.** Base, one of the twelve notes, moves where the columns are counted from: with Base D, the first column is D. (S1)
- **R5.** Transpose, from −36 to +36 semitones, moves every note after it is mapped; a note moved off the keyboard is dropped. (S1)
- **R6.** With Fold on, a mapped note more than six semitones above or below the note that came in is moved by an octave towards it, before Transpose. (S1)
- **R7.** Only notes from Lowest up Range keys are remapped; others pass through unchanged. (S1)
- **R8.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Scale/`. (S3)
- **R9.** Every parameter is automatable and has a description, and nothing Scale does on the audio thread allocates, locks, or blocks. (S3, S4)
- **R10.** Scale Name, a parameter, chooses User or a scale from the list ([scales.md](scales.md) R1), User first and the list after it in its order; a new Scale, and one saved before the chooser, is User (Beatlayer's own). With User the grid is the twelve Map columns (R2, R3). With a named scale each column sends its note to the scale note nearest it, counted from Base ([scales.md](scales.md) R2), and the grid shows that mapping; the Map columns keep their values for when User is chosen again (Beatlayer's own). Base and Scale Name can be automated, modulated and mapped to a rack's macros like any parameter. (S1, S6, S7)
- **R11.** Fold (R6) works only while Scale Name is User; with a named scale it is drawn disabled and acts as off. (S1)
- **R12.** Use Current Scale, a switch in Scale's title bar ([scales.md](scales.md) R16), off in a new Scale: with it on, Base and Scale Name follow the device's current scale ([scales.md](scales.md) R15), its root and its scale name, and are drawn disabled showing the scale they follow; a current scale with Scale Mode off leaves every note as it came (Beatlayer's own). A change of the current scale, Base or Scale Name while a note is held doesn't move it ([scales.md](scales.md) R17). (S1, S5, S6, S7)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/remap.rs`, `the_default_grid_changes_nothing` | real-time trap |
| R2 | `crates/devices/tests/remap.rs`, `a_column_sends_its_note_to_its_row` | real-time trap |
| R3 | `crates/devices/tests/remap.rs`, `a_removed_note_plays_nothing` | real-time trap |
| R4 | `crates/devices/tests/remap.rs`, `base_counts_the_columns_from_another_note` | real-time trap |
| R5 | `crates/devices/tests/remap.rs`, `transpose_moves_the_mapped_notes` | real-time trap |
| R6 | `crates/devices/tests/remap.rs`, `fold_keeps_the_note_near_where_it_was` | real-time trap |
| R7 | `crates/devices/tests/remap.rs`, `notes_outside_the_window_pass_through` | real-time trap |
| R8 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/remap.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |
| R10 | to be written in WP-6.8 | |
| R11 | to be written in WP-6.8 | |
| R12 | to be written in WP-6.8 | |

## Defaults, ranges and details

- A new Scale starts at Base C, Transpose 0, Fold off, Lowest 0, Range 127 and Use Current Scale off, with the identity grid.
- The grid has twelve columns, one for each note of an octave, and thirteen rows, 0 to 12 semitones above Base (row 12 is the base an octave up). The manual draws thirteen columns; the thirteenth repeats the first an octave up and no note can come in on it, so it is not a parameter.
- Each column is a whole number from −1 to 12 (`Map 1` to `Map 12`); an agent reads and sets them like any parameter.
- The chooser of scale names is R10's: User and the scale list of [scales.md](scales.md) R1.
