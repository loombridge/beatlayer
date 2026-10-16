//! Recording armed tracks to disk (WP-2.13).
//!
//! While the transport records, the audio thread copies each recorded
//! track's input into a lock-free ring ([`RecordTrack::producer`]); a
//! writer thread on the other side drains it into a file, so nothing on the
//! audio thread allocates, locks, or waits. The shell makes the
//! [`RecordingPlan`] when recording starts and sends it to the engine with
//! [`Engine::record`](crate::Engine::record); when the take ends, the audio
//! thread drops it, which closes the rings and is how the writer learns the
//! take is over.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use bl_core::time::Frames;
use bl_model::{Channel, TrackId};
use rtrb::Producer;

use crate::program::EngineProgram;

/// One recorded track: the interface channels its input names and the ring
/// its samples go to, one frame's samples after another (one sample a frame
/// for a mono channel, two for a pair).
pub struct RecordTrack {
    /// The track, for the report.
    pub track: TrackId,
    /// The interface channels the track's Audio From names.
    pub channel: Channel,
    /// The samples, for the writer thread to drain.
    pub producer: Producer<f32>,
    /// Frames the ring was too full to take, for the shell to report: the
    /// engine never drops a frame silently (WP-2.13).
    pub dropped: Arc<AtomicU64>,
}

/// What the audio thread records: every armed track's input and its ring.
pub struct RecordingPlan {
    /// The tracks, in the order the shell listed them.
    pub tracks: Vec<RecordTrack>,
}

/// The song frames a finished take covers: `start` to `end`, playback that
/// ran on without a jump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakeBounds {
    /// The song frame the take's first sample has.
    pub start: Frames,
    /// The song frame past its last sample.
    pub end: Frames,
}

impl RecordingPlan {
    /// Copies one part of the device's input into every track's ring:
    /// `offset..offset + frames` of `planar`, which holds `device` channels
    /// `stride` frames apart. A track whose Audio From names a channel the
    /// device doesn't have records silence, as it hears.
    ///
    /// Returns whether every track took the whole part. When a ring is too
    /// full for it, nothing of the part goes in, each short ring's frames
    /// are counted as dropped, and the caller ends the take, so every file
    /// holds exactly the take's frames.
    pub(crate) fn capture(
        &mut self,
        program: &EngineProgram,
        offset: usize,
        frames: usize,
    ) -> bool {
        let (planar, device, stride) = (
            &program.device_input,
            program.device_input_channels,
            program.max_frames,
        );
        let mut fits = true;
        for track in &mut self.tracks {
            if track.producer.slots() < frames * track.channels() {
                track.dropped.fetch_add(frames as u64, Relaxed);
                fits = false;
            }
        }
        if !fits {
            return false;
        }
        for track in &mut self.tracks {
            let (left, right) = match track.channel {
                Channel::Mono(channel) => (channel, None),
                Channel::Pair(channel) => (channel, Some(channel.saturating_add(1))),
            };
            let at = |channel: u16, i: usize| {
                let index = usize::from(channel.saturating_sub(1));
                if index < device {
                    planar[index * stride + offset + i]
                } else {
                    0.0
                }
            };
            match right {
                Some(right) => {
                    for i in 0..frames {
                        let _ = track.producer.push(at(left, i));
                        let _ = track.producer.push(at(right, i));
                    }
                }
                None => {
                    for i in 0..frames {
                        let _ = track.producer.push(at(left, i));
                    }
                }
            }
        }
        true
    }
}

impl RecordTrack {
    /// How many samples one frame has: one for a mono channel, two for a
    /// pair.
    #[must_use]
    pub fn channels(&self) -> usize {
        match self.channel {
            Channel::Mono(_) => 1,
            Channel::Pair(_) => 2,
        }
    }
}

impl std::fmt::Debug for RecordingPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingPlan")
            .field("tracks", &self.tracks.len())
            .finish_non_exhaustive()
    }
}
