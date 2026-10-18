//! What the modulators driven by audio, notes and MIDI share (WP-5.11; the
//! modulation spec, R13, R14, R16, R20, R28): their output stage.
//!
//! Where the LFO and the Shaper work their output out before any
//! track runs, these hear their track's signal, so they run in it, at their
//! place in its chain, and write the rows their targets read as they go. The
//! output is taken every [`GRID`](crate::modulator::GRID) frames on a grid
//! counted from the frame the transport last started and moves in a straight
//! line from each value to the next, as the others' does (R20); but each
//! segment ends at the level taken at its start, since the level at its end
//! isn't heard yet, so the output follows its level one step behind
//! (Beatlayer's own). Everything written is kept for 32 frames, for a target
//! that runs before the modulator in its own chain (R28).

use bl_dsp::Random;
use bl_engine::NoteEvent;
use bl_engine::modbus::{NOT_HELD, ROWS};
use bl_model::ParamId;
use bl_model::modulator::{self, DEPTH, MODE, POLARITY, SLOTS};

use crate::framework::{Audio, Modulation, ParamBank};
use crate::midi_fx::Key;
use crate::modulator::GRID;

/// How many frames a late row lags (R28): the longest part a program with a
/// late row runs in.
const LAG: usize = 32;

/// Where a modulator's mapping parameters sit in its bank, found once per
/// stretch.
pub(crate) struct Indices {
    slots: usize,
    depth: usize,
    mode: usize,
    polarity: usize,
    amounts: [usize; SLOTS as usize],
    mins: [usize; SLOTS as usize],
    maxes: [usize; SLOTS as usize],
}

impl Indices {
    /// The indices for a modulator of `slots` slots.
    pub(crate) fn new(params: &ParamBank, slots: u8) -> Self {
        let index = |id: ParamId| params.index(id).expect("the description holds it");
        let each = |f: fn(u8) -> ParamId| {
            std::array::from_fn(|s| {
                if s < slots as usize {
                    index(f(s as u8))
                } else {
                    0
                }
            })
        };
        Self {
            slots: slots as usize,
            depth: index(DEPTH),
            mode: index(MODE),
            polarity: index(POLARITY),
            amounts: each(modulator::amount),
            mins: each(modulator::min),
            maxes: each(modulator::max),
        }
    }
}

/// The output stage of a modulator driven by the signal.
pub(crate) struct SignalOut {
    /// Frames since the transport last started, or since the first frame.
    clock: u64,
    was_playing: bool,
    primed: bool,
    /// The modulator didn't run since it last did: it was switched off.
    gap: bool,
    /// The song position after the last stretch it ran.
    end: i64,
    /// Each slot's level at the last grid point and the one before.
    from: [f64; SLOTS as usize],
    to: [f64; SLOTS as usize],
    /// What was written in the last [`LAG`] frames, row by row.
    ring: Vec<f32>,
    /// How many frames were written, modulo [`LAG`].
    at: usize,
}

impl SignalOut {
    pub(crate) fn new() -> Self {
        let mut ring = vec![0.0; ROWS * LAG];
        Self::idle(&mut ring);
        Self {
            clock: 0,
            was_playing: false,
            primed: false,
            gap: false,
            end: 0,
            from: [0.0; SLOTS as usize],
            to: [0.0; SLOTS as usize],
            ring,
            at: 0,
        }
    }

    /// Gives every row of `ring` its idle value: no contribution, nothing
    /// held.
    fn idle(ring: &mut [f32]) {
        for (row, values) in ring.chunks_mut(LAG).enumerate() {
            values.fill(if row % 2 == 0 { 0.0 } else { NOT_HELD });
        }
    }

    /// Notes that the modulator is switched off, as it is each block it
    /// doesn't run in.
    pub(crate) fn off(&mut self) {
        self.gap = true;
    }

    /// Notes the transport's state at a stretch's start; true when it has
    /// just started, when the modulator starts again (R20, R21): its
    /// output, and what a target that runs before it reads of it, begin as
    /// they would in a render that starts there. A modulator switched on
    /// again while the song plays takes its level at once, with nothing
    /// behind it, and its grid carries on where the song has got to.
    pub(crate) fn begin(&mut self, audio: &Audio<'_, '_>) -> bool {
        let playing = audio.playing();
        let position = audio.song_position().get();
        let started = playing && !self.was_playing;
        if started {
            self.clock = 0;
        } else if self.gap && playing && position >= self.end {
            self.clock += (position - self.end) as u64;
        }
        if started || self.gap {
            self.primed = false;
            self.at = 0;
            Self::idle(&mut self.ring);
        }
        self.gap = false;
        self.was_playing = playing;
        self.end = position + audio.frames() as i64;
        started
    }

    /// Whether the next frame written is on the grid, where the modulator's
    /// level is taken.
    pub(crate) fn at_grid(&self) -> bool {
        self.clock.is_multiple_of(GRID)
    }

