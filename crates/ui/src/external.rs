//! An external device's section (WP-5.34; the external devices spec, R9):
//! down the left of its body, MIDI To and its channel, Audio From, and
//! Hardware Latency with its unit switch. Every control runs a registered
//! command (`device.set_external`), and a drag on Hardware Latency is one
//! undo step however far it goes.
//!
//! Beatlayer's own layout; Live's is to observe.

use bl_model::{DeviceId, HardwareLatency, MAX_HARDWARE_LATENCY_MS, MAX_HARDWARE_LATENCY_SAMPLES};
use bl_shell::device_commands::{
    CvSection, EXTERNAL_SECTION_WIDTH, ExternalSection, TITLE_BAR_HEIGHT,
};
use bl_shell::routing::{self, Chooser, ExternalChoosers};
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

/// The height of a row of a CV device's section, and the gap between rows:
/// the name sits in the row's left, the control to its right, so the eight
/// outputs of CV Triggers and its return fit the body (L-DEV-026).
const CV_ROW: f32 = 13.0;
const CV_GAP: f32 = 2.0;
/// The width the name of a port takes at the row's left.
const CV_NAME: f32 = 42.0;

/// Row `n` of a CV device's section, from 0: its name's room and its
/// control's.
#[must_use]
pub fn cv_row(frame: Rect, n: usize) -> (Rect, Rect) {
    let section = section_rect(frame).shrink2(vec2(3.0, 0.0));
    let top = section.top() + 3.0 + (CV_ROW + CV_GAP) * n as f32;
    let row = Rect::from_min_size(pos2(section.left(), top), vec2(section.width(), CV_ROW));
    (
        Rect::from_min_size(row.min, vec2(CV_NAME, CV_ROW)),
        Rect::from_min_max(pos2(row.left() + CV_NAME, row.top()), row.max),
    )
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
    let latency = row_at(section, 3);
    let unit = unit_beside(latency);
    let latency = Rect::from_min_max(latency.min, pos2(unit.left() - 2.0, latency.bottom()));
    [
        row_at(section, 0),
        row_at(section, 1),
        row_at(section, 2),
        latency,
        unit,
    ]
}

/// Where the External Audio Effect's controls sit, top to bottom: the Audio
/// To chooser, the Audio From chooser, and Hardware Latency with its unit
/// switch beside it (the box, then the switch).
#[must_use]
pub fn effect_rows(frame: Rect) -> [Rect; 4] {
    let section = section_rect(frame).shrink2(vec2(3.0, 0.0));
    let latency = row_at(section, 2);
    let unit = unit_beside(latency);
    let latency = Rect::from_min_max(latency.min, pos2(unit.left() - 2.0, latency.bottom()));
    [row_at(section, 0), row_at(section, 1), latency, unit]
}

/// The row `n` of the section, each control with its name above it.
fn row_at(section: Rect, n: usize) -> Rect {
    let top = section.top() + 3.0 + (ROW + ROW_GAP) * (n * 2 + 1) as f32 - ROW_GAP;
    Rect::from_min_size(pos2(section.left(), top), vec2(section.width(), ROW))
}

/// The unit switch, at the end of the Hardware Latency box.
fn unit_beside(latency: Rect) -> Rect {
    Rect::from_min_size(pos2(latency.right() - 22.0, latency.top()), vec2(22.0, ROW))
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
    let Some(ExternalChoosers {
        midi_to: midi_to_chooser,
        channel: channel_chooser,
        audio_from: audio_from_chooser,
        audio_to: audio_to_chooser,
        cv_outputs,
        cv_inputs,
    }) = routing::external_choosers(shell.editor().document(), device, &shell.interface())
    else {
        return;
    };
    // Spec: cv-tools R3, R10, R14; L-DEV-026. A CV device's section lists its
    // CV outputs and inputs, then its return.
    if let Some(cv) = &section.cv {
        let ports: Vec<_> = cv_outputs.iter().chain(&cv_inputs).collect();
        draw_cv(
            ui,
            shell,
            theme,
            (device, section, cv),
            (&ports, &audio_from_chooser),
            frame,
            events,
        );
        return;
    }
    // The External Audio Effect sends its signal out and hears it back: it
    // has an Audio To and no MIDI To (WP-5.35, R17).
    let (midi_to, channel, audio_to, audio_from, latency, unit) = if section.output {
        let [audio_to, audio_from, latency, unit] = effect_rows(frame);
        (None, None, Some(audio_to), audio_from, latency, unit)
    } else {
        let [midi_to, channel, audio_from, latency, unit] = rows(frame);
        (
            Some(midi_to),
            Some(channel),
            None,
            audio_from,
            latency,
            unit,
        )
    };

    // The names above the controls.
    let painter = ui.painter_at(area);
    for (rect, name) in [
        (midi_to, "MIDI To"),
        (audio_to, "Audio To"),
        (Some(audio_from), "Audio From"),
        (Some(latency), "Hardware Latency"),
    ] {
        let Some(rect) = rect else { continue };
        painter.text(
            pos2(rect.left() + 1.0, rect.top() - 1.0),
            Align2::LEFT_BOTTOM,
            name,
            theme.small_font(),
            c.text_dim.0,
        );
    }
    if let (Some(midi_to), Some(channel)) = (midi_to, channel) {
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
    }
    if let (Some(audio_to), Some(chooser)) = (audio_to, audio_to_chooser.as_ref()) {
        draw_chooser(
            ui,
            theme,
            control_id(device, "audio-to"),
            audio_to,
            chooser,
            events,
        );
    }
    draw_chooser(
        ui,
        theme,
        control_id(device, "audio-from"),
        audio_from,
        &audio_from_chooser,
        events,
    );

    latency_controls(
        ui,
        shell,
        theme,
        device,
        section.hardware_latency,
        (latency, unit),
        events,
    );
}

