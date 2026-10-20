//! The native device framework (WP-2.5): a device's parameters on the audio
//! thread, clamped and smoothed, and the node that runs a device in the
//! engine.

use core::ops::Range;

use bl_core::time::{Frames, SampleRate, TempoMap, TimeSignatureMap};
use bl_dsp::LinearSmoother;
use bl_dsp::smooth::DEFAULT_SMOOTHING_SECONDS;
use bl_engine::modbus::{Bus, Control};
use bl_engine::{
    AutomationList, Block, DeviceType, DisplayReader, DisplayWriter, Node, NodeConfig, NodeData,
    NodeEvent, NoteBuffer, NoteEvent, ParamValue, Registry, SessionNow, display_channel, segments,
};
use bl_model::modulation::Travel;
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

    /// Ends every note the device started and still holds, by writing a
    /// note-off for each at offset 0 into `out`, and forgets them. The engine
    /// calls it while the device is switched off, and on the first block after
    /// the device was removed or moved (spec: device-framework R32). The
    /// default writes nothing, for a device that starts no notes.
    fn release(&mut self, out: &mut NoteBuffer) {
        let _ = out;
    }

    /// How many notes the device has dropped since it was made, for lack of
    /// room in its fixed tables and queues (real-time rule 4). The default is
    /// none.
    fn dropped_notes(&self) -> u64 {
        0
    }

    /// Takes parameter changes while the device is switched off and
    /// doesn't process; `params` already holds them. A device that keeps
    /// its own copy of its values (a kit plug-in's processor) applies them
    /// here. The default does nothing.
    fn flush(&mut self, events: &[NodeEvent], params: &ParamBank) {
        let _ = (events, params);
    }

    /// Takes data the compiler made for the device off the audio thread:
    /// the decoded audio of a device that plays media (a sampler's sample,
    /// WP-2.16). The default drops it. Dropping a [`NodeData`] here is
    /// real-time safe: it only lets go of it, and the engine's drop thread
    /// frees what it held.
    fn receive_data(&mut self, data: NodeData) {
        drop(data);
    }

    /// How many frames it delays its input by, for delay compensation
    /// (WP-2.11).
    fn latency(&self) -> u32 {
        0
    }

    /// Whether the device is a modulator (WP-5.10): the engine works its
    /// output out in [`control`](Self::control) before any track runs, and
    /// [`process`](Self::process) is never called.
    fn is_modulator(&self) -> bool {
        false
    }

    /// A modulator's output for a stretch between parameter changes: it
    /// writes the rows [`Modulation`] gives, from its parameters, which
    /// are already worked out for the stretch.
    fn control(&mut self, modulation: &mut Modulation<'_, '_>, params: &ParamBank) {
        let _ = (modulation, params);
    }

    /// Whether the device is a modulator driven by the audio, notes or MIDI
    /// it hears at its place in the chain (WP-5.11): it writes its rows in
    /// [`process`](Self::process), with [`Audio::row_mut`], while its track
    /// runs, and in [`lag`](Self::lag) the rows of what it wrote 32 frames
    /// before. Unlike [`is_modulator`](Self::is_modulator), `process` is
    /// called.
    fn is_signal_modulator(&self) -> bool {
        false
    }

    /// A signal modulator's rows of what it wrote 32 frames before the
    /// part's frames, before the part runs: what a target that runs before
    /// the modulator in its own chain reads (the modulation spec, R28). The
    /// part is no longer than 32 frames.
    fn lag(&mut self, modulation: &mut Modulation<'_, '_>) {
        let _ = modulation;
    }

    /// How many values the device's display data has (the framework spec,
    /// R35): 0 for a device with no display to draw from the audio. Asked
    /// once, when the node is made.
    fn display_len(&self) -> usize {
        0
    }

    /// Writes the display's latest values into `out`, [`display_len`] of
    /// them, after each block. It runs on the audio thread: it allocates,
    /// locks and blocks nothing.
    ///
    /// [`display_len`]: Self::display_len
    fn display(&mut self, out: &mut [f32]) {
        let _ = out;
    }
}

/// A stretch of the rows a modulator writes, between parameter changes
/// (WP-5.10).
pub struct Modulation<'c, 'a> {
    ctl: &'c mut Control<'a>,
    range: Range<usize>,
}

