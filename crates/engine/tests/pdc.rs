//! WP-2.11's acceptance in the engine: plug-in delay compensation.
//!
//! A test device delays by a known number of frames and reports that
//! latency. Paths that reach a summing point with different latencies are
//! held back to line up, so two tracks carrying the same impulse, one
//! through the device, null at Main; the same holds through a group and a
//! send; automation is held back with the audio it acts on; and a
//! sidechain's source lines up with the device's own input. Every block
//! runs as a real-time section under the allocation trap.

use crate::common;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use bl_commands::Registry as Commands;
use bl_core::time::Ticks;
use bl_engine::testing::{DELAY, DELAY_FRAMES, INSTRUMENT, KEY, MULTI, THROUGH, VALUE};
use bl_engine::{Block, MediaMap, Node, NodeConfig, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, Chain, ChainId, Clip, ClipData, Command, ContentHash, Device, DeviceId, DeviceKind,
    DeviceRole, Editor, MidiData, Note, RackKind, TrackDelay, TrackId, TrackKind, registry,
};
use common::{Rig, device, drum_rack, effect_in, rack, rack_in, track};
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

// WP-2.11. A native device whose node's latency moves while it runs, as a
// look-ahead does, is published for the shell to report; once reported, the
// compensation follows and nothing is published.
#[test]
fn a_node_whose_latency_moves_is_published_until_it_is_reported() {
    const MOVING: &str = "test.moving";
    let latency = Arc::new(AtomicU32::new(0));
    let shared = Arc::clone(&latency);
    let mut rig = Rig::with_registry(64, move |registry| {
        registry.add_native(MOVING, move |_device, config| {
            Box::new(Restarted::new(config, Arc::clone(&shared)))
        });
    });
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let t = track(&mut rig.editor, TrackKind::Audio, None);
    let d = device(&mut rig.editor, t, MOVING, &[]);
    rig.sync();
    rig.block(64);
    assert!(
        rig.monitor.state().latency_changes.is_empty(),
        "as compiled"
    );

    latency.store(300, Relaxed);
    rig.block(64);
    assert_eq!(rig.monitor.state().latency_changes, [(d, 300)]);

    let update = rig.compiler.latency(d, 300);
    rig.sender.send_update(update);
    assert!(rig.sender.flush(), "the queue is full");
    rig.block(64);
    rig.block(64);
    assert!(rig.monitor.state().latency_changes.is_empty(), "reported");
}

/// A rig with an impulse of `value` for each of `values`, playing.
fn impulses(values: &[f32]) -> Rig {
    let mut media = MediaMap::new();
    for (n, value) in values.iter().enumerate() {
        media.insert(hash(n as u8 + 1), impulse(*value));
    }
    playing(media)
}

/// The frames where the output is not silent, with what it holds there.
fn heard(out: &[f32]) -> Vec<(usize, f32)> {
    out.iter()
        .enumerate()
        .filter(|(_, s)| s.abs() > 1e-6)
        .map(|(at, s)| (at, *s))
        .collect()
}

// Spec: racks R16, delay-compensation R2. A rack's sum is a summing point:
// each chain's output is held back to the slowest chain's latency.
#[test]
fn a_racks_chains_line_up_at_its_sum() {
    let mut rig = impulses(&[1.0]);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    clip(&mut rig.editor, a, hash(1));
    let (_, chains) = rack(&mut rig.editor, a, RackKind::AudioEffect, 3);
    // An empty chain (a dry path), a chain of 300 frames, one of 500.
    effect_in(&mut rig.editor, chains[1], DELAY, &[(DELAY_FRAMES, 300.0)]);
    effect_in(&mut rig.editor, chains[2], DELAY, &[(DELAY_FRAMES, 500.0)]);
    rig.sync();
    let out = rig.render(2 * LATENCY, 64);
    assert_eq!(
        heard(&out),
        vec![(LATENCY, 3.0)],
        "all three chains' impulses reach the sum on the same frame"
    );
    // The same at any block size.
    for size in [1, 512, 4096] {
        let mut again = impulses(&[1.0]);
        let a = track(&mut again.editor, TrackKind::Audio, None);
        clip(&mut again.editor, a, hash(1));
        let (_, chains) = rack(&mut again.editor, a, RackKind::AudioEffect, 3);
        effect_in(
            &mut again.editor,
            chains[1],
            DELAY,
            &[(DELAY_FRAMES, 300.0)],
        );
        effect_in(
            &mut again.editor,
            chains[2],
            DELAY,
            &[(DELAY_FRAMES, 500.0)],
        );
        again.sync();
        assert_eq!(again.render(2 * LATENCY, size), out, "block size {size}");
    }
}

