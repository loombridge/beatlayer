//! The context menu of a mixer control and of a device parameter (menus.md
//! C-PAN, C-PARAM), in Live's order, and what a click on the control does
//! while Automation Mode is on (the automation spec, R29).
//!
//! The pan control adds the pan mode to switch to; every control has Show
//! Automation, Show Automation In New Lane, the modulation source and the
//! maps; a parameter inside a rack adds Edit Macro Map and Map to Macro 1
//! to 16. Delete Automation and Re-Enable Automation come first while they
//! apply. Each runs registered commands, the ones an agent runs.

use bl_model::{AutomationTarget, MixerParam, PanMode, TrackId};
use bl_shell::arrangement_commands as view_ids;
use bl_shell::context_menus::{self, MAP_TO_MACRO_ROWS};
use bl_shell::{Invocation, Shell};
use egui::{Id, PointerButton, Rect, Sense, Ui};
use serde_json::json;

use crate::keymap::{Keymap, Platform};
use crate::main_window::UiEvent;

/// What the menu of one automatable control needs to know, as the frame saw
/// it.
#[derive(Clone, Copy)]
pub struct ParamMenu<'a> {
    shell: &'a Shell,
    track: TrackId,
    target: AutomationTarget,
    /// Whether the parameter has an envelope.
    has: bool,
    /// Whether its envelope is overridden by a hand move.
    overridden: bool,
    /// Whether Automation Mode is on.
    mode: bool,
    /// The lane of the track that shows the parameter, 0 being its main lane.
    lane: Option<usize>,
    /// Whether the track's lanes of its own are hidden.
    hidden: bool,
    /// The pan control's menu, with whether the split mode is in force.
    pan: Option<bool>,
    /// For a device's parameter, the device.
    device: Option<bl_model::DeviceId>,
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

impl<'a> ParamMenu<'a> {
    /// The menu of `target` on `track`.
    #[must_use]
    pub fn new(shell: &'a Shell, track: TrackId, target: AutomationTarget) -> Self {
        let view = shell.arrangement();
        let doc = shell.editor().document();
        let envelope = doc
            .track(track)
            .and_then(|owner| owner.automation_for(&target));
        let pan = (target
            == AutomationTarget::Mixer {
                param: MixerParam::Pan,
            })
        .then(|| {
            doc.track(track)
                .is_some_and(|t| t.mixer().pan_mode == PanMode::Split)
        });
        let device = match target {
            AutomationTarget::Device { device, .. } => Some(device),
            _ => None,
        };
        Self {
            shell,
            track,
            target,
            has: envelope.is_some(),
            overridden: envelope.is_some_and(|automation| !automation.enabled),
            mode: view.automation_mode(),
            lane: view.track_lanes(track).lane_showing(target),
            hidden: view.own_lanes_hidden(track),
            pan,
            device,
        }
    }

