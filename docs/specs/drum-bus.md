# The drum bus

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-5.22b
- **Parity-matrix row:** Audio effects: Drum Buss (the second M5 row)
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.12 "Drum Buss" and its three subsections (29.12.0.1 to 29.12.0.3), read 19 October 2026; the ranges, defaults, crossovers and curves the manual doesn't give are Beatlayer's own and are listed below

## The behaviour

Drum Buss is Beatlayer's drum processor: it trims the input, optionally compresses it, drives it through one of three distortions, shapes its mid-highs with Crunch, Damp and Transients, and fills out its low end with a tuned resonator (Boom, Freq, Decay, Force To Note, Boom Audition), before Dry/Wet and Output.

The signal runs through, in order: Trim; a fixed compressor while Comp is on; a Drive gain into the shape its Type chooses (Soft, Medium or Hard); Crunch on the mid-high band; Damp's low-pass; Transients on the band above 100 Hz; a resonator at Freq, added in proportion to Boom; then the Dry/Wet balance and Output. The resonator's ring-down time is Decay, and the average of its output is the Bass Meter's reading. The framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.12 "Drum Buss" and its subsections 29.12.0.1 "Mid-High Frequency Shaping", 29.12.0.2 "Low-End Enhancement" and 29.12.0.3 "Output" | Trim reduces the input before any processing; Comp applies a fixed compressor before the distortion, with fast attack, medium release, a moderate ratio and ample makeup; three distortion types (Soft, a waveshaping; Medium, a limiting; Hard, a clipping with bass boost), each adding an increasing degree of distortion, with Drive determining how much the input is driven before distorting; Crunch adds a sine-shaped distortion to mid-high frequencies; Damp is a low-pass removing highs the distortion added; Transients emphasizes or de-emphasizes the transients of frequencies above 100 Hz, positive adding attack and sustain and negative adding attack with less sustain; Boom is the amount a resonant filter's low-end enhancement adds, with the Bass Meter showing its effect; Freq is the enhancer's frequency and Force To Note tunes it to the nearest MIDI note; Decay adjusts the low frequencies' decay, acting on the incoming (post-drive and distortion) signal alone at Boom 0 % and on the incoming and processed signals above it; Boom Audition solos the low-frequency enhancer; Dry/Wet balances the processed and dry signals; Output Gain sets the processed signal's level |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter can be automated and modulated |
| S3 | Beatlayer | The [device framework spec](device-framework.md) | A device type's parameters and layout; a render repeats at any block size; the display's data is published after each block and readable headlessly |

## Rules

