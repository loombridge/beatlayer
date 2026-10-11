//! `export.audio` driven headless through `Shell::run`, as agents and
//! `bl-cli export` drive it (WP-1.12): where the range comes from,
//! what's refused, replacing files, and missing media. The behaviour is in
//! the export spec; the audio itself is null-tested against the golden
//! renders in `bl-cli`'s tests.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ContentHash, TrackKind};
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
            json!({ "path": ok("a.mp3"), "bit_depth": 24, "sample_rate": 48000 }),
            "extension",
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
