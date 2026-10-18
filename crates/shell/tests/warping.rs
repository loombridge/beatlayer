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

/// A shell and its first audio track.
fn new_shell(scratch: &Scratch) -> (Shell, bl_model::TrackId) {
    let shell = Shell::new(
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
    (shell, track)
}

/// A shell with the files `(seconds, hz)` on its first audio track, a bar
/// apart, each clip warped: the clips' IDs.
fn shell_with(scratch: &Scratch, files: &[(u32, f64)]) -> (Shell, Vec<ClipId>) {
    let (mut shell, track) = new_shell(scratch);
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
        // Four quarter notes at 120 BPM are two seconds into the file.
        (
            "clip.insert_warp_marker",
            json!({ "clips": [a], "beat": 4 * Ticks::QUARTER.get() }),
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

/// A shell with the generated drum loop (`tests/fixtures/gen/gen_drums.py`)
/// on its first audio track, warped, and its analysis done: the clip.
fn drum_clip(scratch: &Scratch) -> (Shell, ClipId) {
    let (mut shell, track) = new_shell(scratch);
    let path = scratch.0.join("drum-loop.wav");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/drums/drum-loop.wav"),
        &path,
    )
    .unwrap();
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
    wait_for_analysis(&mut shell, clip);
    (shell, clip)
}

fn audio(shell: &Shell, clip: ClipId) -> bl_model::AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

fn wait_for_analysis(shell: &mut Shell, clip: ClipId) {
    let media = audio(shell, clip).media;
    shell.wait_analysis(media).expect("the file is analysed");
}

/// `clip.transients` as `(time, by_hand)` pairs, with whether the analysis
/// was ready.
fn transients(shell: &mut Shell, clip: ClipId) -> (bool, Vec<(f64, bool)>) {
    let result = shell
        .run("clip.transients", json!({ "clip": clip }))
        .unwrap();
    let list = result["transients"].as_array().unwrap();
    (
        result["analysis_ready"].as_bool().unwrap(),
        list.iter()
            .map(|t| (t["time"].as_f64().unwrap(), t["by_hand"].as_bool().unwrap()))
            .collect(),
    )
}

fn times(shell: &mut Shell, clip: ClipId) -> Vec<f64> {
    transients(shell, clip).1.iter().map(|t| t.0).collect()
}

// Spec: warping R23, R24
#[test]
fn transient_edits_are_one_step_saved_and_reset() {
    let scratch = Scratch::new("transients");
    let (mut shell, clip) = drum_clip(&scratch);
    let (ready, shown) = transients(&mut shell, clip);
    assert!(ready);
    let detected = times(&mut shell, clip);
    assert_eq!(detected.len(), 9, "the loop has nine hits");
    assert!(shown.iter().all(|t| !t.1), "none is by hand");
    // A warped clip's transients carry the beat they play on: the second
    // hit is a quarter of a second, half a quarter note, into the loop.
    let listed = shell
        .run("clip.transients", json!({ "clip": clip }))
        .unwrap();
    assert_eq!(listed["transients"][1]["beat"], Ticks::QUARTER.get() / 2);

    let steps = |shell: &Shell| shell.editor().undo_history().len();
    let start = steps(&shell);
    shell
        .run(
            "clip.insert_transient",
            json!({ "clip": clip, "time": 0.1 }),
        )
        .unwrap();
    assert_eq!(steps(&shell), start + 1);
    let (_, shown) = transients(&mut shell, clip);
    assert_eq!(shown.len(), 10);
    assert_eq!(shown[1], (0.1, true));

    shell
        .run(
            "clip.delete_transients",
            json!({ "clip": clip, "times": [detected[3]] }),
        )
        .unwrap();
    assert_eq!(steps(&shell), start + 2);
    assert!(!times(&mut shell, clip).contains(&detected[3]));

    // Only a transient that exists can be deleted or moved, none can be put
    // on another or past the end of the two-second file, and a range's
    // markers can't either; a refusal is no step.
    for (command, args) in [
        (
            "clip.insert_transient",
            json!({ "clip": clip, "time": detected[0] }),
        ),
        (
            "clip.move_transient",
            json!({ "clip": clip, "from": 0.1, "to": detected[0] }),
        ),
        (
            "clip.insert_warp_markers",
            json!({ "clip": clip, "from": 0, "to": 8 * Ticks::QUARTER.get() }),
        ),
        (
            "clip.delete_transients",
            json!({ "clip": clip, "times": [0.37] }),
        ),
        (
            "clip.move_transient",
            json!({ "clip": clip, "from": 0.37, "to": 0.4 }),
        ),
        (
            "clip.move_transient",
            json!({ "clip": clip, "from": 0.1, "to": 3.0 }),
        ),
        (
            "clip.insert_transient",
            json!({ "clip": clip, "time": 3.0 }),
        ),
    ] {
        assert!(shell.run(command, args).is_err(), "{command}");
    }
    assert_eq!(steps(&shell), start + 2);

    shell
        .run(
            "clip.move_transient",
            json!({ "clip": clip, "from": 0.1, "to": 0.11 }),
        )
        .unwrap();
    assert_eq!(steps(&shell), start + 3);
    assert_eq!(transients(&mut shell, clip).1[1], (0.11, true));
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(transients(&mut shell, clip).1[1], (0.1, true));
    shell.run("edit.redo", json!({})).unwrap();

    // The edits are the clip's, saved with the set as times in the file.
    let edits = audio(&shell, clip).transient_edits;
    assert_eq!(
        (edits.added, edits.removed),
        (vec![0.11], vec![detected[3]])
    );

    shell
        .run("clip.reset_transients", json!({ "clip": clip }))
        .unwrap();
    assert_eq!(steps(&shell), start + 4);
    assert_eq!(times(&mut shell, clip), detected);
    assert!(audio(&shell, clip).transient_edits.is_empty());
}

// Spec: warping R25, R26
#[test]
fn markers_go_on_transients_over_a_range_and_beside_a_pseudo_marker() {
    let scratch = Scratch::new("pseudo");
    let (mut shell, clip) = drum_clip(&scratch);
    let detected = times(&mut shell, clip);
    // Over the whole two-second loop, a marker at each of the nine hits,
    // beside the one at the clip's start (the first hit lands a few samples
    // after it).
    shell
        .run(
            "clip.insert_warp_markers",
            json!({ "clip": clip, "from": 0, "to": 4 * Ticks::QUARTER.get() }),
        )
        .unwrap();
    let marked: Vec<f64> = audio(&shell, clip)
        .warp_markers
        .iter()
        .map(|m| m.time)
        .collect();
    assert_eq!(marked.len(), 10);
    for (marker, hit) in marked[1..].iter().zip(&detected) {
        assert!((marker - hit).abs() < 1e-9, "{marker} vs {hit}");
    }
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(markers(&shell, clip), 1);

    // Cmd on a pseudo-warp marker puts markers on the transients beside it.
    let (hat, kick) = (0.5, 0.75);
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "time": hat, "pin_neighbours": true }),
        )
        .unwrap();
    let marked: Vec<f64> = audio(&shell, clip)
        .warp_markers
        .iter()
        .map(|m| m.time)
        .collect();
    assert_eq!(marked.len(), 4, "{marked:?} around {hat} and {kick}");
    assert!((marked[1] - 0.25).abs() < 0.001 && (marked[3] - kick).abs() < 0.001);
}

