# Audio import

- **Status:** implemented
- **Work packages:** WP-1.1 (decoding, conversion, content hashing, the media store). Dragging files in is WP-1.10; disk streaming is WP-1.3.
- **Parity-matrix row:** Files and export: Audio import: WAV, AIFF, AIFF-C, FLAC, Ogg Vorbis, MP3, M4A (AAC, ALAC), RF64 and BWF over 4 GB
- **Checked against:** the Live 12 reference manual (online edition), read on 10 October 2026

## The behaviour

You bring an audio file into a set, and Live plays it as it is: an uncompressed file of any length, sample rate, or bit depth loads without being converted first, and a compressed file is decoded for you. Beatlayer does the same with every format in the parity-matrix row, on every platform, including M4A, which Live imports only on macOS.

This spec covers what a file decodes to: which formats load, how exactly their samples come back, how a file's sample rate is converted to the engine's, and how Beatlayer keeps track of a file by its content so it can find it again after it moves. It doesn't cover the browser, dragging to tracks, or mapping a multichannel file's channels to tracks, which belongs to a later package. Beatlayer decodes every channel of a multichannel file. It decodes short files into memory and streams long ones straight from the file, decoding on the disk thread, so it keeps no cache of decoded files ([disk streaming](disk-streaming.md)).

Beatlayer's own rules sit alongside Live's here, because "decodes the file" needs a measurable meaning: the tolerances below are Beatlayer's, not Live's.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 5, "Managing Files and Sets", section 5.1, "Sample Files" | Live plays mono and stereo WAV, AIFF, AIFF-C, FLAC, and Ogg Vorbis, and imports MP3 and M4A (M4A only on macOS). Uncompressed files of any length, sample rate, or bit depth load without conversion. |
| S2 | Manual | Same chapter, section 5.1.1, "The Decoding Cache" | Live decodes compressed files to uncompressed temporary files when they're imported, and manages that cache's size and location in the File & Folder settings. |
| S3 | Manual | Same chapter, section 5.6, "Locating Missing Files" (5.6.1 manual repair, 5.6.2 automatic repair) | Live searches chosen folders, the project, and the library for missing samples and offers candidates. The manual doesn't say whether it matches by name or by content. |
| S4 | Beatlayer | [Parity matrix](../04-parity-matrix.md), "Files and export"; [architecture](../02-architecture.md), "The document" and "Project files and interop" | Beatlayer also reads RF64 and BWF files over 4 GB, and M4A on every platform. Clips name media by content hash, and a sample reference holds a path, a hash, and a size, so missing files can be relinked by content. |

## Rules

