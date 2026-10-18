# The tube distortion

- **Status:** partial
- **Work packages:** WP-5.23 (with the [saturator](saturator.md) and the [overdrive](overdrive.md))
- **Parity-matrix row:** Audio effects: Dynamic Tube
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.13, read on 18 October 2026

## The behaviour

Valve is Beatlayer's tube distortion. The user chooses one of three tube models, A, B or C, sets how much signal reaches the tube (Drive), how far the signal sits into the tube's curve (Bias), and where the distortion falls in the spectrum (Tone), and can let the input's level move the Bias (Envelope, with Attack and Release). Model A is clean until the signal reaches a threshold and then adds bright harmonics; C always distorts; B is between. Output sets the level after the tube. Hi-Quality, an item of the Show Options menu, runs the tube at a higher rate so its harmonics don't alias.

The signal goes through Tone's shelf, the Drive gain, the tube, Tone's inverse shelf and Output. The tube works the curve at an operating point set by the model, the Bias and the envelope. This spec covers the device; the device framework is the [device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.13 "Dynamic Tube" | Three tube models: A produces little or no distortion at a low Bias but adds bright harmonics once the input passes a threshold, C distorts all the time, and B lies between them. Tone sets where the distortion falls, toward the high registers or through the mids and lows. Drive sets how much signal reaches the tube, and more is dirtier. Bias sets the tube's intensity and pushes the signal into the nonlinear region; at very high values the signal breaks up. Envelope sets how far an envelope follower on the input signal's level moves the Bias: positive values make loud sounds dirtier, negative values expand the signal by lowering distortion on loud material. Attack and Release set how fast the envelope follows level changes and do nothing at Envelope 0. Output cuts or boosts the final level. Hi-Quality, in the title bar's context menu, reduces aliasing at a slight cost in CPU |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain reaches −∞ dB; every parameter can be automated and modulated |
| S3 | Beatlayer | The [device framework spec](device-framework.md), R8, R28, and [device chains](device-chains.md), R35 | An oversampler runs a process at 2, 4 or 8 times the rate and reports its latency; the envelope follower's attack and release are time constants; a device lists switches as items of its Show Options menu |

## Rules

- **R1.** Beatlayer ships Valve, an audio-effect device type of its own name and layout, described like any native device: every parameter has a descriptor and a place in the layout (Hi-Quality in the Show Options menu instead). It has no latency while Hi-Quality is off. The display cell is named Tube Curve. (S1, S2, S3)
- **R2.** The tube is a curve `g` that is linear up to a knee `k` and above it eases to 1: `g(v) = v` for `|v| ≤ k`, and `sign(v) · (k + (1 − k) · tanh((|v| − k) / (1 − k)))` above. Its output for the signal `x` (after Tone's shelf and the Drive gain) is `g(x + o) − g(o)`, where `o` is the operating point: the model's offset plus the Bias as a fraction (0 to 1), plus the envelope (R5), limited to between 0 and 1 before the model's offset is added. **Tube A** has `k = 0.5` and offset 0: at Bias 0 and Drive 0 dB it is the identity while the signal stays under 0.5, so a sine under that has harmonic distortion below −80 dB, and above it the distortion rises (a sine of 0.9 is above −40 dB); a higher Bias brings the threshold down. (S1; the knee and offset are Beatlayer's own)
- **R3.** **Tube C** has `k = 0` (a plain `tanh`) and offset 0.15, so it distorts at every level: a sine of 0.01 to 0.4 has harmonic distortion above −80 dB. (S1; Beatlayer's own)
- **R4.** **Tube B** has `k = 0.2` and offset 0.05; at the same Drive and level its distortion lies between A's and C's. (S1; Beatlayer's own)
- **R5.** Envelope, from −100 to 100% (default 0%), moves the Bias with the input's level: the operating point's fraction is `(Bias + Envelope · level) / 100`, with `level` the larger of the two channels' envelope followers on the input before Tone and Drive, in amplitude. Positive values make loud sounds dirtier, negative ones cleaner; at 0% the envelope does nothing, whatever Attack and Release are. (S1)
- **R6.** Attack, from 0.1 ms to 1 s (default 10 ms), and Release, from 1 ms to 5 s (default 100 ms), both logarithmic, are the envelope follower's time constants, the time the level takes to cover 63% of a step up or down, within 5%. (S1, S3)
- **R7.** Tone, from 0 to 100% (default 50%), is a high shelf at 1.5 kHz of −12 dB at 0%, nothing at 50% and +12 dB at 100% before the Drive gain and the tube, and its inverse after the tube. Where the tube is linear Tone changes nothing; where it isn't, a high Tone makes the high part of the input the stronger source of distortion. (S1; the shelf is Beatlayer's own)
- **R8.** Hi-Quality (default off) runs the tube at 8 times the rate through the framework's oversampler: a 7 kHz sine driven hard at 48 kHz keeps its folded-back harmonics at least 60 dB under the fundamental in each model, and the device reports the oversampler's latency (56 frames) while it is on. (S1, S3)
- **R9.** Drive is a value in dB from 0 to 36 dB (default 6 dB), Bias runs from 0 to 100% (default 20%), and Output is a gain from −∞ to +6 dB (default 0 dB) applied after the tube and Tone's inverse. (S1; the ranges and defaults are Beatlayer's own)
- **R10.** The display holds the tube's curve at the Model, Drive and Bias now (with no envelope) over inputs from −1 to 1 as a series named Tube Curve, and, once an engine publishes, the input's level in dB as the reading Input with a marker on the curve. The cell stays a black frame in the generic UI; `device.display` reads both headlessly. (S1)
- **R11.** Renders are bit-identical at block sizes 1, 64, 512 and 4,096, and nothing on the audio thread allocates, locks or blocks. Tone's coefficients are updated every 16 frames counted from the device's first frame, so they don't follow the block size. (S2)
- **R12.** An agent adds Valve with `device.insert` or `device.add`, sets and reads every parameter by ID with `device.set_param` and `device.parameters` (Hi-Quality reported as a menu option), reads the display with `device.display`, and finds the device and its values when the saved set is opened. Output reaches −∞ dB and is silent there, and every parameter is automatable (device framework R2 and R3). (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/tube_distortion.rs`, `the_tube_is_described_and_laid_out` | unit |
| R2 | `crates/devices/tests/tube_distortion.rs`, `tube_a_is_clean_under_its_threshold_at_low_bias_and_distorts_past_it` | real-time trap |
| R3 | `crates/devices/tests/tube_distortion.rs`, `tube_c_distorts_at_every_level` | real-time trap |
| R4 | `crates/devices/tests/tube_distortion.rs`, `tube_b_lies_between_a_and_c` | real-time trap |
| R5 | `crates/devices/tests/tube_distortion.rs`, `the_envelope_makes_loud_sounds_dirtier_or_cleaner` | real-time trap |
| R6 | `crates/devices/tests/tube_distortion.rs`, `attack_and_release_are_time_constants_within_five_per_cent` | real-time trap |
| R7 | `crates/devices/tests/tube_distortion.rs`, `tone_steers_the_distortion_and_leaves_the_clean_signal_alone` | real-time trap |
| R8 | `crates/devices/tests/tube_distortion.rs`, `hi_quality_keeps_a_hard_driven_sines_harmonics_from_folding_back` | real-time trap |
| R9 | `crates/devices/tests/tube_distortion.rs`, `output_scales_the_result` | real-time trap |
| R10 | `crates/devices/tests/tube_distortion.rs`, `the_display_holds_the_curve_and_the_inputs_level` | unit, real-time trap |
| R11 | `crates/devices/tests/tube_distortion.rs`, `renders_are_bit_identical_at_any_block_size`; `rtsan/tests/devices.rs`, `the_saturation_devices_pass_under_rtsan` | real-time trap, RTSan |
| R12 | `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_the_saturation_devices_by_id_and_reopens_them`; `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity`, `every_parameter_is_automatable_to_the_frame` | MCP script, real-time trap |

## Not built yet

- The cell draws the tube's curve and a marker through the generic display; the curve moves with the envelope in Live, and here it shows the Bias alone.
- Everything the manual leaves unrecorded is Beatlayer's own and listed to observe: the models' curves (knees and offsets), how Bias and the envelope act on them, the ranges and defaults, what the Tone shelf does and where its corner is, and Hi-Quality's oversampling factor.
