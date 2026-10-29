# The delay device

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-2.18b, part 2. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); the compressor, EQ and limiter are WP-2.18a; the reverb and Utility are the rest of WP-2.18b
- **Parity-matrix row:** Devices: Delay
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.11 "Delay", read on 14 October 2026

## The behaviour

The delay is an audio effect with two independent delay lines, one for each
channel. Each channel's delay time is either a beat division (Sync) or a
free time in milliseconds, the left channel's time can be linked to the
right's, and each channel can be offset within its division. A band-pass
filter can shape each repeat, one LFO can move the delay time and the
filter's frequency, the loops can be frozen, the repeats can bounce between
the channels (Ping Pong), and a dry/wet control sets the balance. When the
delay time changes while audio is playing, the change is handled as a
Repitch (a glide, with a pitch change), a Fade (a crossfade between the old
and new times), or a Jump (a cut to the new time).

The device takes Live's name, Delay; its look is the device
framework's generic controls and one display cell, and its parameters,
ranges, defaults and units come from the manual's Delay section. The tempo
it syncs to is the song's tempo map ([the musical time spec](musical-time.md)).
This spec covers the device only; where a delay sits in a chain, and the
device frame around it, are [the device chains spec](device-chains.md) and
[the device framework spec](device-framework.md).

## Sources

Every source is either public documentation, paraphrased rather than quoted,
or an observation of Live running, as the clean-room policy allows.

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.11 "Delay" | Two independent delay lines, one per channel; the delay time as tempo-synced sixteenths (Sync) or milliseconds (Time); one to sixteen sixteenths, with an Offset slider per channel that can shorten or lengthen the division; Time from 1.00 ms to 5.00 s; Stereo Link ties the delay settings of the two channels, while Offset stays per channel; Feedback feeds each channel's output back into its own line, the loops independent; Freeze cycles the buffer and ignores new input; a band-pass filter, switched by the Filter toggle, with a center frequency and a bandwidth; an LFO with separate amounts for the delay time and the filter frequency, a Rate, a Wave and a Morph; three smoothing modes, Repitch (a pitch variation like a tape delay, on by default), Fade (a crossfade) and Jump (a cut); Ping Pong moves the delayed signal back and forth between the channels; Dry/Wet balances the dry and the delayed signal |
| S2 | Manual | Live 12 reference manual, section 29.11.1 "Context Menu Options for Delay" | Two optional modes: Hi-Quality, a higher-quality delay-line interpolation with tape-style smoothing in Repitch, and Dry/Wet Equal-Loudness, which keeps the perceived loudness steady across the dry/wet control |
| S3 | Beatlayer | [The architecture](../02-architecture.md), "Device chains", "Parameters, automation, and modulation", "Real-time rules" | A device runs as a node that takes parameter changes at their frames; no allocation, lock, or blocking call on the audio thread; every buffer is made with the node |
| S4 | Beatlayer | [The native device framework](device-framework.md) | A native device describes its parameters and layout; the engine clamps and smooths continuous values; gains reach −∞ dB (G1) and every parameter is automatable (G2); the generic UI draws any described device |
| S5 | Beatlayer | [The musical time spec](musical-time.md) | The tempo map places ticks on frames and counts quarter notes per minute whatever the time signature |

## Rules

- **R1.** In Sync the delay time is a whole number of sixteenth notes from one
  to sixteen, per channel, and the tempo map turns it into frames at the
  stretch's tempo; in Time it is from 1.00 ms to 5.00 s, per channel. A
  repeat of a signal lands on the exact frame the division names at the
  current tempo, at any tempo, and it follows a tempo change: changing the
  tempo from 120 to 60 BPM moves a one-sixteenth repeat from 6,000 to 12,000
  frames at 48 kHz. The delay lines hold sixteen sixteenths with a +50% Offset
  at 20 BPM, the slowest tempo the transport allows. (S1, S5)
- **R2.** Link gives both channels the left channel's delay time; with Link
  off each channel uses its own. Each channel's Offset stays its own even
  when Link is on, and moves its repeat earlier or later within the
  division, by a percentage of the division. (S1)
- **R3.** Feedback, from 0 to 100 per cent, feeds each channel's output back
  into its own delay line; with Ping Pong off the two loops are separate.
  Freeze cycles what is in the lines and ignores new input. A hundred per
  cent feedback with Freeze sustains a signal without decay and without
  growth. (S1)
