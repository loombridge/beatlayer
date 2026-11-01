//! MIDI Monitor, the display-only MIDI effect (WP-5.32; the MIDI monitor
//! spec): it shows what reaches it — the notes played with their velocity
//! and the chord they make, the flow of notes and messages, and the MPE
//! data — and changes nothing it passes.
//!
//! The display's data is published after each block through the device
//! framework's path (R35 to R37), so the cell draws it and an agent reads
//! the same data with `device.display`. Its look is Beatlayer's own: a
//! display cell beside the Freeze and Clear switches.

use bl_engine::{DeviceType, NoteExpression, NoteKind, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, OrdMap, ParamDescriptor,
    ParamId,
};
use bl_model::{DisplayReading, DisplaySeries};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::value;

/// The MIDI Monitor device type's id.
pub const MIDI_MONITOR: &str = "beatlayer.midi_monitor";

/// Freeze: holds the display where it is while on.
pub const FREEZE: ParamId = ParamId(0);
/// Clear: empties the display on the frame it turns on.
pub const CLEAR: ParamId = ParamId(1);

/// How many flow entries the display holds.
pub const FLOW_ROOM: usize = 16;
/// How many MPE entries the display holds.
pub const MPE_ROOM: usize = 16;

/// Where the keyboard's 128 velocities start.
const OFF_KEYS: usize = 0;
/// How many values one entry of the flow takes.
const FLOW_STRIDE: usize = 3;
/// How many values one entry of the MPE list takes.
const MPE_STRIDE: usize = 4;

/// Where the flow's count sits.
const OFF_FLOW_COUNT: usize = OFF_KEYS + 128;
/// Where the flow's entries sit.
const OFF_FLOW: usize = OFF_FLOW_COUNT + 1;
/// Where the MPE list's count sits.
const OFF_MPE_COUNT: usize = OFF_FLOW + FLOW_STRIDE * FLOW_ROOM;
/// Where the MPE list's entries sit.
const OFF_MPE: usize = OFF_MPE_COUNT + 1;

/// How many values the display's data holds.
pub const DISPLAY_LEN: usize = OFF_MPE + MPE_STRIDE * MPE_ROOM;

/// The views the display offers.
pub const VIEWS: [&str; 3] = ["Note", "Flow", "MPE"];

/// MIDI Monitor's device type.
#[must_use]
pub fn midi_monitor_type() -> DeviceType {
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(DisplayCell::new("MIDI Monitor", 260.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(FREEZE, Widget::Toggle), (CLEAR, Widget::Toggle)],
        )),
    ]));
    DeviceType::native(MIDI_MONITOR, "MIDI Monitor", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::toggle(FREEZE, "Freeze", false)
                .describe("Holds the display where it is while on."),
            ParamDescriptor::toggle(CLEAR, "Clear", false)
                .describe("Empties the display on the frame it turns on."),
        ])
        .with_layout(layout)
        .with_views(&VIEWS)
        .with_banks(&[("Monitor", &["Freeze", "Clear"])]),
    )
}

/// Adds MIDI Monitor to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, midi_monitor_type(), |_, _| Monitor::new());
}

/// The name of a key, such as `C4`, with middle C at key 60.
#[must_use]
pub fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(key) % 12],
        i32::from(key) / 12 - 1
    )
}

/// The root and name of the chord `keys` make, or `None` for a single key or
/// something that isn't one of the chords below. The root is the lowest key,
/// and the chord is read in root position: inversions aren't named. What
/// Live names, and how, isn't recorded; this is Beatlayer's own.
#[must_use]
pub fn chord(keys: &[u8]) -> Option<(u8, &'static str)> {
    let mut sorted: Vec<u8> = keys.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() < 2 {
        return None;
    }
    let root = sorted[0];
    let intervals: Vec<i32> = sorted
        .iter()
        .map(|key| (i32::from(*key) - i32::from(root)).rem_euclid(12))
        .collect();
    let name = match intervals.as_slice() {
        [0, 4, 7] => "Major",
        [0, 3, 7] => "Minor",
        [0, 3, 6] => "Diminished",
        [0, 4, 8] => "Augmented",
        [0, 2, 7] => "Sus2",
        [0, 5, 7] => "Sus4",
        [0, 4, 7, 11] => "Major 7",
        [0, 4, 7, 10] => "Dominant 7",
        [0, 3, 7, 10] => "Minor 7",
        _ => return None,
    };
    Some((root, name))
}

/// A flow entry's kind.
const FLOW_ON: f32 = 0.0;
const FLOW_OFF: f32 = 1.0;
const FLOW_CONTROL: f32 = 2.0;
const FLOW_BEND: f32 = 3.0;
const FLOW_PRESSURE: f32 = 4.0;

