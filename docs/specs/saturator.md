# The saturator

- **Status:** partial
- **Work packages:** WP-5.23 (with the [overdrive](overdrive.md) and the [tube distortion](tube-distortion.md))
- **Parity-matrix row:** Audio effects: Saturator, to Live's current version (Bass Shaper curve of 12.1)
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", sections 29.34, 29.34.0.1 and 29.34.0.2, read on 18 October 2026; the 12.1 release notes

## The behaviour

Saturator is Beatlayer's saturator: a waveshaper that maps each sample of the signal to a new value through a curve the user chooses, from soft saturation to a wavefolder. The user sets the curve, the Drive gain before it, the Output level after it, a post-clip stage that holds the result under Output, and the Dry/Wet balance. Bass Shaper has a threshold, below which it leaves the signal alone. The Waveshaper curve has six controls of its own in the expanded view. The Color section shapes what the curve hears: two filters before it and their inverse after it, so a band the user boosts saturates more and the signal's level comes back after the curve.

The signal goes through an optional DC filter, the Color filters, the Drive gain, the curve, the Color filters' inverse, the post-clip stage and Output, and is mixed with the dry signal. Hi-Quality runs the curve at 8 times the rate through the framework's oversampler, so its harmonics don't fold back, and the dry signal is delayed as long as the wet one. This spec covers the device; the device framework (parameters, smoothing, layout, the menu items and the expand toggle) is the [device framework spec](device-framework.md) and [device chains](device-chains.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.34 "Saturator" | A waveshaper with eight curves: Analog Clip (a smoother transition around the clipping point), Soft Sine, Bass Shaper (like Analog Clip, with a smoother harmonic spectrum when low frequencies are driven hard), Medium Curve, Hard Curve, Sinoid Fold (a wavefolder: the output folds over itself as the input grows), Digital Clip (immediate hard clipping past the clipping point) and Waveshaper. Every curve but Waveshaper is linear below the clipping point. Drive is the gain before the curve; Output attenuates the final level; Dry/Wet balances the dry and processed signals |
| S2 | Manual | Section 29.34 | Bass Shaper's threshold runs from 0 to −50 dB; the curve is linear below it and clips smoothly above it; low values clip softly and high values hard, and low values suit bass driven hard |
| S3 | Manual | Section 29.34 | Post Clip Mode adds a clipping stage after any curve; with Soft Clip or Hard Clip the output never exceeds the Output level, and with Soft Clip Output also applies the Analog Clip curve |
| S4 | Manual | Section 29.34 | The Color section: a switch for two filters whose curve is applied before the shaper and inverted after it, so bass can be kept out of the shaper; Amt Lo sets the saturation of the low frequencies (negative reduces it, positive increases it); in the expanded view Amt Hi sets the saturation of the second filter's band, Frequency its centre and Width the range of frequencies it affects |
| S5 | Manual | Section 29.34.0.1 "Waveshaper controls" | Six controls act when the Waveshaper curve is chosen: Drive (at 0% the other five have no effect, at 100% they fully shape the curve), Curve (adds mostly third-order harmonics), Depth (the amplitude of a sine added to the curve; at 0% it has no effect), Linear (works with Curve and Depth to change the curve's linear part), Damp (flattens signals near the centre, like an ultra-fast noise gate) and Period (the density of the added sine's ripples, in relation to Depth) |
| S6 | Manual | Section 29.34.0.2 "Context menu" | Hi-Quality reduces aliasing at a slight cost in CPU; Pre-DC Filter adds a DC filter at the input stage to remove offsets |
| S7 | Manual | The Live 12.1 release notes | Saturator was redesigned: a main view with the signal's level over the curve, the new bass curve with its own threshold (the notes call it Bass Enhancer and the manual Bass Shaper), a hard-clipping choice for the output stage, and the EQ curve before the shaper in the expanded view |
| S8 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain reaches −∞ dB; every parameter can be automated and modulated |
| S9 | Beatlayer | The [device framework spec](device-framework.md), R8, and [device chains](device-chains.md), R34 and R35 | An oversampler runs a process at 2, 4 or 8 times the rate and reports its latency; a device lists switches as items of its Show Options menu and shows an expanded part through the expand toggle |

## Rules

