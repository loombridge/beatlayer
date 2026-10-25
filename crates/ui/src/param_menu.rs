//! The automation items of a mixer control's and a device parameter's
//! context menu, and what a click on one does while Automation Mode is on
//! (the automation spec, R29).
//!
//! The items come after any of the control's own (C-PAN, C-XFADE): Delete
//! Automation, Re-Enable Automation, Show Automation and Show Automation In
//! New Lane. Each runs registered commands, the ones an agent runs.

use bl_model::{AutomationTarget, TrackId};
use bl_shell::arrangement_commands as view_ids;
use bl_shell::{Invocation, Shell};
use egui::{Id, PointerButton, Rect, Sense, Ui};
use serde_json::json;

use crate::main_window::UiEvent;

/// What the menu of one automatable control needs to know, as the frame saw
/// it.
#[derive(Clone, Copy, Debug)]
pub struct ParamMenu {
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
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

impl ParamMenu {
    /// The menu of `target` on `track`.
    #[must_use]
    pub fn new(shell: &Shell, track: TrackId, target: AutomationTarget) -> Self {
        let view = shell.arrangement();
        let envelope = shell
            .editor()
            .document()
            .track(track)
            .and_then(|owner| owner.automation_for(&target));
        Self {
            track,
            target,
            has: envelope.is_some(),
            overridden: envelope.is_some_and(|automation| !automation.enabled),
            mode: view.automation_mode(),
            lane: view.track_lanes(track).lane_showing(target),
            hidden: view.own_lanes_hidden(track),
        }
    }

    /// The items, in a menu the caller opened. `after_own` is whether the
    /// control has items of its own above them, which a separator sets off.
    pub fn items(&self, ui: &mut Ui, after_own: bool, events: &mut Vec<UiEvent>) {
        if after_own {
            ui.separator();
        }
        let track = self.track;
        let target = self.target;
        if ui
            .add_enabled(self.has, egui::Button::new("Delete Automation"))
            .clicked()
        {
            invoke(
                events,
                "automation.delete",
                json!({ "track": track, "target": target }),
            );
            ui.close();
        }
        if ui
            .add_enabled(self.overridden, egui::Button::new("Re-Enable Automation"))
            .clicked()
        {
            invoke(
                events,
                "automation.re_enable_for",
                json!({ "track": track, "target": target }),
            );
            ui.close();
        }
        if ui.button("Show Automation").clicked() {
            self.show(false, events);
            ui.close();
        }
        if ui.button("Show Automation In New Lane").clicked() {
            self.show(true, events);
            ui.close();
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