- **R4.** Ping Pong sums both input channels into the left line, so the
  first repeat is on the left only, the next on the right only, and so on,
  every delay time; nothing lands on both channels at once. The left line's
  output crosses into the right line at full level and the right line's
  output returns to the left line through Feedback, so each left and right
  pair is level and the pairs fall by the Feedback (measured in Live 12.4,
  25 October 2026). (S1)
- **R5.** The Filter toggle puts a band-pass filter in each delay loop, with
  a center frequency (Freq) and a bandwidth (Width); with it on, a repeat
  holds the band around the center frequency and not the rest. (S1)
- **R6.** Time Mode decides what a change of delay time sounds like. Repitch
  moves the read position to the new time over a short transition, so the
  pitch glides; Fade crossfades between the old and the new read positions,
  so the repeat arrives part-way through the crossfade; Jump cuts to the new
  time at once. (S1)
- **R7.** Dry/Wet sets the balance between the untouched signal and the
  delayed signal, from fully dry to fully wet. (S1)
- **R8.** One LFO moves the delay time and the filter's frequency. Its Rate
  runs from 0.01 Hz to 40 Hz, its Wave is a sine, a triangle, a ramp up, a
  ramp down, a square, a sample-and-hold, or a wander, its Morph shapes the
  chosen wave toward the next, and the Delay and Filter amounts set how far
  it moves each target. (S1)
- **R9.** Every parameter is automatable and nothing it does on the audio
  thread allocates, locks, or blocks: the delay lines, the filter state, and
  the LFO state are made with the device. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/delay.rs`, `a_synced_repeat_lands_on_its_frame`, `a_synced_repeat_follows_a_tempo_change` | unit, real-time trap |
| R2 | `crates/devices/tests/delay.rs`, `link_gives_both_channels_the_left_time_and_offsets_stay_apart` | unit, real-time trap |
| R3 | `crates/devices/tests/delay.rs`, `a_hundred_per_cent_feedback_with_freeze_sustains` | unit, real-time trap |
| R4 | `crates/devices/tests/delay.rs`, `ping_pong_moves_the_repeats_between_the_channels` | unit, real-time trap |
| R5 | `crates/devices/tests/delay.rs`, `the_filter_shapes_the_repeats` | unit, real-time trap |
| R6 | `crates/devices/tests/delay.rs`, `the_time_change_modes_glide_crossfade_and_cut` | unit, real-time trap |
| R7 | `crates/devices/tests/delay.rs`, `dry_wet_balances_the_dry_signal_and_the_repeat` | unit, real-time trap |
| R8 | `crates/devices/src/delay.rs`, `every_lfo_wave_stays_between_minus_one_and_one` | unit |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_gain_parameter_reaches_minus_infinity`; every test in `crates/devices/tests/delay.rs` runs its blocks as real-time sections under the allocation trap | unit, real-time trap |

## Defaults, ranges and details

- A new Delay starts with Sync on, Link off, Filter off, Freeze off, Ping Pong off, a delay time of one beat (four sixteenths) in Sync and 100 ms in Time, 50 per cent Feedback, 100 per cent Dry/Wet, a Freq of 1 kHz at a Width of Q 2.5, and an LFO of a 1 Hz sine with no movement.
- Freq runs from 20 Hz to 20 kHz, logarithmically. Width runs from 0.5 to 9 and is the band-pass's Q, so a wider setting passes a wider band. Offset runs from −50 to +50 per cent of the division. The LFO's Rate runs from 0.01 to 40 Hz, and its Delay and Filter amounts from 0 to 100 per cent. The sync buttons run from 1 to 16 sixteenths.
- The LFO is the effects LFO (the device framework spec, R26 and R27), with the free-rate mode only (Rate, in Hz); its Time, Synced, Triplet, Dotted, and Sixteenth rate modes are not built. Its S&H and Wander waves follow the device's own random seed (R29), and Wander is a line between random values.
- Freeze cycles the buffer at the Feedback setting. Ping Pong crosses the feedback between the channels.
- The band-pass filters the signal entering each line, feedback included, so the repeats thin out as they decay.
- The framework does not smooth the delay time: the device reads the time parameters' targets and moves between them itself, over 20 ms in Repitch and Fade. Other continuous parameters take the framework's smoothing as every device does.
- The device ships as Delay, drawn from its descriptors and one display cell.
