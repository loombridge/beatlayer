# Working with video

- **Status:** draft
- **Work packages:** WP-9.24 (reading and decoding movies in `bl-video`, frame times and gaps: R1 to R4, R26 to R28); WP-9.25 (video clips in the Arrangement and as audio clips in the Session View, their look, trimming, the edits that make them audio clips, warping and tempo leaders, the movie's markers: R5 to R15); WP-9.26 (the Video Window: R16 to R23); WP-9.27 (video export: R24, R25). The Export Audio/Video dialog's Video section is [export](export.md) R38 and R46 to R52; tempo leaders and followers are [warping](warping.md) R43 to R48 and R56 to R58; DAWproject's video timelines are [DAWproject](dawproject.md) R36 and R89; a Live set's video clips are [Live set import](live-set-import.md) R66
- **Parity-matrix row:** Files and export: Video in the arrangement, video window, video export
- **Checked against:** the Live 12 reference manual (online edition), chapter 28 "Working with Video" and chapter 5 section 5.1.3.4, read 30 October 2026; the UI parity reference rows L-OTH-004, M-VIEW-029 and S-VIEW-009

## The behaviour

A QuickTime movie dragged from the browser into the Arrangement becomes a video clip on an audio track: it looks and trims like an audio clip, with sprocket holes along its title bar, and plays the movie's sound. The Video Window, a floating window that stays above the main window, shows the frame at the playhead. Video clips are tempo leaders by default and audio clips followers, so with Warp on, a video clip's warp markers become hit points the music lines up with; while a warp marker is dragged, the Video Window shows the frame under it. The movie's own markers show as cues. Where the movie has gaps, the window shows black and the sound is silent.

In the Session View a movie is an audio clip and shows no picture, and Consolidate, Reverse and Crop turn a video clip into an audio clip, leaving the movie file as it was. File ▸ Export Audio/Video writes a movie of the Arrangement's video with the rendered audio, over the selection, or the whole movie when a video clip is selected.

Decoding runs in a crate of its own, `bl-video`, on a video thread that decodes frames ahead of the playhead through the platform's own decoders, and never touches the audio thread.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 28 "Working with Video", section 28.1 "Importing Video" | QuickTime movies (`.mov`) import as video clips, dragged from the browser into a set; a movie in the Session View is an audio clip and shows no video |
| S2 | Manual | The same chapter, section 28.2.1 "Video Clips in the Arrangement View" | A video clip looks like an audio clip with sprocket holes in its title bar, and trims like one; Consolidate, Reverse and Crop replace it with an audio clip, and the movie file is never changed |
| S3 | Manual | The same chapter, section 28.2.2 "The Video Window" | A floating window that stays above the main window and can be moved anywhere; shown and hidden from the View menu; resized from its bottom-right corner; its size and position aren't saved with a set; a double-click puts it in full screen, which can be on a second monitor; an Option-double-click (Alt) gives the movie's original size |
| S4 | Manual | The same chapter, section 28.2.2.1 "Movies with Partial Tracks" | A movie can have gaps in its video or audio: the window shows black in a video gap and an audio gap plays silence |
| S5 | Manual | The same chapter, section 28.2.3 "Clip View" | In the Arrangement, video clips are tempo leaders and audio clips followers by default; Warp must be on for a clip to lead; warp markers on a video clip become hit points for the music; only the bottom-most playing warped clip leads, so a video clip that isn't leading can be warped, which warps the picture |
| S6 | Manual | The same chapter, section 28.2.3.1 "Warp Markers" | While a video clip's warp marker is dragged, the Video Window shows the frame under it; markers embedded in the movie are shown and serve as cues |
| S7 | Manual | The same chapter, sections 28.3 "Matching Sound to Video" and 28.4; chapter 5, section 5.1.3.4 "Video Rendering Options" | Export Audio/Video mixes the audio and can write the video; the range follows the Arrangement's selection, and with a video clip selected the movie keeps the original's whole length |
| S8 | Reference | [layout.md](../ui-parity/layout.md) L-OTH-004 (the Video Window, floating above the main window, its place and size kept between sets); [menus.md](../ui-parity/menus.md) M-VIEW-029 (View ▸ Video Window, ticked, ⌥⇧⌘V); [shortcuts.md](../ui-parity/shortcuts.md) S-VIEW-009 (Cmd-Option-Shift-V, Ctrl-Alt-Shift-V) | Where the window and its menu item are, and its key |
| S9 | Standard | ISO/IEC 14496-12, the ISO base media file format, and Apple's QuickTime File Format specification | How a movie's tracks, samples, edit lists and time scales are stored |
| S10 | Beatlayer | The architecture's crates table (`bl-video`), "Media and warping", "Real-time rules" and "Agent control" (D17) | Decoding outside the engine, the media store, nothing on the audio thread, every action a command with a readback |

## Rules

### Movies (WP-9.24)

- **R1.** Beatlayer reads QuickTime movies (`.mov`) through its own demuxer in `bl-video`. A movie's first video track is its picture and its first audio track its sound; other tracks are left out. A file that isn't a movie, or whose video codec no decoder on the computer takes, is refused with the reason. (S1, S9)
- **R2.** Frame times are exact rationals in the movie's time scale, through its edit list. The frame shown at a time *t* is the last frame whose presentation time is at or before *t*. (S9; Beatlayer's own)
- **R3.** A movie's sound decodes through `bl-media` as an audio file of the same samples does. (S1, S10)
- **R4.** Where the movie has no video at a time, the picture is black; where it has no audio, the sound is silence. (S4)

### Video clips (WP-9.25)

