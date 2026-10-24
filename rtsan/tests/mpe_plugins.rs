//! MPE to plug-ins and MIDI ports on the audio thread under
//! RealtimeSanitizer (WP-6.17b; the MPE spec, R21): a clip's overlapping
//! notes, with Pitch, Slide and Pressure envelopes, reach a CLAP plug-in with
//! MPE Mode on as note expressions, and leave a track's MIDI To for a port
//! laid out as MPE MIDI, in each of the zones. RTSan stops the process at the
//! first allocation, free, lock, or blocking call on the audio thread, and
//! the allocation trap fails the test at any heap use it sees. The settings
//! change mid-run, so the zone opens again and the notes held end first.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{Collector, Compiler, EngineConfig, NoInput, TransportCommand, channel};
use bl_model::{
    Breakpoint, Clip, ClipData, Command, Device, DeviceKind, DeviceRole, Editor, MidiData, MidiTo,
    MpeMode, MpeSettings, MpeZone, Note, PluginFormat, PluginMpe, TrackKind,
};
use bl_plugin_host::clap::ClapHost;

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);

fn point(at: Ticks, value: f64) -> Breakpoint {
    Breakpoint {
        at,
        value,
        bend: 0.0,
    }
}

/// Two overlapping notes carrying all three envelopes, looping every bar.
fn clip() -> MidiData {
    let mut a = Note::new(60, Ticks::ZERO, BEAT * 2, 100);
    a.pitch = vec![point(Ticks::ZERO, 0.0), point(BEAT, 12.0)];
    a.slide = vec![point(Ticks::ZERO, 127.0), point(BEAT, 20.0)];
    a.pressure = vec![point(Ticks::ZERO, 10.0), point(BEAT * 2, 120.0)];
    let mut b = Note::new(64, BEAT, BEAT * 2, 90);
    b.pitch = vec![point(Ticks::ZERO, -12.0), point(BEAT, 5.0)];
    b.slide = vec![point(Ticks::ZERO, 0.0)];
    b.pressure = vec![point(Ticks::ZERO, 63.5)];
    MidiData::new([a, b])
}

/// A song with one MIDI track holding the clip, and, in `setup`, whatever
/// reads it; runs it for `rounds` blocks, calling `edit` every 40 blocks.
fn run(
    setup: impl FnOnce(&mut Editor, bl_model::TrackId, &mut Vec<Box<dyn std::any::Any>>),
    mut edit: impl FnMut(&mut Editor, bl_model::TrackId, u32),
    registry: bl_engine::Registry,
    mut output: Option<rtrb::Consumer<bl_core::midi::MidiOutputEvent>>,
    out: Option<rtrb::Producer<bl_core::midi::MidiOutputEvent>>,
) -> usize {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);
    if let Some(out) = out {
        processor.set_midi_output(out);
    }
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
    let clip_id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(clip_id, Ticks::ZERO, BEAT * 8, ClipData::Midi(clip())),
        })
        .unwrap();
    let mut keep = Vec::new();
    setup(&mut editor, track, &mut keep);
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());
    sender.transport(TransportCommand::Play);
    let mut messages = 0;
    for round in 0..400u32 {
        if round % 40 == 20 {
            edit(&mut editor, track, round / 40);
            sender.send_update(compiler.sync(editor.document()));
            assert!(sender.flush());
        }
        bl_rt::rt_test!({
            processor.process_at(256, &NoInput, std::time::Instant::now());
        });
        if round % 25 == 0 {
            collector.collect();
        }
        if let Some(output) = output.as_mut() {
            while output.pop().is_ok() {
                messages += 1;
            }
        }
    }
    messages
}

#[test]
fn mpe_midi_out_of_a_tracks_midi_to_passes_under_rtsan() {
    let (out, output) = rtrb::RingBuffer::new(1 << 16);
    let settings = [
        MpeSettings::default(),
        MpeSettings {
            zone: MpeZone::Upper,
            channels: 5,
            ..MpeSettings::default()
        },
        MpeSettings {
            mode: MpeMode::MultiChannel,
            first: 2,
            last: 6,
            ..MpeSettings::default()
        },
    ];
    let to = |settings: MpeSettings| {
        Some(MidiTo::Port {
            name: "Synth".to_owned(),
            channel: 1,
            mpe: Some(settings),
        })
    };
    let messages = run(
        |editor, track, _| {
            editor
                .apply(Command::SetMidiOutput {
                    track,
                    to: to(settings[0]),
                })
                .unwrap();
        },
        // Each edit changes the settings: the notes held end, and the zone
        // opens again.
        |editor, track, n| {
            editor
                .apply(Command::SetMidiOutput {
                    track,
                    to: to(settings[n as usize % 3]),
                })
                .unwrap();
        },
        testing::registry(&Arc::new(Probe::default())),
        Some(output),
        Some(out),
    );
    assert!(messages > 500, "{messages} messages under RTSan");
    eprintln!("400 blocks of MPE MIDI under RealtimeSanitizer: {messages} messages");
}

#[test]
fn mpe_clap_note_expressions_pass_under_rtsan() {
    let host = ClapHost::new();
    host.load(bl_test_clap::bundle())
        .expect("the test plug-in loads");
    let mut registry = testing::registry(&Arc::new(Probe::default()));
    registry.add(host.factory());
    run(
        |editor, track, _| {
            let id = editor.new_id();
            let mut plugin = Device::new(
                id,
                "Test Gain Echo",
                DeviceKind::Plugin {
                    format: PluginFormat::Clap,
                    plugin_id: bl_test_clap::PLUGIN_ID.to_owned(),
                },
            )
            .with_role(DeviceRole::MidiEffect);
            plugin.mpe = PluginMpe {
                on: true,
                settings: MpeSettings::default(),
            };
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device: plugin,
                })
                .unwrap();
            let instrument = editor.new_id();
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 1,
                    device: Device::new(
                        instrument,
                        "",
                        DeviceKind::Native {
                            device_type: testing::INSTRUMENT.to_owned(),
                        },
                    ),
                })
                .unwrap();
        },
        // MPE Mode off and on again, mid-run.
        |editor, track, n| {
            let id = editor
                .document()
                .track(track)
                .unwrap()
                .devices()
                .next()
                .unwrap()
                .id;
            editor
                .apply(Command::SetPluginMpeMode {
                    device: id,
                    on: n % 2 == 1,
                })
                .unwrap();
        },
        registry,
        None,
        None,
    );
}
