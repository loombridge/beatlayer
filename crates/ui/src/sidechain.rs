//! A device's sidechain strip and section (WP-2.19b; the routing spec), as
//! observed on Compressor in Live 12.4.6: a strip down the device's left
//! edge, "Sidechain" written up it, with the section's show/hide toggle at
//! its top and the Sidechain Listen (headphones) button under it; and, while
//! shown, the section beside it, top to bottom: External, the Source label
//! and chooser, the source's channel chooser, Mix, and SC Gain. Mix and SC
//! Gain light up while External is on.
//!
//! The section's width, the strip's, and its controls' sizes are to
//! observe. Every control runs a registered command (`device.*`), and a
//! drag on Mix or SC Gain is one undo step however far it goes.

use std::f32::consts::FRAC_PI_2;

use bl_model::DeviceId;
use bl_shell::device_commands::{
    MidiSidechainSection, SHOW_SIDECHAIN, SIDECHAIN_SECTION_WIDTH, SIDECHAIN_STRIP_WIDTH,
    SidechainSection, TITLE_BAR_HEIGHT,
};
use bl_shell::routing::{self, Chooser, Entry};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{Align2, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The height of a row in the section, and the gap between rows.
const ROW: f32 = 14.0;
const ROW_GAP: f32 = 2.0;

/// How far a drag on a number box moves it: 0.8 % of its range per point,
/// the rate observed on Live's device knobs (whether number boxes share it
/// is to observe).
const DRAG_PER_POINT: f64 = 0.008;

/// How far Up or Down moves a number box: a hundredth of its range (to
/// observe).
const KEY_STEP: f64 = 0.01;

/// SC Gain's range for a drag, in dB, below which it's −∞ (to observe; the
/// manual's range is to confirm).
pub const SC_GAIN_DB: (f64, f64) = (-70.0, 24.0);

/// The ID of one of a device's sidechain controls: `toggle`, `listen`,
/// `external`, `source`, `channel`, `mix`, or `gain`.
#[must_use]
pub fn control_id(device: DeviceId, control: &str) -> Id {
    Id::new(("sidechain-control", device, control))
}

/// The strip down the left of an unfolded frame, under its title bar.
#[must_use]
pub fn strip_rect(frame: Rect) -> Rect {
    Rect::from_min_max(
        pos2(frame.left(), frame.top() + TITLE_BAR_HEIGHT),
        pos2(frame.left() + SIDECHAIN_STRIP_WIDTH, frame.bottom()),
    )
}

/// The section beside the strip, while it's shown.
#[must_use]
pub fn section_rect(frame: Rect) -> Rect {
    let strip = strip_rect(frame);
    Rect::from_min_max(
        pos2(strip.right(), strip.top()),
        pos2(strip.right() + SIDECHAIN_SECTION_WIDTH, strip.bottom()),
    )
}

/// Where each of the section's controls sits, top to bottom: External,
/// the Source label, the Source chooser, the channel chooser, Mix, SC Gain.
#[must_use]
pub fn rows(frame: Rect) -> [Rect; 6] {
    let section = section_rect(frame).shrink2(vec2(3.0, 0.0));
    std::array::from_fn(|i| {
        let top = section.top() + 3.0 + (ROW + ROW_GAP) * i as f32;
        Rect::from_min_size(pos2(section.left(), top), vec2(section.width(), ROW))
    })
}

/// Where the strip's toggle and Listen button sit.
#[must_use]
pub fn strip_buttons(frame: Rect) -> [Rect; 2] {
    let strip = strip_rect(frame);
    let x = strip.center().x;
    let size = vec2(11.0, 11.0);
    let toggle = Rect::from_center_size(pos2(x, strip.top() + 3.0 + 5.5), size);
    let listen = Rect::from_center_size(pos2(x, toggle.bottom() + 3.0 + 5.5), size);
    [toggle, listen]
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

fn focus_ring(ui: &Ui, theme: &Theme, rect: Rect, response: &egui::Response) {
    if response.has_focus() && crate::focus::outlined(ui.ctx()) {
        ui.painter().rect_stroke(
            rect.expand(1.5),
            theme.sizes.corner,
            Stroke::new(1.0, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
}

/// What a click, or Enter with the focus on it, does to a button.
fn pressed(ui: &Ui, response: &egui::Response) -> bool {
    response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
}

/// Draws a device's sidechain strip and, while shown, its section, in its
/// frame at `frame`.
pub fn draw(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    device: DeviceId,
    section: &SidechainSection,
    frame: Rect,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let strip = strip_rect(frame);
    ui.painter_at(strip).rect_filled(strip, 0.0, c.control.0);
    let [toggle, listen] = strip_buttons(frame);

    // The toggle shows and hides the section; only the view changes.
    let id = control_id(device, "toggle");
    let response = ui.interact(toggle, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if pressed(ui, &response) {
        invoke(
            events,
            SHOW_SIDECHAIN,
            json!({ "device": device, "shown": !section.shown }),
        );
    }
    let painter = ui.painter_at(toggle.expand(2.0));
    painter.rect(
        toggle,
        theme.sizes.corner,
        if section.shown {
            c.text_dim.0
        } else {
            c.window.0
        },
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    focus_ring(ui, theme, toggle, &response);
    describe(
        ui,
        id,
        Role::Button,
        "Show/Hide Sidechain",
        on_off(section.shown),
    );

    // Sidechain Listen: headphones, lit while on.
    let id = control_id(device, "listen");
    let response = ui.interact(listen, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if pressed(ui, &response) {
        invoke(
            events,
            "device.set_sidechain_listen",
            json!({ "device": device, "listen": !section.listen }),
        );
    }
    let painter = ui.painter_at(listen.expand(2.0));
    let ink = if section.listen {
        c.accent.0
    } else {
        c.text_dim.0
    };
    let centre = listen.center();
    painter.circle_stroke(centre + vec2(0.0, 1.0), 3.5, Stroke::new(1.0, ink));
    for dx in [-3.5f32, 3.5] {
        painter.rect_filled(
            Rect::from_center_size(centre + vec2(dx, 2.5), vec2(2.0, 3.5)),
            0.5,
            ink,
        );
    }
    focus_ring(ui, theme, listen, &response);
    describe(
        ui,
        id,
        Role::Button,
        "Sidechain Listen",
        on_off(section.listen),
    );

    // "Sidechain" written up the strip.
    let painter = ui.painter_at(strip);
    let galley = painter.layout_no_wrap("Sidechain".to_owned(), theme.small_font(), c.text_dim.0);
    let at = pos2(
        strip.center().x - galley.size().y / 2.0,
        (listen.bottom() + 6.0 + galley.size().x).min(strip.bottom()),
    );
    painter.add(egui::epaint::TextShape::new(at, galley, c.text_dim.0).with_angle(-FRAC_PI_2));

    if !section.shown {
        return;
    }
    let doc = shell.editor().document();
    let [external, label, source, channel, mix, gain] = rows(frame);

    // External: a text button, lit while on (observed).
    let id = control_id(device, "external");
    let response = ui.interact(external, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if pressed(ui, &response) {
        invoke(
            events,
            "device.set_sidechain_enabled",
            json!({ "device": device, "enabled": !section.enabled }),
        );
    }
    let (face, ink) = if section.enabled {
        (c.accent.0, c.accent_text.0)
    } else {
        (c.control.0, c.text.0)
    };
    let painter = ui.painter_at(external.expand(2.0));
    painter.rect(
        external,
        theme.sizes.corner,
        face,
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    painter.text(
        external.center(),
        Align2::CENTER_CENTER,
        "External",
        theme.small_font(),
        ink,
    );
    focus_ring(ui, theme, external, &response);
    describe(ui, id, Role::Button, "External", on_off(section.enabled));

    ui.painter_at(label).text(
        label.left_center(),
        Align2::LEFT_CENTER,
        "Source",
        theme.small_font(),
        c.text_dim.0,
    );
    if let Some(chooser) = routing::sidechain_source(doc, device) {
        draw_chooser(
            ui,
            theme,
            control_id(device, "source"),
            source,
            &chooser,
            events,
        );
    }
    if let Some(chooser) = routing::sidechain_channel(doc, device) {
        draw_chooser(
            ui,
            theme,
            control_id(device, "channel"),
            channel,
            &chooser,
            events,
        );
    }
    let lit = section.enabled;
    number_box(
        ui,
        theme,
        NumberBox {
            id: control_id(device, "mix"),
            name: "Mix",
            text: percent(section.mix),
            fill: section.mix,
            lit,
            command: "device.set_sidechain_mix",
            field: "mix",
            step: "Change \"Mix\"",
            device,
            value: section.mix,
            to_position: |mix| mix,
            from_position: |position| position.clamp(0.0, 1.0),
            default: 1.0,
        },
        mix,
        events,
    );
    number_box(
        ui,
        theme,
        NumberBox {
            id: control_id(device, "gain"),
            name: "SC Gain",
            text: decibels(section.gain),
            fill: gain_position(section.gain),
            lit,
            command: "device.set_sidechain_gain",
            field: "gain",
            step: "Change \"SC Gain\"",
            device,
            value: section.gain,
            to_position: gain_position,
            from_position: gain_from_position,
            default: 1.0,
        },
        gain,
        events,
    );
}

/// Draws a device's MIDI sidechain strip and, while shown, its section, in
/// its frame at `frame` (WP-5.27; the routing spec, R34): the strip as a
/// sidechain's, with the toggle alone, and the section holding the Source
/// label and the MIDI source chooser. The rest of the device's MIDI
/// sidechain is its parameters, in the panel's expanded part.
pub fn draw_midi(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    device: DeviceId,
    section: &MidiSidechainSection,
    frame: Rect,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let strip = strip_rect(frame);
    ui.painter_at(strip).rect_filled(strip, 0.0, c.control.0);
    let [toggle, _] = strip_buttons(frame);
    let id = control_id(device, "toggle");
    let response = ui.interact(toggle, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if pressed(ui, &response) {
        invoke(
            events,
            SHOW_SIDECHAIN,
            json!({ "device": device, "shown": !section.shown }),
        );
    }
    ui.painter_at(toggle.expand(2.0)).rect(
        toggle,
        theme.sizes.corner,
        if section.shown {
            c.text_dim.0
        } else {
            c.window.0
        },
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    focus_ring(ui, theme, toggle, &response);
    describe(
        ui,
        id,
        Role::Button,
        "Show/Hide Sidechain",
        on_off(section.shown),
    );

    // "Sidechain" written up the strip.
    let painter = ui.painter_at(strip);
    let galley = painter.layout_no_wrap("Sidechain".to_owned(), theme.small_font(), c.text_dim.0);
    let at = pos2(
        strip.center().x - galley.size().y / 2.0,
        (toggle.bottom() + 6.0 + galley.size().x).min(strip.bottom()),
    );
    painter.add(egui::epaint::TextShape::new(at, galley, c.text_dim.0).with_angle(-FRAC_PI_2));

    if !section.shown {
        return;
    }
    let [label, source, ..] = rows(frame);
    ui.painter_at(label).text(
        label.left_center(),
        Align2::LEFT_CENTER,
        "MIDI Source",
        theme.small_font(),
        c.text_dim.0,
    );
    if let Some(chooser) = routing::midi_sidechain_source(shell.editor().document(), device) {
        draw_chooser(
            ui,
            theme,
            control_id(device, "midi-source"),
            source,
            &chooser,
            events,
        );
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn describe(ui: &Ui, id: Id, role: Role, label: &str, value: &str) {
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(role);
        node.set_label(label.to_owned());
        node.set_value(value.to_owned());
    });
}

/// Mix as Live shows it: `100 %`, `21 %`, `0.0 %` (observed).
#[must_use]
pub fn percent(mix: f64) -> String {
    let p = mix * 100.0;
    if p >= 10.0 {
        format!("{p:.0} %")
    } else {
        format!("{p:.1} %")
    }
}

/// SC Gain as Live shows it: `0.00 dB` (observed), `−inf dB` at silence.
#[must_use]
pub fn decibels(gain: f64) -> String {
    if gain <= 0.0 {
        return "-inf dB".to_owned();
    }
    let db = 20.0 * gain.log10();
    if db.abs() < 10.0 {
        format!("{db:.2} dB")
    } else {
        format!("{db:.1} dB")
    }
}

/// Where SC Gain sits in its drag range, from 0 to 1.
fn gain_position(gain: f64) -> f64 {
    let (low, high) = SC_GAIN_DB;
    if gain <= 0.0 {
        return 0.0;
    }
    ((20.0 * gain.log10() - low) / (high - low)).clamp(0.0, 1.0)
}

/// SC Gain at a place in its drag range: −∞ at the bottom.
fn gain_from_position(position: f64) -> f64 {
    let (low, high) = SC_GAIN_DB;
    if position <= 0.0 {
        return 0.0;
    }
    10f64.powf((low + position.min(1.0) * (high - low)) / 20.0)
}

/// A number box and what it changes.
struct NumberBox {
    id: Id,
    name: &'static str,
    text: String,
    /// Where the value sits in its range, from 0 to 1: the lit fill.
    fill: f64,
    lit: bool,
    command: &'static str,
    field: &'static str,
    step: &'static str,
    device: DeviceId,
    value: f64,
    to_position: fn(f64) -> f64,
    from_position: fn(f64) -> f64,
    default: f64,
}

/// A number box: a dark box with its name and value, a fill showing the
/// value's place in its range while lit (observed). A vertical drag
/// changes it as one undo step; a double-click, or Delete with the focus
/// on it, sets it back to its default.
fn number_box(ui: &mut Ui, theme: &Theme, b: NumberBox, rect: Rect, events: &mut Vec<UiEvent>) {
    let c = &theme.colors;
    let response = ui.interact(rect, b.id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), b.id, Area::Detail, None);
    let set =
        |value: f64| Invocation::new(b.command, json!({ "device": b.device, b.field: value }));
    if response.drag_started() {
        response.request_focus();
        ui.data_mut(|d| d.insert_temp(b.id, (b.to_position)(b.value)));
        events.push(UiEvent::BeginGesture(b.step));
    }
    if response.dragged() {
        let start: f64 = ui.data(|d| d.get_temp(b.id)).unwrap_or(0.0);
        let moved = -f64::from(response.drag_delta().y) * DRAG_PER_POINT;
        let position = (start + moved).clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(b.id, position));
        let value = (b.from_position)(position);
        if value != b.value {
            events.push(UiEvent::Invoke(set(value)));
        }
    }
    if response.drag_stopped() {
        events.push(UiEvent::EndGesture);
    }
    // With the focus on it, Up and Down move it by a hundredth of its range
    // (to observe), each an undo step.
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                b.id,
                egui::EventFilter {
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let step = ui.input(|i| {
            i32::from(i.key_pressed(egui::Key::ArrowUp))
                - i32::from(i.key_pressed(egui::Key::ArrowDown))
        });
        if step != 0 {
            let position = ((b.to_position)(b.value) + f64::from(step) * KEY_STEP).clamp(0.0, 1.0);
            let value = (b.from_position)(position);
            if value != b.value {
                events.push(UiEvent::Invoke(set(value)));
            }
        }
    }
    let reset = response.double_clicked()
        || (response.has_focus()
            && ui.input(|i| {
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
            }));
    if reset && b.value != b.default {
        events.push(UiEvent::Invoke(set(b.default)));
    }
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(rect, theme.sizes.corner, c.window.0);
    if b.lit {
        let width = rect.width() * b.fill.clamp(0.0, 1.0) as f32;
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(width, rect.height())),
            theme.sizes.corner,
            c.meter.0.gamma_multiply(0.6),
        );
    }
    painter.rect_stroke(
        rect,
        theme.sizes.corner,
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    let ink = if b.lit { c.text.0 } else { c.text_dim.0 };
    painter.text(
        pos2(rect.left() + 3.0, rect.center().y),
        Align2::LEFT_CENTER,
        b.name,
        theme.small_font(),
        ink,
    );
    painter.text(
        pos2(rect.right() - 3.0, rect.center().y),
        Align2::RIGHT_CENTER,
        &b.text,
        theme.small_font(),
        ink,
    );
    focus_ring(ui, theme, rect, &response);
    describe(ui, b.id, Role::SpinButton, b.name, &b.text);
}

/// A chooser: a dark box with the choice and a ▼, whose list a click opens
/// (observed); choosing an entry runs its command.
fn draw_chooser(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    chooser: &Chooser,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let enabled = chooser.enabled();
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::focusable_noninteractive()
        },
    );
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect(
        rect,
        theme.sizes.corner,
        c.window.0,
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    ui.painter_at(Rect::from_min_max(
        rect.min,
        pos2(rect.right() - 9.0, rect.bottom()),
    ))
    .text(
        pos2(rect.left() + 3.0, rect.center().y),
        Align2::LEFT_CENTER,
        &chooser.text,
        theme.small_font(),
        c.text.0,
    );
    if enabled {
        let a = pos2(rect.right() - 5.0, rect.center().y);
        painter.add(egui::Shape::convex_polygon(
            vec![
                a + vec2(-2.5, -1.25),
                a + vec2(2.5, -1.25),
                a + vec2(0.0, 1.75),
            ],
            c.text.0,
            Stroke::NONE,
        ));
    }
    focus_ring(ui, theme, rect, &response);
    describe(ui, id, Role::ComboBox, chooser.name, &chooser.text);
    if !enabled {
        return;
    }
    if response.clicked() {
        response.request_focus();
    }
    // With the focus on it, Up and Down choose the entry before or after the
    // chosen one, as the In/Out section's choosers do.
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
        let step = ui.input(|i| {
            i32::from(i.key_pressed(egui::Key::ArrowDown))
                - i32::from(i.key_pressed(egui::Key::ArrowUp))
        });
        if let Some(run) = stepped(chooser, step) {
            events.push(UiEvent::Invoke(run.clone()));
        }
    }
    egui::Popup::menu(&response)
        .width(rect.width().max(120.0))
        .show(|ui| {
            for entry in &chooser.entries {
                match entry {
                    Entry::Separator => {
                        ui.separator();
                    }
                    Entry::Choice {
                        label, run, chosen, ..
                    } => {
                        let item = ui.add_enabled(
                            run.is_some(),
                            egui::Button::selectable(*chosen, label.as_str()),
                        );
                        if item.clicked()
                            && let Some(run) = run
                        {
                            events.push(UiEvent::Invoke(run.clone()));
                        }
                    }
                }
            }
        });
}

/// The entry `step` places after (or, negative, before) the chosen one:
/// the first with nothing chosen.
fn stepped(chooser: &Chooser, step: i32) -> Option<&Invocation> {
    if step == 0 {
        return None;
    }
    let runs: Vec<(&Invocation, bool)> = chooser
        .entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Choice {
                run: Some(run),
                chosen,
                ..
            } => Some((run, *chosen)),
            _ => None,
        })
        .collect();
    let at = runs.iter().position(|(_, chosen)| *chosen);
    let next = match at {
        Some(at) => at.checked_add_signed(step as isize)?,
        None => 0,
    };
    runs.get(next).map(|(run, _)| *run)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_live_shows_them() {
        assert_eq!(percent(1.0), "100 %");
        assert_eq!(percent(0.21), "21 %");
        assert_eq!(percent(0.0), "0.0 %");
        assert_eq!(decibels(1.0), "0.00 dB");
        assert_eq!(decibels(0.0), "-inf dB");
        assert!((gain_from_position(gain_position(2.0)) - 2.0).abs() < 1e-9);
        assert_eq!(gain_from_position(0.0), 0.0);
    }
}
