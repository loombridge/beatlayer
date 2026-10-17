# The reverb

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-2.18b, part 1. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); the compressor, EQ and limiter are WP-2.18a; the delay and Utility are the rest of WP-2.18b
- **Parity-matrix row:** Devices: Reverb
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.32 "Reverb", read on 15 October 2026

## The behaviour

The reverb is an audio effect that simulates a room. The input passes
through a switchable low-cut and high-cut filter, a predelay, a bank of
early reflection taps, an allpass diffusion network, and a bank of damped
combs whose feedbacks give the tail its decay time. The early reflections
have a Shape and a Spin modulation; the tail's high and low frequencies
can be damped; a Chorus section moves the tail; Freeze holds it, with Flat
and Cut deciding what that keeps; Size scales the room; the Stereo control
sets the width of the output; Density trades the tail's quality against
the device's cost; and Reflect, Diffuse and Dry/Wet set the output's
balance.

Beatlayer's own name for the device is Rotunda. It is built on the WP-2.5
framework, so its parameters, ranges, smoothing, layout, presets and
automation are the framework's, and only the device's own behaviour is
here: what each control does to the sound.

## Sources

Every source is either public documentation, paraphrased rather than
quoted, or an observation of Live running, as the clean-room policy
allows.

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.32 "Reverb" | The input signal passes first through low-cut and high-cut filters, whose X-Y controller changes the band's center frequency (X) and bandwidth (Y), with a slider for each filter and either switched off to save CPU; early reflections whose Spin (a switch, with Amount and Rate) applies modulation and whose Shape sculpts their prominence, their decay and their overlap with the diffused sound (small values decay more gradually and the diffused sound occurs sooner; large values decay rapidly and the diffused onset occurs later); a diffusion network that makes the tail, with high and low shelving filters for frequency-dependent decay (each switchable) and Diffusion and Scale controls for the diffusion's density and coarseness; a Chorus section with Amount and Rate, which can be deactivated; a global Predelay in milliseconds before the first early reflection (typical values for natural sounds, 1 to 25 ms); a Smooth chooser (None, Slow, Fast) deciding whether new delay times from a Size change are applied immediately or over a specific time period; Size, the room's volume; Decay, the time the tail takes to fall to 1/1000th (−60 dB) of its initial amplitude; Freeze, which holds the diffuse response so the reverberation sustains almost endlessly, with Flat bypassing the high and low shelves while frozen and Cut stopping the input adding to the frozen reverberation; a Stereo control of the output's width, from 0 (mono) to 120 degrees (each ear an independent channel); a Density chooser trading quality against performance (Sparse uses minimal CPU, High the richest tail); and the output's Reflect and Diffuse amplitudes and Dry/Wet balance |
| S2 | Beatlayer | [The architecture](../02-architecture.md), "Device chains", "Parameters, automation, and modulation", "Real-time rules" | A device runs as a node that takes parameter changes at their frames; no allocation, lock, or blocking call on the audio thread; every buffer is made with the node |
| S3 | Beatlayer | [The native device framework](device-framework.md) | A native device describes its parameters and layout; the engine clamps and smooths continuous values; gains reach −∞ dB (G1) and every parameter is automatable (G2); the generic UI draws any described device |

## Rules

- **R1.** The input signal passes through a low-cut filter (a high-pass)
  and a high-cut filter (a low-pass), each with its own switch and
  frequency; with a switch off, that filter doesn't shape the signal. A
  50 Hz tone is heard with Low Cut off and taken out with Low Cut at
  500 Hz; the same for a 10 kHz tone and High Cut at 1 kHz. (S1)
- **R2.** The early reflections are short taps of the predelayed input,
  scaled by Size. Shape sets how quickly they decay and how soon the
  diffusion follows: with a small Shape the later reflections decay
  gradually and the diffused onset comes sooner; with a large Shape they
  decay rapidly and the onset later. (S1)
