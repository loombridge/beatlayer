//! Disk streaming (WP-1.3): a stream plays exactly what decoding the file
//! gives, at any block size and through jumps; the clip's start and cued
//! points play without the disk; an underrun plays silence and is counted;
//! and a locate during playback doesn't click.
//!
//! Every read, seek, and stop on a source runs as a real-time section under
//! the allocation trap (`rt_test!`), as the audio thread runs it.
//!
//! `cargo test -p bl-media --test media_tests -- stream:: --nocapture` prints the
//! discontinuity measurements.

use crate::common;
use std::f64::consts::PI;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use bl_media::{
    AudioSource, Decoder, Filled, MediaStore, MemorySource, PlanarMut, Source, StreamConfig,
    StreamSource, Streamer, Transition, convert_sample_rate, decode_file,
};
use common::wav::{self, Format};
use common::{TempDir, manifest};

const WAIT: Duration = Duration::from_secs(10);

/// Small blocks and cues, so short files cross many of each.
fn small() -> StreamConfig {
    StreamConfig {
        block_frames: 1024,
        blocks: 8,
        cue_frames: 2048,
        cue_slots: 3,
        poll: Duration::from_millis(1),
        ..StreamConfig::default()
    }
}

/// A stereo noise file, exact in 16 bits.
fn noise_file(dir: &Path, name: &str, rate: u32, frames: u64, seed: u64) -> PathBuf {
    let path = dir.join(name);
    wav::write(&path, rate, 2, frames, Format::Pcm16, |f, c| {
        wav::noise(seed, f, c)
    });
    path
}

/// A stereo sine at `hz` and `amplitude`, as 32-bit float.
fn sine_file(dir: &Path, name: &str, rate: u32, frames: u64, hz: f64, amplitude: f64) -> PathBuf {
    let path = dir.join(name);
    wav::write(&path, rate, 2, frames, Format::Float32, |f, _| {
        (amplitude * (2.0 * PI * hz * f as f64 / f64::from(rate)).sin()) as f32
    });
    path
}

/// Reads one block as the audio thread would: a real-time section, under
/// the allocation trap. Returns the block's channels.
fn read(
    source: &mut impl AudioSource,
    position: u64,
    frames: usize,
    transition: Transition,
) -> (Vec<Vec<f32>>, Filled) {
    let channels = source.channels();
    let mut data = vec![f32::NAN; channels * frames];
    let filled = {
        let mut out = PlanarMut::new(&mut data, channels, frames, frames);
        bl_rt::rt_test!({ source.read(position, &mut out, transition) })
    };
    let planes = data.chunks(frames).map(<[f32]>::to_vec).collect();
    (planes, filled)
}

/// Reads `total` frames from `from` in blocks of `block`, waiting before
/// each block until the disk thread has it, so the result doesn't depend
/// on timing.
fn read_through(source: &mut StreamSource, from: u64, total: u64, block: usize) -> Vec<Vec<f32>> {
    let mut planes = vec![Vec::new(); source.channels()];
    let mut position = from;
    let end = from + total;
    bl_rt::rt_test!({ source.seek(from) });
    while position < end {
        let n = block.min((end - position) as usize);
        assert!(
            source.wait_until_ready(n as u64, WAIT),
            "the disk thread stalled"
        );
        let (got, filled) = read(source, position, n, Transition::Cut);
        assert_eq!(filled.missing, 0, "audio missing at {position}");
        for (plane, got) in planes.iter_mut().zip(got) {
            plane.extend_from_slice(&got);
        }
        position += n as u64;
    }
    planes
}

fn assert_bit_exact(what: &str, expected: &[&[f32]], got: &[Vec<f32>]) {
    for (c, (e, g)) in expected.iter().zip(got).enumerate() {
        assert_eq!(e.len(), g.len(), "{what}: channel {c} length");
        if let Some(i) = e
            .iter()
            .zip(g)
            .position(|(a, b)| a.to_bits() != b.to_bits())
        {
            panic!(
                "{what}: channel {c} differs first at frame {i}: {} against {}",
                g[i], e[i]
            );
        }
    }
}

