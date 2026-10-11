//! The disk thread, which reads every stream ahead of its reader.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bl_rt::log::Level;
use bl_rt::rt_log;
use rtrb::{Consumer, Producer, RingBuffer};

use super::feed::Feed;
use super::reader::{Ring, StreamId, StreamSource, Streamed};
use super::shared::{Block, Cue, Shared, StreamStats, StreamerStats, ToDisk, ToReader, Totals};
use super::source::{MemorySource, Source};
use super::splice::fade_frames;
use crate::decode::Decoder;
use crate::error::MediaError;
use crate::hash::ContentHash;
use crate::store::MediaStore;

/// How long the disk thread sleeps between passes when no stream is playing.
const IDLE_POLL: Duration = Duration::from_millis(20);

/// How a [`Streamer`] streams. Frame counts are at the engine's rate; the
/// defaults' durations below are at 48 kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    /// Frames in a block, the unit the disk thread reads and the reader
    /// hands back: 4,096 (85 ms).
    pub block_frames: usize,
    /// Blocks each stream holds, which is how far the disk thread reads
    /// ahead of the playhead: 8, so 32,768 frames (683 ms). They're
    /// allocated when the stream first plays, not when it opens.
    pub blocks: usize,
    /// Frames preloaded at the clip's start, and at each cued point: 16,384
    /// (341 ms). It's the time the disk thread has to catch up after a
    /// launch or a jump to a cue, so it covers a slow disk's seek and a
    /// pass over many streams.
    pub cue_frames: usize,
    /// Cue slots per stream, slot 0 being the clip's start: 3, so a clip
    /// loop's start and one more point (the arrangement loop's start, say)
    /// can be cued besides.
    pub cue_slots: usize,
    /// Frames a jump's fades last; `None` for 4 ms at the engine's rate.
    pub fade_frames: Option<usize>,
    /// How often the disk thread checks its streams while any is playing:
    /// 2 ms. A jump nothing was cued for waits about this long, plus the
    /// time to read and decode the new position.
    pub poll: Duration,
    /// Files shorter than this load into memory instead of streaming, in
    /// [`Streamer::source`]: 10 seconds. Short files cost less to hold than
    /// to stream, and the media store shares one decoded copy between every
    /// clip that plays them.
    pub memory_below: Duration,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            block_frames: 4096,
            blocks: 8,
            cue_frames: 16_384,
            cue_slots: 3,
            fade_frames: None,
            poll: Duration::from_millis(2),
            memory_below: Duration::from_secs(10),
        }
    }
}

/// Disk streaming: a thread that reads long files from disk ahead of the
/// playhead, for every stream it has opened.
///
/// [`Streamer::open`] decodes the start of a clip there and then, on the
/// calling thread, and returns the [`StreamSource`] the audio thread reads;
/// from then on the disk thread keeps each source's blocks filled ahead of
/// where it reads, and serves its jumps and [cues](Self::cue). The audio
/// thread never waits on it (see [`StreamSource`]).
///
/// One disk thread serves every stream, a block at a time, round-robin.
/// Dropping the streamer stops the thread; its sources then play silence.
pub struct Streamer {
    config: StreamConfig,
    control: Sender<Control>,
    thread: Option<JoinHandle<()>>,
    totals: Arc<Totals>,
    next_id: AtomicU64,
    started: Instant,
}

impl Streamer {
    /// Starts the disk thread.
    ///
    /// # Errors
    ///
    /// If the thread can't start.
    ///
    /// # Panics
    ///
    /// If a size in `config` is zero.
    pub fn start(config: StreamConfig) -> io::Result<Self> {
        assert!(
            config.block_frames > 0 && config.blocks > 0 && config.cue_frames > 0,
            "a stream needs blocks and cues of at least one frame: {config:?}"
        );
        let totals = Arc::new(Totals::default());
        let (control, requests) = mpsc::channel();
        let thread = {
            let totals = Arc::clone(&totals);
            thread::Builder::new()
                .name("bl-media-disk".into())
                .spawn(move || run(&requests, &totals, &config))?
        };
        Ok(Self {
            config,
            control,
            thread: Some(thread),
            totals,
            next_id: AtomicU64::new(1),
            started: Instant::now(),
        })
    }

