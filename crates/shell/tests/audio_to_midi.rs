//! Convert Harmony, Melody and Drums to New MIDI Track (IMP-06) through the
//! shell, headless. The behaviour is in the audio to MIDI spec, R1 to R12,
//! R19 and R26.
//!
//! The transcription needs packages a person downloads on first use and, for
//! Drums, a classifier; the tests give the shell the notes to find instead
//! (`use_transcription_fixture`) and check everything after them: the track,
//! its instrument, its clip, the timing through the source's warping, and
//! the edit.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use bl_analysis::transcribe::Note as Found;
use bl_core::time::{TempoMap, Ticks};
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_model::{ClipData, ClipId, Device, DeviceKind, Note, RackKind, Track, TrackId, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::export::{self, Length, Render};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-audio-to-midi-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Four seconds at 48 kHz of silence with a 5 ms burst of a 1 kHz sine at
/// each of `times`, in seconds.
fn clicks_file(path: &Path, times: &[f64]) {
    let rate = 48_000usize;
    let mut writer = AudioWriter::create(
        path,
        EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: rate as u32,
            channels: 1,
        },
    )
    .unwrap();
    let mut samples = vec![0.0f32; rate * 4];
    for time in times {
        let from = (time * rate as f64).round() as usize;
        for i in 0..240 {
            let phase = i as f32 / rate as f32 * 1_000.0 * std::f32::consts::TAU;
            samples[from + i] = 0.8 * phase.sin();
        }
    }
    writer.write(&samples).unwrap();
    writer.finish().unwrap();
}

/// Where the clicks of the file are, in seconds.
const CLICKS: [f64; 4] = [0.5, 1.0, 1.75, 2.5];

fn new_shell(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(Track::id)
        .expect("the default set has an audio track")
}

/// A shell with the clicks file on its first audio track at `at` ticks,
/// and the clip.
fn clicks_clip(scratch: &Scratch, at: i64) -> (Shell, TrackId, ClipId) {
    let mut shell = new_shell(scratch);
    let track = audio_track(&shell);
    let file = scratch.path("Clicks.wav");
    clicks_file(&file, &CLICKS);
    let added = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&file)], "track": track, "at": at }),
        )
        .unwrap();
    let clip = serde_json::from_value(added["clips"][0]["clip"].clone()).unwrap();
    (shell, track, clip)
}

fn audio(shell: &Shell, clip: ClipId) -> bl_model::AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

/// A note the stand-in transcription finds, `confidence` sure of its onset.
fn found(pitch: u8, start: f64, end: f64, confidence: f32) -> Found {
    Found {
        start,
        end,
        pitch,
        velocity: 100,
        confidence,
    }
}

/// What a kind finds in the clicks file: notes starting on the clicks.
fn stand_in(kind: &str) -> Vec<Found> {
    match kind {
        "harmony" => vec![
            found(60, 0.5, 0.9, 0.5),
            found(64, 0.5, 0.9, 0.5),
            found(67, 1.75, 2.4, 0.5),
        ],
        "melody" => vec![
            found(60, 0.5, 0.9, 0.5),
            found(62, 1.0, 1.5, 0.5),
            found(64, 1.75, 2.4, 0.5),
        ],
        _ => vec![
            found(36, 0.5, 0.6, 0.5),
            found(42, 1.0, 1.1, 0.5),
            found(38, 1.75, 1.85, 0.5),
            found(36, 2.5, 2.6, 0.5),
        ],
    }
}

/// Converts `clip` as `kind` and waits for the edit to land: the status.
fn convert(shell: &mut Shell, clip: ClipId, kind: &str) -> Value {
    shell
        .run(
            "audio_to_midi.convert",
            json!({ "clip": clip, "kind": kind }),
        )
        .unwrap();
    shell.wait_audio_to_midi();
    shell.run("audio_to_midi.status", json!({})).unwrap()
}

fn id<T: serde::de::DeserializeOwned>(status: &Value, field: &str) -> T {
    serde_json::from_value(status[field].clone()).unwrap_or_else(|_| panic!("{field} in {status}"))
}

