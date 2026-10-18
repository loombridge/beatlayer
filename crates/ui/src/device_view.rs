//! The Device View (WP-2.4; layout L-DEV-001 to L-DEV-009): the selected
//! track's device chain, left to right in signal order, each device in a
//! frame with Live's title bar.
//!
//! The title bar, as observed in Live 12.4.6: the Device Activator at the
//! left, the device's name (the Device Title Bar, which selects the device
//! on a click, folds it on a double-click, and moves it on a drag), and at
//! the right the Hot-Swap Presets, Save Preset, and Show Options buttons.
//! A folded device is a strip a title bar wide with the title bar on its
//! side. Show Options and a right-click open the device's context menu
//! (C-DEVICE). An empty chain says "Drop an Instrument or Sample Here".
//!
//! A floating device (WP-2.5b, IMP-14) keeps its place in the chain as a
//! strip like a folded device's, outlined in the accent, so the signal
//! order stays readable; its window draws the same frame at the window's
//! size, with a Dock button at the title bar's right end
//! ([`DeviceViewUi::show_window`]).
//! Every action runs a registered command (`device.*`), so the keyboard,
//! a screen reader, and an agent reach the same ones.

use std::f32::consts::FRAC_PI_2;

use bl_model::{AutomationTarget, DeviceId, DeviceRole, TrackId};
use bl_shell::context_menus;
use bl_shell::device_commands::{
    self as ids, DEVICE_GAP, DeviceFrame, DeviceViewState, FloatingWindow, TITLE_BAR_HEIGHT,
};
use bl_shell::menus::{MenuItem, shown};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, DragAndDrop, Id, Key, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::keymap::{Keymap, Platform};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// What an empty chain's Device View says (L-DEV-001, observed).
pub const EMPTY_TEXT: &str = "Drop an Instrument or Sample Here";

/// A device being dragged by its title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeviceDrag {
    device: DeviceId,
}

/// The Device View's state between frames: a name being typed and the
/// scroll.
pub struct DeviceViewUi {
    renaming: Option<Renaming>,
    scroll: f32,
    keymap: Keymap,
    platform: Platform,
    menu: Vec<MenuItem>,
}

struct Renaming {
    device: DeviceId,
    text: String,
    focused: bool,
}

impl std::fmt::Debug for DeviceViewUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceViewUi")
            .field("renaming", &self.renaming.as_ref().map(|r| r.device))
            .field("scroll", &self.scroll)
            .finish_non_exhaustive()
    }
}

impl Default for DeviceViewUi {
    fn default() -> Self {
        Self::new()
    }
}

/// The ID of a device's frame.
#[must_use]
pub fn frame_id(device: DeviceId) -> Id {
    Id::new(("device-frame", device))
}

/// The ID of a floating device's frame in its window (WP-2.5b).
#[must_use]
pub fn window_frame_id(device: DeviceId) -> Id {
    Id::new(("device-window", device))
}

/// Where a floating device's Dock button sits in its window at `rect`.
#[must_use]
pub fn dock_rect(rect: Rect) -> Rect {
    window_layout(rect, false)
        .1
        .dock
        .expect("a window has a Dock button")
}

/// The ID of one of a device's title-bar controls: `activator`, `title`,
/// `hot_swap`, `save`, or `options`.
#[must_use]
pub fn control_id(device: DeviceId, control: &str) -> Id {
    Id::new(("device-control", device, control))
}

