# Utility

- **Status:** implemented
- **Work packages:** WP-2.18b (part of); built on WP-2.5's native device framework
- **Parity-matrix row:** Devices: device parameters, device controls
- **Checked against:** the Live 12 reference manual (see Sources)

## The behaviour

Utility is an audio effect that reshapes a track's signal without adding colour: it inverts either input channel's phase, picks which input channels reach the outputs, sets the stereo width, sums to mono, makes the low side signal mono, sets the gain and balance, filters out DC, and mutes. A Live user finds the same parameters as Live's Utility; Beatlayer draws them with the framework's generic layout (its own look).

Beatlayer applies the stages in this order: DC, phase, channel mode, width, bass mono, mono, balance, gain, mute. Balance is drawn as a plain number, not as a pan position. Width has no Mid/Side mode and Bass Mono has no Audition switch: the generic UI's controls have no context menu yet, and Audition is a monitoring-only switch.

This spec covers Utility's parameters and what each does to the audio. The framework it is built on — descriptors, ranges, smoothing, the audio-thread rules, and the generic UI — is the [device framework](device-framework.md); the device's placement in the Device View and its floating window are the [device chains](device-chains.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.40 "Utility" (the Live 11 manual's section 24.43 reads the same) | Two Phase controls, one per input channel, each inverting that channel; a Channel Mode chooser that selects which input channels are processed, where choosing Left ignores the right channel and puts the left channel on both outputs; a Width control whose 0 % is a mono signal and whose values above 100 % widen; a Mono switch that converts a stereo input to mono; a Bass Mono switch that makes the low frequencies mono with a Bass Mono Frequency slider of 50–500 Hz; a Gain control from −∞ dB to +35 dB; a Balance control that pans; a Mute button; and a DC switch that filters out DC offsets and frequencies far below the audible range |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter a user can set can be automated and modulated |
| S3 | Beatlayer | The [device framework](device-framework.md), "The behaviour" and "Rules" | A native device reads its parameters from a bank, already clamped and smoothed frame by frame; nothing on the audio thread allocates, locks, or blocks |

## Rules

- **R1.** Utility is a native audio effect whose parameters are Phase Left and Phase Right (switches, off by default), Channel Mode (a chooser of Stereo, Left, Right and Swap, Stereo by default), Mono, Bass Mono, Bass Mono Frequency (50–500 Hz, logarithmic, 120 Hz by default), Width (0–200 %, 100 % by default), Balance (−50 to 50, 0 by default), DC, Gain (−∞ dB to +35 dB, 0 dB by default) and Mute. Every parameter is automatable (G2), and the gain's bottom is −∞ dB (G1). (S1, S2)
- **R2.** Phase Left inverts the left channel's polarity and leaves the right's; Phase Right the other way. Both together invert both. (S1)
- **R3.** Channel Mode Stereo processes both channels. Left ignores the right channel and puts the left on both outputs; Right does the other way; Swap exchanges the two. Width and Bass Mono have no function in Left, Right or Swap. (S1)
- **R4.** Width sets the stereo width of the mid/side form: 0 % outputs the mid signal alone, so the result is mono; 100 % leaves the signal unchanged; 200 % doubles the side signal against the mid. (S1)
- **R5.** Mono sums the two channels into one and puts it on both outputs. (S1)
- **R6.** Bass Mono removes the side signal below Bass Mono Frequency, leaving the side above it untouched; the mid signal is unchanged. (S1)
- **R7.** Gain scales both channels by a linear amplitude factor, from silence at −∞ dB to +35 dB. (S1, S2)
- **R8.** Balance pans the signal between the channels at full attenuation at either end, and Mute silences both outputs. (S1)
- **R9.** DC filters out a DC offset and frequencies far below the audible range (Beatlayer's first-order high-pass at 5 Hz), and does nothing else. (S1)
- **R10.** Utility runs entirely on the audio thread under the framework's rules: each parameter is clamped, continuous values and gains glide over 20 ms, and nothing allocates, locks, or blocks. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/utility.rs`, `every_parameter_is_automatable_and_the_gain_reaches_silence` | unit |
| R2 | `crates/devices/tests/utility.rs`, `each_phase_switch_inverts_only_its_channel` | unit |
| R3 | `crates/devices/tests/utility.rs`, `the_channel_modes_pick_the_input_channels` | unit |
| R4 | `crates/devices/tests/utility.rs`, `width_zero_is_mono_and_two_hundred_doubles_the_side` | unit |
| R5 | `crates/devices/tests/utility.rs`, `the_mono_switch_converts_the_input_to_mono` | unit |
| R6 | `crates/devices/tests/utility.rs`, `bass_mono_removes_only_the_side_below_its_frequency` | unit |
| R7 | `crates/devices/tests/utility.rs`, `the_gain_scales_by_db_to_the_hundredth_and_reaches_silence` | unit |
| R8 | `crates/devices/tests/utility.rs`, `balance_pans_and_mute_silences` | unit |
| R9 | `crates/devices/tests/utility.rs`, `the_dc_filter_removes_a_dc_offset` | unit |
| R10 | every block in `crates/devices/tests/utility.rs` runs as a real-time section under the allocation trap | real-time trap |