    /// The items, in a menu the caller opened. `after_own` is whether the
    /// control has items of its own above them, which a separator sets off.
    pub fn items(&self, ui: &mut Ui, after_own: bool, events: &mut Vec<UiEvent>) {
        if after_own {
            ui.separator();
        }
        let keymap = Keymap::from_data(self.shell.keymap().clone());
        let platform = Platform::current();
        let (track, target) = (self.track, self.target);
        // The rack the device sits in, with its macro count and whether it
        // is in Map Mode.
        let doc = self.shell.editor().document();
        let rack = self
            .device
            .and_then(|device| doc.device_place(device))
            .and_then(|place| place.chain)
            .and_then(|chain| doc.chain_rack(chain))
            .map(|rack| {
                (
                    rack.id,
                    self.shell.devices().views_of(rack.id).macro_count,
                    self.shell.devices().map_mode() == Some(rack.id),
                )
            });
        let menu = context_menus::param(context_menus::ParamMenu {
            pan: self.pan,
            has_envelope: self.has,
            overridden: self.overridden,
            device: self.device.is_some(),
            in_rack: rack.is_some(),
        });
        for item in bl_shell::menus::shown(&menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            let row = item.row;
            let click = |ui: &mut Ui, enabled: bool, checked: Option<bool>| {
                let text = match checked {
                    Some(true) => format!("\u{2714} {}", item.label),
                    _ => item.label.to_owned(),
                };
                let response = ui.add_enabled(enabled, egui::Button::new(text));
                crate::menus::register_item(ui.ctx(), &response);
                crate::menus::take_focus_first(ui, &response);
                response.clicked()
            };
            match row {
                "C-PAN-001" => {
                    let mode = if self.pan == Some(true) {
                        PanMode::Stereo
                    } else {
                        PanMode::Split
                    };
                    if click(ui, true, None) {
                        invoke(
                            events,
                            "mixer.set_pan_mode",
                            json!({ "track": track, "mode": mode }),
                        );
                        ui.close();
                    }
                }
                "C-PARAM-001" => {
                    if click(ui, self.has, None) {
                        invoke(
                            events,
                            "automation.delete",
                            json!({ "track": track, "target": target }),
                        );
                        ui.close();
                    }
                }
                "C-PARAM-002" => {
                    if click(ui, self.overridden, None) {
                        invoke(
                            events,
                            "automation.re_enable_for",
                            json!({ "track": track, "target": target }),
                        );
                        ui.close();
                    }
                }
                "C-PARAM-003" | "C-PARAM-004" => {
                    if click(ui, true, None) {
                        self.show(row == "C-PARAM-004", events);
                        ui.close();
                    }
                }
                // Edit MIDI Map and Edit Key Map run the maps' keys.
                "C-PARAM-006" | "C-PARAM-007" => {
                    let response =
                        crate::menus::item(ui, item, self.shell, &keymap, platform, events);
                    crate::menus::take_focus_first(ui, &response);
                }
                "C-PARAM-008" => {
                    if let Some((rack, _, on)) = rack
                        && click(ui, true, Some(on))
                    {
                        invoke(events, "rack.map_mode", json!({ "rack": rack, "on": !on }));
                        ui.close();
                    }
                }
                _ if MAP_TO_MACRO_ROWS.contains(&row) => {
                    let number = MAP_TO_MACRO_ROWS
                        .iter()
                        .position(|each| *each == row)
                        .map_or(1, |at| at as u8 + 1);
                    if let Some((rack, count, _)) = rack
                        && let Some(device) = self.device
                        && let AutomationTarget::Device { param, .. } = target
                        && click(ui, number <= count, None)
                    {
                        invoke(
                            events,
                            "rack.map_macro",
                            json!({
                                "rack": rack,
                                "macro_number": number,
                                "target": { "type": "device", "device": device, "param": param },
                            }),
                        );
                        ui.close();
                    }
                }
                // Dim: Beatlayer has no feature behind them.
                _ => {
                    let response = ui.add_enabled(false, egui::Button::new(item.label));
                    crate::menus::register_item(ui.ctx(), &response);
                    if let Some(package) = context_menus::pending_package(row) {
                        response.on_disabled_hover_text(format!(
                            "Not built yet: arrives with {package}."
                        ));
                    }
                }
            }
        }
    }

    /// Shows the parameter: in the track's main lane, or in a lane of its
    /// own, and selects the lane it has now. Show Automation switches
    /// Automation Mode on (R29).
    pub fn show(&self, new_lane: bool, events: &mut Vec<UiEvent>) {
        let (track, target) = (self.track, self.target);
        if !self.mode {
            invoke(events, view_ids::AUTOMATION_MODE, json!({ "on": true }));
        }
        let lane = if new_lane && self.lane.is_none_or(|lane| lane == 0) {
            invoke(
                events,
                view_ids::ADD_AUTOMATION_LANE,
                json!({ "track": track, "target": target }),
            );
            return;
        } else if let Some(lane) = self.lane {
            lane
        } else {
            invoke(
                events,
                view_ids::SHOW_AUTOMATION,
                json!({ "track": track, "target": target }),
            );
            0
        };
        if lane > 0 && self.hidden {
            invoke(
                events,
                view_ids::SHOW_AUTOMATION_LANES,
                json!({ "tracks": [track], "on": true }),
            );
        }
        invoke(
            events,
            view_ids::SELECT_TRACKS,
            json!({ "tracks": [track], "automation_lane": lane }),
        );
    }

    /// What a click on the control does while Automation Mode is on: the
    /// parameter shows in the main lane, or its lane is selected (R29).
    pub fn click(&self, ui: &Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        if self.mode
            && ui.rect_contains_pointer(rect)
            && ui.input(|i| i.pointer.button_clicked(PointerButton::Primary))
        {
            self.show(false, events);
        }
    }

    /// Opens the menu on a right-click in `rect`, for a control that has no
    /// menu of its own, and shows it; also the click's effect. `id` is the
    /// control's own widget ID, which the menu keys apply to.
    pub fn overlay(&self, ui: &mut Ui, rect: Rect, id: Id, events: &mut Vec<UiEvent>) {
        // A hover-only widget takes no clicks from the control under it.
        let response = ui.interact(rect, id.with("param-menu"), Sense::hover());
        if response.contains_pointer()
            && ui.input(|i| i.pointer.button_clicked(PointerButton::Secondary))
            && let Some(at) = ui.input(|i| i.pointer.interact_pos())
        {
            crate::menus::open_keyboard_context_menu(ui.ctx(), response.id, at);
        } else if ui.memory(|m| m.has_focus(id)) && crate::menus::context_menu_keys(ui) {
            // Spec: accessibility R8 (S-CTX-002): the keys on the control.
            crate::menus::open_keyboard_context_menu(ui.ctx(), response.id, rect.left_bottom());
        }
        crate::menus::keyboard_context_menu(&response, |ui| self.items(ui, false, events));
        self.click(ui, rect, events);
    }
}
