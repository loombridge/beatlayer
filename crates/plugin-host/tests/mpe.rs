//! MPE to plug-ins (WP-6.17b; the MPE spec, R14 to R16, R20 and R21): a
//! clip's two overlapping notes, each with Pitch, Slide and Pressure
//! envelopes, play through the engine into a CLAP or a VST3 plug-in, which
//! gets their expressions as its format takes them. Every block runs as a real-time
//! section under the allocation trap, in blocks of 1, 64 and 512 frames.
//!
//! Spec: mpe R21.

use crate::common;
use std::path::PathBuf;
use std::sync::Arc;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, NoteExpression, NoteKind, Offline, TransportCommand};
use bl_model::{
    Breakpoint, Clip, ClipData, Command, DeviceRole, Editor, MidiData, MpeMode, MpeSettings,
    MpeZone, Note, PluginMpe, TrackKind,
};
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::vst3::Vst3Host;
use common::{RATE, bundle, device_with_role, vst3_bundle, vst3_device_with_role};

const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);

/// At 120 BPM and 48 kHz a quarter note is 24,000 frames.
const BEAT_FRAMES: i64 = 24_000;

const BLOCKS: [usize; 3] = [1, 64, 512];

fn point(at: Ticks, value: f64) -> Breakpoint {
    Breakpoint {
        at,
        value,
        bend: 0.0,
    }
}

/// Two overlapping notes: A, key 60, from the start for a beat and a half,
/// and B, key 64, from half a beat in for a beat. A bends up an octave over
/// its first half beat, holds a full slide and a full pressure that drops to
/// none as the bend arrives; B holds a bend down an octave, no slide and
/// half pressure.
fn clip() -> MidiData {
    let half = BEAT / 2;
    let mut a = Note::new(60, Ticks::ZERO, BEAT + half, 100);
    a.pitch = vec![point(Ticks::ZERO, 0.0), point(half, 12.0)];
    a.slide = vec![point(Ticks::ZERO, 127.0)];
    a.pressure = vec![
        point(Ticks::ZERO, 127.0),
        point(half - Ticks(1), 127.0),
        point(half, 0.0),
    ];
    let mut b = Note::new(64, half, BEAT, 90);
    b.pitch = vec![point(Ticks::ZERO, -12.0)];
    b.slide = vec![point(Ticks::ZERO, 0.0)];
    b.pressure = vec![point(Ticks::ZERO, 63.5)];
    MidiData::new([a, b])
}

/// Which format a scenario plays through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Clap,
    Vst3,
}

/// One line of the test plug-in's log.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    frame: i64,
    what: String,
    channel: i32,
    key: i32,
    id: i32,
    value: f64,
}

fn log_path(format: Format) -> PathBuf {
    std::env::temp_dir().join(format!("bl-test-{format:?}-{}.log", std::process::id()))
}

fn read_log(format: Format) -> Vec<Line> {
    let text = std::fs::read_to_string(log_path(format)).unwrap_or_default();
    let mut lines: Vec<Line> = text
        .lines()
        .map(|line| {
            let mut fields = line.split(' ');
            let mut frame = 0;
            let (mut what, mut channel, mut key, mut id, mut value) =
                (String::new(), -1, -1, -1, 0.0);
            while let Some(field) = fields.next() {
                match field.split_once('=') {
                    Some(("frame", v)) => frame = v.parse().unwrap(),
                    Some(("channel", v)) => channel = v.parse().unwrap(),
                    Some(("key", v)) => key = v.parse().unwrap(),
                    Some(("id", v)) => id = v.parse().unwrap(),
                    Some(("value", v)) => value = v.parse().unwrap(),
                    Some(_) => {}
                    None => what = field.to_owned(),
                }
            }
            Line {
                frame,
                what,
                channel,
                key,
                id,
                value,
            }
        })
        .collect();
    // A VST3 plug-in reads a block's parameter changes before its events,
    // so the order it logs them in follows the block, not the frames.
    if format == Format::Clap {
        return lines;
    }
    lines.sort_by(|a, b| {
        (a.frame, &a.what, a.channel, a.key, a.id, a.value.to_bits()).cmp(&(
            b.frame,
            &b.what,
            b.channel,
            b.key,
            b.id,
            b.value.to_bits(),
        ))
    });
    lines
}

