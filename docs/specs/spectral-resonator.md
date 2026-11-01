# Spectral Resonator: harmonic resonances in the spectrum, played by notes

- **Status:** partial (everything is built but the Hz or pitch mode buttons of Freq and its note readout, R5 and R8, which wait for a decision on where the readout is drawn)
- **Work packages:** WP-8.48 (all of it: R1 to R20). The short-time Fourier transform and the spectrogram display kind are Beatlayer's own, `bl_dsp::stft` (WP-8.28, [the architecture](../02-architecture.md)) and [the device framework](device-framework.md)'s spectrogram rule; MIDI sidechains are [routing](routing.md) R32 to R35 and R39; what every scale-aware device shares is [scales](scales.md) R15 to R18; tunings are [tunings](tunings.md)'
- **Parity-matrix row:** Audio effects: Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.36 "Spectral Resonator" with 29.36.1 to 29.36.6, read 30 October 2026; the Live 12.4.3 release notes

## The behaviour

Spectral Resonator makes any sound ring at a pitch and its harmonics, in the spectral domain. The input's spectrum is drawn toward a comb of partials at a fundamental frequency, each partial ringing on with its own decay, damped in the highs and lows as the person sets; Stretch spreads the partials apart or squeezes them together, Harmonics sets how many there are, and Shift moves the input's spectrum before it rings. The fundamental is set by a control or by notes from a MIDI track, one voice or up to sixteen, with glide and pitch bend. A modulation section adds chorus, a wandering motion or granular scatter to the partials, Unison thickens them, and a spectrogram shows the result.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.spectral_resonator`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.36 "Spectral Resonator", its pitch mode | Internal or MIDI; in MIDI, the external source and tapping point, Mono or Poly with Polyphony (2 to 16 voices), MIDI Gate (on or off; always on in Poly), Glide (ms, Mono only) and PB (0 to 24 semitones) |
| S2 | Manual | The same section, its frequency section | Freq, in Hz or as a pitch; Use Current Scale, which turns Freq into SD Shift; Transp. (±48 semitones, or ±28 scale degrees with Use Current Scale); Stretch (below 0 % compressing, above 0 % expanding, 100 % only the odd harmonics); Shift (±48 semitones, on the input only); Decay; HF Damp; LF Damp |
| S3 | Manual | The same section, its modulation section, spectrogram and global parameters | The modes None, Chorus, Wander and Granular, with Mod Rate and Pch. Mod; Harmonics, the number of partials, more being brighter; Quantize, which keeps partials on the scale or tuning, chromatic without one; Input Send with a limiter lamp; Unison (the voice count) and Uni. Amt (its detune); Dry/Wet |
| S4 | Release notes | Live 12.4.3 release notes | Control-surface mappings of the spectral devices improved |
| S5 | Beatlayer | `bl_dsp::stft`; [routing](routing.md) R32 to R35, R39; [scales](scales.md) R15 to R18; [tunings](tunings.md) R1, R6, R12, R16; [the device framework](device-framework.md) R29, R30, R35 to R39 and its spectrogram rule; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | The STFT; MIDI sidechains; current scales; tunings; seeds, notes, channel messages, displays; banks; gains and automation |

## Rules

### The device and the resonance

- **R1.** Spectral Resonator (`beatlayer.spectral_resonator`) is an audio effect with a MIDI sidechain (routing R32), Live's name, Beatlayer's own layout and one display cell, the spectrogram (R15). It is scale-aware, with Use Current Scale in its title bar ([scales](scales.md) R16). (S1 to S3, S5)
- **R2.** The input, raised by Input Send (a gain from −∞ to +12 dB, 0 dB, rule G1) and kept below 0 dBFS by a limiter whose lamp lights while it acts, is analysed by `bl_dsp::stft` (a 2,048-frame Hann window at 48 kHz, scaled with the rate, four times overlap). Each voice's partials sit at f_k = f0 · k^(1 + s) for k from 1 to Harmonics (1 to 64, 32 by default), where f0 is the voice's fundamental and s is Stretch (−100 to +100 %, 0) as a fraction; at Stretch +100 % only the odd partials (k = 1, 3, 5 …) sound, as the manual says, the law moving between. Below 0, each partial is lowered by the number of partials to the power of half the Stretch's depth, so squeezing them onto one another keeps the sum as loud. Partials above the Nyquist frequency are dropped. Each partial's resonance falls within 5 cents of f_k. (S2, S3; the law, the window and the ranges are Beatlayer's own)
- **R3.** Each partial takes the input's level at its frequency (the larger of the two analysis bins around it, read as the amplitude of a sine on the bin) and rings on as a sine at exactly its frequency: its level is the larger of what it rings with and what the input brings, and it falls 60 dB in Decay (50 ms to 20 s, 1 s by default) for a partial at 1 kHz, faster above it by HF Damp (0 to 100 %, up to 8 times faster at 20 kHz) and below it by LF Damp (0 to 100 %, up to 8 times faster at 20 Hz). A partial's ring time is within 10 % of its law. Frequencies away from every partial are silent in the wet signal. (S2; the law is Beatlayer's own)
- **R4.** Shift (−48 to +48 semitones, 0) moves the input's spectrum by its interval before it reaches the resonances; the partials stay where they are. (S2)

### The pitch modes

- **R5.** In Internal, the fundamental is Freq (20 Hz to 5 kHz, logarithmic, 110 Hz by default), shown in Hz or as a note as its mode buttons choose, plus Transp. (−48 to +48 semitones, 0). With Use Current Scale on, Freq becomes SD Shift, a degree of the current scale (−28 to +28 degrees from the scale's root in the octave of C3), and Transp. counts degrees. (S2, S5; the ranges and the octave are Beatlayer's own)
- **R6.** In MIDI, the fundamental is the pitch of the source's notes ([routing](routing.md) R32, with its tapping point, R39) under the set's tuning, plus Transp. In Mono, the newest held note leads, gliding to a new note over Glide (0 to 5,000 ms, 0); with MIDI Gate on, the resonances sound only while a note is held, their Decay ringing out after the note-off; with it off, they ring at the last note. In Poly, each held note is a voice, up to Polyphony (2 to 16, 8 by default); a note past it takes the oldest voice's place; MIDI Gate is always on. PB (0 to 24 semitones, 2) scales the source's pitch bend (device framework R39), which moves every voice. Each note takes effect on its frame. (S1, S5; the stealing rule and defaults are Beatlayer's own)
- **R7.** With Quantize on, each partial is moved to the nearest note of the current scale (or, under a tuning, the tuning's nearest note); with no scale and no tuning, to the nearest semitone. (S3, S5)
- **R8.** Under a tuning, Freq's note readout, the MIDI notes' pitches and Quantize follow the tuning ([tunings](tunings.md) R1, R6, R12); under one that doesn't hold twelve notes, Use Current Scale is disabled ([tunings](tunings.md) R16). (S3, S5)

### Modulation and unison

- **R9.** Modulation is None, Chorus, Wander or Granular, with Mod Rate (0.01 to 20 Hz, 1 Hz) and Pch. Mod (0 to 12 semitones, 0.2): Chorus moves each partial's frequency by a sine of Pch. Mod depth at Mod Rate, each partial's phase offset by its number; Wander moves each by its own random line (the effects LFO's Wander, device framework R26) at Mod Rate; Granular gates each partial on and off at random at Mod Rate, a new draw each grain, and moves its pitch by up to Pch. Mod at random. Every random part follows the device's seed. (S3, S5; the laws are Beatlayer's own)
- **R10.** Unison (1 to 8 voices, 1) duplicates each voice's partials, detuned evenly across ±Uni. Amt (0 to 100 %, up to ±50 cents), spread between the channels. (S3; the laws are Beatlayer's own)
- **R11.** Dry/Wet, 0 to 100 % (100 %), balances the dry signal (delayed by the latency) against the resonances. The device reports the STFT's latency. (S3, S5)

### Throughout

- **R12.** Voices come from a pool of 16 made with the node, and Unison's copies from a pool sized for 16 voices of 8; allocating a voice, gliding and bending allocate, lock and block nothing on the audio thread. (S5)
- **R13.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, with every modulation mode and MIDI notes. (S5)
- **R14.** Agents set the MIDI source with `device.set_midi_sidechain_source` and read it back; the query tool's `device` result lists the sounding voices' fundamentals. (S5)
- **R15.** The spectrogram draws the wet output's magnitudes over the last four seconds against log frequency, with the partials of each sounding voice marked, in Beatlayer's own colour ramp, as [the device framework](device-framework.md)'s spectrogram kind lays it out; `device.display` reads it headless. (S3, S5)
- **R16.** Every parameter is automatable and modulatable (rule G2), Use Current Scale included, and set and read back through the agent API; every gain reaches −∞ dB. (S5)
- **R17.** Spectral Resonator declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Freq, Transp., Decay, Stretch, Shift, Harmonics, Input Send, Dry/Wet), Damping and Modulation (HF Damp, LF Damp, Modulation, Mod Rate, Pch. Mod, Unison, Uni. Amt, Quantize) and MIDI (Pitch Mode, Mono/Poly, Polyphony, MIDI Gate, Glide, PB), and a bank of its own, Scale, for SD Shift, which Freq becomes with Use Current Scale on, so every automatable parameter is in a bank. (S4, S5; Beatlayer's own)
- **R18.** A set with the device saves and reopens with every parameter and its MIDI source. (S5)
- **R19.** Use Current Scale follows the clip's scale on the frame it changes ([scales](scales.md) R15), and a held note keeps its pitch across a scale change ([scales](scales.md) R17). (S5)
- **R20.** Every control takes the keyboard focus in the Device View and assistive technology reads each with its name and value. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R17 | `crates/devices/tests/spectral_resonator.rs`, `the_description_has_a_midi_sidechain_the_switch_in_the_title_bar_and_banks_of_eight`; `crates/devices/tests/scales.rs`, `spectral_resonator_has_the_switch_in_the_title_bar`; `crates/devices/tests/banks.rs` | unit; rule over every device type |
| R1 to R4 | `every_partial_rings_within_5_cents_of_its_frequency`, `stretch_moves_the_partials_by_its_law_and_leaves_the_odd_ones_at_100_percent`, `squeezing_the_partials_together_keeps_the_level`, `harmonics_limits_the_number_of_partials`, `partials_above_nyquist_are_dropped`, `decay_and_the_damps_set_the_ring_time_within_10_percent`, `shift_moves_the_input_spectrum_and_not_the_partials`, `input_send_at_minus_infinity_leaves_the_resonances_silent`, `the_limiters_lamp_lights_while_it_acts` | unit (partials within 5 cents of the law; Stretch at −100, 0 and +100 %; Harmonics; partials above Nyquist dropped; Decay and the damps within 10 %; Shift; the limiter) |
| R5 to R8 | `internal_pitch_is_freq_and_transp_or_a_scale_degree_with_use_current_scale`, `notes_tune_the_resonance_on_their_frames_at_every_block_size`, `mono_returns_to_the_older_note_that_is_still_held`, `midi_gate_makes_the_resonances_sound_only_while_a_note_is_held`, `glide_slides_mono_to_the_next_note`, `pb_scales_the_pitch_bend`, `poly_rings_every_held_note_and_a_note_past_polyphony_takes_the_oldest_voice`, `a_released_voice_rings_out_and_is_taken_before_a_held_one`, `quantize_moves_partials_to_the_nearest_note_of_the_scale_or_the_semitone`, `under_a_19_note_tuning_quantize_and_the_notes_follow_the_tuning`, `a_midi_tracks_note_tunes_spectral_resonator_through_its_sidechain_on_its_frame` | engine test (MIDI notes tune it on their frames at three block sizes; Poly up to its count with the oldest stolen; MIDI Gate; glide; bend); unit (SD Shift in a scale; Quantize; under a tuning) |
| R9 to R11 | `chorus_moves_the_pitch_in_a_sine_of_the_depth_at_the_rate`, `wander_follows_a_random_line_within_the_depth_and_the_seed`, `granular_gates_and_scatters_the_partials_on_the_seed`, `unison_detunes_its_copies_evenly_and_spreads_them_between_the_channels`, `dry_is_delayed_by_the_reported_latency_and_dry_wet_balances_it` | unit (each modulation mode's law, seeded; Unison's detune; reported latency) |
| R12, R13 | `renders_are_bit_identical_at_every_block_size_and_run_after_run`, `every_voice_every_copy_and_every_partial_at_once_stay_bounded`; `rtsan/tests/devices.rs`, `spectral_resonator_passes_under_rtsan` | real-time trap, RealtimeSanitizer; renders at four block sizes |
| R14, R15 | `the_spectrogram_shows_the_partials_marked_and_reads_headless`, `the_display_lists_the_sounding_voices_fundamentals`; `crates/cli/tests/agent.rs`, `an_agent_sets_every_spectral_resonator_parameter_and_its_midi_source_and_reads_them_back`, `an_agent_reads_spectral_resonators_spectrogram_partials_and_fundamental_through_device_display` | agent API (`device.display` reads the spectrogram headless; the MIDI source read back) |
| R16 to R19 | `crates/devices/tests/rules.rs`; `crates/devices/tests/scales.rs`; `crates/devices/tests/banks.rs`; `a_set_with_spectral_resonator_saves_and_reopens_with_every_parameter_and_its_midi_source`, `a_scale_change_retunes_from_its_frame_and_a_held_note_keeps_its_pitch` | rules over every device type; save and reopen |
| R20 | to be written with the Device View's keyboard and assistive-technology pass | UI script |
