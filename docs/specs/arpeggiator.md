# The arpeggiator MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.31. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.1 "Arpeggiator", read on 19 October 2026

## The behaviour

Arpeggiator turns held chords and notes into rhythmic sequences. The sequence follows a chosen Style: directional patterns (Up, Down, UpDown, DownUp, Up & Down, Down & Up, Converge, Diverge, Con & Diverge), alternating patterns (Pinky Up, Pinky UpDown, Thumb Up, Thumb UpDown), played order, chord triggers, or random variations (Random, Random Other, Random Once). Rate sets the step speed, either in free milliseconds or synchronized to musical beat divisions. Gate adjusts note duration relative to step rate, allowing legato overlap at settings above 100 %. Hold preserves the playing pattern after keys are released, letting performers add or remove individual notes.

Offset shifts pattern sequence starting positions. Retrigger restarts the pattern on note arrivals or beat grid intervals. Repeats sets the number of pattern cycles before playback ceases. Pitch transposition can cycle through multiple Steps spaced by Distance in semitones (or scale degrees once Phase 6 scales arrive). Velocity controls decay notes toward a Target level over time, optionally restarting with pattern retriggering.

The device takes Live's name, Arpeggiator; its appearance is the device framework's standard generic layout. Notes held for playback wait in an internal queue made with the device; when the device is deactivated, moved, or deleted, sounding notes terminate cleanly without hanging.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.1 "Arpeggiator" | The Style chooser determines the rhythmic pattern note order (Up, Down, Converge, Diverge, Play Order, Chord Trigger, Random, Random Other, Random Once); Rate sets playback speed in ms or tempo-synced beat divisions; Distance and Steps transpose the pattern across cycles; Gate sets note length as a percentage of Rate; Hold keeps the pattern repeating after keys are released, adding new notes or removing duplicates; Pattern Offset shifts the starting note; Retrigger restarts on Off, Note, or Beat with an Interval; Repeats sets the cycle count; Velocity Decay ramps velocity toward Target |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | Note tracking, queue, release on disable or deletion, per-note expressions |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules"; [the musical time spec](musical-time.md) | No allocation, lock, or blocking call on the audio thread; the tempo map gives tempo and song position |

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
- **R10.** Distance in scale degrees and Use Current Scale wait for Phase 6's scale support; until Phase 6's groove pool, the Groove chooser changes no timing. (S1)
- **R11.** Random styles follow a seeded pseudo-random generator derived from the device ID, ensuring deterministic, bit-identical event generation across renders. (S1, S4)
- **R12.** When transport stops, or when the device is deactivated, moved, or deleted, all currently sounding notes receive matching note-offs and no notes hang. (S2, S4)
- **R13.** Every parameter is automatable, has a description, is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Arpeggiator/`. (S3)
- **R14.** No allocation, lock, or blocking operation occurs on the audio thread. (S4)

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
| R10 | `crates/devices/tests/arpeggiator.rs`, `scale_and_groove_phase_6_placeholders` | real-time trap |
| R11 | `crates/devices/tests/arpeggiator.rs`, `random_styles_are_deterministic_and_repeat_identically` | real-time trap |
| R12 | `crates/devices/tests/arpeggiator.rs`, `stopping_transport_leaves_no_hanging_notes` | real-time trap |
| R13 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R14 | `crates/devices/tests/rules.rs`; `crates/devices/tests/arpeggiator.rs`, `a_chord_of_many_held_notes_allocates_nothing`; `rtsan/tests/midi_effects.rs` | real-time trap |

## Defaults, ranges and details

- Default parameters: Style Up (0), Sync on (true), Rate Sync 1/8 (index 5), Rate Free 200 ms, Gate 100 %, Hold off (false), Offset 0, Retrigger Note (1), Interval 1 bar (4.0 beats), Repeats 0 (∞), Steps 1, Distance 12 semitones, Velocity off (false), Decay 1000 ms, Target 64, Retrigger Velocity on (true), Use Scale off (false), Groove None (0).
- Free rate ranges from 1 ms to 2000 ms.
- Beat divisions for Rate Sync match standard division choices: 1/32, 1/16T, 1/16, 1/16., 1/8T, 1/8, 1/8., 1/4T, 1/4, 1/4., 1/2T, 1/2, 1/2., 1 bar, 2 bars, 4 bars, 8 bars.
- While transport is stopped, synced rate steps using the current song tempo map on the device's own continuous clock.
