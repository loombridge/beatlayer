# The bus compressor

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-5.21
- **Parity-matrix row:** Audio effects: Glue Compressor
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", sections 29.21 "Glue Compressor" and its sidechain and context-menu parts, and the release notes of 12.2, read 18 October 2026; the ratio's values, the knee's law, the ranges and defaults, Auto Release's times and the waveshaper are Beatlayer's own

## The behaviour

Bond is Beatlayer's bus compressor, a compressor for gluing a group of sounds together: Threshold and Ratio set how much of a level above the threshold is turned down, with a knee that is not a control of its own but sharpens as the ratio rises; Attack (in milliseconds) and Release (in seconds, or Auto, where the release follows the audio) shape how the reduction comes and goes; Range caps the reduction, at 0 dB applying none; Makeup adds gain afterwards; Soft Clip rounds loud peaks so the output stays under −0.5 dB; Dry/Wet blends in the unprocessed signal. A reading shows the gain reduction and a light shows clipping. Oversampling, in the device's Show Options menu, runs the gain stage at twice the sample rate. Its sidechain section lets another track's signal drive the detector, through an EQ that hears only a band of it, or lets the user listen to that signal.

The detector hears what the engine hands the device on its sidechain (the [routing spec](routing.md), R22 to R31). Its gain computer is Squeeze's, with a knee that narrows with the ratio, and its SC filter is Squeeze's ([compressor spec](compressor.md), R13). The framework is the [device framework spec](device-framework.md): its oversampler (R8); a device's own Show Options items are the [device chains spec](device-chains.md)'s (R24).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.21 "Glue Compressor" | Threshold sets where compression begins; Ratio the input-to-output ratio above it, with no adjustable knee, the knee sharpening as the ratio rises. Attack is the time to full compression, in milliseconds. Release is the time to return to normal, in seconds; set to A (Auto), it adapts to the audio with a slow base time plus a fast time reacting to transients. Range limits how much compression can occur; about −60 to −70 dB emulates the original hardware, about −40 to −15 dB works as an alternative to Dry/Wet, and 0 dB means no compression. Makeup adds gain to offset the compression. Dry/Wet at 100 % is the compressed signal only, at 0 % the device is effectively bypassed. Soft Clip switches on a fixed waveshaper that tames loud transients and holds the output under −0.5 dB; it is not transparent. A needle meter shows the gain reduction in dB. The clip light turns red when the output exceeds 0 dB, and yellow with Soft Clip on to show peaks are being clipped |
| S2 | Manual | Same, section 29.21's sidechain part, and the release notes of Live 12.2 | The sidechain section is shown or hidden with a toggle in the title bar (at the left from 12.2). The external source can be any internal routing point. Gain changes the trigger level only; Dry/Wet at 100 % means the sidechain alone triggers, at 0 % it is bypassed; an EQ makes only a chosen band trigger the compression; Listen plays only the sidechain input |
| S3 | Manual | Same, the device's context menu | Oversampling processes the signal internally at twice the sample rate, which may reduce aliasing and transient harshness, at a small cost in processing; with Soft Clip on, the output may still go over 0 dB |
| S4 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter can be automated and modulated |

## Rules

