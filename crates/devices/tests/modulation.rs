//! A modulator in an engine (WP-5.10; the modulation spec, R14, R16, R19 and
//! R20): an LFO on one track modulates or holds a device on another, on
//! the same frames at any block size, whether the song plays or not; a
//! mapping edited in the document reaches the target without a new program.
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::SampleRate;
use bl_devices::modulator::{RATE_HZ, WAVE};
use bl_devices::testing::{UTILITY, UTILITY_GAIN, utility_type};
use bl_engine::testing::{self as engine_testing, Probe};
use bl_engine::{
    Collector, Compiler, EngineConfig, Monitor, Processor, Sender, TransportCommand, channel,
};
use bl_model::modulator::{self, MODE, ModTarget, POLARITY};
use bl_model::{Command, DeviceId, Editor, TrackId, TrackKind};

const RATE: SampleRate = SampleRate::HZ_48000;

struct Rig {
    editor: Editor,
    compiler: Compiler,
    sender: Sender,
    processor: Processor,
    _monitor: Monitor,
    _collector: Collector,
}

impl Rig {
    fn new(max_frames: usize) -> Self {
        let config = EngineConfig::new(RATE, max_frames);
        let collector = Collector::new();
        let mut registry = engine_testing::registry(&Arc::new(Probe::default()));
        bl_devices::testing::register(&mut registry);
        bl_devices::modulator::register(&mut registry);
        let compiler = Compiler::new(config, registry, collector.handle());
        let (sender, processor, monitor) = channel(&config);
        Self {
            editor: Editor::new(),
            compiler,
            sender,
            processor,
            _monitor: monitor,
            _collector: collector,
        }
    }

    fn sync(&mut self) {
        let update = self.compiler.sync(self.editor.document());
        self.sender.send_update(update);
        assert!(self.sender.flush(), "the queue is full");
    }

    fn apply(&mut self, command: Command) {
        self.editor.apply(command).unwrap();
        self.sync();
    }

    fn track(&mut self, kind: TrackKind) -> TrackId {
        let id = self.editor.new_id();
        self.apply(Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent: None,
            index: 0,
        });
        id
    }

    fn insert(
        &mut self,
        track: TrackId,
        device_type: &str,
        params: &[(bl_model::ParamId, f64)],
    ) -> DeviceId {
        let id = self.editor.new_id();
        let mut device = bl_model::Device::new(
            id,
            "",
            bl_model::DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        );
        for &(param, value) in params {
            device.params.insert(param, value);
        }
        let index = self.editor.document().track(track).unwrap().devices().len();
        self.apply(Command::InsertDevice {
            track,
            index,
            device,
        });
        id
    }

    /// The first output channel over `total` frames, in blocks of `block`,
    /// each a real-time section.
    fn render(&mut self, total: usize, block: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(total);
        while out.len() < total {
            let n = block.min(total - out.len());
            let processor = &mut self.processor;
            bl_rt::rt_test!({
                processor.process(n);
            });
            out.extend_from_slice(self.processor.output().channel(0));
        }
        out
    }
}

/// A LFO on an audio track, rising at 10 Hz (a cycle of 4,800 frames, a
/// whole number of 32-frame grid steps) and mapped to the gain of a test
/// utility on a track carrying a signal of 1.
fn rig(block: usize, modulator_params: &[(bl_model::ParamId, f64)]) -> (Rig, DeviceId, DeviceId) {
    let mut rig = Rig::new(block);
    let source = rig.track(TrackKind::Audio);
    let target = rig.track(TrackKind::Midi);
    rig.insert(
        target,
        bl_engine::testing::DC,
        &[(bl_engine::testing::VALUE, 1.0)],
    );
    let utility = rig.insert(target, UTILITY, &[]);
    let mut params = vec![
        (WAVE, 1.0),
        (RATE_HZ, 10.0),
        (POLARITY, 1.0),
        (modulator::amount(0), 100.0),
    ];
    params.extend_from_slice(modulator_params);
    let wave = rig.insert(source, modulator::WAVE_MOD, &params);
    rig.apply(Command::MapModulator {
        device: wave,
        slot: 0,
        target: ModTarget::Device {
            device: utility,
            param: UTILITY_GAIN,
        },
    });
    (rig, wave, utility)
}

/// The track's signal at the gain `gain`, through the pan law and Main: what
/// the signal of 1 comes out as, so a played gain can be read from it.
fn factor() -> f64 {
    let mut rig = Rig::new(64);
    let target = rig.track(TrackKind::Midi);
    rig.insert(
        target,
        bl_engine::testing::DC,
        &[(bl_engine::testing::VALUE, 1.0)],
    );
    rig.insert(target, UTILITY, &[]);
    rig.sender.transport(TransportCommand::Play);
    f64::from(rig.render(512, 64)[500])
}

/// The gain at position `position` on the utility's gain travel.
fn gain_at(position: f64) -> f64 {
    let descriptor = utility_type()
        .description
        .param(UTILITY_GAIN)
        .unwrap()
        .clone();
    descriptor.denormalize(position)
}

fn position_of(gain: f64) -> f64 {
    utility_type()
        .description
        .param(UTILITY_GAIN)
        .unwrap()
        .normalize(gain)
}

