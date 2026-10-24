//! Tempo detection and auto-warp in the app (WP-4.9): the warp settings a
//! clip made from a file gets, and Set 1.1.1 Here, Warp From Here and Warp
//! as Loop. The rules are in the warping spec, R29 to R41.

use std::path::{Path, PathBuf};

use bl_analysis::signals::{click_track, pink_noise};
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_model::{AudioData, ClipData, ClipId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

const RATE: u32 = 48_000;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-auto-warp-{}-{name}", std::process::id()));
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

/// Writes mono 16-bit `samples` at 48 kHz as a WAV file.
fn wav(path: &Path, samples: &[f32]) {
    let data = samples.len() as u32 * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&((sample * 32_000.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A click track file, and where each click starts.
fn clicks(
    scratch: &Scratch,
    name: &str,
    bpm: f64,
    offset: f64,
    seconds: f64,
) -> (PathBuf, Vec<f64>) {
    let path = scratch.0.join(format!("{name}.wav"));
    let (samples, starts) = click_track(bpm, offset, seconds, RATE);
    wav(&path, &samples);
    (path, starts)
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

/// Adds the file at `bar` bars in, and gives the clip's ID.
fn add(shell: &mut Shell, track: bl_model::TrackId, path: &Path, bar: i64) -> ClipId {
    let placed = shell
        .run(
            "browser.add_to_arrangement",
            json!({
                "paths": [path.display().to_string()],
                "track": track,
                "at": Ticks::WHOLE.get() * 8 * bar,
            }),
        )
        .unwrap();
    serde_json::from_value(placed["clips"][0]["clip"].clone()).unwrap()
}

fn audio(shell: &Shell, clip: ClipId) -> AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

fn length(shell: &Shell, clip: ClipId) -> i64 {
    shell.editor().document().clip(clip).unwrap().length.get()
}

fn set(shell: &mut Shell, command: &str, args: Value) {
    shell.run(command, args).unwrap();
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn near(a: f64, b: f64, within: f64) -> bool {
    (a - b).abs() <= within
}

// Spec: warping R31, R33, R34
#[test]
fn short_samples_load_as_the_setting_says() {
    let scratch = Scratch::new("short");
    let (mut shell, track) = new_shell(&scratch);
    // Four bars at 120 BPM.
    let (path, _) = clicks(&scratch, "loop", 120.0, 0.0, 8.0);
    let bars = Ticks::WHOLE.get() * 4;

    set(
        &mut shell,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "unwarped_one_shot" }),
    );
    let before = steps(&shell);
    let clip = add(&mut shell, track, &path, 0);
    assert_eq!(steps(&shell), before + 1);
    let unwarped = audio(&shell, clip);
    assert!(!unwarped.warp && !unwarped.looping && unwarped.warp_markers.is_empty());
    assert_eq!(length(&shell, clip), bars, "8 s at 120 BPM");

    for (mode, looping, bar) in [("warped_one_shot", false, 1), ("warped_loop", true, 2)] {
        set(
            &mut shell,
            "settings.set_loop_warp_short_samples",
            json!({ "mode": mode }),
        );
        let clip = add(&mut shell, track, &path, bar);
        let warped = audio(&shell, clip);
        assert!(warped.warp, "{mode}");
        assert_eq!(warped.looping, looping, "{mode}");
        assert_eq!(warped.warp_markers.len(), 2, "{mode}");
        assert_eq!(warped.warp_markers[0].beat, Ticks::ZERO);
        assert!(near(warped.warp_markers[1].time, 8.0, 1e-3), "{mode}");
        assert_eq!(warped.warp_markers[1].beat.get(), bars, "{mode}");
        assert!(near(warped.tempo.unwrap(), 120.0, 0.5), "{mode}");
        assert_eq!(length(&shell, clip), bars, "{mode}");
    }
}

// Spec: warping R32
#[test]
fn auto_tells_a_loop_from_a_one_shot() {
    let scratch = Scratch::new("auto");
    let (mut shell, track) = new_shell(&scratch);
    // Four bars at 100 BPM; a single click; noise with no beat.
    let (looped, _) = clicks(&scratch, "loop", 100.0, 0.0, 9.6);
    let (hit, _) = clicks(&scratch, "hit", 60.0, 0.0, 0.3);
    let pad = scratch.0.join("pad.wav");
    wav(&pad, &pink_noise(-20.0, 4.0, RATE, 7));

    let clip = add(&mut shell, track, &looped, 0);
    let warped = audio(&shell, clip);
    assert!(
        warped.warp && warped.looping,
        "a steady loop is a warped loop"
    );
    assert!(near(warped.tempo.unwrap(), 100.0, 0.5));
    for (name, path, bar) in [("hit", &hit, 1), ("pad", &pad, 2)] {
        let clip = add(&mut shell, track, path, bar);
        let one_shot = audio(&shell, clip);
        assert!(
            !one_shot.warp && !one_shot.looping,
            "{name} is an unwarped one-shot"
        );
    }
}

// Spec: warping R34
#[test]
fn an_even_loop_gets_markers_at_its_ends_and_its_tempo() {
    let scratch = Scratch::new("even");
    let (mut shell, track) = new_shell(&scratch);
    set(
        &mut shell,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "warped_loop" }),
    );
    // Two bars at 90 BPM in a set at 120: the analysis' tempo decides.
    let (slow, _) = clicks(&scratch, "slow", 90.0, 0.0, 16.0 / 3.0);
    let clip = add(&mut shell, track, &slow, 0);
    let warped = audio(&shell, clip);
    assert_eq!(warped.warp_markers[1].beat.get(), Ticks::WHOLE.get() * 2);
    assert!(near(warped.tempo.unwrap(), 90.0, 0.5));
    assert_eq!(length(&shell, clip), Ticks::WHOLE.get() * 2);
    // With no beat, the set's tempo does: 4 s is two bars at 120.
    let pad = scratch.0.join("pad.wav");
    wav(&pad, &pink_noise(-20.0, 4.0, RATE, 3));
    let clip = add(&mut shell, track, &pad, 1);
    let warped = audio(&shell, clip);
    assert_eq!(warped.warp_markers[1].beat.get(), Ticks::WHOLE.get() * 2);
    assert!(near(warped.tempo.unwrap(), 120.0, 1e-6));
}

// Spec: warping R35
#[test]
fn a_long_sample_gets_a_marker_on_every_bars_first_beat() {
    let scratch = Scratch::new("long");
    let (mut shell, track) = new_shell(&scratch);
    let (path, starts) = clicks(&scratch, "track", 128.0, 1.3, 75.0);
    let clip = add(&mut shell, track, &path, 0);
    let warped = audio(&shell, clip);
    assert!(warped.warp && !warped.looping);
    assert_eq!((warped.start, warped.end), (0.0, None));
    let bars = starts.len().div_ceil(4);
    assert_eq!(warped.warp_markers.len(), bars);
    for (bar, marker) in warped.warp_markers.iter().enumerate() {
        assert!(
            near(marker.time, starts[4 * bar], 0.001),
            "bar {bar}: {} vs {}",
            marker.time,
            starts[4 * bar]
        );
        assert_eq!(marker.beat.get(), Ticks::WHOLE.get() * bar as i64);
    }
    assert!(near(warped.tempo.unwrap(), 128.0, 0.5));
    // The clip lasts from 1.1.1 to the file's end.
    let map = warped.warp_map().unwrap();
    assert_eq!(length(&shell, clip), map.time_to_beat(75.0).round() as i64);

    // Off, a long sample stays unwarped.
    set(
        &mut shell,
        "settings.set_auto_warp_long_samples",
        json!({ "on": false }),
    );
    let clip = add(&mut shell, track, &path, 1);
    assert!(!audio(&shell, clip).warp);
}

// Spec: warping R36
#[test]
fn a_clip_waits_for_its_analysis_and_is_one_step() {
    let scratch = Scratch::new("waits");
    let (mut shell, track) = new_shell(&scratch);
    // Never analysed before this call: the file is new to the set.
    let (path, _) = clicks(&scratch, "fresh", 120.0, 0.0, 8.0);
    let before = steps(&shell);
    let clip = add(&mut shell, track, &path, 0);
    assert!(audio(&shell, clip).warp, "made once the analysis was ready");
    assert_eq!(steps(&shell), before + 1);
    set(&mut shell, "edit.undo", json!({}));
    assert!(shell.editor().document().clip(clip).is_none());
    assert_eq!(steps(&shell), before);
}

/// A shell with a 40 second click track at 128 BPM, 0.37 s in, as an
/// unwarped clip, and where its clicks start.
fn track_clip(scratch: &Scratch) -> (Shell, ClipId, Vec<f64>) {
    let (mut shell, track) = new_shell(scratch);
    set(
        &mut shell,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "unwarped_one_shot" }),
    );
    let (path, starts) = clicks(scratch, "track", 128.0, 0.37, 40.0);
    let clip = add(&mut shell, track, &path, 0);
    (shell, clip, starts)
}

