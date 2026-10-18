//! Sampler, the native multisampler (WP-5.13; the multisampler spec, R1 to
//! R31): which layers a note plays, zone fades, the Sample Selector, Round
//! Robin and its resets, pitch, the markers and loops, interpolation, the
//! volume envelope and its loop modes, voices and the steal fade, and the
//! output controls. Every block runs as a real-time section under the
//! allocation trap, at block sizes 1, 100, 512 and 4,096.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::multisampler::*;
use bl_engine::{
    Block, Collector, DeviceLayers, LayerAudio, LayerSample, Node, NodeEvent, NoteBuffer,
    NoteEvent, NoteKind, ParamValue, Registry, TransportInfo,
};
use bl_model::{ContentHash, DeviceRole, Layer, LayerId, ReleaseMode, SustainMode, Zone};
use common::{CHANNELS, RATE, device, node};
use lspk_dsp::filter::{FilterBank, FilterType, VoiceParams, pitch_of};

/// The block sizes every test runs at.
const BLOCKS: [usize; 4] = [1, 100, 512, 4096];

const HZ: f64 = 48_000.0;

/// A note for [`Rig::render`]: the frame it happens on, what it does, its
/// key and velocity.
type Played = (usize, NoteKind, u8, f32);

fn on(frame: usize, key: u8, velocity: f32) -> Played {
    (frame, NoteKind::On, key, velocity)
}

fn off(frame: usize, key: u8) -> Played {
    (frame, NoteKind::Off, key, 0.0)
}

/// A multisampler node and what it takes to drive it: the layers it was
/// sent, the block size it's run at and the transport.
struct Rig {
    node: Box<dyn Node>,
    collector: Collector,
    block: usize,
    frame: i64,
    playing: bool,
    tempo: TempoMap,
}

impl Rig {
    fn new(params: &[(bl_model::ParamId, f64)], block: usize) -> Self {
        let mut registry = Registry::empty();
        bl_devices::multisampler::register(&mut registry);
        let device = device(&multisampler_type(), params);
        Self {
            node: node(&registry, &device, 4096),
            collector: Collector::new(),
            block,
            frame: 0,
            playing: false,
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        }
    }

    /// Sends the layers, each with its audio.
    fn give(&mut self, layers: Vec<(Layer, Arc<bl_media::SampleData>)>, linear: bool) {
        let layers = layers
            .into_iter()
            .map(|(layer, sample)| LayerAudio {
                layer,
                sample: Some(LayerSample::Whole(sample)),
            })
            .collect();
        self.send(layers, linear);
    }

    /// Sends layers with their audio made, and says how many underruns the
    /// node has counted.
    fn send(&mut self, layers: Vec<LayerAudio>, linear: bool) -> Arc<std::sync::atomic::AtomicU64> {
        let data = DeviceLayers::new(layers, false, linear);
        let underruns = Arc::clone(&data.underruns);
        self.node.receive_data(self.collector.handle().data(data));
        underruns
    }

    /// Plays `frames` frames from where the last call stopped, `notes` at
    /// their frames from this call's start and `events` setting
    /// parameters.
    fn render_with(
        &mut self,
        frames: usize,
        notes: &[Played],
        events: &[(usize, bl_model::ParamId, f64)],
    ) -> Vec<Vec<f32>> {
        let mut out: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(frames)).collect();
        let mut start = 0;
        while start < frames {
            let len = self.block.min(frames - start);
            let here = |at: usize| at >= start && at < start + len;
            let block_notes: Vec<NoteEvent> = notes
                .iter()
                .filter(|n| here(n.0))
                .map(|&(at, kind, key, velocity)| NoteEvent {
                    offset: (at - start) as u32,
                    kind,
                    channel: 0,
                    key,
                    velocity,
                    note_id: None,
                })
                .collect();
            let block_events: Vec<NodeEvent> = events
                .iter()
                .filter(|e| here(e.0))
                .map(|&(at, param, value)| NodeEvent {
                    offset: (at - start) as u32,
                    param,
                    value: ParamValue::Set(value),
                })
                .collect();
            let mut data = vec![0.0f32; CHANNELS * len];
            let mut sink = NoteBuffer::with_capacity(block_notes.len().max(1));
            {
                let mut block = Block::new(
                    &mut data,
                    CHANNELS,
                    len,
                    len,
                    &block_events,
                    TransportInfo {
                        playing: self.playing,
                        position: Frames(self.frame + start as i64),
                    },
                    RATE,
                )
                .with_notes(&block_notes, &mut sink)
                .with_tempo(&self.tempo);
                bl_rt::rt_test!({ self.node.process(&mut block) });
            }
            for (channel, chunk) in out.iter_mut().zip(data.chunks(len)) {
                channel.extend_from_slice(chunk);
            }
            start += len;
        }
        self.frame += frames as i64;
        out
    }

    fn render(&mut self, frames: usize, notes: &[Played]) -> Vec<Vec<f32>> {
        self.render_with(frames, notes, &[])
    }
}

/// Decoded audio of `frames` frames at the engine's rate, one channel.
fn sample(frames: usize, value: impl Fn(usize) -> f32) -> Arc<bl_media::SampleData> {
    Arc::new(bl_media::SampleData::new(RATE.hz(), vec![(0..frames).map(value).collect()]).unwrap())
}

/// A layer for `sample`, rooted on C3 with every zone whole.
fn layer(id: u32, sample: &Arc<bl_media::SampleData>) -> Layer {
    Layer::new(
        LayerId(id),
        format!("Layer {id}"),
        ContentHash([id as u8; 32]),
        sample.frames() as u64,
        RATE.hz(),
        1,
    )
}

/// A layer of a constant `level`, one second long.
fn flat(id: u32, level: f32) -> (Layer, Arc<bl_media::SampleData>) {
    let sample = sample(48_000, |_| level);
    (layer(id, &sample), sample)
}

fn level_at(out: &[Vec<f32>], frame: usize) -> f32 {
    out[0][frame]
}

/// The amplitude of `hz` in `signal`'s `frames` from `from`, by a least
/// squares fit, and the RMS of what's left.
fn fit(signal: &[f32], from: usize, frames: usize, hz: f64) -> (f64, f64) {
    let (mut cc, mut ss, mut cs, mut xc, mut xs) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..frames {
        let phase = core::f64::consts::TAU * hz * (from + i) as f64 / HZ;
        let (s, c) = phase.sin_cos();
        let x = f64::from(signal[from + i]);
        cc += c * c;
        ss += s * s;
        cs += c * s;
        xc += x * c;
        xs += x * s;
    }
    let det = cc * ss - cs * cs;
    let (a, b) = ((xc * ss - xs * cs) / det, (xs * cc - xc * cs) / det);
    let mut residual = 0.0;
    for i in 0..frames {
        let phase = core::f64::consts::TAU * hz * (from + i) as f64 / HZ;
        let x = f64::from(signal[from + i]) - (a * phase.cos() + b * phase.sin());
        residual += x * x;
    }
    (a.hypot(b), (residual / frames as f64).sqrt())
}

// Spec: multisampler R1
#[test]
fn the_multisampler_is_described_and_laid_out() {
    let device_type = multisampler_type();
    assert_eq!(device_type.name, "Sampler");
    assert_eq!(device_type.role, DeviceRole::Instrument);
    device_type.description.check().unwrap();
    assert!(
        device_type
            .description
            .params
            .iter()
            .all(|p| p.id.0 < 28 || p.id.0 >= 100),
        "WP-5.14's parameters start at 100"
    );
}

// Spec: multisampler R4
#[test]
fn each_key_and_velocity_plays_the_layers_that_take_it() {
    for block in BLOCKS {
        let mut rig = Rig::new(&[(ENV_RELEASE, 0.0)], block);
        let (mut low, a) = flat(1, 1.0);
        low.key = Zone::new(0, 59);
        let (mut soft, b) = flat(2, 2.0);
        soft.key = Zone::new(60, 127);
        soft.velocity = Zone::new(1, 63);
        let (mut hard, c) = flat(3, 4.0);
        hard.key = Zone::new(60, 127);
        hard.velocity = Zone::new(64, 127);
        let (mut never, d) = flat(4, 8.0);
        never.select = Zone::new(5, 5);
        rig.give(vec![(low, a), (soft, b), (hard, c), (never, d)], false);
        let out = rig.render(
            9000,
            &[
                on(0, 40, 1.0),
                off(2000, 40),
                on(3000, 70, 0.2),
                off(5000, 70),
                on(6000, 70, 1.0),
                off(8000, 70),
            ],
        );
        assert!((level_at(&out, 1000) - 1.0).abs() < 1e-5, "{block}");
        assert!((level_at(&out, 4000) - 2.0).abs() < 1e-5, "{block}");
        assert!((level_at(&out, 7000) - 4.0).abs() < 1e-5, "{block}");
        let mut silent = Rig::new(&[], block);
        let (mut only, e) = flat(5, 1.0);
        only.key = Zone::new(0, 10);
        silent.give(vec![(only, e)], false);
        let out = silent.render(500, &[on(0, 60, 1.0)]);
        assert!(
            out.iter().flatten().all(|s| *s == 0.0),
            "no layer, no sound"
        );
    }
}

// Spec: multisampler R5
#[test]
fn neighbouring_zones_crossfade_over_their_fades_linear_and_constant_power() {
    for block in BLOCKS {
        for (mode, pow) in [(0.0, false), (1.0, true)] {
            let mut rig = Rig::new(&[(FADE_MODE, mode), (ENV_RELEASE, 0.0)], block);
            let (mut a, sa) = flat(1, 1.0);
            a.key = Zone {
                low: 0,
                high: 60,
                fade_low: 0,
                fade_high: 10,
            };
            let (mut b, sb) = flat(2, 4.0);
            b.key = Zone {
                low: 50,
                high: 127,
                fade_low: 10,
                fade_high: 0,
            };
            rig.give(vec![(a, sa), (b, sb)], false);
            for key in 50..=60u8 {
                let notes = [on(0, key, 1.0), off(400, key)];
                let out = rig.render(500, &notes);
                let x = f64::from(key - 50) / 10.0;
                let share = |x: f64| {
                    if pow {
                        (x * core::f64::consts::FRAC_PI_2).sin()
                    } else {
                        x
                    }
                };
                let expected = share(1.0 - x) + 4.0 * share(x);
                let got = f64::from(level_at(&out, 200));
                assert!(
                    (20.0 * (got / expected).log10()).abs() < 0.1,
                    "key {key} pow {pow}: {got} against {expected}"
                );
            }
        }
    }
}

// Spec: multisampler R6
#[test]
fn the_sample_selector_chooses_new_notes_and_leaves_sounding_ones() {
    for block in BLOCKS {
        let mut rig = Rig::new(&[], block);
        let (mut a, sa) = flat(1, 1.0);
        a.select = Zone::new(0, 0);
        let (mut b, sb) = flat(2, 2.0);
        b.select = Zone::new(1, 1);
        rig.give(vec![(a, sa), (b, sb)], false);
        let out = rig.render_with(
            6000,
            &[on(0, 60, 1.0), on(3000, 62, 1.0)],
            &[(2000, SELECTOR, 1.0)],
        );
        assert!((level_at(&out, 1000) - 1.0).abs() < 1e-5, "{block}");
        assert!(
            (level_at(&out, 2500) - 1.0).abs() < 1e-5,
            "moving the selector leaves a sounding note"
        );
        assert!(
            (level_at(&out, 4000) - 3.0).abs() < 1e-5,
            "a new note takes the new value: {}",
            level_at(&out, 4000)
        );
    }
}