/// Where each device's frame sits in a Device View at `rect` scrolled by
/// `scroll` points: left to right from the view's left edge, `DEVICE_GAP`
/// apart, each its frame's width ([`ids::frame_width`]: a described
/// device's layout's docked width, its sidechain strip and section, or a
/// title bar wide folded), and as tall as the view less the gap above and
/// below.
#[must_use]
pub fn frames(state: &DeviceViewState, rect: Rect, scroll: f32) -> Vec<(DeviceId, Rect)> {
    let mut x = rect.left() + DEVICE_GAP - scroll;
    let (top, bottom) = (rect.top() + DEVICE_GAP, rect.bottom() - DEVICE_GAP);
    state
        .devices
        .iter()
        .map(|device| {
            let width = ids::frame_width(device);
            let frame = Rect::from_min_max(pos2(x, top), pos2(x + width, bottom));
            x += width + DEVICE_GAP;
            (device.id, frame)
        })
        .collect()
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// A sample dropped from Finder or Explorer onto a device that plays one
/// loads into it, as Live's Simpler takes one (`device.load_sample`; the
/// sampler spec, R11).
fn sample_drop(
    ui: &Ui,
    state: &DeviceViewState,
    placed: &[(DeviceId, Rect)],
    events: &mut Vec<UiEvent>,
) {
    let (dropped, pointer) = ui.input(|i| {
        let dropped: Vec<String> = i
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_string_lossy().into_owned())
            .collect();
        (dropped, i.pointer.latest_pos())
    });
    let (Some(path), Some(pointer)) = (dropped.into_iter().next(), pointer) else {
        return;
    };
    let Some((device, _)) = placed.iter().find(|(_, rect)| rect.contains(pointer)) else {
        return;
    };
    let takes = state
        .devices
        .iter()
        .find(|frame| frame.id == *device)
        .is_some_and(|frame| frame.takes_sample);
    if !takes {
        return;
    }
    // Dropped on a sample slot's cell, it loads into that slot; anywhere
    // else on the device, into the selected one (WP-5.17).
    let slot = state
        .devices
        .iter()
        .find(|frame| frame.id == *device)
        .and_then(|frame| {
            let panel = frame.panel.as_ref()?;
            let rect = placed.iter().find(|(id, _)| id == device)?.1;
            let mut body = crate::device_panel::body_rect(rect, TITLE_BAR_HEIGHT);
            body.min.x += ids::sidechain_width(frame);
            let (_, cells) = crate::device_panel::place_shown(panel, frame.expanded, body);
            cells
                .into_iter()
                .find_map(|(_, cell, slot)| slot.filter(|_| cell.contains(pointer)))
        });
    let mut args = json!({ "device": device, "path": path });
    if let Some(slot) = slot {
        args["slot"] = json!(slot);
    }
    invoke(events, ids::LOAD_SAMPLE, args);
}

