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
//! | [`GAIN`] | multiplies every sample by its gain (WP-2.4) | the gain, 1 by default |
//! | [`THROUGH`] | passes its input through untouched (WP-2.4) | none |
//! | [`KEY`] | multiplies its input by what its detector hears, while its sidechain is on (WP-2.19b) | none |
//! | [`DELAY`] | delays every sample by its frames, and reports that latency (WP-2.11) | the frames |
//! | [`SPIN`] | passes its input through after busy-waiting, and notes which thread ran it (WP-5.37) | the microseconds to wait |
//!
//! [`GAIN`] and [`THROUGH`] are offered as device types ([`gain_type`],
//! [`through_type`]), audio effects named "Test Gain" and "Test
//! Pass-Through", so tests of the device chain and the Device View have
//! devices to insert; Beatlayer ships no device of its own until WP-2.5.
//! [`KEY`] is offered too ([`key_type`], "Test Sidechain"), an audio
//! effect with a sidechain input, for the sidechain's tests.
//!
//! All of them count themselves in a shared [`Probe`] when they're made and
//! dropped, so a test can tell a node that kept running from one made anew.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use bl_model::{Device, DeviceRole, ParamId};

use crate::node::{Block, Node, NodeConfig, NodeData, NodeEvent, NoteEvent, ParamValue, segments};
use crate::registry::{DeviceType, Registry};

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
/// The gain node's device type.
pub const GAIN: &str = "test.gain";
/// The pass-through node's device type.
pub const THROUGH: &str = "test.through";
/// The sidechain probe's device type.
pub const KEY: &str = "test.key";
/// The test instrument device type (WP-2.1).
pub const INSTRUMENT: &str = "test.instrument";
/// The delay node's device type (WP-2.11).
pub const DELAY: &str = "test.delay";
/// The transposing MIDI effect's device type (WP-5.1).
pub const TRANSPOSE: &str = "test.transpose";
/// The busy-waiting node's device type (WP-5.37).
pub const SPIN: &str = "test.spin";
/// The busy-waiting node's wait, in microseconds.
pub const SPIN_MICROS: ParamId = ParamId(0);
/// The transposing MIDI effect's shift, in semitones.
pub const SEMITONES: ParamId = ParamId(0);
/// The DC node's value.
pub const VALUE: ParamId = ParamId(0);
/// The gain node's gain, linear.
pub const GAIN_PARAM: ParamId = ParamId(0);
/// The delay node's delay, in frames.
pub const DELAY_FRAMES: ParamId = ParamId(0);

/// The test instrument as a device type: an instrument named "Test Instrument".
#[must_use]
pub fn instrument_type() -> DeviceType {
    DeviceType::native(INSTRUMENT, "Test Instrument", DeviceRole::Instrument)
}

/// The test gain as a device type: an audio effect named "Test Gain".
#[must_use]
pub fn gain_type() -> DeviceType {
    DeviceType::native(GAIN, "Test Gain", DeviceRole::AudioEffect)
}

/// The pass-through as a device type: an audio effect named "Test
/// Pass-Through".
#[must_use]
pub fn through_type() -> DeviceType {
    DeviceType::native(THROUGH, "Test Pass-Through", DeviceRole::AudioEffect)
}

/// The sidechain probe as a device type: an audio effect named "Test
/// Sidechain", with a sidechain input.
#[must_use]
pub fn key_type() -> DeviceType {
    DeviceType::native(KEY, "Test Sidechain", DeviceRole::AudioEffect).with_sidechain()
}

/// A note event recorded by the test instrument with its song frame position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedNote {
    pub frame: i64,
    pub event: NoteEvent,
}

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
    /// The consumers of recorded notes, one per test instrument made, in
    /// that order.
    pub note_consumers: std::sync::Mutex<Vec<rtrb::Consumer<RecordedNote>>>,
    /// The threads that have run a [`SPIN`] node, by `bl-rt`'s thread number.
    spun_on: [AtomicU64; 16],
}