/// Plays the clip to its second beat's end through the plug-in, a device of
/// `role` with `mpe`, and returns what the plug-in logged and what a test
/// instrument after it heard. `block` frames at a time, with no heap use.
fn play(
    format: Format,
    block: usize,
    mpe: PluginMpe,
    role: DeviceRole,
) -> (Vec<Line>, Vec<(i64, NoteKind, u8)>) {
    let _ = std::fs::remove_file(log_path(format));
    // SAFETY: the plug-in reads this when it is activated, and each test
    // runs in a process of its own.
    unsafe {
        std::env::set_var(
            match format {
                Format::Clap => bl_test_clap::LOG_VAR,
                Format::Vst3 => bl_test_vst3::LOG_VAR,
            },
            log_path(format),
        );
    }
    let probe = Arc::new(Probe::default());
    let mut registry = testing::registry(&probe);
    // The host lives until the scenario ends, as the app's does.
    let _clap;
    let _vst3;
    let plugin = match format {
        Format::Clap => {
            let host = ClapHost::new();
            host.load(bundle()).expect("the test plug-in loads");
            registry.add(host.factory());
            _clap = host;
            device_with_role(1, &[(common::GAIN, 1.0)], role)
        }
        Format::Vst3 => {
            let host = Vst3Host::new();
            host.load(vst3_bundle()).expect("the test plug-in loads");
            registry.add(host.factory());
            _vst3 = host;
            vst3_device_with_role(1, &[], role)
        }
    };
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);

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
    let id = editor.new_id();
    let mut plugin = plugin;
    plugin.id = id;
    plugin.mpe = mpe;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: plugin,
        })
        .unwrap();
    if role == DeviceRole::MidiEffect {
        let instrument = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 1,
                device: bl_model::Device::new(
                    instrument,
                    "",
                    bl_model::DeviceKind::Native {
                        device_type: testing::INSTRUMENT.to_owned(),
                    },
                )
                .with_role(DeviceRole::Instrument),
            })
            .unwrap();
    }
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(clip, Ticks::ZERO, BEAT * 4, ClipData::Midi(clip_data())),
        })
        .unwrap();
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    offline.render(BEAT_FRAMES as u64 * 2, |_| {});
    assert_eq!(offline.heap_operations(), 0, "block size {block}");
    drop(offline);
    let heard = probe
        .recorded_notes()
        .into_iter()
        .map(|heard| (heard.frame, heard.event.kind, heard.event.key))
        .collect();
    (read_log(format), heard)
}

fn clip_data() -> MidiData {
    clip()
}

fn expressions(lines: &[Line], what: &str, key: i32) -> Vec<(i64, f64)> {
    lines
        .iter()
        .filter(|line| line.what == what && line.key == key)
        .map(|line| (line.frame, line.value))
        .collect()
}

fn on(lines: &[Line], key: i32) -> &Line {
    lines
        .iter()
        .find(|line| line.what == "on" && line.key == key)
        .unwrap_or_else(|| panic!("no note-on for {key}: {lines:?}"))
}

/// Runs the scenario in every block size, which must log the same.
fn in_every_block_size(
    format: Format,
    mpe: PluginMpe,
    role: DeviceRole,
) -> (Vec<Line>, Vec<(i64, NoteKind, u8)>) {
    let first = play(format, BLOCKS[0], mpe, role);
    for block in &BLOCKS[1..] {
        // Not assert_eq: a mismatch would print a log of thousands of lines.
        assert!(
            play(format, *block, mpe, role) == first,
            "{format:?}, block size {block}, plays other than block size {}",
            BLOCKS[0]
        );
    }
    first
}

const ON: PluginMpe = PluginMpe {
    on: true,
    settings: MpeSettings {
        mode: MpeMode::Zone,
        zone: MpeZone::Lower,
        channels: 15,
        first: 1,
        last: 16,
    },
};

