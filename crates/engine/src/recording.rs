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
//! thread copies the note and controller messages its input received into a
//! ring of [`RecordedMidi`] stamped with the song frame they landed on, and
//! the shell makes the clip from them when the take ends (WP-2.14, WP-2.29b).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use bl_core::midi::MidiMessage;
use bl_core::time::Frames;
use bl_model::{Channel, SceneId, TrackId};
use rtrb::Producer;

use crate::controller_take::is_controller;
use crate::midi::MidiEvent;
use crate::mpe::MpeRole;
use crate::node::TransportInfo;
use crate::program::EngineProgram;
use crate::session::SessionEvent;

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

/// A note or controller message an armed MIDI track received, and the song
/// frame it landed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedMidi {
    /// The song frame, to the sample the input placed the message on. In a
    /// take that went round the loop it runs on past the loop's end instead
    /// of wrapping, like the frames of the take's file, so the shell can tell
    /// the passes apart.
    pub frame: Frames,
    /// A note-on or a note-off, or a controller message
    /// ([`is_recorded`]).
    pub message: MidiMessage,
    /// What it is from an MPE port: a note of its own, or one note's
    /// expression, which carries its value (the MPE spec, R12). Otherwise
    /// [`MpeRole::Plain`].
    pub mpe: MpeRole,
}

/// One recorded MIDI track: the ring its messages go to.
pub struct MidiRecordTrack {
    /// The track, for the report.
    pub track: TrackId,
    /// The messages, in order, for the shell to drain.
    pub producer: Producer<RecordedMidi>,
    /// Messages the ring was too full to take, for the shell to report.
    pub dropped: Arc<AtomicU64>,
}

/// A Session slot a recording goes into (WP-3.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTarget {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
    /// Whether the slot's playing clip is overdubbed rather than a new clip
    /// recorded; a recorded clip is launched when the take ends, an
    /// overdubbed one is already playing.
    pub overdub: bool,
}

/// The ring an Arrangement take logs the Session clips that start and end
/// into, for the shell to copy into the Arrangement when it ends (WP-3.13).
pub struct LaunchLog {
    /// The events, in the order they happened.
    pub producer: Producer<SessionEvent>,
    /// Events the ring was too full to take, for the shell to report.
    pub dropped: Arc<AtomicU64>,
}

impl LaunchLog {
    /// Logs one event, or counts it dropped.
    pub(crate) fn push(&mut self, event: SessionEvent) {
        if self.producer.push(event).is_err() {
            self.dropped.fetch_add(1, Relaxed);
        }
    }
}

/// What the audio thread records: every armed track's input and its ring.
pub struct RecordingPlan {
    /// The audio tracks, in the order the shell listed them.
    pub tracks: Vec<RecordTrack>,
    /// The MIDI tracks, in the order the shell listed them (WP-2.14).
    pub midi: Vec<MidiRecordTrack>,
    /// Frames the audio is moved earlier: the device's input and output
    /// latency plus the Driver Error Compensation setting. Negative moves
    /// it later (WP-2.13).
    pub delay: i64,
    /// The slots a Session recording goes into (WP-3.11); empty for an
    /// Arrangement recording. A Session take starts and ends on the launch
    /// grid ([`SessionCommand::Record`](crate::SessionCommand::Record)) and
    /// ignores the Arrangement's loop and punch points.
    pub session: Vec<SessionTarget>,
    /// Where an Arrangement take logs the Session clips that start and end
    /// while it runs (WP-3.13); `None` to log nothing.
    pub launches: Option<LaunchLog>,
}

/// The song frames a finished take covers: `start` to `end`, playback that
/// ran on without a jump, but for the Arrangement loop's wraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakeBounds {
    /// The song frame the take's first sample has.
    pub start: Frames,
    /// The song frame past its last sample, or, for a take that wrapped, the
    /// frame in the loop the last pass reached.
    pub end: Frames,
    /// The loop's start and end frames, if the take went round it at least
    /// once: the file holds the audio from `start` to the loop's end, then
    /// the loop's passes one after another (the recording spec, R11).
    pub wrap: Option<(Frames, Frames)>,
    /// How many frames the take ran for, whatever the loop did: the audio's
    /// delay isn't taken off, since MIDI isn't moved by it.
    pub frames: u64,
}

impl RecordingPlan {
    /// Copies one part of the block into every track's ring: for audio
    /// tracks, `offset..offset + frames` of the device's input (`planar`
    /// holds `device` channels `stride` frames apart; a track whose Audio
    /// From names a channel the device doesn't have records silence, as it
    /// hears), and for MIDI tracks, the note and controller messages their input received
    /// in that part, stamped `position` for the part's first frame and, for
    /// the rest, the song frame `transport` plays them on. The
    /// audio tracks leave out the part's first `skip` frames, which the
    /// recording's delay moves it past (WP-2.13).
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
        skip: usize,
        transport: TransportInfo,
    ) -> bool {
        let (planar, device, stride) = (
            &program.device_input,
            program.device_input_channels,
            program.max_frames,
        );
        let in_part = |event: &&MidiEvent| {
            let at = event.offset as usize;
            at >= offset
                && at < offset + frames
                && (event.mpe != MpeRole::Plain || is_recorded(&event.message))
        };
        let mut fits = true;
        for track in &mut self.tracks {
            if track.producer.slots() < (frames - skip) * track.channels() {
                track.dropped.fetch_add((frames - skip) as u64, Relaxed);
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
            // Spec: link-audio R5. A track that takes a peer's channel
            // records what it hears, as Ext. In's is recorded: its slot's
            // pair stands for the channels its input names.
            let link = program
                .link_inputs
                .iter()
                .position(|input| input.track == track.track);
            let (planar, device, left, right) = match link {
                Some(slot) => (
                    &program.link_input,
                    program.link_inputs.len() * 2,
                    (slot * 2 + 1) as u16,
                    right.map(|_| (slot * 2 + 2) as u16),
                ),
                None => (planar, device, left, right),
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
                    for i in skip..frames {
                        let _ = track.producer.push(at(left, i));
                        let _ = track.producer.push(at(right, i));
                    }
                }
                None => {
                    for i in skip..frames {
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
                    // Spec: transport R24. A message lands on the song frame its
                    // frame of the part plays, whatever the part's speed.
                    frame: position
                        + Frames(
                            transport.song_frame(event.offset as usize - offset)
                                - transport.position.get(),
                        ),
                    message: event.message,
                    mpe: event.mpe,
                });
            }
        }
        true
    }
}

/// Whether a message is one a recorded clip keeps: a note-on or a note-off,
/// or a controller (the MIDI controller data spec, R10).
pub(crate) fn is_recorded(message: &MidiMessage) -> bool {
    message.is_note_on() || message.is_note_off() || is_controller(message)
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
            .field("delay", &self.delay)
            .field("session", &self.session.len())
            .finish_non_exhaustive()
    }
}