// Spec: modulation R14, R20
#[test]
fn an_lfo_on_one_track_modulates_another_on_its_frames() {
    let factor = factor();
    assert!(factor > 0.0);
    let mut renders = Vec::new();
    for block in [1usize, 64, 512] {
        let (mut rig, _, _) = rig(block, &[]);
        rig.sender.transport(TransportCommand::Play);
        renders.push(rig.render(4_800, block));
    }
    assert!(
        renders[0] == renders[1] && renders[1] == renders[2],
        "the same at any block size"
    );
    // At each grid frame the gain is the control's position (0 dB) moved by
    // the wave's output, the share 32·k / 4,800 of the cycle.
    let out = &renders[0];
    for step in [1usize, 10, 40, 80, 120, 149] {
        let share = step as f64 / 150.0;
        let want = gain_at(position_of(1.0) + share) * factor;
        let got = f64::from(out[step * 32]);
        assert!(
            (got - want).abs() < 1e-4 * want.max(1.0),
            "step {step}: {got} isn't {want}"
        );
    }
    assert!(out[4_000] > out[100], "it rises through the cycle");
}

// Spec: modulation R16
#[test]
fn remote_control_holds_the_target_between_min_and_max() {
    let factor = factor();
    let (mut rig, wave, utility) = rig(
        64,
        &[
            (MODE, 1.0),
            (modulator::min(0), 20.0),
            (modulator::max(0), 60.0),
        ],
    );
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(4_800, 64);
    for step in [1usize, 40, 100, 149] {
        let share = step as f64 / 150.0;
        let want = gain_at(0.2 + 0.4 * share) * factor;
        let got = f64::from(out[step * 32]);
        assert!(
            (got - want).abs() < 1e-4 * want.max(1.0),
            "step {step}: {got} isn't {want}"
        );
    }
    // Hand moves of the control are refused while it is held.
    assert!(
        rig.editor
            .apply(Command::SetDeviceParam {
                device: utility,
                param: UTILITY_GAIN,
                value: 0.1,
            })
            .is_err(),
        "the control is held"
    );
    // Back in Modulation mode the control moves, and is the base again.
    rig.apply(Command::SetDeviceParam {
        device: wave,
        param: MODE,
        value: 0.0,
    });
    rig.apply(Command::SetDeviceParam {
        device: utility,
        param: UTILITY_GAIN,
        value: 0.1,
    });
    let back = rig.render(4_800, 64);
    let want = gain_at(position_of(0.1) + 100.0 / 150.0) * factor;
    assert!(
        (f64::from(back[100 * 32]) - want).abs() < 1e-4,
        "the control's value is the base again"
    );
}

// Spec: modulation R15, R20
#[test]
fn a_mapping_edit_reaches_the_target_without_a_new_program() {
    let factor = factor();
    let (mut rig, wave, _) = rig(64, &[]);
    rig.sender.transport(TransportCommand::Play);
    let moving = rig.render(2_400, 64);
    assert!(moving[2_000] > moving[100]);
    let generation = rig.compiler.generation();
    rig.apply(Command::UnmapModulator {
        device: wave,
        slot: 0,
    });
    assert_eq!(rig.compiler.generation(), generation, "no recompile");
    let still = rig.render(2_400, 64);
    assert!(
        still.iter().all(|&s| (f64::from(s) - factor).abs() < 1e-4),
        "an unmapped target plays its own value"
    );
}

// Spec: modulation R20
#[test]
fn the_song_need_not_play_for_a_free_lfo_to_move_its_target() {
    let (mut rig, _, _) = rig(64, &[]);
    let out = rig.render(4_800, 64);
    let (low, high) = out
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &s| (l.min(s), h.max(s)));
    assert!(
        high > low * 1.2,
        "stopped, it still swings: {low} to {high}"
    );
}

// Spec: modulation R12, R19
#[test]
fn a_modulator_inside_a_rack_chain_modulates_a_device_on_another_track() {
    use bl_model::rack::{Chain, RackKind};
    use bl_model::{ChainId, Device, DeviceKind};
    let factor = factor();
    let mut rig = Rig::new(64);
    let source = rig.track(TrackKind::Audio);
    let target = rig.track(TrackKind::Midi);
    rig.insert(
        target,
        bl_engine::testing::DC,
        &[(bl_engine::testing::VALUE, 1.0)],
    );
    let utility = rig.insert(target, UTILITY, &[]);
    let mut wave = Device::new(
        rig.editor.new_id(),
        "LFO",
        DeviceKind::Native {
            device_type: modulator::WAVE_MOD.to_owned(),
        },
    );
    for (param, value) in [
        (WAVE, 1.0),
        (RATE_HZ, 10.0),
        (POLARITY, 1.0),
        (modulator::amount(0), 100.0),
    ] {
        wave.params.insert(param, value);
    }
    let wave_id = wave.id;
    let chain_id: ChainId = rig.editor.new_id();
    let chain = Chain::new(chain_id, "Chain").with_devices(vec![wave]);
    let rack = Device::rack(rig.editor.new_id(), RackKind::AudioEffect).with_chain(chain);
    rig.apply(Command::InsertDevice {
        track: source,
        index: 0,
        device: rack,
    });
    rig.apply(Command::MapModulator {
        device: wave_id,
        slot: 0,
        target: ModTarget::Device {
            device: utility,
            param: UTILITY_GAIN,
        },
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(4_800, 64);
    for step in [10usize, 80, 140] {
        let want = gain_at(position_of(1.0) + step as f64 / 150.0) * factor;
        let got = f64::from(out[step * 32]);
        assert!(
            (got - want).abs() < 1e-4 * want.max(1.0),
            "step {step}: {got} isn't {want}"
        );
    }
}
