# MPE Control, the MPE shaper

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.32. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the per-note expression and channel-message rules of R33 and R39.
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 33 "Max for Live Devices", section 33.3.4 "MPE Control", and chapter 12 "Editing MPE", read on 18 October 2026

## The behaviour

MPE Control shapes the per-note expressions that reach it, note by note, before an instrument reads them, and can turn each of them into a channel message so a controller that speaks MPE can play an instrument that doesn't. Its three sources are pressure, slide and per-note pitch bend. Each is switched on or off and has its own curve — a linear one or an S-shaped one with two segments — a lowest and highest output value, and smoothing with a rise and a fall time. Press can take a default value for a note that carries none, send what pressure carried as slide instead, or convert pressure to channel pressure; Slide can do the same by default, be read as distance from the middle, start each note in the middle or update only when a note starts, and convert to the mod wheel; NotePB scales the bend by a pitch range and can convert it to a channel pitch bend.

The device passes notes, channel messages and any source that is switched off through unchanged. Its curve laws, its time constants and what Default does are Beatlayer's own, since the manual states none of them; they're written out below.

This spec covers the device only. What MPE expressions are is [editing MPE](https://www.ableton.com/en/live-manual/12/editing-mpe/); how expressions reach a device is device-framework R33.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 33, section 33.3.4 "MPE Control" | Three sources, Press, Slide and NotePB, each switched on by default and deactivatable; a linear or S-shaped curve per source, with Min and Max, and a movable breakpoint (X and Y) and a Curve Link for the S-shaped one; a Smooth toggle with Rise and Fall per source; Press's Default, Swap to Slide and Press to AT; Slide's Default, Centered, Abs/Rel/Ons modes and Slide to Mod; NotePB's Pitch Range and NotePB to PB |
| S2 | Manual | Live 12 reference manual, chapter 12 "Editing MPE", sections 12.1 to 12.5 | MPE attaches control information to individual notes: per-note pitch bend, slide (CC 74) and pressure, alongside velocity |
| S3 | Beatlayer | [The native device framework](device-framework.md), R33 and R39 | A per-note expression follows the note it belongs to; a channel message carries a controller, a bend or a pressure for the whole channel |
| S4 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S5 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules" | No allocation, lock or blocking call on the audio thread |

## Rules

- **R1.** With its default curve (linear, Curve A 0.5, Min 0, Max 1, no smoothing), a source shapes a value to itself: pressure 0.25 comes out 0.25. A source that is switched off passes its data through unchanged (R10). (S1)
- **R2.** Min and Max place the output range: an input at the bottom comes out Min, at the top Max. (S1)
- **R3.** The linear curve bends by Curve A: the output is the input raised to `2^(2 × (0.5 − Curve A))`, so 0.5 is straight. (S1; the manual states no law, so this is Beatlayer's own.)
- **R4.** The S-shaped curve joins two segments at its breakpoint (X, Y), each segment bent like the linear curve by Curve A, or by Curve B when the segments aren't linked: an input below X maps into 0 to Y, one above into Y to 1. (S1; the law is Beatlayer's own.)
- **R5.** With Press to AT on, shaped pressure is sent as channel pressure instead of a per-note expression. (S1)
- **R6.** With Swap to Slide on, shaped pressure is sent as a slide expression instead of pressure. (S1)
- **R7.** With Slide to Mod on, shaped slide is sent as CC 1. (S1)
- **R8.** With Slide's Centered on, slide is read as the distance from the middle: the centre is 0 and either end is 1. (S1)
- **R9.** NotePB scales the shaped bend by Pitch Range (0.1 to 4, 1 by default), held between −1 and +1; with NotePB to PB on, it is sent as a channel pitch bend. (S1)
- **R10.** A source switched off passes its data through unchanged. (S1)
- **R11.** With Smooth on, a source's output moves toward its new value over its Rise time when rising and its Fall time when falling, as an exponential approach: after one time constant it has covered 63 %. (S1; the manual states no law, so this is Beatlayer's own.)
- **R12.** With Press's Default on, a note that carries no pressure gets the Press default value on its frame. (S1)
- **R13.** With Slide's Default on, a note that carries no slide gets the Slide default value on its frame; in Slide's Ons mode, slide is updated only when a note starts, and smoothing doesn't apply. (S1)
- **R14.** A channel message that reaches the device passes through unchanged. (S3)
- **R15.** Slide's Rel mode starts each note in the middle and moves by how far the finger has swept since: the first slide value sets where it began. (S1)
- **R16.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/MPE Control/`. (S4)
- **R17.** Every parameter is automatable and has a description, and nothing the device does on the audio thread allocates, locks or blocks, at any block size. (S4, S5)

## Defaults, ranges and details

- All three sources start switched on, with the linear curve, Curve A and Curve B 0.5, linked, Min 0, Max 1, X and Y 0.5, smoothing off, Rise and Fall 50 ms — the manual states no defaults, so these are Beatlayer's own.
- Press's and Slide's default value starts at 0.5; Pitch Range starts at 1. Every conversion and Default switch starts off; Slide's mode starts Abs.
- Smoothing is applied to the value at each message, using the frames since the last one; the first message of a note takes its value at once. Rise and Fall run from 0 to 1000 ms; 0 means no smoothing.
- A note that gets a default and then carries its own expression takes the expression's value from then on.

## What the manual leaves open

For `docs/evidence/spec-notes/mpe-shaper.md`:

- The curve laws, the meaning of Curve A and Curve B, and what Curve Link does exactly.
- The smoothing's law and its units, and whether it acts per source or per note.
- What Default does to a note that carries no expression, and whether the default or an arriving expression wins.
- Whether Press to AT, Slide to Mod and NotePB to PB replace the per-note expression or add to it: here they replace it.
- How NotePB's Pitch Range scales the bend, and what happens when the scaled bend leaves ±1.

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/midi_mpe.rs`, `pressure_passes_the_identity_curve_unchanged` | real-time trap |
| R2 | `crates/devices/tests/midi_mpe.rs`, `min_and_max_place_the_output_range` | real-time trap |
| R3 | `crates/devices/tests/midi_mpe.rs`, `curve_a_bends_the_linear_curve` | real-time trap |
| R4 | `crates/devices/tests/midi_mpe.rs`, `the_s_curve_joins_two_segments` | real-time trap |
| R5 | `crates/devices/tests/midi_mpe.rs`, `press_to_at_converts_to_channel_pressure` | real-time trap |
| R6 | `crates/devices/tests/midi_mpe.rs`, `swap_to_slide_sends_pressure_as_slide` | real-time trap |
| R7 | `crates/devices/tests/midi_mpe.rs`, `slide_to_mod_converts_to_the_mod_wheel` | real-time trap |
| R8 | `crates/devices/tests/midi_mpe.rs`, `centered_slide_reads_distance_from_the_middle` | real-time trap |
| R9 | `crates/devices/tests/midi_mpe.rs`, `notepb_scales_the_bend_and_converts_to_pitch_bend` | real-time trap |
| R10 | `crates/devices/tests/midi_mpe.rs`, `a_source_that_is_off_passes_through` | real-time trap |
| R11 | `crates/devices/tests/midi_mpe.rs`, `smoothing_moves_over_the_rise_time` | real-time trap |
| R12 | `crates/devices/tests/midi_mpe.rs`, `press_default_gives_a_note_pressure` | real-time trap |
| R13 | `crates/devices/tests/midi_mpe.rs`, `slide_defaults_on_note_on` | real-time trap |
| R14 | `crates/devices/tests/midi_mpe.rs`, `a_channel_message_passes_through` | real-time trap |
| R15 | `crates/devices/tests/midi_mpe.rs`, `slide_rel_starts_in_the_middle` | real-time trap |
| R16 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R17 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/midi_mpe.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |
