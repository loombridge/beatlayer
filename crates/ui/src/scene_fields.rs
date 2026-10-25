//! A scene's tempo and time signature fields (WP-3.5; layout.md L-SES-007
//! and L-SES-011), drawn in the Main column and in the Scene View.
//!
//! A field is grey, showing the set's current value, until the scene has its
//! own. Clicking one focuses it; digits type a value, Enter sets it, Esc
//! drops it; Up and Down step it (Shift for the tempo's hundredths), as does
//! a drag up or down; Delete and a double-click turn it off (I-CLICK-010).
//! What each does is `bl_shell::scene_fields`: a change is the registered
//! `scene.set_tempo` or `scene.set_time_signature`, so a person's input and
//! an agent reach the same commands.

use bl_model::SceneId;
use bl_shell::scene_fields::{self, SceneField};
use bl_shell::{Invocation, Shell};
use egui::accesskit::{self, Action, Role};
use egui::{Align2, Event, EventFilter, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// Points of vertical drag per step of a field (I-DRAG-022).
const DRAG_PER_STEP: f32 = 3.0;

/// The longest value typed into a field, in characters.
const MAX_TYPED: usize = 8;

/// The ID of a scene's field.
#[must_use]
pub fn field_id(scene: SceneId, field: SceneField) -> Id {
    Id::new(("scene-field", scene, field as u8))
}

/// What the person is typing in a field, kept between frames.
#[derive(Clone, Default)]
struct Typing(Option<String>);

/// How far a drag has gone since the last step.
#[derive(Clone, Default)]
struct Dragged(f32);

/// What a scene is called: "Scene" and its name, or its number when it has
/// no name of its own.
#[must_use]
pub fn scene_label(shell: &Shell, scene: SceneId) -> String {
    let doc = shell.editor().document();
    let name = doc
        .scene(scene)
        .map(|s| s.name.clone())
        .filter(|name| !name.is_empty())
        .or_else(|| doc.scene_position(scene).map(|at| (at + 1).to_string()))
        .unwrap_or_default();
    format!("Scene {name}")
}

/// What a field is called to assistive technology.
fn name(field: SceneField) -> &'static str {
    match field {
        SceneField::Tempo => "Tempo",
        SceneField::Numerator => "Time Signature Numerator",
        SceneField::Denominator => "Time Signature Denominator",
    }
}