// Spec: racks R16. The rack reports its slowest chain's latency, and its
// track counts the rack once: another track, held back by it, nulls.
#[test]
fn a_rack_reports_its_slowest_chain() {
    let mut rig = impulses(&[1.0, -3.0]);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    clip(&mut rig.editor, a, hash(1));
    clip(&mut rig.editor, b, hash(2));
    let (_, chains) = rack(&mut rig.editor, a, RackKind::AudioEffect, 3);
    effect_in(&mut rig.editor, chains[1], DELAY, &[(DELAY_FRAMES, 300.0)]);
    effect_in(
        &mut rig.editor,
        chains[2],
        DELAY,
        &[(DELAY_FRAMES, LATENCY as f64)],
    );
    rig.sync();
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "the rack's three impulses against the other track's");

    // Racks inside racks: the inner rack's latency is its slowest chain's,
    // and the outer chain's is the sum of what is in it.
    for other in [true, false] {
        let mut rig = impulses(&[1.0, -3.0]);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        clip(&mut rig.editor, a, hash(1));
        if other {
            let b = track(&mut rig.editor, TrackKind::Audio, None);
            clip(&mut rig.editor, b, hash(2));
        }
        let (_, outer) = rack(&mut rig.editor, a, RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, outer[0], DELAY, &[(DELAY_FRAMES, 200.0)]);
        let (_, inner) = rack_in(&mut rig.editor, outer[0], RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, inner[0], DELAY, &[(DELAY_FRAMES, 100.0)]);
        rig.sync();
        let out = rig.render(2 * LATENCY, 64);
        if other {
            assert_null(&out, "the other track is held back by the whole of it");
        } else {
            // The first outer chain is 300 frames and carries two impulses
            // (the inner rack sums its delayed and dry chains); the other is
            // dry and held back to it.
            assert_eq!(heard(&out), vec![(300, 3.0)]);
        }
    }
}

// Spec: racks R16, delay-compensation R1. A device switched off in a chain
// changes that chain's latency and the alignment follows; a chain's
// activator and Solo don't change its latency.
#[test]
fn a_switched_off_device_changes_the_alignment_but_a_chains_activator_does_not() {
    let mut rig = impulses(&[1.0]);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    clip(&mut rig.editor, a, hash(1));
    let (_, chains) = rack(&mut rig.editor, a, RackKind::AudioEffect, 2);
    let slow = effect_in(&mut rig.editor, chains[1], DELAY, &[(DELAY_FRAMES, 500.0)]);
    rig.sync();
    assert_eq!(heard(&rig.render(1024, 64)), vec![(500, 2.0)]);

    rig.apply(Command::SetChainEnabled {
        chain: chains[1],
        enabled: false,
    });
    // Let the chain's level glide down, then rewind and play again: the dry
    // chain is still held back to 500.
    rig.render(2048, 64);
    rig.sender.transport(TransportCommand::Stop);
    rig.render(64, 64);
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(1024, 64);
    assert_eq!(heard(&out), vec![(500, 1.0)], "its latency still counts");

    rig.apply(Command::SetChainEnabled {
        chain: chains[1],
        enabled: true,
    });
    rig.apply(Command::SetDeviceEnabled {
        device: slow,
        enabled: false,
    });
    rig.render(2048, 64);
    rig.sender.transport(TransportCommand::Stop);
    rig.render(64, 64);
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(1024, 64);
    assert_eq!(
        heard(&out),
        vec![(0, 2.0)],
        "a device that is off adds no latency, so the rack reports none"
    );
}

// Spec: racks R14, R16. Grouping and ungrouping keep a device running: an
// impulse inside a delay when its device is grouped still comes out when it
// should.
#[test]
fn grouping_and_ungrouping_keep_a_delays_tail() {
    let mut rig = impulses(&[1.0]);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    clip(&mut rig.editor, a, hash(1));
    let delay = delay_device(&mut rig.editor, a);
    rig.sync();
    let mut out = rig.render(100, 64);
    let rack_id: DeviceId = rig.editor.new_id();
    let chain: ChainId = rig.editor.new_id();
    rig.apply(Command::GroupDevices {
        devices: vec![delay],
        rack: Device::rack(rack_id, RackKind::AudioEffect).with_chain(Chain::new(chain, "Chain")),
    });
    out.extend(rig.render(200, 64));
    rig.apply(Command::UngroupDevice { device: rack_id });
    out.extend(rig.render(2 * LATENCY, 64));
    assert_eq!(heard(&out), vec![(LATENCY, 1.0)]);
}

