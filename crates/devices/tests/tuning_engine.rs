//! A set's tuning in the engine (WP-6.9; the tunings spec, R12, R14, R17 and
//! R18): a tuning loaded, changed or removed, and a track's Bypass Tuning,
//! reach the playing instruments as node data with no new program and
//! nothing allocated on the audio thread; Impulse and a Drum Rack's track
//! render as without a tuning. Every block runs as a real-time section
//! under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_devices::kitbag::{KEYS, KITBAG};
use bl_devices::polysynth::{
    FILTER, FREQ, HP, KEY, NOISE, OSC1_WAVE, OSC2, POLYSYNTH, RES, RETRIGGER, VOLUME,
};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_model::{
    Chain, Clip, ClipData, Command, ContentHash, Device, DeviceKind, DeviceRole, Editor, MidiData,
    Note, RackKind, TrackId, TrackKind, Tuning,
};
use common::RATE;

const HZ: f64 = 48_000.0;

/// A beat at 120 BPM and 48 kHz.
const BEAT: usize = 24_000;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

fn beats(quarters: usize) -> Ticks {
    Ticks(TICKS_PER_QUARTER * quarters as i64)
}

fn track(editor: &mut Editor) -> TrackId {
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
    id
}

/// A Drift of one sine oscillator, with nothing else sounding.
fn drift(editor: &mut Editor) -> Device {
    let device_type = bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == POLYSYNTH))
        .expect("Drift is registered");
    let mut device = device_type.device(editor.new_id());
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
        device.params.insert(param, value);
    }
    device
}

fn insert(editor: &mut Editor, track: TrackId, device: Device) {
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
}

/// A clip of `notes` as `(key, beat, beats)`.
fn clip(editor: &mut Editor, track: TrackId, notes: &[(u8, usize, usize)]) {
    let notes = notes
        .iter()
        .map(|&(key, at, length)| Note::new(key, beats(at), beats(length), 100));
    let clip: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                beats(16),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
}

/// The frequency of the sine in `signal[from..to]`, from the rising zero
/// crossings.
fn measure(signal: &[f32], from: usize, to: usize) -> f64 {
    let (mut first, mut last, mut crossings) = (None, 0.0, 0usize);
    for i in from..to - 1 {
        let (a, b) = (f64::from(signal[i]), f64::from(signal[i + 1]));
        if a <= 0.0 && b > 0.0 {
            let at = i as f64 + (-a) / (b - a);
            first.get_or_insert(at);
            last = at;
            crossings += 1;
        }
    }
    (crossings - 1) as f64 * HZ / (last - first.expect("the signal crosses zero"))
}

fn cents(measured: f64, wanted: f64) -> f64 {
    1200.0 * (measured / wanted).log2()
}

/// Renders `frames` more frames of the left channel.
fn render(offline: &mut Offline, frames: usize) -> Vec<f32> {
    let mut out = Vec::new();
    offline.render(frames as u64, |block| {
        out.extend_from_slice(block.channel(0));
    });
    out
}

fn start(editor: &Editor) -> Offline {
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry());
    let unhosted = offline.sync(editor.document());
    assert!(unhosted.is_empty(), "every device is hosted: {unhosted:?}");
    offline.sender().transport(TransportCommand::Play);
    offline
}

