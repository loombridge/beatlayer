//! Beats' audio-thread work under RealtimeSanitizer (WP-4.4): a Beats clip
//! played as segments with its loop fills, through a tempo ramp, with the
//! playhead located into it and its mode and settings changed while it plays,
//! each sent as the track's new clip list. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    AudioData, BeatsUpdate, Clip, ClipData, Command, ContentHash, Editor, TrackKind,
    TransientLoopMode, WarpMarker, WarpMode,
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
fn beats_playback_passes_under_rtsan() {
    let media_hash = ContentHash([1; 32]);
    let mut media = MediaMap::new();
    media.insert(media_hash, file(96_000, 0.001));
    // A transient every eighth of a second, so a slower song runs each
    // segment out and its loop mode fills the rest (R15).
    media.set_transients(media_hash, (0..8).map(|n| f64::from(n) * 0.25).collect());
    let registry = Registry::new().with_media(Arc::new(media));
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    // A ramp, so the clip's table takes many pieces and its segments loop-fill
    // at a different rate along it.
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
    for index in 0..2 {
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
    let mut clips = Vec::new();
    for (track, start, envelope, loop_mode) in [
        (
            tracks[0],
            Ticks::ZERO,
            100_u32,
            TransientLoopMode::LoopForward,
        ),
        (
            tracks[1],
            Ticks::QUARTER,
            40_u32,
            TransientLoopMode::LoopBackAndForth,
        ),
    ] {
        let mut audio = AudioData {
            warp: true,
            warp_markers: vec![
                WarpMarker {
                    time: 0.0,
                    beat: Ticks::ZERO,
                },
                WarpMarker {
                    time: 2.0,
                    beat: Ticks::QUARTER * 4,
                },
            ],
            tempo: Some(120.0),
            warp_mode: WarpMode::Beats,
            looping: true,
            loop_start: 0.0,
            loop_end: Some(Ticks::QUARTER.get() as f64 * 4.0),
            ..AudioData::new(media_hash)
        };
        audio.warp_settings.beats.transient_envelope = envelope;
        audio.warp_settings.beats.transient_loop_mode = loop_mode;
        let id = editor.new_id();
        let clip = Clip::new(id, start, Ticks::WHOLE * 4, ClipData::Audio(audio));
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        clips.push(id);
    }
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..2_000u64 {
        let clip = clips[(round % 2) as usize];
        // A warp edit every few blocks: each sends the track a new list.
        let command = match round % 24 {
            0 => Some(Command::SetClipWarpMode {
                clip,
                mode: if round % 48 == 0 {
                    WarpMode::RePitch
                } else {
                    WarpMode::Beats
                },
            }),
            6 => Some(Command::SetClipWarpSettings {
                clip,
                beats: Some(BeatsUpdate {
                    transient_loop_mode: Some(if round % 48 == 6 {
                        TransientLoopMode::LoopBackAndForth
                    } else {
                        TransientLoopMode::LoopForward
                    }),
                    transient_envelope: Some((round % 101) as u32),
                    preserve: None,
                }),
                tones: None,
                texture: None,
                complex_pro: None,
            }),
            // Now and then a tempo change: a program swap.
            12 if round % 240 == 12 => Some(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(100.0 + (round % 400) as f64 / 10.0).unwrap()),
            }),
            _ => None,
        };
        if let Some(command) = command
            && editor.apply(command).is_ok()
        {
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
        }
        // Jump into the middle of the clip now and then.
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
    assert!(heard, "the clip plays");
    eprintln!(
        "2,000 blocks of Beats playback under RealtimeSanitizer: {} clip lists, {} swaps, peak \
         engine load {:.1}%",
        state.counters.data_delivered,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
