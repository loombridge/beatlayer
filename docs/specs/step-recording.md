# Step recording: the Preview switch, MIDI step recording, and Step Input Mode

- **Status:** implemented (R1 to R13 each have a passing test)
- **Work packages:** WP-6.5 (R1 to R11 and R13); WP-7.3b (R12). The insert marker and the editing grid it moves by are [piano-roll.md](piano-roll.md)'s; a track's MIDI input, monitoring and the computer MIDI keyboard [midi-io.md](midi-io.md)'s; recording while the transport runs [recording.md](recording.md)'s; mapping the step buttons to keys and MIDI is [map modes](map-modes.md) R21
- **Parity-matrix row:** MIDI editing: Step input and step recording
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026; the Live 12 release notes, 12.0 and 12.0.5; the UI parity reference's rows L-CLIP-012, M-OPT-004, M-OPT-005, M-OPT-018 and S-NOTE-044, recorded from Live 12.4.3 on macOS

## The behaviour

The Preview switch above the MIDI Note Editor's piano ruler lets a person hear notes as they're drawn, selected or moved: each plays on the clip's track, whether or not the song plays. With Preview on, the clip's track armed and the transport stopped, the editor records notes step by step: the person places the insert marker, holds keys on a MIDI keyboard or the computer MIDI keyboard, and presses Right. The held notes are added at the insert marker, one grid step long, and the marker moves on a step. Pressing Right again while still holding them makes them a step longer; pressing Left takes the step back off, and removes them once they have none. Pressing Right with nothing held leaves a rest.

Step Input Mode, in the Options menu, gets the editor ready in one command: it arms the clip's track, puts the keyboard focus in the Clip View, and switches the computer MIDI keyboard.

This spec covers the Preview switch, step recording and Step Input Mode. It leaves the insert marker to [piano-roll.md](piano-roll.md), the MIDI input and the computer MIDI keyboard to [midi-io.md](midi-io.md), and recording while the song plays to [recording.md](recording.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 19 "Recording New Clips", section 19.3.4 "MIDI Step Recording" | With the transport stopped, the MIDI Note Editor records notes held on a MIDI controller or the computer keyboard, the insert marker moving on by the grid. Arm the clip's track, switch the editor's Preview on, and click to place the insert marker. Right moves the marker on by the grid, adding the notes held as it's pressed; pressing Right again with them still held makes them longer; Left with them held removes the notes just recorded. |
| S2 | Manual | Same manual, chapter 10 "Editing MIDI", section 10.4.2 "Previewing Notes" and section 10.1, item 6 | With an instrument on the track, the Preview switch above the piano ruler plays notes as they're added, selected and moved, and the piano ruler's keys as they're clicked; on an armed track it also allows step recording; the switch's state holds for every MIDI track in the set. |
| S3 | Manual | Same manual, chapter 19, section 19.9 "Using Remote Control for Recording" | The step recording's controls can be mapped to keys or MIDI, so a foot switch can step while both hands play. |
| S4 | Release notes | Live 12 release notes, 12.0 (Step Input and Step Input Mode; keyboard note entry and preview) | Options ▸ Step Input Mode arms the track, focuses the Clip View and switches the computer MIDI keyboard; it is a command, not a switch. In the map modes, step buttons show beside the grid's options. MIDI Note Editor Preview plays notes selected from the keyboard, in Focus Mode too, and a held preview lasts longer. |
| S5 | Release notes | Live 12 release notes, 12.0.5 (accessibility) | Step Input Mode makes screen-reader announcements. |
| S6 | Beatlayer | The UI parity reference: [layout.md](../ui-parity/layout.md) L-CLIP-012; [menus.md](../ui-parity/menus.md) M-OPT-004, M-OPT-005 and M-OPT-018 (disabled in a new set); [shortcuts.toml](../ui-parity/shortcuts.toml) S-NOTE-044, S-NOTE-048 and S-NOTE-049, and the `step-recording` context | Where the Preview switch and the menu items sit, and the keys. |
| S7 | Spec | [piano-roll.md](piano-roll.md) R8 and R23; [midi-io.md](midi-io.md) R7, R10, R12, R17 to R23 and R26; [recording.md](recording.md) R1 and R6 to R8; [mixer.md](mixer.md) R21 | The insert marker and a drawn note's velocity; what reaches a track's input, monitoring, the computer MIDI keyboard; arming, and recording notes while the transport runs; arming is `mixer.set_arm`, one undo step. |
| S8 | Beatlayer | [The architecture](../02-architecture.md), "Real-time rules" and "Agent control", decision D17 | No allocation, lock or blocking call on the audio thread; every action is a registered command with a schema, a description and a permission class, and what an agent sets it can read back. |

## Rules

**The Preview switch (WP-6.5)**

- **R1.** The Preview switch above the piano ruler (L-CLIP-012) and Options ▸ MIDI Note Editor Preview (M-OPT-005, ticked while on) are one switch for every MIDI clip's editor (`clip_view.set_preview`). It is off in a new install and kept between launches as a preference; switching it is no undo step and changes nothing in the set. (S2, S6; its default and keeping it as a preference are Beatlayer's own)
- **R2.** With Preview on, the editor plays a note on the clip's track, through its devices and mixer, whether or not the song plays and whether or not the track is armed or monitoring: clicking a key in the piano ruler plays that key at velocity 100 until the button is released; drawing a note, clicking a note, and selecting a note from the keys ([piano-roll.md](piano-roll.md) R25), in Focus Mode too, play it at its own velocity, until the button is released or, from the keys, for 250 ms; a drag that moves notes to a new pitch plays the grabbed note at each new pitch it reaches, ending the one before. A preview changes nothing in the set and is never an undo step. (S2, S4; the key click's velocity, the 250 ms and what a drag plays are Beatlayer's own)

