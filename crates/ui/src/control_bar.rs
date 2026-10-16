//! The Control Bar's transport section (WP-1.18; layout.md L-CTRL-005,
//! L-CTRL-007, L-CTRL-012 to L-CTRL-018): Play, Stop, the Loop switch, and
//! the value fields for the tempo, the time signature, the Arrangement
//! Position, and the loop brace.
//!
//! Every control runs a registered command ([`command`]), so a person's
//! click, drag, or typing and an agent reach the same implementation
//! (D17). What each field shows, and the command each change makes, comes
//! from `bl_shell::transport`; this module turns the person's input into
//! those changes, Live's way (interactions.md I-DRAG-022, I-CLICK-021;
//! shortcuts.toml S-VAL-001 to S-VAL-008): drag up or down, Up and Down
//! (Shift for fine steps), Delete or a double-click for the default, and
//! digits to type a value, with Period or Comma to the next field of a
//! bar-beat-sixteenth value, Enter to set it, and Esc to drop it. The
//! rules are in the control bar transport spec.

use bl_shell::arrangement_commands::FOLLOW;
use bl_shell::transport::{self, Field, Group};
use bl_shell::transport_commands as ids;
use bl_shell::{Invocation, Shell};
use egui::accesskit::Action;
use egui::{Event, EventFilter, Key, Response, Ui};
use serde_json::json;

use crate::layout::Element;
use crate::main_window::{UiEvent, element_id};
use crate::widgets::Look;

/// Points of vertical drag per step of a value field (I-DRAG-022).
pub const DRAG_PER_STEP: f32 = 3.0;

/// The longest value a field takes typed in, in characters.
const MAX_TYPED: usize = 8;

/// The transport section's controls, in on-screen order: every one runs a
/// registered command ([`command`]) and is reached from the keyboard.
pub const SECTION: &[Element] = &[
    Element::Tempo,
    Element::SignatureNumerator,
    Element::SignatureDenominator,
    Element::Metronome,
    Element::Follow,
    Element::PositionBars,
    Element::PositionBeats,
    Element::PositionSixteenths,
    Element::Play,
    Element::Stop,
    Element::LoopStartBars,
    Element::LoopStartBeats,
    Element::LoopStartSixteenths,
    Element::PunchIn,
    Element::Loop,
    Element::PunchOut,
    Element::LoopLengthBars,
    Element::LoopLengthBeats,
    Element::LoopLengthSixteenths,
];

/// The value field an element is, if it's one of the section's.
#[must_use]
pub fn field(element: Element) -> Option<Field> {
    use Element as E;
    use Group as G;
    Some(match element {
        E::Tempo => Field::new(G::Tempo, 0),
        E::SignatureNumerator => Field::new(G::Numerator, 0),
        E::SignatureDenominator => Field::new(G::Denominator, 0),
        E::PositionBars => Field::new(G::Position, 0),
        E::PositionBeats => Field::new(G::Position, 1),
        E::PositionSixteenths => Field::new(G::Position, 2),
        E::LoopStartBars => Field::new(G::LoopStart, 0),
        E::LoopStartBeats => Field::new(G::LoopStart, 1),
        E::LoopStartSixteenths => Field::new(G::LoopStart, 2),
        E::LoopLengthBars => Field::new(G::LoopLength, 0),
        E::LoopLengthBeats => Field::new(G::LoopLength, 1),
        E::LoopLengthSixteenths => Field::new(G::LoopLength, 2),
        _ => return None,
    })
}

/// The element that shows a value field.
#[must_use]
pub fn element_of(wanted: Field) -> Option<Element> {
    SECTION
        .iter()
        .copied()
        .find(|element| field(*element) == Some(wanted))
}

/// The registered command a control of the section runs, which agents run
/// by ID through the agent API (R12).
#[must_use]
pub fn command(element: Element) -> Option<&'static str> {
    match element {
        Element::Play => Some(ids::PLAY),
        Element::Stop => Some(ids::STOP),
        Element::Loop => Some(ids::SET_LOOP_ENABLED),
        Element::PunchIn => Some(ids::SET_PUNCH_IN),
        Element::PunchOut => Some(ids::SET_PUNCH_OUT),
        Element::Metronome => Some(ids::SET_METRONOME_ENABLED),
        Element::Follow => Some(FOLLOW),
        element => Some(match field(element)?.group {
            Group::Position => ids::LOCATE,
            Group::LoopStart | Group::LoopLength => ids::SET_LOOP,
            Group::Tempo => ids::SET_TEMPO,
            Group::Numerator | Group::Denominator => ids::SET_TIME_SIGNATURES,
        }),
    }
}

