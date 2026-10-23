//! MIDI clip playback (WP-2.1): the MIDI clip player at the head of each
//! MIDI track's device chain, and the clips and notes it plays.
//!
//! A MIDI track has a [`MidiClipPlayer`] at the head of its device chain
//! ([`NodeKey::Player`](crate::NodeKey::Player)). The compiler works out the
//! track's clips off the audio thread. That list reaches the player as node
//! data ([`Sender::send_data`](crate::Sender::send_data)), so making, moving,
//! or editing a clip never builds a new program.
//!
//! During playback, the player emits [`NoteEvent`]s with sample accuracy into
//! the block's note buffer, rolls probability and velocity deviation per play
//! from an allocation-free PRNG, and sends note-offs for all sounding notes
//! when stopping, looping, or deleting a clip.

use std::sync::Arc;

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_model::{
    Clip, ClipData, ClipId, GroovePlay, GroovePool, Note, SceneId, Track, TrackId, clip_seed, noise,
};

use crate::midi_controllers::{PlacedController, Sounding, Stretch, play_pressure};
use crate::node::{Block, Node, NodeConfig, NodeData, NodeEvent, NoteEvent, NoteKind, ParamValue};

/// Simple deterministic xorshift64* pseudo-random number generator.
/// Zero allocations, real-time safe.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A new generator with `seed`. A zero seed is replaced by a default non-zero constant.
    pub const fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x853c_49e6_748f_ea9b
        } else {
            seed
        })
    }

    /// Advances state and returns pseudo-random u64.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0 = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        self.0
    }

    /// Returns a pseudo-random float in `[0.0, 1.0)`.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform pseudo-random integer in `[min, max]` inclusive.
    #[inline]
    pub fn next_in_range(&mut self, min: i32, max: i32) -> i32 {
        if min >= max {
            return min;
        }
        let range = (max - min + 1) as u64;
        min + (self.next_u64() % range) as i32
    }
}

/// A sounding note actively playing on the audio thread.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ActiveNote {
    clip_id: ClipId,
    key: u8,
    channel: u8,
    note_id: Option<u32>,
    release_velocity: u8,
    off_frame: i64,
    /// Where the note sits in its clip's note list, and where it started in
    /// the song: what later blocks need to play its pressure (WP-2.29).
    index: usize,
    song_start: Ticks,
}

/// A clip's notes and markers placed on the track.
#[derive(Clone, Debug)]
pub struct PlacedMidiClip {
    pub id: ClipId,
    /// The Session slot the clip sits in, or `None` for an Arrangement clip
    /// (WP-3.2): a slot clip plays only when it is launched, from its launch
    /// frame.
    pub scene: Option<SceneId>,
    pub start: Ticks,
    pub length: Ticks,
    pub mute: bool,
    pub start_marker: Ticks,
    pub end_marker: Option<Ticks>,
    pub looping: bool,
    pub loop_start: Ticks,
    pub loop_end: Option<Ticks>,
    pub notes: Vec<Note>,
    /// The clip's MIDI controller envelopes (WP-2.29).
    pub controllers: Vec<PlacedController>,
    /// The clip's groove at the set's Global Amount, if it has one that
    /// changes anything (the grooves spec, R6 to R11).
    pub groove: Option<Arc<GroovePlay>>,
}

/// The list of MIDI clips for a track, sent to [`MidiClipPlayer`] as [`NodeData`].
#[derive(Clone, Debug)]
pub struct MidiClipList {
    /// The track the list is for.
    pub track: TrackId,
    /// The track's Arrangement clips, sorted by start, and its Session slot
    /// clips, which have no position in the Arrangement.
    pub clips: Vec<PlacedMidiClip>,
}