/// The largest difference between neighbouring samples: how big a step the
/// output takes. A click is a step far larger than the signal's own.
fn max_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_stream_plays_exactly_what_decoding_the_file_gives() {
    // Spec: disk-streaming R1
    let dir = TempDir::new("stream-exact");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 100_000, 1);
    let whole = decode_file(&path).unwrap();
    let streamer = Streamer::start(small()).unwrap();
    for block in [64, 333, 1024, 4096] {
        for start in [0, 777, 51_234] {
            let mut source = streamer
                .open(Decoder::open(&path).unwrap(), 48_000, start)
                .unwrap();
            let total = 100_000 - start;
            let got = read_through(&mut source, start, total, block);
            let expected: Vec<&[f32]> = whole.channels().map(|c| &c[start as usize..]).collect();
            assert_bit_exact(&format!("blocks of {block} from {start}"), &expected, &got);
            assert_eq!(source.frames(), Some(100_000));
            // Past the end: silence, and not an underrun.
            let (tail, filled) = read(&mut source, 100_000, 64, Transition::Cut);
            assert_eq!(filled.past_end, 64);
            assert!(tail.iter().all(|c| c.iter().all(|&s| s == 0.0)));
            assert_eq!(source.stats().underruns, 0);
        }
    }
}

#[test]
fn every_format_streams_bit_exact() {
    // Spec: disk-streaming R1
    let streamer = Streamer::start(small()).unwrap();
    let mut checked = 0;
    for fixture in manifest() {
        let whole = decode_file(fixture.path()).unwrap();
        let frames = whole.frames() as u64;
        for start in [0, frames / 3 + 11] {
            let mut source = streamer
                .open(Decoder::open(fixture.path()).unwrap(), fixture.rate, start)
                .unwrap();
            let got = read_through(&mut source, start, frames - start, 100);
            let expected: Vec<&[f32]> = whole.channels().map(|c| &c[start as usize..]).collect();
            assert_bit_exact(&format!("{} from {start}", fixture.file), &expected, &got);
            checked += 1;
        }
    }
    assert!(checked >= 90, "only {checked} streams checked");
}

#[test]
fn a_stream_converts_to_the_engine_rate_as_a_whole_file_conversion_does() {
    // Spec: disk-streaming R1
    let dir = TempDir::new("stream-convert");
    let path = noise_file(dir.path(), "noise-44k1.wav", 44_100, 60_000, 2);
    let converted = convert_sample_rate(&decode_file(&path).unwrap(), 48_000).unwrap();
    let length = converted.frames() as u64;
    let streamer = Streamer::start(small()).unwrap();
    let mut worst = 0.0f32;
    for start in [0, 1, 4_567, 30_001] {
        let mut source = streamer
            .open(Decoder::open(&path).unwrap(), 48_000, start)
            .unwrap();
        assert_eq!(source.frames(), Some(length));
        let got = read_through(&mut source, start, length - start, 256);
        for (c, plane) in got.iter().enumerate() {
            let reference = &converted.channel(c).unwrap()[start as usize..];
            assert_eq!(plane.len(), reference.len());
            for (a, b) in plane.iter().zip(reference) {
                worst = worst.max((a - b).abs());
            }
        }
    }
    // Seeks start the converter on its block grid, so the output repeats a
    // whole-file conversion to within rounding.
    eprintln!("44.1 to 48 kHz, streamed against converted whole: worst error {worst:e}");
    assert!(worst < 1e-6, "worst error {worst}");
}

