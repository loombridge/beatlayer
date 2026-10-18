# Stem separation

- **Status:** partial
- **Work packages:** WP-9.1 (IMP-11)
- **Parity-matrix row:** Audio clips and warping: Stem separation (vocals, drums, bass, others; fast and high-quality modes; files, clips, time selections; merge stems)
- **Checked against:** the Live 12 reference manual (online edition), chapter 22, read 18 October 2026, and Live 12 on macOS observed on 18 October 2026

## The behaviour

Separate Stems to New Audio Tracks splits an audio clip into its parts (vocals, drums, bass, and everything else) and puts each part on a new audio track, inside a new group, as a copy of the clip playing that part. The original clip is deactivated, so the set sounds the same. A dialog chooses the parts, whether two or three of them go on one track together, and High Speed or High Quality. Separation runs on the computer, with a Demucs network through ONNX Runtime.

Beatlayer doesn't ship the network: the first time, the dialog lists the open-source packages it needs, with their project pages, sizes, and where they install, and downloads them when asked, with a progress bar. After that, the command goes straight to its options.

This package builds the command for Arrangement clips, the dialog, the download, the separation, merging, the files, and the tracks. Separating a file from the browser, a Session clip, or a time selection, the stems' track colours, and copying the source track's devices and automation to the group come in later packages. Running the network on the GPU and its speed target are IMP-11's next step.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 22 "Stem Separation", sections 22.1 and 22.2 | A clip in either view, or a file in the browser, can be separated from its menu or the Create menu. A dialog chooses Vocals, Drums, Bass, and Others; Merge to Single Track puts the chosen parts on one track and works with two or three parts only; a quality chooser offers High Speed and High Quality; Separate starts it. Playback stops when separation begins. Each part becomes a clip on a track of its own inside a new group; one part gets a track with no group; a merge gets one track. The source clip is deactivated. Only the audio from the Start marker to the End marker, or through the loop, is separated. Files go in the project's Samples, Processed, Stems folder at 44.1 kHz; a merge writes each part's file and the merged one. |
| S2 | Manual | Section 22.2.1 "Separation Speed vs. Quality" | High Speed separates every part in one pass; High Quality takes longer for better separation. Speed depends on the computer. |
| S3 | Observation | Live 12 on macOS, 18 October 2026, from a screenshot of an audio clip's menu in the Arrangement | After Crop Clip and Reverse Clip(s) and a separator: Convert Harmony to New MIDI Track, Convert Melody to New MIDI Track, Convert Drums to New MIDI Track, Separate Stems to New Audio Tracks; then the clip colours. |
| S4 | Observation | Live 12 on macOS, 18 October 2026, from a screenshot of the dialog | The dialog is titled Separate Stems. It lists Vocals, Drums, Bass, and Others, each with a coloured icon and a check mark when chosen, all four chosen; then a High Speed / High Quality switch, set to High Speed; the note "This action will stop audio."; and Separate and Cancel. |
| S5 | Beatlayer | [05-improvements.md](../05-improvements.md), IMP-11; the architecture's "Agent control" (D17) | Stem separation on this computer, its network run through ONNX Runtime; every action is a registered command with a schema, a description, a permission class, and a readback. |

## Rules

