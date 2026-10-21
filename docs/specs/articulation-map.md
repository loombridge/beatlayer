# Articulation Map, a MIDI effect

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-6.47 (R1 to R20). The note path between devices it sits on is WP-5.30's ([device-chains.md](device-chains.md) R36), with WP-5.44's channel messages ([device-framework.md](device-framework.md) R39)
- **Parity-matrix row:** Devices: MIDI effects (Beatlayer's own device); [the improvements](../05-improvements.md), IMP-32 (the articulation map)
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026, chapter 30 "Live MIDI Effect Reference" and chapter 23 "Working with Instruments and Effects"; the Live 12 release notes, 12.0 to 12.4.6

## The behaviour

Sampled orchestral and band instruments change how a note is played (legato, staccato, pizzicato, palm-muted) when they receive a keyswitch, a note on a key outside the instrument's range, or a controller value. Each library has its own keys and values, so a part written for one doesn't play right on another, and a person has to remember which low C means what. The Articulation Map is a MIDI effect of Beatlayer's own that sits before such an instrument and holds a list of named articulations. One articulation is current at a time. A person or a clip chooses it with the device's Articulation control, which can be automated, or with a trigger of the person's choosing, a key or a controller range, played live or written in a clip. When the current articulation changes, the device sends what the instrument needs for it: keyswitch notes, controller values, and the MIDI channel its notes go out on.

Live has no device for this; a person sends keyswitches as notes in the clip. Nothing of Live's changes: the device is listed with the other MIDI effects and works like them.

This spec covers the device: its list of articulations, how the current one is chosen, what it sends and when, its parameters and panel, its presets, and its commands. How notes reach it and leave it is [device-chains.md](device-chains.md)'s; how a channel message travels is [device-framework.md](device-framework.md)'s.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The improvements](../05-improvements.md), IMP-32 (an articulation map device), rules 1 to 4, and rule G2 | The device is an addition that moves nothing; every parameter can be automated and modulated |
| S2 | Manual | Live 12 reference manual, chapter 30 "Live MIDI Effect Reference", and chapter 23 "Working with Instruments and Effects", section 23.2 | Live's MIDI effects sit before the instrument and change the notes and messages that reach it; none maps articulations |
| S3 | Release notes | Live 12 release notes, 12.0 to 12.4.6, read 20 October 2026 | No release adds an articulation or keyswitch device |
| S4 | Beatlayer | [device-framework.md](device-framework.md) R16 to R18, R30 to R39; [device-chains.md](device-chains.md) R6, R32, R36 to R38 | A native MIDI effect reads and sends notes on their frames, ends every note it started on the key and channel it started it on, sends channel messages beside the notes, passes everything through while switched off, and is inserted before the instrument; a control's change is one undo step; a device publishes display data |
| S5 | Standard | The MIDI 1.0 specification | A note is a key from 0 to 127 with a velocity from 1 to 127 on a channel from 1 to 16; a control change is a controller from 0 to 127 with a value from 0 to 127 |
| S6 | Beatlayer | [The architecture](../02-architecture.md), "Device chains", "Real-time rules" and "Agent control"; decisions D17 | Presets carry a device's settings; no allocation, lock or blocking call on the audio thread; every action is a registered command with a permission class and a readback |

## Rules

**The articulations**

- **R1.** Articulation Map is a MIDI effect, listed with the native MIDI effects and inserted before the instrument (device-chains R37). Its name is Beatlayer's own. (S1, S2, S4)
- **R2.** The device holds a list of 1 to 32 articulations, in order. Each has a name, one trigger (R3), up to four outputs (R4) and an output channel (R5). A new device holds one articulation, named "Articulation 1", with no trigger, no outputs and no channel. The limits and the defaults are Beatlayer's own. (S1)
- **R3.** An articulation's trigger is one of: none (only the Articulation control chooses it); a key, 0 to 127, matched on any channel at any velocity; or a controller, 0 to 127, with a range of values from a lowest to a highest, 0 to 127 inclusive. No two articulations of one device have the same key, and no two have overlapping ranges of the same controller: a change that would make them so is refused. (S1, S5)
- **R4.** An output is a key, 0 to 127, with a velocity, 1 to 127, or a controller, 0 to 127, with a value, 0 to 127. An articulation's outputs are sent in their order. (S1, S5)
- **R5.** An articulation's channel is none, so notes keep the channel they come in on, or a channel from 1 to 16. (S1, S5)