    /// Writes frame `f` of the stretch's rows from the modulator's `levels`
    /// (0 to 1) at this frame, one for each slot, or one for all of them:
    /// the level is taken at each grid point and the output runs to it over
    /// the next [`GRID`] frames (R20).
    pub(crate) fn frame(
        &mut self,
        audio: &mut Audio<'_, '_>,
        ix: &Indices,
        params: &ParamBank,
        f: usize,
        levels: &[f64],
    ) {
        let level = |slot: usize| levels.get(slot).or(levels.first()).copied().unwrap_or(0.0);
        if !self.primed || self.clock.is_multiple_of(GRID) {
            for slot in 0..ix.slots {
                self.from[slot] = if self.primed {
                    self.to[slot]
                } else {
                    level(slot)
                };
                self.to[slot] = level(slot);
            }
            self.primed = true;
        }
        let t = (self.clock % GRID) as f64 / GRID as f64;
        let at = |index: usize| f64::from(params.ramp(index)[f]);
        let depth = at(ix.depth) / 100.0;
        let remote = at(ix.mode) >= 0.5;
        let unipolar = at(ix.polarity) >= 0.5;
        for slot in 0..ix.slots {
            let u = self.from[slot] + (self.to[slot] - self.from[slot]) * t;
            // Spec: modulation R14, R16.
            let (contribution, hold) = if remote {
                let low = at(ix.mins[slot]) / 100.0;
                let high = at(ix.maxes[slot]) / 100.0;
                (0.0, (low + depth * u * (high - low)).clamp(0.0, 1.0) as f32)
            } else {
                let swing = if unipolar { u } else { 2.0 * u - 1.0 };
                (
                    (depth * at(ix.amounts[slot]) / 100.0 * swing) as f32,
                    NOT_HELD,
                )
            };
            for (row, value) in [(slot * 2, contribution), (slot * 2 + 1, hold)] {
                // A node run outside a program has no rows to write.
                if let Some(out) = audio.row_mut(row).get_mut(f) {
                    *out = value;
                }
                self.ring[row * LAG + self.at] = value;
            }
        }
        self.clock += 1;
        self.at = (self.at + 1) % LAG;
    }

    /// Fills the late rows for the part about to run with what was written
    /// 32 frames before each of its frames (R28).
    pub(crate) fn lag(&self, modulation: &mut Modulation<'_, '_>) {
        // The part that starts the transport is the first of a new start.
        let fresh = self.gap || (modulation.playing() && !self.was_playing);
        for row in 0..ROWS {
            let out = modulation.row_mut(row);
            for (i, value) in out.iter_mut().enumerate() {
                *value = if fresh {
                    if row % 2 == 0 { 0.0 } else { NOT_HELD }
                } else {
                    self.ring[row * LAG + (self.at + i) % LAG]
                };
            }
        }
    }
}

/// How many keys a modulator follows at once.
const HELD_ROOM: usize = 128;

/// The keys held down, in the order they went down, with the velocity each
/// went down with: what the modulators driven by notes follow. Made with
/// the device and fixed in size; a key past its room is not followed.
pub(crate) struct Held {
    keys: Vec<(Key, f32)>,
}

impl Held {
    pub(crate) fn new() -> Self {
        Self {
            keys: Vec::with_capacity(HELD_ROOM),
        }
    }

    /// Notes the key `note` puts down. A key put down again without being
    /// lifted is the same key, so its one note-off lifts it.
    pub(crate) fn on(&mut self, note: &NoteEvent) {
        self.off(note);
        if self.keys.len() < HELD_ROOM {
            self.keys.push((Key::of(note), note.velocity));
        }
    }

    /// Takes away the key `note` lifts, if it was followed; false for a key
    /// that went down while the modulator was switched off.
    pub(crate) fn off(&mut self, note: &NoteEvent) -> bool {
        match self.keys.iter().position(|(key, _)| key.names(note)) {
            Some(at) => {
                self.keys.remove(at);
                true
            }
            None => false,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The most recent key held.
    pub(crate) fn last(&self) -> Option<Key> {
        self.keys.last().map(|&(key, _)| key)
    }

    pub(crate) fn clear(&mut self) {
        self.keys.clear();
    }
}

/// What Jitter, Smooth and Offset do to a level taken at a grid point (R22,
/// the LFO's laws, R26): Jitter adds up to half its share of a draw
/// either way, Smooth moves the output towards each new level by a share
/// that falls from all of it at 0 % to 2 % at 100 %, and Offset then adds
/// its share and clamps the output to 0 to 1. The draws come from a
/// generator seeded by the device and started again when the transport
/// starts (R21).
pub(crate) struct Post {
    seed: u64,
    random: Random,
    smoothed: f64,
    primed: bool,
}

impl Post {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            seed,
            random: Random::seeded(seed ^ 0x5EED),
            smoothed: 0.0,
            primed: false,
        }
    }

    /// Starts again: the transport started.
    pub(crate) fn restart(&mut self) {
        *self = Self::new(self.seed);
    }

    /// `level` through Jitter, Smooth and Offset, each a share from 0 to 1
    /// (Offset from −1 to 1). The draw is made whether or not Jitter is
    /// used, so the stream is the same whatever the control says.
    pub(crate) fn apply(&mut self, level: f64, jitter: f64, smooth: f64, offset: f64) -> f64 {
        let noise = self.random.next_bipolar();
        let w = (level + jitter * 0.5 * noise).clamp(0.0, 1.0);
        self.smoothed = if self.primed {
            self.smoothed + (w - self.smoothed) * (1.0 - 0.98 * smooth)
        } else {
            w
        };
        self.primed = true;
        (self.smoothed + offset).clamp(0.0, 1.0)
    }
}
