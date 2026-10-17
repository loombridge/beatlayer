# The compressor

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-2.18a (part 2; the compressor with sidechain); WP-5.43 (the display, R14 to R16)
- **Parity-matrix row:** Devices: EQ Eight, Compressor, Limiter, Reverb, Delay, Utility
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.9 "Compressor" and its sidechain section, read 15 October 2026; Live 12.4.6 on macOS, observed 12 October 2026 (the sidechain and device controls); the parameter ranges and defaults are Beatlayer's own

## The behaviour

Squeeze is Beatlayer's compressor: it turns a signal's gain down above a threshold, by a ratio that reaches infinity, over a knee; it can expand above the threshold instead, detect peaks or an RMS level, shape its envelope linearly or logarithmically, look ahead, compensate its output with Makeup, blend the compressed and dry signals with Dry/Wet, and listen to another track through its sidechain section, whose own SC Filter hears only a band of it.

The detector hears what the engine hands the device on its sidechain (WP-2.19b: another track through the sidechain's gain and mix, or the device's own input), and the curve turns its level into a gain, over the knee: compression turns the gain down by the ratio, expansion turns it up. The gain falls with the Attack time constant and recovers with the Release one, or a signal-dependent time with Auto Release. The device's two channels share one gain reduction, the larger of the two. The signal is delayed by the look-ahead, so the gain has begun to fall when a peak comes out, and that delay is the device's latency for delay compensation (WP-2.11). The display cell shows the gain reduction as a meter, and the transfer curve or the activity as its view says (R14 to R16); the framework (parameters, smoothing, layout, automation) is the [device framework spec](device-framework.md), and the sidechain's section, source and mix are the [routing spec](routing.md)'s (R22 to R31).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.9 "Compressor" | Threshold and Ratio (a ratio of 1 is no compression); the gain reduction meter; Out and the Makeup button, which automatically compensates the output level if the threshold and ratio change; Dry/Wet, which at 0 % bypasses the device; the Knee (0 dB is a hard knee, and a 10 dB knee with a −20 dB threshold begins compressing at −30 dB and is full at −10 dB); the display's Collapsed, Transfer Curve and Activity views; Attack and Release, and Auto Release, which adjusts the release time from the incoming audio; the look-ahead times zero, one and ten ms; Peak, RMS and Expand, where a ratio of 1 to 2 raises a dB over the threshold to two; the Lin and Log envelope shapes |
| S2 | Manual | Same, section 29.9.1 "Sidechain Parameters" | External sidechaining; Gain; Dry/Wet blends the sidechain with the device's own input; the headphones button listens to the sidechain; the sidechain EQ triggers on a frequency band; automatic Makeup is not available with an external sidechain |
| S3 | Observation | Live 12.4.6 on macOS, 12 October 2026, the sidechain and device controls: Compressor on an audio track | The sidechain section's controls (the framework's, the routing spec's); Compressor's own SC Filter column, with its toggle, Type, Freq and Res; the display cell with Thresh, GR/Output and Out along its top and the view buttons, Knee, Look. and Env. along its bottom; Peak, RMS, Expand, Makeup and SC Filter as text toggles |
| S4 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter can be automated and modulated |

## Rules

