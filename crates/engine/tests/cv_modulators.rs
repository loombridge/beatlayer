//! CV Envelope Follower, CV LFO and CV Shaper (WP-7.18b,
//! `docs/specs/cv-tools.md` R32 to R36): the voltage each writes on its
//! interface channel, in a song, beside the audio they leave alone. The
//! signals are generated, so every number is the test's own. Every block
//! runs as a real-time section under the allocation trap.

use crate::common;
use bl_commands::Registry as Commands;
use bl_devices::cv_modulators::{
    self, MAX, MIN, cv_curve_type, cv_level_type, cv_wave_type,
};
use bl_devices::level_mod::{FALL, RISE};
use bl_devices::modulator::{PHASE, RATE_HZ, WAVE};
use bl_engine::TransportCommand;
use bl_engine::testing::{DELAY, DELAY_FRAMES};
use bl_model::modulator::shape_preset;
use bl_model::{Command, Device, DeviceId, ParamId, TrackId, TrackKind, registry};
use common::{Rig, dc, device, track};
use serde_json::json;

/// The interface's channels, out.
const CHANNELS: usize = 4;

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn rig(max_frames: usize) -> Rig {
    Rig::with_config_and_registry(
        max_frames,
        |config| {
            config.input_channels = CHANNELS;
            config.output_channels = CHANNELS;
        },
        cv_modulators::register,
    )
}

/// Puts `device` on `track` with its CV output on interface channel
/// `channel` (from 1) and `params` set.
fn put(
    rig: &mut Rig,
    track: TrackId,
    mut device: Device,
    channel: u16,
    params: &[(ParamId, f64)],
) -> DeviceId {
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    device.external.as_mut().unwrap().cv_outputs[0].channel = Some(channel);
    let (id, index) = (
        device.id,
        rig.editor.document().track(track).unwrap().devices().len(),
    );
    rig.apply(Command::InsertDevice {
        track,
        index,
        device,
    });
    id
}

