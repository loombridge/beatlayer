//! A device's parameters under a clip envelope (WP-3.15, WP-5.9): the
//! framework adds the envelope's modulation to the parameter's own position
//! on its travel, on the exact frame, while the clip plays, clamped to the
//! travel's ends. Each block runs as a real-time section under the
//! allocation trap.

use crate::common;
use bl_core::time::{Frames, TempoMap, Ticks};
use bl_devices::testing::{self, UTILITY_GAIN};
use bl_engine::{Collector, Compiler, EngineConfig, NodeData, NodeKey, TransportInfo, Update};
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, DeviceId, Editor, MidiData, TrackId,
    TrackKind, Vector,
};
use common::{RATE, node, run_at};

/// At 120 BPM and 48 kHz a quarter note is 24,000 frames and a 4/4 bar
/// 96,000; the same bar is a whole note of ticks.
const BAR: usize = 96_000;

/// A track with the test utility on it, its gain at `gain`, and the gain's
/// modulation target.
fn track_with_utility(editor: &mut Editor, gain: f64) -> (TrackId, DeviceId, AutomationTarget) {
    let type_ = testing::utility_type();
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
    let id = editor.new_id();
    let mut device = type_.device(id);
    device.params.insert(UTILITY_GAIN, gain);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let target = AutomationTarget::Device {
        device: id,
        param: UTILITY_GAIN,
    };
    (track, id, target)
}

/// A one-bar MIDI clip on `track` at the start of the song.
fn one_bar_clip(editor: &mut Editor, track: TrackId) -> ClipId {
    let id: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData {
                    notes: Vector::new(),
                    start: Ticks::ZERO,
                    end: None,
                    looping: false,
                    loop_start: Ticks::ZERO,
                    loop_end: None,
                    signature: None,
                    groups: Vec::new(),
                }),
            ),
        })
        .unwrap();
    id
}

/// The node data the compiler sends the device's node.
fn modulation_data(editor: &Editor, device: DeviceId) -> (TempoMap, NodeData) {
    let collector = Collector::new();
    let config = EngineConfig::new(RATE, 2048);
    let mut compiler = Compiler::new(config, testing::registry(), collector.handle());
    let update: Update = compiler.sync(editor.document());
    let data = update
        .data
        .into_iter()
        .find(|(key, _)| matches!(key, NodeKey::Device(id) if *id == device))
        .expect("the device's node is sent its envelopes")
        .1;
    (editor.document().settings().tempo.clone(), data)
}

// Spec: clip-envelopes R3, R4
#[test]
fn a_devices_parameter_takes_the_clip_envelopes_modulation_on_the_exact_frame() {
    let mut editor = Editor::new();
    let (track, device, target) = track_with_utility(&mut editor, 1.0);
    let clip = one_bar_clip(&mut editor, track);
    // The envelope ramps from 0 at 1.1.1 to 1 at the bar's end: it adds a
    // share of the gain's travel to the position of its 0 dB, so the sample
    // climbs from 1 to the gain's ceiling.
    editor
        .apply(Command::SetClipEnvelopeBreakpoint {
            kind: bl_model::EnvelopeKind::Modulation,
            clip,
            target,
            at: Ticks::ZERO,
            value: 0.0,
        })
        .unwrap();
    editor
        .apply(Command::SetClipEnvelopeBreakpoint {
            kind: bl_model::EnvelopeKind::Modulation,
            clip,
            target,
            at: Ticks::WHOLE,
            value: 1.0,
        })
        .unwrap();
    let (tempo, data) = modulation_data(&editor, device);
    let type_ = testing::utility_type();
    let mut utility = node(&testing::registry(), &type_.device(device), 2048);
    utility.receive_data(data);

    let input = vec![vec![1.0f32; 2048]; 2];
    let run = |utility: &mut dyn bl_engine::Node, position: i64| {
        run_at(
            utility,
            &input,
            &[],
            &tempo,
            TransportInfo {
                playing: true,
                position: Frames(position),
            },
        )
    };
    // A block from the middle of the ramp: the gain is the control's position
    // plus the envelope's share of the travel, one frame at a time.
    let gain = testing::utility_type()
        .description
        .param(UTILITY_GAIN)
        .unwrap()
        .clone();
    let out = run(utility.as_mut(), (BAR / 2) as i64);
    for (i, &sample) in out[0].iter().enumerate() {
        let frame = BAR / 2 + i;
        let want = gain.denormalize(gain.normalize(1.0) + frame as f64 / BAR as f64);
        assert!(
            (f64::from(sample) - want).abs() < 1e-6,
            "frame {frame}: {sample} isn't {want}"
        );
    }
    // On the clip's first frame the envelope's first value holds (nothing
    // added yet); after the clip ends nothing is modulated: the device's
    // own gain plays.
    let start = run(utility.as_mut(), 0);
    assert!(
        start[0][0] == 1.0,
        "the envelope's first value holds on the first frame: {:?}",
        &start[0][..8]
    );
    assert!(
        start[0][1] > 1.0,
        "and the ramp is under way on the next: {:?}",
        &start[0][..8]
    );
    assert!(
        run(utility.as_mut(), BAR as i64)[0]
            .iter()
            .all(|&s| s == 1.0),
        "after the clip the device's gain plays alone"
    );

    // While the song is stopped, the device holds the values it has.
    let stopped = run_at(
        utility.as_mut(),
        &input,
        &[],
        &tempo,
        TransportInfo::default(),
    );
    assert!(
        stopped[0].iter().all(|&s| s == 1.0),
        "no modulation while the song is stopped"
    );
}

// Spec: clip-envelopes R3
#[test]
fn the_sum_is_clamped_to_the_parameters_range() {
    let mut editor = Editor::new();
    let (track, device, target) = track_with_utility(&mut editor, 1.0);
    let clip = one_bar_clip(&mut editor, track);
    // A modulation of 10 on a gain that tops out at +12 dB: the sum is
    // clamped to the parameter's range, so the gain is its ceiling.
    editor
        .apply(Command::SetClipEnvelopeBreakpoint {
            kind: bl_model::EnvelopeKind::Modulation,
            clip,
            target,
            at: Ticks::ZERO,
            value: 10.0,
        })
        .unwrap();
    let (tempo, data) = modulation_data(&editor, device);
    let type_ = testing::utility_type();
    let mut utility = node(&testing::registry(), &type_.device(device), 2048);
    utility.receive_data(data);
    let input = vec![vec![1.0f32; 64]; 2];
    let out = run_at(
        utility.as_mut(),
        &input,
        &[],
        &tempo,
        TransportInfo {
            playing: true,
            position: Frames(0),
        },
    );
    let ceiling = bl_model::param::db_to_gain(12.0);
    assert!(
        out[0]
            .iter()
            .all(|&s| (f64::from(s) - ceiling).abs() < 1e-6),
        "the sum is clamped to the gain's top"
    );
}