- **R1.** Beatlayer decodes WAV, AIFF, AIFF-C, FLAC, Ogg Vorbis, MP3, and M4A (AAC and ALAC), on every platform. (S1, S4)
- **R2.** An uncompressed file loads at its own sample rate, bit depth, and length: 8-bit unsigned to 32-bit integer and 32- and 64-bit float PCM, including RF64 and BW64 (Broadcast Wave) files over 4 GB. Nothing is converted or rewritten when a file is imported. (S1, S4)
- **R3.** Lossless sources decode bit-exact. PCM in any container, FLAC, and ALAC give back exactly the encoded samples, scaled so a *b*-bit integer sample *s* becomes *s* / 2<sup>*b*−1</sup> as a 32-bit float. (S1)
- **R4.** A lossy file decodes to the length and timing of the audio that was encoded: the encoder's delay and padding are removed, using LAME's tag for MP3, granule positions for Ogg Vorbis, and the `iTunSMPB` tag or the edit list for AAC in MPEG-4. A raw ADTS stream can't record them, so it keeps the encoder's priming (2,112 frames from Apple's encoder). (S1)
- **R5.** A lossy file decodes within its format's tolerance (the table below): its signal-to-error ratio below 15 kHz is at least the limit for its codec and signal; digital silence stays below −90 dBFS; every sample is finite; and peaks over full scale are kept, not clipped. (S1)
- **R6.** Converting between sample rates keeps aliasing and imaging below −100 dB from 0 Hz to 90% of the lower Nyquist frequency, and passband tones within 0.1 dB of their level. (S1, S2: a file plays at the engine's rate whatever its own)
- **R7.** Seeking is sample-accurate in every format: reading after a seek gives the same samples as decoding from the start. (S1)
- **R8.** A damaged or foreign file never crashes Beatlayer. A file cut short decodes as far as it goes (MPEG-4 excepted, which is an error); a file damaged in the middle either decodes, with only finite samples, or is an error; and a file that isn't audio is an error. Where the decoder rejects a packet the demuxer could still frame, the packet becomes silence of its length, so what follows stays in time. Symphonia 0.6 stops a FLAC file at a damaged frame rather than finding the next one, and drops a damaged MP3 frame rather than handing it to the decoder, so the rest of the file comes early. (S1)
- **R9.** Media is identified by the BLAKE3 hash of the file's bytes. Before using a file, the media store checks it still holds that content, and reports it missing or changed if not. (S3, S4)
- **R10.** A missing file is found again by content: Beatlayer searches the chosen folders and their subfolders, hashes only files of the recorded length, and relinks the first match, whatever its name. (S3, S4)

### Format tolerances (R5)

Signal-to-error ratio below 15 kHz, in dB, of each lossy fixture against its source, as measured on 10 October 2026, and the limit the tests hold it to. Encoders low-pass the top octave on purpose, which is why the ratio is measured below 15 kHz. The fixtures and their encoders are listed in `tests/fixtures/media/manifest.json`.

| Codec and encoder | Sine | Sweep | Noise | Clipped | Silence |
|---|---|---|---|---|---|
| MP3, LAME 3.100, 320 kbit/s CBR and V0 | 77.2 and 71.5 (limit 65) | 78.6 (limit 70) | 17.3 (limit 14) | 37.4 (limit 32) | digital zero (limit −90 dBFS) |
| Vorbis, libvorbis through libsndfile 1.2.2, quality 0.9 | 41.0 (limit 36) | 36.9 (limit 32) | 20.0 (limit 16) | 40.6 (limit 35) | digital zero (limit −90 dBFS) |
| AAC-LC, afconvert on macOS 26.6, 256 kbit/s | 41.5 and 39.8 (limit 35) | 48.5 (limit 43) | 16.2 (limit 12) | 42.6 (limit 37) | digital zero (limit −90 dBFS) |
| PCM, FLAC, ALAC | bit-exact | bit-exact | bit-exact | bit-exact | bit-exact |

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/media/tests/decode.rs`, `every_format_has_fixtures`, `headers_report_each_file` | unit |
| R2 | `crates/media/tests/decode.rs`, `headers_report_each_file`; `crates/media/tests/rf64.rs`, `reads_bw64_and_rf64_over_4_gb`, `opens_a_sparse_file_over_4_gb_from_disk` | unit |
| R3 | `crates/media/tests/decode.rs`, `source_wavs_decode_to_their_exact_samples`, `lossless_fixtures_decode_bit_exact` | unit |
| R4 | `crates/media/tests/decode.rs`, `lossy_fixtures_decode_within_tolerance` (frame counts) | unit |
| R5 | `crates/media/tests/decode.rs`, `lossy_fixtures_decode_within_tolerance` | measurement |
| R6 | `crates/media/tests/resample.rs`, `aliasing_and_imaging_stay_below_minus_100_db_in_the_passband`, with its control `the_measurement_catches_a_converter_that_aliases` | measurement |
| R7 | `crates/media/tests/decode.rs`, `seeking_lands_on_the_exact_frame` | unit |
| R8 | `crates/media/tests/decode.rs`, `damaged_and_foreign_files_fail_cleanly`; `crates/media/tests/rf64.rs`, `a_file_shorter_than_its_header_says_keeps_what_it_has` | unit |
| R9 | `crates/media/tests/store.rs`, `import_hashes_the_bytes_and_reads_the_header`, `notices_missing_and_changed_files` | unit |
| R10 | `crates/media/tests/store.rs`, `relinks_a_moved_file_by_content`, `relinks_every_missing_file_in_one_pass` | unit |
