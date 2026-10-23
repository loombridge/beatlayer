# The note-echo MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.31 (R1 to R9); WP-6.8 (R10, scales). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 33 "Max for Live Devices", section 33.3.5 "Note Echo", read on 19 October 2026, and again for scales on 20 October 2026

## The behaviour

Note Echo creates rhythmic repetitions of incoming MIDI notes. Delay time can be synchronized to sixteenth-note intervals (from 1 to 16 sixteenths) with a Swing timing adjustment, or specified freely in milliseconds. Input playback mode selects between Thru (audible input note accompanied by echoes) and Mute (original note silenced, outputting only echoes).

Each repetition applies a progressive pitch transposition in semitones (Pitch) and velocity attenuation (Velocity Decay). Feedback determines the recirculation of generated echoes until velocities fall to zero. When the MPE switch is enabled, per-note expression messages (Pressure, Slide, Bend) accompany the echoed notes and decay across repetitions according to dedicated feedback controls; with MPE disabled, expression data is omitted from echoes.

The device takes Live's name, Note Echo; its appearance is the device framework's generic controls. Echoes wait in a preallocated internal queue; deactivating, moving, or deleting the device cleanly terminates sounding notes without hanging notes.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 33 "Max for Live Devices", section 33.3.5 "Note Echo" | Sync switch enables delay time in 16th-note beat divisions (1 to 16); Swing percentage alters timing; free mode sets delay in milliseconds; Input switch selects Thru or Mute; Pitch sets per-repeat transposition in semitones; Delay/Velocity decay and Feedback define note repeat attenuation and duration; MPE switch echoes MPE data alongside notes; Press, Slide, and Note PB controls set per-expression feedback decay |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | Note tracking, queue, release on disable or deletion, per-note expressions |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules"; [the musical time spec](musical-time.md) | No allocation, lock, or blocking call on the audio thread; the tempo map gives tempo and song position |

## Rules

- **R1.** Delay time is configured either in sixteenth notes (1 to 16) with Swing percentage (-50 % to +50 %) when Sync is enabled, or in milliseconds (1 to 5000 ms) when Sync is disabled. (S1, S4)
- **R2.** Input mode selects between Thru (original note is transmitted alongside generated echoes) and Mute (original incoming note is silenced and only echoes sound). (S1)
- **R3.** Pitch sets the transposition in semitones applied to each consecutive echo repetition. (S1)
- **R4.** Velocity Decay and Feedback scale the velocity of each repetition; echoes continue until velocity decays to zero or feedback recirculation ceases. (S1)
- **R5.** Each echo note sounds for the duration of the incoming note that generated it. (S1)
- **R6.** When the MPE switch is active, per-note expressions (Pressure, Slide, Pitch Bend) repeat with the echoes, scaled by their respective feedback amounts (Press, Slide, Pitch Bend); when MPE is inactive, per-note expressions are excluded from echoes. (S1)
- **R7.** When transport stops, or when the device is deactivated, moved, or deleted, sounding echo notes terminate immediately with no hanging notes. (S2, S4)
- **R8.** Every parameter is automatable, has a description, is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Note Echo/`. (S3)
- **R9.** No allocation, lock, or blocking operation occurs on the audio thread. (S4)
- **R10.** Note Echo has no Use Current Scale: its Pitch moves each echo by keys whatever its track's scale ([scales.md](scales.md) R15), and under a tuning a key is a step of the tuning ([tunings.md](tunings.md) R17). (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/note_echo.rs`, `sync_and_free_delay_time_land_on_exact_frames` | real-time trap |
| R2 | `crates/devices/tests/note_echo.rs`, `input_mode_thru_and_mute`, `input_notes_without_an_id_keep_their_echoes` | real-time trap |
| R3 | `crates/devices/tests/note_echo.rs`, `pitch_transposition_applies_per_repeat` | real-time trap |
| R4 | `crates/devices/tests/note_echo.rs`, `velocity_decay_and_feedback_diminish_repeats` | real-time trap |
| R5 | `crates/devices/tests/note_echo.rs`, `echoes_preserve_original_note_duration` | real-time trap |
| R6 | `crates/devices/tests/note_echo.rs`, `mpe_switch_and_expression_feedback` | real-time trap |
| R7 | `crates/devices/tests/note_echo.rs`, `stopping_transport_leaves_no_hanging_echoes`, `stopping_transport_while_an_echo_sounds_leaves_no_hanging_notes` | real-time trap |
| R8 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R9 | `crates/devices/tests/rules.rs`; `rtsan/tests/midi_effects.rs` | real-time trap |
| R10 | `crates/devices/tests/note_echo.rs`, `pitch_moves_by_keys_whatever_the_clip_s_scale` | real-time trap |

## Defaults, ranges and details

- Default parameters: Sync on (true), Time Sync 4 (4 sixteenths = 1 quarter note), Swing 0 %, Time Free 250 ms, Input Thru (0), Pitch 0 semitones, Velocity Decay 80 %, Feedback 60 %, MPE off (false), Press Feedback 100 %, Slide Feedback 100 %, Pitch Bend Feedback 100 %.
- Transposition range: -48 to +48 semitones.
- Time Free range: 1.0 to 5000.0 ms.
- Feedback range: 0 % to 100 %.
