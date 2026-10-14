//! The native device framework (WP-2.5): a device's parameters on the audio
//! thread, clamped and smoothed, and the node that runs a device in the
//! engine.

use core::ops::Range;

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_dsp::LinearSmoother;
use bl_dsp::smooth::DEFAULT_SMOOTHING_SECONDS;
use bl_engine::{
    Block, DeviceType, Node, NodeConfig, NodeEvent, NoteEvent, ParamValue, Registry, segments,
};
use bl_model::{Device, DeviceDescription, DeviceKind, ParamDescriptor, ParamId};

/// A native device: what it does to audio, given its parameters.
///
/// It's made off the audio thread, where it may allocate (an
/// [`Oversampler`](bl_dsp::Oversampler), delay lines), and from then on
/// runs on the audio thread under the real-time rules: no allocation, no
/// lock, no blocking call.
pub trait NativeDevice: Send + 'static {
    /// Processes a stretch of a block in place. Between calls a parameter
    /// may change; within one, `params` gives every parameter's value frame
    /// by frame ([`ParamBank::ramp`]).
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank);

    /// Takes parameter changes while the device is switched off and
    /// doesn't process; `params` already holds them. A device that keeps
    /// its own copy of its values (a kit plug-in's processor) applies them
    /// here. The default does nothing.
    fn flush(&mut self, events: &[NodeEvent], params: &ParamBank) {
        let _ = (events, params);
    }

    /// How many frames it delays its input by, for delay compensation
    /// (WP-2.11).
    fn latency(&self) -> u32 {
        0
    }
}

/// A stretch of a block's audio, between parameter changes.
pub struct Audio<'b, 'a> {
    block: &'b mut Block<'a>,
    range: Range<usize>,
    changes: &'a [NodeEvent],
    last: bool,
}

impl<'a> Audio<'_, 'a> {
    /// The parameter changes that land on the stretch's first frame, which
    /// the [`ParamBank`] has already taken: for a device that applies
    /// changes itself, as a kit plug-in's processor does.
    #[must_use]
    pub fn changes(&self) -> &'a [NodeEvent] {
        self.changes
    }

    /// The notes the device plays in the stretch, in order, each with its
    /// offset from the stretch's first frame.
    pub fn notes(&self) -> impl Iterator<Item = NoteEvent> + 'a {
        let (start, end, last) = (self.range.start, self.range.end, self.last);
        self.block.notes().iter().filter_map(move |note| {
            let at = note.offset as usize;
            (at >= start && (at < end || last)).then(|| NoteEvent {
                offset: u32::try_from(at.min(end.saturating_sub(1)).max(start) - start)
                    .unwrap_or(0),
                ..*note
            })
        })
    }

    /// Every channel's samples in the stretch, in order.
    pub fn channels_mut(&mut self) -> impl Iterator<Item = &mut [f32]> {
        let range = self.range.clone();
        self.block
            .channels_mut()
            .map(move |channel| &mut channel[range.clone()])
    }

    /// Frames in the stretch.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.range.len()
    }

    /// Channels.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.block.channels()
    }

    /// The sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> SampleRate {
        self.block.sample_rate()
    }

    /// The song's tempo map, which places ticks on frames
    /// ([`Block::tempo`]). A tempo-synced device reads the tempo at the
    /// song position of each stretch.
    #[must_use]
    pub fn tempo(&self) -> &'a TempoMap {
        self.block.tempo()
    }

    /// The song position of the stretch's first frame.
    #[must_use]
    pub fn song_position(&self) -> Frames {
        Frames(self.block.transport().position.get() + self.range.start as i64)
    }

    /// One channel's samples in the stretch.
    ///
    /// # Panics
    ///
    /// If `channel` isn't less than [`channels`](Self::channels).
    pub fn channel_mut(&mut self, channel: usize) -> &mut [f32] {
        let range = self.range.clone();
        &mut self.block.channel_mut(channel)[range]
    }
}

/// One parameter's state on the audio thread.
#[derive(Clone, Debug)]
struct Slot {
    descriptor: ParamDescriptor,
    smoother: LinearSmoother,
    /// Its value at each frame of the current stretch.
    ramp: Vec<f32>,
}

/// A device's parameters on the audio thread: each clamped to its range,
/// stepped ones changing at once, and continuous ones and gains gliding to
/// each new value over 20 ms, frame by frame. Its buffers are made with it,
/// so nothing it does on the audio thread allocates.
#[derive(Clone, Debug)]
pub struct ParamBank {
    slots: Vec<Slot>,
    frames: usize,
}

impl ParamBank {
    /// A bank for `description`'s parameters, starting at `device`'s values
    /// (or the defaults), for blocks of up to `config.max_frames` frames.
    #[must_use]
    pub fn new(description: &DeviceDescription, device: &Device, config: &NodeConfig) -> Self {
        let glide =
            LinearSmoother::frames_for(DEFAULT_SMOOTHING_SECONDS, config.sample_rate.as_f64());
        let slots = description
            .params
            .iter()
            .map(|descriptor| {
                let value = description
                    .value(&device.params, descriptor.id)
                    .unwrap_or(descriptor.default);
                let frames = if descriptor.is_stepped() { 0 } else { glide };
                Slot {
                    descriptor: descriptor.clone(),
                    smoother: LinearSmoother::new(value, frames),
                    ramp: vec![value as f32; config.max_frames.max(1)],
                }
            })
            .collect();
        Self { slots, frames: 0 }
    }