    /// How it streams.
    #[must_use]
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    /// Opens a stream of `decoder`'s file at the engine's `rate`, with its
    /// audio from file frame `start` (at `rate`) preloaded into cue slot 0:
    /// a clip's start. Decodes the preload on this thread before it returns,
    /// so the source can play from `start` without waiting on the disk.
    ///
    /// # Errors
    ///
    /// If the rate is zero, the converter can't be set up, or decoding the
    /// start fails.
    pub fn open(
        &self,
        decoder: Decoder,
        rate: u32,
        start: u64,
    ) -> Result<StreamSource, MediaError> {
        if rate == 0 {
            return Err(MediaError::Streaming("an engine rate of zero".into()));
        }
        let config = &self.config;
        let mut feed = Feed::new(decoder, rate)?;
        let channels = feed.channels();
        let mut samples = vec![0.0f32; channels * config.cue_frames].into_boxed_slice();
        feed.seek(start)?;
        let len = feed.fill(&mut samples, config.cue_frames, config.cue_frames)?;
        let target = start + len as u64;
        let end = len < config.cue_frames || feed.length().is_some_and(|l| target >= l);
        let ends_at = end.then(|| feed.end_from(start, len));
        let preload = Cue {
            slot: 0,
            start,
            len,
            ends_at,
            samples,
        };

        let id = self.next_id.fetch_add(1, Relaxed);
        let shared = Arc::new(Shared::new(id, 1, target, Arc::clone(&self.totals)));
        let slots = config.cue_slots.max(1);
        let to_reader_capacity = config.blocks + slots + 2;
        let (to_reader, from_disk) = RingBuffer::new(to_reader_capacity);
        // Room for every buffer the reader could ever hold or be sent, so
        // handing one back never fails on the audio thread.
        let (to_disk, from_reader) = RingBuffer::new(config.blocks + slots + to_reader_capacity);
        let length = ends_at.or(feed.length());

        let disk = DiskStream {
            shared: Arc::clone(&shared),
            feed,
            to_reader,
            from_reader,
            channels,
            block_frames: config.block_frames,
            cue_frames: config.cue_frames,
            max_blocks: config.blocks,
            pool: Vec::new(),
            allocated: 0,
            cue_pool: Vec::new(),
            epoch: 1,
            next: target,
            done: end,
            failed: false,
            end_pending: false,
            cues: Vec::new(),
            reported_underruns: 0,
        };
        let mut cues: Box<[Option<Cue>]> = (0..slots).map(|_| None).collect();
        cues[0] = Some(preload);
        let streamed = Streamed {
            shared,
            from_disk,
            to_disk,
            channels,
            block_frames: config.block_frames,
            cue_frames: config.cue_frames,
            length,
            ring: Ring::new(config.blocks),
            cues,
            epoch: 1,
            expect: target,
            armed: false,
            blocking: false,
        };
        self.control
            .send(Control::Open(Box::new(disk)))
            .map_err(|_| MediaError::Streaming("the disk thread has stopped".into()))?;
        self.totals.streams.fetch_add(1, Relaxed);
        self.wake();
        let fade = config.fade_frames.unwrap_or_else(|| fade_frames(rate));
        Ok(StreamSource::new(streamed, rate, start, fade))
    }

    /// Opens a stream of media from the store, as [`Streamer::open`] does,
    /// after the store checks the file still holds it.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::open`] and [`Streamer::open`].
    pub fn open_media(
        &self,
        store: &MediaStore,
        hash: ContentHash,
        rate: u32,
        start: u64,
    ) -> Result<StreamSource, MediaError> {
        self.open(store.open(hash)?, rate, start)
    }

