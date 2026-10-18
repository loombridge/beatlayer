# The note-length MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.30. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.4 "Note Length", read on 17 October 2026

## The behaviour

Hold changes how long notes sound, and can start notes when keys come up instead of when they go down. In Note On mode each note starts when its key goes down and lasts Length times Gate, in milliseconds or in beat divisions at the tempo of the moment; the key coming up is ignored. In Note Off mode a note starts when its key comes up and then lasts Length times Gate, with its velocity worked out from how long the key was held (Decay Time) and mixed with the key's release velocity (Release Velocity). Key Scale makes notes shorter or longer by pitch around middle C. Latch keeps a note sounding until the next one starts.

Beatlayer's own name for the device is Hold; its look is the device framework's generic controls. Notes held for later wait in a queue made with the device, and every one ends: when the device is switched off, moved or deleted, its sounding notes end at once and nothing waits. This spec covers the device only.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.4 "Note Length" | The trigger source is Note On (the device responds when a note starts) or Note Off (when it is released); in Note On mode Length sets the duration and Gate scales it (100 % is exactly Length, 200 % twice as long), and Length is in milliseconds or in tempo-synced beat divisions; in Note Off mode each note is delayed by its own length so it starts where it would have ended, and then Gate and Length apply; Release Velocity balances the note-on and note-off velocities, and should stay at 0 % for a keyboard that sends no release velocity; Decay Time is how long a note's velocity takes to fall to zero from note-on, and the velocity reached at note-off is the output velocity; Key Scale changes the length by pitch around C3, longer below and shorter above when positive; Latch in Note On mode keeps notes sounding until the next note-on, with Gate and Length unavailable; in Note Off mode it triggers a note once all keys or the sustain pedal are up and a new note-off arrives |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | As in the transposing spec |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules"; [the musical time spec](musical-time.md) | No allocation, lock, or blocking call on the audio thread; the tempo map gives the tempo at a song position |

## Rules

- **R1.** In Note On mode a note starts on its frame and ends Length later, on an exact frame, whatever the length of the note that came in; the key coming up is ignored. (S1)
- **R2.** The sounding length is Length times Gate, with Gate from 1 to 200 per cent. (S1)
- **R3.** With Sync on, Length is a beat division (1/32 to two bars) worked out at the tempo of the stretch the note starts in, so the same note lasts twice as long at 60 BPM as at 120. (S1, S4)
- **R4.** In Note Off mode a note starts on the frame its key comes up and lasts Length times Gate from there. A key coming up that Hold never saw go down starts nothing. (S1)
- **R5.** Key Scale multiplies the length by 2 to the power of −Key Scale times the distance of the key from middle C (key 60) in octaves: positive values lengthen notes below middle C and shorten those above, negative values the opposite. (S1)
- **R6.** A note that starts on a key still sounding from an earlier note ends that earlier note first, on the same frame, and the earlier note's own end no longer happens. (S1)
- **R7.** With Latch on in Note On mode, a note sounds until the next note starts, and Length and Gate do nothing. In Note Off mode a latched note starts once every key is up. (S1)
- **R8.** In Note Off mode the note's velocity is the velocity of the key when it went down, falling in a straight line to zero over Decay Time while it is held (no fall at a Decay Time of 0), then mixed with the key's release velocity by Release Velocity per cent. (S1)
- **R9.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Hold/`. (S3)
- **R10.** Every parameter is automatable and has a description, and nothing Hold does on the audio thread allocates, locks, or blocks. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/hold.rs`, `a_note_lasts_the_length` | real-time trap |
| R2 | `crates/devices/tests/hold.rs`, `gate_scales_the_length` | real-time trap |
| R3 | `crates/devices/tests/hold.rs`, `a_synced_length_follows_the_tempo` | real-time trap |
| R4 | `crates/devices/tests/hold.rs`, `note_off_mode_starts_notes_when_keys_come_up` | real-time trap |
| R5 | `crates/devices/tests/hold.rs`, `key_scale_changes_the_length_by_pitch` | real-time trap |
| R6 | `crates/devices/tests/hold.rs`, `a_retriggered_key_ends_the_note_before` | real-time trap |
| R7 | `crates/devices/tests/hold.rs`, `latch_keeps_a_note_until_the_next_starts` | real-time trap |
| R8 | `crates/devices/tests/hold.rs`, `decay_and_release_velocity_set_the_velocity` | real-time trap |
| R9 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R10 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/hold.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |

## Defaults, ranges and details

- A new Hold starts in Note On mode with Sync off, Length 200 ms, Length Division 1/8, Gate 100 %, Release Velocity 0 %, Decay Time 0 ms (no decay), Key Scale 0 and Latch off. The manual states no ranges or defaults for Length or Gate; Length runs from 1 ms to 10 s, logarithmically, and Gate from 1 to 200 per cent.
- The manual doesn't say what Hold does to a note that is sounding when the transport stops. Here it ends at its own time, as a note does anywhere else; a stop doesn't cut it. The clip player's own note-offs at a stop are ignored in Note On mode.
- A latched Note Off trigger fires when the last held key comes up; the sustain pedal isn't read.
- Release velocity of the notes Hold ends in Note On mode is 64 of 127.
