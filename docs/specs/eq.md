# The parametric EQ

- **Status:** partial (every rule below has a passing test; the Scale field, Audition, and the dots' gestures are not built yet)
- **Work packages:** WP-2.18a, part 1; WP-5.43 (the display, R12 to R14). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); the compressor is WP-2.18a, part 2, and the limiter is part 3; the reverb, delay, and Utility are WP-2.18b
- **Parity-matrix row:** Devices: EQ Eight
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.15 "EQ Eight", read on 15 October 2026

## The behaviour

The EQ is an audio effect with up to eight parametric filters on each of two
curves. Each band is one of six responses — a low cut, a low shelf, a peak, a
notch, a high shelf, or a high cut — with the cuts at 12 or 48 dB per octave,
and each band has a frequency, a gain where its response has one, and a Q. A
band can be switched off on its own. Mode decides how the curves map onto the
channels: Stereo filters both channels with the first curve, L/R gives the
left and right channels a curve each, and M/S encodes the stereo signal to
mid/side and gives mid and side a curve each. Adaptive Q raises a boosting or
cutting band's Q as the size of the boost or cut grows, and Oversample runs
the filters at twice the sample rate. Analyze puts the output's spectrum in
the display, behind the curves.

Beatlayer's own name for the device is Contour; its look is the device
framework's descriptors and layout, and its behaviour, parameters, ranges,
defaults, and units come from the manual's EQ Eight section. This spec covers
the device only; where it sits in a chain and the frame around it are [the
device chains spec](device-chains.md) and [the device framework
spec](device-framework.md).

## Sources

Every source is either public documentation, paraphrased rather than quoted,
or an observation of Live running, as the clean-room policy allows.

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.15 "EQ Eight", and section 29.15.0.1 "Context Menu Options for EQ Eight" | Up to eight parametric filters per input channel; the modes Stereo, L/R, and M/S (M/S for signals recorded in mid/side), with both curves drawn together and one edited at a time through an Edit switch; the output's spectrum behind the curves while Analyze is on (on by default); the responses, top to bottom in the chooser, 48 or 12 dB/octave Low cut, Low shelf, Peak, Notch, High shelf, and 12 or 48 dB/octave High cut; each band switched on and off on its own; the gain not adjustable on the low cut, notch, and high cut, where vertical dragging moves the Q instead; Adaptive Q raising the Q as a boost or cut grows, after the classic analogue EQs; Audition for hearing one band alone; a global Gain slider for the output level; a Scale field scaling the gain of every gain-bearing band; Oversampling as a context-menu option that processes at twice the sample rate for smoother high frequencies, with a little more CPU |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter can be automated and modulated |
| S3 | Beatlayer | [The architecture](../02-architecture.md), "Device chains", "Parameters, automation, and modulation", "Real-time rules" | A device runs as a node that takes parameter changes at their frames; no allocation, lock, or blocking call on the audio thread; every buffer is made with the node |
| S4 | Beatlayer | [The native device framework](device-framework.md) | A native device describes its parameters and layout; the engine clamps and smooths continuous values; gains reach −∞ dB (G1) and every parameter is automatable (G2); the oversampler runs a stage at 2, 4, or 8 times the rate and reports its latency; the generic UI draws any described device |

## Rules

