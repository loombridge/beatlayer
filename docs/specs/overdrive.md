# The overdrive

- **Status:** partial
- **Work packages:** WP-5.23 (with the [saturator](saturator.md) and the [tube distortion](tube-distortion.md))
- **Parity-matrix row:** Audio effects: Overdrive
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.27, read on 18 October 2026

## The behaviour

Grit is Beatlayer's overdrive, a distortion in the manner of a guitar pedal that can be driven hard without losing its dynamics. The signal goes through a band-pass filter, then the distortion, and a Tone control after it; the band-pass decides which part of the signal reaches the distortion, and the user sets its place (Frequency) and its width (Width), which a drawn X–Y display would also move. Drive sets the amount of distortion, and even at 0% the signal is distorted. Dynamics decides how far the distortion's level follows Drive: at low settings more Drive means more compression and make-up gain, at high settings less. Dry/Wet balances the processed signal against the dry one.

This spec covers the device; the device framework is the [device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.27 "Overdrive" | A distortion modelled on classic guitar pedals that can be driven very hard without losing dynamic range. A band-pass filter sits before the distortion; dragging an X–Y controller sets its band's position (horizontally) and its bandwidth (vertically), and two sliders below offer the same settings. Drive sets the distortion's amount, and at 0% the signal is still distorted. Tone is an EQ after the distortion; higher values add high frequencies. Dynamics sets the compression during distortion: at low settings more distortion means more internal compression and make-up gain, at higher settings less compression. Dry/Wet balances processed and dry signals |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain reaches −∞ dB; every parameter can be automated and modulated |

## Rules

- **R1.** Beatlayer ships Grit, an audio-effect device type of its own name and layout, described like any native device: every parameter has a descriptor and a place in the layout, with no menu items and no expanded part. It has no latency. The X–Y display's two parameters, Frequency and Width, are ordinary controls in the cell Drive Curve's bottom band. (S1, S2)
- **R2.** The band-pass is a second-order filter of 0 dB gain at its centre. Frequency, from 80 Hz to 8 kHz (default 1.2 kHz, logarithmic), is its centre, and Width, from 0.2 to 4 octaves (default 1.5), is the distance between its −3 dB points, both within 2% at a small signal; the centre is the geometric mean of the −3 dB points. (S1; the filter and ranges are Beatlayer's own)
- **R3.** Drive, from 0 to 100% (default 50%), sets a gain before the distortion of 6 dB at 0% to 36 dB at 100%, in a straight line over the percentage, and the distortion is the `tanh` of the gain times the band-passed signal. So even Drive 0% adds harmonics to a signal of moderate level, and a harder drive adds more. (S1; the gain range and the curve are Beatlayer's own)
- **R4.** Tone, from 0 to 100% (default 50%), is a high shelf at 1.5 kHz after the distortion, of −12 dB at 0%, nothing at 50% and +12 dB at 100%. (S1; the shelf is Beatlayer's own)
- **R5.** Dynamics, from 0 to 100% (default 50%), sets how far a gain after the distortion pulls its level toward a fixed target. A follower (attack 20 ms, release 200 ms, the larger of the two channels' levels) measures the distorted signal's mean absolute value `L`, and the gain is `(0.3 / max(L, 0.01))^(0.8 · (1 − Dynamics))`, limited to between 1/8 and 8. At Dynamics 0% the output level changes little across a 30 dB sweep of Drive (a sine at −20 dBFS moves less than 6 dB); at 100% there is no gain and the level follows the distortion (it moves more than 12 dB, and more than twice what 0% does). (S1; the law and the times are Beatlayer's own)
- **R6.** Dry/Wet runs from 0 to 100% (default 100%); at 0% the output is the input exactly. The band-pass, the distortion and Tone are in the processed signal only. (S1)
- **R7.** The display holds the distortion curve over inputs from −1 to 1 at the Drive now as a series named Drive Curve, and, once an engine publishes, the input's level in dB as the reading Input with a marker on the curve. The cell stays a black frame in the generic UI; `device.display` reads both headlessly. (S1)
- **R8.** Renders are bit-identical at block sizes 1, 64, 512 and 4,096, and nothing on the audio thread allocates, locks or blocks. The filters' coefficients are updated every 16 frames counted from the device's first frame, so they don't follow the block size. (S2)
- **R9.** An agent adds Grit with `device.insert` or `device.add`, sets and reads every parameter by ID with `device.set_param` and `device.parameters`, reads the display with `device.display`, and finds the device and its values when the saved set is opened. Every parameter is automatable (device framework R3). (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/overdrive.rs`, `the_overdrive_is_described_and_laid_out` | unit |
| R2 | `crates/devices/tests/overdrive.rs`, `the_band_pass_has_its_centre_and_width_within_two_per_cent` | real-time trap |
| R3 | `crates/devices/tests/overdrive.rs`, `harmonics_are_present_at_drive_zero` | real-time trap |
| R4 | `crates/devices/tests/overdrive.rs`, `tone_raises_the_highs` | real-time trap |
| R5 | `crates/devices/tests/overdrive.rs`, `dynamics_levels_the_output_across_a_drive_sweep` | real-time trap |
| R6 | `crates/devices/tests/overdrive.rs`, `dry_wet_at_zero_is_the_input` | real-time trap |
| R7 | `crates/devices/tests/overdrive.rs`, `the_display_holds_the_curve_and_the_inputs_level` | unit, real-time trap |
| R8 | `crates/devices/tests/overdrive.rs`, `renders_are_bit_identical_at_any_block_size`; `rtsan/tests/devices.rs`, `the_saturation_devices_pass_under_rtsan` | real-time trap, RTSan |
| R9 | `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_the_saturation_devices_by_id_and_reopens_them`; `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame` | MCP script, real-time trap |

## Not built yet

- The cell draws the distortion curve and a marker through the generic display; Live's cell is an X–Y controller for the band-pass, which is not drawn or draggable here (Frequency and Width are knobs).
- Everything the manual leaves unrecorded is Beatlayer's own and listed to observe: the band-pass's kind and ranges, the distortion curve and the Drive gain range, the Tone shelf, and Dynamics' law, its times and its target.