// Spec: warping R27
#[test]
fn a_replaced_sample_keeps_its_markers_only_at_the_same_length() {
    let scratch = Scratch::new("replaced");
    let (mut shell, clip) = drum_clip(&scratch);
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "time": 0.5 }),
        )
        .unwrap();
    shell
        .run(
            "clip.insert_transient",
            json!({ "clip": clip, "time": 0.1 }),
        )
        .unwrap();
    // The same length: the drum loop with its first 0.3 s silenced. The
    // other is a second shorter.
    let mut bytes = std::fs::read(scratch.0.join("drum-loop.wav")).unwrap();
    bytes[44..44 + 28_800].fill(0);
    std::fs::write(scratch.0.join("same.wav"), bytes).unwrap();
    wav(&scratch.0.join("shorter.wav"), 1, 220.0);
    let import = |shell: &Shell, name: &str| {
        let hash = shell.media_store().import(scratch.0.join(name)).unwrap();
        bl_model::ContentHash(*hash.as_bytes())
    };
    let replace = |shell: &mut Shell, media| {
        let mut next = audio(shell, clip);
        next.media = media;
        shell
            .run(
                "clip.set_data",
                json!({ "clip": clip, "data": ClipData::Audio(next) }),
            )
            .unwrap();
        audio(shell, clip)
    };

    let before = audio(&shell, clip);
    let media = import(&shell, "same.wav");
    let same = replace(&mut shell, media);
    assert!(same.warp);
    assert_eq!(same.warp_markers, before.warp_markers, "the markers stay");
    assert!(
        same.transient_edits.is_empty(),
        "the edits belonged to the old file"
    );

    let media = import(&shell, "shorter.wav");
    let shorter = replace(&mut shell, media);
    assert!(!shorter.warp && shorter.warp_markers.is_empty() && shorter.tempo.is_none());
    // The Start marker keeps its place in the audio, now in seconds.
    assert_eq!(shorter.start, 0.0);
}

