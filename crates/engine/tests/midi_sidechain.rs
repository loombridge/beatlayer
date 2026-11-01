//! WP-5.27's acceptance in the engine: a device's MIDI sidechain (the
//! routing spec, R32 to R35).
//!
//! The device under test is the MIDI sidechain probe, which writes the last
//! key its block's notes played, over 127, from the frame it played on, so
//! Main's output says exactly what the device heard and when. Every block
//! runs as a real-time section under the allocation trap, and the commands
//! run by ID with JSON arguments, as an agent runs them.

use crate::common;
use bl_commands::Registry as Commands;
use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::routing::Graph;
use bl_engine::testing::{HEAR, INSTRUMENT, SEMITONES, TRANSPOSE};
use bl_engine::{NoteKind, TransportCommand};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceId, DeviceKind, Editor, MidiData, Note, TrackId,
    TrackKind, registry,
};
use common::{Rig, device, track};
use serde_json::{Value, json};

const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);
/// Frames in a beat at 120 BPM and 48 kHz.
const BEAT_FRAMES: usize = 24_000;

fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// Puts the MIDI sidechain probe at the end of `track`'s chain.
fn hear(editor: &mut Editor, track: TrackId) -> DeviceId {
    hear_with(editor, track, false)
}

/// The same, with a tapping point on its MIDI sidechain if `tapped`.
fn hear_with(editor: &mut Editor, track: TrackId, tapped: bool) -> DeviceId {
    let id = editor.new_id();
    let device = Device::new(
        id,
        "Test MIDI Sidechain",
        DeviceKind::Native {
            device_type: HEAR.to_owned(),
        },
    );
    let device = if tapped {
        device.with_midi_sidechain_tap()
    } else {
        device.with_midi_sidechain()
    };
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

fn listen(editor: &mut Editor, device: DeviceId, track: Option<TrackId>) {
    let args = match track {
        Some(track) => json!({ "device": device, "track": track }),
        None => json!({ "device": device }),
    };
    run(editor, "device.set_midi_sidechain_source", args);
}

/// A clip on `track` playing key 60 on the first beat, key 72 on the second,
/// and key 64 a third of a beat into the third.
fn play_keys(editor: &mut Editor, track: TrackId) {
    let id = editor.new_id();
    let notes = [
        Note::new(60, Ticks::ZERO, BEAT, 100),
        Note::new(72, BEAT, BEAT, 100),
        Note::new(64, BEAT * 2 + Ticks(TICKS_PER_QUARTER / 3), BEAT, 100),
    ];
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                BEAT * 4,
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
}

/// What the probe should write at `frame`: the key playing, over 127.
fn expected(frame: usize) -> f32 {
    expected_up(frame, 0)
}

/// The same, with every key `up` semitones higher.
fn expected_up(frame: usize, up: u8) -> f32 {
    let third = BEAT_FRAMES / 3;
    let key: u8 = match frame {
        f if f < BEAT_FRAMES => 60,
        f if f < 2 * BEAT_FRAMES => 72,
        f if f < 2 * BEAT_FRAMES + third => 72,
        _ => 64,
    };
    f32::from(key + up) / 127.0
}

/// Plays three beats from the top in blocks of `block` and returns Main's
/// left channel.
fn play(rig: &mut Rig, block: usize) -> Vec<f32> {
    rig.sender.transport(TransportCommand::Play);
    rig.render(3 * BEAT_FRAMES, block)
}

/// A song with the probe on an audio track that listens to a MIDI track
/// playing the keys. `audio_first` puts the audio track first in display
/// order, so the engine has to run the MIDI track before it.
fn song(rig: &mut Rig, audio_first: bool) -> (TrackId, TrackId, DeviceId) {
    song_with(rig, audio_first, false)
}

/// The same, the probe's MIDI sidechain having a tapping point if `tapped`.
fn song_with(rig: &mut Rig, audio_first: bool, tapped: bool) -> (TrackId, TrackId, DeviceId) {
    let ed = &mut rig.editor;
    let (audio, midi) = if audio_first {
        let midi = track(ed, TrackKind::Midi, None);
        let audio = track(ed, TrackKind::Audio, None);
        (audio, midi)
    } else {
        let audio = track(ed, TrackKind::Audio, None);
        let midi = track(ed, TrackKind::Midi, None);
        (audio, midi)
    };
    play_keys(ed, midi);
    let probe = hear_with(ed, audio, tapped);
    listen(ed, probe, Some(midi));
    rig.sync();
    (audio, midi, probe)
}

// Spec: routing R32
#[test]
fn a_device_hears_a_midi_tracks_notes_on_their_frames_at_any_block_size() {
    for audio_first in [true, false] {
        let mut first: Option<Vec<f32>> = None;
        for block in [1, 64, 512, 4096] {
            let mut rig = Rig::new(4096);
            song(&mut rig, audio_first);
            let out = play(&mut rig, block);
            for frame in [0, 1, BEAT_FRAMES - 1, BEAT_FRAMES, 2 * BEAT_FRAMES + 7_999] {
                assert_eq!(
                    out[frame],
                    expected(frame),
                    "frame {frame}, blocks of {block}"
                );
            }
            assert_eq!(
                out[2 * BEAT_FRAMES + BEAT_FRAMES / 3],
                expected(2 * BEAT_FRAMES + BEAT_FRAMES / 3),
                "the third note lands on its frame"
            );
            match &first {
                None => first = Some(out),
                Some(first) => assert_eq!(&out, first, "blocks of {block}"),
            }
        }
    }
}

// Spec: routing R32
#[test]
fn the_device_hears_the_notes_the_track_plays_before_its_own_devices() {
    let mut rig = Rig::new(512);
    let (_, midi, _) = song(&mut rig, true);
    device(&mut rig.editor, midi, INSTRUMENT, &[]);
    let at = rig.editor.document().track(midi).unwrap().devices().len();
    assert_eq!(at, 1);
    // A transposing MIDI effect first in the source's chain changes what
    // the instrument plays, not what the sidechain hears.
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::InsertDevice {
            track: midi,
            index: 0,
            device: Device::new(
                id,
                "Up",
                DeviceKind::Native {
                    device_type: TRANSPOSE.to_owned(),
                },
            )
            .with_role(bl_model::DeviceRole::MidiEffect),
        })
        .unwrap();
    rig.editor
        .apply(Command::SetDeviceParam {
            device: id,
            param: SEMITONES,
            value: 12.0,
        })
        .unwrap();
    rig.sync();
    let out = play(&mut rig, 512);
    assert_eq!(out[0], expected(0));
    assert_eq!(out[BEAT_FRAMES], expected(BEAT_FRAMES));
    let played = rig.probe.recorded_notes();
    assert!(
        played
            .iter()
            .any(|n| n.event.kind == NoteKind::On && n.event.key == 72),
        "the instrument heard the transposed notes"
    );
}

