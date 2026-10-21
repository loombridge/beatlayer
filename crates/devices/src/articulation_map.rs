//! Articulation Map, the MIDI effect that turns articulations into
//! keyswitches, controllers and channels (WP-6.47; the articulation map
//! spec): it holds a list of named articulations, and when one becomes
//! current it sends what an instrument needs for it — keyswitch notes,
//! controller values, and the MIDI channel its notes go out on.
//!
//! The list reaches the node as node data, of a fixed size made with the
//! node, so a change to it reaches the playing device without a new program
//! (the spec, R14). Nothing on the audio thread allocates, locks or blocks.

use bl_engine::{Articulations, DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DeviceLayout, DisplayCell, LayoutNode, Widget,
};
use bl_model::{
    ArticulationEntry, ArticulationList, ArticulationOutput, ArticulationTrigger,
    DeviceDescription, DeviceRole, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, value};

/// The Articulation Map's device type.
pub const ARTICULATION_MAP: &str = bl_model::ARTICULATION_MAP;

/// Articulation: which articulation is current, from 1 to the number of
/// articulations.
pub const ARTICULATION: ParamId = ParamId(0);
/// Pass Triggers: lets a trigger's note and message through.
pub const PASS_TRIGGERS: ParamId = ParamId(1);
/// Keyswitch Length: how long a keyswitch note sounds, in milliseconds.
pub const KEYSWITCH_LENGTH: ParamId = ParamId(2);
/// Resend on Start: sends the current articulation's outputs again when the
/// transport starts.
pub const RESEND_ON_START: ParamId = ParamId(3);

/// The name of the display cell the list is drawn in.
pub const LIST_CELL: &str = "Articulations";

/// How many key outputs the device holds sounding at once: the most a list
/// can send, every articulation's four outputs. A keyswitch past the room
/// isn't sent, so it can't be left sounding.
const KEY_ROOM: usize = bl_model::MAX_ARTICULATIONS * bl_model::MAX_OUTPUTS;

/// Where the current articulation's number sits in the display's data.
pub const OFF_CURRENT: usize = 0;
/// Where the count of the events the device has heard sits.
pub const OFF_EVENTS: usize = 1;
/// Where the kind of the last event sits.
pub const OFF_KIND: usize = 2;
/// Where the key or controller of the last event sits.
pub const OFF_KEY: usize = 3;
/// Where the value of the last event sits.
pub const OFF_VALUE: usize = 4;

/// How many values the display's data holds.
pub const DISPLAY_LEN: usize = 5;

/// The last event was a note-on.
pub const EVENT_NOTE: f32 = 1.0;
/// The last event was a control change.
pub const EVENT_CONTROL: f32 = 2.0;

/// Articulation Map's device type.
#[must_use]
pub fn articulation_map_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::integer(ARTICULATION, "Articulation", (1, 32), 1, ParamUnit::None)
            .describe(
                "Which articulation is current, from 1 to the number of articulations. \
                       Its value reads the articulation's name.",
            ),
        ParamDescriptor::toggle(PASS_TRIGGERS, "Pass Triggers", false).describe(
            "On, a trigger's note and message reach the next device as they came; off, they \
             don't.",
        ),
        ParamDescriptor::integer(
            KEYSWITCH_LENGTH,
            "Keyswitch Length",
            (1, 1000),
            20,
            ParamUnit::Milliseconds,
        )
        .describe("How long a keyswitch note sounds, in milliseconds."),
        ParamDescriptor::toggle(RESEND_ON_START, "Resend on Start", true).describe(
            "On, the current articulation's outputs are sent again when the transport starts \
             and when the device is switched on.",
        ),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(DisplayCell::new(LIST_CELL, 260.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (ARTICULATION, Widget::Number),
                (PASS_TRIGGERS, Widget::Toggle),
                (KEYSWITCH_LENGTH, Widget::Number),
                (RESEND_ON_START, Widget::Toggle),
            ],
        )),
    ]));
    DeviceType::native(ARTICULATION_MAP, "Articulation Map", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// Adds Articulation Map to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, articulation_map_type(), |device, _| {
        Map::new(ArticulationList::of(&device.articulations))
    });
}