impl DeviceViewUi {
    /// A Device View with nothing being typed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            renaming: None,
            scroll: 0.0,
            keymap: Keymap::live12(),
            platform: Platform::current(),
            menu: context_menus::device(),
        }
    }

    /// Whether a device's name is being typed, so keys go to it.
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.renaming.is_some()
    }

    /// Starts typing a new name for `device` in its title bar (Rename,
    /// S-EDIT-008), from its title now.
    pub fn start_rename(&mut self, device: DeviceId, title: &str) {
        self.renaming = Some(Renaming {
            device,
            text: title.to_owned(),
            focused: false,
        });
    }

    /// Draws the Device View in `rect`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let state = shell.device_view_state();
        let region = Id::new("region-device-view");
        let background = ui.interact(rect, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label("Device View".to_owned());
            node.set_bounds(egui::accesskit::Rect {
                x0: rect.min.x.into(),
                y0: rect.min.y.into(),
                x1: rect.max.x.into(),
                y1: rect.max.y.into(),
            });
            if state.devices.is_empty() {
                node.set_description(EMPTY_TEXT.to_owned());
            }
        });
        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);

        // The chain scrolls sideways when it's wider than the view.
        let content = state
            .devices
            .iter()
            .fold(DEVICE_GAP, |w, d| w + ids::frame_width(d) + DEVICE_GAP);
        if background.hovered() || ui.rect_contains_pointer(rect) {
            let delta = ui.input(|i| i.smooth_scroll_delta);
            self.scroll -= if delta.x == 0.0 { delta.y } else { delta.x };
        }
        self.scroll = self.scroll.clamp(0.0, (content - rect.width()).max(0.0));

        if state.devices.is_empty() {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                EMPTY_TEXT,
                theme.small_font(),
                theme.colors.text_dim.0,
            );
        }

        let placed = frames(&state, rect, self.scroll);
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).accessibility_parent(region),
            |ui| {
                ui.set_clip_rect(rect);
                for (frame, (id, at)) in state.devices.iter().zip(&placed) {
                    debug_assert_eq!(frame.id, *id);
                    self.device(ui, shell, theme, frame, *at, false, events);
                }
            },
        );
        self.drop_target(ui, shell, theme, &state, &placed, rect, events);
        sample_drop(ui, &state, &placed, events);
    }

    /// Draws a floating device's window (WP-2.5b): the device's frame at
    /// the window's size, its title bar with a Dock button at the right
    /// end, and its body below, where only its display cells take the
    /// extra room (IMP-14). Where the window draws its own utility title
    /// bar (macOS), `title_strip` is its height in layout points, and the
    /// window's title is drawn centred in it; the system's close button
    /// sits at its left.
    pub fn show_window(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        window: &FloatingWindow,
        title_strip: f32,
        events: &mut Vec<UiEvent>,
    ) {
        let whole = ui.max_rect();
        let c = &theme.colors;
        ui.painter().rect_filled(whole, 0.0, c.panel.0);
        let mut rect = whole;
        if title_strip > 0.0 {
            let strip = Rect::from_min_size(whole.min, vec2(whole.width(), title_strip));
            ui.painter().rect_filled(strip, 0.0, c.window.0);
            ui.painter().text(
                strip.center(),
                Align2::CENTER_CENTER,
                &window.title,
                theme.small_font(),
                c.text_dim.0,
            );
            rect.min.y = strip.bottom();
        }
        self.device(ui, shell, theme, &window.view, rect, true, events);
    }

    /// One device's frame: in the chain, or filling its floating window
    /// (`window`).
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn device(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        rect: Rect,
        window: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        // A floating device's place in the chain is a strip (IMP-14).
        let strip = !window && (frame.folded || frame.floating);
        let id = if window {
            window_frame_id(frame.id)
        } else {
            frame_id(frame.id)
        };
        let _ = ui.interact(rect, id, Sense::hover());
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Group);
            node.set_label(frame.title_text());
            node.set_description(format!(
                "{}, {}{}",
                frame.role.label(),
                if frame.enabled { "on" } else { "off" },
                if window {
                    ""
                } else if frame.floating {
                    ", floating"
                } else if frame.folded {
                    ", folded"
                } else {
                    ""
                }
            ));
        });
        let painter = ui.painter_at(rect);
        // The body; a folded device is all title bar.
        painter.rect_filled(rect, theme.sizes.corner, c.lane.0);
        let bar_colour = if frame.selected {
            c.text_dim.0
        } else {
            c.control.0
        };
        let text_colour = if frame.selected { c.window.0 } else { c.text.0 };
        let (bar, controls) = if strip {
            folded_layout(rect)
        } else if window {
            window_layout(rect, frame.expandable)
        } else {
            unfolded_layout(rect, frame.expandable)
        };
        painter.rect_filled(bar, theme.sizes.corner, bar_colour);
        if frame.floating && !window {
            // Drawn as floating: outlined in the accent.
            painter.rect_stroke(
                rect.shrink(0.5),
                theme.sizes.corner,
                Stroke::new(1.0, c.accent.0),
                StrokeKind::Inside,
            );
        }

        ui.scope_builder(
            UiBuilder::new().max_rect(rect).accessibility_parent(id),
            |ui| {
                // Spec: device-framework R15. A described device's body is
                // its generic panel, at its layout's docked size.
                if frame.panel.is_some() && !strip {
                    // Right of the sidechain strip and section (WP-2.19b).
                    let mut body = crate::device_panel::body_rect(rect, TITLE_BAR_HEIGHT);
                    body.min.x += ids::sidechain_width(frame);
                    let display = shell.device_display(frame.id);
                    let track = shell.editor().document().device_track(frame.id);
                    let played = |param| {
                        let device = frame.id;
                        track.and_then(|track| {
                            shell.played(track, &AutomationTarget::Device { device, param })
                        })
                    };
                    crate::device_panel::show(
                        ui,
                        theme,
                        frame,
                        display.as_ref(),
                        body,
                        &played,
                        events,
                    );
                }
                self.title(
                    ui,
                    shell,
                    theme,
                    frame,
                    (controls.title, strip, window),
                    text_colour,
                    events,
                );
                self.activator(ui, theme, frame, controls.activator, events);
                if let Some(expand) = controls.expand {
                    self.round_button(ui, theme, frame, "expand", expand, events);
                }
                self.round_button(ui, theme, frame, "hot_swap", controls.hot_swap, events);
                self.round_button(ui, theme, frame, "save", controls.save, events);
                let options =
                    self.round_button(ui, theme, frame, "options", controls.options, events);
                egui::Popup::menu(&options).show(|ui| self.menu(ui, shell, frame, events));
                if let Some(dock) = controls.dock {
                    self.round_button(ui, theme, frame, "dock", dock, events);
                }
                if let (Some(section), false) = (&frame.sidechain, strip) {
                    crate::sidechain::draw(ui, shell, theme, frame.id, section, rect, events);
                }
                if let (Some(section), false) = (&frame.external, strip) {
                    crate::external::draw(ui, shell, theme, frame.id, section, rect, events);
                }
            },
        );
    }

    /// The Device Title Bar: the name, which selects, folds, drags, and
    /// opens the context menu.
    #[expect(clippy::too_many_arguments, reason = "the title's inputs")]
    fn title(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        (rect, strip, window): (Rect, bool, bool),
        colour: Color32,
        events: &mut Vec<UiEvent>,
    ) {
        let id = if window {
            control_id(frame.id, "window-title")
        } else {
            control_id(frame.id, "title")
        };
        if let Some(renaming) = self.renaming.as_mut().filter(|r| r.device == frame.id) {
            let edit = ui.put(
                rect,
                egui::TextEdit::singleline(&mut renaming.text)
                    .id(id)
                    .font(theme.small_font())
                    .margin(vec2(2.0, 0.0)),
            );
            if !renaming.focused {
                edit.request_focus();
                renaming.focused = true;
            }
            let (enter, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if edit.lost_focus() || enter || escape {
                if enter || (edit.lost_focus() && !escape) {
                    invoke(
                        events,
                        "device.rename",
                        json!({ "device": frame.id, "name": renaming.text }),
                    );
                }
                self.renaming = None;
            }
            return;
        }
        let response = ui.interact(rect, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let painter = ui.painter_at(rect);
        if strip {
            // The name reads upwards on a folded device (observed).
            let galley = painter.layout_no_wrap(frame.title_text(), theme.small_font(), colour);
            let at = pos2(
                rect.center().x - galley.size().y / 2.0,
                rect.top() + galley.size().x.min(rect.height()),
            );
            painter.add(egui::epaint::TextShape::new(at, galley, colour).with_angle(-FRAC_PI_2));
        } else {
            painter.text(
                pos2(rect.left(), rect.center().y),
                Align2::LEFT_CENTER,
                frame.title_text(),
                theme.small_font(),
                colour,
            );
        }
        let select = || json!({ "device": frame.id });
        if response.double_clicked() {
            // Spec: device-chains R11. A double-click folds and unfolds; a
            // floating device's strip and window don't fold.
            if !window && !frame.floating {
                invoke(events, ids::FOLD, json!({ "device": frame.id }));
            }
        } else if (response.clicked()
            || response.secondary_clicked()
            || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
            && !frame.selected
        {
            invoke(events, ids::SELECT, select());
        }
        response.context_menu(|ui| self.menu(ui, shell, frame, events));
        if response.drag_started() && !window {
            DragAndDrop::set_payload(ui.ctx(), DeviceDrag { device: frame.id });
            if !frame.selected {
                invoke(events, ids::SELECT, select());
            }
        }
        focus_ring(ui, theme, rect, &response);
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, frame.selected, frame.title_text())
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(frame.title_text());
            node.set_selected(frame.selected);
            node.set_description(
                if window {
                    "Device Title Bar"
                } else if frame.floating {
                    "Device Title Bar: the device floats in its own window; click to select it, \
                     drag to move it"
                } else {
                    "Device Title Bar: click to select the device, double-click to fold it, drag \
                     to move it"
                }
                .to_owned(),
            );
        });
    }

    /// The Device Activator: a round button, lit while the device is on.
    fn activator(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let id = control_id(frame.id, "activator");
        let response = ui.interact(rect, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            invoke(
                events,
                "device.set_enabled",
                json!({ "device": frame.id, "enabled": !frame.enabled }),
            );
        }
        let c = &theme.colors;
        // Spec: device-chains R6, observed: lit while on, dark when off.
        let fill = if frame.enabled {
            c.accent.0
        } else {
            c.window.0
        };
        let painter = ui.painter_at(rect.expand(2.0));
        painter.circle_filled(rect.center(), rect.width() / 2.0, fill);
        painter.circle_stroke(
            rect.center(),
            rect.width() / 2.0,
            Stroke::new(1.0, c.separator.0),
        );
        focus_ring(ui, theme, rect, &response);
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, frame.enabled, "Device Activator")
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label("Device Activator".to_owned());
            node.set_value(if frame.enabled { "on" } else { "off" }.to_owned());
            node.set_description(
                "Switches the device off and on; off, it passes its input through".to_owned(),
            );
        });
    }

    /// Hot-Swap Presets, Save Preset, or Show Options: round buttons at the
    /// title bar's right end.
    fn round_button(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        which: &'static str,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) -> egui::Response {
        let id = control_id(frame.id, which);
        let response = ui.interact(rect, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let pressed =
            response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
        let (name, on) = match which {
            "hot_swap" => ("Hot-Swap Presets", frame.hot_swap),
            "expand" => ("Expand", frame.expanded),
            "save" => ("Save Preset", false),
            "dock" => ("Dock", false),
            _ => ("Show Options", false),
        };
        if pressed {
            match which {
                "hot_swap" => invoke(events, ids::HOT_SWAP, json!({ "device": frame.id })),
                // Spec: device-chains R34. It behaves as Fold does.
                "expand" => invoke(events, ids::EXPAND, json!({ "device": frame.id })),
                "save" => invoke(events, ids::SAVE_PRESET, json!({ "device": frame.id })),
                // Spec: device-chains R27. One undo step, Dock Device.
                "dock" => invoke(
                    events,
                    ids::SET_FLOATING,
                    json!({ "device": frame.id, "floating": false }),
                ),
                _ => {
                    if !frame.selected {
                        invoke(events, ids::SELECT, json!({ "device": frame.id }));
                    }
                }
            }
        }
        let c = &theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let centre = rect.center();
        let r = rect.width() / 2.0;
        let face = if on {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.window.0
        };
        painter.circle_filled(centre, r, face);
        let ink = if on { c.accent_text.0 } else { c.text_dim.0 };
        let stroke = Stroke::new(1.0, ink);
        match which {
            "hot_swap" => {
                // Circular arrows.
                painter.circle_stroke(centre, r * 0.5, stroke);
                painter.add(Shape::convex_polygon(
                    vec![
                        centre + vec2(r * 0.5, -r * 0.35),
                        centre + vec2(r * 0.85, 0.05),
                        centre + vec2(r * 0.15, 0.05),
                    ],
                    ink,
                    Stroke::NONE,
                ));
            }
            "expand" => {
                // A triangle: pointing right while the part is hidden, down
                // while it shows.
                let (a, b, c) = if on {
                    (vec2(-0.5, -0.3), vec2(0.5, -0.3), vec2(0.0, 0.5))
                } else {
                    (vec2(-0.3, -0.5), vec2(-0.3, 0.5), vec2(0.5, 0.0))
                };
                painter.add(Shape::convex_polygon(
                    vec![centre + a * r, centre + b * r, centre + c * r],
                    ink,
                    Stroke::NONE,
                ));
            }
            "save" => {
                // A disk.
                painter.rect_stroke(
                    Rect::from_center_size(centre, vec2(r, r)),
                    0.5,
                    stroke,
                    StrokeKind::Inside,
                );
            }
            "dock" => {
                // A window going down into its place.
                let tray =
                    Rect::from_center_size(centre + vec2(0.0, r * 0.25), vec2(r * 1.1, r * 0.6));
                painter.rect_stroke(tray, 0.5, stroke, StrokeKind::Inside);
                painter.line_segment(
                    [centre + vec2(0.0, -r * 0.75), centre + vec2(0.0, r * 0.1)],
                    stroke,
                );
            }
            _ => {
                // Three dots.
                for dx in [-0.45f32, 0.0, 0.45] {
                    painter.circle_filled(centre + vec2(dx * r, 0.0), 0.9, ink);
                }
            }
        }
        focus_ring(ui, theme, rect, &response);
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(name.to_owned());
            if which == "hot_swap" || which == "expand" {
                node.set_value(if on { "on" } else { "off" }.to_owned());
            }
            if which == "expand" {
                node.set_description("Shows or hides the device's expanded part".to_owned());
            }
            if which == "dock" {
                node.set_description("Docks the device back in its chain".to_owned());
            }
        });
        response
    }

    /// The device's context menu (C-DEVICE): each item runs what its
    /// shortcut runs in the Device View, or names the package that builds
    /// it.
    fn menu(&self, ui: &mut Ui, shell: &Shell, frame: &DeviceFrame, events: &mut Vec<UiEvent>) {
        for item in shown(&self.menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            let response = crate::menus::item(ui, item, shell, &self.keymap, self.platform, events);
            let response = match context_menus::pending_package(item.row) {
                Some(package) => {
                    let text = format!("Not built yet: arrives with {package}.");
                    ui.ctx().accesskit_node_builder(response.id, |node| {
                        node.set_description(text.clone());
                    });
                    response.on_disabled_hover_text(text)
                }
                None => response,
            };
            crate::menus::take_focus_first(ui, &response);
        }
        // The device's own items follow Save as Default Preset
        // (C-DEVICE-015): each switch ticked while it's on, and choosing it
        // turns it over, one undo step like any parameter change.
        let options = frame.menu_options();
        if !options.is_empty() {
            ui.separator();
        }
        for option in options {
            let text = if option.on {
                format!("✔ {}", option.name)
            } else {
                option.name.to_owned()
            };
            let response = ui.button(text);
            crate::menus::register_item(ui.ctx(), &response);
            if response.clicked() {
                let value = if option.on { 0.0 } else { 1.0 };
                invoke(
                    events,
                    "device.set_param",
                    json!({ "device": frame.id, "param": option.param, "value": value }),
                );
                ui.close();
            }
            crate::menus::take_focus_first(ui, &response);
        }
    }

    /// Dragging a device's title bar moves it: a line shows where it would
    /// go, at places that keep the chain in order, and the drop runs
    /// `device.move`.
    #[expect(clippy::too_many_arguments, reason = "the drop's inputs")]
    fn drop_target(
        &self,
        ui: &Ui,
        shell: &Shell,
        theme: &Theme,
        state: &DeviceViewState,
        placed: &[(DeviceId, Rect)],
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(drag) = DragAndDrop::payload::<DeviceDrag>(ui.ctx()) else {
            return;
        };
        let (Some(pointer), Some(track)) = (ui.input(|i| i.pointer.interact_pos()), state.track)
        else {
            return;
        };
        if !rect.contains(pointer) {
            return;
        }
        let Some(index) = move_index(shell, state, placed, drag.device, track, pointer.x) else {
            return;
        };
        let gap = index.gap;
        let x = match placed.get(gap) {
            Some((_, frame)) => frame.left() - DEVICE_GAP / 2.0,
            None => placed
                .last()
                .map_or(rect.left() + DEVICE_GAP / 2.0, |(_, f)| {
                    f.right() + DEVICE_GAP / 2.0
                }),
        };
        ui.painter().line_segment(
            [pos2(x, rect.top() + 2.0), pos2(x, rect.bottom() - 2.0)],
            Stroke::new(2.0, theme.colors.accent.0),
        );
        if ui.input(|i| i.pointer.any_released()) {
            DragAndDrop::clear_payload(ui.ctx());
            if index.changes {
                invoke(
                    events,
                    "device.move",
                    json!({ "device": drag.device, "track": track, "index": index.index }),
                );
            }
        }
    }
}

