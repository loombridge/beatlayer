//! Accent and Repeat (`docs/specs/push-2-notes.md`, R54 to R56).

use std::collections::BTreeMap;
use std::time::Instant;

use bl_core::midi::MidiMessage;
use bl_core::time::Ticks;
use bl_model::TrackId;

use super::{Down, Note, pads::note_on};
use crate::content::{Ctx, Gesture};
use crate::controls::cc;

/// The velocity Accent gives (R54).
const ACCENT: u8 = 127;

/// A switch that a tap turns on or off and a hold turns on until released
/// (R54; [Push 2](push-2.md) R7).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Switch {
    latched: bool,
    held: bool,
}

impl Switch {
    pub(super) fn on(self) -> bool {
        self.latched || self.held
    }

    fn gesture(&mut self, gesture: Gesture) {
        match gesture {
            Gesture::Tap => self.latched = !self.latched,
            Gesture::Hold => self.held = true,
            Gesture::Release => self.held = false,
            _ => {}
        }
    }
}

/// The pads held while Repeat is on, and where its notes are (R55).
#[derive(Default)]
pub(super) struct Repeat {
    /// Repeat held rather than tapped.
    held: bool,
    /// The held pads' notes and the velocity their pressure gives.
    pads: BTreeMap<(u8, u8), (u8, u8)>,
    /// The next note's place on the step grid, counted in steps.
    next: Option<i64>,
    /// While the transport is stopped, where the grid starts: the press.
    origin: Option<Instant>,
    /// The notes sounding and the tick each ends at.
    sounding: Vec<(u8, i64)>,
}

/// A pad's pressure as a velocity from 1 to 127 (R55).
fn velocity_of(pressure: u8) -> u8 {
    (1 + u32::from(pressure.min(127)) * 126 / 127) as u8
}

impl Note {
    /// Whether Repeat is on for `track`.
    pub(super) fn repeating(&self, track: TrackId) -> bool {
        self.repeat.held || self.memory.get(&track).is_some_and(|m| m.repeat)
    }

    /// `velocity`, or Accent's where it is on (R54).
    pub(super) fn accented(&self, velocity: u8) -> u8 {
        if self.accent.on() { ACCENT } else { velocity }
    }

    /// Plays `note` for a pad pressed now, unless Repeat is on: its notes
    /// come from the grid (R55).
    pub(super) fn voice(&self, track: TrackId, ctx: &mut Ctx<'_>, note: u8, velocity: u8) {
        if !self.repeating(track) {
            ctx.cx.play_at(ctx.at, note_on(note, velocity));
        }
    }

    /// Accent's and Repeat's buttons (R54, R55).
    pub(super) fn repeat_button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        if button == cc::ACCENT {
            self.accent.gesture(gesture);
            return;
        }
        match gesture {
            Gesture::Tap => {
                if let Some(track) = ctx.model.and_then(super::clip::selected_track) {
                    let memory = self.memory(track);
                    memory.repeat = !memory.repeat;
                }
            }
            Gesture::Hold => self.repeat.held = true,
            Gesture::Release => self.repeat.held = false,
            _ => {}
        }
    }

    /// A pad that played `note` is held with `velocity`: Repeat takes it.
    pub(super) fn repeat_pad(&mut self, track: TrackId, pad: (u8, u8), down: Down, velocity: u8) {
        if !self.repeating(track) {
            return;
        }
        if let Down::Played(note)
        | Down::Picked {
            key: note,
            played: true,
        } = down
        {
            self.repeat.pads.insert(pad, (note, velocity));
        }
    }

    pub(super) fn repeat_release(&mut self, pad: (u8, u8)) {
        self.repeat.pads.remove(&pad);
    }

    pub(super) fn repeat_pressure(&mut self, pad: (u8, u8), pressure: u8) {
        if let Some(held) = self.repeat.pads.get_mut(&pad) {
            held.1 = if self.accent.on() {
                ACCENT
            } else {
                velocity_of(pressure)
            };
        }
    }

    /// Plays the notes that came due since the last tick, and ends those
    /// that have lasted a step (R55).
    pub(super) fn tick_repeat(&mut self, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(size) = self
            .track
            .map(|track| self.memory.get(&track).copied().unwrap_or_default().size)
        else {
            return;
        };
        let step = size.ticks().get();
        let swing = ctx.swing.clamp(0.0, 1.0);
        let playing = model.transport.playing;
        let repeat = &mut self.repeat;
        // The tick the grid has reached: the song's, or since the press.
        let now = if playing {
            repeat.origin = None;
            model.transport.position.get()
        } else {
            let origin = *repeat.origin.get_or_insert(ctx.at);
            let beats =
                ctx.at.saturating_duration_since(origin).as_secs_f64() * model.tempo.get() / 60.0;
            (beats * Ticks::QUARTER.get() as f64) as i64
        };
        // Every second note is delayed by `swing` of half a step.
        let due = |n: i64| {
            n * step
                + if n % 2 != 0 {
                    (step as f64 * swing / 2.0) as i64
                } else {
                    0
                }
        };
        let mut ended = Vec::new();
        repeat.sounding.retain(|&(note, end)| {
            let over = repeat.pads.is_empty() || now >= end;
            if over {
                ended.push(note);
            }
            !over
        });
        for note in ended {
            ctx.cx.play_at(ctx.at, MidiMessage::note_off(0, note));
        }
        if repeat.pads.is_empty() {
            repeat.next = None;
            repeat.origin = None;
            return;
        }
        let mut next = *repeat.next.get_or_insert(if playing {
            (now + step - 1).div_euclid(step)
        } else {
            0
        });
        // A long stall, a locate or the loop wrapping skips to the present,
        // not a burst or a silence.
        if (now - due(next)).abs() > 2 * step {
            next = (now + step - 1).div_euclid(step);
        }
        while due(next) <= now {
            for &(note, velocity) in repeat.pads.values() {
                ctx.cx.play_at(ctx.at, MidiMessage::note_off(0, note));
                ctx.cx.play_at(ctx.at, note_on(note, velocity));
                repeat.sounding.push((note, due(next) + step));
            }
            next += 1;
        }
        repeat.next = Some(next);
    }
}
