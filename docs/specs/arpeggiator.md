# The arpeggiator MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.31 (R1 to R14); WP-6.8 (Root, Scale and Use Current Scale: R15 to R17; under a tuning the switch follows [tunings.md](tunings.md) R16); WP-6.68 (the Groove chooser's grooves and their swing under the Global Amount: R18 to R20, with R10 as it reads now). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.1 "Arpeggiator", read on 19 October 2026; for Root, Scale and Use Current Scale, the same section and the manual's chapter 3, section 3.13 "Scale Awareness", read 20 October 2026, and the Live 12 release notes, 12.0, 12.1.5 and 12.1.10; for the Groove chooser, section 30.1 and the Live 12 release notes, 12.0 and 12.2, read 20 October 2026

## The behaviour

Arpeggiator turns held chords and notes into rhythmic sequences. The sequence follows a chosen Style: directional patterns (Up, Down, UpDown, DownUp, Up & Down, Down & Up, Converge, Diverge, Con & Diverge), alternating patterns (Pinky Up, Pinky UpDown, Thumb Up, Thumb UpDown), played order, chord triggers, or random variations (Random, Random Other, Random Once). Rate sets the step speed, either in free milliseconds or synchronized to musical beat divisions. Gate adjusts note duration relative to step rate, allowing legato overlap at settings above 100 %. Hold preserves the playing pattern after keys are released, letting performers add or remove individual notes.

Offset shifts pattern sequence starting positions. Retrigger restarts the pattern on note arrivals or beat grid intervals. Repeats sets the number of pattern cycles before playback ceases. Pitch transposition can cycle through multiple Steps spaced by Distance, in semitones, or in degrees of a scale: the one its own Root and Scale choosers name, or, with Use Current Scale on in its title bar, the device's current scale ([scales.md](scales.md)). Velocity controls decay notes toward a Target level over time, optionally restarting with pattern retriggering.

