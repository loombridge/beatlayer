//! WP-1.4b's acceptance: eight long stereo stems play from disk through the
//! engine on a real-time audio thread (the null device, paced like a sound
//! card), with memory use bounded by the streams' buffers rather than the
//! files' lengths, and loading them doesn't delay Play or Stop. The rules
//! are in the audio clips spec (R16 and R17) and the disk streaming spec.
//!
//! The stems are generated in a temporary folder, a minute each by default;
//! `BL_STEM_MINUTES=10` makes them ten minutes long, as the acceptance
//! criterion says (about 0.9 GB of WAV files):
//!
//! ```text
//! BL_STEM_MINUTES=10 cargo test -p bl-engine --release --test streaming -- --nocapture
//! ```
//!
//! Heap use is counted by a global allocator that wraps the allocation
//! trap, so the audio thread still runs under the trap.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, NullBackend, NullOptions, Pacing,
    PortRequest, ProcessContext, StreamRequest,
};
use bl_core::time::{SampleRate, Ticks};
use bl_engine::{Engine, EngineConfig, MediaLibrary, MediaStreams, Processor, Registry};
use bl_media::MediaStore;
use bl_model::{AudioData, Clip, ClipData, Command, ContentHash, Editor, TrackId, TrackKind};
use bl_rt::trap::TrapAllocator;

/// Heap bytes in use, and the most in use since the last reset.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, counting the bytes in use.
struct Counting;

// SAFETY: every method forwards to the system allocator with the same
// arguments, and only adds atomic counting around it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grew(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            grew(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        // SAFETY: as for `alloc`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            grew(new_size);
        }
        new
    }
}

fn grew(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}

#[global_allocator]
static ALLOCATOR: TrapAllocator<Counting> = TrapAllocator::new(Counting);

const RATE: SampleRate = SampleRate::HZ_48000;
const BLOCK: usize = 256;
const STEMS: usize = 8;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("bl-stems-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit stereo WAV file of `frames` frames at 48 kHz: a sine at
/// `hz`, 60 dB below full scale.
fn write_stem(path: &Path, frames: u64, hz: f64) {
    let data = u32::try_from(frames * 4).expect("a stem fits a WAV file");
    let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(path).unwrap());
    out.write_all(b"RIFF").unwrap();
    out.write_all(&(36 + data).to_le_bytes()).unwrap();
    out.write_all(b"WAVEfmt ").unwrap();
    out.write_all(&16u32.to_le_bytes()).unwrap();
    out.write_all(&1u16.to_le_bytes()).unwrap();
    out.write_all(&2u16.to_le_bytes()).unwrap();
    out.write_all(&48_000u32.to_le_bytes()).unwrap();
    out.write_all(&192_000u32.to_le_bytes()).unwrap();
    out.write_all(&4u16.to_le_bytes()).unwrap();
    out.write_all(&16u16.to_le_bytes()).unwrap();
    out.write_all(b"data").unwrap();
    out.write_all(&data.to_le_bytes()).unwrap();
    let step = hz / 48_000.0 * std::f64::consts::TAU;
    for i in 0..frames {
        let sample = ((i as f64 * step).sin() * 32.0) as i16;
        out.write_all(&sample.to_le_bytes()).unwrap();
        out.write_all(&(-sample).to_le_bytes()).unwrap();
    }
    out.flush().unwrap();
}

/// The engine as a stream's callback. The stream runs it under the
/// allocation trap and counts heap use and dropouts.
struct Play {
    processor: Processor,
}

impl AudioCallback for Play {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let mut done = 0;
        while done < ctx.frames {
            let n = (ctx.frames - done).min(self.processor.config().max_frames);
            let block = self.processor.process(n);
            for c in 0..output.channels() {
                let from = block.channel(c.min(block.channels() - 1));
                output.channel_mut(c)[done..done + n].copy_from_slice(from);
            }
            done += n;
        }
    }
}

/// A song of `STEMS` audio tracks, each playing one of `media` whole from
/// 1.1.1 (120 BPM, so a minute is 30 bars). New IDs each time, so every
/// clip needs its source opened.
fn song(media: &[ContentHash], minutes: u64) -> Editor {
    let mut editor = Editor::new();
    let length = Ticks::WHOLE * 30 * minutes as i64;
    for (index, &hash) in media.iter().enumerate() {
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: format!("Stem {}", index + 1),
                parent: None,
                index,
            })
            .unwrap();
        let clip = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    length,
                    ClipData::Audio(AudioData::new(hash)),
                ),
            })
            .unwrap();
    }
    editor
}

