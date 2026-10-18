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

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::{SampleRate, TempoMap};
use bl_model::modulator::{Mapping, ModTarget, SLOTS};
use bl_model::{ChainId, Device, DeviceId, Document, Track, TrackId};

use crate::disjoint::Shared;
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
///
/// A row is read from the shared storage only when asked for, so a thread
/// never holds a reference to a row another thread is writing: a signal
/// modulator's task writes its own rows while other tasks run.
#[derive(Clone, Copy)]
pub struct Bus<'a> {
    data: Option<&'a Shared<f32>>,
    /// How many values from the start of `data` the bus may read.
    len: usize,
    stride: usize,
    frames: usize,
}

impl<'a> Bus<'a> {
    /// No rows: what a block outside a program's run sees.
    pub const EMPTY: Bus<'static> = Bus {
        data: None,
        len: 0,
        stride: 0,
        frames: 0,
    };

    /// Rows `stride` apart in `data`, `frames` frames of each.
    ///
    /// # Safety
    ///
    /// While the bus is in use, no thread writes a row that the holder asks
    /// it for. The compiler chooses those rows: a modulator's direct row only
    /// for a target that runs after the modulator's task or chain, and its
    /// late row otherwise (R28).
    pub(crate) unsafe fn new(data: &'a Shared<f32>, stride: usize, frames: usize) -> Self {
        Self {
            data: Some(data),
            len: data.len(),
            stride,
            frames,
        }
    }

    /// The same bus, reading only the first `rows` rows.
    pub(crate) fn below(self, rows: usize) -> Self {
        Self {
            len: self.len.min(rows * self.stride),
            ..self
        }
    }

    fn at(&self, row: u32) -> &'a [f32] {
        let start = row as usize * self.stride;
        match self.data {
            // SAFETY: the contract of `new`.
            Some(data) if start + self.frames <= self.len => unsafe {
                data.view(start..start + self.frames)
            },
            _ => &[],
        }
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

/// Where a modulator's rows sit on the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModRow {
    /// The first row of its [`ROWS`]: what it writes for the part being run.
    pub(crate) direct: u32,
    /// For a modulator driven by audio, notes or MIDI, the first row of the
    /// [`ROWS`] holding what it wrote 32 frames ago (WP-5.11; the modulation
    /// spec, R28), for a target that runs before it in its own chain.
    pub(crate) late: Option<u32>,
}

/// The modulators of a program: where their rows sit and which tracks wait
/// on which (WP-5.11).
#[derive(Clone, Debug, Default)]
pub(crate) struct ModRows {
    pub(crate) rows: BTreeMap<DeviceId, ModRow>,
    /// `(a, b)`: track `b`, which holds a parameter a modulator on track `a`
    /// maps, runs after `a` (R28).
    pub(crate) waits: BTreeSet<(TrackId, TrackId)>,
}

impl ModRows {
    /// The row the slot of `device`, on `owner`, mapping `mapping`, is read
    /// from: its direct row, its late row where the target runs before the
    /// modulator in the same chain (R28), or none where the mapping carries
    /// nothing because the tracks would wait on each other.
    pub(crate) fn row_of(
        &self,
        document: &Document,
        owner: &Track,
        device: &Device,
        mapping: &Mapping,
    ) -> Option<u32> {
        let row = self.rows.get(&device.id)?;
        let Some(late) = row.late else {
            return Some(row.direct);
        };
        let track = document.target_track(mapping.target)?;
        if track != owner.id() {
            return self
                .waits
                .contains(&(owner.id(), track))
                .then_some(row.direct);
        }
        let position = |id: DeviceId| owner.devices_deep().position(|d| d.id == id);
        // Sends run as their return track's inputs, before its chain; a
        // mixer after it; a device in the order of the chain, unless the
        // two sit in different chains of one rack, which may run at once
        // on different threads (racks R15).
        let lagged = match mapping.target {
            ModTarget::Mixer { .. } => false,
            ModTarget::Send { .. } => true,
            ModTarget::Device { device: target, .. } => {
                position(target)
                    .zip(position(device.id))
                    .is_some_and(|(t, m)| t <= m)
                    || in_parallel_chains(owner, target, device.id)
            }
        };
        Some(if lagged { late } else { row.direct })
    }

    /// Whether any mapping of a modulator driven by audio, notes or MIDI
    /// reads a late row: the program then runs in parts of at most 32 frames.
    pub(crate) fn any_late(&self, document: &Document) -> bool {
        document.tracks().any(|owner| {
            owner.devices_deep().any(|device| {
                self.rows.get(&device.id).is_some_and(|r| r.late.is_some())
                    && device.mappings.iter().any(|mapping| {
                        self.row_of(document, owner, device, mapping)
                            .is_some_and(|row| Some(row) == self.rows[&device.id].late)
                    })
            })
        })
    }
}

/// Whether devices `a` and `b` of `track` sit in different chains of one
/// rack, at any depth: those chains may run on different threads at once.
fn in_parallel_chains(track: &Track, a: DeviceId, b: DeviceId) -> bool {
    fn path(device: &Device, id: DeviceId, at: &mut Vec<(DeviceId, ChainId)>) -> bool {
        if device.id == id {
            return true;
        }
        for chain in &device.chains {
            at.push((device.id, chain.id));
            if chain.devices.iter().any(|d| path(d, id, at)) {
                return true;
            }
            at.pop();
        }
        false
    }
    let find = |id| {
        let mut at = Vec::new();
        track.devices().any(|d| path(d, id, &mut at)).then_some(at)
    };
    let (Some(a), Some(b)) = (find(a), find(b)) else {
        return false;
    };
    a.iter()
        .zip(&b)
        .find(|(x, y)| x != y)
        .is_some_and(|(x, y)| x.0 == y.0)
}

/// A Shaper's shape as node data (R23): sent to the device when it
/// changes, as its breakpoints aren't parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape(pub Vec<bl_model::modulator::ShapePoint>);