/// Hardware Latency's number box and its unit switch, in `latency` and
/// `unit`.
fn latency_controls(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    device: DeviceId,
    current: HardwareLatency,
    (latency, unit): (Rect, Rect),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    // Hardware Latency: a number box a vertical drag changes, as one undo
    // step; a double-click puts it back to 0. Spec: delay-compensation R7.
    // While Delay Compensation is off both it and its unit switch are
    // disabled, keeping the value.
    let available = shell.editor().document().settings().delay_compensation;
    let text = latency_text(current);
    let id = control_id(device, "latency");
    let response = ui.interact(
        latency,
        id,
        if available {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    if available {
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
    }
    let painter = ui.painter_at(latency.expand(2.0));
    painter.rect(
        latency,
        theme.sizes.corner,
        if available {
            c.window.0
        } else {
            c.control.0.gamma_multiply(0.6)
        },
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    painter.text(
        pos2(latency.right() - 3.0, latency.center().y),
        Align2::RIGHT_CENTER,
        &text,
        theme.small_font(),
        if available { c.text.0 } else { c.pending.0 },
    );
    focus_ring(ui, theme, latency, &response);
    describe(ui, id, Role::SpinButton, "Hardware Latency", &text);
    if !available {
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_disabled();
            node.set_description("Unavailable while Delay Compensation is off".to_owned());
        });
    }

    // The unit switch: milliseconds or samples. Switching converts the value
    // at the sample rate the device runs at, to whole samples (R12).
    let id = control_id(device, "unit");
    let response = ui.interact(
        unit,
        id,
        if available {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if available {
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if pressed(ui, &response) {
            let rate = shell.audio().sample_rate().unwrap_or(48_000);
            invoke_latency(events, device, current.switched(rate));
        }
    }
    let label = match current {
        HardwareLatency::Milliseconds { .. } => "ms",
        HardwareLatency::Samples { .. } => "smp",
    };
    let painter = ui.painter_at(unit.expand(2.0));
    painter.rect(
        unit,
        theme.sizes.corner,
        if available {
            c.control.0
        } else {
            c.control.0.gamma_multiply(0.6)
        },
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    painter.text(
        unit.center(),
        Align2::CENTER_CENTER,
        label,
        theme.small_font(),
        if available { c.text.0 } else { c.pending.0 },
    );
    focus_ring(ui, theme, unit, &response);
    describe(ui, id, Role::Button, "Hardware Latency Unit", label);
    if !available {
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_disabled();
            node.set_description("Unavailable while Delay Compensation is off".to_owned());
        });
    }
}

/// A CV device's section (L-DEV-026): a chooser per CV output and input
/// under its name, then Audio From and Hardware Latency, and for CV
/// Instrument the Calibrate and Clear buttons with the readout.
fn draw_cv(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    (device, section, cv): (DeviceId, &ExternalSection, &CvSection),
    (ports, audio_from): (&[&(String, Chooser)], &Chooser),
    frame: Rect,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let painter = ui.painter_at(section_rect(frame));
    let name = |rect: Rect, text: &str| {
        painter.text(
            pos2(rect.left() + 1.0, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            theme.small_font(),
            c.text_dim.0,
        );
    };
    for (n, (port, chooser)) in ports.iter().map(|p| (&p.0, &p.1)).enumerate() {
        let (label, control) = cv_row(frame, n);
        name(label, port);
        let id = control_id(device, &format!("cv-{port}"));
        draw_chooser(ui, theme, id, control, chooser, events);
        describe(
            ui,
            id,
            Role::ComboBox,
            &format!("{} {port}", chooser.name),
            &chooser.text,
        );
    }
    let rows = ports.len();
    if !cv.returns {
        // A CV effect or CV In brings no sound back: below its ports, only
        // CV Clock In's readout and pulse light (R24).
        if !cv.readout.is_empty() {
            let (_, readout) = cv_row(frame, rows);
            let rect = Rect::from_min_max(
                pos2(readout.left() - CV_NAME + 10.0, readout.top()),
                pos2(readout.right(), readout.top() + 2.0 * CV_ROW),
            );
            let id = control_id(device, "clock");
            ui.interact(rect, id, Sense::focusable_noninteractive());
            let painter = ui.painter_at(section_rect(frame));
            painter.circle_filled(
                pos2(rect.left() - 5.0, rect.top() + CV_ROW / 2.0),
                3.0,
                if cv.lamp == Some(true) {
                    c.text.0
                } else {
                    c.separator.0
                },
            );
            painter.text(
                pos2(rect.left(), rect.top()),
                Align2::LEFT_TOP,
                &cv.readout,
                theme.small_font(),
                c.text_dim.0,
            );
            describe(
                ui,
                id,
                Role::Label,
                "Clock",
                &cv.readout.replace('\n', ", "),
            );
        }
        return;
    }
    let (label, control) = cv_row(frame, rows);
    name(label, "Audio From");
    draw_chooser(
        ui,
        theme,
        control_id(device, "audio-from"),
        control,
        audio_from,
        events,
    );
    // Hardware Latency, its box then its unit switch beside it.
    let (label, row) = cv_row(frame, rows + 1);
    name(label, "Latency");
    let unit = Rect::from_min_size(pos2(row.right() - 22.0, row.top()), vec2(22.0, CV_ROW));
    let latency = Rect::from_min_max(row.min, pos2(unit.left() - 2.0, row.bottom()));
    latency_controls(
        ui,
        shell,
        theme,
        device,
        section.hardware_latency,
        (latency, unit),
        events,
    );
    if !cv.calibrates {
        return;
    }
    // Calibrate and Clear, side by side, and the readout under them.
    let (_, row) = cv_row(frame, rows + 2);
    let (_, readout) = cv_row(frame, rows + 3);
    let half = (row.width() + CV_NAME) / 2.0;
    let left = Rect::from_min_size(
        pos2(row.left() - CV_NAME, row.top()),
        vec2(half - 1.0, CV_ROW),
    );
    let right = Rect::from_min_max(pos2(left.right() + 2.0, row.top()), row.max);
    for (rect, label, command) in [
        (left, "Calibrate", "cv.calibrate"),
        (right, "Clear", "cv.clear_calibration"),
    ] {
        let id = control_id(device, label);
        let args = json!({ "device": device });
        // Calibrate plays through the interface, so it needs an engine.
        let enabled = if command == "cv.calibrate" {
            shell.audio().engine_running()
        } else {
            cv.calibrated
        };
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        if enabled {
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
            if pressed(ui, &response) {
                invoke(events, command, args);
            }
        }
        let painter = ui.painter_at(rect.expand(2.0));
        painter.rect(
            rect,
            theme.sizes.corner,
            if enabled {
                c.control.0
            } else {
                c.control.0.gamma_multiply(0.6)
            },
            Stroke::new(1.0, c.separator.0),
            StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            theme.small_font(),
            if enabled { c.text.0 } else { c.pending.0 },
        );
        focus_ring(ui, theme, rect, &response);
        describe(ui, id, Role::Button, label, "");
        if !enabled {
            ui.ctx()
                .accesskit_node_builder(id, |node| node.set_disabled());
        }
    }
    let readout_rect = Rect::from_min_max(
        pos2(readout.left() - CV_NAME, readout.top()),
        pos2(readout.right(), readout.top() + 2.0 * CV_ROW),
    );
    let id = control_id(device, "calibration");
    ui.interact(readout_rect, id, Sense::focusable_noninteractive());
    ui.painter_at(readout_rect).text(
        pos2(readout_rect.left() + 1.0, readout_rect.top()),
        Align2::LEFT_TOP,
        &cv.readout,
        theme.small_font(),
        c.text_dim.0,
    );
    describe(ui, id, Role::Label, "Calibration", &cv.readout);
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
