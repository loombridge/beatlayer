# Recording the Session into the Arrangement

- **Status:** partial
- **Work packages:** WP-3.13
- **Parity-matrix row:** Session performance into the Arrangement, Back to Arrangement
- **Checked against:** the Live 12 reference manual, chapter 19 "Recording New Clips", read 17 October 2026, and an observation of Live 12.4.3 on 17 October 2026

## The behaviour

While Arrangement Record runs, the Session clips that play are copied into the Arrangement. Each time a clip is launched it makes one Arrangement clip on its track, as long as the clip played, starting where the Session clip started, and carrying the Session clip's loop settings. Stopping a clip, launching another on the same track, launching the same clip again, Stop All Clips and Back to Arrangement each end the copies they stop. When recording ends, the Session clips keep playing and the tracks stay taken over until Back to Arrangement.

This spec covers what the copies are, where they start and end, what a clip edited mid-take becomes, how the copies sit beside an armed track's input recording, the automation a performance writes, undo, and what is left when recording ends. It leaves to other specs: recording armed tracks' input ([recording](recording.md)), recording into Session slots ([session-recording](session-recording.md)), and the Session launching rules ([session-view](session-view.md)).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 19 "Recording New Clips" | While Arrangement Record is on, launched Session clips are copied into their tracks at the matching song positions, clip property changes and control moves are logged, and recording ends with the record button or when playback stops. |
| S2 | Observation | Live 12.4.3 on macOS, 17 October 2026, 120 BPM, a one-bar launch quantisation, Start Transport With Record on | A looping one-bar MIDI clip that played for three bars became one Arrangement clip three bars long, with Loop on and a one-bar loop; a looping two-bar audio clip likewise, keeping its Start, End and loop, and the clip's name. A clip launched after recording began got its copy from its launch grid line, with the lane before it left alone, and cut an older Arrangement clip where the copy began. A clip already playing when recording began got a copy from the record point, its start marker where it was in its loop. A track's stop, a launch of another clip on the track, and Stop All Clips ended the copies on the stop's grid line. Relaunching the playing clip began a new copy where the relaunch landed. Typing a new loop length into the playing MIDI clip split its copy a moment after the edit, and the new clip's start marker continued from the loop position. Pressing Arrangement Record again ended the copies where it was pressed, not on a grid line, while the Session clips and the transport carried on and Back to Arrangement stayed lit. Stopping the transport ended recording and the clips, and Back to Arrangement stayed lit until pressed. Back to Arrangement pressed during the take ended that track's copy there while recording went on. The whole pass was one undo step, "Record", which removed every copy and restored the older Arrangement clip a copy had cut. A track's control moved during the take wrote Arrangement automation with Automation Arm off. An armed audio track recorded its input from the record point in the same pass as the Session copies; a launch on an armed track ended its input clip where the launch landed. |
| S3 | Spec | [session-view](session-view.md) R20 to R24; [recording](recording.md) R4, R7; [automation](automation.md) R17 | A launched Session clip takes its track over until Back to Arrangement; an armed track's take becomes a clip over the take; a recorded move is written as a touch. |
| S4 | Spec | [clip-envelopes](clip-envelopes.md) R1 to R3 | A clip's envelopes modulate its track's parameters while the clip plays and are part of the clip. |
| S5 | Decision | Built this way, 17 October 2026 | A clip's envelopes are copied with it, because the Arrangement has to play what the performance played; the stretch of the copy a clip's change belongs to is found by watching the clip while the take runs; a take that went round the Arrangement loop copies no Session clips. |

## Rules