impl Modulation<'_, '_> {
    /// Frames in the stretch.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.range.len()
    }

    /// The sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> SampleRate {
        self.ctl.sample_rate
    }

    /// The song's tempo map.
    #[must_use]
    pub fn tempo(&self) -> &TempoMap {
        self.ctl.tempo
    }

    /// The song position of the stretch's first frame.
    #[must_use]
    pub fn song_position(&self) -> Frames {
        Frames(self.ctl.transport.position.get() + self.range.start as i64)
    }

    /// Whether the song is playing.
    #[must_use]
    pub fn playing(&self) -> bool {
        self.ctl.transport.playing
    }

    /// Row `index` of the modulator's rows, over the stretch, writable
    /// ([`bl_engine::modbus::ROWS`] of them, a contribution row and a hold
    /// row per slot).
    ///
    /// # Panics
    ///
    /// If `index` isn't a row.
    pub fn row_mut(&mut self, index: usize) -> &mut [f32] {
        let range = self.range.clone();
        &mut self.ctl.row_mut(index)[range]
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

    /// What an external device brings back from the audio interface over
    /// the stretch on `channel` (WP-5.34), or `None` for any other device.
    ///
    /// # Panics
    ///
    /// If `channel` isn't less than [`channels`](Self::channels).
    #[must_use]
    pub fn external(&self, channel: usize) -> Option<&'a [f32]> {
        let range = self.range.clone();
        self.block.external(channel).map(|data| &data[range])
    }

    /// Sends a note on from the stretch, at `note.offset` frames after its
    /// first frame (clamped into it): the notes go out in order of offset,
    /// note-offs before note-ons on one frame (spec: device-framework R30).
    /// A note past the out buffer's room is dropped and counted. Never
    /// allocates.
    pub fn send(&mut self, note: NoteEvent) {
        let offset = note.offset.min(self.range.len().saturating_sub(1) as u32);
        self.block.insert_note(NoteEvent {
            offset: offset + self.range.start as u32,
            ..note
        });
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

    /// The song's time-signature map, which places bar lines on ticks: the
    /// launch quantisation grid is counted on it.
    #[must_use]
    pub fn signatures(&self) -> &'a TimeSignatureMap {
        self.block.signatures()
    }

    /// Whether the song is playing, so a tempo-synced device locks to the
    /// song position.
    #[must_use]
    pub fn playing(&self) -> bool {
        self.block.transport().playing
    }

    /// Row `index` of the rows a signal modulator writes ([`bl_engine::modbus::ROWS`] of them,
    /// a contribution row and a hold row per slot), over the stretch; empty
    /// for a block that has no rows to write.
    pub fn row_mut(&mut self, index: usize) -> &mut [f32] {
        let range = self.range.clone();
        self.block.row_mut(index).get_mut(range).unwrap_or_default()
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

    /// What the device's detector hears over the stretch on `channel`, if
    /// the device has a sidechain input and its sidechain is on (WP-2.19b):
    /// the source track's signal through the sidechain's gain and mix, with
    /// the device's own input, or `None` while it's off: then the detector
    /// hears the stretch's own audio ([`Self::channel_mut`]).
    ///
    /// # Panics
    ///
    /// If `channel` isn't less than [`channels`](Self::channels).
    #[must_use]
    pub fn sidechain(&self, channel: usize) -> Option<&[f32]> {
        let range = self.range.clone();
        self.block.sidechain(channel).map(|data| &data[range])
    }
}

/// One parameter's state on the audio thread.
#[derive(Clone, Debug)]
struct Slot {
    descriptor: ParamDescriptor,
    smoother: LinearSmoother,
    /// Its value at each frame of the current stretch.
    ramp: Vec<f32>,
    /// Its base and the value it played at the stretch's last frame, while
    /// a clip's modulation acts on it.
    modulated: Option<(f64, f64)>,
}