/// Three layers of one key zone playing 1, 2 and 4, for Round Robin.
fn rotation() -> Vec<(Layer, Arc<bl_media::SampleData>)> {
    vec![flat(1, 1.0), flat(2, 2.0), flat(3, 4.0)]
}

/// Which of the rotation's layers each of `count` notes played: a note every
/// 1,000 frames, each held 100 frames.
fn rotation_run(rig: &mut Rig, count: usize) -> Vec<f32> {
    let mut notes = Vec::new();
    for n in 0..count {
        notes.push(on(n * 1000, 60, 1.0));
        notes.push(off(n * 1000 + 100, 60));
    }
    let out = rig.render(count * 1000, &notes);
    (0..count).map(|n| out[0][n * 1000 + 50].round()).collect()
}

// Spec: multisampler R8
#[test]
fn round_robin_rotates_in_each_mode_and_renders_the_same_every_time() {
    for block in BLOCKS {
        let params = |mode: f64| [(ROUND_ROBIN, 1.0), (RR_MODE, mode), (ENV_RELEASE, 0.0)];
        let mut forward = Rig::new(&params(0.0), block);
        forward.give(rotation(), false);
        assert_eq!(
            rotation_run(&mut forward, 6),
            [1.0, 2.0, 4.0, 1.0, 2.0, 4.0]
        );
        let mut backward = Rig::new(&params(1.0), block);
        backward.give(rotation(), false);
        assert_eq!(
            rotation_run(&mut backward, 6),
            [4.0, 2.0, 1.0, 4.0, 2.0, 1.0]
        );
        let mut other = Rig::new(&params(2.0), block);
        other.give(rotation(), false);
        let played = rotation_run(&mut other, 40);
        assert!(
            played.windows(2).all(|w| w[0] != w[1]),
            "never the same twice"
        );
        assert!([1.0, 2.0, 4.0].iter().all(|v| played.contains(v)));
        // A random mode repeats bit for bit between renders.
        let render = |block| {
            let mut random = Rig::new(&params(3.0), block);
            random.give(rotation(), false);
            rotation_run(&mut random, 30)
        };
        assert_eq!(render(block), render(block));
        assert!(
            render(block).windows(2).any(|w| w[0] == w[1]),
            "repeats are possible"
        );
        // Without Round Robin every layer plays.
        let mut all = Rig::new(&[(ENV_RELEASE, 0.0)], block);
        all.give(rotation(), false);
        assert_eq!(rotation_run(&mut all, 2), [7.0, 7.0]);
    }
}

// Spec: multisampler R9
#[test]
fn the_reset_interval_restarts_each_mode_on_the_grid() {
    // At 120 bpm a quarter note is 24,000 frames.
    for block in BLOCKS {
        let params = |mode: f64| {
            [
                (ROUND_ROBIN, 1.0),
                (RR_MODE, mode),
                (RESET_INTERVAL, 3.0),
                (ENV_RELEASE, 0.0),
            ]
        };
        let quarter = 24_000;
        let run = |mode: f64| {
            let mut rig = Rig::new(&params(mode), block);
            rig.playing = true;
            rig.give(rotation(), false);
            let times = [
                100,
                6_000,
                quarter + 100,
                quarter + 6_000,
                2 * quarter + 100,
            ];
            let mut notes = Vec::new();
            for &t in &times {
                notes.push(on(t, 60, 1.0));
                notes.push(off(t + 100, 60));
            }
            let out = rig.render(3 * quarter, &notes);
            times.map(|t| out[0][t + 50].round())
        };
        assert_eq!(run(0.0), [1.0, 2.0, 1.0, 2.0, 1.0], "Forward from the top");
        assert_eq!(
            run(1.0),
            [4.0, 2.0, 4.0, 2.0, 4.0],
            "Backward from the bottom"
        );
        for mode in [2.0, 3.0] {
            let played = run(mode);
            assert_eq!(
                played[2], played[0],
                "mode {mode} restarts on the first it played"
            );
            assert_eq!(played[4], played[0], "mode {mode}");
        }
    }
}

// Spec: multisampler R15
#[test]
fn a_layer_plays_at_its_root_key_and_a_semitone_a_key() {
    for block in BLOCKS {
        let impulse = sample(4800, |i| if i == 1000 { 1.0 } else { 0.0 });
        let first = |out: &[Vec<f32>]| out[0].iter().position(|s| s.abs() > 0.9).unwrap();
        for (root, key, at) in [
            (60u8, 60u8, 1000),
            (60, 72, 500),
            (48, 60, 500),
            (72, 60, 2000),
        ] {
            let mut rig = Rig::new(&[], block);
            let mut l = layer(1, &impulse);
            l.root_key = root;
            rig.give(vec![(l, Arc::clone(&impulse))], false);
            let out = rig.render(4800, &[on(0, key, 1.0)]);
            assert_eq!(first(&out), at, "root {root} key {key}");
        }
        // Volume and Pan of the layer.
        let mut rig = Rig::new(&[], block);
        let (mut l, s) = flat(1, 1.0);
        l.volume = 0.5;
        l.pan = 50.0;
        rig.give(vec![(l, s)], false);
        let out = rig.render(500, &[on(0, 60, 1.0)]);
        assert!(out[0][200].abs() < 1e-6, "hard right has no left");
        assert!((out[1][200] - 0.5).abs() < 1e-5, "at the layer's Volume");
    }
}

// Spec: multisampler R16
#[test]
fn a_voice_plays_from_sample_start_to_sample_end_and_link_starts_at_the_loop() {
    for block in BLOCKS {
        let whole = sample(4800, |_| 1.0);
        let mut rig = Rig::new(&[], block);
        let mut l = layer(1, &whole);
        l.start = 1200;
        l.end = 2400;
        l.loop_start = 1200;
        l.loop_end = 2400;
        l.release_loop = 2400;
        rig.give(vec![(l, Arc::clone(&whole))], false);
        let out = rig.render(4800, &[on(0, 60, 1.0)]);
        assert!(
            out[0][..1200].iter().all(|s| (s - 1.0).abs() < 1e-5),
            "{block}"
        );
        assert!(out[0][1200..].iter().all(|s| *s == 0.0));

        let impulse = sample(4800, |i| if i == 1500 { 1.0 } else { 0.0 });
        let mut rig = Rig::new(&[], block);
        let mut l = layer(1, &impulse);
        l.loop_start = 1000;
        l.link = true;
        rig.give(vec![(l, Arc::clone(&impulse))], false);
        let out = rig.render(4800, &[on(0, 60, 1.0)]);
        let at = out[0].iter().position(|s| s.abs() > 0.9).unwrap();
        assert_eq!(at, 500, "Link starts at Loop Start");
    }
}

/// A ramp of `frames` frames: frame i holds i / frames.
fn ramp(frames: usize) -> Arc<bl_media::SampleData> {
    sample(frames, move |i| i as f32 / frames as f32)
}

/// A layer of a 4,800-frame ramp looping from 1,000 to 2,000.
fn looped(sustain: SustainMode, release: ReleaseMode) -> (Layer, Arc<bl_media::SampleData>) {
    let sample = ramp(4800);
    let mut l = layer(1, &sample);
    l.loop_start = 1000;
    l.loop_end = 2000;
    l.release_loop = 3000;
    l.sustain = sustain;
    l.release = release;
    (l, sample)
}

// Spec: multisampler R17
#[test]
fn each_sustain_mode_loops_as_it_says() {
    for block in BLOCKS {
        let value = |frame: usize| frame as f32 / 4800.0;
        let mut straight = Rig::new(&[], block);
        straight.give(vec![looped(SustainMode::Off, ReleaseMode::Off)], false);
        let out = straight.render(6000, &[on(0, 60, 1.0)]);
        assert!(
            (out[0][3000] - value(3000)).abs() < 1e-5,
            "plays straight through"
        );
        assert_eq!(out[0][5000], 0.0, "and ends at Sample End");

        let mut forward = Rig::new(&[], block);
        forward.give(vec![looped(SustainMode::Loop, ReleaseMode::Off)], false);
        let out = forward.render(6000, &[on(0, 60, 1.0)]);
        for i in 0..900 {
            assert!((out[0][1000 + i] - value(1000 + i)).abs() < 1e-5);
            assert!(
                (out[0][2000 + i] - value(1000 + i)).abs() < 1e-5,
                "the loop wraps to its start: {i}"
            );
            assert!((out[0][3000 + i] - value(1000 + i)).abs() < 1e-5);
        }

        let mut back = Rig::new(&[], block);
        back.give(
            vec![looped(SustainMode::BackAndForth, ReleaseMode::Off)],
            false,
        );
        let out = back.render(6000, &[on(0, 60, 1.0)]);
        for i in 0..900 {
            assert!(
                (out[0][2000 + i] - value(1998 - i)).abs() < 1e-5,
                "back from Loop End: {i}"
            );
        }
        assert!(
            (out[0][3003] - value(1000 + 5)).abs() < 1e-5
                && (out[0][3010] - value(1000 + 12)).abs() < 1e-5,
            "and forward again from Loop Start"
        );
    }
}

// Spec: multisampler R18
#[test]
fn each_release_mode_plays_the_release_as_it_says() {
    for block in BLOCKS {
        let value = |frame: usize| frame as f32 / 4800.0;
        // A 20-second release keeps the level near 1 for the whole check.
        let params = [(ENV_RELEASE, 20_000.0)];
        let held_then_off = [on(0, 60, 1.0), off(2500, 60)];
        // At frame 2500 the voice is 500 frames into its second pass.
        let mut keep = Rig::new(&params, block);
        keep.give(vec![looped(SustainMode::Loop, ReleaseMode::Off)], false);
        let out = keep.render(8000, &held_then_off);
        assert!(
            out[0][3000..7000]
                .iter()
                .all(|s| *s <= value(2000) + 1e-3 && *s >= value(1000) * 0.99),
            "Off stays in the loop until the release ends"
        );

        let mut release = Rig::new(&params, block);
        release.give(vec![looped(SustainMode::Loop, ReleaseMode::Release)], false);
        let out = release.render(8000, &held_then_off);
        // 1,500 is the voice's position at the note's end.
        assert!(
            (out[0][5000] - value(1500 + 2500) * (1.0 - 2500.0 / 960_000.0)).abs() < 2e-3,
            "Release plays on through Loop End: {}",
            out[0][5000]
        );
        assert_eq!(out[0][7000], 0.0, "and ends at Sample End");

        let mut looping = Rig::new(&params, block);
        looping.give(vec![looped(SustainMode::Loop, ReleaseMode::Loop)], false);
        let out = looping.render(9000, &held_then_off);
        assert!(
            out[0][6000..9000]
                .iter()
                .all(|s| *s >= value(3000) * 0.99 && *s <= value(4800)),
            "Release Loop loops from its point to Sample End"
        );

        let mut bounce = Rig::new(&params, block);
        bounce.give(
            vec![looped(SustainMode::Loop, ReleaseMode::BackAndForth)],
            false,
        );
        let out = bounce.render(9000, &held_then_off);
        // The first turn at Sample End is at frame 5,800.
        let after: Vec<f32> = out[0][5000..9000].to_vec();
        assert!(
            after
                .iter()
                .all(|s| *s >= value(3000) * 0.99 && *s <= value(4800))
        );
        assert!(
            after.windows(2).any(|w| w[1] < w[0]) && after.windows(2).any(|w| w[1] > w[0]),
            "Back-and-Forth Release Loop goes both ways"
        );
    }
}

