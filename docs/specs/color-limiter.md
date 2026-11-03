# Color Limiter: a limiter with character

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-8.50, with [Re-Enveloper](re-enveloper.md) (all of it: R1 to R11). Its look-ahead limiting is [the limiter](limiter.md)'s, the same code; the framework is WP-2.5's ([the native device framework](device-framework.md))
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Creative Extensions pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Color Limiter is a limiter with the grit of hardware: Loudness drives the signal into it, Ceiling sets the level it never passes, Lookahead and Release set how it catches peaks and lets go, and Saturation and Color add harmonics and tilt the tone.

Live's Color Limiter comes in its Creative Extensions pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.color_limiter`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Color Limiter, a limiter inspired by hardware, with Loudness (the input level), Ceiling (the threshold), Lookahead, Release, Saturation and Color |
| S2 | Beatlayer | [The limiter](limiter.md) R2, R3 and R5; [the saturator](saturator.md); [the device framework](device-framework.md) R4, R8; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | Look-ahead limiting under a ceiling with its latency reported; shapers; smoothing, the oversampler; banks; gains and automation |

## Rules

- **R1.** Color Limiter (`beatlayer.color_limiter`) is an audio effect with Live's name, Beatlayer's own layout and a gain-reduction meter in its one display cell, whose reading `device.display` reports. (S1; the layout is Beatlayer's own)
- **R2.** Loudness, a gain from −∞ to +24 dB (0 dB by default, rule G1), drives the input into the device. (S1; the range is Beatlayer's own)
- **R3.** Saturation, 0 to 100 % (0 by default), passes the signal after Loudness through a soft clipper, y = tanh(k x) / k with k from 1 to 4, run at twice the sample rate (device framework R8), before the limiter. Color, −100 to +100 % (0), tilts the saturated signal about 1 kHz by up to ±6 dB at the ends of the band, one first-order section whose pole and zero sit an equal factor either side of 1 kHz (a low and a high shelf in one), before the limiter, so a brighter Color is limited harder in the highs. (S1; the laws are Beatlayer's own)
- **R4.** The limiter is [the limiter](limiter.md)'s look-ahead stage, the same code: Ceiling (−24 to 0 dB, −0.3 dB) is the highest sample peak the output reaches; Lookahead is 1, 3 or 6 ms (3 ms) and is reported as latency, with the oversampler's; Release (1 to 1,000 ms, 100 ms, logarithmic) is the gain's recovery time constant. (S1, S2; ranges and defaults are Beatlayer's own)
- **R5.** The output's sample peak never passes Ceiling, at any setting of Loudness, Saturation and Color. (S1, S2)
- **R6.** Saturation's and Color's measured static curve and tilt are within 0.01 and 0.5 dB of their laws. (Beatlayer's own)
- **R7.** A render is bit-identical at block sizes 1, 64, 512 and 4,096, and nothing on the audio thread allocates, locks or blocks. (S2)
- **R8.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R9.** Color Limiter declares one bank of eight ([control surfaces](control-surfaces.md) R19): Loudness, Ceiling, Lookahead, Release, Saturation, Color. (S2; Beatlayer's own)
- **R10.** A set with the device saves and reopens with every parameter. (S2)
- **R11.** The device reuses the limiter's look-ahead and the saturator's oversampling rather than copies of them. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/color_limiter.rs`, `color_limiter_is_described_with_one_bank_of_six`, `the_display_reads_the_gain_reduction` | unit |
| R2 | `crates/devices/tests/color_limiter.rs`, `color_limiter_is_described_with_one_bank_of_six`, `loudness_reaches_minus_infinity` | unit, real-time trap |
| R3, R6 | `crates/devices/tests/color_limiter.rs`, `saturations_measured_curve_is_within_a_hundredth_of_a_decibel_of_its_law`, `colors_measured_tilt_is_within_half_a_decibel_of_its_law` | unit, real-time trap |
| R4 | `crates/devices/tests/color_limiter.rs`, `the_lookahead_and_the_oversampler_are_reported_as_latency`, `release_sets_how_quickly_the_gain_recovers` | unit, real-time trap |
| R5 | `crates/devices/tests/color_limiter.rs`, `the_output_never_passes_ceiling` | unit, real-time trap |
| R7 | `crates/devices/tests/color_limiter.rs`, `renders_are_the_same_at_any_block_size`; `rtsan/tests/devices.rs`, `color_limiter_passes_under_rtsan` | real-time trap, RTSan |
| R8 | `crates/cli/tests/agent.rs`, `an_agent_inserts_color_limiter_and_re_enveloper_and_sets_and_reads_them`; `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_is_modulatable_to_the_frame` | MCP script, real-time trap |
| R9 | `crates/devices/tests/banks.rs`, `every_native_device_types_banks_hold_every_parameter_and_none_more_than_eight` | unit |
| R10 | `crates/shell/tests/creative_extensions.rs`, `a_set_with_color_limiter_reopens_with_every_parameter` | unit |
| R11 | `crates/devices/src/color_limiter.rs` calls `Brickwall::limit` (the limiter's look-ahead stage, unchanged for the limiter itself: `crates/devices/tests/limiter.rs`) and `bl_dsp::Oversampler` | review |