- **R3.** Spin modulates the early reflections. Spin Amount sets how far
  its LFO moves them, Spin Rate how fast; with Spin off, or its amount at
  zero, the reflections don't move. (S1)
- **R4.** The diffusion network is a chain of allpass stages; Diffusion
  sets how much they diffuse, and Scale the coarseness of their delays
  (small is finer). (S1)
- **R5.** Each comb's feedback runs through damping that makes a
  frequency-dependent decay, switchable and set by amount: High damps the
  tail's high frequencies (as air and soft furnishings do) and Low its low
  frequencies, for a thinner decay. (S1)
- **R6.** Chorus adds a modulation of Amount and Rate to the diffusion's
  delay lines; with Chorus off, or its amount at zero, the tail is still.
  (S1)
- **R7.** Predelay delays the onset of the first early reflection, in
  milliseconds. (S1)
- **R8.** Size scales the delay lines, so a larger size moves the early
  reflections and the tail later; Smooth decides how a change of Size
  reaches them: None at once, Fast or Slow over a short or long glide.
  (S1)
- **R9.** Decay is the time the tail takes to fall by 60 dB (RT60). Each
  comb's feedback is worked out from Decay and its own delay time, so the
  sum of the combs decays at the setting. (S1)
- **R10.** Freeze holds the diffuse response: the combs' feedbacks reach
  1, so the tail sustains without decaying and without growing. Flat
  bypasses the damping shelves while frozen, so with High or Low damping
  the frozen tail would otherwise lose energy in their bands. Cut stops
  the input adding to the frozen reverberation; with Cut off, new input
  still reaches it. (S1)
- **R11.** Stereo sets the width of the output: at 0 the two channels are
  one (mono), at 120 each channel keeps its own network, and in between
  the side signal is scaled between the two. (S1)
- **R12.** Density chooses how many combs run: Sparse the fewest, High all
  of them, for a richer tail at a higher cost. (S1)
- **R13.** Reflect sets the amplitude of the early reflections, Diffuse
  that of the diffuse tail: at 0 the section is silent, and the two mix
  before the width. (S1)
- **R14.** Dry/Wet sets the balance between the untouched signal and the
  reverberated one: at 0 the output is the input, at 100 it is the
  reverb alone. (S1)
