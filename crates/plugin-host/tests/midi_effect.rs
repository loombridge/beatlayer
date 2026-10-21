//! A plug-in placed as a MIDI effect (WP-5.33; IMP-24): it sits before the
//! instrument, its notes reach the instrument on their frames at any block
//! size, its audio output is dropped, and a copy inside a MIDI effect
//! rack's chain changes that chain's notes. Every block runs as a real-time
//! section under the allocation trap.
//!
//! Spec: device-chains R3, R36; clap-hosting R1, R4.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_core::time::Ticks;
use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, NoteKind, Offline, TransportCommand};
use bl_model::{
    Chain, ChainId, Clip, ClipData, ClipId, Command, Device, DeviceId, DeviceKind, DeviceRole,
    Editor, MidiData, Note, PluginFormat, RackKind, TrackId, TrackKind,
};
use bl_plugin_host::clap::ClapHost;
use common::{RATE, bundle};

/// A beat at 48 kHz and 120 BPM, in frames.
const BEAT_FRAMES: i64 = 24_000;

/// A host with the test plug-in declaring `features` (IMP-24).
fn host(features: &str) -> ClapHost {
    // SAFETY: the plug-in reads this when it is asked for its features, and
    // this test runs in a process of its own.
    unsafe { std::env::set_var(bl_test_clap::FEATURES_VAR, features) };
    let host = ClapHost::new();
    host.load(bundle()).expect("the test plug-in loads");
    host
}

/// The test plug-in as a device with `role`.
fn plugin(id: DeviceId, role: DeviceRole) -> Device {
    Device::new(
        id,
        bl_test_clap::PLUGIN_NAME,
        DeviceKind::Plugin {
            format: PluginFormat::Clap,
            plugin_id: bl_test_clap::PLUGIN_ID.to_owned(),
        },
    )
    .with_role(role)
}

/// The test instrument as a device.
fn instrument(id: DeviceId) -> Device {
    Device::new(
        id,
        "Test Instrument",
        DeviceKind::Native {
            device_type: testing::INSTRUMENT.to_owned(),
        },
    )
}

/// A MIDI track with one note from frame 0 for a beat: `front` inserts what
/// comes before the instrument.
fn song(editor: &mut Editor, front: impl FnOnce(&mut Editor, TrackId)) {
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    front(editor, track);
    let after = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 1,
            device: instrument(after),
        })
        .unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new([Note::new(
                    60,
                    Ticks::ZERO,
                    Ticks::QUARTER,
                    100,
                )])),
            ),
        })
        .unwrap();
}

/// Plays a beat and a frame in blocks of `block`, and returns what the
/// instrument heard, as (song frame, kind, key), and the engine's heap
/// operations.
fn play(
    features: &str,
    block: usize,
    front: impl FnOnce(&mut Editor, TrackId),
) -> (Vec<(i64, NoteKind, u8)>, u64) {
    let host = host(features);
    let probe = Arc::new(Probe::default());
    let mut registry = testing::registry(&probe);
    registry.add(host.factory());
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    let mut editor = Editor::new();
    song(&mut editor, front);
    assert!(offline.sync(editor.document()).is_empty());
    offline.sender().transport(TransportCommand::Play);
    offline.render((BEAT_FRAMES + 1) as u64, |_| {});
    let heard = probe
        .recorded_notes()
        .iter()
        .map(|note| (note.frame, note.event.kind, note.event.key))
        .collect();
    (heard, offline.heap_operations())
}

/// The plug-in as a MIDI effect before the instrument.
fn note_effect(editor: &mut Editor, track: TrackId) {
    let device = plugin(editor.new_id(), DeviceRole::MidiEffect);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
}

/// A note effect before the instrument: what the instrument heard.
fn heard(features: &str, block: usize) -> Vec<(i64, NoteKind, u8)> {
    play(features, block, note_effect).0
}

// Spec: device-chains R3, R36; clap-hosting R1, R4; IMP-24
#[test]
fn a_plugin_note_effect_before_the_instrument_changes_its_notes_on_their_frames() {
    for block in [1usize, 100, 512, 4096] {
        let (heard, heap) = play("note-effect", block, note_effect);
        assert_eq!(
            heard,
            [(0, NoteKind::On, 72), (BEAT_FRAMES, NoteKind::Off, 72)],
            "the plug-in's notes, an octave up on their frames, at block size {block}"
        );
        assert_eq!(
            heap, 0,
            "nothing allocates on the audio thread, at block size {block}"
        );
    }
}

// Spec: device-chains R3, R36; IMP-24
#[test]
fn a_note_and_audio_plug_in_placed_as_a_midi_effect_changes_the_notes_too() {
    assert_eq!(
        heard("note-and-audio", 512),
        [(0, NoteKind::On, 72), (BEAT_FRAMES, NoteKind::Off, 72)]
    );
}

// Spec: device-chains R6, R36; IMP-24
#[test]
fn switching_a_midi_effect_plugin_off_passes_its_notes_through() {
    let host = host("note-effect");
    let probe = Arc::new(Probe::default());
    let mut registry = testing::registry(&probe);
    registry.add(host.factory());
    let mut offline = Offline::new(EngineConfig::new(RATE, 512), registry);
    let mut editor = Editor::new();
    let id = {
        let device = plugin(editor.new_id(), DeviceRole::MidiEffect);
        let id = device.id;
        song(&mut editor, move |editor, track| {
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device,
                })
                .unwrap();
        });
        id
    };
    editor
        .apply(Command::SetDeviceEnabled {
            device: id,
            enabled: false,
        })
        .unwrap();
    assert!(offline.sync(editor.document()).is_empty());
    offline.sender().transport(TransportCommand::Play);
    offline.render((BEAT_FRAMES + 1) as u64, |_| {});
    let heard: Vec<(i64, NoteKind, u8)> = probe
        .recorded_notes()
        .iter()
        .map(|note| (note.frame, note.event.kind, note.event.key))
        .collect();
    assert_eq!(
        heard,
        [(0, NoteKind::On, 60), (BEAT_FRAMES, NoteKind::Off, 60)],
        "a switched-off device passes its notes on unchanged"
    );
}

// Spec: racks R1, R4; IMP-24
#[test]
fn a_plugin_inside_a_midi_effect_racks_chain_changes_that_chains_notes() {
    let (heard, _) = play("note-effect", 512, |editor, track| {
        let rack = editor.new_id();
        let chain: ChainId = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: Device::rack(rack, RackKind::MidiEffect)
                    .with_chain(Chain::new(chain, "Chain")),
            })
            .unwrap();
        let device = plugin(editor.new_id(), DeviceRole::MidiEffect);
        editor
            .apply(Command::InsertChainDevice {
                chain,
                index: 0,
                device,
            })
            .unwrap();
    });
    assert_eq!(
        heard,
        [(0, NoteKind::On, 72), (BEAT_FRAMES, NoteKind::Off, 72)],
        "the rack's chain runs the plug-in and its notes go on"
    );
}
