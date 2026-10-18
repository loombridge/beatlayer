# The random-pitch MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.30. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.6 "Random", read on 17 October 2026

## The behaviour

Random moves some of the notes that reach it by a random number of steps. Chance says how often a note moves, as a dry/wet balance for the randomness. Choices is how many different distances a note may move by and Interval is the step between them, so together they set how far a note can go. Mode Random picks a distance with no fixed order; Mode Alt goes through the distances in turn. Sign says whether notes move up (Add), down (Sub), or either way (Bi). Each note ends on the key it was started on, and its expressions follow it.

The device takes Live's name, Random; its look is the device framework's generic controls. Random choices come from a generator seeded from the device, so an offline render repeats. The Use Current Scale switch, which would keep moved notes in the clip's scale, is carried and does nothing until Phase 6's scales. This spec covers the device only.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.6 "Random" | Chance is how likely a note is to be altered, like a dry/wet control; Choices, from 1 to 24, is how many pitches are possible, and the scale value is the spacing, together making the range; Mode Random picks pitches in no fixed order, Alt cycles through them in a fixed order, one step for each output at 100 % Chance, and at 0 % the input passes unchanged; Sign Add gives higher pitches, Sub lower, Bi both; three lights show whether a note was raised, left alone or lowered; Use Current Scale keeps pitches in the clip's scale |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | As in the transposing spec |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules", and the Determinism requirement | No allocation, lock, or blocking call on the audio thread; random choices repeat in an offline render |

## Rules

- **R1.** At 0 % Chance every note passes unchanged. (S1)
- **R2.** A note that moves, with Sign Add, goes up by k times Interval semitones, with k a whole number from 1 to Choices. (S1)
- **R3.** With Sign Sub the note goes down by the same distances, and with Bi it goes up or down, each with equal chance. (S1)
- **R4.** In Alt mode the distances come in a fixed order, one for each note that moves: for Add, 1 to Choices times Interval, then again; for Sub the same downwards; for Bi, up one, down one, up two, down two, and so on. At 100 % Chance every note moves. (S1)
- **R5.** Chance is the share of notes that move, from 0 to 100 per cent; each note moves or not by its own draw. (S1)
- **R6.** The draws come from a generator seeded from the device, so two renders of one set are bit for bit the same. (S1, S4)
- **R7.** Each note's note-off, and each of its expressions, goes to the key its note-on went to; a moved note that would leave the keyboard is folded by octaves back onto it. (S2)
- **R8.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Random/`. (S3)
- **R9.** Every parameter is automatable and has a description, and nothing Random does on the audio thread allocates, locks, or blocks. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/dice.rs`, `zero_chance_leaves_every_note_alone` | real-time trap |
| R2 | `crates/devices/tests/dice.rs`, `add_moves_notes_up_by_choices_of_the_interval` | real-time trap |
| R3 | `crates/devices/tests/dice.rs`, `sub_moves_down_and_bi_either_way` | real-time trap |
| R4 | `crates/devices/tests/dice.rs`, `alt_goes_through_the_choices_in_turn` | real-time trap |
| R5 | `crates/devices/tests/dice.rs`, `chance_sets_how_often_notes_move` | real-time trap |
| R6 | `crates/devices/tests/dice.rs`, `two_renders_are_identical` | real-time trap |
| R7 | `crates/devices/tests/dice.rs`, `every_note_ends_on_its_own_key` | real-time trap |
| R8 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/dice.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |

## Defaults, ranges and details

- A new Random starts at Chance 100 %, Choices 24, Interval 1 semitone, Mode Random, Sign Add and Use Current Scale off. The manual states only the Mode and Sign defaults; the rest are Beatlayer's.
- Interval runs from 1 to 24 semitones. The manual doesn't give its range.
- Whether a distance of 0 is among the choices is not stated; here the distances are 1 to Choices times Interval, and "0" is a note Chance left alone.
- The three direction lights are not drawn: they need the engine to publish a device's display state.
