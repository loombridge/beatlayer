//! The Clip View's Launch Properties panel (WP-3.9; layout.md L-CLIP-003
//! and L-CLIP-005): the left group's second panel, shown for a Session clip
//! only, to the left of the editor.
//!
//! Launch mode, Legato, Quantize and Velocity edit the clip through
//! `clip.set_launch_mode`, `clip.set_legato`, `clip.set_launch_quantization`
//! and `clip.set_velocity_amount`, the commands agents run, and read the
//! clip's saved launch settings back. The follow-action controls are drawn
//! in place and pending until WP-3.10, and so are the nudge buttons: what
//! nudge does isn't recorded yet. The MIDI Bank, Sub and Pgm fields wait
//! for an answer to what they do. The layout follows L-CLIP-005's order;
//! the reference didn't measure the tab, so the sizes are the manual's
//! reading, not pixel parity.

use bl_model::{ClipId, LaunchMode, LaunchQuantization};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{Align2, Id, Rect, Sense, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice};

/// How wide the left group of panels is (L-CLIP-003: x 249 to 591).
pub const WIDTH: f32 = 342.0;
/// The panel selector row's height (L-CLIP-003).
const SELECTOR: f32 = 16.0;
/// A control's size, and the step between rows.
const CONTROL: Vec2 = vec2(120.0, 17.0);
const ROW: f32 = 19.0;
/// Where the controls start, from the panel's left edge.
const CONTROLS_AT: f32 = 100.0;
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
    Rect::from_min_size(view.min, vec2(WIDTH.min(view.width() / 2.0), view.height()))
}

/// The editor's place beside the panel group.
#[must_use]
pub fn editor_rect(view: Rect) -> Rect {
    Rect::from_min_max(pos2(panel_rect(view).right(), view.top()), view.max)
}

/// A control drawn in place that does nothing yet, announced as disabled
/// with the reason.
fn pending(ui: &mut Ui, theme: &Theme, salt: &str, rect: Rect, text: &str, reason: &str) {
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

/// Draws the Launch Properties panel for the Session clip `clip` in `rect`.
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
        node.set_role(Role::Group);
        node.set_label("Launch Properties".to_owned());
        node.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
    ui.painter().rect_filled(rect, 0.0, c.panel.0);

    // The panel selector row (L-CLIP-003): Main Properties comes with its
    // own package, so it shows disabled; Launch Properties is the one shown.
    let half = rect.width() / 2.0;
    for (index, (name, enabled)) in [("Main Properties", false), ("Launch Properties", true)]
        .into_iter()
        .enumerate()
    {
        let button = Rect::from_min_size(
            rect.min + vec2(half * index as f32, 0.0),
            vec2(half - 2.0, SELECTOR),
        );
        let id = Id::new(("launch-panel-selector", index));
        let response = ui.interact(button, id, Sense::hover());
        ui.painter().rect_filled(
            button,
            theme.sizes.corner,
            if enabled {
                c.control_pressed.0
            } else {
                c.control.0
            },
        );
        ui.painter().text(
            button.center(),
            Align2::CENTER_CENTER,
            name,
            theme.small_font(),
            if enabled { c.text.0 } else { c.pending.0 },
        );
        response
            .widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, enabled, enabled, name));
    }

    let left = rect.left() + 8.0;
    let mut y = rect.top() + SELECTOR + 6.0;
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
    let later = "Not built yet: arrives with WP-3.10.";

    // Follow Action, its actions A and B with their chances, and Linked
    // with the multiplier it shows (the recorded defaults).
    let at = row("");
    pending(
        ui,
        theme,
        "Follow Action",
        Rect::from_min_size(at, CONTROL),
        "Follow Action",
        later,
    );
    for (name, action, chance) in [("A", "Next", "100 %"), ("B", "No Action", "0 %")] {
        let at = row(&format!("Action {name}"));
        pending(
            ui,
            theme,
            &format!("Follow Action {name}"),
            Rect::from_min_size(at, CONTROL),
            action,
            later,
        );
        pending(
            ui,
            theme,
            &format!("Chance {name}"),
            Rect::from_min_size(at + vec2(CONTROL.x + 6.0, 0.0), SMALL),
            chance,
            later,
        );
    }
    let at = row("Follow Time");
    pending(
        ui,
        theme,
        "Follow Time Linked",
        Rect::from_min_size(at, SMALL),
        "Linked",
        later,
    );
    pending(
        ui,
        theme,
        "Follow Time Multiplier",
        Rect::from_min_size(at + vec2(SMALL.x + 6.0, 0.0), SMALL),
        "1x",
        later,
    );

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

    // The nudge buttons.
    let at = row("Nudge");
    let nudge = "Not built yet: what nudge does isn't recorded.";
    pending(
        ui,
        theme,
        "Nudge Back",
        Rect::from_min_size(at, vec2(26.0, SMALL.y)),
        "«",
        nudge,
    );
    pending(
        ui,
        theme,
        "Nudge Forward",
        Rect::from_min_size(at + vec2(32.0, 0.0), vec2(26.0, SMALL.y)),
        "»",
        nudge,
    );

    // Quantize: Global, then the global menu's list.
    let at = row("Quantize");
    let steps: Vec<_> = std::iter::once(Choice {
        label: "Global".to_owned(),
        value: None,
    })
    .chain(LaunchQuantization::ALL.into_iter().map(|step| Choice {
        label: step.label().to_owned(),
        value: Some(step),
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
}
