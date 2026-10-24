//! The Sample Editor in the shell (WP-4.10): what an audio clip shows, what
//! the editor has selected, what its keys and menu items mean, and that
//! every action it makes is a registered command. The rules are in the
//! sample editor spec.

use std::path::{Path, PathBuf};

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

const QUARTER: i64 = Ticks::QUARTER.get();

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-sample-shell-{}-{name}", std::process::id()));
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
/// apart, each clip warped, and the Clip View open: the clips' IDs.
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
        .unwrap()
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
    shell.run("view.toggle_clip_view", json!({})).unwrap();
    (shell, ids)
}

fn markers(shell: &Shell, clip: ClipId) -> Vec<(f64, i64)> {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio
            .warp_markers
            .iter()
            .map(|m| (m.time, m.beat.get()))
            .collect(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

fn state(shell: &mut Shell) -> Value {
    shell.run("clip_view.sample_editor", json!({})).unwrap()
}

// Spec: sample-editor R1
#[test]
fn an_audio_clip_shows_the_sample_editor_and_the_tabs_are_view_state() {
    let scratch = Scratch::new("tabs");
    let (mut shell, clips) = shell_with(&scratch, &[(1, 220.0)]);
    let clip = clips[0];
    let steps = shell.editor().undo_history().len();
    // Nothing shows until a clip is selected; then the Clip View's editor is
    // the Sample Editor, and the keys of its context apply.
    assert_eq!(state(&mut shell)["clip"], Value::Null);
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    assert_eq!(state(&mut shell)["clip"], json!(clip));
    shell.set_focused_area(bl_shell::Area::Detail);
    let contexts = shell.active_contexts();
    assert!(contexts.contains(&Context::SampleEditor), "{contexts:?}");
    assert!(!contexts.contains(&Context::NoteEditor));
    // The Sample tab is the content editor's (Option+Shift+1); Option+Tab
    // goes on to the Envelopes tab, and again back.
    shell
        .run("view.clip_tab", json!({ "tab": "envelopes" }))
        .unwrap();
    let contexts = shell.active_contexts();
    assert!(contexts.contains(&Context::Envelope) && !contexts.contains(&Context::SampleEditor));
    shell.run("view.clip_tab", json!({})).unwrap();
    assert!(shell.active_contexts().contains(&Context::SampleEditor));
    assert_eq!(
        shell.editor().undo_history().len(),
        steps,
        "view state, never a step"
    );
}

// Spec: sample-editor R6, R10
#[test]
fn selected_markers_follow_the_map_through_edits_and_the_keys_act_on_them() {
    let scratch = Scratch::new("follow");
    let (mut shell, clips) = shell_with(&scratch, &[(2, 220.0)]);
    let clip = clips[0];
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    for time in [0.5, 1.0, 1.5] {
        shell
            .run(
                "clip.insert_warp_marker",
                json!({ "clips": [clip], "time": time }),
            )
            .unwrap();
    }
    shell
        .run("clip_view.select_warp_markers", json!({ "markers": [2] }))
        .unwrap();
    assert_eq!(state(&mut shell)["markers"], json!([2]));
    // A marker inserted before the selected one moves its place on.
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "time": 0.25 }),
        )
        .unwrap();
    assert_eq!(state(&mut shell)["markers"], json!([3]));
    // Deleting another marker moves it back; deleting it leaves none.
    shell
        .run(
            "clip.delete_warp_markers",
            json!({ "clips": [clip], "markers": [1] }),
        )
        .unwrap();
    assert_eq!(state(&mut shell)["markers"], json!([2]));
    // Delete, as a key or a menu item, acts on the selected marker.
    let steps = shell.editor().undo_history().len();
    shell.invoke("clip.delete_warp_markers", json!({})).unwrap();
    assert_eq!(markers(&shell, clip).len(), 3);
    assert_eq!(state(&mut shell)["markers"], json!([]));
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    // With nothing selected it is refused, and says what to do.
    let error = shell
        .invoke("clip.delete_warp_markers", json!({}))
        .unwrap_err();
    assert!(
        error.to_string().contains("select a warp marker"),
        "{error}"
    );
    // Insert Warp Marker needs the insert marker; with a time selection it
    // puts markers at its transients instead.
    assert!(shell.invoke("clip.insert_warp_marker", json!({})).is_err());
    shell
        .run("clip_view.set_insert_marker", json!({ "time": 1.25 }))
        .unwrap();
    shell.invoke("clip.insert_warp_marker", json!({})).unwrap();
    assert!(
        markers(&shell, clip)
            .iter()
            .any(|m| (m.0 - 1.25).abs() < 1e-9)
    );
    // The insert marker and the time selection replace each other.
    shell
        .run("clip_view.select_time", json!({ "start": 0.5, "end": 1.0 }))
        .unwrap();
    let now = state(&mut shell);
    assert_eq!(now["insert_marker"], Value::Null);
    assert_eq!(now["time_selection"][1]["time"], 1.0);
    shell
        .run(
            "clip_view.set_insert_marker",
            json!({ "beat": 2 * QUARTER }),
        )
        .unwrap();
    assert_eq!(state(&mut shell)["time_selection"], Value::Null);
    assert_eq!(state(&mut shell)["insert_marker"]["time"], 1.0);
}