impl MidiClipList {
    /// Builds the placed MIDI clips for `track`, with the grooves of `grooves`
    /// they use.
    #[must_use]
    pub fn build(
        track: &Track,
        _tempo: &TempoMap,
        _rate: SampleRate,
        grooves: &GroovePool,
    ) -> Self {
        let groove = |clip: &Clip| {
            clip.groove
                .and_then(|id| grooves.play(id))
                .filter(|play| !play.is_inert())
                .map(Arc::new)
        };
        let mut clips = Vec::new();
        for clip in track.played_clips() {
            if clip.mute {
                continue;
            }
            let ClipData::Midi(midi) = &clip.data else {
                continue;
            };
            if clip.length <= Ticks::ZERO {
                continue;
            }
            let mut notes: Vec<Note> = midi.notes.iter().filter(|n| !n.mute).cloned().collect();
            notes.sort_by_key(|n| n.start);
            clips.push(PlacedMidiClip {
                id: clip.id,
                scene: None,
                start: clip.start,
                length: clip.length,
                mute: clip.mute,
                start_marker: midi.start,
                end_marker: midi.end,
                looping: midi.looping,
                loop_start: midi.loop_start,
                loop_end: midi.loop_end,
                notes,
                controllers: PlacedController::of(clip),
                groove: groove(clip),
            });
        }
        // The track's Session slot clips (WP-3.2): they play only when the
        // engine launches them, so they carry no Arrangement position.
        for (scene, clip) in track.slots() {
            let ClipData::Midi(midi) = &clip.data else {
                continue;
            };
            if clip.mute || clip.length <= Ticks::ZERO {
                continue;
            }
            let mut notes: Vec<Note> = midi.notes.iter().filter(|n| !n.mute).cloned().collect();
            notes.sort_by_key(|n| n.start);
            clips.push(PlacedMidiClip {
                id: clip.id,
                scene: Some(scene),
                start: clip.start,
                length: clip.length,
                mute: clip.mute,
                start_marker: midi.start,
                end_marker: midi.end,
                looping: midi.looping,
                loop_start: midi.loop_start,
                loop_end: midi.loop_end,
                notes,
                controllers: PlacedController::of(clip),
                groove: groove(clip),
            });
        }
        clips.sort_by_key(|c| c.start);
        Self {
            track: track.id(),
            clips,
        }
    }
}

/// The most times round a launched clip's loop one block plays. A block is
/// a few thousand frames at most, so this only bounds a loop shorter than a
/// frame, which no clip holds for long.
///
/// `shortcut:` a loop a 128th of a frame long would drop the rest of the
/// block's notes; raise it if a real clip ever needs more.
const MAX_SESSION_REPEATS: usize = 128;