// Spec: mpe R16; clap-hosting R22
#[test]
fn with_mpe_mode_on_a_clap_plugin_gets_note_expressions_for_each_note() {
    let (log, _) = in_every_block_size(Format::Clap, ON, DeviceRole::Instrument);
    let (a, b) = (on(&log, 60), on(&log, 64));
    assert_ne!(a.id, -1);
    assert_ne!(b.id, -1);
    assert_ne!(a.id, b.id, "each note has an ID of its own");
    assert_eq!((a.frame, b.frame), (0, BEAT_FRAMES / 2));
    let for_note = |what: &str, key: i32| expressions(&log, what, key);
    // A's tuning starts at 0 semitones on the note-on's frame and ends the
    // octave it bends up by the half beat.
    let tuning = for_note("expression:tuning", 60);
    assert_eq!(tuning.first(), Some(&(0, 0.0)));
    let (last_frame, last) = *tuning.last().unwrap();
    assert!((last - 12.0).abs() < 1e-3, "{tuning:?}");
    assert!(last_frame <= BEAT_FRAMES / 2 + 1, "{last_frame}");
    assert!(
        tuning.windows(2).all(|pair| pair[0].1 < pair[1].1),
        "a rising stream"
    );
    // A slide of 127 is brightness 1 and a pressure of 127, then none.
    assert_eq!(for_note("expression:brightness", 60), [(0, 1.0)]);
    assert_eq!(
        for_note("expression:pressure", 60),
        [(0, 1.0), (BEAT_FRAMES / 2, 0.0)]
    );
    // B: down an octave, no slide, half pressure, on its note-on's frame.
    let at = BEAT_FRAMES / 2;
    assert_eq!(for_note("expression:tuning", 64), [(at, -12.0)]);
    assert_eq!(for_note("expression:brightness", 64), [(at, 0.0)]);
    let pressure = for_note("expression:pressure", 64);
    assert_eq!(pressure.len(), 1);
    assert!((pressure[0].1 - 0.5).abs() < 1e-6, "{pressure:?}");
    // The expressions name their notes.
    for line in log.iter().filter(|l| l.what.starts_with("expression")) {
        let note = if line.key == 60 { a } else { b };
        assert_eq!(line.id, note.id, "{line:?}");
        assert_eq!(line.channel, 0);
    }
    // And no polyphonic pressure or other MIDI came with them.
    assert!(log.iter().all(|line| !line.what.starts_with("midi")));
}

// Spec: mpe R15; clap-hosting R21
#[test]
fn with_mpe_mode_off_a_clap_plugin_gets_polyphonic_key_pressure_only() {
    let (log, _) = in_every_block_size(Format::Clap, PluginMpe::default(), DeviceRole::Instrument);
    assert!(log.iter().all(|line| !line.what.starts_with("expression")));
    let midi: Vec<(i64, &str)> = log
        .iter()
        .filter(|line| line.what.starts_with("midi"))
        .map(|line| (line.frame, line.what.as_str()))
        .collect();
    // 0xA0 is polyphonic key pressure on the note's channel: A's key 60 at
    // full pressure, then none; B's key 64 at half (64 of 127).
    assert_eq!(
        midi,
        [
            (0, "midi:160,60,127"),
            (BEAT_FRAMES / 2, "midi:160,60,0"),
            (BEAT_FRAMES / 2, "midi:160,64,64"),
        ]
    );
    assert_eq!(log.iter().filter(|line| line.what == "on").count(), 2);
}

// Spec: mpe R16, R17; clap-hosting R22
#[test]
fn a_note_port_preferring_midi_mpe_gets_mpe_midi() {
    // SAFETY: the plug-in reads this when it is asked for its note ports,
    // and this test runs in a process of its own.
    unsafe { std::env::set_var(bl_test_clap::DIALECT_VAR, "midi-mpe") };
    let (log, _) = in_every_block_size(Format::Clap, ON, DeviceRole::Instrument);
    assert!(log.iter().all(|line| !line.what.starts_with("expression")));
    assert!(
        log.iter()
            .all(|line| line.what != "on" && line.what != "off")
    );
    let midi: Vec<(i64, String)> = log
        .iter()
        .map(|line| (line.frame, line.what.clone()))
        .collect();
    // The zone opens first: the configuration message on channel 1 (0xB0)
    // and a sensitivity of 48 on channels 2 to 16.
    assert_eq!(midi[0], (0, "midi:176,101,0".to_owned()));
    assert_eq!(midi[1], (0, "midi:176,100,6".to_owned()));
    assert_eq!(midi[2], (0, "midi:176,6,15".to_owned()));
    let sensitivities = midi
        .iter()
        .filter(|(_, what)| what.ends_with(",6,48"))
        .count();
    assert_eq!(sensitivities, 15);
    let notes: Vec<&(i64, String)> = midi.iter().skip(3 + 5 + 15 * 6 - 5).collect();
    // Note A takes channel 2 (0x91), B channel 3 (0x92), each with its
    // starting slide and pressure sent before its note-on.
    let first_on = notes
        .iter()
        .position(|(_, w)| w == "midi:145,60,100")
        .unwrap();
    assert_eq!(
        notes[first_on - 2..=first_on]
            .iter()
            .map(|(frame, what)| (*frame, what.as_str()))
            .collect::<Vec<_>>(),
        [
            (0, "midi:177,74,127"),
            (0, "midi:209,127,0"),
            (0, "midi:145,60,100")
        ]
    );
    assert!(
        notes
            .iter()
            .any(|(frame, what)| *frame == BEAT_FRAMES / 2 && what == "midi:146,64,90"),
        "{notes:?}"
    );
    // A's bend arrives as a stream of pitch bends on channel 2 up to +12 of
    // 48 semitones: 10,240 (0x2800), lsb 0 and msb 80.
    assert!(notes.iter().any(|(_, what)| what == "midi:225,0,80"));
}