// Spec: sample-editor R19, warping R11
#[test]
fn marker_edits_reach_every_selected_clip_of_one_length() {
    let scratch = Scratch::new("several");
    let (mut shell, clips) = shell_with(&scratch, &[(1, 220.0), (1, 330.0), (2, 440.0)]);
    let (a, b, longer) = (clips[0], clips[1], clips[2]);
    shell
        .run(
            "arrangement.select_clips",
            json!({ "clips": [a, b, longer] }),
        )
        .unwrap();
    // The editor shows the first; the edit reaches the one of its length.
    assert_eq!(state(&mut shell)["clip"], json!(a));
    assert_eq!(shell.sample_edit_clips(a, &[]), [a, b]);
    shell
        .run("clip_view.set_insert_marker", json!({ "time": 0.5 }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let steps = shell.editor().undo_history().len();
    shell.invoke("clip.insert_warp_marker", json!({})).unwrap();
    assert_eq!(markers(&shell, a).len(), 2);
    assert_eq!(markers(&shell, b).len(), 2);
    assert_eq!(
        markers(&shell, longer).len(),
        1,
        "another length is left alone"
    );
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    // A clip without that many markers is left out of an edit that names one.
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [a], "time": 0.75 }),
        )
        .unwrap();
    assert_eq!(shell.sample_edit_clips(a, &[2]), [a]);
}

// Spec: linked-track editing R10, sample-editor R19. Selecting a clip on a
// linked track selects its matching clips, and a warp-marker edit reaches
// those whose file is as long as the shown clip's, and not one of another
// length.
#[test]
fn a_marker_edit_reaches_a_linked_clip_of_the_same_length_and_not_another() {
    let scratch = Scratch::new("linked");
    let (mut shell, clips) = shell_with(&scratch, &[(1, 220.0)]);
    let a = clips[0];
    let audio: Vec<_> = shell
        .editor()
        .document()
        .tracks()
        .filter(|t| t.kind() == TrackKind::Audio)
        .map(|t| t.id())
        .collect();
    let third = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": third, "kind": "audio", "name": "Third", "index": 4 }),
        )
        .unwrap();
    let (second, third_track) = (audio[1], third);
    // The same length of file on the second track; a longer file on the
    // third, cut to the same stretch of the Arrangement.
    let mut placed = Vec::new();
    for (track, seconds) in [(second, 1), (third_track, 2)] {
        let path = scratch.0.join(format!("Linked {seconds}.wav"));
        wav(&path, seconds, 440.0);
        shell
            .run(
                "browser.add_to_arrangement",
                json!({ "paths": [path.display().to_string()], "track": track, "at": 0 }),
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
        shell
            .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
            .unwrap();
        placed.push(clip);
    }
    let length = shell.editor().document().clip(a).unwrap().length;
    shell
        .run(
            "clip.set_length",
            json!({ "clip": placed[1], "length": length, "linked": false }),
        )
        .unwrap();
    shell
        .run(
            "track.link",
            json!({ "tracks": [audio[0], second, third_track] }),
        )
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [a] }))
        .unwrap();
    assert_eq!(shell.arrangement().selected_clips().len(), 3);
    assert_eq!(shell.sample_edit_clips(a, &[]), [a, placed[0]]);
    shell
        .run("clip_view.set_insert_marker", json!({ "time": 0.5 }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    shell.invoke("clip.insert_warp_marker", json!({})).unwrap();
    assert_eq!(markers(&shell, a).len(), 2);
    assert_eq!(markers(&shell, placed[0]).len(), 2);
    assert_eq!(markers(&shell, placed[1]).len(), 1, "another length");
}

// Spec: sample-editor R7, R8
#[test]
fn a_gesture_that_inserts_a_marker_then_moves_it_is_one_undo_step_and_esc_undoes_it() {
    let scratch = Scratch::new("gesture");
    let (mut shell, clips) = shell_with(&scratch, &[(2, 220.0)]);
    let clip = clips[0];
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let steps = shell.editor().undo_history().len();
    let insert = Invocation::new(
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "time": 1.0 }),
    );
    let moved = |by: i64| {
        Invocation::new(
            "clip.move_warp_markers",
            json!({ "clips": [clip], "markers": [1], "by": by }),
        )
    };
    shell.gesture(GesturePhase::Update, Some(&insert)).unwrap();
    for by in [QUARTER, 2 * QUARTER, 3 * QUARTER] {
        shell
            .gesture(GesturePhase::Update, Some(&moved(by)))
            .unwrap();
        // Each frame is measured from where the gesture began.
        assert_eq!(markers(&shell, clip)[1].1, 2 * QUARTER + by);
    }
    // A frame the model refuses keeps the last that worked.
    assert!(
        shell
            .gesture(GesturePhase::Update, Some(&moved(-10 * QUARTER)))
            .is_err()
    );
    assert_eq!(markers(&shell, clip)[1].1, 2 * QUARTER + 3 * QUARTER);
    shell
        .gesture(GesturePhase::Commit, Some(&moved(QUARTER)))
        .unwrap();
    assert_eq!(markers(&shell, clip), [(0.0, 0), (1.0, 3 * QUARTER)]);
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(markers(&shell, clip).len(), 1);
    // Esc puts everything back.
    shell.gesture(GesturePhase::Update, Some(&insert)).unwrap();
    shell
        .gesture(GesturePhase::Update, Some(&moved(QUARTER)))
        .unwrap();
    shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(markers(&shell, clip).len(), 1);
}

