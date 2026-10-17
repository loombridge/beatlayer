# Waveform peaks

- **Status:** partial (R7 is measured on an Apple M5 Max; no M1 measurement yet)
- **Work packages:** WP-1.2 (peaks: building them, the peak file, the cache and when it's invalidated). Drawing waveforms from them is WP-1.8a (the arrangement) and WP-1.10 (the browser's preview strip).
- **Parity-matrix row:** Audio clips and warping: Analysis files (`.asd`)
- **Checked against:** the Live 12 reference manual (online edition), read on 10 October 2026

## The behaviour

You bring a sample into a set, and Live works out the data it needs to draw the sample's waveform quickly, once, the first time the sample is used. Until that's done, a long sample's waveform is greyed out. Live keeps the result so the next use is instant, and when you edit the sample in another program, Live plays, and so draws, the edited version.

Beatlayer does the same with peaks: per channel, the lowest and highest sample of every short window of the file, at several resolutions. It builds them in the background the first time a file is used, names them by the file's content, and keeps them in the user's cache folder rather than beside the sample. A file whose content changes never shows its old waveform.

Beatlayer notices that a file has changed when the app calls `PeakCache::check`; when the app calls it (on regaining focus, on opening a set) is for the app shell and WP-1.13. Peaks never hold playback; whether warp analysis should is for auto-warp in M4. Steps of 1/127 resolve down to about −42 dBFS, and quieter material shows as a one-step band.

This spec covers what peaks hold, how they're laid out for the GPU and on disk, when they're built, kept, and dropped, and how fast they build. Drawing them (zoom, colours, the vertical zoom of L-ARR-017) belongs to the UI packages, and the rest of what Live's analysis files hold (warp markers, tempo) to the warping packages in M4.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 5, "Managing Files and Sets", section 5.1.2, "Analysis Files (.asd)" | Live analyses a sample the first time it's added to a set and keeps the result in an `.asd` file beside the sample, partly so the waveform draws quickly. A long sample's waveform stays greyed out, and the sample can't play, until its analysis finishes. With the "Create Analysis Files" setting off, no files are written and samples are analysed each time they load. |
| S2 | Manual | Same chapter, section 5.1.1, "The Decoding Cache" | Live keeps files it derives from samples in a cache folder of its own, deletes old ones to stay under a size limit, and offers a cleanup. |
| S3 | Manual | Live 12 reference manual, chapter 8, "Clip View", section 8.4.3, "Destructive Sample Editing" | After you edit a sample in an external editor, Live plays the edited version when you come back, and the edit reaches every clip that uses the sample. |
| S4 | Beatlayer | [Parity matrix](../04-parity-matrix.md), "Audio clips and warping", the analysis-files row | Beatlayer keeps its own analysis cache and doesn't read `.asd` files. |
| S5 | Beatlayer | [Architecture](../02-architecture.md), "UI" and "Media and warping"; ADR-001; the egui spike's waveform shader, `spikes/ui-egui/src/dense.wgsl`; [the build plan](../03-build-plan.md), WP-1.2 | Waveforms are drawn on the GPU straight from peak data, which the spike's shader reads as `[min, max]` `i8` pairs packed in a storage buffer, at 256 samples per peak and coarser levels merging four. Peaks are keyed by content hash. An hour of audio builds in under 5 seconds on an M1. |

## Rules

