//! The Articulation Map's list of articulations (WP-6.47; the articulation
//! map spec, R15): one row each with its number, name, trigger, outputs and
//! channel, the current one lit. Clicking a row's number makes that
//! articulation current, a double-click on a name renames it in place, and
//! each row's Learn button takes the next note-on or control change the
//! device hears as that row's trigger.
//!
//! The look is Beatlayer's own.

use egui::accesskit::Role;
use egui::{Align2, Id, Key, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use bl_devices::articulation_map::{ARTICULATION, LIST_CELL};
use bl_model::Articulation;
use bl_shell::Invocation;
use bl_shell::device_commands::{DeviceFrame, DevicePanel};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How tall a row is, in points.
const ROW: f32 = 15.0;
/// The width of the number column, in points.
const NUMBER: f32 = 16.0;
/// The width of the name column, in points.
const NAME: f32 = 78.0;
/// The width of the trigger column, in points.
const TRIGGER: f32 = 60.0;
/// The width of the Learn button, in points.
const LEARN: f32 = 38.0;

/// An articulation being renamed in place.
#[derive(Clone, Debug, Default)]
pub struct Renaming {
    /// The row being renamed, from 0.
    pub row: usize,
    /// Its name as it is typed.
    pub text: String,
    /// Whether the edit has the keyboard.
    pub focused: bool,
}

/// The ID of a device's list, where its scroll offset and the articulation
/// it showed last are kept.
fn list_id(device: bl_model::DeviceId) -> Id {
    Id::new(("articulation-list", device))
}

/// Draws the Articulation Map's list in `cell`, and turns what the person
/// does into commands. `current` is the articulation that is current now,
/// from 0.
pub fn cell(
    ui: &mut egui::Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    current: usize,
    cell: Rect,
    renaming: &mut Option<Renaming>,
    events: &mut Vec<UiEvent>,
) {
    let Some(panel) = frame.panel.as_ref() else {
        return;
    };
    let inner = cell.shrink(3.0);
    let shown = ((inner.height() / ROW).floor() as usize).max(1);
    let id = list_id(frame.id);
    // The list scrolls: the wheel over it moves the offset, and the
    // current articulation is brought into view when it changes. The offset
    // it was scrolled to, and the articulation it showed last, are kept
    // beside it.
    let (last, mut offset) = ui
        .data(|d| d.get_temp::<(usize, usize)>(id))
        .unwrap_or((usize::MAX, 0));
    if last != current && (current < offset || current >= offset + shown) {
        offset = current.saturating_sub(shown / 2);
    }
    let max = panel.articulations.len().saturating_sub(shown);
    offset = offset.min(max);
    let response = ui.interact(cell, id, Sense::hover());
    if response.hovered() {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel > 0.0 {
            offset = (offset + 1).min(max);
        } else if wheel < 0.0 {
            offset = offset.saturating_sub(1);
        }
    }
    ui.data_mut(|d| d.insert_temp(id, (current, offset)));
    for (index, articulation) in panel
        .articulations
        .iter()
        .enumerate()
        .skip(offset)
        .take(shown)
    {
        let top = inner.top() + (index - offset) as f32 * ROW;
        let row = Rect::from_min_size(pos2(inner.left(), top), vec2(inner.width(), ROW));
        row_of(
            ui,
            theme,
            frame,
            panel,
            articulation,
            index,
            current,
            row,
            renaming,
            events,
        );
    }
}

/// Draws one articulation's row.
#[expect(clippy::too_many_arguments, reason = "the row's inputs")]
fn row_of(
    ui: &mut egui::Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    articulation: &Articulation,
    index: usize,
    current: usize,
    row: Rect,
    renaming: &mut Option<Renaming>,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let lit = current == index;
    let ink = if lit { c.accent.0 } else { c.text.0 };
    // The number: clicking it makes the articulation current.
    let number = Rect::from_min_size(row.min, vec2(NUMBER, ROW));
    let response = ui.interact(
        number,
        Id::new(("articulation-number", frame.id, index)),
        Sense::click(),
    );
    if response.clicked() {
        events.push(UiEvent::BeginNamedGesture(
            "Change \"Articulation\"".to_owned(),
        ));
        events.push(UiEvent::Invoke(Invocation::new(
            "device.set_param",
            json!({ "device": frame.id, "param": ARTICULATION.0, "value": index + 1 }),
        )));
        events.push(UiEvent::EndGesture);
    }
    ui.painter_at(row).text(
        pos2(number.left() + 2.0, number.center().y),
        Align2::LEFT_CENTER,
        format!("{}", index + 1),
        theme.small_font(),
        ink,
    );
    // The name: a double-click renames it in place.
    let name = Rect::from_min_size(pos2(row.left() + NUMBER, row.top()), vec2(NAME, ROW));
    let mut done = None;
    match renaming.as_mut().filter(|r| r.row == index) {
        Some(renaming) => {
            let id = Id::new(("articulation-name", frame.id, index));
            let edit = ui.put(
                name,
                egui::TextEdit::singleline(&mut renaming.text)
                    .id(id)
                    .font(theme.small_font())
                    .margin(vec2(2.0, 0.0)),
            );
            if !renaming.focused {
                edit.request_focus();
                renaming.focused = true;
            }
            let (enter, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if edit.lost_focus() || enter || escape {
                done = Some((
                    enter || (edit.lost_focus() && !escape),
                    renaming.text.clone(),
                ));
            }
        }
        None => {
            let response = ui.interact(
                name,
                Id::new(("articulation-name", frame.id, index)),
                Sense::click(),
            );
            if response.double_clicked() {
                *renaming = Some(Renaming {
                    row: index,
                    text: articulation.name.clone(),
                    focused: false,
                });
            }
            ui.painter_at(row).text(
                pos2(name.left() + 2.0, name.center().y),
                Align2::LEFT_CENTER,
                &articulation.name,
                theme.small_font(),
                ink,
            );
        }
    }
    if let Some((commit, text)) = done {
        if commit {
            events.push(UiEvent::Invoke(Invocation::new(
                "articulation.set",
                json!({
                    "device": frame.id,
                    "index": index,
                    "name": text,
                }),
            )));
        }
        *renaming = None;
    }
    // The trigger, the outputs and the channel.
    let trigger = Rect::from_min_size(
        pos2(row.left() + NUMBER + NAME, row.top()),
        vec2(TRIGGER, ROW),
    );
    ui.painter_at(row).text(
        pos2(trigger.left(), trigger.center().y),
        Align2::LEFT_CENTER,
        bl_devices::articulation_map::trigger_text(&articulation.trigger),
        theme.small_font(),
        if lit { c.text.0 } else { c.text_dim.0 },
    );
    let outputs = Rect::from_min_size(
        pos2(trigger.right(), row.top()),
        vec2(row.width() - NUMBER - NAME - TRIGGER - LEARN, ROW),
    );
    ui.painter_at(row).text(
        pos2(outputs.left() + 4.0, outputs.center().y),
        Align2::LEFT_CENTER,
        bl_devices::articulation_map::outputs_text(articulation),
        theme.small_font(),
        if lit { c.text.0 } else { c.text_dim.0 },
    );
    // The Learn button: while it is lit, the next note-on or control change
    // the device hears is that articulation's trigger.
    let learn = Rect::from_min_size(
        pos2(row.right() - LEARN, row.top() + 1.0),
        vec2(LEARN - 2.0, ROW - 2.0),
    );
    let learn_id = Id::new(("articulation-learn", frame.id, index));
    let response = ui.interact(learn, learn_id, Sense::click());
    crate::focus::register(ui.ctx(), learn_id, bl_shell::Area::Detail, None);
    let learning = panel.learning == Some(index as u8);
    let face = if learning {
        c.accent.0
    } else if response.hovered() {
        c.control_hover.0
    } else {
        c.control.0
    };
    ui.painter().rect_filled(learn, theme.sizes.corner, face);
    ui.painter().rect_stroke(
        learn,
        0.0,
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    ui.painter().text(
        learn.center(),
        Align2::CENTER_CENTER,
        "Learn",
        theme.small_font(),
        if learning { c.accent_text.0 } else { c.text.0 },
    );
    if response.clicked() {
        events.push(UiEvent::Invoke(Invocation::new(
            "articulation.learn",
            json!({ "device": frame.id, "index": index }),
        )));
    }
    ui.ctx().accesskit_node_builder(learn_id, |node| {
        node.set_role(Role::Button);
        node.set_label(format!("Learn articulation {}", index + 1));
        node.set_selected(learning);
    });
}

/// The list as a screen reader reads it: each articulation's number, name,
/// trigger, outputs and channel.
#[must_use]
pub fn list_text(articulations: &[bl_model::Articulation], current: usize) -> String {
    articulations
        .iter()
        .enumerate()
        .map(|(index, articulation)| {
            let channel = articulation.channel.map_or_else(
                || "no channel".to_owned(),
                |channel| format!("channel {channel}"),
            );
            format!(
                "{} {}{} {}, {}, {}",
                index + 1,
                articulation.name,
                if index == current { ", current," } else { "," },
                bl_devices::articulation_map::trigger_text(&articulation.trigger),
                bl_devices::articulation_map::outputs_text(articulation),
                channel
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Whether the cell named `name` shows the Articulation Map's list.
#[must_use]
pub fn is_list(name: &str) -> bool {
    name == LIST_CELL
}
