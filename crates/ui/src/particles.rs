//! The particle field of Vector Delay's display (WP-8.52; the Vector Delay
//! spec, R8 and R13): each marker on the series called `Particles` is drawn
//! where its particle is, with its tap's number, and is a handle on the
//! parameters `<marker> X` and `<marker> Y`, its tap's starting place.
//! Dragging a particle sets them to the place it is dropped at, both in one
//! undo step; the arrow keys and a screen reader's steps move it a step at a
//! time. The particle is put there at rest, so it follows.

use bl_model::{DeviceDisplay, DisplayMarker, ParamDescriptor};
use bl_shell::Invocation;
use bl_shell::device_commands::DevicePanel;
use egui::accesskit::{Action, Role};
use egui::{Align2, Event, Id, Key, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The series whose markers are particles.
pub const PARTICLES: &str = "Particles";
/// A particle's drawn radius, and the half side of what takes a press.
const RADIUS: f32 = 6.0;
const HIT: f32 = 9.0;

/// Where `marker` is drawn in `area`: across by its place across the square,
/// and up by its place up it.
#[must_use]
pub fn particle_center(area: Rect, marker: &DisplayMarker) -> Pos2 {
    pos2(
        area.left() + area.width() * marker.x.clamp(0.0, 1.0),
        area.bottom() - area.height() * marker.y.clamp(0.0, 1.0),
    )
}

/// The undo step a drag or a key makes on `marker`'s particle.
#[must_use]
pub fn gesture_name(marker: &DisplayMarker) -> String {
    format!("Move \"{}\"", marker.name)
}

/// Draws the particles of `display` in `area` and turns what the person does
/// with them into `device.set_param` invocations.
pub fn draw(
    ui: &mut Ui,
    theme: &Theme,
    area: Rect,
    (id, enabled): (Id, bool),
    display: &DeviceDisplay,
    panel: &DevicePanel,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let ink = if enabled { c.accent.0 } else { c.inactive.0 };
    let params = &panel.description.params;
    let place = |name: String| params.iter().position(|p| p.name == name);
    for marker in display.markers.iter().filter(|m| m.series == PARTICLES) {
        let (Some(xi), Some(yi)) = (
            place(format!("{} X", marker.name)),
            place(format!("{} Y", marker.name)),
        ) else {
            continue;
        };
        let at = particle_center(area, marker);
        let hid = id.with(("particle", marker.name.as_str()));
        let response = ui.interact(
            Rect::from_center_size(at, vec2(HIT * 2.0, HIT * 2.0)),
            hid,
            Sense::click_and_drag(),
        );
        crate::focus::register(ui.ctx(), hid, bl_shell::Area::Detail, None);
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        let set = |index: usize, value: f64| {
            Invocation::new(
                "device.set_param",
                json!({ "device": display.device, "param": params[index].id, "value": value }),
            )
        };
        let name = gesture_name(marker);
        if response.drag_started() {
            events.push(UiEvent::BeginNamedGesture(name.clone()));
        }
        if response.dragged()
            && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
        {
            let dropped = [
                (pointer.x - area.left()) / area.width(),
                (area.bottom() - pointer.y) / area.height(),
            ];
            for (index, travel) in [(xi, dropped[0]), (yi, dropped[1])] {
                let value = params[index].clamp(f64::from(travel.clamp(0.0, 1.0)));
                if value != panel.value(index) {
                    events.push(UiEvent::Invoke(set(index, value)));
                }
            }
        }
        if response.drag_stopped() {
            events.push(UiEvent::EndGesture);
        }
        // The arrow keys and a screen reader's increments, a step each.
        let (mut across, mut up) = (0, 0);
        across += ui.input(|i| {
            i.num_accesskit_action_requests(hid, Action::Increment) as i32
                - i.num_accesskit_action_requests(hid, Action::Decrement) as i32
        });
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    hid,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
            ui.input(|i| {
                for event in &i.events {
                    if let Event::Key {
                        key, pressed: true, ..
                    } = event
                    {
                        match key {
                            Key::ArrowRight => across += 1,
                            Key::ArrowLeft => across -= 1,
                            Key::ArrowUp => up += 1,
                            Key::ArrowDown => up -= 1,
                            _ => {}
                        }
                    }
                }
            });
        }
        for (steps, index) in [(across, xi), (up, yi)] {
            if steps == 0 {
                continue;
            }
            let value = crate::device_panel::step(&params[index], panel.value(index), steps);
            if value != panel.value(index) {
                events.push(UiEvent::BeginNamedGesture(name.clone()));
                events.push(UiEvent::Invoke(set(index, value)));
                events.push(UiEvent::EndGesture);
            }
        }
        let painter = ui.painter_at(area);
        painter.circle(at, RADIUS, ink, Stroke::new(1.0, c.text.0));
        painter.text(
            at,
            Align2::CENTER_CENTER,
            marker.name.trim_start_matches("Tap "),
            theme.small_font(),
            c.accent_text.0,
        );
        crate::device_panel::focus_ring(
            ui,
            theme,
            Rect::from_center_size(at, vec2(RADIUS * 2.0, RADIUS * 2.0)),
            &response,
        );
        describe(ui, &response, marker, &params[xi]);
    }
}

/// A particle as a screen reader hears it: a slider named for its marker,
/// reading what the marker says; its steps move it across.
fn describe(ui: &Ui, response: &egui::Response, marker: &DisplayMarker, param: &ParamDescriptor) {
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Slider);
        node.set_label(marker.name.clone());
        node.set_value(marker.text.clone());
        node.set_numeric_value(f64::from(marker.x));
        node.set_min_numeric_value(param.min());
        node.set_max_numeric_value(param.max());
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    });
}
