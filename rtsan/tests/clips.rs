//! Clip playback's audio-thread work under RealtimeSanitizer (WP-1.4): clip
//! players reading their files through looped and unlooped clips with gain
//! and fades, overlapping on one track; new clip lists swapped in at block
//! boundaries as clips are made, edited, moved, and deleted, with the old
//! lists (and the last references to their media) let go of on the audio
//! thread for the collector to free; locates into the middle of clips; and
//! program swaps from tempo changes, with the players migrating. RTSan stops
//! the process at the first allocation, free, lock, or blocking call in any
//! of it, and the allocation trap, installed as in every Beatlayer test
//! binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{AudioData, Clip, ClipData, Command, ContentHash, Editor, TrackKind};

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
            }),
            8 => Some(Command::MoveClip {
                clip,
                track: tracks[(round % 2) as usize + 1],
                start: Ticks::QUARTER * (round % 4) as i64,
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