// Spec: racks R40, delay-compensation R2. A chain tap is a route into the
// track that takes it, held back as every route is: with the latency of the
// chain's devices behind it at Post FX and Post Mixer, and none at Pre FX.
#[test]
fn a_chain_tap_lines_up() {
    // Tapping an inner rack's chain adds the outer chain's device before it.
    for (nested, tap, at) in [
        (false, "pre_fx", 0),
        (false, "post_fx", LATENCY),
        (false, "post_mixer", LATENCY),
        (true, "pre_fx", LATENCY),
        (true, "post_fx", 2 * LATENCY),
    ] {
        let mut rig = impulses(&[1.0, 3.0]);
        let ed = &mut rig.editor;
        let source = track(ed, TrackKind::Audio, None);
        let other = track(ed, TrackKind::Audio, None);
        let tapper = track(ed, TrackKind::Audio, None);
        clip(ed, source, hash(1));
        clip(ed, other, hash(2));
        let (_, chains) = rack(ed, source, RackKind::AudioEffect, 1);
        effect_in(ed, chains[0], DELAY, &[(DELAY_FRAMES, LATENCY as f64)]);
        let mut chain = chains[0];
        if nested {
            let (_, inner) = rack_in(ed, chain, RackKind::AudioEffect, 1);
            effect_in(ed, inner[0], DELAY, &[(DELAY_FRAMES, LATENCY as f64)]);
            chain = inner[0];
        }
        run(
            ed,
            "routing.set_output",
            json!({ "track": source, "output": { "type": "sends_only" } }),
        );
        run(
            ed,
            "routing.set_input",
            json!({ "track": tapper, "input":
                    { "type": "track", "track": source, "tap": tap, "chain": chain } }),
        );
        run(
            ed,
            "routing.set_monitor",
            json!({ "track": tapper, "monitor": "in" }),
        );
        rig.sync();
        let out = rig.render(4 * LATENCY, 64);
        assert_eq!(
            heard(&out),
            vec![(at, 4.0)],
            "{tap}{}: the tap's impulse and the other track's meet at Main",
            if nested { ", nested" } else { "" }
        );
    }
}

/// The audio device's input, as a cable from its outputs to its inputs
/// makes it: each input channel carries the output channel of the same
/// index, `delay` frames late.
struct Loopback {
    delay: usize,
    /// Every frame played on each device output channel so far.
    played: Vec<Vec<f32>>,
    /// The input for the block being processed.
    block: Vec<Vec<f32>>,
}

impl bl_engine::DeviceInput for Loopback {
    fn channels(&self) -> usize {
        self.block.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.block[index]
    }
}

impl Loopback {
    fn new(channels: usize, delay: usize) -> Self {
        Self {
            delay,
            played: vec![Vec::new(); channels],
            block: vec![Vec::new(); channels],
        }
    }

    /// Processes `frames` frames, feeding the input what the outputs played
    /// `delay` frames ago, and returns Main's left channel.
    fn block(&mut self, rig: &mut Rig, frames: usize) -> Vec<f32> {
        let from = self.played[0].len();
        for (c, block) in self.block.iter_mut().enumerate() {
            *block = (0..frames)
                .map(|i| {
                    (from + i)
                        .checked_sub(self.delay)
                        .map_or(0.0, |at| self.played[c][at])
                })
                .collect();
        }
        rig.block_with_input(frames, &*self);
        let output = rig.processor.output();
        for (c, played) in self.played.iter_mut().enumerate() {
            if c < output.ext_channels() {
                played.extend_from_slice(&output.ext_channel(c)[..frames]);
            } else {
                played.extend(std::iter::repeat_n(0.0, frames));
            }
        }
        output.channel(0).to_vec()
    }
}