// Spec: routing R33
#[test]
fn a_track_can_listen_to_its_own_notes() {
    let mut rig = Rig::new(512);
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    play_keys(&mut rig.editor, midi);
    let probe = hear(&mut rig.editor, midi);
    listen(&mut rig.editor, probe, Some(midi));
    rig.sync();
    let out = play(&mut rig, 512);
    assert_eq!(out[0], expected(0));
    assert_eq!(out[BEAT_FRAMES], expected(BEAT_FRAMES));
    assert!(
        Graph::of(rig.editor.document()).broken.is_empty(),
        "its own track's notes are there before its chain runs"
    );
}

// Spec: routing R33
#[test]
fn deleting_the_source_leaves_no_input_and_undo_brings_it_back() {
    let mut rig = Rig::new(512);
    let (_, midi, probe) = song(&mut rig, true);
    rig.apply(Command::DeleteTrack { track: midi });
    let sidechain = rig
        .editor
        .document()
        .device(probe)
        .unwrap()
        .midi_sidechain
        .clone()
        .unwrap();
    assert_eq!(sidechain.source, None);
    let out = play(&mut rig, 512);
    assert!(out.iter().all(|&s| s == 0.0), "No Input: nothing is heard");

    rig.editor.undo().unwrap();
    rig.sync();
    let out = play(&mut rig, 512);
    assert_eq!(
        out[BEAT_FRAMES],
        expected(BEAT_FRAMES),
        "the source is back"
    );
}

// Spec: routing R32
#[test]
fn changing_the_source_after_the_song_is_compiled_takes_effect() {
    let mut rig = Rig::new(512);
    let (_, _, probe) = song(&mut rig, true);
    listen(&mut rig.editor, probe, None);
    rig.sync();
    let out = play(&mut rig, 512);
    assert!(out.iter().all(|&s| s == 0.0), "No Input: nothing is heard");
}

// Spec: routing R33
#[test]
fn a_source_that_would_close_a_loop_carries_nothing() {
    let mut rig = Rig::new(512);
    let ed = &mut rig.editor;
    let (x, m) = (
        track(ed, TrackKind::Midi, None),
        track(ed, TrackKind::Midi, None),
    );
    let probe = hear(ed, x);
    listen(ed, probe, Some(m));
    let graph = Graph::of(rig.editor.document());
    assert_eq!(graph.midi_links.len(), 1, "no loop yet");

    // The source takes its input from the device's track, so the device's
    // track feeds it: the link would close a loop.
    rig.editor
        .apply(Command::SetInput {
            track: m,
            input: bl_model::Input::Track {
                track: x,
                tap: bl_model::Tap::PostMixer,
                chain: None,
                output: None,
            },
            channel: None,
        })
        .unwrap();
    let graph = Graph::of(rig.editor.document());
    assert!(graph.midi_links.is_empty());
    assert!(graph.broken.contains(&(m, x)), "{:?}", graph.broken);
    // The engine still runs: the device hears nothing.
    rig.sync();
    let out = play(&mut rig, 512);
    assert!(out.iter().all(|&s| s == 0.0));
}

