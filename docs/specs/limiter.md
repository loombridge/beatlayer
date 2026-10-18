# The limiter

- **Status:** partial
- **Work packages:** WP-2.18a (part 3; the limiter with a true-peak look-ahead)
- **Parity-matrix row:** Devices: EQ Eight, Compressor, Limiter, Reverb, Delay, Utility
- **Checked against:** Live 12.1's reference manual, section "Limiter"

## The behaviour

Limiter is Beatlayer's limiter: a look-ahead brickwall peak limiter that keeps a signal's output at or below a ceiling, for the Main track or any track. The user sets the ceiling, the input gain, the release time (or lets the device choose it), the look-ahead time, the ceiling mode, how the two channels share their gain reduction, and whether they are left/right or mid/side.

The device delays the signal by the look-ahead time and, over that same window, follows the least gain the signal needs, so the gain has already fallen to a peak's gain by the time the peak comes out. The gain falls at once, so no sample passes the ceiling, and recovers over the Release time. In True Peak mode the device detects peaks at four times the sample rate, so inter-sample peaks are held too. This spec covers the device's behaviour; the device framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md), and delay compensation is WP-2.11.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section "Limiter", and the 12.1 release notes | The Limiter is a mastering peak limiter: a Ceiling sets the highest level it outputs; an Input Gain boosts or attenuates before limiting; Release sets how fast limiting stops after a peak, with an Auto toggle beside it; the Lookahead can be 1.5, 3, or 6 ms and delays the output as long as it looks ahead; the Ceiling Mode is Standard, Soft Clip, or True Peak (Standard is the default); a Link shares the gain reduction across two channels, 0 to 100 %; and the Routing Mode is L/R or M/S, M/S encoding to mid/side before the limiting stage |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter is automatable |
| S3 | Beatlayer | The [device framework spec](device-framework.md), R8 | An oversampler runs a stage at 2, 4, or 8 times the rate and reports its latency |

## Rules

- **R1.** Beatlayer ships Limiter, an audio-effect device type of its own layout, described like any native device: every parameter has a descriptor and a place in the layout. (S2, device-framework)
- **R2.** Ceiling, in dB, from −18 to 0 dB (default −0.3 dB), is the highest level the output reaches in Standard mode: the sample peak of the output never passes it, while a signal whose peaks are all below it is left alone. (S1)
- **R3.** Release, in milliseconds, from 1 to 1000 ms (default 100 ms, logarithmic), is the time constant of the gain's recovery after a peak; a short release is louder and a long one smoother. Auto Release picks a fast time just after a peak and a slow one once a reduction has been held; with Auto on the Release control is ignored. (S1)
- **R4.** Ceiling Mode offers Standard, Soft Clip, and True Peak, Standard by default. True Peak detects peaks at four times the sample rate and holds the inter-sample peak at the ceiling, leaving a little headroom for the reconstruction filters. Soft Clip rounds peaks off near the ceiling, so the output stays below it. (S1, S3)
- **R5.** Lookahead offers 1.5, 3, or 6 ms, 3 ms by default. The output is delayed by the look-ahead time, and the device reports that delay as its latency for delay compensation (WP-2.11). (S1)
- **R6.** Link, 0 to 100 % (default 100 %), shares the gain reduction across the two channels: at 100 % both take the larger reduction, at 0 % each is limited on its own. (S1)
- **R7.** Routing Mode is L/R or M/S: in M/S the signal is encoded to mid/side, the ceiling applies in that domain, and the output is decoded back to left/right. The mid/side matrix adds no latency. (S1)
- **R8.** Soft Clip never lets the output pass the ceiling: it is the identity below half the ceiling and curves off above it. (S1)
- **R9.** Gain is the input gain, a gain parameter that boosts to a top of +18 dB (default 0 dB) and reaches −∞ dB at the bottom (rule G1), and is automatable, like every parameter (rule G2). (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/limiter.rs`, `the_limiter_is_described_and_laid_out`; `crates/devices/tests/rules.rs` over the shipped types | unit, real-time trap |
| R2 | `crates/devices/tests/limiter.rs`, `the_sample_peak_never_passes_the_ceiling`, `standard_mode_leaves_the_inter_sample_peak_alone` | unit, real-time trap |
| R3 | `crates/devices/tests/limiter.rs`, `the_release_recovers_with_its_time_constant`, `auto_release_recovers_more_quickly_from_a_peak` | unit, real-time trap |
| R4 | `crates/devices/tests/limiter.rs`, `true_peak_mode_holds_the_inter_sample_peaks_at_the_ceiling` | unit, real-time trap |
| R5 | `crates/devices/tests/limiter.rs`, `the_output_is_delayed_by_the_look_ahead_and_reported_as_latency` | unit, real-time trap |
| R6 | `crates/devices/tests/limiter.rs`, `the_link_shares_the_gain_reduction_across_the_channels` | unit, real-time trap |
| R7 | `crates/devices/tests/limiter.rs`, `mid_side_routing_limits_the_mid_and_the_side` | unit, real-time trap |
| R8 | `crates/devices/tests/limiter.rs`, `soft_clip_keeps_the_output_below_the_ceiling` | unit, real-time trap |
| R9 | `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity`, `every_parameter_is_automatable_to_the_frame` | real-time trap |

## Not built yet

- Live 12.1's Maximize toggle, which turns Ceiling into Threshold and Gain into Output for one-control loudness.
- What the display cell named Gain Reduction draws (the Gain Reduction meter, and the Soft Clip LED the manual mentions): the layout leaves the cell, and this spec doesn't cover its drawing.