// Spec: warping R49, R50
#[test]
fn quantising_audio_moves_each_lines_nearest_transient() {
    let scratch = Scratch::new("quantise-audio");
    let (mut shell, clip) = drum_clip(&scratch);
    let steps = |shell: &Shell| shell.editor().undo_history().len();
    let initial_markers = audio(&shell, clip).warp_markers;
    let initial_steps = steps(&shell);

    // 100% quantise: moves nearest transient to each grid line.
    shell
        .run(
            "clip.quantize_audio",
            json!({ "clip": clip, "amount": 100, "grid": 480 }),
        )
        .unwrap();
    assert_eq!(steps(&shell), initial_steps + 1, "one undo step");
    let quantised_100 = audio(&shell, clip).warp_markers;
    assert_ne!(initial_markers, quantised_100);
    for m in &quantised_100 {
        assert_eq!(
            m.beat.get() % 480,
            0,
            "marker at beat {} landed on grid",
            m.beat.get()
        );
    }

    // Undo restores the initial markers
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(audio(&shell, clip).warp_markers, initial_markers);

    // Redo restores quantised markers
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(audio(&shell, clip).warp_markers, quantised_100);

    // Undo back to start
    shell.run("edit.undo", json!({})).unwrap();

    // 50% quantise: moves halfway toward grid lines
    shell
        .run(
            "clip.quantize_audio",
            json!({ "clip": clip, "amount": 50, "grid": 480 }),
        )
        .unwrap();
    assert_eq!(steps(&shell), initial_steps + 1);
    let quantised_50 = audio(&shell, clip).warp_markers;
    assert_ne!(initial_markers, quantised_50);
    assert_ne!(quantised_100, quantised_50);

    // An unwarped clip is refused
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": false }))
        .unwrap();
    let err = shell.run("clip.quantize_audio", json!({ "clip": clip }));
    assert!(err.is_err(), "unwarped clip is refused");
}

// Spec: warping R49
#[test]
fn quantising_at_zero_changes_nothing() {
    let scratch = Scratch::new("quantise-zero");
    let (mut shell, clip) = drum_clip(&scratch);
    let steps = |shell: &Shell| shell.editor().undo_history().len();
    let initial_markers = audio(&shell, clip).warp_markers;
    let initial_steps = steps(&shell);

    let res = shell
        .run(
            "clip.quantize_audio",
            json!({ "clip": clip, "amount": 0, "grid": 480 }),
        )
        .unwrap();
    assert_eq!(res, serde_json::Value::Null);
    assert_eq!(audio(&shell, clip).warp_markers, initial_markers);
    assert_eq!(steps(&shell), initial_steps, "0% is no undo step");
}
