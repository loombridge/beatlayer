//! CV as a signal in the engine (WP-7.17, `docs/specs/cv-tools.md` R1 to R9,
//! R10): a CV device writes its outputs on the interface's channels at full
//! value, adds to whatever else plays there, reads its inputs as the
//! interface delivered them, and an effect's outputs wait for the delay
//! compensation of its track's path into Main. Every block runs as a
//! real-time section under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::time::Ticks;
use bl_devices::cv_instrument::cv_instrument_type;
use bl_engine::testing::{CV_IMPULSE_AT, DELAY, DELAY_FRAMES, cv_probe_type};
use bl_engine::{DeviceInput, MediaMap, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, Channel, Clip, ClipData, Command, ContentHash, CvPort, Device, DeviceId,
    HardwareLatency, MidiData, Note, Output, TrackId, TrackKind,
};
use common::{Rig, dc, device, track};

/// The interface's channels, both ways.
const CHANNELS: usize = 4;

/// The frames the interface's round trip takes, and the instrument's own
/// Hardware Latency.
const ROUND_TRIP: u32 = 300;
const HARDWARE: u32 = 100;
const TOTAL: usize = (ROUND_TRIP + HARDWARE) as usize;

fn rig(max_frames: usize) -> Rig {
    Rig::with_config_and_registry(
        max_frames,
        |config| {
            config.input_channels = CHANNELS;
            config.output_channels = CHANNELS;
            config.round_trip_latency = ROUND_TRIP;
        },
        |registry| {
            bl_devices::cv_instrument::register(registry);
        },
    )
}

/// Puts Test CV on `track`, its outputs (Constant, Impulse, Through) and its
/// input (In) set to the interface channels given.
fn probe(rig: &mut Rig, track: TrackId, outputs: [Option<u16>; 3], input: Option<u16>) -> DeviceId {
    let id = rig.editor.new_id();
    let mut device = cv_probe_type().device(id);
    let external = device.external.as_mut().unwrap();
    for (port, channel) in external.cv_outputs.iter_mut().zip(outputs) {
        port.channel = channel;
    }
    external.cv_inputs[0].channel = input;
    let index = rig.editor.document().track(track).unwrap().devices().len();
    rig.apply(Command::InsertDevice {
        track,
        index,
        device,
    });
    id
}

/// The interface's input: every channel a ramp, or a constant, set by
/// `channel`, for the block that starts at `start`.
struct Input(Vec<Vec<f32>>);

impl Input {
    fn block(start: usize, frames: usize) -> Self {
        Self(
            (0..CHANNELS)
                .map(|c| {
                    (0..frames)
                        .map(|i| match c {
                            // Channel 2 holds a constant, 3 a ramp whose steps
                            // a float holds exactly.
                            1 => 0.25,
                            2 => (start + i) as f32 / 4096.0,
                            _ => 0.0,
                        })
                        .collect()
                })
                .collect(),
        )
    }
}

impl DeviceInput for Input {
    fn channels(&self) -> usize {
        self.0.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.0[index]
    }
}

/// Plays the song for `blocks` blocks of `frames` frames and returns each
/// device channel's samples.
fn play(rig: &mut Rig, frames: usize, blocks: usize) -> Vec<Vec<f32>> {
    rig.sender.transport(TransportCommand::Play);
    let mut played = vec![Vec::new(); CHANNELS];
    for block in 0..blocks {
        rig.block_with_input(frames, &Input::block(block * frames, frames));
        let output = rig.processor.output();
        for (c, samples) in played.iter_mut().enumerate() {
            if c < output.ext_channels() {
                samples.extend_from_slice(&output.ext_channel(c)[..frames]);
            } else {
                samples.extend(std::iter::repeat_n(0.0, frames));
            }
        }
    }
    played
}

/// The frames where the signal is not silent, with what it holds there.
fn heard(out: &[f32]) -> Vec<(usize, f32)> {
    out.iter()
        .enumerate()
        .filter(|(_, s)| s.abs() > 1e-9)
        .map(|(at, s)| (at, *s))
        .collect()
}

// Spec: cv-tools R1, R3
#[test]
fn a_cv_output_is_one_channel_at_full_value_and_adds_to_other_audio() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    probe(&mut rig, a, [Some(3), None, None], None);
    let out = play(&mut rig, 64, 2);
    assert!(
        out[2].iter().all(|&s| s == 0.25),
        "the value, not half of it, on channel 3"
    );
    assert!(
        out[0]
            .iter()
            .chain(&out[1])
            .chain(&out[3])
            .all(|&s| s == 0.0)
    );

    // A track on Ext. Out 3 adds to it on the same channel.
    let mut rig = self::rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    probe(&mut rig, a, [Some(3), None, None], None);
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, b, 0.5);
    rig.apply(Command::SetOutput {
        track: b,
        output: Output::External {
            channel: Channel::Mono(3),
        },
    });
    let out = play(&mut rig, 64, 2);
    assert!(out[2].iter().all(|&s| s == 0.75), "0.5 and 0.25 add");
}

