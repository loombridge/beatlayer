//! Align Delay, the alignment delay (WP-5.25; the alignment delay spec): Time,
//! Samples and Distance modes, the temperature, Link, and no reported
//! latency. Every block runs as a real-time section under the allocation
//! trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, SampleRate, TempoMap};
use bl_devices::alignment_delay::{
    DISTANCE_UNIT, FEET, LINK, METRES, MODE, SAMPLES, TEMPERATURE_C, TEMPERATURE_F,
    TEMPERATURE_UNIT, TIME, alignment_delay_type, side_param, speed_of_sound,
};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{
    Block, EngineConfig, NodeConfig, Offline, Registry, TransportCommand, TransportInfo,
};
use bl_model::{Command, Device, DeviceKind, Editor, ParamId, TrackKind};
use common::{device, noise, run_blocks, set};

const TIME_MODE: f64 = 0.0;
const SAMPLES_MODE: f64 = 1.0;
const DISTANCE_MODE: f64 = 2.0;

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

/// Align Delay at `rate` with `params`, run over `input` on both channels as one
/// block.
fn render(rate: SampleRate, params: &[(ParamId, f64)], input: &[f32]) -> Vec<Vec<f32>> {
    let config = NodeConfig {
        sample_rate: rate,
        max_frames: input.len(),
        channels: 2,
    };
    let mut node = registry()
        .create(&device(&alignment_delay_type(), params), &config)
        .expect("the registry makes Align Delay");
    let mut data: Vec<f32> = input.iter().chain(input).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            2,
            input.len(),
            input.len(),
            &[],
            TransportInfo::default(),
            rate,
        );
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(input.len()).map(<[f32]>::to_vec).collect()
}

fn impulse(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[0] = 1.0;
    input
}

/// The frame the impulse lands on, as a fraction when it's spread.
fn lands(channel: &[f32]) -> usize {
    channel
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map(|(frame, _)| frame)
        .unwrap()
}

// Spec: alignment-delay R2
#[test]
fn samples_mode_shifts_the_input_by_exactly_its_count() {
    let input = noise(5_000);
    let params = [
        (MODE, SAMPLES_MODE),
        (side_param(0, SAMPLES), 137.0),
        (side_param(1, SAMPLES), 137.0),
    ];
    let out = render(SampleRate::HZ_48000, &params, &input);
    for channel in &out {
        assert!(channel[..137].iter().all(|s| *s == 0.0), "silence first");
        for n in 137..input.len() {
            assert_eq!(channel[n].to_bits(), input[n - 137].to_bits(), "frame {n}");
        }
    }
    // No delay passes the input untouched.
    let out = render(SampleRate::HZ_48000, &[(MODE, SAMPLES_MODE)], &input);
    assert_eq!(out[0], input);
}

// Spec: alignment-delay R1
#[test]
fn time_mode_delays_by_its_frame_count_at_44_1_48_and_96_khz() {
    for (rate, frames) in [
        (SampleRate::HZ_44100, 441usize),
        (SampleRate::HZ_48000, 480),
        (SampleRate::HZ_96000, 960),
    ] {
        let params = [
            (MODE, TIME_MODE),
            (side_param(0, TIME), 10.0),
            (side_param(1, TIME), 10.0),
        ];
        let out = render(rate, &params, &impulse(2_000));
        assert_eq!(lands(&out[0]), frames, "10 ms at {rate:?}");
        assert_eq!(out[0][frames], 1.0);
        assert_eq!(lands(&out[1]), frames);
    }
}

// Spec: alignment-delay R3
#[test]
fn distance_mode_delays_by_the_time_sound_takes_in_metres_and_feet() {
    let speed = speed_of_sound(20.0);
    assert!((speed - 343.42).abs() < 0.01);
    // 3.43 m at 20 °C is about 10 ms: 479.4 frames at 48 kHz.
    let wanted = 3.43 / speed * 48_000.0;
    assert!((wanted - 479.4).abs() < 0.1);
    let at = |unit_feet: bool, fahrenheit: bool, distance: f64, temperature: f64| {
        let distance_param = if unit_feet { FEET } else { METRES };
        let params = [
            (MODE, DISTANCE_MODE),
            (DISTANCE_UNIT, f64::from(u8::from(unit_feet))),
            (TEMPERATURE_UNIT, f64::from(u8::from(fahrenheit))),
            (TEMPERATURE_C, temperature),
            (TEMPERATURE_F, temperature),
            (side_param(0, distance_param), distance),
            (side_param(1, distance_param), distance),
        ];
        lands(&render(SampleRate::HZ_48000, &params, &impulse(2_000))[0]) as f64
    };
    let within_one = |got: f64, want: f64, what: &str| {
        assert!((got - want).abs() <= 1.0, "{what}: {got} against {want}");
    };
    within_one(at(false, false, 3.43, 20.0), wanted, "metres, °C");
    within_one(at(true, false, 3.43 / 0.3048, 20.0), wanted, "feet, °C");
    within_one(at(false, true, 3.43, 68.0), wanted, "metres, °F");
    within_one(at(true, true, 3.43 / 0.3048, 68.0), wanted, "feet, °F");
    // Colder air is slower: 0 °C (32 °F) takes longer.
    let cold = 3.43 / speed_of_sound(0.0) * 48_000.0;
    within_one(at(false, false, 3.43, 0.0), cold, "metres, 0 °C");
    within_one(at(false, true, 3.43, 32.0), cold, "metres, 32 °F");
    assert!(cold > wanted + 10.0);
}

