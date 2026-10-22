//! Warped clips streamed from disk (the warping spec, R18, and the warp modes
//! spec, R11): a clip read at four times its own speed underruns no more than
//! one at its own speed, a clip held in memory never underruns and sounds the
//! same, and switching the RAM mode while a clip plays leaves no gap and no
//! step. The engine plays on a real-time audio thread (the null device, paced
//! like a sound card) under the allocation trap.
//!
//! The files are generated in a temporary folder. The tests play for a few
//! seconds each, a second or two with `BL_TEST_DEPTH` unset or `quick`.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::thread;
use std::time::Duration;

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, NullBackend, NullOptions, Pacing,
    PortRequest, ProcessContext, StreamRequest,
};
use bl_core::time::{SampleRate, Ticks};
use bl_engine::{
    Engine, EngineConfig, MediaLibrary, MediaStreams, Offline, Processor, Registry,
    TransportCommand,
};
use bl_media::MediaStore;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, TrackKind, WarpMarker,
    WarpMode,
};
use bl_rt::trap::TrapAllocator;

#[global_allocator]
static ALLOCATOR: TrapAllocator<std::alloc::System> = TrapAllocator::new(std::alloc::System);

const RATE: SampleRate = SampleRate::HZ_48000;
const BLOCK: usize = 256;
/// A file long enough to stream (the threshold is ten seconds).
const FILE_SECONDS: u64 = 40;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-warp-{name}-{}", std::process::id()));
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

/// Writes a 16-bit stereo WAV file of `frames` frames at 48 kHz: a sine,
/// quiet but well above the 16-bit steps.
fn write_wav(path: &Path, frames: u64) {
    let data = u32::try_from(frames * 4).expect("a file fits a WAV file");
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
    let step = 220.0 / 48_000.0 * std::f64::consts::TAU;
    for i in 0..frames {
        let sample = ((i as f64 * step).sin() * 2_000.0) as i16;
        out.write_all(&sample.to_le_bytes()).unwrap();
        out.write_all(&(-sample).to_le_bytes()).unwrap();
    }
    out.flush().unwrap();
}

/// The left channel the engine played, kept in room made beforehand: the
/// audio thread stores into it without allocating or locking.
struct Heard {
    samples: Vec<AtomicU32>,
    len: AtomicUsize,
}

impl Heard {
    fn new(frames: usize) -> Arc<Self> {
        Arc::new(Self {
            samples: (0..frames).map(|_| AtomicU32::new(0)).collect(),
            len: AtomicUsize::new(0),
        })
    }

    fn push(&self, block: &[f32]) {
        let at = self.len.load(Relaxed);
        for (slot, sample) in self.samples[at..].iter().zip(block) {
            slot.store(sample.to_bits(), Relaxed);
        }
        self.len
            .store((at + block.len()).min(self.samples.len()), Relaxed);
    }

    fn all(&self) -> Vec<f32> {
        self.samples[..self.len.load(Relaxed)]
            .iter()
            .map(|s| f32::from_bits(s.load(Relaxed)))
            .collect()
    }
}

/// The engine as a stream's callback, keeping the left channel it played.
struct Play {
    processor: Processor,
    heard: Arc<Heard>,
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
            self.heard.push(block.channel(0));
            done += n;
        }
    }
}

/// A warped clip of `media` from 1.1.1 for `bars` bars whose file runs at
/// `file_bpm`, so a song at 120 BPM reads it at `120 / file_bpm` times its
/// speed; unwarped for a `file_bpm` of `None`.
fn song(media: ContentHash, bars: i64, file_bpm: Option<f64>, ram: bool) -> (Editor, ClipId) {
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
    let id = editor.new_id();
    let data = AudioData {
        warp: file_bpm.is_some(),
        warp_markers: file_bpm
            .map(|_| WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            })
            .into_iter()
            .collect(),
        tempo: file_bpm,
        warp_mode: WarpMode::RePitch,
        ram,
        ..AudioData::new(media)
    };
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE * bars, ClipData::Audio(data)),
        })
        .unwrap();
    (editor, id)
}

/// What a real-time run showed.
struct Run {
    heard: Vec<f32>,
    underruns: u64,
}

/// Plays `editor` for `seconds` on a real-time audio thread, streaming from
/// `store`, calling `act` after each of its `(delay, command)` steps has
/// waited its delay (the command is applied to the set and the engine synced).
fn run(
    store: &Arc<MediaStore>,
    editor: &mut Editor,
    seconds: f64,
    steps: &[(Duration, Command)],
) -> Run {
    let streams = Arc::new(MediaStreams::new(Arc::clone(store)).unwrap());
    let registry = Registry::new().with_media(Arc::clone(&streams) as Arc<dyn MediaLibrary>);
    let (engine, processor) = Engine::start(EngineConfig::new(RATE, BLOCK), registry).unwrap();
    let heard = Heard::new((seconds as usize + 4) * 48_000);
    let backend = NullBackend::with_options(NullOptions {
        dropout_after: None,
        pacing: Pacing::RealTime,
        frame_limit: None,
        channels: 2,
    });
    let request = StreamRequest {
        output: Some(PortRequest::default()),
        input: None,
        sample_rate: Some(RATE.hz()),
        buffer_size: Some(BLOCK as u32),
    };
    let mut stream = backend
        .open_stream(
            &request,
            Box::new(Play {
                processor,
                heard: Arc::clone(&heard),
            }),
        )
        .unwrap();
    stream.start().unwrap();
    engine.sync(editor.document());
    assert!(engine.flush(Duration::from_secs(5)));
    // Let the loader open the clip's source before the song plays.
    thread::sleep(Duration::from_millis(300));
    engine.play();
    let mut waited = Duration::ZERO;
    for (delay, command) in steps {
        thread::sleep(*delay);
        waited += *delay;
        editor.apply(command.clone()).unwrap();
        engine.sync(editor.document());
    }
    thread::sleep(Duration::from_secs_f64(seconds).saturating_sub(waited));
    engine.stop();
    stream.stop().unwrap();
    let audio = stream.stats();
    assert_eq!(audio.heap_operations, 0);
    drop(stream);
    drop(engine);
    Run {
        heard: heard.all(),
        underruns: streams.stats().totals.underruns,
    }
}

