//! WP-1.3's acceptance test: 64 stereo files stream at 64-frame buffers
//! without an underrun.
//!
//! The files are generated into a temporary folder (never committed): 48
//! at 48 kHz in 16-bit, 24-bit, and float PCM, and 16 at 44.1 kHz, which the
//! disk thread converts to the engine's 48 kHz. The null device plays them,
//! paced in real time like a sound card, at 64-frame buffers (1.33 ms), with
//! every block a real-time section under the allocation trap. Each block
//! reads 64 frames from every file and mixes them. A third of the way
//! through, the song locates back to a cued point, and two thirds of the
//! way, back to the clips' start, both crossfaded, as a loop or a click in
//! the ruler would. The 48 kHz files hold a known pattern, and the callback checks
//! every sample it reads from four of them each block, in turn, against it.
//!
//! It plays 3 seconds by default. `BL_STREAM_SECONDS` sets the length, and
//! asks for the device to have no dropouts too, which a busy CI runner can't
//! promise:
//!
//! ```text
//! BL_STREAM_SECONDS=30 cargo test -p bl-media --release --test stream_realtime -- --nocapture
//! ```

bl_rt::install_trap!();

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, NullBackend, NullOptions, Pacing,
    PortRequest, ProcessContext, StreamRequest,
};
use bl_media::{AudioSource, Decoder, PlanarMut, StreamConfig, StreamSource, Streamer, Transition};
use common::TempDir;
use common::wav::{self, Format};

const FILES: usize = 64;
const RATE: u32 = 48_000;
const BUFFER: u32 = 64;
/// The cued point every stream can locate to: half a second in.
const CUED: u64 = RATE as u64 / 2;

/// What the callback found, for the test thread to read.
#[derive(Default)]
struct Findings {
    /// Samples checked against the pattern, and how many differed.
    checked: AtomicU64,
    wrong: AtomicU64,
    /// Frames of silence for want of data, summed over every stream.
    missing: AtomicU64,
    /// Samples that weren't finite.
    not_finite: AtomicU64,
    locates: AtomicU64,
}

/// The callback: reads a block from every stream and mixes them.
struct Mix {
    sources: Vec<StreamSource>,
    /// For each source, the pattern's seed if its samples can be checked
    /// exactly (the files at the engine's rate).
    seeds: Vec<Option<u64>>,
    scratch: Vec<f32>,
    stride: usize,
    /// The song position, in frames.
    position: u64,
    /// Frames played.
    played: u64,
    /// When to locate, in frames played, and where to.
    locates: Vec<(u64, u64)>,
    next_locate: usize,
    /// Until this song position, a crossfade runs, so samples aren't the
    /// pattern's.
    fading_until: u64,
    /// Which streams this block checks.
    turn: usize,
    findings: Arc<Findings>,
}

impl AudioCallback for Mix {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let frames = ctx.frames;
        let mut transition = Transition::Cut;
        if let Some(&(at, to)) = self.locates.get(self.next_locate)
            && self.played >= at
        {
            self.position = to;
            transition = Transition::Crossfade;
            self.next_locate += 1;
            self.fading_until = to + self.sources[0].fade_frames() as u64;
            self.findings.locates.fetch_add(1, Relaxed);
        }
        let checking = self.position >= self.fading_until;
        let (mut checked, mut wrong, mut missing, mut not_finite) = (0, 0, 0, 0);
        for (i, source) in self.sources.iter_mut().enumerate() {
            let mut block = PlanarMut::new(&mut self.scratch, 2, frames, self.stride);
            let filled = source.read(self.position, &mut block, transition);
            missing += filled.missing as u64;
            if checking
                && i % (FILES / 4) == self.turn
                && let Some(seed) = self.seeds[i]
            {
                for c in 0..2u16 {
                    for (f, &sample) in block.channel(usize::from(c)).iter().enumerate() {
                        checked += 1;
                        if sample != wav::noise(seed, self.position + f as u64, c) {
                            wrong += 1;
                        }
                    }
                }
            }
            for c in 0..output.channels() {
                let from = block.channel(c.min(1));
                for (out, &sample) in output.channel_mut(c).iter_mut().zip(from) {
                    if !sample.is_finite() {
                        not_finite += 1;
                    }
                    *out += sample / FILES as f32;
                }
            }
        }
        self.turn = (self.turn + 1) % (FILES / 4);
        self.position += frames as u64;
        self.played += frames as u64;
        let f = &self.findings;
        f.checked.fetch_add(checked, Relaxed);
        f.wrong.fetch_add(wrong, Relaxed);
        f.missing.fetch_add(missing, Relaxed);
        f.not_finite.fetch_add(not_finite, Relaxed);
    }
}