/// The set the loopback tests play: a track playing +0.5 into Main, a
/// track playing -0.5 on Ext. Out 3/4, and an external instrument bringing
/// 3/4 back, on an interface with four channels each way and 300 frames of
/// round trip.
fn loopback_rig(rate: bl_core::time::SampleRate, latency: bl_model::HardwareLatency) -> Rig {
    use bl_model::{Channel, External};
    use common::dc;

    let mut rig = Rig::with_config(64, |config| {
        config.sample_rate = rate;
        config.input_channels = 4;
        config.output_channels = 4;
        config.round_trip_latency = LOOPBACK_ROUND_TRIP;
    });
    let ed = &mut rig.editor;
    let reference = track(ed, TrackKind::Audio, None);
    dc(ed, reference, 0.5);
    let sender = track(ed, TrackKind::Audio, None);
    dc(ed, sender, -0.5);
    run(
        ed,
        "routing.set_output",
        json!({ "track": sender, "output": { "type": "external",
            "channel": { "type": "pair", "channel": 3 } } }),
    );
    let hardware_track = track(ed, TrackKind::Midi, None);
    let id = ed.new_id();
    let mut instrument = Device::new(
        id,
        "Hardware",
        DeviceKind::Native {
            device_type: bl_engine::testing::EXTERNAL.to_owned(),
        },
    )
    .with_role(bl_model::DeviceRole::Instrument)
    .with_external();
    instrument.external = Some(External {
        port: None,
        channel: 1,
        mpe: None,
        audio_from: Channel::Pair(3),
        audio_to: None,
        hardware_latency: latency,
        ..External::default()
    });
    ed.apply(Command::InsertDevice {
        track: hardware_track,
        index: 0,
        device: instrument,
    })
    .unwrap();
    rig.sync();
    rig
}

/// The interface's output and input latency, which a loopback through it
/// takes.
const LOOPBACK_ROUND_TRIP: u32 = 300;

/// The peak of Main over `4 * total_latency + 512` frames with a cable that
/// takes `cable` frames.
fn loopback_peak(rig: &mut Rig, total_latency: usize, cable: usize) -> f32 {
    let mut cable = Loopback::new(4, cable);
    let mut out = Vec::new();
    while out.len() < 4 * total_latency + 512 {
        out.extend(cable.block(rig, 64));
    }
    out.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

// Spec: external-devices R13
#[test]
fn an_external_instruments_round_trip_lines_up_at_main() {
    use bl_core::time::SampleRate;
    use bl_model::HardwareLatency;

    // Hardware Latency in milliseconds follows the sample rate, and in
    // samples it doesn't.
    for (rate, latency, hardware) in [
        (
            SampleRate::HZ_48000,
            HardwareLatency::Milliseconds { value: 4.0 },
            192,
        ),
        (
            SampleRate::HZ_48000,
            HardwareLatency::Samples { value: 100 },
            100,
        ),
        (
            SampleRate::HZ_96000,
            HardwareLatency::Milliseconds { value: 4.0 },
            384,
        ),
        (
            SampleRate::HZ_96000,
            HardwareLatency::Samples { value: 100 },
            100,
        ),
    ] {
        let total = LOOPBACK_ROUND_TRIP as usize + hardware;
        // The reference reaches Main `total` frames late, and the
        // instrument's sound comes back `total` frames late: they cancel on
        // every frame.
        let aligned = loopback_peak(&mut loopback_rig(rate, latency), total, total);
        assert!(aligned < 1e-6, "{rate:?} {latency:?}: peak {aligned}");
        // A cable one frame longer than the device reports shows as a
        // click, so the test can tell.
        let off_by_one = loopback_peak(&mut loopback_rig(rate, latency), total, total + 1);
        assert!(off_by_one > 0.4, "{rate:?} {latency:?}: peak {off_by_one}");
    }
}

// Spec: external-devices R13
#[test]
fn a_round_trip_that_changes_reaches_the_running_song() {
    use bl_model::External;

    let mut rig = Rig::with_config(64, |config| config.round_trip_latency = 100);
    let ed = &mut rig.editor;
    let reference = track(ed, TrackKind::Audio, None);
    common::dc(ed, reference, 0.5);
    let hardware_track = track(ed, TrackKind::Midi, None);
    let id = ed.new_id();
    let mut instrument = Device::new(
        id,
        "Hardware",
        DeviceKind::Native {
            device_type: bl_engine::testing::EXTERNAL.to_owned(),
        },
    )
    .with_role(bl_model::DeviceRole::Instrument)
    .with_external();
    instrument.external = Some(External::default());
    ed.apply(Command::InsertDevice {
        track: hardware_track,
        index: 0,
        device: instrument,
    })
    .unwrap();
    rig.sync();
    // With 100 frames of round trip, the reference reaches Main 100 frames
    // late.
    let first = rig.render(256, 64);
    assert_eq!(first[99], 0.0);
    assert!(first[100] > 0.0);
    // The device reports 300 now.
    let update = rig.compiler.round_trip(300);
    assert!(update.program.is_some());
    rig.sender.send_update(update);
    assert!(rig.sender.flush());
    let out = rig.render(512, 64);
    let delayed = out
        .iter()
        .position(|&s| s.abs() > 1e-6)
        .unwrap_or(out.len());
    // The new delay line starts empty: silence for 300 frames, then the
    // reference.
    assert_eq!(delayed, 300, "{delayed}");
    // The same figure again changes nothing.
    assert!(rig.compiler.round_trip(300).program.is_none());
}

// Spec: drum-racks R22, delay-compensation R2. A drum rack's sum is a
// summing point for its chains, its return chains and every send into one:
// each chain's output waits for the slowest chain, the sends take the chains
// there, and the rack's output waits for the slowest return chain it sums.
#[test]
fn a_drum_racks_chains_returns_and_sends_line_up() {
    use bl_engine::testing::{DC, VALUE};
    for size in [1, 64, 512, 4096] {
        let mut rig = Rig::new(4096);
        let midi = track(&mut rig.editor, TrackKind::Midi, None);
        let (_, chains, returns) = drum_rack(&mut rig.editor, midi, 2, 1);
        // Chain 0 has no latency, chain 1 has 300 frames; the return chain
        // adds 200 to what the sends bring it.
        for chain in &chains {
            effect_in(&mut rig.editor, *chain, DC, &[(VALUE, 1.0)]);
        }
        effect_in(&mut rig.editor, chains[1], DELAY, &[(DELAY_FRAMES, 300.0)]);
        effect_in(&mut rig.editor, returns[0], DELAY, &[(DELAY_FRAMES, 200.0)]);
        // The send is set before the program is made, so it needs no glide.
        let send = Command::SetChainSend {
            chain: chains[0],
            to: returns[0],
            level: 1.0,
        };
        rig.editor.apply(send).unwrap();
        rig.sync();
        let out = rig.render(2048, size);
        let edge = out.iter().position(|s| s.abs() > 1e-6).unwrap();
        assert_eq!(
            edge, 500,
            "block size {size}: 300 for the slowest chain, 200 for the return"
        );
        assert!(
            out[edge..].iter().all(|s| *s == 3.0),
            "block size {size}: both chains and the return arrive together"
        );
    }
}

/// Two audio tracks: the first carrying `sa`, the second `sb`, both playing
/// an impulse at frame 0 from 1.1.1. The caller syncs after further edits.
fn two_tracks(sa: f32, sb: f32) -> (Rig, TrackId, TrackId) {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(sa));
    media.insert(hash(2), impulse(sb));
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    let b = track(ed, TrackKind::Audio, None);
    clip(ed, a, hash(1));
    clip(ed, b, hash(2));
    (rig, a, b)
}