/// Where a dragged device would go.
struct MoveIndex {
    /// The gap between frames the pointer is at: 0 before the first.
    gap: usize,
    /// The index `device.move` takes, counted without the device.
    index: usize,
    /// Whether it moves at all.
    changes: bool,
}

/// Where a device dragged to `x` lands in the shown chain, if that keeps
/// the chain in Live's order (the device chains spec, R2 and R3).
fn move_index(
    shell: &Shell,
    state: &DeviceViewState,
    placed: &[(DeviceId, Rect)],
    device: DeviceId,
    track: TrackId,
    x: f32,
) -> Option<MoveIndex> {
    let doc = shell.editor().document();
    let role = doc.device(device)?.role;
    let kind = state.track_kind?;
    if !role.fits(kind) {
        return None;
    }
    let gap = placed.iter().filter(|(_, f)| f.center().x < x).count();
    let from = state.devices.iter().position(|d| d.id == device);
    let index = match from {
        Some(from) if from < gap => gap - 1,
        _ => gap,
    };
    let others: Vec<DeviceRole> = state
        .devices
        .iter()
        .filter(|d| d.id != device)
        .map(|d| d.role)
        .collect();
    let fits = others[..index].iter().all(|r| *r <= role)
        && others[index..].iter().all(|r| *r >= role)
        && !(role == DeviceRole::Instrument
            && others.contains(&DeviceRole::Instrument)
            && doc.device_track(device) != Some(track));
    fits.then_some(MoveIndex {
        gap,
        index,
        changes: from != Some(index) || doc.device_track(device) != Some(track),
    })
}

