//! `export.audio` driven headless through `Shell::run`, as agents and
//! `bl-cli export` drive it (WP-1.12): where the range comes from,
//! what's refused, replacing files, and missing media. The behaviour is in
//! the export spec; the audio itself is null-tested against the golden
//! renders in `bl-cli`'s tests.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ContentHash, DeviceId, TrackKind};
use bl_shell::export_commands::{self as ids, ExportResult, RangeSource};
use bl_shell::{Shell, ShellError, browser_commands};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-export-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz: a quiet ramp.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let mut bytes = Vec::new();
    let data = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 20;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn export(shell: &mut Shell, args: Value) -> ExportResult {
    let result = shell
        .run(ids::AUDIO, args)
        .unwrap_or_else(|error| panic!("{error}"));
    serde_json::from_value(result).unwrap()
}

/// A shell holding a one-second file as a clip at bar 5.
fn shell_with_a_clip(scratch: &Scratch) -> Shell {
    let mut shell = Shell::headless();
    let file = scratch.path("Tone.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [text(&file)], "at": 4 * BAR }),
        )
        .unwrap();
    shell
}

/// Spec: export R1, R15.
#[test]
fn the_range_is_the_selection_or_else_the_whole_set() {
    let scratch = Scratch::new("range");
    let mut shell = shell_with_a_clip(&scratch);
    let document = shell.editor().document().clone();
    let history = shell.editor().undo_history().len();

    // The whole set: 1.1.1 to the end of the clip, a second (two beats at
    // 120 BPM) after bar 5.
    let whole = export(
        &mut shell,
        json!({ "path": text(&scratch.path("whole.wav")), "bit_depth": 24, "sample_rate": 48000 }),
    );
    assert_eq!(whole.range, RangeSource::Set);
    assert_eq!(
        (whole.start, whole.end),
        (Ticks::ZERO, Ticks(4 * BAR + BAR / 2))
    );
    assert_eq!(whole.frames, 9 * 48_000);
    assert_eq!(whole.channels, 2);
    assert_eq!(whole.render_rate, 48_000);
    assert!(whole.peak_dbfs.is_some(), "the clip plays");
    assert!(whole.speed > 1.0, "{}", whole.speed);
    assert!(Path::new(&whole.path).is_file());

    // A time selection decides it instead.
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": 4 * BAR, "end": 5 * BAR }),
        )
        .unwrap();
    let selected = export(
        &mut shell,
        json!({ "path": text(&scratch.path("selection.aif")), "bit_depth": 16,
                "sample_rate": 48000 }),
    );
    assert_eq!(selected.range, RangeSource::Selection);
    assert_eq!(
        (selected.start, selected.end),
        (Ticks(4 * BAR), Ticks(5 * BAR))
    );
    assert_eq!(selected.frames, 2 * 48_000);

    // And arguments override both: a start as bars, a length in ticks.
    let given = export(
        &mut shell,
        json!({ "path": text(&scratch.path("given.flac")), "bit_depth": 24,
                "sample_rate": 44100, "start": "5.2", "length": BAR / 4 }),
    );
    assert_eq!(given.range, RangeSource::Arguments);
    assert_eq!(given.start, Ticks(4 * BAR + BAR / 4));
    assert_eq!(given.frames, 22_050);
    assert_eq!(given.render_rate, 48_000, "rendered at the engine's rate");
    // A start alone runs to the end of the selection.
    let from = export(
        &mut shell,
        json!({ "path": text(&scratch.path("from.wav")), "bit_depth": 32,
                "sample_rate": 48000, "start": 4 * BAR + BAR / 2 }),
    );
    assert_eq!(from.end, Ticks(5 * BAR));
    assert_eq!(from.dither, ids::Dither::None, "no dither at 32 bits");

    // Exporting changes nothing in the set.
    assert_eq!(*shell.editor().document(), document);
    assert_eq!(shell.editor().undo_history().len(), history);
    assert_eq!(
        scratch.names(),
        [
            "Tone.wav",
            "from.wav",
            "given.flac",
            "selection.aif",
            "whole.wav"
        ]
    );
}

fn refused(shell: &mut Shell, args: Value) -> ShellError {
    shell.run(ids::AUDIO, args).expect_err("refused")
}

