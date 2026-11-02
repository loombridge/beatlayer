//! The Clip View's Pitch and Time Utilities panel (WP-6.16; layout.md
//! L-CLIP-006; the pitch and time utilities spec, R1 to R15): a display, then
//! Transpose, Fit to Scale and Invert; Interval Size and Add Interval;
//! Stretch, ×2 and ÷2; Duration and Set Length; Humanize Amount and Humanize;
//! Reverse and Legato.
//!
//! Every control runs a command agents run (`note.transpose`,
//! `note.fit_to_scale`, `note.invert`, `note.add_interval`, `note.stretch`,
//! `note.set_length`, `note.humanize`, `note.reverse`, `note.legato`, and
//! `clip_view.utilities` for the panel's own values). A drag on Transpose,
//! Interval Size or Stretch is a gesture of its command: every frame runs it
//! from where the drag began, the drag is one undo step, and Esc puts the
//! notes back.

use std::f32::consts::PI;

use bl_shell::note_commands::{
    self as ids, DURATIONS, UtilitiesState, duration_length, stretch_factor,
};
use bl_shell::{Area, GesturePhase, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{Align2, Id, Key, Rect, Sense, Shape, Stroke, Ui, WidgetInfo, pos2, vec2};
use serde_json::{Value, json};

use crate::clip_view::key_name;
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::tool_panel::{CONTROL, CONTROLS_AT, ROW, button_named, end_gesture, info, typing};
use crate::widgets::{self, Choice, NO_LEDS};

/// Points of vertical drag for one step of a number field.
const POINTS_PER_STEP: f32 = 4.0;
/// How far the Stretch knob's drag goes from one end of its travel to the
/// other, in points.
const KNOB_TRAVEL: f32 = crate::device_panel::DRAG_TRAVEL;
/// The knob's face, and the height of the row that holds it.
const KNOB: f32 = 30.0;
/// The factor a step of the knob from the keyboard or a screen reader makes:
/// a tenth of the knob's decades.
const KNOB_STEP: f64 = 0.1;

/// What a [`field`] was moved by.
enum Moved {
    /// Dragged: `steps` from where the drag began, when the field showed
    /// `from`.
    Drag { from: f64, steps: i32 },
    /// A drag ended, having moved the field or not.
    Stopped { ran: bool },
    /// The arrow keys, or a screen reader's steps.
    Steps(i32),
    /// A whole number typed and Enter pressed.
    Typed(i64),
}

/// A whole number as typed: the digits, with a sign.
fn parse_whole(text: &str) -> Option<i64> {
    text.trim().trim_start_matches('+').parse().ok()
}

/// A factor as typed: `2` or `×2` for twice, `/2` or `÷2` for half.
fn parse_factor(text: &str) -> Option<f64> {
    let text = text.trim();
    let (divides, number) = match text.chars().next()? {
        '/' | '\u{f7}' => (true, &text[text.chars().next()?.len_utf8()..]),
        'x' | 'X' | '\u{d7}' => (false, &text[text.chars().next()?.len_utf8()..]),
        _ => (false, text),
    };
    let factor: f64 = number.trim().parse().ok().filter(|n: &f64| *n > 0.0)?;
    Some(if divides { 1.0 / factor } else { factor })
}

/// How a factor reads: `×2.00` above 1, `÷2.00` below (R8).
fn factor_text(factor: f64) -> String {
    if factor >= 1.0 {
        format!("\u{d7}{factor:.2}")
    } else {
        format!("\u{f7}{:.2}", 1.0 / factor)
    }
}

/// A number field of the panel: a box with its text, dragged up and down in
/// steps, stepped from the keyboard, and typed into. `fill` shades it as a
/// slider; `dim` draws it quiet; `drag_text` is what it adds to its text while
/// it is dragged.
#[expect(clippy::too_many_arguments, reason = "a field's whole context")]
fn field(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    name: &str,
    rect: Rect,
    (text, value): (&str, f64),
    (fill, dim): (Option<f32>, bool),
    drag_text: &dyn Fn(i32) -> String,
    events: &mut Vec<UiEvent>,
) -> Vec<Moved> {
    let id = Id::new(("utilities-field", salt));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    let mut moved = Vec::new();
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    // While the field has the keyboard, Delete doesn't delete notes.
    if response.gained_focus() {
        events.push(UiEvent::FocusValue(true));
    }
    if response.lost_focus() {
        events.push(UiEvent::FocusValue(false));
    }
    let drag = id.with("drag");
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(drag, (value, 0.0f32, 0i32, false)));
    }
    let mut dragging = None;
    if response.dragged() {
        let delta = response.drag_delta().y;
        let (from, pending, steps, ran) = ui
            .data(|d| d.get_temp::<(f64, f32, i32, bool)>(drag))
            .unwrap_or((value, 0.0, 0, false));
        let pending = pending - delta;
        let whole = (pending / POINTS_PER_STEP).trunc();
        let (pending, total) = (pending - whole * POINTS_PER_STEP, steps + whole as i32);
        let ran = ran || total != 0;
        ui.data_mut(|d| d.insert_temp(drag, (from, pending, total, ran)));
        if total != steps {
            moved.push(Moved::Drag { from, steps: total });
        }
        dragging = Some(total);
    }
    if response.drag_stopped() {
        let ran = ui
            .data_mut(|d| d.remove_temp::<(f64, f32, i32, bool)>(drag))
            .is_some_and(|(.., ran)| ran);
        moved.push(Moved::Stopped { ran });
    }
    let mut steps = ui.input(|i| {
        i.num_accesskit_action_requests(id, accesskit::Action::Increment) as i32
            - i.num_accesskit_action_requests(id, accesskit::Action::Decrement) as i32
    });
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        steps += ui.input(|i| {
            i32::from(i.key_pressed(Key::ArrowUp)) - i32::from(i.key_pressed(Key::ArrowDown))
        });
    }
    if steps != 0 {
        moved.push(Moved::Steps(steps));
    }
    // The keys go to the typed entry, not to shortcuts.
    ui.data_mut(|d| d.insert_temp(id.with("typed-field"), true));
    let typed = typing(
        ui,
        id.with("typing"),
        response.has_focus(),
        parse_whole,
        &mut |number| moved.push(Moved::Typed(number)),
    );
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if response.hovered() || response.dragged() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    if let Some(fill) = fill {
        let part = Rect::from_min_size(rect.min, vec2(rect.width() * fill, rect.height()));
        painter.rect_filled(part, theme.sizes.corner, c.accent.0.gamma_multiply(0.35));
    }
    let shown = match (&typed, dragging) {
        (Some(typed), _) => format!("{typed}_"),
        (None, Some(total)) if total != 0 => format!("{text} {}", drag_text(total)),
        _ => text.to_owned(),
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        &shown,
        theme.small_font(),
        if dim { c.text_dim.0 } else { c.text.0 },
    );
    crate::device_panel::focus_ring(ui, theme, rect, &response);
    response.widget_info(|| WidgetInfo::slider(true, value, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::SpinButton);
        node.set_label(name.to_owned());
        node.set_value(shown.clone());
        node.set_numeric_value(value);
        node.add_action(accesskit::Action::Increment);
        node.add_action(accesskit::Action::Decrement);
    });
    moved
}

