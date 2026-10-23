//! Clip playback's audio-thread work under RealtimeSanitizer (WP-1.4): clip
//! players reading their files through looped and unlooped clips with gain
//! and fades, overlapping on one track; new clip lists swapped in at block
//! boundaries as clips are made, edited, moved, and deleted, with the old
//! lists (and the last references to their media) let go of on the audio
//! thread for the collector to free; locates into the middle of clips; and
//! program swaps from tempo changes, with the players migrating; and a take
//! lane auditioned and let go of again as the clips play, which swaps a
//! track's clip list for the lane's and back (the comping spec, R14). RTSan stops
//! the process at the first allocation, free, lock, or blocking call in any
//! of it, and the allocation trap, installed as in every Beatlayer test
//! binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    AudioData, Breakpoint, Clip, ClipData, Command, ContentHash, Device, DeviceKind, Editor,
    MidiData, Note, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A stereo file of `frames` frames at 48 kHz, quiet.
fn file(frames: usize, level: f32) -> Arc<SampleData> {
    let left: Vec<f32> = (0..frames)
        .map(|i| if i % 100 < 50 { level } else { -level })
        .collect();
    let right = left.iter().map(|s| s * 0.5).collect();
    Arc::new(SampleData::new(48_000, vec![left, right]).unwrap())
}