/// Whether two samples are the same to a hair.
fn near(value: f32, expected: f32) -> bool {
    (value - expected).abs() < 1e-4
}

// WP-5.36: a track delayed 10 ms is 480 frames late at 48 kHz, and one
// playing early holds the others back.
#[test]
fn a_track_delay_moves_it_frames_at_main_and_a_negative_one_holds_the_others_back() {
    // Spec: mixer R28.
    // A carries 1 and B carries 4, both at frame 0, so the height says which
    // one is where.
    for frames in [1usize, 100, 512, 4096] {
        let (mut rig, _a, b) = two_tracks(1.0, 4.0);
        rig.apply(Command::SetTrackDelay {
            track: b,
            delay: TrackDelay::Samples { value: 480 },
        });
        let out = rig.render(1024, frames);
        assert!(near(out[0], 1.0), "block {frames}: A stays at 0");
        assert!(near(out[480], 4.0), "block {frames}: B is 480 late");
        assert!(
            out[1..480].iter().all(|s| near(*s, 0.0)),
            "block {frames}: nothing between"
        );

        let (mut rig, _a, b) = two_tracks(1.0, 4.0);
        rig.apply(Command::SetTrackDelay {
            track: b,
            delay: TrackDelay::Samples { value: -480 },
        });
        let out = rig.render(1024, frames);
        assert!(near(out[0], 4.0), "block {frames}: B plays early");
        assert!(near(out[480], 1.0), "block {frames}: A is held back for it");
    }
}

// The same delay named in milliseconds: 10 ms is 480 frames at 48 kHz.
#[test]
fn a_delay_in_milliseconds_moves_it_by_the_frames_it_names() {
    // Spec: mixer R28.
    let (mut rig, _a, b) = two_tracks(1.0, 4.0);
    rig.apply(Command::SetTrackDelay {
        track: b,
        delay: TrackDelay::Milliseconds { value: 10.0 },
    });
    let out = rig.render(1024, 64);
    assert!(near(out[0], 1.0));
    assert!(near(out[480], 4.0));
}