#[test]
fn rates_without_a_small_ratio_convert_too() {
    // 44,100 to 44,101 Hz reduces to no small ratio, so the sinc resampler
    // converts, and a seek lands within half a frame.
    let dir = TempDir::new("stream-sinc");
    let path = sine_file(dir.path(), "sine.wav", 44_100, 40_000, 100.0, 0.5);
    let converted = convert_sample_rate(&decode_file(&path).unwrap(), 44_101).unwrap();
    let streamer = Streamer::start(small()).unwrap();
    for start in [0, 20_000] {
        let mut source = streamer
            .open(Decoder::open(&path).unwrap(), 44_101, start)
            .unwrap();
        let got = read_through(&mut source, start, 10_000, 512);
        let reference = &converted.channel(0).unwrap()[start as usize..start as usize + 10_000];
        let worst = got[0]
            .iter()
            .zip(reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        // Half a frame of a 100 Hz sine at amplitude 0.5.
        let bound = 0.5 * 2.0 * PI * 100.0 / 44_101.0 * 0.5;
        assert!(
            f64::from(worst) < bound,
            "from {start}: {worst} against {bound}"
        );
    }
}

#[test]
fn the_clip_start_plays_without_the_disk_and_an_underrun_is_counted() {
    // Spec: disk-streaming R2, R3
    let dir = TempDir::new("stream-preload");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 50_000, 3);
    let whole = decode_file(&path).unwrap();
    let streamer = Streamer::start(small()).unwrap();
    let mut source = streamer
        .open(Decoder::open(&path).unwrap(), 48_000, 1_000)
        .unwrap();
    // No disk from here on: only the preloaded start is there.
    streamer.pause();
    let mut position = 1_000;
    for _ in 0..2048 / 64 {
        let (got, filled) = read(&mut source, position, 64, Transition::Cut);
        assert_eq!(filled.frames, 64, "the preload covers the clip's start");
        let expected: Vec<&[f32]> = whole
            .channels()
            .map(|c| &c[position as usize..position as usize + 64])
            .collect();
        assert_bit_exact("the preload", &expected, &got);
        position += 64;
    }
    assert_eq!(source.stats().underruns, 0);

    // Past the preload, with the disk paused: silence, counted once.
    for _ in 0..10 {
        let (got, filled) = read(&mut source, position, 64, Transition::Cut);
        assert_eq!(filled.missing, 64);
        assert!(got.iter().all(|c| c.iter().all(|&s| s == 0.0)));
        position += 64;
    }
    let stats = source.stats();
    assert_eq!(stats.underruns, 1);
    assert_eq!(stats.underrun_frames, 640);
    assert_eq!(
        streamer.stats().totals.underruns,
        1,
        "the streamer reports it"
    );

    // The disk comes back: audio resumes, faded in, then exact.
    streamer.resume();
    assert!(source.wait_until_ready(64, WAIT));
    let fade = source.fade_frames();
    let mut faded = Vec::new();
    let mut exact = Vec::new();
    for _ in 0..(fade / 64 + 2) {
        let (got, filled) = read(&mut source, position, 64, Transition::Cut);
        assert_eq!(filled.frames, 64);
        let reference = &whole.channel(0).unwrap()[position as usize..position as usize + 64];
        for (&got, &reference) in got[0].iter().zip(reference) {
            if faded.len() < fade {
                faded.push((got, reference));
            } else {
                exact.push((got, reference));
            }
        }
        position += 64;
        assert!(source.wait_until_ready(64, WAIT));
    }
    // During the fade-in the audio is the source scaled by a rising gain.
    for (i, &(got, reference)) in faded.iter().enumerate() {
        assert!(got.abs() <= reference.abs() + 1e-7, "frame {i} of the fade");
    }
    assert!(exact.iter().all(|(a, b)| a == b));
    assert_eq!(source.stats().underruns, 1);
}

