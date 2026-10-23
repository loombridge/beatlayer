//! Modulators driven by audio, notes and MIDI in an engine (WP-5.11; the
//! modulation spec, R28): the track holding a parameter an Envelope Follower maps
//! waits for the Envelope Follower's, so its effect lands on the frames it is worked
//! out for; a mapping that would make two tracks wait on each other is
//! refused; and a target earlier in the modulator's own chain takes its
//! effect one 32-frame step late, the same at any block size. Every block
//! runs as a real-time section under the allocation trap.

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::gate_mod::SUSTAIN_LEVEL;
use bl_devices::level_mod::{FALL, RISE};
use bl_devices::testing::{UTILITY, UTILITY_BALANCE, UTILITY_GAIN, utility_type};
use bl_engine::testing::{self as engine_testing, Probe};
use bl_engine::{
    Collector, Compiler, EngineConfig, Monitor, Pool, PoolConfig, Processor, Sender,
    TransportCommand, channel,
};
use bl_model::modulator::{self, ModTarget, POLARITY};
use bl_model::{
    Chain, ChainId, Clip, ClipData, Command, Device, DeviceId, DeviceKind, DeviceRole, Editor,
    Input, MidiData, Note, Output, RackKind, Tap, TrackId, TrackKind, Vector,
};

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
    /// A rig that runs its tasks on the workers `BL_TEST_WORKERS` names, if
    /// it is set (`BL_TEST_WORKERS=3 cargo nextest run -p bl-devices`).
    fn new(max_frames: usize) -> Self {
        let mut rig = Self::serial(max_frames);
        let workers = std::env::var("BL_TEST_WORKERS")
            .ok()
            .and_then(|workers| workers.parse().ok())
            .unwrap_or(0);
        rig.use_workers(workers);
        rig
    }

    /// Runs the rig's tasks on `workers` workers beside the test's thread;
    /// `0` runs them on the one.
    fn use_workers(&mut self, workers: usize) {
        let config = *self.processor.config();
        let pool =
            (workers > 0).then(|| Pool::start(&config, &PoolConfig::plain(workers)).unwrap());
        let _ = self.processor.set_pool(pool);
    }

    fn serial(max_frames: usize) -> Self {
        let config = EngineConfig::new(RATE, max_frames);
        let collector = Collector::new();
        let mut registry = engine_testing::registry(&Arc::new(Probe::default()));
        bl_devices::testing::register(&mut registry);
        bl_devices::level_mod::register(&mut registry);
        bl_devices::gate_mod::register(&mut registry);
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

/// What the unit signal comes out as through a track's mixer and Main.
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

/// The unit signal's track `target` (a test DC at 1 and a utility), made
/// after the source so it comes first in display order, which is where it
/// would run without a wait; and a muted source track holding a DC at
/// `level`, then an Envelope Follower that follows it at once, mapped to the
/// utility's gain, unipolar at full amount. Returns the rig and the pieces.
fn follower_rig(block: usize, level: f64) -> (Rig, TrackId, TrackId, DeviceId, DeviceId) {
    follower_rig_of(block, level, TrackKind::Midi)
}

/// As [`follower_rig`], on tracks of `kind`.
fn follower_rig_of(
    block: usize,
    level: f64,
    kind: TrackKind,
) -> (Rig, TrackId, TrackId, DeviceId, DeviceId) {
    let mut rig = Rig::new(block);
    let source = rig.track(kind);
    let target = rig.track(kind);
    rig.insert(
        source,
        bl_engine::testing::DC,
        &[(bl_engine::testing::VALUE, level)],
    );
    // The type's own device, which has a sidechain.
    let mut device = bl_devices::level_mod::level_mod_type().device(rig.editor.new_id());
    for (param, value) in [
        (POLARITY, 1.0),
        (modulator::amount(0), 100.0),
        (RISE, 0.0),
        (FALL, 0.0),
    ] {
        device.params.insert(param, value);
    }
    let follower = device.id;
    rig.apply(Command::InsertDevice {
        track: source,
        index: 1,
        device,
    });
    rig.apply(Command::SetMute {
        track: source,
        mute: true,
    });
    rig.insert(
        target,
        bl_engine::testing::DC,
        &[(bl_engine::testing::VALUE, 1.0)],
    );
    let utility = rig.insert(target, UTILITY, &[]);
    rig.apply(Command::MapModulator {
        device: follower,
        slot: 0,
        target: ModTarget::Device {
            device: utility,
            param: UTILITY_GAIN,
        },
    });
    // Let the mute's glide finish, so the muted source is silent.
    rig.render(2_048, block);
    (rig, source, target, follower, utility)
}

// Spec: modulation R28
#[test]
fn a_follower_on_one_track_modulates_a_later_track_on_its_frames() {
    let factor = factor();
    let mut renders = Vec::new();
    for block in [1usize, 64, 512] {
        let (mut rig, ..) = follower_rig(block, 0.5);
        rig.sender.transport(TransportCommand::Play);
        renders.push(rig.render(2_048, block));
    }
    assert!(
        renders[0] == renders[1] && renders[1] == renders[2],
        "the same at any block size"
    );
    // The source's track runs first, though the target's is first in display
    // order, so the half-scale signal moves the gain up by half its travel
    // from the first frame: no frame of the target plays before its source.
    let want = gain_at((position_of(1.0) + 0.5).min(1.0)) * factor;
    for frame in [0, 1, 31, 32, 500, 2_047] {
        let got = f64::from(renders[0][frame]);
        assert!(
            (got - want).abs() < 1e-4 * want.max(1.0),
            "frame {frame}: {got} isn't {want}"
        );
    }
}

// Spec: modulation R28
#[test]
fn a_mapping_that_would_loop_tracks_is_refused() {
    let (mut rig, source, target, follower, utility) = follower_rig_of(64, 0.5, TrackKind::Audio);
    // The target's track feeds the source's: the follower couldn't run
    // before the track it moves.
    rig.apply(Command::SetOutput {
        track: target,
        output: Output::Track { track: source },
    });
    let refused = rig
        .editor
        .apply(Command::MapModulator {
            device: follower,
            slot: 1,
            target: ModTarget::Device {
                device: utility,
                param: UTILITY_BALANCE,
            },
        })
        .unwrap_err()
        .to_string();
    assert!(refused.contains("wait on each other"), "{refused}");
    assert_eq!(
        rig.editor
            .document()
            .device(follower)
            .unwrap()
            .mappings
            .len(),
        1,
        "the refused mapping isn't made"
    );

    // Routing the other way is fine: the follower's track feeds the target's.
    let (mut rig, source, target, follower, utility) = follower_rig_of(64, 0.5, TrackKind::Audio);
    rig.apply(Command::SetOutput {
        track: source,
        output: Output::Track { track: target },
    });
    rig.editor
        .apply(Command::MapModulator {
            device: follower,
            slot: 1,
            target: ModTarget::Device {
                device: utility,
                param: UTILITY_BALANCE,
            },
        })
        .unwrap();

    // A routing made after the mapping that would close the loop is kept in
    // the set and the mapping carries nothing: the target plays its own
    // value, and the engine has an order to run in. The follower hears
    // silence and swings its target down to nothing, bipolar, when it works.
    let (mut rig, source, target, follower, _) = follower_rig_of(64, 0.0, TrackKind::Audio);
    rig.apply(Command::SetMute {
        track: source,
        mute: false,
    });
    rig.apply(Command::SetDeviceParam {
        device: follower,
        param: POLARITY,
        value: 0.0,
    });
    rig.sender.transport(TransportCommand::Play);
    rig.render(2_048, 64);
    assert!(
        rig.render(1_024, 64).iter().all(|&s| s.abs() < 1e-3),
        "the mapping works while the tracks are apart"
    );
    rig.apply(Command::SetSidechainInput {
        device: follower,
        input: Input::Track {
            track: target,
            tap: Tap::PostMixer,
            chain: None,
            output: None,
        },
    });
    rig.render(2_048, 64);
    let out = rig.render(1_024, 64);
    assert!(
        out.iter().all(|&s| (f64::from(s) - 1.0).abs() < 1e-3),
        "the mapping carries nothing: {} for 1",
        out[500]
    );
}

// Spec: modulation R28
#[test]
fn an_earlier_target_in_the_chain_is_one_step_late_at_any_block_size() {
    let factor = factor();
    let build = |block: usize, follower_first: bool| {
        let mut rig = Rig::new(block);
        let track = rig.track(TrackKind::Midi);
        rig.insert(
            track,
            bl_engine::testing::DC,
            &[(bl_engine::testing::VALUE, 0.25)],
        );
        let params = [
            (POLARITY, 1.0),
            (modulator::amount(0), 100.0),
            (RISE, 0.0),
            (FALL, 0.0),
        ];
        // The follower hears the signal after the utility, or before it.
        let (follower, utility) = if follower_first {
            let follower = rig.insert(track, modulator::LEVEL_MOD, &params);
            (follower, rig.insert(track, UTILITY, &[]))
        } else {
            let utility = rig.insert(track, UTILITY, &[]);
            (rig.insert(track, modulator::LEVEL_MOD, &params), utility)
        };
        rig.apply(Command::MapModulator {
            device: follower,
            slot: 0,
            target: ModTarget::Device {
                device: utility,
                param: UTILITY_GAIN,
            },
        });
        rig.sender.transport(TransportCommand::Play);
        rig
    };

    let mut renders = Vec::new();
    for block in [1usize, 64, 512] {
        let mut rig = build(block, false);
        renders.push(rig.render(2_048, block));
    }
    assert!(
        renders[0] == renders[1] && renders[1] == renders[2],
        "the same at any block size"
    );
    // A quarter-scale signal through the gain: the utility sits before the
    // follower, so its input is what the follower hears, and 32 frames of
    // the base play before the first of the follower's output reaches it.
    let base = 0.25 * factor;
    for (frame, &sample) in renders[0][..32].iter().enumerate() {
        assert!(
            (f64::from(sample) - base).abs() < 1e-4,
            "frame {frame} plays the base: {sample} for {base}"
        );
    }
    assert!(
        (f64::from(renders[0][32]) - base).abs() > 1e-3,
        "frame 32 plays the modulation: {}",
        renders[0][32]
    );

    // With the follower before the utility the same effect lands at once.
    let mut direct = build(64, true);
    let out = direct.render(2_048, 64);
    assert!(
        (f64::from(out[0]) - base).abs() > 1e-3,
        "no step late: {}",
        out[0]
    );
}

/// A source track playing a held note into an Envelope MIDI mapped to the gain of
/// a utility on the target track after it, playing.
fn gate_rig(block: usize) -> Rig {
    let mut rig = Rig::new(block);
    let source = rig.track(TrackKind::Midi);
    let target = rig.track(TrackKind::Midi);
    rig.insert(
        target,
        bl_engine::testing::DC,
        &[(bl_engine::testing::VALUE, 1.0)],
    );
    let utility = rig.insert(target, UTILITY, &[]);
    let gate = rig.insert(
        source,
        modulator::GATE_MOD,
        &[
            (POLARITY, 1.0),
            (modulator::amount(0), 100.0),
            (SUSTAIN_LEVEL, 60.0),
        ],
    );
    rig.apply(Command::MapModulator {
        device: gate,
        slot: 0,
        target: ModTarget::Device {
            device: utility,
            param: UTILITY_GAIN,
        },
    });
    let clip = rig.editor.new_id();
    let note = Note::new(60, Ticks::ZERO, Ticks::WHOLE, 100);
    rig.apply(Command::CreateClip {
        track: source,
        clip: Clip::new(
            clip,
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Midi(MidiData {
                notes: Vector::from(vec![note]),
                start: Ticks::ZERO,
                end: None,
                looping: false,
                loop_start: Ticks::ZERO,
                loop_end: None,
                signature: None,
                groups: Vec::new(),
            }),
        ),
    });
    rig.sender.transport(TransportCommand::Play);
    rig
}

// Spec: modulation R25, R28
#[test]
fn a_note_driven_modulator_modulates_a_later_track_at_any_block_size() {
    let factor = factor();
    let mut renders = Vec::new();
    for block in [1usize, 64, 512] {
        renders.push(gate_rig(block).render(24_000, block));
    }
    assert!(
        renders[0] == renders[1] && renders[1] == renders[2],
        "the same at any block size"
    );
    // The envelope holds 60 % while the note is held, once its 10 ms attack
    // and 300 ms decay are over (14,880 frames).
    let want = gain_at((position_of(1.0) + 0.6).min(1.0)) * factor;
    let got = f64::from(renders[0][20_000]);
    assert!((got - want).abs() < 2e-3 * want, "{got} isn't {want}");
    assert!(
        (f64::from(renders[0][100]) - factor).abs() > 1e-3,
        "the note moved the target at its start"
    );
}

// Spec: modulation R28; racks R15
#[test]
fn modulators_render_the_same_with_workers_as_on_the_audio_thread_alone() {
    for block in [64usize, 512] {
        let serial = {
            let mut rig = follower_rig(block, 0.5).0;
            rig.sender.transport(TransportCommand::Play);
            (
                rig.render(4_096, block),
                gate_rig(block).render(8_192, block),
            )
        };
        for workers in [1, 3] {
            let mut rig = follower_rig(block, 0.5).0;
            rig.use_workers(workers);
            rig.sender.transport(TransportCommand::Play);
            let follower = rig.render(4_096, block);
            let mut gate = gate_rig(block);
            gate.use_workers(workers);
            let gate = gate.render(8_192, block);
            assert!(
                follower == serial.0 && gate == serial.1,
                "{workers} workers, blocks of {block}, differ from the audio thread alone"
            );
        }
    }
}

// Spec: modulation R28; racks R15
#[test]
fn a_target_in_another_chain_of_the_modulators_rack_is_one_step_late() {
    let factor = factor();
    let build = |block: usize, workers: usize| {
        let mut rig = Rig::new(block);
        rig.use_workers(workers);
        let track = rig.track(TrackKind::Midi);
        rig.insert(
            track,
            bl_engine::testing::DC,
            &[(bl_engine::testing::VALUE, 0.25)],
        );
        let chains: [ChainId; 2] = [rig.editor.new_id(), rig.editor.new_id()];
        let rack = Device::rack(rig.editor.new_id(), RackKind::AudioEffect)
            .with_chain(Chain::new(chains[0], "Follower"))
            .with_chain(Chain::new(chains[1], "Target"));
        rig.apply(Command::InsertDevice {
            track,
            index: 1,
            device: rack,
        });
        let mut follower = bl_devices::level_mod::level_mod_type().device(rig.editor.new_id());
        for (param, value) in [
            (POLARITY, 1.0),
            (modulator::amount(0), 100.0),
            (RISE, 0.0),
            (FALL, 0.0),
        ] {
            follower.params.insert(param, value);
        }
        let follower_id = follower.id;
        rig.apply(Command::InsertChainDevice {
            chain: chains[0],
            index: 0,
            device: follower.with_role(DeviceRole::AudioEffect),
        });
        let utility = Device::new(
            rig.editor.new_id(),
            "",
            DeviceKind::Native {
                device_type: UTILITY.to_owned(),
            },
        )
        .with_role(DeviceRole::AudioEffect);
        let utility_id = utility.id;
        rig.apply(Command::InsertChainDevice {
            chain: chains[1],
            index: 0,
            device: utility,
        });
        rig.apply(Command::MapModulator {
            device: follower_id,
            slot: 0,
            target: ModTarget::Device {
                device: utility_id,
                param: UTILITY_GAIN,
            },
        });
        rig.sender.transport(TransportCommand::Play);
        rig.render(4_096, block)
    };
    let alone = build(64, 0);
    // The chains may run at once, so the target reads what the follower
    // wrote 32 frames before: the base plays for the first step, whatever
    // the threads do.
    let base = alone[0];
    assert!(
        alone[..32].iter().all(|&s| s == base),
        "the first step plays the base"
    );
    assert!(
        (f64::from(alone[40]) - f64::from(base)).abs() > 1e-3 * factor,
        "the follower moves the target after the first step"
    );
    for (block, workers) in [(1, 0), (512, 0), (64, 1), (64, 3), (512, 3)] {
        let got = build(block, workers);
        assert!(
            got == alone,
            "{workers} workers, blocks of {block}, differ from the audio thread alone"
        );
    }
}