// A delay in samples moves by exactly that many frames.
#[test]
fn a_delay_in_samples_moves_it_by_exactly_that_many_frames() {
    // Spec: mixer R28.
    let (mut rig, _a, b) = two_tracks(1.0, 4.0);
    rig.apply(Command::SetTrackDelay {
        track: b,
        delay: TrackDelay::Samples { value: 137 },
    });
    let out = rig.render(1024, 64);
    assert!(near(out[0], 1.0));
    assert!(near(out[137], 4.0));
    assert!(near(out[138], 0.0));
}

// Spec: mixer R28. Main has a delay too: it holds the whole mix back, since
// nothing feeds off Main.
#[test]
fn a_main_delay_holds_the_whole_mix_back() {
    let (mut rig, _a, _b) = two_tracks(1.0, 4.0);
    let main = rig.editor.document().main_id();
    rig.apply(Command::SetTrackDelay {
        track: main,
        delay: TrackDelay::Samples { value: 480 },
    });
    let out = rig.render(1024, 64);
    assert!(near(out[0], 0.0), "nothing lands at 0");
    assert!(near(out[480], 5.0), "the whole mix is 480 late");
    assert!(near(out[481], 0.0));
}

// With Delay Compensation off nothing is held back and the delay does nothing.
#[test]
fn with_compensation_off_a_track_delay_has_no_effect_and_nothing_is_held_back() {
    // Spec: delay-compensation R7; mixer R28.
    let (mut rig, _a, b) = two_tracks(1.0, 4.0);
    rig.apply(Command::SetTrackDelay {
        track: b,
        delay: TrackDelay::Samples { value: 480 },
    });
    rig.apply(Command::SetDelayCompensation { enabled: false });
    // The command is refused while compensation is off, keeping the value.
    assert!(
        rig.editor
            .apply(Command::SetTrackDelay {
                track: b,
                delay: TrackDelay::Samples { value: 0 },
            })
            .is_err(),
        "the delay can't be set while compensation is off"
    );
    assert_eq!(
        rig.editor.document().track(b).unwrap().delay(),
        TrackDelay::Samples { value: 480 },
        "the value stays as it was"
    );
    let out = rig.render(1024, 64);
    assert!(near(out[0], 5.0), "both impulses stay at 0");
    assert!(
        out[1..].iter().all(|s| near(*s, 0.0)),
        "nothing is held back or moved"
    );
}

// The Delay Compensation switch: off, nothing is held back (a device's
// latency included) and Track Delay has no effect.
#[test]
fn the_delay_compensation_switch_holds_nothing_back_and_disables_track_delay() {
    // Spec: delay-compensation R7; mixer R28.
    let (mut rig, _a, b) = two_tracks(1.0, 4.0);
    delay_device(&mut rig.editor, b);
    rig.apply(Command::SetTrackDelay {
        track: b,
        delay: TrackDelay::Samples { value: 480 },
    });
    rig.apply(Command::SetDelayCompensation { enabled: false });
    // Turning Reduced Latency When Monitoring on is refused too (R8).
    assert!(
        rig.editor
            .apply(Command::SetReducedLatencyMonitoring { enabled: true })
            .is_err(),
        "Reduced Latency When Monitoring needs compensation on"
    );
    let out = rig.render(1024, 64);
    // A reaches Main at 0; B's own device latency isn't held back either, so
    // it lands at 500, and the track delay adds nothing to it.
    assert!(near(out[0], 1.0), "A isn't held back");
    assert!(near(out[500], 4.0), "B's latency isn't compensated");
    assert!(
        out[501..].iter().all(|s| near(*s, 0.0)),
        "the track delay has no effect"
    );
}

// Reduced Latency When Monitoring, with Keep Monitoring Latency off: the
// monitored track is given the lowest latency.
#[test]
fn reduced_latency_monitoring_gives_a_monitored_track_no_compensation() {
    // Spec: delay-compensation R8, R9.
    let (mut rig, _a, b) = two_tracks(1.0, -1.0);
    delay_device(&mut rig.editor, b);
    run(
        &mut rig.editor,
        "mixer.set_arm",
        json!({ "track": b, "arm": true }),
    );
    rig.apply(Command::SetReducedLatencyMonitoring { enabled: true });
    rig.apply(Command::SetKeepMonitoringLatency {
        track: b,
        enabled: false,
    });
    let doc = rig.editor.document();
    let bt = doc.track(b).unwrap();
    assert!(doc.settings().reduced_latency_monitoring);
    assert!(!bt.keep_monitoring_latency());
    assert!(bt.mixer().arm, "B is armed");
    assert!(bt.routing().monitoring(bt.mixer().arm), "B is monitored");
    let out = rig.render(2 * LATENCY + 512, 64);
    let peak = out.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    assert!(
        peak > 0.5,
        "the monitored track isn't held with the rest: peak {peak}"
    );
}