- **R1.** Beatlayer ships Squeeze, an audio-effect device type with a sidechain input, of its own name and layout: every parameter has a descriptor and a place in the layout. (S3, device-framework)
- **R2.** Threshold sets the level above which the curve turns the gain down, over the Knee: none below the threshold less half the knee, all of it by the threshold plus half, and gradually between, as the manual's example describes. (S1)
- **R3.** Ratio is the input's dB per the output's dB above the threshold, from 1 : 1 to ∞ : 1: at the top of the control nothing above the threshold gets through. The framework's ratio values are finite, so Beatlayer's tops at 100, where the gain computer's slope is 1 and the control reads `100 : 1`. (S1, S4)
- **R4.** Expand mode turns the gain up above the threshold: a ratio of 1 to 2 raises a dB over the threshold to two. The expansion stops at +24 dB (Beatlayer's limit). (S1)
- **R5.** Attack and Release are the gain's time constants, in milliseconds: after one of them, the gap has closed by 1 − 1/e, whether the gain is falling to a new reduction (Attack) or recovering toward unity (Release). (S1, S4)
- **R6.** With Auto Release on, the release time is Beatlayer's own: 50 ms just after a peak, moving to 1000 ms while a reduction has been held for 300 ms. (S1)
- **R7.** Env is Lin or Log: Log's release step grows with the depth of the reduction (twice as fast at 6 dB, Beatlayer's own shape), so sharply compressed peaks let go sooner. (S1)
- **R8.** Mode is Peak or RMS: Peak follows every sample's magnitude, while RMS follows a 10 ms mean square, so a short peak barely moves it. (S1)
- **R9.** Look offers 0, 1 and 10 ms. The output is delayed by the look-ahead, and the device reports that delay as its latency from the moment it is built, for delay compensation (WP-2.11). (S1)
- **R10.** Makeup adds the reduction the threshold and ratio take from a 0 dB signal (the slope times the threshold's distance from 0 dB), and adds nothing while an external sidechain is on, as the manual says. (S2)
- **R11.** Out is the output gain, reaching −∞ dB (rule G1); Dry/Wet blends the compressed and dry signals, and at 0 % the device is effectively bypassed. (S1, S4)
- **R12.** With External on, the detector hears the sidechain's key: another track's signal through SC Gain and Mix, or the device's own input with Mix, as the routing spec's R24 says; with External off it hears its own input. A key above the threshold turns the device's gain down, so one track ducks under another. (S2, routing R24)
- **R13.** SC Filter, its Type (Low Pass, Band Pass, High Pass), Freq and Res filter what the detector hears, so only a band of it triggers the device. Beatlayer's types and ranges are its own, and the filter acts on the trigger only. (S2, S3)
- **R14.** Squeeze publishes three values after each block, through the framework's display data (the device framework spec, R35), whatever the block's size: the gain now in dB, and the detector's input and the output's peaks since the last block in dB. (S1, S3)
- **R15.** Squeeze's display (device framework R36) holds, with an engine publishing, the readings Gain Reduction, Input and Output in dB; always, the Transfer Curve, the output level for each input level from −60 dB to 0 dB, a point to the dB, from the threshold, ratio, knee and mode; and the markers Threshold, on the curve at the threshold, and Level, on it at the detector's input level now. (S1)
- **R16.** The display offers the views Transfer Curve, Activity and Collapsed, in that order, and starts in the first; the view shown is view state (device framework R38). Every view draws the gain reduction as a meter down the cell's right edge, in dB of reduction over 24 dB. Transfer Curve draws the curve with the two markers; Activity draws the input's level scrolling to the left, with the gain reduction hanging from the top; Collapsed draws the meter alone. The views' buttons sit along the cell's top-right corner. (S1, S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/compressor.rs`, `the_compressor_is_described_and_declares_a_sidechain` | unit, real-time trap |
| R2, R3 | `crates/devices/tests/compressor.rs`, `the_static_curve_matches_at_several_ratios_and_knees` | unit, real-time trap |
| R4 | `crates/devices/tests/compressor.rs`, `expand_mode_raises_the_output_above_the_threshold` | unit, real-time trap |
| R5 | `crates/devices/tests/compressor.rs`, `the_attack_and_release_time_constants_are_their_parameters` | unit, real-time trap |
| R6 | `crates/devices/tests/compressor.rs`, `auto_release_recovers_more_quickly_than_a_long_fixed_release` | unit, real-time trap |
| R7 | `crates/devices/tests/compressor.rs`, `the_log_envelope_releases_faster_from_a_deeper_reduction` | unit, real-time trap |
| R8 | `crates/devices/tests/compressor.rs`, `rms_mode_is_less_sensitive_to_a_short_peak` | unit, real-time trap |
| R9 | `crates/devices/tests/compressor.rs`, `the_look_ahead_delays_the_output_and_is_reported_as_latency` | unit, real-time trap |
| R10 | `crates/devices/tests/compressor.rs`, `makeup_compensates_the_output_without_an_external_sidechain` | unit, real-time trap |
| R11 | `crates/devices/tests/compressor.rs`, `dry_wet_mixes_the_compressed_and_dry_signals` | unit, real-time trap |
| R12 | `crates/devices/tests/compressor.rs`, `a_sidechain_key_ducks_the_input`; `crates/engine/tests/sidechain.rs`, `a_device_hears_its_source_through_gain_and_mix_and_listen_plays_it` | unit, real-time trap |
| R13 | `crates/devices/tests/compressor.rs`, `the_sc_filter_hears_only_its_band` | unit, real-time trap |
| R14 | `crates/shell/tests/device_displays.rs`, `the_contours_spectrum_and_the_squeezes_gain_reduction_are_published_after_every_block`; `rtsan/tests/device_displays.rs`, `displays_publish_after_every_block_under_rtsan` | unit, real-time trap, RTSan |
| R15 | `crates/shell/tests/device_displays.rs`, `the_squeezes_view_is_view_state_that_agents_set_and_read`; `crates/cli/tests/agent.rs`, `an_agent_reads_a_contours_spectrum_and_a_squeezes_gain_reduction_by_device_id` | unit, MCP script |
| R16 | `crates/ui/tests/device_panel.rs`, `the_squeezes_cell_draws_its_gain_reduction_and_the_chosen_view`; `crates/shell/tests/device_displays.rs`, `the_squeezes_view_is_view_state_that_agents_set_and_read` | UI script, unit |
| G1, G2 | `crates/devices/tests/rules.rs` over the shipped types | real-time trap |