fn analysed(shell: &mut Shell, clip: ClipId) {
    let media = audio(shell, clip).media;
    shell.wait_analysis(media).unwrap();
}

// Spec: warping R38
#[test]
fn set_1_1_1_here_moves_the_downbeat_and_keeps_the_tempos() {
    let scratch = Scratch::new("downbeat");
    let (mut shell, clip, starts) = track_clip(&scratch);
    set(
        &mut shell,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    analysed(&mut shell, clip);
    set(
        &mut shell,
        "clip.warp_from_here",
        json!({ "clip": clip, "time": starts[0], "method": "auto" }),
    );
    let before = audio(&shell, clip);
    let tempos = |audio: &AudioData| -> Vec<i64> {
        let map = audio.warp_map().unwrap();
        (0..audio.warp_markers.len() - 1)
            .map(|i| (map.segment_bpm(i) * 1e6) as i64)
            .collect()
    };

    // The third bar's downbeat becomes 1.1.1.
    let steps_before = steps(&shell);
    set(
        &mut shell,
        "clip.set_1_1_1_here",
        json!({ "clip": clip, "time": starts[8] }),
    );
    assert_eq!(steps(&shell), steps_before + 1);
    let after = audio(&shell, clip);
    let here = after
        .warp_markers
        .iter()
        .find(|m| near(m.time, starts[8], 1e-9))
        .unwrap();
    assert_eq!(here.beat, Ticks::ZERO);
    assert_eq!(after.start, 0.0);
    assert_eq!(tempos(&after), tempos(&before), "no segment changes tempo");
    assert_eq!(after.tempo, before.tempo);
    set(&mut shell, "edit.undo", json!({}));
    assert_eq!(audio(&shell, clip), before);

    // An unwarped clip is refused.
    set(
        &mut shell,
        "clip.set_warp",
        json!({ "clip": clip, "warp": false }),
    );
    assert!(
        shell
            .run(
                "clip.set_1_1_1_here",
                json!({ "clip": clip, "time": starts[8] })
            )
            .is_err()
    );
}

// Spec: warping R39
#[test]
fn each_warp_from_here_keeps_the_markers_before_it() {
    let scratch = Scratch::new("from-here");
    let (mut shell, clip, starts) = track_clip(&scratch);
    set(
        &mut shell,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    analysed(&mut shell, clip);
    // Markers at the file's start and at the fourth bar's downbeat.
    set(
        &mut shell,
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "time": starts[12] }),
    );
    let kept = audio(&shell, clip).warp_markers.clone();
    assert_eq!(kept.len(), 2);

    for (method, args) in [
        ("auto", json!({})),
        ("start_at", json!({ "bpm": 128.0 })),
        ("straight", json!({})),
        ("bpm", json!({ "bpm": 90.0 })),
    ] {
        let steps_before = steps(&shell);
        let mut call = json!({ "clip": clip, "marker": 1, "method": method });
        call.as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        set(&mut shell, "clip.warp_from_here", call);
        assert_eq!(steps(&shell), steps_before + 1, "{method}");
        let warped = audio(&shell, clip);
        assert_eq!(
            warped.warp_markers[..2],
            kept[..],
            "{method} keeps the markers before it"
        );
        match method {
            "auto" | "start_at" => {
                // A marker on each bar after the point; each bar is
                // 60 / 128 * 4 seconds.
                assert!(warped.warp_markers.len() > 10, "{method}");
                for pair in warped.warp_markers[1..].windows(2) {
                    assert!(
                        near(pair[1].time - pair[0].time, 240.0 / 128.0, 0.002),
                        "{method}"
                    );
                }
                assert!(near(warped.tempo.unwrap(), 128.0, 0.5), "{method}");
            }
            "straight" => {
                assert_eq!(warped.warp_markers.len(), 2);
                assert!(near(warped.tempo.unwrap(), 128.0, 0.5));
            }
            _ => {
                assert_eq!(warped.warp_markers.len(), 2);
                assert_eq!(warped.tempo, Some(90.0));
            }
        }
        set(&mut shell, "edit.undo", json!({}));
        assert_eq!(audio(&shell, clip).warp_markers, kept);
    }
}