// Spec: sample-editor R2, browser R6
#[test]
fn show_in_browser_selects_the_file_in_the_place_it_lies_in() {
    let scratch = Scratch::new("show");
    let (mut shell, clips) = shell_with(&scratch, &[(1, 220.0)]);
    let clip = clips[0];
    // The file lies in no place yet.
    let error = shell
        .run("browser.show_file", json!({ "clip": clip }))
        .unwrap_err();
    assert!(error.to_string().contains("browser.add_folder"), "{error}");
    shell
        .run(
            "browser.add_folder",
            json!({ "path": scratch.0.display().to_string() }),
        )
        .unwrap();
    let before = shell.editor().undo_history().len();
    let shown = shell
        .run("browser.show_file", json!({ "clip": clip }))
        .unwrap();
    let file = scratch.0.join("File 0.wav");
    let selected: Vec<String> = shown["selected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(selected, [file.display().to_string()]);
    assert_eq!(shell.editor().undo_history().len(), before);
    assert!(shell.browser_visible());
}

// Spec: sample-editor R21
#[test]
fn every_sample_editor_action_is_a_registered_command() {
    let scratch = Scratch::new("registry");
    let (shell, _) = shell_with(&scratch, &[]);
    let registry = shell.registry();
    // What the editor's gestures, keys and menu items run.
    for id in [
        "clip.insert_warp_marker",
        "clip.insert_warp_markers",
        "clip.delete_warp_markers",
        "clip.move_warp_markers",
        "clip.shift_warp_marker",
        "clip.insert_transient",
        "clip.delete_transients",
        "clip.move_transient",
        "clip.reset_transients",
        "clip.set_region",
        "clip.set_loop",
        "clip_view.select_warp_markers",
        "clip_view.select_transients",
        "clip_view.set_insert_marker",
        "clip_view.select_time",
        "clip_view.sample_view",
        "clip_view.sample_editor",
        "arrangement.follow",
        "browser.show_file",
        "transport.locate",
    ] {
        assert!(registry.contains(id), "{id} isn't registered");
    }
    // Selecting, zooming and Follow change the view and nothing else.
    for id in [
        "clip_view.select_warp_markers",
        "clip_view.select_transients",
        "clip_view.set_insert_marker",
        "clip_view.select_time",
        "clip_view.sample_view",
        "clip_view.sample_editor",
    ] {
        assert_eq!(
            registry.get(id).unwrap().permission,
            Some(bl_commands::Permission::Read),
            "{id}"
        );
    }
}