// Spec: alignment-delay R4
#[test]
fn link_gives_the_right_side_the_left_delay() {
    let params = |link: f64| {
        [
            (MODE, SAMPLES_MODE),
            (LINK, link),
            (side_param(0, SAMPLES), 100.0),
            (side_param(1, SAMPLES), 300.0),
        ]
    };
    let apart = render(SampleRate::HZ_48000, &params(0.0), &impulse(1_000));
    assert_eq!((lands(&apart[0]), lands(&apart[1])), (100, 300));
    let linked = render(SampleRate::HZ_48000, &params(1.0), &impulse(1_000));
    assert_eq!((lands(&linked[0]), lands(&linked[1])), (100, 100));
}

// Spec: alignment-delay R5
#[test]
fn a_change_of_delay_cuts_to_the_new_delay_on_its_frame() {
    let input = noise(2_000);
    let registry = registry();
    let params = [
        (MODE, SAMPLES_MODE),
        (side_param(0, SAMPLES), 100.0),
        (LINK, 1.0),
    ];
    let mut node = common::node(&registry, &device(&alignment_delay_type(), &params), 2_000);
    let out = common::run(
        node.as_mut(),
        &input,
        &[set(1_000, side_param(0, SAMPLES), 50.0)],
    );
    assert_eq!(out[0][999], input[899]);
    assert_eq!(out[0][1_000], input[950]);
    assert_eq!(out[0][1_500], input[1_450]);
}

// Spec: alignment-delay R6
#[test]
fn it_reports_no_latency_and_its_track_lands_later_than_the_others_by_its_delay() {
    let registry = registry();
    assert_eq!(
        common::node(&registry, &device(&alignment_delay_type(), &[]), 64).latency(),
        0
    );

    // Two tracks play the same tone; the second goes through Align Delay.
    let render = |plumb: bool, only_first: bool| {
        let mut editor = Editor::new();
        let tracks = if only_first { 1 } else { 2 };
        for n in 0..tracks {
            let track = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id: track,
                    kind: TrackKind::Audio,
                    name: String::new(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
            let mut tone = Device::new(
                editor.new_id(),
                "",
                DeviceKind::Native {
                    device_type: TEST_TONE.to_owned(),
                },
            );
            tone.params.insert(test_tone::LEVEL, -12.0);
            let mut chain = vec![tone];
            if plumb && n == 1 {
                let mut delay = alignment_delay_type().device(editor.new_id());
                delay.params.insert(MODE, SAMPLES_MODE);
                delay.params.insert(side_param(0, SAMPLES), 100.0);
                delay.params.insert(LINK, 1.0);
                chain.push(delay);
            }
            for (index, device) in chain.into_iter().enumerate() {
                editor
                    .apply(Command::InsertDevice {
                        track,
                        index,
                        device,
                    })
                    .unwrap();
            }
        }
        let mut offline = Offline::new(EngineConfig::new(common::RATE, 256), self::registry());
        offline.sync(editor.document());
        offline.sender().transport(TransportCommand::Play);
        let mut out = Vec::new();
        offline.render(4_800, |block| out.extend_from_slice(block.channel(0)));
        assert_eq!(offline.heap_operations(), 0);
        out
    };
    let one = render(false, true);
    let both = render(true, false);
    assert!(one[50].abs() > 1e-4, "the tone plays");
    for n in 0..4_800 {
        let later = if n >= 100 { one[n - 100] } else { 0.0 };
        assert!(
            (both[n] - (one[n] + later)).abs() < 1e-5,
            "frame {n}: {} against {}",
            both[n],
            one[n] + later
        );
    }
}

// Spec: alignment-delay R7
#[test]
fn renders_are_bit_identical_at_every_block_size_and_run_after_run() {
    let input = {
        let n = noise(20_000);
        vec![n.clone(), n.iter().rev().copied().collect()]
    };
    let params = [
        (MODE, TIME_MODE),
        (side_param(0, TIME), 12.5),
        (side_param(1, TIME), 40.0),
    ];
    let events = [
        set(7_000, side_param(0, TIME), 3.0),
        set(9_001, MODE, DISTANCE_MODE),
    ];
    let tempo = TempoMap::constant(Bpm::DEFAULT);
    let render = |block: usize| {
        let mut node = common::node(&registry(), &device(&alignment_delay_type(), &params), 4096);
        run_blocks(node.as_mut(), &input, block, &events, &tempo)
    };
    let reference = render(4096);
    assert_eq!(render(4096), reference, "run after run");
    for block in [1, 64, 512] {
        assert_eq!(render(block), reference, "block size {block}");
    }
}
