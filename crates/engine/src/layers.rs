//! A multisampler's layers as its node reads them (WP-5.13; the multisampler
//! spec, R31): the document's layers with each one's decoded audio, sent to
//! the node as node data so a zone or layer edit never makes a new program.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};

use bl_media::{ProgressiveSample, SampleData};
use bl_model::Layer;

/// A layer's audio, at the engine's rate.
#[derive(Clone, Debug)]
pub enum LayerSample {
    /// Decoded whole into memory.
    Whole(Arc<SampleData>),
    /// A long file whose start is in memory and whose rest is still being
    /// read (R23).
    Progressive(Arc<ProgressiveSample>),
}

impl LayerSample {
    /// The number of frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        match self {
            Self::Whole(data) => data.frames(),
            Self::Progressive(data) => data.frames(),
        }
    }

    /// The number of channels.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        match self {
            Self::Whole(data) => data.channel_count(),
            Self::Progressive(data) => data.channel_count(),
        }
    }

    /// The sample of `channel` at `frame`: silence past the end and, for a
    /// progressive sample, before the disk has delivered it. A mono sample
    /// reads as the same on every channel. Real-time safe.
    #[must_use]
    pub fn at(&self, channel: usize, frame: usize) -> f32 {
        match self {
            Self::Whole(data) => data
                .channel(channel)
                .or_else(|| data.channel(0))
                .and_then(|samples| samples.get(frame))
                .copied()
                .unwrap_or(0.0),
            Self::Progressive(data) => {
                let channel = if channel < data.channel_count() {
                    channel
                } else {
                    0
                };
                data.get(channel, frame)
            }
        }
    }

    /// Whether `frame` is in the file but not decoded yet.
    #[must_use]
    pub fn pending(&self, frame: usize) -> bool {
        match self {
            Self::Whole(_) => false,
            Self::Progressive(data) => frame < data.frames() && frame >= data.ready(),
        }
    }
}

/// One layer and the audio it plays. A layer whose media the library can't
/// supply plays silence.
#[derive(Clone, Debug)]
pub struct LayerAudio {
    /// The layer as the document has it: zones, markers (in frames of the
    /// sample at its own rate) and settings.
    pub layer: Layer,
    /// Its audio, if the library had it.
    pub sample: Option<LayerSample>,
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
    /// How many frames voices read before the disk had delivered them (R23),
    /// counted by the node and read by whoever shows the disk's load.
    pub underruns: Arc<AtomicU64>,
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
            underruns: Arc::default(),
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