/// What a control tells the Info View while the pointer is over it: its name
/// and the registered description of the command it runs (R1).
fn tell(ui: &Ui, shell: &Shell, rect: Rect, name: &str, command: &str) {
    if ui.rect_contains_pointer(rect) {
        let body = shell
            .registry()
            .get(command)
            .map(|spec| spec.description.to_string())
            .unwrap_or_default();
        info(ui, name, &body);
    }
}

/// A command run by a control as one call.
fn call(id: &str, args: Value) -> UiEvent {
    UiEvent::Invoke(Invocation::new(id, args))
}

/// One frame of a drag that edits the notes.
fn frame(id: &str, args: Value) -> UiEvent {
    UiEvent::Gesture {
        phase: GesturePhase::Update,
        invocation: Some(Invocation::new(id, args)),
    }
}

/// A step of the scale or a semitone, as the panel's fields read it.
fn unit(state: &UtilitiesState) -> &'static str {
    if state.scale.is_some() { "sd" } else { "st" }
}

/// The range of keys Transpose shows: one name when the notes are at one key
/// (R4).
fn range_text(state: &UtilitiesState) -> String {
    match (state.lowest, state.highest) {
        (Some(low), Some(high)) if low == high => key_name(low),
        (Some(low), Some(high)) => format!("{} - {}", key_name(low), key_name(high)),
        _ => String::new(),
    }
}