/// What the set renders offline, which waits for the disk instead of
/// underrunning: the same samples a real-time run should play.
fn render(store: &Arc<MediaStore>, editor: &Editor, frames: u64) -> Vec<f32> {
    let streams = MediaStreams::blocking(Arc::clone(store)).unwrap();
    let registry = Registry::new().with_media(Arc::new(streams) as Arc<dyn MediaLibrary>);
    let mut offline = Offline::new(EngineConfig::new(RATE, 512), registry);
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(frames, |block| out.extend_from_slice(block.channel(0)));
    out
}

fn import(name: &str) -> (Scratch, Arc<MediaStore>, ContentHash) {
    let scratch = Scratch::new(name);
    let path = scratch.0.join("loop.wav");
    write_wav(&path, FILE_SECONDS * 48_000);
    let store = Arc::new(MediaStore::new());
    let media = ContentHash(*store.import(&path).unwrap().as_bytes());
    (scratch, store, media)
}

/// The index of the first sample that isn't silence.
fn first_sound(heard: &[f32]) -> usize {
    heard
        .iter()
        .position(|&s| s != 0.0)
        .expect("the clip sounded")
}

// Spec: warping R18. A clip read at four times its own speed (or a little
// above it) streams without more underruns than one at its own speed.
#[test]
fn a_clip_warped_to_four_times_its_speed_streams_without_extra_underruns() {
    let (_scratch, store, media) = import("four");
    let (mut own, _) = song(media, 3, None, false);
    let baseline = run(&store, &mut own, 3.0, &[]);
    assert!(baseline.heard.iter().any(|&s| s != 0.0));
    for (file_bpm, speed) in [(120.0, 1.0), (80.0, 1.5), (30.0, 4.0)] {
        let (mut warped, _) = song(media, 3, Some(file_bpm), false);
        let got = run(&store, &mut warped, 3.0, &[]);
        assert!(got.heard.iter().any(|&s| s != 0.0), "speed {speed}");
        assert!(
            got.underruns <= baseline.underruns,
            "speed {speed}: {} underruns against {} at its own speed",
            got.underruns,
            baseline.underruns
        );
    }
}

// Spec: warp-modes R11. A clip held in memory never underruns, and plays the
// same samples a streamed one does.
#[test]
fn a_ram_clip_never_underruns_and_sounds_the_same() {
    let (_scratch, store, media) = import("ram");
    let frames = 3 * 48_000;
    let (streamed, _) = song(media, 2, Some(100.0), false);
    let (mut held, _) = song(media, 2, Some(100.0), true);
    assert_eq!(
        render(&store, &held, frames),
        render(&store, &streamed, frames),
        "RAM changes nothing the clip plays"
    );
    let played = run(&store, &mut held, 2.0, &[]);
    assert_eq!(played.underruns, 0);
    assert!(played.heard.iter().any(|&s| s != 0.0));
}

// Spec: warp-modes R11. Switched on while the clip plays, it reads its stream
// until the file is in memory, then memory, on the same frame; switched off,
// memory until a stream has the audio it has reached: no gap, no step.
#[test]
fn switching_ram_while_playing_leaves_no_gap() {
    let (_scratch, store, media) = import("switch");
    let (mut editor, clip) = song(media, 3, Some(100.0), false);
    let reference = render(&store, &editor, 6 * 48_000);
    let steps = [
        (
            Duration::from_millis(700),
            Command::SetClipRamMode { clip, ram: true },
        ),
        (
            Duration::from_millis(1_000),
            Command::SetClipRamMode { clip, ram: false },
        ),
    ];
    let got = run(&store, &mut editor, 3.0, &steps);
    // The song starts a moment after the stream does, with the clip's
    // source open: from the clip's first sound, every sample is the offline
    // render's, through both switches.
    let (from, start) = (first_sound(&got.heard), first_sound(&reference));
    // Up to a moment before the engine stopped: silence follows it.
    let length = (got.heard.len() - from)
        .min(reference.len() - start)
        .min(2_800 * 48);
    assert!(length > 2 * 48_000, "{length} frames compared");
    assert_eq!(got.underruns, 0);
    for (frame, (a, b)) in got.heard[from..from + length]
        .iter()
        .zip(&reference[start..start + length])
        .enumerate()
    {
        assert_eq!(a, b, "frame {frame} of the clip");
    }
}
