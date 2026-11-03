# Resonators: five tuned resonators

- **Status:** implemented
- **Work packages:** WP-8.43 (all of it: R1 to R15). What every scale-aware device shares is [scales](scales.md) R15 to R18; a device following the set's tuning is [tunings](tunings.md)'; the framework is WP-2.5's ([the native device framework](device-framework.md))
- **Parity-matrix row:** Audio effects: Corpus, Resonators, Vocoder
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.31 "Resonators", read 30 October 2026

## The behaviour

Resonators rings an input at five pitches. The input first passes a filter; then five resonators in parallel, each a tuned comb, ring at their pitches: the first set by a note and a fine tune, the other four by an interval above or below it, each with its own switch and gain. Decay sets how long they ring, Const keeps that time the same at every pitch, Color tilts their brightness, and Mode chooses between two resonator designs. Width spreads the resonators between the channels, and Gain and Dry/Wet set the output. With Use Current Scale on, the pitches are counted in degrees of the track's current scale; under a tuning system, in the tuning's notes.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.resonators`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.31 "Resonators" | The input filter, low-pass, band-pass, high-pass or notch, with its frequency; Mode A (more realistic) and B (suited to a low first resonator); Decay; Const; Color; Resonator I's switch, Gain, Note (C-1 to C5 in semitones, or in scale degrees, or in the tuning's notes) and Fine; Resonators II to V, each with its switch, Gain, Pitch (±24 semitones, or ±21 scale degrees) relative to I, and Fine; Width, Gain and Dry/Wet |
| S2 | Beatlayer | [Scales](scales.md) R15 to R18; [tunings](tunings.md) R1, R6, R12 and R16 | A scale-aware device's current scale and Use Current Scale switch; the pitch a key has under the set's tuning; scale controls disabled under a tuning that doesn't hold twelve notes |
| S3 | Beatlayer | [The device framework](device-framework.md) R4, R9; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2; [Filter Delay](filter-delay.md) | Smoothing, banks of eight, gains to −∞ dB, automatable parameters; comb and filter parts |

## Rules

- **R1.** Resonators (`beatlayer.resonators`) is an audio effect with Live's name, Beatlayer's own layout and no display cell: the filter section, the global section (Mode, Decay, Const, Color), the five resonators' columns, and Width, Gain and Dry/Wet. It is scale-aware: a Use Current Scale switch in its title bar, as [scales](scales.md) R16 says. (S1, S2)
- **R2.** The input filter is a 12 dB per octave low-pass, band-pass, high-pass or notch (band-pass in a new device) at its frequency (50 Hz to 18 kHz, 1 kHz by default, logarithmic), measured within 0.5 dB of its design, a second-order section of Q 1/√2 in every type. (S1; ranges, the slope, the Q and the default are Beatlayer's own)
- **R3.** Resonator I's pitch is Note (C-1 to C5, whole semitones, C3 by default) plus Fine (−50 to +50 cents). Resonators II to V each take Pitch (−24 to +24 semitones; defaults +12, +19, +24 and −12) plus their own Fine, relative to I's pitch. Each has a switch (I to III on, IV and V off in a new device) and Gain, a gain from −∞ to 0 dB (rule G1). (S1; the defaults are Beatlayer's own)
- **R4.** With Use Current Scale on, Note counts degrees of the device's current scale ([scales](scales.md) R15) from its root in the octave of C-1, and Pitch counts degrees (−21 to +21) from Resonator I's note within that scale; the Fines stay in cents. Note keeps its range of 0 to 84 in degrees, a key past 127 holding at 127, and Pitch counts at most 21 degrees. A change of current scale retunes the resonators from the frame it happens, gliding over 20 ms. (S1, S2)
- **R5.** While the set has a tuning ([tunings](tunings.md)), Note counts the tuning's notes and Pitch its steps, each pitch being the frequency the tuning gives that note, plus Fine; under a tuning that doesn't hold twelve notes to its pseudo-octave, Use Current Scale is disabled as [tunings](tunings.md) R16 says. Without a tuning, pitches are twelve equal steps from A3 = 440 Hz. (S1, S2)
- **R6.** Each resonator is a feedback comb whose loop is one period of its pitch long, made exact by a fractional delay (a fourth-order Thiran allpass), with a one-pole low-pass in the loop that Color sets; its strongest resonance, measured on white noise through the device with that resonator alone, is within 1 cent of its pitch from C-1 to C5 at 44.1, 48 and 96 kHz. (S1; the design is Beatlayer's own)
- **R7.** Decay, 0.01 to 30 s (1 s by default, logarithmic), sets each loop's feedback so its ring falls 60 dB in Decay, within 10 %, for Resonator I; with Const off, the others keep Resonator I's feedback, so higher resonators ring shorter, as strings do; with Const on, each resonator's feedback is set for its own pitch, so all ring for Decay. A loop's feedback stays below 1 at any setting (it is held at 0.9999 at most), and each comb's input is scaled by the square root of one less the square of its feedback, so a resonator's level on noise doesn't change with Decay. (S1; the law is Beatlayer's own)
- **R8.** Color, −100 to +100 % (0 by default), moves the loops' low-pass, from dark (a one-pole low-pass at a cutoff of 2 times the pitch at −100 %, and at 2 times the pitch divided by the share of −100 % between) to open (no filtering at 0 % and above), and the loop doesn't make up what the low-pass takes, so a darker resonator rings shorter. Above 0 % a first-order high shelf after each resonator's combs, with its corner at 5 times the pitch and outside the loops so it can't push a loop's gain above 1, boosts by up to +6 dB at 10 times the pitch (in proportion to the share of +100 %). (S1; the law is Beatlayer's own)
- **R9.** Mode A runs each resonator as one comb on the filtered input. Mode B runs each as two combs a fifth apart (the second at 1.5 times the pitch, 12 dB lower), which gives a low Resonator I a clearer pitch. (S1; Mode B's design is Beatlayer's own)
- **R10.** The resonators hear the sum of the two channels, halved. Width, 0 to 100 % (100 % by default), places the resonators, in constant power: at 0 % all in the centre; at 100 % I in the centre, II and IV left, III and V right; between, in proportion. Gain, a gain from −∞ to +12 dB (0 dB), sets the resonators' sum (rule G1), and Dry/Wet, 0 to 100 % (100 % by default), balances it against the dry signal. (S1; the placement and ranges are Beatlayer's own)
- **R11.** The comb lines are made with the node, long enough for C-1 at 192 kHz; nothing on the audio thread allocates, locks or blocks. (S3)
- **R12.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S3)
- **R13.** Every parameter is automatable and modulatable (rule G2), Use Current Scale included, and set and read back through the agent API; every gain reaches −∞ dB. (S2, S3)
- **R14.** Resonators declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Filter Frequency, Note, Decay, Color, Width, Gain, Dry/Wet, Mode), Pitches (II to V's Pitch and Gain), Detail (the filter type, Const, the five switches and I's Gain) and Fines (the five Fines), so every automatable parameter is in a bank. (S3; Beatlayer's own)
- **R15.** A set with Resonators saves and reopens with every parameter. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | `crates/devices/tests/resonators.rs`, `the_input_filter_measures_within_half_a_decibel_of_its_design`, `the_description_has_the_switch_in_the_title_bar_and_every_parameter_is_in_a_bank` | unit (the filter's measured response per type) |
| R3, R6 | `a_resonators_peak_is_within_a_cent_of_its_pitch_from_c_minus_1_to_c5`, `fine_and_pitch_set_the_pitches` | unit (each resonator's peak within 1 cent from C-1 to C5 at three rates) |
| R4, R5 | `use_current_scale_counts_degrees_of_a_d_minor_clip`, `a_scale_change_retunes_from_its_frame`, `under_a_19_note_tuning_the_pitches_come_from_the_tuning`; `crates/devices/tests/scales.rs`, `resonators_has_the_switch_in_the_title_bar` | unit (degrees in a minor scale; a scale change on its frame; a 19-note tuning) |
| R7 to R10 | `decay_sets_the_ring_time_within_ten_per_cent`, `const_keeps_the_ring_time_the_same_at_every_pitch`, `color_tilts_the_loops_within_half_a_decibel`, `mode_b_adds_a_fifth_twelve_decibels_down`, `width_places_the_resonators`, `dry_wet_and_gain_set_the_output`, `the_longest_decay_never_grows` | unit |
| R11, R12 | `renders_are_bit_identical_at_every_block_size_and_run_after_run`; `crates/cli/tests/golden.rs`, the `resonators` fixture; `rtsan/tests/devices.rs`, `resonators_passes_under_rtsan` | real-time trap; golden render at four block sizes; RealtimeSanitizer |
| R13 to R15 | `crates/devices/tests/rules.rs` (every parameter automatable and modulatable, every gain to −∞ dB); `crates/devices/tests/banks.rs`; `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_resonators_by_id_and_reopens_it` | rules over every device type; agent API; save and reopen |
