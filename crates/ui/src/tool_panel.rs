//! The Clip View's right panel group (WP-6.11; layout.md L-CLIP-003,
//! L-CLIP-006 and L-CLIP-007): the panel selector row, and the Transform and
//! Generate panels of a MIDI clip.
//!
//! A tool panel shows the Transformation or Generator Selector, the chosen
//! tool's controls drawn from its description, then its Auto switch, its
//! apply button and Reset. Every control runs a command agents run
//! (`midi_tool.choose`, `midi_tool.set`, `midi_tool.set_auto`,
//! `midi_tool.apply`, `midi_tool.reset`), and a drag on a value is a gesture
//! of `midi_tool.set`: one undo step named after the tool, and Esc puts the
//! notes back. The Pitch and Time Utilities panel comes with its own
//! package (WP-6.16) and is drawn empty. The rules are in the MIDI Tools
//! spec.

use bl_midi_tools::{DIVISION_NAMES, ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use bl_model::{ClipId, ParamDescriptor, ParamId, ParamUnit};
use bl_shell::midi_tool_commands::Panel;
use bl_shell::midi_tools::ToolView;
use bl_shell::{Area, GesturePhase, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{Align2, Id, Key, Rect, Sense, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::{InfoViewContent, UiEvent};
use crate::theme::Theme;
use crate::widgets::{self, Choice, NO_LEDS};

/// How wide the right panel group is (L-CLIP-003: x 424 to 588).
pub const WIDTH: f32 = crate::audio_panel::WIDTH;
/// The panel selector row's height (L-CLIP-003).
pub const SELECTOR: f32 = 16.0;
/// A control's size, and the step between rows.
const CONTROL: Vec2 = vec2(84.0, 14.0);
const ROW: f32 = 17.0;
/// Where the controls start, from the panel's left edge.
const CONTROLS_AT: f32 = 70.0;
/// How wide the Auto switch is.
const SWITCH: f32 = 40.0;
/// Points of vertical drag that move a control from one end of its range to
/// the other, as the device framework's knobs do.
const DRAG_TRAVEL: f32 = crate::device_panel::DRAG_TRAVEL;

/// The right group's place: beside the left group, whose right edge is at
/// `left`.
#[must_use]
pub fn panel_rect(view: Rect, left: f32) -> Rect {
    crate::audio_panel::panel_rect(view, left)
}

/// The ID of the group a panel is.
#[must_use]
pub fn group_id(panel: Panel) -> Id {
    Id::new(("clip-view-panel", panel as u8))
}

/// The name a panel's button has for assistive technology.
fn panel_name(panel: Panel, audio: bool) -> &'static str {
    match (panel, audio) {
        (Panel::Utilities, false) => "Pitch and Time Utilities",
        (Panel::Utilities, true) => "Audio Utilities",
        (Panel::Transform, _) => "MIDI Transformation Tools",
        (Panel::Generate, _) => "MIDI Generative Tools",
    }
}

/// The buttons of the panel selector row, in order: the panel each shows and
/// its short label.
fn buttons(audio: bool) -> &'static [(Panel, &'static str)] {
    if audio {
        &[
            (Panel::Utilities, "Audio Utilities"),
            (Panel::Transform, "Transform"),
        ]
    } else {
        &[
            (Panel::Utilities, "Utilities"),
            (Panel::Transform, "Transform"),
            (Panel::Generate, "Generate"),
        ]
    }
}

/// The right group's panel selector row (L-CLIP-003): a button for each
/// panel, the shown one lit. Clicking one runs `clip_view.show_panel`.
pub fn selector(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    audio: bool,
    shown: Panel,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let row = buttons(audio);
    let each = rect.width() / row.len() as f32;
    for (index, (panel, label)) in row.iter().enumerate() {
        let button = Rect::from_min_size(
            rect.min + vec2(each * index as f32, 0.0),
            vec2(each - 2.0, SELECTOR),
        );
        let id = Id::new(("clip-view-panel-selector", audio, index));
        let response = ui.interact(button, id, Sense::click());
        let is_shown = *panel == shown;
        ui.painter().rect_filled(
            button,
            theme.sizes.corner,
            if is_shown {
                c.control_pressed.0
            } else if response.hovered() {
                c.control_hover.0
            } else {
                c.control.0
            },
        );
        ui.painter().with_clip_rect(button).text(
            button.center(),
            Align2::CENTER_CENTER,
            label,
            theme.small_font(),
            if is_shown { c.text.0 } else { c.text_dim.0 },
        );
        let name = panel_name(*panel, audio);
        response
            .widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, is_shown, name));
        if response.clicked() && !is_shown {
            events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::midi_tool_commands::SHOW_PANEL,
                json!({ "panel": panel }),
            )));
        }
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
    }
}

