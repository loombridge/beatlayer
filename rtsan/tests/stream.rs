//! Disk streaming's audio-thread work under RealtimeSanitizer (WP-1.3):
//! reading a stream block by block, jumps that land in the look-ahead, in
//! the preloaded start, in a cue, and nowhere ready (which asks the disk
//! thread for a new position), crossfades, an underrun with the disk thread
//! paused, and the in-memory source, each block a real-time section. RTSan
//! stops the process at the first allocation, free, lock, or blocking call
//! in any of it, and the allocation trap fails the test at any heap use.

bl_rt::install_trap!();

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bl_media::{
    AudioSource, Decoder, MemorySource, PlanarMut, SampleData, StreamConfig, StreamSource,
    Streamer, Transition,
};

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
);

const WAIT: Duration = Duration::from_secs(30);

/// A folder for the test's file, removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit stereo WAV of a ramp that wraps, so every frame differs.
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
        let sample = ((frame * 7) % 65_536) as u16 as i16;
        out.write_all(&sample.to_le_bytes()).unwrap();
        out.write_all(&(-(sample / 2)).to_le_bytes()).unwrap();
    }
    out.flush().unwrap();
}

/// One block, read as the audio thread reads it.
fn block(source: &mut impl AudioSource, position: u64, transition: Transition) {
    let mut data = [0.0f32; 2 * 64];
    bl_rt::rt_test!({
        let mut out = PlanarMut::new(&mut data, 2, 64, 64);
        source.read(position, &mut out, transition);
    });
}

fn wait_cued(source: &mut StreamSource, slot: usize, frame: u64) {
    let began = std::time::Instant::now();
    while source.cued(slot) != Some(frame) {
        assert!(began.elapsed() < WAIT, "the cue never arrived");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn streaming_reads_pass_under_rtsan() {
    let dir = TempDir(std::env::temp_dir().join(format!("bl-rtsan-stream-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("ramp.wav");
    write_wav(&path, 300_000);

    let config = StreamConfig {
        block_frames: 1024,
        blocks: 8,
        cue_frames: 4096,
        ..StreamConfig::default()
    };
    let streamer = Streamer::start(config).unwrap();
    let mut source = streamer
        .open(Decoder::open(&path).unwrap(), 48_000, 0)
        .unwrap();
    streamer.cue(source.id(), 1, 200_000).unwrap();
    wait_cued(&mut source, 1, 200_000);

    // Locates every 100 blocks: to the cue, back to the start, forward into
    // the look-ahead, and to a point nothing was ready for.
    let locates = [
        (100, 200_000),
        (200, 100),
        (300, 100 + 100 * 64 + 3_000),
        (400, 150_000),
    ];
    let mut position = 0;
    for round in 0..600 {
        let transition = match locates.iter().find(|(at, _)| *at == round) {
            Some(&(_, to)) => {
                assert!(source.wait_until_ready(4096, WAIT));
                position = to;
                Transition::Crossfade
            }
            None => {
                if !(400..410).contains(&round) {
                    assert!(source.wait_until_ready(64, WAIT));
                }
                Transition::Cut
            }
        };
        block(&mut source, position, transition);
        position += 64;
    }
    let stats = source.stats();
    assert_eq!((stats.seeks, stats.seek_hits, stats.seek_misses), (4, 3, 1));

    // An underrun, with the disk thread paused.
    streamer.pause();
    for _ in 0..200 {
        block(&mut source, position, Transition::Cut);
        position += 64;
    }
    assert!(source.stats().underruns >= 1);
    streamer.resume();

    // Seeking, stopping, and asking what's buffered.
    bl_rt::rt_test!({ source.seek(50_000) });
    assert!(source.wait_until_ready(64, WAIT));
    bl_rt::rt_test!({ source.stop() });
    let buffered = bl_rt::rt_test!({ source.buffered() });
    assert!(buffered >= 64);
    block(&mut source, 50_000, Transition::Crossfade);

    // The in-memory source.
    let data = Arc::new(SampleData::new(48_000, vec![vec![0.25; 20_000]; 2]).unwrap());
    let mut memory = MemorySource::new(data);
    block(&mut memory, 0, Transition::Cut);
    block(&mut memory, 64, Transition::Cut);
    block(&mut memory, 10_000, Transition::Crossfade);
    block(&mut memory, 19_990, Transition::Cut);
    drop(source);
}
