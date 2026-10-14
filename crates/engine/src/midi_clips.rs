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

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_model::{ClipData, ClipId, Note, Track};

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
}

/// A clip's notes and markers placed on the track.
#[derive(Clone, Debug)]
pub struct PlacedMidiClip {
    pub id: ClipId,
    pub start: Ticks,
    pub length: Ticks,
    pub mute: bool,
    pub start_marker: Ticks,
    pub end_marker: Option<Ticks>,
    pub looping: bool,
    pub loop_start: Ticks,
    pub loop_end: Option<Ticks>,
    pub notes: Vec<Note>,
}

/// The list of MIDI clips for a track, sent to [`MidiClipPlayer`] as [`NodeData`].
#[derive(Clone, Debug, Default)]
pub struct MidiClipList {
    pub clips: Vec<PlacedMidiClip>,
}

impl MidiClipList {
    /// Builds the placed MIDI clips for `track`.
    #[must_use]
    pub fn build(track: &Track, _tempo: &TempoMap, _rate: SampleRate) -> Self {
        let mut clips = Vec::new();
        for clip in track.clips() {
            if clip.mute {
                continue;
            }
            let ClipData::Midi(midi) = &clip.data else {
                continue;
            };
            if clip.length <= Ticks::ZERO {
                continue;
            }
            let mut notes: Vec<Note> = midi.notes.iter().filter(|n| !n.mute).copied().collect();
            notes.sort_by_key(|n| n.start);
            clips.push(PlacedMidiClip {
                id: clip.id,
                start: clip.start,
                length: clip.length,
                mute: clip.mute,
                start_marker: midi.start,
                end_marker: midi.end,
                looping: midi.looping,
                loop_start: midi.loop_start,
                loop_end: midi.loop_end,
                notes,
            });
        }
        clips.sort_by_key(|c| c.start);
        Self { clips }
    }
}

/// The node that plays a MIDI track's clips.
pub struct MidiClipPlayer {
    clips: Option<NodeData>,
    sounding: Vec<ActiveNote>,
    scratch_notes: Vec<NoteEvent>,
    audible: bool,
    last_position: Option<i64>,
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
            audible: true,
            last_position: None,
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
        if self.scratch_notes.is_empty() {
            return;
        }
        let frames = block.frames() as u32;
        for note in &mut self.scratch_notes {
            if note.offset >= frames {
                note.offset = frames.saturating_sub(1);
            }
        }
        self.scratch_notes.sort_by(|a, b| {
            a.offset
                .cmp(&b.offset)
                .then_with(|| match (a.kind, b.kind) {
                    (NoteKind::Off, NoteKind::On) => std::cmp::Ordering::Less,
                    (NoteKind::On, NoteKind::Off) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                })
        });
        for &note in &self.scratch_notes {
            block.send_note(note);
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
    from_frame: i64,
    to_frame: i64,
    tempo: &TempoMap,
    sample_rate: SampleRate,
    rng: &mut Rng,
    next_note_id: &mut u32,
    scratch_notes: &mut Vec<NoteEvent>,
    sounding: &mut Vec<ActiveNote>,
) {
    let segment_len = segment_song_end - segment_song_start;
    let segment_clip_end = segment_clip_start + segment_len;

    for note in &clip.notes {
        if note.start < segment_clip_start || note.start >= segment_clip_end {
            continue;
        }
        let note_song_start = segment_song_start + (note.start - segment_clip_start);
        let note_on_frame = tempo.ticks_to_frames(note_song_start, sample_rate).get();
        if note_on_frame < from_frame || note_on_frame >= to_frame {
            continue;
        }

        // Probability check
        if note.probability < 1.0 && (note.probability <= 0.0 || rng.next_f64() > note.probability)
        {
            continue;
        }

        // Velocity deviation
        let dev = note.velocity_deviation as i32;
        let vel = if dev == 0 {
            note.velocity
        } else if dev > 0 {
            rng.next_in_range(note.velocity as i32, (note.velocity as i32 + dev).min(127)) as u8
        } else {
            rng.next_in_range((note.velocity as i32 + dev).max(1), note.velocity as i32) as u8
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

        for clip in &list.clips {
            let clip_start_frame = tempo.ticks_to_frames(clip.start, sample_rate).get();
            let clip_end_frame = tempo
                .ticks_to_frames(clip.start + clip.length, sample_rate)
                .get();

            if clip_start_frame >= to_frame || clip_end_frame <= from_frame {
                continue;
            }

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
                        from_frame,
                        to_frame,
                        tempo,
                        sample_rate,
                        &mut self.rng,
                        &mut self.next_note_id,
                        &mut self.scratch_notes,
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
                            from_frame,
                            to_frame,
                            tempo,
                            sample_rate,
                            &mut self.rng,
                            &mut self.next_note_id,
                            &mut self.scratch_notes,
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
                                    from_frame,
                                    to_frame,
                                    tempo,
                                    sample_rate,
                                    &mut self.rng,
                                    &mut self.next_note_id,
                                    &mut self.scratch_notes,
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