// Spec: cv-tools R2
#[test]
fn a_constant_through_a_track_on_ext_out_arrives_unchanged() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, a, 0.3);
    rig.apply(Command::SetOutput {
        track: a,
        output: Output::External {
            channel: Channel::Pair(1),
        },
    });
    let out = play(&mut rig, 64, 3);
    let want = out[0][0];
    assert!((want - 0.3).abs() < 1e-6);
    assert!(
        out[0].iter().chain(&out[1]).all(|&s| s == want),
        "the same sample on every frame, from the first"
    );
}

// Spec: cv-tools R3
#[test]
fn a_channel_past_the_devices_width_carries_nothing() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    probe(&mut rig, a, [Some(9), Some(2), None], None);
    let out = play(&mut rig, 64, 1);
    assert!(out.iter().flatten().all(|&s| s == 0.0 || s == 0.0));
    // Channel 2 takes the impulse's output, which falls later in the song.
    assert!(out[1].iter().all(|&s| s == 0.0));
    assert_eq!(out[1].len(), 64);
}

// Spec: cv-tools R6
#[test]
fn a_cv_input_reads_a_generated_constant_and_ramp_exactly() {
    for input in [2u16, 3] {
        let mut rig = rig(64);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        // Through, set to channel 4, carries what In reads.
        probe(&mut rig, a, [None, None, Some(4)], Some(input));
        let out = play(&mut rig, 64, 4);
        let want: Vec<f32> = (0..256)
            .map(|i| if input == 2 { 0.25 } else { i as f32 / 4096.0 })
            .collect();
        assert_eq!(out[3], want, "input {input}");
    }
    // No Input reads 0 V; so does a channel the interface lacks.
    for input in [None, Some(9)] {
        let mut rig = rig(64);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        probe(&mut rig, a, [None, None, Some(4)], input);
        let out = play(&mut rig, 64, 2);
        assert!(out[3].iter().all(|&s| s == 0.0), "{input:?}");
    }
}

// Spec: cv-tools R18
#[test]
fn every_output_goes_to_zero_volts_on_switch_off_and_removal() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let id = probe(&mut rig, a, [Some(1), None, None], None);
    assert!(play(&mut rig, 64, 2)[0].iter().all(|&s| s == 0.25));
    rig.apply(Command::SetDeviceEnabled {
        device: id,
        enabled: false,
    });
    rig.block(64);
    assert!(
        play(&mut rig, 64, 1).iter().flatten().all(|&s| s == 0.0),
        "switched off"
    );
    rig.apply(Command::SetDeviceEnabled {
        device: id,
        enabled: true,
    });
    rig.block(64);
    assert!(play(&mut rig, 64, 1)[0].iter().all(|&s| s == 0.25));
    rig.apply(Command::RemoveDevice { device: id });
    rig.block(64);
    assert!(
        play(&mut rig, 64, 1).iter().flatten().all(|&s| s == 0.0),
        "removed"
    );
}

// Spec: cv-tools R18. A CV device inside a rack that is switched off sends
// 0 V too.
#[test]
fn a_cv_device_in_a_switched_off_rack_sends_zero_volts() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let (rack, chains) = common::rack(&mut rig.editor, a, bl_model::RackKind::AudioEffect, 1);
    let id = rig.editor.new_id();
    let mut device = cv_probe_type().device(id);
    device.external.as_mut().unwrap().cv_outputs[0].channel = Some(1);
    rig.apply(Command::InsertChainDevice {
        chain: chains[0],
        index: 0,
        device,
    });
    assert!(play(&mut rig, 64, 2)[0].iter().all(|&s| s == 0.25));
    rig.apply(Command::SetDeviceEnabled {
        device: rack,
        enabled: false,
    });
    rig.block(64);
    assert!(
        play(&mut rig, 64, 1)[0].iter().all(|&s| s == 0.0),
        "the rack is off"
    );
}

// Spec: cv-tools R8
#[test]
fn a_render_is_the_same_at_any_block_size() {
    let mut renders = Vec::new();
    for frames in [1, 64, 512, 4096] {
        let mut rig = rig(4096);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        probe(&mut rig, a, [Some(1), Some(2), Some(3)], Some(3));
        let out = play(&mut rig, frames, 4096 / frames);
        renders.push(out);
    }
    assert_eq!(heard(&renders[0][1]), vec![(CV_IMPULSE_AT as usize, 1.0)]);
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}

