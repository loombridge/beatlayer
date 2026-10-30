# Digital degradation

- **Status:** partial
- **Work packages:** WP-5.24 (part 1; Erosion)
- **Parity-matrix row:** Audio effects: Erosion, to Live's current version (the 12.4 Erosion)
- **Checked against:** Live 12.4's reference manual, section 29.17, and the 12.4 release notes

## The behaviour

Erosion is Beatlayer's version of Live's Erosion. It delays the signal by a few milliseconds and moves that delay with a modulation made of a sine wave and band-filtered noise, which smears the signal: from a warble when the sine leads, through sidebands around every partial, to a gritty, noisy roughness when the noise leads. The user sets the modulation's frequency (the sine's frequency, and the centre of the noise's band), how far it moves the delay, the balance between sine and noise, the width of the noise's band, and how different the two channels' modulation is. A display shows the spectrum of the input.

This spec covers the device's behaviour. The device framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md), and delay compensation is [its own spec](delay-compensation.md). Live's older Erosion, which sets from before 12.4 load as Erosion Legacy, is not built; a Live set or preset holding it maps it to Erosion ([Live device mapping](live-device-mapping.md) R15).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.17 "Erosion" | Erosion adds digital degradation by modulating a short delay with a sine wave mixed with filtered noise. Its display shows the input's spectrum and works as an X–Y controller: horizontally it sets the sine's frequency and the noise filter's centre, vertically the modulation amount, and with a modifier key held, vertically it sets the noise filter's width instead. Noise Blend balances the two carriers, from sine only at 0 % to noise only at 100 %. Filter Width sets the noise filter's bandwidth and is inactive at a blend of 0 %. Stereo Width morphs the modulation from mono at 0 % to stereo at 100 %. |
| S2 | Manual | Live 12 release notes, 12.4 | Erosion was redesigned: Noise Blend mixes sine and noise modulation, Stereo Width goes from mono to stereo, and its algorithmic latency fell from about 5 ms to about 2 ms. Sets from before 12.4 load the previous version as Erosion Legacy. |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter is automatable and modulatable |
| S4 | Beatlayer | The [device framework spec](device-framework.md), R29 | A random source follows the device's own seed, so a render repeats run after run; nothing depends on how blocks fall |
| S5 | Beatlayer | The [delay compensation spec](delay-compensation.md), R1 | A device reports its latency in frames, and the engine delays the other tracks to line up |

## Rules

Everything below that the manual doesn't state (the ranges, the defaults, the delay's length and depth, the noise's level, and the filter's shape) is Beatlayer's own, listed to observe in Live.

- **R1.** Freq runs from 20 Hz to 20 kHz (default 1 kHz, logarithmic) and Amount from 0 to 100 % (default 50 %). With Noise Blend at 0 %, a sine input gains sidebands at its frequency plus and minus Freq, within 1 %, and their level grows as Amount grows. The delay moves by Amount × 1.5 ms either way at most. (S1)
- **R2.** Noise Blend runs from 0 to 100 % (default 0 %). At 100 % the modulation is band-filtered noise alone, so a sine input gains no line sidebands, only a band of noise around its frequency plus and minus Freq. Between the two the carriers are mixed in proportion. (S1)
- **R3.** At an Amount of 0 the output is the input delayed by the reported latency alone: once the two are aligned, the difference is below −100 dB, at any Noise Blend and Stereo Width. (S1, S2)
- **R4.** Filter Width runs from 0 to 100 % (default 50 %) and sets the noise band's width between its half-power points: a quarter of an octave at 0 %, four octaves at 100 %, geometrically between, centred on Freq. A narrower band keeps more of the noise's power near Freq. At a Noise Blend of 0 % it changes nothing. (S1)
- **R5.** Stereo Width runs from 0 to 100 % (default 0 %). At 0 % both channels have the same modulation, so channels that hear the same input come out identical. As it rises the right channel's sine moves out of phase with the left's (opposed at 100 %) and its noise moves from the left's to an independent noise of the same level. (S1)
- **R6.** The noise follows the device's seed, so a render repeats run after run, and every value is worked out frame by frame, so a render is bit-identical at block sizes 1, 64, 512 and 4,096. (S4)
- **R7.** The delay's centre is 2 ms, and the device reports it as its latency (96 frames at 48 kHz) for delay compensation. This matches the 12.4 figure of about 2 ms. (S2, S5)
- **R8.** The display shows the input's spectrum, in 64 logarithmic bands from 20 Hz to 20 kHz, in decibels, once the engine has published it (`device.display`); before that it is empty. (S1)
- **R9.** Every parameter is set, read and saved by its ID: `device.insert`, `device.set_param` and `device.parameters` reach all of them, and a saved set reopens with the same values. Every parameter is automatable (rule G2). (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/digital_degradation.rs`, `a_sine_input_gains_sidebands_at_its_frequency_plus_and_minus_freq_growing_with_amount` | real-time trap |
| R2 | `crates/devices/tests/digital_degradation.rs`, `at_a_noise_blend_of_100_per_cent_the_sidebands_are_not_lines` | real-time trap |
| R3 | `crates/devices/tests/digital_degradation.rs`, `amount_0_passes_the_input_delayed_only_by_the_reported_latency` | real-time trap |
| R4 | `crates/devices/tests/digital_degradation.rs`, `filter_width_sets_how_wide_the_noise_band_is`, `filter_width_does_nothing_at_a_noise_blend_of_0`; `crates/devices/src/degrade.rs`, `filter_width_runs_from_a_quarter_octave_to_four` | real-time trap, unit |
| R5 | `crates/devices/tests/digital_degradation.rs`, `stereo_width_0_gives_identical_channels_and_more_does_not` | real-time trap |
| R6 | `crates/devices/tests/digital_degradation.rs`, `a_render_repeats_run_after_run_and_at_any_block_size`; `rtsan/tests/devices.rs`, `degrade_passes_under_rtsan` | real-time trap, RTSan |
| R7 | `crates/devices/tests/digital_degradation.rs`, `the_latency_is_about_two_milliseconds` | real-time trap |
| R8 | `crates/devices/tests/digital_degradation.rs`, `the_display_shows_the_inputs_spectrum` | unit |
| R9 | `crates/cli/tests/agent.rs`, `an_agent_sets_the_lo_fi_effects_by_id_and_finds_them_after_a_save_and_reopen`; `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame` | MCP script, real-time trap |

## Not built yet

- The display as an X–Y control: dragging horizontally to set Freq, vertically to set Amount, and with the modifier key vertically to set Filter Width. Until then the display cell is a frame that shows the spectrum, and Freq, Amount and Filter Width are ordinary controls. Live also lights icons under the display to show which carrier is dominant, and draws the sine as a line and the noise as dotted lines; this spec doesn't cover that drawing.
- Erosion Legacy, the version before 12.4, which Live set import maps to Erosion ([Live device mapping](live-device-mapping.md) R15).
