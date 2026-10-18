# The bit-crusher

- **Status:** implemented
- **Work packages:** WP-5.24 (part 2; the equivalent of Redux)
- **Parity-matrix row:** Audio effects: Redux, to Live's current version
- **Checked against:** Live 12's reference manual, sections 29.30 to 29.30.2

## The behaviour

Crush is Beatlayer's equivalent of Live's Redux. It lowers the sample rate and the bit depth of the signal. The rate reduction holds the signal at a lower rate, so its images appear around multiples of that rate; a jitter on its clock roughens it and widens the stereo image, and a low-pass filter before and one after can narrow the signal that is held and soften the images. The bit reduction rounds the signal to a number of levels, with a shape that makes the steps finer for quiet parts and a DC shift that moves where the rounding falls. Dry/Wet mixes the result with the input.

This spec covers the device's behaviour. The device framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.30.1 "Downsampling" | Rate sets the sample rate the signal is reduced to, and lower values give more imaging and more inharmonic tones. Jitter adds noise to the downsampler's clock, which makes it noisier and widens the stereo image. Pre turns on a filter before the downsampling that narrows the bandwidth being downsampled (and, with Jitter, the stereo width). Post turns on a low-pass filter after it to reduce imaging, and its cutoff is set in octaves above or below half the Rate. |
| S2 | Manual | The same chapter, section 29.30.2 "Bit Reduction" | Bits sets how many bits the output has; fewer give more noise and distortion and less dynamic range. Shape changes the quantiser's curve so that higher values give finer resolution at low amplitudes. DC Shift adds an offset to the amplitude before the quantiser, which changes the distortion's character, most at low bit counts. Dry/Wet sets the balance between processed and unprocessed signal. |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter is automatable and modulatable |
| S4 | Beatlayer | The [device framework spec](device-framework.md), R29 | A random source follows the device's own seed, so a render repeats run after run; nothing depends on how blocks fall |

## Rules

Everything below that the manual doesn't state (the ranges, the defaults, the filters' shape and the quantiser's exact curves) is Beatlayer's own, listed to observe in Live.

- **R1.** Rate runs from 100 Hz to 48 kHz (default 8 kHz, logarithmic). The signal is held at that rate: the output's lines sit at multiples of the Rate plus and minus the input's frequency, within 1 Hz. At the engine's rate or above, the signal isn't held at all. (S1)
- **R2.** Bits runs from 2 to 16 (default 8). The quantiser has `2^Bits` levels, evenly spaced from −1 to one step under +1, with a level at zero, so a ramp through full scale takes exactly that many values. (S2)
- **R3.** Shape runs from 0 to 100 % (default 0 %). At 0 % the steps are even. As it rises the quantiser works on the signal raised to the power `1 / (1 + 3 × Shape)`, so its steps are finer at small amplitudes and coarser at large ones, and full scale still reaches the top level the plain quantiser reaches. (S2)
- **R4.** DC Shift runs from −50 % to +50 % (default 0 %) of one step. It is added to the signal, in steps, before the rounding and is not taken off afterwards, so it moves where the quantiser's thresholds fall and leaves its levels where they are. (S2)
- **R5.** Post is a 24 dB per octave low-pass filter after the hold (default off), whose corner is half the Rate times two to the power of Post Octave, Post Octave running from −4 to +4 (default 0). Its corner sits at that frequency within 2 %. The corner never passes 0.45 of the engine's rate. (S1)
- **R6.** Pre is the same filter before the hold (default off), with its corner at half the Rate; it does nothing when the Rate is at or above the engine's. (S1)
- **R7.** Jitter runs from 0 to 100 % (default 0 %). It varies the step of each channel's hold clock by up to ±50 % at 100 %, each channel with a random source of its own, so channels that hear the same input stop being identical: their correlation falls below 1. At 0 % they are identical. (S1, S4)
- **R8.** Dry/Wet runs from 0 to 100 % (default 100 %). At 0 % the output is the input exactly. (S2)
- **R9.** The jitter follows the device's seed, so a render repeats run after run, and every value is worked out frame by frame, so a render is bit-identical at block sizes 1, 64, 512 and 4,096. The device has no latency. (S4)
- **R10.** Every parameter is set, read and saved by its ID: `device.insert`, `device.set_param` and `device.parameters` reach all of them, and a saved set reopens with the same values. Every parameter is automatable (rule G2). (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/bit_crusher.rs`, `below_the_engines_rate_the_images_sit_at_multiples_of_the_rate_around_the_input` | real-time trap |
| R2 | `crates/devices/tests/bit_crusher.rs`, `bits_n_gives_two_to_the_n_levels_on_a_ramp` | real-time trap |
| R3 | `crates/devices/tests/bit_crusher.rs`, `shape_makes_the_steps_finer_near_zero` | real-time trap |
| R4 | `crates/devices/tests/bit_crusher.rs`, `dc_shift_moves_the_quantisers_offset` | real-time trap |
| R5 | `crates/devices/tests/bit_crusher.rs`, `the_post_filters_corner_sits_at_its_octave_setting` | real-time trap |
| R6 | `crates/devices/tests/bit_crusher.rs`, `the_pre_filter_takes_out_what_the_rate_cannot_hold` | real-time trap |
| R7 | `crates/devices/tests/bit_crusher.rs`, `jitter_lowers_the_correlation_between_the_channels` | real-time trap |
| R8 | `crates/devices/tests/bit_crusher.rs`, `dry_wet_0_per_cent_nulls_against_the_input` | real-time trap |
| R9 | `crates/devices/tests/bit_crusher.rs`, `a_render_repeats_run_after_run_and_at_any_block_size`; `rtsan/tests/devices.rs`, `crush_passes_under_rtsan` | real-time trap, RTSan |
| R10 | `crates/cli/tests/agent.rs`, `an_agent_sets_the_lo_fi_effects_by_id_and_finds_them_after_a_save_and_reopen`; `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame` | MCP script, real-time trap |