/// Spec: export R1, R5.
#[test]
fn an_empty_set_or_a_bad_range_is_refused() {
    let scratch = Scratch::new("refused");
    // The factory default set has no clips.
    let mut shell = Shell::headless();
    let error = refused(
        &mut shell,
        json!({ "path": text(&scratch.path("a.wav")), "bit_depth": 24, "sample_rate": 48000 }),
    );
    assert!(error.to_string().contains("no clips"), "{error}");

    let mut shell = shell_with_a_clip(&scratch);
    let ok = |path: &str| text(&scratch.path(path));
    for (args, says) in [
        (
            json!({ "path": ok("a.flac"), "bit_depth": 32, "sample_rate": 48000 }),
            "FLAC",
        ),
        (
            json!({ "path": ok("a.wav"), "bit_depth": 24, "sample_rate": 48000,
                    "file_type": "flac" }),
            "extension",
        ),
        (
            json!({ "path": ok("a.ogg"), "bit_depth": 24, "sample_rate": 48000 }),
            "extension",
        ),
        (
            json!({ "path": ok("a.mp3"), "bit_depth": 24, "sample_rate": 96000 }),
            "MP3 holds",
        ),
        (
            json!({ "path": ok("a.wav"), "bit_depth": 8, "sample_rate": 48000 }),
            "8 bits",
        ),
        (
            json!({ "path": ok("a.wav"), "bit_depth": 24, "sample_rate": 1000 }),
            "1000 Hz",
        ),
        (
            json!({ "path": ok("a.wav"), "bit_depth": 24, "sample_rate": 48000,
                    "start": -1, "length": BAR }),
            "before 1.1.1",
        ),
        (
            json!({ "path": ok("a.wav"), "bit_depth": 24, "sample_rate": 48000,
                    "length": 0 }),
            "empty",
        ),
        (
            json!({ "path": ok("no/such/folder/a.wav"), "bit_depth": 24,
                    "sample_rate": 48000 }),
            "no folder",
        ),
    ] {
        let error = refused(&mut shell, args.clone());
        assert!(
            matches!(error, ShellError::InvalidArgs { .. }),
            "{args}: {error:?}"
        );
        assert!(error.to_string().contains(says), "{args}: {error}");
    }
    assert_eq!(scratch.names(), ["Tone.wav"], "nothing written");
}

/// Spec: export R23 (WP-2.26). An MP3 export takes the extension, writes a
/// file Beatlayer's decoder reads back at the export's rate, and refuses a
/// `file_type` that disagrees with the extension.
#[test]
fn an_mp3_export_takes_the_extension() {
    let scratch = Scratch::new("mp3");
    let mut shell = shell_with_a_clip(&scratch);
    let written = export(
        &mut shell,
        json!({ "path": text(&scratch.path("mix.mp3")), "bit_depth": 16,
                "sample_rate": 44100 }),
    );
    assert_eq!(written.file_type, ids::FileType::Mp3);
    assert_eq!(written.dither, ids::Dither::None, "MP3 has no dither");
    let decoder = bl_media::Decoder::open(&written.path).unwrap();
    assert_eq!(decoder.info().sample_rate, 44_100);
    assert_eq!(
        decoder.decode_to_end().unwrap().frames() as u64,
        written.frames
    );

    let error = refused(
        &mut shell,
        json!({ "path": text(&scratch.path("mix.mp3")), "bit_depth": 16,
                "sample_rate": 48000, "file_type": "wav" }),
    );
    assert!(error.to_string().contains("extension"), "{error}");
}

/// Spec: export R11.
#[test]
fn an_existing_file_is_replaced_only_when_asked() {
    let scratch = Scratch::new("replace");
    let mut shell = shell_with_a_clip(&scratch);
    let path = scratch.path("mix.wav");
    std::fs::write(&path, b"an older mix").unwrap();
    let args = json!({ "path": text(&path), "bit_depth": 16, "sample_rate": 48000 });
    let error = refused(&mut shell, args.clone());
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error:?}");
    assert!(error.to_string().contains("replace"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), b"an older mix");

    let mut replace = args;
    replace["replace"] = json!(true);
    let result = export(&mut shell, replace);
    assert_eq!(result.frames, 9 * 48_000);
    let decoded = bl_media::decode_file(&path).unwrap();
    assert_eq!(decoded.frames(), 9 * 48_000);
    assert_eq!(scratch.names(), ["Tone.wav", "mix.wav"]);
}

/// Spec: export R13.
#[test]
fn missing_media_plays_silence_and_is_named() {
    let scratch = Scratch::new("missing");
    let mut shell = Shell::headless();
    let hash = ContentHash([7; 32]);
    let track = shell.new_id::<bl_model::TrackId>();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": TrackKind::Audio, "name": "Gone", "index": 0 }),
        )
        .unwrap();
    let clip = Clip::new(
        shell.new_id(),
        Ticks::ZERO,
        Ticks(BAR),
        ClipData::Audio(AudioData::new(hash)),
    );
    shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    let result = export(
        &mut shell,
        json!({ "path": text(&scratch.path("gone.wav")), "bit_depth": 24,
                "sample_rate": 48000, "normalize": true }),
    );
    assert_eq!(result.missing_media, [hash.to_string()]);
    assert_eq!(result.peak_dbfs, None, "silence");
    assert_eq!(result.gain_db, 0.0, "Normalize leaves silence alone");
    assert_eq!(result.frames, 2 * 48_000);
}