/// What the Info View shows for the control the pointer is over, put there
/// by [`control_info`] and taken once a frame (the MIDI Tools spec, R7).
fn info_id() -> Id {
    Id::new("midi-tool-info")
}

/// Says what the pointer is over, for the Info View.
fn control_info(ui: &Ui, spec: &ParamSpec) {
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            info_id(),
            InfoViewContent {
                title: spec.name.clone(),
                body: spec.description.clone(),
                shortcut: None,
            },
        );
    });
}

/// The Info View's content for a tool control the pointer is over, if it is
/// over one.
pub(crate) fn take_info(ctx: &egui::Context) -> Option<InfoViewContent> {
    ctx.data_mut(|d| d.remove_temp::<InfoViewContent>(info_id()))
}

/// Draws the right group of a MIDI clip in `rect`: the selector row, and the
/// shown panel.
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    events: &mut Vec<UiEvent>,
) {
    // An Esc a frame took that the note editor didn't read (another tab
    // showed) would swallow the editor's next Esc.
    took_escape(ui.ctx());
    let shown = shell.clip_view().midi_tools().panel();
    let c = &theme.colors;
    ui.painter().rect_filled(rect, 0.0, c.panel.0);
    let group = ui.interact(rect, group_id(shown), Sense::hover());
    ui.ctx().accesskit_node_builder(group.id, |node| {
        node.set_role(Role::Group);
        node.set_label(panel_name(shown, false).to_owned());
        node.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
    selector(
        ui,
        theme,
        Rect::from_min_size(rect.min, vec2(rect.width(), SELECTOR)),
        false,
        shown,
        events,
    );
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + SELECTOR + 4.0), rect.max);
    match shown {
        // Pitch and Time Utilities come with WP-6.16.
        Panel::Utilities => {}
        Panel::Transform => tools(
            ui,
            shell,
            theme,
            body,
            clip,
            ToolKind::Transformation,
            events,
        ),
        Panel::Generate => tools(ui, shell, theme, body, clip, ToolKind::Generator, events),
    }
}