- **R1.** Beatlayer ships Saturator, an audio-effect device type of its own layout, described like any native device: every parameter has a descriptor and a place in the layout (the menu items Hi-Quality and Pre-DC Filter in the Show Options menu instead), and the Waveshaper's controls and Color's second filter sit in the expanded part. The display cell is named Shaper Curve. (S7, S8, S9)
- **R2.** Each curve maps its input `x` (the signal times the Drive gain) as follows, and the output matches the function within 0.001. With `k` a knee, **Analog Clip** is `x` up to `|x| = k` with `k = 0.5`, and above it `sign(x) · (k + (1 − k) · tanh((|x| − k) / (1 − k)))`; **Medium Curve** is the same with `k = 0.2` and **Hard Curve** with `k = 0.8`. **Soft Sine** is `x` up to 0.5 and above it `sign(x) · (0.5 + 0.5 · sin(2 · (|x| − 0.5)))`, holding 1 once the sine reaches its top. **Digital Clip** is `x` limited to −1 and 1. (S1; the functions are Beatlayer's own)
- **R3.** Below its clipping point (0.5 for Analog Clip and Soft Sine, 1 for Digital Clip) a curve leaves the signal as it is. Digital Clip is flat past 1, at exactly −1 or 1. (S1)
- **R4.** **Sinoid Fold** is `x` up to 0.5 and above it `sign(x) · (0.5 + 0.5 · sin(2 · (|x| − 0.5)))` without a hold: the output reaches 1 at `|x|` of about 1.29 and then falls back, folding over itself as the input grows. (S1; Beatlayer's own function)
- **R5.** **Bass Shaper** has a Threshold from −50 to 0 dB (default −12 dB). With `t` the threshold as an amplitude factor, it is `x` up to `|x| = t` and above it `sign(x) · (t + (1 − t) · u / (1 + u))` with `u = (|x| − t) / (1 − t)`, which leaves the knee more slowly than a tanh does. At −50 dB it eases away from the line from almost nothing (soft), and at 0 dB it is linear to full scale and clips there (hard). (S1, S2; Beatlayer's own function)
- **R6.** Drive is a value in dB from −36 to 36 dB (default 5 dB), the gain applied before the curve. (S1; the range and default are Beatlayer's own)
- **R7.** The **Waveshaper** curve, with controls from 0 to 100% (Drive, Curve, Depth, Linear, Damp; Period from 1 to 16, default 4), is, for the input `x` and `u = x` limited to −1 and 1: `core = u + c · (0.5u − 0.5u³)`, `ripple = 0.25 · depth · sin(π · period · u)`, `shaped = (1 − linear) · (core + ripple) + linear · u`, `gate = smoothstep(|u| / (0.25 · damp))` (1 when Damp is 0), and the output `(1 − drive) · x + drive · shaped · gate`. At Drive 0% the curve is the straight line, whatever the others are, and at Linear 100% the shaped part is straight; each control changes the curve as the formula says. The defaults are Drive 100%, Curve 50%, Depth 0%, Linear 0%, Damp 0%. (S1, S5; the functions and defaults are Beatlayer's own)
- **R8.** Post Clip is Off, Soft or Hard (default Off), a stage after the Color filters' inverse and before Output: Soft applies the Analog Clip curve of R2 and Hard limits to −1 and 1, so the result, times Output, never passes Output. Output is a gain from −∞ to +6 dB (default 0 dB). Whether Live's Post Clip Mode has an Off choice and its default are not recorded; Off is Beatlayer's own. (S3, S8)
- **R9.** Dry/Wet runs from 0 to 100% (default 100%); at 0% the output is the input exactly (delayed as R11 says), and the Output gain acts on the processed signal only. (S1; where Output acts is Beatlayer's own)
- **R10.** With Color on, the signal goes through a low shelf at 200 Hz of Amt Lo times 12 dB and a peak at Frequency (50 Hz to 15 kHz, default 1 kHz) of Amt Hi times 12 dB and a width of Width octaves (0.1 to 4, default 1), both amounts from −100 to 100% (default 0), before the Drive gain and curve; after the curve it goes through the inverse filters, each of the same design with the opposite gain. With both amounts at 0 the filters do nothing, so the output nulls against Color off below −100 dB; at a low drive the curve is linear and the two sets of filters cancel; at a hard drive the amounts change the curve's output. (S4; the filters' kinds and ranges are Beatlayer's own)
- **R11.** Hi-Quality (default off) runs the curve at 8 times the rate through the framework's oversampler: a 7 kHz sine driven hard at 48 kHz keeps its folded-back harmonics at least 60 dB under the fundamental, and the device reports the oversampler's latency (56 frames) while it is on and none while it is off. The dry signal is delayed by the same frames so the mix stays aligned. (S6, S9)
- **R12.** Pre-DC Filter (default off) is a one-pole high-pass at 5 Hz before everything else; a DC offset at the input falls below −80 dB of itself within a second. (S6; the corner is Beatlayer's own)
- **R13.** The display holds the curve over inputs from −2 to 2 as a series named Shaper Curve, and, once an engine publishes, the input's level now in dB as the reading Input and a marker on the curve at that level times Drive. The cell stays a black frame in the generic UI; `device.display` reads both headlessly. (S7)
- **R14.** Renders are bit-identical at block sizes 1, 64, 512 and 4,096, and nothing on the audio thread allocates, locks or blocks. The Color filters' coefficients are updated every 16 frames counted from the device's first frame, so they don't follow the block size. (S8)
- **R15.** An agent adds Saturator with `device.insert` or `device.add`, sets and reads every parameter by ID with `device.set_param` and `device.parameters` (Hi-Quality and Pre-DC Filter reported as menu options), shows and hides the expanded part with `device.expand`, reads the display with `device.display`, and finds the device and its values when the saved set is opened. (S9)
- **R16.** Output, a gain, reaches −∞ dB and is silent there, and every parameter is automatable (device framework R2 and R3). (S8)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/saturator.rs`, `the_saturator_is_described_and_laid_out` | unit |
| R2 | `crates/devices/tests/saturator.rs`, `each_curves_transfer_matches_the_spec_at_three_drive_settings` | real-time trap |
| R3 | `crates/devices/tests/saturator.rs`, `digital_clip_is_flat_past_its_point_and_analog_clip_is_linear_below_theirs` | real-time trap |
| R4 | `crates/devices/tests/saturator.rs`, `sinoid_fold_folds_back_past_its_point` | real-time trap |
| R5 | `crates/devices/tests/saturator.rs`, `bass_shapers_threshold_moves_it_from_the_soft_shape_to_the_hard_one` | real-time trap |
| R6 | `crates/devices/tests/saturator.rs`, `drive_is_a_gain_in_decibels_ahead_of_the_curve` | real-time trap |
| R7 | `crates/devices/tests/saturator.rs`, `each_waveshaper_control_moves_the_curve_as_the_spec_states` | real-time trap |
| R8 | `crates/devices/tests/saturator.rs`, `post_clip_holds_the_output_at_or_under_output` | real-time trap |
| R9 | `crates/devices/tests/saturator.rs`, `dry_wet_at_zero_is_the_input_and_the_halves_add` | real-time trap |
| R10 | `crates/devices/tests/saturator.rs`, `the_color_filters_cancel_at_zero_and_are_undone_after_the_curve` | real-time trap |
| R11 | `crates/devices/tests/saturator.rs`, `hi_quality_keeps_a_hard_driven_sines_harmonics_from_folding_back`, `hi_quality_delays_the_dry_signal_as_long_as_the_wet_one` | real-time trap |
| R12 | `crates/devices/tests/saturator.rs`, `the_pre_dc_filter_takes_a_dc_offset_down_within_a_second` | real-time trap |
| R13 | `crates/devices/tests/saturator.rs`, `the_display_holds_the_curve_and_the_inputs_level` | unit, real-time trap |
| R14 | `crates/devices/tests/saturator.rs`, `renders_are_bit_identical_at_any_block_size`; `rtsan/tests/devices.rs`, `the_saturation_devices_pass_under_rtsan` | real-time trap, RTSan |
| R15 | `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_the_saturation_devices_by_id_and_reopens_them` | MCP script |
| R16 | `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity`, `every_parameter_is_automatable_to_the_frame` | real-time trap |

## Not built yet

- The cell Shaper Curve draws the curve and a marker through the generic display; Live's main view also draws the signal's level over the curve as it moves, and the expanded view draws the Color filters' curve with draggable handles. Neither is drawn here, and the Color curve is not in `device.display`.
- Everything the manual leaves unrecorded is Beatlayer's own and listed to observe: the curves' functions and knees, the ranges and defaults, whether Post Clip Mode has an Off choice, where Output sits relative to Dry/Wet, the Color filters' kinds and gain range, Hi-Quality's default and oversampling factor, and the Pre-DC Filter's corner.
