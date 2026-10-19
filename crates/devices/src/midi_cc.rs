//! CC Control, the controller-sending MIDI effect (WP-5.32; the MIDI CC
//! sender spec): three fixed controls (Mod Wheel, Pitch Bend, Pressure), one
//! Custom A button and twelve Custom dials, each with its own CC Type, a
//! page switch, a Send button and a Learn switch.
//!
//! A control sends its controller whenever its value changes, and Send sends
//! every assigned control once. Its messages travel in the block beside the
//! notes (the device framework spec, R39), so the next device and a plug-in
//! hear them on their frames. The device forwards what it hears unchanged,
//! and merges with a clip's own controller data for the same controller by
//! sending its message beside it (the MIDI controller data spec, R3): both
//! reach the next device, and the one that arrives last wins.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::{DeviceType, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::value;

/// The CC Control device type's id.
pub const CC_CONTROL: &str = "beatlayer.cc_control";

/// The Mod Wheel's value, 0 to 127.
pub const MOD_WHEEL: ParamId = ParamId(0);
/// The Pitch Bend's value, −8192 to 8191, 0 at the wheel's centre.
pub const PITCH_BEND: ParamId = ParamId(1);
/// The Pressure's value, 0 to 127.
pub const PRESSURE: ParamId = ParamId(2);
/// Custom A's value, 0 to 127.
pub const CUSTOM_A: ParamId = ParamId(3);
/// Custom A's controller, a CC Type.
pub const CUSTOM_A_CC: ParamId = ParamId(4);
/// The page shown: controls 1 to 8 or 9 to 16.
pub const PAGE: ParamId = ParamId(5);
/// Send: sends every assigned control's value once, on the frame it turns on.
pub const SEND: ParamId = ParamId(6);
/// Learn: hands an incoming controller to the next dial without one.
pub const LEARN: ParamId = ParamId(7);

/// How many Custom dials there are, B to M.
pub const DIALS: u32 = 12;

/// Custom dial `k`'s value, 0 to 127.
#[must_use]
pub fn dial_value(k: u32) -> ParamId {
    ParamId(8 + k * 2)
}

/// Custom dial `k`'s controller, a CC Type.
#[must_use]
pub fn dial_cc(k: u32) -> ParamId {
    ParamId(9 + k * 2)
}

/// The CC Type list: `None` first, then the controllers 1 to 119. 0 and 32
/// (bank select) and 120 to 127 (channel mode) are left out. The manual
/// doesn't state Live's list.
#[must_use]
pub fn cc_names() -> Vec<String> {
    let mut names = Vec::with_capacity(120);
    names.push("None".to_owned());
    for controller in 1..=119 {
        names.push(format!("CC {controller}"));
    }
    names
}

/// The value `None` has in a CC Type.
pub const NONE: f64 = 0.0;

fn cc_descriptor(id: ParamId, name: &str, default: u32, describe: &str) -> ParamDescriptor {
    let names = cc_names();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    ParamDescriptor::choice(id, name, &refs, default as usize).describe(describe)
}

/// The CC Type index a controller number has: its position in [`cc_names`].
#[must_use]
pub fn cc_type_for(controller: u8) -> f64 {
    f64::from(controller)
}

/// CC Control's device type.
#[must_use]
pub fn cc_control_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::integer(MOD_WHEEL, "Mod Wheel", (0, 127), 0, ParamUnit::None)
            .describe("The modulation amount sent on CC 1."),
        ParamDescriptor::integer(PITCH_BEND, "Pitch Bend", (-8192, 8191), 0, ParamUnit::None)
            .describe("The pitch bend sent, negative down and positive up."),
        ParamDescriptor::integer(PRESSURE, "Pressure", (0, 127), 0, ParamUnit::None)
            .describe("The channel pressure sent."),
        ParamDescriptor::integer(CUSTOM_A, "Custom A", (0, 127), 0, ParamUnit::None)
            .describe("Custom A's value, sent on its own controller."),
        cc_descriptor(
            CUSTOM_A_CC,
            "Custom A CC",
            64,
            "Custom A's controller. It starts on CC 64, the sustain pedal.",
        ),
        ParamDescriptor::choice(PAGE, "Page", &["1-8", "9-16"], 0)
            .describe("Which page of controls the device shows."),
        ParamDescriptor::toggle(SEND, "Send", false)
            .describe("Sends every assigned control's value once when it turns on."),
        ParamDescriptor::toggle(LEARN, "Learn", false).describe(
            "Hands the next controller an incoming MIDI source sends to the next Custom \
             dial without one, with its value.",
        ),
    ];
    for k in 0..DIALS {
        let letter = char::from(b'B' + k as u8);
        params.push(
            ParamDescriptor::integer(
                dial_value(k),
                &format!("Custom {letter}"),
                (0, 127),
                0,
                ParamUnit::None,
            )
            .describe(&format!(
                "Custom {letter}'s value, sent on its own controller."
            )),
        );
        params.push(cc_descriptor(
            dial_cc(k),
            &format!("Custom {letter} CC"),
            NONE as u32,
            &format!("Custom {letter}'s controller, or None."),
        ));
    }
    DeviceType::native(CC_CONTROL, "CC Control", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(params))
}