/// What clicking Play, Stop, or the Loop switch runs (R1, R2, R5).
#[must_use]
pub fn click(element: Element, shell: &Shell) -> Option<Invocation> {
    match element {
        Element::ReEnableAutomation => Some(Invocation::new(RE_ENABLE_AUTOMATION, json!({}))),
        Element::Play => Some(Invocation::new(ids::PLAY, json!({}))),
        Element::Stop => Some(Invocation::new(ids::STOP, json!({}))),
        Element::Loop => Some(Invocation::new(
            ids::SET_LOOP_ENABLED,
            json!({ "enabled": !loop_on(shell) }),
        )),
        Element::PunchIn => Some(Invocation::new(
            ids::SET_PUNCH_IN,
            json!({ "enabled": !punch_in_on(shell) }),
        )),
        Element::PunchOut => Some(Invocation::new(
            ids::SET_PUNCH_OUT,
            json!({ "enabled": !punch_out_on(shell) }),
        )),
        Element::Metronome => Some(Invocation::new(
            ids::SET_METRONOME_ENABLED,
            json!({ "enabled": !metronome_on(shell) }),
        )),
        _ => None,
    }
}

/// Whether any of the set's automated controls is overridden: the Control
/// Bar's Re-Enable Automation button lights while one is (the automation
/// spec, R5).
#[must_use]
pub fn automation_overridden(shell: &Shell) -> bool {
    shell
        .editor()
        .document()
        .tracks()
        .any(bl_model::Track::automation_overridden)
}

/// The Re-Enable Automation command (the automation spec, R5).
pub const RE_ENABLE_AUTOMATION: &str = bl_model::registry::RE_ENABLE_AUTOMATION;

fn loop_on(shell: &Shell) -> bool {
    shell
        .editor()
        .document()
        .settings()
        .arrangement_loop
        .enabled
}

fn punch_in_on(shell: &Shell) -> bool {
    shell.editor().document().settings().punch_in
}

fn punch_out_on(shell: &Shell) -> bool {
    shell.editor().document().settings().punch_out
}

fn metronome_on(shell: &Shell) -> bool {
    shell.editor().document().settings().metronome.enabled
}

/// What a group is called, for assistive technology.
fn group_name(group: Group) -> &'static str {
    match group {
        Group::Position => "Arrangement Position",
        Group::LoopStart => "Loop Start",
        Group::LoopLength => "Loop Length",
        Group::Tempo => "Tempo",
        Group::Numerator | Group::Denominator => "Time Signature",
    }
}

/// A value being typed into a group's fields (R9).
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    group: Group,
    /// What's been typed into each of the group's fields.
    typed: [Option<String>; 3],
}

/// A drag on a value field.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    field: Field,
    /// Points dragged that haven't made a whole step yet.
    pending: f32,
    /// Whether the drag opened a gesture, so its edits are one undo step.
    gesture: bool,
}

/// The section's state between frames: a value being typed, and a drag.
#[derive(Clone, Debug, Default)]
pub struct TransportFields {
    entry: Option<Entry>,
    drag: Option<Drag>,
}