// Spec: mpe R20; clap-hosting R23
#[test]
fn note_expressions_a_plugin_sends_out_reach_the_next_device_as_expressions() {
    let (_, heard) = in_every_block_size(Format::Clap, ON, DeviceRole::MidiEffect);
    let sent_by_a: Vec<(i64, f32)> = heard
        .iter()
        .filter(|(_, _, key)| *key == 72)
        .filter_map(|&(frame, kind, _)| match kind {
            NoteKind::Expression {
                expression: NoteExpression::Bend,
                value,
            } => Some((frame, value)),
            _ => None,
        })
        .collect();
    // The plug-in echoes each tuning an octave up; the next device hears
    // them as a bend of the semitones over 48: A's end at +12 is a quarter.
    assert_eq!(sent_by_a.first(), Some(&(0, 0.0)));
    let (_, last) = *sent_by_a.last().unwrap();
    assert!((last - 0.25).abs() < 1e-4, "{sent_by_a:?}");
    let kinds: Vec<NoteKind> = heard
        .iter()
        .filter(|(frame, _, key)| *frame == BEAT_FRAMES / 2 && *key == 76)
        .map(|&(_, kind, _)| kind)
        .collect();
    assert!(
        kinds.contains(&NoteKind::Expression {
            expression: NoteExpression::Bend,
            value: -0.25
        }),
        "{kinds:?}"
    );
    assert!(kinds.contains(&NoteKind::Expression {
        expression: NoteExpression::Slide,
        value: 0.0
    }));
}

// Spec: mpe R20; clap-hosting R23
#[test]
fn note_expressions_a_plugin_sends_out_are_dropped_while_mpe_mode_is_off() {
    let (_, heard) =
        in_every_block_size(Format::Clap, PluginMpe::default(), DeviceRole::MidiEffect);
    assert!(
        heard
            .iter()
            .all(|(_, kind, _)| !matches!(kind, NoteKind::Expression { .. })),
        "{heard:?}"
    );
}

// Spec: mpe R16; vst3-hosting R20
#[test]
fn a_vst3_controller_with_note_expression_gets_note_expression_values() {
    // SAFETY: the plug-in reads this when its controller is made, and this
    // test runs in a process of its own.
    unsafe { std::env::set_var(bl_test_vst3::EXPRESSIONS_VAR, "on") };
    let (log, _) = in_every_block_size(Format::Vst3, ON, DeviceRole::Instrument);
    let (a, b) = (on(&log, 60), on(&log, 64));
    assert_ne!(a.id, -1);
    assert_ne!(a.id, b.id);
    assert_eq!((a.frame, b.frame), (0, BEAT_FRAMES / 2));
    let tuning = format!("expression:{}", bl_test_vst3::TUNING_TYPE);
    let slide = format!("expression:{}", bl_test_vst3::BRIGHTNESS_TYPE);
    let pressure = format!("expression:{}", bl_test_vst3::PRESSURE_TYPE);
    let of = |what: &str, id: i32| -> Vec<(i64, f64)> {
        log.iter()
            .filter(|line| line.what == what && line.id == id)
            .map(|line| (line.frame, line.value))
            .collect()
    };
    // Tuning is 0.5 plus the semitones over 240: A starts at 0.5 and bends
    // up an octave, 0.55; B holds down an octave, 0.45.
    let a_tuning = of(&tuning, a.id);
    assert_eq!(a_tuning.first(), Some(&(0, 0.5)));
    assert!(
        (a_tuning.last().unwrap().1 - 0.55).abs() < 1e-6,
        "{a_tuning:?}"
    );
    let at = BEAT_FRAMES / 2;
    let b_tuning = of(&tuning, b.id);
    assert_eq!(b_tuning.len(), 1);
    assert_eq!(b_tuning[0].0, at);
    assert!((b_tuning[0].1 - 0.45).abs() < 1e-6, "{b_tuning:?}");
    // The key's Y movement is brightness, and its pressure the custom type,
    // both from 0 to 1.
    assert_eq!(of(&slide, a.id), [(0, 1.0)]);
    assert_eq!(of(&slide, b.id), [(at, 0.0)]);
    assert_eq!(of(&pressure, a.id), [(0, 1.0), (at, 0.0)]);
    let b_pressure = of(&pressure, b.id);
    assert_eq!(b_pressure.len(), 1);
    assert!((b_pressure[0].1 - 0.5).abs() < 1e-6, "{b_pressure:?}");
    // Nothing came as polyphonic pressure.
    assert!(log.iter().all(|line| line.what != "pressure"));
}