/// How long after `act` the audio thread shows `playing`, polling its state.
fn latency(engine: &mut Engine, playing: bool, act: impl FnOnce(&Engine)) -> Duration {
    let began = Instant::now();
    act(engine);
    while engine.state().playing != playing {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the engine never showed playing = {playing}"
        );
        thread::sleep(Duration::from_micros(50));
    }
    began.elapsed()
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The median and the longest of some durations, in milliseconds.
fn spread(mut times: Vec<Duration>) -> (f64, f64) {
    times.sort_unstable();
    (ms(times[times.len() / 2]), ms(times[times.len() - 1]))
}

// Spec: audio-clips R16, R17; disk-streaming R1, R3
#[test]
fn eight_long_stems_stream_in_bounded_memory_and_loading_them_never_delays_play_or_stop() {
    let minutes: u64 = std::env::var("BL_STEM_MINUTES")
        .ok()
        .and_then(|m| m.parse().ok())
        .unwrap_or(1);
    let frames = minutes * 60 * 48_000;
    let scratch = Scratch::new();
    let store = Arc::new(MediaStore::new());
    let generated = Instant::now();
    let mut media = Vec::new();
    for stem in 0..STEMS {
        let path = scratch.0.join(format!("Stem {}.wav", stem + 1));
        write_stem(&path, frames, 110.0 * (stem + 1) as f64);
        media.push(ContentHash(*store.import(&path).unwrap().as_bytes()));
    }
    let file_bytes = STEMS as u64 * (44 + frames * 4);
    let decoded_bytes = STEMS as u64 * frames * 2 * 4;
    eprintln!(
        "{STEMS} stereo stems of {minutes} min at 48 kHz: {:.0} MiB of 16-bit WAV, {:.0} MiB \
         decoded to 32-bit float; generated and imported in {:.1} s",
        mib(file_bytes as usize),
        mib(decoded_bytes as usize),
        generated.elapsed().as_secs_f64()
    );

    // What the compiler thread did before WP-1.4b: decode each file whole,
    // into memory, before its clip could play, with Play and Stop waiting
    // behind it. Timed here for one stem, which is then let go of.
    let decoding = Instant::now();
    let whole = store
        .load_at_rate(bl_media::ContentHash::from_bytes(media[0].0), RATE.hz())
        .unwrap();
    let decode_one = decoding.elapsed();
    drop(whole);

    // The engine, streaming the store's media, on a real-time audio thread.
    let baseline = LIVE.load(Relaxed);
    PEAK.store(baseline, Relaxed);
    let streams = Arc::new(MediaStreams::new(Arc::clone(&store)).unwrap());
    let registry = Registry::new().with_media(Arc::clone(&streams) as Arc<dyn MediaLibrary>);
    let config = EngineConfig::new(RATE, BLOCK);
    let (mut engine, processor) = Engine::start(config, registry).unwrap();
    let backend = NullBackend::with_options(NullOptions {
        pacing: Pacing::RealTime,
        frame_limit: None,
    });
    let request = StreamRequest {
        output: Some(PortRequest::default()),
        input: None,
        sample_rate: Some(RATE.hz()),
        buffer_size: Some(BLOCK as u32),
    };
    let mut stream = backend
        .open_stream(&request, Box::new(Play { processor }))
        .unwrap();
    stream.start().unwrap();
    let empty = Editor::new();
    engine.sync(empty.document());
    assert!(engine.flush(Duration::from_secs(5)));

    // Play and Stop with nothing to load.
    let mut idle_play = Vec::new();
    let mut idle_stop = Vec::new();
    for _ in 0..10 {
        idle_play.push(latency(&mut engine, true, Engine::play));
        thread::sleep(Duration::from_millis(20));
        idle_stop.push(latency(&mut engine, false, Engine::stop));
        thread::sleep(Duration::from_millis(20));
    }

    // Play and Stop straight after a set whose eight clips each need their
    // source opened: the loader opens them while the transport answers.
    let mut loading_play = Vec::new();
    let mut loading_stop = Vec::new();
    let mut ready = Vec::new();
    let mut sets = Vec::new();
    for _ in 0..5 {
        let set = song(&media, minutes);
        let stems: Vec<TrackId> = set.document().top_level().collect();
        let began = Instant::now();
        engine.sync(set.document());
        loading_play.push(latency(&mut engine, true, Engine::play));
        loading_stop.push(latency(&mut engine, false, Engine::stop));
        // Then play until every stem sounds.
        engine.play();
        loop {
            let state = engine.state();
            let sounding = stems
                .iter()
                .filter(|&&t| state.meter(t).is_some_and(|[left, _]| left > 0.0))
                .count();
            if sounding == STEMS {
                ready.push(began.elapsed());
                break;
            }
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "only {sounding} stems sound"
            );
            thread::sleep(Duration::from_micros(200));
        }
        engine.stop();
        sets.push(set);
    }
    let set = sets.pop().unwrap();
    drop(sets);

    // Play through the stems, and jump about in them: to a quarter, half,
    // and nine tenths of the way through, where nothing was preloaded.
    engine.play();
    thread::sleep(Duration::from_secs(2));
    let length = Ticks::WHOLE * 30 * minutes as i64;
    for fraction in [4, 2] {
        engine.locate(Ticks(length.get() / fraction));
        thread::sleep(Duration::from_millis(1_500));
    }
    engine.locate(Ticks(length.get() / 10 * 9));
    thread::sleep(Duration::from_millis(1_500));
    let peak = PEAK.load(Relaxed) - baseline;
    let state = engine.state().clone();
    let disk = streams.stats();
    engine.stop();
    stream.stop().unwrap();
    let audio = stream.stats();
    drop(stream);
    drop(engine);
    drop(set);

    let (idle_play, idle_play_max) = spread(idle_play);
    let (idle_stop, idle_stop_max) = spread(idle_stop);
    let (loading_play, loading_play_max) = spread(loading_play);
    let (loading_stop, loading_stop_max) = spread(loading_stop);
    let (ready, ready_max) = spread(ready);
    eprintln!(
        "memory: {:.1} MiB at most above the {:.1} MiB in use before the engine started, for \
         {} streams ({:.1} MiB each), against {:.0} MiB of decoded audio",
        mib(peak),
        mib(baseline),
        disk.streams,
        mib(peak) / STEMS as f64,
        mib(decoded_bytes as usize),
    );
    eprintln!(
        "Play: median {idle_play:.1} ms, longest {idle_play_max:.1} ms with nothing to load; \
         median {loading_play:.1} ms, longest {loading_play_max:.1} ms straight after eight \
         clips needing their files"
    );
    eprintln!(
        "Stop: median {idle_stop:.1} ms, longest {idle_stop_max:.1} ms with nothing to load; \
         median {loading_stop:.1} ms, longest {loading_stop_max:.1} ms while eight files load"
    );
    eprintln!(
        "every stem sounding {ready:.1} ms (median), {ready_max:.1} ms (longest) after the set \
         reached the engine; decoding one stem whole, as the compiler thread did before, takes \
         {:.0} ms, so eight held Play for about {:.1} s",
        ms(decode_one),
        decode_one.as_secs_f64() * STEMS as f64
    );
    eprintln!(
        "disk thread: {} blocks read, busy {:.1} s, {} underruns ({} frames), {} jumps waited \
         {} frames; audio thread: {} blocks, {} dropouts, {} heap operations",
        disk.totals.blocks,
        disk.busy.as_secs_f64(),
        disk.totals.underruns,
        disk.totals.underrun_frames,
        disk.totals.seek_misses,
        disk.totals.waited_frames,
        audio.blocks,
        audio.dropouts,
        audio.heap_operations
    );

    // Spec: audio-clips R16. The streams' buffers, not the files: eight
    // streams of eight 4,096-frame blocks and three 16,384-frame cues hold
    // about 3 MiB of stereo audio; decoders and the rest stay well within
    // 32 MiB, however long the stems.
    assert!(
        peak < 32 << 20,
        "{:.1} MiB in use while streaming",
        mib(peak)
    );
    assert!((peak as u64) < decoded_bytes / 10);
    // Spec: audio-clips R17. Loading never holds up the transport: Play and
    // Stop answer as soon as they would with nothing to load, within a few
    // blocks of 5.3 ms.
    assert!(
        loading_play_max < 100.0 && loading_stop_max < 100.0,
        "Play {loading_play_max:.1} ms, Stop {loading_stop_max:.1} ms"
    );
    assert_eq!(state.counters.data_unrouted, 0);
    assert_eq!(disk.totals.errors, 0);
    assert_eq!(disk.totals.underruns, 0, "{disk:?}");
    assert_eq!(audio.heap_operations, 0);
    assert!(audio.audio_thread.unwrap().trap_installed);
}
