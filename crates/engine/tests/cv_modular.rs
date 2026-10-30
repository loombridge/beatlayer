//! CV In and CV Utility (WP-7.18, `docs/specs/cv-tools.md` R26 to R29): a
//! voltage on an input that moves a target on another track, and two signals
//! added or multiplied into a voltage on a track and its CV output. The
//! signals are generated, so every number is the test's own. Every block
//! runs as a real-time section under the allocation trap.

use crate::common;
use bl_commands::Registry as Commands;
use bl_core::time::Ticks;
use bl_devices::cv_in::{self, cv_in_type};
use bl_devices::cv_utility::{self, cv_utility_type};
use bl_devices::testing::{UTILITY, UTILITY_GAIN, utility_type};
use bl_engine::{DeviceInput, TransportCommand};
use bl_model::modulator::{MODE, ModTarget};
use bl_model::{
    AutomationTarget, Channel, Command, Device, DeviceId, Output, ParamId, TrackId, TrackKind,
    registry,
};
use common::{Rig, dc, track};
use serde_json::json;

/// The interface's channels, both ways.
const CHANNELS: usize = 4;

fn rig(max_frames: usize) -> Rig {
    Rig::with_config_and_registry(
        max_frames,
        |config| {
            config.input_channels = CHANNELS;
            config.output_channels = CHANNELS;
        },
        |registry| {
            cv_in::register(registry);
            cv_utility::register(registry);
            bl_devices::testing::register(registry);
        },
    )
}

fn insert(rig: &mut Rig, track: TrackId, device: Device) -> DeviceId {
    let id = device.id;
    let index = rig.editor.document().track(track).unwrap().devices().len();
    rig.apply(Command::InsertDevice {
        track,
        index,
        device,
    });
    id
}

/// A ramp on the interface's first input from `from` volts to `to` across
/// `length` frames, held after: the block of `frames` that starts at `start`.
struct Ramp {
    from: f64,
    to: f64,
    length: usize,
    start: usize,
    frames: usize,
}

/// The input of one block: channel 1 a ramp, every other 0.
struct Block(Vec<Vec<f32>>);

impl Block {
    fn of(ramp: &Ramp) -> Self {
        let sample = |n: usize| {
            let share = (n as f64 / ramp.length as f64).min(1.0);
            ((ramp.from + (ramp.to - ramp.from) * share) / 10.0) as f32
        };
        let mut channels = vec![vec![0.0; ramp.frames]; CHANNELS];
        for (i, s) in channels[0].iter_mut().enumerate() {
            *s = sample(ramp.start + i);
        }
        Self(channels)
    }
}

impl DeviceInput for Block {
    fn channels(&self) -> usize {
        CHANNELS
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.0[index]
    }
}

/// What Main played over `total` frames, in blocks of `frames`, with a ramp
/// on the interface's first input.
fn render(
    rig: &mut Rig,
    total: usize,
    frames: usize,
    from: f64,
    to: f64,
    length: usize,
) -> Vec<f32> {
    let mut out = Vec::new();
    while out.len() < total {
        let input = Block::of(&Ramp {
            from,
            to,
            length,
            start: out.len(),
            frames,
        });
        rig.block_with_input(frames, &input);
        out.extend_from_slice(&rig.processor.output().channel(0)[..frames]);
    }
    out
}

/// A rig with a CV In on one track, its input channel 1, and a test utility
/// on another, whose gain the CV In maps in Remote Control from Min 0 % to
/// Max 100 % of its travel.
fn mapped(frames: usize, params: &[(ParamId, f64)]) -> Rig {
    mapped_on(frames, params, Some(1))
}

fn mapped_on(frames: usize, params: &[(ParamId, f64)], channel: Option<u16>) -> Rig {
    let mut rig = rig(frames);
    let source = track(&mut rig.editor, TrackKind::Audio, None);
    let target = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, target, 1.0);
    let utility = common::device(&mut rig.editor, target, UTILITY, &[]);
    let id = rig.editor.new_id();
    let mut device = cv_in_type().device(id);
    device.external.as_mut().unwrap().cv_inputs[0].channel = channel;
    device.params.insert(MODE, 1.0);
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    insert(&mut rig, source, device);
    rig.apply(Command::MapModulator {
        device: id,
        slot: 0,
        target: ModTarget::Device {
            device: utility,
            param: UTILITY_GAIN,
        },
    });
    rig
}

/// The gain at position `position` on the utility's gain travel.
fn gain_at(position: f64) -> f64 {
    utility_type()
        .description
        .param(UTILITY_GAIN)
        .unwrap()
        .clone()
        .denormalize(position)
}