/// Plays a launched Session clip for `from_frame..to_frame`: as if the clip
/// sat at its launch frame, so it plays from its Start marker, repeats its
/// loop brace while its Loop switch is on, and plays once while it's off
/// (WP-3.2). What a non-looping clip does when it reaches its end is to
/// observe; Beatlayer leaves the slot playing.
#[allow(clippy::too_many_arguments)]
fn play_session_clip(
    clip: &PlacedMidiClip,
    launch_tick: Ticks,
    from_frame: i64,
    to_frame: i64,
    tempo: &TempoMap,
    sample_rate: SampleRate,
    rng: &mut Rng,
    next_note_id: &mut u32,
    scratch_notes: &mut Vec<NoteEvent>,
    messages: &mut Vec<NoteEvent>,
    sounding: &mut Vec<ActiveNote>,
) {
    play_unlinked(
        clip,
        launch_tick,
        from_frame,
        to_frame,
        tempo,
        sample_rate,
        messages,
    );
    let start_marker = clip.start_marker;
    if !clip.looping {
        let end_marker = clip.end_marker.unwrap_or(start_marker + clip.length);
        if end_marker <= start_marker {
            return;
        }
        let active_ticks = clip.length.min(end_marker - start_marker);
        let end = launch_tick + active_ticks;
        let end_frame = tempo.ticks_to_frames(end, sample_rate).get();
        if end_frame <= from_frame {
            return;
        }
        process_clip_segment(
            clip,
            start_marker,
            launch_tick,
            end,
            0,
            from_frame,
            to_frame,
            tempo,
            sample_rate,
            rng,
            next_note_id,
            scratch_notes,
            messages,
            sounding,
        );
        if from_frame <= end_frame && end_frame < to_frame {
            stop_clip_sounding(
                clip.id,
                (end_frame - from_frame) as u32,
                sounding,
                scratch_notes,
            );
        }
        return;
    }
    let loop_start = clip.loop_start;
    let loop_end = clip
        .loop_end
        .unwrap_or_else(|| (loop_start + clip.length).max(loop_start + Ticks::QUARTER));
    if loop_end <= loop_start || start_marker >= loop_end {
        return;
    }
    let loop_len = loop_end - loop_start;
    let first_len = loop_end - start_marker;
    let mut start = launch_tick;
    let mut first = true;
    for pass in 0..MAX_SESSION_REPEATS as u64 {
        let (clip_start, repeat_len) = if first {
            (start_marker, first_len)
        } else {
            (loop_start, loop_len)
        };
        let end = start + repeat_len;
        let start_frame = tempo.ticks_to_frames(start, sample_rate).get();
        let end_frame = tempo.ticks_to_frames(end, sample_rate).get();
        if start_frame >= to_frame {
            break;
        }
        if end_frame > from_frame {
            process_clip_segment(
                clip,
                clip_start,
                start,
                end,
                pass,
                from_frame,
                to_frame,
                tempo,
                sample_rate,
                rng,
                next_note_id,
                scratch_notes,
                messages,
                sounding,
            );
            if from_frame <= end_frame && end_frame < to_frame {
                stop_clip_sounding(
                    clip.id,
                    (end_frame - from_frame) as u32,
                    sounding,
                    scratch_notes,
                );
            }
        }
        start = end;
        first = false;
    }
}

/// Plays the clip's unlinked controller envelopes (WP-2.29): each runs once
/// from `origin`, where the clip starts in the song, one tick per tick for
/// as long as the clip lasts, like an unlinked clip envelope
/// (clip envelopes R2).
fn play_unlinked(
    clip: &PlacedMidiClip,
    origin: Ticks,
    from_frame: i64,
    to_frame: i64,
    tempo: &TempoMap,
    sample_rate: SampleRate,
    messages: &mut Vec<NoteEvent>,
) {
    let stretch = Stretch {
        content_start: Ticks::ZERO,
        content_end: clip.length,
        song_start: origin,
    };
    for controller in clip.controllers.iter().filter(|c| !c.linked) {
        controller.play(stretch, from_frame, to_frame, tempo, sample_rate, messages);
    }
}

/// The node that plays a MIDI track's clips.
pub struct MidiClipPlayer {
    clips: Option<NodeData>,
    sounding: Vec<ActiveNote>,
    scratch_notes: Vec<NoteEvent>,
    /// The controller messages of the block, which go in ahead of its notes
    /// so a step lands before a note-on on its frame (WP-2.29, R3).
    messages: Vec<NoteEvent>,
    audible: bool,
    last_position: Option<i64>,
    /// The Session slot clip sounding in the last block and the frame it
    /// launched on: when the engine stops it, restarts it or another slot
    /// takes over, its notes are let go.
    session: Option<(ClipId, i64)>,
    /// Whether a Session clip has taken the track over from the Arrangement
    /// (WP-3.6).
    taken_over: bool,
    rng: Rng,
    next_note_id: u32,
}

impl MidiClipPlayer {
    /// A new player with preallocated sounding and scratch note buffers.
    #[must_use]
    pub fn new(_config: &NodeConfig) -> Self {
        Self {
            clips: None,
            sounding: Vec::with_capacity(256),
            scratch_notes: Vec::with_capacity(512),
            messages: Vec::with_capacity(512),
            audible: true,
            last_position: None,
            session: None,
            taken_over: false,
            rng: Rng::new(0x853c_49e6_748f_ea9b),
            next_note_id: 1,
        }
    }

