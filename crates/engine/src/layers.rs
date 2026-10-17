//! A multisampler's layers as its node reads them (WP-5.13; the multisampler
//! spec, R31): the document's layers with each one's decoded audio, sent to
//! the node as node data so a zone or layer edit never makes a new program.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};

use bl_media::SampleData;
use bl_model::Layer;

/// One layer and the audio it plays, at the engine's rate. A layer whose
/// media the library can't supply plays silence.
#[derive(Clone, Debug)]
pub struct LayerAudio {
    /// The layer as the document has it: zones, markers (in frames of the
    /// sample at its own rate) and settings.
    pub layer: Layer,
    /// The sample decoded whole into memory, if the library had it.
    pub sample: Option<Arc<SampleData>>,
}

/// Where Round Robin is in a group of layers with identical key zones (R8,
/// R9). The audio thread changes it through the shared data, so the node
/// allocates nothing for it.
#[derive(Debug, Default)]
pub struct RoundRobin {
    /// The layer (list index + 1) the group played last; 0 for none.
    pub last: AtomicU32,
    /// The layer (list index + 1) the group played first since the
    /// transport started; 0 for none.
    pub first: AtomicU32,
    /// Whether Other and Random go back to `first` on their next note,
    /// after a Reset Interval.
    pub restart: AtomicU32,
    /// The Reset Interval's multiple the group last played in, plus one; 0
    /// before it played.
    pub epoch: AtomicI64,
    /// The note that last chose a layer of this group, to choose once a
    /// note.
    pub stamp: AtomicU64,
}

/// Layers whose key zones are identical: the layers Round Robin rotates
/// among (R8).
#[derive(Debug, Default)]
pub struct Group {
    /// The layers' list indexes, in list order.
    pub members: Vec<u32>,
    /// The group's place in its rotation.
    pub state: RoundRobin,
}

/// What a multisampler's node is sent: its layers in list order and the
/// options of its layer list.
#[derive(Debug, Default)]
pub struct DeviceLayers {
    /// The layers, in list order.
    pub layers: Vec<LayerAudio>,
    /// RAM Mode (R23).
    pub ram_mode: bool,
    /// Loop crossfades are linear rather than constant-power (R19).
    pub linear_loop_fades: bool,
    /// Each layer's group, as an index into [`groups`](Self::groups).
    pub group_of: Vec<u32>,
    /// The groups of layers with identical key zones.
    pub groups: Vec<Group>,
}

impl DeviceLayers {
    /// The data for `layers`, with its groups worked out here, off the audio
    /// thread.
    #[must_use]
    pub fn new(layers: Vec<LayerAudio>, ram_mode: bool, linear_loop_fades: bool) -> Self {
        let mut groups: Vec<Group> = Vec::new();
        let mut group_of = Vec::with_capacity(layers.len());
        let mut seen: std::collections::BTreeMap<_, u32> = std::collections::BTreeMap::new();
        for (index, entry) in layers.iter().enumerate() {
            let key = entry.layer.key;
            let zone = (key.low, key.high, key.fade_low, key.fade_high);
            let group = *seen.entry(zone).or_insert_with(|| {
                groups.push(Group::default());
                groups.len() as u32 - 1
            });
            groups[group as usize].members.push(index as u32);
            group_of.push(group);
        }
        Self {
            layers,
            ram_mode,
            linear_loop_fades,
            group_of,
            groups,
        }
    }

    /// Takes every group back to the start of its rotation: the transport
    /// started (R8).
    pub fn reset_rotations(&self) {
        for group in &self.groups {
            let s = &group.state;
            s.last.store(0, Relaxed);
            s.first.store(0, Relaxed);
            s.restart.store(0, Relaxed);
            s.epoch.store(0, Relaxed);
        }
    }
}