/// What the signal of 1 comes out of Main as at a gain of 1.
fn factor() -> f64 {
    let mut rig = rig(64);
    let target = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, target, 1.0);
    common::device(&mut rig.editor, target, UTILITY, &[]);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    f64::from(rig.render(512, 64)[500])
}

const LENGTH: usize = 4096;

// Spec: cv-tools R26, R27
#[test]
fn a_ramp_from_minus_5_to_5_volts_moves_a_target_on_another_track_over_its_full_travel() {
    let factor = factor();
    let mut renders = Vec::new();
    for frames in [1, 64, 512, 4096] {
        let mut rig = mapped(frames, &[]);
        rig.sender.transport(TransportCommand::Play);
        renders.push(render(&mut rig, LENGTH, frames, -5.0, 5.0, LENGTH));
    }
    for render in &renders[1..] {
        assert_eq!(render, &renders[0], "the same at any block size");
    }
    // Each frame is the line between the grid values on either side, played
    // the 32 frames after the later one: the ramp's share, 32 frames late.
    let out = &renders[0];
    for frame in [64usize, 100, 1000, 2000, 3000, 4000, 4095] {
        let share = (frame - 32) as f64 / LENGTH as f64;
        let want = gain_at(share) * factor;
        let got = f64::from(out[frame]);
        assert!(
            (got - want).abs() < 2e-4,
            "frame {frame}: {got} against {want}"
        );
    }
    assert_eq!(out[0], 0.0, "-5 V is Low: the target at the bottom");
    assert!(out[LENGTH - 1] > out[LENGTH / 2], "and rising to the top");
}

// Spec: cv-tools R26
#[test]
fn low_above_high_inverts_and_smooth_slows_the_output() {
    let factor = factor();
    // Low +5 V and High −5 V: the ramp rising from −5 V to +5 V falls.
    let mut rig = mapped(64, &[(cv_in::LOW, 5.0), (cv_in::HIGH, -5.0)]);
    rig.sender.transport(TransportCommand::Play);
    let out = render(&mut rig, LENGTH, 64, -5.0, 5.0, LENGTH);
    for frame in [64usize, 1000, 3000, 4095] {
        let share = 1.0 - (frame - 32) as f64 / LENGTH as f64;
        let want = gain_at(share) * factor;
        assert!(
            (f64::from(out[frame]) - want).abs() < 2e-4,
            "frame {frame} inverted"
        );
    }
    // A Smooth of 500 ms holds the output well behind the unsmoothed ramp
    // through its first quarter.
    let mut plain = mapped(64, &[]);
    let mut slow = mapped(64, &[(cv_in::SMOOTH, 500.0)]);
    plain.sender.transport(TransportCommand::Play);
    slow.sender.transport(TransportCommand::Play);
    let plain = render(&mut plain, LENGTH, 64, -5.0, 5.0, LENGTH);
    let slow = render(&mut slow, LENGTH, 64, -5.0, 5.0, LENGTH);
    assert!(
        slow[1000] < plain[1000] * 0.9 && slow[LENGTH - 1] > slow[1000],
        "{} behind {}",
        slow[1000],
        plain[1000]
    );
}

// Spec: cv-tools R7, R26
#[test]
fn an_input_at_no_channel_reads_zero_volts() {
    let factor = factor();
    // No Input: the device reads 0 V, the middle of Low and High.
    let mut rig = mapped_on(64, &[], None);
    rig.sender.transport(TransportCommand::Play);
    let out = render(&mut rig, 512, 64, -5.0, 5.0, LENGTH);
    let want = gain_at(0.5) * factor;
    assert!((f64::from(out[500]) - want).abs() < 2e-4, "{}", out[500]);
}

