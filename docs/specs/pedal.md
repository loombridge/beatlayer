# Pedal: overdrive, distortion and fuzz

- **Status:** partial (R1 to R12 are built and tested; the aliasing level of R9 holds for Overdrive and Distortion at −36 dBFS and below, not at full drive, and Fuzz's stage is closed to a signal that small)
- **Work packages:** WP-8.41 (all of it: R1 to R12). [Amp](amp.md) and [Cabinet](cabinet.md) are their own devices; the oversampler and the framework are WP-2.5's ([the native device framework](device-framework.md) R8)
- **Parity-matrix row:** Audio effects: Amp, Cabinet, Pedal
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.28 "Pedal" with 29.28.1, read 30 October 2026

## The behaviour

Pedal models a guitar effects pedal in three types: Overdrive, warm and smooth; Distortion, tighter and harder; and Fuzz, unstable and gated. Gain drives the signal into the circuit, a three-band EQ after it shapes the tone (Mid with a choice of three centre frequencies), Sub raises the lowest frequencies, Output sets the level and Dry/Wet mixes in the dry signal. A Hi-Quality option in the title bar's context menu reduces aliasing further.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.pedal`. Its three circuits are Beatlayer's own designs, written below, named after no manufacturer's product.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.28 "Pedal" | Types Overdrive, Distortion and Fuzz; Gain; Output; the EQ after the circuit: Bass, a peak at 100 Hz; Mid, a peak whose frequency a three-position switch sets to 500 Hz, 1 kHz or 2 kHz; Treble, a shelf at 3.3 kHz; Sub, a low shelf below 250 Hz; Dry/Wet; Hi-Quality in the title bar's context menu, for less aliasing |
| S2 | Manual | Section 29.28.1 "Pedal Tips" | Pedal before Amp and Cabinet; Sub to restore low end |
| S3 | Beatlayer | [The device framework](device-framework.md) R4, R8; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2; [overdrive](overdrive.md), [saturator](saturator.md) | Smoothing, the oversampler, banks of eight, gains to −∞ dB, automatable parameters; Beatlayer's shapers already built |

## Rules

- **R1.** Pedal (`beatlayer.pedal`) is an audio effect with Live's name, Beatlayer's own layout and no display cell: the type switch (Overdrive, Distortion, Fuzz; Overdrive in a new device), Gain, Output, Bass, Mid with its frequency switch, Treble, Sub and Dry/Wet. (S1; the layout is Beatlayer's own)
- **R2.** Gain, 0 to 100 % (50 % by default), sets the circuit's drive g = 10^(Gain × 48 / 20) (0 to +48 dB). The circuit runs at four times the sample rate through the oversampler (device framework R8), or eight times with Hi-Quality on, a switch saved with the device. (S1; the law and factors are Beatlayer's own)
- **R3.** Overdrive: a 720 Hz first-order high-pass before the clipper (so the lows stay clean), a soft clipper that keeps part of the clean signal, y = g (u + tanh(g u)) / (1 + g), whose slope at zero is g, and a 6 kHz low-pass after. (S1; Beatlayer's own)
- **R4.** Distortion: a 100 Hz high-pass, a hard clipper of two diodes y = sign(v)·(1 − e^(−|v|)) with v = g u, and a 5 kHz second-order low-pass after. (S1; Beatlayer's own)
- **R5.** Fuzz: a biased transistor stage y = tanh(g (u + β)) − tanh(g β), β = 0.3, whose bias β falls by up to 0.25 while the stage's envelope (a 2 ms attack, 80 ms release follower of its input) is high, so loud notes sputter and decaying ones gate; then a second stage y = tanh(3y) and a 4 kHz low-pass. The bias's motion depends only on the signal, so a render repeats run after run. (S1; Beatlayer's own)
- **R6.** Each type's static curve, measured with a slow sine sweep with the envelope held, lies within 0.01 of its formula, and its harmonic balance on a 220 Hz sine at −12 dBFS with Gain 50 % (the second against the third harmonic) is within 1 dB of its design's figure. DC after the circuit is below −80 dBFS (a 5 Hz high-pass follows it). (Beatlayer's own)
- **R7.** The EQ, after the circuit: Bass, −15 to +15 dB, a peak at 100 Hz with Q 0.7; Mid, −15 to +15 dB, a peak with Q 1 at 500 Hz, 1 kHz or 2 kHz as its switch says (1 kHz by default); Treble, −15 to +15 dB, a shelf at 3.3 kHz; Sub, 0 to +15 dB, a low shelf below 250 Hz. Each band's measured response at small signal is within 0.5 dB of its design at every Mid position. (S1; gains and Qs are Beatlayer's own)
- **R8.** Output is a gain from −∞ to +12 dB (0 dB by default, rule G1); Dry/Wet, 0 to 100 % (100 % by default), balances the dry signal, delayed by the latency, against the pedal's. The device reports the oversampler's latency for the chosen quality. (S1, S3)
- **R9.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run; aliasing from a 5 kHz sine at −36 dBFS at Gain 100 % stays 60 dB below the fundamental in the band below 20 kHz, and 75 dB with Hi-Quality (at full drive the clipped edges' harmonics past half the oversampled rate fold back and no filter removes them, so louder sines alias more: about −30 dB at −6 dBFS); nothing on the audio thread allocates, locks or blocks. (S1, S3; the levels are Beatlayer's own)
- **R10.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; changing type crossfades over 20 ms. (S3)
- **R11.** Pedal declares one bank of eight ([control surfaces](control-surfaces.md) R19): Type, Gain, Bass, Mid, Treble, Sub, Output, Dry/Wet, with Mid Frequency in a second, and Hi-Quality, a menu option, in the Options bank the framework adds last, so every automatable parameter is in a bank. (S3; Beatlayer's own)
- **R12.** A set with Pedal saves and reopens with every parameter and Hi-Quality. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | `crates/devices/tests/pedal.rs`, `the_pedal_is_described_and_laid_out`, `gain_drives_the_circuit_by_zero_to_forty_eight_decibels`, `the_latency_is_the_oversamplers_for_the_quality_and_delays_the_dry_signal` | unit, real-time trap |
| R3 to R6 | `crates/devices/tests/pedal.rs`, `each_types_curve_lies_within_a_hundredth_of_its_formula`, `each_types_harmonics_on_a_220_hz_sine_are_those_of_its_design_within_a_decibel`, `fuzz_sputters_on_a_decaying_tone_and_repeats_run_after_run`, `each_type_leaves_no_dc_above_minus_eighty_dbfs` | unit, real-time trap |
| R7, R8 | `crates/devices/tests/pedal.rs`, `each_eq_band_is_within_half_a_decibel_of_its_design_at_each_mid_position`, `sub_raises_the_lows_with_a_shelf_below_250_hz`, `output_sets_the_wet_level_down_to_silence_and_dry_wet_balances_it` | unit, real-time trap |
| R9 | `crates/devices/tests/pedal.rs`, `a_5_khz_sine_at_gain_100_aliases_less_than_60_db_and_with_hi_quality_75_db`, `renders_are_bit_identical_at_four_block_sizes_and_run_after_run`; `rtsan/tests/devices.rs`, `pedal_passes_under_rtsan` | real-time trap, RTSan, golden renders |
| R10 | `crates/devices/tests/pedal.rs`, `a_change_of_type_crossfades_over_twenty_milliseconds`; `crates/devices/tests/rules.rs` over the shipped types; `crates/cli/tests/agent.rs`, `an_agent_inserts_pedal_and_sets_and_reads_back_its_parameters` | unit, real-time trap, agent API |
| R11 | `crates/devices/tests/banks.rs` over the shipped types | unit |
| R12 | `crates/shell/tests/pedal.rs`, `a_set_with_pedal_reopens_with_every_parameter` | unit |
