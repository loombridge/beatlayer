# The grain delay device

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.25. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); its random values are the effects' seeded source (device framework R29); its synced timing is the [delay](delay.md)'s
- **Parity-matrix row:** Audio effects: Grain Delay
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.22 "Grain Delay", read on 18 October 2026

## The behaviour

The grain delay delays its input and plays it back in short overlapping pieces, grains. The delay time is a beat division (Sync), optionally shortened or lengthened by a percentage, or a free time in milliseconds. Frequency sets how often a grain starts and so how long each lasts. Pitch transposes the grains; Random Pitch varies each grain's pitch at random; Spray starts each grain a random time later than the delay time, smearing the signal. Feedback sends the output back into the delay, and Dry/Wet balances the input against the grains. An X–Y display moves two parameters at once; any parameter can be put on either axis, and the assignment is part of the set.

Beatlayer's own name for the device is Scatter. It runs on the device framework with the generic controls and one display cell for the X–Y pad, a black frame holding the two axis choosers. The manual names the controls and their effects but gives no ranges, units or defaults, nor the grain design, so those are Beatlayer's own. The device reports no latency of its own: the delay time is the effect.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.22 "Grain Delay" | Sync enables a chooser of sixteenth-note divisions (4 is one beat) and a percentage that shortens or lengthens the time for a swing; with Sync off the time is in milliseconds; Spray adds random changes to the delay time, smearing the signal at low values and breaking up the source at high ones; Frequency sets the size and duration of each grain; Pitch transposes the grains like a crude pitch shifter; Random Pitch adds random variation to each grain's pitch, a mutant chorus at low values and an unrecognisable pitch at high ones; Feedback sends the output back into the input and a very high value can run away; Dry/Wet balances processed and unprocessed signal; an X–Y controller moves two parameters, the delay time on the horizontal axis and Dry/Wet on the vertical by default, and a parameter is assigned to an axis from the row beside it |
| S2 | Beatlayer | [The delay spec](delay.md), R1 | A synced time is sixteenth notes through the tempo map at the stretch's tempo |
| S3 | Beatlayer | [The native device framework](device-framework.md), R3, R4, R9 and R29 | Every parameter is automatable; a continuous value glides over 20 ms; nothing on the audio thread allocates, locks or blocks; a random source follows the device's own ID, so a render repeats |

## Rules

