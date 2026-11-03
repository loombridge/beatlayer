//! The similar sample swap controls (WP-9.10; sound similarity R25 to R27 and
//! R30, drum racks R30): Previous Similar, Next Similar and Show Similar Files
//! buttons at the top right of Simpler's waveform, the menu items that go
//! with them, and the arrows over a pad's edges while Option (Alt) is held.
//!
//! Every control runs a registered command (`device.swap_similar`,
//! `device.set_similarity_reference`, `device.return_to_reference`,
//! `browser.show_similar`); a control that can't run is drawn disabled with
//! the reason on hover, as `device.similar` gives it.

use bl_model::DeviceId;
use bl_shell::Shell;
use bl_shell::device_commands::SimilarState;
use egui::accesskit::Role;
use egui::{Id, Rect, Response, Sense, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::browser::{Glyph, bounds, glyph};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The size of a display's button, in points.
const BUTTON: f32 = 16.0;

/// The gap between a display's buttons and from its edges.
const GAP: f32 = 2.0;

/// The width of the arrow over a pad's edge.
const ARROW: f32 = 14.0;

/// What a button does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Previous,
    Next,
    Show,
}

impl Control {
    /// What assistive technology reads (R30).
    fn name(self) -> &'static str {
        match self {
            Self::Previous => "Previous Similar Sample",
            Self::Next => "Next Similar Sample",
            Self::Show => "Show Similar Files",
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Self::Previous => Glyph::Left,
            Self::Next => Glyph::Right,
            Self::Show => Glyph::Similar,
        }
    }

    /// Whether the control can run, and why not if it can't (R27).
    fn available(self, state: &SimilarState) -> Result<(), String> {
        let why = || {
            state
                .reason
                .clone()
                .unwrap_or_else(|| "there is no similar sample to go to".to_owned())
        };
        match self {
            Self::Previous if state.can_previous => Ok(()),
            Self::Next if state.can_next => Ok(()),
            Self::Show if state.reference_path.is_some() => Ok(()),
            Self::Previous => Err(if state.position == Some(0) {
                "the sample is the reference, the first of the list".to_owned()
            } else {
                why()
            }),
            Self::Next => Err(if state.count.is_some() && state.position.is_some() {
                "the sample is the last of the list".to_owned()
            } else {
                why()
            }),
            Self::Show => Err(why()),
        }
    }
}

/// Where a display's three buttons sit: the top right of `area`, Previous
/// Similar, Next Similar, then Show Similar Files (R25).
pub(crate) fn display_rects(area: Rect) -> [Rect; 3] {
    let top = area.top() + GAP;
    let right = area.right() - GAP;
    std::array::from_fn(|n| {
        let x = right - BUTTON * (3 - n) as f32 - GAP * (2 - n) as f32;
        Rect::from_min_size(pos2(x, top), vec2(BUTTON, BUTTON))
    })
}

/// One button: drawn in the theme's colours, dim and without a click while
/// disabled, with its name and, if disabled, the reason for assistive
/// technology and on hover.
fn button(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    control: Control,
    available: &Result<(), String>,
) -> Response {
    let enabled = available.is_ok();
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
    let painter = ui.painter();
    let face = if enabled && response.hovered() {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.rect_filled(rect, theme.sizes.corner, face);
    let ink = if enabled { c.text.0 } else { c.text_dim.0 };
    glyph(painter, rect, control.glyph(), ink);
    crate::device_panel::focus_ring(ui, theme, rect, &response);
    if enabled {
        crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    }
    let reason = available.as_ref().err().cloned();
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(control.name().to_owned());
        node.set_bounds(bounds(rect));
        if let Some(reason) = &reason {
            node.set_disabled();
            node.set_description(reason.clone());
        }
    });
    match reason {
        Some(reason) => response.on_hover_text(reason),
        None => response.on_hover_text(control.name()),
    }
}