// Spec: tunings R18. A reference pitch changed while a note sounds, and a
// tuning removed, reach the notes that start after them, on their frames
// and with no new program; the note that sounds keeps its pitch.
#[test]
fn a_changed_tuning_reaches_the_next_notes_with_no_new_program() {
    let tuning = nineteen();
    let mut editor = Editor::new();
    let t = track(&mut editor);
    let device = drift(&mut editor);
    insert(&mut editor, t, device);
    // Key 60 sounds from 0 to 1.75 s, key 61 from 2 s to 3.5 s, and key 61
    // again from 4 s.
    clip(&mut editor, t, &[(60, 0, 3), (61, 4, 3), (61, 8, 3)]);
    editor
        .apply(Command::SetTuning {
            tuning: tuning.clone(),
            retune: false,
        })
        .unwrap();
    let mut offline = start(&editor);
    let mut out = render(&mut offline, BEAT * 2);
    let swaps = offline.state().swaps;
    // The reference pitch rises 10 Hz while key 60 sounds.
    editor
        .apply(Command::SetTuningReference {
            octave: None,
            degree: None,
            frequency: Some(271.626),
        })
        .unwrap();
    offline.sync(editor.document());
    out.extend(render(&mut offline, BEAT * 5));
    // The tuning is removed before the third note.
    editor
        .apply(Command::RemoveTuning { retune: false })
        .unwrap();
    offline.sync(editor.document());
    out.extend(render(&mut offline, BEAT * 6));
    assert_eq!(offline.heap_operations(), 0, "nothing allocates in a block");
    assert_eq!(offline.state().swaps, swaps, "no new program");

    let at = |beat: f64| (beat * BEAT as f64) as usize;
    let first = measure(&out, at(0.5), at(2.5));
    assert!(
        cents(first, tuning.key_frequency(60).unwrap()).abs() < 0.1,
        "the sounding note stays where it was: {first}"
    );
    let higher = tuning.with_reference(None, None, Some(271.626)).unwrap();
    let second = measure(&out, at(4.5), at(6.5));
    assert!(
        cents(second, higher.key_frequency(61).unwrap()).abs() < 0.1,
        "the next note plays the new reference: {second}"
    );
    let third = measure(&out, at(8.5), at(10.5));
    assert!(
        cents(third, 440.0 * 2f64.powf(-8.0 / 12.0)).abs() < 0.1,
        "the note after the removal plays twelve equal steps: {third}"
    );
}

// Spec: tunings R14, R18. Bypass Tuning reaches the next notes on its track
// and no other.
#[test]
fn bypass_tuning_makes_a_track_play_twelve_steps_and_only_that_track() {
    let tuning = nineteen();
    // Two tracks play key 61 from beat 2; track A's switch goes on at beat
    // 1. Each is heard alone, the other muted.
    let heard = |mute_a: bool| {
        let mut editor = Editor::new();
        let (a, b) = (track(&mut editor), track(&mut editor));
        for t in [a, b] {
            let device = drift(&mut editor);
            insert(&mut editor, t, device);
            clip(&mut editor, t, &[(61, 2, 3)]);
        }
        editor
            .apply(Command::SetTuning {
                tuning: tuning.clone(),
                retune: false,
            })
            .unwrap();
        editor
            .apply(Command::SetMute {
                track: if mute_a { a } else { b },
                mute: true,
            })
            .unwrap();
        let mut offline = start(&editor);
        let swaps = {
            let mut out = render(&mut offline, BEAT);
            let swaps = offline.state().swaps;
            editor
                .apply(Command::SetTuningBypass {
                    track: a,
                    bypass: true,
                })
                .unwrap();
            offline.sync(editor.document());
            out.extend(render(&mut offline, BEAT * 4));
            assert_eq!(offline.heap_operations(), 0);
            (out, swaps)
        };
        assert_eq!(offline.state().swaps, swaps.1, "no new program");
        swaps.0
    };
    let at = |beat: f64| (beat * BEAT as f64) as usize;
    let bypassing = heard(false);
    let plain = measure(&bypassing, at(2.5), at(4.5));
    assert!(
        cents(plain, 440.0 * 2f64.powf(-8.0 / 12.0)).abs() < 0.1,
        "the bypassing track plays twelve steps: {plain}"
    );
    let other = heard(true);
    let tuned = measure(&other, at(2.5), at(4.5));
    assert!(
        cents(tuned, tuning.key_frequency(61).unwrap()).abs() < 0.1,
        "the other track plays the tuning: {tuned}"
    );
}

/// A Drum Rack with a Drift in one pad's chain, on a track of its own.
fn drum_track(editor: &mut Editor) -> TrackId {
    let t = track(editor);
    let mut rack = Device::rack(editor.new_id(), RackKind::Drum);
    let chain = editor.new_id();
    rack = rack.with_chain(Chain::for_pad(chain, "Pad".to_owned(), 60));
    insert(editor, t, rack);
    let device = drift(editor);
    editor
        .apply(Command::InsertChainDevice {
            chain,
            index: 0,
            device: device.with_role(DeviceRole::Instrument),
        })
        .unwrap();
    clip(editor, t, &[(60, 0, 3)]);
    t
}

