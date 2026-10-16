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
//!
//! An armed MIDI track records the same way, without a file: the audio
//! thread copies the note messages its input received into a ring of
//! [`RecordedMidi`] stamped with the song frame they landed on, and the
//! shell makes the clip from them when the take ends (WP-2.14).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use bl_core::midi::MidiMessage;
use bl_core::time::Frames;
use bl_model::{Channel, TrackId};
use rtrb::Producer;

use crate::midi::MidiEvent;
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

/// A note message an armed MIDI track received, and the song frame it
/// landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedMidi {
    /// The song frame, to the sample the input placed the message on.
    pub frame: Frames,
    /// A note-on or a note-off.
    pub message: MidiMessage,
}

/// One recorded MIDI track: the ring its note messages go to.
pub struct MidiRecordTrack {
    /// The track, for the report.
    pub track: TrackId,
    /// The messages, in order, for the shell to drain.
    pub producer: Producer<RecordedMidi>,
    /// Messages the ring was too full to take, for the shell to report.
    pub dropped: Arc<AtomicU64>,
}

/// What the audio thread records: every armed track's input and its ring.
pub struct RecordingPlan {
    /// The audio tracks, in the order the shell listed them.
    pub tracks: Vec<RecordTrack>,
    /// The MIDI tracks, in the order the shell listed them (WP-2.14).
    pub midi: Vec<MidiRecordTrack>,
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
    /// Copies one part of the block into every track's ring: for audio
    /// tracks, `offset..offset + frames` of the device's input (`planar`
    /// holds `device` channels `stride` frames apart; a track whose Audio
    /// From names a channel the device doesn't have records silence, as it
    /// hears), and for MIDI tracks, the note messages their input received
    /// in that part, stamped `position` for the part's first frame.
    ///
    /// Returns whether every track took the whole part. When a ring is too
    /// full for it, nothing of the part goes in, each short ring's frames
    /// (or messages) are counted as dropped, and the caller ends the take,
    /// so every file holds exactly the take's frames.
    pub(crate) fn capture(
        &mut self,
        program: &EngineProgram,
        position: Frames,
        offset: usize,
        frames: usize,
    ) -> bool {
        let (planar, device, stride) = (
            &program.device_input,
            program.device_input_channels,
            program.max_frames,
        );
        let in_part = |event: &&MidiEvent| {
            let at = event.offset as usize;
            at >= offset && at < offset + frames && is_note(&event.message)
        };
        let mut fits = true;
        for track in &mut self.tracks {
            if track.producer.slots() < frames * track.channels() {
                track.dropped.fetch_add(frames as u64, Relaxed);
                fits = false;
            }
        }
        for track in &mut self.midi {
            let wanted = program
                .midi_input(track.track)
                .map_or(0, |input| input.events.iter().filter(in_part).count());
            if track.producer.slots() < wanted {
                track.dropped.fetch_add(wanted as u64, Relaxed);
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
        for track in &mut self.midi {
            let Some(input) = program.midi_input(track.track) else {
                continue;
            };
            for event in input.events.iter().filter(in_part) {
                let _ = track.producer.push(RecordedMidi {
                    frame: position + Frames((event.offset as usize - offset) as i64),
                    message: event.message,
                });
            }
        }
        true
    }
}

/// Whether a message is one a recorded clip keeps: a note-on or a note-off.
fn is_note(message: &MidiMessage) -> bool {
    message.is_note_on() || message.is_note_off()
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
            .field("midi", &self.midi.len())
            .finish_non_exhaustive()
    }
}
