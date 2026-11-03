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
//! Its panel is laid out as Live draws it (layout L-DEV-069): two rows of
//! four controls on each of the two pages its title-bar tabs show, with Learn
//! and Send in the title bar.

use bl_engine::{DeviceType, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, Control as Placed, ControlBand, LayoutNode, Page, PageStyle, PagesNode, TitlePart,
    Widget,
};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

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
/// The name of the pages node whose tabs show controls 1 to 8 or 9 to 16: the
/// page shown is view state, not a parameter.
pub const PAGES: &str = "Controls";
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

/// The name of controller `n` in a CC Type, as Live's chooser lists them
/// (observed in Live 12.4.16 on 3 November 2026): the standard MIDI
/// controller names, "Undefined" for those the standard leaves open, and for
/// 32 to 63 the least significant byte of controller `n - 32`.
fn cc_name(n: u8) -> String {
    let name = match n {
        1 => "Modulation Wheel",
        2 => "Breath Controller",
        4 => "Foot Controller",
        5 => "Portamento Time",
        6 => "Data Entry",
        7 => "Channel Volume",
        8 => "Balance",
        10 => "Pan",
        11 => "Expression Controller",
        12 => "Effect Control 1",
        13 => "Effect Control 2",
        16..=19 => return format!("General Purpose {}", n - 15),
        32..=63 => return format!("LSB for CC {} ({})", n - 32, cc_name(n - 32)),
        64 => "Sustain Pedal",
        65 => "Portamento On/Off",
        66 => "Sostenuto",
        67 => "Soft Pedal",
        68 => "Legato Footswitch",
        69 => "Hold 2",
        74 => "MPE Slide (Sound Controller 5)",
        70..=79 => return format!("Sound Controller {}", n - 69),
        80..=83 => return format!("General Purpose {}", n - 75),
        84 => "Portamento Control",
        91 => "Reverb Send Level",
        92 => "Effect 2 Depth",
        93 => "Chorus Send Level",
        94 => "Effect 4 Depth",
        95 => "Effect 5 Depth",
        96 => "Data Increment",
        97 => "Data Decrement",
        98 => "NRPN LSB",
        99 => "NRPN MSB",
        100 => "RPN LSB",
        101 => "RPN MSB",
        _ => "Undefined",
    };
    name.to_owned()
}

/// The CC Type list: `None` first, then the controllers 1 to 119 as "N:
/// Name". 0 (bank select's most significant byte) and 120 to 127 (channel
/// mode) are left out.
#[must_use]
pub fn cc_names() -> Vec<String> {
    let mut names = Vec::with_capacity(120);
    names.push("None".to_owned());
    names.extend((1..=119u8).map(|n| format!("{n}: {}", cc_name(n))));
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

/// Whether a controller has a CC Type in [`cc_names`]: 1 to 119. Controllers
/// 0 and 120 to 127 (channel mode) aren't listed, so Learn leaves them alone.
#[must_use]
pub fn cc_is_listed(controller: u8) -> bool {
    (1..=119).contains(&controller)
}

/// CC Control's panel as Live draws it: on each page two rows of four cells,
/// the first page's first row holding the three fixed controls and Custom A.
fn layout() -> DeviceLayout {
    let at = |param, widget, x, y| Placed::new(param, widget, x, y);
    // The centres of a row's four cells, in points.
    let centres = [33.0, 96.0, 159.0, 222.0];
    let dial = |k: u32, column: usize, row_y: f32, chooser_y: f32| {
        [
            at(
                dial_value(k),
                Widget::SmallKnob,
                centres[column] - 16.0,
                row_y,
            ),
            at(
                dial_cc(k),
                Widget::Chooser,
                centres[column] - 25.5,
                chooser_y,
            )
            .sized(51.0, 12.0)
            .unnamed(),
        ]
    };
    let band =
        |controls: Vec<Placed>| LayoutNode::Band(ControlBand::free(256.0, BODY_HEIGHT, controls));
    let mut first: Vec<Placed> = vec![
        at(MOD_WHEEL, Widget::Knob, centres[0] - 20.0, 7.0),
        at(PITCH_BEND, Widget::Knob, centres[1] - 20.0, 7.0),
        at(PRESSURE, Widget::Knob, centres[2] - 20.0, 7.0),
        at(CUSTOM_A, Widget::Check, centres[3] - 25.5, 7.0).sized(51.0, 40.0),
        at(CUSTOM_A_CC, Widget::Chooser, centres[3] - 25.5, 64.0)
            .sized(51.0, 12.0)
            .unnamed(),
    ];
    first.extend((0..4).flat_map(|k| dial(k, k as usize, 86.0, 142.0)));
    let mut second: Vec<Placed> = (4..8)
        .flat_map(|k| dial(k, k as usize - 4, 7.0, 64.0))
        .collect();
    second.extend((8..12).flat_map(|k| dial(k, k as usize - 8, 86.0, 142.0)));
    DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        PAGES,
        PageStyle::TitleBar,
        vec![
            Page::new("1-8", band(first)),
            Page::new("9-16", band(second)),
        ],
    )))
    .with_title(vec![TitlePart::Button(LEARN), TitlePart::Button(SEND)])
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
    DeviceType::native(CC_CONTROL, "CC Control", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Main",
                    &["Mod Wheel", "Pitch Bend", "Pressure", "Send", "Learn"],
                ),
                (
                    "Custom A-D",
                    &[
                        "Custom A",
                        "Custom A CC",
                        "Custom B",
                        "Custom B CC",
                        "Custom C",
                        "Custom C CC",
                        "Custom D",
                        "Custom D CC",
                    ],
                ),
                (
                    "Custom E-H",
                    &[
                        "Custom E",
                        "Custom E CC",
                        "Custom F",
                        "Custom F CC",
                        "Custom G",
                        "Custom G CC",
                        "Custom H",
                        "Custom H CC",
                    ],
                ),
                (
                    "Custom I-L",
                    &[
                        "Custom I",
                        "Custom I CC",
                        "Custom J",
                        "Custom J CC",
                        "Custom K",
                        "Custom K CC",
                        "Custom L",
                        "Custom L CC",
                    ],
                ),
                ("Custom M", &["Custom M", "Custom M CC"]),
            ]),
    )
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
