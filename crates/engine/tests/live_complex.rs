//! Complex and Complex Pro clips of a streamed file sound in live playback as
//! they do in memory (the warp modes spec, R5, R7, R21 and R24).
//!
//! A live player reads a long file through a disk stream that never waits: a
//! read that doesn't continue the last is a jump, and what the disk hasn't
//! read yet plays as silence. These tests play a clip through a source that
//! behaves so, and compare it with the same clip played from memory, in the
//! clip's first block, after a locate, and after a change of mode, at blocks
//! of 64, 512 and 4,096 frames under the allocation trap.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use crate::common::{RATE, track};
use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{
    EngineConfig, MediaLibrary, MediaMap, MediaStreams, Offline, OpenedSource, Registry,
    SourceCues, TransportCommand,
};
use bl_media::SampleData;
use bl_media::stream::{AudioSource, Filled, PlanarMut, StreamStats, Transition};
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Editor, TrackKind, WarpMarker, WarpMode,
};

const FILE: ContentHash = ContentHash([13; 32]);

/// What the disk thread has ready past a jump to a cued point: the cue's
/// frames.
fn cue() -> u64 {
    MediaStreams::config().cue_frames as u64
}

/// What the disk thread keeps ready past the playhead once it has had a
/// moment: the stream's ring, all its blocks but the one being played.
fn ahead() -> u64 {
    let config = MediaStreams::config();
    ((config.blocks - 1) * config.block_frames) as u64
}

/// A streamed source as the clip player meets it: read forwards, the disk
/// thread filling a ring [`AHEAD`] of the playhead between blocks, a jump
/// ready only as far as its cue, and silence past what is ready. A seek
/// (the player readying a clip while the song is stopped) gives the disk the
/// time to fill the ring.
struct Streamed {
    data: Arc<SampleData>,
    position: u64,
    ready_end: u64,
    jumps: Arc<AtomicU64>,
}

impl AudioSource for Streamed {
    fn channels(&self) -> usize {
        self.data.channel_count()
    }

    fn sample_rate(&self) -> u32 {
        self.data.sample_rate()
    }

    fn frames(&self) -> Option<u64> {
        Some(self.data.frames() as u64)
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn read(&mut self, position: u64, out: &mut PlanarMut<'_>, _: Transition) -> Filled {
        // A read ahead, inside what the ring holds, is served from it.
        let inside = (self.position..self.ready_end).contains(&position);
        if position != self.position && !inside {
            self.jumps.fetch_add(1, Relaxed);
            self.ready_end = position + cue();
        }
        let (n, total) = (out.frames(), self.data.frames() as u64);
        let heard = self
            .ready_end
            .min(total)
            .saturating_sub(position)
            .min(n as u64) as usize;
        for c in 0..out.channels() {
            let file = self.data.channel(c.min(self.channels() - 1)).unwrap();
            let channel = out.channel_mut(c);
            let from = (position as usize).min(file.len());
            channel[..heard].copy_from_slice(&file[from..from + heard]);
            channel[heard..].fill(0.0);
        }
        self.position = position + n as u64;
        self.ready_end = self.ready_end.max(self.position + ahead());
        let past_end = (position + n as u64).saturating_sub(total.max(position)) as usize;
        Filled {
            frames: heard,
            missing: n - heard - past_end.min(n - heard),
            past_end: past_end.min(n - heard),
        }
    }

    fn seek(&mut self, position: u64) {
        self.position = position;
        self.ready_end = position + ahead();
    }

    fn stop(&mut self) {}

    fn buffered(&mut self) -> u64 {
        self.ready_end.saturating_sub(self.position)
    }

    fn stats(&self) -> StreamStats {
        StreamStats::default()
    }
}

struct NoCues;

impl SourceCues for NoCues {
    fn cue(&self, _slot: usize, _frame: u64) {}
}

struct Library {
    data: Arc<SampleData>,
    jumps: Arc<AtomicU64>,
}

impl MediaLibrary for Library {
    fn open(&self, _media: ContentHash, _rate: SampleRate, start: u64) -> Option<OpenedSource> {
        let source = Streamed {
            data: Arc::clone(&self.data),
            position: start,
            ready_end: start + ahead(),
            jumps: Arc::clone(&self.jumps),
        };
        Some(OpenedSource {
            source: Box::new(source),
            cues: Some(Arc::new(NoCues)),
        })
    }
}

/// Nine seconds of two channels: a slow vibrato over a few partials, with a
/// swell every beat, so a stretcher that is late, quiet or silent shows.
fn file() -> Arc<SampleData> {
    let frames = 9 * 48_000;
    let channel = |detune: f64| -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                let tone = (0..4)
                    .map(|k| {
                        let f = 220.0 * f64::from(k + 1) * (1.0 + detune);
                        (std::f64::consts::TAU * f * t + 2.0 * (t * 5.0).sin()).sin()
                            / f64::from(k + 1)
                    })
                    .sum::<f64>();
                (0.3 * tone * (0.6 + 0.4 * (std::f64::consts::TAU * t * 2.0).sin())) as f32
            })
            .collect()
    };
    Arc::new(SampleData::new(48_000, vec![channel(0.0), channel(0.002)]).unwrap())
}

