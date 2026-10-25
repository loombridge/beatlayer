//! The meters between the devices of a chain (the MIDI meters spec, R2 and
//! R6): one point at every device boundary, made with the program, that the
//! chain's run raises and the publisher reads and lets fall.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::mixer::METER_RELEASE_DB_PER_SECOND;
use crate::node::{NoteEvent, NoteKind};
use crate::state::{ChainMeter, MeterOwner, MeterSite, PointReading};

/// How long a MIDI meter takes to fall to dark after a note-on, and how long
/// a channel message or a per-note expression lights its dot (the MIDI meters
/// spec, R3 to R5).
pub const MIDI_METER_FALL_SECONDS: f64 = 0.3;

/// One meter point. The audio thread that runs the chain raises its cells
/// while the block runs; the publisher reads and lowers them once every task
/// has finished, so no cell is written by two threads at once. The cells hold
/// `f32` bits: a MIDI point's velocity, channel and expression levels, or an
/// audio point's two peaks.
pub(crate) struct MeterPoint {
    pub(crate) owner: MeterOwner,
    pub(crate) at: MeterSite,
    /// Whether the signal here is MIDI.
    pub(crate) midi: bool,
    cells: [AtomicU32; 3],
}

impl MeterPoint {
    pub(crate) fn new(owner: MeterOwner, at: MeterSite, midi: bool) -> Self {
        Self {
            owner,
            at,
            midi,
            cells: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
        }
    }

    /// Raises cell `cell` to `value` if it is higher.
    fn raise(&self, cell: usize, value: f32) {
        let held = &self.cells[cell];
        if value > f32::from_bits(held.load(Ordering::Relaxed)) {
            held.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// Notes the MIDI that passes in a block.
    pub(crate) fn measure_notes(&self, notes: &[NoteEvent]) {
        for note in notes {
            match note.kind {
                NoteKind::On => self.raise(0, note.velocity),
                NoteKind::Expression { .. } => self.raise(2, 1.0),
                kind if kind.is_message() => self.raise(1, 1.0),
                _ => {}
            }
        }
    }

    /// Notes the peak of the first two channels of a block's audio.
    pub(crate) fn measure_audio(
        &self,
        data: &[f32],
        channels: usize,
        stride: usize,
        frames: usize,
    ) {
        for c in 0..channels.min(2) {
            let peak = data[c * stride..c * stride + frames]
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            self.raise(c, peak);
        }
        // A mono engine shows its one channel on both sides, as a track's
        // meter does.
        if channels == 1 {
            self.raise(1, f32::from_bits(self.cells[0].load(Ordering::Relaxed)));
        }
    }

    /// What passed since the cells last fell, then lets them fall by the
    /// `frames` of the block just run at `rate`.
    pub(crate) fn publish(&self, frames: usize, rate: f64) -> ChainMeter {
        let seconds = frames as f64 / rate;
        let cell = |i: usize| f32::from_bits(self.cells[i].load(Ordering::Relaxed));
        let fall = |i: usize, to: f32| self.cells[i].store(to.to_bits(), Ordering::Relaxed);
        let reading = if self.midi {
            let linear = (seconds / MIDI_METER_FALL_SECONDS) as f32;
            let reading = PointReading::Midi {
                velocity: cell(0),
                channel: cell(1),
                expression: cell(2),
            };
            for i in 0..3 {
                fall(i, (cell(i) - linear).max(0.0));
            }
            reading
        } else {
            let reading = PointReading::Audio {
                peak: [cell(0), cell(1)],
            };
            let factor = libm::pow(10.0, -METER_RELEASE_DB_PER_SECOND * seconds / 20.0) as f32;
            for i in 0..2 {
                fall(i, cell(i) * factor);
            }
            reading
        };
        ChainMeter {
            owner: self.owner,
            at: self.at,
            reading,
        }
    }
}
