//! The Export Audio/Video dialog's commands through the shell (WP-6.71; the
//! export spec, R28 to R45): opening it with a new install's defaults,
//! changing its controls, what is disabled and why, the save dialog Export
//! asks for, an export on its thread with its share and Cancel, the
//! choices kept as preferences, and the options `export.audio` takes with
//! them (Convert to Mono, Create Analysis File, Encode MP3).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_assistant::keychain::MemoryKeychain;
use bl_core::time::Ticks;
use bl_shell::audio::AudioHost;
use bl_shell::export_commands::{self as ids, ExportResult, ExportStatus, RenderedTrack};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Effect, Shell, ShellError, browser_commands};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-export-dialog-{}-{name}", std::process::id()));
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

/// A shell holding a one-second file as a clip at bar 5.
fn shell_with_a_clip(scratch: &Scratch) -> Shell {
    let mut shell = Shell::headless();
    add_the_clip(&mut shell, scratch);
    shell
}

fn add_the_clip(shell: &mut Shell, scratch: &Scratch) {
    let file = scratch.path("Tone.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [text(&file)], "at": 4 * BAR }),
        )
        .unwrap();
}

/// A shell keeping its preferences in `dir`.
fn shell_saving_to(dir: &Path) -> Shell {
    let prefs = Preferences::load(dir).unwrap();
    let mut shell = Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    );
    shell.use_agent_keychain(Arc::new(MemoryKeychain::new()));
    shell
}

fn status(shell: &mut Shell, id: &str, args: Value) -> ExportStatus {
    let result = shell
        .run(id, args)
        .unwrap_or_else(|error| panic!("{id}: {error}"));
    serde_json::from_value(result).unwrap()
}

fn dialog(shell: &mut Shell) -> ids::ExportDialogStatus {
    status(shell, ids::STATUS, json!({}))
        .dialog
        .expect("the dialog is open")
}

/// Runs the shell's polling until the export on its thread has ended.
fn finish(shell: &mut Shell) {
    while shell.export_running() {
        shell.poll();
        std::thread::yield_now();
    }
}

fn disabled(dialog: &ids::ExportDialogStatus, control: &str) -> Option<String> {
    dialog
        .disabled
        .iter()
        .find(|d| d.control == control)
        .map(|d| d.reason.clone())
}

