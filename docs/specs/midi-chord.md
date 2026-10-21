# The chord MIDI effect

- **Status:** partial (R1 to R15 have passing tests; R16 to R18, Use Current Scale, are WP-6.8's)
- **Work packages:** WP-5.30 (R1 to R15); WP-6.8 (Use Current Scale: R16 to R18; under a tuning the switch follows [tunings.md](tunings.md) R16). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R30 to R34
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.3 "Chord", read on 17 October 2026; for Use Current Scale, the same manual's chapter 3, section 3.13 "Scale Awareness", and chapter 30's introduction, read 20 October 2026, and the Live 12 release notes, 12.0 and 12.1

## The behaviour

Chord builds a chord from each note that reaches it by adding up to six notes, each a set number of semitones above or below it. Each added note has its own velocity, as a share of the note's, and its own chance of sounding. Strum spreads the chord over a time, the note first and then the shifts in order, or the other way round when negative; Tension makes the spread start slowly or quickly, and Crescendo ramps the velocity along it. A shift of 0, or one that an earlier shift already has, adds nothing. The context menu's two options in Live, to let a strum play a key that is already sounding and to send per-note events to the added notes, are two switches.

Each note and the notes it made end together: when the key comes up, every added note that is sounding ends, and every one still waiting for the strum never sounds. The device takes Live's name, Chord; its look is the device framework's generic controls. Chance comes from a generator seeded from the device, so an offline render repeats. The Use Current Scale switch in its title bar makes the shifts count degrees of the device's current scale ([scales.md](scales.md)), so the chord stays in the scale, and a bent chord stays in it too. This spec covers the device only.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.3 "Chord" | Up to six added pitches, each a Shift control from −36 to +36 semitones, in any order, each with a light that flashes when it sounds; a pitch can be used only once, and a repeat is deactivated; Learn takes a chord held on a MIDI controller, the first key being the root and the rest becoming the shifts in order; each shift has a Velocity from 1 to 200 per cent of the note's and a Chance from 0 to 100 per cent; Strum delays the added notes by up to 400 ms, playing the note first and then Shift 1 to 6 when positive and the reverse when negative; Tension makes a positive strum start slowly and speed up, a negative one the opposite; Crescendo ramps velocity up (positive) or down (negative) along the strum; Use Current Scale reads the shifts in scale degrees; the context menu has Play Duplicate Notes When Strumming and Send Per Note Events to Generated Notes |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R34 | As in the transposing spec |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules", and the Determinism requirement | No allocation, lock, or blocking call on the audio thread; random choices repeat in an offline render |
| S5 | Manual | Live 12 reference manual, chapter 3, section 3.13 "Scale Awareness", and chapter 30's introduction | Chord is scale-aware: its Use Current Scale switch, in its title bar, applies the clip's scale and sets its pitch controls in scale degrees |
| S6 | Release notes | Live 12 release notes, 12.0 (Chord transforms per-note pitch bend to stay in the scale) and 12.1 (a switched-off Chord following a scale no longer changes per-note pitch bend) | A bent chord stays in the scale; a device switched off passes its input on |
| S7 | Beatlayer | [scales.md](scales.md) R2 and R15 to R18 | Moving a key by degrees; a device's current scale, its Use Current Scale switch, held notes and per-note bends |

## Rules

- **R1.** A note-on brings its added notes with it on the same frame when Strum is 0: the note first, then the shifts in order, each at its own key, with the note's channel; a note-off ends all of them with its release velocity. (S1)
- **R2.** A shift of 0, or one equal to an earlier shift's, adds nothing. (S1)
- **R3.** An added note's velocity is the note's times its shift's Velocity (1 to 200 per cent), held between 1 and 127. (S1)
- **R4.** An added note sounds with its shift's Chance, from 0 to 100 per cent, by its own draw each time. The note itself always sounds. (S1)
- **R5.** With Strum other than 0, the notes sound over |Strum| ms, from the first to the last, each on its exact frame; the note first and then the shifts in order when Strum is positive, the reverse when negative. (S1)
- **R6.** Tension shapes the strum: with positive Tension the gaps between notes shrink along the strum, with negative they grow, and with 0 they are equal. (S1)
- **R7.** Crescendo ramps the velocity along the strum: up with positive values, down with negative. (S1)
- **R8.** A key that comes up before the strum finishes ends the added notes that sounded and cancels those still waiting, so nothing hangs and nothing sounds after its key. (S1, S2)
- **R9.** With Play Duplicate Notes off, a strum doesn't sound a key that is already sounding or waiting to sound; with it on, it does. A chord without a strum plays every key. (S1)
- **R10.** With Per-Note Events on, each added note has an ID of its own, different from its source's and from every other added note's, and the source's per-note expressions are sent to each. With it off, added notes have no ID and only the source gets its expressions. (S1, S2)
- **R11.** Two renders with Chance are bit for bit the same. (S4)
- **R12.** Several hundred notes in one block, each bringing six, go through without allocating; notes past the device's room (1,024 sounding or waiting) are dropped and counted, and a note-off for a note that was dropped passes through. (S2, S4)
- **R13.** While Learn is on, the keys played set the shifts: the first key down after all keys were up is the root, and the other keys held with it become Shift 1 to 6 in the order they went down, each as its distance from the root in semitones; shifts past the chord go to 0; a key added while the chord is held sets the next shift. Each change of the chord is one undo step, "Learn Chord". With Learn off, playing changes nothing. (S1)
- **R14.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Chord/`. (S3)
- **R15.** Every parameter is automatable and has a description, and nothing Chord does on the audio thread allocates, locks, or blocks. (S3, S4)
- **R16.** Use Current Scale, a switch in Chord's title bar ([scales.md](scales.md) R16), off in a new Chord: with it on, each shift counts degrees of the device's current scale ([scales.md](scales.md) R15), from −36 to +36 (the semitone range read as degrees, Beatlayer's own), and each added note is that many degrees from the note ([scales.md](scales.md) R2). The added notes of a note outside the scale are counted from the scale note below it, so they land in the scale while the note itself sounds as it came (Beatlayer's own). R2 holds in degrees: a shift of 0, or one that lands where an earlier shift's note does, adds nothing; an added note off the keyboard isn't played. (S1, S5, S7)
- **R17.** While Learn is on with Use Current Scale on, each learned key sets its shift to the number of degrees from the root to it, a key outside the scale counting as the scale note below it (Beatlayer's own). (S1, S7)
- **R18.** With Use Current Scale and Per-Note Events on, the per-note bend sent to each added note is carried in degrees, as [scales.md](scales.md) R18 says, so a bent chord stays in the scale; a change of the current scale while a chord is held moves none of its notes (R8). A Chord switched off passes the bends it receives on unchanged ([device-chains.md](device-chains.md) R6). (S1, S6, S7)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/stack.rs`, `a_note_brings_its_shifts_on_the_same_frame` | real-time trap |
| R2 | `crates/devices/tests/stack.rs`, `a_repeated_or_zero_shift_adds_nothing` | real-time trap |
| R3 | `crates/devices/tests/stack.rs`, `each_shift_has_its_own_velocity` | real-time trap |
| R4 | `crates/devices/tests/stack.rs`, `each_shift_has_its_own_chance` | real-time trap |
| R5 | `crates/devices/tests/stack.rs`, `a_strum_spreads_the_chord_over_its_time` | real-time trap |
| R6 | `crates/devices/tests/stack.rs`, `tension_shapes_the_strum` | unit, real-time trap |
| R7 | `crates/devices/tests/stack.rs`, `crescendo_ramps_the_velocity` | real-time trap |
| R8 | `crates/devices/tests/stack.rs`, `notes_that_have_not_sounded_when_the_key_comes_up_never_do` | real-time trap |
| R9 | `crates/devices/tests/stack.rs`, `a_strum_skips_a_key_already_sounding` | real-time trap |
| R10 | `crates/devices/tests/stack.rs`, `per_note_events_give_each_added_note_its_own_id_and_expressions` | real-time trap |
| R11 | `crates/devices/tests/stack.rs`, `two_renders_are_identical` | real-time trap |
| R12 | `crates/devices/tests/stack.rs`, `several_hundred_notes_in_a_block_allocate_nothing` | real-time trap |
| R13 | `crates/shell/tests/midi_io.rs`, `learn_sets_stacks_shifts_from_the_chord_played` | unit |
| R14 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R15 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/stack.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |
| R16 | to be written in WP-6.8 | |
| R17 | to be written in WP-6.8 | |
| R18 | to be written in WP-6.8 | |

## Defaults, ranges and details

- A new Chord starts with Shift 1 at +4 and Shift 2 at +7 (a major triad) and the other shifts at 0, every Velocity 100 %, every Chance 100 %, Strum 0, Tension 0, Crescendo 0, and every switch off. The manual doesn't state the defaults.
- Strum runs from −400 to +400 ms and is the time from the first note of the chord to the last. The manual says "up to 400 ms between the notes" without saying whether that is each gap or the whole; the whole is used here. Note k of n sounds at |Strum| times (1 − (1 − k/(n − 1))^q) after the first, with q = 2^(2 × Tension), Tension from −1 to 1.
- Crescendo multiplies note k of n by 1 + Crescendo × (k/(n − 1) − ½), Crescendo from −1 to 1.
- Learn is heard on the main thread, not in the device: the MIDI input queue copies note messages for the shell while any Chord device has Learn on (`bl_midi_io::InputSender::set_tap`), and the shell sets the shifts. The chord is learned from the keys the computer keyboard and the MIDI inputs send, whichever track they play.
- The six lights beside the shifts are not drawn: they need the engine to publish a device's display state.
- The context menu's two options are switches among the device's parameters, `Play Duplicate Notes` and `Per-Note Events`.
