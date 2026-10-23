//! What an automation lane and the Envelope Editor share when an envelope is
//! edited from the keyboard (WP-6.26; the automation editing spec, R20 to
//! R28): the keys the keymap leaves to them, the value being typed, the
//! insert marker with the value shown beside it, and how the focused and
//! selected breakpoints are drawn and read.
//!
//! Every key becomes a registered command (`envelope.*`) the shell
//! carries out; this module only maps keys to them.

use bl_core::time::Ticks;
use bl_shell::Invocation;
use bl_shell::envelope_commands::{ArrowKey, EnvelopeKeyState, MarkerStep};
use egui::{
    Align2, Color32, Context, Event, Id, Key, Modifiers, Painter, Rect, Stroke, Ui, pos2, vec2,
};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How often a selected breakpoint's pulse is redrawn.
pub(crate) const PULSE_FRAME: std::time::Duration = std::time::Duration::from_millis(40);

/// The longest value that can be typed.
const LONGEST: usize = 12;

/// A value being typed for a breakpoint (R23, R27).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Typed {
    /// What has been typed.
    pub text: String,
    /// Where Return makes a new breakpoint (Add Value); `None` gives the
    /// selected breakpoint the value.
    pub add_at: Option<Ticks>,
    /// What Return runs after the value, to let go of the breakpoint a menu
    /// item selected, so the whole is one undo step.
    pub release: Option<(String, Value)>,
}

fn typed_id() -> Id {
    Id::new("envelope-typed-value")
}

/// The value being typed, if any.
#[must_use]
pub(crate) fn typed(ctx: &Context) -> Option<Typed> {
    ctx.data(|d| d.get_temp::<Typed>(typed_id()))
}

/// Starts typing a value, or stops with `None`.
pub(crate) fn set_typed(ctx: &Context, typed: Option<Typed>) {
    ctx.data_mut(|d| {
        if let Some(typed) = typed {
            d.insert_temp(typed_id(), typed);
        } else {
            d.remove::<Typed>(typed_id());
        }
    });
}

/// Whether a value is being typed, so key presses go to it rather than to
/// shortcuts.
#[must_use]
pub fn is_typing(ctx: &Context) -> bool {
    typed(ctx).is_some()
}

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

