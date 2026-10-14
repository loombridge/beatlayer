//! Streamed clip playback's audio-thread work under RealtimeSanitizer
//! (WP-1.4b): clip players reading long files through disk streams (and a
//! short one from memory), with clips looped and unlooped, starting, ending,
//! wrapping their loops, and split, which moves a part's Start marker; new
//! lists arriving as clips are edited, the player carrying each kept clip's
//! stream across from its old list and handing the rest back for the drop
//! thread; locates into the middle of clips, where nothing was preloaded;
//! the song stopping, which stops every stream, and the streams being
//! readied for where it stopped; and program swaps from tempo changes. The
//! test runs blocks faster than real time, so the disk thread falls behind
//! now and then and reads underrun, which play silence. RTSan stops the
//! process at the first allocation, free, lock, or blocking call in any of
//! it, and the allocation trap fails the test at any heap use.
//!
//! Spec: audio-clips R20, R21.

bl_rt::install_trap!();

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{
    Collector, Compiler, EngineConfig, MediaLibrary, MediaStreams, Registry, TransportCommand,
    channel,
};
use bl_media::MediaStore;
use bl_model::{AudioData, Clip, ClipData, Command, ContentHash, Editor, TrackKind};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A folder for the test's files, removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a quiet 16-bit stereo WAV at 48 kHz.
fn write_wav(path: &Path, frames: u32) {
    let data = frames * 4;
    let mut out = BufWriter::new(File::create(path).unwrap());
    out.write_all(b"RIFF").unwrap();
    out.write_all(&(36 + data).to_le_bytes()).unwrap();
    out.write_all(b"WAVEfmt ").unwrap();
    for field in [16u32.to_le_bytes(), [1, 0, 2, 0], 48_000u32.to_le_bytes()] {
        out.write_all(&field).unwrap();
    }
    out.write_all(&(48_000u32 * 4).to_le_bytes()).unwrap();
    out.write_all(&[4, 0, 16, 0]).unwrap();
    out.write_all(b"data").unwrap();
    out.write_all(&data.to_le_bytes()).unwrap();
    for frame in 0..frames {
        let sample = ((frame % 100) as i16 - 50) * 2;
        out.write_all(&sample.to_le_bytes()).unwrap();
        out.write_all(&(-sample).to_le_bytes()).unwrap();
    }
    out.flush().unwrap();
}

#[test]
fn streamed_clip_playback_passes_under_rtsan() {
    let dir = TempDir(std::env::temp_dir().join(format!("bl-rtsan-clips-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let store = Arc::new(MediaStore::new());
    let mut media = Vec::new();
    // Two files long enough to stream, and one short enough for memory.
    for (name, seconds) in [("long-a.wav", 30), ("long-b.wav", 20), ("short.wav", 2)] {
        let path = dir.0.join(name);
        write_wav(&path, seconds * 48_000);
        media.push(ContentHash(*store.import(&path).unwrap().as_bytes()));
    }
    let streams = Arc::new(MediaStreams::new(Arc::clone(&store)).unwrap());
    let registry = Registry::new().with_media(Arc::clone(&streams) as Arc<dyn MediaLibrary>);
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
    let looped = AudioData {
        start: 1.5,
        looping: true,
        loop_start: 2.0,
        loop_end: Some(2.3),
        ..AudioData::new(media[0]).with_edge_fades()
    };
    let mut clips = Vec::new();
    for (track, start, audio) in [
        (tracks[0], Ticks::ZERO, looped),
        (tracks[1], Ticks(12_345), AudioData::new(media[1])),
        (tracks[2], Ticks::QUARTER, AudioData::new(media[2])),
        (tracks[2], Ticks::WHOLE * 3, AudioData::new(media[0])),
    ] {
        let id = editor.new_id();
        let clip = Clip::new(id, start, Ticks::WHOLE * 3, ClipData::Audio(audio));
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        clips.push(id);
    }
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..3_000u64 {
        let clip = clips[(round % 4) as usize];
        // A clip edit every few blocks: each sends its track a new list,
        // which keeps the clips' streams or opens new ones.
        let command = match round % 30 {
            0 => Some(Command::SetClipGain {
                clip,
                gain: (round % 7) as f64 / 6.0,
            }),
            6 => Some(Command::MoveClip {
                clip,
                track: tracks[(round % 3) as usize],
                start: Ticks::QUARTER * (round % 4) as i64,
            }),
            12 => Some(Command::SetClipMute {
                clip,
                mute: round % 60 == 12,
            }),
            18 => {
                // Trim the clip's left edge, which moves its Start marker,
                // and back again.
                let at = editor.document().clip(clip).unwrap().start;
                let start = if round % 60 == 18 {
                    at + Ticks::QUARTER
                } else {
                    at.saturating_sub(Ticks::QUARTER).max(Ticks::ZERO)
                };
                Some(Command::TrimClipStart { clip, start })
            }
            24 if round % 300 == 24 => Some(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(100.0 + (round % 400) as f64 / 10.0).unwrap()),
            }),
            _ => None,
        };
        if let Some(command) = command {
            // A trim that would take a Start marker before its file's start
            // is refused, and changes nothing.
            if editor.apply(command).is_ok() {
                let update = compiler.sync(editor.document());
                programs += u64::from(update.program.is_some());
                sender.send_update(update);
            }
        }
        // Jump into the middle of the clips now and then, stop, and play
        // again from where it stopped.
        match round % 200 {
            50 => sender.transport(TransportCommand::Locate(Ticks(98_765 * (round % 7) as i64))),
            120 => sender.transport(TransportCommand::Stop),
            150 => sender.transport(TransportCommand::Continue),
            _ => {}
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
    let disk = streams.stats();
    assert!(disk.totals.blocks > 0, "{disk:?}");
    assert!(disk.totals.seeks > 0, "{disk:?}");
    assert_eq!(disk.totals.errors, 0);
    eprintln!(
        "3,000 blocks of streamed clip playback under RealtimeSanitizer: {} clip lists, {} \
         swaps, {} streams open, {} blocks read, {} jumps, {} underruns",
        state.counters.data_delivered,
        state.swaps,
        disk.streams,
        disk.totals.blocks,
        disk.totals.seeks,
        disk.totals.underruns
    );
}
