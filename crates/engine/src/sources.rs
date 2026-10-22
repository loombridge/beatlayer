//! Where each clip's source comes from (WP-1.4b): the compiler's record of
//! which source every clip player holds, the loader thread that opens new
//! ones, and the handoff that moves sources to the audio thread.
//!
//! Opening a source reads and decodes: a short file whole, a long one's
//! start (disk streaming's preload). The compiler thread never does that
//! for a running engine, so a long file can't hold up Play or Stop behind
//! it: it asks the [`Loader`] for the clips that need a source, leaves them
//! out of their track's list meanwhile (they're silent), and sends the
//! track its clips again when the source is ready (the audio clips spec,
//! R17 and R18). An offline render has no loader: its compiler opens
//! sources where it is, which is the thread doing the rendering.
//!
//! A source holds a stream's position and buffers, so it belongs to one
//! clip in one player. A new list for a player carries over the sources of
//! the clips it kept: the list says which, and the player moves them from
//! its old list on the audio thread, which moves pointers and allocates
//! nothing. Sources the new list doesn't keep go with the old list to the
//! drop thread.

use std::collections::BTreeMap;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering::AcqRel};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use bl_core::time::SampleRate;
use bl_model::{ClipId, ContentHash, TrackId};

use crate::clip_source::ClipSource;
use crate::clips::{Geometry, MediaLibrary, OpenedSource, SourceCues};

/// A value that one thread puts in and another takes out, once, without a
/// lock: how a list's sources reach the audio thread inside data that's
/// otherwise shared and read-only.
pub(crate) struct Handoff<T> {
    slot: AtomicPtr<T>,
}

impl<T> Handoff<T> {
    /// A handoff holding `value`.
    pub(crate) fn new(value: Box<T>) -> Self {
        Self {
            slot: AtomicPtr::new(Box::into_raw(value)),
        }
    }

