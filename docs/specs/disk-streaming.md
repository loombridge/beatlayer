# Disk streaming

- **Status:** implemented
- **Work packages:** WP-1.3 (streaming, preloaded clip starts and cues, underrun reporting, click-free jumps). The clip player that reads the streams is WP-1.4's, streaming since WP-1.4b (the audio clips spec, R17 to R21), the transport that locates is WP-1.5, the disk overload indicator in the Control Bar is the app shell's, and the clip RAM switch belongs with the other audio clip properties (M4 in the parity matrix).
- **Parity-matrix row:** Mixing and routing: CPU meter, disk overload indicator; Audio clips and warping: Clip pitch, reverse, RAM mode, high-quality interpolation (RAM mode)
- **Checked against:** the Live 12 reference manual (online edition), read on 10 October 2026; the UI parity reference's observations of Live 12.4.3 on macOS, 10 October 2026

## The behaviour

Live plays audio clips from disk as they play, rather than loading every file into memory first. Users see this only when it goes wrong: if the disk can't keep up, the audio has gaps, and the disk overload indicator in the Control Bar flashes. A clip's RAM switch loads that clip into memory instead, at the cost of memory and loading time. Nothing else about playback changes: a streamed clip sounds exactly as it would from memory, starts on time, and jumps when the song is located elsewhere without a click.

Beatlayer does the same, with one design choice of its own: short files (under 10 seconds) always load into memory, where they cost less than a stream and one decoded copy serves every clip that plays them. Longer files stream. A disk thread decodes each stream ahead of the playhead; the start of every clip, and any point a jump is expected to land on, such as a loop's start, is decoded in advance, so a launch or a locate there never waits for the disk. A jump anywhere else asks the disk thread for the new position and plays silence for the few milliseconds that takes, fading the old audio out and the new audio in.

This spec covers streaming, what an underrun does, and how a jump sounds. Where a locate lands (launch quantisation, clicking in the scrub area) is WP-1.5's transport spec; clip fades and loops are WP-1.4's.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, "Clip View" chapter, "Audio Clip Properties" section, on clip RAM mode (the UI parity reference cites it as section 8.4) | Live normally streams an audio clip's samples from disk while it plays. The RAM switch loads the clip into memory instead, which can help when the disk is too slow for the number of clips playing, but uses memory and takes longer to load. |
| S2 | Manual | Live 12 reference manual, "Computer Audio Resources and Strategies" chapter, the section on the disk load | When the disk can't read or write audio fast enough, playback or recording drops out and the Control Bar's disk overload indicator flashes. Fewer streams, RAM mode, or a faster disk are the remedies it suggests. |
| S3 | Observation | Live 12.4.3 on macOS 26.6, 10 October 2026, recorded as layout row L-CTRL-025 in [the UI parity reference](../ui-parity/layout.md) | The Control Bar shows an overload (disk) indicator beside the CPU meter, always. |
| S4 | Manual | Live 12 reference manual, Settings chapter, Record, Warp & Launch (section 2.3.9, as L-SET-018 cites it), "Create Fades on Clip Edges"; and the "Clip View" chapter's clip fade controls | Live puts short fades on clip edges to avoid clicks where audio starts or stops abruptly. |
| S5 | Beatlayer | [Architecture](../02-architecture.md), "Processes and threads", "Media and warping", and "Real-time rules" | A disk thread streams long files ahead of the playhead; short files load into memory; the audio thread never waits, locks, or allocates. |

## Rules

