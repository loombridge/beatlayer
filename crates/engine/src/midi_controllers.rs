//! MIDI controller data in a playing MIDI clip (WP-2.29): the clip's MIDI
//! Ctrl envelopes and its notes' pressure, sent to the track's devices as
//! channel messages and polyphonic pressure.
//!
//! The rules are in the MIDI controller data spec, R3 to R5. Everything here
//! depends only on the frame it is asked about and on the clip, never on
//! what an earlier block sent, so a render is the same in any block size,
//! and nothing allocates: events go into a buffer the player made before
//! the audio thread ran.

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_model::{AutomationTarget, Breakpoint, Clip, Note, curve_value};

use crate::node::{NoteEvent, NoteExpression, NoteKind};

/// How far apart a sloped stretch's messages are, in seconds: a message at
/// least this often while the value changes (R3 asks for 5 ms).
const STREAM_SECONDS: f64 = 0.004;

/// One of a clip's MIDI controller envelopes, ready for the audio thread.
#[derive(Clone, Debug)]
pub struct PlacedController {
    target: AutomationTarget,
    /// Whether it follows the clip's region and loop.
    pub linked: bool,
    points: Vec<Breakpoint>,
}

/// A stretch of a clip's playback: the contents `content_start..content_end`
/// play at `song_start` on, one tick per tick.
#[derive(Clone, Copy, Debug)]
pub struct Stretch {
    pub content_start: Ticks,
    pub content_end: Ticks,
    pub song_start: Ticks,
}

impl PlacedController {
    /// The MIDI controller envelopes of `clip` that hold breakpoints.
    pub fn of(clip: &Clip) -> Vec<Self> {
        clip.envelopes()
            .filter(|e| e.target.is_midi() && !e.envelope.is_empty())
            .map(|e| Self {
                target: e.target,
                linked: e.linked,
                points: e.envelope.points().collect(),
            })
            .collect()
    }

    /// The controller's value as a whole number of the message's own units.
    fn level(&self, value: f64) -> i32 {
        let (low, high) = self.target.midi_range().unwrap_or((0.0, 127.0));
        value.clamp(low, high).round() as i32
    }

    fn message(&self, offset: u32, level: i32) -> Option<NoteEvent> {
        let seven = level as f32 / 127.0;
        let kind = match self.target {
            AutomationTarget::MidiCc { number } => NoteKind::Control {
                controller: number,
                value: seven,
            },
            AutomationTarget::MidiPitchBend => NoteKind::PitchBend {
                value: if level >= 0 {
                    level as f32 / 8191.0
                } else {
                    level as f32 / 8192.0
                },
            },
            AutomationTarget::MidiChannelPressure => NoteKind::ChannelPressure { value: seven },
            _ => return None,
        };
        Some(NoteEvent {
            offset,
            kind,
            channel: 0,
            key: 0,
            velocity: 0.0,
            note_id: None,
        })
    }

