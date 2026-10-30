//! The Clip View's Launch Properties panel (WP-3.9; layout.md L-CLIP-003
//! and L-CLIP-005): the left group's second panel, shown for a Session clip
//! only, in the place of Main Properties (WP-6.55), to the left of the
//! editor.
//!
//! Launch mode, Legato, Quantize and Velocity edit the clip through
//! `clip.set_launch_mode`, `clip.set_legato`, `clip.set_launch_quantization`
//! and `clip.set_velocity_amount`, the commands agents run, and read the
//! clip's saved launch settings back. The follow-action controls above them
//! edit through `clip.set_follow_actions` (WP-3.10). The two nudge buttons
//! run `session.nudge` for the clip's slot, with -1 and 1 (WP-7.29). The
//! MIDI Bank, Sub and Pgm fields wait for an answer to what they do. The layout follows L-CLIP-005's order;
//! the reference didn't measure the tab, so the sizes are the manual's
//! reading, not pixel parity.

use bl_model::{
    ClipControl, ClipData, ClipId, Document, LaunchMode, LaunchQuantization, SceneId, TrackId,
};
use bl_shell::clip_properties_commands::Properties;
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{Align2, Id, Rect, Sense, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice, NO_LEDS};

/// How wide the left group of panels is: L-CLIP-003 puts both groups across
/// x 249 to 591, and the right group takes x 424 to 588.
pub const WIDTH: f32 = 174.0;
/// The panel selector row's height (L-CLIP-003).
pub(crate) const SELECTOR: f32 = 16.0;

/// How wide the left group of panels is in a view of this width.
#[must_use]
pub fn panel_width(view: Rect) -> f32 {
    WIDTH.min(view.width() / 2.0)
}
/// A control's size, and the step between rows.
const CONTROL: Vec2 = vec2(100.0, 17.0);
const ROW: f32 = 19.0;
/// Where the controls start, from the panel's left edge.
const CONTROLS_AT: f32 = 62.0;
/// Where the labels start, from the panel's left edge.
const LABELS_AT: f32 = 4.0;
/// A chance's or a nudge button's size.
const SMALL: Vec2 = vec2(54.0, 17.0);

/// The ID of the panel's group.
#[must_use]
pub fn launch_panel_id() -> Id {
    Id::new("launch-properties")
}

/// The panel group's place at the Clip View's left edge.
#[must_use]
pub fn panel_rect(view: Rect) -> Rect {
    Rect::from_min_size(view.min, vec2(panel_width(view), view.height()))
}

/// A control drawn in place that does nothing yet, announced as disabled
/// with the reason.
pub(crate) fn pending(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    rect: Rect,
    text: &str,
    reason: &str,
) {
    let id = Id::new(("launch-pending", salt));
    let response = ui.interact(rect, id, Sense::hover());
    let c = &theme.colors;
    ui.painter()
        .rect_filled(rect, theme.sizes.corner, c.control.0.gamma_multiply(0.55));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        c.pending.0,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, false, salt));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_description(reason.to_owned());
        node.set_value(text.to_owned());
    });
}

/// The Session slot that holds the clip `clip`.
fn slot_of(document: &Document, clip: ClipId) -> Option<(TrackId, SceneId)> {
    document.track_order().into_iter().find_map(|track| {
        let held = document
            .track(track)?
            .slots()
            .find_map(|(scene, held)| (held.id == clip).then_some(scene))?;
        Some((track, held))
    })
}

/// A nudge button (the launch quantisation spec, R36): live while the Clip
/// View shows a Session clip, whether or not it plays. A click runs
/// `session.nudge` with `steps` for the clip's slot.
fn nudge_button(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    (name, glyph, steps): (&str, &str, i32),
    slot: Option<(TrackId, SceneId)>,
    events: &mut Vec<UiEvent>,
) {
    let id = Id::new(("launch-nudge", name));
    let response = ui.interact(rect, id, Sense::click());
    let c = &theme.colors;
    ui.painter().rect_filled(
        rect,
        theme.sizes.corner,
        if response.is_pointer_button_down_on() {
            c.control_pressed.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme.small_font(),
        c.text.0,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_description(format!(
            "Moves the playing clip {} by one step of the global launch quantization.",
            if steps < 0 { "back" } else { "forward" }
        ));
    });
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if let (true, Some((track, scene))) = (response.clicked(), slot) {
        events.push(UiEvent::Invoke(Invocation::new(
            "session.nudge",
            json!({ "track": track, "scene": scene, "steps": steps }),
        )));
    }
    // Spec: map-modes R11. The map modes tint the nudge buttons.
    crate::mapping::clip_overlay(
        ui,
        theme,
        rect,
        id,
        if steps < 0 {
            ClipControl::NudgeDown
        } else {
            ClipControl::NudgeUp
        },
        events,
    );
}

