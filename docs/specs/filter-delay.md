# The filter delay device

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.25. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); its timing is the [delay](delay.md)'s
- **Parity-matrix row:** Audio effects: Filter Delay
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.19 "Filter Delay", read on 18 October 2026

## The behaviour

The filter delay is an audio effect with three delay lines. The first hears the left input, the third the right input, and the second both. Each line has an On switch and a filter in its loop: a high-pass and a low-pass that move together as one band, set by the band's centre frequency and its width (the two axes of an X–Y display, whose parameters are ordinary controls here). Each line's time is either a beat division (Sync), optionally shortened or lengthened by a percentage for a swing, or a free time in milliseconds. One Feedback sends every line's output back into its own input through its band. Each line has a Pan, which by default leaves it on the side it hears, and a Volume that can be raised to 6 dB; one Dry sets the unprocessed signal.

The device takes Live's name, Filter Delay. It runs on the device framework: its look is the framework's generic controls and one display cell for each line's X–Y pad, which stays a black frame, and its parameters, ranges and defaults come from the manual's section where it gives them and are Beatlayer's own where it does not. The device reports no latency. Where a delay sits in a chain and the device frame around it are [the device chains spec](device-chains.md) and [the device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.19 "Filter Delay" | Three independent delay lines, each preceded by linked low-pass and high-pass filters, and each line's feedback routed back through its filters; the first line takes the left input, the second both, the third the right; each line has an On switch, and each filter its own On switch beside its X–Y controller, whose horizontal axis sets the band's frequency and vertical axis its width; the Pan controls can override where the delays leave, which is otherwise the channel each takes its input from; Sync enables a chooser of sixteenth-note divisions (4 is one beat), and with Sync on a percentage shortens or lengthens the time for a swing; with Sync off the time is in milliseconds; Feedback sets how much output returns to the input and a very high value can run away; Volume can be raised to +6 dB to make up for heavy filtering at the input; Dry sets the unprocessed signal |
| S2 | Beatlayer | [The delay spec](delay.md), R1, R6 and the framework's details | A synced time is sixteenth notes through the tempo map at the stretch's tempo; a change of time moves the read position over 20 ms (Repitch) |
| S3 | Beatlayer | [The native device framework](device-framework.md), R2, R3, R4 and R9 | A gain reaches −∞ dB (G1); every parameter is automatable (G2); a continuous value glides over 20 ms; nothing on the audio thread allocates, locks or blocks |
| S4 | Beatlayer | [The improvements](../05-improvements.md), IMP-08 | A delay's levels reach silence |

## Rules