// Spec: warping R40, R41
#[test]
fn warp_as_loop_picks_the_bar_count_nearest_the_set_tempo() {
    let scratch = Scratch::new("as-loop");
    let (mut shell, track) = new_shell(&scratch);
    set(
        &mut shell,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "unwarped_one_shot" }),
    );
    // Eight seconds: four bars at 120, two at 60.
    let (path, _) = clicks(&scratch, "loop", 120.0, 0.0, 8.0);
    let clip = add(&mut shell, track, &path, 0);

    // R41: not while the clip is unwarped.
    assert!(
        shell
            .run("clip.warp_as_loop", json!({ "clip": clip }))
            .is_err()
    );
    set(
        &mut shell,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    analysed(&mut shell, clip);
    for (set_bpm, bars) in [(120.0, 4), (60.0, 2)] {
        let tempo = serde_json::to_value(TempoMap::constant(Bpm::new(set_bpm).unwrap())).unwrap();
        set(&mut shell, "song.set_tempo", json!({ "tempo": tempo }));
        let steps_before = steps(&shell);
        set(&mut shell, "clip.warp_as_loop", json!({ "clip": clip }));
        assert_eq!(steps(&shell), steps_before + 1);
        let looped = audio(&shell, clip);
        assert!(looped.looping && looped.warp);
        assert_eq!(looped.warp_markers.len(), 2);
        assert_eq!(looped.warp_markers[0].beat, Ticks::ZERO);
        assert!(near(looped.warp_markers[1].time, 8.0, 1e-3));
        assert_eq!(looped.warp_markers[1].beat.get(), Ticks::WHOLE.get() * bars);
        assert_eq!(looped.loop_end, Some((Ticks::WHOLE.get() * bars) as f64));
    }

    // A time selection of two seconds is one bar at 120.
    let tempo = serde_json::to_value(TempoMap::constant(Bpm::new(120.0).unwrap())).unwrap();
    set(&mut shell, "song.set_tempo", json!({ "tempo": tempo }));
    set(
        &mut shell,
        "clip.warp_selection_as_loop",
        json!({ "clip": clip, "from": 2.0, "to": 4.0 }),
    );
    let selection = audio(&shell, clip);
    assert_eq!(selection.warp_markers[0].time, 2.0);
    assert_eq!(selection.warp_markers[1].beat, Ticks::WHOLE);
    assert!(selection.looping);
}