    /// Puts the messages of the frames `from..to` (song frames) into `out`,
    /// at offsets counted from `from`, for the contents of `stretch`
    /// (R3). A breakpoint sends its value on its own frame unless the value
    /// is already there, so a flat stretch sends nothing; a sloped stretch
    /// sends the value on every frame that is a multiple of the stream step
    /// when it differs from the one before.
    // Spec: midi-controller-data R3
    pub fn play(
        &self,
        stretch: Stretch,
        from: i64,
        to: i64,
        tempo: &TempoMap,
        rate: SampleRate,
        out: &mut Vec<NoteEvent>,
    ) {
        let Stretch {
            content_start,
            content_end,
            song_start,
        } = stretch;
        let step = ((rate.as_f64() * STREAM_SECONDS) as i64).max(1);
        let points = &self.points;
        let frame_of = |at: Ticks| {
            tempo
                .ticks_to_frames(song_start + (at - content_start), rate)
                .get()
        };
        let tick_of =
            |frame: i64| content_start + (tempo.frames_to_ticks(Frames(frame), rate) - song_start);
        // The level on the slope from `a` to `b` at `frame`.
        let on_slope = |a: &Breakpoint, b: &Breakpoint, frame: i64| {
            let span = (b.at - a.at).get() as f64;
            let along = ((tick_of(frame) - a.at).get() as f64 / span).clamp(0.0, 1.0);
            self.level(curve_value(a.value, b.value, along, a.bend))
        };
        let mut push = |frame: i64, level: i32| {
            if out.len() < out.capacity()
                && let Some(event) = self.message((frame - from) as u32, level)
            {
                out.push(event);
            }
        };

        // shortcut: each block places the breakpoints before it again, as the
        // notes are; search by frame if a clip ever holds thousands.
        let first = points.partition_point(|p| p.at < content_start);
        for i in first..points.len() {
            let b = points[i];
            if b.at >= content_end {
                break;
            }
            let at = frame_of(b.at);
            if at >= to {
                break;
            }
            if at < from {
                continue;
            }
            // What the controller held just before: the value the stretch
            // before this breakpoint ended on. The first breakpoint of a
            // stretch has no before, so it always sends.
            let held = (i > first).then(|| {
                let a = points[i - 1];
                let last = (at - 1).div_euclid(step) * step;
                if a.value != b.value && last > frame_of(a.at) {
                    on_slope(&a, &b, last)
                } else {
                    self.level(a.value)
                }
            });
            let level = self.level(b.value);
            if held != Some(level) {
                push(at, level);
            }
        }

        for j in first.saturating_sub(1)..points.len().saturating_sub(1) {
            let (a, b) = (points[j], points[j + 1]);
            if a.at >= content_end {
                break;
            }
            if a.value == b.value {
                continue;
            }
            let began = a.at >= content_start;
            let start = frame_of(if began { a.at } else { content_start });
            if start >= to {
                break;
            }
            let end = frame_of(b.at.min(content_end));
            let mut frame = (start + 1).max(from);
            frame = (frame + step - 1).div_euclid(step) * step;
            while frame < end && frame < to {
                let level = on_slope(&a, &b, frame);
                let before = if frame - step > start {
                    Some(on_slope(&a, &b, frame - step))
                } else {
                    began.then(|| self.level(a.value))
                };
                if before != Some(level) {
                    push(frame, level);
                }
                frame += step;
            }
        }
    }
}

/// A note while it sounds: what its pressure needs to be placed.
#[derive(Clone, Copy, Debug)]
pub struct Sounding {
    /// The ID its note-on carried.
    pub note_id: Option<u32>,
    /// Where the note starts in the song.
    pub song_start: Ticks,
    /// The song frames it starts and ends on.
    pub on: i64,
    pub off: i64,
}

/// Puts the pressure of `note` that falls on the frames `from..to` into
/// `out`, while the note sounds: each point is polyphonic pressure for the
/// note, on its frame (R3).
// Spec: midi-controller-data R3
pub fn play_pressure(
    note: &Note,
    sounding: Sounding,
    (from, to): (i64, i64),
    tempo: &TempoMap,
    rate: SampleRate,
    out: &mut Vec<NoteEvent>,
) {
    let Sounding {
        note_id,
        song_start,
        on,
        off,
    } = sounding;
    for point in &note.pressure {
        let frame = tempo.ticks_to_frames(song_start + point.at, rate).get();
        if frame < on.max(from) || frame >= off.min(to) {
            continue;
        }
        if out.len() < out.capacity() {
            out.push(NoteEvent {
                offset: (frame - from) as u32,
                kind: NoteKind::Expression {
                    expression: NoteExpression::Pressure,
                    value: f32::from(point.value) / 127.0,
                },
                channel: 0,
                key: note.key,
                velocity: 0.0,
                note_id,
            });
        }
    }
}