/// A button drawn in place: returns whether it was clicked.
fn button(ui: &mut Ui, theme: &Theme, salt: &str, rect: Rect, text: &str, enabled: bool) -> bool {
    let id = Id::new(("tool-button", salt));
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let c = &theme.colors;
    ui.painter().rect_filled(
        rect,
        theme.sizes.corner,
        if !enabled {
            c.control.0.gamma_multiply(0.55)
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        if enabled { c.text.0 } else { c.pending.0 },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    if enabled {
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
    }
    enabled
        && (response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
}

/// A tool panel's body: the selector, the chosen tool's controls, then Auto,
/// the apply button and Reset.
fn tools(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    kind: ToolKind,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    let track = doc.clip_track(clip);
    let tools = shell.clip_view().midi_tools();
    let registry = bl_midi_tools::Registry::native();
    let current = tools.chosen(track, kind);
    let left = rect.left() + 6.0;
    let mut y = rect.top();

    // The Transformation or Generator Selector (R2).
    let choices: Vec<Choice<String>> = registry
        .tools(kind)
        .map(|tool| Choice {
            label: tool.descriptor().name.clone(),
            value: tool.descriptor().id.clone(),
            leds: NO_LEDS,
        })
        .collect();
    let (salt, name) = match kind {
        ToolKind::Transformation => ("tool-selector-transformation", "Transformation Selector"),
        ToolKind::Generator => ("tool-selector-generator", "Generator Selector"),
    };
    let selected = current.map(|tool| tool.descriptor().id.clone());
    crate::focus::register(ui.ctx(), Id::new(("chooser", salt)), Area::Detail, None);
    if let Some(id) = widgets::chooser(
        ui,
        theme,
        salt,
        name,
        Rect::from_min_size(pos2(left, y), vec2(rect.width() - 12.0, CONTROL.y + 2.0)),
        &choices,
        selected.as_ref(),
        !choices.is_empty(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::midi_tool_commands::CHOOSE,
            json!({ "clip": clip, "tool": id }),
        )));
    }
    y += CONTROL.y + 8.0;
    let Some(tool) = current else {
        return;
    };
    let view = tools.view_of(tool);
    let descriptor = view.descriptor;
    y = parameters(ui, theme, rect, y, clip, &view, None, events);
    if descriptor.id == "lfo" {
        y = lfo_curve(ui, theme, rect, y, clip, &view.values, events);
    }
    y += 4.0;
    let name = |what: &str| format!("{} {what}", descriptor.name);
    // Auto (R2).
    ui.painter().text(
        pos2(left, y + CONTROL.y / 2.0),
        Align2::LEFT_CENTER,
        "Auto",
        theme.small_font(),
        theme.colors.text_dim.0,
    );
    let auto = format!("auto-{}", descriptor.id);
    crate::focus::register(
        ui.ctx(),
        Id::new(("switch", auto.as_str())),
        Area::Detail,
        None,
    );
    if let Some(on) = widgets::switch(
        ui,
        theme,
        &auto,
        &name("Auto"),
        Rect::from_min_size(pos2(rect.left() + CONTROLS_AT, y), vec2(SWITCH, CONTROL.y)),
        view.auto,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::midi_tool_commands::SET_AUTO,
            json!({ "clip": clip, "tool": descriptor.id, "auto": on }),
        )));
    }
    y += ROW + 2.0;
    // The apply button reads Transform or Generate; Reset beside it.
    let label = match kind {
        ToolKind::Transformation => "Transform",
        ToolKind::Generator => "Generate",
    };
    let half = (rect.width() - 18.0) / 2.0;
    if button(
        ui,
        theme,
        &format!("apply-{}", descriptor.id),
        Rect::from_min_size(pos2(left, y), vec2(half, CONTROL.y + 2.0)),
        label,
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::midi_tool_commands::APPLY,
            json!({ "clip": clip, "tool": descriptor.id }),
        )));
    }
    if button(
        ui,
        theme,
        &format!("reset-{}", descriptor.id),
        Rect::from_min_size(pos2(left + half + 6.0, y), vec2(half, CONTROL.y + 2.0)),
        "Reset",
        !view.at_defaults,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::midi_tool_commands::RESET,
            json!({ "clip": clip, "tool": descriptor.id }),
        )));
    }
}