**Choosing the articulation**

- **R6.** One articulation is current. The Articulation control is a stepped parameter from 1 to the number of articulations, its value text the articulation's name; it's 1 in a new device. Like any parameter it's set by hand, by automation, by a clip envelope and by modulation (rule G2), and the articulation it names becomes current on the frame its value changes. (S1, S4)
- **R7.** A trigger arriving makes its articulation current on its frame: a note-on on a trigger's key, or a control change of a trigger's controller with a value in its range. Whichever of R6's change and a trigger came last decides the current articulation. Removing the current articulation makes the one before it current, or the first. (S1, S4)
- **R8.** With Pass Triggers off, as it is in a new device, a trigger's note-on, its note-off and its control change don't reach the next device; with it on, they pass on as they came. Every note and message that isn't a trigger passes on as R10 says. (S1, S4)

**What it sends**

- **R9.** When an articulation becomes current, by R6 or R7, the device sends its outputs on that frame, before any note-on on the same frame, in their order: a key output as a note-on with its velocity, and its note-off Keyswitch Length later (R11); a controller output as a control change. Outputs go out on the articulation's channel, or on channel 1 when it has none. A trigger for the articulation that's already current sends its outputs again. (S1, S4, S5)
- **R10.** Every note-on from that frame on goes out on the current articulation's channel when it has one, and on its own channel otherwise; so do the control changes, pitch bends and channel pressure that follow it. A note that's sounding keeps the channel it started on for its note-off and its per-note expression, whatever becomes current while it's held (device-framework R31, R33). (S4)
- **R11.** Keyswitch Length is a parameter from 1 to 1,000 ms, 20 ms in a new device. A key output sent again while its last note-on is still held sends that note's note-off first, on the same frame. The range and default are Beatlayer's own. (S1)
- **R12.** Resend on Start is a switch, on in a new device. While it's on, the device sends the current articulation's outputs on the first frame the transport plays after it starts, and on the first frame after the device is switched on, so an instrument reloaded or reset since the last change follows. (S1)
- **R13.** Switched off, the device passes everything through unchanged (device-chains R6) and ends any key output it has sounding on the first frame it's off (device-framework R32); the current articulation stays as it was. (S4)
- **R14.** Everything the device sends lands on its frame at block sizes 1, 100, 512 and 4,096 alike. The list reaches the engine as the node's data, of a fixed size made with the node, so a change to it reaches the playing device without a new program, and nothing on the audio thread allocates, locks or blocks. (S4, S6)

**Its panel and commands**