- **R1.** Beatlayer ships Drum Buss, an audio-effect device type, of its own layout: every parameter has a descriptor and a place in the layout, and it reports no latency. (S1, S3)
- **R2.** Trim is a gain that sets the input level before any other processing, from silence to unity (0 dB): at its bottom the device is silent. Output is the output gain after Dry/Wet, also reaching −∞. (S1, S2)
- **R3.** Comp is a switch. On, a fixed compressor runs before the distortion: peak detection shared across the channels, a threshold of −12 dB, a ratio of 4 : 1, an attack of 1 ms and a release of 100 ms, both as time constants, and a makeup gain of 9 dB (the reduction a 0 dB signal takes). These settings are Beatlayer's own, within the manual's "fast attack, medium release, a moderate ratio, ample makeup". (S1)
- **R4.** Type is Soft, Medium or Hard, and Drive is the gain into the shape, from 0 dB at 0 % to +24 dB at 100 %. Soft waveshapes the driven signal (`tanh`); Medium limits it (`x / (1 + |x|)`, saturating at ±1); Hard clips it at ±1 through a +6 dB low shelf at 100 Hz as its bass boost. The three curves are Beatlayer's own; the manual names their characters only. (S1)
- **R5.** At one Drive, Hard's harmonic distortion of a sine exceeds Medium's, and Medium's exceeds Soft's. (S1, S2)
- **R6.** Crunch adds a sine-shaped distortion (`sin(g·x)`, `g` the driven gain) to the band above 200 Hz, in proportion to its setting; the band below 200 Hz passes unchanged. The crossover and the curve are Beatlayer's own. (S1)
- **R7.** Damp is a low-pass after the distortion whose corner falls from 20 kHz at 0 % to 200 Hz at 100 %, evenly in octaves. The range is Beatlayer's own. (S1)
- **R8.** Transients shapes the band above 100 Hz with a faster and a slower level follower: its attack, in both directions of the control, is the faster follower's excess over the slower one; positive settings raise the slower follower's share (more sustain) and negative settings lower it (less sustain). The band below 100 Hz passes within 0.5 dB. The crossover, the times and the shaper are Beatlayer's own. (S1)
- **R9.** Boom is the amount of a resonant band-pass at Freq added to the signal, from none at 0 %; its output is the low-end enhancer. (S1)
- **R10.** Freq is the resonator's frequency, from 20 Hz to 200 Hz. Force To Note, on, snaps it to the nearest note of equal temperament at A = 440 Hz; off, Freq is the resonator's frequency as it is. The range and the tuning are Beatlayer's own, until tuning systems arrive in Phase 6. (S1)
- **R11.** Decay is the resonator's ring-down time constant, from 10 ms to 2 s: the resonator rings at Freq within 1 % and its ring-down time is within 10 % of Decay. The resonator hears the signal after the drive and distortion, and Boom mixes its output in, so at Boom 0 % it adds nothing and Decay is heard only through the resonator's silence. The manual's rule that at Boom 0 % Decay acts on the incoming (post-drive and distortion) signal and above 0 % on the incoming and the processed signals is Beatlayer's, read as the resonator's ring-down; how the incoming band decays at Boom 0 % exactly is to observe. (S1)
- **R12.** Boom Audition is a switch; on, the device outputs only the low-end enhancer, whatever Dry/Wet says. (S1)
- **R13.** The Bass Meter reads the low-end enhancer's loudest sample since the last block, in dB. The device publishes it after each block through the framework's display data (R35), so it is readable headlessly. (S1, S3)
- **R14.** Dry/Wet is the balance between the processed and the dry (untrimmed) signals; at 0 % the output is the input. Output scales the result. (S1)
- **R15.** A render is bit-identical at any block size. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/drum_bus.rs`, `the_drum_bus_is_described_and_laid_out` | unit, real-time trap |
| R2 | `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity`; `crates/devices/tests/drum_bus.rs`, `trim_and_output_reach_silence` | real-time trap |
| R3 | `crates/devices/tests/drum_bus.rs`, `comp_reduces_the_levels_of_a_loud_hit` | real-time trap |
| R4, R5 | `crates/devices/tests/drum_bus.rs`, `the_three_distortions_distort_more_from_the_first_to_the_third` | real-time trap |
| R6 | `crates/devices/tests/drum_bus.rs`, `crunch_adds_harmonics_to_the_mid_highs_only` | real-time trap |
| R7 | `crates/devices/tests/drum_bus.rs`, `damps_corner_is_where_the_spec_puts_it` | real-time trap |
| R8 | `crates/devices/tests/drum_bus.rs`, `transients_raises_the_attack_to_sustain_ratio_above_a_hundred_hertz` | real-time trap |
| R9, R10 | `crates/devices/tests/drum_bus.rs`, `booms_resonator_rings_at_freq_and_force_to_note_snaps_it` | real-time trap |
| R11 | `crates/devices/tests/drum_bus.rs`, `decay_sets_the_resonators_ring_down_time` | real-time trap |
| R12 | `crates/devices/tests/drum_bus.rs`, `boom_audition_outputs_only_the_enhancer` | real-time trap |
| R13 | `crates/devices/tests/drum_bus.rs`, `the_bass_meter_is_readable_headlessly` | real-time trap |
| R14 | `crates/devices/tests/drum_bus.rs`, `dry_wet_at_zero_is_the_input` | real-time trap |
| R15 | `crates/devices/tests/drum_bus.rs`, `renders_are_bit_identical_at_any_block_size` | real-time trap |
| R1, R15 | `crates/devices/tests/drum_bus.rs`, `a_set_with_drum_buss_saves_and_reopens` | unit |
| R1 | `crates/cli/tests/agent.rs`, `an_agent_adds_drum_buss_sets_it_by_id_and_reads_it_back` | MCP script |
| R1, R15 | `rtsan/tests/devices.rs`, `drum_buss_passes_under_rtsan` | RTSan |

## Details

- The device type's ID is `beatlayer.buss`; its name is Drum Buss, as Live's.
- The fixed compressor's reduction is shared across the channels (the frame takes the loudest channel's level), like Beatlayer's Compressor.
- Crunch's crossover is 200 Hz and Transients' is 100 Hz; both are Linkwitz-Riley fourth-order splits (two Butterworth sections), so a band split and re-summed at unity is flat in magnitude.
- Transients always splits the band above 100 Hz with a Linkwitz-Riley fourth-order crossover and re-sums it, shaping the high band; at 0 the shaping gain is unity, so the split and re-sum is flat in magnitude (an all-pass at the crossover). Its two followers: the faster one has a 1 ms attack and a 20 ms release; the slower one a 30 ms attack and a 150 ms release.
