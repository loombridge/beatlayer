//! Nodes for tests and diagnostics, and a registry that makes them. Not for
//! songs.
//!
//! Each is a native device type in [`registry`]:
//!
//! | Type | Node | Parameter 0 |
//! |---|---|---|
//! | [`DC`] | adds a constant to every sample | the constant |
//! | [`COUNTER`] | counts the frames it processes and publishes the count | none |
//! | [`TRANSPORT`] | adds 1 to every sample while the song plays | none |
//! | [`POSITION`] | adds each frame's song position while the song plays, and −1 while it's stopped | none |
//! | [`DATA`] | adds the `f32` it was last sent as [`NodeData`] | none |
//!
//! All of them count themselves in a shared [`Probe`] when they're made and
//! dropped, so a test can tell a node that kept running from one made anew.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use bl_model::{Device, ParamId};

use crate::node::{Block, Node, NodeData, NodeEvent, ParamValue, segments};
use crate::registry::Registry;

/// The DC node's device type.
pub const DC: &str = "test.dc";
/// The counter's device type.
pub const COUNTER: &str = "test.counter";
/// The transport probe's device type.
pub const TRANSPORT: &str = "test.transport";
/// The position probe's device type.
pub const POSITION: &str = "test.position";
/// The data probe's device type.
pub const DATA: &str = "test.data";
/// The DC node's value.
pub const VALUE: ParamId = ParamId(0);

/// What the test nodes report, shared with the test.
#[derive(Debug, Default)]
pub struct Probe {
    /// Test nodes made.
    pub made: AtomicU64,
    /// Test nodes dropped.
    pub dropped: AtomicU64,
    /// The latest counter's count of frames.
    pub counted: AtomicU64,
    /// Which counter published `counted`: 1 for the first one made, and so
    /// on.
    pub counter: AtomicU64,
}

/// The engine's own devices and the test nodes, reporting to `probe`.
#[must_use]
pub fn registry(probe: &Arc<Probe>) -> Registry {
    let mut registry = Registry::new();
    let p = Arc::clone(probe);
    registry.add_native(DC, move |device, _| Box::new(Dc::new(device, &p)));
    let p = Arc::clone(probe);
    registry.add_native(COUNTER, move |_, _| Box::new(Counter::new(&p)));
    let p = Arc::clone(probe);
    registry.add_native(TRANSPORT, move |_, _| {
        Box::new(TransportProbe {
            _tracked: Tracked::new(&p),
        })
    });
    let p = Arc::clone(probe);
    registry.add_native(POSITION, move |_, _| {
        Box::new(PositionProbe {
            _tracked: Tracked::new(&p),
        })
    });
    let p = Arc::clone(probe);
    registry.add_native(DATA, move |_, _| {
        Box::new(DataProbe {
            data: None,
            _tracked: Tracked::new(&p),
        })
    });
    registry
}

/// Counts a node in its probe when made and when dropped.
#[derive(Debug)]
struct Tracked {
    probe: Arc<Probe>,
    serial: u64,
}

impl Tracked {
    fn new(probe: &Arc<Probe>) -> Self {
        let serial = probe.made.fetch_add(1, Relaxed) + 1;
        Self {
            probe: Arc::clone(probe),
            serial,
        }
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.probe.dropped.fetch_add(1, Relaxed);
    }
}

fn add(block: &mut Block<'_>, range: std::ops::Range<usize>, value: f32) {
    for channel in block.channels_mut() {
        for sample in &mut channel[range.clone()] {
            *sample += value;
        }
    }
}

/// Adds its value to every sample.
#[derive(Debug)]
struct Dc {
    value: f32,
    _tracked: Tracked,
}

impl Dc {
    fn new(device: &Device, probe: &Arc<Probe>) -> Self {
        Self {
            value: device.params.get(&VALUE).copied().unwrap_or(0.0) as f32,
            _tracked: Tracked::new(probe),
        }
    }

    fn set(&mut self, event: &NodeEvent) {
        if event.param == VALUE {
            self.value = match event.value {
                ParamValue::Set(value) => value as f32,
                ParamValue::Default => 0.0,
            };
        }
    }
}

impl Node for Dc {
    fn process(&mut self, block: &mut Block<'_>) {
        for segment in segments(block.events(), block.frames()) {
            for event in segment.events {
                self.set(event);
            }
            add(block, segment.range, self.value);
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event);
        }
    }
}

/// Counts the frames it processes.
#[derive(Debug)]
struct Counter {
    frames: u64,
    tracked: Tracked,
}

impl Counter {
    fn new(probe: &Arc<Probe>) -> Self {
        Self {
            frames: 0,
            tracked: Tracked::new(probe),
        }
    }
}

impl Node for Counter {
    fn process(&mut self, block: &mut Block<'_>) {
        self.frames += block.frames() as u64;
        self.tracked.probe.counted.store(self.frames, Relaxed);
        self.tracked
            .probe
            .counter
            .store(self.tracked.serial, Relaxed);
    }
}

/// Adds 1 while the song plays.
#[derive(Debug)]
struct TransportProbe {
    _tracked: Tracked,
}

impl Node for TransportProbe {
    fn process(&mut self, block: &mut Block<'_>) {
        if block.transport().playing {
            let frames = block.frames();
            add(block, 0..frames, 1.0);
        }
    }
}

/// Adds each frame's song position while the song plays, and −1 while it's
/// stopped. Positions are exact as `f32`s up to 2²⁴ frames (about 350
/// seconds at 48 kHz).
#[derive(Debug)]
struct PositionProbe {
    _tracked: Tracked,
}

impl Node for PositionProbe {
    fn process(&mut self, block: &mut Block<'_>) {
        let transport = block.transport();
        for channel in block.channels_mut() {
            for (i, sample) in channel.iter_mut().enumerate() {
                *sample += if transport.playing {
                    (transport.position.get() + i as i64) as f32
                } else {
                    -1.0
                };
            }
        }
    }
}

/// Adds the `f32` it was last sent.
#[derive(Debug)]
struct DataProbe {
    data: Option<NodeData>,
    _tracked: Tracked,
}

impl Node for DataProbe {
    fn process(&mut self, block: &mut Block<'_>) {
        let value = self
            .data
            .as_ref()
            .and_then(|data| data.downcast_ref::<f32>())
            .copied()
            .unwrap_or(0.0);
        let frames = block.frames();
        add(block, 0..frames, value);
    }

    fn receive_data(&mut self, data: NodeData) {
        // The old data is dropped here, on the audio thread; the drop thread
        // frees it.
        self.data = Some(data);
    }
}