impl TransportFields {
    /// Nothing typed, nothing dragged.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a value is being typed: the keys then go to the field, not
    /// to shortcuts (R10).
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.entry.is_some()
    }

    /// What a control of the section shows, or `None` for the rest.
    #[must_use]
    pub fn look(&self, shell: &Shell, element: Element) -> Option<Look> {
        match element {
            Element::Play => {
                let playing = shell.playing();
                return Some(Look {
                    on: playing,
                    value: Some(if playing { "playing" } else { "stopped" }.to_owned()),
                    ..Look::default()
                });
            }
            Element::Loop | Element::PunchIn | Element::PunchOut => {
                let on = match element {
                    Element::PunchIn => punch_in_on(shell),
                    Element::PunchOut => punch_out_on(shell),
                    _ => loop_on(shell),
                };
                return Some(Look {
                    on,
                    value: Some(if on { "on" } else { "off" }.to_owned()),
                    ..Look::default()
                });
            }
            Element::ReEnableAutomation => {
                // Spec: automation R5. It lights while any of the set's
                // automated controls is overridden: the Re-Enable Automation
                // button of the manual's Overriding Automation.
                let overridden = automation_overridden(shell);
                return Some(Look {
                    on: overridden,
                    value: Some(
                        if overridden {
                            "some automation is off"
                        } else {
                            "every envelope drives its control"
                        }
                        .to_owned(),
                    ),
                    description: Some(
                        "Puts every control whose automation a hand move turned off \
                         back under its envelope."
                            .to_owned(),
                    ),
                    ..Look::default()
                });
            }
            Element::Metronome => {
                let on = metronome_on(shell);
                return Some(Look {
                    on,
                    value: Some(if on { "on" } else { "off" }.to_owned()),
                    ..Look::default()
                });
            }
            _ => {}
        }
        let field = field(element)?;
        let typed = self
            .entry
            .as_ref()
            .filter(|entry| entry.group == field.group)
            .and_then(|entry| entry.typed[field.part].clone());
        let shown = transport::text(shell, field);
        let mut description = format!(
            "{} {}. Type a value, or use Up and Down.",
            group_name(field.group),
            transport::group_text(shell, field.group)
        );
        if !transport::editable(shell, field.group) {
            description = format!(
                "{} {}: the song changes it along the arrangement, so it can't be set here.",
                group_name(field.group),
                transport::group_text(shell, field.group)
            );
        }
        Some(Look {
            entry: typed.is_some(),
            text: Some(typed.unwrap_or_else(|| shown.clone())),
            value: Some(shown),
            number: Some(transport::number(shell, field)),
            description: Some(description),
            ..Look::default()
        })
    }

    /// Handles what the person did to a value field this frame, pushing the
    /// commands it runs onto `events`: a click or a drag gives it the
    /// keyboard; a drag up or down, Up and Down, or a screen reader's
    /// increment and decrement step it; Delete and a double-click reset it;
    /// digits type a value.
    pub fn interact(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        field: Field,
        response: &Response,
        events: &mut Vec<UiEvent>,
    ) {
        let push = |events: &mut Vec<UiEvent>, change: Option<Invocation>| {
            if let Some(invocation) = change {
                events.push(UiEvent::Invoke(invocation));
            }
        };
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        // Spec: control-bar-transport R8 (I-CLICK-021).
        if response.double_clicked() {
            self.entry = None;
            push(events, transport::reset(shell, field));
        }
        // Spec: control-bar-transport R8 (I-DRAG-022, S-VAL-003).
        if response.dragged() {
            let fine = ui.input(|i| i.modifiers.shift);
            let drag = self.drag.get_or_insert(Drag {
                field,
                pending: 0.0,
                gesture: false,
            });
            drag.pending -= response.drag_delta().y;
            let steps = (drag.pending / DRAG_PER_STEP).trunc();
            drag.pending -= steps * DRAG_PER_STEP;
            if steps != 0.0
                && let Some(change) = transport::step(shell, field, steps as i32, fine)
            {
                if !drag.gesture
                    && let Some(label) = field.group.gesture_label()
                {
                    drag.gesture = true;
                    events.push(UiEvent::BeginGesture(label));
                }
                self.entry = None;
                events.push(UiEvent::Invoke(change));
            }
        }
        if response.drag_stopped()
            && let Some(drag) = self.drag.take()
            && drag.gesture
        {
            events.push(UiEvent::EndGesture);
        }
        // A screen reader's increment and decrement (R11).
        let id = response.id;
        let requested = ui.input(|i| {
            i.num_accesskit_action_requests(id, Action::Increment) as i32
                - i.num_accesskit_action_requests(id, Action::Decrement) as i32
        });
        if requested != 0 {
            push(events, transport::step(shell, field, requested, false));
        }
        if response.has_focus() {
            self.keys(ui, shell, field, events);
        }
    }

    /// The keys a focused value field takes (S-VAL-001 to S-VAL-008).
    fn keys(&mut self, ui: &mut Ui, shell: &Shell, field: Field, events: &mut Vec<UiEvent>) {
        let typing = self.entry.is_some();
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                element_id_of(field),
                EventFilter {
                    vertical_arrows: true,
                    escape: typing,
                    ..Default::default()
                },
            );
        });
        // The keys this field acts on are taken out of the frame's input, so
        // the next field, if Period moves the focus to it this frame, and
        // egui itself don't act on them again.
        let takes = |key: Key| {
            digit(key).is_some()
                || matches!(
                    key,
                    Key::Period
                        | Key::Comma
                        | Key::Backspace
                        | Key::ArrowUp
                        | Key::ArrowDown
                        | Key::Delete
                )
                || (typing && matches!(key, Key::Enter | Key::Escape))
        };
        let pressed: Vec<(Key, egui::Modifiers)> = ui.input_mut(|i| {
            let mut taken = Vec::new();
            i.events.retain(|event| match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command && !modifiers.ctrl && takes(*key) => {
                    taken.push((*key, *modifiers));
                    false
                }
                _ => true,
            });
            taken
        });
        for (key, modifiers) in pressed {
            if let Some(digit) = digit(key) {
                // Spec: control-bar-transport R9 (S-VAL-005).
                self.type_char(field, digit);
                continue;
            }
            match key {
                Key::Period | Key::Comma if field.group == Group::Tempo => {
                    self.type_char(field, '.');
                }
                // S-VAL-006: on to the next field of the value.
                Key::Period | Key::Comma if field.part + 1 < field.group.parts() => {
                    self.entry_for(field.group);
                    if let Some(next) = element_of(Field::new(field.group, field.part + 1)) {
                        ui.memory_mut(|m| m.request_focus(element_id(next)));
                    }
                }
                Key::Backspace => {
                    if let Some(typed) = self
                        .entry
                        .as_mut()
                        .and_then(|entry| entry.typed[field.part].as_mut())
                    {
                        typed.pop();
                    }
                }
                // S-VAL-008: set the typed value.
                Key::Enter => {
                    if let Some(entry) = self.entry.take()
                        && let Some(change) = transport::typed(shell, field.group, &entry.typed)
                    {
                        events.push(UiEvent::Invoke(change));
                    }
                }
                // S-VAL-007: drop it.
                Key::Escape => self.entry = None,
                // S-VAL-001, S-VAL-002.
                Key::ArrowUp | Key::ArrowDown => {
                    self.entry = None;
                    let steps = if key == Key::ArrowUp { 1 } else { -1 };
                    if let Some(change) = transport::step(shell, field, steps, modifiers.shift) {
                        events.push(UiEvent::Invoke(change));
                    }
                }
                // S-VAL-004.
                Key::Delete => {
                    self.entry = None;
                    if let Some(change) = transport::reset(shell, field) {
                        events.push(UiEvent::Invoke(change));
                    }
                }
                _ => {}
            }
        }
    }

    /// The entry for `group`, started if none is, or if one for another
    /// group was.
    fn entry_for(&mut self, group: Group) -> &mut Entry {
        if self.entry.as_ref().is_none_or(|entry| entry.group != group) {
            self.entry = Some(Entry {
                group,
                typed: Default::default(),
            });
        }
        self.entry.as_mut().expect("just made")
    }

    fn type_char(&mut self, field: Field, c: char) {
        let entry = self.entry_for(field.group);
        let typed = entry.typed[field.part].get_or_insert_with(String::new);
        if typed.len() < MAX_TYPED && !(c == '.' && typed.contains('.')) {
            typed.push(c);
        }
    }

    /// Drops a typed value once the keyboard focus has left its group's
    /// fields (R9), and a drag whose field is gone.
    pub fn end_frame(&mut self, ui: &Ui) {
        let focused = ui.memory(egui::Memory::focused);
        if let Some(group) = self.entry.as_ref().map(|entry| entry.group) {
            let inside = (0..group.parts())
                .any(|part| Some(element_id_of(Field::new(group, part))) == focused);
            if !inside {
                self.entry = None;
            }
        }
    }
}

/// The widget ID of a field's element.
fn element_id_of(field: Field) -> egui::Id {
    element_of(field).map_or(egui::Id::NULL, element_id)
}

/// The digit a key types, if it's a digit key.
fn digit(key: Key) -> Option<char> {
    Some(match key {
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_its_element_and_every_control_a_command() {
        for element in SECTION {
            assert!(command(*element).is_some(), "{element:?}");
            if let Some(f) = field(*element) {
                assert_eq!(element_of(f), Some(*element));
            }
        }
        assert_eq!(command(Element::Record), None, "inert");
    }
}