fn shell_with_two_tracks(scratch: &Scratch) -> (Shell, bl_model::TrackId, bl_model::TrackId) {
    let mut shell = Shell::headless();
    let file1 = scratch.path("Tone1.wav");
    let file2 = scratch.path("Tone2.wav");
    wav(&file1);
    wav(&file2);

    let track1 = shell.new_id::<bl_model::TrackId>();
    shell
        .run(
            "track.create",
            json!({ "id": track1, "kind": TrackKind::Audio, "name": "Kick", "index": 0 }),
        )
        .unwrap();
    let track2 = shell.new_id::<bl_model::TrackId>();
    shell
        .run(
            "track.create",
            json!({ "id": track2, "kind": TrackKind::Audio, "name": "Bass", "index": 1 }),
        )
        .unwrap();

    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [text(&file1)], "at": 0, "track": track1 }),
        )
        .unwrap();
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [text(&file2)], "at": 0, "track": track2 }),
        )
        .unwrap();

    (shell, track1, track2)
}

/// Spec: WP-2.26. A track render without returns or Main effects by default
/// renders dry and unaffected by return sends or Main devices.
/// With `include_return_and_main_effects`, it includes them (equals track soloed).
#[test]
fn single_track_export_renders_dry_or_with_return_and_main_effects() {
    let scratch = Scratch::new("single-track-dry-wet");
    let (mut shell, track1, _) = shell_with_two_tracks(&scratch);
    let doc = shell.editor().document().clone();
    let reverb_return = doc.returns().next().expect("Return A");
    let main_id = doc.main_id();

    // Turn up track1's send to Return A
    shell
        .run(
            "mixer.set_send",
            json!({ "track": track1, "to": reverb_return, "level": 1.0 }),
        )
        .unwrap();

    // Add a gain device on Return A boosting by +6 dB (gain 1.0 in kit example gain is 6 dB)
    let gain_type = shell
        .kit()
        .device_types()
        .into_iter()
        .find(|t| t.name == "Example Gain")
        .expect("Example Gain");

    let added_return = shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": reverb_return,
            }),
        )
        .unwrap();
    let dev_return: DeviceId = serde_json::from_value(added_return["device"].clone()).unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": dev_return, "param": lspk_example_gain::GAIN, "value": 1.0 }),
        )
        .unwrap();

    // Add a gain device on Main attenuating by -12 dB (gain 0.25)
    let added_main = shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": main_id,
            }),
        )
        .unwrap();
    let dev_main: DeviceId = serde_json::from_value(added_main["device"].clone()).unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": dev_main, "param": lspk_example_gain::GAIN, "value": 0.25 }),
        )
        .unwrap();

    // Set Main volume to 0.5 and pan left (-0.8): dry render ignores Main fader and pan.
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": main_id, "volume": 0.5 }),
        )
        .unwrap();
    shell
        .run("mixer.set_pan", json!({ "track": main_id, "pan": -0.8 }))
        .unwrap();

    // 1. Export track1 dry (default include_return_and_main_effects: false)
    let dry_path = scratch.path("dry_kick.wav");
    let dry_result = export(
        &mut shell,
        json!({
            "path": text(&dry_path),
            "rendered_track": { "track": track1 },
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    assert_eq!(
        PathBuf::from(&dry_result.files[0]),
        std::fs::canonicalize(&dry_path).unwrap()
    );

    // 2. Export with include_return_and_main_effects: true
    let wet_path = scratch.path("wet_kick.wav");
    let wet_result = export(
        &mut shell,
        json!({
            "path": text(&wet_path),
            "rendered_track": { "track": track1 },
            "include_return_and_main_effects": true,
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    assert_eq!(
        PathBuf::from(&wet_result.files[0]),
        std::fs::canonicalize(&wet_path).unwrap()
    );

    // 3. Export pure track1 in a set without sends or Main effects
    let (mut pure_shell, pure_track1, _) = shell_with_two_tracks(&scratch);
    let pure_path = scratch.path("pure_kick.wav");
    export(
        &mut pure_shell,
        json!({
            "path": text(&pure_path),
            "rendered_track": { "track": pure_track1 },
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );

    let dry_file = bl_media::decode_file(&dry_path).unwrap();
    let wet_file = bl_media::decode_file(&wet_path).unwrap();
    let pure_file = bl_media::decode_file(&pure_path).unwrap();

    // Dry render is deterministic: compare both channels exactly.
    assert_eq!(dry_file.frames(), pure_file.frames());
    assert_eq!(dry_file.channel(0).unwrap(), pure_file.channel(0).unwrap());
    assert_eq!(dry_file.channel(1).unwrap(), pure_file.channel(1).unwrap());

    // Wet render equals the track soloed in the shell with return and Main effects:
    shell
        .run("mixer.solo_exclusive", json!({ "track": track1 }))
        .unwrap();
    let soloed_path = scratch.path("soloed_kick.wav");
    export(
        &mut shell,
        json!({
            "path": text(&soloed_path),
            "rendered_track": "main",
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    let soloed_file = bl_media::decode_file(&soloed_path).unwrap();
    assert_eq!(wet_file.frames(), soloed_file.frames());
    assert_eq!(
        wet_file.channel(0).unwrap(),
        soloed_file.channel(0).unwrap()
    );
    assert_eq!(
        wet_file.channel(1).unwrap(),
        soloed_file.channel(1).unwrap()
    );

    // Wet render includes the send and Main effect and differs from dry:
    assert_ne!(dry_file.channel(0).unwrap(), wet_file.channel(0).unwrap());
    assert_ne!(dry_file.channel(1).unwrap(), wet_file.channel(1).unwrap());
}

/// Spec: WP-2.26. All-tracks or selected-tracks exports write one file per track.
#[test]
fn all_tracks_and_selected_tracks_write_one_file_per_track() {
    let scratch = Scratch::new("multi-track");
    let (mut shell, _track1, track2) = shell_with_two_tracks(&scratch);

    // Add an instrument to one of the default MIDI tracks so it makes audio
    let midi_tracks: Vec<bl_model::TrackId> = shell
        .editor()
        .document()
        .tracks()
        .filter(|t| t.kind() == TrackKind::Midi)
        .map(|t| t.id())
        .collect();
    let midi_track = midi_tracks[0];
    let empty_midi_track = midi_tracks[1];
    let sine_type = shell
        .kit()
        .device_types()
        .into_iter()
        .find(|t| t.name == "Example Sine")
        .expect("Example Sine");
    shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": sine_type.kind,
                    "name": sine_type.name,
                    "role": sine_type.role,
                },
                "track": midi_track,
            }),
        )
        .unwrap();

    // All tracks export:
    // - 1 MIDI track with instrument: included
    // - 1 MIDI track without instrument/effect: skipped
    // - 2 default audio tracks + 2 added audio tracks: included (4)
    // - 2 return tracks: included
    // - 1 Main track: included
    // Total = 1 + 4 + 2 + 1 = 8 tracks
    let base_path = scratch.path("Song.wav");
    let all_result = export(
        &mut shell,
        json!({
            "path": text(&base_path),
            "rendered_track": "all_tracks",
            "bit_depth": 24,
            "sample_rate": 48000,
        }),
    );

    let kick_path = scratch.path("Song - Kick.wav");
    let bass_path = scratch.path("Song - Bass.wav");
    let main_path = scratch.path("Song - Main.wav");
    let sine_path = scratch.path("Song - 1 MIDI.wav");
    let empty_midi_path = scratch.path("Song - 2 MIDI.wav");

    assert_eq!(all_result.files.len(), 8);
    let kick_canonical = std::fs::canonicalize(&kick_path)
        .unwrap()
        .display()
        .to_string();
    let bass_canonical = std::fs::canonicalize(&bass_path)
        .unwrap()
        .display()
        .to_string();
    let main_canonical = std::fs::canonicalize(&main_path)
        .unwrap()
        .display()
        .to_string();
    let sine_canonical = std::fs::canonicalize(&sine_path)
        .unwrap()
        .display()
        .to_string();

    assert!(all_result.files.contains(&kick_canonical));
    assert!(all_result.files.contains(&bass_canonical));
    assert!(all_result.files.contains(&main_canonical));
    assert!(all_result.files.contains(&sine_canonical));
    assert!(!empty_midi_path.exists(), "empty MIDI track is skipped");
    assert!(kick_path.is_file());
    assert!(bass_path.is_file());
    assert!(main_path.is_file());
    assert!(sine_path.is_file());

    // All exported files have the same length
    let kick_file = bl_media::decode_file(&kick_path).unwrap();
    for f in &all_result.files {
        let dec = bl_media::decode_file(Path::new(f)).unwrap();
        assert_eq!(
            dec.frames(),
            kick_file.frames(),
            "all files have the same length"
        );
    }

    // Selected tracks only: select Bass (track2) and empty 2-MIDI
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [track2, empty_midi_track] }),
        )
        .unwrap();
    let stems_base = scratch.path("Stems.wav");
    let selected_result = export(
        &mut shell,
        json!({
            "path": text(&stems_base),
            "rendered_track": "selected_tracks",
            "bit_depth": 24,
            "sample_rate": 48000,
        }),
    );
    let stem_bass_path = scratch.path("Stems - Bass.wav");
    let stem_kick_path = scratch.path("Stems - Kick.wav");
    let stem_empty_midi = scratch.path("Stems - 2 MIDI.wav");
    assert_eq!(
        selected_result.files,
        [std::fs::canonicalize(&stem_bass_path)
            .unwrap()
            .display()
            .to_string()]
    );
    assert!(stem_bass_path.is_file());
    assert!(!stem_kick_path.exists());
    assert!(
        !stem_empty_midi.exists(),
        "selected empty MIDI track is skipped"
    );
}

/// Spec: export R26 (WP-4.17). Each file of a multi-track loop render folds
/// its own track's tail: a track with a reverb gets the reverb's, and one
/// without gets nothing added.
#[test]
fn each_track_of_a_loop_render_adds_its_own_tail() {
    let scratch = Scratch::new("loop-tracks");
    let (mut shell, kick, _) = shell_with_two_tracks(&scratch);
    shell
        .run(
            "device.add",
            json!({
                "track": kick,
                "device": { "kind": { "native": { "device_type": "beatlayer.rotunda" } },
                            "name": "Reverb", "role": "audio_effect" },
            }),
        )
        .unwrap();
    // The first half second, 705,600 ticks to the quarter note.
    for (name, looped) in [("Plain", false), ("Loop", true)] {
        let result = export(
            &mut shell,
            json!({
                "path": text(&scratch.path(&format!("{name}.wav"))),
                "rendered_track": "all_tracks",
                "bit_depth": 32,
                "sample_rate": 48000,
                "start": 0,
                "length": 705_600,
                "render_as_loop": looped,
            }),
        );
        assert_eq!(result.frames, 24_000);
    }
    let read = |name: &str| std::fs::read(scratch.path(name)).unwrap();
    assert_ne!(read("Plain - Kick.wav"), read("Loop - Kick.wav"));
    assert_eq!(read("Plain - Bass.wav"), read("Loop - Bass.wav"));
}

/// Spec: export R10, WP-2.26. Multi-track export cancel and failure leave no partial files.
#[test]
fn multi_track_export_cancellation_and_failure_leave_no_partial_files() {
    let scratch = Scratch::new("multi-cancel-fail");
    let (shell, track1, track2) = shell_with_two_tracks(&scratch);
    let doc = shell.editor().document().clone();
    let media = shell.media_store();

    // Cancellation: cancel during the second track.
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let job1 = bl_shell::export::Job {
        path: scratch.path("Cancel - Kick.wav"),
        file_type: bl_media::FileType::Wav,
        bit_depth: bl_media::BitDepth::Int24,
        dither: bl_media::Dither::None,
        sample_rate: 48000,
        engine_rate: 48000,
        normalize: false,
        start: Ticks::ZERO,
        length: bl_shell::export::Length::Frames(48000),
        replace: true,
        render_as_loop: false,
        block: bl_shell::export::BLOCK,
    };
    let job2 = bl_shell::export::Job {
        path: scratch.path("Cancel - Bass.wav"),
        file_type: bl_media::FileType::Wav,
        bit_depth: bl_media::BitDepth::Int24,
        dither: bl_media::Dither::None,
        sample_rate: 48000,
        engine_rate: 48000,
        normalize: false,
        start: Ticks::ZERO,
        length: bl_shell::export::Length::Frames(48000),
        replace: true,
        render_as_loop: false,
        block: bl_shell::export::BLOCK,
    };
    let mut solo1 = doc.clone();
    solo1
        .apply(&bl_model::Command::SoloExclusive { track: track1 })
        .unwrap();
    let mut solo2 = doc.clone();
    solo2
        .apply(&bl_model::Command::SoloExclusive { track: track2 })
        .unwrap();

    let renders = [(&solo1, &job1), (&solo2, &job2)];
    let result = bl_shell::export::export_multi(
        &renders,
        media.clone(),
        Default::default(),
        &cancel,
        &mut |share| {
            if share >= 0.6 {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        },
    );
    assert_eq!(result, Err(bl_shell::export::ExportError::Cancelled));
    for name in scratch.names() {
        assert!(!name.starts_with("Cancel"), "found unexpected file: {name}");
        assert!(!name.contains("partial"), "found partial file: {name}");
        assert!(
            !name.contains("render"),
            "found render scratch file: {name}"
        );
    }

    // Failure: the second job's partial path is obstructed by a directory.
    let fail_path = scratch.path("Fail - Bass.wav");
    let partial_obstructed = bl_shell::export::hidden_beside(&fail_path, "partial");
    std::fs::create_dir_all(&partial_obstructed).unwrap();
    let job_fail = bl_shell::export::Job {
        path: fail_path,
        file_type: bl_media::FileType::Wav,
        bit_depth: bl_media::BitDepth::Int24,
        dither: bl_media::Dither::None,
        sample_rate: 48000,
        engine_rate: 48000,
        normalize: false,
        start: Ticks::ZERO,
        length: bl_shell::export::Length::Frames(48000),
        replace: true,
        render_as_loop: false,
        block: bl_shell::export::BLOCK,
    };
    let fail_renders = [(&solo1, &job1), (&solo2, &job_fail)];
    let fail_result = bl_shell::export::export_multi(
        &fail_renders,
        media,
        bl_shell::export::Hosts::default(),
        &std::sync::atomic::AtomicBool::new(false),
        &mut |_| {},
    );
    assert!(fail_result.is_err());
    let _ = std::fs::remove_dir(&partial_obstructed);
    for name in scratch.names() {
        assert!(!name.starts_with("Cancel"), "found unexpected file: {name}");
        assert!(!name.starts_with("Fail"), "found unexpected file: {name}");
        assert!(!name.contains("partial"), "found partial file: {name}");
        assert!(
            !name.contains("render"),
            "found render scratch file: {name}"
        );
    }
}

/// Spec: WP-2.26. Refusals for track export parameters.
#[test]
fn track_export_refusals() {
    let scratch = Scratch::new("track-refusals");
    let (mut shell, _track1, _) = shell_with_two_tracks(&scratch);
    let main_id = shell.editor().document().main_id();

    // Refuse Main track ID in track
    let err = refused(
        &mut shell,
        json!({
            "path": text(&scratch.path("main.wav")),
            "rendered_track": { "track": main_id },
            "bit_depth": 24,
            "sample_rate": 48000,
        }),
    );
    assert!(
        err.to_string().contains("Main track is the whole mix"),
        "{err}"
    );

    // Refuse include_return_and_main_effects when rendered_track is Main
    let err = refused(
        &mut shell,
        json!({
            "path": text(&scratch.path("main_effects.wav")),
            "rendered_track": "main",
            "include_return_and_main_effects": true,
            "bit_depth": 24,
            "sample_rate": 48000,
        }),
    );
    assert!(
        err.to_string().contains("include_return_and_main_effects"),
        "{err}"
    );

    // Refuse unknown track ID
    let bogus_track = shell.new_id::<bl_model::TrackId>();
    let err = refused(
        &mut shell,
        json!({
            "path": text(&scratch.path("bogus.wav")),
            "rendered_track": { "track": bogus_track },
            "bit_depth": 24,
            "sample_rate": 48000,
        }),
    );
    assert!(err.to_string().contains("no track has the ID"), "{err}");

    // Refuse selected_tracks when no track is selected
    let err = refused(
        &mut shell,
        json!({
            "path": text(&scratch.path("none-selected.wav")),
            "rendered_track": "selected_tracks",
            "bit_depth": 24,
            "sample_rate": 48000,
        }),
    );
    assert!(err.to_string().contains("no tracks are selected"), "{err}");

    // Refuse selected_tracks when only non-audio-producing tracks are selected
    let empty_midi_track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .expect("empty MIDI track")
        .id();
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [empty_midi_track] }),
        )
        .unwrap();
    let err = refused(
        &mut shell,
        json!({
            "path": text(&scratch.path("empty-midi.wav")),
            "rendered_track": "selected_tracks",
            "bit_depth": 24,
            "sample_rate": 48000,
        }),
    );
    assert!(err.to_string().contains("make no audio"), "{err}");
}

/// Spec: WP-2.26, S2, S3.
/// - A grouped track renders dry: without the group's device or fader, and equal
///   to the same track ungrouped.
/// - The group track's file holds its children's signal through the group's
///   devices and fader.
#[test]
fn grouped_track_renders_dry_without_group_device_or_fader_and_group_holds_children_signal() {
    let scratch = Scratch::new("grouped-track-acceptance");
    let (mut shell, child_track, _) = shell_with_two_tracks(&scratch);

    // Create a group track and put child_track into it.
    let group_id = shell.new_id::<bl_model::TrackId>();
    shell
        .run(
            "track.create",
            json!({
                "id": group_id,
                "kind": TrackKind::Group,
                "name": "Drums Group",
                "index": 0,
            }),
        )
        .unwrap();
    shell
        .run(
            "track.move",
            json!({
                "track": child_track,
                "parent": group_id,
                "index": 0,
            }),
        )
        .unwrap();

    // Add a gain device on the group boosting by +6 dB (gain 1.0)
    let gain_type = shell
        .kit()
        .device_types()
        .into_iter()
        .find(|t| t.name == "Example Gain")
        .expect("Example Gain");
    let added_group_dev = shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": group_id,
            }),
        )
        .unwrap();
    let dev_group: DeviceId = serde_json::from_value(added_group_dev["device"].clone()).unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": dev_group, "param": lspk_example_gain::GAIN, "value": 1.0 }),
        )
        .unwrap();

    // Set group fader volume to 0.75 and pan to 0.2
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": group_id, "volume": 0.75 }),
        )
        .unwrap();
    shell
        .run("mixer.set_pan", json!({ "track": group_id, "pan": 0.2 }))
        .unwrap();

    // 1. Export the grouped child track dry:
    let dry_child_path = scratch.path("dry_child.wav");
    export(
        &mut shell,
        json!({
            "path": text(&dry_child_path),
            "rendered_track": { "track": child_track },
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    let dry_child_file = bl_media::decode_file(&dry_child_path).unwrap();

    // 2. Export the same track in an ungrouped set:
    let (mut pure_shell, pure_track, _) = shell_with_two_tracks(&scratch);
    let pure_child_path = scratch.path("pure_child.wav");
    export(
        &mut pure_shell,
        json!({
            "path": text(&pure_child_path),
            "rendered_track": { "track": pure_track },
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    let pure_child_file = bl_media::decode_file(&pure_child_path).unwrap();

    // Dry grouped child equals ungrouped track on both channels bit-for-bit:
    assert_eq!(dry_child_file.frames(), pure_child_file.frames());
    assert_eq!(
        dry_child_file.channel(0).unwrap(),
        pure_child_file.channel(0).unwrap()
    );
    assert_eq!(
        dry_child_file.channel(1).unwrap(),
        pure_child_file.channel(1).unwrap()
    );

    // 3. Export the group track itself:
    let group_path = scratch.path("group.wav");
    export(
        &mut shell,
        json!({
            "path": text(&group_path),
            "rendered_track": { "track": group_id },
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    let group_file = bl_media::decode_file(&group_path).unwrap();

    // The group file holds children's signal through group devices and fader;
    // it differs from the child's dry signal due to the +6dB device and fader.
    assert_eq!(group_file.frames(), dry_child_file.frames());
    assert_ne!(
        group_file.channel(0).unwrap(),
        dry_child_file.channel(0).unwrap()
    );

    // Soloing the group in the shell renders the exact same output:
    shell
        .run("mixer.solo_exclusive", json!({ "track": group_id }))
        .unwrap();
    let soloed_group_path = scratch.path("soloed_group.wav");
    export(
        &mut shell,
        json!({
            "path": text(&soloed_group_path),
            "rendered_track": "main",
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    let soloed_group_file = bl_media::decode_file(&soloed_group_path).unwrap();
    assert_eq!(
        group_file.channel(0).unwrap(),
        soloed_group_file.channel(0).unwrap()
    );
    assert_eq!(
        group_file.channel(1).unwrap(),
        soloed_group_file.channel(1).unwrap()
    );
}

/// Spec: WP-2.26, S2, S3.
/// A return track's file holds the signal sent to it through its own devices,
/// and nothing from Main's devices or fader.
#[test]
fn return_track_file_holds_signal_through_return_devices_and_nothing_from_main() {
    let scratch = Scratch::new("return-track-acceptance");
    let (mut shell, track1, _) = shell_with_two_tracks(&scratch);
    let doc = shell.editor().document().clone();
    let return_a = doc.returns().next().expect("Return A");
    let main_id = doc.main_id();

    // Route track1's send to Return A with level 0.8
    shell
        .run(
            "mixer.set_send",
            json!({ "track": track1, "to": return_a, "level": 0.8 }),
        )
        .unwrap();

    // Add a gain device on Return A boosting by +6 dB (gain 1.0)
    let gain_type = shell
        .kit()
        .device_types()
        .into_iter()
        .find(|t| t.name == "Example Gain")
        .expect("Example Gain");
    let added_return_dev = shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": return_a,
            }),
        )
        .unwrap();
    let dev_return: DeviceId = serde_json::from_value(added_return_dev["device"].clone()).unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": dev_return, "param": lspk_example_gain::GAIN, "value": 1.0 }),
        )
        .unwrap();

    // Add a gain device on Main attenuating by -12 dB (gain 0.25)
    let added_main_dev = shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": main_id,
            }),
        )
        .unwrap();
    let dev_main: DeviceId = serde_json::from_value(added_main_dev["device"].clone()).unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": dev_main, "param": lspk_example_gain::GAIN, "value": 0.25 }),
        )
        .unwrap();

    // Set Main volume to 0.4 and pan to -0.6
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": main_id, "volume": 0.4 }),
        )
        .unwrap();
    shell
        .run("mixer.set_pan", json!({ "track": main_id, "pan": -0.6 }))
        .unwrap();

    // 1. Export Return A directly (dry individual track export)
    let return_path = scratch.path("return_a.wav");
    export(
        &mut shell,
        json!({
            "path": text(&return_path),
            "rendered_track": { "track": return_a },
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    let return_file = bl_media::decode_file(&return_path).unwrap();

    // 2. Export in a reference shell that has the same send and Return A device,
    // but NO Main devices and default Main fader (volume 1.0, pan 0.0), soloing Return A:
    let (mut ref_shell, ref_track1, _) = shell_with_two_tracks(&scratch);
    let ref_doc = ref_shell.editor().document().clone();
    let ref_return_a = ref_doc.returns().next().expect("Return A");
    ref_shell
        .run(
            "mixer.set_send",
            json!({ "track": ref_track1, "to": ref_return_a, "level": 0.8 }),
        )
        .unwrap();
    let added_ref_dev = ref_shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": ref_return_a,
            }),
        )
        .unwrap();
    let ref_dev_id: DeviceId = serde_json::from_value(added_ref_dev["device"].clone()).unwrap();
    ref_shell
        .run(
            "device.set_param",
            json!({ "device": ref_dev_id, "param": lspk_example_gain::GAIN, "value": 1.0 }),
        )
        .unwrap();
    ref_shell
        .run("mixer.solo_exclusive", json!({ "track": ref_return_a }))
        .unwrap();

    let ref_path = scratch.path("ref_return_a.wav");
    export(
        &mut ref_shell,
        json!({
            "path": text(&ref_path),
            "rendered_track": "main",
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    let ref_file = bl_media::decode_file(&ref_path).unwrap();

    // The return track's render exactly matches the return track soloed without Main processing:
    assert_eq!(return_file.frames(), ref_file.frames());
    assert_eq!(
        return_file.channel(0).unwrap(),
        ref_file.channel(0).unwrap()
    );
    assert_eq!(
        return_file.channel(1).unwrap(),
        ref_file.channel(1).unwrap()
    );

    // It has non-zero signal (from the send and return device):
    assert!(return_file.channel(0).unwrap().iter().any(|&s| s != 0.0));
}

/// Spec: WP-2.26, S2, S3.
/// A MIDI track with an audio effect and no instrument is included in track export
/// and renders silence.
#[test]
fn midi_track_with_audio_effect_and_no_instrument_renders_silence() {
    let scratch = Scratch::new("midi-audio-effect-silence");
    let mut shell = Shell::headless();
    let clip_file = scratch.path("Tone.wav");
    wav(&clip_file);

    // Add an audio clip to Audio 1 so the set has a song range.
    let audio_track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .unwrap()
        .id();
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [text(&clip_file)], "at": 0, "track": audio_track }),
        )
        .unwrap();

    // Get an existing MIDI track
    let midi_track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .unwrap()
        .id();

    // Add an audio effect (Example Gain) to the MIDI track, but NO instrument.
    let gain_type = shell
        .kit()
        .device_types()
        .into_iter()
        .find(|t| t.name == "Example Gain")
        .expect("Example Gain");
    shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": midi_track,
            }),
        )
        .unwrap();

    // Export the MIDI track:
    let out_path = scratch.path("midi_effect_only.wav");
    let result = export(
        &mut shell,
        json!({
            "path": text(&out_path),
            "rendered_track": { "track": midi_track },
            "bit_depth": 24,
            "sample_rate": 48000,
            "dither": "none",
        }),
    );
    assert_eq!(result.files.len(), 1);
    assert!(out_path.is_file());

    // File contains digital silence on both channels:
    let decoded = bl_media::decode_file(&out_path).unwrap();
    assert!(decoded.frames() > 0);
    assert!(decoded.channel(0).unwrap().iter().all(|&s| s == 0.0));
    assert!(decoded.channel(1).unwrap().iter().all(|&s| s == 0.0));
}