/// Runs `control` on `device`, or on the pad `pad` of the drum rack `rack`.
fn run(events: &mut Vec<UiEvent>, control: Control, target: Value, state: &SimilarState) {
    let mut args = target;
    match control {
        Control::Previous | Control::Next => {
            args["direction"] = json!(if control == Control::Next {
                "next"
            } else {
                "previous"
            });
            events.push(UiEvent::Invoke(bl_shell::Invocation::new(
                "device.swap_similar",
                args,
            )));
        }
        Control::Show => {
            if let Some(path) = &state.reference_path {
                events.push(UiEvent::Invoke(bl_shell::Invocation::new(
                    "browser.show_similar",
                    json!({ "path": path }),
                )));
            }
        }
    }
}

/// The three buttons at the top right of Simpler's waveform (R25).
pub(crate) fn display_buttons(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    device: DeviceId,
    area: Rect,
    events: &mut Vec<UiEvent>,
) {
    let state = shell.similar_state(device);
    for (control, rect) in [Control::Previous, Control::Next, Control::Show]
        .into_iter()
        .zip(display_rects(area))
    {
        let id = Id::new(("similar-button", device, control.name()));
        let available = control.available(&state);
        if button(ui, theme, id, rect, control, &available).clicked() {
            run(events, control, json!({ "device": device }), &state);
        }
    }
}

/// The arrows over the left and right edges of a pad whose chain plays a
/// Simpler or a Drum Sampler, while Option (Alt) is held over the pads
/// (R26; drum racks R30).
pub(crate) fn pad_arrows(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    (rack, note): (DeviceId, u8),
    pad: Rect,
    events: &mut Vec<UiEvent>,
) {
    let Some(state) = shell.pad_similar_state(rack, note) else {
        return;
    };
    let height = pad.height().min(BUTTON * 2.0);
    for (control, left) in [(Control::Previous, true), (Control::Next, false)] {
        let x = if left {
            pad.left()
        } else {
            pad.right() - ARROW
        };
        let rect = Rect::from_min_size(pos2(x, pad.center().y - height / 2.0), vec2(ARROW, height));
        let id = Id::new(("pad-similar", rack, note, control.name()));
        let available = control.available(&state);
        if button(ui, theme, id, rect, control, &available).clicked() {
            run(
                events,
                control,
                json!({ "rack": rack, "note": note }),
                &state,
            );
        }
    }
}

/// Show Similar Files, Save as Similarity Reference and Return to Reference
/// in a display's or a device's menu (R25; one-shot drum sampler R27).
pub(crate) fn menu_items(ui: &mut Ui, shell: &Shell, device: DeviceId, events: &mut Vec<UiEvent>) {
    let state = shell.similar_state(device);
    let items = [
        (
            "Show Similar Files",
            Control::Show.available(&state),
            "browser.show_similar",
            state
                .reference_path
                .as_ref()
                .map_or(Value::Null, |path| json!({ "path": path })),
        ),
        (
            "Save as Similarity Reference",
            if state.sample.is_some() && state.sample != state.reference {
                Ok(())
            } else if state.sample.is_some() {
                Err("the sample is the reference already".to_owned())
            } else {
                Err("it holds no sample".to_owned())
            },
            "device.set_similarity_reference",
            json!({ "device": device }),
        ),
        (
            "Return to Reference",
            if state.can_return {
                Ok(())
            } else {
                Err(state
                    .reason
                    .clone()
                    .unwrap_or_else(|| "the sample is the reference already".to_owned()))
            },
            "device.return_to_reference",
            json!({ "device": device }),
        ),
    ];
    for (label, available, id, args) in items {
        let enabled = available.is_ok();
        let response = ui.add_enabled(enabled, egui::Button::new(label));
        if let Err(reason) = &available {
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_description(reason.clone());
            });
        }
        let response = match available {
            Err(reason) => response.on_disabled_hover_text(reason),
            Ok(()) => response,
        };
        if response.clicked() {
            events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
            ui.close();
        }
    }
}
