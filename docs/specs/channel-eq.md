# The channel EQ

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-5.21
- **Parity-matrix row:** Audio effects: Channel EQ
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.7 "Channel EQ", read 18 October 2026; the shelf's shape, the mid band's Q, the high shelf's corner and the high-pass's order are Beatlayer's own

## The behaviour

Strip is Beatlayer's channel EQ, a three-band EQ in the style of a mixing desk: a switch for a high-pass filter at 80 Hz, a low shelf at 100 Hz, a sweepable mid peak, and a high band that is a shelf when it boosts and a shelf with a low-pass filter when it cuts, followed by Output. Its display shows the bands' combined response and the spectrum of what comes out.

The filters are Contour's: second-order sections from the standard audio-EQ cookbook, so each band's magnitude response is its analytic curve, and the spectrum is Contour's analyser (the [EQ spec](eq.md)). Strip adds no latency. The framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.7 "Channel EQ" | A three-band EQ. A switch turns on a high-pass filter at 80 Hz. The low band is a low shelf tuned to 100 Hz with a gain of ±15 dB, and its curve changes with the gain. The mid band is a peak of ±12 dB whose centre is swept from 120 Hz to 7.5 kHz. The high band boosts up to 15 dB with a high shelf; when it cuts, the shelf is joined by a low-pass filter whose cutoff falls from 20 kHz at no cut to 8 kHz at −15 dB. Output sets the level of the processed signal. A display shows the filter curves and the processed signal's spectrum |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain reaches −∞ dB; every parameter can be automated and modulated |

## Rules

- **R1.** Beatlayer ships Strip, an audio-effect device type of its own name and layout: every parameter has a descriptor and a place in the layout, and the parameters keep their values through a saved set and through the agent API. HP 80 is a switch, off at first; Low is ±15 dB, Mid ±12 dB, High ±15 dB, all at 0 dB at first; Mid Freq runs from 120 Hz to 7.5 kHz, at 1 kHz at first (Beatlayer's own default); Output is a gain from −∞ to +12 dB (Beatlayer's own top), at 0 dB at first. (S1, S2)
- **R2.** With every band at 0 dB, the high-pass off and Output at 0 dB, Strip's output equals its input, below −120 dB of difference. (S1)
- **R3.** With HP 80 on, the signal passes a second-order Butterworth high-pass (12 dB per octave, Beatlayer's own order) whose −3 dB point is 80 Hz, within 2 %. (S1)
- **R4.** The low band is a low shelf at 100 Hz, its midpoint, with a gain of ±15 dB: the response reaches the full gain (within 0.1 dB at 10 Hz) and returns to 0 dB above a few hundred Hz. Its shape follows its gain, as the manual says, by Beatlayer's rule: the shelf's Q is 0.5 at 0 dB and rises evenly to 0.71 at ±15 dB, so a larger gain makes a steeper shelf. Its curve is the cookbook's low shelf at that Q, within 0.1 dB. (S1)
- **R5.** The mid band is a peak at Mid Freq with a gain of ±12 dB: at its centre the response is the gain, within 0.1 dB, anywhere from 120 Hz to 7.5 kHz, and the curve is the cookbook's peak at a Q of 0.8 (Beatlayer's own), within 0.1 dB. (S1)
- **R6.** With High above 0 dB the high band is a high shelf, its midpoint at 6 kHz (Beatlayer's own) and its Q 1/√2, up to +15 dB. With High below 0 dB it is that shelf and a second-order Butterworth low-pass in series, the low-pass's −3 dB corner at 20 kHz × (8 kHz / 20 kHz)^(cut / 15 dB): 20 kHz as the cut begins, about 12.6 kHz at −7.5 dB, 8 kHz at −15 dB, within 2 %. At exactly 0 dB the band does nothing. (S1)
- **R7.** Output multiplies the signal after the bands and reaches −∞ dB, where Strip is silent. (S1, S2)
- **R8.** Strip adds no latency, and its output is the same, bit for bit, at any block size: the sections are redesigned from the parameters' values every 16 frames counted from the device's start, not from each block's. (S1)
- **R9.** Strip's display (device framework R36) holds the series Response, the sum of the sections' responses in dB at 96 places from 20 Hz to 20 kHz, worked out from the parameters; the marker Mid, at the mid peak's centre and gain; and, with an engine publishing, the series Spectrum, the output's 64 bands in dB. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/channel_eq.rs`, `the_channel_eq_is_described_with_its_ranges_and_defaults`; `crates/shell/tests/eqs_and_dynamics.rs`, `a_set_with_the_four_devices_reopens_with_every_parameter`; `crates/cli/tests/agent.rs`, `an_agent_adds_the_eqs_and_dynamics_sets_them_by_id_and_keys_them_from_another_track`; `rtsan/tests/devices.rs`, `the_channel_eq_passes_under_rtsan` | unit, MCP script, RTSan |
| R2 | `crates/devices/tests/channel_eq.rs`, `flat_with_the_high_pass_off_nulls_against_the_input` | unit, real-time trap |
| R3 | `crates/devices/tests/channel_eq.rs`, `the_high_pass_is_3_db_down_at_80_hz` | unit, real-time trap |
| R4 | `crates/devices/tests/channel_eq.rs`, `the_low_shelf_follows_its_curve_and_reaches_its_plateaus`, `the_low_shelfs_shape_changes_with_its_gain` | unit, real-time trap |
| R5 | `crates/devices/tests/channel_eq.rs`, `the_mid_peak_reaches_its_gain_across_its_sweep` | unit, real-time trap |
| R6 | `crates/devices/tests/channel_eq.rs`, `a_boosting_high_band_is_a_shelf`, `a_cutting_high_band_adds_a_low_pass_whose_corner_falls_with_the_cut` | unit, real-time trap |
| R7 | `crates/devices/tests/channel_eq.rs`, `output_scales_the_result_and_reaches_minus_infinity` | unit, real-time trap |
| R8 | `crates/devices/tests/channel_eq.rs`, `renders_are_bit_identical_at_any_block_size_and_add_no_latency` | unit, real-time trap |
| R9 | `crates/devices/tests/channel_eq.rs`, `the_display_holds_the_response_the_mid_marker_and_the_spectrum` | unit |
| G1, G2 | `crates/devices/tests/rules.rs` over the shipped types | real-time trap |
