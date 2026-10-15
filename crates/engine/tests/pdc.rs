//! WP-2.11's acceptance in the engine: plug-in delay compensation.
//!
//! A test device delays by a known number of frames and reports that
//! latency. Paths that reach a summing point with different latencies are
//! held back to line up, so two tracks carrying the same impulse, one
//! through the device, null at Main; the same holds through a group and a
//! send; automation is held back with the audio it acts on; and a
//! sidechain's source lines up with the device's own input. Every block
//! runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use bl_commands::Registry as Commands;
use bl_core::time::Ticks;
use bl_engine::testing::{DELAY, DELAY_FRAMES, KEY};
use bl_engine::{Block, MediaMap, Node, NodeConfig, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Device, DeviceId, DeviceKind, Editor, TrackId,
    TrackKind, registry,
};
use common::{Rig, device, track};
use serde_json::{Value, json};

/// The latency the test device adds.
const LATENCY: usize = 500;

/// Runs a document command by ID, as an agent does.
fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

fn hash(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

/// A short impulse: `value` on the first frame, silence after.
fn impulse(value: f32) -> Arc<SampleData> {
    let mut left = vec![0.0f32; 256];
    let mut right = vec![0.0f32; 256];
    left[0] = value;
    right[0] = value;
    Arc::new(SampleData::new(48_000, vec![left, right]).unwrap())
}

/// Puts a whole-song clip of `audio` on `track`.
fn clip(editor: &mut Editor, track: TrackId, audio: ContentHash) {
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE * 4,
                ClipData::Audio(AudioData::new(audio)),
            ),
        })
        .unwrap();
}

/// The test delay device, at the end of `track`'s chain.
fn delay_device(editor: &mut Editor, track: TrackId) -> DeviceId {
    device(editor, track, DELAY, &[(DELAY_FRAMES, LATENCY as f64)])
}

/// The sidechain probe, with a sidechain input, at the end of `track`'s chain.
fn key_device(editor: &mut Editor, track: TrackId) -> DeviceId {
    let id = editor.new_id();
    let device = Device::new(
        id,
        "Test Sidechain",
        DeviceKind::Native {
            device_type: KEY.to_owned(),
        },
    )
    .with_sidechain();
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

/// A rig whose clips' audio is `media`, playing from 1.1.1.
fn playing(media: MediaMap) -> Rig {
    let mut rig = Rig::with_registry(64, move |registry| {
        registry.set_media(Arc::new(media));
    });
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    rig
}

/// Asserts every sample is silent within a hair of zero.
fn assert_null(out: &[f32], what: &str) {
    let peak = out.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    assert!(peak < 1e-6, "{what}: peak {peak}");
}

// WP-2.11: two tracks, one delayed, null at Main.
#[test]
fn two_tracks_one_through_a_latency_device_null_at_main() {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    media.insert(hash(2), impulse(-1.0));
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    let b = track(ed, TrackKind::Audio, None);
    clip(ed, a, hash(1));
    clip(ed, b, hash(2));
    delay_device(ed, b);
    rig.sync();
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "aligned tracks cancel");
}

// Switching the device off and on keeps its latency: a switched-off device
// adds none, but the compensation comes back with it.
#[test]
fn switching_the_latency_device_off_and_on_keeps_its_compensation() {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    media.insert(hash(2), impulse(-1.0));
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    let b = track(ed, TrackKind::Audio, None);
    clip(ed, a, hash(1));
    clip(ed, b, hash(2));
    let d = delay_device(ed, b);
    rig.sync();
    rig.render(LATENCY + 256, 64);
    rig.apply(Command::SetDeviceEnabled {
        device: d,
        enabled: false,
    });
    rig.render(LATENCY + 256, 64);
    rig.apply(Command::SetDeviceEnabled {
        device: d,
        enabled: true,
    });
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "the compensation comes back with the device");
}

// The same through a group.
#[test]
fn the_same_through_a_group_lines_up() {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    media.insert(hash(2), impulse(-1.0));
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    let g = track(ed, TrackKind::Group, None);
    let a = track(ed, TrackKind::Audio, Some(g));
    let b = track(ed, TrackKind::Audio, Some(g));
    clip(ed, a, hash(1));
    clip(ed, b, hash(2));
    delay_device(ed, b);
    rig.sync();
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "aligned tracks cancel through the group");
}

// The same through a send and a return.
#[test]
fn the_same_through_a_send_lines_up() {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    media.insert(hash(2), impulse(-1.0));
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    let b = track(ed, TrackKind::Audio, None);
    let r = track(ed, TrackKind::Return, None);
    clip(ed, a, hash(1));
    clip(ed, b, hash(2));
    delay_device(ed, b);
    // B reaches Main only through the send; its send carries the whole of it.
    run(
        ed,
        "routing.set_output",
        json!({ "track": b, "output": { "type": "sends_only" } }),
    );
    run(
        ed,
        "mixer.set_send",
        json!({ "track": b, "to": r, "level": 1.0 }),
    );
    rig.sync();
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "aligned tracks cancel through the send");
}

