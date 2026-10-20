# CC Control, the MIDI controller sender

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.32. The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)), with the channel-message rules of R39; its messages travel on WP-5.44's path.
- **Parity-matrix row:** Devices: MIDI effects
- **Checked against:** the Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.2 "CC Control", read on 18 October 2026

## The behaviour

CC Control sends MIDI control changes to whatever follows it in a track's chain, or out to hardware through the track's MIDI To. It has three fixed controls — Mod Wheel (CC 1), Pitch Bend and Pressure — a Custom A button meant for a sustain pedal, and twelve Custom dials, B to M. Each of Custom A and the dials has a CC Type of its own, so a person can send any controller. The device shows one page of controls at a time; Send sends every assigned control's value at once; Learn hands the next controller an incoming MIDI source sends to the next Custom dial without one.

A control sends its controller on the frame its value changes. Its messages travel in the block beside the notes (device-framework R39), so the next device and a plug-in hear them on that frame, and they merge with a clip's own controller data for the same controller by travelling beside it (the MIDI controller data spec, R3): both reach the next device, and the one that arrives later wins. The device forwards every note and message it hears unchanged.

This spec covers the device only. Where a message goes on the wire is [midi I/O](midi-io.md); what a clip's own controller envelopes carry is [MIDI controller data](midi-controller-data.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", section 30.2 "CC Control" | Three fixed knobs (Mod Wheel, Pitch Bend, Pressure); a Custom A button sending on/off or minimum/maximum values, meant for CC 64 and assignable to any parameter through its CC Type chooser; twelve Custom dials, renamable and each assignable to any parameter through its CC Type chooser; two title-bar toggles for controls 1 to 8 and 9 to 16; a Send button sending all current values; a Learn toggle taking CC from an incoming MIDI source to a customizable control; CC automation already existing for a sent controller is merged |
| S2 | Beatlayer | [The native device framework](device-framework.md), R30 to R32 and R34, R39 | A native device reads the notes and channel messages that reach it and sends its own on their frames; a channel message carries a controller 0 to 127 with a value 0 to 1; a device forwards what it does not change |
| S3 | Beatlayer | [MIDI controller data](midi-controller-data.md) R3, R4 | A clip's controller envelope reaches the track's devices as channel messages on their frames |
| S4 | Beatlayer | [The improvements](../05-improvements.md), rule G2, and the info view spec's D17 | Every parameter is automatable and has a written description |
| S5 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules" | No allocation, lock or blocking call on the audio thread |
| S6 | Standard | The MIDI 1.0 specification | Controller 1 is the modulation wheel, 64 the sustain pedal, and 0 and 32 select banks |

## Rules

- **R1.** CC Control forwards every note, per-note expression and channel message it hears unchanged, and starts no note of its own. (S2)
- **R2.** A control sends its controller on the exact frame its value changes: Mod Wheel on CC 1, Custom A and each Custom dial on its CC Type, Pressure as channel pressure, and Pitch Bend as a pitch bend. A Custom control whose CC Type is None sends nothing. (S1, S2)
- **R3.** Custom A starts on CC 64; every Custom dial starts with no controller. (S1; the manual doesn't state the defaults.)
- **R4.** Pitch Bend's value runs from −8192 to 8191, 0 at the wheel's centre, and is sent as a pitch bend; Pressure's and the others' run from 0 to 127. (S1)
- **R5.** When Send turns on, every assigned control sends its current value once, on that frame. (S1)
- **R6.** While Learn is on, each control change an incoming MIDI source sends is handed to the next Custom dial without a controller, setting that dial's CC Type to the controller and its value to the message's; a control message also passes through unchanged. A controller the CC Type list leaves out (0, 32, and 120 to 127) is ignored. A device that already has every dial assigned takes nothing. (S1, S2, S6)
- **R7.** Each change Learn makes is one undo step named "Learn CC Control". (S1, S4)
- **R8.** A control's message reaches the next device in the chain, on its frame, at block sizes 1, 100, 512 and 4,096 alike, allocating nothing. (S2, S3, S5)
- **R9.** Every parameter is set by `device.set_param` and read by `device.parameters` through the agent API, a change is one undo step, and the device's preset is saved under `Presets/MIDI Effects/CC Control/`. (S4)
- **R10.** Every parameter is automatable and has a description, and nothing the device does on the audio thread allocates, locks or blocks. (S4, S5)

## Defaults, ranges and details

- Every Custom control's value starts at 0.
- The CC Type list starts with None, then controllers 1 to 119 by number; 0 and 32 (bank select) and 120 to 127 (channel mode) are left out. The manual doesn't state Live's list or its order (S6).
- The page switch shows controls 1 to 8 (Mod Wheel, Pitch Bend, Pressure, Custom A, Custom B to E) or 9 to 16 (Custom F to M). It is view state in Live; here it is a parameter among the others, since the generic layout has no paging.
- Custom controls' custom names (S1) aren't built: the framework has no per-parameter name to set. A dial is named by its letter.

## What the manual leaves open

For `docs/evidence/spec-notes/midi-cc-sender.md`:

- The CC Type list's order and which controller each Custom control starts on.
- What Learn does when a control already has a controller: here it fills the next dial without one.
- How merging with a clip envelope resolves two values for one controller at once: here both messages travel and the later one wins at the receiver.
- Whether Send leaves a controller at rest or sends every value including the fixed ones: here it sends every assigned control, fixed ones included.
- The page switch's two toggles, and how a Custom control is renamed.

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/midi_cc.rs`, `what_it_hears_passes_through_unchanged` | real-time trap |
| R2 | `crates/devices/tests/midi_cc.rs`, `a_control_sends_on_the_frame_its_value_changes`, `a_dial_sends_on_its_own_controller`, `pitch_bend_and_pressure_send_their_kinds` | real-time trap |
| R3 | `crates/devices/tests/midi_cc.rs`, `a_dial_sends_on_its_own_controller`; `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | real-time trap, MCP script |
| R4 | `crates/devices/tests/midi_cc.rs`, `pitch_bend_and_pressure_send_their_kinds` | real-time trap |
| R5 | `crates/devices/tests/midi_cc.rs`, `send_sends_every_assigned_control` | real-time trap |
| R6 | `crates/devices/tests/midi_cc.rs`, `learn_names_the_next_free_dial`; `crates/shell/tests/midi_io.rs`, `learn_assigns_an_incoming_controller_to_a_dial` | real-time trap, shell |
| R7 | `crates/shell/tests/midi_io.rs`, `learn_assigns_an_incoming_controller_to_a_dial` | shell |
| R8 | `crates/devices/tests/midi_cc.rs`, `a_dial_reaches_the_next_device_on_its_frame` (block sizes 1, 100, 512 and 4,096, under the allocation trap) | real-time trap |
| R9 | `crates/agent/tests/midi_effects.rs`, `an_agent_reaches_every_parameter_of_every_midi_effect` | MCP script |
| R10 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`, `every_parameter_has_a_description`; every test in `crates/devices/tests/midi_cc.rs` runs its blocks as real-time sections under the allocation trap; `rtsan/tests/midi_effects.rs` | real-time trap |