// Spec: mpe R16; vst3-hosting R20
#[test]
fn pressure_the_controller_maps_to_no_type_is_polyphonic_pressure() {
    // SAFETY: as above.
    unsafe { std::env::set_var(bl_test_vst3::EXPRESSIONS_VAR, "no-pressure") };
    let (log, _) = in_every_block_size(Format::Vst3, ON, DeviceRole::Instrument);
    let (a, b) = (on(&log, 60), on(&log, 64));
    let pressure: Vec<(i64, i32, i32, i32, f64)> = log
        .iter()
        .filter(|line| line.what == "pressure")
        .map(|line| (line.frame, line.channel, line.key, line.id, line.value))
        .collect();
    let at = BEAT_FRAMES / 2;
    assert_eq!(pressure.len(), 3);
    assert_eq!(pressure[0], (0, 0, 60, a.id, 1.0));
    assert_eq!(pressure[1], (at, 0, 60, a.id, 0.0));
    assert_eq!(
        (pressure[2].0, pressure[2].2, pressure[2].3),
        (at, 64, b.id)
    );
    assert!((pressure[2].4 - 0.5).abs() < 1e-6);
    assert!(
        log.iter()
            .any(|line| line.what == format!("expression:{}", bl_test_vst3::BRIGHTNESS_TYPE))
    );
}

// Spec: mpe R15; vst3-hosting R19
#[test]
fn with_mpe_mode_off_a_vst3_plugin_gets_polyphonic_pressure_only() {
    // SAFETY: as above.
    unsafe { std::env::set_var(bl_test_vst3::EXPRESSIONS_VAR, "on") };
    let (log, _) = in_every_block_size(Format::Vst3, PluginMpe::default(), DeviceRole::Instrument);
    assert!(log.iter().all(|line| !line.what.starts_with("expression")));
    let pressure: Vec<(i64, i32, f64)> = log
        .iter()
        .filter(|line| line.what == "pressure")
        .map(|line| (line.frame, line.key, line.value))
        .collect();
    let at = BEAT_FRAMES / 2;
    assert_eq!(pressure.len(), 3);
    assert_eq!(pressure[0], (0, 60, 1.0));
    assert_eq!(pressure[1], (at, 60, 0.0));
    assert_eq!((pressure[2].0, pressure[2].1), (at, 64));
    assert!((pressure[2].2 - 0.5).abs() < 1e-6);
}