/// Spec: export R28, R29, R30 to R39, R43, R44. The dialog opens with a new
/// install's defaults, and `export.audio` with only a path writes the same
/// bytes as its Export does.
#[test]
fn the_dialogs_defaults_write_what_a_call_with_only_a_path_writes() {
    let scratch = Scratch::new("defaults");
    let mut shell = shell_with_a_clip(&scratch);
    let document = shell.editor().document().clone();
    let history = shell.editor().undo_history().len();

    let called: ExportResult = serde_json::from_value(
        shell
            .run(
                ids::AUDIO,
                json!({ "path": text(&scratch.path("called.wav")) }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(called.bit_depth, 24, "R44");
    assert_eq!(called.sample_rate, 48_000, "the engine's rate");
    assert_eq!(called.dither, ids::Dither::Triangular);
    assert_eq!(called.file_type, ids::FileType::Wav);
    assert_eq!(called.channels, 2);

    // R28: nothing is open until the command opens it.
    assert!(status(&mut shell, ids::STATUS, json!({})).dialog.is_none());
    assert!(shell.export_dialog().is_none());
    let opened = status(&mut shell, ids::OPEN_DIALOG, json!({}))
        .dialog
        .unwrap();
    assert!(shell.export_dialog().is_some());
    assert_eq!(opened.choices.rendered_track, RenderedTrack::Main);
    assert!(!opened.choices.include_return_and_main_effects);
    assert!(!opened.choices.render_as_loop);
    assert!(!opened.choices.convert_to_mono);
    assert!(!opened.choices.normalize);
    assert!(!opened.choices.create_analysis_file);
    // R35: the engine's rate, on the list.
    assert_eq!(opened.choices.sample_rate, Some(48_000));
    assert!(opened.choices.encode_pcm);
    assert_eq!(opened.choices.file_type, ids::FileType::Wav);
    assert_eq!(opened.choices.bit_depth, 24);
    assert_eq!(opened.choices.dither, ids::Dither::Triangular);
    assert!(!opened.choices.encode_mp3);
    // R31: the whole set, as no time selection was made.
    assert_eq!(
        (opened.start, opened.length),
        (Ticks::ZERO, Ticks(4 * BAR + BAR / 2))
    );
    assert!(opened.can_export, "{:?}", opened.reason);

    // Export: the save dialog first, then the export on its thread.
    status(&mut shell, ids::START, json!({}));
    assert!(matches!(
        shell.take_effects().as_slice(),
        [Effect::PickExportPath { .. }]
    ));
    let started = status(
        &mut shell,
        ids::START,
        json!({ "path": text(&scratch.path("dialog.wav")) }),
    );
    assert!(started.dialog.is_none(), "Export closes the dialog (R40)");
    finish(&mut shell);
    assert_eq!(
        std::fs::read(scratch.path("called.wav")).unwrap(),
        std::fs::read(scratch.path("dialog.wav")).unwrap(),
        "the same bytes"
    );
    let message = shell.message().expect("the Status Bar says where it went");
    assert!(message.text.contains("dialog.wav"), "{}", message.text);
    assert!(!message.error);

    // R43: choosing and exporting changed nothing in the set.
    assert_eq!(*shell.editor().document(), document);
    assert_eq!(shell.editor().undo_history().len(), history);
}

/// Spec: export R30, R32, R36, R37, R38, R45. Each disabled control and
/// the reason the dialog gives.
#[test]
fn the_dialogs_disabled_states_hold_with_their_reasons() {
    let scratch = Scratch::new("disabled");
    let mut shell = shell_with_a_clip(&scratch);
    let error = shell.run(ids::SET_CHOICES, json!({})).unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
    assert!(
        !shell.can_invoke(ids::START, &json!({})),
        "no dialog, no Export"
    );
    assert!(shell.can_invoke(ids::OPEN_DIALOG, &json!({})));
    status(&mut shell, ids::OPEN_DIALOG, json!({}));

    // R32: Include Return and Main Effects waits for a track.
    let at_start = dialog(&mut shell);
    assert!(disabled(&at_start, "Include Return and Main Effects").is_some());
    let all = status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "rendered_track": "all_tracks" }),
    )
    .dialog
    .unwrap();
    assert_eq!(disabled(&all, "Include Return and Main Effects"), None);

    // R30: Selected Tracks Only with nothing selected.
    let selected = status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "rendered_track": "selected_tracks" }),
    )
    .dialog
    .unwrap();
    assert!(!selected.can_export);
    assert!(selected.reason.unwrap().contains("no track is selected"));
    let main = status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "rendered_track": "main" }),
    )
    .dialog
    .unwrap();
    assert!(main.can_export);
    // A track that isn't there, and the Main track, are not choices.
    for track in [
        json!({ "track": 12_345 }),
        json!({ "track": shell.editor().document().main_id().0 }),
    ] {
        let error = shell
            .run(ids::SET_CHOICES, json!({ "rendered_track": track }))
            .unwrap_err();
        assert!(matches!(error, ShellError::InvalidArgs { .. }), "{error}");
    }

    // R31: a length of zero disables Export.
    let zero = status(&mut shell, ids::SET_CHOICES, json!({ "length": 0 }))
        .dialog
        .unwrap();
    assert!(!zero.can_export);
    assert!(zero.reason.unwrap().contains("Render Length is zero"));
    let restored = status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "start": "2.1.1", "length": BAR }),
    )
    .dialog
    .unwrap();
    assert!(restored.can_export);
    assert_eq!((restored.start, restored.length), (Ticks(BAR), Ticks(BAR)));

    // R36: FLAC holds 16 or 24 bits; choosing it with 32 sets 24.
    let thirty_two = status(&mut shell, ids::SET_CHOICES, json!({ "bit_depth": 32 }))
        .dialog
        .unwrap();
    assert!(
        disabled(&thirty_two, "Dither Options").is_some(),
        "no dither at 32 bits"
    );
    let flac = status(&mut shell, ids::SET_CHOICES, json!({ "file_type": "flac" }))
        .dialog
        .unwrap();
    assert_eq!(flac.choices.bit_depth, 24);
    assert!(disabled(&flac, "Bit Depth 32").is_some());
    assert_eq!(disabled(&flac, "Dither Options"), None);
    let error = shell
        .run(ids::SET_CHOICES, json!({ "file_type": "mp3" }))
        .unwrap_err();
    assert!(error.to_string().contains("Encode MP3"), "{error}");
    let error = shell
        .run(ids::SET_CHOICES, json!({ "sample_rate": 12_345 }))
        .unwrap_err();
    assert!(error.to_string().contains("Sample Rate"), "{error}");

    // R36, R37: Encode PCM off disables its controls, and with Encode MP3
    // off too there is nothing to write.
    let nothing = status(&mut shell, ids::SET_CHOICES, json!({ "encode_pcm": false }))
        .dialog
        .unwrap();
    for control in ["File Type", "Bit Depth", "Dither Options"] {
        assert_eq!(
            disabled(&nothing, control).as_deref(),
            Some("Encode PCM is off."),
            "{control}"
        );
    }
    assert!(!nothing.can_export);
    assert!(nothing.reason.unwrap().contains("Encode PCM or Encode MP3"));

    // R37: MP3 holds some of the Sample Rate list.
    let mp3 = status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "encode_mp3": true, "sample_rate": 96_000 }),
    )
    .dialog
    .unwrap();
    assert!(!mp3.can_export);
    assert!(mp3.reason.unwrap().contains("can't hold 96000 Hz"));
    let mp3 = status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "sample_rate": 44_100 }),
    )
    .dialog
    .unwrap();
    assert!(mp3.can_export);

    // R38: the Video section is drawn disabled.
    for control in ["Create Video", "Video Encoder", "Encoder Settings"] {
        assert!(disabled(&mp3, control).is_some(), "{control}");
    }
    // And Export is refused with the reason, not run.
    status(&mut shell, ids::SET_CHOICES, json!({ "length": 0 }));
    let error = shell.run(ids::START, json!({})).unwrap_err();
    assert!(
        error.to_string().contains("Render Length is zero"),
        "{error}"
    );

    // Cancel closes it.
    status(&mut shell, ids::CLOSE_DIALOG, json!({}));
    assert!(shell.export_dialog().is_none());
}

