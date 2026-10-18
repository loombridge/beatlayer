//! Warped playback's audio-thread work under RealtimeSanitizer (WP-4.2):
//! warped clips read through their tables in Re-Pitch at both interpolators,
//! looped and not, under a tempo ramp; warp edits, mode and setting changes,
//! HiQ and RAM switches sent as the tracks' new clip lists; locates into the
//! middle of them; and program swaps from tempo changes. RTSan stops the
//! process at the first allocation, free, lock, or blocking call in any of
//! it, and the allocation trap, installed as in every Beatlayer test binary,
//! fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    AudioData, BeatsUpdate, Clip, ClipData, Command, ContentHash, Editor, TrackKind, WarpMarker,
    WarpMode,
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
fn warped_playback_passes_under_rtsan() {
    let mut media = MediaMap::new();
    media.insert(ContentHash([1; 32]), file(96_000, 0.001));
    media.insert(ContentHash([2; 32]), file(12_000, 0.001));
    let registry = Registry::new().with_media(Arc::new(media));
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    // A ramp, so the tables take many pieces.
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::from_breakpoints([
                (Ticks::ZERO, Bpm::new(70.0).unwrap()),
                (Ticks::WHOLE * 2, Bpm::new(170.0).unwrap()),
            ])
            .unwrap(),
        })
        .unwrap();
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
    let warped = |media: u8, tempo: f64, looping: bool, high_quality: bool| AudioData {
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: 0.5,
                beat: Ticks::QUARTER,
            },
        ],
        tempo: Some(tempo),
        warp_mode: WarpMode::RePitch,
        high_quality,
        looping,
        loop_start: Ticks::QUARTER.get() as f64 / 2.0,
        loop_end: looping.then_some(Ticks::QUARTER.get() as f64 * 3.0),
        ..AudioData::new(ContentHash([media; 32])).with_edge_fades()
    };
    let mut clips = Vec::new();
    for (track, start, audio) in [
        (tracks[0], Ticks::ZERO, warped(1, 100.0, true, true)),
        (tracks[1], Ticks(12_345), warped(1, 130.0, false, false)),
        (tracks[1], Ticks::QUARTER, warped(2, 90.0, true, false)),
        (tracks[2], Ticks::ZERO, warped(1, 120.0, false, true)),
    ] {
        let id = editor.new_id();
        let clip = Clip::new(id, start, Ticks::WHOLE * 2, ClipData::Audio(audio));
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        clips.push(id);
    }
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..2_000u64 {
        let clip = clips[(round % 4) as usize];
        // A warp edit every few blocks: each sends its track a new list.
        let command = match round % 24 {
            0 => Some(Command::SetClipHighQuality {
                clip,
                high_quality: round % 48 == 0,
            }),
            3 => Some(Command::SetClipRamMode {
                clip,
                ram: round % 48 == 3,
            }),
            6 => Some(Command::SetClipWarpMode {
                clip,
                mode: if round % 48 == 6 {
                    WarpMode::Complex
                } else {
                    WarpMode::RePitch
                },
            }),
            9 => Some(Command::SetSegmentBpm {
                clip,
                marker: Some(0),
                bpm: 80.0 + (round % 60) as f64,
            }),
            12 => Some(Command::InsertWarpMarker {
                clips: vec![clip],
                beat: Some(Ticks(100_000 + (round as i64 % 7) * 90_000)),
                time: None,
                pin_neighbours: false,
                transients: None,
            }),
            15 => Some(Command::SetClipWarpSettings {
                clip,
                beats: Some(BeatsUpdate {
                    transient_envelope: Some((round % 101) as u32),
                    ..BeatsUpdate::default()
                }),
                tones: None,
                texture: None,
                complex_pro: None,
            }),
            18 => Some(Command::SetClipGain {
                clip,
                gain: (round % 7) as f64 / 6.0,
            }),
            // Now and then a tempo change: a program swap.
            21 if round % 240 == 21 => Some(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(100.0 + (round % 400) as f64 / 10.0).unwrap()),
            }),
            _ => None,
        };
        if let Some(command) = command {
            // A marker that lands on or past a neighbour is refused, and
            // changes nothing.
            if editor.apply(command).is_ok() {
                let update = compiler.sync(editor.document());
                programs += u64::from(update.program.is_some());
                sender.send_update(update);
            }
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
        "2,000 blocks of warped playback under RealtimeSanitizer: {} clip lists, {} swaps, peak \
         engine load {:.1}%",
        state.counters.data_delivered,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
