# Pitch Hack: a pitch-shifting delay line

- **Status:** draft
- **Work packages:** WP-8.51, with [Gated Delay](gated-delay.md) (all of it: R1 to R11). Delay-line pitch shifting is [the pitch and frequency shifter](pitch-and-frequency-shifter.md) R3's, the same code; the framework is WP-2.5's ([the native device framework](device-framework.md)), its seeded random source WP-5.42's
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Creative Extensions pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Pitch Hack is a delay whose echoes come out transposed. Each echo can be reversed, the transposition can be chosen at random from echo to echo, and feedback folds the transposed echoes back into the line, so they climb or fall further each time.

Live's Pitch Hack comes in its Creative Extensions pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.pitch_hack`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Pitch Hack, a real-time pitch-shifting delay line: transposition in semitones and cents, reversing the audio, randomising the transposition, and folding the delayed signal back into the line |
| S2 | Beatlayer | [The pitch and frequency shifter](pitch-and-frequency-shifter.md) R3; [the delay](delay.md) R1; [the device framework](device-framework.md) R4, R29; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | Delay-line pitch shifting with two crossfaded heads; synced times; smoothing, seeds; banks; gains and automation; a delay's input reaching silence |

## Rules

- **R1.** Pitch Hack (`beatlayer.pitch_hack`) is an audio effect with Live's name, Beatlayer's own layout and no display cell. (S1; the layout is Beatlayer's own)
- **R2.** The delay's time is, with Sync on (the default), a division from 1/32 note to 1 bar with triplet and dotted forms (1/8 by default), landing on its frame through the tempo map, or with Sync off a time from 10 ms to 2 s (250 ms). The input enters through Input, a gain from −∞ to +12 dB (0 dB, rule G1 and IMP-08). (S1, S2; the ranges are Beatlayer's own)
- **R3.** Each echo leaves the line transposed by Transpose (−24 to +24 semitones, +7) plus Cents (−50 to +50), through the shifter's delay-line pitch shifting ([the pitch and frequency shifter](pitch-and-frequency-shifter.md) R3, the same code) with a 30 ms window, so a steady tone's echo is at the transposition within 5 cents. The line is the shifter's own channel history, so the echo is read at the delay time itself and the shifting adds no latency of its own. (S1, S2; the window is Beatlayer's own)
- **R4.** Reverse, 0 to 100 % (0), is the chance each echo plays its window of the line backwards, drawn per echo from the device's seed; at 100 % every echo is reversed. Echoes are counted from the song's start, one every delay time (on the song's grid with Sync on), and an echo's draws are a function of the seed and its count, so a render repeats and the same place in the song draws the same echoes from any start. A reversed echo reads the line backwards at the transposition's rate, from the newest input back, and fades in and out over 5 ms through the forward echo, so a reversed echo with no transposition is the delay time's worth of input before it, backwards. (S1; the chance law and the reading are Beatlayer's own)
- **R5.** Random, 0 to 100 % (0), is the chance each echo takes, instead of Transpose, an interval drawn at random from the set −12, −7, −5, 0, +5, +7, +12 semitones, from the device's seed. (S1; the set and the law are Beatlayer's own)
- **R6.** Feedback, 0 to 100 % (30 %), returns each transposed echo to the line's input, so the next echo is transposed again; the loop's gain stays below 1: Feedback at 100 % is a gain of 0.98. (S1; the ceiling is Beatlayer's own)
- **R7.** Dry/Wet, 0 to 100 % (50 %), balances the dry signal against the echoes. The device reports no latency. (S1; Beatlayer's own)
- **R8.** The line is made with the node for the longest time at the slowest tempo; nothing on the audio thread allocates, locks or blocks; a render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S2)
- **R9.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R10.** Pitch Hack declares one bank of eight ([control surfaces](control-surfaces.md) R19): Time, Sync, Transpose, Cents, Reverse, Random, Feedback, Dry/Wet, with Division and Input in a second, so the free Time and the synced Division both have a place. (S2; Beatlayer's own)
- **R11.** A set with the device saves and reopens with every parameter. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | `crates/devices/tests/pitch_hack.rs`, `pitch_hack_is_described_with_every_parameter_in_its_layout_and_no_latency`, `an_echo_lands_one_delay_time_after_its_input_free_and_synced`, `input_reaches_silence_and_gains_up_to_twelve_decibels` | unit |
| R3 | `crates/devices/tests/pitch_hack.rs`, `a_steady_tones_echo_is_at_the_transposition_within_five_cents` | unit |
| R4 | `crates/devices/tests/pitch_hack.rs`, `reverse_plays_each_echo_backwards_and_at_nothing_none` | unit |
| R4, R5 | `crates/devices/tests/pitch_hack.rs`, `random_gives_each_echo_an_interval_from_the_set_by_the_seed_run_after_run` | unit |
| R6 | `crates/devices/tests/pitch_hack.rs`, `feedback_transposes_each_pass_again_and_stays_below_one` | unit |
| R7 | `crates/devices/tests/pitch_hack.rs`, `dry_wet_balances_the_dry_signal_against_the_echoes`, `pitch_hack_is_described_with_every_parameter_in_its_layout_and_no_latency` | unit |
| R8 | `crates/devices/tests/pitch_hack.rs`, `renders_are_bit_identical_at_four_block_sizes_and_run_after_run`; `rtsan/tests/devices.rs`, `gated_delay_and_pitch_hack_pass_under_rtsan` | unit, real-time trap, RealtimeSanitizer |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_is_modulatable_to_the_frame`, `every_gain_parameter_reaches_minus_infinity`; `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_gated_delay_and_pitch_hack_and_reopens_them` | unit, agent API |
| R10 | `crates/devices/tests/banks.rs`, `every_native_device_types_banks_hold_every_parameter_and_none_more_than_eight` | unit |
| R11 | `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_gated_delay_and_pitch_hack_and_reopens_them` | agent API |