/// Spec: export R31, R39, R40. Render Start and Length open as the
/// Arrangement's time selection; the other choices are kept as preferences
/// and come back after a restart; the save dialog suggests the set's name
/// with the file type's extension.
#[test]
fn the_choices_are_kept_and_the_save_dialog_suggests_the_sets_name() {
    let scratch = Scratch::new("kept");
    let prefs = Scratch::new("kept-prefs");
    let mut shell = shell_saving_to(&prefs.0);
    add_the_clip(&mut shell, &scratch);
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": 4 * BAR, "end": 5 * BAR }),
        )
        .unwrap();
    let opened = status(&mut shell, ids::OPEN_DIALOG, json!({}))
        .dialog
        .unwrap();
    assert_eq!((opened.start, opened.length), (Ticks(4 * BAR), Ticks(BAR)));

    status(
        &mut shell,
        ids::SET_CHOICES,
        json!({
            "rendered_track": "all_tracks",
            "include_return_and_main_effects": true,
            "render_as_loop": true,
            "convert_to_mono": true,
            "normalize": true,
            "create_analysis_file": true,
            "sample_rate": 96_000,
            "file_type": "aiff",
            "bit_depth": 16,
            "dither": "rectangular",
            "encode_mp3": false,
            "start": 8 * BAR,
            "length": 2 * BAR,
        }),
    );
    // Export asks for the file: the set's name and the type's extension.
    status(&mut shell, ids::START, json!({}));
    let effects = shell.take_effects();
    let [Effect::PickExportPath { file_name, folder }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(file_name, &format!("{}.aif", shell.set_name()));
    assert_eq!(
        *folder, None,
        "a set never saved, so where the system leaves it"
    );
    // Cancelling the save dialog goes back to the dialog with its choices.
    assert!(shell.export_dialog().is_none(), "behind the save dialog");
    let back = status(&mut shell, ids::OPEN_DIALOG, json!({}))
        .dialog
        .unwrap();
    assert_eq!(back.choices.file_type, ids::FileType::Aiff);
    assert_eq!((back.start, back.length), (Ticks(8 * BAR), Ticks(2 * BAR)));
    assert!(shell.export_dialog().is_some());

    // With only Encode MP3 on, the suggestion ends in .mp3.
    status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "encode_pcm": false, "encode_mp3": true, "sample_rate": 48_000 }),
    );
    status(&mut shell, ids::START, json!({}));
    let effects = shell.take_effects();
    let [Effect::PickExportPath { file_name, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(file_name.ends_with(".mp3"), "{file_name}");

    // A restart finds the choices, but not Render Start and Length.
    let mut again = shell_saving_to(&prefs.0);
    add_the_clip(&mut again, &scratch);
    let reopened = status(&mut again, ids::OPEN_DIALOG, json!({}))
        .dialog
        .unwrap();
    assert_eq!(reopened.choices.rendered_track, RenderedTrack::AllTracks);
    assert!(reopened.choices.render_as_loop && reopened.choices.convert_to_mono);
    assert!(reopened.choices.normalize && reopened.choices.create_analysis_file);
    assert_eq!(reopened.choices.sample_rate, Some(48_000));
    assert_eq!(reopened.choices.file_type, ids::FileType::Aiff);
    assert!(!reopened.choices.encode_pcm && reopened.choices.encode_mp3);
    assert_eq!(
        (reopened.start, reopened.length),
        (Ticks::ZERO, Ticks(4 * BAR + BAR / 2)),
        "the whole set again"
    );

    // A single track that no longer exists falls back to Main.
    let track = again.editor().document().top_level().next().unwrap();
    status(
        &mut again,
        ids::SET_CHOICES,
        json!({ "rendered_track": { "track": track.0 } }),
    );
    status(&mut again, ids::START, json!({}));
    again.take_effects();
    let mut elsewhere = shell_saving_to(&prefs.0);
    elsewhere
        .run("track.delete", json!({ "track": track.0 }))
        .unwrap();
    let fallen = status(&mut elsewhere, ids::OPEN_DIALOG, json!({}))
        .dialog
        .unwrap();
    assert_eq!(
        fallen.choices.rendered_track,
        RenderedTrack::Main,
        "the new set has no such track"
    );
}

/// Spec: export R40. Where other files the export writes exist, the shell
/// lists them for the question before the save dialog's path is used.
#[test]
fn the_files_an_export_would_replace_are_listed() {
    let scratch = Scratch::new("conflicts");
    let mut shell = shell_with_a_clip(&scratch);
    status(&mut shell, ids::OPEN_DIALOG, json!({}));
    status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "rendered_track": "all_tracks", "encode_mp3": true, "sample_rate": 44_100 }),
    );
    let path = scratch.path("Mix.wav");
    assert!(
        shell.export_conflicts(&path).is_empty(),
        "nothing there yet"
    );
    // The first track's file and its MP3 exist already.
    // An audio track makes audio, so All Individual Tracks writes it.
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Audio)
        .unwrap()
        .name()
        .to_owned();
    let wav_file = scratch.path(&format!("Mix - {track}.wav"));
    let mp3_file = scratch.path(&format!("Mix - {track}.mp3"));
    std::fs::write(&wav_file, b"a").unwrap();
    std::fs::write(&mp3_file, b"b").unwrap();
    let mut listed = shell.export_conflicts(&path);
    listed.sort();
    assert_eq!(listed, [mp3_file, wav_file]);
    // Main with an MP3: the path itself is the save dialog's question.
    status(
        &mut shell,
        ids::SET_CHOICES,
        json!({ "rendered_track": "main" }),
    );
    std::fs::write(&path, b"c").unwrap();
    std::fs::write(scratch.path("Mix.mp3"), b"d").unwrap();
    assert_eq!(shell.export_conflicts(&path), [scratch.path("Mix.mp3")]);
}

