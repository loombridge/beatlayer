//! A drum rack's audio-thread work under RealtimeSanitizer (WP-5.6): notes
//! routed to the chains that answer them at their Play notes, choke groups
//! ending the others' notes on the frame of the note that chokes them,
//! Preview held and released, sends feeding return chains whose levels
//! glide, a return chain sent to a return track, and program swaps as
//! return chains come and go with every node migrating. RTSan stops the
//! process at the first allocation, free, lock, or blocking call in any of
//! it, and the allocation trap, installed as in every Beatlayer test binary,
//! fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, DELAY, DELAY_FRAMES, GAIN, GAIN_PARAM, Probe};
use bl_engine::{
    Collector, Compiler, EngineConfig, NodeKey, ParamEvent, TransportCommand, channel,
};
use bl_model::{
    Chain, ChainId, Command, Device, DeviceKind, DeviceRole, Editor, RackKind, Receive, TrackId,
    TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

fn track(editor: &mut Editor, kind: TrackKind) -> TrackId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    id
}

fn native(
    editor: &mut Editor,
    device_type: &str,
    role: DeviceRole,
    params: &[(u32, f64)],
) -> Device {
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    )
    .with_role(role);
    for &(param, value) in params {
        device.params.insert(bl_model::ParamId(param), value);
    }
    device
}

#[test]
fn a_kit_with_chokes_and_returns_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let probe = std::sync::Arc::new(Probe::default());
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, testing::registry(&probe), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let midi = track(&mut editor, TrackKind::Midi);
    let reverb_track = track(&mut editor, TrackKind::Return);
    // Four pads from C1, each an instrument and a DC source, the first two
    // in one choke group, with a return chain after them.
    let mut kit = Device::rack(editor.new_id(), RackKind::Drum);
    for n in 0..4u8 {
        let mut pad = Chain::for_pad(editor.new_id(), format!("Pad {n}"), 36 + n);
        pad.devices = vec![
            native(
                &mut editor,
                testing::INSTRUMENT,
                DeviceRole::Instrument,
                &[],
            ),
            native(
                &mut editor,
                testing::DC,
                DeviceRole::AudioEffect,
                &[(0, 0.1)],
            ),
        ];
        pad.choke = (n < 2).then_some(1);
        kit.chains.push(pad);
    }
    let pads: Vec<ChainId> = kit.chains.iter().map(|c| c.id).collect();
    let mut ret = Chain::new_return(editor.new_id(), "Return");
    ret.devices = vec![
        native(
            &mut editor,
            DELAY,
            DeviceRole::AudioEffect,
            &[(DELAY_FRAMES.0, 300.0)],
        ),
        native(
            &mut editor,
            GAIN,
            DeviceRole::AudioEffect,
            &[(GAIN_PARAM.0, 0.5)],
        ),
    ];
    let ret_id = ret.id;
    kit.chains.push(ret);
    editor
        .apply(Command::InsertDevice {
            track: midi,
            index: 0,
            device: kit,
        })
        .unwrap();
    for pad in &pads {
        editor
            .apply(Command::SetChainSend {
                chain: *pad,
                to: ret_id,
                level: 0.5,
            })
            .unwrap();
    }
    // Sixteen notes over four pads, the first two choking each other.
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: midi,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 8),
                bl_model::ClipData::Midi(bl_model::MidiData::new((0..32u8).map(|n| {
                    bl_model::Note::new(
                        36 + n % 4,
                        Ticks(TICKS_PER_QUARTER / 4 * i64::from(n)),
                        Ticks(TICKS_PER_QUARTER / 2),
                        100,
                    )
                }))),
            ),
        })
        .unwrap();

    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    let mut extra: Option<ChainId> = None;
    for round in 0..2_000u64 {
        let command = match round % 40 {
            0 => Some(Command::SetChainSend {
                chain: pads[(round / 40) as usize % 4],
                to: ret_id,
                level: (round % 7) as f64 / 6.0,
            }),
            5 => Some(Command::SetChainReceive {
                chain: pads[2],
                receive: if round % 80 == 5 {
                    Receive::AllNotes
                } else {
                    Receive::Note(38)
                },
            }),
            9 => Some(Command::SetChainPlay {
                chain: pads[3],
                play: 55 + (round % 12) as u8,
            }),
            13 => Some(Command::SetChainChoke {
                chain: pads[1],
                choke: (round % 80 != 13).then_some(1),
            }),
            17 => Some(Command::SetChainSolo {
                chain: pads[0],
                solo: round % 160 == 17,
            }),
            21 => Some(Command::SetChainAudioTo {
                chain: ret_id,
                track: (round % 80 == 21).then_some(reverb_track),
            }),
            // Now and then a structural edit: a return chain made or taken
            // away, a program swap in which every node migrates.
            30 if extra.is_none() => {
                let id: ChainId = editor.new_id();
                extra = Some(id);
                Some(Command::AddChain {
                    rack: editor
                        .document()
                        .track(midi)
                        .unwrap()
                        .devices()
                        .next()
                        .unwrap()
                        .id,
                    index: 5,
                    chain: Chain::new_return(id, "Extra"),
                })
            }
            30 => extra.take().map(|chain| Command::RemoveChain { chain }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
        }
        // A Preview button held for a few blocks at a time.
        if round % 25 == 0 {
            sender.send_param(ParamEvent::now(
                NodeKey::ChainNotes(pads[round as usize / 25 % 4]),
                bl_engine::drum::param::PREVIEW,
                1.0,
            ));
        } else if round % 25 == 4 {
            for pad in &pads {
                sender.send_param(ParamEvent::now(
                    NodeKey::ChainNotes(*pad),
                    bl_engine::drum::param::PREVIEW,
                    0.0,
                ));
            }
        }
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(256);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.swaps, programs);
    assert!(programs > 5, "{programs} programs");
    eprintln!(
        "2,000 blocks of a kit under RealtimeSanitizer: {} parameter changes, {} swaps, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