impl Probe {
    /// How many different threads have run a [`SPIN`] node.
    #[must_use]
    pub fn threads_spun_on(&self) -> usize {
        self.spun_on
            .iter()
            .filter(|thread| thread.load(Relaxed) != 0)
            .count()
    }

    fn spun(&self) {
        let me = bl_rt::thread::serial();
        for thread in &self.spun_on {
            match thread.compare_exchange(0, me, Relaxed, Relaxed) {
                Ok(_) => return,
                Err(was) if was == me => return,
                Err(_) => {}
            }
        }
    }

    /// Drains all recorded notes, from every test instrument in the order
    /// they were made: a song with one instrument reads its notes in the
    /// order it played them.
    #[must_use]
    pub fn recorded_notes(&self) -> Vec<RecordedNote> {
        let mut notes = Vec::new();
        for consumer in self.note_consumers.lock().unwrap().iter_mut() {
            while let Ok(note) = consumer.pop() {
                notes.push(note);
            }
        }
        notes
    }
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
    let p = Arc::clone(probe);
    registry.add_device(gain_type(), move |device, _| {
        Box::new(Gain::new(device, &p))
    });
    let p = Arc::clone(probe);
    registry.add_device(through_type(), move |_, _| {
        Box::new(Through {
            _tracked: Tracked::new(&p),
        })
    });
    let p = Arc::clone(probe);
    registry.add_device(key_type(), move |_, _| {
        Box::new(Key {
            _tracked: Tracked::new(&p),
        })
    });
    let p = Arc::clone(probe);
    registry.add_device(instrument_type(), move |_, _| {
        let (producer, consumer) = rtrb::RingBuffer::new(4096);
        p.note_consumers.lock().unwrap().push(consumer);
        Box::new(TestInstrument {
            producer,
            _tracked: Tracked::new(&p),
        })
    });
    let p = Arc::clone(probe);
    registry.add_native(TRANSPOSE, move |device, _| {
        Box::new(Transpose::new(device, &p))
    });
    let p = Arc::clone(probe);
    registry.add_native(SPIN, move |device, _| Box::new(Spin::new(device, &p)));
    let p = Arc::clone(probe);
    registry.add_native(DELAY, move |device, config| {
        Box::new(DelayNode::new(device, config, &p))
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

/// Multiplies every sample by its gain.
#[derive(Debug)]
struct Gain {
    gain: f32,
    _tracked: Tracked,
}

impl Gain {
    fn new(device: &Device, probe: &Arc<Probe>) -> Self {
        Self {
            gain: device.params.get(&GAIN_PARAM).copied().unwrap_or(1.0) as f32,
            _tracked: Tracked::new(probe),
        }
    }

    fn set(&mut self, event: &NodeEvent) {
        if event.param == GAIN_PARAM {
            self.gain = match event.value {
                ParamValue::Set(value) => value as f32,
                ParamValue::Default => 1.0,
            };
        }
    }
}

impl Node for Gain {
    fn process(&mut self, block: &mut Block<'_>) {
        for segment in segments(block.events(), block.frames()) {
            for event in segment.events {
                self.set(event);
            }
            for channel in block.channels_mut() {
                for sample in &mut channel[segment.range.clone()] {
                    *sample *= self.gain;
                }
            }
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event);
        }
    }
}

/// Passes its input through after busy-waiting for a while each block, so
/// that work running on several threads overlaps, and notes the thread it
/// ran on in its probe.
#[derive(Debug)]
struct Spin {
    micros: u64,
    probe: Arc<Probe>,
    _tracked: Tracked,
}

impl Spin {
    fn new(device: &Device, probe: &Arc<Probe>) -> Self {
        Self {
            micros: device.params.get(&SPIN_MICROS).copied().unwrap_or(0.0) as u64,
            probe: Arc::clone(probe),
            _tracked: Tracked::new(probe),
        }
    }
}

impl Node for Spin {
    fn process(&mut self, _block: &mut Block<'_>) {
        self.probe.spun();
        let until = std::time::Instant::now() + std::time::Duration::from_micros(self.micros);
        while std::time::Instant::now() < until {
            std::hint::spin_loop();
        }
    }
}

/// Passes its input through.
#[derive(Debug)]
struct Through {
    _tracked: Tracked,
}

impl Node for Through {
    fn process(&mut self, _block: &mut Block<'_>) {}
}

/// Multiplies its input by what its detector hears while its sidechain is
/// on, as a gate keyed by its sidechain; passes its input through while
/// it's off.
#[derive(Debug)]
struct Key {
    _tracked: Tracked,
}

impl Node for Key {
    fn process(&mut self, block: &mut Block<'_>) {
        let Some(heard) = block.sidechain else {
            return;
        };
        let (frames, stride) = (block.frames, block.stride);
        for c in 0..block.channels {
            let at = c * stride..c * stride + frames;
            for (sample, key) in block.data[at.clone()].iter_mut().zip(&heard[at]) {
                *sample *= key;
            }
        }
    }
}

/// Records all note events it receives with their song frames.
struct TestInstrument {
    producer: rtrb::Producer<RecordedNote>,
    _tracked: Tracked,
}

impl Node for TestInstrument {
    fn process(&mut self, block: &mut Block<'_>) {
        let pos = block.transport().position.get();
        for &event in block.notes() {
            let _ = self.producer.push(RecordedNote {
                frame: pos + event.offset as i64,
                event,
            });
        }
    }
}

/// A MIDI effect that sends every note on shifted by its semitones (WP-5.1):
/// racks of MIDI effects have something to merge.
#[derive(Debug)]
struct Transpose {
    semitones: i32,
    _tracked: Tracked,
}

impl Transpose {
    fn new(device: &Device, probe: &Arc<Probe>) -> Self {
        Self {
            semitones: device.params.get(&SEMITONES).copied().unwrap_or(0.0) as i32,
            _tracked: Tracked::new(probe),
        }
    }
}

impl Node for Transpose {
    fn process(&mut self, block: &mut Block<'_>) {
        for &note in block.notes() {
            let key = (i32::from(note.key) + self.semitones).clamp(0, 127) as u8;
            block.send_note(NoteEvent { key, ..note });
        }
    }
}

/// Delays every sample by its number of frames, and reports that latency, so
/// delay compensation has a device with a known delay to test with (WP-2.11).
#[derive(Debug)]
struct DelayNode {
    frames: usize,
    write: usize,
    /// One ring of `frames` samples per channel.
    samples: Vec<f32>,
    channels: usize,
    _tracked: Tracked,
}

impl DelayNode {
    fn new(device: &Device, config: &NodeConfig, probe: &Arc<Probe>) -> Self {
        let frames = device
            .params
            .get(&DELAY_FRAMES)
            .copied()
            .unwrap_or(0.0)
            .max(0.0) as usize;
        Self {
            frames,
            write: 0,
            samples: vec![0.0; config.channels * frames],
            channels: config.channels,
            _tracked: Tracked::new(probe),
        }
    }
}

impl Node for DelayNode {
    fn process(&mut self, block: &mut Block<'_>) {
        if self.frames == 0 {
            return;
        }
        let channels = block.channels().min(self.channels);
        for c in 0..channels {
            let base = c * self.frames;
            let mut w = self.write;
            for sample in block.channel_mut(c) {
                let at = base + w;
                std::mem::swap(&mut self.samples[at], sample);
                w += 1;
                if w == self.frames {
                    w = 0;
                }
            }
            if c + 1 == channels {
                self.write = w;
            }
        }
    }

    fn latency(&self) -> u32 {
        self.frames as u32
    }
}