/// The largest jump between neighbouring frames.
fn biggest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

// Spec: multisampler R19
#[test]
fn the_loop_crossfade_has_no_click_in_either_law() {
    for block in BLOCKS {
        // 1.5 cycles of a cosine: a bare wrap jumps from −1 to 1.
        let cosine = sample(4800, |i| {
            (core::f64::consts::TAU * 15.0 * i as f64 / HZ).cos() as f32
        });
        for linear in [false, true] {
            let mut bare = Rig::new(&[], block);
            let mut l = layer(1, &cosine);
            l.sustain = SustainMode::Loop;
            bare.give(vec![(l.clone(), Arc::clone(&cosine))], linear);
            let out = bare.render(15_000, &[on(0, 60, 1.0)]);
            assert!(biggest_step(&out[0][1..]) > 1.5, "a bare wrap clicks");

            let mut rig = Rig::new(&[], block);
            l.crossfade_ms = 20.0;
            rig.give(vec![(l, Arc::clone(&cosine))], linear);
            let out = rig.render(15_000, &[on(0, 60, 1.0)]);
            assert!(
                biggest_step(&out[0][1..]) < 0.05,
                "linear {linear}: {}",
                biggest_step(&out[0][1..])
            );
        }
    }
}

/// How many times `signal` rises through zero.
fn rising(signal: &[f32]) -> usize {
    signal
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count()
}

// Spec: multisampler R19
#[test]
fn loop_detune_retunes_inside_the_loop() {
    for block in BLOCKS {
        // A 1 kHz sine of exactly 100 cycles, so the loop wraps seamlessly.
        let sine = sample(4800, |i| {
            (core::f64::consts::TAU * 1000.0 * i as f64 / HZ).sin() as f32
        });
        let count = |detune: f64| {
            let mut rig = Rig::new(&[], block);
            let mut l = layer(1, &sine);
            l.sustain = SustainMode::Loop;
            l.loop_detune = detune;
            rig.give(vec![(l, Arc::clone(&sine))], false);
            let out = rig.render(16_800, &[on(0, 60, 1.0)]);
            rising(&out[0][4800..12_800])
        };
        let (plain, detuned) = (count(0.0), count(50.0));
        // 8,000 frames of 1 kHz hold 166.7 cycles; 50 cents up, 171.5.
        assert!((165..=168).contains(&plain), "{plain}");
        assert!((170..=173).contains(&detuned), "{detuned}");
    }
}

// Spec: multisampler R21
#[test]
fn reverse_plays_from_the_end_through_the_loop_to_the_start() {
    for block in BLOCKS {
        let value = |frame: usize| frame as f32 / 4800.0;
        let mut rig = Rig::new(&[(REVERSE, 1.0), (ENV_RELEASE, 0.0)], block);
        rig.give(vec![looped(SustainMode::Off, ReleaseMode::Off)], false);
        let out = rig.render(6000, &[on(0, 60, 1.0)]);
        assert!(
            (out[0][0] - value(4799)).abs() < 1e-5,
            "starts at Sample End"
        );
        assert!(
            (out[0][1000] - value(3799)).abs() < 1e-5,
            "and runs backwards"
        );
        assert!((out[0][4790] - value(9)).abs() < 1e-5, "to Sample Start");
        assert_eq!(out[0][5000], 0.0);

        let mut held = Rig::new(&[(REVERSE, 1.0), (ENV_RELEASE, 0.0)], block);
        held.give(vec![looped(SustainMode::Loop, ReleaseMode::Off)], false);
        let out = held.render(9000, &[on(0, 60, 1.0), off(7000, 60)]);
        assert!(
            out[0][2900..7000]
                .iter()
                .all(|s| *s >= value(1000) - 1e-5 && *s <= value(2000) + 1e-5),
            "through the sustain loop"
        );
    }
}

// Spec: multisampler R22
#[test]
fn the_root_key_plays_the_frames_exactly_in_every_mode() {
    for block in BLOCKS {
        let noise = sample(2000, |i| {
            let mut x = (i as u32).wrapping_mul(2_654_435_761) ^ 0x9E37_79B9;
            x ^= x >> 15;
            (x % 2000) as f32 / 1000.0 - 1.0
        });
        for mode in 0..3 {
            let mut rig = Rig::new(&[(INTERPOLATION, f64::from(mode))], block);
            rig.give(vec![(layer(1, &noise), Arc::clone(&noise))], false);
            let out = rig.render(2000, &[on(0, 60, 1.0)]);
            for (i, (played, frame)) in out[0]
                .iter()
                .zip(noise.channel(0).unwrap())
                .enumerate()
                .take(1990)
            {
                assert_eq!(played, frame, "mode {mode} frame {i}");
            }
        }
    }
}

// Spec: multisampler R22
#[test]
fn best_interpolation_keeps_aliases_eighty_db_down() {
    // A 3 kHz sine an octave up and 49 cents: 6.17 kHz, at positions that
    // fall between frames.
    let sine = sample(48_000, |i| {
        (core::f64::consts::TAU * 3000.0 * i as f64 / HZ).sin() as f32
    });
    let hz = 3000.0 * 2f64.powf(1.0 + 49.0 / 1200.0);
    let residual = |mode: f64| {
        let mut rig = Rig::new(&[(INTERPOLATION, mode)], 4096);
        let mut l = layer(1, &sine);
        l.detune = 49.0;
        rig.give(vec![(l, Arc::clone(&sine))], false);
        let out = rig.render(24_000, &[on(0, 72, 1.0)]);
        let (amplitude, rms) = fit(&out[0], 2000, 16_000, hz);
        if mode == 2.0 {
            assert!(
                (amplitude - 1.0).abs() < 0.01,
                "Best keeps the level: {amplitude}"
            );
        }
        20.0 * (rms / amplitude).log10()
    };
    let best = residual(2.0);
    assert!(best < -80.0, "Best leaves {best} dB");
    assert!(residual(0.0) > best + 20.0, "Normal is far worse");
    assert!(residual(1.0) > best + 10.0, "Good is between");
}

/// Plays a note on a constant sample and returns the level every `step`
/// frames.
fn envelope_levels(
    params: &[(bl_model::ParamId, f64)],
    notes: &[Played],
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let mut rig = Rig::new(params, block);
    rig.playing = true;
    let (l, s) = flat(1, 1.0);
    rig.give(vec![(l, s)], false);
    rig.render(frames, notes).swap_remove(0)
}

// Spec: multisampler R24
#[test]
fn the_envelope_takes_its_levels_times_and_slopes() {
    for block in BLOCKS {
        let params = [
            (ENV_INITIAL, 20.0),
            (ENV_PEAK, 100.0),
            (ENV_SUSTAIN, 50.0),
            (ENV_ATTACK, 100.0),
            (ENV_DECAY, 100.0),
            (ENV_RELEASE, 100.0),
        ];
        let ms = |t: f64| (t * 48.0) as usize;
        let levels = envelope_levels(
            &params,
            &[on(0, 60, 1.0), off(ms(500.0), 60)],
            ms(700.0),
            block,
        );
        let near = |t: f64, expected: f32| {
            assert!(
                (levels[ms(t)] - expected).abs() < 0.01,
                "at {t} ms: {} not {expected}",
                levels[ms(t)]
            );
        };
        near(1.0, 0.208);
        near(50.0, 0.6);
        near(100.0, 1.0);
        near(150.0, 0.75);
        near(200.0, 0.5);
        near(400.0, 0.5);
        near(550.0, 0.25);
        assert_eq!(
            levels[ms(650.0)],
            0.0,
            "the voice ends when the release does"
        );
        // A positive attack slope moves fast first: x^(4^-1) at half time.
        let steep = envelope_levels(
            &[
                (ENV_INITIAL, 0.0),
                (ENV_ATTACK, 100.0),
                (ENV_ATTACK_SLOPE, 100.0),
            ],
            &[on(0, 60, 1.0)],
            ms(200.0),
            block,
        );
        assert!(
            (steep[ms(50.0)] - 0.5f32.powf(0.25)).abs() < 0.01,
            "{}",
            steep[ms(50.0)]
        );
    }
}

// Spec: multisampler R25
#[test]
fn each_envelope_loop_mode_repeats_as_it_says() {
    for block in BLOCKS {
        let ms = |t: f64| (t * 48.0) as usize;
        let cycle = [
            (ENV_INITIAL, 0.0),
            (ENV_ATTACK, 100.0),
            (ENV_DECAY, 100.0),
            (ENV_SUSTAIN, 50.0),
            (ENV_LOOP_TIME, 100.0),
            (ENV_RELEASE, 50.0),
        ];
        // Loop: attack, decay and the way back repeat every 300 ms.
        let mut params = cycle.to_vec();
        params.push((ENV_LOOP, 1.0));
        let levels = envelope_levels(&params, &[on(0, 60, 1.0)], ms(1000.0), block);
        for t in [10.0, 50.0, 150.0, 250.0, 290.0] {
            assert!(
                (levels[ms(t)] - levels[ms(t + 300.0)]).abs() < 0.01,
                "Loop repeats at {t}"
            );
            assert!((levels[ms(t)] - levels[ms(t + 600.0)]).abs() < 0.01);
        }
        // Trigger: a short note's release waits for the decay's end.
        let mut params = cycle.to_vec();
        params.push((ENV_LOOP, 4.0));
        let levels = envelope_levels(
            &params,
            &[on(0, 60, 1.0), off(ms(50.0), 60)],
            ms(500.0),
            block,
        );
        assert!(
            (levels[ms(150.0)] - 0.75).abs() < 0.02,
            "still decaying: {}",
            levels[ms(150.0)]
        );
        assert!(levels[ms(190.0)] > 0.45);
        assert!(
            levels[ms(240.0)] < levels[ms(200.0)],
            "released at the decay's end"
        );
        assert_eq!(levels[ms(400.0)], 0.0);
        // Beat: at 120 bpm a 1/8 note is 250 ms; the note goes again from
        // its level at the attack's rate.
        let mut params = cycle.to_vec();
        params.extend([(ENV_LOOP, 2.0), (ENV_REPEAT, 3.0)]);
        let levels = envelope_levels(&params, &[on(0, 60, 1.0)], ms(500.0), block);
        assert!((levels[ms(240.0)] - 0.5).abs() < 0.01);
        assert!(
            (levels[ms(275.0)] - 0.75).abs() < 0.02,
            "{}",
            levels[ms(275.0)]
        );
        assert!(
            (levels[ms(300.0)] - 1.0).abs() < 0.01,
            "{}",
            levels[ms(300.0)]
        );
    }
}

// Spec: multisampler R25
#[test]
fn sync_repeats_land_on_the_song_grid() {
    for block in BLOCKS {
        let ms = |t: f64| (t * 48.0) as usize;
        // A note at 100 ms, with a 1/8 (250 ms) Sync repeat: it goes again
        // at 250 ms, not at 350.
        let params = [
            (ENV_INITIAL, 0.0),
            (ENV_ATTACK, 100.0),
            (ENV_DECAY, 100.0),
            (ENV_SUSTAIN, 50.0),
            (ENV_LOOP, 3.0),
            (ENV_REPEAT, 3.0),
        ];
        let levels = envelope_levels(&params, &[on(ms(100.0), 60, 1.0)], ms(600.0), block);
        // Decaying at 240 ms (0.8), the grid's 250 ms sends it up from 0.75
        // to its peak at 275, where a repeat counted from the note would
        // still be falling (0.69 at 262).
        assert!(
            (levels[ms(240.0)] - 0.8).abs() < 0.01,
            "{}",
            levels[ms(240.0)]
        );
        assert!(
            (levels[ms(262.0)] - 0.875).abs() < 0.03,
            "{}",
            levels[ms(262.0)]
        );
        assert!(
            (levels[ms(277.0)] - 1.0).abs() < 0.02,
            "{}",
            levels[ms(277.0)]
        );
        assert!(
            (levels[ms(330.0)] - 0.725).abs() < 0.03,
            "{}",
            levels[ms(330.0)]
        );
    }
}

