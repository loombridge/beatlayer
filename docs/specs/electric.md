# Electric: an electric piano of a hammer, a fork, a damper and a pickup

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-8.23 (this spec, R1 to R14). The tine and tone bar are modal resonators of [the physical models](physical-models.md); voices and per-note bends are [the instruments' voice section](instrument-voices.md); the oversampler is [the device framework](device-framework.md) R8
- **Parity-matrix row:** Instruments: Collision, Tension, Electric (Electric)
- **Checked against:** the Live 12 reference manual (online edition), read 30 October 2026 (chapter 31, section 31.5: 31.5.1 to 31.5.5); the Live 12 release notes, 12.0 (every instrument plays MPE)

## The behaviour

Electric models an electric piano. A key throws a hammer at a fork: a thin tine that the hammer strikes and a heavier tone bar coupled to it, which rings on. A pickup beside the tine turns its motion into the sound, adding the harmonics of its own nonlinearity as the tine swings closer and further; a damper stops the fork when the key is released and adds its own small noises. The global section holds the volume, the voices, the tuning and the bend ranges.

The device's sections, Hammer, Fork and Damper/Pickup, are chosen by icons, with the global section beside them. Every law here is Beatlayer's own reading of the manual's controls on a modal model of the fork.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 31 "Live Instrument Reference", section 31.5.1 "Architecture and Interface" | Hammer, Fork, Damper/Pickup and Global sections, the first three chosen by their icons |
| S2 | Manual | Section 31.5.2 "Hammer Section" | Stiffness (harder: brighter), moved by velocity and key; Noise with its Pitch (the noise's centre), Decay and Key (its level following the key); Force's Amount (soft to hard), moved by velocity and key |
| S3 | Manual | Section 31.5.3 "Fork Section" | The tine, the struck part, with Color (the balance of high and low partials), Decay (while the key is held) and Key (its level following the key); the tone bar, a second resonance, with its own Decay; Release, the fork's decay after the key is let go, for both |
| S4 | Manual | Section 31.5.4 "Damper/Pickup Section", 31.5.4.1 "Pickup Parameters" and 31.5.4.2 "Damper Parameters" | The pickup's Symmetry (its height against the tine; 50 % straight in front, brighter), Distance (closer, more overdrive), Type R (electrodynamic) or W (electrostatic), Input (the fork's level into it, driving its distortion) and Output (moved by the key); the damper's Tone (soft to bright), Level (its noise) and Att/Rel (−100 % its noise at the attack only, 100 % at the release only, 0 both) |
| S5 | Manual | Section 31.5.5 "Global Section" | Volume; Voices; Semi; Detune (±50 cents); Stretch (0 % equal temperament; positive stretching the upper notes sharp and the lower flat); Pitch Bend and Note PB ranges in semitones |
| S6 | Beatlayer | [The physical models](physical-models.md) R1 to R7 and R12 to R17; [the instruments' voice section](instrument-voices.md); [the device framework](device-framework.md) R8, R29 and R41 to R45 | The Cantilever and Beam resonators; the shared voices; the oversampler and its latency; seeded randomness; pages |

## Rules

- **R1.** Beatlayer ships Electric (`beatlayer.electric`), an instrument listed under the browser's Instruments label, with factory presets of Beatlayer's own. Its body is a shell and display (device framework R41): sections Hammer, Fork and Damper/Pickup chosen by their buttons, the selected one's controls filling the display, and the global section always shown. (S1, S6)
- **R2.** The hammer strikes the tine once per note with a force Force's Amount (0 to 100 %) sets, moved by velocity and key (Vel and Key amounts, −100 % to 100 %): a raised-cosine pulse whose width Stiffness sets (soft, 5 ms; hard, 0.2 ms), moved by velocity and key, so a harder hammer is shorter and brighter. Noise (0 to 100 %) adds a burst of band-pass noise from the device's seed, one stream per note, at Pitch (100 Hz to 10 kHz) decaying over Decay (1 ms to 1 s), its level following the key by Key (−100 % to 100 %, up to 12 dB over four octaves from C3); the burst enters the pickup with the tine's motion. (S2; the pulse and ranges are Beatlayer's own)
- **R3.** The tine is the physical models' Cantilever resonator at the note's pitch (physical models R1); Color (−100 % to 100 %) tilts its partials' gains from the fundamental to the upper partials (physical models R6's Brightness); Decay (100 ms to 60 s) is its decay while the key is held; Key (−100 % to 100 %) moves its level with the key by up to 12 dB over four octaves from C3. (S3, S6)
- **R4.** The tone bar is a Beam resonator (physical models R1) tuned to the note's fundamental and driven by the tine, so the fundamental rings on after the tine's high partials die; its Decay (100 ms to 60 s) is its own. (S3, S6; the coupling is Beatlayer's own)
- **R5.** Release (10 ms to 10 s) is the fork's decay, tine and tone bar together, after the key is let go: from the note's end both fall 60 dB within Release, and a tine or tone bar that already decays faster keeps its own decay, since the damper only shortens one. (S3)
- **R6.** The tine and tone bar ring at their pitch across the keyboard within 1 cent, and their decay follows Decay and Release within 5 %. (S3, S6)
- **R7.** The pickup turns the tine's displacement x into a signal through a nonlinearity set by its geometry: Type R (electrodynamic) responds to the change of a magnetic field, as 1 / (d − x)² differentiated; Type W (electrostatic) to capacitance, as 1 / (d − x); each is a balanced pair, the same law mirrored on the other side of the tine, so that straight in front the response is odd and only odd harmonics come out, and R's differentiator passes the note's own pitch at unity; d is Distance (closer, more distortion), from one to four times the tine's full swing, and the tine's position is bent smoothly short of 0.92 d on either side; Symmetry (0 to 100 %, 50 % in front) offsets the tine's rest position against the pickup's centre, so away from 50 % even harmonics rise. Input (−∞ to +12 dB) drives the fork's signal into it and Output (−∞ to +12 dB, moved by Out Key, −100 % to 100 %, up to 12 dB over four octaves from C3) sets its level. (S4; the laws are Beatlayer's own readings of the two pickup types)
- **R8.** The pickup's nonlinearity runs at four times the sample rate through the device framework's oversampler (device framework R8), and the device reports that latency. (S6)
- **R9.** The damper lands when a note ends, stopping the fork as R5 says, and adds a short noise: Tone (dark to bright) its colour, Level (−∞ to 0 dB) its level, and Att/Rel (−100 % to 100 %) where it sounds, at the note's start (the damper lifting), its end (landing) or both; it is a burst of about a tenth of a second added after the pickup. (S4)
- **R10.** The global section: Volume (−∞ to 0 dB), Voices (1 to 32, 16 by default; instrument voices R3), Semi (−48 to +48), Detune (−50 to +50 cents), Stretch (instrument voices R17), Pitch Bend (0 to 24 semitones, 2 by default) and Note PB (0 to 48, 48 by default; instrument voices R14). (S5)
- **R11.** Every control is a parameter, automatable and modulatable (rule G2), and Electric's banks hold every one. (S6)
- **R12.** Sixteen voices render at 48 kHz inside the device budget. (S6)
- **R13.** The output is the same at any block size and run after run; nothing on the audio thread allocates, locks or blocks. (S6)
- **R14.** A note's sound depends on its velocity through the hammer's force and stiffness alone, as their Vel amounts give, so at every amount 0 two velocities sound alike but for level: the velocity scales the note's level after the pickup, so it never changes how hard the pickup is driven. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R11 | `crates/devices/tests/electric.rs`, `electric_is_an_instrument_with_the_three_sections_and_the_global_one`, `the_factory_presets_set_parameters_in_range_and_sound`; `crates/devices/tests/banks.rs`; `crates/devices/tests/rules.rs`; `crates/agent/tests/electric.rs`, `an_agent_sets_every_electric_parameter_and_reads_it_back` | unit; agent API |
| R2, R14 | `crates/devices/tests/electric.rs`, `a_hammers_pulse_sums_to_one_and_is_5_ms_soft_and_a_fifth_of_a_ms_hard`, `force_sets_the_attacks_height_and_stiffness_its_brightness`, `the_noise_is_a_burst_by_its_level_decay_pitch_and_key`, `velocity_moves_the_sound_only_through_the_amounts_and_otherwise_only_its_level` | unit under the real-time trap (the attack's height and spectrum against force and stiffness) |
| R3, R4, R5, R6 | `crates/devices/tests/electric.rs`, `the_fork_rings_at_its_pitch_within_a_cent_across_the_keyboard`, `the_tine_decays_as_its_decay_says`, `the_tone_bar_rings_on_for_its_own_decay_after_the_tine_has_died`, `the_fork_falls_60_db_within_release_after_the_key_is_let_go`, `key_moves_the_tines_and_the_pickups_level_by_up_to_12_db_over_four_octaves` | render (pitch within 1 cent; decays within 5 %) |
| R7, R8 | `crates/devices/tests/electric.rs`, `symmetry_and_distance_shape_the_pickups_harmonics`, `the_pickup_never_divides_by_zero_however_hard_it_is_driven`, `the_device_reports_the_oversamplers_latency` | unit (harmonics against Symmetry and Distance; latency reported) |
| R9, R10 | `crates/devices/tests/electric.rs`, `the_damper_sounds_at_the_start_the_end_or_both_by_att_rel`, `semi_detune_and_stretch_tune_the_fork`, `volume_scales_the_output_and_voices_limits_the_notes`, `pitch_bend_sets_the_wheels_range_and_zero_means_no_bend` | render |
| R12 | `cargo bench -p bl-devices --bench electric`; `crates/devices/tests/electric.rs`, `sixteen_voices_render_with_no_allocation` | benchmark against 10 % of real time for sixteen voices at 48 kHz and 128 frames; real-time trap |
| R13 | `crates/devices/tests/electric.rs`, `the_render_is_the_same_at_every_block_size_and_run_after_run`; `crates/cli/tests/golden.rs`, the `electric` fixture | render at block sizes 1, 64, 512 and 4,096; golden render |
