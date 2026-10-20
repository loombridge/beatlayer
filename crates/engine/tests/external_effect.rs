//! The external effect in the engine (WP-5.35, `docs/specs/external-
//! devices.md` R17 to R21): it sends its input to the interface's outputs,
//! hears the interface's inputs back, and mixes the dry and the returning
//! signal so they line up. Every block runs as a real-time section under
//! the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_core::time::Ticks;
use bl_engine::{DeviceInput, MediaMap};
use bl_media::SampleData;
use bl_model::{
    Channel, Clip, ClipData, Command, ContentHash, Device, DeviceId, DeviceKind, DeviceRole,
    External, HardwareLatency, ParamId, TrackId, TrackKind,
};
use common::{Rig, track};

/// The frames the interface's round trip takes, and the effect's own
/// Hardware Latency.
const ROUND_TRIP: u32 = 300;
const HARDWARE: u32 = 100;

/// The effect's type id.
const EFFECT: &str = "beatlayer.hardware_effect";

/// The effect's Dry/Wet parameter.
const DRY_WET: ParamId = ParamId(2);
/// The effect's Invert parameter.
const INVERT: ParamId = ParamId(3);

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

/// Puts an external effect on `track`, sending to `audio_to` and hearing
/// `audio_from` back.
fn effect(
    editor: &mut bl_model::Editor,
    track: TrackId,
    audio_to: Channel,
    audio_from: Channel,
) -> DeviceId {
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "Effect",
        DeviceKind::Native {
            device_type: EFFECT.to_owned(),
        },
    )
    .with_role(DeviceRole::AudioEffect)
    .with_external();
    let mut external = External {
        audio_from,
        hardware_latency: HardwareLatency::Samples { value: HARDWARE },
        ..External::default()
    };
    external.audio_to = Some(audio_to);
    device.external = Some(external);
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

/// Puts a whole-song clip of `audio` on `track`.
fn clip(editor: &mut bl_model::Editor, track: TrackId, audio: ContentHash) {
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE * 4,
                ClipData::Audio(bl_model::AudioData::new(audio)),
            ),
        })
        .unwrap();
}

/// The interface's input, as a cable from its outputs to its inputs makes
/// it: each input channel carries the output channel of the same index,
/// `delay` frames late.
struct Loopback {
    delay: usize,
    /// Every frame played on each device output channel so far.
    played: Vec<Vec<f32>>,
    /// The input for the block being processed.
    block: Vec<Vec<f32>>,
}

impl DeviceInput for Loopback {
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
    /// `delay` frames ago, and returns what Main played and what the
    /// interface's first output channel played.
    fn block(&mut self, rig: &mut Rig, frames: usize) -> (Vec<f32>, Vec<f32>) {
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
        let mut ext = Vec::new();
        for (c, played) in self.played.iter_mut().enumerate() {
            if c < output.ext_channels() {
                let samples = output.ext_channel(c)[..frames].to_vec();
                played.extend_from_slice(&samples);
                if c == 0 {
                    ext = samples;
                }
            } else {
                played.extend(std::iter::repeat_n(0.0, frames));
            }
        }
        (output.channel(0).to_vec(), ext)
    }
}

/// The frames the device's latency takes: the interface's round trip and
/// its Hardware Latency.
const TOTAL: usize = (ROUND_TRIP + HARDWARE) as usize;

/// A rig with an audio track playing an impulse, and an external effect on
/// it, with the interface's round trip.
fn rig() -> Rig {
    let mut media = MediaMap::new();
    media.insert(hash(1), impulse(1.0));
    let mut rig = Rig::with_config_and_registry(
        64,
        |config| {
            config.input_channels = 2;
            config.output_channels = 2;
            config.round_trip_latency = ROUND_TRIP;
        },
        |registry| registry.set_media(Arc::new(media)),
    );
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    clip(&mut rig.editor, audio, hash(1));
    effect(
        &mut rig.editor,
        audio,
        Channel::Pair(1),
        Channel::Pair(1),
    );
    rig.sync();
    rig.sender.transport(bl_engine::TransportCommand::Play);
    rig
}

/// The frames where the output is not silent, with what it holds there.
fn heard(out: &[f32]) -> Vec<(usize, f32)> {
    out.iter()
        .enumerate()
        .filter(|(_, s)| s.abs() > 1e-6)
        .map(|(at, s)| (at, *s))
        .collect()
}