// Automation is held back with the audio it acts on.
#[test]
fn automation_lands_on_the_compensated_sample() {
    let mut rig = Rig::new(64);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Midi, None);
    common::dc(ed, a, 1.0);
    delay_device(ed, a);
    // The fader rests silent, so only the automation brings it up.
    rig.editor
        .apply(Command::SetVolume {
            track: a,
            volume: 0.0,
        })
        .unwrap();
    rig.sync();
    let at = 1_000u64;
    rig.sender.send_param(bl_engine::ParamEvent::at_frame(
        bl_engine::NodeKey::Mixer(a),
        bl_engine::mixer::track::VOLUME,
        1.0,
        at,
    ));
    let out = rig.render(at as usize + LATENCY + 2_000, 64);
    // The change is held back by the device's latency and the 20 ms glide.
    let onset = at as usize + LATENCY;
    assert!(out[..onset].iter().all(|&s| s == 0.0), "silent before");
    let settled = onset + 960;
    assert!(out[settled + 64..].iter().all(|&s| s == 1.0), "up after");
}

// A sidechain's source lines up with the device's own input.
#[test]
fn a_sidechain_keyed_from_another_track_lines_up() {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    let s = track(ed, TrackKind::Audio, None);
    let t = track(ed, TrackKind::Audio, None);
    clip(ed, s, hash(1));
    clip(ed, t, hash(1));
    delay_device(ed, s);
    let key = key_device(ed, t);
    // S is heard only by the detector; T is heard at Main keyed by S.
    run(
        ed,
        "routing.set_output",
        json!({ "track": s, "output": { "type": "sends_only" } }),
    );
    run(
        ed,
        "device.set_sidechain_input",
        json!({ "device": key, "input": { "type": "track", "track": s, "tap": "post_fx" } }),
    );
    rig.sync();
    run(
        &mut rig.editor,
        "device.set_sidechain_enabled",
        json!({ "device": key, "enabled": true }),
    );
    rig.sync();
    let out = rig.render(2 * LATENCY + 512, 64);
    // T's impulse, delayed by the compensation, is gated open by S's.
    assert!(out[..LATENCY].iter().all(|&s| s == 0.0), "silent before");
    assert!(
        (out[LATENCY] - 1.0).abs() < 1e-6,
        "the gate opens at {}",
        LATENCY
    );
    assert!(out[LATENCY + 1..].iter().all(|&s| s == 0.0), "silent after");
}

/// Plays the song from 1.1.1 again, so the impulse it carries sounds once
/// more.
fn replay(rig: &mut Rig) {
    rig.sender.transport(TransportCommand::Stop);
    rig.sender.transport(TransportCommand::Play);
    assert!(rig.sender.flush(), "the queue is full");
}

/// A test device whose latency changes when a test moves it, as a plug-in
/// that restarted for a new one has (WP-2.7): it delays every sample by
/// what it reports, so a test can tell whether the compensation followed.
struct Restarted {
    latency: Arc<AtomicU32>,
    samples: Vec<f32>,
    write: usize,
    channels: usize,
}

impl Restarted {
    /// The longest latency the ring holds.
    const MAX: usize = 4096;

    fn new(config: &NodeConfig, latency: Arc<AtomicU32>) -> Self {
        Self {
            latency,
            samples: vec![0.0; config.channels * Self::MAX],
            write: 0,
            channels: config.channels,
        }
    }
}

impl Node for Restarted {
    fn process(&mut self, block: &mut Block<'_>) {
        let frames = usize::try_from(self.latency.load(Relaxed))
            .unwrap_or(0)
            .min(Self::MAX);
        if frames == 0 {
            return;
        }
        let channels = block.channels().min(self.channels);
        for c in 0..channels {
            let base = c * Self::MAX;
            let mut w = self.write;
            for sample in block.channel_mut(c) {
                let read = (w + Self::MAX - frames) % Self::MAX;
                let held = self.samples[base + read];
                self.samples[base + w] = *sample;
                *sample = held;
                w += 1;
                if w == Self::MAX {
                    w = 0;
                }
            }
            if c + 1 == channels {
                self.write = w;
            }
        }
    }

    fn latency(&self) -> u32 {
        self.latency.load(Relaxed)
    }
}

// WP-2.7, delay-compensation R6. A device's node reports a new latency, as
// a plug-in that restarted for one does; the compiler recompiles with what
// it's told and the compensation follows.
#[test]
fn a_latency_change_recompiles_and_the_compensation_follows() {
    const RESTARTED: &str = "test.restarted";
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    media.insert(hash(2), impulse(-1.0));
    let latency = Arc::new(AtomicU32::new(LATENCY as u32));
    let shared = Arc::clone(&latency);
    let mut rig = Rig::with_registry(64, move |registry| {
        registry.set_media(Arc::new(media));
        registry.add_native(RESTARTED, move |_device, config| {
            Box::new(Restarted::new(config, Arc::clone(&shared)))
        });
    });
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    let b = track(ed, TrackKind::Audio, None);
    clip(ed, a, hash(1));
    clip(ed, b, hash(2));
    let d = device(ed, b, RESTARTED, &[]);
    rig.sync();
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "aligned at the first latency");

    // The device now reports 300: the compiler recompiles with what the
    // shell tells it for a restarted plug-in, and the compensation follows.
    latency.store(300, Relaxed);
    let update = rig.compiler.latency(d, 300);
    assert!(update.program.is_some(), "a new program");
    rig.sender.send_update(update);
    assert!(rig.sender.flush(), "the queue is full");
    replay(&mut rig);
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "aligned at the new latency");
}