// Spec: multisampler R26
#[test]
fn time_time_key_and_time_vel_scale_the_segments() {
    for block in BLOCKS {
        let ms = |t: f64| (t * 48.0) as usize;
        let attack = [(ENV_INITIAL, 0.0), (ENV_ATTACK, 100.0)];
        let at = |extra: &[(bl_model::ParamId, f64)], key: u8, velocity: f32, t: f64| {
            let mut params = attack.to_vec();
            params.extend_from_slice(extra);
            envelope_levels(&params, &[on(0, key, velocity)], ms(1200.0), block)[ms(t)]
        };
        assert!((at(&[], 60, 1.0, 50.0) - 0.5).abs() < 0.01);
        // Time +100 %: ten times as long, so 500 ms is halfway.
        assert!((at(&[(TIME, 100.0)], 60, 1.0, 500.0) - 0.5).abs() < 0.01);
        // Time < Key 100 %: an octave above C3 halves it.
        assert!((at(&[(TIME_KEY, 100.0)], 72, 1.0, 25.0) - 0.5).abs() < 0.01);
        assert!((at(&[(TIME_KEY, 100.0)], 48, 1.0, 100.0) - 0.5).abs() < 0.01);
        // Time < Vel 100 %: a full-velocity note's attack takes no time, a
        // half-velocity one's takes half.
        assert!(at(&[(ENV_TIME_VEL, 100.0)], 60, 1.0, 5.0) > 0.99);
        assert!((at(&[(ENV_TIME_VEL, 100.0)], 60, 0.5, 25.0) - 0.5).abs() < 0.02);
    }
}

// Spec: multisampler R27
#[test]
fn a_stolen_note_fades_out_and_retrigger_restarts_its_key() {
    for block in BLOCKS {
        // One voice, and the new note's attack as long as the steal's fade:
        // the two cross, so the level never dips or jumps.
        let mut rig = Rig::new(&[(VOICES, 1.0), (ENV_ATTACK, 5.0)], block);
        let (l, s) = flat(1, 1.0);
        rig.give(vec![(l, s)], false);
        let out = rig.render(48_000, &[on(0, 60, 1.0), on(24_000, 64, 1.0)]);
        let left = &out[0];
        assert!((left[20_000] - 1.0).abs() < 0.01, "one note before");
        assert!(
            left[24_000..24_240].iter().all(|s| (s - 1.0).abs() < 0.05),
            "the steal crosses"
        );
        assert!((left[30_000] - 1.0).abs() < 0.01, "one note after");
        assert!(biggest_step(&left[1..]) < 0.05);

        // A note's layers count as one voice.
        let mut rig = Rig::new(&[(VOICES, 2.0)], block);
        let (a, sa) = flat(1, 1.0);
        let (b, sb) = flat(2, 1.0);
        rig.give(vec![(a, sa), (b, sb)], false);
        let out = rig.render(
            12_000,
            &[on(0, 60, 1.0), on(2000, 62, 1.0), on(6000, 64, 1.0)],
        );
        assert!((out[0][1000] - 2.0).abs() < 1e-4, "one note, two layers");
        assert!((out[0][4000] - 4.0).abs() < 1e-4, "two notes, four voices");
        assert!(
            (out[0][9000] - 4.0).abs() < 1e-4,
            "the third steals the first note"
        );

        // Without Retrigger a key played twice sounds twice; with it, once.
        for (retrigger, expected) in [(0.0, 2.0), (1.0, 1.0)] {
            let mut rig = Rig::new(&[(RETRIGGER, retrigger), (ENV_ATTACK, 5.0)], block);
            let (l, s) = flat(1, 1.0);
            rig.give(vec![(l, s)], false);
            let out = rig.render(8000, &[on(0, 60, 1.0), on(3000, 60, 1.0)]);
            assert!(
                (out[0][6000] - expected).abs() < 0.02,
                "retrigger {retrigger}"
            );
        }
    }
}

// Spec: multisampler R28
#[test]
fn volume_velocity_pan_and_random_pan_place_the_note() {
    for block in BLOCKS {
        let play = |params: &[(bl_model::ParamId, f64)], velocity: f32| {
            let mut rig = Rig::new(params, block);
            let (l, s) = flat(1, 1.0);
            rig.give(vec![(l, s)], false);
            rig.render(500, &[on(0, 60, velocity)])
        };
        let out = play(&[(VEL_VOL, 100.0)], 0.5);
        assert!(
            (out[0][100] - 0.5).abs() < 0.01,
            "Vel > Vol follows velocity"
        );
        let out = play(&[(VEL_VOL, 0.0)], 0.5);
        assert!((out[0][100] - 1.0).abs() < 1e-5, "and at 0 it doesn't");
        let out = play(&[(VOLUME, db_gain(-6.0))], 1.0);
        assert!((out[0][100] - 0.501).abs() < 0.01);
        let out = play(&[(VOLUME, 0.0)], 1.0);
        assert!(out.iter().flatten().all(|s| *s == 0.0), "−∞ dB is silent");
        let out = play(&[(PAN, -50.0)], 1.0);
        assert!(out[1][100].abs() < 1e-6 && out[0][100] > 0.99, "hard left");
        let out = play(&[(PAN, 50.0)], 1.0);
        assert!(out[0][100].abs() < 1e-6 && out[1][100] > 0.99, "hard right");
        // Pan < Rnd moves each note's place, the same way every render.
        let pans = |block| {
            let mut rig = Rig::new(&[(PAN_RND, 100.0), (ENV_RELEASE, 0.0)], block);
            let (l, s) = flat(1, 1.0);
            rig.give(vec![(l, s)], false);
            let mut notes = Vec::new();
            for n in 0..12 {
                notes.push(on(n * 500, 60, 1.0));
                notes.push(off(n * 500 + 100, 60));
            }
            let out = rig.render(12 * 500, &notes);
            (0..12)
                .map(|n| out[0][n * 500 + 50] - out[1][n * 500 + 50])
                .collect::<Vec<f32>>()
        };
        let first = pans(block);
        assert_eq!(first, pans(block), "seeded: a render repeats");
        assert!(
            first.iter().any(|d| d.abs() > 0.1),
            "the notes move: {first:?}"
        );
        assert!(first.windows(2).any(|w| (w[0] - w[1]).abs() > 0.05));
    }
}

/// A gain in decibels as the linear value a gain parameter holds.
fn db_gain(db: f64) -> f64 {
    bl_model::param::db_to_gain(db)
}

// Spec: multisampler R31
#[test]
fn a_layer_edit_keeps_sounding_notes_going() {
    let mut rig = Rig::new(&[], 512);
    let (a, sa) = flat(1, 1.0);
    let (b, sb) = flat(2, 2.0);
    rig.give(
        vec![(a.clone(), Arc::clone(&sa)), (b.clone(), Arc::clone(&sb))],
        false,
    );
    let first = rig.render(2000, &[on(0, 60, 1.0)]);
    assert!((first[0][1000] - 3.0).abs() < 1e-4);
    // The list is reordered and the second layer's zone narrowed: the note
    // already sounding goes on through both, the next one takes the zones.
    let mut narrow = b.clone();
    narrow.key = Zone::new(0, 10);
    rig.give(vec![(narrow, sb), (a, sa)], false);
    let second = rig.render(2000, &[]);
    assert!((second[0][1000] - 3.0).abs() < 1e-4, "{}", second[0][1000]);
}

/// A mono 16-bit WAV of `seconds` of a ramp that wraps, at 48 kHz, in a
/// folder of its own.
fn long_file(name: &str, seconds: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-multisampler-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let samples: Vec<f32> = (0..seconds * 48_000)
        .map(|i| ((i % 20_000) as f32 / 20_000.0 - 0.5) * 0.8)
        .collect();
    let path = dir.join("Long.wav");
    std::fs::write(
        &path,
        bl_media::wav_bytes(
            &samples,
            1,
            48_000,
            bl_media::BitDepth::Int16,
            bl_media::Dither::None,
        )
        .unwrap(),
    )
    .unwrap();
    path
}

