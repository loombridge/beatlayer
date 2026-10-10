//! The source trait a clip player reads through, and the in-memory source.

use std::sync::Arc;

use super::planar::PlanarMut;
use super::player::{Got, Player, Supply};
use super::reader::StreamSource;
use super::shared::StreamStats;
use super::splice::fade_frames;
use crate::sample_data::SampleData;

/// What a read does when it doesn't continue from where the last one ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transition {
    /// Plays the new position at once, at full level: a launch, or a clip's
    /// loop wrapping round, which must be sample-accurate. Only if the new
    /// position isn't ready does its audio fade in when it arrives.
    #[default]
    Cut,
    /// Fades from the audio that was playing to the new position, over the
    /// source's fade length (4 ms by default): a locate during playback,
    /// which would otherwise click. If the new position isn't ready, the old
    /// audio still fades out, and the new fades in when it arrives.
    Crossfade,
}

/// What a read wrote. The three counts add up to the frames asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filled {
    /// Frames of the source's audio.
    pub frames: usize,
    /// Frames of silence because the disk hadn't read them yet: an underrun,
    /// or the wait after a jump nothing was cued for.
    pub missing: usize,
    /// Frames of silence past the end of the source.
    pub past_end: usize,
}

/// Audio a clip plays, at the engine's sample rate, read a block at a time
/// on the audio thread: a file decoded into memory ([`MemorySource`]), or
/// one streamed from disk ([`StreamSource`]).
///
/// This is the interface between disk streaming and the clip player
/// (WP-1.4). The player works out, each block, which source frames the
/// block plays (from the song position, the clip's region and loop, and
/// the tempo map), and reads them with [`read`](Self::read). The source
/// notices when a read doesn't continue the last one, and handles the jump
/// as the read's [`Transition`] says. Positions are frames at the engine's
/// rate from the start of the file, whatever the file's own rate.
///
/// Every method is real-time safe: no allocation, lock, or blocking call.
/// Making a source is not, and neither is dropping one, which frees its
/// buffers: make sources off the audio thread, and let the engine drop them
/// on its drop thread, as it does every node.
pub trait AudioSource: Send {
    /// The number of channels.
    fn channels(&self) -> usize;

    /// Frames per second: the engine's rate.
    fn sample_rate(&self) -> u32;

    /// The length in frames at the engine's rate, if known. A streamed file
    /// whose header doesn't give its length learns it when the disk thread
    /// reaches the end.
    fn frames(&self) -> Option<u64>;

    /// The frame the next read continues from.
    fn position(&self) -> u64;

    /// Writes `out.frames()` frames from source frame `position` into `out`:
    /// source channel `c` into channel `c` of `out`, for as many channels as
    /// both have, and silence in any more channels `out` has. Past the end
    /// of the source, and where the disk hasn't read the audio yet, it
    /// writes silence (see [`Filled`]).
    ///
    /// If `position` isn't where the last read ended, the read is a jump,
    /// handled as `transition` says. A jump to a position that's buffered,
    /// preloaded, or cued plays at once; anywhere else, the source asks the
    /// disk thread for it and plays silence until it comes.
    fn read(&mut self, position: u64, out: &mut PlanarMut<'_>, transition: Transition) -> Filled;

    /// Moves to `position` without playing, so a read from there later finds
    /// its audio ready: for a stopped transport whose position changed, or a
    /// clip about to start. Audio stops sounding, as with
    /// [`stop`](Self::stop).
    fn seek(&mut self, position: u64);

    /// Audio stops sounding (the transport stopped, or the clip ended), so
    /// the next read starts afresh rather than fading from the last one.
    fn stop(&mut self);

    /// How many frames from [`position`](Self::position) are ready to read
    /// without waiting on the disk; past the end, the frames up to it.
    fn buffered(&mut self) -> u64;

    /// What this source has counted: underruns, jumps, and so on.
    fn stats(&self) -> StreamStats;
}

/// A source decoded into memory: short files, which cost less to hold than
/// to stream, and clips whose RAM switch is on.
#[derive(Debug)]
pub struct MemorySource {
    player: Player<InMemory>,
}

impl MemorySource {
    /// A source over decoded audio, at the audio's own rate, with the
    /// default fade (4 ms).
    #[must_use]
    pub fn new(data: Arc<SampleData>) -> Self {
        let fade = fade_frames(data.sample_rate());
        Self::with_fade(data, fade)
    }