#[test]
fn sixty_four_stereo_files_stream_at_64_frame_buffers_without_an_underrun() {
    // Spec: disk-streaming R7
    let requested = std::env::var("BL_STREAM_SECONDS").ok();
    let seconds: f64 = requested.as_deref().map_or(3.0, |s| s.parse().unwrap());
    let play_frames = (seconds * f64::from(RATE)) as u64;

    // The files: long enough for the whole run, locates or none.
    let dir = TempDir::new("stream-64");
    let began = Instant::now();
    let length = play_frames + u64::from(RATE);
    let mut files = Vec::new();
    let mut bytes = 0;
    for i in 0..FILES {
        let (rate, format) = match i % 4 {
            0 => (RATE, Format::Pcm16),
            1 => (RATE, Format::Pcm24),
            2 => (RATE, Format::Float32),
            _ => (44_100, Format::Pcm16),
        };
        let frames = length * u64::from(rate) / u64::from(RATE) + 1;
        let path = dir.path().join(format!("track-{i:02}.wav"));
        let seed = i as u64 + 100;
        wav::write(&path, rate, 2, frames, format, |f, c| {
            wav::noise(seed, f, c)
        });
        bytes += std::fs::metadata(&path).unwrap().len();
        files.push((path, (rate == RATE).then_some(seed)));
    }
    let generated = began.elapsed();

    // Open every stream (each decodes its start now) and cue the point the
    // first locate goes to.
    let streamer = Streamer::start(StreamConfig::default()).unwrap();
    let began = Instant::now();
    let mut sources = Vec::new();
    let mut seeds = Vec::new();
    for (path, seed) in &files {
        let source = streamer
            .open(Decoder::open(path).unwrap(), RATE, 0)
            .unwrap();
        streamer.cue(source.id(), 1, CUED).unwrap();
        sources.push(source);
        seeds.push(*seed);
    }
    let opened = began.elapsed();
    let deadline = Instant::now() + Duration::from_secs(30);
    for source in &mut sources {
        while source.cued(1) != Some(CUED) {
            assert!(Instant::now() < deadline, "the cues never arrived");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    let findings = Arc::new(Findings::default());
    let third = play_frames / 3;
    let mix = Mix {
        sources,
        seeds,
        scratch: vec![0.0; 2 * 4096],
        stride: 4096,
        position: 0,
        played: 0,
        locates: vec![(third, CUED), (2 * third, 0)],
        next_locate: 0,
        fading_until: 0,
        turn: 0,
        findings: Arc::clone(&findings),
    };
    let backend = NullBackend::with_options(NullOptions {
        pacing: Pacing::RealTime,
        frame_limit: Some(play_frames),
    });
    let request = StreamRequest {
        output: Some(PortRequest::default()),
        input: None,
        sample_rate: Some(RATE),
        buffer_size: Some(BUFFER),
    };
    let mut stream = backend.open_stream(&request, Box::new(mix)).unwrap();
    let busy_before = streamer.stats().busy;
    let began = Instant::now();
    stream.start().unwrap();
    while stream.stats().frames < play_frames {
        assert!(
            began.elapsed().as_secs_f64() < seconds * 2.0 + 10.0,
            "the stream stopped early"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let played = began.elapsed();
    stream.stop().unwrap();
    let device = stream.stats();
    let streaming = streamer.stats();
    let busy = streaming.busy - busy_before;

    let totals = streaming.totals;
    eprintln!(
        "{FILES} stereo files ({:.0} MB, generated in {:.1} s; opening them, with each start \
         decoded, took {:.0} ms) played for {:.2} s at {BUFFER}-frame buffers ({:.3} ms) through \
         the null device in real time:\n\
         \x20 underruns: {} ({} frames of silence); frames missing in reads: {}\n\
         \x20 locates: {} ({} ready at once from a cue or the buffer, {} waited)\n\
         \x20 samples checked against the files' pattern: {} ({} wrong); not finite: {}\n\
         \x20 device: {} blocks, {} dropouts ({} late blocks), {} heap operations on the audio \
         thread, longest callback {:.3} ms\n\
         \x20 disk thread: {} blocks decoded, busy {:.1}% of the time; {} read errors",
        bytes as f64 / 1e6,
        generated.as_secs_f64(),
        opened.as_secs_f64() * 1000.0,
        played.as_secs_f64(),
        f64::from(BUFFER) / f64::from(RATE) * 1000.0,
        totals.underruns,
        totals.underrun_frames,
        findings.missing.load(Relaxed),
        findings.locates.load(Relaxed),
        totals.seek_hits,
        totals.seek_misses,
        findings.checked.load(Relaxed),
        findings.wrong.load(Relaxed),
        findings.not_finite.load(Relaxed),
        device.blocks,
        device.dropouts,
        device.late_blocks,
        device.heap_operations,
        device.longest_block.as_secs_f64() * 1000.0,
        totals.blocks,
        busy.as_secs_f64() / played.as_secs_f64() * 100.0,
        totals.errors,
    );

    assert!(device.audio_thread.unwrap().trap_installed);
    assert_eq!(
        device.heap_operations, 0,
        "the audio thread touched the heap"
    );
    assert_eq!(totals.underruns, 0, "a stream underran");
    assert_eq!(findings.missing.load(Relaxed), 0);
    assert_eq!(findings.locates.load(Relaxed), 2);
    assert_eq!(
        (totals.seeks, totals.seek_hits, totals.seek_misses),
        (2 * FILES as u64, 2 * FILES as u64, 0)
    );
    assert!(findings.checked.load(Relaxed) > 0);
    assert_eq!(
        findings.wrong.load(Relaxed),
        0,
        "a stream played wrong samples"
    );
    assert_eq!(findings.not_finite.load(Relaxed), 0);
    assert_eq!(totals.errors, 0);
    if requested.is_some() {
        assert_eq!(device.dropouts, 0, "{device:?}");
    }
}