/// Runs a registered command by ID.
fn run(rig: &mut Rig, id: &str, args: serde_json::Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(&mut rig.editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// A CV Utility on a track on Ext. Out 1/2 carrying a signal of `a`, its CV
/// output on interface channel 3, and a sidechain from a track carrying `b`.
fn utility(frames: usize, a: f64, b: Option<f64>, params: &[(ParamId, f64)]) -> (Rig, DeviceId) {
    let mut rig = rig(frames);
    let on = track(&mut rig.editor, TrackKind::Audio, None);
    rig.apply(Command::SetOutput {
        track: on,
        output: Output::External {
            channel: Channel::Pair(1),
        },
    });
    if a != 0.0 {
        dc(&mut rig.editor, on, a);
    }
    let id = rig.editor.new_id();
    let mut device = cv_utility_type().device(id);
    device.external.as_mut().unwrap().cv_outputs[0].channel = Some(3);
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    insert(&mut rig, on, device);
    if let Some(b) = b {
        let other = track(&mut rig.editor, TrackKind::Audio, None);
        dc(&mut rig.editor, other, b);
        run(
            &mut rig,
            "device.set_sidechain_input",
            json!({ "device": id, "input": { "type": "track", "track": other, "tap": "post_fx" } }),
        );
        run(
            &mut rig,
            "device.set_sidechain_enabled",
            json!({ "device": id, "enabled": true }),
        );
        rig.sync();
    }
    (rig, id)
}

/// The track's signal and the CV output's, after the controls settled.
fn settled(rig: &mut Rig) -> (f32, f32) {
    rig.sender.transport(TransportCommand::Play);
    for _ in 0..40 {
        rig.block(64);
    }
    let output = rig.processor.output();
    (output.ext_channel(0)[63], output.ext_channel(2)[63])
}

// Spec: cv-tools R28
#[test]
fn utility_gives_a_plus_b_and_a_times_b_with_gains_and_offset_clamped_on_both_outputs() {
    use cv_utility::{GAIN_A, GAIN_B, MODE as MODE_PARAM, OFFSET};
    let cases: [(f64, f64, &[(ParamId, f64)], f32); 6] = [
        (0.2, 0.1, &[], 0.3),
        (0.2, 0.1, &[(GAIN_A, 200.0), (GAIN_B, -50.0)], 0.35),
        // A volt of Offset is a tenth of full scale.
        (0.2, 0.1, &[(OFFSET, 1.0)], 0.4),
        (0.5, 0.4, &[(MODE_PARAM, 1.0)], 0.2),
        (
            0.5,
            0.4,
            &[(MODE_PARAM, 1.0), (GAIN_A, 200.0), (OFFSET, -3.0)],
            0.1,
        ),
        // Clamped to ±10 V.
        (0.8, 0.8, &[(GAIN_A, 200.0)], 1.0),
    ];
    for (a, b, params, want) in cases {
        let (mut rig, _) = utility(64, a, Some(b), params);
        let (track, cv) = settled(&mut rig);
        assert!(
            (track - want).abs() < 1e-4,
            "{a} and {b} {params:?}: track {track}"
        );
        assert!((cv - want).abs() < 1e-4, "{a} and {b} {params:?}: CV {cv}");
    }
    // Without a sidechain, B is nothing: Add leaves A, Multiply leaves 0.
    let (mut rig, _) = utility(64, 0.25, None, &[]);
    assert!((settled(&mut rig).1 - 0.25).abs() < 1e-4);
    let (mut rig, _) = utility(64, 0.25, None, &[(cv_utility::MODE, 1.0)]);
    assert!(settled(&mut rig).1.abs() < 1e-4);
}

// Spec: cv-tools R29
#[test]
fn an_automated_offset_makes_a_voltage_that_follows_the_envelope_frame_by_frame() {
    // A's Gain at 0 % and no sidechain, so the output is the Offset alone:
    // an envelope from the bottom of its travel (−10 V) to the top.
    let mut renders = Vec::new();
    for frames in [1usize, 64, 4096] {
        let (mut rig, id) = utility(frames, 0.0, None, &[(cv_utility::GAIN_A, 0.0)]);
        let track = rig.editor.document().tracks().next().unwrap().id();
        for (at, value) in [(Ticks::ZERO, -10.0), (Ticks::QUARTER, 10.0)] {
            rig.apply(Command::SetAutomationBreakpoint {
                track,
                target: AutomationTarget::Device {
                    device: id,
                    param: cv_utility::OFFSET,
                },
                at,
                value,
            });
        }
        rig.sender.transport(TransportCommand::Play);
        let mut cv = Vec::new();
        for _ in 0..(24_576 / frames) {
            rig.block(frames);
            cv.extend_from_slice(&rig.processor.output().ext_channel(2)[..frames]);
        }
        renders.push(cv);
    }
    for render in &renders[1..] {
        assert_eq!(render, &renders[0], "the same at any block size");
    }
    for frame in [0usize, 1, 100, 6000, 12_000, 23_999] {
        let want = (-1.0 + 2.0 * frame as f64 / 24_000.0) as f32;
        assert!(
            (renders[0][frame] - want).abs() < 1e-4,
            "frame {frame}: {} against {want}",
            renders[0][frame]
        );
    }
    assert_eq!(renders[0][24_000], 1.0);
}