// Spec: tunings R14. A track that holds a Drum Rack plays twelve equal
// steps whatever the set's tuning or the track's switch says.
#[test]
fn a_drum_racks_track_renders_as_without_a_tuning() {
    let render_with = |tuning: Option<Tuning>| {
        let mut editor = Editor::new();
        let t = drum_track(&mut editor);
        if let Some(tuning) = tuning {
            editor
                .apply(Command::SetTuning {
                    tuning,
                    retune: false,
                })
                .unwrap();
        }
        assert!(!editor.document().track(t).unwrap().plays_tuning());
        let mut offline = start(&editor);
        let out = render(&mut offline, BEAT * 4);
        assert_eq!(offline.heap_operations(), 0);
        out
    };
    let plain = render_with(None);
    assert!(plain.iter().any(|s| s.abs() > 0.1), "the rack sounds");
    assert!(
        plain == render_with(Some(nineteen())),
        "a tuning changes nothing the rack plays"
    );
}

// Spec: eight-slot drum sampler R21. Impulse's keys choose its slots, so a
// tuning changes nothing it plays.
#[test]
fn impulse_renders_as_without_a_tuning() {
    let hash = ContentHash([7; 32]);
    let render_with = |tuning: Option<Tuning>| {
        let mut media = MediaMap::new();
        media.insert(
            hash,
            Arc::new(
                bl_media::SampleData::new(
                    RATE.hz(),
                    vec![(0..96_000).map(|i| (i as f32 * 0.05).sin()).collect()],
                )
                .unwrap(),
            ),
        );
        let mut editor = Editor::new();
        let t = track(&mut editor);
        let mut device = Device::new(
            editor.new_id(),
            "Impulse",
            DeviceKind::Native {
                device_type: KITBAG.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        device.set_media(0, Some(hash));
        insert(&mut editor, t, device);
        clip(&mut editor, t, &[(KEYS[0], 0, 2)]);
        if let Some(tuning) = tuning {
            editor
                .apply(Command::SetTuning {
                    tuning,
                    retune: false,
                })
                .unwrap();
        }
        let mut offline = Offline::new(
            EngineConfig::new(RATE, 256),
            registry().with_media(Arc::new(media)),
        );
        offline.sync(editor.document());
        offline.sender().transport(TransportCommand::Play);
        let out = render(&mut offline, BEAT * 3);
        assert_eq!(offline.heap_operations(), 0);
        out
    };
    let plain = render_with(None);
    assert!(plain.iter().any(|s| s.abs() > 0.01), "Impulse sounds");
    assert!(plain == render_with(Some(nineteen())));
}

// Spec: tunings R17. MIDI effects work on keys whatever the tuning: a move
// of one semitone is a move of one key, one step of the tuning.
#[test]
fn a_midi_effect_moves_keys_and_the_instrument_plays_the_tuned_pitch_of_the_new_key() {
    let tuning = nineteen();
    let mut editor = Editor::new();
    let t = track(&mut editor);
    let remap_type = bl_devices::device_types()
        .into_iter()
        .find(|t| {
            matches!(&t.kind, DeviceKind::Native { device_type } if device_type == bl_devices::remap::REMAP)
        })
        .expect("the Scale device is registered");
    let mut remap = remap_type.device(editor.new_id());
    remap.params.insert(bl_devices::remap::TRANSPOSE, 1.0);
    insert(&mut editor, t, remap);
    let device = drift(&mut editor);
    editor
        .apply(Command::InsertDevice {
            track: t,
            index: 1,
            device,
        })
        .unwrap();
    // Key 60 goes into the effect and key 61 comes out.
    clip(&mut editor, t, &[(60, 0, 3)]);
    editor
        .apply(Command::SetTuning {
            tuning: tuning.clone(),
            retune: false,
        })
        .unwrap();
    let mut offline = start(&editor);
    let out = render(&mut offline, BEAT * 4);
    assert_eq!(offline.heap_operations(), 0);
    let measured = measure(&out, BEAT / 2, BEAT * 2);
    assert!(
        cents(measured, tuning.key_frequency(61).unwrap()).abs() < 0.1,
        "key 60 through +1 plays key 61 of the tuning: {measured}"
    );
}