// Spec: routing R32
#[test]
fn workers_hear_the_same_notes_on_the_same_frames_as_the_audio_thread_alone() {
    // Several audio tracks listen to MIDI tracks, one of them to its own;
    // tone tracks beside them keep the workers busy. Each source finishes
    // before the tracks that listen to it start, whichever thread runs them.
    let song = |rig: &mut Rig| {
        let ed = &mut rig.editor;
        let keys = [
            track(ed, TrackKind::Midi, None),
            track(ed, TrackKind::Midi, None),
        ];
        for &k in &keys {
            play_keys(ed, k);
        }
        for n in 0..6 {
            let audio = track(ed, TrackKind::Audio, None);
            device(
                ed,
                audio,
                bl_engine::nodes::TEST_TONE,
                &[(
                    bl_engine::nodes::test_tone::FREQUENCY,
                    200.0 + 50.0 * f64::from(n),
                )],
            );
            if n % 2 == 0 {
                let probe = hear(ed, audio);
                listen(ed, probe, Some(keys[(n / 2) as usize % 2]));
            }
        }
        let own = hear(ed, keys[1]);
        listen(ed, own, Some(keys[1]));
        rig.sync();
    };
    let render = |rig: &mut Rig| -> Vec<u32> {
        rig.sender.transport(TransportCommand::Play);
        let mut out = Vec::new();
        for _ in 0..40 {
            rig.block(1_000);
            out.extend(
                rig.processor.output().channel(0)[..1_000]
                    .iter()
                    .map(|s| s.to_bits()),
            );
        }
        out
    };
    let mut alone = Rig::with_workers(4096, 0);
    song(&mut alone);
    let expected = render(&mut alone);
    assert!(expected.iter().any(|&b| b != 0), "the song is silent");
    for workers in [1, 3, 7] {
        let mut rig = Rig::with_workers(4096, workers);
        song(&mut rig);
        assert_eq!(render(&mut rig), expected, "{workers} workers");
        let pool = rig.pool.as_ref().unwrap();
        assert_eq!(pool.heap_operations(), 0, "the workers touched the heap");
        assert_eq!(pool.panics(), 0);
    }
}

/// Puts a MIDI effect that transposes by `up` semitones first in `track`'s
/// chain.
fn transpose(editor: &mut Editor, track: TrackId, up: f64) {
    let id = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                id,
                "Up",
                DeviceKind::Native {
                    device_type: TRANSPOSE.to_owned(),
                },
            )
            .with_role(bl_model::DeviceRole::MidiEffect),
        })
        .unwrap();
    editor
        .apply(Command::SetDeviceParam {
            device: id,
            param: SEMITONES,
            value: up,
        })
        .unwrap();
}

// Spec: routing R39
#[test]
fn a_tapping_point_hands_the_device_the_notes_before_or_after_the_midi_effects_at_any_block_size() {
    for (tap, instrument, up) in [
        ("pre_fx", false, 0),
        ("post_fx", false, 12),
        ("post_mixer", false, 12),
        ("post_fx", true, 12),
        ("pre_fx", true, 0),
    ] {
        let mut first: Option<Vec<f32>> = None;
        for block in [1, 64, 512, 4096] {
            let mut rig = Rig::new(4096);
            let (_, midi, probe) = song_with(&mut rig, true, true);
            transpose(&mut rig.editor, midi, 12.0);
            if instrument {
                device(&mut rig.editor, midi, INSTRUMENT, &[]);
            }
            run(
                &mut rig.editor,
                "device.set_midi_sidechain_source",
                json!({ "device": probe, "track": midi, "tap": tap }),
            );
            rig.sync();
            let out = play(&mut rig, block);
            for frame in [0, BEAT_FRAMES, 2 * BEAT_FRAMES + 8_000, 3 * BEAT_FRAMES - 1] {
                assert_eq!(
                    out[frame],
                    expected_up(frame, up),
                    "{tap}, frame {frame}, blocks of {block}"
                );
            }
            match &first {
                None => first = Some(out),
                Some(first) => assert_eq!(&out, first, "{tap}, blocks of {block}"),
            }
        }
    }
}

// Spec: routing R39
#[test]
fn a_new_tapped_device_takes_post_fx_and_the_tap_reads_back() {
    let mut rig = Rig::new(512);
    let (_, midi, probe) = song(&mut rig, true);
    let tap = |rig: &Rig| {
        rig.editor
            .document()
            .device(probe)
            .unwrap()
            .midi_sidechain
            .as_ref()
            .unwrap()
            .tap
    };
    assert_eq!(tap(&rig), None, "a device built without a tap has none");
    let tapped = bl_engine::DeviceType::native(HEAR, "Tapped", bl_model::DeviceRole::AudioEffect)
        .with_midi_sidechain_tap()
        .device(rig.editor.new_id());
    assert_eq!(
        tapped.midi_sidechain.unwrap().tap,
        Some(bl_model::Tap::PostFx)
    );
    transpose(&mut rig.editor, midi, 12.0);
    assert!(
        registry::run(
            &mut rig.editor,
            "device.set_midi_sidechain_source",
            json!({ "device": probe, "track": midi, "tap": "post_fx" }),
        )
        .is_err(),
        "a MIDI sidechain without a tapping point refuses one"
    );
}
