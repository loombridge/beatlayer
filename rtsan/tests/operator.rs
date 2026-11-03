//! Operator, part 2, under RealtimeSanitizer (WP-8.13; the Operator spec,
//! R30): overlapping notes play while the LFO, the filter and the shaper,
//! the global targets, Glide and Spread change, and drawn partials swap new
//! tables in. RTSan stops the process at the first allocation, free, lock, or
//! blocking call in any of it, and the allocation trap, installed as in every
//! Beatlayer test binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_devices::operator::{
    self as op, ENV_LOOP, GLIDE_ON, GLIDE_TIME, LFO_AMOUNT, LFO_AMOUNT_A, LFO_DEST_B, LFO_FILTER,
    LFO_OSC, LFO_RANGE, LFO_RATE, LFO_WAVE, OPERATOR, OSC_REPEAT, OSC_REPEAT_ON, OSC_WAVE, SPREAD,
    osc, target_amount, target_dest,
};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceKind, DeviceRole, Editor, MidiData, Note, ParamId,
    TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn part_twos_controls_and_partial_edits_during_playback_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut registry = Registry::new();
    op::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: OPERATOR.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    let id = device.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let notes: Vec<Note> = (0..40u32)
        .map(|n| {
            Note::new(
                (36 + (n * 7) % 48) as u8,
                Ticks(i64::from(n) * Ticks::QUARTER.0 / 4),
                Ticks::WHOLE,
                (30 + (n * 11) % 90) as u8,
            )
        })
        .collect();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE * 4,
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let set = |param: ParamId, value: f64| Command::SetDeviceParam {
        device: id,
        param,
        value,
    };
    // Everything on: the LFO to a level, the cutoff and a destination, the
    // filter and shaper, Spread, Glide and a target from each source.
    for command in [
        set(osc(1, op::OSC_LEVEL), 0.4),
        set(osc(0, OSC_WAVE), 22.0),
        set(LFO_AMOUNT, 80.0),
        set(ParamId(LFO_OSC), 1.0),
        set(LFO_FILTER, 1.0),
        set(op::FILTER_ON, 1.0),
        set(op::FILTER_FREQ, 900.0),
        set(op::SHAPER_TYPE, 2.0),
        set(op::SHAPER_WET, 60.0),
        set(SPREAD, 70.0),
        set(GLIDE_ON, 1.0),
        set(GLIDE_TIME, 60.0),
        set(target_dest(0, false), 2.0),
        set(target_amount(0, false), 50.0),
        set(target_dest(3, false), 10.0),
        set(target_amount(3, false), 40.0),
    ] {
        editor.apply(command).unwrap();
    }
    sender.send_update(compiler.sync(editor.document()));
    let mut seen = 0;
    for round in 0..2_000u64 {
        let step = (round / 20) as usize;
        let command = match round % 20 {
            1 => Some(set(LFO_WAVE, (step % 7) as f64)),
            2 => Some(set(LFO_RANGE, (step % 3) as f64)),
            3 => Some(set(LFO_RATE, (step % 5) as f64 * 25.0)),
            4 => Some(set(LFO_AMOUNT_A, (step % 5) as f64 * 50.0 - 100.0)),
            5 => Some(set(LFO_DEST_B, (step % 23) as f64)),
            6 => Some(set(op::FILTER_TYPE, (step % 10) as f64)),
            7 => Some(set(op::FILTER_CIRCUIT, (step % 5) as f64)),
            8 => Some(set(op::SHAPER_TYPE, (step % 5) as f64)),
            9 => Some(set(SPREAD, (step % 4) as f64 * 33.0)),
            10 => Some(set(target_dest(step % 5, step % 2 == 0), (step % 23) as f64)),
            11 => Some(set(osc(0, ENV_LOOP), (step % 5) as f64)),
            // Partials drawn while notes play swap a table in.
            12 => Some(Command::SetPartials {
                device: id,
                osc: (step % 4) as u8,
                levels: (0..16 << (step % 3))
                    .map(|k| 1.0 / (1.0 + k as f32 + step as f32 % 3.0))
                    .collect(),
            }),
            13 => Some(set(osc(0, OSC_REPEAT), (step % 5) as f64 * 25.0)),
            14 => Some(set(osc(0, OSC_REPEAT_ON), (step % 2) as f64)),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        if round % 400 == 399 {
            sender.transport(TransportCommand::Locate(Ticks::ZERO));
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(128);
        });
        seen = seen.max(monitor.state().counters.events_applied);
        if round % 50 == 0 {
            collector.collect();
        }
    }
    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert!(seen > 100);
}