#[test]
fn a_jump_to_a_cued_point_plays_at_once() {
    // Spec: disk-streaming R2
    let dir = TempDir::new("stream-cue");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 80_000, 4);
    let whole = decode_file(&path).unwrap();
    let streamer = Streamer::start(small()).unwrap();
    let mut source = streamer
        .open(Decoder::open(&path).unwrap(), 48_000, 0)
        .unwrap();
    let loop_start = 60_123;
    streamer.cue(source.id(), 1, loop_start).unwrap();
    let began = std::time::Instant::now();
    while source.cued(1) != Some(loop_start) {
        assert!(began.elapsed() < WAIT, "the cue never arrived");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(streamer.cue(source.id(), 3, 0).is_err(), "three slots");

    // Play a little, then jump to the cue and to the clip's start, with the
    // disk paused: both play at once.
    assert!(source.wait_until_ready(512, WAIT));
    let _ = read(&mut source, 0, 512, Transition::Cut);
    streamer.pause();
    for target in [loop_start, 0, loop_start + 1000] {
        let (got, filled) = read(&mut source, target, 512, Transition::Cut);
        assert_eq!(filled.frames, 512, "a jump to {target}");
        let expected: Vec<&[f32]> = whole
            .channels()
            .map(|c| &c[target as usize..target as usize + 512])
            .collect();
        assert_bit_exact(&format!("a cut to {target}"), &expected, &got);
    }
    let stats = source.stats();
    assert_eq!((stats.seeks, stats.seek_hits, stats.seek_misses), (3, 3, 0));
    streamer.resume();

    // A jump nothing was cued for waits for the disk, and isn't an
    // underrun.
    let (_, filled) = read(&mut source, 30_000, 64, Transition::Cut);
    assert_eq!(filled.missing, 64);
    assert!(source.wait_until_ready(64, WAIT));
    let (got, filled) = read(&mut source, 30_064, 64, Transition::Cut);
    assert_eq!(filled.frames, 64);
    // It fades in from the silence.
    let reference = whole.channel(0).unwrap()[30_064];
    assert!(got[0][0].abs() <= reference.abs() * 1e-3 + 1e-9);
    let stats = source.stats();
    assert_eq!(
        (stats.seek_misses, stats.waited_frames, stats.underruns),
        (1, 64, 0)
    );
}