/// A keyswitch the device has sounding.
#[derive(Clone, Copy, Debug)]
struct KeyOutput {
    /// The channel it went out on.
    channel: u8,
    /// The key it plays.
    key: u8,
    /// The frame its note-off is due at.
    off_at: i64,
}

/// The Articulation Map's audio-thread half.
pub struct Map {
    /// The articulations, as the node holds them.
    list: ArticulationList,
    /// Which articulation is current, from 0.
    current: u8,
    /// The current articulation's own data, so a list that changed can be
    /// followed.
    current_entry: ArticulationEntry,
    /// The notes the device passed on, and the channel each went out on.
    router: Router,
    /// The keyswitches the device has sounding.
    keys: Vec<KeyOutput>,
    /// The Articulation control's value the device last saw.
    last: f64,
    /// Whether the transport was playing the last time the device ran.
    was_playing: bool,
    /// Whether the device was switched off the last time it ran.
    was_off: bool,
    /// The display's data.
    data: Vec<f32>,
    /// How many events the device has heard, and the last one's own data.
    events: u64,
    /// The last event's kind: 0 for none, 1 for a note-on, 2 for a control
    /// change.
    event_kind: f32,
    /// The last event's key or controller.
    event_key: f32,
    /// The last event's value.
    event_value: f32,
}

impl Map {
    /// A device holding `list`.
    fn new(list: ArticulationList) -> Self {
        let current = 0u8;
        let current_entry = list.entry(0).copied().unwrap_or_default();
        Self {
            list,
            current,
            current_entry,
            router: Router::new(),
            keys: Vec::with_capacity(KEY_ROOM),
            last: 1.0,
            was_playing: false,
            was_off: false,
            data: vec![0.0; DISPLAY_LEN],
            events: 0,
            event_kind: 0.0,
            event_key: 0.0,
            event_value: 0.0,
        }
    }

    /// The channel the current articulation's notes go out on, or `None` for
    /// the channel they come in on.
    fn channel(&self) -> Option<u8> {
        match self.current_entry.channel {
            0 => None,
            channel => Some(channel - 1),
        }
    }

    /// The articulation the trigger `key` names, if one does.
    fn trigger_of_key(&self, key: u8) -> Option<usize> {
        (0..self.list.len()).find(|index| {
            self.list
                .entry(*index)
                .is_some_and(|entry| entry.trigger.matches_key(key))
        })
    }

    /// The articulation the control change names, if one does.
    fn trigger_of_control(&self, controller: u8, value: u8) -> Option<usize> {
        (0..self.list.len()).find(|index| {
            self.list
                .entry(*index)
                .is_some_and(|entry| entry.trigger.matches_control(controller, value))
        })
    }

    /// Makes the articulation at `index` current and sends its outputs, as a
    /// trigger does (the spec, R7 and R9).
    fn triggered(&mut self, audio: &mut Audio<'_, '_>, index: usize, at: u32, length: i64) {
        if index != usize::from(self.current) {
            self.current = index as u8;
            self.current_entry = self.list.entry(index).copied().unwrap_or_default();
        }
        // A trigger for the articulation that's already current sends its
        // outputs again.
        self.send_outputs(audio, at, length);
    }

    /// Sends the current articulation's outputs on the frame `at`, in their
    /// order (the spec, R9).
    fn send_outputs(&mut self, audio: &mut Audio<'_, '_>, at: u32, length: i64) {
        let Some(entry) = self.list.entry(usize::from(self.current)).copied() else {
            return;
        };
        // The outputs go out on the articulation's channel, or on channel 1
        // when it has none.
        let channel = entry.channel.saturating_sub(1);
        for index in 0..usize::from(entry.output_count) {
            match entry.outputs[index] {
                ArticulationOutput::Key { key, velocity } => {
                    self.send_key(audio, at, channel, key, velocity, length);
                }
                ArticulationOutput::Controller { controller, value } => {
                    audio.send(NoteEvent {
                        offset: at,
                        kind: NoteKind::Control {
                            controller,
                            value: f32::from(value) / 127.0,
                        },
                        channel,
                        key: 0,
                        velocity: 0.0,
                        note_id: None,
                    });
                }
            }
        }
    }