/// The file warped from 110 to 120 BPM, four bars long, in `mode`.
fn song(mode: WarpMode) -> Editor {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    let audio_track = track(&mut editor, TrackKind::Audio, None);
    let audio = AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(110.0),
        warp_mode: mode,
        ..AudioData::new(FILE)
    };
    let clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE * 4,
        ClipData::Audio(audio),
    );
    editor
        .apply(Command::CreateClip {
            track: audio_track,
            clip,
        })
        .unwrap();
    editor
}

#[derive(Clone, Copy, Debug)]
enum Scenario {
    /// The clip, made in the mode, plays from the song's start.
    FromStart,
    /// The song is located a bar in while stopped, then plays.
    AfterLocate,
    /// The clip plays in Re-Pitch, and its mode is switched mid-play.
    AfterModeChange,
}

/// Plays `scenario` in `mode` from `library` for 40,960 frames in blocks of
/// `block` under the allocation trap, and returns the left channel.
fn play(
    library: Arc<dyn MediaLibrary>,
    mode: WarpMode,
    scenario: Scenario,
    block: usize,
) -> Vec<f32> {
    let start = if matches!(scenario, Scenario::AfterModeChange) {
        WarpMode::RePitch
    } else {
        mode
    };
    let mut editor = song(start);
    let clip = editor
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .next()
        .unwrap()
        .id;
    let registry = Registry::new().with_media(library);
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    if matches!(scenario, Scenario::AfterLocate) {
        offline
            .sender()
            .transport(TransportCommand::Locate(Ticks::WHOLE));
        offline.process(block);
        offline.process(block);
    }
    offline.sender().transport(TransportCommand::Play);
    let (frames, change) = (40_960_usize, 20_480_usize);
    let mut out = Vec::new();
    while out.len() < frames {
        if matches!(scenario, Scenario::AfterModeChange) && out.len() == change {
            editor
                .apply(Command::SetClipWarpMode { clip, mode })
                .unwrap();
            offline.sync(editor.document());
        }
        let length = block.min(frames - out.len());
        out.extend_from_slice(offline.process(length).channel(0));
    }
    assert_eq!(
        offline.heap_operations(),
        0,
        "{mode:?} {scenario:?} {block}"
    );
    out
}

// Spec: warp-modes R5, R7, R21, R24. A Complex or Complex Pro clip of a file
// that streams plays as the same clip does from memory, however it begins:
// from the song's start, after a locate, or after a mode change, and at any
// block size. The stream is read forwards, so its reads number a few per
// start and none per block.
#[test]
fn streamed_complex_clips_sound_as_they_do_from_memory() {
    let data = file();
    let mut media = MediaMap::new();
    media.insert(FILE, Arc::clone(&data));
    let memory: Arc<dyn MediaLibrary> = Arc::new(media);
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        for scenario in [
            Scenario::FromStart,
            Scenario::AfterLocate,
            Scenario::AfterModeChange,
        ] {
            for block in [64, 512, 4_096] {
                let want = play(Arc::clone(&memory), mode, scenario, block);
                let level = want.iter().map(|s| s.abs()).fold(0.0, f32::max);
                assert!(
                    level > 0.1,
                    "{mode:?} {scenario:?}: the reference is silent"
                );
                let jumps = Arc::new(AtomicU64::new(0));
                let library = Arc::new(Library {
                    data: Arc::clone(&data),
                    jumps: Arc::clone(&jumps),
                });
                let got = play(library, mode, scenario, block);
                let worst = got
                    .iter()
                    .zip(&want)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f32::max);
                assert!(
                    worst <= 1e-3,
                    "{mode:?} {scenario:?} at blocks of {block}: off by {worst} against a peak \
                     of {level}"
                );
                assert!(
                    jumps.load(Relaxed) <= 2,
                    "{mode:?} {scenario:?} at blocks of {block}: the stream jumped {} times",
                    jumps.load(Relaxed)
                );
            }
        }
    }
}