/// Opens a stream from frame 0 and waits until cue slot 1 holds `cue`.
fn open_cued(streamer: &Streamer, path: &Path, cue: Option<u64>) -> StreamSource {
    let mut source = streamer
        .open(Decoder::open(path).unwrap(), 48_000, 0)
        .unwrap();
    if let Some(cue) = cue {
        streamer.cue(source.id(), 1, cue).unwrap();
        let began = std::time::Instant::now();
        while source.cued(1) != Some(cue) {
            assert!(began.elapsed() < WAIT, "the cue never arrived");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    source
}

/// Plays `blocks` blocks of 64 frames from frame 0, locating at the blocks
/// `locates` names, and returns the first channel. Before every block but
/// a locate and the `unwaited` ones, it waits for the disk, so silence can
/// only come from a locate.
fn play(
    source: &mut StreamSource,
    blocks: usize,
    locates: &[(usize, u64)],
    unwaited: core::ops::Range<usize>,
    transition: Transition,
) -> Vec<f32> {
    let mut out = Vec::new();
    let mut position = 0;
    for block in 0..blocks {
        match locates.iter().find(|(at, _)| *at == block) {
            Some(&(_, to)) => {
                // Let the disk thread fill the look-ahead, so a locate
                // forward into it finds it buffered.
                assert!(source.wait_until_ready(16_384, WAIT));
                position = to;
            }
            None if !unwaited.contains(&block) => {
                assert!(source.wait_until_ready(64, WAIT));
            }
            None => {}
        }
        let (got, _) = read(source, position, 64, transition);
        out.extend_from_slice(&got[0]);
        position += 64;
    }
    out
}

#[test]
fn a_locate_during_playback_doesnt_click() {
    // Spec: disk-streaming R4
    let dir = TempDir::new("stream-click");
    let rate = 48_000.0;
    let (hz, amplitude) = (1_000.0, 0.5);
    // A 1 kHz sine has a 48-frame period at 48 kHz.
    let path = sine_file(dir.path(), "sine.wav", 48_000, 480_000, hz, amplitude);
    // The largest step a sine of this frequency and level takes by itself.
    let natural = (2.0 * amplitude * (PI * hz / rate).sin()) as f32;
    let streamer = Streamer::start(StreamConfig::default()).unwrap();

    // Four locates, each landing half a cycle out of phase with where the
    // playhead was: back to the clip's start (preloaded), to a cued loop
    // start, forward to audio already buffered, and to a point nothing was
    // cued for, where the old audio fades out while the disk reads the new
    // position, which then fades in.
    let cued = 300_032;
    let locates = [(100, 40), (200, cued), (400, 320_872), (500, 200_048)];
    let unwaited = 500..520;

    let mut source = open_cued(&streamer, &path, None);
    let steady = max_step(&play(&mut source, 700, &[], 0..0, Transition::Cut));
    drop(source);

    let mut measured = Vec::new();
    for transition in [Transition::Crossfade, Transition::Cut] {
        let mut source = open_cued(&streamer, &path, Some(cued));
        let out = play(&mut source, 700, &locates, unwaited.clone(), transition);
        let stats = source.stats();
        assert_eq!((stats.seeks, stats.seek_hits, stats.seek_misses), (4, 3, 1));
        assert_eq!(stats.underruns, 0);
        let step = max_step(&out);
        eprintln!(
            "{transition:?}: the largest step across 4 locates is {step:.5}, {:.2}x the sine's \
             own largest step ({natural:.5}; {steady:.5} measured without locates); the \
             uncued locate waited {} frames for the disk",
            step / natural,
            stats.waited_frames,
        );
        measured.push(step);
    }
    // Crossfaded, no step is more than a little larger than the sine's own:
    // the fades add at most their slope, pi/2 / 192 a frame, times the gap
    // between the two signals. Cut, the same locates step by up to twice
    // the amplitude.
    assert!(steady <= natural * 1.0001);
    assert!(measured[0] <= natural * 1.2, "crossfaded: {}", measured[0]);
    assert!(
        measured[1] >= natural * 5.0,
        "the measurement misses clicks: {}",
        measured[1]
    );
}

#[test]
fn a_crossfade_bounds_the_step_even_between_unrelated_levels() {
    // Spec: disk-streaming R4
    // A file at +0.5 then -0.5, and a locate from one half to a cued point in
    // the other: the audio jumps a full unit. Crossfaded, the largest step is
    // the fades' slope, pi/2 / 192.
    let dir = TempDir::new("stream-step");
    let path = dir.path().join("step.wav");
    wav::write(&path, 48_000, 1, 96_000, Format::Float32, |f, _| {
        if f < 48_000 { 0.5 } else { -0.5 }
    });
    let streamer = Streamer::start(StreamConfig::default()).unwrap();
    let slope = (PI / 2.0 / 192.0) as f32;
    for transition in [Transition::Crossfade, Transition::Cut] {
        let mut source = open_cued(&streamer, &path, Some(60_000));
        let out = play(&mut source, 60, &[(20, 60_000)], 0..0, transition);
        let step = max_step(&out);
        eprintln!("{transition:?} across a unit step: the largest step is {step:.5}");
        assert_eq!(source.stats().seek_hits, 1);
        match transition {
            Transition::Crossfade => assert!(step <= slope * 1.01, "{step} against {slope}"),
            Transition::Cut => assert_eq!(step, 1.0),
        }
        assert_eq!(*out.last().unwrap(), -0.5);
    }
}

#[test]
fn seeking_while_stopped_readies_the_new_position() {
    let dir = TempDir::new("stream-seek");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 80_000, 5);
    let whole = decode_file(&path).unwrap();
    let streamer = Streamer::start(small()).unwrap();
    let mut source = streamer
        .open(Decoder::open(&path).unwrap(), 48_000, 0)
        .unwrap();
    bl_rt::rt_test!({ source.seek(45_000) });
    assert!(source.wait_until_ready(4096, WAIT));
    streamer.pause();
    let (got, filled) = read(&mut source, 45_000, 2048, Transition::Crossfade);
    assert_eq!(filled.frames, 2048);
    let expected: Vec<&[f32]> = whole.channels().map(|c| &c[45_000..47_048]).collect();
    // Seeking stopped the audio, so there's nothing to fade from.
    assert_bit_exact("after a seek", &expected, &got);
    bl_rt::rt_test!({ source.stop() });
    let buffered = bl_rt::rt_test!({ source.buffered() });
    assert!(buffered > 0);
    streamer.resume();
}

#[test]
fn a_blocking_source_waits_for_the_disk_instead_of_underrunning() {
    // Spec: disk-streaming R8
    // Offline renders read this way, so an export never underruns.
    let dir = TempDir::new("stream-blocking");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 40_000, 6);
    let whole = decode_file(&path).unwrap();
    let streamer = Arc::new(Streamer::start(small()).unwrap());
    let mut source = streamer
        .open(Decoder::open(&path).unwrap(), 48_000, 0)
        .unwrap();
    source.set_blocking(true);
    streamer.pause();
    let resumer = {
        let streamer = Arc::clone(&streamer);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            streamer.resume();
        })
    };
    let channels = 2;
    let mut got = vec![Vec::new(); channels];
    let mut position = 0;
    while position < 40_000 {
        let mut data = vec![0.0; channels * 500];
        let mut out = PlanarMut::new(&mut data, channels, 500, 500);
        let filled = source.read(position, &mut out, Transition::Cut);
        assert_eq!(filled.missing, 0);
        for (c, plane) in got.iter_mut().enumerate() {
            plane.extend_from_slice(out.channel(c));
        }
        position += 500;
    }
    resumer.join().unwrap();
    let expected: Vec<&[f32]> = whole.channels().collect();
    assert_bit_exact("a blocking read", &expected, &got);
    assert_eq!(source.stats().underruns, 0);
}