/// Draws a scene's field in `rect` and adds what the person did to `events`.
/// `label` names the scene for assistive technology ("Scene 3").
#[expect(clippy::too_many_arguments, reason = "one field, drawn in full")]
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    scene: SceneId,
    field: SceneField,
    label: &str,
    events: &mut Vec<UiEvent>,
) {
    let id = field_id(scene, field);
    let view = scene_fields::view(shell, scene, field);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut push = |change: Option<Invocation>| {
        if let Some(change) = change {
            events.push(UiEvent::Invoke(change));
        }
    };

    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    // Spec: session-context-menus R3. While the field has the keyboard,
    // Delete and the other keys are its.
    if response.has_focus() {
        crate::session_menus::note_field_focus(ui.ctx());
    }
    // I-CLICK-010: a double-click turns the field off.
    if response.double_clicked() {
        push(scene_fields::off(shell, scene, field));
        ui.memory_mut(|m| m.data.remove::<Typing>(id));
    }
    // A drag up or down steps the value (I-DRAG-022).
    if response.dragged() {
        let moved = response.drag_delta().y;
        let total = ui.memory_mut(|m| {
            let dragged = m.data.get_temp_mut_or_default::<Dragged>(id);
            dragged.0 -= moved;
            dragged.0
        });
        let steps = (total / DRAG_PER_STEP).trunc() as i32;
        if steps != 0 {
            ui.memory_mut(|m| {
                m.data.get_temp_mut_or_default::<Dragged>(id).0 -= steps as f32 * DRAG_PER_STEP
            });
            push(scene_fields::step(shell, scene, field, steps, false));
        }
    }
    // A screen reader's increment and decrement.
    let requested = ui.input(|i| {
        i.num_accesskit_action_requests(id, Action::Increment) as i32
            - i.num_accesskit_action_requests(id, Action::Decrement) as i32
    });
    if requested != 0 {
        push(scene_fields::step(shell, scene, field, requested, false));
    }

    let mut typing = ui
        .memory(|m| m.data.get_temp::<Typing>(id))
        .unwrap_or_default();
    if response.has_focus() {
        keys(ui, shell, scene, field, id, &mut typing, &mut push);
    } else {
        typing = Typing::default();
    }
    ui.memory_mut(|m| m.data.insert_temp(id, typing.clone()));

    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(
        rect.shrink(0.5),
        theme.sizes.corner,
        if view.own { c.control.0 } else { c.lane.0 },
    );
    let shown = typing.0.as_deref().unwrap_or(&view.text);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        shown,
        theme.small_font(),
        if view.own || typing.0.is_some() {
            c.text.0
        } else {
            c.text_dim.0
        },
    );
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    let accessible = format!("{label} {}", name(field));
    response.widget_info(|| WidgetInfo::labeled(egui::WidgetType::Other, true, &accessible));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::SpinButton);
        node.set_label(accessible);
        node.set_value(view.text.clone());
        if let Ok(number) = view.text.parse::<f64>() {
            node.set_numeric_value(number);
        }
        node.set_description(if view.own {
            "Set for this scene.".to_owned()
        } else {
            "Off: this scene leaves the set's value as it is.".to_owned()
        });
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
        node.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
    // Spec: session-context-menus R3. A right-click opens Return to Default.
    let mut menu_events = Vec::new();
    crate::session_menus::SessionMenus::default().scene_field(
        &response,
        shell,
        scene,
        field,
        &mut menu_events,
    );
    events.extend(menu_events);
}

/// The keys a focused field takes.
fn keys(
    ui: &mut Ui,
    shell: &Shell,
    scene: SceneId,
    field: SceneField,
    id: Id,
    typing: &mut Typing,
    push: &mut impl FnMut(Option<Invocation>),
) {
    let editing = typing.0.is_some();
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            EventFilter {
                vertical_arrows: true,
                escape: editing,
                ..Default::default()
            },
        );
    });
    let events: Vec<Event> = ui.input_mut(|i| {
        let mut taken = Vec::new();
        i.events.retain(|event| {
            let ours = match event {
                Event::Text(text) => text.chars().all(|c| c.is_ascii_digit() || ".,".contains(c)),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    !modifiers.command
                        && !modifiers.ctrl
                        && (matches!(
                            key,
                            Key::Backspace | Key::Delete | Key::ArrowUp | Key::ArrowDown
                        ) || (editing && matches!(key, Key::Enter | Key::Escape)))
                }
                _ => false,
            };
            if ours {
                taken.push(event.clone());
            }
            !ours
        });
        taken
    });
    for event in events {
        match event {
            Event::Text(text) => {
                let typed = typing.0.get_or_insert_with(String::new);
                for c in text.chars() {
                    if typed.len() < MAX_TYPED && (field == SceneField::Tempo || c.is_ascii_digit())
                    {
                        typed.push(c);
                    }
                }
            }
            Event::Key { key, modifiers, .. } => match key {
                Key::Backspace => {
                    if let Some(typed) = typing.0.as_mut() {
                        typed.pop();
                    }
                }
                Key::Delete => {
                    typing.0 = None;
                    push(scene_fields::off(shell, scene, field));
                }
                Key::ArrowUp | Key::ArrowDown => {
                    typing.0 = None;
                    let steps = if key == Key::ArrowUp { 1 } else { -1 };
                    push(scene_fields::step(
                        shell,
                        scene,
                        field,
                        steps,
                        modifiers.shift,
                    ));
                }
                // Set what was typed; text that is no number leaves the field.
                Key::Enter => {
                    if let Some(text) = typing.0.take() {
                        push(scene_fields::typed(shell, scene, field, &text));
                    }
                }
                Key::Escape => typing.0 = None,
                _ => {}
            },
            _ => {}
        }
    }
}