    /// A source over decoded audio with a fade of `fade` frames.
    #[must_use]
    pub fn with_fade(data: Arc<SampleData>, fade: usize) -> Self {
        let channels = data.channel_count();
        Self {
            player: Player::new(
                InMemory {
                    data,
                    stats: StreamStats::default(),
                },
                channels,
                0,
                fade,
            ),
        }
    }

    /// The audio.
    #[must_use]
    pub fn data(&self) -> &Arc<SampleData> {
        &self.player.supply.data
    }

    /// The length of a jump's fades, in frames.
    #[must_use]
    pub fn fade_frames(&self) -> usize {
        self.player.fade_frames()
    }
}

impl AudioSource for MemorySource {
    fn channels(&self) -> usize {
        self.player.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.player.supply.data.sample_rate()
    }

    fn frames(&self) -> Option<u64> {
        Some(self.player.supply.data.frames() as u64)
    }

    fn position(&self) -> u64 {
        self.player.position()
    }

    fn read(&mut self, position: u64, out: &mut PlanarMut<'_>, transition: Transition) -> Filled {
        self.player.read(position, out, transition)
    }

    fn seek(&mut self, position: u64) {
        self.player.seek(position);
    }

    fn stop(&mut self) {
        self.player.stop();
    }

    fn buffered(&mut self) -> u64 {
        (self.player.supply.data.frames() as u64).saturating_sub(self.player.position())
    }

    fn stats(&self) -> StreamStats {
        self.player.supply.stats
    }
}

#[derive(Debug)]
struct InMemory {
    data: Arc<SampleData>,
    stats: StreamStats,
}

impl Supply for InMemory {
    fn fetch(
        &mut self,
        at: u64,
        out: &mut PlanarMut<'_>,
        offset: usize,
        frames: usize,
        channels: usize,
        _consume: bool,
    ) -> Got {
        let length = self.data.frames() as u64;
        if at >= length {
            return Got::End(frames);
        }
        let from = at as usize;
        let n = frames.min(self.data.frames() - from);
        for (c, samples) in self.data.channels().take(channels).enumerate() {
            out.span_mut(c, offset..offset + n)
                .copy_from_slice(&samples[from..from + n]);
        }
        Got::Data(n)
    }

    fn relocate(&mut self, _to: u64) -> bool {
        true
    }

    fn count(&mut self, delta: &StreamStats) {
        let s = &mut self.stats;
        s.seeks += delta.seeks;
        s.seek_hits += delta.seek_hits;
        s.seek_misses += delta.seek_misses;
    }
}

/// A clip's audio, in memory or streamed: what [`Streamer::source`] gives,
/// by the file's length.
///
/// [`Streamer::source`]: super::Streamer::source
#[derive(Debug)]
pub enum Source {
    /// Decoded into memory.
    Memory(MemorySource),
    /// Streamed from disk.
    Stream(StreamSource),
}

impl Source {
    fn inner(&self) -> &dyn AudioSource {
        match self {
            Self::Memory(s) => s,
            Self::Stream(s) => s,
        }
    }

    fn inner_mut(&mut self) -> &mut dyn AudioSource {
        match self {
            Self::Memory(s) => s,
            Self::Stream(s) => s,
        }
    }
}

impl AudioSource for Source {
    fn channels(&self) -> usize {
        self.inner().channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner().sample_rate()
    }

    fn frames(&self) -> Option<u64> {
        self.inner().frames()
    }

    fn position(&self) -> u64 {
        self.inner().position()
    }

    fn read(&mut self, position: u64, out: &mut PlanarMut<'_>, transition: Transition) -> Filled {
        self.inner_mut().read(position, out, transition)
    }

    fn seek(&mut self, position: u64) {
        self.inner_mut().seek(position);
    }

    fn stop(&mut self) {
        self.inner_mut().stop();
    }

    fn buffered(&mut self) -> u64 {
        self.inner_mut().buffered()
    }

    fn stats(&self) -> StreamStats {
        self.inner().stats()
    }
}
