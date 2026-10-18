# MIDI clips in the Arrangement: placement, region, loop, notes, and playback

- **Status:** implemented
- **Work packages:** WP-2.1, WP-2.21
- **Parity-matrix row:** MIDI clips and editing: Velocity, release velocity, probability, velocity deviation, note mute; Views and workflow: Arrangement View
- **Checked against:** the Live 12 reference manual (online edition), read October 2026 (chapter 8 "Clip View", section 8.2.1 "Clip and Loop Region Settings", section 8.9 "Looping Clips", section 8.1.1.1 "Clip Activator Toggle", and section 42.12 "MIDI Note Probability and Velocity Range"); the Standard MIDI Files 1.0 specification (MIDI Manufacturers Association); Live 12.4.6 on macOS observed 11 October 2026 (S15, trimming and resizing a MIDI clip); and Live 12.4.3 on macOS observed 15 October 2026 (S8, MIDI file import).

## The behaviour

A MIDI clip sits on a MIDI track's lane in the Arrangement, at a musical position and for a musical length. While the song plays, the clip plays its notes from the clip's Start marker, with note-ons and note-offs dispatched sample-accurately through the track's device chain. With its Loop switch off, it plays until its End marker or until the clip ends in the Arrangement, whichever comes first. With the loop on, it starts at its Start marker, plays into the loop brace, and repeats the brace for as long as the clip lasts.

Each note in a MIDI clip has a pitch (MIDI key 0 to 127), start position in ticks, length in ticks, velocity (1 to 127), release velocity (1 to 127, default 64), probability (0.0 to 1.0, default 1.0), velocity deviation (-127 to 127, default 0), mute switch, and its own pressure points ([midi-controller-data.md](midi-controller-data.md) R2). Playback rolls note probability and velocity deviation per play from an allocation-free PRNG on the audio thread. Muted notes and notes on muted clips never sound. When playback stops, when a loop wraps, or when a sounding clip is deleted or edited, note-offs are sent immediately for all sounding notes so no note is ever stuck.

The engine plays each MIDI track's clips with a MIDI clip player at the head of the track's device chain. The compiler sends the track's clips as node data, so creating, moving, or editing a clip never builds a new program.

This spec covers Arrangement playback, data, and file import and export of MIDI clips. It leaves to other specs and packages: the Piano Roll and MIDI note editing UI (WP-2.3); MIDI clip groove and quantization (Phase 4).

A MIDI clip's data also carries the Clip View's Signature (`MidiData.signature`): the time signature its End is measured in, `null` for the song's signature at the clip's position. `midi.import_file` sets it from the file and `midi.export_clip` writes it (R16, R19); the Clip View's Signature numerator and denominator controls (L-CLIP-004) aren't drawn yet.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 8 "Clip View", section 8.2.1 "Clip and Loop Region Settings", and section 8.9 "Looping Clips" | A clip has Start and End markers and a loop brace (Position and Length) with a Loop switch. An unlooped clip plays from its start to its end. A clip always begins at its Start marker wherever the loop brace is, so it can play into its loop. The loop's length defaults to the clip's whole length. For MIDI clips, the markers read bars, beats, and sixteenths (ticks). |
| S2 | Manual | Live 12 reference manual, section 8.1.1.1 "Clip Activator Toggle", and section 8.3 | The clip activator switches a clip off without deleting it. Arrangement clips aren't launched: they play by their position on the timeline. |
| S3 | Manual | Live 12 reference manual, section 42.12 "MIDI Note Probability and Velocity Range" | Each MIDI note has a Probability value from 0 % to 100 %. Live rolls chance each time the note is reached; a 0 % note never plays, a 100 % note always plays. Velocity Range (velocity deviation) defines an offset range from nominal velocity; when played, Live randomly picks a velocity in that range. Release velocity sets the note-off velocity. Notes can be muted individually. |
| S4 | Observation | Live 12.4.6 on macOS, 11 October 2026, recorded in UI parity notes (S15) | On a looped MIDI clip, trimming the left edge moves the Start marker by the amount trimmed, leaving the loop brace; trimmed past the loop's end, the Start marker wraps inside the brace by whole lengths of it. On an unlooped MIDI clip, dragging the right edge past the End marker takes the End marker with it. |
| S5 | Spec | [audio-clips.md](audio-clips.md) | Arrangement placement, clip player node, node data delivery without recompile, real-time safety, stop and loop wrap handling. |
| S6 | Spec | [transport.md](transport.md) | Playback starts, jumps, and wraps at the loop on exact frames through the tempo map. |
| S7 | Beatlayer | [02-architecture.md](../02-architecture.md), "Real-time rules" | No allocation, lock, or blocking call on the audio thread. Preallocated buffers for sounding and emitted notes. |
| S8 | Observation | Live 12.4.3 on macOS, 15 October 2026 | The tempo prompt and what Yes and No do to the song's tempo, its automation and the time-signature markers; one clip per track chunk with notes, on new MIDI tracks or a given one; the clip's name, start, Signature, End and loop; the chunk's leading silence staying inside the clip. |
| S9 | Standard | Standard MIDI Files 1.0 (MIDI Manufacturers Association, 1996) | A file's header names its format (0, 1, or 2) and its time division in ticks to the quarter note. Each track chunk holds delta-timed events; a note-on with velocity 0 is a note-off; 120 bpm and 4/4 are the defaults before a file's tempo and time-signature meta events. |