    /// Sends a keyswitch note, ending the last one on the same key and
    /// channel first if it is still sounding (the spec, R11).
    fn send_key(
        &mut self,
        audio: &mut Audio<'_, '_>,
        at: u32,
        channel: u8,
        key: u8,
        velocity: u8,
        length: i64,
    ) {
        if self.keys.len() >= KEY_ROOM {
            // No room for the note-off: the keyswitch isn't sent, so it
            // can't be left sounding.
            return;
        }
        if let Some(index) = self
            .keys
            .iter()
            .position(|output| output.channel == channel && output.key == key)
        {
            let output = self.keys.swap_remove(index);
            audio.send(NoteEvent {
                offset: at,
                kind: NoteKind::Off,
                channel: output.channel,
                key: output.key,
                velocity: 0.0,
                note_id: None,
            });
        }
        audio.send(NoteEvent {
            offset: at,
            kind: NoteKind::On,
            channel,
            key,
            velocity: f32::from(velocity) / 127.0,
            note_id: None,
        });
        self.keys.push(KeyOutput {
            channel,
            key,
            off_at: self.router.now() + i64::from(at) + length,
        });
    }

    /// Ends the keyswitches that are due before the frame `at` of the
    /// stretch, and sends their note-offs.
    fn end_due_keys(&mut self, audio: &mut Audio<'_, '_>, at: i64) {
        let now = self.router.now();
        let mut index = 0;
        while index < self.keys.len() {
            let output = self.keys[index];
            if output.off_at < now + at {
                self.keys.swap_remove(index);
                audio.send(NoteEvent {
                    offset: (output.off_at - now).max(0) as u32,
                    kind: NoteKind::Off,
                    channel: output.channel,
                    key: output.key,
                    velocity: 0.0,
                    note_id: None,
                });
                continue;
            }
            index += 1;
        }
    }

    /// The note as it goes out, on the current articulation's channel when it
    /// has one (the spec, R10).
    fn on_channel(&self, note: NoteEvent) -> NoteEvent {
        match self.channel() {
            Some(channel) => NoteEvent { channel, ..note },
            None => note,
        }
    }

    /// Follows the current articulation into a list that changed (the spec,
    /// R7): the same articulation if it is still there, else the one before
    /// the removed one, or the first.
    fn follow(&self, list: ArticulationList) -> u8 {
        if list.is_empty() {
            return 0;
        }
        let current = usize::from(self.current);
        if let Some(index) = (0..list.len()).find(|index| {
            list.entry(*index)
                .is_some_and(|entry| *entry == self.current_entry)
        }) {
            return index as u8;
        }
        if list.len() == self.list.len() {
            // Nothing was removed: the articulation was edited, so it keeps
            // its place.
            return current.min(list.len() - 1) as u8;
        }
        // Something was removed: the first place the lists differ is where
        // it was.
        let mut at = 0;
        while at < list.len() && at < self.list.len() && self.list.entry(at) == list.entry(at) {
            at += 1;
        }
        if current == at {
            return at.saturating_sub(1).min(list.len() - 1) as u8;
        }
        let shifted = if current > at { current - 1 } else { current };
        shifted.min(list.len() - 1) as u8
    }

    /// Takes a new list, and follows the current articulation into it.
    fn set_list(&mut self, list: ArticulationList) {
        if list == self.list {
            return;
        }
        let current = self.follow(list);
        self.list = list;
        self.current = current;
        self.current_entry = list
            .entry(usize::from(current))
            .copied()
            .unwrap_or_default();
    }
}

impl NativeDevice for Map {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let length = value(params, KEYSWITCH_LENGTH);
        let sample_rate = audio.sample_rate().as_f64();
        let length_frames = (length * sample_rate / 1000.0).round().max(1.0) as i64;
        let pass = value(params, PASS_TRIGGERS) >= 0.5;
        let resend = value(params, RESEND_ON_START) >= 0.5;