/// A device's parameters on the audio thread: each clamped to its range,
/// stepped ones changing at once, and continuous ones and gains gliding to
/// each new value over 20 ms, frame by frame. Its buffers are made with it,
/// so nothing it does on the audio thread allocates.
#[derive(Clone, Debug)]
pub struct ParamBank {
    slots: Vec<Slot>,
    frames: usize,
    /// The envelopes driving the device's parameters while the song plays
    /// (WP-2.12).
    automation: Option<NodeData>,
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
                    modulated: None,
                }
            })
            .collect();
        Self {
            slots,
            frames: 0,
            automation: None,
        }
    }

    /// Takes the envelopes driving the device's parameters (WP-2.12), if
    /// that's what the data holds; anything else is let go of.
    pub fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<AutomationList>().is_some() {
            self.automation = Some(data);
        }
    }

    /// Where parameter `id` is in the bank: its place in the description.
    #[must_use]
    pub fn index(&self, id: ParamId) -> Option<usize> {
        // Most devices list their parameters in the order of their IDs, so
        // the place named by the ID is usually the one.
        let guess = id.0 as usize;
        if self.slots.get(guess).is_some_and(|s| s.descriptor.id == id) {
            return Some(guess);
        }
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

    /// Works out every parameter's value for the next `frames` frames: an
    /// automated one from its envelope, on the exact frame, when the song
    /// plays at song frame `from`, or from the automation envelope of the
    /// Session clip playing on the track (WP-3.12); a held one at the
    /// position a modulator in Remote Control holds it at; then the clips'
    /// and the modulators' modulation on top of that base (the modulation
    /// spec, R2, R3 and R14), clamped to the parameter's range; the rest
    /// gliding toward their values as always. `bus` holds the modulators'
    /// rows, whose frame `offset` is the stretch's first. `block` says which
    /// Session clip plays; a modulator's own parameters, worked out without
    /// one, follow no Session clip.
    fn advance(
        &mut self,
        frames: usize,
        from: Option<i64>,
        bus: Bus<'_>,
        offset: usize,
        block: Option<&Block<'_>>,
    ) {
        self.frames = frames;
        let automation = self.automation.clone();
        let automation = automation
            .as_ref()
            .and_then(|data| data.downcast_ref::<AutomationList>());
        for slot in &mut self.slots {
            let frames = frames.min(slot.ramp.len());
            let id = slot.descriptor.id;
            let descriptor = &slot.descriptor;
            slot.modulated = None;
            let Some(list) = automation else {
                Self::glide(slot, frames);
                continue;
            };
            let driven = from.is_some() && list.drives(id);
            let modulates = list.modulates(id);
            if !driven && !modulates {
                Self::glide(slot, frames);
                continue;
            }
            let now = block.map_or(SessionNow::NONE, |block| block.session_now(list));
            // The base is a modulator's hold, else the value an envelope
            // sets on each frame, else the control's, which glides as it
            // always does; the modulation acts on it on the frame, without a
            // glide of its own, and never changes it: when it goes, the
            // parameter plays its base again.
            let travel = Travel::Param(descriptor);
            let mut base = 0.0;
            let mut played = 0.0;
            let mut held = false;
            let mut set = false;
            for (i, value) in slot.ramp[..frames].iter_mut().enumerate() {
                let song = from.map(|from| from + i as i64);
                let at = offset + i;
                let driven = song.and_then(|frame| list.driven(id, frame, &now));
                set = driven.is_some();
                let control = if set { 0.0 } else { slot.smoother.step() };
                let position = list.held(id, bus, at);
                held = position.is_some();
                base = match (position, driven) {
                    (Some(position), _) => travel.value(position),
                    (None, Some(value)) => value,
                    _ => control,
                };
                played = descriptor.clamp(list.played(travel, id, song, base, bus, at));
                *value = played as f32;
            }
            if frames == 0 {
                continue;
            }
            // A parameter an envelope set in the block's last frame leaves
            // its control's glide at that value.
            if set && !held {
                slot.smoother.snap(descriptor.clamp(base));
            }
            slot.modulated =
                (modulates && (held || played != descriptor.clamp(base))).then_some((base, played));
        }
    }

    /// Fills the next `frames` frames of `slot`'s ramp from its smoother:
    /// the control's gliding value.
    fn glide(slot: &mut Slot, frames: usize) {
        if slot.smoother.is_settled() {
            slot.ramp[..frames].fill(slot.smoother.value() as f32);
        } else {
            for value in &mut slot.ramp[..frames] {
                *value = slot.smoother.step() as f32;
            }
        }
    }

    /// Calls `report` with the id, base and played value of each parameter
    /// a clip's modulation acted on in the last stretch (WP-5.9).
    pub fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        for slot in &self.slots {
            if let Some((base, played)) = slot.modulated {
                report(slot.descriptor.id, base, played);
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

/// A delay of a stretch of samples, read a whole number of samples back.
/// [`read`](Self::read) reads what was written before the current sample,
/// so a delay of 0 isn't representable: a device that offers one passes
/// its input through instead.
pub(crate) struct DelayLine {
    samples: Vec<f32>,
    at: usize,
}

impl DelayLine {
    pub(crate) fn new(max_delay: usize) -> Self {
        Self {
            samples: vec![0.0; max_delay + 1],
            at: 0,
        }
    }

    /// The sample `delay` writes ago.
    pub(crate) fn read(&self, delay: usize) -> f32 {
        let len = self.samples.len();
        self.samples[(self.at + len - delay % len) % len]
    }

    /// Forgets everything written.
    pub(crate) fn clear(&mut self) {
        self.samples.fill(0.0);
    }

    pub(crate) fn write(&mut self, sample: f32) {
        let len = self.samples.len();
        self.samples[self.at] = sample;
        self.at = (self.at + 1) % len;
    }
}

/// A native device in the engine: it splits each block at its parameter
/// changes, so each lands on its exact frame (sample-accurate automation,
/// rule G2), and hands the device each stretch with its parameters.
pub struct NativeNode<D> {
    device: D,
    params: ParamBank,
    /// Where the device publishes its display after each block, and the
    /// reader the compiler takes once.
    display: Option<(DisplayWriter, Option<DisplayReader>)>,
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
        let len = device.display_len();
        let display = (len > 0).then(|| {
            let (writer, reader) = display_channel(len);
            (writer, Some(reader))
        });
        Self {
            device,
            params: ParamBank::new(description, values, config),
            display,
        }
    }

    /// Its parameters.
    #[must_use]
    pub fn params(&self) -> &ParamBank {
        &self.params
    }

    /// How many notes the device has dropped for lack of room.
    #[must_use]
    pub fn dropped_notes(&self) -> u64 {
        self.device.dropped_notes()
    }

    /// The device.
    #[must_use]
    pub fn device(&self) -> &D {
        &self.device
    }
}

impl<D: NativeDevice> Node for NativeNode<D> {
    fn process(&mut self, block: &mut Block<'_>) {
        if self.device.is_modulator() {
            return;
        }
        let frames = block.frames();
        let transport = block.transport();
        let bus = block.modulation();
        for segment in segments(block.events(), frames) {
            for event in segment.events {
                self.params.apply(event);
            }
            // Spec: automation R2. While the song plays, an automated
            // parameter takes its value from its envelope on the exact
            // frame.
            let from = transport
                .playing
                .then(|| transport.position.get() + segment.range.start as i64);
            self.params.advance(
                segment.range.len(),
                from,
                bus,
                segment.range.start,
                Some(block),
            );
            let last = segment.range.end >= frames;
            let mut audio = Audio {
                block,
                range: segment.range,
                changes: segment.events,
                last,
            };
            self.device.process(&mut audio, &self.params);
        }
        // Spec: device-framework R35. The display's latest values, after
        // each block.
        if let Some((writer, _)) = &mut self.display {
            writer.publish(|out| self.device.display(out));
        }
    }

    fn control(&mut self, ctl: &mut Control<'_>) -> bool {
        if self.device.is_signal_modulator() {
            let range = 0..ctl.frames;
            self.device.lag(&mut Modulation { ctl, range });
            return true;
        }
        if !self.device.is_modulator() {
            return false;
        }
        let transport = ctl.transport;
        let (events, frames, earlier) = (ctl.events, ctl.frames, ctl.earlier);
        for segment in segments(events, frames) {
            for event in segment.events {
                self.params.apply(event);
            }
            let from = transport
                .playing
                .then(|| transport.position.get() + segment.range.start as i64);
            self.params.advance(
                segment.range.len(),
                from,
                earlier,
                segment.range.start,
                None,
            );
            let mut modulation = Modulation {
                ctl: &mut *ctl,
                range: segment.range,
            };
            self.device.control(&mut modulation, &self.params);
        }
        true
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.params.apply_now(event);
        }
        self.device.flush(events, &self.params);
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        self.device.release(out);
    }

    fn receive_data(&mut self, data: NodeData) {
        // Automation (WP-2.12) goes to the parameter bank; anything else is
        // the device's own data (WP-2.16).
        if data.downcast_ref::<AutomationList>().is_some() {
            self.params.receive_data(data);
        } else {
            self.device.receive_data(data);
        }
    }

    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        self.params.modulated(report);
    }

    fn latency(&self) -> u32 {
        self.device.latency()
    }

    fn display(&mut self) -> Option<DisplayReader> {
        self.display.as_mut().and_then(|(_, reader)| reader.take())
    }
}

/// Registers a native device type with the engine: the browser offers it,
/// and each device of its kind runs as a [`NativeNode`] around the device
/// `make` makes.
///
/// `make` sees the device's parameter values, so a device whose latency
/// they set (a look-ahead, say) reports it from the moment it's built, as
/// the compiler asks for delay compensation (WP-2.11).
///
/// # Panics
///
/// If `device_type` isn't native, or its description breaks a rule
/// ([`DeviceDescription::check`]): among them, a parameter that can't be
/// automated (G2). A device that breaks one never reaches a set.
pub fn add<D, F>(registry: &mut Registry, device_type: DeviceType, make: F)
where
    D: NativeDevice,
    F: Fn(&Device, &NodeConfig) -> D + Send + Sync + 'static,
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
        Box::new(NativeNode::new(
            make(device, config),
            &description,
            device,
            config,
        ))
    });
}