// Spec: external-devices R18
#[test]
fn the_effect_sends_to_its_outputs() {
    let mut rig = rig();
    let mut cable = Loopback::new(2, 0);
    let (_, ext) = cable.block(&mut rig, 64);
    assert_eq!(
        heard(&ext),
        vec![(0, 1.0)],
        "the effect sent its signal to the interface's outputs"
    );
}

// Spec: external-devices R18. What the effect sends adds to whatever else
// plays on those channels.
#[test]
fn the_effects_signal_adds_to_what_plays_on_those_channels() {
    let mut rig = rig();
    // A track on Ext. Out 1/2, playing a constant.
    let other = track(&mut rig.editor, TrackKind::Audio, None);
    common::dc(&mut rig.editor, other, 0.25);
    rig.editor
        .apply(Command::SetOutput {
            track: other,
            output: bl_model::Output::External {
                channel: Channel::Pair(1),
            },
        })
        .unwrap();
    rig.sync();
    let mut cable = Loopback::new(2, 0);
    let (_, ext) = cable.block(&mut rig, 64);
    assert_eq!(
        heard(&ext),
        vec![(0, 1.25)],
        "the effect's signal and the track's add on the channel"
    );
}

// Spec: external-devices R19, R20
#[test]
fn dry_and_wet_line_up_and_the_return_is_flipped_by_invert() {
    let mut rig = rig();
    // A cable as long as the device's latency: the returning signal and
    // the dry one arrive together.
    let mut cable = Loopback::new(2, TOTAL);
    let mut main = Vec::new();
    for _ in 0..8 {
        let (out, _) = cable.block(&mut rig, 64);
        main.extend(out);
    }
    // With Dry/Wet at 100 % the output is the returning signal alone, at
    // the frame the hardware brought it back.
    assert_eq!(heard(&main), vec![(TOTAL, 1.0)], "the wet signal plays");

    // With Dry/Wet at 50 % the dry and the wet sum: they line up, so the
    // impulse is at one frame with its own height.
    let device = rig
        .editor
        .document()
        .tracks()
        .flat_map(|track| track.devices_deep())
        .find(|device| device.external.is_some())
        .unwrap()
        .id;
    rig.apply(Command::SetDeviceParam {
        device,
        param: DRY_WET,
        value: 50.0,
    });
    rig.sync();
    let mut cable = Loopback::new(2, TOTAL);
    let mut main = Vec::new();
    for _ in 0..8 {
        let (out, _) = cable.block(&mut rig, 64);
        main.extend(out);
    }
    assert_eq!(
        heard(&main),
        vec![(TOTAL, 1.0)],
        "the dry and the wet sum at one frame"
    );

    // With Invert on, the returning signal's phase is flipped.
    rig.apply(Command::SetDeviceParam {
        device,
        param: DRY_WET,
        value: 100.0,
    });
    rig.apply(Command::SetDeviceParam {
        device,
        param: INVERT,
        value: 1.0,
    });
    rig.sync();
    let mut cable = Loopback::new(2, TOTAL);
    let mut main = Vec::new();
    for _ in 0..8 {
        let (out, _) = cable.block(&mut rig, 64);
        main.extend(out);
    }
    assert_eq!(
        heard(&main),
        vec![(TOTAL, -1.0)],
        "the return is flipped"
    );
}

// Spec: external-devices R19
#[test]
fn a_cable_one_frame_longer_than_the_latency_shows_in_the_sum() {
    let mut rig = rig();
    let device = rig
        .editor
        .document()
        .tracks()
        .flat_map(|track| track.devices_deep())
        .find(|device| device.external.is_some())
        .unwrap()
        .id;
    rig.apply(Command::SetDeviceParam {
        device,
        param: DRY_WET,
        value: 50.0,
    });
    rig.sync();
    let mut cable = Loopback::new(2, TOTAL + 1);
    let mut main = Vec::new();
    for _ in 0..8 {
        let (out, _) = cable.block(&mut rig, 64);
        main.extend(out);
    }
    assert_eq!(
        heard(&main),
        vec![(TOTAL, 0.5), (TOTAL + 1, 0.5)],
        "the dry and the wet are a frame apart, so they comb"
    );
}