/// A clip of an eight-second loop, warped as a loop of whole bars in Tones
/// at −6 dB: settings no preference would give a new clip.
fn tuned_clip(scratch: &Scratch) -> (Shell, bl_model::TrackId, PathBuf, ClipId) {
    let (mut shell, track) = new_shell(scratch);
    set(
        &mut shell,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "unwarped_one_shot" }),
    );
    let (path, _) = clicks(scratch, "loop", 120.0, 0.0, 8.0);
    let clip = add(&mut shell, track, &path, 0);
    analysed(&mut shell, clip);
    set(
        &mut shell,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    set(&mut shell, "clip.warp_as_loop", json!({ "clip": clip }));
    set(
        &mut shell,
        "clip.set_warp_mode",
        json!({ "clip": clip, "mode": "tones" }),
    );
    set(
        &mut shell,
        "clip.set_gain",
        json!({ "clip": clip, "gain": 0.5 }),
    );
    (shell, track, path, clip)
}

// Spec: warping R33, R42
#[test]
fn save_default_clip_writes_beside_the_sample_and_new_clips_take_it() {
    let scratch = Scratch::new("save-default");
    let (mut shell, track, path, clip) = tuned_clip(&scratch);
    let tuned = audio(&shell, clip);
    let file = bl_shell::clip_defaults::path_for(&path);

    // Nothing is written until Save is pressed, and nothing is saved yet.
    assert!(!file.exists());
    let before = shell.run("clip.defaults", json!({ "clip": clip })).unwrap();
    assert_eq!(before["defaults"], Value::Null);
    assert_eq!(before["file"], file.display().to_string());

    let steps_before = steps(&shell);
    let saved = shell
        .run("clip.save_default", json!({ "clip": clip }))
        .unwrap();
    assert!(file.exists(), "a file beside the sample");
    assert_eq!(file.parent(), path.parent());
    assert_eq!(saved["file"], file.display().to_string());
    assert_eq!(steps(&shell), steps_before, "no undo step");
    assert_eq!(audio(&shell, clip), tuned, "the set doesn't change");
    let read = shell.run("clip.defaults", json!({ "clip": clip })).unwrap();
    assert_eq!(read["defaults"], saved["defaults"]);
    assert_eq!(read["defaults"]["warp_mode"], "tones");
    assert_eq!(read["defaults"]["gain"], 0.5);

    // A new clip of the file starts with them, not auto-warped as Auto
    // would: the preference asks for an unwarped one-shot, the file says
    // otherwise.
    let next = add(&mut shell, track, &path, 2);
    let new = audio(&shell, next);
    assert!(new.warp && !new.looping);
    assert_eq!(new.warp_markers, tuned.warp_markers);
    assert_eq!(new.tempo, tuned.tempo);
    assert_eq!(new.warp_mode, tuned.warp_mode);
    assert_eq!(new.gain, 0.5);
    assert_eq!(length(&shell, next), Ticks::WHOLE.get() * 4);
    // The clips already in the set don't change.
    assert_eq!(audio(&shell, clip), tuned);

    // The saved settings come before Loop/Warp Short Samples and a changed
    // setting alters a clip made without them only.
    set(
        &mut shell,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "warped_loop" }),
    );
    let third = add(&mut shell, track, &path, 3);
    assert_eq!(audio(&shell, third).warp_mode, tuned.warp_mode);
}

