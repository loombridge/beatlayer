# Bass and Poli: a monophonic and a polyphonic virtual analogue synthesiser

- **Status:** built (R1 to R18: `bl_devices::bass` and `bl_devices::poli` on the instruments' voice section and filter, the chorus Chorus-Ensemble's own)
- **Work packages:** WP-8.16 (this spec, R1 to R18). Voices, glide and per-note bends are [the instruments' voice section](instrument-voices.md); the filter is [the instruments' filter](instrument-filter.md); Poli's chorus reuses [Chorus-Ensemble](chorus-ensemble.md)'s chorus
- **Parity-matrix row:** Instruments: Bass, Poli
- **Checked against:** Ableton's Creative Extensions pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026; neither device is described in the Live 12 reference manual or the Live 12 release notes, read 30 October 2026

## The behaviour

Bass and Poli are two instruments of Live's Creative Extensions pack. Bass is a monophonic virtual analogue synthesiser meant to cover the whole range of bass sounds in one device, from deep and clean to heavily distorted. Poli is a polyphonic virtual analogue synthesiser for classic chords and phrases, whose character comes from detunable oscillators, a modulation section and a built-in chorus.

Beatlayer builds both with Live's names, on the instruments' shared voice section and filter. The pack page names no controls, so every control, range and law below is Beatlayer's own, in the shape of a classic monophonic bass synthesiser and a classic polyphonic synthesiser.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Bass, a monophonic virtual analogue synthesiser covering bass sounds from deep and clean to heavily distorted; Poli, a virtual analogue synthesiser for classic polyphonic chords and phrases, with detunable oscillators, a modulation section and a built-in chorus; the pack's other devices (Melodic Steps, Pitch Hack, Gated Delay, Color Limiter, Re-Enveloper and Spectral Blur) |
| S2 | Beatlayer | [The instruments' voice section](instrument-voices.md) and [filter](instrument-filter.md); [Chorus-Ensemble](chorus-ensemble.md); [the polysynth](polysynth.md) R3 and R15 | The shared voices and filter; the chorus; band-limited oscillators and the velocity law |
| S3 | Beatlayer | [The device framework](device-framework.md) R1 to R16; [the improvements](../05-improvements.md), rules G1 and G2 | Descriptors, the generic panel, gains reaching −∞ dB, every parameter automatable and modulatable |

## Rules

**Both**

- **R1.** Beatlayer ships Bass (`beatlayer.bass`) and Poli (`beatlayer.poli`), instruments listed under the browser's Instruments label, each with its generic panel in pages (device framework R41) and factory presets of Beatlayer's own. (S1, S3)
- **R2.** Every control of both is a parameter, automatable and modulatable (rule G2), set by `device.set_param`, read by `device.parameters` and held in the device's banks; every gain reaches −∞ dB (rule G1). (S3)
- **R3.** Their oscillators are band-limited at every key, folding back less than −80 dB at the top key, as Drift's are ([polysynth](polysynth.md) R3). (S2)

**Bass**

- **R4.** Bass plays one voice (instrument voices R6): the newest held key sounds, and releasing it returns to the most recent key still held. (S1, S2)
- **R5.** A note that arrives while another key is held glides to its pitch over Glide (0 ms to 2 s, 60 ms by default) in a straight line in semitones and doesn't restart the envelopes; a note after every key was released starts at its pitch and restarts them (instrument voices R7, R9, with Legato always on). (S1; Beatlayer's own)
- **R6.** Bass's oscillator morphs from saw to square by Shape (0 to 100 %, 0 % by default), at Octave (−2 to +1, −1 by default) and Tune (−12 to +12 semitones); Sub (−∞ to 0 dB, −∞ by default) adds a sine an octave below, and Noise (−∞ to 0 dB, −∞ by default) adds white noise, drawn from a generator the device's seed and the number of the note seed afresh with each note. The saw falls, so that its fundamental and the square's agree and a morph never cancels one against the other: halfway, the second harmonic is a sixth of the fundamental. (S1; Beatlayer's own)
- **R7.** Bass's filter is the instruments' low-pass at 24 dB per octave with every circuit it offers (instrument filter R2), Freq (20 Hz to 20 kHz, 400 Hz by default), Res, Drive and Key (0 to 100 %); its envelope is an Attack and Decay (0 ms to 10 s each, 0 ms and 300 ms by default) moving the cutoff by Env Amount (−100 % to 100 %, 50 % by default, up to six octaves); it falls to nothing, and a key let go falls over the same Decay. Key moves the cutoff by that share of the note's distance from C3. (S1, S2; Beatlayer's own)
- **R8.** Bass's amplifier envelope has Attack, Decay, Release (0 ms to 10 s) and Sustain (0 to 100 %), Vel > Vol (0 to 100 %, Drift's law) and Volume (−∞ to 0 dB). After the amplifier, Distortion (0 to 100 %, 0 % by default) drives a soft clipper by up to 36 dB with its output scaled back by half the drive, for the heavily distorted end: the output is tanh(g x) / √g for the gain g of the drive, and at 0 % the signal passes as it is. Bass's pitch wheel moves its note by PB Range (0 to 24 semitones, 2 by default). (S1, S2; Beatlayer's own)

