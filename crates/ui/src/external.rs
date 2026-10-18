//! An external device's section (WP-5.34; the external devices spec, R9):
//! down the left of its body, MIDI To and its channel, Audio From, and
//! Hardware Latency with its unit switch. Every control runs a registered
//! command (`device.set_external`), and a drag on Hardware Latency is one
//! undo step however far it goes.
//!
//! Beatlayer's own layout; Live's is to observe.

use bl_model::{DeviceId, HardwareLatency, MAX_HARDWARE_LATENCY_MS, MAX_HARDWARE_LATENCY_SAMPLES};
use bl_shell::device_commands::{EXTERNAL_SECTION_WIDTH, ExternalSection, TITLE_BAR_HEIGHT};
use bl_shell::routing::{self, ExternalChoosers};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{Align2, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::sidechain::{describe, draw_chooser, focus_ring, invoke, pressed};
use crate::theme::Theme;

/// The height of a row in the section, and the gap between rows.
const ROW: f32 = 14.0;
const ROW_GAP: f32 = 2.0;

/// The undo step a drag on Hardware Latency is.
const LATENCY_STEP: &str = "Change \"Hardware Latency\"";

/// How far a drag moves Hardware Latency: 0.8 % of its range per point, as
/// the sidechain's number boxes move.
const DRAG_PER_POINT: f64 = 0.008;

/// The ID of one of a device's external controls: `midi-to`, `channel`,
/// `audio-from`, `latency`, or `unit`.
#[must_use]
pub fn control_id(device: DeviceId, control: &str) -> Id {
    Id::new(("external-control", device, control))
}

/// The section down the left of an unfolded frame, under its title bar.
#[must_use]
pub fn section_rect(frame: Rect) -> Rect {
    Rect::from_min_max(
        pos2(frame.left(), frame.top() + TITLE_BAR_HEIGHT),
        pos2(frame.left() + EXTERNAL_SECTION_WIDTH, frame.bottom()),
    )
}

/// Where each control sits, top to bottom: the MIDI To chooser, its channel,
/// the Audio From chooser, and Hardware Latency with its unit switch beside
/// it (the box, then the switch).
#[must_use]
pub fn rows(frame: Rect) -> [Rect; 5] {
    let section = section_rect(frame).shrink2(vec2(3.0, 0.0));
    // Each control has its name above it.
    let row = |n: usize| {
        let top = section.top() + 3.0 + (ROW + ROW_GAP) * (n * 2 + 1) as f32 - ROW_GAP;
        Rect::from_min_size(pos2(section.left(), top), vec2(section.width(), ROW))
    };
    let latency = row(3);
    let unit = Rect::from_min_size(pos2(latency.right() - 22.0, latency.top()), vec2(22.0, ROW));
    let latency = Rect::from_min_max(latency.min, pos2(unit.left() - 2.0, latency.bottom()));
    [row(0), row(1), row(2), latency, unit]
}

/// A Hardware Latency as the box shows it: `12.50 ms` or `600 smp`.
#[must_use]
pub fn latency_text(latency: HardwareLatency) -> String {
    match latency {
        HardwareLatency::Milliseconds { value } => format!("{value:.2} ms"),
        HardwareLatency::Samples { value } => format!("{value} smp"),
    }
}

/// Hardware Latency's place in its range, 0 to 1.
fn position(latency: HardwareLatency) -> f64 {
    match latency {
        HardwareLatency::Milliseconds { value } => value / MAX_HARDWARE_LATENCY_MS,
        HardwareLatency::Samples { value } => {
            f64::from(value) / f64::from(MAX_HARDWARE_LATENCY_SAMPLES)
        }
    }
}

/// The Hardware Latency in `latency`'s unit at `position`, 0 to 1: whole
/// samples, or hundredths of a millisecond.
fn at(latency: HardwareLatency, position: f64) -> HardwareLatency {
    let position = position.clamp(0.0, 1.0);
    match latency {
        HardwareLatency::Milliseconds { .. } => HardwareLatency::Milliseconds {
            value: (position * MAX_HARDWARE_LATENCY_MS * 100.0).round() / 100.0,
        },
        HardwareLatency::Samples { .. } => HardwareLatency::Samples {
            value: (position * f64::from(MAX_HARDWARE_LATENCY_SAMPLES)).round() as u32,
        },
    }
}

fn set_latency(device: DeviceId, latency: HardwareLatency) -> Invocation {
    let value = match latency {
        HardwareLatency::Milliseconds { value } => {
            json!({ "unit": "milliseconds", "value": value })
        }
        HardwareLatency::Samples { value } => json!({ "unit": "samples", "value": value }),
    };
    Invocation::new(
        "device.set_external",
        json!({ "device": device, "hardware_latency": value }),
    )
}

/// Draws a device's external section in its frame at `frame`.
pub fn draw(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    device: DeviceId,
    section: &ExternalSection,
    frame: Rect,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let area = section_rect(frame);
    ui.painter_at(area).rect_filled(area, 0.0, c.control.0);
    let [midi_to, channel, audio_from, latency, unit] = rows(frame);
    let Some(ExternalChoosers {
        midi_to: midi_to_chooser,
        channel: channel_chooser,
        audio_from: audio_from_chooser,
    }) = routing::external_choosers(shell.editor().document(), device, &shell.interface())
    else {
        return;
    };

    // The names above the controls.
    let painter = ui.painter_at(area);
    for (rect, name) in [
        (midi_to, "MIDI To"),
        (audio_from, "Audio From"),
        (latency, "Hardware Latency"),
    ] {
        painter.text(
            pos2(rect.left() + 1.0, rect.top() - 1.0),
            Align2::LEFT_BOTTOM,
            name,
            theme.small_font(),
            c.text_dim.0,
        );
    }
    draw_chooser(
        ui,
        theme,
        control_id(device, "midi-to"),
        midi_to,
        &midi_to_chooser,
        events,
    );
    draw_chooser(
        ui,
        theme,
        control_id(device, "channel"),
        channel,
        &channel_chooser,
        events,
    );
    draw_chooser(
        ui,
        theme,
        control_id(device, "audio-from"),
        audio_from,
        &audio_from_chooser,
        events,
    );

    // Hardware Latency: a number box a vertical drag changes, as one undo
    // step; a double-click puts it back to 0.
    let current = section.hardware_latency;
    let text = latency_text(current);
    let id = control_id(device, "latency");
    let response = ui.interact(latency, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if response.drag_started() {
        response.request_focus();
        ui.data_mut(|d| d.insert_temp(id, position(current)));
        events.push(UiEvent::BeginGesture(LATENCY_STEP));
    }
    if response.dragged() {
        let start: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
        let moved = -f64::from(response.drag_delta().y) * DRAG_PER_POINT;
        let next = (start + moved).clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(id, next));
        let value = at(current, next);
        if value != current {
            events.push(UiEvent::Invoke(set_latency(device, value)));
        }
    }
    if response.drag_stopped() {
        events.push(UiEvent::EndGesture);
    }
    if response.double_clicked() {
        let zero = at(current, 0.0);
        if zero != current {
            events.push(UiEvent::Invoke(set_latency(device, zero)));
        }
    }
    let painter = ui.painter_at(latency.expand(2.0));
    painter.rect(
        latency,
        theme.sizes.corner,
        c.window.0,
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    painter.text(
        pos2(latency.right() - 3.0, latency.center().y),
        Align2::RIGHT_CENTER,
        &text,
        theme.small_font(),
        c.text.0,
    );
    focus_ring(ui, theme, latency, &response);
    describe(ui, id, Role::SpinButton, "Hardware Latency", &text);

    // The unit switch: milliseconds or samples. Switching converts the value
    // at the sample rate the device runs at, to whole samples (R12).
    let id = control_id(device, "unit");
    let response = ui.interact(unit, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if pressed(ui, &response) {
        let rate = shell.audio().sample_rate().unwrap_or(48_000);
        invoke_latency(events, device, current.switched(rate));
    }
    let label = match current {
        HardwareLatency::Milliseconds { .. } => "ms",
        HardwareLatency::Samples { .. } => "smp",
    };
    let painter = ui.painter_at(unit.expand(2.0));
    painter.rect(
        unit,
        theme.sizes.corner,
        c.control.0,
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    painter.text(
        unit.center(),
        Align2::CENTER_CENTER,
        label,
        theme.small_font(),
        c.text.0,
    );
    focus_ring(ui, theme, unit, &response);
    describe(ui, id, Role::Button, "Hardware Latency Unit", label);
}

fn invoke_latency(events: &mut Vec<UiEvent>, device: DeviceId, latency: HardwareLatency) {
    let run = set_latency(device, latency);
    invoke(events, &run.id, run.args);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_moves_the_value_in_its_unit() {
        let ms = HardwareLatency::Milliseconds { value: 0.0 };
        assert_eq!(at(ms, 0.5), HardwareLatency::Milliseconds { value: 500.0 });
        assert_eq!(at(ms, 2.0), HardwareLatency::Milliseconds { value: 1000.0 });
        let samples = HardwareLatency::Samples { value: 0 };
        assert_eq!(at(samples, 0.5), HardwareLatency::Samples { value: 96_000 });
        assert!((position(HardwareLatency::Samples { value: 48_000 }) - 0.25).abs() < 1e-12);
        assert_eq!(
            latency_text(HardwareLatency::Milliseconds { value: 12.5 }),
            "12.50 ms"
        );
        assert_eq!(
            latency_text(HardwareLatency::Samples { value: 600 }),
            "600 smp"
        );
    }
}