- **R1.** A streamed file plays exactly the samples decoding the whole file gives, at the engine's rate: bit for bit at the file's own rate, in every format Beatlayer reads, at any block size, from any start, and after any jump once its fade has ended. A file at another rate plays what converting the whole file gives. (S1, S5)
- **R2.** The start of every streamed clip is decoded before the clip can play, and so is every point cued for a jump (three cue slots by default: the clip's start, and two more, such as a clip loop's start and the arrangement loop's start). A launch, or a locate to a preloaded or cued point, plays at once, never waiting on the disk. (S1, S5)
- **R3.** When the disk thread hasn't delivered audio a playing clip needs, the missing frames play as silence and the playhead doesn't wait. Each stretch of silence counts as one underrun, with its length in frames; the counts are reported for the disk overload indicator and written to the log. When audio returns, it fades in over 4 ms. (S2, S3)
- **R4.** A locate during playback doesn't click. The audio that was playing crossfades to the new position over 4 ms, with gains along a raised cosine that sum to one, so no sample steps further than the audio's own largest step plus the fade's slope (π/2 ÷ 192 at 48 kHz) times the gap between the two signals. Where the new position isn't ready, the old audio still fades out, and the new fades in when it arrives. A clip's loop wrap and a launch cut without a fade, since they must be sample-accurate; the clip's own edge fades (WP-1.4) apply there. (S4)
- **R5.** A jump to a position nothing was ready for is not an underrun: it waits for the disk (up to 2 ms for the disk thread to notice, plus the time to read and decode the new position; 64 streams jumping at once waited 3 to 4 ms on average on an M5 Max) and counts as a miss, with the frames it waited. (S2, S5)
- **R6.** Files shorter than 10 seconds load into memory rather than stream, and play through the same interface, crossfades included. (S1, S5)
- **R7.** On an Apple Silicon Mac, 64 stereo files stream at 64-frame buffers without an underrun or a dropout, through the null device paced in real time. (S5)
- **R8.** An offline render (export, previews, golden tests) reads streams in blocking mode: a read waits for the disk thread instead of playing silence, so a render never underruns and renders the same every time. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/media/tests/stream.rs`, `a_stream_plays_exactly_what_decoding_the_file_gives`, `every_format_streams_bit_exact`, `a_stream_converts_to_the_engine_rate_as_a_whole_file_conversion_does`, `rates_without_a_small_ratio_convert_too`; `crates/media/tests/stream_realtime.rs` (checks samples while playing) | unit |
| R2 | `crates/media/tests/stream.rs`, `the_clip_start_plays_without_the_disk_and_an_underrun_is_counted`, `a_jump_to_a_cued_point_plays_at_once` | unit |
| R3 | `crates/media/tests/stream.rs`, `the_clip_start_plays_without_the_disk_and_an_underrun_is_counted` | unit |
| R4 | `crates/media/tests/stream.rs`, `a_locate_during_playback_doesnt_click`, `a_crossfade_bounds_the_step_even_between_unrelated_levels`; `crates/media/tests/stream_engine.rs`, `a_locate_through_the_transport_doesnt_click` | measurement |
| R5 | `crates/media/tests/stream.rs`, `a_jump_to_a_cued_point_plays_at_once`, `a_locate_during_playback_doesnt_click`; `crates/media/tests/stream_realtime.rs`, `sixty_four_streams_locating_where_nothing_was_cued_wait_only_for_the_disk` | unit, measurement |
| R6 | `crates/media/tests/stream.rs`, `short_files_load_into_memory_and_long_ones_stream`, `a_memory_source_plays_and_crossfades_like_a_stream` | unit |
| R7 | `crates/media/tests/stream_realtime.rs`, `sixty_four_stereo_files_stream_at_64_frame_buffers_without_an_underrun` | measurement |
| R8 | `crates/media/tests/stream.rs`, `a_blocking_source_waits_for_the_disk_instead_of_underrunning` | unit |

The real-time rules hold for every read: each test reads inside `rt_test!`, and `rtsan/tests/stream.rs` runs reads, jumps, crossfades, and an underrun under RealtimeSanitizer (`cargo xtask rtsan`).

## Details

- Files under 10 seconds load into memory whatever a clip's RAM switch says.
- Beatlayer keeps no decoding cache: compressed files stream straight from the file, decoded on the disk thread. The disk thread plays 64 streams of WAV (a quarter of them converted from 44.1 kHz) busy 3% (release) to 6% (debug) of the time on an M5 Max.
- Jump fades use 4 ms, the length of Live's automatic clip-edge fades (S4).