    /// Where parameter `id` is in the bank: its place in the description.
    #[must_use]
    pub fn index(&self, id: ParamId) -> Option<usize> {
        self.slots.iter().position(|s| s.descriptor.id == id)
    }

    /// Applies a change: the parameter heads for the new value (or its
    /// default), clamped to its range. A change for a parameter the device
    /// doesn't have is ignored.
    pub fn apply(&mut self, event: &NodeEvent) {
        let Some(index) = self.index(event.param) else {
            return;
        };
        let slot = &mut self.slots[index];
        let value = match event.value {
            ParamValue::Set(value) => slot.descriptor.clamp(value),
            ParamValue::Default => slot.descriptor.default,
        };
        slot.smoother.set_target(value);
    }

    /// Applies a change at once, without gliding, as while the device is
    /// switched off.
    pub fn apply_now(&mut self, event: &NodeEvent) {
        self.apply(event);
        if let Some(index) = self.index(event.param) {
            let slot = &mut self.slots[index];
            let target = slot.smoother.target();
            slot.smoother.snap(target);
        }
    }

    /// Works out every parameter's value for the next `frames` frames.
    fn advance(&mut self, frames: usize) {
        self.frames = frames;
        for slot in &mut self.slots {
            let frames = frames.min(slot.ramp.len());
            if slot.smoother.is_settled() {
                slot.ramp[..frames].fill(slot.smoother.value() as f32);
            } else {
                for value in &mut slot.ramp[..frames] {
                    *value = slot.smoother.step() as f32;
                }
            }
        }
    }

    /// The parameter at `index`'s value at each frame of the stretch being
    /// processed.
    ///
    /// # Panics
    ///
    /// If there's no parameter at `index`.
    #[must_use]
    pub fn ramp(&self, index: usize) -> &[f32] {
        &self.slots[index].ramp[..self.frames]
    }

    /// The parameter at `index`'s value at the end of the stretch (or now,
    /// between stretches): for a stepped parameter, the value throughout.
    ///
    /// # Panics
    ///
    /// If there's no parameter at `index`.
    #[must_use]
    pub fn value(&self, index: usize) -> f64 {
        self.slots[index].smoother.value()
    }

    /// The value the parameter at `index` is heading for.
    ///
    /// # Panics
    ///
    /// If there's no parameter at `index`.
    #[must_use]
    pub fn target(&self, index: usize) -> f64 {
        self.slots[index].smoother.target()
    }

    /// Whether the parameter at `index` is still gliding.
    ///
    /// # Panics
    ///
    /// If there's no parameter at `index`.
    #[must_use]
    pub fn is_moving(&self, index: usize) -> bool {
        !self.slots[index].smoother.is_settled()
    }

    /// How many parameters it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// A native device in the engine: it splits each block at its parameter
/// changes, so each lands on its exact frame (sample-accurate automation,
/// rule G2), and hands the device each stretch with its parameters.
pub struct NativeNode<D> {
    device: D,
    params: ParamBank,
}

impl<D: NativeDevice> NativeNode<D> {
    /// `device`, with its parameters from `description` set to `values`'
    /// (a device in a chain).
    #[must_use]
    pub fn new(
        device: D,
        description: &DeviceDescription,
        values: &Device,
        config: &NodeConfig,
    ) -> Self {
        Self {
            device,
            params: ParamBank::new(description, values, config),
        }
    }

    /// Its parameters.
    #[must_use]
    pub fn params(&self) -> &ParamBank {
        &self.params
    }

    /// The device.
    #[must_use]
    pub fn device(&self) -> &D {
        &self.device
    }
}

impl<D: NativeDevice> Node for NativeNode<D> {
    fn process(&mut self, block: &mut Block<'_>) {
        let frames = block.frames();
        for segment in segments(block.events(), frames) {
            for event in segment.events {
                self.params.apply(event);
            }
            self.params.advance(segment.range.len());
            let last = segment.range.end >= frames;
            let mut audio = Audio {
                block,
                range: segment.range,
                changes: segment.events,
                last,
            };
            self.device.process(&mut audio, &self.params);
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.params.apply_now(event);
        }
        self.device.flush(events, &self.params);
    }

    fn latency(&self) -> u32 {
        self.device.latency()
    }
}

/// Registers a native device type with the engine: the browser offers it,
/// and each device of its kind runs as a [`NativeNode`] around the device
/// `make` makes.
///
/// # Panics
///
/// If `device_type` isn't native, or its description breaks a rule
/// ([`DeviceDescription::check`]): among them, a parameter that can't be
/// automated (G2). A device that breaks one never reaches a set.
pub fn add<D, F>(registry: &mut Registry, device_type: DeviceType, make: F)
where
    D: NativeDevice,
    F: Fn(&NodeConfig) -> D + Send + Sync + 'static,
{
    assert!(
        matches!(device_type.kind, DeviceKind::Native { .. }),
        "{} isn't a native device type",
        device_type.name
    );
    if let Err(problem) = device_type.description.check() {
        panic!("{}: {problem}", device_type.name);
    }
    let description = device_type.description.clone();
    registry.add_device(device_type, move |device, config| {
        Box::new(NativeNode::new(make(config), &description, device, config))
    });
}
