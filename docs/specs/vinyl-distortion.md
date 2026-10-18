# Vinyl distortion

- **Status:** implemented
- **Work packages:** WP-5.24 (part 3; the equivalent of Vinyl Distortion)
- **Parity-matrix row:** Audio effects: Vinyl Distortion
- **Checked against:** Live 12's reference manual, section 29.41

## The behaviour

Wax is Beatlayer's equivalent of Live's Vinyl Distortion. It adds the distortion of a record to a signal in two parts, each working on a band of the spectrum that the user places: the tracing model, which adds even harmonics, and the pinch, which adds odd harmonics, in opposite phase between the two channels so that they widen the stereo image. A single Drive raises or lowers both, Soft and Hard choose between a gentler and a plainer distortion, and a crackle of random clicks can be added.

This spec covers the device's behaviour. The device framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.41 "Vinyl Distortion" | The tracing model adds even-harmonic distortion; its Drive sets the amount, and its Freq and bandwidth place the band it works on. The pinch adds odd harmonics, normally in opposite phase between the channels for a fuller stereo image, with the same controls. A global Drive raises or lowers the distortion of both. Soft imitates a dub plate and Hard a standard record. A Stereo or Mono switch decides whether the pinch's distortion is stereo or mono. Crackle adds noise: Density sets how much, and Volume sets its gain. |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter is automatable and modulatable |
| S3 | Beatlayer | The [device framework spec](device-framework.md), R29 | A random source follows the device's own seed, so a render repeats run after run; nothing depends on how blocks fall |

## Rules

The manual gives no ranges, defaults, curves or statistics, so everything below that it doesn't state is Beatlayer's own, listed to observe in Live.

- **R1.** Tracing Drive runs from 0 to 100 % (default 50 %), Tracing Freq from 20 Hz to 20 kHz (default 3 kHz, logarithmic) and Tracing Width from a quarter of an octave to four (default one). A band-pass filter of that centre and width picks the band out of the signal, and the tracing model adds twice the square of the band, scaled by the two Drives, to the signal, with the square's DC taken off by a 20 Hz high-pass. On a sine at Freq its second harmonic exceeds its third by at least 40 dB, and the same in both channels. (S1)
- **R2.** Pinch Drive, Pinch Freq (default 6 kHz) and Pinch Width have the same ranges and defaults as the tracing model's, apart from Freq. The pinch adds twice the cube of its band, scaled by the two Drives. On a sine at Pinch Freq its third harmonic exceeds its second by at least 40 dB. With Pinch Mode Stereo (the default) the first channel gets the pinch added and the second gets it taken away, so for the same input their third harmonics are in opposite phase; with Mono both get it added, in phase. (S1)
- **R3.** Drive runs from 0 to 100 % (default 100 %) and scales the distortion of both models: half the Drive gives half the distortion. At 0 % the signal passes exactly as it came. (S1)
- **R4.** Soft/Hard offers Soft and Hard (default Hard). Hard adds the distortion as it is; Soft halves it and passes it through a low-pass filter at 4 kHz, so it is gentler and duller. (S1)
- **R5.** Crackle Density runs from 0 to 250 clicks a second (default 0, which is off). After a gap of at least 2 ms, each frame has a chance of a click such that the average gap is 1 / Density, so the clicks a second come to Density within 10 %, in each channel. A click has a random peak between a quarter of full scale and full scale, of random sign, and dies away over 0.2 ms. The channels' clicks are independent. (S1)
- **R6.** Crackle Volume is a gain from −∞ to +6 dB (default −24 dB) that scales the clicks, and reaches −∞ dB (rule G1): there the crackle is silent. A Density of 0 gives no crackle at any Volume. (S1, S2)
- **R7.** The crackle follows the device's seed, so a render repeats run after run, and every value is worked out frame by frame, so a render is bit-identical at block sizes 1, 64, 512 and 4,096. The device has no latency. (S3)
- **R8.** Every parameter is set, read and saved by its ID: `device.insert`, `device.set_param` and `device.parameters` reach all of them, and a saved set reopens with the same values. Every parameter is automatable (rule G2). (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/vinyl_distortion.rs`, `the_tracing_models_second_harmonic_exceeds_its_third` | real-time trap |
| R2 | `crates/devices/tests/vinyl_distortion.rs`, `the_pinchs_third_harmonic_exceeds_its_second_in_opposite_phase_in_stereo_and_in_phase_in_mono` | real-time trap |
| R3 | `crates/devices/tests/vinyl_distortion.rs`, `drive_scales_both_models_and_at_zero_the_signal_passes_whole` | real-time trap |
| R4 | `crates/devices/tests/vinyl_distortion.rs`, `soft_distorts_less_than_hard` | real-time trap |
| R5 | `crates/devices/tests/vinyl_distortion.rs`, `the_crackles_clicks_per_second_are_within_ten_per_cent_of_density` | real-time trap |
| R6 | `crates/devices/tests/vinyl_distortion.rs`, `crackle_volume_at_minus_infinity_is_silent_and_no_density_is_no_crackle`; `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity` | real-time trap |
| R7 | `crates/devices/tests/vinyl_distortion.rs`, `a_render_repeats_run_after_run_and_at_any_block_size`; `rtsan/tests/devices.rs`, `wax_passes_under_rtsan` | real-time trap, RTSan |
| R8 | `crates/cli/tests/agent.rs`, `an_agent_sets_the_lo_fi_effects_by_id_and_finds_them_after_a_save_and_reopen`; `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame` | MCP script, real-time trap |

## Not built yet

- The X–Y display of each model's Freq, Drive and bandwidth. Until then the layout has no display cell, and those parameters are ordinary controls.