- **R1.** With Sync off the delay time is the Time control, from 1 ms to 5 s (100 ms to start), and an impulse's grain lands that many milliseconds of frames after the impulse, with Spray and Random Pitch at 0 and Pitch at 0. (S1; the range is Beatlayer's own)
- **R2.** With Sync on the delay time is a whole number of sixteenth notes from 1 to 16 (4 to start), changed by Time % from −50 % to +50 %, and an impulse's grain lands on the frame the tempo map gives: 24,000 frames for four sixteenths at 120 BPM and 48 kHz. (S1, S2)
- **R3.** Frequency, from 1 to 150 Hz (20 Hz to start), starts a grain every 1/Frequency seconds; a grain lasts two such periods under a Hann window, so two grains overlap and their windows add to 1, and a signal at Pitch 0 passes the grains at its own level. Each grain reads the delay at a speed of 2 to the power of Pitch over 12, centred so that its middle sits at the delay time; Pitch runs from −36 to +36 semitones (0 to start). A sine through Pitch +12 comes out an octave up and through Pitch −12 an octave down, within a cent. A grain whose reading would pass the newest sample, or the oldest kept, is held at the nearest. (S1; the grain design and the ranges are Beatlayer's own)
- **R4.** Spray, from 0 to 500 ms (0 to start), starts each grain a random time later than the delay time, between none and the Spray setting, so an impulse's grains land between the delay time and the delay time plus Spray. (S1; the range and the distribution are Beatlayer's own)
- **R5.** Random Pitch, from 0 to 100 % (0 to start), adds to each grain's Pitch a random number of semitones between minus and plus the setting's share of 12. Spray and Random Pitch draw from the effects' seeded random source, seeded with the device's ID, two values for every grain whatever the settings: renders repeat run after run and at any block size, and two devices differ. (S1, S3; the scale is Beatlayer's own)
- **R6.** Feedback, from 0 to 100 % (0 to start), writes the output times its gain back into the delay along with the input, so each repeat of an impulse falls by the feedback gain: 50 % gives repeats at 1, 0.5, 0.25 and 0.125 times the first. (S1)
- **R7.** Dry/Wet, from 0 % (the input alone) to 100 % (the grains alone), 50 % to start, mixes the input and the grains linearly. At 0 % the output is the input, bit for bit. (S1)
- **R8.** The X–Y display's two axes are the X Axis and Y Axis parameters, each a choice among Delay Time, Frequency, Pitch, Random Pitch, Spray, Feedback and Dry/Wet, starting at Delay Time and Dry/Wet. Delay Time stands for the time control shown, the division with Sync on and the milliseconds with it off. They are ordinary parameters: saved with the set, set by `device.set_param`, one undo step each, and automatable. (S1; the list is Beatlayer's own)
- **R9.** Every parameter is automatable; nothing the device does on the audio thread allocates, locks or blocks (the delay lines and the grain table are made with the device); an agent inserts the device with `device.insert`, sets any parameter by ID with `device.set_param` and reads them all with `device.parameters`; and the set saves and reopens with every value. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | `crates/devices/tests/grain_delay.rs`, `an_impulses_grain_lands_on_the_set_delay_free_and_synced` | unit, real-time trap |
| R3 | `crates/devices/tests/grain_delay.rs`, `pitch_moves_a_sine_an_octave_up_and_down_within_a_cent`, `an_impulses_grain_lands_on_the_set_delay_free_and_synced` | unit, real-time trap |
| R4 | `crates/devices/tests/grain_delay.rs`, `spray_smears_the_grain_later_within_its_range` | unit, real-time trap |
| R5 | `crates/devices/tests/grain_delay.rs`, `spray_and_random_pitch_repeat_run_after_run_and_differ_between_devices` | unit, real-time trap |
| R6 | `crates/devices/tests/grain_delay.rs`, `feedbacks_repeats_decay_by_its_gain` | unit, real-time trap |
| R7 | `crates/devices/tests/grain_delay.rs`, `dry_wet_mixes_the_input_with_the_grains` | unit, real-time trap |
| R8 | `crates/cli/tests/agent.rs`, `an_agent_inserts_sets_and_reads_the_three_delays_by_command_id` | MCP script |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; `crates/cli/tests/agent.rs`, `an_agent_inserts_sets_and_reads_the_three_delays_by_command_id`; `rtsan/tests/devices.rs`, `the_three_delays_pass_under_rtsan` | unit, MCP script, RTSan |

## Defaults, ranges and details

- A new Scatter has Sync off, a time of 100 ms (4 sixteenths, 0 % when synced), Frequency 20 Hz, Pitch 0, Random Pitch 0 %, Spray 0 ms, Feedback 0 %, Dry/Wet 50 %, and the X–Y axes on Delay Time and Dry/Wet.
- Up to eight grains sound at once; two overlap, and the rest are older grains that outlast a rise in Frequency. A ninth takes the oldest's place.
- A grain is read by linear interpolation, so high pitches lose some brightness, as the manual's "crude pitch shifter" allows. The two channels share each grain's delay and pitch, which keeps the stereo image.
- The delay line holds a little over 5.5 s, the longest time plus the widest spray. A time set at a slow tempo past it is held there.
- Pitch up shortens a grain's delay as it plays (it reads faster than it is written), so Pitch +12 needs a delay time of at least 1/Frequency seconds (50 ms at 20 Hz, a second at 1 Hz); at a shorter time the grain's start is held at the newest sample and sounds early.
- The dry signal is not delayed, so the device adds no latency.
- Beatlayer's own, where the manual gives no figure: every range and default above, the grain window and overlap, the pitch unit, the scales of Random Pitch and Spray, and the axis list.