#[test]
fn clip_playback_passes_under_rtsan() {
    let mut media = MediaMap::new();
    media.insert(ContentHash([1; 32]), file(48_000, 0.001));
    media.insert(ContentHash([2; 32]), file(12_000, 0.001));
    let registry = Registry::new().with_media(Arc::new(media));
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let mut tracks = Vec::new();
    for index in 0..3 {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index,
            })
            .unwrap();
        tracks.push(id);
    }
    // A looped clip with fades, and two overlapping clips, one a short file.
    let looped = AudioData {
        start: 0.05,
        looping: true,
        loop_start: 0.1,
        loop_end: Some(0.13),
        gain: 0.5,
        ..AudioData::new(ContentHash([1; 32])).with_edge_fades()
    };
    let mut clips = Vec::new();
    for (track, start, audio) in [
        (tracks[0], Ticks::ZERO, looped),
        (
            tracks[1],
            Ticks(12_345),
            AudioData::new(ContentHash([1; 32])),
        ),
        (
            tracks[1],
            Ticks::QUARTER,
            AudioData::new(ContentHash([2; 32])),
        ),
    ] {
        let id = editor.new_id();
        let clip = Clip::new(id, start, Ticks::WHOLE * 2, ClipData::Audio(audio));
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        clips.push(id);
    }
    // A take lane on the first track, to audition now and then.
    let lane: bl_model::TakeLaneId = editor.new_id();
    editor
        .apply(Command::AddTakeLane {
            track: tracks[0],
            lane: bl_model::NewTakeLane {
                id: Some(lane),
                name: "Take 1".to_owned(),
                clips: Vec::new(),
            },
            index: None,
        })
        .unwrap();
    let take = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE * 2,
        ClipData::Audio(AudioData::new(ContentHash([2; 32]))),
    );
    editor
        .apply(Command::CreateTakeClip { lane, clip: take })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..2_000u64 {
        let clip = clips[(round % 3) as usize];
        // A clip edit every few blocks: each sends its track a new list.
        let command = match round % 20 {
            0 => Some(Command::SetClipGain {
                clip,
                gain: (round % 7) as f64 / 6.0,
            }),
            4 => Some(Command::SetClipFades {
                clip,
                fade_in: (round % 5) as f64 * 0.01,
                fade_out: 0.004,
                fade_in_curve: None,
                fade_out_curve: None,
            }),
            8 => Some(Command::MoveClip {
                clip,
                track: tracks[(round % 2) as usize + 1],
                start: Ticks::QUARTER * (round % 4) as i64,
            }),
            // The lane is auditioned for a stretch, then let go of.
            2 => Some(Command::SetAudition {
                lane,
                on: round % 40 == 2,
            }),
            12 => Some(Command::SetClipMute {
                clip,
                mute: round % 40 == 12,
            }),
            // Now and then a tempo change: a program swap.
            16 if round % 200 == 16 => Some(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(100.0 + (round % 400) as f64 / 10.0).unwrap()),
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
        }
        // Jump into the middle of the clips now and then.
        if round % 100 == 50 {
            sender.transport(TransportCommand::Locate(Ticks(98_765 * (round % 7) as i64)));
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
    assert_eq!(state.counters.data_unrouted, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.swaps, programs);
    assert!(programs > 5, "{programs} programs");
    assert!(
        state.counters.data_delivered > 200,
        "{} lists",
        state.counters.data_delivered
    );
    let heard = tracks
        .iter()
        .filter_map(|&t| state.meter(t))
        .any(|[left, _]| left > 0.0);
    assert!(heard, "the clips play");
    eprintln!(
        "2,000 blocks of clip playback under RealtimeSanitizer: {} clip lists, {} swaps, peak \
         engine load {:.1}%",
        state.counters.data_delivered,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}

// Spec: audio-clips R31. Crossfaded clips play on past their edges, from the
// sources they keep, while their crossfades are made, changed, ended and moved
// and the clips edited around them: nothing allocates, frees or locks.
#[test]
fn crossfaded_clip_playback_passes_under_rtsan() {
    let mut media = MediaMap::new();
    media.insert(ContentHash([1; 32]), file(96_000, 0.001));
    media.insert(ContentHash([2; 32]), file(96_000, 0.002));
    let registry = Registry::new().with_media(Arc::new(media));
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
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
    // Two clips that meet, the later starting half a second into its file so
    // that it can play on before its start.
    let mut clips = Vec::new();
    for (start, audio) in [
        (Ticks::ZERO, AudioData::new(ContentHash([1; 32]))),
        (
            Ticks::WHOLE,
            AudioData {
                start: 0.5,
                ..AudioData::new(ContentHash([2; 32]))
            },
        ),
    ] {
        let id = editor.new_id();
        let clip = Clip::new(id, start, Ticks::WHOLE, ClipData::Audio(audio));
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        clips.push(id);
    }
    editor
        .apply(Command::SetCrossfade {
            track,
            boundary: Ticks::WHOLE,
            before: 0.01,
            after: 0.02,
            curve: 0.0,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    for round in 0..2_000u64 {
        // An edit every few blocks, most of them refused or no-ops once the
        // clips have moved: the ones that land send the track a new list.
        let command = match round % 20 {
            0 => Some(Command::SetCrossfade {
                track,
                boundary: Ticks::WHOLE,
                before: (round % 5) as f64 * 0.005,
                after: 0.005 + (round % 3) as f64 * 0.01,
                curve: ((round % 7) as f64 - 3.0) / 3.0,
            }),
            5 => Some(Command::DeleteCrossfade {
                track,
                boundary: Ticks::WHOLE,
            }),
            10 => Some(Command::SetClipLength {
                clip: clips[0],
                length: Ticks::WHOLE - Ticks::QUARTER * (round % 3) as i64 / 4,
            }),
            15 => Some(Command::MoveClip {
                clip: clips[1],
                track,
                start: Ticks::WHOLE - Ticks::QUARTER * (round % 3) as i64 / 4,
            }),
            _ => None,
        };
        if let Some(command) = command {
            // A refused edit changes nothing.
            if editor.apply(command).is_ok() {
                sender.send_update(compiler.sync(editor.document()));
            }
        }
        // Into the middle of a span now and then.
        if round % 100 == 50 {
            sender.transport(TransportCommand::Locate(Ticks::WHOLE - Ticks(7_000)));
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
    assert_eq!(state.counters.data_unrouted, 0);
    assert!(state.counters.data_delivered > 100);
    assert!(state.meter(track).is_some_and(|[left, _]| left > 0.0));
}

/// Notes with Pitch, Slide and Pressure envelopes (WP-6.17; the MPE spec, R5
/// and R8): steps, slopes and curves played to a test instrument through
/// looped and unlooped clips, with the notes edited, the clip moved, locates
/// into the middle of notes and tempo changes while it plays.
#[test]
fn midi_clip_expressions_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(
        config,
        testing::registry(&Arc::new(Probe::default())),
        collector.handle(),
    );
    let (mut sender, mut processor, _monitor) = channel(&config);

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
    let instrument = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                instrument,
                "",
                DeviceKind::Native {
                    device_type: testing::INSTRUMENT.to_owned(),
                },
            ),
        })
        .unwrap();
    let point = |at: i64, value: f64, bend: f64| Breakpoint {
        at: Ticks(at),
        value,
        bend,
    };
    let beat = Ticks::QUARTER.get();
    let note = |key: u8, start: i64, length: i64| Note {
        pitch: vec![
            point(0, 0.0, 0.0),
            point(length / 2, 12.0, 0.5),
            point(length, -7.0, 0.0),
        ],
        slide: vec![
            point(0, 10.0, 0.0),
            point(length / 4 - 1, 10.0, 0.0),
            point(length / 4, 100.0, 0.0),
        ],
        pressure: vec![point(length / 3, 64.0, -0.5)],
        ..Note::new(key, Ticks(start), Ticks(length), 100)
    };
    let notes = [
        note(60, 0, beat),
        note(64, 0, 2 * beat),
        note(67, beat / 2, 3 * beat),
        note(72, 4 * beat, beat),
    ];
    let clip = editor.new_id();
    let mut data = MidiData::new(notes);
    data.looping = true;
    data.loop_end = Some(Ticks(4 * beat));
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(clip, Ticks::ZERO, Ticks(16 * beat), ClipData::Midi(data)),
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());
    sender.transport(TransportCommand::Play);

    for round in 0..2_000u64 {
        let command = match round % 40 {
            // Edit the notes, so each block's list is new.
            0 => {
                let mut changed = MidiData::new([
                    note(60 + (round % 12) as u8, 0, beat),
                    note(64, beat / 4, 2 * beat),
                ]);
                changed.looping = round % 80 == 0;
                changed.loop_end = Some(Ticks(4 * beat));
                Some(Command::SetClipData {
                    clip,
                    data: ClipData::Midi(changed),
                })
            }
            20 => Some(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(90.0 + (round % 600) as f64 / 10.0).unwrap()),
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        if round % 100 == 50 {
            sender.transport(TransportCommand::Locate(Ticks(
                beat / 3 * (round % 11) as i64,
            )));
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(if round % 3 == 0 { 64 } else { 256 });
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
}
