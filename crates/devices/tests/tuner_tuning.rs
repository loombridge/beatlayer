//! The Tuner under a tuning (WP-6.10; the tunings spec, R33): it names the
//! nearest note of the set's tuning, as the note editor names it, with its
//! distance in cents; the Reference is set aside; on a MIDI track that
//! bypasses the tuning it reads twelve equal steps. Every block runs as a
//! real-time section under the allocation trap.

use crate::common;
use std::collections::BTreeMap;
use std::f64::consts::TAU;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_devices::NativeNode;
use bl_devices::polysynth::{
    FILTER, FREQ, HP, KEY, NOISE, OSC1_WAVE, OSC2, POLYSYNTH, RES, RETRIGGER, VOLUME,
};
use bl_devices::tuner::{REFERENCE, TUNER, Tuner, tuner_type};
use bl_engine::{
    Block, Collector, DisplayData, EngineConfig, KeyPitches, Node, NodeConfig, Offline, Registry,
    TransportCommand, TransportInfo,
};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceId, DeviceKind, Editor, MidiData, Note, ParamId,
    TrackId, TrackKind, Tuning,
};
use common::{RATE, device};

/// 19 equal steps to the octave, with the default reference and range.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

fn node(params: &[(ParamId, f64)], frames: usize) -> NativeNode<Tuner> {
    let desc = tuner_type();
    let dev = device(&desc, params);
    let config = NodeConfig {
        round_trip_latency: 0,
        sample_rate: RATE,
        max_frames: frames,
        channels: 2,
    };
    NativeNode::new(Tuner::new(&config), &desc.description, &dev, &config)
}