    /// Sets whether the clips are audible (Monitor In silences them).
    #[must_use]
    pub fn audible(mut self, audible: bool) -> Self {
        self.audible = audible;
        self
    }

    /// Configures the PRNG seed.
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = Rng::new(seed);
        self
    }

    /// Sets the PRNG seed in place.
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
    }

    fn send_scratch_notes(&mut self, block: &mut Block<'_>) {
        for &message in &self.messages {
            block.insert_note(message);
        }
        self.messages.clear();
        if self.scratch_notes.is_empty() {
            return;
        }
        let frames = block.frames() as u32;
        for note in &mut self.scratch_notes {
            if note.offset >= frames {
                note.offset = frames.saturating_sub(1);
            }
        }
        // In order of offset, note-offs before note-ons on one frame. The
        // buffer puts each note in its place without allocating, where a
        // stable sort would take heap scratch for a long run of notes.
        for &note in &self.scratch_notes {
            block.insert_note(note);
        }
        self.scratch_notes.clear();
    }
}

fn stop_clip_sounding(
    clip_id: ClipId,
    off_offset: u32,
    sounding: &mut Vec<ActiveNote>,
    scratch_notes: &mut Vec<NoteEvent>,
) {
    for i in (0..sounding.len()).rev() {
        if sounding[i].clip_id == clip_id {
            let active = sounding.swap_remove(i);
            if scratch_notes.len() < scratch_notes.capacity() {
                scratch_notes.push(NoteEvent {
                    offset: off_offset,
                    kind: NoteKind::Off,
                    channel: active.channel,
                    key: active.key,
                    velocity: active.release_velocity as f32 / 127.0,
                    note_id: active.note_id,
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_clip_segment(
    clip: &PlacedMidiClip,
    segment_clip_start: Ticks,
    segment_song_start: Ticks,
    segment_song_end: Ticks,
    pass: u64,
    from_frame: i64,
    to_frame: i64,
    tempo: &TempoMap,
    sample_rate: SampleRate,
    rng: &mut Rng,
    next_note_id: &mut u32,
    scratch_notes: &mut Vec<NoteEvent>,
    messages: &mut Vec<NoteEvent>,
    sounding: &mut Vec<ActiveNote>,
) {
    let segment_len = segment_song_end - segment_song_start;
    let segment_clip_end = segment_clip_start + segment_len;
    // Spec: grooves R7. A groove moves a note by less than a Base step, so
    // only the notes that start within one of the block need working out.
    let reach = clip.groove.as_ref().map(|play| {
        let slack = Ticks(play.base() as i64 + 1);
        let from_tick = tempo.frames_to_ticks(Frames(from_frame), sample_rate) - slack;
        let to_tick = tempo.frames_to_ticks(Frames(to_frame), sample_rate) + slack;
        (from_tick, to_tick)
    });
    // A note held to the tick before its pass ends can round onto the pass's
    // last frame's end, which isn't in it.
    let end_frame = tempo.ticks_to_frames(segment_song_end, sample_rate).get();

    // Spec: midi-controller-data R3
    let stretch = Stretch {
        content_start: segment_clip_start,
        content_end: segment_clip_end,
        song_start: segment_song_start,
    };
    for controller in clip.controllers.iter().filter(|c| c.linked) {
        controller.play(stretch, from_frame, to_frame, tempo, sample_rate, messages);
    }

    for (index, note) in clip.notes.iter().enumerate() {
        if note.start < segment_clip_start || note.start >= segment_clip_end {
            continue;
        }
        let mut note_song_start = segment_song_start + (note.start - segment_clip_start);
        let mut grooved_velocity = note.velocity;
        if let (Some(play), Some((from_tick, to_tick))) = (&clip.groove, reach) {
            if note_song_start < from_tick || note_song_start >= to_tick {
                continue;
            }
            // Spec: grooves R7 to R11. The note plays where the groove puts
            // it, held inside its pass, and at the velocity it gives.
            let at = play.note_start(note.start, note.key, clip_seed(clip.id), pass);
            let held = (at.round() as i64)
                .max(segment_clip_start.get())
                .min(segment_clip_end.get() - 1);
            note_song_start = segment_song_start + (Ticks(held) - segment_clip_start);
            grooved_velocity = play.note_velocity(note.start, note.velocity);
        }
        let mut note_on_frame = tempo.ticks_to_frames(note_song_start, sample_rate).get();
        if reach.is_some() {
            note_on_frame = note_on_frame.min(end_frame - 1);
        }
        if note_on_frame < from_frame || note_on_frame >= to_frame {
            continue;
        }

        // Spec: grooves R15. A groove can reorder notes, so a grooved clip's
        // chance and deviation draw from the note itself, not from the order
        // the block meets them in, and a render is the same at any block size.
        let mut own;
        let rng: &mut Rng = if clip.groove.is_some() {
            let key = ((note.start.get() as u64) << 8) | u64::from(note.key);
            own = Rng::new(noise(clip_seed(clip.id), pass, !key).to_bits());
            &mut own
        } else {
            &mut *rng
        };

        // Probability check
        if note.probability < 1.0 && (note.probability <= 0.0 || rng.next_f64() > note.probability)
        {
            continue;
        }

        // Velocity deviation
        let dev = note.velocity_deviation as i32;
        let base = i32::from(grooved_velocity);
        let vel = if dev == 0 {
            grooved_velocity
        } else if dev > 0 {
            rng.next_in_range(base, (base + dev).min(127)) as u8
        } else {
            rng.next_in_range((base + dev).max(1), base) as u8
        };

        let note_id = Some(*next_note_id);
        *next_note_id = next_note_id.wrapping_add(1);

        let note_song_end = (note_song_start + note.length).min(segment_song_end);
        let mut note_off_frame = tempo.ticks_to_frames(note_song_end, sample_rate).get();
        if note_off_frame <= note_on_frame {
            note_off_frame = note_on_frame + 1;
        }

        if scratch_notes.len() < scratch_notes.capacity() {
            scratch_notes.push(NoteEvent {
                offset: (note_on_frame - from_frame) as u32,
                kind: NoteKind::On,
                channel: 0,
                key: note.key,
                velocity: vel as f32 / 127.0,
                note_id,
            });
        }

        // Spec: midi-controller-data R3. The pressure of the blocks after
        // this one is played from `sounding`.
        play_pressure(
            note,
            Sounding {
                note_id,
                song_start: note_song_start,
                on: note_on_frame,
                off: note_off_frame,
            },
            (from_frame, to_frame),
            tempo,
            sample_rate,
            scratch_notes,
        );

        if note_off_frame < to_frame {
            if scratch_notes.len() < scratch_notes.capacity() {
                scratch_notes.push(NoteEvent {
                    offset: (note_off_frame - from_frame) as u32,
                    kind: NoteKind::Off,
                    channel: 0,
                    key: note.key,
                    velocity: note.release_velocity as f32 / 127.0,
                    note_id,
                });
            }
        } else if sounding.len() < sounding.capacity() {
            sounding.push(ActiveNote {
                clip_id: clip.id,
                key: note.key,
                channel: 0,
                note_id,
                release_velocity: note.release_velocity,
                off_frame: note_off_frame,
                index,
                song_start: note_song_start,
            });
        }
    }
}

impl Node for MidiClipPlayer {
    fn process(&mut self, block: &mut Block<'_>) {
        for event in block.events() {
            if event.param == crate::clips::player::AUDIBLE {
                self.audible = match event.value {
                    ParamValue::Set(v) => v != 0.0,
                    ParamValue::Default => true,
                };
                if !self.audible {
                    for active in self.sounding.drain(..) {
                        if self.scratch_notes.len() < self.scratch_notes.capacity() {
                            self.scratch_notes.push(NoteEvent {
                                offset: event.offset,
                                kind: NoteKind::Off,
                                channel: active.channel,
                                key: active.key,
                                velocity: active.release_velocity as f32 / 127.0,
                                note_id: active.note_id,
                            });
                        }
                    }
                }
            }
        }

        let transport = block.transport();
        if !transport.playing {
            self.last_position = None;
            for active in self.sounding.drain(..) {
                if self.scratch_notes.len() < self.scratch_notes.capacity() {
                    self.scratch_notes.push(NoteEvent {
                        offset: 0,
                        kind: NoteKind::Off,
                        channel: active.channel,
                        key: active.key,
                        velocity: active.release_velocity as f32 / 127.0,
                        note_id: active.note_id,
                    });
                }
            }
            self.send_scratch_notes(block);
            return;
        }

        if !self.audible {
            self.send_scratch_notes(block);
            return;
        }

        let from_frame = transport.position.get();
        let frames = block.frames() as i64;
        let to_frame = from_frame + frames;

        // Check if playback located or jumped.
        if let Some(last) = self.last_position
            && from_frame != last
        {
            for active in self.sounding.drain(..) {
                if self.scratch_notes.len() < self.scratch_notes.capacity() {
                    self.scratch_notes.push(NoteEvent {
                        offset: 0,
                        kind: NoteKind::Off,
                        channel: active.channel,
                        key: active.key,
                        velocity: active.release_velocity as f32 / 127.0,
                        note_id: active.note_id,
                    });
                }
            }
        }
        self.last_position = Some(to_frame);

        let Some(list) = self
            .clips
            .as_ref()
            .and_then(|d| d.downcast_ref::<MidiClipList>())
        else {
            for active in self.sounding.drain(..) {
                if self.scratch_notes.len() < self.scratch_notes.capacity() {
                    self.scratch_notes.push(NoteEvent {
                        offset: 0,
                        kind: NoteKind::Off,
                        channel: active.channel,
                        key: active.key,
                        velocity: active.release_velocity as f32 / 127.0,
                        note_id: active.note_id,
                    });
                }
            }
            self.send_scratch_notes(block);
            return;
        };

        // Spec: midi-controller-data R3. The pressure of notes that began in
        // an earlier block.
        let (rate, map) = (block.sample_rate(), block.tempo());
        for active in &self.sounding {
            let note = list
                .clips
                .iter()
                .find(|c| c.id == active.clip_id)
                .and_then(|c| c.notes.get(active.index))
                .filter(|n| n.key == active.key);
            if let Some(note) = note {
                play_pressure(
                    note,
                    Sounding {
                        note_id: active.note_id,
                        song_start: active.song_start,
                        on: i64::MIN,
                        off: active.off_frame,
                    },
                    (from_frame, to_frame),
                    map,
                    rate,
                    &mut self.scratch_notes,
                );
            }
        }

        // Note-offs for clips or notes that were deleted while sounding.
        for i in (0..self.sounding.len()).rev() {
            let active = self.sounding[i];
            let still_valid = list.clips.iter().any(|c| {
                c.id == active.clip_id
                    && !c.mute
                    && c.notes.iter().any(|n| n.key == active.key && !n.mute)
            });
            if !still_valid {
                self.sounding.swap_remove(i);
                if self.scratch_notes.len() < self.scratch_notes.capacity() {
                    self.scratch_notes.push(NoteEvent {
                        offset: 0,
                        kind: NoteKind::Off,
                        channel: active.channel,
                        key: active.key,
                        velocity: active.release_velocity as f32 / 127.0,
                        note_id: active.note_id,
                    });
                }
            }
        }

        // Note-offs for notes reaching their off_frame in this block.
        for i in (0..self.sounding.len()).rev() {
            let active = self.sounding[i];
            if active.off_frame <= from_frame {
                self.sounding.swap_remove(i);
                if self.scratch_notes.len() < self.scratch_notes.capacity() {
                    self.scratch_notes.push(NoteEvent {
                        offset: 0,
                        kind: NoteKind::Off,
                        channel: active.channel,
                        key: active.key,
                        velocity: active.release_velocity as f32 / 127.0,
                        note_id: active.note_id,
                    });
                }
            } else if active.off_frame < to_frame {
                self.sounding.swap_remove(i);
                if self.scratch_notes.len() < self.scratch_notes.capacity() {
                    self.scratch_notes.push(NoteEvent {
                        offset: (active.off_frame - from_frame) as u32,
                        kind: NoteKind::Off,
                        channel: active.channel,
                        key: active.key,
                        velocity: active.release_velocity as f32 / 127.0,
                        note_id: active.note_id,
                    });
                }
            }
        }

        let sample_rate = block.sample_rate();
        let tempo = block.tempo();

        // Spec: launch-quantisation R9. A launched slot clip plays from its
        // launch frame. When the engine stops it or another slot takes over,
        // its notes are let go at once.
        let taken_over = block
            .session()
            .iter()
            .find(|entry| entry.track == list.track);
        let launched = taken_over.and_then(|entry| {
            let scene = entry.scene?;
            list.clips
                .iter()
                .find(|clip| clip.scene == Some(scene))
                .map(|clip| (entry, clip))
        });
        // Spec: session-view R20. The Arrangement's notes
        // are let go where the Session clip takes the track over.
        if taken_over.is_some() && !self.taken_over {
            for active in self.sounding.drain(..) {
                if self.scratch_notes.len() < self.scratch_notes.capacity() {
                    self.scratch_notes.push(NoteEvent {
                        offset: 0,
                        kind: NoteKind::Off,
                        channel: active.channel,
                        key: active.key,
                        velocity: active.release_velocity as f32 / 127.0,
                        note_id: active.note_id,
                    });
                }
            }
        }
        self.taken_over = taken_over.is_some();
        let launched_id = launched.map(|(entry, clip)| (clip.id, entry.frame.get()));
        if self.session != launched_id {
            if let Some((old, _)) = self.session {
                stop_clip_sounding(old, 0, &mut self.sounding, &mut self.scratch_notes);
            }
            self.session = launched_id;
        }
        if let Some((entry, clip)) = launched {
            let launch_tick = tempo.frames_to_ticks(Frames(entry.frame.get()), sample_rate);
            play_session_clip(
                clip,
                launch_tick,
                from_frame,
                to_frame,
                tempo,
                sample_rate,
                &mut self.rng,
                &mut self.next_note_id,
                &mut self.scratch_notes,
                &mut self.messages,
                &mut self.sounding,
            );
        }

        if self.taken_over {
            self.send_scratch_notes(block);
            return;
        }

        for clip in &list.clips {
            if clip.scene.is_some() {
                continue;
            }
            let clip_start_frame = tempo.ticks_to_frames(clip.start, sample_rate).get();
            let clip_end_frame = tempo
                .ticks_to_frames(clip.start + clip.length, sample_rate)
                .get();

            if clip_start_frame >= to_frame || clip_end_frame <= from_frame {
                continue;
            }
            play_unlinked(
                clip,
                clip.start,
                from_frame,
                to_frame,
                tempo,
                sample_rate,
                &mut self.messages,
            );

            if !clip.looping {
                let start_marker = clip.start_marker;
                let end_marker = clip.end_marker.unwrap_or(start_marker + clip.length);
                if end_marker > start_marker {
                    let active_ticks = clip.length.min(end_marker - start_marker);
                    let rep_end_tick = clip.start + active_ticks;
                    let rep_end_frame = tempo.ticks_to_frames(rep_end_tick, sample_rate).get();

                    process_clip_segment(
                        clip,
                        start_marker,
                        clip.start,
                        rep_end_tick,
                        0,
                        from_frame,
                        to_frame,
                        tempo,
                        sample_rate,
                        &mut self.rng,
                        &mut self.next_note_id,
                        &mut self.scratch_notes,
                        &mut self.messages,
                        &mut self.sounding,
                    );
                    if from_frame <= rep_end_frame && rep_end_frame < to_frame {
                        let off_offset = (rep_end_frame - from_frame) as u32;
                        stop_clip_sounding(
                            clip.id,
                            off_offset,
                            &mut self.sounding,
                            &mut self.scratch_notes,
                        );
                    }
                }
            } else {
                let start_marker = clip.start_marker;
                let loop_start = clip.loop_start;
                let loop_end = clip
                    .loop_end
                    .unwrap_or_else(|| (loop_start + clip.length).max(loop_start + Ticks::QUARTER));

                if loop_end > loop_start && start_marker < loop_end {
                    let loop_len = loop_end - loop_start;
                    let first_len = loop_end - start_marker;
                    let rep0_end_tick = (clip.start + first_len).min(clip.start + clip.length);
                    let rep0_end_frame = tempo.ticks_to_frames(rep0_end_tick, sample_rate).get();

                    // Repetition 0
                    if clip_start_frame < to_frame && rep0_end_frame > from_frame {
                        process_clip_segment(
                            clip,
                            start_marker,
                            clip.start,
                            rep0_end_tick,
                            0,
                            from_frame,
                            to_frame,
                            tempo,
                            sample_rate,
                            &mut self.rng,
                            &mut self.next_note_id,
                            &mut self.scratch_notes,
                            &mut self.messages,
                            &mut self.sounding,
                        );
                    }
                    if from_frame <= rep0_end_frame && rep0_end_frame < to_frame {
                        let off_offset = (rep0_end_frame - from_frame) as u32;
                        stop_clip_sounding(
                            clip.id,
                            off_offset,
                            &mut self.sounding,
                            &mut self.scratch_notes,
                        );
                    }

                    // Repetitions k >= 1
                    if rep0_end_tick < clip.start + clip.length {
                        let from_tick = tempo.frames_to_ticks(Frames(from_frame), sample_rate);
                        let to_tick = tempo.frames_to_ticks(Frames(to_frame), sample_rate);

                        let k_start = if from_tick > rep0_end_tick {
                            1 + ((from_tick - rep0_end_tick).get() / loop_len.get()) as usize
                        } else {
                            1
                        };
                        let k_end = if to_tick > rep0_end_tick {
                            1 + ((to_tick - rep0_end_tick).get() / loop_len.get() + 1) as usize
                        } else {
                            1
                        };

                        for k in k_start..=k_end {
                            let rep_k_start_tick = rep0_end_tick + loop_len * (k as i64 - 1);
                            if rep_k_start_tick >= clip.start + clip.length {
                                break;
                            }
                            let rep_k_end_tick =
                                (rep_k_start_tick + loop_len).min(clip.start + clip.length);
                            let rep_k_start_frame =
                                tempo.ticks_to_frames(rep_k_start_tick, sample_rate).get();
                            let rep_k_end_frame =
                                tempo.ticks_to_frames(rep_k_end_tick, sample_rate).get();

                            if rep_k_start_frame < to_frame && rep_k_end_frame > from_frame {
                                process_clip_segment(
                                    clip,
                                    loop_start,
                                    rep_k_start_tick,
                                    rep_k_end_tick,
                                    k as u64,
                                    from_frame,
                                    to_frame,
                                    tempo,
                                    sample_rate,
                                    &mut self.rng,
                                    &mut self.next_note_id,
                                    &mut self.scratch_notes,
                                    &mut self.messages,
                                    &mut self.sounding,
                                );
                            }
                            if from_frame <= rep_k_end_frame && rep_k_end_frame < to_frame {
                                let off_offset = (rep_k_end_frame - from_frame) as u32;
                                stop_clip_sounding(
                                    clip.id,
                                    off_offset,
                                    &mut self.sounding,
                                    &mut self.scratch_notes,
                                );
                            }
                        }
                    }
                }
            }
        }

        self.send_scratch_notes(block);
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            if event.param == crate::clips::player::AUDIBLE {
                self.audible = match event.value {
                    ParamValue::Set(v) => v != 0.0,
                    ParamValue::Default => true,
                };
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        self.clips = Some(data);
    }
}