/// The notes of a MIDI clip, in start order.
fn notes(shell: &Shell, clip: ClipId) -> Vec<Note> {
    let ClipData::Midi(midi) = &shell.editor().document().clip(clip).unwrap().data else {
        panic!("a MIDI clip");
    };
    let mut notes: Vec<Note> = midi.notes.iter().cloned().collect();
    notes.sort_by_key(|note| (note.start, note.key));
    notes
}

fn midi(shell: &Shell, clip: ClipId) -> bl_model::MidiData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => midi.clone(),
        ClipData::Audio(_) => panic!("a MIDI clip"),
    }
}

/// The undo steps' names, the newest first.
fn step_names(shell: &Shell) -> Vec<String> {
    shell.editor().undo_history().map(str::to_owned).collect()
}

fn instrument(shell: &Shell, track: TrackId) -> Device {
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .next()
        .unwrap()
        .clone()
}

/// The first tick of `seconds` into the file for an unwarped clip at `at`,
/// at the set's tempo, counted from the clip's start.
fn ticks_in(shell: &Shell, at: Ticks, seconds: f64) -> Ticks {
    let tempo: &TempoMap = &shell.editor().document().settings().tempo;
    tempo.seconds_to_ticks(tempo.ticks_to_seconds(at) + seconds) - at
}

// Spec: audio-to-midi R5, R6, R7, R9, R10. Each kind makes a MIDI track
// directly after the clip's, named for the clip and the kind and selected,
// holding the kind's instrument and a clip where the source clip is, as
// long, with the notes found at the beats the source plays them; one undo
// step named after the command takes it all back and the source is left as it
// was.
#[test]
fn each_kind_makes_a_track_after_the_clip_s_with_its_instrument_and_notes() {
    for (kind, command) in [
        ("harmony", "Convert Harmony to New MIDI Track"),
        ("melody", "Convert Melody to New MIDI Track"),
        ("drums", "Convert Drums to New MIDI Track"),
    ] {
        let scratch = Scratch::new(kind);
        let at = Ticks::WHOLE.get();
        let (mut shell, track, clip) = clicks_clip(&scratch, at);
        shell.use_transcription_fixture(stand_in(kind));
        let before = shell.editor().document().clone();
        let source = shell.editor().document().clip(clip).unwrap().clone();

        let status = convert(&mut shell, clip, kind);
        assert_eq!(status["conversion"]["state"], "done", "{kind}: {status}");
        let (new_track, new_clip): (TrackId, ClipId) =
            (id(&status, "created_track"), id(&status, "created_clip"));
        assert_eq!(status["kind"], kind);
        assert_eq!(status["clip"], json!(clip));

        let doc = shell.editor().document();
        // R5. Directly after the clip's track, named for the clip and the
        // kind, and the one track selected.
        let order: Vec<TrackId> = doc.top_level().collect();
        let place = order.iter().position(|t| *t == track).unwrap();
        assert_eq!(order[place + 1], new_track, "{kind}");
        let name = format!("Clicks {}{}", &kind[..1].to_uppercase(), &kind[1..]);
        assert_eq!(doc.track(new_track).unwrap().name(), name);
        assert_eq!(doc.track(new_track).unwrap().kind(), TrackKind::Midi);
        assert_eq!(shell.arrangement().selected_tracks(), [new_track]);

        // R6. The clip is where the source is, as long, with its name.
        let made = doc.clip(new_clip).unwrap();
        assert_eq!((made.start, made.length), (source.start, source.length));
        assert_eq!(made.name, source.name);
        assert_eq!(made.colour, source.colour);

        // R9. The kind's instrument.
        let device = instrument(&shell, new_track);
        assert_eq!(device.name, kind[..1].to_uppercase() + &kind[1..]);
        let rack = device.rack_kind().unwrap();
        match kind {
            "drums" => {
                assert_eq!(rack, RackKind::Drum);
                let pads: Vec<_> = device
                    .chains
                    .iter()
                    .map(|chain| (chain.name.as_str(), chain.receive))
                    .collect();
                assert_eq!(
                    pads,
                    [
                        ("Kick", bl_model::Receive::Note(36)),
                        ("Snare", bl_model::Receive::Note(38)),
                        ("Closed Hi-Hat", bl_model::Receive::Note(42)),
                    ]
                );
            }
            "melody" => {
                assert_eq!(rack, RackKind::Instrument);
                assert_eq!(device.macros.setting(1).name, "Synth to Piano");
                assert_eq!(device.macros.mappings.len(), 2);
                assert_eq!(device.chains.len(), 2);
            }
            _ => {
                assert_eq!(rack, RackKind::Instrument);
                assert_eq!(device.chains.len(), 1);
            }
        }
        assert!(matches!(device.kind, DeviceKind::Rack { .. }));

        // R7. Each note starts at the beat the source plays its time.
        let want: Vec<(Ticks, u8)> = {
            let mut want: Vec<_> = stand_in(kind)
                .iter()
                .map(|f| (ticks_in(&shell, source.start, f.start), f.pitch))
                .collect();
            want.sort();
            want
        };
        let got: Vec<(Ticks, u8)> = notes(&shell, new_clip)
            .iter()
            .map(|n| (n.start, n.key))
            .collect();
        assert_eq!(got, want, "{kind}");
        assert_eq!(status["notes"], want.len());

        // R10. One undo step, named after the command; the source as it was.
        assert_eq!(step_names(&shell)[0], command);
        assert_eq!(shell.editor().document().clip(clip).unwrap(), &source);
        shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*shell.editor().document(), before, "{kind}");
    }
}

