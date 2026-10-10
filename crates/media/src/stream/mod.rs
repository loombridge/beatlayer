//! Disk streaming: long files read from disk ahead of the playhead, on a
//! disk thread, with the start of every clip preloaded so a launch or a
//! locate never waits on the disk (WP-1.3; the behaviour is in
//! `docs/specs/disk-streaming.md`).
//!
//! - [`AudioSource`] is what a clip player reads, on the audio thread: a
//!   block of frames from any position, with jumps handled by a
//!   [`Transition`]. [`MemorySource`] holds a file decoded into memory,
//!   [`StreamSource`] streams one, and [`Source`] is either.
//! - [`Streamer`] runs the disk thread. [`Streamer::open`] decodes a clip's
//!   start on the calling thread and returns its [`StreamSource`];
//!   [`Streamer::source`] picks memory or streaming by the file's length;
//!   [`Streamer::cue`] preloads another point, such as a loop's start.
//!
//! # How a stream works
//!
//! The disk thread decodes each stream's file a block at a time (4,096
//! frames by default), converts it to the engine's rate if the file's
//! differs, and sends the blocks to the source over a lock-free queue
//! (`rtrb`). The source plays them and hands each one back over a second
//! queue once it's past it, so buffers are reused and are only ever freed on
//! the disk thread. A stream holds eight blocks, so the disk thread stays up
//! to 683 ms ahead at 48 kHz.
//!
//! Besides the blocks, each source keeps cues: stretches decoded in advance
//! (16,384 frames, 341 ms at 48 kHz) that stay until replaced. Slot 0 holds
//! the clip's start, decoded before the source exists; other slots hold
//! points a jump will land on, such as a loop's start. A jump to a cued
//! point plays from the cue at once, while the disk thread reads on from
//! the cue's end.
//!
//! A jump anywhere else asks the disk thread for a new sequence of blocks:
//! the source bumps a sequence number (its epoch) and the disk thread, on
//! its next pass (every 2 ms while anything plays), seeks there. Blocks of
//! an old sequence still in the queue are handed back unplayed. Until the
//! new blocks come, the source plays silence; with
//! [`Transition::Crossfade`] the old audio fades out over 4 ms meanwhile,
//! and the new audio fades in when it arrives, so the jump doesn't click.
//!
//! If the disk thread falls behind while a source plays on, the source
//! plays silence for what's missing, counts an underrun, and fades the audio
//! back in when it returns; the disk thread logs it, and
//! [`Streamer::stats`] reports it for the disk overload indicator. The
//! source never waits: the playhead doesn't stop for the disk.
//!
//! # Why not creek
//!
//! The architecture named `creek` (Meadowlark's streaming crate) for this.
//! WP-1.3 evaluated creek 1.2.3 (creek-core 0.2.3) and kept its design,
//! prefetch blocks recycled through `rtrb` queues and caches at chosen
//! frames, but not the crate:
//!
//! - each creek stream runs its own server thread, which sleeps 1 ms
//!   between polls, so 64 clips are 64 threads; here one disk thread serves
//!   every stream;
//! - a creek read that finds no data outputs silence without counting it
//!   (its source marks the spot "TODO: warn of buffer underflow"), and
//!   a full message queue fails the read;
//! - a seek away from a cache plays silence until data comes, with no fade;
//! - its decoder trait opens files by path and decodes with symphonia 0.5,
//!   where Beatlayer's decoder (symphonia 0.6, with RF64 and AAC trimming)
//!   comes from the media store, which checks the file's content first, and
//!   it has no sample-rate conversion;
//! - it depends on `rtrb` 0.3, a second copy beside the workspace's 0.4.

mod disk;
mod feed;
mod planar;
mod player;
mod reader;
mod shared;
mod source;
mod splice;

pub use disk::{StreamConfig, Streamer};
pub use planar::PlanarMut;
pub use reader::{StreamId, StreamSource};
pub use shared::{StreamStats, StreamerStats};
pub use source::{AudioSource, Filled, MemorySource, Source, Transition};
pub use splice::{DEFAULT_FADE, fade_frames};