/// Where a title bar's controls sit.
struct Controls {
    activator: Rect,
    /// The expand toggle, right of the activator, on a device that has an
    /// expanded part (L-DEV-004).
    expand: Option<Rect>,
    title: Rect,
    hot_swap: Rect,
    save: Rect,
    options: Rect,
    /// A floating window's Dock button, at the right end.
    dock: Option<Rect>,
}

/// The size of a title bar's round buttons.
const BUTTON: f32 = 11.0;

/// An unfolded frame's title bar along its top: the activator, the expand
/// toggle where the device has an expanded part, the name, then Hot-Swap
/// Presets, Save Preset, and Show Options at the right.
fn unfolded_layout(rect: Rect, expandable: bool) -> (Rect, Controls) {
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE_BAR_HEIGHT));
    let y = bar.center().y;
    let button = |x: f32| Rect::from_center_size(pos2(x, y), vec2(BUTTON, BUTTON));
    let step = BUTTON + 3.0;
    let options = button(bar.right() - 2.0 - BUTTON / 2.0);
    let save = button(options.center().x - step);
    let hot_swap = button(save.center().x - step);
    let activator = button(bar.left() + 2.0 + BUTTON / 2.0);
    let expand = expandable.then(|| button(activator.right() + 3.0 + BUTTON / 2.0));
    let title = Rect::from_min_max(
        pos2(expand.unwrap_or(activator).right() + 4.0, bar.top()),
        pos2(hot_swap.left() - 4.0, bar.bottom()),
    );
    (
        bar,
        Controls {
            activator,
            expand,
            title,
            hot_swap,
            save,
            options,
            dock: None,
        },
    )
}