// Spec: mpe R16, R17; vst3-hosting R20
#[test]
fn a_vst3_controller_without_note_expression_gets_mpe_through_its_midi_mapping() {
    // SAFETY: as above.
    unsafe { std::env::set_var(bl_test_vst3::MAPPING_VAR, "1") };
    let (log, _) = in_every_block_size(Format::Vst3, ON, DeviceRole::Instrument);
    assert!(log.iter().all(|line| !line.what.starts_with("expression")));
    assert!(log.iter().all(|line| line.what != "pressure"));
    // Note A is on member channel 2 (index 1), B on channel 3.
    let a = on(&log, 60);
    let b = on(&log, 64);
    assert_eq!((a.channel, a.frame), (1, 0));
    assert_eq!((b.channel, b.frame), (2, BEAT_FRAMES / 2));
    let param =
        |channel: u32, what: u32| format!("param:{}", bl_test_vst3::mapped_param(channel, what));
    let of = |what: String| -> Vec<(i64, f64)> {
        log.iter()
            .filter(|line| line.what == what)
            .map(|line| (line.frame, line.value))
            .collect()
    };
    // Channel 2's pitch bend: normalized over 14 bits, centred at the start
    // and +12 of 48 semitones (10,240 of 16,383) at the half beat.
    let bend = of(param(1, 0));
    assert!((bend[0].1 - 8192.0 / 16383.0).abs() < 1e-9, "{bend:?}");
    assert_eq!(bend[0].0, 0);
    assert!((bend.last().unwrap().1 - 10240.0 / 16383.0).abs() < 1e-9);
    // Slide as controller 74 and pressure as channel pressure, over 127.
    assert_eq!(of(param(1, 1)), [(0, 1.0)]);
    assert_eq!(of(param(1, 2)), [(0, 1.0), (BEAT_FRAMES / 2, 0.0)]);
    // B: down an octave (6,144 of 16,383), no slide, half pressure (64).
    let b_bend = of(param(2, 0));
    assert!((b_bend[0].1 - 6144.0 / 16383.0).abs() < 1e-9, "{b_bend:?}");
    assert_eq!(of(param(2, 1)), [(BEAT_FRAMES / 2, 0.0)]);
    assert_eq!(of(param(2, 2)), [(BEAT_FRAMES / 2, 64.0 / 127.0)]);
}

// Spec: mpe R20; vst3-hosting R21
#[test]
fn note_expression_values_a_vst3_plugin_sends_out_reach_the_next_device() {
    // SAFETY: as above.
    unsafe { std::env::set_var(bl_test_vst3::EXPRESSIONS_VAR, "on") };
    let (_, heard) = in_every_block_size(Format::Vst3, ON, DeviceRole::MidiEffect);
    let bends: Vec<(i64, f32)> = heard
        .iter()
        .filter(|(_, _, key)| *key == 72)
        .filter_map(|&(frame, kind, _)| match kind {
            NoteKind::Expression {
                expression: NoteExpression::Bend,
                value,
            } => Some((frame, value)),
            _ => None,
        })
        .collect();
    assert_eq!(bends.first(), Some(&(0, 0.0)));
    assert!((bends.last().unwrap().1 - 0.25).abs() < 1e-4, "{bends:?}");
    let at_b: Vec<NoteKind> = heard
        .iter()
        .filter(|(frame, _, key)| *frame == BEAT_FRAMES / 2 && *key == 76)
        .map(|&(_, kind, _)| kind)
        .collect();
    assert!(
        at_b.iter().any(|kind| matches!(
            kind,
            NoteKind::Expression { expression: NoteExpression::Bend, value } if (value + 0.25).abs() < 1e-4
        )),
        "{at_b:?}"
    );
    assert!(at_b.contains(&NoteKind::Expression {
        expression: NoteExpression::Slide,
        value: 0.0
    }));
    // Without MPE Mode they are dropped.
    let (_, off) = in_every_block_size(Format::Vst3, PluginMpe::default(), DeviceRole::MidiEffect);
    assert!(
        off.iter()
            .all(|(_, kind, _)| !matches!(kind, NoteKind::Expression { .. }))
    );
}

/// Apple's DLS synth, an Audio Unit, with MPE Mode on: two notes of one
/// chord bent apart by different Pitch envelopes sound that far apart (the
/// MPE spec, R16; Audio Unit hosting R20).
#[cfg(target_os = "macos")]
mod au {
    use super::*;
    use bl_model::{Device, DeviceKind, PluginFormat};
    use bl_plugin_host::au::AuHost;

    /// A chord of two notes, a second each: C (key 60) bent up three
    /// semitones, and E (key 64) bent down five.
    fn chord() -> MidiData {
        let mut a = Note::new(60, Ticks::ZERO, BEAT * 2, 100);
        a.pitch = vec![point(Ticks::ZERO, 3.0)];
        let mut b = Note::new(64, Ticks::ZERO, BEAT * 2, 100);
        b.pitch = vec![point(Ticks::ZERO, -5.0)];
        MidiData::new([a, b])
    }