    /// A source for a clip that plays media from the store at the engine's
    /// `rate` from file frame `start`: in memory if the file is shorter than
    /// [`StreamConfig::memory_below`], or streamed otherwise.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::load_at_rate`] or [`Streamer::open_media`].
    pub fn source(
        &self,
        store: &MediaStore,
        hash: ContentHash,
        rate: u32,
        start: u64,
    ) -> Result<Source, MediaError> {
        let info = store.info(hash)?;
        let short = info.frames.is_some_and(|frames| {
            (frames as f64) < self.config.memory_below.as_secs_f64() * f64::from(info.sample_rate)
        });
        if short {
            let data = store.load_at_rate(hash, rate)?;
            let fade = self.config.fade_frames.unwrap_or_else(|| fade_frames(rate));
            Ok(Source::Memory(MemorySource::with_fade(data, fade)))
        } else {
            Ok(Source::Stream(self.open_media(store, hash, rate, start)?))
        }
    }

    /// Preloads a stream's audio from engine-rate frame `frame` into cue
    /// slot `slot`, so a later jump there plays at once: a clip loop's start,
    /// the arrangement loop's start, or the clip's new start after an edit
    /// (slot 0). The disk thread reads it soon after; until then the slot
    /// keeps what it had.
    ///
    /// # Errors
    ///
    /// If the slot is out of range or the disk thread has stopped.
    pub fn cue(&self, stream: StreamId, slot: usize, frame: u64) -> Result<(), MediaError> {
        if slot >= self.config.cue_slots.max(1) {
            return Err(MediaError::Streaming(format!(
                "cue slot {slot} of {}",
                self.config.cue_slots.max(1)
            )));
        }
        self.control
            .send(Control::Cue {
                id: stream.0,
                slot,
                frame,
            })
            .map_err(|_| MediaError::Streaming("the disk thread has stopped".into()))?;
        self.wake();
        Ok(())
    }

    /// The disk thread's figures. Underruns are what the disk overload
    /// indicator shows: watch [`StreamStats::underruns`] in
    /// [`StreamerStats::totals`] rise.
    #[must_use]
    pub fn stats(&self) -> StreamerStats {
        StreamerStats {
            streams: self.totals.streams.load(Relaxed),
            totals: self.totals.counters.snapshot(),
            busy: Duration::from_nanos(self.totals.busy_nanos.load(Relaxed)),
        }
    }

    /// How long the streamer has run.
    #[must_use]
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Stops the disk thread filling streams until [`Streamer::resume`], and
    /// returns once it has stopped (or after a second, if it doesn't answer).
    /// For tests and diagnostics: it's how a test shows a preloaded or cued
    /// start plays without the disk, and what an underrun does.
    pub fn pause(&self) {
        if self.control.send(Control::Pause(true)).is_err() {
            return;
        }
        self.wake();
        let began = Instant::now();
        while !self.totals.paused.load(Acquire) && began.elapsed() < Duration::from_secs(1) {
            thread::sleep(Duration::from_micros(100));
        }
    }

    /// Lets the disk thread fill streams again after [`Streamer::pause`].
    pub fn resume(&self) {
        let _ = self.control.send(Control::Pause(false));
        self.wake();
    }