/// A reader over bytes that fails once reading passes `fail_at`, after
/// `armed` is set (opening the file reads all of it).
struct Failing {
    inner: Cursor<Vec<u8>>,
    fail_at: u64,
    armed: Arc<AtomicBool>,
}

impl Read for Failing {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.armed.load(Relaxed) && self.inner.position() >= self.fail_at {
            return Err(io::Error::other("the disk went away"));
        }
        self.inner.read(buf)
    }
}

impl Seek for Failing {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
fn a_file_that_fails_mid_stream_plays_silence_and_is_reported() {
    let dir = TempDir::new("stream-fail");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 40_000, 7);
    let bytes = std::fs::read(&path).unwrap();
    // Fails about 20,000 frames in (4 bytes a frame, after a 44-byte header).
    let armed = Arc::new(AtomicBool::new(false));
    let reader = Failing {
        inner: Cursor::new(bytes),
        fail_at: 44 + 4 * 20_000,
        armed: Arc::clone(&armed),
    };
    let streamer = Streamer::start(small()).unwrap();
    let mut source = streamer
        .open(
            Decoder::from_reader(reader, Some("wav")).unwrap(),
            48_000,
            0,
        )
        .unwrap();
    armed.store(true, Relaxed);
    let mut position = 0;
    let mut past_end = 0;
    while position < 40_000 {
        let _ = source.wait_until_ready(64, Duration::from_millis(200));
        let (_, filled) = read(&mut source, position, 64, Transition::Cut);
        past_end += filled.past_end;
        position += 64;
    }
    let stats = source.stats();
    assert_eq!(stats.errors, 1);
    assert_eq!(streamer.stats().totals.errors, 1);
    assert!(
        past_end > 15_000,
        "silence from the failure on: {past_end} frames"
    );
}

#[test]
fn past_the_end_is_silence_not_an_underrun() {
    let dir = TempDir::new("stream-end");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 30_000, 11);
    let whole = decode_file(&path).unwrap();
    let streamer = Streamer::start(small()).unwrap();

    // A clip whose start is past the file's end.
    let mut source = streamer
        .open(Decoder::open(&path).unwrap(), 48_000, 31_000)
        .unwrap();
    assert_eq!(source.frames(), Some(30_000));
    let (_, filled) = read(&mut source, 31_000, 64, Transition::Cut);
    assert_eq!(filled.past_end, 64);

    // Jumps past the end and back.
    let mut source = streamer
        .open(Decoder::open(&path).unwrap(), 48_000, 0)
        .unwrap();
    assert!(source.wait_until_ready(64, WAIT));
    let _ = read(&mut source, 0, 64, Transition::Cut);
    let (_, filled) = read(&mut source, 40_000, 64, Transition::Crossfade);
    assert_eq!(filled.past_end, 64);
    let (got, filled) = read(&mut source, 29_990, 64, Transition::Cut);
    assert_eq!((filled.frames + filled.missing, filled.past_end), (10, 54));
    if filled.frames == 10 {
        assert_eq!(got[0][..10], whole.channel(0).unwrap()[29_990..]);
    }
    let (got, filled) = read(&mut source, 100, 64, Transition::Cut);
    assert_eq!(filled.frames, 64, "the preloaded start");
    assert_eq!(got[0], whole.channel(0).unwrap()[100..164]);
    assert_eq!(source.stats().underruns, 0);
}

