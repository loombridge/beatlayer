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

use bl_engine::NudgeDirection;
use bl_shell::arrangement_commands::{AUTOMATION_ARM, FOLLOW};
use bl_shell::link_commands as link;
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
    Element::GlobalAmount,
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
        E::GlobalAmount => Field::new(G::GlobalAmount, 0),
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
        Element::AutomationArm => Some(AUTOMATION_ARM),
        Element::CaptureMidi => Some(CAPTURE_MIDI),
        Element::NudgeDown | Element::NudgeUp => Some(ids::PHASE_NUDGE),
        Element::TapTempo => Some(ids::TAP_TEMPO),
        Element::ExternalSync => Some(ids::SET_EXTERNAL_SYNC),
        Element::LinkToggle => Some(link::SET_ENABLED),
        element => Some(match field(element)?.group {
            Group::Position => ids::LOCATE,
            Group::LoopStart | Group::LoopLength => ids::SET_LOOP,
            Group::Tempo => ids::SET_TEMPO,
            Group::Numerator | Group::Denominator => ids::SET_TIME_SIGNATURES,
            Group::GlobalAmount => bl_shell::groove_commands::SET_GLOBAL_AMOUNT,
        }),
    }
}

/// What clicking Play, Stop, or the Loop switch runs (R1, R2, R5).
#[must_use]
pub fn click(element: Element, shell: &Shell) -> Option<Invocation> {
    match element {
        Element::ReEnableAutomation => Some(Invocation::new(RE_ENABLE_AUTOMATION, json!({}))),
        Element::AutomationArm => Some(Invocation::new(AUTOMATION_ARM, json!({}))),
        Element::CaptureMidi => Some(Invocation::new(CAPTURE_MIDI, json!({}))),
        Element::TapTempo => Some(Invocation::new(ids::TAP_TEMPO, json!({}))),
        Element::ExternalSync => Some(Invocation::new(
            ids::SET_EXTERNAL_SYNC,
            json!({ "on": !shell.external_sync() }),
        )),
        Element::LinkToggle => Some(Invocation::new(
            link::SET_ENABLED,
            json!({ "on": !shell.link_enabled() }),
        )),
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

/// The Phase Nudge button an element is.
fn nudge_direction(element: Element) -> Option<NudgeDirection> {
    match element {
        Element::NudgeDown => Some(NudgeDirection::Down),
        Element::NudgeUp => Some(NudgeDirection::Up),
        _ => None,
    }
}

/// Whether any of the set's automated controls is overridden: the Control
/// Bar's Re-Enable Automation button lights while one is (the automation
/// spec, R5).
#[must_use]
pub fn automation_overridden(shell: &Shell) -> bool {
    // Spec: session-view R23. A launched scene's tempo or signature
    // overrides the set's, and lights it too (observed in Live 12.4.3).
    !shell.scene_override().is_none()
        // Spec: link R4. A peer's tempo took the tempo envelope's place.
        || shell.tempo_overridden()
        || shell
            .editor()
            .document()
            .tracks()
            .any(bl_model::Track::automation_overridden)
}

/// What the Link toggle reads: Link while Link is off, then the number of
/// other peers, as in 0 Links, 1 Link, 2 Links (the Link spec, R18).
#[must_use]
pub fn link_text(on: bool, peers: u64) -> String {
    match (on, peers) {
        (false, _) => "Link".to_owned(),
        (true, 1) => "1 Link".to_owned(),
        (true, peers) => format!("{peers} Links"),
    }
}

/// The Capture MIDI command (the capture MIDI spec).
pub const CAPTURE_MIDI: &str = bl_shell::capture_midi_commands::CAPTURE;

/// The Re-Enable Automation command (the automation spec, R5).
pub const RE_ENABLE_AUTOMATION: &str = bl_model::registry::RE_ENABLE_AUTOMATION;

/// Unfollow Tempo Automation, in the tempo field's context menu (the
/// warping spec, R46).
pub const UNFOLLOW_TEMPO_AUTOMATION: &str = "song.unfollow_tempo_automation";

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
        Group::GlobalAmount => "Global Amount",
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

/// The section's state between frames: a value being typed, a drag, and the
/// Phase Nudge button the mouse holds.
#[derive(Clone, Debug, Default)]
pub struct TransportFields {
    entry: Option<Entry>,
    drag: Option<Drag>,
    nudging: Option<Element>,
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
            Element::AutomationArm => {
                // Spec: automation R17.
                let on = shell.arrangement().automation_arm();
                return Some(Look {
                    on,
                    value: Some(if on { "on" } else { "off" }.to_owned()),
                    description: Some(
                        "While Arrangement Record runs, moving a control writes its \
                         values into its automation."
                            .to_owned(),
                    ),
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
            Element::CaptureMidi => {
                // Spec: capture-midi R1. It is enabled once notes have been
                // played on a monitoring MIDI track.
                let ready = shell.capture_midi_ready();
                return Some(Look {
                    disabled: !ready,
                    value: Some(if ready { "ready" } else { "nothing to capture" }.to_owned()),
                    description: Some(
                        "Brings back the notes just played on a MIDI track as a clip.".to_owned(),
                    ),
                    ..Look::default()
                });
            }
            Element::ExternalSync => {
                // Spec: midi-clock R15, R27. The song follows the incoming
                // clock while it is on.
                let on = shell.external_sync();
                return Some(Look {
                    on,
                    text: Some("EXT".to_owned()),
                    value: Some(if on { "on" } else { "off" }.to_owned()),
                    description: Some(
                        "Makes the song follow the tempo, start, stop and position of the \
                         MIDI clock an input with Sync on receives."
                            .to_owned(),
                    ),
                    ..Look::default()
                });
            }
            Element::LinkToggle => {
                // Spec: link R17, R18, R23. It reads Link while off, and
                // the other peers' number while on.
                let on = shell.link_enabled();
                let text = link_text(on, shell.link_peers());
                return Some(Look {
                    on,
                    value: Some(text.clone()),
                    text: Some(text),
                    description: Some(
                        "Shares the tempo, beat and phase with other apps on the network \
                         through Link."
                            .to_owned(),
                    ),
                    ..Look::default()
                });
            }
            Element::NudgeDown | Element::NudgeUp => {
                // Spec: transport R28, R29. Pressed while held, and off while
                // an outside tempo leads.
                let pressed = shell.phase_nudge() == nudge_direction(element);
                return Some(Look {
                    on: pressed,
                    disabled: shell.following(),
                    value: Some(if pressed { "pressed" } else { "released" }.to_owned()),
                    description: Some(
                        "While held, the song plays 4 % slower or faster, to bring it into \
                         phase."
                            .to_owned(),
                    ),
                    ..Look::default()
                });
            }
            Element::TapTempo => {
                // Spec: control-bar-transport R21, R23. It is disabled while
                // the song has tempo changes, and describes the run's tempo.
                let editable = transport::editable(shell, Group::Tempo);
                let description = match (editable, shell.tap_tempo_tempo()) {
                    (false, _) => {
                        "The song has tempo changes, so tapping can't set its tempo.".to_owned()
                    }
                    (true, Some(tempo)) => format!("Tempo {tempo:.2} BPM."),
                    (true, None) => "Tap once a beat to set the tempo.".to_owned(),
                };
                return Some(Look {
                    text: Some("TAP".to_owned()),
                    disabled: !editable,
                    description: Some(description),
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
        let editable = transport::editable(shell, field.group);
        if !editable {
            // Spec: midi-clock R17. With EXT on, the clock sets the tempo.
            let why = if field.group == Group::Tempo && shell.cv_clock_leader().is_some() {
                "a CV Clock In sets it"
            } else if field.group == Group::Tempo && shell.external_tempo().is_some() {
                "the incoming MIDI clock sets it"
            } else {
                "the song changes it along the arrangement"
            };
            description = format!(
                "{} {}: {why}, so it can't be set here.",
                group_name(field.group),
                transport::group_text(shell, field.group)
            );
        }
        // Spec: time-editing R33. The signature fields show an automation
        // LED while the set has signature changes.
        let automated = matches!(field.group, Group::Numerator | Group::Denominator)
            && shell
                .editor()
                .document()
                .settings()
                .time_signatures
                .has_changes();
        if automated {
            description
                .push_str(" The set has time-signature changes; this sets the one in force.");
        }
        Some(Look {
            automated,
            disabled: !editable,
            entry: typed.is_some(),
            text: Some(typed.unwrap_or_else(|| shown.clone())),
            value: Some(shown),
            number: Some(transport::number(shell, field)),
            description: Some(description),
            ..Look::default()
        })
    }

    /// Presses and releases a Phase Nudge button as the mouse holds it
    /// (transport R28): the press runs `transport.phase_nudge` held, and the
    /// release runs it not held. A hold also ends when the window loses the
    /// mouse or the focus, which can leave the release unseen. A press is
    /// ignored while an outside tempo leads (R29); a release never is.
    pub fn nudge(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        element: Element,
        response: &Response,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(direction) = nudge_direction(element) else {
            return;
        };
        let run = |held| {
            UiEvent::Invoke(Invocation::new(
                ids::PHASE_NUDGE,
                json!({ "direction": direction, "held": held }),
            ))
        };
        let (down, pressed, lost) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.primary_pressed(),
                i.events
                    .iter()
                    .any(|event| matches!(event, Event::PointerGone | Event::WindowFocused(false))),
            )
        });
        if self.nudging == Some(element) {
            if !down || lost {
                self.nudging = None;
                events.push(run(false));
            }
        } else if self.nudging.is_none()
            && pressed
            && !shell.following()
            && response.is_pointer_button_down_on()
        {
            self.nudging = Some(element);
            events.push(run(true));
        }
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