The device takes Live's name, Arpeggiator; its appearance is the device framework's standard generic layout. Notes held for playback wait in an internal queue made with the device; when the device is deactivated, moved, or deleted, sounding notes terminate cleanly without hanging.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.1 "Arpeggiator" | The Style chooser determines the rhythmic pattern note order (Up, Down, Converge, Diverge, Play Order, Chord Trigger, Random, Random Other, Random Once); Rate sets playback speed in ms or tempo-synced beat divisions; Distance and Steps transpose the pattern across cycles; Gate sets note length as a percentage of Rate; Hold keeps the pattern repeating after keys are released, adding new notes or removing duplicates; Pattern Offset shifts the starting note; Retrigger restarts on Off, Note, or Beat with an Interval; Repeats sets the cycle count; Velocity Decay ramps velocity toward Target |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | Note tracking, queue, release on disable or deletion, per-note expressions |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules"; [the musical time spec](musical-time.md) | No allocation, lock, or blocking call on the audio thread; the tempo map gives tempo and song position |
| S5 | Manual | Live 12 reference manual, chapter 30, section 30.1 "Arpeggiator", and chapter 3, section 3.13 "Scale Awareness" | Distance is in semitones or scale degrees; the Root and Scale choosers transpose the pattern within a chosen scale; Use Current Scale, in the title bar, transposes by the clip's scale and disables Root and Scale |
| S6 | Release notes | Live 12 release notes, 12.0 (Distance shows scale degrees; Arpeggiator transforms per-note pitch bend to stay in the scale), 12.1.5 (its Scale and Root parameters can be mapped to a rack's macros) and 12.1.10 (its handling of the per-note bend of a note outside the scale) | Root and Scale are parameters; bends stay in the scale |
| S7 | Beatlayer | [scales.md](scales.md) R1, R2 and R15 to R18 | The scale list, moving a key by degrees, a device's current scale, its Use Current Scale switch, held notes and per-note bends |
| S8 | Release notes | Live 12 release notes, 12.0 (the redesigned Arpeggiator has Groove controls) and 12.2 (a fix: the Arpeggiator's groove now acts with Gate from 75 % to 100 % too) | The groove acts whatever the Gate |
| S9 | Beatlayer | [grooves.md](grooves.md) R6, R7, R10, R21 and R36 | How a groove moves a note from its grid line, the Global Amount that scales it and reaches every device with each block, and Beatlayer's bundled swing grooves |

## Rules

- **R1.** Style determines the sequence order of incoming notes in the arpeggiated pattern: Up, Down, UpDown, DownUp, Up & Down, Down & Up, Converge, Diverge, Con & Diverge, Pinky Up, Pinky UpDown, Thumb Up, Thumb UpDown, Play Order, Chord Trigger, Random, Random Other, or Random Once. (S1)
- **R2.** Rate sets the step duration between arpeggio notes, either in milliseconds when Sync is off or in tempo-synced beat divisions following the tempo map when Sync is on. (S1, S4)
- **R3.** Gate sets note duration as a percentage of step length from 1 % to 200 %, with values exceeding 100 % overlapping into subsequent steps. (S1)
- **R4.** With Hold active, notes continue arpeggiating after keys are released; playing a note not in the active pattern adds it, while playing a note already present removes it. (S1)
- **R5.** Pattern Offset rotates the pattern note order counterclockwise by the specified step offset. (S1)
- **R6.** Retrigger restarts the pattern from its starting note: Off (continuous), Note (on every incoming note-on), or Beat (on beat boundaries spaced by Interval). (S1, S4)
- **R7.** Repeats limits the number of full pattern repetitions across all transposition steps (1 to 64), or runs indefinitely when set to 0 (representing ∞). (S1)
- **R8.** Steps sets how many times the pattern is transposed (0 to 8), with each successive cycle transposed by Distance semitones. (S1)
- **R9.** When the Velocity switch is active, note velocities ramp toward Target over Decay time; if Retrigger Velocity is enabled, the decay timer restarts when the pattern retriggers. (S1)
- **R10.** The Groove chooser is R18 to R20's; Distance in scale degrees, Root, Scale and Use Current Scale are R15 to R17's. (S1)
- **R11.** Random styles follow a seeded pseudo-random generator derived from the device ID, ensuring deterministic, bit-identical event generation across renders. (S1, S4)
- **R12.** When transport stops, or when the device is deactivated, moved, or deleted, all currently sounding notes receive matching note-offs and no notes hang. (S2, S4)
- **R13.** Every parameter is automatable, has a description, is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Arpeggiator/`. (S3)
- **R14.** No allocation, lock, or blocking operation occurs on the audio thread. (S4)
- **R15.** Root, the twelve pitch classes from C (C in a new Arpeggiator), and Scale, Chromatic and then the scale list ([scales.md](scales.md) R1; Chromatic in a new Arpeggiator and in one saved before the choosers; the list's head and the defaults are Beatlayer's own), are parameters that can be automated, modulated and mapped to a rack's macros. With Scale on Chromatic, Distance counts semitones as R8 says; with a named scale, it counts degrees of that scale from Root. (S5, S6, S7)
- **R16.** Use Current Scale, a switch in Arpeggiator's title bar ([scales.md](scales.md) R16), off in a new Arpeggiator: with it on, Distance counts degrees of the device's current scale ([scales.md](scales.md) R15), and Root and Scale are drawn disabled. (S5, S7)
- **R17.** Counting degrees (R15, R16), each successive cycle of R8 moves the pattern's notes Distance more degrees ([scales.md](scales.md) R2), a held note outside the scale moving as the scale note below it does; the per-note bend of a note it moved is carried in degrees, as [scales.md](scales.md) R18 says; and a change of the scale while a note sounds doesn't move it. Distance keeps its −48 to +48 range, read as degrees (Beatlayer's own). (S5, S6, S7)
- **R18.** The Groove chooser lists None, then Beatlayer's bundled swing grooves in their order ([grooves.md](grooves.md) R36): Swing 8ths 55, 60, 65, 70 and 75, then Swing 16ths 55, 60, 65, 70 and 75 (the list is Beatlayer's own). A new Arpeggiator chooses None. An Arpeggiator saved with the earlier placeholder's Swing loads with None, so it plays as it did. (S1, S8, S9)
- **R19.** With a groove chosen and Sync on, each note's start moves as a clip's note at the same song position moves under that groove ([grooves.md](grooves.md) R6, R7 and R10): the grid counted from the song's start (1.1.1) in place of a clip's contents, the groove's own parameters (Quantize 0 %, Timing 100 %), and the move scaled by the set's Global Amount, which reaches the device with each block ([grooves.md](grooves.md) R21). At a Global Amount of 0 % the pattern plays straight. Each note keeps the length Gate gives it, whatever the Gate (S8). With Sync off, the groove moves nothing. (S1, S8, S9; the grid's start and Sync off are Beatlayer's own)
- **R20.** A grooved pattern's notes land on the same frames at any block size and render the same every time, and moving them allocates nothing on the audio thread: a note waiting for its grooved frame is held in the device's queue (R12, R14). (S4, S9)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/arpeggiator.rs`, `style_determines_note_playback_order` | real-time trap |
| R2 | `crates/devices/tests/arpeggiator.rs`, `rate_sync_follows_tempo_and_divisions` | real-time trap |
| R3 | `crates/devices/tests/arpeggiator.rs`, `gate_sets_duration_and_overlaps_above_100_percent` | real-time trap |
| R4 | `crates/devices/tests/arpeggiator.rs`, `hold_mode_retains_adds_and_removes_notes` | real-time trap |
| R5 | `crates/devices/tests/arpeggiator.rs`, `offset_shifts_pattern_starting_position` | real-time trap |
| R6 | `crates/devices/tests/arpeggiator.rs`, `retrigger_modes_restart_pattern` | real-time trap |
| R7 | `crates/devices/tests/arpeggiator.rs`, `repeats_limits_pattern_cycles` | real-time trap |
| R8 | `crates/devices/tests/arpeggiator.rs`, `steps_and_distance_transpose_cycles` | real-time trap |
| R9 | `crates/devices/tests/arpeggiator.rs`, `velocity_decay_ramps_toward_target` | real-time trap |
| R10 | `crates/devices/tests/arpeggiator.rs`, `the_groove_chooser_lists_none_and_the_ten_swing_grooves` | unit |
| R11 | `crates/devices/tests/arpeggiator.rs`, `random_styles_are_deterministic_and_repeat_identically` | real-time trap |
| R12 | `crates/devices/tests/arpeggiator.rs`, `stopping_transport_leaves_no_hanging_notes` | real-time trap |
| R13 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R14 | `crates/devices/tests/rules.rs`; `crates/devices/tests/arpeggiator.rs`, `a_chord_of_many_held_notes_allocates_nothing`; `rtsan/tests/midi_effects.rs`, `a_chain_of_midi_effects_passes_under_rtsan` | real-time trap |
| R15 | `crates/devices/tests/arpeggiator.rs`, `distance_counts_degrees_of_the_chosen_root_and_scale`; `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect`; `crates/shell/tests/macros.rs`, `the_scale_choosers_map_to_macros`; `crates/cli/tests/older_sets.rs`, `a_set_saved_before_use_current_scale_loads_its_devices_unchanged` | real-time trap; MCP script; unit |
| R16 | `crates/devices/tests/arpeggiator.rs`, `use_current_scale_counts_distance_in_the_current_scale`; `crates/ui/tests/device_view.rs`, `the_choosers_a_device_takes_from_the_current_scale_are_disabled` | real-time trap; unit |
| R17 | `crates/devices/tests/arpeggiator.rs`, `a_scale_change_while_a_note_sounds_does_not_move_it`, `a_bend_is_carried_in_degrees`, `random_styles_with_the_scale_repeat_identically` | real-time trap |
| R18 | `crates/devices/tests/arpeggiator.rs`, `the_groove_chooser_lists_none_and_the_ten_swing_grooves`; `crates/agent/tests/midi_effects.rs`, `an_agent_chooses_the_arpeggiators_groove_by_id`; `crates/cli/tests/older_sets.rs`, `an_arpeggiator_saved_with_the_placeholders_swing_loads_with_none` | unit; MCP script |
| R19 | `crates/devices/tests/arpeggiator.rs`, `swing_moves_every_second_note_on_where_a_clip_note_would_play`, `nothing_moves_at_zero_percent_with_sync_off_or_with_none`, `the_global_amount_scales_the_move`, `a_moved_note_keeps_its_gate_length_and_renders_the_same_every_time` | real-time trap |
| R20 | `crates/devices/tests/arpeggiator.rs`, `a_moved_note_keeps_its_gate_length_and_renders_the_same_every_time`, `a_note_waiting_for_its_frame_when_the_key_goes_up_never_sounds`; `rtsan/tests/midi_effects.rs`, `a_swung_arpeggio_passes_under_rtsan` | real-time trap |

## Defaults, ranges and details

- Default parameters: Style Up (0), Sync on (true), Rate Sync 1/8 (index 5), Rate Free 200 ms, Gate 100 %, Hold off (false), Offset 0, Retrigger Note (1), Interval 1 bar (4.0 beats), Repeats 0 (∞), Steps 1, Distance 12 semitones, Velocity off (false), Decay 1000 ms, Target 64, Retrigger Velocity on (true), Use Scale off (false), Groove None (0); Root C and Scale Chromatic (R15).
- Free rate ranges from 1 ms to 2000 ms.
- Beat divisions for Rate Sync match standard division choices: 1/32, 1/16T, 1/16, 1/16., 1/8T, 1/8, 1/8., 1/4T, 1/4, 1/4., 1/2T, 1/2, 1/2., 1 bar, 2 bars, 4 bars, 8 bars.
- While transport is stopped, synced rate steps using the current song tempo map on the device's own continuous clock.