    fn wake(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        self.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl fmt::Debug for Streamer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Streamer")
            .field("config", &self.config)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

/// What the main thread asks of the disk thread.
enum Control {
    Open(Box<DiskStream>),
    Cue { id: u64, slot: usize, frame: u64 },
    Pause(bool),
    Stop,
}

/// The disk thread: takes requests, then fills streams a block at a time,
/// round-robin, until none needs anything, and sleeps until the next poll.
fn run(requests: &Receiver<Control>, totals: &Totals, config: &StreamConfig) {
    bl_rt::log::register_current_thread();
    let mut streams: Vec<DiskStream> = Vec::new();
    let mut paused = false;
    let mut first = 0;
    loop {
        loop {
            match requests.try_recv() {
                Ok(Control::Open(stream)) => streams.push(*stream),
                Ok(Control::Cue { id, slot, frame }) => {
                    if let Some(stream) = streams.iter_mut().find(|s| s.shared.id == id) {
                        stream.request_cue(slot, frame);
                    }
                }
                Ok(Control::Pause(pause)) => {
                    paused = pause;
                    totals.paused.store(pause, Release);
                }
                Ok(Control::Stop) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }

        // A stream whose reader has gone is closed; its buffers are freed
        // here.
        let open = streams.len();
        streams.retain(|stream| !stream.to_reader.is_abandoned());
        if streams.len() < open {
            totals.streams.fetch_sub(open - streams.len(), Relaxed);
        }

        let began = Instant::now();
        let mut worked = false;
        if !paused && !streams.is_empty() {
            let count = streams.len();
            for i in 0..count {
                worked |= streams[(first + i) % count].service();
            }
            first = (first + 1) % count;
        }
        if worked {
            let spent = u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX);
            totals.busy_nanos.fetch_add(spent, Relaxed);
        }
        for stream in &mut streams {
            stream.report();
        }
        if !worked {
            let playing = streams.iter().any(|s| s.shared.armed.load(Relaxed));
            thread::park_timeout(if playing { config.poll } else { IDLE_POLL });
        }
    }
}

/// The disk thread's end of one stream.
struct DiskStream {
    shared: Arc<Shared>,
    feed: Feed,
    to_reader: Producer<ToReader>,
    from_reader: Consumer<ToDisk>,
    channels: usize,
    block_frames: usize,
    cue_frames: usize,
    max_blocks: usize,
    /// Block buffers the reader has handed back.
    pool: Vec<Box<[f32]>>,
    /// Block buffers made so far, at most `max_blocks`.
    allocated: usize,
    cue_pool: Vec<Box<[f32]>>,
    /// The sequence being read, and where its next block starts.
    epoch: u64,
    next: u64,
    /// Whether the sequence has reached the end of the file.
    done: bool,
    /// Whether reading the file failed; it plays silence from there.
    failed: bool,
    /// Whether the reader is still to be told the sequence ends at `next`:
    /// it starts past the end, or reading failed.
    end_pending: bool,
    /// Cues to read: slot and frame, the latest per slot.
    cues: Vec<(usize, u64)>,
    reported_underruns: u64,
}

impl DiskStream {
    fn request_cue(&mut self, slot: usize, frame: u64) {
        self.cues.retain(|&(s, _)| s != slot);
        self.cues.push((slot, frame));
    }

    /// One pass: takes back spent buffers, follows the reader's jumps, and
    /// reads one cue or one block if one is wanted. Returns whether it read
    /// anything.
    fn service(&mut self) -> bool {
        while let Ok(message) = self.from_reader.pop() {
            match message {
                ToDisk::Block(buffer) if !buffer.is_empty() => self.pool.push(buffer),
                ToDisk::Cue(buffer) if !buffer.is_empty() => self.cue_pool.push(buffer),
                _ => {}
            }
        }
        let epoch = self.shared.epoch.load(Acquire);
        if epoch != self.epoch {
            self.epoch = epoch;
            self.next = self.shared.target.load(Relaxed);
            self.done = self.feed.length().is_some_and(|l| self.next >= l);
            self.end_pending = self.done || self.failed;
        }
        if self.end_pending {
            return self.send_end();
        }
        if self.failed || self.to_reader.slots() == 0 {
            return false;
        }
        if !self.cues.is_empty() {
            let (slot, frame) = self.cues.remove(0);
            if let Err(error) = self.read_cue(slot, frame) {
                self.fail(&error);
            }
            return true;
        }
        if self.done || !self.shared.armed.load(Acquire) {
            return false;
        }
        let Some(buffer) = self.take_buffer() else {
            return false;
        };
        if let Err(error) = self.read_block(buffer) {
            self.fail(&error);
        }
        true
    }

    fn take_buffer(&mut self) -> Option<Box<[f32]>> {
        if let Some(buffer) = self.pool.pop() {
            return Some(buffer);
        }
        if self.allocated == self.max_blocks {
            return None;
        }
        self.allocated += 1;
        Some(vec![0.0; self.channels * self.block_frames].into_boxed_slice())
    }

    fn read_block(&mut self, mut samples: Box<[f32]>) -> Result<(), MediaError> {
        if self.feed.position() != self.next {
            self.feed.seek(self.next)?;
        }
        let start = self.next;
        let len = self
            .feed
            .fill(&mut samples, self.block_frames, self.block_frames)?;
        let end =
            len < self.block_frames || self.feed.length().is_some_and(|l| start + len as u64 >= l);
        self.next = start + len as u64;
        self.done = end;
        let block = Block {
            epoch: self.epoch,
            start,
            len,
            ends_at: end.then(|| self.feed.end_from(start, len)),
            samples,
        };
        // Can't fail: there's one producer, and `service` saw a free slot.
        let _ = self.to_reader.push(ToReader::Block(block));
        self.shared.count(&StreamStats {
            blocks: 1,
            ..StreamStats::default()
        });
        Ok(())
    }

    fn read_cue(&mut self, slot: usize, frame: u64) -> Result<(), MediaError> {
        let mut samples = self
            .cue_pool
            .pop()
            .unwrap_or_else(|| vec![0.0; self.channels * self.cue_frames].into_boxed_slice());
        self.feed.seek(frame)?;
        let len = self
            .feed
            .fill(&mut samples, self.cue_frames, self.cue_frames)?;
        let end =
            len < self.cue_frames || self.feed.length().is_some_and(|l| frame + len as u64 >= l);
        let cue = Cue {
            slot,
            start: frame,
            len,
            ends_at: end.then(|| self.feed.end_from(frame, len)),
            samples,
        };
        let _ = self.to_reader.push(ToReader::Cue(cue));
        Ok(())
    }

    /// Tells the reader the current sequence ends at `next`.
    fn send_end(&mut self) -> bool {
        if self.to_reader.slots() == 0 {
            return false;
        }
        // A failed file ends where it failed; otherwise the sequence was
        // asked for at or past the end.
        let ends_at = if self.failed {
            self.next
        } else {
            self.feed.end_from(self.next, 0)
        };
        let block = Block {
            epoch: self.epoch,
            start: self.next,
            len: 0,
            ends_at: Some(ends_at),
            samples: Box::default(),
        };
        let _ = self.to_reader.push(ToReader::Block(block));
        self.end_pending = false;
        true
    }

    fn fail(&mut self, error: &MediaError) {
        if !self.failed {
            self.failed = true;
            self.end_pending = true;
            self.shared.count(&StreamStats {
                errors: 1,
                ..StreamStats::default()
            });
            rt_log!(
                Level::Error,
                "disk stream {} stopped at frame {}: the file failed to read or decode ({})",
                self.shared.id,
                self.next,
                error_kind(error)
            );
        }
    }

    /// Logs new underruns, so they reach the log as well as the counters.
    fn report(&mut self) {
        let stats = self.shared.counters.snapshot();
        if stats.underruns > self.reported_underruns {
            self.reported_underruns = stats.underruns;
            rt_log!(
                Level::Warn,
                "disk stream {} underran: {} underruns, {} frames of silence so far",
                self.shared.id,
                stats.underruns,
                stats.underrun_frames
            );
        }
    }
}

/// A fixed word for a media error, since a log record carries only static
/// text and numbers.
fn error_kind(error: &MediaError) -> &'static str {
    match error {
        MediaError::Io(_) => "a read error",
        MediaError::Unsupported(_) => "unsupported media",
        MediaError::Malformed(_) => "damaged media",
        MediaError::Resample(_) => "a rate conversion error",
        _ => "an error",
    }
}