// Keep Monitoring Latency gives the compensation back.
#[test]
fn keep_monitoring_latency_gives_a_monitored_tracks_compensation_back() {
    // Spec: delay-compensation R8, R9.
    let (mut rig, _a, b) = two_tracks(1.0, -1.0);
    delay_device(&mut rig.editor, b);
    run(
        &mut rig.editor,
        "mixer.set_arm",
        json!({ "track": b, "arm": true }),
    );
    rig.apply(Command::SetReducedLatencyMonitoring { enabled: true });
    // A new track keeps its monitoring latency, so B is compensated as usual.
    let out = rig.render(2 * LATENCY + 512, 64);
    assert_null(&out, "a track that keeps its monitoring latency cancels");
}

/// The multi-output test device (WP-5.45), at `value`, at the end of
/// `track`'s chain: an audio effect, so a delay may sit before it (the device
/// chains spec, R2).
fn multi_device(editor: &mut Editor, track: TrackId, value: f64) -> DeviceId {
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "Test Multi",
        DeviceKind::Native {
            device_type: MULTI.to_owned(),
        },
    )
    .with_role(DeviceRole::AudioEffect);
    device.params.insert(VALUE, value);
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

// A device's extra output is where the device sits in its chain: a device
// before it delays what its outputs carry, so a track that takes one holds
// its own clips to that place (WP-5.45, delay-compensation R2).
#[test]
fn an_instrument_output_behind_a_latency_device_lines_up() {
    // A single sample a second into the clip, past the clip's opening glide.
    let mut side = vec![0.0f32; 4_096];
    side[2_048] = 1.0;
    let mut media = MediaMap::new();
    media.insert(
        hash(4),
        Arc::new(SampleData::new(48_000, vec![side.clone(), side]).unwrap()),
    );
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    // The delay comes before the multi-output device, so its output sits
    // 500 frames into the source track's chain.
    let source = track(ed, TrackKind::Audio, None);
    delay_device(ed, source);
    let instrument = multi_device(ed, source, 1.0);
    // The tapper takes the instrument's output; its Monitor is off, so its
    // clip alone is heard, and the clip is held back to the route.
    let tapper = track(ed, TrackKind::Audio, None);
    clip(ed, tapper, hash(4));
    run(
        ed,
        "routing.set_monitor",
        json!({ "track": tapper, "monitor": "off" }),
    );
    run(
        ed,
        "routing.set_input",
        json!({ "track": tapper, "input": { "type": "track", "track": source,
                "output": { "device": instrument, "output": 0 } } }),
    );
    // The source's own signal stays out of Main, so the impulse is alone.
    run(
        ed,
        "routing.set_output",
        json!({ "track": source, "output": { "type": "sends_only" } }),
    );
    rig.sync();
    let out = rig.render(2_048 + 2 * LATENCY + 512, 64);
    let at = out
        .iter()
        .position(|&s| s.abs() > 1e-6)
        .expect("the impulse is heard");
    assert_eq!(
        at,
        2_048 + LATENCY,
        "the impulse lands where the instrument's output is, not at the chain's start"
    );
}

// Delay-compensation R5. A send's level is automated on the audio it scales,
// which reaches the return as late as the sending track's chain makes it.
#[test]
fn send_automation_lands_on_the_compensated_sample() {
    let mut rig = Rig::new(64);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    common::dc(ed, a, 1.0);
    delay_device(ed, a);
    let r = track(ed, TrackKind::Return, None);
    // A reaches Main only through its send, which rests closed.
    run(
        ed,
        "routing.set_output",
        json!({ "track": a, "output": { "type": "sends_only" } }),
    );
    rig.sync();
    let at = 1_000u64;
    rig.sender.send_param(bl_engine::ParamEvent::at_frame(
        bl_engine::NodeKey::Send { track: a, to: r },
        bl_engine::mixer::route::LEVEL,
        1.0,
        at,
    ));
    let out = rig.render(at as usize + LATENCY + 2_000, 64);
    // The send opens when the delayed audio gets there, then glides for 20 ms.
    let onset = at as usize + LATENCY;
    assert!(out[..onset].iter().all(|&s| s == 0.0), "silent before");
    assert!(
        out[onset + 960 + 64..].iter().all(|&s| near(s, 1.0)),
        "up after"
    );
}

/// A clip of one quarter-note C4 at the start of `track`.
fn note_clip(editor: &mut Editor, track: TrackId) {
    let id = editor.new_id();
    let notes = vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER, 100)];
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
}