/// Adds CC Control to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cc_control_type(), |_, _| CcControl {
        last: [0.0; 16],
        primed: false,
        sent: false,
    });
}

/// One of the device's sixteen controls, in the units its controller sends.
#[derive(Clone, Copy)]
struct Control {
    /// Its controller, or `None` when it sends nothing.
    cc: Option<u8>,
    /// Its value, 0 to 127, or the pitch bend's raw value.
    value: f64,
    /// Whether it's the pitch bend.
    bend: bool,
    /// Whether it sends something at all.
    assigned: bool,
}

/// The event a control sends for its value.
fn event(control: &Control) -> NoteEvent {
    let kind = if control.bend {
        NoteKind::PitchBend {
            value: (control.value / 8192.0).clamp(-1.0, 1.0) as f32,
        }
    } else if let Some(0) = control.cc {
        NoteKind::ChannelPressure {
            value: (control.value / 127.0).clamp(0.0, 1.0) as f32,
        }
    } else {
        NoteKind::Control {
            controller: control.cc.unwrap_or(0),
            value: (control.value / 127.0).clamp(0.0, 1.0) as f32,
        }
    };
    NoteEvent {
        offset: 0,
        kind,
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    }
}

struct CcControl {
    /// The value each control last sent.
    last: [f64; 16],
    /// Whether the values have been taken as the starting point.
    primed: bool,
    /// Whether Send was on last block.
    sent: bool,
}

impl CcControl {
    /// The sixteen controls in order: 0 mod wheel, 1 pitch bend, 2 pressure,
    /// 3 Custom A, 4 to 15 Custom B to M.
    fn controls(&self, params: &ParamBank) -> [Control; 16] {
        let simple = |cc: Option<u8>, value: f64| Control {
            cc,
            value,
            bend: false,
            assigned: true,
        };
        let mut controls = [Control {
            cc: None,
            value: 0.0,
            bend: false,
            assigned: false,
        }; 16];
        controls[0] = simple(Some(1), value(params, MOD_WHEEL));
        controls[1] = Control {
            cc: None,
            value: value(params, PITCH_BEND),
            bend: true,
            assigned: true,
        };
        controls[2] = simple(Some(0), value(params, PRESSURE));
        let custom_a = value(params, CUSTOM_A_CC) as u8;
        controls[3] = simple((custom_a != 0).then_some(custom_a), value(params, CUSTOM_A));
        controls[3].assigned = custom_a != 0;
        for k in 0..DIALS {
            let cc = value(params, dial_cc(k)) as u8;
            controls[4 + k as usize] =
                simple((cc != 0).then_some(cc), value(params, dial_value(k)));
            controls[4 + k as usize].assigned = cc != 0;
        }
        controls
    }
}

impl NativeDevice for CcControl {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        // The device forwards what it hears unchanged.
        for note in audio.notes() {
            audio.send(note);
        }

        let controls = self.controls(params);
        if !self.primed {
            for (index, control) in controls.iter().enumerate() {
                self.last[index] = control.value;
            }
            self.primed = true;
        }
        let send = value(params, SEND) >= 0.5;
        let sending = send && !self.sent;
        self.sent = send;
        for (index, control) in controls.iter().enumerate() {
            if !control.assigned {
                continue;
            }
            if sending || control.value != self.last[index] {
                audio.send(event(control));
                self.last[index] = control.value;
            }
        }
    }
}

/// The controller an incoming control change names, with its value 0 to 127,
/// or `None` for anything that isn't a control change.
#[must_use]
pub fn learned_controller(note: &NoteEvent) -> Option<(u8, f64)> {
    match note.kind {
        NoteKind::Control { controller, value } => Some((controller, f64::from(value) * 127.0)),
        _ => None,
    }
}

/// The controller a learned message assigns: the first Custom dial without
/// one, or `None` when every dial has one.
#[must_use]
pub fn next_unassigned(params: &[(ParamId, f64)]) -> Option<u32> {
    (0..DIALS).find(|k| {
        params
            .iter()
            .find(|(id, _)| *id == dial_cc(*k))
            .is_none_or(|(_, cc)| *cc == NONE)
    })
}