// Spec: multisampler R23
#[test]
fn a_long_layer_plays_from_its_held_start_while_the_rest_is_read() {
    use std::sync::atomic::Ordering::Relaxed;
    let path = long_file("progressive", 20);
    let whole = bl_media::decode_file(&path).unwrap();
    let frames = whole.frames();
    for block in [100, 4096] {
        let decoder = bl_media::Decoder::open(&path).unwrap();
        let (sample, fill) = bl_media::ProgressiveSample::start(decoder, 10 * 48_000).unwrap();
        assert_eq!(
            sample.ready(),
            10 * 48_000,
            "the start is held before it plays"
        );
        let mut layer = Layer::new(
            LayerId(1),
            "Long",
            ContentHash([9; 32]),
            frames as u64,
            48_000,
            1,
        );
        layer.root_key = 60;
        let mut rig = Rig::new(&[(ENV_RELEASE, 0.0)], block);
        let underruns = rig.send(
            vec![LayerAudio {
                layer,
                sample: Some(LayerSample::Progressive(Arc::clone(&sample))),
            }],
            false,
        );
        // The first half plays from the held start while nothing reads the
        // rest.
        let first = rig.render(5 * 48_000, &[on(0, 60, 1.0)]);
        // Then the rest is read, in the background, and the note plays on.
        fill.spawn().unwrap();
        let waited = std::time::Instant::now();
        while sample.ready() < frames {
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(20),
                "the read stalled"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let rest = rig.render(15 * 48_000, &[]);
        let played: Vec<f32> = first[0].iter().chain(&rest[0]).copied().collect();
        let expected = whole.channel(0).unwrap();
        assert!(
            played
                .iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "block {block}: it plays what decoding the whole file gives"
        );
        assert_eq!(underruns.load(Relaxed), 0, "never waited for the disk");
    }
}

// Spec: multisampler R23
#[test]
fn a_frame_the_disk_hasnt_delivered_plays_as_silence_and_counts() {
    use std::sync::atomic::Ordering::Relaxed;
    let path = long_file("behind", 14);
    let decoder = bl_media::Decoder::open(&path).unwrap();
    // Nothing reads past the held start.
    let (sample, _fill) = bl_media::ProgressiveSample::start(decoder, 10 * 48_000).unwrap();
    let frames = sample.frames();
    let layer = Layer::new(
        LayerId(1),
        "Long",
        ContentHash([9; 32]),
        frames as u64,
        48_000,
        1,
    );
    let mut rig = Rig::new(&[(ENV_RELEASE, 0.0)], 512);
    let underruns = rig.send(
        vec![LayerAudio {
            layer,
            sample: Some(LayerSample::Progressive(sample)),
        }],
        false,
    );
    let out = rig.render(12 * 48_000, &[on(0, 60, 1.0)]);
    assert!(
        out[0][9 * 48_000..10 * 48_000].iter().any(|s| *s != 0.0),
        "the held start plays"
    );
    assert!(
        out[0][10 * 48_000 + 10..].iter().all(|s| *s == 0.0),
        "then silence"
    );
    assert!(underruns.load(Relaxed) > 0, "counted");
}

// ----- WP-5.14: the Pitch/Osc, Filter/Global, Modulation and MIDI tabs ----------

/// A layer of a looping sine of `hz`, a whole number of cycles long.
fn tone(id: u32, hz: f64) -> (Layer, Arc<bl_media::SampleData>) {
    let sine = sample(4800, |i| {
        (core::f64::consts::TAU * hz * i as f64 / HZ).sin() as f32
    });
    let mut l = layer(id, &sine);
    l.sustain = SustainMode::Loop;
    (l, sine)
}

/// A rig playing one tone.
fn tone_rig(params: &[(bl_model::ParamId, f64)], hz: f64, block: usize) -> Rig {
    let mut rig = Rig::new(params, block);
    let (l, s) = tone(1, hz);
    rig.give(vec![(l, s)], false);
    rig
}

/// The amplitude of `hz` in a channel's `frames` frames from `from`.
fn amp(channel: &[f32], hz: f64, from: usize, frames: usize) -> f64 {
    fit(channel, from, frames, hz).0
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

// Spec: multisampler R32
#[test]
fn transpose_detune_and_spread_move_the_pitch() {
    for block in BLOCKS {
        let count = |params: &[(bl_model::ParamId, f64)]| {
            let mut rig = tone_rig(params, 1000.0, block);
            let out = rig.render(12_800, &[on(0, 60, 1.0)]);
            rising(&out[0][4800..12_800])
        };
        // 8,000 frames of 1 kHz hold 166.7 cycles.
        assert!((165..=168).contains(&count(&[])));
        assert!((332..=335).contains(&count(&[(TRANSPOSE, 12.0)])));
        assert!((82..=85).contains(&count(&[(TRANSPOSE, -12.0)])));
        assert!(
            (170..=173).contains(&count(&[(DETUNE, 50.0)])),
            "50 cents up"
        );
        // Spread: two voices 25 cents either side, half the level each, that
        // count as one voice.
        let mut rig = tone_rig(&[(SPREAD, 100.0), (VOICES, 1.0)], 1000.0, block);
        let out = rig.render(12_800, &[on(0, 60, 1.0)]);
        let up = 1000.0 * 2f64.powf(25.0 / 1200.0);
        let down = 1000.0 * 2f64.powf(-25.0 / 1200.0);
        for hz in [up, down] {
            let a = amp(&out[0], hz, 4800, 8000);
            assert!((a - 0.5).abs() < 0.08, "{hz}: {a}");
        }
    }
}

// Spec: multisampler R33
#[test]
fn key_zone_shift_chooses_other_layers_at_the_same_pitch() {
    for block in BLOCKS {
        let impulse = |at: usize| sample(4800, move |i| if i == at { 1.0 } else { 0.0 });
        let first = |out: &[Vec<f32>]| out[0].iter().position(|s| s.abs() > 0.9);
        let play = |shift: f64, key: u8| {
            let mut rig = Rig::new(&[(KEY_ZONE_SHIFT, shift), (ENV_RELEASE, 0.0)], block);
            let (a, b) = (impulse(300), impulse(1000));
            let mut low = layer(1, &a);
            low.key = Zone::new(0, 59);
            low.root_key = 48;
            let mut high = layer(2, &b);
            high.key = Zone::new(60, 127);
            high.root_key = 72;
            rig.give(vec![(low, a), (high, b)], false);
            first(&rig.render(6000, &[on(0, key, 1.0)]))
        };
        assert_eq!(
            play(0.0, 48),
            Some(300),
            "key 48 plays the low layer at its root"
        );
        // Shifted an octave up, key 48 chooses the high layer, and plays it
        // 24 semitones below its root: four times as long to the impulse.
        assert_eq!(play(12.0, 48), Some(4000));
        // And the pitch doesn't follow the shift for the same layer.
        assert_eq!(
            play(-12.0, 72),
            Some(1000),
            "key 60 chooses the high layer at its root"
        );
    }
}

/// The peak magnitude of a channel's `frames` frames from `from`.
fn peak(channel: &[f32], from: usize, frames: usize) -> f32 {
    channel[from..from + frames]
        .iter()
        .fold(0.0, |m, s| m.max(s.abs()))
}

// Spec: multisampler R34
#[test]
fn glide_and_portamento_slide_as_snapshots_do() {
    for block in BLOCKS {
        let play = |mode: f64| {
            let mut rig = tone_rig(&[(GLIDE, mode), (GLIDE_TIME, 100.0)], 1000.0, block);
            rig.render(14_000, &[on(0, 60, 1.0), on(6000, 72, 1.0)])
        };
        // Glide takes the sounding note over: it starts the slide at 1 kHz,
        // ends at 2 kHz, and never doubles.
        let out = play(1.0);
        let start = rising(&out[0][6000..6480]);
        assert!(
            (9..=12).contains(&start),
            "the slide starts at 1 kHz: {start}"
        );
        assert!(
            (82..=85).contains(&rising(&out[0][11_000..13_000])),
            "and ends at 2 kHz"
        );
        assert!(peak(&out[0], 6000, 8000) < 1.05, "one voice");
        // Portamento gives the new note a voice of its own: the first goes
        // on at 1 kHz while the second slides up to 2 kHz.
        let out = play(2.0);
        assert!(amp(&out[0], 1000.0, 11_000, 2000) > 0.8);
        assert!(amp(&out[0], 2000.0, 11_000, 2000) > 0.8);
        // A note already released is not taken over: the new one sounds.
        let mut rig = tone_rig(&[(GLIDE, 1.0), (ENV_RELEASE, 2000.0)], 1000.0, block);
        let out = rig.render(9000, &[on(0, 60, 1.0), off(100, 60), on(200, 72, 1.0)]);
        assert!(
            amp(&out[0], 2000.0, 4000, 2000) > 0.8,
            "the new note starts afresh"
        );
        let mut rig = tone_rig(&[(GLIDE, 0.0)], 1000.0, block);
        let out = rig.render(14_000, &[on(0, 60, 1.0), on(6000, 72, 1.0)]);
        assert!(amp(&out[0], 1000.0, 11_000, 2000) > 0.8, "Off: no slide");
        assert!(amp(&out[0], 2000.0, 6000, 480) > 0.8);
    }
}

// Spec: multisampler R35
#[test]
fn the_pitch_envelope_moves_the_pitch_by_its_amount() {
    for block in BLOCKS {
        let count = |params: &[(bl_model::ParamId, f64)], from: usize, frames: usize| {
            let mut all = vec![(PITCH_ENV_AMOUNT, 12.0), (ENV_RELEASE, 500.0)];
            all.extend_from_slice(params);
            let mut rig = tone_rig(&all, 1000.0, block);
            let out = rig.render(24_000, &[on(0, 60, 1.0), off(14_000, 60)]);
            rising(&out[0][from..from + frames])
        };
        // At full level the envelope moves the pitch by its Amount: 12
        // semitones is 2 kHz.
        assert!((332..=335).contains(&count(&[], 4800, 8000)));
        assert!((166..=168).contains(&count(&[(PITCH_ENV_AMOUNT, 0.0)], 4800, 8000)));
        assert!((248..=252).contains(&count(&[(PITCH_ENV_AMOUNT, 7.0)], 4800, 8000)));
        assert!(
            (41..=43).contains(&count(&[(PITCH_ENV_AMOUNT, -12.0)], 4800, 4000)),
            "down an octave"
        );
        // A slow attack takes the pitch up over its time, and the release
        // takes it back to End (0 %) over its own.
        let slow = [(PITCH_ENV.attack, 200.0), (PITCH_ENV.release, 100.0)];
        let early = count(&slow, 0, 480);
        assert!((9..=11).contains(&early), "starts at Initial: {early}");
        assert!(
            (165..=168).contains(&count(&slow, 10_000, 4000)),
            "reaches Peak"
        );
        assert!(
            (82..=85).contains(&count(&slow, 14_000 + 6000, 4000)),
            "back at End"
        );
    }
}

// Spec: multisampler R36
#[test]
fn fm_and_am_make_the_sidebands_their_laws_give() {
    for block in BLOCKS {
        // A 100 Hz oscillator on a 1 kHz carrier, in whole cycles of 100 Hz.
        let sidebands = |params: &[(bl_model::ParamId, f64)]| {
            let mut all = vec![
                (OSC_ON, 1.0),
                (OSC_FIXED, 1.0),
                (OSC_FREQ, 100.0),
                (OSC_MULTI, 2.0),
            ];
            all.extend_from_slice(params);
            let mut rig = tone_rig(&all, 1000.0, block);
            let out = rig.render(14_400, &[on(0, 60, 1.0)]);
            [900.0, 1000.0, 1100.0].map(|hz| amp(&out[0], hz, 4800, 9600))
        };
        // FM swings the read rate by 2^(2 · depth · osc): at 2 % the carrier's
        // frequency moves by 2.8 %, an index of 0.277, whose first sidebands
        // are J1/J0 = 0.140 of the carrier.
        let [lower, carrier, upper] = sidebands(&[(OSC_VOLUME, 2.0)]);
        assert!((carrier - 0.98).abs() < 0.03, "{carrier}");
        for side in [lower, upper] {
            assert!((side / carrier - 0.14).abs() < 0.02, "{}", side / carrier);
        }
        // AM multiplies the level by 1 − depth · (1 − osc) / 2: at full depth
        // a carrier of one half and sidebands of a quarter each.
        let [lower, carrier, upper] = sidebands(&[(OSC_MODE, 1.0), (OSC_VOLUME, 100.0)]);
        assert!((carrier - 0.5).abs() < 0.02, "{carrier}");
        assert!((lower - 0.25).abs() < 0.02 && (upper - 0.25).abs() < 0.02);
        // Off, or at no depth, the oscillator changes nothing.
        let [lower, carrier, _] = sidebands(&[(OSC_ON, 0.0), (OSC_VOLUME, 100.0)]);
        assert!(carrier > 0.99 && lower < 0.01);
        // Its envelope shapes the depth: a slow attack leaves the start clean.
        let mut rig = tone_rig(
            &[
                (OSC_ON, 1.0),
                (OSC_MODE, 1.0),
                (OSC_FIXED, 1.0),
                (OSC_VOLUME, 100.0),
                (OSC_ENV.attack, 400.0),
            ],
            1000.0,
            block,
        );
        let out = rig.render(24_000, &[on(0, 60, 1.0)]);
        assert!(amp(&out[0], 1000.0, 0, 480) > 0.95);
        assert!(amp(&out[0], 1000.0, 20_000, 2400) < 0.6);
    }
}

/// `input` through one voice of the kit's filter, as the multisampler's
/// filter runs it: the prototype the filter's types are held to.
fn kit_run(
    kind: FilterType,
    hz: f32,
    res: f32,
    drive: f32,
    character: f32,
    input: &[f32],
) -> Vec<f32> {
    let mut bank = FilterBank::new(48_000.0, 1, 1);
    bank.set_type(kind);
    let stride = bank.stride();
    bank.reset_voice(
        0,
        &VoiceParams {
            resonance: res,
            drive,
            saturation: 0.0,
            character,
            stereo_offset: 0.0,
            mix: 1.0,
            level: 1.0,
            morph_x: 0.0,
            morph_y: 0.0,
        },
    );
    let pitch = vec![pitch_of(hz, 440.0); input.len() * stride];
    let mut audio = vec![0.0; input.len() * stride];
    for (i, x) in input.iter().enumerate() {
        audio[i * stride] = *x;
    }
    bank.process_voices(1, &pitch, &mut audio);
    (0..input.len()).map(|i| audio[i * stride]).collect()
}

/// A tone of `hz` as the layers hold it, `frames` long.
fn tone_samples(hz: f64, frames: usize, level: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| (core::f64::consts::TAU * hz * (i % 4800) as f64 / HZ).sin() as f32 * level)
        .collect()
}

// Spec: multisampler R37
#[test]
fn the_filters_types_and_morph_match_their_prototypes() {
    let patch = |kind: f64, slope: f64, morph: f64| {
        vec![
            (FILTER_ON, 1.0),
            (FILTER_TYPE, kind),
            (FILTER_SLOPE, slope),
            (FILTER_FREQ, 1000.0),
            (FILTER_RES, 40.0),
            (FILTER_MORPH, morph),
            (FILTER_DRIVE, 12.0),
        ]
    };
    for block in [1, 512, 4096] {
        // Each type and slope, with Res and Drive, against the kit's filter.
        let mut cases = Vec::new();
        for (kind, types) in [
            (0.0, [FilterType::Lp12, FilterType::Lp24]),
            (1.0, [FilterType::Hp12, FilterType::Hp24]),
            (2.0, [FilterType::Bp12, FilterType::Bp24]),
            (3.0, [FilterType::Notch12, FilterType::Notch24]),
        ] {
            for (slope, prototype) in types.into_iter().enumerate() {
                // Drive acts on low-pass, high-pass and band-pass.
                let drive = if kind < 3.0 { 0.5 } else { 0.0 };
                cases.push((patch(kind, slope as f64, 0.0), prototype, drive));
            }
        }
        // Morph passes through low-pass, band-pass, high-pass, notch and
        // low-pass again, at 0, 25, 50, 75 and 100 %; Drive isn't heard.
        for (morph, prototype) in [
            (0.0, FilterType::Lp12),
            (25.0, FilterType::Bp12),
            (50.0, FilterType::Hp12),
            (75.0, FilterType::Notch12),
            (100.0, FilterType::Lp12),
        ] {
            cases.push((patch(4.0, 0.0, morph), prototype, 0.0));
        }
        for (params, prototype, drive) in cases {
            for hz in [250.0, 1000.0, 4000.0] {
                let mut rig = tone_rig(&params, hz, block);
                let out = rig.render(9600, &[on(0, 60, 1.0)]);
                let want = kit_run(
                    prototype,
                    1000.0,
                    0.4,
                    drive,
                    0.0,
                    &tone_samples(hz, 9600, 1.0),
                );
                let (got, want) = (amp(&out[0], hz, 4800, 4800), amp(&want, hz, 4800, 4800));
                assert!(
                    db(got / want).abs() < 0.2,
                    "{params:?} {prototype:?} at {hz}: {got} against {want}"
                );
            }
        }
    }
}

/// The low-pass patch the filter envelope and shaper tests run.
fn low_pass(freq: f64) -> Vec<(bl_model::ParamId, f64)> {
    vec![(FILTER_ON, 1.0), (FILTER_FREQ, freq)]
}

// Spec: multisampler R38
#[test]
fn the_filter_envelope_moves_the_cutoff_by_its_amount() {
    for block in [1, 512, 4096] {
        // The steady amplitude of a 1.5 kHz tone through the low-pass, and
        // the kit's low-pass at `cutoff` for it.
        let run = |extra: &[(bl_model::ParamId, f64)], from: usize, frames: usize| {
            let mut params = low_pass(1000.0);
            params.extend_from_slice(extra);
            let mut rig = tone_rig(&params, 1500.0, block);
            let out = rig.render(from + frames, &[on(0, 60, 1.0)]);
            amp(&out[0], 1500.0, from, frames)
        };
        let prototype = |cutoff: f32| {
            let out = kit_run(
                FilterType::Lp12,
                cutoff,
                0.0,
                0.0,
                0.0,
                &tone_samples(1500.0, 9600, 1.0),
            );
            amp(&out, 1500.0, 4800, 4800)
        };
        let close = |got: f64, want: f64| db(got / want).abs() < 0.2;
        // F. Env off, or its Amount at 0, leaves the cutoff.
        assert!(close(run(&[], 4800, 4800), prototype(1000.0)));
        assert!(
            close(run(&[(FENV_ON, 1.0)], 4800, 4800), prototype(1000.0)),
            "Amount 0 does nothing"
        );
        // At its full level the envelope moves it by Amount: an octave up.
        assert!(close(
            run(&[(FENV_ON, 1.0), (FENV_AMOUNT, 12.0)], 4800, 4800),
            prototype(2000.0)
        ));
        // A negative level moves it down.
        let down = [
            (FENV_ON, 1.0),
            (FENV_AMOUNT, 12.0),
            (FILTER_ENV.peak, -100.0),
            (FILTER_ENV.sustain, -100.0),
        ];
        assert!(close(run(&down, 4800, 4800), prototype(500.0)));
        // A slow attack starts at Initial, the cutoff as set.
        let slow = [
            (FENV_ON, 1.0),
            (FENV_AMOUNT, 12.0),
            (FILTER_ENV.attack, 2000.0),
        ];
        assert!(db(run(&slow, 4800, 480) / prototype(1000.0)).abs() < 1.0);
        assert!(run(&slow, 4800, 480) < run(&[(FENV_ON, 1.0), (FENV_AMOUNT, 12.0)], 4800, 480));
    }
}

/// The shaper's curve for `x` at `amount` (0 to 1): the spec's law.
fn curve(kind: usize, amount: f32, x: f32) -> f32 {
    let driven = x * 10f32.powf(amount * 24.0 / 20.0);
    match kind {
        0 => driven.tanh(),
        1 => driven.clamp(-1.0, 1.0),
        2 => driven.sin(),
        _ => (driven * 8.0).round().clamp(-8.0, 7.0) / 8.0,
    }
}

// Spec: multisampler R39
#[test]
fn the_shaper_curves_before_and_after_the_filter() {
    for block in [1, 512, 4096] {
        let (frames, level) = (9600, 0.5f32);
        let input = tone_samples(1000.0, frames, level);
        let play = |extra: &[(bl_model::ParamId, f64)]| {
            let mut rig = Rig::new(extra, block);
            let (mut l, s) = tone(1, 1000.0);
            l.volume = f64::from(level);
            rig.give(vec![(l, s)], false);
            rig.render(frames, &[on(0, 60, 1.0)])
        };
        let near = |got: &[f32], want: &[f32]| {
            got.iter()
                .zip(want)
                .skip(4800)
                .all(|(g, w)| (g - w).abs() < 2e-4)
        };
        // Each curve, at 25 % (6 dB of drive), is what the spec says.
        for kind in 0..4 {
            let out = play(&[
                (SHAPER_ON, 1.0),
                (SHAPER_TYPE, kind as f64),
                (SHAPER_AMOUNT, 25.0),
            ]);
            let want: Vec<f32> = input.iter().map(|x| curve(kind, 0.25, *x)).collect();
            assert!(near(&out[0], &want), "curve {kind}");
        }
        // At 0 % the shaper is bypassed.
        let out = play(&[(SHAPER_ON, 1.0), (SHAPER_AMOUNT, 0.0)]);
        assert!(near(&out[0], &input));
        // Before the filter (the default) the filter smooths the shaped
        // signal; after it, the filtered signal is shaped.
        let filter = [
            (FILTER_ON, 1.0),
            (FILTER_FREQ, 400.0),
            (SHAPER_ON, 1.0),
            (SHAPER_TYPE, 1.0),
            (SHAPER_AMOUNT, 60.0),
        ];
        let pre = play(&filter);
        let shaped: Vec<f32> = input.iter().map(|x| curve(1, 0.6, *x)).collect();
        let want = kit_run(FilterType::Lp12, 400.0, 0.0, 0.0, 0.0, &shaped);
        assert!(near(&pre[0], &want), "before the filter");
        let mut after = filter.to_vec();
        after.push((SHAPER_POST, 1.0));
        let post = play(&after);
        let filtered = kit_run(FilterType::Lp12, 400.0, 0.0, 0.0, 0.0, &input);
        let want: Vec<f32> = filtered.iter().map(|x| curve(1, 0.6, *x)).collect();
        assert!(near(&post[0], &want), "after the filter");
        assert!(!near(&pre[0], &post[0]), "and the two differ");
    }
}

/// A rig playing a constant 1.0, so the level of its output is the level
/// the modulation gives.
fn flat_rig(params: &[(bl_model::ParamId, f64)], block: usize) -> Rig {
    let mut rig = Rig::new(params, block);
    let (l, s) = flat(1, 1.0);
    rig.give(vec![(l, s)], false);
    rig
}

fn near(got: f32, want: f32) -> bool {
    (got - want).abs() < 0.01
}

// Spec: multisampler R40
#[test]
fn the_aux_envelope_reaches_both_destinations() {
    for block in BLOCKS {
        let route = |amount_a: f64, amount_b: f64| {
            vec![
                (AUX_ON, 1.0),
                (AUX_DEST_A, dest::VOLUME as f64),
                (AUX_AMOUNT_A, amount_a),
                (AUX_DEST_B, dest::PAN as f64),
                (AUX_AMOUNT_B, amount_b),
                (AUX_ENV.peak, 50.0),
                (AUX_ENV.sustain, 50.0),
            ]
        };
        // Half level, to Volume at 100 % and Pan at 50 %: 1 + 0.5 in level,
        // and a quarter of the way to the right, which takes the left to
        // half.
        let out = flat_rig(&route(100.0, 50.0), block).render(3000, &[on(0, 60, 1.0)]);
        assert!(
            near(out[0][2000], 0.75) && near(out[1][2000], 1.5),
            "{out:?}"
        );
        // A negative amount goes the other way.
        let out = flat_rig(&route(-100.0, -50.0), block).render(3000, &[on(0, 60, 1.0)]);
        assert!(
            near(out[0][2000], 0.5) && near(out[1][2000], 0.25),
            "{:?}",
            out[1][2000]
        );
        // Off, it does nothing.
        let mut off_params = route(100.0, 50.0);
        off_params[0].1 = 0.0;
        let out = flat_rig(&off_params, block).render(3000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][2000], 1.0) && near(out[1][2000], 1.0));
        // Its envelope shapes the send: a 100 ms attack climbs to the level.
        let mut slow = route(100.0, 0.0);
        slow.push((AUX_ENV.attack, 100.0));
        let out = flat_rig(&slow, block).render(6000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][0], 1.0) && near(out[0][2400], 1.25) && near(out[0][5500], 1.5));
    }
}

