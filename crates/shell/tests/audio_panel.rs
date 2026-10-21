//! The Audio Utilities panel's own commands in the app (WP-4.10b):
//! replacing a clip's file, and a file edited in the external sample editor
//! reaching every clip that played it. The rules are in the sample editor
//! spec, R16 and R18.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-audio-panel-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of `seconds` at 48 kHz holding a quiet sine
/// at `hz`, so two files of one length still differ.
fn wav(path: &Path, seconds: u32, hz: f64) {
    let frames = seconds * 48_000;
    let data = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
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
        let phase = f64::from(i) * hz / 48_000.0 * std::f64::consts::TAU;
        bytes.extend_from_slice(&((phase.sin() * 32.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A command that does nothing, so the test's own writing of the file is the
/// edit: the editor is faked, and the test rewrites the file.
fn harmless_editor() -> &'static str {
    if cfg!(windows) {
        "cmd /C rem"
    } else {
        "/bin/sh -c exit"
    }
}

/// A shell with one audio clip on its first audio track: the clip and the
/// file it plays.
fn shell_with(scratch: &Scratch) -> (Shell, ClipId, PathBuf) {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .expect("the default set has audio tracks")
        .id();
    let path = scratch.0.join("Tone.wav");
    wav(&path, 2, 440.0);
    shell
        .run(
            "browser.add_to_arrangement",
            json!({
                "paths": [path.display().to_string()],
                "track": track,
                "at": Ticks::ZERO.get(),
            }),
        )
        .unwrap();
    let clip = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .id;
    (shell, clip, path)
}

fn audio(shell: &Shell, clip: ClipId) -> bl_model::AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

/// The clips of the set that play `media`.
fn clips_of(shell: &Shell, media: bl_model::ContentHash) -> Vec<ClipId> {
    shell
        .editor()
        .document()
        .tracks()
        .flat_map(|track| track.clips().chain(track.slots().map(|(_, clip)| clip)))
        .filter(|clip| matches!(&clip.data, ClipData::Audio(audio) if audio.media == media))
        .map(|clip| clip.id)
        .collect()
}

/// Replacing a clip's file keeps its gain, and its warp markers only when
/// the new file is exactly as long as the old one (R18, warping R27).
#[test]
fn replacing_the_sample_keeps_the_gain_and_the_markers_of_one_length() {
    let scratch = Scratch::new("replace");
    let (mut shell, clip, _) = shell_with(&scratch);
    shell
        .run("clip.set_gain", json!({ "clip": clip, "gain": 0.5 }))
        .unwrap();
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    let markers = audio(&shell, clip).warp_markers.len();
    assert!(markers > 0, "the clip is warped");

    // A file of the same length: the gain and the markers are kept.
    let same = scratch.0.join("Same.wav");
    wav(&same, 2, 880.0);
    let before = shell.editor().undo_history().len();
    shell
        .run(
            "clip.replace_sample",
            json!({ "clip": clip, "path": same.display().to_string() }),
        )
        .unwrap();
    let after = audio(&shell, clip);
    assert_eq!(after.gain, 0.5, "the gain is kept");
    assert_eq!(
        after.warp_markers.len(),
        markers,
        "the warp markers are kept at the same length"
    );
    assert_eq!(
        shell.editor().undo_history().len(),
        before + 1,
        "the replacement is one undo step"
    );

    // A file of another length: the markers are dropped.
    let other = scratch.0.join("Other.wav");
    wav(&other, 3, 880.0);
    shell
        .run(
            "clip.replace_sample",
            json!({ "clip": clip, "path": other.display().to_string() }),
        )
        .unwrap();
    let after = audio(&shell, clip);
    assert_eq!(after.gain, 0.5, "the gain is kept");
    assert!(
        after.warp_markers.is_empty(),
        "the warp markers are dropped at another length"
    );
    assert!(!after.warp, "the clip isn't warped any more");
}

/// A file edited in the external sample editor reaches every clip of the set
/// that played it, in one undo step, and keeps its warp markers only when
/// its length is unchanged (R16, warping R27).
#[test]
fn an_edited_file_reaches_every_clip_of_it_in_one_step() {
    let scratch = Scratch::new("edited");
    let (mut shell, clip, path) = shell_with(&scratch);
    // A second clip of the same file: the edit reaches every clip of it.
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.clips().any(|other| other.id == clip))
        .expect("the clip is on a track")
        .id();
    shell
        .run(
            "browser.add_to_arrangement",
            json!({
                "paths": [path.display().to_string()],
                "track": track,
                "at": Ticks::WHOLE.get() * 8,
            }),
        )
        .unwrap();
    let clips = clips_of(&shell, audio(&shell, clip).media);
    assert_eq!(clips.len(), 2, "two clips play the file");
    for other in &clips {
        shell
            .run("clip.set_warp", json!({ "clip": other, "warp": true }))
            .unwrap();
    }
    let markers: Vec<usize> = clips
        .iter()
        .map(|other| audio(&shell, *other).warp_markers.len())
        .collect();
    assert!(
        markers.iter().all(|count| *count > 0),
        "the clips are warped"
    );
    shell
        .run(
            "settings.set_sample_editor",
            json!({ "path": harmless_editor() }),
        )
        .unwrap();
    let opened = shell
        .run("clip.edit_sample", json!({ "clip": clip }))
        .unwrap();
    assert!(
        opened["opened"].as_bool().unwrap_or(false),
        "the editor was started"
    );

    // The file is edited: another tone, of the same length.
    let media = audio(&shell, clip).media;
    wav(&path, 2, 880.0);
    let before = shell.editor().undo_history().len();
    shell.check_edited_samples();
    for (index, other) in clips.iter().enumerate() {
        let after = audio(&shell, *other);
        assert_ne!(after.media, media, "the clip plays the edited file");
        assert_eq!(
            after.warp_markers.len(),
            markers[index],
            "the warp markers are kept at the same length"
        );
    }
    assert_eq!(
        shell.editor().undo_history().len(),
        before + 1,
        "the change is one undo step"
    );
}
