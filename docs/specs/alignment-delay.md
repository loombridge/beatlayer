# The alignment delay device

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.25. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); the compensation it must not undo is [plug-in delay compensation](delay-compensation.md)
- **Parity-matrix row:** Audio effects: Align Delay
- **Checked against:** the Live 12 reference manual, chapter 33 "Max for Live Devices", section 33.2.1 "Align Delay", read on 18 October 2026

## The behaviour

The alignment delay delays the signal on each side by a set amount, set in one of three ways: a time in milliseconds, a number of samples, or a distance in metres or feet that sound covers in that time, for which the temperature of the air matters. Link gives the right side the left side's delay.

The device takes Live's name, Align Delay. It runs on the device framework with the generic controls and no display. The delay is the effect, so the device reports no latency and the engine's delay compensation does not hold the other tracks back to meet it. Where it sits in a chain is [the device chains spec](device-chains.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 33 "Max for Live Devices", section 33.2.1 "Align Delay" | It delays incoming signals in samples, milliseconds, or metres and feet; a Delay Mode chooser selects Time (milliseconds), Samples (for making up latency other devices add), or Distance (metres or feet); in Distance mode a Celsius/Fahrenheit switch and a temperature slider set the speed of sound; the Delay Left and Delay Right sliders set the two sides; Link makes the left slider set both and greys out the right |
| S2 | Beatlayer | [Plug-in delay compensation](delay-compensation.md), R1 and R2 | A device reports the frames it delays its input by, and every path is held back to the longest |
| S3 | Beatlayer | [The native device framework](device-framework.md), R3, R4 and R9 | Every parameter is automatable; a continuous value glides over 20 ms; nothing on the audio thread allocates, locks or blocks |

## Rules

- **R1.** In Time mode a side's delay is its time in milliseconds, from 0 to 1,000 ms, rounded to the nearest frame at the sample rate: 441, 480 and 960 frames for 10 ms at 44.1, 48 and 96 kHz. (S1; the range and the rounding are Beatlayer's own)
- **R2.** In Samples mode a side's delay is a whole number of samples from 0 to 96,000, and the output is the input shifted by exactly that count, bit for bit, with silence before it. A delay of 0 passes the input untouched. (S1; the range is Beatlayer's own)
- **R3.** In Distance mode a side's delay is the time sound takes to cover the distance, rounded to the nearest frame: the distance, from 0 to 300 in the chosen unit, converted to metres (a foot is 0.3048 m), over the speed of sound at the set temperature, from 331.3 + 0.606 × T metres a second for a temperature of T °C, which is 343.4 m/s at 20 °C, so 3.43 m is about 10 ms. The distance is set in metres or in feet (the Distance Unit, metres to start) and the temperature in °C, from −30 to 50 (20 °C to start), or in °F, from −22 to 122 (68 °F to start), by the Temperature Unit; each unit has a control of its own, and only the one the unit chooses acts. (S1; the formula, the ranges and the separate controls are Beatlayer's own)
- **R4.** With Link on, the right side takes the left side's delay, in whichever mode; with it off, each side has its own. Channels past the second take the right side's delay. (S1)
- **R5.** A change of delay while audio plays cuts to the new delay on the frame it arrives, as the delay is then read: a delay set in samples changes at once, and a time or distance set by a control glides with the control over 20 ms, which sounds as a short pitch bend. (Beatlayer's own, as the delay's time-change modes are its own)
- **R6.** The device reports no latency. In a set with delay compensation, a track through the alignment delay reaches Main later than a track without it by the delay, and the other tracks are not held back to meet it. (S2; Beatlayer's own rule, because the delay is the effect: compensating would undo it)
- **R7.** Renders are bit-identical at any block size and run after run, and nothing the device does on the audio thread allocates, locks or blocks: the delay lines are made with the device at their longest. (S3)
- **R8.** Every parameter is automatable; an agent inserts the device with `device.insert`, sets any parameter by ID with `device.set_param` and reads them all with `device.parameters`; and the set saves and reopens with every value. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/alignment_delay.rs`, `time_mode_delays_by_its_frame_count_at_44_1_48_and_96_khz` | unit, real-time trap |
| R2 | `crates/devices/tests/alignment_delay.rs`, `samples_mode_shifts_the_input_by_exactly_its_count` | unit, real-time trap |
| R3 | `crates/devices/tests/alignment_delay.rs`, `distance_mode_delays_by_the_time_sound_takes_in_metres_and_feet` | unit, real-time trap |
| R4 | `crates/devices/tests/alignment_delay.rs`, `link_gives_the_right_side_the_left_delay` | unit, real-time trap |
| R5 | `crates/devices/tests/alignment_delay.rs`, `a_change_of_delay_cuts_to_the_new_delay_on_its_frame` | unit, real-time trap |
| R6 | `crates/devices/tests/alignment_delay.rs`, `it_reports_no_latency_and_its_track_lands_later_than_the_others_by_its_delay` | golden render, real-time trap |
| R7 | `crates/devices/tests/alignment_delay.rs`, `renders_are_bit_identical_at_every_block_size_and_run_after_run`; `rtsan/tests/devices.rs`, `the_three_delays_pass_under_rtsan` | unit, RTSan |
| R8 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; `crates/cli/tests/agent.rs`, `an_agent_inserts_sets_and_reads_the_three_delays_by_command_id` | unit, MCP script |

## Defaults, ranges and details

- A new Align Delay is in Time mode with Link off, both sides at 0, metres, °C and 20 °C.
- The delay line is as long as the longer of 96,000 frames and one second, so a time or distance within the ranges fits at rates up to 96 kHz (a distance of 300 m at −30 °C is 0.96 s); above that a delay is held at the line's length.
- Beatlayer's own, where the manual gives no figure: every range and default above, the rounding to whole frames, the speed-of-sound formula, the separate unit controls, and the cut on a change.
