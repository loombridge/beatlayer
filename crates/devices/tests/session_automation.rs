//! A device's parameter under a Session clip's automation envelope
//! (WP-3.12): the framework takes the envelope's value, in the parameter's
//! own units, on the exact frame while the clip plays, holds the last one
//! when the clip has stopped on a track that stays taken over, and plays the
//! control again once nothing sets it. Each block runs as a real-time section
//! under the allocation trap.

use crate::common;
use bl_core::time::{Frames, TempoMap, Ticks};
use bl_devices::testing::{self, UTILITY_GAIN};
use bl_engine::{
    Block, Collector, Compiler, EngineConfig, Node, NodeData, NodeKey, SessionPlayback,
    StoppedClip, TransportInfo, Update,
};
use bl_model::{
    AutomationTarget, Clip, ClipData, Command, DeviceId, Editor, EnvelopeKind, MidiData, SceneId,
    TrackId, TrackKind,
};
use common::{CHANNELS, RATE, node};

/// A 4/4 bar is 96,000 frames at 120 BPM and 48 kHz and a whole note of
/// ticks.
const BAR: usize = 96_000;

/// The song: a track with the test utility (gain 1.0), and a one-bar
/// looping Session clip in its first scene whose automation envelope ramps
/// the gain from 0.25 to 0.75.
fn song() -> (Editor, TrackId, SceneId, DeviceId) {
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let id: DeviceId = editor.new_id();
    let mut device = testing::utility_type().device(id);
    device.params.insert(UTILITY_GAIN, 1.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let scene = editor.document().scenes().next().unwrap().id;
    let clip = editor.new_id();
    let data = MidiData {
        looping: true,
        loop_end: Some(Ticks::WHOLE),
        ..MidiData::new([])
    };
    editor
        .apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, ClipData::Midi(data)),
        })
        .unwrap();
    for (at, value) in [(Ticks::ZERO, 0.25), (Ticks::WHOLE, 0.75)] {
        editor
            .apply(Command::SetClipEnvelopeBreakpoint {
                clip,
                target: AutomationTarget::Device {
                    device: id,
                    param: UTILITY_GAIN,
                },
                kind: EnvelopeKind::Automation,
                at,
                value,
            })
            .unwrap();
    }
    (editor, track, scene, id)
}

/// The node data the compiler sends the device's node.
fn data(editor: &Editor, device: DeviceId) -> (TempoMap, NodeData) {
    let collector = Collector::new();
    let mut compiler = Compiler::new(
        EngineConfig::new(RATE, 2048),
        testing::registry(),
        collector.handle(),
    );
    let update: Update = compiler.sync(editor.document());
    let data = update
        .data
        .into_iter()
        .find(|(key, _)| matches!(key, NodeKey::Device(id) if *id == device))
        .expect("the device's node is sent its envelopes")
        .1;
    (editor.document().settings().tempo.clone(), data)
}

/// Runs a block of ones at song `position`, with the Session clips
/// `session` playing, and returns the first channel.
fn run(
    node: &mut dyn Node,
    tempo: &TempoMap,
    position: usize,
    frames: usize,
    session: &[SessionPlayback],
) -> Vec<f32> {
    let mut samples = vec![1.0f32; CHANNELS * frames];
    {
        let mut block = Block::new(
            &mut samples,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo {
                playing: true,
                position: Frames(position as i64),
            },
            RATE,
        )
        .with_tempo(tempo)
        .with_session(session);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    samples[..frames].to_vec()
}

/// What the clip's envelope holds `frame` frames after its launch.
fn gain(frame: usize) -> f64 {
    0.25 + 0.5 * (frame % BAR) as f64 / BAR as f64
}

// Spec: session-automation R6, R7, R8
#[test]
fn a_device_parameter_follows_the_playing_clip_and_holds_when_it_stops() {
    let (editor, track, scene, device) = song();
    let (tempo, data) = data(&editor, device);
    let (_, data_again) = self::data(&editor, device);
    let mut utility = node(
        &testing::registry(),
        &testing::utility_type().device(device),
        2048,
    );
    utility.receive_data(data);
    let launched = Frames(BAR as i64);
    let playing = [SessionPlayback {
        track,
        scene: Some(scene),
        frame: launched,
        velocity: 127,
        last: None,
    }];
    // The clip launched at the second bar line: from it, in blocks of any
    // size, the gain is the envelope's value on the exact frame, and the
    // loop repeats it.
    for block in [1, 64, 1_000, 2_048] {
        let mut at = BAR + 7;
        while at < BAR * 2 + 10_000 {
            let out = run(utility.as_mut(), &tempo, at, block, &playing);
            for (i, &sample) in out.iter().enumerate() {
                let want = gain(at + i - BAR);
                assert!(
                    (f64::from(sample) - want).abs() < 2e-5,
                    "blocks of {block}, frame {}: {sample} isn't {want}",
                    at + i
                );
            }
            at += 20_011;
        }
    }
    // With the track not taken over nothing sets the gain: the control's
    // plays, whatever the clip's envelope holds there.
    let mut idle = node(
        &testing::registry(),
        &testing::utility_type().device(device),
        2048,
    );
    idle.receive_data(data_again);
    let out = run(idle.as_mut(), &tempo, BAR + 500, 256, &[]);
    assert!(out.iter().all(|&s| s == 1.0), "the control's gain");
    // The clip stopped three bars on, on a track that stays taken over: the
    // gain holds the value of its last frame.
    let stopped = [SessionPlayback {
        track,
        scene: None,
        frame: Frames::ZERO,
        velocity: 127,
        last: Some(StoppedClip {
            scene,
            launched,
            stopped: Frames(BAR as i64 * 4),
        }),
    }];
    let out = run(utility.as_mut(), &tempo, BAR * 5, 512, &stopped);
    let held = gain(BAR * 4 - 1 - BAR);
    assert!(
        out.iter().all(|&s| (f64::from(s) - held).abs() < 2e-5),
        "held at {held}: {:?}",
        &out[..4]
    );
}