/// The rows of a tool's parameters, from `y` down, each with its name and
/// control; `only` names the parameters to draw, all where it is `None`.
/// Returns where the next row goes.
#[expect(clippy::too_many_arguments, reason = "a panel's whole context")]
fn parameters(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    mut y: f32,
    clip: ClipId,
    view: &ToolView<'_>,
    only: Option<&[&str]>,
    events: &mut Vec<UiEvent>,
) -> f32 {
    let descriptor = view.descriptor;
    let left = rect.left() + 6.0;
    for (index, spec) in descriptor.parameters.iter().enumerate() {
        let Some(value) = view.values.get(index) else {
            continue;
        };
        if only.is_some_and(|only| !only.contains(&spec.id.as_str())) {
            continue;
        }
        ui.painter().with_clip_rect(rect).text(
            pos2(left, y + CONTROL.y / 2.0),
            Align2::LEFT_CENTER,
            &spec.name,
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        // Chop's Pattern and Emphasis are rows of toggles (R43).
        if descriptor.id == "chop" && matches!(value, ParamValue::Toggles(_)) {
            y += CONTROL.y + 2.0;
            toggles(ui, theme, rect, y, clip, view, index, events);
            y += TOGGLE + 4.0;
            continue;
        }
        let at = Rect::from_min_size(pos2(rect.left() + CONTROLS_AT, y), CONTROL);
        let shown = chop_gaps_text(view, spec);
        let enabled = available(descriptor, spec, &view.values);
        control(
            ui,
            theme,
            clip,
            descriptor,
            spec,
            value,
            Shown {
                at,
                text: shown,
                enabled,
            },
            events,
        );
        y += ROW;
    }
    y
}

/// LFO's curve display (the MIDI transformations spec, R43): the envelope its
/// settings write over a note of two periods, which a gesture can't set; and
/// with the Random wave, the Reseed button (R39) that runs
/// `midi_tool.reseed`. Returns where the next row goes.
fn lfo_curve(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    mut y: f32,
    clip: ClipId,
    values: &[ParamValue],
    events: &mut Vec<UiEvent>,
) -> f32 {
    let left = rect.left() + 6.0;
    let size = vec2(rect.width() - 12.0, 36.0);
    let curve = Rect::from_min_size(pos2(left, y + 2.0), size);
    let response = ui.interact(
        curve,
        Id::new(("tool-display", "lfo-curve")),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, false, "LFO curve"));
    let painter = ui.painter_at(curve);
    painter.rect_filled(
        curve,
        theme.sizes.corner,
        theme.colors.control.0.gamma_multiply(0.55),
    );
    let line = bl_midi_tools::lfo_curve(values)
        .iter()
        .map(|p| {
            pos2(
                curve.left() + p.x as f32 * curve.width(),
                curve.bottom() - p.y as f32 * curve.height(),
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        line,
        egui::Stroke::new(1.0, theme.colors.accent.0),
    ));
    y += size.y + 6.0;
    // Random is the fourth Shape Type.
    if matches!(values.get(1), Some(ParamValue::Choice(3))) {
        let half = (rect.width() - 18.0) / 2.0;
        if button(
            ui,
            theme,
            "reseed-lfo",
            Rect::from_min_size(pos2(left, y), vec2(half, CONTROL.y + 2.0)),
            "Reseed",
            true,
        ) {
            events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::midi_tool_commands::RESEED,
                json!({ "clip": clip }),
            )));
        }
        y += ROW + 2.0;
    }
    y
}

/// What a control is drawn with besides its value: where, a text standing
/// for the value, and whether it can be used.
struct Shown {
    at: Rect,
    text: Option<&'static str>,
    enabled: bool,
}

/// Whether a parameter is available: Quantize's Triplets are not with
/// Current Grid (the MIDI transformations spec, R18).
fn available(tool: &ToolDescriptor, spec: &ParamSpec, values: &[ParamValue]) -> bool {
    !(tool.id == "quantize"
        && spec.id == "triplets"
        && matches!(values.get(2), Some(ParamValue::Choice(0))))
}