// Spec: audio-to-midi R22. A drum hit is a sixteenth note long, cut where the
// next hit of its pad starts, with the velocity found.
#[test]
fn drum_notes_are_sixteenth_notes_cut_by_the_next_hit_of_their_pad() {
    let scratch = Scratch::new("drum-lengths");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    let mut hits = stand_in("drums");
    // A second kick a thirty-second after the first.
    hits.push(found(36, 0.5 + 0.0625, 0.7, 0.5));
    shell.use_transcription_fixture(hits);
    let status = convert(&mut shell, clip, "drums");
    let made = notes(&shell, id(&status, "created_clip"));
    let sixteenth = Ticks::QUARTER.get() / 4;
    let first = &made[0];
    assert_eq!(first.key, 36);
    assert_eq!(first.length.get(), sixteenth / 2);
    for note in made.iter().filter(|n| n.start.get() > 1_000_000) {
        assert!(note.length.get() <= sixteenth);
        assert_eq!(note.velocity, 100);
    }
    assert_eq!(
        made.iter().filter(|n| n.length.get() == sixteenth).count(),
        4
    );
}

// Spec: audio-to-midi R15. A confident onset within 20 ms of a sixteenth
// note's line moves onto it; a doubtful one stays.
#[test]
fn a_confident_onset_snaps_to_the_sixteenth_note_grid() {
    let scratch = Scratch::new("snap");
    let (mut shell, _track, clip) = clicks_clip(&scratch, Ticks::WHOLE.get());
    shell.use_transcription_fixture(vec![
        // 10 ms late and sure, 10 ms late and unsure, 40 ms late and sure.
        found(60, 0.51, 0.7, 0.9),
        found(62, 1.01, 1.2, 0.5),
        found(64, 1.54, 1.7, 0.9),
    ]);
    let status = convert(&mut shell, clip, "melody");
    let made = notes(&shell, id(&status, "created_clip"));
    let quarter = Ticks::QUARTER.get();
    assert_eq!(made[0].start.get(), quarter, "snapped to the line");
    let second = ticks_in(&shell, Ticks::WHOLE, 1.01);
    assert_eq!(made[1].start, second, "an unsure onset stays");
    let third = ticks_in(&shell, Ticks::WHOLE, 1.54);
    assert_eq!(made[2].start, third, "40 ms is too far");
}