/// LFO 2 sent to Volume at 100 %: the level follows `1 + the LFO`.
fn lfo_two(extra: &[(bl_model::ParamId, f64)]) -> Vec<(bl_model::ParamId, f64)> {
    let ids = LFOS[1];
    let mut params = vec![
        (ids.on, 1.0),
        (ids.wave, 1.0),
        (ids.extra(LFO_DEST_A), dest::VOLUME as f64),
        (ids.extra(LFO_AMOUNT_A), 100.0),
    ];
    params.extend_from_slice(extra);
    params
}

// Spec: multisampler R41
#[test]
fn the_lfos_rates_sync_retrigger_and_key_tracking() {
    let ids = LFOS[1];
    for block in BLOCKS {
        // A square wave at 10 Hz, a period of 4,800 frames: up for the first
        // half, down for the second.
        let ten = lfo_two(&[(ids.rate, 10.0)]);
        let out = flat_rig(&ten, block).render(10_000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][1000], 2.0) && near(out[0][3600], 0.0) && near(out[0][6000], 2.0));
        // Synced to a quarter note at 120 bpm, a period of 24,000 frames.
        let synced = lfo_two(&[(ids.sync, 1.0), (ids.division, 4.0)]);
        let out = flat_rig(&synced, block).render(26_000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][6000], 2.0) && near(out[0][18_000], 0.0) && near(out[0][25_000], 2.0));
        // Retrig starts the wave at Offset on each note, 180° into its cycle;
        // without it a note takes up the free-running phase.
        let mut retrig = ten.clone();
        retrig.extend([(ids.retrig, 1.0), (ids.offset, 180.0)]);
        let out = flat_rig(&retrig, block).render(8000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][500], 0.0) && near(out[0][3000], 2.0));
        let notes = [
            on(0, 60, 1.0),
            off(1000, 60),
            on(3600, 60, 1.0),
            off(5000, 60),
        ];
        let mut free = ten.clone();
        free.push((ENV_RELEASE, 0.0));
        let out = flat_rig(&free, block).render(6000, &notes);
        assert!(
            near(out[0][500], 2.0) && near(out[0][3700], 0.0),
            "free-running: {}",
            out[0][3700]
        );
        free.extend([(ids.retrig, 1.0)]);
        let out = flat_rig(&free, block).render(6000, &notes);
        assert!(near(out[0][3700], 2.0), "retriggered");
        // Key speeds it up with pitch: at 100 % an octave above C3 runs
        // twice as fast.
        let mut key = ten.clone();
        key.push((ids.key, 100.0));
        let out = flat_rig(&key, block).render(3000, &[on(0, 72, 1.0)]);
        assert!(
            near(out[0][1000], 2.0) && near(out[0][1800], 0.0),
            "{}",
            out[0][1800]
        );
        let out = flat_rig(&key, block).render(3000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][1800], 2.0));
        // Attack brings it up to full intensity.
        let mut attack = ten.clone();
        attack.push((ids.attack, 1000.0));
        let out = flat_rig(&attack, block).render(48_000, &[on(0, 60, 1.0)]);
        assert!((out[0][1000] - 1.02).abs() < 0.02, "{}", out[0][1000]);
        assert!(out[0][46_000] > 1.9 || out[0][47_000] < 0.1);
    }
}

