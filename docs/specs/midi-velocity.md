# The velocity MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.30. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the note rules of R26 to R30
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.8 "Velocity", read on 17 October 2026

## The behaviour

Touch reshapes the velocity of the notes that reach it. A curve runs from an input window of velocities (Lowest and Range) to an output range (Out Low and Out High); Operation says whether it is applied to a note's velocity, its release velocity, or both. Mode says what happens to a velocity outside the window: Clip holds it at the window's edge, Gate drops the note, and Fixed gives every note the Out High velocity. Random moves each velocity by a random amount inside the output range; Drive bends the curve toward loud or soft; Compand pushes the output toward the ends of the output range or toward its middle.

Beatlayer's own name for the device is Touch; its look is the device framework's generic controls (the curve's display waits for display cells that the engine can publish to). Random choices come from a generator seeded from the device, so an offline render repeats. This spec covers the device only.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.8 "Velocity" | Velocities from 1 to 127 are reshaped along a curve; the grid's horizontal axis is the input range (Lowest and Range), the vertical axis the output range (Out Low and Out Hi); with Lowest and Out Low at 0 and Range and Out Hi at 127 the effect does nothing; Operation chooses note-on velocity, release velocity, or both; Mode: Clip holds velocities outside the range at its edges, Gate removes them and a light flashes, Fixed gives every note the Out Hi velocity; Random adds random movement to every velocity, bounded by the output range, and a grey area shows its spread; Drive pushes values toward the extremes, loud when positive and soft when negative; Compand expands outputs toward the curve's boundaries when positive and compresses them toward the middle when negative |
| S2 | Beatlayer | [The native device framework](device-framework.md), R26 to R30 | As in the transposing spec |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S4 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules", and the Determinism requirement | No allocation, lock, or blocking call on the audio thread; random choices repeat in an offline render |

## Rules

- **R1.** A note-on's velocity, 1 to 127, lands where the curve puts it: its place in the window, from 0 at Lowest to 1 at Lowest plus Range, becomes a place between Out Low and Out High, and the velocity is the nearest whole number there. The note's key, channel, ID and frame are unchanged. (S1)
- **R2.** Operation Note On shapes note-on velocities only; Release shapes release velocities only; Both shapes both. (S1)
- **R3.** In Clip mode a velocity below the window is shaped as Lowest and one above it as the window's top. (S1)
- **R4.** In Gate mode a note whose velocity is outside the window is dropped, with its note-off and its expressions. A release velocity can't drop a note-off, so it is shaped as Clip does. (S1)
- **R5.** In Fixed mode every shaped velocity is Out High. (S1)
- **R6.** Drive bends the curve so more of the window comes out loud (positive) or soft (negative): the place in the window is raised to the power 2 to the −2 times Drive, with Drive from −1 to +1. Compand then moves each place toward a smooth S shape that pushes low places lower and high places higher (positive), or toward the middle of the output range (negative). Every shape stays between 0 and 1, and a Drive and Compand of 0 leave the curve straight. (S1)
- **R7.** Random, from 0 to 127, moves each shaped velocity by an amount drawn evenly from minus to plus Random, and the result stays between Out Low and Out High. The generator is seeded from the device, so two renders of one set give the same velocities. (S1, S4)
- **R8.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/Touch/`. (S3)
- **R9.** Every parameter is automatable and has a description, and nothing Touch does on the audio thread allocates, locks, or blocks. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/touch.rs`, `the_curve_maps_the_input_window_to_the_output_range` | real-time trap |
| R2 | `crates/devices/tests/touch.rs`, `operation_picks_what_the_curve_is_applied_to` | real-time trap |
| R3 | `crates/devices/tests/touch.rs`, `clip_holds_velocities_at_the_windows_edges` | real-time trap |
| R4 | `crates/devices/tests/touch.rs`, `gate_drops_notes_outside_the_window` | real-time trap |
| R5 | `crates/devices/tests/touch.rs`, `fixed_gives_every_note_out_high` | real-time trap |
| R6 | `crates/devices/tests/touch.rs`, `drive_and_compand_bend_the_curve` | unit, real-time trap |
| R7 | `crates/devices/tests/touch.rs`, `random_stays_in_the_output_range_and_repeats` | real-time trap |
| R8 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/touch.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |

## Defaults, ranges and details

- A new Touch starts at Lowest 0, Range 127, Out Low 0, Out High 127, Operation Note On, Mode Clip, Random 0, Drive 0 and Compand 0: it leaves velocities alone. The manual doesn't state the defaults.
- Lowest, Range, Out Low, Out High and Random are whole numbers from 0 to 127; Drive and Compand run from −100 to +100 per cent. An Out Low above Out High turns the curve upside down.
- The manual doesn't give the shapes of Drive and Compand; the formulas of R6 are Beatlayer's, chosen to agree with the manual's description.
- The curve's display and the light for a gated note are not drawn: they need the engine to publish a device's display state.