- **R15.** Every parameter is automatable and nothing the device does on
  the audio thread allocates, locks, or blocks: every line, filter and
  LFO is made with the device. Its impulse response is finite, bounded,
  and flushes to exact silence with no denormals left when the tail has
  gone. (S2, S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/reverb.rs`, `the_input_filter_shapes_the_input` | unit, real-time trap |
| R2 | `crates/devices/tests/reverb.rs`, `shape_sets_how_the_early_reflections_decay` | unit, real-time trap |
| R3 | `crates/devices/tests/reverb.rs`, `spin_moves_the_early_reflections` | unit, real-time trap |
| R4 | `crates/devices/tests/reverb.rs`, `diffusion_and_scale_shape_the_tail` | unit, real-time trap |
| R5 | `crates/devices/tests/reverb.rs`, `the_damping_shelves_thin_the_tail` | unit, real-time trap |
| R6 | `crates/devices/tests/reverb.rs`, `chorus_moves_the_tail` | unit, real-time trap |
| R7 | `crates/devices/tests/reverb.rs`, `predelay_delays_the_first_reflection` | unit, real-time trap |
| R8 | `crates/devices/tests/reverb.rs`, `size_moves_the_tail_and_smooth_glides_a_size_change` | unit, real-time trap |
| R9 | `crates/devices/tests/reverb.rs`, `decay_time_matches_the_setting_at_three_settings`, `decay_time_holds_at_full_size` | unit, real-time trap |
| R10 | `crates/devices/tests/reverb.rs`, `freeze_holds_the_tail_without_growing`, `flat_and_cut_decide_what_a_freeze_keeps` | unit, real-time trap |
| R11 | `crates/devices/tests/reverb.rs`, `zero_stereo_is_mono_and_full_stereo_keeps_the_channels_apart` | unit, real-time trap |
| R12 | `crates/devices/tests/reverb.rs`, `density_chooses_how_many_combs_run` | unit, real-time trap |
| R13 | `crates/devices/tests/reverb.rs`, `reflect_and_diffuse_route_the_early_reflections_and_the_tail` | unit, real-time trap |
| R14 | `crates/devices/tests/reverb.rs`, `dry_wet_balances_the_dry_and_the_wet` | unit, real-time trap |
| R15 | `crates/devices/tests/reverb.rs`, `the_impulse_response_is_finite_and_ends_in_silence`, and every test in the file runs its blocks as real-time sections under the allocation trap; `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame` (rule G2; the device has no gain, so G1 holds trivially) | unit, real-time trap |
| the layout | `crates/devices/tests/reverb.rs`, `the_layout_places_every_parameter_once` | unit |

## Beatlayer's own choices

- **The algorithm is Beatlayer's own.** A predelay line feeds four early
  reflection taps and a chain of four Schroeder allpass stages, which feed
  a bank of combs (eight at Normal, twelve at High). Each comb's feedback
  is `10^(−3·T/decay)` for its own delay time `T`, so the set decays by
  60 dB in Decay seconds. Fractional delay reads use a first-order allpass
  (Jaffe and Smith's method, published), so a feedback loop keeps its
  energy exactly: Decay is what it says, and Freeze neither decays nor
  grows.
- **The X-Y controller is not built.** The manual's X-Y controller moves
  the band's center frequency and bandwidth — the same band the two
  filter sliders set — and Beatlayer's generic layout has no X-Y pad. The
  device keeps the two sliders the manual also names (Low Cut Frequency,
  High Cut Frequency) and their switches.
- **The damping shelves are one-poles.** Beatlayer mixes a low-passed copy
  of each comb's feedback into it by High's amount (turnover 4 kHz) and
  subtracts a low-passed copy by Low's amount (turnover 500 Hz).
- **Density is how many combs run**: 4 at Sparse, 8 at Normal, 12 at
  High. Smooth's times are Beatlayer's: Fast 50 ms, Slow one second.
- **Freeze's Cut defaults on.** A new device starts with Cut on, so Freeze
  alone holds what is there.
- **Stereo is a mid/side scaling of the output**, so at 0 the early
  reflections and the tail are mono together, as the manual's lowest
  setting is.
- **The device ships as Rotunda**, Beatlayer's own name.

## Defaults and ranges

A new Reverb starts with Low Cut and High Cut off (100 Hz and 10 kHz),
Spin off (20 %, 1 Hz), Shape 50 %, High on at 50 % and Low off at 50 %,
Diffusion and Scale 50 %, Chorus off (20 %, 0.5 Hz), Predelay 10 ms,
Smooth None, Size 50 %, Decay 2 s, Freeze off, Flat off, Cut on,
Stereo 120, Density Normal, Reflect and Diffuse and Dry/Wet 100 %.

The ranges are Low Cut Frequency 20 to 500 Hz, High Cut Frequency 1 to
20 kHz, Spin Amount 0 to 100 % and Spin Rate 0.01 to 10 Hz, Shape 0 to
100 %, High and Low 0 to 100 %, Diffusion and Scale 0 to 100 %, Chorus
Amount 0 to 100 % and Rate 0.01 to 10 Hz, Predelay 0 to 250 ms, Size 0
to 100 %, Decay 0.1 to 60 s, Stereo 0 to 120, and Reflect, Diffuse and
Dry/Wet 0 to 100 %.

Spin's modulation starts the right channel's LFO a third of a cycle on
from the left's. Spin and Chorus reach 3 ms and 0.5 ms at 100 %.