- **R1.** Beatlayer ships Contour, an audio-effect device with eight bands on each of two curves: each band's switch, type, frequency, gain, and Q, and the mode, the switches, and the gain, are described, laid out, and reachable as any native device's. The frequency runs from 20 Hz to 20 kHz (logarithmic), the gain is ±15 dB, and the Q runs from 0.1 to 18 (logarithmic). A new device is in Stereo mode with Adaptive Q off, Oversample off, and Analyze on; every band is switched off, so the device starts transparent; band 1 is a low cut, band 8 a high cut, and the rest are peaks, at frequencies an octave apart from 100 Hz to 12.8 kHz, each with a gain of 0 dB and a Q of 0.71. (S1, S4)
- **R2.** The analyser measures the device's output, a window of samples at a time, as the level of each of 64 logarithmic bands from 20 Hz to 20 kHz in dB, and a caller reads its last window without a window of its own. A full-scale sine reads 0 dB in its band. Each window is a 1024-point FFT with a Hann window, about six and a half bands to an octave; a band's level is its peak bin, and a band the window's bins don't reach reads its nearest bin. (S1)
- **R3.** A band's response is one of the eight the chooser offers, and its magnitude response is that response's analytic curve, within 0.1 dB: the low cut and high cut as second-order high-pass and low-pass sections, four in cascade for the 48 dB/octave pair (12 and 48 dB/octave slopes), the shelves and the peak with their gain, and the notch from the cookbook's bilinear designs. The designs are the standard bilinear (RBJ cookbook) sections, written in the device. A shelf's Q is the cookbook's `alpha` in place of its slope `S`, so a higher Q over- and undershoots around the corner as an analogue shelf does. Every stage of a cascaded cut gets the band's Q. (S1)
- **R4.** Mode maps the curves onto the channels: Stereo filters both channels with the first curve (the second curve's controls are not read), L/R gives the left channel the first curve and the right the second, and M/S encodes (mid, side) from (left, right) and gives the mid the first curve and the side the second, decoding back. (S1)
- **R5.** With Adaptive Q on, a gain-bearing band's Q rises as the size of its boost or cut grows — to twice its setting at a full 15 dB boost or cut, linearly in the gain's size, capped at the Q control's top. A band without a gain (the cuts and the notch), and any band with no boost or cut, is unchanged. (S1, S2)
- **R6.** A band's Gain boosts or cuts its frequencies, reaching ±15 dB; a shelf's response levels off at the gain, on its own side. (S1)
- **R7.** In M/S mode, a curve moves its own domain and leaves the other untouched: a pure side signal passes the mid curve's chain without change, and a pure mid signal passes the side curve's chain. (S1)
- **R8.** Oversample runs the bands at twice the sample rate, through the framework's oversampler, and the device reports that trip's latency for delay compensation (WP-2.11; the plain path reports none). A band's response is the same through either path, within the oversampler's own ripple. Turning Oversample on or off clears the oversamplers' histories, so no stale sample crosses the change. (S1, S4)
- **R9.** Analyze switches the analyser: while it is on the analyser measures the output, and while it is off it measures nothing and the audio passes the filters as ever. (S1, R2)
- **R10.** Gain scales the output, after the filters, as a gain that reaches −∞ dB (rule G1) and is automatable like every parameter (rule G2). (S2)
- **R11.** Nothing the device does on the audio thread allocates, locks, or blocks: each channel's band states, the oversamplers, and the analyser's window, FFT buffers, and twiddles are made with the device. (S3, S4)
- **R12.** Contour publishes the analyser's last window, its 64 bands in dB, after each block, through the framework's display data (the device framework spec, R35), whatever the block's size. (S3, S4)
- **R13.** Contour's display (device framework R36) holds, while Analyze is on and an engine publishes, the spectrum: the 64 bands over 20 Hz to 20 kHz, each at its centre. It holds, for each curve in use (one in Stereo mode, left and right in L/R, mid and side in M/S), the sum of its switched-on bands' responses in dB at 96 places along 20 Hz to 20 kHz, logarithmically, and each such band's own response; and a marker for each switched-on band at its frequency and its gain (0 dB for a band that has none), reading as its name, type, frequency and gain. The responses are the bands' analytic curves (R3, with Adaptive Q's rise, R5), worked out at 48 kHz. (S1)
- **R14.** The device's display cell draws the spectrum behind the curves while Analyze is on, each curve in the accent with the bands' own responses faint, and a dot for each marker, and says them to a screen reader. The dots' gestures and Audition are not built. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/eq.rs`, `the_eq_is_described_and_laid_out` | unit, real-time trap |
| R2 | `crates/devices/src/eq.rs`, `the_analyser_tables_a_full_scale_sine_near_zero_db`; `crates/devices/tests/eq.rs`, `the_analyser_reports_the_outputs_spectrum` | unit, real-time trap |
| R3 | `crates/devices/tests/eq.rs`, `every_filter_type_matches_its_analytic_curve`, `the_cut_filters_slope_as_their_names_say` | unit, real-time trap |
| R4 | `crates/devices/tests/eq.rs`, `stereo_mode_gives_both_channels_the_first_curve`, `left_right_mode_gives_each_channel_its_own_curve` | unit, real-time trap |
| R5 | `crates/devices/src/eq.rs`, `adaptive_q_only_moves_the_gain_bearing_types`; `crates/devices/tests/eq.rs`, `adaptive_q_tightens_the_band_as_the_gain_rises` | unit, real-time trap |
| R6 | `crates/devices/tests/eq.rs`, `a_shelfs_boost_and_cut_meet_their_plateaus` | unit, real-time trap |
| R7 | `crates/devices/tests/eq.rs`, `mid_side_mode_leaves_the_other_curve_untouched` | unit, real-time trap |
| R8 | `crates/devices/tests/eq.rs`, `the_oversampled_path_passes_its_bands_and_reports_its_latency` | unit, real-time trap |
| R9 | `crates/devices/tests/eq.rs`, `the_analyser_reports_the_outputs_spectrum` | unit, real-time trap |
| R10 | `crates/devices/tests/eq.rs`, `the_output_gain_scales_the_finished_signal`; `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity` | unit, real-time trap |
| R11 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; every test in `crates/devices/tests/eq.rs` runs its blocks as real-time sections under the allocation trap | unit, real-time trap |
| R12 | `crates/shell/tests/device_displays.rs`, `the_contours_spectrum_and_the_squeezes_gain_reduction_are_published_after_every_block`; `rtsan/tests/device_displays.rs`, `displays_publish_after_every_block_under_rtsan` | unit, real-time trap, RTSan |
| R13 | `crates/shell/tests/device_displays.rs`, `an_agent_reads_a_display_from_its_parameters_and_from_the_newest_data`; `crates/cli/tests/agent.rs`, `an_agent_reads_a_contours_spectrum_and_a_squeezes_gain_reduction_by_device_id` | unit, MCP script |
| R14 | `crates/ui/tests/device_panel.rs`, `the_contours_cell_draws_its_curve_dots_and_spectrum_and_says_them`, `a_floating_window_grows_the_display_and_draws_it_there_too` | UI script |

## Not built yet

- The Scale field, which scales the gain of every gain-bearing band.
- Audition, which plays one band alone while its dot is held; it is a display gesture.
- The dots' gestures: dragging a dot to set its band's frequency and gain (and Q, where the band has no gain), switching a band on by clicking in the display, and the band editing the Edit switch chooses between the curves.
