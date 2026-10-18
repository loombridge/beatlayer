//! Where modulators put their output and targets read it (WP-5.10; the
//! modulation spec, R14, R16 and R20).
//!
//! Before a program runs its tasks it runs each modulator once over the
//! block's frames ([`Node::control`](crate::Node::control)). A modulator
//! writes, for each of its eight slots, two rows of one value per frame:
//! the slot's contribution to its target in Modulation mode, and the
//! position it holds its target at in Remote Control mode (`-1` for none).
//! A target reads the rows of the slots that map it while it works out the
//! value it plays ([`AutomationList::played`](crate::AutomationList::played)).
//! No allocation, lock or blocking call is involved: the rows are one buffer
//! made with the program.

use bl_core::time::{SampleRate, TempoMap};
use bl_model::modulator::SLOTS;

use crate::node::{NodeEvent, TransportInfo};

/// The rows a modulator owns: a contribution row and a hold row per slot.
pub const ROWS: usize = SLOTS as usize * 2;

/// A hold row's value while the slot isn't holding its target.
pub const NOT_HELD: f32 = -1.0;

/// The first row of the pair `slot` of modulator `modulator` owns.
#[must_use]
pub const fn row(modulator: u32, slot: u8) -> u32 {
    modulator * ROWS as u32 + slot as u32 * 2
}

/// The modulators' rows for the block part being run, read-only.
#[derive(Clone, Copy, Debug)]
pub struct Bus<'a> {
    data: &'a [f32],
    stride: usize,
    frames: usize,
}

impl<'a> Bus<'a> {
    /// No rows: what a block outside a program's run sees.
    pub const EMPTY: Bus<'static> = Bus {
        data: &[],
        stride: 0,
        frames: 0,
    };

    /// Rows `stride` apart in `data`, `frames` frames of each.
    pub(crate) fn new(data: &'a [f32], stride: usize, frames: usize) -> Self {
        Self {
            data,
            stride,
            frames,
        }
    }

    fn at(&self, row: u32) -> &'a [f32] {
        let start = row as usize * self.stride;
        self.data
            .get(start..start + self.frames)
            .unwrap_or_default()
    }

    /// The contribution row starting a slot's pair: its share of the
    /// target's travel on each frame (R14), 0 while it holds (R16). Empty
    /// for a row the bus doesn't hold.
    #[must_use]
    pub fn contribution(&self, row: u32) -> &'a [f32] {
        self.at(row)
    }

    /// The hold row of the pair starting at `row`: the position on the
    /// target's travel the slot holds it at on each frame (R16), or
    /// [`NOT_HELD`].
    #[must_use]
    pub fn hold(&self, row: u32) -> &'a [f32] {
        self.at(row + 1)
    }
}

/// What a modulator node is given to work out its rows for a block part.
pub struct Control<'a> {
    /// The frames in the part.
    pub frames: usize,
    /// The part's parameter changes, in order, with offsets inside it.
    pub events: &'a [NodeEvent],
    /// The transport as the part starts.
    pub transport: TransportInfo,
    /// The sample rate.
    pub sample_rate: SampleRate,
    /// The song's tempo map.
    pub tempo: &'a TempoMap,
    /// The rows of the modulators that ran before this one, which its own
    /// parameters may be mapped to.
    pub earlier: Bus<'a>,
    /// Where this modulator writes its [`ROWS`] rows, `stride` apart.
    pub out: &'a mut [f32],
    /// The distance between rows in `out`.
    pub stride: usize,
}

impl Control<'_> {
    /// Row `index` of the modulator's own, `frames` long, writable.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`ROWS`].
    pub fn row_mut(&mut self, index: usize) -> &mut [f32] {
        assert!(index < ROWS, "row {index} out of range");
        let start = index * self.stride;
        &mut self.out[start..start + self.frames]
    }

    /// Gives every row its idle value: no contribution, nothing held.
    pub fn idle(&mut self) {
        for index in 0..ROWS {
            let value = if index % 2 == 0 { 0.0 } else { NOT_HELD };
            self.row_mut(index).fill(value);
        }
    }
}

/// A Shaper's shape as node data (R23): sent to the device when it
/// changes, as its breakpoints aren't parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape(pub Vec<bl_model::modulator::ShapePoint>);