    /// An empty handoff, holding nothing: what a list carries where it keeps
    /// the value the player already has rather than replacing it, so `take`
    /// says so with `None`.
    pub(crate) fn empty() -> Self {
        Self {
            slot: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Takes the value out, if it's there. Real-time safe: one atomic swap.
    pub(crate) fn take(&self) -> Option<Box<T>> {
        let value = self.slot.swap(ptr::null_mut(), AcqRel);
        // SAFETY: a non-null pointer in the slot came from `Box::into_raw`
        // in `new` or `put`, and the swap took it out, so this is its only
        // owner.
        (!value.is_null()).then(|| unsafe { Box::from_raw(value) })
    }

    /// Puts a value back in an empty handoff, so it's freed with it: where
    /// the audio thread leaves what it mustn't free. Gives the value back
    /// if the handoff isn't empty.
    pub(crate) fn put(&self, value: Box<T>) -> Result<(), Box<T>> {
        let raw = Box::into_raw(value);
        match self.slot.compare_exchange(
            ptr::null_mut(),
            raw,
            AcqRel,
            std::sync::atomic::Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            // SAFETY: the exchange failed, so `raw` is still only ours, made
            // by `Box::into_raw` just above.
            Err(_) => Err(unsafe { Box::from_raw(raw) }),
        }
    }
}

impl<T> Drop for Handoff<T> {
    fn drop(&mut self) {
        drop(self.take());
    }
}

// SAFETY: the value moves between threads whole, through an atomic swap, and
// only the thread that took it touches it; nothing reads it through a shared
// reference. That needs only `T: Send`, as for `Mutex<T>`.
unsafe impl<T: Send> Sync for Handoff<T> {}
// SAFETY: as above; the handoff owns its value.
unsafe impl<T: Send> Send for Handoff<T> {}

/// A clip's source slot in a player: the source, while the clip has one.
pub(crate) type Slot = Option<Box<ClipSource>>;

/// A list's sources, one slot per clip in the list's order.
pub(crate) type Bank = Box<[Slot]>;

/// What a source was opened for: a clip's media at the engine's rate, held
/// in memory or not (the clip's RAM switch, the warp modes spec, R11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    media: ContentHash,
    rate: SampleRate,
    ram: bool,
}

/// The cue slots a clip's source keeps: its start, its loop's start, and
/// the Arrangement loop's start (the disk streaming spec, R2).
pub(crate) const CUE_SLOTS: usize = 3;

/// A source the compiler knows a player holds.
struct Held {
    key: Key,
    /// The source's length in frames, if it knows it.
    frames: Option<u64>,
    cues: Option<Arc<dyn SourceCues>>,
    /// What each cue slot was last asked to hold.
    cued: [Option<u64>; CUE_SLOTS],
    /// Where the clip was placed when its cues were last set.
    geometry: Option<Geometry>,
}

impl Held {
    /// Asks the source to preload the points a jump will land on: the
    /// clip's start, its loop's start, and where the Arrangement loop starts,
    /// `loop_start` (a song frame), if that's inside the clip. Only what
    /// changed is asked for; a source in memory has nothing to preload.
    fn cue(&mut self, loop_start: Option<i64>) {
        let (Some(cues), Some(geometry)) = (&self.cues, &self.geometry) else {
            return;
        };
        // A warped clip reads a little before the point it plays at.
        let back = geometry.reach_back();
        let wanted = [
            Some(geometry.region_start.saturating_sub(back)),
            geometry
                .looping
                .then_some(geometry.loop_start.saturating_sub(back)),
            loop_start
                .filter(|&at| geometry.start <= at && at < geometry.end)
                .and_then(|at| geometry.file_frame(at))
                .map(|frame| frame.saturating_sub(back)),
        ];
        for (slot, (want, had)) in wanted.into_iter().zip(&mut self.cued).enumerate() {
            if let Some(frame) = want
                && *had != Some(frame)
            {
                cues.cue(slot, frame);
                *had = Some(frame);
            }
        }
    }
}

/// A source opened and not yet in a list.
struct Ready {
    key: Key,
    opened: OpenedSource,
    /// Where it was opened, which its first cue slot holds.
    start: u64,
}

/// An open the loader hasn't answered.
struct Opening {
    key: Key,
    ticket: u64,
}

/// A request to the loader.
struct Job {
    clip: ClipId,
    ticket: u64,
    key: Key,
    start: u64,
}

/// Opens the source `key` names: the whole file in memory if its RAM switch
/// is on, otherwise as the library chooses.
fn open(library: &dyn MediaLibrary, key: Key, start: u64) -> Option<OpenedSource> {
    if key.ram {
        library.open_in_memory(key.media, key.rate)
    } else {
        library.open(key.media, key.rate, start)
    }
}

/// The loader's answer: the source it opened for a clip, or `None` if the
/// media is missing, has changed, or doesn't decode.
pub(crate) struct Loaded {
    clip: ClipId,
    ticket: u64,
    key: Key,
    start: u64,
    opened: Option<OpenedSource>,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("clip", &self.clip)
            .field("ticket", &self.ticket)
            .field("opened", &self.opened.is_some())
            .finish_non_exhaustive()
    }
}