/// Chop's Gaps shows a dash once a Pattern toggle has been clicked, until it
/// is set again (R8).
fn chop_gaps_text(view: &ToolView<'_>, spec: &ParamSpec) -> Option<&'static str> {
    if view.descriptor.id != "chop" || spec.id != "gaps" {
        return None;
    }
    let (
        Some(ParamValue::Whole(parts)),
        Some(ParamValue::Whole(gaps)),
        Some(ParamValue::Toggles(pattern)),
    ) = (view.values.first(), view.values.get(1), view.values.get(2))
    else {
        return None;
    };
    (!bl_midi_tools::pattern_follows_gaps(*gaps, *parts, pattern)).then_some("\u{2013}")
}

/// A pattern or emphasis toggle's size, and the step between them.
const TOGGLE: f32 = 9.0;

/// Chop's Pattern or Emphasis toggles (R8, R43): one for each of the first
/// `min(Parts, 16)` elements, on for a part or an emphasis. A click is a
/// `midi_tool.set` of the whole list. An element with Emphasis on keeps its
/// Pattern state and its Pattern toggle is unavailable.
#[expect(clippy::too_many_arguments, reason = "a control's whole context")]
fn toggles(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    y: f32,
    clip: ClipId,
    view: &ToolView<'_>,
    index: usize,
    events: &mut Vec<UiEvent>,
) {
    let descriptor = view.descriptor;
    let spec = &descriptor.parameters[index];
    let (Some(ParamValue::Whole(parts)), Some(ParamValue::Toggles(list))) =
        (view.values.first(), view.values.get(index))
    else {
        return;
    };
    let emphasis = match view.values.get(3) {
        Some(ParamValue::Toggles(emphasis)) if spec.id == "pattern" => emphasis.as_slice(),
        _ => &[],
    };
    let shown = usize::try_from(*parts).unwrap_or(2).min(list.len());
    let c = &theme.colors;
    for at in 0..shown {
        let box_ = Rect::from_min_size(
            pos2(rect.left() + 6.0 + (TOGGLE + 1.0) * at as f32, y),
            vec2(TOGGLE, TOGGLE),
        );
        let on = list[at];
        let enabled = !emphasis.get(at).copied().unwrap_or(false);
        let salt = format!("{}-{}-{at}", descriptor.id, spec.id);
        let id = Id::new(("tool-toggle", salt.as_str()));
        let response = ui.interact(
            box_,
            id,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        ui.painter().rect_filled(
            box_,
            theme.sizes.corner,
            match (on, enabled) {
                (true, true) => c.accent.0,
                (true, false) => c.accent.0.gamma_multiply(0.4),
                (false, true) => c.control.0,
                (false, false) => c.control.0.gamma_multiply(0.55),
            },
        );
        let name = format!("{} {}", spec.name, at + 1);
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, &name));
        if enabled {
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
        }
        if ui.rect_contains_pointer(box_) {
            control_info(ui, spec);
        }
        let pressed =
            response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
        if enabled && pressed {
            let mut next = list.clone();
            next[at] = !on;
            events.push(UiEvent::Invoke(set(clip, descriptor, spec, json!(next))));
        }
    }
}

/// An audio clip's Transform panel (the MIDI transformations spec, R22):
/// Quantize's Grid, Triplets and Amount, and the apply button, which runs
/// `clip.quantize_audio` with them. There is no Start, End or Auto.
pub fn show_audio_transform(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    events: &mut Vec<UiEvent>,
) {
    let group = ui.interact(rect, group_id(Panel::Transform), Sense::hover());
    ui.ctx().accesskit_node_builder(group.id, |node| {
        node.set_role(Role::Group);
        node.set_label(panel_name(Panel::Transform, true).to_owned());
    });
    let Some(tool) = bl_midi_tools::Registry::native().find("quantize") else {
        return;
    };
    let view = shell.clip_view().midi_tools().view_of(tool);
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + SELECTOR + 4.0), rect.max);
    let y = parameters(
        ui,
        theme,
        body,
        body.top(),
        clip,
        &view,
        Some(&["grid", "triplets", "amount"]),
        events,
    ) + 4.0;
    let half = (body.width() - 18.0) / 2.0;
    if button(
        ui,
        theme,
        "apply-audio-quantize",
        Rect::from_min_size(pos2(body.left() + 6.0, y), vec2(half, CONTROL.y + 2.0)),
        "Transform",
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::midi_tool_commands::APPLY,
            json!({ "clip": clip, "tool": "quantize" }),
        )));
    }
}

/// The `midi_tool.set` for one parameter.
fn set(clip: ClipId, tool: &ToolDescriptor, spec: &ParamSpec, value: Value) -> Invocation {
    Invocation::new(
        bl_shell::midi_tool_commands::SET,
        json!({ "clip": clip, "tool": tool.id, "values": { spec.id.clone(): value } }),
    )
}