/// The scrub control between the nudge buttons (L-CLIP-023, map modes R22),
/// which shows in MIDI Map Mode only: a slider named Scrub with no value of
/// its own, which a relative controller's increments nudge the playing clip
/// by (launch quantisation R35, R36). A click on it only selects it.
fn scrub_control(ui: &mut Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
    let id = Id::new(("launch-nudge", "Scrub"));
    let response = ui.interact(rect, id, Sense::hover());
    let c = &theme.colors;
    ui.painter()
        .rect_filled(rect, theme.sizes.corner, c.control.0);
    ui.painter().line_segment(
        [rect.left_center() + vec2(4.0, 0.0), rect.right_center() - vec2(4.0, 0.0)],
        egui::Stroke::new(1.0, c.text_dim.0),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Slider, true, "Scrub"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Slider);
        node.set_label("Scrub".to_owned());
        node.set_description(
            "Moves the playing clip by steps of the global launch quantization.".to_owned(),
        );
    });
    crate::mapping::clip_overlay(ui, theme, rect, id, ClipControl::Scrub, events);
}

/// Draws the panel selector row of the left group in `rect` (L-CLIP-003):
/// Main Properties, and for a Session clip Launch Properties beside it. The
/// one `shown` is lit; the other runs `clip_view.show_properties`.
pub fn selector(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    shown: Properties,
    session: bool,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let half = rect.width() / 2.0;
    crate::clip_panels::tab_list(
        ui,
        Id::new("launch-panel-tabs"),
        Rect::from_min_size(rect.min, vec2(rect.width(), SELECTOR)),
        "Properties Panels",
    );
    let mut buttons = Vec::new();
    let panels = [
        (Properties::Main, "Main Properties"),
        (Properties::Launch, "Launch Properties"),
    ];
    for (index, (panel, name)) in panels.into_iter().enumerate() {
        if panel == Properties::Launch && !session {
            continue;
        }
        let button = Rect::from_min_size(
            rect.min + vec2(half * index as f32, 0.0),
            vec2(half - 2.0, SELECTOR),
        );
        let id = crate::clip_panels::left_tab_id(index);
        let response = ui.interact(button, id, Sense::click());
        let lit = panel == shown;
        ui.painter().rect_filled(
            button,
            theme.sizes.corner,
            if lit {
                c.control_pressed.0
            } else if response.hovered() {
                c.control_hover.0
            } else {
                c.control.0
            },
        );
        ui.painter().text(
            button.center(),
            Align2::CENTER_CENTER,
            name,
            theme.small_font(),
            if lit { c.text.0 } else { c.text_dim.0 },
        );
        crate::clip_panels::tab(ui, &response, name, lit);
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        buttons.push(id);
        if response.clicked() && !lit {
            events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::clip_properties_commands::SHOW_PROPERTIES,
                json!({ "panel": panel }),
            )));
        }
    }
    crate::clip_panels::selector_keys(ui, &buttons);
}