- **R1.** The first line hears only the left input and the third only the right. The second hears both, at the mean of the two, so a signal on one channel reaches it at half level and the same signal on both reaches it at full level. (S1; the mean is Beatlayer's own)
- **R2.** In Sync a line's time is a whole number of sixteenth notes from 1 to 16, and its echo lands on the frame the tempo map gives: 6,000 frames per sixteenth at 120 BPM and 48 kHz, 12,000 at 60 BPM. The echo follows a change of tempo. (S1, S2)
- **R3.** The Time % control, from −50 % to +50 %, shortens or lengthens a synced time by that share of it. With Sync off the line's time is its Time control, from 1 ms to 5 s, and its echo lands on that many milliseconds of frames. A synced time past the line's length, a little over 5 s (at a slow tempo), is held at that length. (S1; the ranges are Beatlayer's own)
- **R4.** A line leaves on the side it hears by default: Pan runs from −100 % (left) to +100 % (right), starting at −100 % for the first line, 0 % for the second and +100 % for the third, and the pan law is constant power, so a line at 0 % sends each channel 1/√2 of its level. (S1; the pan law and the defaults are Beatlayer's own)
- **R5.** A line's Pan moves its echo between the channels over the whole range. (S1)
- **R6.** A line's Filter switch puts a band in its loop: a high-pass at the band's lower edge and a low-pass at its upper edge, each second order (12 dB an octave, Butterworth). The band's centre frequency (Freq, 20 Hz to 20 kHz) is the geometric mean of its edges and its width (Width, 1 to 8 octaves) is the span between them, so the edges are at the centre divided and multiplied by two to the power of half the width. At a width of 4 octaves or more the band's −3 dB points sit at its edges, its centre and width within 2 %. The filters act on what enters the line, feedback included. (S1; the filter design and the ranges are Beatlayer's own)
- **R7.** Feedback, from 0 to 100 % (50 % to start), sends each line's output back into its own input, through its band, so each repeat is the one before times the feedback gain and the band's gain, which is within 0.1 dB of the feedback gain alone inside a wide band. (S1)
- **R8.** Volume is a gain from −∞ dB to +6 dB (0 dB to start) on a line's output, not on what feeds back; Dry is a gain from −∞ dB to 0 dB (0 dB to start) on the unprocessed signal; a line switched off sends nothing and takes in nothing, and its line is emptied. (S1, S3, S4; Volume acting on the output is Beatlayer's own)
- **R9.** Every parameter is automatable; renders are bit-identical at any block size and run after run; nothing the device does on the audio thread allocates, locks or blocks (the three lines and their bands are made with the device); an agent inserts the device with `device.insert`, sets any parameter by ID with `device.set_param` and reads them all with `device.parameters`; and the set saves and reopens with every value. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/filter_delay.rs`, `the_first_line_hears_the_left_the_third_the_right_and_the_second_both` | unit, real-time trap |
| R2 | `crates/devices/tests/filter_delay.rs`, `each_lines_echo_lands_on_its_synced_frame_at_two_tempos`, `an_echo_follows_a_tempo_change` | unit, real-time trap |
| R3 | `crates/devices/tests/filter_delay.rs`, `a_free_time_lands_on_its_frame_and_the_percentage_moves_a_synced_one` | unit, real-time trap |
| R4 | `crates/devices/tests/filter_delay.rs`, `each_lines_echo_lands_on_its_synced_frame_at_two_tempos` | unit, real-time trap |
| R5 | `crates/devices/tests/filter_delay.rs`, `a_lines_pan_moves_it` | unit, real-time trap |
| R6 | `crates/devices/tests/filter_delay.rs`, `the_bands_centre_and_width_hold_within_two_per_cent` | unit, real-time trap |
| R7 | `crates/devices/tests/filter_delay.rs`, `each_repeat_falls_by_the_feedback_gain_through_the_band` | unit, real-time trap |
| R8 | `crates/devices/tests/filter_delay.rs`, `volume_reaches_plus_six_and_minus_infinity_and_dry_reaches_minus_infinity`; `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity` | unit, real-time trap |
| R9 | `crates/devices/tests/filter_delay.rs`, `renders_are_bit_identical_at_every_block_size_and_run_after_run`; `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; `crates/cli/tests/agent.rs`, `an_agent_inserts_sets_and_reads_the_three_delays_by_command_id`; `rtsan/tests/devices.rs`, `the_three_delays_pass_under_rtsan` | unit, MCP script, RTSan |

## Defaults, ranges and details

- A new Filter Delay has all three lines on with their bands on at 1 kHz and 4 octaves, Sync on at 4, 8 and 12 sixteenths (one, two and three beats), Time % at 0 %, a free time of 100 ms, Pan at −100 %, 0 % and +100 %, Volume at 0 dB, Feedback at 50 % and Dry at 0 dB.
- The device is stereo: on a track with fewer than two channels it passes the input through.
- The line reads its time as the parameter's target and moves between times itself, in Repitch, over 20 ms (the delay spec's details), so a tempo change or a new time glides in pitch rather than clicking.
- The X–Y pad of each line is a display cell holding its Freq and Width controls; the cell is a black frame and the two parameters are the pad's axes.
- Beatlayer's own, where the manual gives no figure: every range and default above, the second line's mean, the pan law, the filter's order and design, the line's length, and Volume acting on the output.