/// A number-like parameter as the device framework describes one: the
/// framework's descriptor gives its range, its steps and how it reads.
fn framework_descriptor(spec: &ParamSpec) -> Option<(ParamDescriptor, f64)> {
    let id = ParamId(0);
    let unit = match spec.unit.as_deref() {
        Some("%") => ParamUnit::Percent,
        Some("st") => ParamUnit::Semitones,
        _ => ParamUnit::None,
    };
    let (descriptor, current): (ParamDescriptor, fn(&ParamValue) -> Option<f64>) = match &spec.kind
    {
        ParamKind::Number { min, max, decimals } => {
            let ParamValue::Number(default) = spec.default else {
                return None;
            };
            (
                ParamDescriptor::continuous(id, &spec.name, (*min, *max), default, unit)
                    .decimals(*decimals),
                |value| match value {
                    ParamValue::Number(n) => Some(*n),
                    _ => None,
                },
            )
        }
        ParamKind::Whole { min, max } => {
            let ParamValue::Whole(default) = spec.default else {
                return None;
            };
            (
                ParamDescriptor::integer(id, &spec.name, (*min, *max), default, unit),
                |value| match value {
                    ParamValue::Whole(n) => Some(*n as f64),
                    _ => None,
                },
            )
        }
        ParamKind::Pitch => {
            let ParamValue::Pitch(default) = spec.default else {
                return None;
            };
            (
                ParamDescriptor::integer(id, &spec.name, (0, 127), i64::from(default), unit),
                |value| match value {
                    ParamValue::Pitch(n) => Some(f64::from(*n)),
                    _ => None,
                },
            )
        }
        ParamKind::Division => {
            let ParamValue::Division(default) = spec.default else {
                return None;
            };
            (
                ParamDescriptor::choice(id, &spec.name, &DIVISION_NAMES, default),
                |value| match value {
                    ParamValue::Division(n) => Some(*n as f64),
                    _ => None,
                },
            )
        }
        _ => return None,
    };
    Some((descriptor, current(&spec.default)?))
}

/// A number as the JSON `midi_tool.set` takes for a parameter of this kind.
fn json_of(spec: &ParamSpec, value: f64) -> Value {
    match &spec.kind {
        ParamKind::Whole { .. } => json!(value.round() as i64),
        ParamKind::Pitch => json!(value.round() as u8),
        ParamKind::Division => json!(DIVISION_NAMES.get(value.round() as usize)),
        _ => json!(value),
    }
}

/// A value being dragged: where it started in its range, and how far the
/// pointer has moved it since, as a share of the travel.
#[derive(Clone, Copy, Debug, Default)]
struct Drag {
    start: f64,
    moved: f64,
    /// Whether a frame of the gesture has run.
    ran: bool,
}

fn escape_id() -> Id {
    Id::new("midi-tool-escape")
}

/// Whether a drag on a tool's control ended with Esc this frame, so the
/// note editor leaves its own Esc (deselect) alone. Taken once.
pub(crate) fn took_escape(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(escape_id()))
        .unwrap_or(false)
}