/// An MPE entry's kind.
const MPE_PRESSURE: f32 = 0.0;
const MPE_SLIDE: f32 = 1.0;
const MPE_BEND: f32 = 2.0;

/// The monitor's audio-thread half: what it has seen, and the data it
/// publishes.
pub struct Monitor {
    /// Each key's velocity, 0 when it isn't sounding.
    keys: [f32; 128],
    /// The flow's entries and where the next goes.
    flow: [(f32, u8, f32); FLOW_ROOM],
    flow_next: usize,
    flow_count: u64,
    /// The MPE entries and where the next goes: kind, key, first value,
    /// second value.
    mpe: [(f32, u8, f32, f32); MPE_ROOM],
    mpe_next: usize,
    mpe_count: u64,
    /// Where the display's data is written.
    data: Vec<f32>,
    cleared: bool,
}

impl Monitor {
    fn new() -> Self {
        Self {
            keys: [0.0; 128],
            flow: [(FLOW_ON, 0, 0.0); FLOW_ROOM],
            flow_next: 0,
            flow_count: 0,
            mpe: [(0.0, 0, 0.0, 0.0); MPE_ROOM],
            mpe_next: 0,
            mpe_count: 0,
            data: vec![0.0; DISPLAY_LEN],
            cleared: false,
        }
    }

    fn push_flow(&mut self, kind: f32, data1: u8, data2: f32) {
        self.flow[self.flow_next] = (kind, data1, data2);
        self.flow_next = (self.flow_next + 1) % FLOW_ROOM;
        self.flow_count += 1;
    }

    fn push_mpe(&mut self, kind: f32, key: u8, a: f32, b: f32) {
        self.mpe[self.mpe_next] = (kind, key, a, b);
        self.mpe_next = (self.mpe_next + 1) % MPE_ROOM;
        self.mpe_count += 1;
    }

    fn clear(&mut self) {
        self.keys = [0.0; 128];
        self.flow_next = 0;
        self.flow_count = 0;
        self.mpe_next = 0;
        self.mpe_count = 0;
    }

    /// Works out the data the display reads.
    fn publish(&mut self) {
        let data = &mut self.data;
        data[OFF_KEYS..OFF_KEYS + 128].copy_from_slice(&self.keys);
        data[OFF_FLOW_COUNT] = self.flow_count.min(u64::from(u32::MAX)) as f32;
        for (i, (kind, data1, data2)) in self.flow.iter().enumerate() {
            let at = OFF_FLOW + i * FLOW_STRIDE;
            data[at] = *kind;
            data[at + 1] = f32::from(*data1);
            data[at + 2] = *data2;
        }
        data[OFF_MPE_COUNT] = self.mpe_count.min(u64::from(u32::MAX)) as f32;
        for (i, (kind, key, a, b)) in self.mpe.iter().enumerate() {
            let at = OFF_MPE + i * MPE_STRIDE;
            data[at] = *kind;
            data[at + 1] = f32::from(*key);
            data[at + 2] = *a;
            data[at + 3] = *b;
        }
    }
}

impl NativeDevice for Monitor {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let freeze = value(params, FREEZE) >= 0.5;
        let clear = value(params, CLEAR) >= 0.5;
        if clear && !self.cleared {
            self.clear();
        }
        self.cleared = clear;
        for note in audio.notes() {
            if !freeze {
                match note.kind {
                    NoteKind::On => {
                        self.keys[usize::from(note.key)] = note.velocity;
                        self.push_flow(FLOW_ON, note.key, note.velocity);
                    }
                    NoteKind::Off | NoteKind::Choke => {
                        self.keys[usize::from(note.key)] = 0.0;
                        self.push_flow(FLOW_OFF, note.key, note.velocity);
                    }
                    NoteKind::Control { controller, value } => {
                        self.push_flow(FLOW_CONTROL, controller, value);
                    }
                    NoteKind::PitchBend { value } => {
                        self.push_flow(FLOW_BEND, 0, value);
                    }
                    NoteKind::ChannelPressure { value } => {
                        self.push_flow(FLOW_PRESSURE, 0, value);
                    }
                    NoteKind::Expression { expression, value } => {
                        let kind = match expression {
                            NoteExpression::Pressure => MPE_PRESSURE,
                            NoteExpression::Slide => MPE_SLIDE,
                            NoteExpression::Bend => MPE_BEND,
                        };
                        self.push_mpe(kind, note.key, value, 0.0);
                    }
                }
            }
            // The monitor changes nothing it passes.
            audio.send(note);
        }
        if !freeze {
            self.publish();
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.data);
    }
}