/// The character a key types into a value: a digit, a minus, a point, or
/// one of the letters a pan reads (`25L`, `C`) or a switch (`on`).
fn character(key: Key) -> Option<char> {
    digit(key).or(match key {
        Key::Minus => Some('-'),
        Key::Period => Some('.'),
        Key::L => Some('L'),
        Key::R => Some('R'),
        Key::C => Some('C'),
        _ => None,
    })
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

fn arrow(key: Key) -> Option<ArrowKey> {
    Some(match key {
        Key::ArrowLeft => ArrowKey::Left,
        Key::ArrowRight => ArrowKey::Right,
        Key::ArrowUp => ArrowKey::Up,
        Key::ArrowDown => ArrowKey::Down,
        _ => return None,
    })
}

/// Reads the keys the keymap leaves to the envelope that has the keyboard,
/// takes them out of this frame's input, and returns the commands they run
/// into `events` (S-A11Y-018, -020, -023, -026 to -030):
///
/// - Left and Right move the insert marker a grid step, and Option-Left and
///   Option-Right (Alt) to the previous or next breakpoint, or with a
///   breakpoint selected move that breakpoint in time;
/// - Up and Down change the selected breakpoint's value;
/// - Option-Up and Option-Down (Alt), with Shift too, ask `cycle` for the
///   command that shows the previous or next parameter;
/// - digits start a value to type, which Return sets and Esc drops.
///
/// Up and Down with nothing selected are left to the view.
pub(crate) fn read(
    ui: &Ui,
    state: &EnvelopeKeyState,
    cycle: impl Fn(bool, bool) -> Option<Invocation>,
    events: &mut Vec<UiEvent>,
) {
    let ctx = ui.ctx().clone();
    let mut entry = typed(&ctx);
    let selected = state.selected.is_some();
    let typing = entry.is_some();
    let wants = |key: Key, mods: Modifiers| {
        if mods.command || mods.ctrl {
            return false;
        }
        if typing {
            return !mods.alt
                && (character(key).is_some()
                    || matches!(key, Key::Backspace | Key::Enter | Key::Escape));
        }
        match key {
            _ if digit(key).is_some() || key == Key::Minus => selected && !mods.alt && !mods.shift,
            Key::ArrowLeft | Key::ArrowRight => !mods.shift && !(selected && mods.alt),
            Key::ArrowUp | Key::ArrowDown => mods.alt || (selected && !mods.shift),
            _ => false,
        }
    };
    let taken: Vec<(Key, Modifiers)> = ui.input_mut(|i| {
        let mut taken = Vec::new();
        i.events.retain(|event| match event {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if wants(*key, *modifiers) => {
                taken.push((*key, *modifiers));
                false
            }
            _ => true,
        });
        taken
    });
    for (key, mods) in taken {
        if let Some(mut typed) = entry.take() {
            match key {
                Key::Enter => {
                    let mut args = json!({ "text": typed.text });
                    if let Some(at) = typed.add_at {
                        args["at"] = json!(at);
                    }
                    invoke(events, "envelope.set_value", args);
                    if let Some((id, args)) = typed.release {
                        invoke(events, &id, args);
                    }
                }
                Key::Escape => {
                    // The selection a menu item made is let go of too.
                    if let Some((id, args)) = typed.release {
                        invoke(events, &id, args);
                    }
                }
                Key::Backspace => {
                    typed.text.pop();
                    if !(typed.text.is_empty() && typed.add_at.is_none() && typed.release.is_none())
                    {
                        entry = Some(typed);
                    }
                }
                key => {
                    if let Some(c) = character(key)
                        && typed.text.len() < LONGEST
                    {
                        typed.text.push(c);
                    }
                    entry = Some(typed);
                }
            }
            continue;
        }
        if let Some(c) = character(key) {
            entry = Some(Typed {
                text: c.to_string(),
                add_at: None,
                release: None,
            });
            continue;
        }
        let Some(direction) = arrow(key) else {
            continue;
        };
        match direction {
            ArrowKey::Left | ArrowKey::Right if selected => {
                invoke(
                    events,
                    "envelope.move_selected",
                    json!({ "direction": direction }),
                );
            }
            ArrowKey::Left | ArrowKey::Right => {
                let by = if mods.alt {
                    MarkerStep::Breakpoint
                } else {
                    MarkerStep::Grid
                };
                invoke(
                    events,
                    "envelope.move_marker",
                    json!({ "direction": direction, "by": by }),
                );
            }
            ArrowKey::Up | ArrowKey::Down if mods.alt => {
                if let Some(invocation) = cycle(direction == ArrowKey::Down, mods.shift) {
                    events.push(UiEvent::Invoke(invocation));
                }
            }
            ArrowKey::Up | ArrowKey::Down => {
                invoke(
                    events,
                    "envelope.move_selected",
                    json!({ "direction": direction }),
                );
            }
        }
    }
    if entry != typed(&ctx) {
        set_typed(&ctx, entry);
    }
}

/// The colour of a breakpoint the keyboard has: the focused one (at the
/// insert marker) in the hover colour, and a selected one pulsing between
/// that and the selection colour (R21, R22).
#[must_use]
pub(crate) fn breakpoint_colour(
    theme: &Theme,
    at: Ticks,
    state: &EnvelopeKeyState,
    time: f64,
) -> Option<Color32> {
    let c = &theme.colors;
    if state.selected == Some(at) {
        let t = ((time * 4.0).sin() * 0.5 + 0.5) as f32;
        Some(c.focus.0.lerp_to_gamma(c.time_selection.0, t))
    } else if state.marker == at {
        Some(c.focus.0)
    } else {
        None
    }
}

/// Draws the insert marker as a line down a lane at `x`, with the value the
/// envelope has there (or what is being typed) beside it (R21).
pub(crate) fn paint_marker(
    painter: &Painter,
    theme: &Theme,
    rect: Rect,
    x: f32,
    text: &str,
    typing: bool,
) {
    let c = &theme.colors;
    painter.line_segment(
        [pos2(x, rect.top()), pos2(x, rect.bottom())],
        Stroke::new(1.0, c.text_dim.0),
    );
    let (anchor, side) = if x + 90.0 > rect.right() {
        (Align2::RIGHT_TOP, -3.0)
    } else {
        (Align2::LEFT_TOP, 3.0)
    };
    let text = if typing {
        format!("{text}|")
    } else {
        text.to_owned()
    };
    let galley = painter.layout_no_wrap(text, theme.small_font(), c.text.0);
    let at = pos2(x + side, rect.top() + 1.0);
    let text_rect = anchor
        .anchor_size(at, galley.size())
        .expand2(vec2(2.0, 0.0));
    painter.rect_filled(text_rect, 0.0, c.window.0);
    if typing {
        painter.rect_stroke(
            text_rect,
            0.0,
            Stroke::new(1.0, c.focus.0),
            egui::StrokeKind::Outside,
        );
    }
    painter.galley(text_rect.min + vec2(2.0, 0.0), galley, c.text.0);
}

/// What the screen reader reads for a breakpoint: its position in bars,
/// beats and sixteenths and its value as the parameter reads it, and
/// whether it is selected (R28).
pub(crate) fn describe_breakpoint(
    ui: &Ui,
    id: Id,
    rect: Rect,
    position: String,
    value: String,
    selected: bool,
) {
    let _ = ui.interact(rect, id, egui::Sense::hover());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::ListItem);
        node.set_label("Breakpoint".to_owned());
        node.set_selected(selected);
        node.set_value(format!("{position}, {value}"));
    });
}
