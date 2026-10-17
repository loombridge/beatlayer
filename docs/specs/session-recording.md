# Session recording: slots, overdub and the take

- **Status:** partial
- **Work packages:** WP-3.11
- **Parity-matrix row:** Session slot recording, Session overdub
- **Checked against:** the Live 12 reference manual, chapter 19 "Recording New Clips" and section 8.14, read 17 October 2026

## The behaviour

Arming a track puts a record button in the empty slots of its Session column. The Session Record button records every armed track into the Session View; a slot's own record button records that one slot. The recording starts on the next line of the launch quantisation grid while the song plays, and from the playhead (after any count-in) while it is stopped, which starts the transport. A new clip exists in its slot from the take's start and grows as it records. Pressing Session Record again, or the recording slot's launch button, ends the recording and the new clip loops; a clip stop ends it without looping. On an armed MIDI track whose clip is playing, Session Record overdubs that clip instead, one loop pass at a time.

This spec covers which slot a recording goes into, how a take starts and ends, what a recorded clip is, overdub, and the settings that touch them. It leaves to other specs: recording into the Arrangement ([recording](recording.md)), recording the Session into the Arrangement (WP-3.13), and Session automation recording (WP-3.12).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, 19.3.2 "Recording Into Session Slots" | Arming shows Clip Record buttons in the empty slots of armed tracks; Session Record records into the selected scene on all armed tracks and the new clips' launch buttons turn red; pressing it again switches to loop playback; a slot's record button records that slot and its launch button switches to loop playback; a clip stop or the Control Bar stop ends it; launching a scene records into empty record-enabled slots only with "Start Recording on Scene Launch" on; a non-zero Global Quantization cuts the clips correctly. |
| S2 | Manual | 19.3.3 "Overdub Recording MIDI Patterns" | Notes are added to a looping MIDI clip on each pass; Session Record again pauses overdub while the clip plays, and again resumes it; Undo removes the last take. |
| S3 | Manual | 19.6 "Recording with Count-in" | Recording starts when the count-in ends. |
| S4 | Manual | 8.14 "Clip Defaults and Update Rate" | The Clip Update Rate is how often edits in the Clip View apply to a running clip; it is not about recording. |
| S5 | Observation | Live 12.4.3 on macOS, 15 October 2026, Session View | An empty slot on an armed track shows a record circle and the track's arm button is red. |
| S6 | Observation | Live 12.4.6 on macOS, 11 October 2026 and Live 12 on 15 October 2026, Settings > Record, Warp & Launch | File Type WAV, Bit Depth 24, Count-In None, Clip Update Rate 1/16, Record Session automation in Armed Tracks, Start Transport With Record On, Start Recording on Scene Launch Off. |
| S7 | Decision | Decided, 17 October 2026 | Which slot an occupied track records into, a clip that does not loop, what Stop leaves, one undo step per overdub pass, the transport and count-in with the transport stopped, and that Start Recording on Scene Launch is the scene menu's Trigger Recording on Launch. |

## Rules