/// One parameter's control.
#[expect(clippy::too_many_arguments, reason = "a control's whole context")]
fn control(
    ui: &mut Ui,
    theme: &Theme,
    clip: ClipId,
    tool: &ToolDescriptor,
    spec: &ParamSpec,
    value: &ParamValue,
    shown: Shown,
    events: &mut Vec<UiEvent>,
) {
    let Shown {
        at: rect,
        text: shown_text,
        enabled,
    } = shown;
    let salt = format!("{}-{}", tool.id, spec.id);
    match (&spec.kind, value) {
        (ParamKind::Switch, ParamValue::Switch(on)) if !enabled => {
            // Unavailable: drawn dim and answering nothing.
            let id = Id::new(("switch", salt.as_str()));
            let response = ui.interact(rect, id, Sense::hover());
            ui.painter().rect_filled(
                rect,
                theme.sizes.corner,
                theme.colors.control.0.gamma_multiply(0.55),
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                if *on { "On" } else { "Off" },
                theme.font(),
                theme.colors.pending.0,
            );
            let name = spec.name.clone();
            response.widget_info(|| WidgetInfo::selected(WidgetType::Button, false, *on, &name));
        }
        (ParamKind::Switch, ParamValue::Switch(on)) => {
            crate::focus::register(
                ui.ctx(),
                Id::new(("switch", salt.as_str())),
                Area::Detail,
                None,
            );
            if let Some(on) = widgets::switch(ui, theme, &salt, &spec.name, rect, *on) {
                events.push(UiEvent::Invoke(set(clip, tool, spec, json!(on))));
            }
        }
        (ParamKind::Choice { options }, ParamValue::Choice(current)) => {
            let choices: Vec<Choice<usize>> = options
                .iter()
                .enumerate()
                .map(|(at, option)| Choice {
                    label: option.name.clone(),
                    value: at,
                    leds: NO_LEDS,
                })
                .collect();
            crate::focus::register(
                ui.ctx(),
                Id::new(("chooser", salt.as_str())),
                Area::Detail,
                None,
            );
            if let Some(chosen) = widgets::chooser(
                ui,
                theme,
                &salt,
                &spec.name,
                rect,
                &choices,
                Some(current),
                true,
            ) {
                events.push(UiEvent::Invoke(set(
                    clip,
                    tool,
                    spec,
                    json!(options[chosen].id),
                )));
            }
        }
        (ParamKind::PitchRange, ParamValue::PitchRange { low, high }) => {
            let half = (rect.width() - 4.0) / 2.0;
            for (index, (name, shown)) in [("low", *low), ("high", *high)].into_iter().enumerate() {
                let part = Rect::from_min_size(
                    rect.min + vec2((half + 4.0) * index as f32, 0.0),
                    vec2(half, rect.height()),
                );
                let id = format!("{salt}-{name}");
                crate::focus::register(
                    ui.ctx(),
                    Id::new(("number-box", id.as_str())),
                    Area::Detail,
                    None,
                );
                if let Some(next) = widgets::number_box(
                    ui,
                    theme,
                    &id,
                    &format!("{} {name}", spec.name),
                    part,
                    f64::from(shown),
                    (0.0, 127.0),
                    &|value, steps| value + f64::from(steps),
                    &shown.to_string(),
                ) {
                    let (low, high) = if name == "low" {
                        (next as u8, (*high).max(next as u8))
                    } else {
                        ((*low).min(next as u8), next as u8)
                    };
                    events.push(UiEvent::Invoke(set(
                        clip,
                        tool,
                        spec,
                        json!({ "low": low, "high": high }),
                    )));
                }
            }
        }
        _ => match framework_descriptor(spec).zip(number_of(value)) {
            Some(((descriptor, _), shown)) => {
                value_control(
                    ui,
                    theme,
                    clip,
                    tool,
                    spec,
                    &descriptor,
                    shown,
                    (rect, shown_text),
                    events,
                );
            }
            None => display_data(ui, theme, spec, rect),
        },
    }
    // The Info View shows the name and registered description of the control
    // under the pointer (R7).
    if ui.rect_contains_pointer(rect) {
        control_info(ui, spec);
    }
}

/// A number-like value as a number.
fn number_of(value: &ParamValue) -> Option<f64> {
    match value {
        ParamValue::Number(n) => Some(*n),
        ParamValue::Whole(n) => Some(*n as f64),
        ParamValue::Pitch(n) => Some(f64::from(*n)),
        ParamValue::Division(n) => Some(*n as f64),
        _ => None,
    }
}

/// A display's data (breakpoints, levels, toggles) or a tool's own
/// structured value: drawn as its name in a quiet box, since the tools that
/// have them bring their displays.
fn display_data(ui: &mut Ui, theme: &Theme, spec: &ParamSpec, rect: Rect) {
    let id = Id::new(("tool-display", &spec.id));
    let response = ui.interact(rect, id, Sense::hover());
    ui.painter().rect_filled(
        rect,
        theme.sizes.corner,
        theme.colors.control.0.gamma_multiply(0.55),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, false, &spec.name));
}