        // Spec: articulation-map R12. The device sends the current
        // articulation's outputs again on the first frame it runs after
        // being switched on, and on the first frame the transport plays.
        if self.was_off {
            self.was_off = false;
            if resend {
                self.send_outputs(audio, 0, length_frames);
            }
        }
        let playing = audio.playing();
        if playing && !self.was_playing && resend {
            self.send_outputs(audio, 0, length_frames);
        }
        self.was_playing = playing;

        // The Articulation control's value, frame by frame: the articulation
        // it names becomes current on the frame its value changes (R6).
        let index = params
            .index(ARTICULATION)
            .expect("the description holds the parameter");
        let ramp = params.ramp(index);
        let mut frame = 0usize;
        for note in audio.notes() {
            let at = note.offset as usize;
            while frame < ramp.len() && frame <= at {
                self.take_control(ramp[frame], audio, frame as u32, length_frames);
                frame += 1;
            }
            self.router.advance(audio, note.offset);
            self.end_due_keys(audio, i64::from(note.offset));
            self.handle(audio, note, pass, length_frames);
        }
        while frame < ramp.len() {
            self.take_control(ramp[frame], audio, frame as u32, length_frames);
            frame += 1;
        }
        self.end_due_keys(audio, ramp.len() as i64);
        self.router.finish(audio);
        self.publish();
    }

    fn receive_data(&mut self, data: bl_engine::NodeData) {
        if let Some(list) = data.downcast_ref::<Articulations>() {
            self.set_list(list.0);
        }
    }

    fn flush(&mut self, _events: &[bl_engine::NodeEvent], _params: &ParamBank) {
        // The device is switched off: it passes everything through, and
        // ends the notes it started (device-framework R32).
        self.was_off = true;
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        // Spec: articulation-map R13. A device switched off ends the
        // keyswitches it has sounding.
        for output in &self.keys {
            out.push(NoteEvent {
                offset: 0,
                kind: NoteKind::Off,
                channel: output.channel,
                key: output.key,
                velocity: 0.0,
                note_id: None,
            });
        }
        self.keys.clear();
        // Spec: device-framework R31, R32. The notes the device started,
        // on the channels they went out on, are ended too.
        self.router.release(out);
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.data);
    }
}

impl Map {
    /// Takes the Articulation control's value, and makes the articulation it
    /// names current when it changed.
    fn take_control(&mut self, value: f32, audio: &mut Audio<'_, '_>, at: u32, length: i64) {
        let number = f64::from(value).round();
        if number == self.last {
            return;
        }
        self.last = number;
        let count = self.list.len();
        if count == 0 {
            return;
        }
        let index = (number as usize).clamp(1, count) - 1;
        if index == usize::from(self.current) {
            return;
        }
        self.current = index as u8;
        self.current_entry = self.list.entry(index).copied().unwrap_or_default();
        self.send_outputs(audio, at, length);
    }