// Spec: warping R42
#[test]
fn saved_defaults_are_for_the_content_they_were_saved_for() {
    let scratch = Scratch::new("save-default-content");
    let (mut shell, track, path, clip) = tuned_clip(&scratch);
    set(&mut shell, "clip.save_default", json!({ "clip": clip }));
    // The same name holding other content is new media: it isn't shown the
    // old file's defaults.
    let file = bl_shell::clip_defaults::path_for(&path);
    let (other, _) = clicks(&scratch, "other", 90.0, 0.0, 8.0);
    std::fs::rename(&other, &path).unwrap();
    assert!(file.exists());
    let changed = add(&mut shell, track, &path, 2);
    assert_eq!(audio(&shell, changed).gain, 1.0);
    assert_eq!(audio(&shell, changed).warp_mode, bl_model::WarpMode::Beats);
    let read = shell
        .run("clip.defaults", json!({ "clip": changed }))
        .unwrap();
    assert_eq!(read["defaults"], Value::Null);

    // A file that isn't whole is no defaults either.
    std::fs::write(&file, b"{").unwrap();
    let broken = add(&mut shell, track, &path, 3);
    assert_eq!(audio(&shell, broken).gain, 1.0);
}

// Spec: warp-modes R2
#[test]
fn defaults_saved_before_the_loop_mode_default_changed_keep_loop_forward() {
    let scratch = Scratch::new("save-default-v1");
    let (mut shell, _, path, clip) = tuned_clip(&scratch);
    set(&mut shell, "clip.save_default", json!({ "clip": clip }));
    // A version 1 file names no Transient Loop Mode for a clip that had Loop
    // Forward, the default then.
    let file = bl_shell::clip_defaults::path_for(&path);
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    saved["version"] = 1.into();
    saved["defaults"]["warp_settings"] = json!({});
    std::fs::write(&file, serde_json::to_vec(&saved).unwrap()).unwrap();
    let read = shell.run("clip.defaults", json!({ "clip": clip })).unwrap();
    assert_eq!(
        read["defaults"]["warp_settings"]["beats"]["transient_loop_mode"],
        "loop_forward"
    );
}

// Spec: warping R42
#[test]
fn save_default_clip_fails_loudly_when_it_cannot_write() {
    let scratch = Scratch::new("save-default-fails");
    let (mut shell, _, path, clip) = tuned_clip(&scratch);
    // A folder where the file goes: it cannot be replaced.
    let file = bl_shell::clip_defaults::path_for(&path);
    std::fs::create_dir(&file).unwrap();
    let error = shell
        .run("clip.save_default", json!({ "clip": clip }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("couldn't write"), "{error}");
    assert!(error.contains(&file.display().to_string()), "{error}");
    assert!(file.is_dir(), "nothing else was written in its place");
}

// Spec: warping R33, R37
#[test]
fn a_new_warped_clip_takes_default_warp_mode() {
    let scratch = Scratch::new("default-mode");
    let (mut shell, track) = new_shell(&scratch);
    let (path, _) = clicks(&scratch, "loop", 120.0, 0.0, 8.0);
    set(
        &mut shell,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "warped_loop" }),
    );
    set(
        &mut shell,
        "settings.set_default_warp_mode",
        json!({ "mode": "complex_pro" }),
    );
    let clip = add(&mut shell, track, &path, 0);
    let warped = audio(&shell, clip);
    assert!(warped.warp);
    assert_eq!(warped.warp_mode, bl_model::WarpMode::ComplexPro);
}