/// Runs a registered command by ID.
fn run(rig: &mut Rig, id: &str, args: serde_json::Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(&mut rig.editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// What the interface's output channel `channel` (from 0, so channel 1)
/// and Main's left carried over `blocks` blocks of `frames` frames, the
/// song playing from its start.
fn play(rig: &mut Rig, frames: usize, blocks: usize, channel: usize) -> (Vec<f32>, Vec<f32>) {
    rig.sender.transport(TransportCommand::Play);
    let (mut cv, mut main) = (Vec::new(), Vec::new());
    for _ in 0..blocks {
        rig.block(frames);
        let output = rig.processor.output();
        if output.ext_channels() > channel {
            cv.extend_from_slice(&output.ext_channel(channel)[..frames]);
        } else {
            cv.extend(std::iter::repeat_n(0.0, frames));
        }
        main.extend_from_slice(&output.channel(0)[..frames]);
    }
    (cv, main)
}

/// Where `main` first differs from `untouched`, over what both have.
fn first_difference(main: &[f32], untouched: &[f32]) -> Option<(usize, f32, f32)> {
    main.iter()
        .zip(untouched)
        .position(|(a, b)| a != b)
        .map(|at| (at, main[at], untouched[at]))
}

/// A track carrying a signal of `value`.
fn carrier(rig: &mut Rig, value: f64) -> TrackId {
    let id = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, id, value);
    id
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// Spec: cv-tools R32, R33, R34, R35, R36
#[test]
fn each_device_writes_its_signal_as_volts_and_leaves_the_audio_alone() {
    // The same song without the device: what Main plays.
    let mut plain = rig(64);
    carrier(&mut plain, 0.25);
    plain.sync();
    let (_, untouched) = play(&mut plain, 64, 75, 0);

    // CV LFO: the rising ramp at 10 Hz from −5 V to +5 V.
    let mut rig = rig(64);
    let a = carrier(&mut rig, 0.25);
    let id = rig.editor.new_id();
    put(
        &mut rig,
        a,
        cv_wave_type().device(id),
        1,
        &[(WAVE, 1.0), (RATE_HZ, 10.0)],
    );
    let (cv, main) = play(&mut rig, 64, 75, 0);
    assert!(close(cv[0], -0.5), "−5 V at the start: {}", cv[0]);
    assert!(close(cv[2_400], 0.0), "0 V half way: {}", cv[2_400]);
    // The last frame is one frame short of the top of the ramp.
    assert!((cv[4_799] - 0.5).abs() < 1e-3, "+5 V at the end: {}", cv[4_799]);
    assert_eq!(first_difference(&main, &untouched), None, "the track's audio passes untouched");

    // CV Shaper: a rise, 0 V to 5 V, at 10 Hz.
    let mut rig = rig_with_shape();
    let (cv, main) = play(&mut rig, 64, 75, 0);
    assert!(cv[0] < 1e-3 && close(cv[2_400], 0.25), "{}", cv[2_400]);
    assert!((cv[4_799] - 0.5).abs() < 1e-3, "{}", cv[4_799]);
    assert_eq!(first_difference(&main, &untouched), None, "the track's audio passes untouched");

    // CV Envelope Follower: a signal of 0.5 through Rise 0 is 0.5, 2.5 V of
    // its 0 to 5 V.
    let (mut rig, _) = rig_with_level(0.25, &[(RISE, 0.0)]);
    let (cv, main) = play(&mut rig, 64, 20, 0);
    assert!(close(cv[1_000], 0.125), "{}", cv[1_000]);
    assert_eq!(first_difference(&main, &untouched), None, "the track's audio passes untouched");
}

fn rig_with_shape() -> Rig {
    let mut rig = rig(64);
    let a = carrier(&mut rig, 0.25);
    let id = rig.editor.new_id();
    put(&mut rig, a, cv_curve_type().device(id), 1, &[(RATE_HZ, 10.0)]);
    rig.apply(Command::SetShape {
        device: id,
        points: shape_preset(1).unwrap(),
    });
    rig
}

/// A CV Envelope Follower on a track carrying `value`, on channel 1.
fn rig_with_level(value: f64, params: &[(ParamId, f64)]) -> (Rig, DeviceId) {
    let mut rig = rig(64);
    let a = carrier(&mut rig, value);
    let id = rig.editor.new_id();
    put(&mut rig, a, cv_level_type().device(id), 1, params);
    (rig, id)
}

// Spec: cv-tools R34
#[test]
fn a_free_cv_lfo_starts_again_from_phase_when_the_transport_starts() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let id = rig.editor.new_id();
    put(
        &mut rig,
        a,
        cv_wave_type().device(id),
        1,
        &[(WAVE, 1.0), (RATE_HZ, 10.0), (PHASE, 25.0), (MIN, 0.0), (MAX, 10.0)],
    );
    // Stopped, a free cycle runs.
    for _ in 0..20 {
        rig.block(64);
    }
    let stopped = rig.processor.output().ext_channel(0)[63];
    assert!(stopped > 0.25 + 0.1, "it ran while stopped: {stopped}");
    // The transport starts: from Phase.
    let (cv, _) = play(&mut rig, 64, 2, 0);
    assert!(close(cv[0], 0.25), "from Phase: {}", cv[0]);
    assert!(close(cv[64], 0.25 + 64.0 / 4_800.0), "and on from there");
}

// Spec: cv-tools R36
#[test]
fn the_follower_follows_the_sidechain_mix() {
    // The track carries 0.2 and the sidechain's source 0.8.
    for (mix, want) in [(0.0, 0.2f32), (0.5, 0.5), (1.0, 0.8)] {
        let (mut rig, id) = rig_with_level(0.2, &[(RISE, 0.0), (FALL, 0.0), (MIN, 0.0), (MAX, 10.0)]);
        let other = carrier(&mut rig, 0.8);
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
        run(
            &mut rig,
            "device.set_sidechain_mix",
            json!({ "device": id, "mix": mix }),
        );
        rig.sync();
        let (cv, _) = play(&mut rig, 64, 40, 0);
        assert!(
            close(cv[2_559], want),
            "mix {mix}: {} isn't {want}",
            cv[2_559]
        );
    }
}

// Spec: cv-tools R5, R33
#[test]
fn a_square_edge_waits_for_the_tracks_compensation_into_main() {
    // A square at 100 Hz falls 240 frames in. Another track through a
    // device that adds 500 frames makes this one wait that long to line up at
    // Main, and so does its CV.
    for (compensation, falls) in [(true, 740usize), (false, 240)] {
        let mut rig = rig(64);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        let id = rig.editor.new_id();
        put(
            &mut rig,
            a,
            cv_wave_type().device(id),
            1,
            &[(WAVE, 4.0), (RATE_HZ, 10.0), (common_times_ten(), 1.0), (MIN, 0.0), (MAX, 10.0)],
        );
        let b = track(&mut rig.editor, TrackKind::Audio, None);
        device(&mut rig.editor, b, DELAY, &[(DELAY_FRAMES, 500.0)]);
        rig.apply(Command::SetDelayCompensation {
            enabled: compensation,
        });
        let (cv, _) = play(&mut rig, 64, 30, 0);
        let fall = cv.windows(2).position(|w| w[0] == 1.0 && w[1] == 0.0).unwrap() + 1;
        assert_eq!(fall, falls, "compensation {compensation}");
    }
}

fn common_times_ten() -> ParamId {
    bl_devices::modulator::TIMES_TEN
}

// Spec: cv-tools R8
#[test]
fn a_render_is_the_same_at_any_block_size() {
    let build = |frames: usize| {
        let mut rig = rig(frames);
        let a = carrier(&mut rig, 0.3);
        let id = rig.editor.new_id();
        put(
            &mut rig,
            a,
            cv_wave_type().device(id),
            1,
            &[(WAVE, 7.0), (RATE_HZ, 12.0), (bl_devices::modulator::JITTER, 50.0)],
        );
        let b = carrier(&mut rig, 0.6);
        let id = rig.editor.new_id();
        put(&mut rig, b, cv_curve_type().device(id), 2, &[(RATE_HZ, 7.0)]);
        rig.apply(Command::SetShape {
            device: id,
            points: shape_preset(6).unwrap(),
        });
        let c = carrier(&mut rig, 0.4);
        let id = rig.editor.new_id();
        put(&mut rig, c, cv_level_type().device(id), 3, &[(RISE, 10.0), (FALL, 80.0)]);
        rig
    };
    let channels = |frames: usize| -> Vec<Vec<f32>> {
        let mut rig = build(frames);
        rig.sender.transport(TransportCommand::Play);
        let mut out = vec![Vec::new(); 3];
        for _ in 0..(8_192 / frames) {
            rig.block(frames);
            let output = rig.processor.output();
            for (at, channel) in out.iter_mut().enumerate() {
                channel.extend_from_slice(&output.ext_channel(at)[..frames]);
            }
        }
        out
    };
    let first = channels(BLOCKS[0]);
    assert!(first.iter().all(|c| c.iter().any(|&s| s != 0.0)));
    for block in &BLOCKS[1..] {
        assert!(channels(*block) == first, "block size {block}");
    }
}
