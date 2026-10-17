//! The warp map in the app (WP-4.1): a marker edit on several clips is one
//! undo step when their files are as long as each other's, and is refused,
//! changing nothing, when they aren't or when it would leave a file. The
//! rules are in the warping spec.

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
        let path = std::env::temp_dir().join(format!("bl-warping-{}-{name}", std::process::id()));
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

/// A shell with the files `(seconds, hz)` on its first audio track, a bar
/// apart, each clip warped: the clips' IDs.
fn shell_with(scratch: &Scratch, files: &[(u32, f64)]) -> (Shell, Vec<ClipId>) {
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
    for (n, &(seconds, hz)) in files.iter().enumerate() {
        let path = scratch.0.join(format!("File {n}.wav"));
        wav(&path, seconds, hz);
        shell
            .run(
                "browser.add_to_arrangement",
                json!({
                    "paths": [path.display().to_string()],
                    "track": track,
                    "at": Ticks::WHOLE.get() * 4 * n as i64,
                }),
            )
            .unwrap();
    }
    let mut clips: Vec<_> = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .map(|c| (c.start, c.id))
        .collect();
    clips.sort();
    let ids: Vec<ClipId> = clips.into_iter().map(|(_, id)| id).collect();
    for id in &ids {
        shell
            .run("clip.set_warp", json!({ "clip": id, "warp": true }))
            .unwrap();
    }
    (shell, ids)
}

fn markers(shell: &Shell, clip: ClipId) -> usize {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.warp_markers.len(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

// Spec: warping R7, R11
#[test]
fn a_marker_edit_on_several_clips_of_one_length_is_one_step() {
    let scratch = Scratch::new("same");
    let (mut shell, clips) = shell_with(&scratch, &[(1, 220.0), (1, 330.0), (2, 440.0)]);
    let steps = shell.editor().undo_history().len();
    let (a, b, longer) = (clips[0], clips[1], clips[2]);

    // Two files of one second: the marker goes into both, in one step.
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [a, b], "beat": Ticks::QUARTER.get() }),
        )
        .unwrap();
    assert_eq!((markers(&shell, a), markers(&shell, b)), (2, 2));
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!((markers(&shell, a), markers(&shell, b)), (1, 1));

    // A file of another length refuses the whole edit.
    let error = shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [a, longer], "beat": Ticks::QUARTER.get() }),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("as long"), "{error}");
    assert_eq!(
        (markers(&shell, a), markers(&shell, longer)),
        (1, 1),
        "neither changed"
    );
    assert_eq!(shell.editor().undo_history().len(), steps);

    // A marker can't be put, or shifted, beyond the end of its file.
    for (command, args) in [
        (
            "clip.insert_warp_marker",
            json!({ "clips": [a], "time": 1.5 }),
        ),
        (
            "clip.shift_warp_marker",
            json!({ "clips": [a], "marker": 0, "by": 1.5 }),
        ),
    ] {
        let error = shell.run(command, args).unwrap_err().to_string();
        assert!(error.contains("leave the file"), "{error}");
    }
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [longer], "time": 1.5 }),
        )
        .expect("inside the two-second file");
    assert_eq!(markers(&shell, longer), 2);
}
