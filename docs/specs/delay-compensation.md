# Plug-in delay compensation

- **Status:** partial
- **Work packages:** WP-2.11; the Delay Compensation, Reduced Latency When Monitoring, and Keep Monitoring Latency rules (R7 to R9) since WP-5.36
- **Parity-matrix row:** Mixing: Delay compensation
- **Checked against:** Live 12.4.3 on 14 October 2026, and the Live 12 reference manual, chapter 23 section 23.3 "Device Delay Compensation" and chapter 18 sections 18.7 "Track Delays" and 18.8 "Keep Monitoring Latency", read 17 October 2026

## The behaviour

A device or plug-in can delay its input by a number of frames (look-ahead,
linear-phase filtering, oversampling, a hosted plug-in's own reported latency).
Without compensation, a signal through a high-latency device reaches the Main
output later than a signal that does not, so tracks that should line up drift
apart. Beatlayer compensates for this the way Live does: it holds every path
back to the longest one so everything reaches Main aligned, offsets a track's
clips and a device's sidechain source to match, and offsets a parameter change
meant for a song position by the same latency so automation acts on the audio it
is meant for.

Two set settings switch the compensation: Delay Compensation turns it on or
off, and Reduced Latency When Monitoring gives a monitored track the lowest
possible latency at the cost of lining up with the rest of the set. Every track
also has a delay of its own, which joins this model ([mixer](mixer.md) R28), and
a Keep Monitoring Latency switch ([mixer](mixer.md) R30) deciding what a
recording made while the track is monitored holds.

This spec covers the latency model and the compensation the engine applies. It
leaves the latency a particular device reports to that device's own spec
(`device-chains.md` for the native framework, `clap-hosting.md` for plug-ins),
and automation editing to `automation.md`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, "Mixing", delay compensation | Live delays every track so that signals through high-latency devices stay aligned at the Main output; automation and sidechains are compensated with the audio; compensation is recomputed when the device set changes. |
| S2 | Observation | Live 12.4.3 on macOS, 14 October 2026 | A device disabled from its title bar reports no latency and the chain realigns; re-enabling restores it. |
| S3 | Manual | Live 12 reference manual, chapter 23, section 23.3 "Device Delay Compensation" | Compensation is on by default and switched in the Options menu; while it's on, Reduced Latency When Monitoring is available; while it's off, no path is held back and a track's Track Delay controls are unavailable. A monitored track with Reduced Latency When Monitoring on takes the lowest possible latency and may be out of sync with other tracks such as returns, which stay compensated. |
| S4 | Manual | Live 12 reference manual, chapter 18, section 18.7 "Track Delays" | Every track has a Track Delay control that shifts its output against the others, positive to delay and negative to play early, in milliseconds or in samples. |
| S5 | Manual | Live 12 reference manual, chapter 18, section 18.8 "Keep Monitoring Latency" | A track's Keep Monitoring Latency switch decides what a recording made while the track is monitored holds: on, the track keeps the compensation of its monitoring path so the recording lines up with the rest of the set; off, the monitoring path keeps none and the recording matches what was heard. |
| S6 | Beatlayer | [the build plan](../03-build-plan.md) WP-5.36; [mixer](mixer.md) R16, R28, R30 | A track's delay joins the delay-compensation model so its path counts the delay with the path's latency; the set saves the switches as Beatlayer saves its other settings; what Reduced Latency When Monitoring does to a monitored track and its neighbours is Beatlayer's own reading of the manual until Live is observed. |

## Rules

- **R1.** Each device and plug-in reports the frames it delays its input by; a switched-off device reports none, but its latency is remembered so it returns when the device is switched on again. A switched-off device isn't run (IMP-15), so it reports no latency and takes no time. (S1, S2)
- **R2.** Every path into a summing point — a track's inputs, a track's output into Main, into a group or into another track's input, and a send into a return — is held back to the latency of the longest path into that point, so all paths reach Main aligned. (S1)
- **R3.** A track's clips are held back to line up with that track's inputs. A MIDI track's clips are notes, so the notes are held back, on their own frames, before its devices play them. (S1)
- **R4.** A device's sidechain source is held back to line up with the device's own input at the point in the chain where the device sits. (S1)
- **R5.** A parameter change sent for a song frame is held back by its node's latency, so automation acts on the audio it is meant for; a change sent to take effect now, such as a person's control move, is not delayed. A send's level acts on the signal after its delay line, so its changes are held back as long as that signal is behind song time at the return. (S1)
- **R6.** Compensation is recomputed when the graph or a device's latency changes, including a native device whose latency follows a parameter, such as a look-ahead: the engine publishes the devices whose node reports a latency other than the compensated one, and the shell reports each back; an offline render ([offline plug-in renders](offline-plugin-renders.md)) reports them back to its own compiler between blocks. The delay lines are preallocated and processed in place, so the audio thread allocates nothing. A recompile keeps the audio, and the notes of a MIDI track, held in each delay that holds the same signal back by as many frames as before; a MIDI track's notes also carry over a change of the delay's length, moved by the difference so they keep their order. (S1)
- **R7.** Delay Compensation is a set setting, `delay_compensation`, on in a new set and saved with the set only where it's off, as [mixer](mixer.md) R16 saves Solo in Place in the same pattern; `mixer.set_delay_compensation { "enabled": … }` sets it as one undo step. While it's off: no path is held back and no node's latency counts (R2 to R5 stand aside), a track's delay ([mixer](mixer.md) R28) has no effect, and every Track Delay control and every external device's Hardware Latency control is disabled, keeping its value (the external devices' controls it disables are [external devices](external-devices.md) R14). The manual puts the switch in chapter 23. (S3, S6)
- **R8.** Reduced Latency When Monitoring is a set setting, `reduced_latency_monitoring`, off in a new set and saved with the set only where it's on; `mixer.set_reduced_latency_monitoring { "enabled": … }` sets it as one undo step, and it can be on only while Delay Compensation is on (R7). With it on, a track whose input is monitored — Monitor In, or Auto while the track is armed and no clip is playing ([routing](routing.md) R11) — is given no input or route compensation into its summing point while the rest of the set stays held back, so the monitored track reaches the output earliest; a track on Auto keeps its compensation for its clips, and its signal skips the held path only while it is monitored, moving between the two over the mixer's 20 ms glide as a clip starts or ends; this is Beatlayer's own reading of the manual until Live is observed. What it does to the track's neighbours, and how Keep Monitoring Latency interacts with it, aren't settled by the manual; Beatlayer's own choice is R9's. (S3, S6)
- **R9.** A monitored track's compensation follows Keep Monitoring Latency ([mixer](mixer.md) R30) and R8: with Reduced Latency When Monitoring on, a monitored track is given no compensation, so the recording matches what is heard and Keep Monitoring Latency has nothing to keep; with it off, an input-monitored track whose Keep Monitoring Latency is off also loses its compensation (the recording matches the lowest-latency monitoring path), while with Keep Monitoring Latency on the track's normal compensation stands, so a recording made from its input is moved to line up with the rest of the set. (S5, S6)
- **R10.** Main's output lags song time by the frames its slowest path into it and its own devices add, or by its own devices alone while delay compensation is off. An offline render discards that many frames from its start and plays on that many past the range's end, so its first frame is the frame the range starts on; a real-time render captures the range that much later. A frozen or bounced track therefore plays where the unfrozen one did. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R2 | `crates/engine/tests/pdc.rs`, `two_tracks_one_through_a_latency_device_null_at_main`, `the_same_through_a_group_lines_up`, `the_same_through_a_send_lines_up` | golden render |
| R1 | `crates/engine/tests/pdc.rs`, `switching_the_latency_device_off_and_on_keeps_its_compensation` | golden render |
| R3 | `crates/engine/tests/pdc.rs` (clips held back in every aligned case), `a_midi_tracks_notes_are_held_back_to_line_up_with_its_sidechain` | golden render |
| R4 | `crates/engine/tests/pdc.rs`, `a_sidechain_keyed_from_another_track_lines_up` | golden render |
| R5 | `crates/engine/tests/pdc.rs`, `automation_lands_on_the_compensated_sample`, `send_automation_lands_on_the_compensated_sample`, `chain_selector_automation_lands_on_the_compensated_sample` | golden render |
| R6 | `crates/engine/tests/pdc.rs`, `a_latency_change_recompiles_and_the_compensation_follows`, `a_node_whose_latency_moves_is_published_until_it_is_reported`, `a_recompile_keeps_the_audio_in_a_routes_delay`, `a_recompile_keeps_the_clips_in_a_tracks_delay`, `a_recompile_keeps_the_notes_a_midi_track_holds_back`; `crates/engine/src/program.rs`, `a_shorter_note_delay_keeps_the_notes_it_adopts_in_order`; every block in `crates/engine/tests/pdc.rs` runs under the allocation trap | golden render |
| R7 | `crates/engine/tests/pdc.rs`, `the_delay_compensation_switch_holds_nothing_back_and_disables_track_delay` | golden render |
| R8 | `crates/engine/tests/pdc.rs`, `reduced_latency_monitoring_gives_a_monitored_track_no_compensation`, `an_armed_auto_track_playing_its_clip_keeps_its_compensation_under_reduced_latency`, `an_armed_auto_track_hearing_its_input_reaches_main_unheld_under_reduced_latency` | golden render |
| R9 | `crates/engine/tests/pdc.rs`, `keep_monitoring_latency_gives_a_monitored_tracks_compensation_back` | golden render |
| R10 | `crates/engine/tests/pdc.rs`, `offline_reports_how_far_main_lags_song_time`, `an_offline_render_follows_a_latency_reported_after_the_first_block`; `crates/shell/tests/export.rs`, `an_export_starts_on_its_first_frame_through_a_lookahead_on_main`; `crates/shell/tests/freeze.rs`, `a_frozen_track_lines_up_with_an_unfrozen_neighbour` | golden render, shell test |

## Not built yet

- A plug-in's latency counts from the compile that follows its activation, so it can read as 0 on the first compile; native devices report at creation and compensate fully. A VST3 plug-in that changes its latency asks for a restart, the host reports the new latency to the shell, which tells the engine, and the compiler compiles again with the device's node kept and the new latency in its slot. CLAP latency changes are not reported to the engine yet.
- A latency change that forces a recompile starts the delay lines whose length changed empty, so it can click; the old and new delays are not crossfaded.
- R5's offset applies to parameter changes sent for a song frame; a scheduled command's `Action::Param` fires at the song position directly and is not shifted yet.