    /// The DLS synth rendering `notes` for `frames` frames, in blocks of
    /// `block`, with MPE Mode as `mpe` says: the left channel.
    fn render(block: usize, mpe: PluginMpe, notes: MidiData, frames: u64) -> Vec<f32> {
        let host = AuHost::new();
        host.load("aumu", "dls ", "appl").expect("the DLS synth");
        let mut registry = testing::registry(&Arc::new(Probe::default()));
        registry.add(host.factory());
        let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
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
        let id = editor.new_id();
        let mut synth = Device::new(
            id,
            "DLSMusicDevice",
            DeviceKind::Plugin {
                format: PluginFormat::AudioUnit,
                plugin_id: "aumu/dls /appl".to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        synth.mpe = mpe;
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: synth,
            })
            .unwrap();
        let clip = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(clip, Ticks::ZERO, BEAT * 4, ClipData::Midi(notes)),
            })
            .unwrap();
        offline.sync(editor.document());
        offline.sender().transport(TransportCommand::Play);
        let mut left = Vec::new();
        offline.render(frames, |out| left.extend_from_slice(out.channel(0)));
        assert_eq!(offline.heap_operations(), 0, "block size {block}");
        left
    }

    /// The frequency near `expected` (within 60 cents either way) where
    /// `signal`, from its second quarter on, is strongest, found by a scan
    /// of one cent steps, in cents from `expected`.
    fn peak_cents(signal: &[f32], expected: f64) -> f64 {
        let part = &signal[signal.len() / 4..];
        let rate = f64::from(RATE.hz());
        let mut best = (f64::MIN, 0.0);
        for cents in -60..=60 {
            let hz = expected * 2f64.powf(f64::from(cents) / 1200.0);
            let w = 2.0 * std::f64::consts::PI * hz / rate;
            let (mut re, mut im) = (0.0, 0.0);
            for (n, s) in part.iter().enumerate() {
                // A Hann window keeps the neighbouring note out.
                let hann =
                    0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / part.len() as f64).cos();
                let x = f64::from(*s) * hann;
                re += x * (w * n as f64).cos();
                im += x * (w * n as f64).sin();
            }
            let power = re * re + im * im;
            if power > best.0 {
                best = (power, f64::from(cents));
            }
        }
        best.1
    }

    // Spec: mpe R16; au-hosting R20
    #[test]
    fn two_notes_bent_apart_by_their_pitch_envelopes_sound_within_five_cents_of_their_bends() {
        let on = PluginMpe {
            on: true,
            settings: MpeSettings::default(),
        };
        let signal = render(512, on, chord(), 40_000);
        assert!(signal.iter().any(|s| s.abs() > 0.01), "the chord sounds");
        let hz = |key: f64, semitones: f64| 440.0 * 2f64.powf((key - 69.0 + semitones) / 12.0);
        for (key, bend) in [(60.0, 3.0), (64.0, -5.0)] {
            let off = peak_cents(&signal, hz(key, bend));
            assert!(
                off.abs() <= 5.0,
                "key {key} bent {bend} semitones sounds {off} cents away"
            );
        }
        // The same chord with MPE Mode off isn't bent at all.
        let flat = render(512, PluginMpe::default(), chord(), 40_000);
        let off = peak_cents(&flat, hz(60.0, 3.0));
        assert!(
            off.abs() > 20.0,
            "unbent, the note is far from its bend: {off}"
        );
    }

    // Spec: mpe R17, R21; au-hosting R20. The unit is scheduled MPE MIDI's
    // messages on their frames, whatever the blocks: it renders the same.
    #[test]
    fn the_unit_renders_the_same_in_any_block_size() {
        let on = PluginMpe {
            on: true,
            settings: MpeSettings::default(),
        };
        let first = render(512, on, chord(), 12_000);
        for block in [64, 4096] {
            assert!(
                render(block, on, chord(), 12_000) == first,
                "block size {block} renders other than block size 512"
            );
        }
    }
}

