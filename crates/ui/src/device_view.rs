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
//! A plug-in device (WP-2.10, L-DEV-010) adds the window button, a
//! wrench named Show/Hide Plug-In Window, after the activator and before
//! the name, lit while the plug-in's editor window is open; its panel is
//! eight knobs mapped to the plug-in's first parameters (D19, IMP-03).
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

/// The plug-in window button's name, as Live's Info View names it
/// (L-DEV-010).
pub const PLUGIN_WINDOW_BUTTON: &str = "Show/Hide Plug-In Window";

/// `plugin.show_window`, which the window button runs.
const PLUGIN_SHOW_WINDOW: &str = bl_shell::plugin_commands::SHOW_WINDOW;

/// What an empty chain's Device View says (L-DEV-001, observed).
pub const EMPTY_TEXT: &str = "Drop an Instrument or Sample Here";

/// A device being dragged by its title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeviceDrag {
    pub(crate) device: DeviceId,
}

/// The Device View's state between frames: a name being typed and the
/// scroll.
pub struct DeviceViewUi {
    multisampler: crate::multisampler::MultisamplerUi,
    /// Snapshot's sample displays (WP-5.15).
    sample_displays: crate::sample_display::Displays,
    renaming: Option<Renaming>,
    pub(crate) renaming_chain: Option<crate::rack_view::ChainRenaming>,
    scroll: f32,
    /// The device that was selected when the last frame was drawn, so a new
    /// selection scrolls into view (racks spec, R25).
    seen_selected: Option<DeviceId>,
    /// How far each rack's chain list is scrolled, in points.
    pub(crate) chain_scrolls: std::collections::HashMap<DeviceId, f32>,
    pub(crate) keymap: Keymap,
    pub(crate) platform: Platform,
    menu: Vec<MenuItem>,
    rack_menu: Vec<MenuItem>,
    pub(crate) chain_menu: Vec<MenuItem>,
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
    window_layout(rect, false, false)
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
    rect: Rect,
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
    // Spec: multisampler R47. An SFZ instrument dropped on a multisampler
    // adds its regions as layers; anywhere else in a MIDI track's Device
    // View it makes a multisampler on the track.
    if crate::browser::is_sfz(&path) {
        if !rect.contains(pointer) {
            return;
        }
        let over = placed
            .iter()
            .find(|(_, frame)| frame.contains(pointer))
            .filter(|(device, _)| {
                state
                    .devices
                    .iter()
                    .any(|frame| frame.id == *device && frame.multisampler.is_some())
            });
        match (over, state.track, state.track_kind) {
            (Some((device, _)), _, _) => invoke(
                events,
                bl_shell::multisampler_commands::IMPORT_SFZ,
                json!({ "path": path, "device": device }),
            ),
            (None, Some(track), Some(bl_model::TrackKind::Midi)) => invoke(
                events,
                bl_shell::multisampler_commands::IMPORT_SFZ,
                json!({ "path": path, "track": track }),
            ),
            _ => {}
        }
        return;
    }
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
            multisampler: crate::multisampler::MultisamplerUi::default(),
            sample_displays: crate::sample_display::Displays::default(),
            renaming: None,
            renaming_chain: None,
            scroll: 0.0,
            seen_selected: None,
            chain_scrolls: std::collections::HashMap::new(),
            keymap: Keymap::live12(),
            platform: Platform::current(),
            menu: context_menus::device(),
            rack_menu: context_menus::rack(),
            chain_menu: context_menus::chain(),
        }
    }

    /// Whether a device's name is being typed, so keys go to it.
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.renaming.is_some() || self.renaming_chain.is_some()
    }

    /// Starts typing a new name for `chain` in its row (Rename, S-EDIT-008),
    /// from its name now.
    pub fn start_chain_rename(&mut self, chain: bl_model::ChainId, name: &str) {
        self.renaming_chain = Some(crate::rack_view::ChainRenaming {
            chain,
            text: name.to_owned(),
            focused: false,
        });
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
        let over_chain_list = ui.ctx().pointer_hover_pos().is_some_and(|pointer| {
            crate::rack_view::over_chain_list(&state, rect, self.scroll, pointer)
        });
        if (background.hovered() || ui.rect_contains_pointer(rect)) && !over_chain_list {
            let delta = ui.input(|i| i.smooth_scroll_delta);
            self.scroll -= if delta.x == 0.0 { delta.y } else { delta.x };
        }
        // Spec: racks R25. A device chosen from the list scrolls into view,
        // wherever in its racks it is.
        let selected = shell.devices().selected();
        if selected != self.seen_selected {
            self.seen_selected = selected;
            let found = selected.and_then(|device| {
                crate::rack_view::all_frames(&state, rect, 0.0)
                    .into_iter()
                    .find(|(id, _)| *id == device)
            });
            if let Some((_, at)) = found {
                let (left, right) = (at.left() - rect.left(), at.right() - rect.left());
                if left < self.scroll {
                    self.scroll = left - DEVICE_GAP;
                } else if right > self.scroll + rect.width() && right - left <= rect.width() {
                    self.scroll = right + DEVICE_GAP - rect.width();
                }
            }
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
        sample_drop(ui, &state, &placed, rect, events);
    }

    /// Where the zone editor is, if the selected multisampler has it open:
    /// above the Device View in `detail` (R1).
    #[must_use]
    pub fn zone_editor_rect(shell: &Shell, detail: Rect) -> Option<Rect> {
        shell.device_view_state().devices.iter().find_map(|d| {
            d.multisampler
                .as_ref()
                .filter(|m| d.selected && m.view.zone_editor)
                .map(|m| crate::multisampler::zone_editor_rect(detail, m.view.size))
        })
    }

    /// Draws the zone editor of the selected multisampler whose editor is
    /// open, above the Device View in `detail` (R1, R10).
    pub fn show_zone_editor(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        detail: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let state = shell.device_view_state();
        let Some((frame, multisampler)) = state.devices.iter().find_map(|d| {
            d.multisampler
                .as_ref()
                .filter(|m| d.selected && m.view.zone_editor)
                .map(|m| (d, m))
        }) else {
            return;
        };
        let rect = crate::multisampler::zone_editor_rect(detail, multisampler.view.size);
        self.multisampler
            .zone_editor(ui, shell, theme, frame, multisampler, rect, events);
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
    pub(crate) fn device(
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
        // A rack with every view hidden folds into its view column too
        // (racks spec, R19).
        let strip = !window
            && (frame.folded
                || frame.floating
                || frame
                    .rack
                    .as_ref()
                    .is_some_and(bl_shell::device_commands::RackFrame::is_strip));
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
        let plugin = is_plugin(frame);
        let (bar, mut controls) = if strip {
            folded_layout(rect)
        } else if window {
            window_layout(rect, frame.expandable, plugin)
        } else {
            unfolded_layout(rect, frame.expandable, plugin)
        };
        if strip && frame.rack.is_some() {
            // A rack's strip holds its view selectors under the activator.
            controls.title.min.y = controls.activator.bottom() + 6.0 + 3.0 * (BUTTON + 3.0);
        }
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
                if let Some(rack) = &frame.rack {
                    // Spec: racks R18. The rack's own frame holds its views;
                    // a strip draws only the view selectors.
                    self.rack_body(ui, shell, theme, frame, rack, rect, events);
                } else if frame.panel.is_some() && !strip {
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
                    match &frame.multisampler {
                        // Spec: multisampler R1. The tab chosen in the title
                        // bar shows its controls.
                        Some(state) => self.multisampler.body(
                            ui,
                            shell,
                            theme,
                            (frame, state),
                            display.as_ref(),
                            body,
                            &played,
                            events,
                        ),
                        None => {
                            crate::device_panel::show(
                                ui,
                                theme,
                                frame,
                                display.as_ref(),
                                body,
                                &played,
                                events,
                            );
                            // Spec: multisampler R54. Snapshot's sample
                            // display fills its first cell.
                            if matches!(
                                &frame.kind,
                                bl_model::DeviceKind::Native { device_type }
                                    if device_type == bl_devices::sampler::SAMPLER
                            ) && let Some(panel) = frame.panel.as_ref()
                                && let Some((_, cell, _)) =
                                    crate::device_panel::place_shown(panel, frame.expanded, body)
                                        .1
                                        .into_iter()
                                        .next()
                            {
                                self.sample_displays
                                    .snapshot(ui, shell, theme, frame, cell, events);
                            }
                        }
                    }
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
                if let (Some(state), false) = (&frame.multisampler, strip) {
                    self.multisampler
                        .tabs(ui, theme, frame, state, controls.title, events);
                }
                self.activator(ui, theme, frame, controls.activator, events);
                if let Some(expand) = controls.expand {
                    self.round_button(ui, theme, frame, "expand", expand, events);
                }
                if let Some(wrench) = controls.wrench {
                    self.wrench(ui, shell, theme, frame, wrench, events);
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
                } else if let (Some(section), false) = (&frame.midi_sidechain, strip) {
                    crate::sidechain::draw_midi(ui, shell, theme, frame.id, section, rect, events);
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
            // Spec: racks R12 (I-CLICK-024). Option-click (Alt-click) on a
            // rack's activator switches everything inside it too.
            let all = frame.rack.is_some() && ui.input(|i| i.modifiers.alt);
            invoke(
                events,
                if all {
                    "device.set_enabled_all"
                } else {
                    "device.set_enabled"
                },
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

    /// A plug-in device's window button (L-DEV-010): a wrench, named
    /// Show/Hide Plug-In Window, lit while the plug-in's editor window is
    /// open, which opens and closes it (`plugin.show_window`). Dimmed for a
    /// plug-in without an editor that can open.
    fn wrench(
        &self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let info = shell.plugin_info(frame.id);
        let has_window = info.as_ref().is_some_and(|i| i.has_window);
        let open = info.as_ref().is_some_and(|i| i.window_open);
        let id = control_id(frame.id, "plugin_window");
        let sense = if has_window {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(rect, id, sense);
        if has_window {
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
        }
        if has_window
            && (response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
        {
            // Spec: clap-hosting R13. Not an undo step.
            invoke(events, PLUGIN_SHOW_WINDOW, json!({ "device": frame.id }));
        }
        let c = &theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let centre = rect.center();
        let r = rect.width() / 2.0;
        let face = if open {
            c.accent.0
        } else if response.hovered() && has_window {
            c.control_hover.0
        } else {
            c.window.0
        };
        painter.circle_filled(centre, r, face);
        let ink = if open {
            c.accent_text.0
        } else if has_window {
            c.text_dim.0
        } else {
            // Dimmed, but still drawn: Live always shows the wrench.
            c.text_dim.0.gamma_multiply(0.55)
        };
        // A wrench: a handle from the lower left to a ring at the upper
        // right.
        let stroke = Stroke::new(1.2, ink);
        let head = centre + vec2(r * 0.3, -r * 0.3);
        painter.line_segment([centre + vec2(-r * 0.6, r * 0.6), head], stroke);
        painter.circle_stroke(head, r * 0.3, stroke);
        focus_ring(ui, theme, rect, &response);
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, has_window, open, PLUGIN_WINDOW_BUTTON)
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(PLUGIN_WINDOW_BUTTON.to_owned());
            node.set_value(if open { "on" } else { "off" }.to_owned());
            if !has_window {
                node.set_disabled();
            }
            node.set_description(
                "Shows or hides the plug-in's own panel in a floating window".to_owned(),
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
        // Spec: racks R26. A rack's menu adds Ungroup and disables Compare.
        let menu = if frame.rack.is_some() {
            &self.rack_menu
        } else {
            &self.menu
        };
        for item in shown(menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            let response = crate::menus::item(ui, item, shell, &self.keymap, self.platform, events);
            let rack_compare =
                frame.rack.is_some() && context_menus::RACK_COMPARE_ROWS.contains(&item.row);
            let response = match context_menus::pending_package(item.row) {
                _ if rack_compare => {
                    let text = "A/B compare doesn't apply to racks.".to_owned();
                    ui.ctx().accesskit_node_builder(response.id, |node| {
                        node.set_description(text.clone());
                    });
                    response.on_disabled_hover_text(text)
                }
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
        // Spec: multisampler R29. The multisampler's own options follow the
        // device's (C-DEVICE-015).
        if frame.multisampler.is_some() {
            ui.separator();
            let linear = shell
                .multisampler_device(frame.id)
                .is_some_and(|d| d.layers.linear_loop_fades);
            if ui
                .selectable_label(!linear, "Use Constant Power Fade for Loops")
                .clicked()
            {
                invoke(
                    events,
                    "device.set_layer_options",
                    json!({ "device": frame.id, "linear_loop_fades": !linear }),
                );
                ui.close();
            }
            if ui.button("Convert to Snapshot").clicked() {
                invoke(
                    events,
                    bl_shell::multisampler_commands::CONVERT,
                    json!({ "device": frame.id, "to": "snapshot" }),
                );
                ui.close();
            }
        }
        // Spec: multisampler R62. Snapshot's menu converts it the other way.
        if matches!(
            &frame.kind,
            bl_model::DeviceKind::Native { device_type }
                if device_type == bl_devices::sampler::SAMPLER
        ) {
            ui.separator();
            if ui.button("Convert to Multisampler").clicked() {
                invoke(
                    events,
                    bl_shell::multisampler_commands::CONVERT,
                    json!({ "device": frame.id, "to": "multisampler" }),
                );
                ui.close();
            }
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
        // Strike's own items after its switches (31.4.5): the swap to
        // Snapshot, and Save as Default Pad, which the Drum Rack's pads
        // (T145) bring.
        if frame.swaps_to_snapshot {
            let response = ui.button("Swap to Snapshot");
            crate::menus::register_item(ui.ctx(), &response);
            if response.clicked() {
                invoke(events, ids::SWAP_TO_SNAPSHOT, json!({ "device": frame.id }));
                ui.close();
            }
            crate::menus::take_focus_first(ui, &response);
            let text = "Not built yet: arrives with the Drum Rack's pads (T145).";
            let pad = ui.add_enabled(false, egui::Button::new("Save as Default Pad"));
            ui.ctx().accesskit_node_builder(pad.id, |node| {
                node.set_description(text.to_owned());
            });
            pad.on_disabled_hover_text(text);
        }
    }

    /// Dragging a device's title bar moves it: a line shows where it would
    /// go, at places that keep the chain in order, and the drop runs
    /// `device.move`, or `device.move_to_chain` inside a rack's devices view
    /// (racks spec, R7). Dragging a chain's row over the track's own chain
    /// drops its devices there, and the chain goes (`chain.drop_on_track`,
    /// R24).
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
        let device = DragAndDrop::payload::<DeviceDrag>(ui.ctx()).map(|d| d.device);
        let chains = DragAndDrop::payload::<crate::rack_view::ChainDrag>(ui.ctx());
        if device.is_none() && chains.is_none() {
            return;
        }
        let (Some(pointer), Some(track)) = (ui.input(|i| i.pointer.interact_pos()), state.track)
        else {
            return;
        };
        if !rect.contains(pointer) {
            return;
        }
        let mut slots = vec![Slot {
            chain: None,
            rack: None,
            rack_kind: None,
            area: rect,
            frames: placed
                .iter()
                .zip(&state.devices)
                .map(|((id, at), d)| (*id, *at, d.role))
                .collect(),
        }];
        for (device, at) in state.devices.iter().zip(placed) {
            nested_slots(device, at.1, &mut slots);
        }
        // The innermost devices view the pointer is over.
        let Some(slot) = slots.iter().rev().find(|s| s.area.contains(pointer)) else {
            return;
        };
        let (gap, index, target) = match (device, &chains) {
            (Some(device), _) => {
                let Some(landing) = landing(shell, state, slot, device, track, pointer.x) else {
                    return;
                };
                (landing.gap, landing.index, Some((device, landing.changes)))
            }
            (None, Some(drag)) if slot.chain.is_none() => {
                // Spec: racks R24. Only a drop outside every rack spreads a
                // chain's devices on the track.
                let over_rack = placed
                    .iter()
                    .zip(&state.devices)
                    .any(|((_, at), d)| d.rack.is_some() && at.contains(pointer));
                if over_rack {
                    return;
                }
                let Some((gap, ok)) = chain_landing(shell, state, drag, track, slot, pointer.x)
                else {
                    return;
                };
                if !ok {
                    return;
                }
                (gap, gap, None)
            }
            _ => return,
        };
        let x = match slot.frames.get(gap) {
            Some((_, frame, _)) => frame.left() - DEVICE_GAP / 2.0,
            None => slot
                .frames
                .last()
                .map_or(slot.area.left() + DEVICE_GAP / 2.0, |(_, f, _)| {
                    f.right() + DEVICE_GAP / 2.0
                }),
        };
        ui.painter().line_segment(
            [
                pos2(x, slot.area.top() + 2.0),
                pos2(x, slot.area.bottom() - 2.0),
            ],
            Stroke::new(2.0, theme.colors.accent.0),
        );
        if !ui.input(|i| i.pointer.any_released()) {
            return;
        }
        DragAndDrop::clear_payload(ui.ctx());
        match (target, chains) {
            (Some((device, true)), _) => match slot.chain {
                None => invoke(
                    events,
                    "device.move",
                    json!({ "device": device, "track": track, "index": index }),
                ),
                Some(chain) => invoke(
                    events,
                    "device.move_to_chain",
                    json!({ "device": device, "chain": chain, "index": index }),
                ),
            },
            (None, Some(drag)) => {
                events.push(UiEvent::BeginNamedGesture("Move Chain".to_owned()));
                let mut at = index;
                for chain in &drag.chains {
                    invoke(
                        events,
                        "chain.drop_on_track",
                        json!({ "chain": chain, "track": track, "index": at }),
                    );
                    at += shell
                        .editor()
                        .document()
                        .chain(*chain)
                        .map_or(0, |c| c.devices.len());
                }
                events.push(UiEvent::EndGesture);
            }
            _ => {}
        }
    }
}

/// A place devices can be dropped: a track's own chain, or the devices
/// view of a rack, with the frames it holds.
struct Slot {
    /// The rack's chain the frames are in; none for the track's own.
    chain: Option<bl_model::ChainId>,
    /// The rack, if this is a chain's.
    rack: Option<DeviceId>,
    rack_kind: Option<bl_model::RackKind>,
    area: Rect,
    frames: Vec<(DeviceId, Rect, DeviceRole)>,
}

/// Adds the devices view of `device`'s rack, and of the racks inside it, to
/// `slots`, outermost first.
fn nested_slots(device: &DeviceFrame, at: Rect, slots: &mut Vec<Slot>) {
    let Some(rack) = &device.rack else {
        return;
    };
    let here = crate::rack_view::layout(rack, at, 0.0);
    if let (Some(area), [chain]) = (here.devices, rack.selected_chains.as_slice()) {
        slots.push(Slot {
            chain: Some(*chain),
            rack: Some(device.id),
            rack_kind: Some(rack.kind),
            area,
            frames: rack
                .devices
                .iter()
                .zip(&here.device_frames)
                .map(|(d, (id, at))| (*id, *at, d.role))
                .collect(),
        });
    }
    for (child, (_, child_at)) in rack.devices.iter().zip(&here.device_frames) {
        nested_slots(child, *child_at, slots);
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

/// Whether a device with `role` fits a rack of `kind` (racks spec, R1).
fn rack_takes(kind: bl_model::RackKind, role: DeviceRole) -> bool {
    match kind {
        bl_model::RackKind::AudioEffect => role == DeviceRole::AudioEffect,
        bl_model::RackKind::MidiEffect => role == DeviceRole::MidiEffect,
        bl_model::RackKind::Instrument | bl_model::RackKind::Drum => true,
    }
}

/// Where a device dragged to `x` lands in `slot`, if that keeps the chain in
/// Live's order (the device chains spec, R2 and R3; the racks spec, R1).
fn landing(
    shell: &Shell,
    state: &DeviceViewState,
    slot: &Slot,
    device: DeviceId,
    track: TrackId,
    x: f32,
) -> Option<MoveIndex> {
    let doc = shell.editor().document();
    let dragged = doc.device(device)?;
    let role = dragged.role;
    let place = doc.device_place(device);
    match (slot.rack_kind, slot.rack) {
        (None, _) => {
            if !role.fits(state.track_kind?) {
                return None;
            }
        }
        (Some(kind), Some(rack)) => {
            // A rack can't go into one of its own chains.
            if !rack_takes(kind, role) || dragged.find(rack).is_some() {
                return None;
            }
        }
        (Some(_), None) => return None,
    }
    let gap = slot
        .frames
        .iter()
        .filter(|(_, f, _)| f.center().x < x)
        .count();
    let from = slot.frames.iter().position(|(id, ..)| *id == device);
    let index = match from {
        Some(from) if from < gap => gap - 1,
        _ => gap,
    };
    let others: Vec<DeviceRole> = slot
        .frames
        .iter()
        .filter(|(id, ..)| *id != device)
        .map(|(.., r)| *r)
        .collect();
    let fits = others[..index].iter().all(|r| *r <= role)
        && others[index..].iter().all(|r| *r >= role)
        && !(role == DeviceRole::Instrument && others.contains(&DeviceRole::Instrument));
    let moves = match slot.chain {
        None => place.is_none_or(|p| p.chain.is_some() || p.track != track),
        Some(chain) => place.is_none_or(|p| p.chain != Some(chain)),
    };
    fits.then_some(MoveIndex {
        gap,
        index,
        changes: from != Some(index) || moves,
    })
}

/// Where chains dragged to `x` land in the track's own chain, and whether
/// their devices fit there in order.
fn chain_landing(
    shell: &Shell,
    state: &DeviceViewState,
    drag: &crate::rack_view::ChainDrag,
    track: TrackId,
    slot: &Slot,
    x: f32,
) -> Option<(usize, bool)> {
    let doc = shell.editor().document();
    let kind = state.track_kind?;
    let gap = slot
        .frames
        .iter()
        .filter(|(_, f, _)| f.center().x < x)
        .count();
    let dropped: Vec<DeviceRole> = drag
        .chains
        .iter()
        .filter_map(|c| doc.chain(*c))
        .flat_map(|c| c.devices.iter().map(|d| d.role))
        .collect();
    let others: Vec<DeviceRole> = slot.frames.iter().map(|(.., r)| *r).collect();
    let mut all = others[..gap].to_vec();
    all.extend(&dropped);
    all.extend(&others[gap..]);
    let instruments = all.iter().filter(|r| **r == DeviceRole::Instrument).count();
    let ok = !dropped.is_empty()
        && dropped.iter().all(|r| r.fits(kind))
        && all.windows(2).all(|pair| pair[0] <= pair[1])
        && instruments <= 1
        && doc.track(track).is_some();
    Some((gap, ok))
}

/// Where a title bar's controls sit.
struct Controls {
    activator: Rect,
    /// The expand toggle, right of the activator, on a device that has an
    /// expanded part (L-DEV-004).
    expand: Option<Rect>,
    /// A plug-in device's window button, after the activator and the
    /// expand toggle, before the name (L-DEV-010).
    wrench: Option<Rect>,
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
/// toggle where the device has an expanded part, a plug-in's window button,
/// the name, then Hot-Swap Presets, Save Preset, and Show Options at the
/// right.
fn unfolded_layout(rect: Rect, expandable: bool, plugin: bool) -> (Rect, Controls) {
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE_BAR_HEIGHT));
    let y = bar.center().y;
    let button = |x: f32| Rect::from_center_size(pos2(x, y), vec2(BUTTON, BUTTON));
    let step = BUTTON + 3.0;
    let options = button(bar.right() - 2.0 - BUTTON / 2.0);
    let save = button(options.center().x - step);
    let hot_swap = button(save.center().x - step);
    let activator = button(bar.left() + 2.0 + BUTTON / 2.0);
    let expand = expandable.then(|| button(activator.right() + 3.0 + BUTTON / 2.0));
    let wrench = plugin.then(|| button(expand.unwrap_or(activator).right() + 3.0 + BUTTON / 2.0));
    let title = Rect::from_min_max(
        pos2(
            wrench.or(expand).unwrap_or(activator).right() + 4.0,
            bar.top(),
        ),
        pos2(hot_swap.left() - 4.0, bar.bottom()),
    );
    (
        bar,
        Controls {
            activator,
            expand,
            wrench,
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
fn window_layout(rect: Rect, expandable: bool, plugin: bool) -> (Rect, Controls) {
    let step = BUTTON + 3.0;
    let mut narrower = rect;
    narrower.max.x -= step;
    let (_, mut controls) = unfolded_layout(narrower, expandable, plugin);
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
            wrench: None,
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
        unfolded_layout(frame, false, false).1.activator
    }
}

/// Where a device's title (its Device Title Bar's name) sits in its frame.
#[must_use]
pub fn title_rect(frame: Rect, folded: bool) -> Rect {
    if folded {
        folded_layout(frame).1.title
    } else {
        unfolded_layout(frame, false, false).1.title
    }
}

/// Where a device's three round buttons sit in its frame: Hot-Swap
/// Presets, Save Preset, and Show Options.
#[must_use]
pub fn button_rects(frame: Rect, folded: bool) -> [Rect; 3] {
    let controls = if folded {
        folded_layout(frame).1
    } else {
        unfolded_layout(frame, false, false).1
    };
    [controls.hot_swap, controls.save, controls.options]
}

/// Where a plug-in device's window button (Show/Hide Plug-In Window,
/// L-DEV-010) sits in its unfolded frame.
#[must_use]
pub fn plugin_window_rect(frame: Rect, expandable: bool) -> Rect {
    unfolded_layout(frame, expandable, true)
        .1
        .wrench
        .expect("a plug-in's title bar has the window button")
}

/// Whether a frame is a plug-in device's, whose title bar has the window
/// button.
fn is_plugin(frame: &DeviceFrame) -> bool {
    matches!(frame.kind, bl_model::DeviceKind::Plugin { .. })
}