/// An offset as the fields show it while they are dragged: `+5 sd`.
fn offset_text(steps: i32, unit: &str) -> String {
    format!("{steps:+} {unit}")
}

/// A control's name at the left of its row, `y` down the panel.
fn label(ui: &Ui, theme: &Theme, rect: Rect, y: f32, text: &str) {
    ui.painter().with_clip_rect(rect).text(
        pos2(rect.left() + 6.0, y + CONTROL.y / 2.0),
        Align2::LEFT_CENTER,
        text,
        theme.small_font(),
        theme.colors.text_dim.0,
    );
}

/// Draws the panel's body in `rect`.
pub fn show(ui: &mut Ui, shell: &Shell, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
    let doc = shell.editor().document();
    let state = bl_shell::clip_view::utilities_state(doc, shell.clip_view(), shell.arrangement());
    let grid = shell.arrangement().grid_ticks(doc);
    let selected = !shell
        .clip_view()
        .selected_notes(doc, shell.arrangement())
        .is_empty();
    let acts = selected || state.time_range;
    let c = &theme.colors;
    let left = rect.left() + 6.0;
    let width = rect.width() - 12.0;
    let at = rect.left() + CONTROLS_AT;
    let mut y = rect.top();
    let half = (width - 6.0) / 2.0;
    let wide = |y: f32| Rect::from_min_size(pos2(left, y), vec2(width, CONTROL.y + 2.0));
    let left_half = |y: f32| Rect::from_min_size(pos2(left, y), vec2(half, CONTROL.y + 2.0));
    let right_half =
        |y: f32| Rect::from_min_size(pos2(left + half + 6.0, y), vec2(half, CONTROL.y + 2.0));

    // The display: the clip's scale, and nothing while Scale Mode is off (R1).
    let display = Rect::from_min_size(pos2(left, y), vec2(width, CONTROL.y));
    let shown = state.scale.clone().unwrap_or_default();
    let response = ui.interact(display, Id::new("utilities-display"), Sense::hover());
    ui.painter().rect_filled(
        display,
        theme.sizes.corner,
        c.control.0.gamma_multiply(0.55),
    );
    ui.painter().text(
        display.center(),
        Align2::CENTER_CENTER,
        &shown,
        theme.small_font(),
        c.text.0,
    );
    response.widget_info(|| WidgetInfo::labeled(egui::WidgetType::Label, false, &shown));
    y += ROW;

    // Transpose (R4).
    label(ui, theme, rect, y, "Transpose");
    let field_at = Rect::from_min_size(pos2(at, y), CONTROL);
    let text = range_text(&state);
    let steps_unit = unit(&state);
    for moved in field(
        ui,
        theme,
        "transpose",
        "Transpose",
        field_at,
        (&text, 0.0),
        (None, !acts),
        &|steps| offset_text(steps, steps_unit),
        events,
    ) {
        match moved {
            Moved::Drag { steps, .. } if acts => {
                events.push(frame(ids::TRANSPOSE, json!({ "by": steps })));
            }
            Moved::Stopped { ran } if acts => end_gesture(ui, ran, events),
            Moved::Steps(steps) if acts => {
                events.push(call(ids::TRANSPOSE, json!({ "by": steps })));
            }
            Moved::Typed(number) if acts => {
                events.push(call(ids::TRANSPOSE, json!({ "by": number })));
            }
            _ => {}
        }
    }
    tell(ui, shell, field_at, "Transpose", ids::TRANSPOSE);
    y += ROW;

    // Fit to Scale and Invert (R5, R6).
    for (rect, text, command, enabled) in [
        (
            left_half(y),
            "Fit to Scale",
            ids::FIT_TO_SCALE,
            state.fit_to_scale,
        ),
        (right_half(y), "Invert", ids::INVERT, true),
    ] {
        if button_named(ui, theme, command, rect, text, text, enabled) {
            events.push(call(command, json!({})));
        }
        tell(ui, shell, rect, text, command);
    }
    y += ROW + 2.0;

    // Interval Size and Add Interval (R7).
    label(ui, theme, rect, y, "Interval Size");
    let field_at = Rect::from_min_size(pos2(at, y), CONTROL);
    let live = Id::new("utilities-interval-live");
    let shown_interval = ui
        .data(|d| d.get_temp::<i32>(live))
        .unwrap_or(state.interval);
    let text = format!(
        "{} {steps_unit}",
        if shown_interval == 0 {
            "0".to_owned()
        } else {
            format!("{shown_interval:+}")
        }
    );
    let (low, high) = state.interval_range;
    let held = |by: i32| by.clamp(low, high);
    for moved in field(
        ui,
        theme,
        "interval",
        "Interval Size",
        field_at,
        (&text, f64::from(shown_interval)),
        (None, false),
        &|steps| offset_text(steps, steps_unit),
        events,
    ) {
        match moved {
            Moved::Drag { from, steps } => {
                let by = held(from as i32 + steps);
                ui.data_mut(|d| d.insert_temp(live, by));
                // With notes selected, moving it adds their copies at once.
                events.push(if selected {
                    frame(ids::ADD_INTERVAL, json!({ "by": by }))
                } else {
                    call(ids::UTILITIES, json!({ "interval": by }))
                });
            }
            Moved::Stopped { ran } => {
                let by = ui.data_mut(|d| d.remove_temp::<i32>(live));
                if selected {
                    let escaped = ui.input(|i| i.key_pressed(Key::Escape));
                    end_gesture(ui, ran, events);
                    if let Some(by) = by.filter(|_| ran && !escaped) {
                        events.push(call(ids::UTILITIES, json!({ "interval": by })));
                    }
                }
            }
            Moved::Steps(steps) => {
                let by = held(state.interval + steps);
                if selected {
                    events.push(frame(ids::ADD_INTERVAL, json!({ "by": by })));
                    events.push(UiEvent::Gesture {
                        phase: GesturePhase::Commit,
                        invocation: None,
                    });
                }
                events.push(call(ids::UTILITIES, json!({ "interval": by })));
            }
            // Enter adds the interval at once, and the field goes back to 0.
            Moved::Typed(number) => {
                let by = held(i32::try_from(number).unwrap_or(0));
                events.push(call(ids::ADD_INTERVAL, json!({ "by": by })));
                events.push(call(ids::UTILITIES, json!({ "interval": 0 })));
            }
        }
    }
    tell(ui, shell, field_at, "Interval Size", ids::ADD_INTERVAL);
    y += ROW;
    let add = wide(y);
    if button_named(
        ui,
        theme,
        ids::ADD_INTERVAL,
        add,
        "Add Interval",
        "Add Interval",
        true,
    ) {
        events.push(call(ids::ADD_INTERVAL, json!({})));
    }
    tell(ui, shell, add, "Add Interval", ids::ADD_INTERVAL);
    y += ROW + 2.0;

    // Stretch, ×2 and ÷2 (R8, R9).
    label(ui, theme, rect, y + KNOB / 2.0 - CONTROL.y / 2.0, "Stretch");
    knob(
        ui,
        shell,
        theme,
        Rect::from_min_size(pos2(at, y), vec2(KNOB + 14.0, KNOB)),
        &state,
        acts,
        events,
    );
    let buttons = at + KNOB + 18.0;
    let each = (rect.right() - 6.0 - buttons - 2.0) / 2.0;
    for (index, (text, name, factor)) in [("\u{d7}2", "Double", 2.0), ("\u{f7}2", "Halve", 0.5)]
        .into_iter()
        .enumerate()
    {
        let rect = Rect::from_min_size(
            pos2(
                buttons + (each + 2.0) * index as f32,
                y + KNOB / 2.0 - CONTROL.y / 2.0,
            ),
            vec2(each, CONTROL.y + 2.0),
        );
        if button_named(ui, theme, name, rect, text, name, true) {
            events.push(call(
                ids::STRETCH,
                json!({ "factor": factor, "markers": true }),
            ));
        }
        tell(ui, shell, rect, name, ids::STRETCH);
    }
    y += KNOB + 4.0;

    // Duration and Set Length (R10).
    label(ui, theme, rect, y, "Duration");
    let chooser_at = Rect::from_min_size(pos2(at, y), CONTROL);
    let choices: Vec<Choice<String>> = DURATIONS
        .iter()
        .map(|name| Choice {
            label: match *name {
                "time_range" => "Fit to Time Range".to_owned(),
                "grid" => "Grid".to_owned(),
                other => other.to_owned(),
            },
            value: (*name).to_owned(),
            leds: NO_LEDS,
        })
        .collect();
    crate::focus::register(
        ui.ctx(),
        Id::new(("chooser", "utilities-duration")),
        Area::Detail,
        None,
    );
    if let Some(name) = widgets::chooser(
        ui,
        theme,
        "utilities-duration",
        "Duration",
        chooser_at,
        &choices,
        Some(&state.duration),
        true,
    ) {
        events.push(call(ids::UTILITIES, json!({ "duration": name })));
    }
    tell(ui, shell, chooser_at, "Duration", ids::SET_LENGTH);
    y += ROW;
    let length = wide(y);
    let by_range = state.duration == "time_range";
    if button_named(
        ui,
        theme,
        ids::SET_LENGTH,
        length,
        "Set Length",
        "Set Length",
        !by_range || state.time_range,
    ) {
        events.push(call(
            ids::SET_LENGTH,
            match duration_length(&state.duration, grid) {
                Some(length) => json!({ "length": length }),
                None => json!({ "to_time_range": true }),
            },
        ));
    }
    tell(ui, shell, length, "Set Length", ids::SET_LENGTH);
    y += ROW + 2.0;

    // Humanize Amount and Humanize (R11).
    label(ui, theme, rect, y, "Amount");
    let field_at = Rect::from_min_size(pos2(at, y), CONTROL);
    let amount = i32::from(state.humanize_amount);
    let text = format!("{amount} %");
    for moved in field(
        ui,
        theme,
        "humanize",
        "Humanize Amount",
        field_at,
        (&text, f64::from(amount)),
        (Some(amount as f32 / 100.0), false),
        &|_| String::new(),
        events,
    ) {
        match moved {
            Moved::Drag { from, steps } => {
                events.push(call(
                    ids::UTILITIES,
                    json!({ "humanize_amount": (from as i32 + steps).clamp(0, 100) }),
                ));
            }
            Moved::Steps(steps) => events.push(call(
                ids::UTILITIES,
                json!({ "humanize_amount": (amount + steps).clamp(0, 100) }),
            )),
            // Enter sets the amount and runs Humanize.
            Moved::Typed(number) => {
                events.push(call(
                    ids::UTILITIES,
                    json!({ "humanize_amount": number.clamp(0, 100) }),
                ));
                events.push(call(ids::HUMANIZE, json!({})));
            }
            Moved::Stopped { .. } => {}
        }
    }
    tell(ui, shell, field_at, "Humanize Amount", ids::HUMANIZE);
    y += ROW;
    let humanize = wide(y);
    if button_named(
        ui,
        theme,
        ids::HUMANIZE,
        humanize,
        "Humanize",
        "Humanize",
        true,
    ) {
        events.push(call(ids::HUMANIZE, json!({})));
    }
    tell(ui, shell, humanize, "Humanize", ids::HUMANIZE);
    y += ROW + 2.0;

    // Reverse and Legato (R12, R13).
    for (rect, text, command) in [
        (left_half(y), "Reverse", ids::REVERSE),
        (right_half(y), "Legato", ids::LEGATO),
    ] {
        if button_named(ui, theme, command, rect, text, text, true) {
            events.push(call(command, json!({})));
        }
        tell(ui, shell, rect, text, command);
    }
}