- **R1.** While an Arrangement take runs, every Session clip that plays gets one clip in its track's Arrangement for each time it played: from the song frame it started on, which is its launch grid line, to the frame it stopped on. A clip already playing when the take begins gets one from the take's first frame. The copy is as long as the clip played, is the Session clip's data with its Loop switch, loop brace and Start and End markers as they are, and keeps its name, its mute and its clip envelopes, which are part of the clip and so play as they did. Its start marker stands where the Session clip was at the copy's first frame, so the copy plays what the Session clip played: a launched clip's marker is unchanged, a clip already playing has its marker moved on by the time since it launched, wrapped round the loop brace while the Loop switch is on. The Session clip's launch settings are not written. A copy replaces whatever Arrangement clips lay under it. (S1, S2)
- **R2.** A copy ends where its Session clip stops: a track's clip stop and Stop All Clips on their grid line, a launch of another clip on the same track where that clip starts (which is also where its copy begins), a relaunch of the clip where the relaunch lands (where the next copy begins), and Back to Arrangement where it is pressed. A copy of a clip still playing when the take ends ends where the take does, which is where Arrangement Record is pressed again or the transport stops, and is not on a grid line. (S2)
- **R3.** A change to the playing Session clip's contents, markers or loop splits its copy where the change is seen: the part before is the clip as it was, and the part after is the clip as it became, its start marker moved on by the time since the clip launched. A change to a clip's launch settings does not split it. The change is seen when the shell next polls the engine, so a split sits within a frame or two after the edit; a drag that is still going on is seen when it ends. Note edits and marker drags split the copy as every other change does. (S2, S5)
- **R4.** Armed tracks record their input in the same pass as the Session clips are copied. Where a Session clip is launched on an armed track, the track's recorded input is kept only up to the first copy's start. (S2)
- **R5.** While a Session clip has taken a track over, moving a control of any track is written into the Arrangement's automation as [automation](automation.md) R17 describes, whether or not Automation Arm is on. The moves are written as their own undo step, `Record Automation`, when recording stops. (S2, S3)
- **R6.** The copies of a pass, with the recorded input of the armed tracks, are one undo step, `Record`; Undo removes every copy and restores the Arrangement clips they replaced. A take with no copies makes the undo step recording makes without them. (S2)
- **R7.** When recording ends, the Session clips and the transport go on as they were, and Back to Arrangement stays lit until it is pressed. The copies are made when the take ends. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/engine/tests/session_launch_log.rs`, `the_launcher_logs_each_start_and_end_on_its_frame`, `nothing_is_logged_before_the_take_begins`; `crates/shell/src/session_capture.rs`, `starts_and_ends_pair_into_the_stretches_the_clips_played`, `the_copies_render_as_the_performance_did`; `crates/shell/tests/session_to_arrangement.rs`, `session_clips_launched_during_arrangement_record_land_in_the_arrangement` | frame by frame, golden render, headless |
| R2 | `crates/engine/tests/session_launch_log.rs`, `the_launcher_logs_each_start_and_end_on_its_frame`; `crates/shell/src/session_capture.rs`, `starts_and_ends_pair_into_the_stretches_the_clips_played` | frame by frame, unit |
| R3 | `crates/shell/src/session_capture.rs`, `the_copies_render_as_the_performance_did` | golden render |
| R4 | `crates/shell/src/session_capture.rs`, `an_armed_tracks_input_is_kept_only_up_to_its_first_copy` | unit |
| R5 | `crates/shell/tests/session_to_arrangement.rs`, `session_clips_launched_during_arrangement_record_land_in_the_arrangement`; `crates/shell/tests/automation_record.rs`, `with_automation_arm_off_nothing_is_recorded` | headless |
| R6 | `crates/shell/tests/session_to_arrangement.rs`, `session_clips_launched_during_arrangement_record_land_in_the_arrangement` | headless |
| R7 | `crates/shell/tests/session_to_arrangement.rs`, `session_clips_launched_during_arrangement_record_land_in_the_arrangement` | headless |

Every engine test runs its blocks under the allocation trap, and the shell test checks that the stream's `heap_operations` stays 0.

## Not built yet

- A Session clip's Automation envelopes (values, not modulation) written into the track's Arrangement automation. A clip's envelopes of both kinds are copied with it (R1), and the Automation ones play on the copy's track as [session-automation.md](session-automation.md) describes only while the clip plays in the Session.
- The scene tempo and time signature a launched scene sets, which the manual says are logged too.
- Copies of a take that went round the Arrangement loop: it copies no Session clips.
- Splitting an armed track's recorded input where Stop All Clips is pressed.
- The undo step of a pass that is split in two when Back to Arrangement is pressed mid-take; here the pass is one step.