- **R5.** A movie dropped on an audio track in the Arrangement (or below the tracks, making one) becomes a video clip: an audio clip that names its movie as its video, playing the movie's sound. It saves and reopens as a video clip. Dropped in the Session View it becomes an audio clip of the movie's sound with no video. (S1, S2)
- **R6.** A video clip draws as an audio clip does, with a row of sprocket holes along its title bar in Beatlayer's own look. (S2)
- **R7.** A video clip trims, moves, copies and loops as an audio clip does; the picture follows the sound's time. (S2)
- **R8.** Consolidate, Reverse and Crop turn a video clip into an audio clip, through the one writer of [audio clip editing](audio-clip-editing.md); the movie file isn't changed. A video clip copied or moved into the Session View becomes an audio clip there. (S1, S2)
- **R9.** In the Arrangement a new video clip has Warp on and Lead set ([warping](warping.md) R56); a new audio clip stays a follower. (S5)
- **R10.** A video clip leads only with Warp on; of the playing warped leaders, the one on the bottom-most track leads ([warping](warping.md) R44, R57). A video clip that isn't leading but is warped plays its picture through its warp map, so the picture is warped. (S5)
- **R11.** A video clip's warp markers move which frame plays when: the frame shown at a song time is the frame at the movie time the warp map gives. (S5)
- **R12.** The markers embedded in a movie (its chapter and marker tracks) show in the Clip View's Sample Editor as cues at their movie times, named as the movie names them, and can't be moved. (S6)
- **R13.** `video.frame_at` (`read`) gives a video clip's frame index and movie time at a song time; `clip` reads back a clip's video. (S10)
- **R14.** A video clip's movie is a media file: it's collected, relinked and listed by the File Manager as audio files are. (S10)
- **R15.** Video clips import from DAWproject ([DAWproject](dawproject.md) R89) and from Live sets ([Live set import](live-set-import.md) R66). (S10)

### The Video Window (WP-9.26)

- **R16.** View ▸ Video Window (M-VIEW-029) and Cmd-Option-Shift-V, Ctrl-Alt-Shift-V (S-VIEW-009), show and hide the Video Window (L-OTH-004), ticked while it shows; enabled only while the set has a video clip in the Arrangement. Dropping a movie into the Arrangement shows it. (S3, S8)
- **R17.** The window floats above the main window and the second window, can be moved anywhere and resized from its bottom-right corner, the picture scaled to fit with its aspect kept and black around it. Its place and size are kept in the preferences, not in a set, so they're the same in every set. (S3, S8)
- **R18.** The window shows the frame at the playhead within one frame, from the bottom-most track's playing video clip; with no video clip playing at the playhead, black. (S3, S4)
- **R19.** While a video clip's warp marker is dragged, the window shows the frame under the marker. (S6)
- **R20.** A double-click on the window puts it in full screen on the screen it's on, and another double-click brings it back. An Option-double-click (Alt) sets its size to the movie's own. (S3)
- **R21.** `video.window_state` (`read`) gives whether the window shows, its place, size, full screen and the frame it shows; `video.set_window` (`read`, as a view command) shows, hides and places it. (S10)
- **R22.** The window is in the accessibility tree as a window named Video, its picture an image whose description is the clip's name and the frame's time; Esc leaves full screen. (S10; Beatlayer's own)
- **R23.** The window draws frames the video thread has already decoded, uploaded as a texture; it never waits for a frame, showing the last one it had. (S10)

### Video export (WP-9.27)

- **R24.** With video clips in the Arrangement and a PCM file type chosen, the Export Audio/Video dialog's Create Video writes a movie beside the audio ([export](export.md) R46 to R52): its frames are the Arrangement's picture at each frame time of the chosen frame rate, and its sound is the rendered audio. (S7)
- **R25.** The movie's range is the export's range; with a video clip selected, the movie has the original movie's whole length, with what comes before the clip's start in the movie as pre-roll. (S7)

### Decoding (WP-9.24)

- **R26.** Decoders sit behind one trait in `bl-video`: Motion JPEG in Rust, so test movies decode on every computer; H.264, HEVC and ProRes through VideoToolbox on macOS and Media Foundation on Windows; on Linux, FFmpeg's shared libraries loaded at run time where they're installed. Nothing is downloaded at build or run time. (S10; Beatlayer's own)
- **R27.** A video thread decodes frames ahead of the time the window and the export ask for into a frame cache, and drops frames it's done with; it never runs on, waits for or signals the audio thread. A seek lands on the frame R2 gives. (S10)
- **R28.** A damaged movie is refused, or its damaged frames shown black, without a crash; the demuxer is fuzzed. (S10)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2, R4, R26 | `crates/video/tests/decode.rs` (frame times, the frame at a time, seeks, stored order, gaps, refusals) and `crates/video/tests/platform.rs` (H.264, HEVC and ProRes through the platform's decoders and FFmpeg's libraries), on movies `tests/fixtures/gen/gen_video.py` generates whose frames encode their index | unit |
| R3, R4 | `crates/video/tests/audio.rs`: a movie's audio against the same samples as a WAV file, silence in the gaps, blocks read anywhere | unit |
| R27 | `crates/video/tests/thread.rs`: decoding ahead, dropping what's done with, seeks, gaps | unit |
| R28 | `crates/video/tests/damaged.rs` and `fuzz/fuzz_targets/movie.rs` | unit, fuzz |
| R5 to R15 | to be written in WP-9.25: model, shell and UI tests per rule; a save and load round trip | unit, shell test, UI script |
| R16 to R23 | to be written in WP-9.26: `crates/ui/tests/video_window.rs` on the test clock; `crates/ui/tests/keymap/resolve.rs` for S-VIEW-009 | UI script |
| R24, R25 | to be written in WP-9.27: a Motion JPEG export checked frame by frame | shell test |