/// Draws the Launch Properties panel for the Session clip `clip` in `rect`,
/// under the selector row.
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    events: &mut Vec<UiEvent>,
) {
    let Some(launch) = shell.editor().document().clip(clip).map(|c| c.launch) else {
        return;
    };
    let c = &theme.colors;
    let group = ui.interact(rect, launch_panel_id(), Sense::hover());
    ui.ctx().accesskit_node_builder(group.id, |node| {
        node.set_role(Role::TabPanel);
        node.set_label("Launch Properties".to_owned());
        node.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
    let left = rect.left() + LABELS_AT;
    if crate::main_panel::take_focus_request(ui.ctx()) {
        ui.memory_mut(|m| m.request_focus(Id::new(("switch", "follow-enabled"))));
    }
    // Spec: follow-actions R13. Follow Action, its actions A and B with
    // their chances, and the follow time, from the first row down.
    let document = shell.editor().document();
    let loop_off = !document.clip(clip).is_some_and(|clip| match &clip.data {
        ClipData::Midi(midi) => midi.looping,
        ClipData::Audio(audio) => audio.looping,
    });
    let mut y = crate::follow_controls::show(
        ui,
        theme,
        pos2(left, rect.top() + SELECTOR + 6.0),
        crate::follow_controls::Owner::Clip(clip, loop_off),
        &launch.follow,
        "",
        document.scenes().len(),
        events,
    );
    let painter = ui.painter_at(rect);
    let mut row = |label: &str| {
        let at = pos2(rect.left() + CONTROLS_AT, y);
        painter.text(
            pos2(left, y + CONTROL.y / 2.0),
            Align2::LEFT_CENTER,
            label,
            theme.small_font(),
            c.text_dim.0,
        );
        y += ROW;
        at
    };

    let command = |id: &str, field: &str, value: serde_json::Value| {
        UiEvent::Invoke(Invocation::new(id, json!({ "clip": clip, field: value })))
    };

    // Launch mode.
    let at = row("Launch Mode");
    let modes: Vec<_> = LaunchMode::ALL
        .into_iter()
        .map(|mode| Choice {
            label: mode.label().to_owned(),
            value: mode,
            leds: NO_LEDS,
        })
        .collect();
    crate::focus::register(
        ui.ctx(),
        Id::new(("chooser", "launch-mode")),
        Area::Detail,
        None,
    );
    if let Some(mode) = widgets::chooser(
        ui,
        theme,
        "launch-mode",
        "Launch Mode",
        Rect::from_min_size(at, CONTROL),
        &modes,
        Some(&launch.mode),
        true,
    ) {
        events.push(command("clip.set_launch_mode", "mode", json!(mode)));
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, CONTROL),
        Id::new(("chooser", "launch-mode")),
        ClipControl::LaunchMode,
        events,
    );

    // Legato.
    let at = row("Legato");
    crate::focus::register(
        ui.ctx(),
        Id::new(("switch", "launch-legato")),
        Area::Detail,
        None,
    );
    if let Some(legato) = widgets::switch(
        ui,
        theme,
        "launch-legato",
        "Legato",
        Rect::from_min_size(at, vec2(54.0, CONTROL.y)),
        launch.legato,
    ) {
        events.push(command("clip.set_legato", "legato", json!(legato)));
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, vec2(54.0, CONTROL.y)),
        Id::new(("switch", "launch-legato")),
        ClipControl::Legato,
        events,
    );

    // The nudge buttons, with the scrub control between them in MIDI Map
    // Mode (launch quantisation R35, map modes R22).
    let at = row("Nudge");
    let slot = slot_of(document, clip);
    let scrub = crate::mapping::remote_map(ui.ctx())
        .is_some_and(|map| map.mode == bl_shell::remote_commands::MapMode::Midi);
    nudge_button(
        ui,
        theme,
        Rect::from_min_size(at, vec2(26.0, SMALL.y)),
        ("Nudge Backward", "«", -1),
        slot,
        events,
    );
    if scrub {
        scrub_control(ui, theme, Rect::from_min_size(at + vec2(28.0, 0.0), vec2(40.0, SMALL.y)), events);
    }
    nudge_button(
        ui,
        theme,
        Rect::from_min_size(
            at + vec2(if scrub { 70.0 } else { 32.0 }, 0.0),
            vec2(26.0, SMALL.y),
        ),
        ("Nudge Forward", "»", 1),
        slot,
        events,
    );

    // Quantize: Global, then the global menu's list.
    let at = row("Quantize");
    let steps: Vec<_> = std::iter::once(Choice {
        label: "Global".to_owned(),
        value: None,
        leds: NO_LEDS,
    })
    .chain(LaunchQuantization::ALL.into_iter().map(|step| Choice {
        label: step.label().to_owned(),
        value: Some(step),
        leds: NO_LEDS,
    }))
    .collect();
    crate::focus::register(
        ui.ctx(),
        Id::new(("chooser", "launch-quantize")),
        Area::Detail,
        None,
    );
    if let Some(step) = widgets::chooser(
        ui,
        theme,
        "launch-quantize",
        "Launch Quantization",
        Rect::from_min_size(at, CONTROL),
        &steps,
        Some(&launch.quantization),
        true,
    ) {
        events.push(command(
            "clip.set_launch_quantization",
            "quantization",
            json!(step),
        ));
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, CONTROL),
        Id::new(("chooser", "launch-quantize")),
        ClipControl::LaunchQuantization,
        events,
    );

    // Velocity, in percent, a hundredth at a time.
    let at = row("Velocity");
    crate::focus::register(
        ui.ctx(),
        Id::new(("number-box", "launch-velocity")),
        Area::Detail,
        None,
    );
    if let Some(amount) = widgets::number_box(
        ui,
        theme,
        "launch-velocity",
        "Velocity",
        Rect::from_min_size(at, SMALL),
        launch.velocity_amount,
        (0.0, 1.0),
        &|value, steps| ((value * 100.0).round() + f64::from(steps)) / 100.0,
        &format!("{:.1} %", launch.velocity_amount * 100.0),
    ) {
        events.push(command("clip.set_velocity_amount", "amount", json!(amount)));
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, SMALL),
        Id::new(("number-box", "launch-velocity")),
        ClipControl::Velocity,
        events,
    );
}