// Spec: mpe R14, R16. The device's MPE Mode reaches its running node when it
// changes, with no new plug-in instance: the plug-in gets polyphonic key
// pressure until it is switched on and note expressions after.
#[test]
fn switching_mpe_mode_while_playing_changes_what_a_plugin_gets() {
    let _ = std::fs::remove_file(log_path(Format::Clap));
    // SAFETY: the plug-in reads this when it is activated, and this test
    // runs in a process of its own.
    unsafe { std::env::set_var(bl_test_clap::LOG_VAR, log_path(Format::Clap)) };
    let host = ClapHost::new();
    host.load(bundle()).expect("the test plug-in loads");
    let mut registry = testing::registry(&Arc::new(Probe::default()));
    registry.add(host.factory());
    let mut offline = Offline::new(EngineConfig::new(RATE, 128), registry);
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
    let id = editor.new_id();
    let mut plugin = device_with_role(1, &[], DeviceRole::Instrument);
    plugin.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: plugin,
        })
        .unwrap();
    // One long note whose pitch bends up 48 semitones over four beats, with
    // a pressure that rises with it.
    let mut note = Note::new(62, Ticks::ZERO, BEAT * 4, 100);
    note.pitch = vec![point(Ticks::ZERO, 0.0), point(BEAT * 4, 48.0)];
    note.pressure = vec![point(Ticks::ZERO, 0.0), point(BEAT * 4, 127.0)];
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                BEAT * 4,
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    offline.render(BEAT_FRAMES as u64, |_| {});
    editor
        .apply(Command::SetPluginMpeMode {
            device: id,
            on: true,
        })
        .unwrap();
    offline.sync(editor.document());
    offline.render(BEAT_FRAMES as u64, |_| {});
    assert_eq!(offline.heap_operations(), 0);

    let log = read_log(Format::Clap);
    let tuning: Vec<(i64, f64)> = log
        .iter()
        .filter(|line| line.what == "expression:tuning")
        .map(|line| (line.frame, line.value))
        .collect();
    assert!(!tuning.is_empty(), "note expressions came after the switch");
    assert!(
        tuning.iter().all(|(frame, _)| *frame >= BEAT_FRAMES - 128),
        "none came before it: {:?}",
        tuning.first()
    );
    // Two beats of four is 24 of 48 semitones.
    assert!((tuning.last().unwrap().1 - 24.0).abs() < 0.1, "{tuning:?}");
    // Before it, only polyphonic key pressure.
    let pressure_before = log
        .iter()
        .filter(|line| line.what.starts_with("midi:160") && line.frame < BEAT_FRAMES)
        .count();
    assert!(pressure_before > 10, "{pressure_before}");
    assert!(
        log.iter()
            .filter(|line| line.what.starts_with("midi:160"))
            .all(|line| line.frame < BEAT_FRAMES),
        "and none after"
    );
}

// Spec: mpe R17. A note held when MPE Mode is switched on, for a plug-in that
// gets MPE through its MIDI mapping, is ended: the layout knows only the
// notes it laid out.
#[test]
fn a_note_held_while_mpe_mode_goes_on_is_ended() {
    let _ = std::fs::remove_file(log_path(Format::Vst3));
    // SAFETY: the plug-in reads these when it is made and activated, and
    // this test runs in a process of its own.
    unsafe {
        std::env::set_var(bl_test_vst3::LOG_VAR, log_path(Format::Vst3));
        std::env::set_var(bl_test_vst3::MAPPING_VAR, "1");
    }
    let host = Vst3Host::new();
    host.load(vst3_bundle()).expect("the test plug-in loads");
    let mut registry = testing::registry(&Arc::new(Probe::default()));
    registry.add(host.factory());
    let mut offline = Offline::new(EngineConfig::new(RATE, 128), registry);
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
    let id = editor.new_id();
    let mut plugin = vst3_device_with_role(1, &[], DeviceRole::Instrument);
    plugin.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: plugin,
        })
        .unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                BEAT * 4,
                ClipData::Midi(MidiData::new([Note::new(62, Ticks::ZERO, BEAT * 4, 100)])),
            ),
        })
        .unwrap();
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    offline.render(BEAT_FRAMES as u64, |_| {});
    editor
        .apply(Command::SetPluginMpeMode {
            device: id,
            on: true,
        })
        .unwrap();
    offline.sync(editor.document());
    offline.render(BEAT_FRAMES as u64, |_| {});
    let log = read_log(Format::Vst3);
    let ons: Vec<_> = log.iter().filter(|l| l.what == "on").collect();
    let offs: Vec<_> = log.iter().filter(|l| l.what == "off").collect();
    assert_eq!(ons.len(), 1, "the note started once, as a plain note");
    assert_eq!(ons[0].channel, 0);
    assert_eq!(offs.len(), 1, "and was ended when MPE Mode went on");
    assert_eq!((offs[0].channel, offs[0].key), (0, 62));
    assert!(
        (offs[0].frame - BEAT_FRAMES).abs() <= 128,
        "{}",
        offs[0].frame
    );
}