- **R1.** The first time media's peaks are asked for, they're built in the background: asking never waits, and until they're ready it says how far the build has got, so the UI can show the waveform as pending. Building peaks never holds up playback. (S1)
- **R2.** Peaks hold, for each channel, the lowest and highest sample of every 256-frame window (level 0, whose last window covers what's left), then levels that each merge four peaks of the level below, down to a level of one peak. Each value is an 8-bit step of 1/127 of full scale, the minimum rounded down and the maximum rounded up, clamped to ±127: a window with any signal shows at least one step, and only digital silence draws as a flat line. Every level equals rounding its own windows' extremes. (S5)
- **R3.** Peaks are named by the media's content hash and saved as peak files in the user's cache folder, never beside the sample: `~/Library/Caches/io.loombridge.beatlayer/Peaks` on macOS, `%LOCALAPPDATA%\Beatlayer\Cache\Peaks` on Windows, and `$XDG_CACHE_HOME/beatlayer/peaks` (or `~/.cache/beatlayer/peaks`) on Linux. A later use of the same content, from any path, project, or session, loads the file instead of decoding. With no cache folder, the equivalent of Live's "Create Analysis Files" off, peaks are kept in memory and built again in each session. (S1, S2, S4)
- **R4.** A peak file is used only if it's whole, of the current format version, and built from the media whose hash names it; otherwise the peaks are built again and the file replaced. (S5)
- **R5.** A changed source never shows its old peaks. Peaks are served only after the media store confirms the file still holds the content they were built from; a build whose file changes while it's read gives no peaks; checking media whose file has changed drops its peaks from memory and deletes its peak file. The edited file is new media, and its peaks are built from its new content. A missing file's peaks leave memory but stay on disk until the file is found. (S3)
- **R6.** Peaks built in parallel are identical to peaks built by decoding in order. A long file decodes in segments, each after a sample-accurate seek ([audio import](audio-import.md) R7); a header that misstates the file's length makes the build decode in order instead. (S5)
- **R7.** An hour of 48 kHz stereo builds in under 5 seconds on an M1, in every format Beatlayer imports. (S5)
- **R8.** The peak data uploads to the waveform shader without conversion, in the layout below. (S5)

### The peak data (R2, R8)

`Peaks::data` is one block, which the shader reads as `array<u32>` in a storage buffer:

- A peak is two bytes, `[min, max]`, each an `i8`. Two peaks share a little-endian `u32`, so peak *i* sits in word *i* / 2 at bit 16 × (*i* mod 2), as `peak()` in `dense.wgsl` reads it.
- The block holds one run per level and channel, in level order and, within a level, channel order: level 0 channel 0, level 0 channel 1, ..., level 1 channel 0, and so on. `Peaks::run(level, channel)` gives each run's offset and length in peaks from the block's start.
- Every run starts on a word: a run of odd length is followed by one zero peak. A block's length is therefore a multiple of four bytes, and blocks from several files can sit one after another in one buffer. Uploaded at byte offset *b*, a run's first peak is at peak index *b* / 2 + offset.
- Peak *i* of level *k* covers frames *i* × 256 × 4<sup>*k*</sup> up to, not including, (*i* + 1) × 256 × 4<sup>*k*</sup>. Coarse levels come last, so a UI short of GPU memory can upload the levels from *k* up as one range.
- The UI picks the coarsest level with at least one peak per pixel (`Peaks::level_for`), so each pixel column reads one to four peaks. Zoomed in below 256 frames per pixel, it draws from the samples.

An hour of 48 kHz stereo has 11 levels and 3,600,040 bytes of peaks.

### The peak file, version 1 (R3, R4)

A peak file is `<cache folder>/<first two hex digits of the hash>/<hash>.blpeaks`: a header, then the data block exactly as above, so loading one is a read and a check. Numbers are little-endian.

| Offset | Size | Field |
|---|---|---|
| 0 | 8 | Magic: `BLPEAKS` and a zero byte |
| 8 | 4 | Format version, `u32`: 1 |
| 12 | 4 | Header length in bytes, `u32`, a multiple of 16: where the data block starts |
| 16 | 32 | The source's content hash |
| 48 | 8 | Frames in the source, `u64` |
| 56 | 4 | Sample rate, `u32` |
| 60 | 2 | Channels, `u16` |
| 62 | 2 | Levels, `u16` |
| 64 | 4 | Frames per peak at level 0, `u32`: 256 |
| 68 | 4 | Peaks merged per level, `u32`: 4 |
| 72 | 8 | Data block length in bytes, `u64` |
| 80 | 32 | Checksum: the BLAKE3 hash of the whole file, with these 32 bytes read as zero |
| 112 | 16 | Reserved, zero |
| 128 | 8 per run | The runs, in the data block's order: offset and length in peaks, two `u32`s each |
| header length | data length | The data block |

A reader rejects a file whose magic, version, constants, source hash, run table, lengths, or checksum aren't what it expects. The version goes up whenever the layout or the way peaks are computed changes, so old files are rebuilt rather than misread. Files are written under a temporary name and renamed into place. `PeakCache::prune` deletes the least recently used files over a size limit, as Live's decoding cache does (S2).

### Build times (R7)

Measured on 10 October 2026 on an Apple M5 Max (6 "Super" and 12 "Performance" cores) with `cargo run --release -p bl-media --example peaks`, best of three runs, with each one-hour file already in the operating system's cache. The 24-bit WAV is the example's own `--write-wav` output; the other files are an hour of noise and a sine from ffmpeg 8.1.1, encoded with ffmpeg's FLAC, libmp3lame at 320 kbit/s, AAC at 256 kbit/s, and Vorbis at quality 6. Times are in seconds.

| One hour, 48 kHz stereo | All 18 cores | 8 threads | 4 threads | 1 thread |
|---|---|---|---|---|
| WAV, 24-bit (1.04 GB) | 0.043 | 0.066 | 0.105 | 0.344 |
| WAV, 16-bit (691 MB) | 0.031 | 0.050 | 0.088 | 0.282 |
| FLAC, 24-bit (394 MB) | 0.065 | 0.134 | 0.247 | 0.912 |
| MP3, 320 kbit/s (144 MB) | 0.205 | 0.279 | 0.459 | 1.603 |
| AAC in MPEG-4, 256 kbit/s (116 MB) | 0.126 | 0.232 | 0.405 | 1.456 |
| Ogg Vorbis, quality 6 (90 MB) | 0.144 | 0.260 | 0.444 | 1.622 |

Every parallel build was identical to the one-thread build (`--verify`). Through the whole background path (`--cache`), importing the 24-bit WAV, which hashes it, took 0.42 s, the cache's build and save 0.044 s, and loading the 3.6 MB peak file in a new cache 0.002 s.

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/media/tests/peaks.rs`, `peaks_build_in_the_background_and_are_saved_by_content_hash` | unit |
| R2 | `crates/media/tests/peaks.rs`, `every_fixture_gives_the_peaks_of_its_decoded_samples`, `the_last_peak_covers_what_is_left`, `only_digital_silence_draws_flat`; `crates/media/src/peaks/build.rs`, `quantising_rounds_outwards`; `crates/media/src/peaks/mod.rs`, `levels_above_zero_merge_four_peaks` | unit |
| R3 | `crates/media/tests/peaks.rs`, `peaks_build_in_the_background_and_are_saved_by_content_hash`, `a_new_session_loads_the_peak_file_instead_of_decoding`, `a_memory_only_cache_writes_no_files`; `crates/media/src/peaks/cache.rs`, `the_cache_folder_follows_the_platform` | unit |
| R4 | `crates/media/src/peaks/file.rs`, `rejects_anything_not_whole_and_current`; `crates/media/tests/peaks.rs`, `a_damaged_or_foreign_peak_file_is_rebuilt` | unit |
| R5 | `crates/media/tests/peaks.rs`, `editing_the_source_invalidates_its_peaks`, `a_later_session_never_shows_the_peaks_of_an_edited_file`, `a_file_edited_during_a_build_gives_no_peaks`, `missing_media_has_no_peaks_until_it_is_found` | unit |
| R6 | `crates/media/tests/peaks.rs`, `every_fixture_gives_the_peaks_of_its_decoded_samples`, `a_header_that_overstates_the_length_falls_back_to_decoding_in_order`; on one-hour files, `--verify` in `crates/media/examples/peaks.rs` | unit, measurement |
| R7 | `crates/media/examples/peaks.rs` (the table above); not yet run on an M1 | measurement |
| R8 | `crates/media/tests/peaks.rs`, `the_data_block_reads_the_way_the_waveform_shader_reads_it`; `crates/media/src/peaks/mod.rs`, `runs_are_level_major_and_word_aligned`; `crates/media/src/peaks/file.rs`, `the_header_is_laid_out_as_documented` | unit |