/// A drag, an arrow key, a double-click, and a screen reader's steps on a
/// number-like value, which the device framework's controls take (R7). A
/// drag is a gesture of `midi_tool.set`, so it is one undo step and Esc puts
/// the notes back.
#[expect(clippy::too_many_arguments, reason = "a control's whole context")]
fn value_control(
    ui: &mut Ui,
    theme: &Theme,
    clip: ClipId,
    tool: &ToolDescriptor,
    spec: &ParamSpec,
    descriptor: &ParamDescriptor,
    value: f64,
    (rect, shown_text): (Rect, Option<&str>),
    events: &mut Vec<UiEvent>,
) {
    let id = Id::new(("tool-value", &tool.id, &spec.id));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    let drag_id = id.with("drag");
    let invoke = |value: f64| set(clip, tool, spec, json_of(spec, descriptor.clamp(value)));
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    // While the control has the keyboard, Delete sets its default rather
    // than deleting notes (the device framework's R17).
    if response.gained_focus() {
        events.push(UiEvent::FocusValue(true));
    }
    if response.lost_focus() {
        events.push(UiEvent::FocusValue(false));
    }
    if response.drag_started() {
        ui.data_mut(|d| {
            d.insert_temp(
                drag_id,
                Drag {
                    start: descriptor.normalize(value),
                    ..Drag::default()
                },
            );
        });
    }
    if response.dragged() {
        let dy = response.drag_delta().y;
        let fine = ui.input(|i| i.modifiers.shift);
        let mut drag = ui
            .data_mut(|d| d.get_temp::<Drag>(drag_id))
            .unwrap_or_default();
        if dy != 0.0 {
            let travel = if fine { crate::device_panel::FINE } else { 1.0 };
            drag.moved += f64::from(-dy / DRAG_TRAVEL / travel);
            let next = descriptor.denormalize((drag.start + drag.moved).clamp(0.0, 1.0));
            if next != value || drag.ran {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Update,
                    invocation: Some(invoke(next)),
                });
                drag.ran = true;
            }
        }
        ui.data_mut(|d| d.insert_temp(drag_id, drag));
    }
    if response.drag_stopped() {
        let drag = ui
            .data_mut(|d| d.remove_temp::<Drag>(drag_id))
            .unwrap_or_default();
        // Esc ends a drag too (egui stops it as a release): the notes go
        // back as they were before it.
        let escaped = ui.input(|i| i.key_pressed(Key::Escape));
        if drag.ran {
            events.push(UiEvent::Gesture {
                phase: if escaped {
                    GesturePhase::Cancel
                } else {
                    GesturePhase::Commit
                },
                invocation: None,
            });
            if escaped {
                ui.ctx().data_mut(|d| d.insert_temp(escape_id(), true));
            }
        }
    }
    // Arrow keys and a screen reader's steps, and a double-click for the
    // default.
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
    let delete = response.has_focus()
        && ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
    if response.double_clicked() || delete {
        if descriptor.default != value {
            events.push(UiEvent::Invoke(invoke(descriptor.default)));
        }
    } else if steps != 0 {
        let unit = if descriptor.is_stepped() {
            1.0
        } else {
            (descriptor.max() - descriptor.min()) / 100.0
        };
        let next = descriptor.clamp(value + unit * f64::from(steps));
        if next != value {
            events.push(UiEvent::Invoke(invoke(next)));
        }
    }
    // The control: a box with the value centred and a fill showing where it
    // sits in its range.
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
    let fill = Rect::from_min_size(
        rect.min,
        vec2(
            rect.width() * descriptor.normalize(value) as f32,
            rect.height(),
        ),
    );
    painter.rect_filled(fill, theme.sizes.corner, c.accent.0.gamma_multiply(0.35));
    let text = match (&spec.unit, shown_text) {
        (_, Some(text)) => text.to_owned(),
        (Some(unit), None) if !matches!(unit.as_str(), "%" | "st") => {
            format!("{} {unit}", descriptor.format(value))
        }
        _ => descriptor.format(value),
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        &text,
        theme.small_font(),
        c.text.0,
    );
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            egui::Stroke::new(1.5, c.focus.0),
            egui::StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::slider(true, value, &spec.name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::SpinButton);
        node.set_label(spec.name.clone());
        node.set_value(text);
        node.set_numeric_value(value);
        node.set_min_numeric_value(descriptor.min());
        node.set_max_numeric_value(descriptor.max());
        node.add_action(accesskit::Action::Increment);
        node.add_action(accesskit::Action::Decrement);
        if !spec.description.is_empty() {
            node.set_description(spec.description.clone());
        }
    });
}