// Spec: cv-tools R5
#[test]
fn an_effects_cv_waits_for_its_tracks_compensation_into_main() {
    // Another track through a device that adds 500 frames makes this one wait
    // that long to line up at Main, and so does its CV.
    let latency = 500usize;
    for (compensation, at) in [(true, CV_IMPULSE_AT as usize + latency), (false, 1000)] {
        let mut rig = rig(64);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        probe(&mut rig, a, [None, Some(1), None], None);
        let b = track(&mut rig.editor, TrackKind::Audio, None);
        device(&mut rig.editor, b, DELAY, &[(DELAY_FRAMES, latency as f64)]);
        rig.apply(Command::SetDelayCompensation {
            enabled: compensation,
        });
        let out = play(&mut rig, 64, 30);
        assert_eq!(
            heard(&out[0]),
            vec![(at, 1.0)],
            "compensation {compensation}"
        );
    }
}

fn hash(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

// Spec: cv-tools R5, R10
#[test]
fn the_instruments_pitch_leaves_at_its_place_and_its_return_lines_up_at_main() {
    // A cable from the interface's outputs to its inputs, `TOTAL` frames long,
    // as long as the instrument's latency.
    struct Loopback(Vec<Vec<f32>>, Vec<Vec<f32>>);
    impl DeviceInput for Loopback {
        fn channels(&self) -> usize {
            self.1.len()
        }
        fn channel(&self, index: usize) -> &[f32] {
            &self.1[index]
        }
    }
    let mut impulse = vec![0.0f32; 256];
    impulse[0] = 1.0;
    let mut media = MediaMap::new();
    media.insert(
        hash(1),
        Arc::new(SampleData::new(48_000, vec![impulse.clone(), impulse]).unwrap()),
    );
    let mut rig = Rig::with_config_and_registry(
        64,
        |config| {
            config.input_channels = CHANNELS;
            config.output_channels = CHANNELS;
            config.round_trip_latency = ROUND_TRIP;
        },
        |registry| {
            bl_devices::cv_instrument::register(registry);
            registry.set_media(Arc::new(media));
        },
    );
    // An audio track playing an impulse at the start of the song.
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    let id = rig.editor.new_id();
    rig.apply(Command::CreateClip {
        track: audio,
        clip: Clip::new(
            id,
            Ticks::ZERO,
            Ticks::WHOLE * 4,
            ClipData::Audio(AudioData::new(hash(1))),
        ),
    });
    // A MIDI track playing key 72 from the start through CV Instrument, whose
    // Pitch is on channel 1 and which hears channel 1.
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let id = rig.editor.new_id();
    let mut instrument: Device = cv_instrument_type().device(id);
    let external = instrument.external.as_mut().unwrap();
    external.cv_outputs[0] = CvPort {
        name: "Pitch".to_owned(),
        channel: Some(1),
    };
    external.audio_from = Channel::Mono(1);
    external.hardware_latency = HardwareLatency::Samples { value: HARDWARE };
    rig.apply(Command::InsertDevice {
        track: midi,
        index: 0,
        device: instrument,
    });
    let id = rig.editor.new_id();
    rig.apply(Command::CreateClip {
        track: midi,
        clip: Clip::new(
            id,
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Midi(MidiData::new(vec![Note::new(
                72,
                Ticks::ZERO,
                Ticks::WHOLE,
                100,
            )])),
        ),
    });
    rig.sender.transport(TransportCommand::Play);
    let (mut cable, mut main, mut pitch) = (
        Loopback(vec![Vec::new(); CHANNELS], vec![Vec::new(); CHANNELS]),
        Vec::new(),
        Vec::new(),
    );
    for _ in 0..16 {
        // The input for this block is what the first channel played TOTAL
        // frames ago.
        let from = cable.0[0].len();
        cable.1[0] = (0..64)
            .map(|i| {
                (from + i)
                    .checked_sub(TOTAL)
                    .map_or(0.0, |at| cable.0[0][at])
            })
            .collect();
        rig.block_with_input(64, &cable);
        let output = rig.processor.output();
        main.extend_from_slice(output.channel(0));
        let sent = output.ext_channel(0).to_vec();
        pitch.extend_from_slice(&sent);
        cable.0[0].extend_from_slice(&sent);
    }
    // The Pitch is on its channel from the note's first frame, not held back.
    assert_eq!(heard(&pitch)[0], (0, 0.1), "key 72 is 1 V");
    // The sound that comes back and the other track's impulse reach Main
    // together, TOTAL frames in.
    let first = heard(&main)[0];
    assert_eq!(first.0, TOTAL, "the instrument's latency is compensated");
    assert!(
        (first.1 - 1.1).abs() < 1e-6,
        "the impulse and the return: {first:?}"
    );
    assert!(main[..TOTAL].iter().all(|&s| s == 0.0));
}