- **R1.** Beatlayer ships Bond, an audio-effect device type with a sidechain input, of its own name and layout: every parameter has a descriptor and a place in the layout, except Oversampling, which is a switch listed in the device's Show Options menu as its own item and has no control. The parameters keep their values through a saved set and through the agent API. Threshold runs from −60 dB to 0 dB (0 dB at first), Ratio from 1 : 1 to 20 : 1 (4 : 1), Attack from 0.01 to 100 ms (10 ms), Release from 0.05 to 4 s (0.2 s), Range from −70 dB to 0 dB (−60 dB), all Beatlayer's own; Makeup is a gain from −∞ dB to +24 dB (0 dB at first); Dry/Wet is 100 % at first; Auto, Soft Clip, SC EQ and Oversampling are switches, off at first. (S1, S3, S4)
- **R2.** The static curve is Squeeze's gain computer (the compressor spec, R2) with a slope of 1 − 1/ratio and a knee of 12 dB / ratio (Beatlayer's own law, so the knee narrows as the ratio rises): no reduction below Threshold less half the knee, the full slope above Threshold plus half the knee, and a smooth turn between. The reduction at a steady level is the curve's, within 0.1 dB, at each ratio. (S1)
- **R3.** Attack and Release are the time constants of the reduction in dB: after one of them the gap between the reduction and its target has closed by 1 − 1/e, within 5 %, whether the reduction is rising (Attack) or falling (Release). (S1)
- **R4.** With Auto on, the reduction falls in two parts of equal share, one with a time constant of 100 ms and one with 1 s (Beatlayer's own), so a reduction that has been held falls to 64 % of its depth after 0.1 s and 18 % after 1 s, within 5 %; the rising stays Attack's. (S1)
- **R5.** Range caps the reduction at its depth: with a target deeper than Range the reduction stops at Range, within 0.1 dB, and at 0 dB no reduction is applied. (S1)
- **R6.** Makeup multiplies the signal after the compression and before Soft Clip, and reaches −∞ dB, where Bond is silent. (S1, S4)
- **R7.** With Soft Clip on, each wet sample goes through c · tanh(x / c) with c at −0.5 dB, so the output never exceeds −0.5 dBFS and a small signal passes nearly unchanged (a difference under 0.001 dB at −40 dB). The clip light, published after each block, is red (1) when the output went over 0 dB, yellow (2) while Soft Clip is on and the wet signal went above the ceiling, and off (0) otherwise. (S1)
- **R8.** Dry/Wet blends the processed signal with the unprocessed one; at 0 % the output is the input, bit for bit. (S1)
- **R9.** With Oversampling on, the gain, Makeup and Soft Clip run at twice the rate through the framework's oversampler (device framework R8), and the device reports the oversampler's latency (32 frames) from the moment it is built, plain or oversampled as the switch says; the dry signal is delayed by the same trip, so Dry/Wet stays aligned. The detector hears the input undelayed. (S3)
- **R10.** With the sidechain on, the detector hears the key the engine hands the device in place of the track's own signal, so a key from another track turns the input down; with the SC EQ on it hears the key through a filter (Beatlayer's types Low Pass, Band Pass and High Pass, Freq and Res), so only a band of it triggers the compressor. With Listen on the device's output is the key (routing R24). (S2)
- **R11.** The output is the same, bit for bit, at any block size, with or without Oversampling: the continuous parameters the detector works from are read every 16 frames counted from the device's start, and the stepped ones and Makeup and Dry/Wet on every frame. (S1)
- **R12.** The display (device framework R36) holds, with an engine publishing, the readings Gain Reduction (in dB, 0 or below) and Clip (the light), published after each block and readable headlessly with `device.display`, for the meters that follow them. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/bus_compressor.rs`, `the_bus_compressor_is_described_with_oversampling_as_a_menu_option`; `crates/shell/tests/eqs_and_dynamics.rs`, `a_set_with_the_four_devices_reopens_with_every_parameter`; `crates/cli/tests/agent.rs`, `an_agent_adds_the_eqs_and_dynamics_sets_them_by_id_and_keys_them_from_another_track`; `rtsan/tests/devices.rs`, `the_bus_compressor_passes_under_rtsan` | unit, MCP script, RTSan |
| R2 | `crates/devices/tests/bus_compressor.rs`, `the_static_curve_matches_at_each_ratio`, `the_knee_narrows_as_the_ratio_rises` | unit, real-time trap |
| R3 | `crates/devices/tests/bus_compressor.rs`, `attack_and_release_are_time_constants_of_the_reduction` | unit, real-time trap |
| R4 | `crates/devices/tests/bus_compressor.rs`, `auto_release_lets_go_in_a_fast_part_and_a_slow_one` | unit, real-time trap |
| R5 | `crates/devices/tests/bus_compressor.rs`, `range_caps_the_reduction_and_zero_applies_none` | unit, real-time trap |
| R6 | `crates/devices/tests/bus_compressor.rs`, `makeup_adds_gain_after_the_compression_down_to_silence` | unit, real-time trap |
| R7, R12 | `crates/devices/tests/bus_compressor.rs`, `soft_clip_holds_the_peak_under_minus_half_a_db_and_lights_the_clip` | unit, real-time trap |
| R8 | `crates/devices/tests/bus_compressor.rs`, `dry_wet_at_zero_nulls_against_the_input` | unit, real-time trap |
| R9 | `crates/devices/tests/bus_compressor.rs`, `oversampling_reports_the_oversamplers_latency_and_keeps_the_dry_signal_aligned` | unit, real-time trap |
| R10 | `crates/devices/tests/bus_compressor.rs`, `a_key_from_another_track_ducks_the_input_and_its_eq_hears_only_its_band`; `crates/cli/tests/agent.rs`, `an_agent_adds_the_eqs_and_dynamics_sets_them_by_id_and_keys_them_from_another_track` | unit, MCP script, real-time trap |
| R11 | `crates/devices/tests/bus_compressor.rs`, `renders_are_bit_identical_at_any_block_size` | unit, real-time trap |
| G1, G2 | `crates/devices/tests/rules.rs` over the shipped types | real-time trap |