    /// Handles one note or message the device hears.
    fn handle(&mut self, audio: &mut Audio<'_, '_>, note: NoteEvent, pass: bool, length: i64) {
        match note.kind {
            NoteKind::On => {
                self.events += 1;
                self.event_kind = EVENT_NOTE;
                self.event_key = f32::from(note.key);
                self.event_value = note.velocity * 127.0;
                if let Some(index) = self.trigger_of_key(note.key) {
                    self.triggered(audio, index, note.offset, length);
                    // Spec: articulation-map R8. A trigger's note doesn't
                    // reach the next device unless Pass Triggers is on, and
                    // then it passes as it came.
                    if !pass {
                        return;
                    }
                    audio.send(note);
                    return;
                }
                let out = self.on_channel(note);
                if out.channel == note.channel {
                    // The note keeps its channel: it isn't the device's,
                    // and its own note-off ends it.
                    audio.send(out);
                } else {
                    // The device starts the note on the articulation's
                    // channel, and ends it there (device-framework R31).
                    self.router.track(&note, Some(Key::of(&out)));
                    self.router.send_now(audio, out, Key::of(&note));
                }
            }
            NoteKind::Off | NoteKind::Choke => {
                if self.trigger_of_key(note.key).is_some() {
                    if !pass {
                        return;
                    }
                    audio.send(note);
                    return;
                }
                // Spec: articulation-map R10. The note-off keeps the
                // channel its note-on went out on.
                self.router.end(audio, &note, note.velocity);
            }
            NoteKind::Control { controller, value } => {
                self.events += 1;
                self.event_kind = EVENT_CONTROL;
                self.event_key = f32::from(controller);
                self.event_value = value * 127.0;
                let raw = (value * 127.0).round().clamp(0.0, 127.0) as u8;
                if let Some(index) = self.trigger_of_control(controller, raw) {
                    self.triggered(audio, index, note.offset, length);
                    if !pass {
                        return;
                    }
                    audio.send(note);
                    return;
                }
                let out = self.on_channel(note);
                audio.send(out);
            }
            NoteKind::Expression { .. } => {
                // The expression follows the note it belongs to, on the
                // channel that note went out on.
                self.router.expression(audio, &note);
            }
            NoteKind::PitchBend { .. } | NoteKind::ChannelPressure { .. } => {
                let out = self.on_channel(note);
                audio.send(out);
            }
        }
    }

    /// Works out the data the display reads.
    fn publish(&mut self) {
        self.data[OFF_CURRENT] = f32::from(self.current) + 1.0;
        self.data[OFF_EVENTS] = self.events.min(u64::from(u32::MAX)) as f32;
        self.data[OFF_KIND] = self.event_kind;
        self.data[OFF_KEY] = self.event_key;
        self.data[OFF_VALUE] = self.event_value;
    }
}

/// The Articulation Map's display (the articulation map spec, R17): the
/// current articulation's number and name.
#[must_use]
pub fn display(
    device: bl_model::DeviceId,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    articulations: &[bl_model::Articulation],
) -> bl_model::DeviceDisplay {
    let mut display = bl_model::DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    let current = data
        .filter(|data| data.len() >= DISPLAY_LEN)
        .map_or_else(
            || values.get(&ARTICULATION).copied().unwrap_or(1.0).round(),
            |data| f64::from(data[OFF_CURRENT]).round(),
        )
        .max(1.0) as usize;
    let name = articulations.get(current - 1).map_or_else(
        || current.to_string(),
        |articulation| articulation.name.clone(),
    );
    display.readings.push(bl_model::DisplayReading {
        name: "Articulation".to_owned(),
        value: current as f32,
        unit: String::new(),
        text: current.to_string(),
    });
    display.readings.push(bl_model::DisplayReading {
        name: "Name".to_owned(),
        value: 0.0,
        unit: String::new(),
        text: name,
    });
    display
}

/// The trigger an articulation has, as the panel shows it: the key it plays,
/// or the controller and the range of values.
#[must_use]
pub fn trigger_text(trigger: &ArticulationTrigger) -> String {
    match *trigger {
        ArticulationTrigger::None => "-".to_owned(),
        ArticulationTrigger::Key { key } => format!("Key {key}"),
        ArticulationTrigger::Controller {
            controller,
            lowest,
            highest,
        } => {
            if lowest == highest {
                format!("CC {controller} {lowest}")
            } else {
                format!("CC {controller} {lowest}-{highest}")
            }
        }
    }
}

/// The outputs an articulation has, as the panel shows them: the keys it
/// plays and the controllers it sets.
#[must_use]
pub fn outputs_text(articulation: &bl_model::Articulation) -> String {
    let parts: Vec<String> = articulation
        .outputs
        .iter()
        .map(|output| match *output {
            ArticulationOutput::Key { key, velocity } => format!("Key {key} ({velocity})"),
            ArticulationOutput::Controller { controller, value } => {
                format!("CC {controller} {value}")
            }
        })
        .collect();
    if parts.is_empty() {
        "-".to_owned()
    } else {
        parts.join(", ")
    }
}