/// The Stretch knob (R8): dragged, it stretches the notes by the factor its
/// position gives, as one undo step; let go, it goes back to ×1. Typing a
/// factor and pressing Enter stretches by it, the whole clip with nothing
/// selected.
fn knob(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    state: &UtilitiesState,
    acts: bool,
    events: &mut Vec<UiEvent>,
) {
    let id = Id::new(("utilities-field", "stretch"));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    if response.gained_focus() {
        events.push(UiEvent::FocusValue(true));
    }
    if response.lost_focus() {
        events.push(UiEvent::FocusValue(false));
    }
    let drag = id.with("drag");
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(drag, (state.stretch, false)));
    }
    let mut position = state.stretch;
    if response.dragged() {
        let (from, ran) = ui
            .data(|d| d.get_temp::<(f64, bool)>(drag))
            .unwrap_or((state.stretch, false));
        let fine = ui.input(|i| i.modifiers.shift);
        let travel = KNOB_TRAVEL * if fine { crate::device_panel::FINE } else { 1.0 };
        let moved = f64::from(-response.drag_delta().y / travel);
        let next = (ui
            .data(|d| d.get_temp::<f64>(drag.with("at")))
            .unwrap_or(from)
            + moved)
            .clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(drag.with("at"), next));
        position = next;
        // With nothing to stretch, the knob only turns.
        let ran = ran || (acts && moved != 0.0);
        ui.data_mut(|d| d.insert_temp(drag, (from, ran)));
        if acts && moved != 0.0 {
            events.push(frame(
                ids::STRETCH,
                json!({ "factor": stretch_factor(next) }),
            ));
        }
    }
    if response.drag_stopped() {
        let (_, ran) = ui
            .data_mut(|d| d.remove_temp::<(f64, bool)>(drag))
            .unwrap_or((state.stretch, false));
        ui.data_mut(|d| d.remove_temp::<f64>(drag.with("at")));
        if acts {
            end_gesture(ui, ran, events);
        }
    }
    let mut steps = ui.input(|i| {
        i.num_accesskit_action_requests(id, accesskit::Action::Increment) as i32
            - i.num_accesskit_action_requests(id, accesskit::Action::Decrement) as i32
    });
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        steps += ui.input(|i| {
            i32::from(i.key_pressed(Key::ArrowUp)) - i32::from(i.key_pressed(Key::ArrowDown))
        });
    }
    if steps != 0 {
        events.push(call(
            ids::STRETCH,
            json!({ "factor": 10f64.powf(KNOB_STEP * f64::from(steps)) }),
        ));
    }
    ui.data_mut(|d| d.insert_temp(id.with("typed-field"), true));
    let typed = typing(
        ui,
        id.with("typing"),
        response.has_focus(),
        parse_factor,
        &mut |factor| {
            events.push(call(
                ids::STRETCH,
                json!({ "factor": factor.clamp(0.1, 10.0) }),
            ));
        },
    );
    // Drawn as the device framework's knobs are: an arc over 300 degrees, the
    // value under it.
    let c = &theme.colors;
    let face = Rect::from_min_size(rect.min, vec2(rect.width(), rect.height() - 11.0));
    let centre = face.center();
    let radius = (face.height().min(face.width()) / 2.0 - 2.0).max(4.0);
    let (from, sweep) = (PI * 0.5 + PI / 6.0, PI * 5.0 / 3.0);
    let arc = |to: f32| -> Vec<egui::Pos2> {
        (0..=32)
            .map(|i| {
                let a = from + sweep * to * i as f32 / 32.0;
                centre + vec2(a.cos(), a.sin()) * radius
            })
            .collect()
    };
    let painter = ui.painter();
    painter.add(Shape::line(arc(1.0), Stroke::new(2.0, c.control.0)));
    let n = position as f32;
    if n > 0.0 {
        painter.add(Shape::line(arc(n), Stroke::new(2.0, c.accent.0)));
    }
    let a = from + sweep * n;
    painter.line_segment(
        [centre, centre + vec2(a.cos(), a.sin()) * radius],
        Stroke::new(1.5, c.text.0),
    );
    let text = typed.map_or_else(
        || factor_text(stretch_factor(position)),
        |typed| format!("{typed}_"),
    );
    painter.text(
        pos2(rect.center().x, rect.bottom() - 5.0),
        Align2::CENTER_CENTER,
        &text,
        theme.small_font(),
        c.text.0,
    );
    crate::device_panel::focus_ring(ui, theme, rect, &response);
    response.widget_info(|| WidgetInfo::slider(true, position, "Stretch Factor"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Slider);
        node.set_label("Stretch Factor".to_owned());
        node.set_value(text.clone());
        node.set_numeric_value(stretch_factor(position));
        node.set_min_numeric_value(0.1);
        node.set_max_numeric_value(10.0);
        node.add_action(accesskit::Action::Increment);
        node.add_action(accesskit::Action::Decrement);
    });
    tell(ui, shell, rect, "Stretch Factor", ids::STRETCH);
}
