# The limiter

- **Status:** partial (R1 to R9 each have a passing test; R10 to R13 are WP-8.33's)
- **Work packages:** WP-2.18a (part 3; the limiter with a true-peak look-ahead: R1 to R9); WP-8.33 (Maximize, from Live 12.1, and the device's banks: R10 to R13)
- **Parity-matrix row:** Devices: EQ Eight, Compressor, Limiter, Reverb, Delay, Utility
- **Checked against:** Live 12.1's reference manual, section "Limiter"; the Live 12 reference manual (online edition), section 29.24 "Limiter", and the Live 12.1 release notes, read 30 October 2026

## The behaviour

Limiter is Beatlayer's limiter: a look-ahead brickwall peak limiter that keeps a signal's output at or below a ceiling, for the Main track or any track. The user sets the ceiling, the input gain, the release time (or lets the device choose it), the look-ahead time, the ceiling mode, how the two channels share their gain reduction, and whether they are left/right or mid/side.

The device delays the signal by the look-ahead time and, over that same window, follows the least gain the signal needs, so the gain has already fallen to a peak's gain by the time the peak comes out. The gain falls at once, so no sample passes the ceiling, and recovers over the Release time. In True Peak mode the device detects peaks at four times the sample rate, so inter-sample peaks are held too. This spec covers the device's behaviour; the device framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md), and delay compensation is WP-2.11.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section "Limiter", and the 12.1 release notes | The Limiter is a mastering peak limiter: a Ceiling sets the highest level it outputs; an Input Gain boosts or attenuates before limiting; Release sets how fast limiting stops after a peak, with an Auto toggle beside it; the Lookahead can be 1.5, 3, or 6 ms and delays the output as long as it looks ahead; the Ceiling Mode is Standard, Soft Clip, or True Peak (Standard is the default); a Link shares the gain reduction across two channels, 0 to 100 %; and the Routing Mode is L/R or M/S, M/S encoding to mid/side before the limiting stage |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter is automatable |
| S3 | Beatlayer | The [device framework spec](device-framework.md), R8 | An oversampler runs a stage at 2, 4, or 8 times the rate and reports its latency |
| S4 | Manual | Live 12 reference manual (online edition, read 30 October 2026), section 29.24 "Limiter"; Live 12.1 release notes | A Maximize toggle; with it on, Ceiling becomes Threshold and Input Gain becomes Output, for loudness from one control |
| S5 | Beatlayer | [Control surfaces](control-surfaces.md) R19 | Each native device type declares named banks of eight for instant mappings |

## Rules

- **R1.** Beatlayer ships Limiter, an audio-effect device type of its own layout, described like any native device: every parameter has a descriptor and a place in the layout. (S2, device-framework)
- **R2.** Ceiling, in dB, from −18 to 0 dB (default −0.3 dB), is the highest level the output reaches in Standard mode: the sample peak of the output never passes it, while a signal whose peaks are all below it is left alone. (S1)
- **R3.** Release, in milliseconds, from 1 to 1000 ms (default 100 ms, logarithmic), is the time constant of the gain's recovery after a peak; a short release is louder and a long one smoother. Auto Release picks a fast time just after a peak and a slow one once a reduction has been held; with Auto on the Release control is ignored. (S1)
- **R4.** Ceiling Mode offers Standard, Soft Clip, and True Peak, Standard by default. True Peak detects peaks at four times the sample rate and holds the inter-sample peak at the ceiling, leaving a little headroom for the reconstruction filters. Soft Clip rounds peaks off near the ceiling, so the output stays below it. (S1, S3)
- **R5.** Lookahead offers 1.5, 3, or 6 ms, 3 ms by default. The output is delayed by the look-ahead time, and the device reports that delay as its latency for delay compensation (WP-2.11). (S1)
- **R6.** Link, 0 to 100 % (default 100 %), shares the gain reduction across the two channels: at 100 % both take the larger reduction, at 0 % each is limited on its own. (S1)
- **R7.** Routing Mode is L/R or M/S: in M/S the signal is encoded to mid/side, the output is decoded back to left/right, and the ceiling bounds the left and right peaks: mid and side take the same gain reduction, set by the sum of the mid peak and the side peak (measured in Live 12.4, 25 October 2026). The mid/side matrix adds no latency. (S1)
- **R8.** Soft Clip never lets the output pass the ceiling: it is the identity below half the ceiling and curves off above it. (S1)
- **R9.** Gain is the input gain, a gain parameter that boosts to a top of +18 dB (default 0 dB) and reaches −∞ dB at the bottom (rule G1), and is automatable, like every parameter (rule G2). (S2)

### Maximize (WP-8.33)

- **R10.** Maximize is a switch, off in a new device, automatable like every parameter. With it on, the device shows Threshold in Ceiling's place and Output in Gain's: two parameters of their own (because a descriptor's range and unit are fixed), Threshold from −24 to 0 dB (default 0 dB) and Output a gain from −∞ to 0 dB (default −0.3 dB, rule G1). Ceiling and Gain keep their values while Maximize is on, and Threshold and Output keep theirs while it is off. (S4; the separate parameters, ranges and defaults are Beatlayer's own)
- **R11.** With Maximize on, the input is raised by Output minus Threshold, in dB, and limited at Output in the chosen Ceiling Mode, with Release, Auto, Lookahead, Link and Routing Mode as R3 to R7 say: lowering Threshold by 6 dB raises everything below the limiting by 6 dB, and the output's sample peak (or true peak, in True Peak) never passes Output. At Threshold 0 dB and Output −0.3 dB a signal peaking below −0.3 dB passes unchanged. (S4; the law is Beatlayer's own)
- **R12.** A set or preset saved before WP-8.33 opens with Maximize off and renders sample-identically. Switching Maximize is one undo step named Change "Maximize", and the controls' labels follow it in the device's body, in `device.parameters` (each parameter with whether it is shown) and in the accessibility tree. (S4)
- **R13.** Limiter declares its banks of eight (S5): Main (Ceiling, Gain, Release, Auto, Lookahead, Link, Ceiling Mode, Routing Mode) and Maximize (Maximize, Threshold, Output), so every automatable parameter is in a bank. (S5; the banks are Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/limiter.rs`, `the_limiter_is_described_and_laid_out`; `crates/devices/tests/rules.rs` over the shipped types | unit, real-time trap |
| R2 | `crates/devices/tests/limiter.rs`, `the_sample_peak_never_passes_the_ceiling`, `standard_mode_leaves_the_inter_sample_peak_alone` | unit, real-time trap |
| R3 | `crates/devices/tests/limiter.rs`, `the_release_recovers_with_its_time_constant`, `auto_release_recovers_more_quickly_from_a_peak` | unit, real-time trap |
| R4 | `crates/devices/tests/limiter.rs`, `true_peak_mode_holds_the_inter_sample_peaks_at_the_ceiling` | unit, real-time trap |
| R5 | `crates/devices/tests/limiter.rs`, `the_output_is_delayed_by_the_look_ahead_and_reported_as_latency` | unit, real-time trap |
| R6 | `crates/devices/tests/limiter.rs`, `the_link_shares_the_gain_reduction_across_the_channels` | unit, real-time trap |
| R7 | `crates/devices/tests/limiter.rs`, `mid_side_routing_keeps_the_left_and_right_under_the_ceiling` | unit, real-time trap |
| R8 | `crates/devices/tests/limiter.rs`, `soft_clip_keeps_the_output_below_the_ceiling` | unit, real-time trap |
| R9 | `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity`, `every_parameter_is_automatable_to_the_frame` | real-time trap |
| R10, R11 | to be written in WP-8.33 | unit, real-time trap (the output peak never above Output; the gain below the limiting; golden renders at block sizes 1, 64 and 4,096) |
| R12 | to be written in WP-8.33 | unit (an old set renders identically); shell test (undo label, shown labels); UI script |
| R13 | to be written in WP-8.33 | `crates/devices/tests/banks.rs` |

## Not built yet

- What the display cell named Gain Reduction draws (the Gain Reduction meter, and the Soft Clip LED the manual mentions): the layout leaves the cell, and this spec doesn't cover its drawing.