**Poli**

- **R9.** Poli plays up to Voices (1 to 16, 8 by default) through the voice section (instrument voices R3, R4), with Glide (0 ms to 2 s, 0 ms) as polyphonic Portamento and a pitch bend range (0 to 24 semitones, 2 by default). (S1, S2; Beatlayer's own)
- **R10.** Poli's two oscillators each have Shape (Saw, Square or Pulse), Octave (−2 to +2), Level (−∞ to 0 dB) and, on the second, Detune (−50 to +50 cents, 7 cents by default), so the two beat against each other; Pulse's width is PW (5 to 95 %, 50 % by default), one control for both oscillators; Square is a pulse of half width. Each oscillator's Level is −6 dB by default. (S1; Beatlayer's own)
- **R11.** Poli's filter is the instruments' low-pass or high-pass at 12 or 24 dB per octave with Clean, MS2 and PRD (instrument filter R2), Freq, Res, Key (0 to 100 %) and its own ADSR envelope with Env Amount (−100 % to 100 %, 0 % by default, up to six octaves). Its slope is 24 dB by default. Key moves the cutoff by that share of the note's distance from C3. (S2; Beatlayer's own)
- **R12.** Poli's amplifier envelope is an ADSR with Vel > Vol and Volume, as Bass's (R8) without the distortion. (S2; Beatlayer's own)
- **R13.** Poli's modulation section is an LFO (Sine, Triangle, Square, Saw or S&H; Rate 0.05 to 20 Hz or synced from 1/16 to 4 bars; Delay 0 ms to 5 s, fading in from the note's start) sent to Pitch (up to an octave), PW (its whole range) and Filter (up to four octaves) by three amounts from −100 % to 100 %, with the mod wheel scaling the LFO's depth by Wheel (0 to 100 %, 0 % by default). A full PW amount swings the width by 45 points either way, so the LFO sweeps the whole 5 to 95 %; the depth is scaled by 1 − Wheel + Wheel × the wheel, so at 100 % the LFO is silent with the wheel down and whole with it up; the Delay's fade is a straight line. The LFO runs per voice, restarting with each note: a Sine and a Triangle rise from their middle, a Square starts at its top and a Saw at its bottom, and S&H draws a value per cycle from a generator the device's seed and the note's number seed. (S1; Beatlayer's own)
- **R14.** Poli's chorus, after the amplifier, is Chorus-Ensemble's chorus ([Chorus-Ensemble](chorus-ensemble.md)) in its two-voice mode, with a switch (on in a new device), Rate, Amount and Dry/Wet, in stereo. (S1, S2; Beatlayer's own)

**Throughout**

- **R15.** Each device's output is the same at any block size and run after run; nothing either does on the audio thread allocates, locks or blocks. (S3)
- **R16.** Sixteen voices of Poli render at 48 kHz inside the device budget. (S3)
- **R17.** Every control's name is the one Beatlayer's panel shows, and every one reads its value with its unit (device framework R5). (S3)
- **R18.** A per-note bend on either device moves its note as instrument voices R14 says, with 48 semitones per ±1. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2, R17 | `crates/devices/tests/banks.rs`, `every_native_device_types_banks_hold_every_parameter_and_none_more_than_eight`; `crates/devices/tests/bass.rs`, `bass_is_an_instrument_that_describes_every_control_with_its_unit`, `every_factory_preset_sets_parameters_in_range_and_plays`; `crates/devices/tests/poli.rs`, `poli_is_an_instrument_that_describes_every_control_with_its_unit`, `every_factory_preset_sets_parameters_in_range_and_plays`; `crates/agent/tests/bass_and_poli.rs`, `an_agent_sets_and_reads_every_parameter_of_bass`, `an_agent_sets_and_reads_every_parameter_of_poli` | unit; agent API (`device.set_param`, `device.parameters`) |
| R3 | `crates/devices/tests/bass.rs`, `the_oscillator_aliases_below_minus_80_db_at_the_top_key`; `crates/devices/tests/poli.rs`, `every_shape_aliases_below_minus_80_db_at_the_top_key` | unit (fold-back at the top key) |
| R4, R5 | `crates/devices/tests/bass.rs`, `one_voice_plays_the_newest_key_and_returns_to_the_one_held`, `a_legato_note_glides_in_a_straight_line_without_retriggering_the_envelope`, `a_note_after_silence_starts_at_its_pitch_and_restarts_the_envelopes` | render under the allocation trap (one voice; legato glide without retrigger) |
| R6 | `crates/devices/tests/bass.rs`, `shape_morphs_from_a_saw_to_a_square`, `octave_and_tune_move_the_pitch_and_sub_adds_a_sine_an_octave_below`, `noise_comes_from_the_devices_seed_and_fills_between_the_harmonics` | render |
| R7 | `crates/devices/tests/bass.rs`, `the_filter_falls_24_db_an_octave_and_key_moves_the_cutoff`, `the_filter_envelope_moves_the_cutoff_over_its_decay`, `every_circuit_plays_bounded_and_drive_acts_on_all_but_clean` | render |
| R8 | `crates/devices/tests/bass.rs`, `the_amplifier_envelope_rises_falls_holds_and_releases`, `vel_vol_and_volume_scale_the_level_as_drifts_law_does`, `distortion_drives_a_soft_clipper_by_up_to_36_db_and_scales_back_by_half` | render |
| R6, R7, R8 | `crates/devices/tests/bass.rs`, `a_phrase_renders_as_it_did_when_the_device_was_written` | golden render |
| R9 | `crates/devices/tests/poli.rs`, `it_plays_up_to_voices_notes_and_the_oldest_makes_way_for_a_new_one`, `glide_is_polyphonic_from_the_note_before_and_the_wheel_has_its_range` | render under the allocation trap |
| R10 | `crates/devices/tests/poli.rs`, `two_oscillators_beat_by_their_detune_and_have_their_own_shape_octave_and_level` | render |
| R11 | `crates/devices/tests/poli.rs`, `the_filter_is_low_or_high_pass_at_12_or_24_db_and_key_follows_the_note`, `the_filter_envelope_has_four_stages_and_moves_the_cutoff` | render |
| R12 | `crates/devices/tests/poli.rs`, `the_amplifier_is_an_adsr_with_vel_vol_and_volume` | render |
| R13 | `crates/devices/tests/poli.rs`, `the_lfo_moves_the_pitch_up_to_an_octave_and_restarts_with_each_note`, `the_lfo_fades_in_over_its_delay_and_the_wheel_scales_its_depth`, `the_lfo_moves_a_pulses_width_and_the_cutoff` | render |
| R14 | `crates/devices/tests/poli.rs`, `the_chorus_is_chorus_ensembles_in_stereo_after_the_amplifier` | render (against a Chorus-Ensemble in Chorus mode with two taps, sample for sample) |
| R9 to R14 | `crates/devices/tests/poli.rs`, `a_phrase_renders_as_it_did_when_the_device_was_written` | golden render |
| R15 | `crates/devices/tests/bass.rs` and `crates/devices/tests/poli.rs`, `a_phrase_renders_the_same_at_every_block_size_and_every_run`, `a_phrase_that_moves_pitch_renders_the_same_at_block_sizes_64_512_and_4096` | render at block sizes 1, 64, 512 and 4,096 (a pitch in motion: 64 and up) |
| R16 | `crates/devices/tests/polysynth.rs`, `sixteen_voices_of_poli_render_faster_than_the_device_budget` | benchmark (judged at full depth) |
| R18 | `crates/devices/tests/bass.rs`, `a_per_note_bend_moves_its_note_by_48_semitones_a_unit_and_the_wheel_by_pb_range`; `crates/devices/tests/poli.rs`, `a_per_note_bend_moves_only_its_note_by_48_semitones_a_unit` | render |

## Not built yet

- The factory presets are `bass::presets` and `poli::presets`, named parameter sets of Beatlayer's own; nothing lists or loads them from the browser yet.
- While a pitch moves (a bend, the wheel, a glide, the LFO's Pitch), a render in one-frame blocks differs from the others in a few samples, because the plug-in kit's oscillator picks its mipmap from the pitch's rise over each render call; Drift does the same. From 64 frames a block up, the renders are the same.