// Spec: audio-to-midi R8. A transient the person placed is a hint that wins:
// an onset within 30 ms of it moves to it, and a held note that crosses it
// is split there.
#[test]
fn transient_markers_the_person_placed_move_onsets_and_split_held_notes() {
    let scratch = Scratch::new("hints");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    // The clicks' own transients are found by the analysis; these are the
    // person's.
    for time in [0.55, 1.2] {
        shell
            .run(
                "clip.insert_transient",
                json!({ "clip": clip, "time": time }),
            )
            .unwrap();
    }
    shell.use_transcription_fixture(vec![
        // 15 ms after the marker at 0.55, and held across the one at 1.2.
        found(60, 0.565, 1.6, 0.5),
        found(62, 2.0, 2.4, 0.5),
    ]);
    let status = convert(&mut shell, clip, "melody");
    let made = notes(&shell, id(&status, "created_clip"));
    let tempo = shell.editor().document().settings().tempo.clone();
    let at = |seconds: f64| tempo.seconds_to_ticks(seconds);
    assert_eq!(
        made.iter()
            .map(|n| (n.key, n.start, n.start + n.length))
            .collect::<Vec<_>>(),
        [
            (60, at(0.55), at(1.2)),
            (60, at(1.2), at(1.6)),
            (62, at(2.0), at(2.4)),
        ]
    );
    // Drums never splits, only moves.
    shell.run("edit.undo", json!({})).unwrap();
    shell.use_transcription_fixture(vec![found(36, 0.565, 1.6, 0.5)]);
    let status = convert(&mut shell, clip, "drums");
    let made = notes(&shell, id(&status, "created_clip"));
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].start, at(0.55));
}

// Spec: audio-to-midi R19. A conversion that finds no notes makes the track
// and an empty clip, as one undo step, and says so.
#[test]
fn a_run_that_finds_nothing_makes_an_empty_clip() {
    let scratch = Scratch::new("empty");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    shell.use_transcription_fixture(Vec::new());
    let before = shell.editor().document().clone();
    let status = convert(&mut shell, clip, "harmony");
    assert_eq!(status["conversion"]["state"], "done");
    assert_eq!(status["notes"], 0);
    assert!(notes(&shell, id(&status, "created_clip")).is_empty());
    assert!(
        shell
            .message()
            .is_some_and(|m| m.text.contains("found no notes"))
    );
    assert_eq!(step_names(&shell)[0], "Convert Harmony to New MIDI Track");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*shell.editor().document(), before);
}