**Step recording (WP-6.5)**

- **R3.** The editor step-records while the track of the clip it shows is armed, Preview is on and the transport is stopped (the `step-recording` context). The notes held are those held on the track's MIDI input ([midi-io.md](midi-io.md) R7, R12): a port's or the computer MIDI keyboard's note-ons whose note-offs haven't arrived. They sound through monitoring as any input does ([midi-io.md](midi-io.md) R10). (S1, S2, S7)
- **R4.** Right (S-NOTE-048) with notes held and the insert marker placed adds each held note at the insert marker, one step of the editing grid long, at the velocity it was played with, and moves the insert marker one grid step later. One undo step, `Step Record`. With nothing held, Right moves the insert marker as [piano-roll.md](piano-roll.md) R23 says, leaving a rest. (S1, S7)
- **R5.** Pressing Right again while some of the notes it added are still held makes those notes one grid step longer and moves the insert marker one step on; notes let go are left as they are, and keys pressed since the last Right are added at the insert marker as R4 says. The notes a step adds or lengthens are the ones just recorded until every key is up. One undo step each press. (S1; mixing held and new keys is Beatlayer's own)
- **R6.** Left (S-NOTE-049) while holding notes just recorded makes them one grid step shorter and moves the insert marker one step back, and a note that would be left with no length is removed. One undo step. With nothing held, Left moves the insert marker as [piano-roll.md](piano-roll.md) R23 says. (S1; shortening a step at a time is Beatlayer's own reading)
- **R7.** Step recording applies no record quantisation: the notes start on the insert marker. While the transport runs nothing is step-recorded, and an armed track records as [recording.md](recording.md) R6 to R8 say. (S1, S7)
- **R8.** Reading the held notes takes nothing from the audio thread that it doesn't already send: the main thread reads the input's notes through the copy the MIDI input already offers its listeners, and the audio thread neither allocates, locks nor blocks for it. (S8)

**Step Input Mode (WP-6.5)**

- **R9.** Options ▸ Step Input Mode (M-OPT-018) and Cmd-Shift-B (Ctrl-Shift-B on Windows; S-NOTE-044) run a command, not a switch, so the item never shows a tick; it is enabled while the editor shows a MIDI clip. It arms the clip's track as the track's arm button does (`mixer.set_arm`), moves the keyboard focus to the Clip View, and switches the computer MIDI keyboard on when it's off and off when it's on ([midi-io.md](midi-io.md) R17). The arming is an undo step as arming is; the rest isn't. (S4, S6, S7)
- **R10.** While the computer MIDI keyboard is on, its keys play notes that R3 counts as held ([midi-io.md](midi-io.md) R18 to R23), and Left and Right still step. (S1, S7)
- **R11.** Assistive technology announces each step: the notes added, made longer or removed, and where the insert marker now stands; and Step Input Mode announces what it switched. (S5; the wording is Beatlayer's own)
- **R12.** In MIDI Map Mode and Key Map Mode the editor shows Step Back and Step Forward buttons just before its grid chooser, which step as Left and Right do (R4 to R6) when pressed by a mapping or a click, so stepping maps to keys or MIDI; their place, their mappings and what a press runs are [map modes](map-modes.md) R11, R12 and R21. (S3, S4)
- **R13.** Every action here is a registered command, run by the switch, the keys, the menu and agents alike: `clip_view.set_preview` (edit, as a preference), `note.preview` (edit; plays a note on a clip's track and changes nothing in the set), `note.step_record` (edit; steps forward or back from the insert marker with the held notes given as keys and velocities, so an agent can step-record without holding keys) and `clip_view.step_input_mode` (edit). `view.state`'s `clip_view` reads the Preview switch and the insert marker, the query tool's clip the notes, and the query tool's track the arm. (S8)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/shell/tests/step_recording.rs`, `the_preview_switch_is_a_preference_that_survives_a_restart`; `crates/ui/tests/clip_view.rs`, `the_preview_switch_above_the_piano_ruler_runs_set_preview` | unit, UI script |
| R2 | `crates/engine/tests/midi_play.rs`, `a_preview_plays_on_its_own_track_with_the_song_stopped_and_is_never_input`; `crates/shell/tests/step_recording.rs`, `a_preview_plays_into_the_clips_track_until_released_and_never_hangs`, `a_note_selected_from_the_keys_sounds_at_its_own_velocity_for_250_ms`; `crates/ui/tests/clip_view.rs`, `a_key_held_in_the_piano_ruler_plays_until_released_with_preview_on`, `a_dragged_note_plays_each_pitch_it_reaches_and_ends_on_release` | engine, unit, UI script |
| R3 | `crates/shell/tests/step_recording.rs`, `nothing_step_records_with_the_transport_running_preview_off_or_the_track_disarmed`, `a_preview_is_not_a_held_key`; `crates/ui/tests/clip_view.rs`, `left_and_right_step_while_step_recording_and_move_the_marker_otherwise` | unit, UI script |
| R4 | `crates/shell/tests/step_recording.rs`, `holding_two_keys_and_pressing_right_twice_adds_two_notes_two_steps_long`, `right_with_nothing_held_leaves_a_rest` | unit |
| R5 | `crates/shell/tests/step_recording.rs`, `letting_one_key_go_lengthens_only_the_other_and_a_new_key_is_added_at_the_marker` | unit |
| R6 | `crates/shell/tests/step_recording.rs`, `left_shortens_the_notes_just_recorded_and_then_removes_them` | unit |
| R7 | `crates/shell/tests/step_recording.rs`, `letting_one_key_go_lengthens_only_the_other_and_a_new_key_is_added_at_the_marker`, `nothing_step_records_with_the_transport_running_preview_off_or_the_track_disarmed` | unit |
| R8 | `crates/engine/tests/midi_play.rs`, `a_preview_plays_on_its_own_track_with_the_song_stopped_and_is_never_input` (every block under the allocation trap); `crates/shell/tests/step_recording.rs`, `a_preview_is_not_a_held_key` | engine, unit |
| R9 | `crates/shell/tests/step_recording.rs`, `step_input_mode_arms_the_track_focuses_the_clip_view_and_switches_the_keyboard`; `crates/ui/tests/keymap/editors.rs`, `cmd_shift_b_runs_step_input_mode_and_the_arrows_step_in_the_note_editor`; `crates/ui/tests/clip_view.rs`, `cmd_shift_b_arms_the_track_and_switches_the_computer_keyboard` | unit, UI script |
| R10 | `crates/shell/tests/step_recording.rs`, `holding_two_keys_and_pressing_right_twice_adds_two_notes_two_steps_long` (the computer MIDI keyboard's keys); `crates/ui/tests/clip_view.rs`, `left_and_right_step_while_step_recording_and_move_the_marker_otherwise` | unit, UI script |
| R11 | `crates/shell/tests/step_recording.rs`, `each_step_is_announced` | unit |
| R12 | `bl-shell` `mapping_targets::the_step_buttons_step_record_with_a_held_note`; `bl-ui` `mapping_targets::the_clip_views_controls_tint_the_scrub_shows_in_midi_map_mode_only_and_the_step_buttons_show` | UI script, integration |
| R13 | `crates/shell-spec/src/notes.rs`, `every_listed_id_is_registered_and_complete`; `crates/shell/tests/step_recording.rs`, `step_record_by_id_takes_the_held_notes_as_keys_and_velocities`; `cargo xtask commands` (the readback table) | unit |