/// A rig playing a MIDI track whose instrument is followed by a device keyed
/// from an audio track with `LATENCY` frames of latency, so the MIDI track
/// waits that long for its sidechain, and an empty track to edit.
fn keyed_midi_track() -> (Rig, TrackId) {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    let s = track(ed, TrackKind::Audio, None);
    clip(ed, s, hash(1));
    delay_device(ed, s);
    let m = track(ed, TrackKind::Midi, None);
    device(ed, m, INSTRUMENT, &[]);
    let key = key_device(ed, m);
    note_clip(ed, m);
    run(
        ed,
        "device.set_sidechain_input",
        json!({ "device": key, "input": { "type": "track", "track": s, "tap": "post_fx" } }),
    );
    let unrelated = track(ed, TrackKind::Midi, None);
    rig.sync();
    (rig, unrelated)
}

/// The frames the instruments played note-ons on.
fn note_ons(rig: &Rig) -> Vec<i64> {
    rig.probe
        .recorded_notes()
        .into_iter()
        .filter(|note| note.event.kind == bl_engine::NoteKind::On)
        .map(|note| note.frame)
        .collect()
}

// Delay-compensation R3. A MIDI track's clips are notes, not audio: they are
// held back to line up with the sidechain the track waits for.
#[test]
fn a_midi_tracks_notes_are_held_back_to_line_up_with_its_sidechain() {
    let (mut rig, _) = keyed_midi_track();
    rig.render(2 * LATENCY + 512, 64);
    assert_eq!(note_ons(&rig), vec![LATENCY as i64]);
}

// Delay-compensation R6. A recompile that leaves a MIDI track's delay alone
// keeps the notes it holds back.
#[test]
fn a_recompile_keeps_the_notes_a_midi_track_holds_back() {
    let (mut rig, unrelated) = keyed_midi_track();
    // The note is on its way, 400 frames from sounding.
    rig.render(100, 64);
    assert!(note_ons(&rig).is_empty());
    device(&mut rig.editor, unrelated, THROUGH, &[]);
    assert!(rig.sync().program);
    rig.render(LATENCY + 512, 64);
    assert_eq!(note_ons(&rig), vec![LATENCY as i64]);
}

/// Renders `frames` frames and asserts all of them are `value`.
fn assert_steady(rig: &mut Rig, frames: usize, value: f32, what: &str) {
    let out = rig.render(frames, 64);
    assert!(out.iter().all(|&s| near(s, value)), "{what}");
}

// Delay-compensation R6. A recompile that leaves a route's delay alone keeps
// the audio in its line.
#[test]
fn a_recompile_keeps_the_audio_in_a_routes_delay() {
    let mut rig = Rig::new(64);
    let ed = &mut rig.editor;
    // A reaches Main 500 frames before B does, so its route holds it back.
    let a = track(ed, TrackKind::Midi, None);
    common::dc(ed, a, 1.0);
    let b = track(ed, TrackKind::Midi, None);
    delay_device(ed, b);
    let unrelated = track(ed, TrackKind::Midi, None);
    rig.sync();
    rig.render(2 * LATENCY, 64);
    assert_steady(&mut rig, 512, 1.0, "A is heard in full");
    device(&mut rig.editor, unrelated, THROUGH, &[]);
    assert!(rig.sync().program);
    assert_steady(&mut rig, 2 * LATENCY + 512, 1.0, "A goes on in full");
}

// The same for a track's clips, held back through a line of their own.
#[test]
fn a_recompile_keeps_the_clips_in_a_tracks_delay() {
    let mut media = MediaMap::new();
    media.insert(
        hash(1),
        Arc::new(SampleData::new(48_000, vec![vec![0.5; 48_000]; 2]).unwrap()),
    );
    let mut rig = playing(media);
    let ed = &mut rig.editor;
    // X takes its input from B, which has latency, and so waits for it.
    let b = track(ed, TrackKind::Audio, None);
    delay_device(ed, b);
    let x = track(ed, TrackKind::Audio, None);
    clip(ed, x, hash(1));
    run(
        ed,
        "routing.set_input",
        json!({ "track": x, "input": { "type": "track", "track": b } }),
    );
    let unrelated = track(ed, TrackKind::Midi, None);
    rig.sync();
    rig.render(2 * LATENCY, 64);
    assert_steady(&mut rig, 512, 0.5, "X is heard in full");
    device(&mut rig.editor, unrelated, THROUGH, &[]);
    assert!(rig.sync().program);
    assert_steady(&mut rig, 2 * LATENCY + 512, 0.5, "X goes on in full");
}