#[test]
fn a_dropped_source_closes_its_stream() {
    let dir = TempDir::new("stream-close");
    let path = noise_file(dir.path(), "noise.wav", 48_000, 20_000, 8);
    let streamer = Streamer::start(small()).unwrap();
    let sources: Vec<StreamSource> = (0..3)
        .map(|_| {
            streamer
                .open(Decoder::open(&path).unwrap(), 48_000, 0)
                .unwrap()
        })
        .collect();
    assert_eq!(streamer.stats().streams, 3);
    drop(sources);
    let began = std::time::Instant::now();
    while streamer.stats().streams > 0 {
        assert!(
            began.elapsed() < WAIT,
            "the disk thread kept closed streams"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_memory_source_plays_and_crossfades_like_a_stream() {
    // Spec: disk-streaming R4, R6
    let data = Arc::new(
        bl_media::SampleData::new(
            48_000,
            vec![
                (0..48_000)
                    .map(|f| if f < 24_000 { 0.5 } else { -0.5 })
                    .collect(),
                vec![0.25; 48_000],
            ],
        )
        .unwrap(),
    );
    let mut source = MemorySource::new(Arc::clone(&data));
    assert_eq!(source.fade_frames(), 192);
    let (got, filled) = read(&mut source, 0, 1000, Transition::Cut);
    assert_eq!(filled.frames, 1000);
    assert!(got[0].iter().all(|&s| s == 0.5) && got[1].iter().all(|&s| s == 0.25));
    let mut out = got[0].clone();
    for (i, position) in (30_000..30_000 + 640).step_by(64).enumerate() {
        let transition = if i == 0 {
            Transition::Crossfade
        } else {
            Transition::Cut
        };
        let (got, _) = read(&mut source, position, 64, transition);
        out.extend_from_slice(&got[0]);
    }
    assert!(max_step(&out) <= (PI / 2.0 / 192.0 * 1.01) as f32);
    assert_eq!(*out.last().unwrap(), -0.5);
    let (_, filled) = read(&mut source, 47_990, 64, Transition::Cut);
    assert_eq!((filled.frames, filled.past_end), (10, 54));
    assert_eq!(source.stats().seeks, 2);
}

#[test]
fn short_files_load_into_memory_and_long_ones_stream() {
    // Spec: disk-streaming R6
    let dir = TempDir::new("stream-policy");
    let short = noise_file(dir.path(), "short.wav", 44_100, 44_100, 9);
    let long = noise_file(dir.path(), "long.wav", 48_000, 48_000 * 11, 10);
    let store = MediaStore::new();
    let short = store.import(&short).unwrap();
    let long = store.import(&long).unwrap();
    let streamer = Streamer::start(StreamConfig::default()).unwrap();
    match streamer.source(&store, short, 48_000, 0).unwrap() {
        Source::Memory(source) => {
            assert_eq!(source.data().sample_rate(), 48_000);
            assert_eq!(source.frames(), Some(48_000));
        }
        Source::Stream(_) => panic!("a one-second file streams"),
    }
    match streamer.source(&store, long, 48_000, 0).unwrap() {
        Source::Stream(source) => assert_eq!(source.frames(), Some(48_000 * 11)),
        Source::Memory(_) => panic!("an eleven-second file loads into memory"),
    }
}