// Spec: audio-to-midi R6, R5. A Session clip's conversion goes in the new
// track's slot in the clip's scene, with the clip's launch settings, name and
// colour.
#[test]
fn a_session_clip_s_notes_go_in_its_scene_s_slot() {
    let scratch = Scratch::new("session");
    let mut shell = new_shell(&scratch);
    let file = scratch.path("Clicks.wav");
    clicks_file(&file, &CLICKS);
    let track = audio_track(&shell);
    let scene = shell.editor().document().scenes().nth(1).unwrap().id;
    shell
        .run(
            "session.drop_files",
            json!({ "paths": [text(&file)], "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap().id;
    shell
        .run(
            "clip.set_launch_mode",
            json!({ "clip": clip, "mode": "gate" }),
        )
        .unwrap();
    shell
        .run("clip.set_colour", json!({ "clips": [clip], "colour": 33 }))
        .unwrap();
    let source = shell
        .editor()
        .document()
        .slot(track, scene)
        .unwrap()
        .clone();
    shell.use_transcription_fixture(stand_in("drums"));

    let status = convert(&mut shell, clip, "drums");
    let new_track: TrackId = id(&status, "created_track");
    let doc = shell.editor().document();
    let order: Vec<TrackId> = doc.top_level().collect();
    let place = order.iter().position(|t| *t == track).unwrap();
    assert_eq!(order[place + 1], new_track);
    let held = doc.track(new_track).unwrap();
    assert_eq!(held.clips().count(), 0, "nothing in the Arrangement");
    let slot = doc.slot(new_track, scene).unwrap();
    assert_eq!(slot.id, id::<ClipId>(&status, "created_clip"));
    assert_eq!(slot.launch, source.launch);
    assert_eq!(slot.name, source.name);
    assert_eq!(slot.colour, source.colour);
    assert_eq!(slot.length, source.length);
    assert_eq!(held.slots().count(), 1);
    assert_eq!(notes(&shell, slot.id).len(), 4);
}

// Spec: audio-to-midi R26. A MIDI clip, a missing clip, and a conversion while
// another runs are refused, and a refusal changes nothing.
#[test]
fn what_can_not_be_converted_is_refused_and_changes_nothing() {
    let scratch = Scratch::new("refused");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    shell.use_transcription_fixture(stand_in("melody"));
    let status = convert(&mut shell, clip, "melody");
    let made: ClipId = id(&status, "created_clip");
    let before = shell.editor().document().clone();
    let steps = step_names(&shell).len();

    let refused = |shell: &mut Shell, args: Value| {
        let error = shell.run("audio_to_midi.convert", args).unwrap_err();
        assert!(matches!(
            error,
            ShellError::Unavailable { .. } | ShellError::InvalidArgs { .. }
        ));
        error.to_string()
    };
    assert!(refused(&mut shell, json!({ "clip": made, "kind": "melody" })).contains("audio clip"));
    let nothing = json!({ "kind": "drums" });
    assert!(refused(&mut shell, nothing).contains("select one audio clip"));
    refused(&mut shell, json!({ "clip": 999_999, "kind": "drums" }));
    assert_eq!(*shell.editor().document(), before);
    assert_eq!(step_names(&shell).len(), steps);
    // The menus are disabled on a MIDI clip, and enabled on an audio one.
    assert!(!shell.can_invoke(
        "audio_to_midi.convert",
        &json!({ "clip": made, "kind": "drums" })
    ));
    assert!(shell.can_invoke(
        "audio_to_midi.convert",
        &json!({ "clip": clip, "kind": "drums" })
    ));
}

// Spec: audio-to-midi R1. With no clip named, the one audio clip selected
// is converted; with none, or two, the command is disabled.
#[test]
fn the_selected_audio_clip_is_the_one_converted() {
    let scratch = Scratch::new("selected");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    shell.use_transcription_fixture(stand_in("drums"));
    let args = json!({ "kind": "drums" });
    shell
        .run("arrangement.select_clips", json!({ "clips": [] }))
        .unwrap();
    assert!(!shell.can_invoke("audio_to_midi.convert", &args));
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    assert!(shell.can_invoke("audio_to_midi.convert", &args));
    shell.run("audio_to_midi.convert", args).unwrap();
    shell.wait_audio_to_midi();
    let status = shell.run("audio_to_midi.status", json!({})).unwrap();
    assert_eq!(status["clip"], json!(clip));
}

// Spec: audio-to-midi R4, R6, R7. A warped clip whose loop is on converts from
// the earlier of its Start marker and its loop's start to the loop's end; a
// note maps through the warp markers to the beat it plays, counted from the
// Start marker; a note before the Start marker is dropped; and the new clip
// loops over the same brace.
#[test]
fn a_looped_warped_clip_converts_its_loop_and_the_new_clip_loops_with_it() {
    let scratch = Scratch::new("loop");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    let quarter = Ticks::QUARTER.get() as f64;
    // The brace from the second beat to the fifth, the Start marker on the
    // third.
    shell
        .run(
            "clip.set_loop",
            json!({ "clip": clip, "start": quarter, "end": 4.0 * quarter, "ticks": true }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_loop_enabled",
            json!({ "clip": clip, "enabled": true }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_region",
            json!({ "clip": clip, "start": 2.0 * quarter, "ticks": true }),
        )
        .unwrap();
    let source = audio(&shell, clip);
    assert_eq!((source.start, source.loop_start), (2.0 * quarter, quarter));
    // The file plays a beat at half a second, as the set's tempo is 120.
    shell.use_transcription_fixture(vec![
        // At 0.7 s into the file: a beat and 0.4 beats, before the marker.
        found(60, 0.2, 0.4, 0.5),
        // At 1.5 s into the file: beat 3, a beat after the Start marker.
        found(62, 1.0, 1.2, 0.5),
    ]);
    let status = convert(&mut shell, clip, "melody");
    let new_clip: ClipId = id(&status, "created_clip");
    let made = notes(&shell, new_clip);
    assert_eq!(made.len(), 1, "the note before the Start marker is dropped");
    assert_eq!(
        (made[0].key, made[0].start.get()),
        (62, Ticks::QUARTER.get())
    );
    let data = midi(&shell, new_clip);
    assert!(data.looping);
    assert_eq!(data.start, Ticks::ZERO);
    // The brace begins before the Start marker, so it begins at the marker.
    assert_eq!(data.loop_start, Ticks::ZERO);
    assert_eq!(data.loop_end, Some(Ticks(2 * Ticks::QUARTER.get())));
}

// Spec: audio-to-midi R7. A warped clip's notes are timed through its warp
// markers: a marker moved so the first second plays in a different time shifts
// the notes after it with the audio.
#[test]
fn notes_follow_the_warp_markers_of_the_source() {
    let scratch = Scratch::new("warp");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "time": 1.0 }),
        )
        .unwrap();
    // The first second now plays in a beat and a half rather than two beats.
    shell
        .run(
            "clip.move_warp_markers",
            json!({ "clips": [clip], "markers": [1], "by": -Ticks::QUARTER.get() / 2 }),
        )
        .unwrap();
    let source = audio(&shell, clip);
    shell.use_transcription_fixture(vec![found(60, 0.5, 0.9, 0.5), found(62, 1.5, 1.9, 0.5)]);
    let status = convert(&mut shell, clip, "harmony");
    let made = notes(&shell, id(&status, "created_clip"));
    let want: Vec<i64> = [0.5, 1.5]
        .iter()
        .map(|t| (source.in_clip_form(*t) - source.start).round() as i64)
        .collect();
    assert_eq!(made.iter().map(|n| n.start.get()).collect::<Vec<_>>(), want);
    // The half second before the marker plays in three quarters of a beat,
    // and the tempo after the marker is the first segment's: 90 BPM.
    let quarter = Ticks::QUARTER.get();
    assert!((want[0] - quarter * 3 / 4).abs() <= 1, "{want:?}");
    assert!((want[1] - quarter * 9 / 4).abs() <= 1, "{want:?}");
}

/// Renders the open set from its start for `frames` frames at `block`, as an
/// export does: interleaved stereo.
fn render(shell: &Shell, frames: u64, block: usize) -> Vec<f32> {
    let media: Arc<dyn bl_engine::MediaLibrary> = shell.media_store();
    let spec = Render {
        start: Ticks::ZERO,
        length: Length::Frames(frames),
        sample_rate: 48_000,
        engine_rate: 48_000,
        block,
    };
    let cancel = AtomicBool::new(false);
    let mut out: Vec<f32> = Vec::new();
    {
        let mut sink = |samples: &[f32]| {
            out.extend_from_slice(samples);
            Ok(())
        };
        export::render(
            shell.editor().document(),
            media,
            shell.plugin_hosts(),
            &spec,
            86_400.0,
            &cancel,
            &mut |_| {},
            &mut sink,
            None,
        )
        .unwrap();
    }
    out
}

/// The frames at which a sound starts: the first frame of each stretch
/// above `threshold` that follows at least a tenth of a second below it.
fn onsets(interleaved: &[f32], threshold: f32) -> Vec<usize> {
    let mut found = Vec::new();
    let mut quiet = usize::MAX / 2;
    for (frame, pair) in interleaved.chunks_exact(2).enumerate() {
        if pair[0].abs().max(pair[1].abs()) > threshold {
            if quiet >= 4_800 {
                found.push(frame);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    found
}

// Spec: audio-to-midi R7. Played together, the new clip's notes start on the
// frames on which the source clip plays their onsets, at any block size: the
// frame is within a millisecond (a note is rounded to a tick), and the render
// is the same at every block size.
#[test]
fn the_new_track_sounds_on_the_frames_the_source_plays_the_onsets() {
    for warped in [false, true] {
        let scratch = Scratch::new(if warped { "frames-warped" } else { "frames" });
        let (mut shell, track, clip) = clicks_clip(&scratch, 0);
        if warped {
            shell
                .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
                .unwrap();
            shell
                .run(
                    "clip.set_warp_mode",
                    json!({ "clip": clip, "mode": "re_pitch" }),
                )
                .unwrap();
            shell
                .run(
                    "clip.insert_warp_marker",
                    json!({ "clips": [clip], "time": 1.0 }),
                )
                .unwrap();
            shell
                .run(
                    "clip.move_warp_markers",
                    json!({ "clips": [clip], "markers": [1], "by": Ticks::QUARTER.get() / 2 }),
                )
                .unwrap();
        }
        shell
            .run(
                "clip.set_fades",
                json!({ "clip": clip, "fade_in": 0.0, "fade_out": 0.0 }),
            )
            .unwrap();
        let frames = 5 * 48_000;
        let source = onsets(&render(&shell, frames, 512), 0.02);
        assert_eq!(source.len(), CLICKS.len(), "warped {warped}: {source:?}");

        shell.use_transcription_fixture(stand_in("drums"));
        convert(&mut shell, clip, "drums");
        shell
            .run("mixer.set_mute", json!({ "track": track, "mute": true }))
            .unwrap();
        let mut renders = Vec::new();
        for block in [1usize, 64, 512] {
            renders.push(render(&shell, frames, block));
        }
        assert!(
            renders.windows(2).all(|pair| pair[0] == pair[1]),
            "warped {warped}: the render depends on the block size"
        );
        let new = onsets(&renders[0], 0.02);
        assert_eq!(new.len(), source.len(), "warped {warped}: {new:?}");
        for (new, source) in new.iter().zip(&source) {
            assert!(
                new.abs_diff(*source) <= 48,
                "warped {warped}: a hit sounds at frame {new}, the source at {source}"
            );
        }
    }
}

// Spec: audio-to-midi R2. Without its packages Harmony opens the package
// page instead and changes nothing; once they're installed, the conversion
// runs; Drums needs none; Cancel closes the page.
#[test]
fn harmony_asks_for_its_packages_the_first_time_and_runs_once_they_are_installed() {
    let scratch = Scratch::new("packages");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    let manifest = |dir: &Path| {
        let package = |id: &str, role: &str, bytes: &[u8]| {
            let file = dir.join(format!("{id}.source"));
            std::fs::write(&file, bytes).unwrap();
            let sha: String = {
                use sha2::{Digest, Sha256};
                Sha256::digest(bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            };
            json!({
                "id": id, "name": id, "version": "1", "project": "https://example.com/",
                "purpose": "A test package", "platform": "any", "role": role,
                "url": format!("file://{}", file.display()), "size": bytes.len(),
                "sha256": sha, "unpack": "file", "folder": format!("{id}-1"),
                "files": [{ "name": format!("{id}.bin") }],
            })
        };
        json!({
            "features": [{ "id": "audio-to-midi", "name": "Audio to MIDI",
                "roles": ["onnx-runtime", "pitch-model"] }],
            "packages": [
                package("runtime", "onnx-runtime", b"the library"),
                package("model", "pitch-model", b"the network"),
            ],
        })
    };
    shell.use_manifest(
        bl_shell::packages::Manifest::parse(&manifest(&scratch.0).to_string()).unwrap(),
    );
    let before = shell.editor().document().clone();
    shell
        .run(
            "audio_to_midi.convert",
            json!({ "clip": clip, "kind": "harmony" }),
        )
        .unwrap();
    let status = shell.run("audio_to_midi.status", json!({})).unwrap();
    assert_eq!(status["dialog"]["step"], "packages");
    assert_eq!(status["dialog"]["kind"], "harmony");
    assert_eq!(status["installed"], false);
    assert_eq!(status["conversion"]["state"], "idle");
    assert_eq!(status["packages"].as_array().unwrap().len(), 2);
    assert_eq!(*shell.editor().document(), before);

    // Cancel closes the page and forgets the conversion.
    shell.run("audio_to_midi.cancel", json!({})).unwrap();
    assert!(shell.audio_to_midi_dialog().is_none());

    // Drums needs no package.
    shell.use_transcription_fixture(stand_in("drums"));
    let made = convert(&mut shell, clip, "drums");
    assert_eq!(made["conversion"]["state"], "done");
    shell.run("edit.undo", json!({})).unwrap();

    // Asked again, installed, it runs: the stand-in packages aren't a real
    // network, so the run fails loading the model, which shows it began.
    let mut plain = new_shell(&scratch);
    plain.use_manifest(
        bl_shell::packages::Manifest::parse(&manifest(&scratch.0).to_string()).unwrap(),
    );
    let (_, _, clip) = {
        let track = audio_track(&plain);
        let added = plain
            .run(
                "browser.add_to_arrangement",
                json!({ "paths": [text(&scratch.path("Clicks.wav"))], "track": track, "at": 0 }),
            )
            .unwrap();
        (
            (),
            (),
            serde_json::from_value::<ClipId>(added["clips"][0]["clip"].clone()).unwrap(),
        )
    };
    plain
        .run(
            "audio_to_midi.convert",
            json!({ "clip": clip, "kind": "melody" }),
        )
        .unwrap();
    plain
        .run("packages.install", json!({ "ids": ["runtime", "model"] }))
        .unwrap();
    plain.wait_audio_to_midi();
    let status = plain.run("audio_to_midi.status", json!({})).unwrap();
    assert_eq!(status["installed"], true);
    assert_eq!(status["conversion"]["state"], "failed", "{status}");
    assert!(status["dialog"].is_null());
}

// Spec: audio-to-midi R25. The progress of the conversion reaches the window
// the status holds: its step, kind and clip.
#[test]
fn the_window_is_open_while_a_conversion_runs() {
    let scratch = Scratch::new("window");
    let (mut shell, _track, clip) = clicks_clip(&scratch, 0);
    shell.use_transcription_fixture(stand_in("drums"));
    shell
        .run(
            "audio_to_midi.convert",
            json!({ "clip": clip, "kind": "drums" }),
        )
        .unwrap();
    let dialog = shell.audio_to_midi_dialog().expect("the window is open");
    assert_eq!(dialog.clip, clip);
    assert_eq!(dialog.kind.command(), "Convert Drums to New MIDI Track");
    shell.wait_audio_to_midi();
    assert!(shell.audio_to_midi_dialog().is_none());
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

// Spec: audio-to-midi R9. Harmony's and Melody's racks make sound, and
// Melody's first macro, Synth to Piano, moves its sound from the synth's
// chain to the electric piano's.
#[test]
fn the_pitched_instruments_sound_and_synth_to_piano_moves_between_its_chains() {
    let scratch = Scratch::new("racks");
    let (mut shell, track, clip) = clicks_clip(&scratch, 0);
    shell
        .run("mixer.set_mute", json!({ "track": track, "mute": true }))
        .unwrap();
    shell.use_transcription_fixture(stand_in("harmony"));
    convert(&mut shell, clip, "harmony");
    let harmony = render(&shell, 2 * 48_000, 512);
    assert!(peak(&harmony) > 0.02, "Harmony is silent");
    shell.run("edit.undo", json!({})).unwrap();

    shell.use_transcription_fixture(stand_in("melody"));
    let status = convert(&mut shell, clip, "melody");
    let rack = instrument(&shell, id(&status, "created_track"));
    let mut renders = Vec::new();
    for value in [0.0, 1.0] {
        shell
            .run(
                "device.set_param",
                json!({ "device": rack.id, "param": bl_model::macro_param(1), "value": value }),
            )
            .unwrap();
        renders.push(render(&shell, 2 * 48_000, 512));
    }
    assert!(peak(&renders[0]) > 0.02, "the synth is silent");
    assert!(peak(&renders[1]) > 0.02, "the electric piano is silent");
    assert_ne!(renders[0], renders[1], "the macro changes nothing");
}
