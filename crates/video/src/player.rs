//! The video thread and the frame cache (R27): frames decoded ahead of the
//! time asked for, the ones done with dropped, and nothing here waits for or
//! signals an audio thread: the audio thread never calls into this module.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use crate::decoder::{CodedSample, Frame, VideoDecoder, open_decoder};
use crate::error::VideoError;
use crate::movie::{Movie, ReadSeek};
use crate::rational::Rational;

/// What the cache holds for the picture at a time.
#[derive(Clone, Debug)]
pub enum Picture {
    /// The decoded frame.
    Frame(Arc<Frame>),
    /// The movie has no video at that time, or the frame is damaged: black
    /// (R4, R28).
    Black,
    /// Not decoded yet. A window shows the last picture it had (R23).
    Pending,
}

/// Decoded frames by presentation rank. A `None` is a frame whose decoding
/// failed: black.
type Cache = BTreeMap<u32, Option<Arc<Frame>>>;

/// The most bytes of decoded pictures the cache aims to hold ahead.
const AHEAD_BYTES: u64 = 192 << 20;
/// The most frames decoded ahead at once.
const MAX_AHEAD: u32 = 32;

/// Decodes frames on request: the part of the video thread that runs
/// without a lock, and what a caller without a thread uses directly.
struct Decoding {
    movie: Arc<Movie>,
    reader: Box<dyn ReadSeek>,
    decoder: Box<dyn VideoDecoder>,
    /// `rank_of[index]`: where the frame stands in presentation order.
    rank_of: Vec<u32>,
    ahead: u32,
}

impl Decoding {
    fn new(movie: Arc<Movie>) -> Result<Self, VideoError> {
        let track = movie.video().ok_or(VideoError::NoVideo)?;
        let decoder = open_decoder(track)?;
        let mut rank_of = vec![0; track.frame_count()];
        for (rank, &index) in track.presentation_order().iter().enumerate() {
            rank_of[index as usize] = rank as u32;
        }
        let frame_bytes = u64::from(track.width()) * u64::from(track.height()) * 4;
        let ahead = (AHEAD_BYTES / frame_bytes.max(1)).clamp(2, u64::from(MAX_AHEAD)) as u32;
        let reader = movie.reader()?;
        Ok(Self {
            movie,
            reader,
            decoder,
            rank_of,
            ahead,
        })
    }

    /// The presentation rank of the frame to show at `t`; in a gap, of the
    /// first frame after it, so decoding is ready for the gap's end.
    fn rank_at(&self, t: Rational) -> Option<u32> {
        if let Some(index) = self.movie.frame_at(t) {
            return Some(self.rank_of[index]);
        }
        let track = self.movie.video()?;
        track
            .presentation_order()
            .iter()
            .position(|&i| track.frame_start(i as usize).is_some_and(|s| s >= t))
            .map(|rank| rank as u32)
    }

    /// Makes `cache` hold the window from `rank` on: drops what's behind it
    /// or past it, and decodes what's missing. Does nothing while at least
    /// half the window is already there, so a playing movie decodes in
    /// batches and not a frame at a time.
    fn fill(&mut self, cache: &mut Cache, rank: u32) {
        let end = rank.saturating_add(self.ahead);
        cache.retain(|&r, _| (rank..end).contains(&r));
        let ready = (rank..end).take_while(|r| cache.contains_key(r)).count() as u32;
        if ready >= self.ahead.div_ceil(2) || rank as usize >= self.rank_of.len() {
            return;
        }
        let missing: Vec<u32> = (rank..end)
            .take_while(|&r| (r as usize) < self.rank_of.len())
            .filter(|r| !cache.contains_key(r))
            .collect();
        cache.extend(self.decode(&missing));
    }

    /// Decodes the frames at `ranks`: each group that shares a sync frame is
    /// decoded from that frame to its last one.
    fn decode(&mut self, ranks: &[u32]) -> Vec<(u32, Option<Arc<Frame>>)> {
        let movie = Arc::clone(&self.movie);
        let Some(track) = movie.video() else {
            return Vec::new();
        };
        let mut wanted: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for &rank in ranks {
            let index = track.presentation_order()[rank as usize] as usize;
            wanted
                .entry(track.sync_before(index))
                .or_default()
                .insert(index);
        }
        let mut done = Vec::new();
        for (sync, indices) in wanted {
            let last = *indices.last().unwrap_or(&sync);
            // A frame the file can't give is skipped: the decoder shows black
            // for it and goes on with the rest.
            let coded: Vec<(usize, i64, Vec<u8>)> = (sync..=last)
                .filter_map(|i| {
                    let bytes = track.read_sample(&mut *self.reader, i).ok()?;
                    Some((i, track.sample(i)?.pts, bytes))
                })
                .collect();
            let samples: Vec<CodedSample<'_>> = coded
                .iter()
                .map(|(index, pts, data)| CodedSample {
                    index: *index,
                    pts: *pts,
                    data,
                })
                .collect();
            let mut frames: BTreeMap<usize, Frame> = BTreeMap::new();
            // A decoder that fails as a whole leaves every frame black.
            let _ = self.decoder.decode(&samples, &mut |index, frame| {
                if indices.contains(&index) {
                    frames.insert(index, frame);
                }
            });
            for index in indices {
                done.push((self.rank_of[index], frames.remove(&index).map(Arc::new)));
            }
        }
        done
    }
}