// Spec: multisampler R42
#[test]
fn lfo_one_moves_volume_pan_filter_and_pitch() {
    let one = LFOS[0];
    for block in BLOCKS {
        let square = |part: u32, amount: f64| {
            vec![
                (one.on, 1.0),
                (one.wave, 1.0),
                (one.rate, 10.0),
                (one.extra(part), amount),
            ]
        };
        // Vol at 100 %: the level swings between full and silence.
        let out = flat_rig(&square(LFO1_VOL, 100.0), block).render(5000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][1000], 1.0) && near(out[0][3600], 0.0));
        // Pan at 100 %: from hard right (the wave up) to hard left.
        let out = flat_rig(&square(LFO1_PAN, 100.0), block).render(5000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][1000], 0.0) && near(out[1][1000], 1.0));
        assert!(near(out[0][3600], 1.0) && near(out[1][3600], 0.0));
        // Pitch at 100 %: an octave up, then an octave down.
        let mut rig = tone_rig(&square(LFO1_PITCH, 100.0), 1000.0, block);
        let out = rig.render(5000, &[on(0, 60, 1.0)]);
        assert!((82..=85).contains(&rising(&out[0][200..2200])), "up");
        assert!((19..=22).contains(&rising(&out[0][2600..4600])), "down");
        // Filter at 12 semitones: the cutoff moves an octave either way.
        let mut params = low_pass(1000.0);
        params.extend(square(LFO1_FILTER, 12.0));
        let mut rig = tone_rig(&params, 1500.0, block);
        let out = rig.render(5000, &[on(0, 60, 1.0)]);
        let tone = tone_samples(1500.0, 9600, 1.0);
        let at = |cutoff: f32| {
            let want = kit_run(FilterType::Lp12, cutoff, 0.0, 0.0, 0.0, &tone);
            amp(&want, 1500.0, 4800, 4800)
        };
        assert!(
            db(amp(&out[0], 1500.0, 1400, 1000) / at(2000.0)).abs() < 1.0,
            "up"
        );
        assert!(
            db(amp(&out[0], 1500.0, 3800, 1000) / at(500.0)).abs() < 1.0,
            "down"
        );
    }
}

// Spec: multisampler R42
#[test]
fn phase_and_spin_split_the_channels() {
    let ids = LFOS[1];
    for block in BLOCKS {
        // Phase 180°: the right channel's wave runs half a cycle ahead.
        let phase = lfo_two(&[(ids.rate, 10.0), (ids.extra(LFO_PHASE), 180.0)]);
        let out = flat_rig(&phase, block).render(5000, &[on(0, 60, 1.0)]);
        assert!(near(out[0][1000], 2.0) && near(out[1][1000], 0.0));
        assert!(near(out[0][3600], 0.0) && near(out[1][3600], 2.0));
        // Spin 50 %: the right runs half as fast again, a period of 3,200.
        let spin = lfo_two(&[(ids.rate, 10.0), (ids.extra(LFO_SPIN), 50.0)]);
        let out = flat_rig(&spin, block).render(5000, &[on(0, 60, 1.0)]);
        assert!(
            near(out[0][1700], 2.0) && near(out[1][1700], 0.0),
            "{} {}",
            out[0][1700],
            out[1][1700]
        );
        assert!(near(out[0][500], 2.0) && near(out[1][500], 2.0));
    }
}

// Spec: multisampler R43
#[test]
fn each_midi_source_reaches_its_destinations() {
    for block in BLOCKS {
        // Each source sent to Volume at 100 % makes the level 1 + its value;
        // the messages arrive before the note, except Release Velocity,
        // which waits for its end, and Aftertouch, which a pressure moves
        // while it sounds.
        let send = |source: usize, a: f64, b: f64| {
            vec![
                (midi_param(source, 0), dest::VOLUME as f64),
                (midi_param(source, 2), a),
                (midi_param(source, 1), dest::PAN as f64),
                (midi_param(source, 3), b),
            ]
        };
        let control =
            |controller: u8, value: f32| (0, NoteKind::Control { controller, value }, 0, 0.0);
        let cases: [(usize, Vec<Played>, usize, f32); 7] = [
            (0, vec![on(100, 76, 1.0)], 3000, 1.25),
            (1, vec![on(100, 60, 0.5)], 3000, 1.5),
            (
                3,
                vec![
                    on(100, 60, 1.0),
                    (500, NoteKind::ChannelPressure { value: 0.5 }, 0, 0.0),
                ],
                3000,
                1.5,
            ),
            (4, vec![control(1, 0.5), on(100, 60, 1.0)], 3000, 1.5),
            (5, vec![control(4, 0.25), on(100, 60, 1.0)], 3000, 1.25),
            (
                6,
                vec![
                    (0, NoteKind::PitchBend { value: 0.5 }, 0, 0.0),
                    on(100, 60, 1.0),
                ],
                3000,
                1.5,
            ),
            (
                2,
                vec![on(100, 60, 1.0), (1000, NoteKind::Off, 60, 0.8)],
                3000,
                1.8,
            ),
        ];
        for (source, notes, at, want) in cases {
            let mut params = send(source, 100.0, 0.0);
            params.push((ENV_RELEASE, 20_000.0));
            let with = flat_rig(&params, block).render(at + 1, &notes);
            let mut params = send(source, 0.0, 0.0);
            params.push((ENV_RELEASE, 20_000.0));
            let without = flat_rig(&params, block).render(at + 1, &notes);
            let ratio = with[0][at] / without[0][at];
            assert!(
                (ratio - want).abs() < 0.01,
                "{}: {ratio}",
                MIDI_SOURCES[source]
            );
        }
        // The second destination: Velocity to Pan at 50 % at velocity 1 is
        // half the pan's span, which takes it from the middle to the right.
        let out = flat_rig(&send(1, 0.0, 50.0), block).render(500, &[on(0, 60, 1.0)]);
        assert!(near(out[0][300], 0.0) && near(out[1][300], 1.0));
    }
}

// Spec: multisampler R44
#[test]
fn a_full_bend_moves_the_pitch_by_the_range() {
    for block in BLOCKS {
        let count = |range: Option<f64>, bend: f32| {
            let params: Vec<_> = range.map(|r| (BEND_RANGE, r)).into_iter().collect();
            let mut rig = tone_rig(&params, 1000.0, block);
            let out = rig.render(
                12_800,
                &[
                    (0, NoteKind::PitchBend { value: bend }, 0, 0.0),
                    on(100, 60, 1.0),
                ],
            );
            rising(&out[0][4800..12_800])
        };
        // 8,000 frames of 1 kHz hold 166.7 cycles.
        assert!((165..=168).contains(&count(Some(5.0), 0.0)), "no bend");
        assert!(
            (221..=224).contains(&count(Some(5.0), 1.0)),
            "5 semitones up: 222.4"
        );
        assert!(
            (124..=126).contains(&count(Some(5.0), -1.0)),
            "5 semitones down: 124.9"
        );
        assert!(
            (187..=188).contains(&count(None, 1.0)),
            "the range starts at 2: 187.1"
        );
        assert!(
            (165..=168).contains(&count(Some(0.0), 1.0)),
            "range 0: no movement"
        );
    }
}