fn hear(node: &mut NativeNode<Tuner>, hz: f64) {
    let frames = 8192;
    let mut data: Vec<f32> = (0..frames)
        .map(|i| (TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
        .collect();
    let mono = data.clone();
    data.extend(mono);
    let mut block = Block::new(
        &mut data,
        2,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    );
    bl_rt::rt_test!({
        node.process(&mut block);
    });
}

fn follow(node: &mut NativeNode<Tuner>, tuning: Option<&Tuning>) {
    node.receive_data(Collector::new().handle().data(KeyPitches::of(tuning)));
}

// Spec: tunings R33. Sines at a key's tuned frequency, and a few cents either
// side of it, name the key and its distance within a cent.
#[test]
fn sines_under_a_19_step_tuning_name_the_nearest_note_and_its_cents() {
    let tuning = nineteen();
    for key in [30u8, 48, 60, 61, 70, 79] {
        for offset in [0.0f64, 20.0, -25.0] {
            let mut node = node(&[], 8192);
            follow(&mut node, Some(&tuning));
            let hz = tuning.key_frequency(key).unwrap() * 2f64.powf(offset / 1200.0);
            hear(&mut node, hz);
            assert_eq!(node.device().tuned_key(), Some(key), "{hz} Hz");
            let cents = node.device().cents().expect("the tone is heard");
            assert!(
                (cents - offset).abs() < 1.0,
                "key {key} at {offset:+} cents reads {cents:.2}"
            );
            assert_eq!(node.device().note_name(), None, "the display names it");
        }
    }
}

// Spec: tunings R33. The Reference is set aside while a tuning is loaded: the
// tuning's own reference pitch places the notes.
#[test]
fn the_reference_is_set_aside_under_a_tuning() {
    let tuning = nineteen();
    let key = 61;
    let hz = tuning.key_frequency(key).unwrap();
    let cents_at = |reference: f64| {
        let mut node = node(&[(REFERENCE, reference)], 8192);
        follow(&mut node, Some(&tuning));
        hear(&mut node, hz);
        assert_eq!(node.device().tuned_key(), Some(key));
        node.device().cents().unwrap()
    };
    assert!((cents_at(440.0) - cents_at(450.0)).abs() < 1e-9);
    // Without a tuning it reads twelve steps from the Reference, as before.
    let mut plain = node(&[(REFERENCE, 432.0)], 8192);
    follow(&mut plain, None);
    hear(&mut plain, 432.0);
    assert_eq!(plain.device().note_name().as_deref(), Some("A4"));
    assert!(plain.device().cents().unwrap().abs() < 1.0);
    assert_eq!(plain.device().tuned_key(), None);
}

// Spec: tunings R29, R33. The display names the note as the note editor does:
// the file's name for the degree, or the degree's number, with the octave.
#[test]
fn the_display_names_the_note_as_the_editor_does() {
    let mut named = nineteen();
    named.names = (0..19).map(|d| format!("n{d}")).collect();
    // Key 63 is degree 3 of octave 3 in either.
    for (tuning, want) in [(nineteen(), "3:3"), (named, "n33")] {
        let mut node = node(&[], 8192);
        follow(&mut node, Some(&tuning));
        hear(&mut node, tuning.key_frequency(63).unwrap());
        let mut reader = node.display().expect("the Tuner has a display");
        let data = reader.read().values.clone();
        let shown = bl_devices::tuner::display_in(
            DeviceId(bl_core::id::Id::new(1).unwrap()),
            &tuner_type().description,
            &bl_model::OrdMap::new(),
            Some(&data),
            None,
            Some(&tuning),
        );
        assert_eq!(shown.readings[0].text, want);
        assert_eq!(
            shown.readings[0].text,
            tuning.note_name(tuning.key_note(63).unwrap())
        );
    }
}

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// A MIDI track with a sine Drift playing key 61 for 8 beats, then a Tuner.
fn track_with_tuner(editor: &mut Editor) -> (TrackId, DeviceId) {
    let id: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let types = bl_devices::device_types();
    let make = |editor: &mut Editor, name: &str| -> Device {
        types
            .iter()
            .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == name))
            .unwrap()
            .device(editor.new_id())
    };
    let mut drift = make(editor, POLYSYNTH);
    for (param, value) in [
        (OSC1_WAVE, 0.0),
        (OSC2, 0.0),
        (NOISE, 0.0),
        (FILTER, 0.0),
        (FREQ, 20_000.0),
        (HP, 20.0),
        (RES, 0.0),
        (KEY, 0.0),
        (RETRIGGER, 1.0),
        (VOLUME, 1.0),
    ] {
        drift.params.insert(param, value);
    }
    let tuner = make(editor, TUNER);
    let tuner_id = tuner.id;
    editor
        .apply(Command::InsertDevice {
            track: id,
            index: 0,
            device: drift,
        })
        .unwrap();
    editor
        .apply(Command::InsertDevice {
            track: id,
            index: 1,
            device: tuner,
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    let note = Note::new(61, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 8), 100);
    editor
        .apply(Command::CreateClip {
            track: id,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 16),
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();
    (id, tuner_id)
}

/// What the Tuner after Drift publishes while Drift plays key 61: the key it
/// names, its cents, and whether it says that is a key of a tuning.
fn heard(bypass: bool, tuned: bool) -> (f32, f32, f32) {
    let mut editor = Editor::new();
    let (track, tuner) = track_with_tuner(&mut editor);
    if tuned {
        editor
            .apply(Command::SetTuning {
                tuning: nineteen(),
                retune: false,
            })
            .unwrap();
    }
    if bypass {
        editor
            .apply(Command::SetTuningBypass {
                track,
                bypass: true,
            })
            .unwrap();
    }
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry());
    assert!(offline.sync(editor.document()).is_empty());
    offline.sender().transport(TransportCommand::Play);
    offline.render(48_000 * 3, |_| {});
    assert_eq!(offline.heap_operations(), 0, "nothing allocates in a block");
    let mut all: BTreeMap<DeviceId, DisplayData> = BTreeMap::new();
    offline.displays().read_all(&mut all);
    let values = &all[&tuner].values;
    (
        values[1],
        values[2],
        values[bl_devices::tuner::TUNER_DISPLAY_LEN - 1],
    )
}

// Spec: tunings R14, R33. Through the engine: the Tuner after an instrument
// on a track that plays a 19-step tuning names the key the instrument plays,
// and on a MIDI track that bypasses it, or in a set with none, it reads
// twelve equal steps from its Reference.
#[test]
fn through_the_engine_a_tuner_follows_the_tuning_a_bypassing_track_does_not() {
    let (key, cents, tuned) = heard(false, true);
    assert_eq!((key, tuned), (61.0, 1.0), "key 61 of the tuning");
    assert!(cents.abs() < 1.0, "{cents}");
    // Bypassing, Drift plays twelve steps: key 61 is C#3's 277.18 Hz, MIDI
    // note 61 to the Tuner.
    let (note, cents, tuned) = heard(true, true);
    assert_eq!((note, tuned), (61.0, 0.0), "twelve steps");
    assert!(cents.abs() < 1.0, "{cents}");
    let (note, _, tuned) = heard(false, false);
    assert_eq!((note, tuned), (61.0, 0.0), "no tuning");
}