- **R1.** Session Record puts each armed audio or MIDI track's recording where its clip and slots say: a MIDI track whose clip is playing overdubs it, whether or not the clip loops; any other armed track records a new clip into the slot of the selected scene (else the selected slot's scene, else the first) if empty, else into the next empty slot below on that track, else into a scene inserted at the end, in the same undo step. (S1, S2, S7)
- **R2.** A slot's record button works on an empty slot of an armed track; a slot with a clip has a launch button that never records. While a recording goes into a slot the engine reports it as recording, whatever the launcher says of it. (S1, S5, S7)
- **R3.** A take starts on the next launch grid line while the song plays and ends on the next one after it is asked to end (the press frame with quantisation off). It ignores the Arrangement's loop and punch points: its frames run on through the loop's wraps, and if the loop would wrap before the grid line it ends at the wrap. (S1, S7)
- **R4.** Session Record pressed while a Session recording runs, a launch of the recording slot, and `session.stop_recording` end the recording on the grid. A new clip's slot launches on the frame the take ends, so it loops in step from there; an overdubbed clip plays on. (S1, S2)
- **R5.** A clip stop of a recording track ends the recording on the grid without launching the clip. Transport Stop ends it where the playhead is, and nothing launches. The clip of a recording that ended is as long as the take, even if nothing was played. (S1, S7)
- **R6.** With the transport stopped, a recording starts it, and a Count-In runs first. An armed audio track with Audio From Ext. In records its input to a WAV in the project's `Samples/Recorded/`, moved earlier by the recording delay as in [recording](recording.md) R7; the set must have been saved. (S1, S3, S7)
- **R7.** A new MIDI clip is created in its slot when its take begins, loops, and its length and notes follow the take; a new audio clip is created when its file is finished and loops. The clip starts at 0 in its slot, is a normal slot clip, and is saved with the set. A new recording is one undo step, including a scene it inserted. (S1, S7)
- **R8.** An overdub adds the notes played to the playing clip, each to the loop pass it started in, placed where that pass plays it; a note is cut at the end of its pass; a clip that does not loop takes notes until it ends. Each pass is one undo step. Audio clips are never overdubbed. (S2, S7)
- **R9.** `session.record`, `session.record_slot` and `session.overdub` capture an input device, so an agent is asked first (permission class `external`); `session.stop_recording` never asks. (S7)
- **R10.** With Start Recording on Scene Launch on, launching a scene also records into the empty slots of armed tracks in it, on the scene's grid line. It is off in a new install. (S1, S6)
- **R11.** One recording runs at a time: Arrangement Record is refused while a Session recording runs, and a Session recording is refused while another recording runs. (S7)
- **R12.** `transport.state` reads a Session recording back under `recording.session_slots`, and the Record section of Record, Warp & Launch shows S6's values. (S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/shell/tests/session_recording.rs`, `a_taken_slot_sends_the_recording_to_the_next_empty_one_or_a_new_scene` | unit |
| R2 | `crates/shell/tests/session_recording.rs`, `a_slots_record_button_needs_an_armed_track_and_an_empty_slot`; `crates/engine/tests/session_recording.rs`, `a_take_starts_and_ends_on_grid_lines` | unit |
| R3 | `crates/engine/tests/session_recording.rs`, `a_take_starts_and_ends_on_grid_lines`, `a_recording_asked_for_mid_bar_waits_for_the_line`, `the_arrangement_loop_does_not_break_a_take`, `without_quantisation_a_take_runs_from_press_to_press` | frame by frame |
| R4 | `crates/engine/tests/session_recording.rs`, `a_launch_of_the_recording_slot_ends_it_and_a_stop_does_not_loop`, `a_recorded_clip_plays_in_step_with_where_the_take_ended`; `crates/shell/tests/session_recording.rs`, `session_record_again_ends_the_recording` | frame by frame |
| R5 | `crates/engine/tests/session_recording.rs`, `stop_ends_the_take_without_launching_the_slot`, `a_launch_of_the_recording_slot_ends_it_and_a_stop_does_not_loop` | frame by frame |
| R6 | `crates/engine/tests/session_recording.rs`, `a_count_in_delays_the_take_by_a_bar`; `crates/shell/tests/session_recording.rs`, `an_armed_audio_track_records_a_looping_audio_clip_into_the_slot` | frame by frame, null backend |
| R7 | `crates/shell/tests/session_recording.rs`, `session_record_records_a_midi_clip_into_the_slot_in_one_undo_step`; `crates/shell/src/recording.rs`, `a_session_takes_notes_start_from_the_takes_start` | unit |
| R8 | `crates/shell/src/recording.rs`, `an_overdub_sorts_notes_into_the_loop_passes_they_started_in`, `an_overdub_of_a_clip_that_does_not_loop_stops_at_its_end`; `crates/shell/tests/session_recording.rs`, `an_overdub_adds_each_loop_pass_as_an_undo_step` | unit |
| R9 | `crates/shell-spec/src/session_record.rs`, `starting_a_recording_asks_and_ending_one_does_not`; `crates/cli/tests/agent_session_recording.rs`, `an_agent_records_into_a_slot_and_ends_it_without_asking` | unit, agent API |
| R10 | `crates/shell/tests/session_recording.rs`, `launching_a_scene_records_only_when_start_recording_on_scene_launch_is_on` | unit |
| R11 | `crates/shell/tests/session_recording.rs`, `arrangement_record_waits_for_a_session_recording_to_end` | unit |
| R12 | `crates/cli/tests/agent_session_recording.rs`, `the_session_recording_commands_are_described`; `crates/ui/tests/clip_edge_fades.rs`, `the_record_rows_show_the_values_live_shows_in_a_new_install` | agent API, UI |

Every engine test runs its blocks under the allocation trap, and the shell tests check that the stream's `heap_operations` stays 0.

## Not built yet

- The Session View grid's record circle and launch button have no click handler; they arrive with the launching UI.
- Trigger Recording on Launch in the scene's context menu waits for that menu.
- An audio clip grows when its file is finished rather than while it records, and an overdub's notes appear when it ends.