/// A thread that opens clips' sources, one after another, and hands each
/// to `deliver`.
pub(crate) struct Loader {
    jobs: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Loader {
    /// Starts the thread, opening sources from `media`.
    pub(crate) fn start(
        media: Arc<dyn MediaLibrary>,
        deliver: impl Fn(Loaded) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let thread = thread::Builder::new()
            .name("bl-engine-loader".into())
            .spawn(move || {
                for job in queue {
                    let opened = open(&*media, job.key, job.start)
                        .filter(|opened| opened.source.channels() > 0);
                    deliver(Loaded {
                        clip: job.clip,
                        ticket: job.ticket,
                        key: job.key,
                        start: job.start,
                        opened,
                    });
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // Closing the queue ends the thread after the open it's doing.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How a clip in a new list gets its source.
pub(crate) enum Supply {
    /// The player carries over the source it has for the clip.
    Carry,
    /// A source opened for it.
    Fresh(Box<ClipSource>),
    /// The player carries over the source it has, and hands it over to this
    /// one once this one is ready: the clip's RAM switch changed while it
    /// played (the warp modes spec, R11).
    Switch(Box<ClipSource>),
}

/// A clip's source in a list being built, and what the list needs to know
/// of it.
pub(crate) struct Sourced {
    pub(crate) supply: Supply,
    /// The source's length in frames, if known.
    pub(crate) frames: Option<u64>,
}

/// The compiler's record of clips' sources: which each player holds, which
/// are open and waiting for a list, and which the loader is opening.
#[derive(Default)]
pub(crate) struct ClipSources {
    loader: Option<Loader>,
    held: BTreeMap<TrackId, BTreeMap<ClipId, Held>>,
    /// The clips of the list being built, and their sources' records, which
    /// replace the track's in `held` when the list is done.
    building: BTreeMap<ClipId, Held>,
    ready: BTreeMap<ClipId, Ready>,
    opening: BTreeMap<ClipId, Opening>,
    next_ticket: u64,
}

impl ClipSources {
    /// From now on, opens sources on `loader`'s thread rather than the
    /// compiler's.
    pub(crate) fn set_loader(&mut self, loader: Loader) {
        self.loader = Some(loader);
    }

    /// Forgets every source: the players are new (a reset, or a new rate),
    /// so none holds one. Opens under way are answered and ignored.
    pub(crate) fn reset(&mut self) {
        self.held.clear();
        self.building.clear();
        self.ready.clear();
        self.opening.clear();
    }

    /// Forgets players and open sources of tracks and clips `keep` says are
    /// gone.
    pub(crate) fn retain(
        &mut self,
        mut keep_track: impl FnMut(TrackId) -> bool,
        mut keep_clip: impl FnMut(ClipId) -> bool,
    ) {
        self.held.retain(|&track, _| keep_track(track));
        self.ready.retain(|&clip, _| keep_clip(clip));
    }

    /// Starts a list for `track`'s player.
    pub(crate) fn begin(&mut self) {
        self.building.clear();
    }

    /// The source for `clip` in the list being built for `track`'s player,
    /// whose clip plays `media` at `rate` from file frame `start`: the one
    /// the player holds, one ready, or one opened now (without a loader).
    /// `None` while the loader opens one, which it's asked to, or if the
    /// media can't be had: `missing` then says which.
    #[expect(
        clippy::too_many_arguments,
        reason = "a clip's source has this many parts"
    )]
    pub(crate) fn source(
        &mut self,
        track: TrackId,
        clip: ClipId,
        media: ContentHash,
        rate: SampleRate,
        ram: bool,
        start: u64,
        library: Option<&dyn MediaLibrary>,
        missing: &mut Vec<ContentHash>,
    ) -> Option<Sourced> {
        let key = Key { media, rate, ram };
        let held = self
            .held
            .get_mut(&track)
            .and_then(|clips| clips.remove(&clip));
        // Only the RAM switch differs: the clip goes on playing the source
        // it has until the new one is ready, then hands over to it.
        let mut playing = None;
        if let Some(held) = held {
            if held.key == key {
                let frames = held.frames;
                self.building.insert(clip, held);
                return Some(Sourced {
                    supply: Supply::Carry,
                    frames,
                });
            }
            if held.key.media == media && held.key.rate == rate && self.loader.is_some() {
                playing = Some(held);
            }
        }
        let ready = match self.ready.remove(&clip) {
            Some(ready) if ready.key == key => Some(ready),
            _ => None,
        };
        let ready = match (ready, &self.loader) {
            (Some(ready), _) => ready,
            (None, None) => {
                let Some(opened) = library
                    .and_then(|library| open(library, key, start))
                    .filter(|opened| opened.source.channels() > 0)
                else {
                    missing.push(media);
                    return None;
                };
                Ready { key, opened, start }
            }
            (None, Some(loader)) => {
                if library.is_none() {
                    missing.push(media);
                    return None;
                }
                let asked = self.opening.get(&clip).is_some_and(|o| o.key == key);
                if !asked {
                    self.next_ticket += 1;
                    let ticket = self.next_ticket;
                    self.opening.insert(clip, Opening { key, ticket });
                    loader.send(Job {
                        clip,
                        ticket,
                        key,
                        start,
                    });
                }
                // The clip plays on, from the source it has, meanwhile.
                let held = playing?;
                let frames = held.frames;
                self.building.insert(clip, held);
                return Some(Sourced {
                    supply: Supply::Carry,
                    frames,
                });
            }
        };
        let frames = ready.opened.source.frames();
        let streamed = ready.opened.cues.is_some();
        let mut cued = [None; CUE_SLOTS];
        cued[0] = Some(ready.start);
        self.building.insert(
            clip,
            Held {
                key,
                frames,
                cues: ready.opened.cues,
                cued,
                geometry: None,
            },
        );
        let source = Box::new(ClipSource::new(ready.opened.source, streamed));
        Some(Sourced {
            supply: if playing.is_some() {
                Supply::Switch(source)
            } else {
                Supply::Fresh(source)
            },
            frames,
        })
    }

    /// Places `clip`'s source, in the list being built, at `geometry`, and
    /// asks it to preload the points a jump will land on there.
    pub(crate) fn cue(&mut self, clip: ClipId, geometry: &Geometry, loop_start: Option<i64>) {
        if let Some(held) = self.building.get_mut(&clip) {
            held.geometry = Some(geometry.clone());
            held.cue(loop_start);
        }
    }

    /// The Arrangement loop now starts at song frame `loop_start` (or is
    /// off): every source the players hold preloads its new start.
    pub(crate) fn cue_loop(&mut self, loop_start: Option<i64>) {
        for held in self.held.values_mut().flat_map(BTreeMap::values_mut) {
            held.cue(loop_start);
        }
    }

    /// Ends the list for `track`'s player: the sources it holds are now the
    /// ones built.
    pub(crate) fn finish(&mut self, track: TrackId) {
        let built = std::mem::take(&mut self.building);
        if built.is_empty() {
            self.held.remove(&track);
        } else {
            self.held.insert(track, built);
        }
    }

    /// Takes the loader's answer. Returns the clip whose track needs its
    /// clips again (its source is ready), or, if the media can't be had,
    /// that media.
    pub(crate) fn loaded(&mut self, loaded: Loaded) -> Option<Result<ClipId, ContentHash>> {
        let current = self
            .opening
            .get(&loaded.clip)
            .is_some_and(|o| o.ticket == loaded.ticket && o.key == loaded.key);
        if !current {
            return None;
        }
        self.opening.remove(&loaded.clip);
        match loaded.opened {
            Some(opened) => {
                self.ready.insert(
                    loaded.clip,
                    Ready {
                        key: loaded.key,
                        opened,
                        start: loaded.start,
                    },
                );
                Some(Ok(loaded.clip))
            }
            None => Some(Err(loaded.key.media)),
        }
    }
}

impl std::fmt::Debug for ClipSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipSources")
            .field("loader", &self.loader.is_some())
            .field("players", &self.held.len())
            .field("ready", &self.ready.len())
            .field("opening", &self.opening.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handoff_gives_its_value_once_and_takes_one_back() {
        let handoff = Handoff::new(Box::new(5));
        assert_eq!(handoff.take().as_deref(), Some(&5));
        assert!(handoff.take().is_none());
        assert!(handoff.put(Box::new(6)).is_ok());
        assert_eq!(handoff.put(Box::new(7)).unwrap_err().as_ref(), &7);
        assert_eq!(handoff.take().as_deref(), Some(&6));
        // Dropped full, it frees what it holds.
        let full = Handoff::new(Box::new(vec![1u8; 16]));
        drop(full);
    }
}