// Spec: multisampler R45
#[test]
fn routings_scale_by_their_spans_add_and_clamp() {
    for block in BLOCKS {
        let wheel = |value: f32| {
            (
                0,
                NoteKind::Control {
                    controller: 1,
                    value,
                },
                0,
                0.0,
            )
        };
        let foot = |value: f32| {
            (
                0,
                NoteKind::Control {
                    controller: 4,
                    value,
                },
                0,
                0.0,
            )
        };
        let to = |source: usize, dest: usize, amount: f64| {
            vec![
                (midi_param(source, 0), dest as f64),
                (midi_param(source, 2), amount),
            ]
        };
        // Routings to one destination add: 0.5 and 0.25 make level 1.75; and
        // the sum never takes the level below silence.
        let mut sum = to(4, dest::VOLUME, 50.0);
        sum.extend(to(5, dest::VOLUME, 25.0));
        let out = flat_rig(&sum, block).render(500, &[wheel(1.0), foot(1.0), on(0, 60, 1.0)]);
        assert!(near(out[0][300], 1.75));
        let mut under = to(4, dest::VOLUME, -100.0);
        under.extend(to(5, dest::VOLUME, -100.0));
        let out = flat_rig(&under, block).render(500, &[wheel(1.0), foot(1.0), on(0, 60, 1.0)]);
        assert!(
            near(out[0][300], 0.0) && near(out[1][300], 0.0),
            "clamped at silence"
        );
        // Pan moves across the whole width: half the span is hard right.
        let out =
            flat_rig(&to(4, dest::PAN, 100.0), block).render(500, &[wheel(0.5), on(0, 60, 1.0)]);
        assert!(near(out[0][300], 0.0) && near(out[1][300], 1.0));
        // Pitch by up to 24 semitones: Velocity at 50 % is 12.
        let mut rig = tone_rig(&to(1, dest::PITCH, 50.0), 1000.0, block);
        let out = rig.render(12_800, &[on(0, 60, 1.0)]);
        assert!((332..=335).contains(&rising(&out[0][4800..12_800])));
        // Filter Freq by up to 8 octaves: Velocity at 25 % is two, 1 kHz to
        // 4 kHz.
        let mut params = low_pass(1000.0);
        params.extend(to(1, dest::FILTER_FREQ, 25.0));
        let mut rig = tone_rig(&params, 3000.0, block);
        let out = rig.render(9600, &[on(0, 60, 1.0)]);
        let want = kit_run(
            FilterType::Lp12,
            4000.0,
            0.0,
            0.0,
            0.0,
            &tone_samples(3000.0, 9600, 1.0),
        );
        assert!(db(amp(&out[0], 3000.0, 4800, 4800) / amp(&want, 3000.0, 4800, 4800)).abs() < 0.2);
        // Filter Env Amount is held to its range, 0 to 72: 12 less 72 is 0,
        // so the envelope leaves the cutoff at 1 kHz rather than pull it
        // down five octaves.
        let mut params = low_pass(1000.0);
        params.extend([(FENV_ON, 1.0), (FENV_AMOUNT, 12.0)]);
        params.extend(to(1, dest::FILTER_ENV, -100.0));
        let mut rig = tone_rig(&params, 1500.0, block);
        let out = rig.render(9600, &[on(0, 60, 1.0)]);
        let want = kit_run(
            FilterType::Lp12,
            1000.0,
            0.0,
            0.0,
            0.0,
            &tone_samples(1500.0, 9600, 1.0),
        );
        assert!(db(amp(&out[0], 1500.0, 4800, 4800) / amp(&want, 1500.0, 4800, 4800)).abs() < 0.2);
        // Sample Start by up to the playback span: half of it starts the
        // ramp halfway.
        let mut rig = Rig::new(&to(1, dest::SAMPLE_START, 50.0), block);
        let (l, s) = looped(SustainMode::Off, ReleaseMode::Off);
        rig.give(vec![(l, s)], false);
        let out = rig.render(100, &[on(0, 60, 1.0)]);
        assert!(near(out[0][0], 0.5), "{}", out[0][0]);
        // Loop Length by up to the same span: a shorter loop wraps to a
        // later frame.
        let wrapped = |amount: f64| {
            let mut rig = Rig::new(&to(1, dest::LOOP_LENGTH, amount), block);
            let (l, s) = looped(SustainMode::Loop, ReleaseMode::Off);
            rig.give(vec![(l, s)], false);
            rig.render(2100, &[on(0, 60, 1.0)])[0][2100 - 1]
        };
        assert!(
            (wrapped(0.0) - 1099.0 / 4800.0).abs() < 0.01,
            "{}",
            wrapped(0.0)
        );
        // 10 % of the span is 480 frames, so the loop 1,000 long is 520 and starts at 1,480.
        assert!(
            (wrapped(-10.0) - 1579.0 / 4800.0).abs() < 0.01,
            "{}",
            wrapped(-10.0)
        );
        // Sample Selector by up to 127: the wheel at full plays the layer
        // that takes 127.
        let mut rig = Rig::new(&to(4, dest::SELECTOR, 100.0), block);
        let (mut low, a) = flat(1, 1.0);
        low.select = Zone::new(0, 126);
        let (mut high, b) = flat(2, 3.0);
        high.select = Zone::new(127, 127);
        rig.give(vec![(low, a), (high, b)], false);
        let out = rig.render(500, &[wheel(1.0), on(0, 60, 1.0)]);
        assert!(near(out[0][300], 3.0));
    }
}

// Spec: multisampler R46
#[test]
fn every_destination_is_reachable() {
    // A patch with every stage on and every send listening, so Velocity
    // (1.0 at this note) sent to any destination changes the sound.
    let lfo_pitch = |n: usize| {
        let ids = LFOS[n];
        if n == 0 {
            vec![
                (ids.on, 1.0),
                (ids.extra(LFO1_PITCH), 40.0),
                (ids.extra(LFO1_VOL), 40.0),
            ]
        } else {
            vec![
                (ids.on, 1.0),
                (ids.extra(LFO_DEST_A), dest::PITCH as f64),
                (ids.extra(LFO_AMOUNT_A), 20.0),
            ]
        }
    };
    let mut patch = vec![
        (FILTER_ON, 1.0),
        (FILTER_RES, 30.0),
        (FENV_ON, 1.0),
        (SHAPER_ON, 1.0),
        (OSC_ON, 1.0),
        (OSC_MODE, 1.0),
        (OSC_VOLUME, 30.0),
        (ENV_ATTACK, 200.0),
    ];
    for n in 0..3 {
        patch.extend(lfo_pitch(n));
    }
    let play = |extra: &[(bl_model::ParamId, f64)], dest: usize| {
        let mut params = patch.clone();
        params.extend_from_slice(extra);
        // Morph's own destination needs the Morph type.
        if dest == dest::FILTER_MORPH {
            params.push((FILTER_TYPE, 4.0));
        }
        let mut rig = Rig::new(&params, 512);
        let (mut low, a) = tone(1, 1000.0);
        low.select = Zone::new(0, 126);
        low.loop_start = 1000;
        low.loop_end = 4000;
        let (mut high, b) = tone(2, 2000.0);
        high.select = Zone::new(127, 127);
        rig.give(vec![(low, a), (high, b)], false);
        rig.render(12_000, &[on(0, 60, 1.0)])
    };
    let base = play(&[], 0);
    for (dest, name) in DESTINATIONS.iter().enumerate().skip(1) {
        // Loop Length can only shorten the loop, and a whole turn of Morph is
        // back where it started.
        let amount = match dest {
            dest::LOOP_LENGTH => -30.0,
            dest::FILTER_MORPH => 50.0,
            _ => 100.0,
        };
        let routed = play(
            &[(midi_param(1, 0), dest as f64), (midi_param(1, 2), amount)],
            dest,
        );
        let reference = if dest == dest::FILTER_MORPH {
            play(&[], dest)
        } else {
            base.clone()
        };
        let change: f32 = routed[0]
            .iter()
            .zip(&reference[0])
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(change > 1.0, "{name} changes nothing: {change}");
    }
}

// Spec: multisampler R50
#[test]
fn a_one_shot_layer_plays_to_sample_end_whatever_the_note_does() {
    for block in BLOCKS {
        let play = |one_shot: bool| {
            let mut rig = Rig::new(&[(ENV_RELEASE, 0.0)], block);
            let (mut l, s) = looped(SustainMode::Off, ReleaseMode::Off);
            l.one_shot = one_shot;
            rig.give(vec![(l, s)], false);
            rig.render(6000, &[on(0, 60, 1.0), off(100, 60)])
        };
        let plain = play(false);
        assert!(plain[0][1000] == 0.0, "the note's end stops it");
        let shot = play(true);
        assert!(
            (shot[0][1000] - 1000.0 / 4800.0).abs() < 1e-3,
            "{}",
            shot[0][1000]
        );
        assert!(
            (shot[0][4700] - 4700.0 / 4800.0).abs() < 1e-3,
            "plays on to the end"
        );
        assert!(shot[0][4900] == 0.0, "and stops there");
    }
}

// Spec: multisampler R57
#[test]
fn an_edited_sample_reaches_the_node_off_the_audio_thread() {
    use bl_core::time::{TICKS_PER_QUARTER, Ticks};
    use bl_engine::{EngineConfig, Offline, TransportCommand};
    use bl_model::{
        Clip, ClipData, Command, Device, DeviceKind, Editor, MidiData, Note, SampleEdit,
        SampleEditKind, SampleRange, TrackId, TrackKind,
    };
    let hash = ContentHash([9u8; 32]);
    let ramp = Arc::new(
        bl_media::SampleData::new(
            48_000,
            vec![(0..96_000).map(|i| i as f32 / 96_000.0).collect()],
        )
        .unwrap(),
    );
    let mut media = bl_engine::MediaMap::new();
    media.insert(hash, Arc::clone(&ramp));
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
    let device_id = editor.new_id();
    let device = Device::new(
        device_id,
        "Sampler",
        DeviceKind::Native {
            device_type: bl_devices::multisampler::MULTISAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    editor
        .apply(Command::InsertLayers {
            device: device_id,
            index: None,
            layers: vec![Layer::new(LayerId(0), "Ramp", hash, 96_000, 48_000, 1)],
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    let note = Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 2), 100);
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 2),
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();
    let mut registry = Registry::empty();
    bl_devices::multisampler::register(&mut registry);
    let registry = registry.with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    assert!(offline.sync(editor.document()).is_empty());
    let render = |offline: &mut Offline| {
        offline.sender().transport(TransportCommand::Stop);
        offline.sender().transport(TransportCommand::Play);
        let mut out = Vec::new();
        offline.render(48_000, |block| out.extend_from_slice(block.channel(0)));
        out
    };
    let out = render(&mut offline);
    assert!(
        (out[1000] - 1000.0 / 96_000.0).abs() < 1e-3,
        "the source: {}",
        out[1000]
    );
    // A cut of the first second reaches the node as the layer's audio, and
    // the layer's markers follow: the note starts at the ramp's middle.
    editor
        .apply(Command::AddSampleEdit {
            device: device_id,
            layer: Some(LayerId(1)),
            edit: SampleEdit {
                kind: SampleEditKind::Cut,
                range: Some(SampleRange {
                    from: 0,
                    to: 48_000,
                }),
            },
        })
        .unwrap();
    assert!(offline.sync(editor.document()).is_empty());
    assert!(offline.missing_media().is_empty());
    let out = render(&mut offline);
    assert!(
        (out[1000] - (0.5 + 1000.0 / 96_000.0)).abs() < 1e-3,
        "edited: {}",
        out[1000]
    );
    assert_eq!(offline.heap_operations(), 0, "no allocation in a block");
}

// Spec: multisampler R54
#[test]
fn a_click_in_the_sample_tab_auditions_one_layer_from_its_point() {
    use bl_engine::Audition;
    let sample = Arc::new(
        bl_media::SampleData::new(
            48_000,
            vec![(0..48_000).map(|i| 0.25 + i as f32 / 96_000.0).collect()],
        )
        .unwrap(),
    );
    let mut rig = Rig::new(&[], 512);
    let mut quiet = Layer::new(LayerId(1), "Quiet", ContentHash([1; 32]), 48_000, 48_000, 1);
    // Its zones don't take any key: only an audition plays it.
    quiet.key = Zone {
        low: 100,
        high: 101,
        fade_low: 0,
        fade_high: 0,
    };
    rig.give(vec![(quiet, Arc::clone(&sample))], false);
    let press = |rig: &mut Rig, down| {
        let data = rig.collector.handle().data(Audition {
            layer: Some(LayerId(1)),
            from: 0.5,
            down,
        });
        rig.node.receive_data(data);
    };
    assert!(rig.render_with(512, &[], &[])[0].iter().all(|v| *v == 0.0));
    press(&mut rig, true);
    let played = rig.render_with(512, &[], &[]);
    assert!(
        (played[0][10] - (0.25 + 24_010.0 / 96_000.0)).abs() < 1e-3,
        "{}",
        played[0][10]
    );
    press(&mut rig, false);
    let faded = rig.render_with(512, &[], &[]);
    let silent = rig.render_with(512, &[], &[]);
    assert!(faded[0][0] > 0.0, "it faded rather than cut");
    assert!(silent[0].iter().all(|v| *v == 0.0), "and ended");
}