- **R1.** Separate Stems to New Audio Tracks is in an audio clip's menu in the Arrangement, after a separator below Reverse Clip(s), last after the three Convert … to New MIDI Track commands (C-CLIP-ARR-027a to 027d), and in the Create menu (M-CREATE-015). It's enabled with one audio clip selected and opens the Separate Stems dialog, a modal window over the main window (`stems.open`). Esc closes it while nothing is under way. (S1, S3)
- **R2.** The first time, before its packages are installed, the dialog lists each package (ONNX Runtime and the Demucs model): its name and version, linked to its project's page on GitHub, what it does, and its download size; then the total and the folder they install in, `Packages` in Beatlayer's application-data folder; then Download Required Packages Now and Cancel. Beatlayer's own. (S5)
- **R3.** Download Required Packages Now (`stems.install`) downloads the packages not yet installed, on a thread of its own, with a progress bar, what it's doing, and Cancel. Each download must have the size and SHA-256 Beatlayer expects, or nothing of it is installed. From ONNX Runtime's release archive only its shared library and its licence notices are kept. A package shows installed only once its file is whole. A failure shows its reason in the dialog, which goes back to the package list. Beatlayer's own. (S5)
- **R4.** Once the packages are installed, the dialog offers the stems; later, the command opens on them directly. Beatlayer's own. (S5)
- **R5.** The options are, in order, Vocals, Drums, Bass, and Others, each a row with its colour, its name, and a check mark when chosen, all four chosen to begin with; Merge to Single Track, enabled with two or three parts chosen only; the High Speed / High Quality switch, set to High Speed; "This action will stop audio."; and Separate, enabled with at least one part chosen, and Cancel. Merge to Single Track is Beatlayer's own placement: the manual names it but doesn't place it. (S1, S4)
- **R6.** Separate (`stems.separate`) stops playback, if it's playing, then separates on a thread of its own, with a progress bar and Cancel in the dialog. A cancelled separation changes nothing. (S1, S4)
- **R7.** High Speed runs the network once over the clip; High Quality runs it twice, the second time shifted, and averages the two, taking about twice as long. Beatlayer's own method. (S2)
- **R8.** The audio separated is the clip's file from its Start marker to its End marker, or with the loop on, from the earlier of the Start marker and the loop's start to the later of the End marker and the loop's end, at 44.1 kHz. (S1)
- **R9.** With more than one track to make, a new group track named after the clip (or its file, if the clip has no name) goes directly below the clip's track, at the same level, holding a track for each part, named after it, in the order Vocals, Drums, Bass, Others. With one part, or a merge, the one new track goes directly below the clip's track, with no group. A merged track is named for its parts, such as "Drums + Bass". Group naming and order are Beatlayer's own. (S1)
- **R10.** Each part is written as a 32-bit float WAV at 44.1 kHz, named "<clip> <part>.wav", in the project's `Samples/Processed/Stems` folder (or the Temporary Folder's `Stems` folder in a set never saved). A merge writes every chosen part's file and the merged file, and only the merged file gets a track. Each file starts with silence up to the separated region's start, so it lines up with the clip's own file. (S1)
- **R11.** Each new track holds a copy of the clip playing its part: the clip's position, length, markers, loop, warping, gain, fades, and envelopes carry over. The source clip is deactivated. The tracks, the clips, and the deactivation are one undo step, "Separate Stems". (S1)
- **R12.** The network sees the mix normalised by its mono mean and deviation, in windows of 343,980 frames overlapping by a quarter, blended back with triangular weights; each part is scaled back by the same mean and deviation. Progress is the share of windows done. Beatlayer's own method, Demucs's. (S5)
- **R13.** High Quality's second pass sees the mix after a quarter of a second of silence, and its result is shifted back by as much before the two passes are averaged. Beatlayer's own method. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/shell-spec/src/context_menus.rs`, `an_audio_clips_menu_adds_the_convert_and_stems_rows_after_reverse`; `crates/shell/tests/stems.rs`, `the_dialog_opens_on_one_audio_clip`; `crates/ui/tests/stems_dialog.rs`, `an_audio_clips_menu_opens_the_packages_dialog_the_first_time` | unit, UI script |
| R2 | `crates/shell/tests/stems.rs`, `the_first_time_the_dialog_lists_the_packages_to_download`; `crates/ui/tests/stems_dialog.rs`, `an_audio_clips_menu_opens_the_packages_dialog_the_first_time` | unit, UI script |
| R3 | `crates/shell/src/stems.rs`, `only_the_library_and_its_notices_are_kept_from_the_archive`; `crates/shell/tests/stems.rs`, `the_packages_download_check_and_install` (where `BL_STEMS_DOWNLOAD` is set) | unit |
| R4 | `crates/shell/tests/stems.rs`, `the_packages_download_check_and_install` | unit |
| R5 | `crates/ui/tests/stems_dialog.rs`, `an_audio_clips_menu_opens_the_packages_dialog_the_first_time`; `crates/shell/tests/stems.rs`, `merge_needs_two_or_three_parts` | UI script, unit |
| R6 | `crates/shell/tests/stems.rs`, `separating_makes_a_group_of_stem_tracks_and_deactivates_the_clip` | unit |
| R7 | `crates/analysis/src/stems.rs`, `high_quality_averages_a_shifted_second_pass` | unit |
| R8 | `crates/shell/tests/stems.rs`, `separating_makes_a_group_of_stem_tracks_and_deactivates_the_clip` | unit |
| R9 | `crates/shell/tests/stems.rs`, `separating_makes_a_group_of_stem_tracks_and_deactivates_the_clip`, `merged_parts_share_one_track_and_keep_their_own_files` | unit |
| R10 | `crates/shell/tests/stems.rs`, `merged_parts_share_one_track_and_keep_their_own_files`; `crates/shell/src/stems.rs`, `a_stems_file_lines_up_with_the_clip_and_sums_its_parts` | unit |
| R11 | `crates/shell/tests/stems.rs`, `separating_makes_a_group_of_stem_tracks_and_deactivates_the_clip` | unit |
| R12 | `crates/analysis/src/stems.rs`, `overlapping_windows_blend_back_into_the_whole_mix`, `progress_rises_to_one_and_can_cancel`; `crates/analysis/tests/stems_model.rs`, `a_bass_line_and_a_hi_hat_separate_into_bass_and_drums` (where the packages are named) | unit |
| R13 | `crates/analysis/src/stems.rs`, `high_quality_averages_a_shifted_second_pass` | unit |

The tests that separate with the real network run where `BL_STEMS_RUNTIME` names ONNX Runtime's shared library and `BL_STEMS_MODEL` the model; without them they pass untested.