- **R15.** The device's panel shows the Articulation control, Pass Triggers, Keyswitch Length and Resend on Start, and a list of the articulations as a display cell: one row each with its number, name, trigger, outputs and channel, the current one lit. Clicking a row's number makes it current by setting the Articulation control, one undo step named Change "Articulation". A double-click on a name renames it in place. Each row's Learn button, while lit, takes the next note-on or control change the device hears as that row's trigger (a key, or the controller with that one value as its range), as one undo step, and goes out; Esc or another click on it goes out without a change. The panel's layout is Beatlayer's own. (S1, S4)
- **R16.** Adding, deleting, moving and renaming an articulation, and setting its trigger, its outputs or its channel, are each one undo step, with names of Beatlayer's own, and are saved with the set and in the device's presets, under `Presets/MIDI Effects/Articulation Map/`, so a person makes a map once for an instrument and loads it again. (S1, S6)
- **R17.** The device publishes, after each block, the current articulation's number and name as its display readings (device-framework R35 to R37), so `device.display` reads which articulation is current. (S4)
- **R18.** Every action is a registered command run by ID through the agent API with the device named: `articulation.add`, `articulation.delete`, `articulation.move`, `articulation.set` (any of the name, trigger, outputs and channel), each `edit` and read back through `articulation.list` (`read`), which gives every articulation in order with its settings, and through the query tool's `device` object, whose `articulations` field gives the same; and `articulation.learn` (`read`: it changes only the panel, and the trigger it learns is the undo step). A gesture on the panel runs one of them. (S6)
- **R19.** Its parameters, Articulation, Pass Triggers, Keyswitch Length and Resend on Start, are set by `device.set_param` and read by `device.parameters`, each change one undo step, and each is automatable and has a description (rule G2). (S1, S4)
- **R20.** A saved device whose list is empty or holds more than 32 articulations, whose triggers break R3, or whose channel or values fall outside R3 to R5's ranges doesn't load. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/articulation_map.rs`, `the_device_sends_on_its_frame_at_any_block_size`; `crates/shell/tests/articulation_map.rs`, `the_list_is_edited_one_undo_step_at_a_time` | unit, headless |
| R2 | `crates/model/src/articulation.rs`, `a_list_of_one_articulation_is_sound`; `crates/shell/tests/articulation_map.rs`, `the_list_is_edited_one_undo_step_at_a_time` | unit, headless |
| R3 | `crates/model/src/articulation.rs`, `two_articulations_with_the_same_key_are_refused`, `two_articulations_with_overlapping_controller_ranges_are_refused`; `crates/shell/tests/articulation_map.rs`, `a_list_that_breaks_the_rules_is_refused` | unit, headless |
| R4 | `crates/model/src/articulation.rs`, `values_outside_their_ranges_are_refused`; `crates/devices/tests/articulation_map.rs`, `a_trigger_sends_its_outputs_before_the_notes_on_the_same_frame` | unit |
| R5 | `crates/model/src/articulation.rs`, `values_outside_their_ranges_are_refused`; `crates/devices/tests/articulation_map.rs`, `a_note_held_across_a_change_ends_on_its_own_channel` | unit |
| R6 | `crates/devices/tests/articulation_map.rs`, `the_articulation_control_makes_its_articulation_current`, `an_automated_articulation_lane_switches_on_its_breakpoints_frames` | unit |
| R7 | `crates/devices/tests/articulation_map.rs`, `a_trigger_sends_its_outputs_before_the_notes_on_the_same_frame` | unit |
| R8 | `crates/devices/tests/articulation_map.rs`, `pass_triggers_lets_the_trigger_through` | unit |
| R9 | `crates/devices/tests/articulation_map.rs`, `a_trigger_sends_its_outputs_before_the_notes_on_the_same_frame` | unit |
| R10 | `crates/devices/tests/articulation_map.rs`, `a_note_held_across_a_change_ends_on_its_own_channel` | unit |
| R11 | `crates/devices/tests/articulation_map.rs`, `a_repeated_key_output_ends_the_earlier_one_first` | unit |
| R12 | `crates/devices/tests/articulation_map.rs`, `resend_on_start_sends_the_outputs_again_when_the_transport_plays` | unit |
| R13 | `crates/devices/tests/articulation_map.rs`, `a_device_switched_off_ends_its_sounding_keyswitch` | unit |
| R14 | `crates/devices/tests/articulation_map.rs`, `the_device_sends_on_its_frame_at_any_block_size`; `rtsan/tests/midi_effects.rs`, `the_articulation_map_passes_under_rtsan` | unit, headless |
| R15 | `crates/ui/tests/articulation_map.rs`, `a_click_on_a_rows_number_makes_that_articulation_current`, `a_rows_learn_button_lights_while_it_is_armed` | UI script |
| R16 | `crates/shell/tests/articulation_map.rs`, `the_list_is_edited_one_undo_step_at_a_time`, `the_list_is_saved_with_the_set_and_in_the_presets` | headless |
| R17 | `crates/shell/tests/articulation_map.rs`, `the_display_reads_the_current_articulation` | headless |
| R18 | `crates/cli/tests/agent.rs`, `an_agent_builds_an_articulation_map_by_command_and_reads_it_back`; `crates/shell/tests/articulation_map.rs`, `the_list_reads_back_through_the_command_and_the_query_tool` | headless |
| R19 | `crates/shell/tests/articulation_map.rs`, `the_list_is_edited_one_undo_step_at_a_time` | headless |
| R20 | `crates/project/src/format.rs`, `a_set_with_a_broken_articulation_list_does_not_load`; `crates/model/src/articulation.rs`, `an_empty_or_long_list_is_refused` | unit |
