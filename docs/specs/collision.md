# Collision: mallet and noise exciters, two resonators, LFOs and the MIDI/MPE tab

- **Status:** built (WP-8.21), except R6's X-Y control and R10's Copy buttons, which have no control kind in the device layout yet: the command, `collision.copy_resonator`, is built and tested
- **Work packages:** WP-8.21 (this spec, R1 to R20). The resonators are [the physical models](physical-models.md); voices and per-note bends are [the instruments' voice section](instrument-voices.md); the routings ride on [the instruments' modulation matrix](instrument-modulation.md); the tabs are [the device framework](device-framework.md)'s pages (R41 to R45)
- **Parity-matrix row:** Instruments: Collision, Tension, Electric (Collision)
- **Checked against:** the Live 12 reference manual (online edition), read 30 October 2026 (chapter 31, section 31.2: 31.2.1 to 31.2.6.1; 31.2.7 holds tips, not behaviour)

## The behaviour

Collision models mallet percussion: a mallet and a noise source excite two resonators, each a bar, a marimba bar, a string, a membrane, a plate, a pipe or a tube, linked in series (the first resonator's sound exciting the second) or in parallel (the exciters reaching both). The mallet's stiffness and noise shape the strike; the noise source has its own filter and envelope, so it can also blow a pipe for as long as a key is held. Each resonator has its decay and material, its tuning and its mixer, and positions where it is struck and heard. Two LFOs and the MIDI/MPE tab's sources move the device's controls, and the key and velocity reach many of them directly.

The device's sections sit on tabs: Mallet, Noise, Resonator 1, Resonator 2, LFO and MIDI/MPE, with the global section beside them.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 31 "Live Instrument Reference", section 31.2.1 "Architecture and Interface" | The Mallet, Noise, Resonator 1, Resonator 2, LFO and MIDI/MPE tabs and a global section; the Mallet and Noise switches, silent with both off; the Structure, Serial 1 > 2 (Resonator 1 into Resonator 2) or Parallel 1 + 2 (the exciters' mix into both) |
| S2 | Manual | Section 31.2.2 "Mallet Section" | Volume, Stiffness (harder: brighter and shorter), Noise (the impact's noise) and Color (the noise's frequency, doing nothing with Noise at 0), each of the first three moved by the key and velocity |
| S3 | Manual | Section 31.2.3 "Noise Section" | Its switch; a filter of LP, HP, BP or LP+HP with Freq and Res (a bandwidth for LP+HP), the key and velocity moving Freq; Env Amt, the envelope's amount on the filter; Volume; an envelope of Attack, Decay, Sustain and Release, Sustain at 0 removing the sustain and at 100 the decay |
| S4 | Manual | Section 31.2.4 "Resonator Tabs", 31.2.4.1 "Tuning Section" and 31.2.4.2 "Mixer Section" | Each resonator's switch, silent with both off; its type and Quality; Copy (1 → 2 or 2 → 1); an X-Y control of Decay across and Material or Radius up; Decay, Material, Radius, Ratio, Brightness, Inharm, Opening, Hit, Rnd, Note Off (how far a note's end mutes it: 0 % ignores it, 100 % mutes at once), Pos. L and Pos. R; Tune (coarse, semitones, moved by the key), Fine (±50 cents), Key (100 % equal temperament, 200 % doubled intervals, negative reversed), Pitch Env (a starting pitch, moved by velocity) and Time; Gain, Pan (moved by the key) and Bleed (the exciter's signal mixed in) |
| S5 | Manual | Section 31.2.5 "LFO Tab" | LFO 1 and LFO 2, each with its switch; Sine, Square, Triangle, Sawtooth Up, Sawtooth Down, Noise type 1 (stepped) and Noise type 2 (smooth); Offs. and Retrigger; two destinations with amounts, scaled by the LFO's Amount (moved by velocity); Rate in hertz or synced, moved by the key |
| S6 | Manual | Section 31.2.6 "MIDI/MPE Tab" and 31.2.6.1 "The Global Section" | Pitch bend (per-note included), mod wheel, pressure and slide as sources, each with two destinations and amounts; the Key and Vel sliders for the mallet, noise, resonator and LFO controls; Voices; Retrig.; Structure; Volume |
| S7 | Beatlayer | [The physical models](physical-models.md); [the instruments' voice section](instrument-voices.md) and [modulation matrix](instrument-modulation.md); [the device framework](device-framework.md) R29 and R41 to R45 | Resonators, voices, routes, seeded randomness and pages |

## Rules

- **R1.** Beatlayer ships Collision (`beatlayer.collision`), an instrument listed under the browser's Instruments label, with factory presets of Beatlayer's own. Its body is in pages (device framework R41), tabs along its body's top: Mallet, Noise, Resonator 1, Resonator 2, LFO and MIDI/MPE, the global section always shown beside them. (S1, S7)
- **R2.** The Mallet section, with its switch (on in a new device), strikes once per note: a raised-cosine pulse whose width Stiffness sets (0 %, 20 ms; 100 %, 0.1 ms), so a stiffer mallet is shorter and brighter, plus a burst of noise from the device's seed as long as the pulse, its level set by Noise and its centre frequency by Color (200 Hz to 16 kHz) through a band-pass; Volume sets the strike's level. Key and velocity move Volume, Stiffness and Noise by their Key and Vel amounts (R16). (S2; the pulse and its widths are Beatlayer's own)
- **R3.** The Noise section, with its switch (off in a new device), plays white noise from the device's seed through a filter of LP, HP, BP or LP+HP (a band between a high-pass and a low-pass, Res setting its width) at Freq (20 Hz to 20 kHz) and Res, moved by its envelope by Env Amt (−100 % to 100 %, up to six octaves) and by the key and velocity; its envelope (Attack, Decay, Release from 0 ms to 20 s; Sustain 0 to 100 %) shapes its level, so held notes keep exciting the resonators while Sustain is above 0. (S3)
- **R4.** With both exciters off the device is silent. (S1)
- **R5.** Each resonator, with its switch (Resonator 1 on and Resonator 2 off in a new device), is one of Beam, Marimba, String, Membrane, Plate, Pipe and Tube from the physical models (physical models R1 to R11), with Quality (Eco, Low, Medium or High; not for Pipe or Tube) setting its partial count (physical models R3). (S4, S7)
- **R6.** Decay, Material (Beam, Marimba, String, Membrane, Plate), Radius (Pipe, Tube), Ratio (Membrane, Plate), Brightness, Inharm, Opening (Pipe) and the positions Hit, Pos. L and Pos. R drive the physical models' laws (physical models R2, R4 to R7, R9 to R11); Rnd varies Hit at each note by up to its value, from the device's seed; controls that don't apply to the chosen type are greyed and change nothing. The X-Y control moves Decay across and Material or Radius up, as one undo step per drag. (S4, S7)
- **R7.** Note Off (0 to 100 %) multiplies a resonator's decay, when its note ends, by 1 − Note Off × 0.99 over 10 ms, so at 0 % a note's end changes nothing and at 100 % it falls silent within a few milliseconds. (S4; the law is Beatlayer's own)
- **R8.** A resonator's pitch is the note's times its Key (−100 % to 200 %, 100 % by default: the interval from C3 multiplied by Key / 100 %), plus Tune (−48 to +48 semitones) and Fine (−50 to +50 cents), plus its pitch envelope, which starts Pitch Env (−48 to +48 semitones, moved by velocity) away and reaches the pitch in a straight line in semitones over Time (0 ms to 10 s). (S4; the ranges are Beatlayer's own)
- **R9.** Each resonator's mixer: Gain (−∞ to +12 dB), Pan (moved by the key) and Bleed (0 to 100 %, the share of the exciter's signal mixed into the resonator's output). (S4)
- **R10.** Copy copies every setting of one resonator to the other (`collision.copy_resonator`, `edit`, one undo step). (S4; the command's name is Beatlayer's own)
- **R11.** Structure, Serial 1 > 2 (the default) or Parallel 1 + 2, links the resonators: in series the exciters reach Resonator 1 and Resonator 1's output excites Resonator 2, both heard; in parallel the exciters reach both. With both on in series at full Decay and Material, the output stays bounded (physical models R13). (S1, S4)
- **R12.** LFO 1 and LFO 2, each with its switch (off in a new device), play Sine, Square, Triangle, Saw Up, Saw Down, Noise 1 (stepped random values) or Noise 2 (smooth ramps between random values) per voice, at Rate (0.01 to 40 Hz, or synced from 1/64 to 8 bars; moved by the key), with Offs. (0 to 360°) and Retrigger; each has two destinations (chosen slots of the matrix, instrument modulation R2) with amounts (−100 % to 100 %), scaled by Amount (0 to 100 %, moved by velocity). (S5)
- **R13.** The MIDI/MPE tab: Pitch Bend, Mod Wheel, Pressure and Slide each have two destinations with amounts, chosen slots of the matrix; Pressure and Slide are per-note under MPE, and the per-note bend follows instrument voices R14 with 48 semitones per ±1. (S6)
- **R14.** The destinations every LFO and MIDI/MPE slot offers are Off and each of the device's continuous controls: the mallet's Volume, Stiffness, Noise and Color; the noise's Freq, Res, Env Amt and Volume; each resonator's Decay, Material or Radius, Brightness, Inharm, Opening, Hit, Pos. L, Pos. R, Tune, Fine, Gain, Pan and Bleed; the other LFO's Rate and Amount; and Volume. (S5, S6; Beatlayer's own list)
- **R15.** The global section: Voices (1 to 32, 8 by default) and Retrig. (on in a new device) follow instrument voices R3 and R5; Structure (R11); Volume (−∞ to 0 dB). (S6)
- **R16.** Each control the manual says the key or velocity moves has a Key and a Vel amount (−100 % to 100 %) on the MIDI/MPE tab: Key moves it by its amount times (key − 60) / 48 of its travel, Vel by its amount times the velocity's 0-to-1 reading. (S2 to S6; the scale is Beatlayer's own)
- **R17.** Every control is a parameter, automatable and modulatable (rule G2), and Collision's banks hold every one. (S7)
- **R18.** Sixteen voices with both resonators at Medium render at 48 kHz inside the device budget. (S7)
- **R19.** The output is the same at any block size and run after run; nothing on the audio thread allocates, locks or blocks. (S7)
- **R20.** A held note on a Pipe or Tube resonator, excited by the Noise section with Sustain above 0, keeps sounding until the note ends. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R17 | `crates/devices/tests/collision.rs`, `collision_is_an_instrument_with_its_tabs_its_matrix_and_every_parameter_banked`, `the_factory_presets_set_parameters_the_device_has_and_sound`; `crates/devices/tests/banks.rs`; `crates/devices/tests/rules.rs` | unit (tabs, matrix, banks, presets) |
| R2 | `crates/devices/tests/collision.rs`, `a_stiffer_mallet_excites_the_upper_partials_and_noise_adds_to_them`; `crates/devices/src/collision.rs`, `the_strike_is_a_raised_cosine_whose_width_stiffness_sets` | unit under the real-time trap (the strike's width and spectrum) |
| R3 | `crates/devices/tests/collision.rs`, `the_noise_filters_its_types_and_envelope_shape_what_it_blows` | render under the real-time trap (the noise filter and envelope) |
| R4 | `crates/devices/tests/collision.rs`, `with_both_exciters_off_or_both_resonators_off_it_is_silent` | render |
| R5 | `crates/devices/tests/collision.rs`, `each_type_rings_at_the_notes_pitch` | render under the real-time trap (each type at its pitch) |
| R6 | `crates/devices/tests/collision.rs`, `decay_material_inharm_the_positions_and_radius_move_what_rings`, `controls_that_dont_apply_to_a_type_change_nothing` | render under the real-time trap (controls moving it; the others changing nothing) |
| R7 | `crates/devices/tests/collision.rs`, `note_off_mutes_a_resonator_when_its_note_ends` | render |
| R8 | `crates/devices/tests/collision.rs`, `pitch_is_the_notes_times_key_plus_tune_fine_and_a_pitch_envelope` | render |
| R9 | `crates/devices/tests/collision.rs`, `gain_pan_and_bleed_mix_each_resonator` | render |
| R10 | `crates/shell/tests/collision.rs`, `copy_sends_every_setting_of_a_resonator_to_the_other_in_one_undo_step`, `copy_refuses_a_resonator_that_isnt_there_and_a_device_that_isnt_a_collision`; `crates/agent/tests/collision.rs`, `an_agent_sets_and_reads_back_every_parameter_copies_a_resonator_and_reads_the_routes`; `cargo xtask commands` | shell test; agent API |
| R11 | `crates/devices/tests/collision.rs`, `series_and_parallel_link_the_resonators_and_the_worst_case_stays_bounded` | render (series and parallel; the worst case bounded) |
| R12, R13, R14, R16 | `crates/devices/tests/collision.rs`, `an_lfo_moves_its_destinations_by_its_shape_rate_and_amount`, `the_midi_sources_reach_their_destinations`, `key_and_vel_move_the_controls_the_manual_names`; `crates/devices/src/collision.rs`, `every_chooser_lists_off_then_the_destinations_by_their_parameters_names` | render (each source reaching its destinations) |
| R15 | `crates/devices/tests/collision.rs`, `voices_retrigger_and_volume_follow_the_global_section` | render |
| R18 | `crates/devices/tests/collision.rs`, `sixteen_voices_with_both_resonators_at_medium_render_inside_the_device_budget` | benchmark against the device budget (judged at `BL_TEST_DEPTH=full`) |
| R19 | `crates/devices/tests/collision.rs`, `the_output_is_the_same_at_every_block_size_and_run_after_run` | render at block sizes 1, 64, 512 and 4,096; repeated renders identical |
| R20 | `crates/devices/tests/collision.rs`, `a_held_pipe_or_tube_blown_with_noise_keeps_sounding_until_the_key_ends` | render |