/// The monitor's display (the MIDI monitor spec): the keyboard with each
/// sounding key's velocity, its root and chord, and how many flow and MPE
/// entries it has seen.
#[must_use]
pub fn display(
    device: DeviceId,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let frozen = values.get(&FREEZE).is_some_and(|on| *on >= 0.5);
    let mut display = DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    let Some(data) = data.filter(|data| data.len() >= DISPLAY_LEN) else {
        display.readings.push(DisplayReading {
            name: "Notes".to_owned(),
            value: 0.0,
            unit: "notes".to_owned(),
            text: "no engine".to_owned(),
        });
        return display;
    };
    let velocity = &data[OFF_KEYS..OFF_KEYS + 128];
    display.series.push(DisplaySeries {
        name: "Keyboard".to_owned(),
        unit: "velocity".to_owned(),
        x_unit: "key".to_owned(),
        x_range: [0.0, 127.0],
        log_x: false,
        y_range: [0.0, 1.0],
        detail: false,
        values: velocity.to_vec(),
    });
    let keys: Vec<u8> = velocity
        .iter()
        .enumerate()
        .filter(|(_, v)| **v > 0.001)
        .map(|(key, _)| key as u8)
        .collect();
    let count = keys.len();
    display.readings.push(DisplayReading {
        name: "Notes".to_owned(),
        value: count as f32,
        unit: "notes".to_owned(),
        text: count.to_string(),
    });
    let (root, name) = match chord(&keys) {
        Some((root, name)) => (note_name(root), name.to_owned()),
        None => (
            keys.first()
                .map_or_else(|| "-".to_owned(), |key| note_name(*key)),
            "-".to_owned(),
        ),
    };
    display.readings.push(DisplayReading {
        name: "Root".to_owned(),
        value: 0.0,
        unit: "note".to_owned(),
        text: root,
    });
    display.readings.push(DisplayReading {
        name: "Chord".to_owned(),
        value: 0.0,
        unit: String::new(),
        text: name,
    });
    display.readings.push(DisplayReading {
        name: "Flow".to_owned(),
        value: data[OFF_FLOW_COUNT],
        unit: "messages".to_owned(),
        text: format!("{}", data[OFF_FLOW_COUNT] as u64),
    });
    display.readings.push(DisplayReading {
        name: "MPE".to_owned(),
        value: data[OFF_MPE_COUNT],
        unit: "messages".to_owned(),
        text: format!("{}", data[OFF_MPE_COUNT] as u64),
    });
    display.readings.push(DisplayReading {
        name: "Frozen".to_owned(),
        value: if frozen { 1.0 } else { 0.0 },
        unit: String::new(),
        text: if frozen { "on" } else { "off" }.to_owned(),
    });
    display
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(display: &DeviceDisplay, name: &str) -> String {
        display.reading(name).unwrap().text.clone()
    }

    // Spec: midi-monitor R7. The chord name reads the notes in root position.
    #[test]
    fn the_chord_name_reads_root_position() {
        assert_eq!(chord(&[60, 64, 67]), Some((60, "Major")));
        assert_eq!(chord(&[60, 63, 67]), Some((60, "Minor")));
        assert_eq!(chord(&[60, 64, 67, 71]), Some((60, "Major 7")));
        assert_eq!(chord(&[60]), None, "one key is no chord");
        assert_eq!(chord(&[60, 61]), None, "a semitone is no chord");
    }

    // Spec: midi-monitor R1, R5. The display reads the keyboard, the root
    // and chord, and the counts from the published data.
    #[test]
    fn the_display_reads_the_keyboard_and_the_chord() {
        let mut data = vec![0.0f32; DISPLAY_LEN];
        for key in [60u8, 64, 67] {
            data[OFF_KEYS + usize::from(key)] = 100.0 / 127.0;
        }
        data[OFF_FLOW_COUNT] = 3.0;
        data[OFF_MPE_COUNT] = 1.0;
        let values: OrdMap<ParamId, f64> = OrdMap::new();
        let display = display(
            DeviceId(bl_core::id::Id::new(1).unwrap()),
            &values,
            Some(&data),
        );
        let keyboard = display.series("Keyboard").unwrap();
        assert!((keyboard.values[60] - 100.0 / 127.0).abs() < 1e-6);
        assert_eq!(key(&display, "Notes"), "3");
        assert_eq!(key(&display, "Root"), "C4");
        assert_eq!(key(&display, "Chord"), "Major");
        assert_eq!(key(&display, "Flow"), "3");
        assert_eq!(key(&display, "MPE"), "1");
    }

    // Spec: midi-monitor R8. A key's name counts octaves from middle C.
    #[test]
    fn a_key_is_named_from_middle_c() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(69), "A4");
    }
}