fn picture(movie: &Movie, cache: &Cache, rank_of: &[u32], t: Rational) -> Picture {
    let Some(index) = movie.frame_at(t) else {
        return Picture::Black;
    };
    match cache.get(&rank_of[index]) {
        Some(Some(frame)) => Picture::Frame(Arc::clone(frame)),
        Some(None) => Picture::Black,
        None => Picture::Pending,
    }
}

/// Decoding on the calling thread: for export, which asks for each frame in
/// turn and may wait for it, and for tests.
pub struct FrameSource {
    decoding: Decoding,
    cache: Cache,
}

impl FrameSource {
    /// Opens a decoder for the movie's video.
    ///
    /// # Errors
    ///
    /// [`VideoError::NoVideo`] without a picture, [`VideoError::NoDecoder`] if
    /// no decoder takes it, or a read error.
    pub fn new(movie: Arc<Movie>) -> Result<Self, VideoError> {
        Ok(Self {
            decoding: Decoding::new(movie)?,
            cache: Cache::new(),
        })
    }

    /// The picture at `t`, decoded now if it isn't in the cache, with the
    /// frames after it. Never [`Picture::Pending`].
    pub fn picture_at(&mut self, t: Rational) -> Picture {
        if let Some(rank) = self.decoding.rank_at(t) {
            self.decoding.fill(&mut self.cache, rank);
        }
        picture(&self.decoding.movie, &self.cache, &self.decoding.rank_of, t)
    }
}

/// What the video thread and its callers share.
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    movie: Arc<Movie>,
    rank_of: Vec<u32>,
}

struct State {
    /// The latest time asked for, and its sequence number.
    want: Option<(u64, Rational)>,
    /// The sequence number the thread has finished.
    handled: u64,
    cache: Cache,
    quit: bool,
}

/// A thread that decodes the frames at and after the time asked for into a
/// cache (R27). Asking is quick and never waits for a decode; neither does
/// reading the cache (R23).
pub struct VideoThread {
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl VideoThread {
    /// Starts the thread on a movie's video.
    ///
    /// # Errors
    ///
    /// As for [`FrameSource::new`].
    pub fn spawn(movie: Arc<Movie>) -> Result<Self, VideoError> {
        let mut decoding = Decoding::new(Arc::clone(&movie))?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                want: None,
                handled: 0,
                cache: Cache::new(),
                quit: false,
            }),
            changed: Condvar::new(),
            rank_of: decoding.rank_of.clone(),
            movie,
        });
        let thread_shared = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("bl-video".into())
            .spawn(move || run(&thread_shared, &mut decoding))
            .map_err(VideoError::Io)?;
        Ok(Self {
            shared,
            join: Some(join),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A thread that panicked leaves a cache that is still only frames.
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks for the frames at `t` and after. Returns at once.
    pub fn request(&self, t: Rational) {
        let mut state = self.lock();
        let seq = state.want.map_or(1, |(seq, _)| seq + 1);
        state.want = Some((seq, t));
        self.shared.changed.notify_all();
    }

    /// The picture at `t` from what's been decoded. Returns at once.
    #[must_use]
    pub fn picture_at(&self, t: Rational) -> Picture {
        picture(
            &self.shared.movie,
            &self.lock().cache,
            &self.shared.rank_of,
            t,
        )
    }

    /// Blocks until the thread has done what was last asked of it. For
    /// export and tests: a window never calls it.
    pub fn wait_idle(&self) {
        let mut state = self.lock();
        while !state.quit && state.want.is_some_and(|(seq, _)| state.handled < seq) {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Drop for VideoThread {
    fn drop(&mut self) {
        self.lock().quit = true;
        self.shared.changed.notify_all();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(shared: &Shared, decoding: &mut Decoding) {
    loop {
        let (seq, t, mut cache) = {
            let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if state.quit {
                    return;
                }
                match state.want {
                    Some((seq, t)) if state.handled < seq => {
                        break (seq, t, state.cache.clone());
                    }
                    _ => {
                        state = shared
                            .changed
                            .wait(state)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                }
            }
        };
        // The decode works on a copy and publishes it whole, so a window
        // reading the cache meanwhile never waits on a lock held across a
        // decode, and never sees frames missing that it had.
        if let Some(rank) = decoding.rank_at(t) {
            decoding.fill(&mut cache, rank);
        }
        let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.cache = cache;
        state.handled = state.handled.max(seq);
        shared.changed.notify_all();
    }
}