## Rules

- **R1.** A MIDI clip plays only while the song plays, from the frame its start falls on through the tempo map to the frame its end falls on. Before its start and from its end on, it emits no notes. A deactivated clip emits no notes. (S2, S5, S6)
- **R2.** Notes within a clip are triggered with sample accuracy: each note-on and note-off lands on the exact sample frame computed by converting its song tick position through the tempo map at the engine's sample rate. (S1, S6)
- **R3.** With its loop off, a clip plays from its Start marker until its End marker or the clip's end, whichever comes first. Notes outside this region do not sound. (S1)
- **R4.** With its loop on, a clip begins at its Start marker, plays into the loop brace, and repeats the loop brace for the duration of the clip. The Start marker must be before the loop brace's end. (S1, S4)
- **R5.** Each note play evaluates note probability on the audio thread using an allocation-free PRNG: probability 0.0 never sounds, probability 1.0 always sounds, and probability 0.5 sounds approximately half the time over repeated iterations with a fixed seed. (S3, S7)
- **R6.** When a note plays, its velocity is selected within its velocity deviation range: nominal velocity to nominal velocity plus deviation, clamped to 1..=127. (S3)
- **R7.** When a note ends, its note-off event carries the note's release velocity (1..=127, default 64) normalized to 0.0..=1.0. (S3)
- **R8.** Muted notes (`mute == true`) never sound. (S3)
- **R9.** When the transport stops, when a loop wraps, or when a sounding clip is deleted or edited, note-off events are emitted immediately for all currently sounding notes. No note remains stuck. (S5, S7)
- **R10.** Every clip edit is a registered command in the `edit` class: `clip.set_midi_region`, `clip.set_midi_loop`, `clip.set_loop_enabled`, `clip.set_data`. Edits reach the engine as node data without recompiling the program. (S5, S7)
- **R11.** All audio-thread operations of the MIDI clip player are zero-allocation and lock-free, using preallocated note event buffers. (S7)
- **R12.** A Standard MIDI File of type 0 or 1 imports as one MIDI clip per track chunk that holds any note, in the file's order. A chunk with no notes makes nothing: a conductor track of tempo and time signature, or a chunk with only a name, is not imported. A type 0 file's single chunk becomes one clip. (S8)
- **R13.** A file's tempo events read into a tempo map and its time-signature events into a time-signature map: 120 bpm and 4/4 before the first event of each kind, the standard's defaults. A note-on with velocity 0 ends a note, a note-off's velocity is the note's release velocity (64 when 0 or missing), and a note left open at the end of a track is closed there. (S8, S9)
- **R14.** With no target track, each track chunk that has notes gets a new MIDI track below the last top-level track, named as a new track is (`<n> MIDI`, n the count of tracks plus one). With a target MIDI track, the clip goes on that track and the track's name is unchanged; a file with more than one track of notes is refused rather than guessed. (S8)
- **R15.** A clip starts where the file is inserted, at the Arrangement's insert marker or the drop position, and its contents begin at the file's start: leading silence stays inside the clip. It is named after its chunk's Track Name event, or `Track <n>` with n the chunk's index from 0. (S8)
- **R16.** The clip's Signature is the file's time signature, whatever the answer to the tempo question. Its End is its last note-off rounded up to a whole bar of that signature, and it is at least one bar long. Loop is on, with Position 1.1.1 and Length equal to the clip. (S8)
- **R17.** `midi.import_file` reads a Standard MIDI File at `path` with `import_tempo` off unless it is given, putting the file's tracks at `at` (the insert marker when it is left out) on `track`, or on new MIDI tracks. Placed over other clips, each clip replaces what it covers, as a drop from the browser does (arrangement-editing R6). It is one undo step and changes nothing when the file can't be read or holds no notes. (S8)
- **R18.** With `import_tempo`, a file whose tempo is steady sets the song's tempo everywhere, writing no automation. A file whose tempo changes writes Song Tempo automation over the clip's span, placing the file's tempo map at the clip's position, and restores the tempo in force before the clip at the clip's end. Either way it writes a time-signature marker with the file's signature at the clip's start and one restoring the signature in force before the clip at the clip's end, even when the two are the same. Without it, the song's tempo, automation and markers are untouched. (S8)
- **R19.** `midi.export_clip` writes the clip's notes as a type 1 Standard MIDI File at `path`, with one tempo event carrying the song's tempo where the clip starts and the clip's Signature as the time signature, both at tick 0. It refuses an audio clip, and a file already at `path` unless `replace` is true. (decided 15 October 2026)
- **R20.** A MIDI file over 64 MiB is refused from its length, before it's read. (decided 18 October 2026)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | `crates/engine/tests/midi_clips.rs`, `note_timing_to_the_sample_at_two_tempos` | unit |
| R3, R4, R9 | `crates/engine/tests/midi_clips.rs`, `note_timing_across_a_loop_wrap`, `no_stuck_notes_after_stop_or_clip_deletion` | unit |
| R5 | `crates/engine/tests/midi_clips.rs`, `probability_0_1_and_50_percent` | unit |
| R6 | `crates/engine/tests/midi_clips.rs`, `velocity_deviation_stays_in_range` | unit |
| R7 | `crates/engine/tests/midi_clips.rs`, `release_velocity_reaches_the_instrument` | unit |
| R8 | `crates/engine/tests/midi_clips.rs`, `muted_notes_and_muted_clips_never_sound` | unit |
| R10 | `crates/engine/tests/midi_clips.rs`, `midi_clip_round_trip`, `midi_clip_commands` | unit |
| R11 | `crates/engine/tests/midi_clips.rs` (under allocation trap), `rtsan/tests/clips.rs` | unit, RealtimeSanitizer |
| R12 | `crates/interop/tests/midi_files.rs`, `a_type_0_file_reads_as_one_track_of_notes`, `a_type_1_file_reads_as_a_track_each`; `crates/shell/tests/midi_files.rs`, `a_type_0_file_becomes_one_new_midi_track`, `a_type_1_file_makes_a_track_per_chunk_with_notes` | unit |
| R13 | `crates/interop/tests/midi_files.rs`, `a_file_with_tempo_changes_reads_a_tempo_map`, `note_on_with_velocity_zero_and_unclosed_notes_close` | unit |
| R14 | `crates/shell/tests/midi_files.rs`, `a_type_1_file_makes_a_track_per_chunk_with_notes`, `a_clip_goes_on_a_given_midi_track_and_a_multi_chunk_file_is_refused` | unit |
| R15 | `crates/interop/tests/midi_files.rs`, `a_clip_round_trips_through_a_written_file`; `crates/cli/tests/agent_midi_files.rs`, `an_agent_imports_and_exports_midi_files` | unit |
| R16 | `crates/shell/tests/midi_files.rs`, `a_type_0_file_becomes_one_new_midi_track`, `import_tempo_writes_time_signature_markers` | unit |
| R17 | `crates/shell/tests/midi_files.rs`, `a_clip_goes_on_a_given_midi_track_and_a_multi_chunk_file_is_refused`, `an_import_over_a_clip_replaces_what_it_covers`, `a_bad_file_changes_nothing`; `crates/cli/tests/agent_midi_files.rs`, `an_agent_imports_and_exports_midi_files`, `without_autonomous_a_midi_file_import_is_refused` | unit |
| R18 | `crates/shell/tests/midi_files.rs`, `import_tempo_sets_the_song_tempo_for_a_single_tempo_file`, `import_tempo_writes_automation_for_a_changing_tempo`, `import_tempo_writes_time_signature_markers` | unit |
| R19 | `crates/shell/tests/midi_files.rs`, `a_clip_round_trips_through_export_and_import` | unit |
| R20 | `crates/interop/tests/midi_files.rs`, `a_file_past_the_size_cap_is_refused_before_it_is_read`, `bytes_past_the_size_cap_are_refused`; the `midi_file` fuzz target | unit, fuzz |