/// A floating window's frame: the unfolded title bar along its top, with
/// the Dock button at its right end, after Show Options.
fn window_layout(rect: Rect, expandable: bool) -> (Rect, Controls) {
    let step = BUTTON + 3.0;
    let mut narrower = rect;
    narrower.max.x -= step;
    let (_, mut controls) = unfolded_layout(narrower, expandable);
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE_BAR_HEIGHT));
    controls.dock = Some(Rect::from_center_size(
        pos2(bar.right() - 2.0 - BUTTON / 2.0, bar.center().y),
        vec2(BUTTON, BUTTON),
    ));
    (bar, controls)
}

/// A folded frame: the title bar turned on its side, the activator at the
/// top, the name reading upwards, and the buttons at the bottom.
fn folded_layout(rect: Rect) -> (Rect, Controls) {
    let x = rect.center().x;
    let button = |y: f32| Rect::from_center_size(pos2(x, y), vec2(BUTTON, BUTTON));
    let step = BUTTON + 3.0;
    let options = button(rect.bottom() - 2.0 - BUTTON / 2.0);
    let save = button(options.center().y - step);
    let hot_swap = button(save.center().y - step);
    let activator = button(rect.top() + 2.0 + BUTTON / 2.0);
    let title = Rect::from_min_max(
        pos2(rect.left(), activator.bottom() + 4.0),
        pos2(rect.right(), hot_swap.top() - 4.0),
    );
    (
        rect,
        Controls {
            activator,
            expand: None,
            title,
            hot_swap,
            save,
            options,
            dock: None,
        },
    )
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

/// The orange bar at the top of the browser while a device is hot-swapped
/// (observed: "Swapping Audio Effect", with a close button that ends
/// hot-swapping). Returns the rest of `rect`, for the browser.
pub fn hot_swap_bar(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    events: &mut Vec<UiEvent>,
) -> Rect {
    let Some(hot) = shell.device_view_state().hot_swap else {
        return rect;
    };
    let height = 20.0;
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), height));
    let c = &theme.colors;
    ui.painter().rect_filled(bar, 0.0, c.accent.0);
    ui.painter().text(
        pos2(bar.left() + 6.0, bar.center().y),
        Align2::LEFT_CENTER,
        &hot.label,
        theme.small_font(),
        c.accent_text.0,
    );
    let close = Rect::from_center_size(pos2(bar.right() - 10.0, bar.center().y), vec2(14.0, 14.0));
    let id = Id::new("hot-swap-close");
    let response = ui.interact(close, id, Sense::click());
    ui.painter().text(
        close.center(),
        Align2::CENTER_CENTER,
        "×",
        theme.small_font(),
        c.accent_text.0,
    );
    if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
        invoke(events, ids::HOT_SWAP, json!({ "on": false }));
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Close Hot-Swap"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label("Close Hot-Swap".to_owned());
        node.set_description(hot.label.clone());
    });
    Rect::from_min_max(pos2(rect.left(), bar.bottom()), rect.max)
}

/// The Device View's frames, for tests and view captures: what
/// [`DeviceViewUi::show`] draws at `rect` unscrolled.
#[must_use]
pub fn layout(shell: &Shell, rect: Rect) -> Vec<(DeviceId, Rect)> {
    frames(&shell.device_view_state(), rect, 0.0)
}

/// Where a device's Device Activator sits in its frame.
#[must_use]
pub fn activator_rect(frame: Rect, folded: bool) -> Rect {
    if folded {
        folded_layout(frame).1.activator
    } else {
        unfolded_layout(frame, false).1.activator
    }
}

/// Where a device's title (its Device Title Bar's name) sits in its frame.
#[must_use]
pub fn title_rect(frame: Rect, folded: bool) -> Rect {
    if folded {
        folded_layout(frame).1.title
    } else {
        unfolded_layout(frame, false).1.title
    }
}

/// Where a device's three round buttons sit in its frame: Hot-Swap
/// Presets, Save Preset, and Show Options.
#[must_use]
pub fn button_rects(frame: Rect, folded: bool) -> [Rect; 3] {
    let controls = if folded {
        folded_layout(frame).1
    } else {
        unfolded_layout(frame, false).1
    };
    [controls.hot_swap, controls.save, controls.options]
}
