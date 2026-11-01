# Re-Enveloper: a three-band envelope processor

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-8.50, with [Color Limiter](color-limiter.md) (all of it: R1 to R10). Its crossovers are [Multiband Dynamics](multiband-dynamics.md)'s, the same code; the framework is WP-2.5's ([the native device framework](device-framework.md)), its envelope follower WP-5.42's
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Creative Extensions pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Re-Enveloper changes the shape of a sound's envelope rather than its level: in three bands it can flatten the envelope, so attacks and tails come closer to one level, or exaggerate it, so attacks stand out and tails fall away. Two crossovers set the bands, and each band's C/E Factor compresses its envelope when positive and expands it when negative.

Live's Re-Enveloper comes in its Creative Extensions pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.re_enveloper`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Re-Enveloper, a multiband envelope processor working on the signal's envelope rather than its level, with three adjustable bands, each with a C/E Factor that compresses above 0 and expands below |
| S2 | Beatlayer | [Multiband Dynamics](multiband-dynamics.md) (three bands split by zero-latency Linkwitz-Riley crossovers); [the device framework](device-framework.md) R4, R28; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | Crossovers that sum flat; the envelope follower; banks; gains and automation |

## Rules

- **R1.** Re-Enveloper (`beatlayer.re_enveloper`) is an audio effect with Live's name, Beatlayer's own layout and one display cell drawing the three bands with each band's envelope and gain, whose data `device.display` reports. (S1; the layout is Beatlayer's own)
- **R2.** Low Crossover (50 Hz to 2 kHz, 200 Hz) and High Crossover (500 Hz to 16 kHz, 2.5 kHz, kept above Low) split the input into three bands with [Multiband Dynamics](multiband-dynamics.md)'s crossovers, the same code, whose sum is flat within 0.1 dB with every C/E Factor at 0. (S1, S2; ranges and defaults are Beatlayer's own)
- **R3.** In each band, a fast envelope (a follower with 1 ms attack and 30 ms release, device framework R28) is compared with a slow one (50 ms attack and 500 ms release); with e the fast envelope's level over the slow one's, in dB, the band's gain in dB is −F × e, F being its C/E Factor from −100 to +100 % (0) as a fraction, limited to ±24 dB. A positive F so lowers attacks and raises tails toward the slow level (compressing the envelope); a negative F raises attacks and lowers tails (expanding it). (S1; the law is Beatlayer's own)
- **R4.** On a generated percussive envelope (a 1 ms attack and 300 ms exponential decay on noise, in one band), F = +100 % brings the attack's peak within 3 dB of the tail's level at 100 ms, and F = −100 % raises their difference by at least 12 dB, as measured on the band's output. (S1; the figures are Beatlayer's own)
- **R5.** Each band has a Gain, a gain from −∞ to +12 dB (0 dB, rule G1), after its envelope stage; Output, a gain from −∞ to +12 dB (0 dB), sets the sum's level. The device reports no latency. (S1, S2; the controls beyond the pack page's are Beatlayer's own)
- **R6.** A render is bit-identical at block sizes 1, 64, 512 and 4,096, and nothing on the audio thread allocates, locks or blocks. (S2)
- **R7.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R8.** Re-Enveloper declares one bank of eight ([control surfaces](control-surfaces.md) R19): Low Crossover, High Crossover, the three C/E Factors, the three band Gains, with Output in a second, so every automatable parameter is in a bank. (S2; Beatlayer's own)
- **R9.** A set with the device saves and reopens with every parameter. (S2)
- **R10.** The device reuses Multiband Dynamics' crossovers rather than a copy of them. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/re_enveloper.rs`, `re_enveloper_is_described_with_two_banks_and_no_latency`, `the_display_reads_each_bands_envelope_and_gain` | unit |
| R2 | `crates/devices/tests/re_enveloper.rs`, `with_every_factor_at_zero_the_crossovers_sum_flat_within_a_tenth_of_a_decibel` | unit, real-time trap |
| R3, R4 | `crates/devices/tests/re_enveloper.rs`, `a_positive_factor_flattens_a_percussive_envelope_and_a_negative_one_sharpens_it` | unit, real-time trap |
| R5 | `crates/devices/tests/re_enveloper.rs`, `band_gains_and_output_reach_minus_infinity`, `output_sets_the_level_of_the_sum`, `re_enveloper_is_described_with_two_banks_and_no_latency` | unit, real-time trap |
| R6 | `crates/devices/tests/re_enveloper.rs`, `renders_are_the_same_at_any_block_size`; `rtsan/tests/devices.rs`, `re_enveloper_passes_under_rtsan` | real-time trap, RTSan |
| R7 | `crates/cli/tests/agent.rs`, `an_agent_inserts_color_limiter_and_re_enveloper_and_sets_and_reads_them`; `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_is_modulatable_to_the_frame` | MCP script, real-time trap |
| R8 | `crates/devices/tests/banks.rs`, `every_native_device_types_banks_hold_every_parameter_and_none_more_than_eight` | unit |
| R9 | `crates/shell/tests/creative_extensions.rs`, `a_set_with_re_enveloper_reopens_with_every_parameter` | unit |
| R10 | `crates/devices/src/re_enveloper.rs` calls `multiband_dynamics::Crossover` (Multiband Dynamics' own: `crates/devices/tests/multiband_dynamics.rs`) | review |