/// Spec: export R41, R42, R44. An export runs on its own thread: its share
/// shows in `export.status`, a second is refused while it runs, and
/// `export.cancel` stops it and leaves nothing behind.
#[test]
fn an_export_runs_on_its_thread_and_cancel_stops_it() {
    let scratch = Scratch::new("cancel");
    let mut shell = shell_with_a_clip(&scratch);
    // A day of silence after the clip: more than any test waits for.
    let day = 172_800 * Ticks::QUARTER.0;
    let args = json!({ "path": text(&scratch.path("Long.wav")), "bit_depth": 16,
                       "sample_rate": 8000, "start": 0, "length": day });
    shell.start_export(args.clone()).unwrap();
    // The share moves while the render goes on.
    loop {
        let running = status(&mut shell, ids::STATUS, json!({})).export;
        if running.is_some_and(|running| running.progress > 0.0) {
            break;
        }
        shell.poll();
        std::thread::yield_now();
    }
    let running = status(&mut shell, ids::STATUS, json!({})).export.unwrap();
    assert!(running.progress < 1.0);
    assert_eq!(running.files.len(), 1);
    assert!(
        running.files[0].ends_with("Long.wav"),
        "{:?}",
        running.files
    );
    // R42: one at a time, and the refusal names the export.
    let error = shell.run(ids::AUDIO, args).unwrap_err();
    assert!(
        matches!(&error, ShellError::Unavailable { reason, .. } if reason.contains("Long.wav")),
        "{error}"
    );
    let error = shell
        .start_export(json!({ "path": text(&scratch.path("Other.wav")) }))
        .unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");

    // Cancel stops it; nothing is left behind.
    assert!(shell.can_invoke(ids::CANCEL, &json!({})));
    status(&mut shell, ids::CANCEL, json!({}));
    finish(&mut shell);
    let ended = shell.take_export_result().expect("it ended");
    assert!(
        ended
            .as_ref()
            .is_err_and(|e| e.to_string().contains("cancelled")),
        "{ended:?}"
    );
    assert_eq!(scratch.names(), ["Tone.wav"]);
    assert!(!shell.can_invoke(ids::CANCEL, &json!({})));
    assert!(status(&mut shell, ids::STATUS, json!({})).export.is_none());
    assert!(!shell.message().unwrap().error, "a cancel is no failure");
    // The next export runs: Cancel was forgotten.
    let done: ExportResult = serde_json::from_value(
        shell
            .run(
                ids::AUDIO,
                json!({ "path": text(&scratch.path("After.wav")), "length": BAR / 4 }),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(done.frames > 0);
}

/// Spec: export R41. A failure shows its reason in the Status Bar and
/// writes nothing.
#[test]
fn a_failed_export_says_why_and_writes_nothing() {
    let scratch = Scratch::new("failed");
    let mut shell = shell_with_a_clip(&scratch);
    status(&mut shell, ids::OPEN_DIALOG, json!({}));
    // The folder isn't there.
    status(
        &mut shell,
        ids::START,
        json!({ "path": text(&scratch.path("nowhere/Mix.wav")) }),
    );
    finish(&mut shell);
    let message = shell.message().unwrap();
    assert!(message.error, "{}", message.text);
    assert!(message.text.contains("no folder"), "{}", message.text);
    assert_eq!(scratch.names(), ["Tone.wav"]);
}

/// Spec: export R33, R37. Convert to Mono and Encode MP3 go through track
/// exports: each track's file is one channel, with its MP3 beside it.
#[test]
fn a_track_export_converts_to_mono_and_writes_an_mp3_beside_each_file() {
    let scratch = Scratch::new("tracks");
    let mut shell = shell_with_a_clip(&scratch);
    let result: ExportResult = serde_json::from_value(
        shell
            .run(
                ids::AUDIO,
                json!({ "path": text(&scratch.path("Mix.flac")), "rendered_track": "all_tracks",
                        "convert_to_mono": true, "encode_mp3": true, "sample_rate": 44_100 }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result.channels, 1);
    assert!(
        result.files.len() >= 2 && result.files.len() % 2 == 0,
        "{:?}",
        result.files
    );
    for pair in result.files.chunks(2) {
        assert!(
            pair[0].ends_with(".flac") && pair[1].ends_with(".mp3"),
            "{pair:?}"
        );
        for file in pair {
            let decoded = bl_media::decode_file(file).unwrap();
            assert_eq!(decoded.channel_count(), 1, "{file}");
        }
    }
    // Both file names are refused if either exists, before anything renders.
    let again = shell
        .run(
            ids::AUDIO,
            json!({ "path": text(&scratch.path("Mix.flac")), "rendered_track": "all_tracks",
                    "encode_mp3": true, "sample_rate": 44_100 }),
        )
        .unwrap_err();
    assert!(again.to_string().contains("replace"), "{again}");
    // And MP3 at a rate it can't hold is refused.
    let error = shell
        .run(
            ids::AUDIO,
            json!({ "path": text(&scratch.path("Other.wav")), "encode_mp3": true,
                    "sample_rate": 96_000 }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("MP3"), "{error}");
}

/// Spec: export R34. Create Analysis File leaves each written file's peaks
/// and analysis in the caches under the file's content hash, and nothing
/// beside the file; without it, nothing is made.
#[test]
fn create_analysis_file_keeps_the_peaks_and_analysis_by_content_hash() {
    let scratch = Scratch::new("analysis");
    let caches = Scratch::new("analysis-caches");
    let mut shell = shell_with_a_clip(&scratch);
    shell.use_analysis_options(bl_analysis::cache::AnalysisOptions::in_dir(
        caches.path("analysis"),
    ));
    shell.use_peak_options(bl_media::PeakCacheOptions::in_dir(caches.path("peaks")));

    let without = scratch.path("Without.wav");
    shell
        .run(
            ids::AUDIO,
            json!({ "path": text(&without), "length": BAR / 2 }),
        )
        .unwrap();
    let with = scratch.path("With.wav");
    let result: ExportResult = serde_json::from_value(
        shell
            .run(
                ids::AUDIO,
                json!({ "path": text(&with), "length": BAR, "create_analysis_file": true,
                        "encode_mp3": true, "sample_rate": 48_000 }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result.files.len(), 2);
    let store = bl_media::MediaStore::new();
    for file in &result.files {
        let hash = bl_model::ContentHash(*store.import(file).unwrap().as_bytes());
        shell.wait_analysis(hash).expect("the analysis is made");
        shell.wait_peaks(hash).expect("the peaks are made");
        let name = store.import(file).unwrap().to_string();
        let found = |folder: &str, extension: &str| {
            std::fs::read_dir(caches.path(folder))
                .into_iter()
                .flatten()
                .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
                .any(|entry| {
                    entry.unwrap().file_name().to_string_lossy() == format!("{name}.{extension}")
                })
        };
        assert!(found("analysis", "blanalysis"), "the analysis of {file}");
        assert!(found("peaks", "blpeaks"), "the peaks of {file}");
    }
    // Nothing is written beside the files, and the file made without the
    // switch has none of its own.
    assert_eq!(
        scratch.names(),
        ["Tone.wav", "With.mp3", "With.wav", "Without.wav"]
    );
    let hash = store.import(&without).unwrap();
    let in_caches = std::fs::read_dir(caches.path("analysis"))
        .into_iter()
        .flatten()
        .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
        .any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&hash.to_string())
        });
    assert!(!in_caches, "no analysis without the switch");
}

/// Spec: export R22. Normalize applies to each file of a multi-track export
/// separately: with the Main fader down, the Main file and the track's
/// both reach full scale.
#[test]
fn normalize_applies_to_each_file_of_a_multi_track_export_separately() {
    let scratch = Scratch::new("normalize");
    let mut shell = shell_with_a_clip(&scratch);
    let main = shell.editor().document().main_id();
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": main.0, "volume": 0.25 }),
        )
        .unwrap();
    let result: ExportResult = serde_json::from_value(
        shell
            .run(
                ids::AUDIO,
                json!({ "path": text(&scratch.path("Mix.wav")), "rendered_track": "all_tracks",
                        "normalize": true, "bit_depth": 32 }),
            )
            .unwrap(),
    )
    .unwrap();
    let mut loud = 0;
    for file in &result.files {
        let decoded = bl_media::decode_file(file).unwrap();
        let peak = (0..decoded.channel_count())
            .flat_map(|c| decoded.channel(c).unwrap().iter().copied())
            .fold(0.0f32, |peak, s| peak.max(s.abs()));
        // A track that plays nothing stays silent; the rest are at full
        // scale, whatever their level was.
        assert!(peak == 0.0 || peak > 0.999, "{file}: {peak}");
        loud += usize::from(peak > 0.999);
    }
    assert!(loud >= 2, "the Main file and the audio track's: {loud}");
}
