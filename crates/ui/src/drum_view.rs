//! A drum rack's pad view, and the controls its chain list gains (WP-5.6;
//! the drum racks spec, R11 to R21): sixteen pads in four rows of four with
//! an overview of all 128 at the left, each pad's mute, solo, preview and
//! hot-swap, dropping and dragging on pads; and in the chain list a drum
//! chain's Preview, Receive, Play, Choke and sends, and a return chain's
//! Audio To. Every gesture runs a registered command (`pad.*`, `chain.*`).

use bl_model::{DeviceId, Receive};
use bl_shell::device_commands::{ChainRow, DeviceFrame, RackFrame};
use bl_shell::pad_commands::{
    PAD_GAP, PAD_HEIGHT, PAD_OVERVIEW_WIDTH, PAD_WIDTH, PADS_PER_ROW, PADS_SHOWN, PadFrame,
    PadKind, PadState, PadSwitch,
};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, DragAndDrop, Id, Key, Rect, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::device_view::{DeviceDrag, DeviceViewUi};
use crate::main_window::UiEvent;
use crate::rack_view::{ChainListRects, ChainRowRects, volume_at, volume_text};
use crate::theme::Theme;

/// A pad being dragged: the rack and the pad's note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadDrag {
    /// The drum rack.
    pub rack: DeviceId,
    /// The pad's note.
    pub note: u8,
}

/// Where the parts of one pad sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadRects {
    /// The pad's note.
    pub note: u8,
    /// The pad.
    pub pad: Rect,
    /// Its mute button.
    pub mute: Rect,
    /// Its solo button.
    pub solo: Rect,
    /// Its preview button.
    pub preview: Rect,
    /// Its hot-swap button.
    pub hot_swap: Rect,
    /// Its Hot-Swap Sample button, at the top right.
    pub sample: Rect,
}

/// Where a pad view's parts sit.
#[derive(Clone, Debug, PartialEq)]
pub struct PadLayout {
    /// The whole pad view.
    pub area: Rect,
    /// The overview of all 128 notes.
    pub overview: Rect,
    /// The overview's cells, a group of sixteen notes each, lowest first.
    pub groups: Vec<Rect>,
    /// The part of the overview showing the sixteen notes now.
    pub selector: Rect,
    /// The sixteen pads shown, from the lowest note up.
    pub pads: Vec<PadRects>,
}

/// The size of a pad's small buttons.
const SMALL: f32 = 9.0;

/// Where a pad view's parts sit in `area` (drum racks spec, R11): the pads
/// rise left to right and from the bottom row up.
#[must_use]
pub fn pad_layout(frame: &PadFrame, area: Rect) -> PadLayout {
    let overview = Rect::from_min_max(
        pos2(area.left() + 2.0, area.top() + 2.0),
        pos2(area.left() + 2.0 + PAD_OVERVIEW_WIDTH, area.bottom() - 2.0),
    );
    let cell_height = overview.height() / 8.0;
    let groups = (0..8)
        .map(|g| {
            let bottom = overview.bottom() - g as f32 * cell_height;
            Rect::from_min_max(
                pos2(overview.left(), bottom - cell_height + 1.0),
                pos2(overview.right(), bottom),
            )
        })
        .collect();
    let per_note = overview.height() / 128.0;
    let selector = Rect::from_min_max(
        pos2(
            overview.left(),
            overview.bottom() - (f32::from(frame.first) + f32::from(PADS_SHOWN)) * per_note,
        ),
        pos2(
            overview.right(),
            overview.bottom() - f32::from(frame.first) * per_note,
        ),
    );
    let grid_height = 4.0 * PAD_HEIGHT + 3.0 * PAD_GAP;
    let grid_bottom = area.center().y + grid_height / 2.0;
    let left = overview.right() + 4.0;
    let pads = (0..PADS_SHOWN)
        .map(|n| {
            let (column, row) = (n % PADS_PER_ROW, n / PADS_PER_ROW);
            let min = pos2(
                left + f32::from(column) * (PAD_WIDTH + PAD_GAP),
                grid_bottom - f32::from(row + 1) * PAD_HEIGHT - f32::from(row) * PAD_GAP,
            );
            let pad = Rect::from_min_size(min, vec2(PAD_WIDTH, PAD_HEIGHT));
            let small = |k: f32| {
                Rect::from_min_size(
                    pos2(
                        pad.left() + 2.0 + k * (SMALL + 1.0),
                        pad.bottom() - SMALL - 2.0,
                    ),
                    vec2(SMALL, SMALL),
                )
            };
            PadRects {
                note: frame.first + n,
                pad,
                mute: small(0.0),
                solo: small(1.0),
                preview: small(2.0),
                hot_swap: small(3.0),
                sample: Rect::from_min_size(
                    pos2(pad.right() - SMALL - 2.0, pad.top() + 2.0),
                    vec2(SMALL, SMALL),
                ),
            }
        })
        .collect();
    PadLayout {
        area,
        overview,
        groups,
        selector,
        pads,
    }
}

/// The ID of a pad, by its place among the sixteen shown (`slot`, 0 for the
/// lowest), so the keyboard keeps its place while the view moves.
#[must_use]
pub fn pad_id(rack: DeviceId, slot: u8) -> Id {
    Id::new(("pad", rack, slot))
}

/// The ID of one of a pad's buttons: `mute`, `solo`, `preview`, `hot_swap`
/// or `sample`.
#[must_use]
pub fn pad_control_id(rack: DeviceId, slot: u8, control: &str) -> Id {
    Id::new(("pad-control", rack, slot, control))
}

/// The ID of a pad view's overview.
#[must_use]
pub fn overview_id(rack: DeviceId) -> Id {
    Id::new(("pad-overview", rack))
}

/// The ID of one of a drum chain's chain-list controls: `preview`,
/// `receive`, `play`, `choke`, `send` with its place, or `audio_to`.
#[must_use]
pub fn drum_control_id(chain: bl_model::ChainId, control: &str) -> Id {
    Id::new(("drum-control", chain, control))
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// A small button's face and ink.
fn button_look(theme: &Theme, on: bool, mixed: bool, hovered: bool) -> (Color32, Color32) {
    let c = &theme.colors;
    if on {
        (c.accent.0, c.accent_text.0)
    } else if mixed {
        (c.accent.0.gamma_multiply(0.45), c.text.0)
    } else if hovered {
        (c.control_hover.0, c.text_dim.0)
    } else {
        (c.window.0, c.text_dim.0)
    }
}

/// The browser's files, or the files dropped from the desktop, that a pad
/// takes, with whether Cmd (Alt on Windows and Linux) is held.
fn dropped_items(ui: &Ui, released: bool) -> Option<(Vec<String>, bool)> {
    let paths: Vec<String> = if let Some(drag) =
        DragAndDrop::payload::<crate::browser::BrowserDrag>(ui.ctx())
        && released
    {
        DragAndDrop::clear_payload(ui.ctx());
        drag.paths.clone()
    } else {
        ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_string_lossy().into_owned())
                .collect()
        })
    };
    let layer = ui.input(|i| i.modifiers.command);
    (!paths.is_empty()).then_some((paths, layer))
}

impl DeviceViewUi {
    /// A drum rack's pad view.
    pub(crate) fn pad_view(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        pads: &PadFrame,
        layout: &PadLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let painter = ui.painter_at(layout.area);
        painter.rect_filled(layout.area, 0.0, c.window.0.gamma_multiply(0.5));
        self.overview(ui, theme, frame, pads, layout, events);
        self.pad_keys(ui, shell, frame, pads, events);
        let drag = DragAndDrop::payload::<PadDrag>(ui.ctx());
        let released = ui.input(|i| i.pointer.any_released());
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let mut hint = None;
        for (pad, rects) in pads.pads.iter().zip(&layout.pads) {
            self.pad(ui, theme, frame, pads, pad, rects, drag.as_deref(), events);
            if pointer.is_some_and(|p| rects.pad.contains(p)) {
                hint = Some(pad.hover.clone());
                self.pad_drop(ui, frame, pad, rects, drag.as_deref(), released, events);
            }
        }
        let key = Id::new(("pad-hint", frame.id));
        let was = ui
            .data(|d| d.get_temp::<Option<String>>(key))
            .unwrap_or(None);
        if hint != was {
            ui.data_mut(|d| d.insert_temp(key, hint.clone()));
            events.push(UiEvent::StatusHint(hint));
        }
    }

    /// Up and Down move the visible pads a group, and with Cmd a row, while
    /// the pad view has the keyboard (drum racks spec, R11).
    fn pad_keys(
        &self,
        ui: &Ui,
        shell: &Shell,
        frame: &DeviceFrame,
        pads: &PadFrame,
        events: &mut Vec<UiEvent>,
    ) {
        let focused = ui.memory(|m| m.focused());
        let in_view = focused.is_some_and(|id| {
            id == overview_id(frame.id) || pads.pads.iter().any(|p| pad_id(frame.id, p.slot) == id)
        });
        if !in_view || shell.focused_area() != Area::Detail || egui::Popup::is_any_open(ui.ctx()) {
            return;
        }
        for (key, sign) in [(Key::ArrowUp, 1), (Key::ArrowDown, -1)] {
            for command in [true, false] {
                let modifiers = if command {
                    egui::Modifiers::COMMAND
                } else {
                    egui::Modifiers::NONE
                };
                if ui.input_mut(|i| i.consume_key(modifiers, key)) {
                    let args = if command {
                        json!({ "rack": frame.id, "rows": sign })
                    } else {
                        json!({ "rack": frame.id, "groups": sign })
                    };
                    invoke(events, "rack.move_pads", args);
                }
            }
        }
    }

    /// The overview of all 128 notes in eight groups of sixteen, and the
    /// selector that shows which are on the pads.
    fn overview(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        pads: &PadFrame,
        layout: &PadLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let id = overview_id(frame.id);
        let response = ui.interact(layout.overview, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        let painter = ui.painter_at(layout.overview.expand(1.0));
        painter.rect_filled(layout.overview, theme.sizes.corner, c.control.0);
        for (g, cell) in layout.groups.iter().enumerate() {
            // A dot per note that has a chain, in the group's 4 by 4.
            for n in 0..16u8 {
                let note = g as u8 * 16 + n;
                if pads.occupied.contains(&note) {
                    let (column, row) = (n % 4, n / 4);
                    let dot = pos2(
                        cell.left() + 4.0 + f32::from(column) * (cell.width() - 8.0) / 3.0,
                        cell.bottom() - 3.0 - f32::from(row) * (cell.height() - 6.0) / 3.0,
                    );
                    painter.circle_filled(dot, 1.0, c.text_dim.0);
                }
            }
            painter.line_segment(
                [cell.left_top(), cell.right_top()],
                Stroke::new(0.5, c.separator.0),
            );
        }
        painter.rect_stroke(
            layout.selector,
            theme.sizes.corner,
            Stroke::new(1.5, c.accent.0),
            StrokeKind::Inside,
        );
        // Dragging the selector, or clicking a group, moves the view; the
        // rows follow the pointer, a group at a time unless Cmd is held.
        if (response.dragged() || response.clicked())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let from_bottom = (layout.overview.bottom() - pointer.y) / layout.overview.height();
            let centre = (from_bottom * 128.0 - f32::from(PADS_SHOWN) / 2.0).round();
            let per_row = ui.input(|i| i.modifiers.command);
            let first = if per_row {
                centre / 4.0 * 4.0
            } else {
                (centre / 16.0).round() * 16.0
            }
            .clamp(0.0, 112.0) as u8;
            if first != pads.first {
                invoke(
                    events,
                    "rack.move_pads",
                    json!({ "rack": frame.id, "first": first }),
                );
            }
        }
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Slider, true, "Pad Overview"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Slider);
            node.set_label("Pad Overview".to_owned());
            node.set_value(format!(
                "{} to {}",
                bl_model::note_name(pads.first),
                bl_model::note_name(pads.first + PADS_SHOWN - 1)
            ));
            node.set_description(
                "Drag to show other pads, Up and Down move a group, with Cmd a row".to_owned(),
            );
        });
    }

    /// One pad: its face, its buttons, and what a click or a drag on it
    /// does.
    #[expect(clippy::too_many_arguments, reason = "the pad's inputs")]
    fn pad(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        pads: &PadFrame,
        pad: &PadState,
        rects: &PadRects,
        drag: Option<&PadDrag>,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let id = pad_id(frame.id, pad.slot);
        let response = ui.interact(rects.pad, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            invoke(
                events,
                "pad.select",
                json!({ "rack": frame.id, "note": pad.note }),
            );
        }
        if response.drag_started() && pad.kind != PadKind::Empty {
            DragAndDrop::set_payload(
                ui.ctx(),
                PadDrag {
                    rack: frame.id,
                    note: pad.note,
                },
            );
            invoke(
                events,
                "pad.select",
                json!({ "rack": frame.id, "note": pad.note }),
            );
        }
        let face = match pad.kind {
            PadKind::Empty => c.control.0.gamma_multiply(0.7),
            PadKind::Single => c.control.0,
            PadKind::Multi => c.control.0.gamma_multiply(1.15),
        };
        let hovered_drop = drag.is_some_and(|d| d.rack == frame.id && d.note != pad.note)
            && response.contains_pointer();
        let painter = ui.painter_at(rects.pad.expand(2.0));
        painter.rect_filled(
            rects.pad,
            theme.sizes.corner,
            if response.hovered() {
                c.control_hover.0
            } else {
                face
            },
        );
        let outline = if pad.selected {
            Some((c.accent.0, 1.5))
        } else if pads.hot_swap == Some(pad.note) || hovered_drop {
            Some((c.focus.0, 1.5))
        } else {
            None
        };
        if let Some((colour, width)) = outline {
            painter.rect_stroke(
                rects.pad,
                theme.sizes.corner,
                Stroke::new(width, colour),
                StrokeKind::Inside,
            );
        }
        let note_ink = c.text_dim.0;
        painter.text(
            pos2(rects.pad.left() + 3.0, rects.pad.top() + 2.0),
            Align2::LEFT_TOP,
            &pad.name,
            egui::FontId::proportional(8.0),
            note_ink,
        );
        let renaming = pad.kind == PadKind::Single
            && self
                .renaming_chain
                .as_ref()
                .is_some_and(|r| pad.chains.first() == Some(&r.chain));
        if renaming {
            self.pad_name(ui, theme, pad, rects, events);
        } else if pad.kind != PadKind::Empty {
            let dim = pad.mute == PadSwitch::On;
            painter.text(
                pos2(rects.pad.center().x, rects.pad.top() + 14.0),
                Align2::CENTER_CENTER,
                &pad.label,
                theme.small_font(),
                if dim { note_ink } else { c.text.0 },
            );
        }
        if pad.kind != PadKind::Empty {
            response.context_menu(|ui| self.pad_menu(ui, frame, pad, events));
            // Shift-F10 (S-CTX-002) opens the same menu from the keyboard.
            if response.has_focus() && crate::menus::context_menu_keys(ui) {
                invoke(
                    events,
                    "pad.select",
                    json!({ "rack": frame.id, "note": pad.note }),
                );
                crate::menus::open_keyboard_context_menu(
                    ui.ctx(),
                    response.id,
                    rects.pad.left_bottom(),
                );
            }
            crate::menus::keyboard_context_menu(&response, |ui| {
                self.pad_menu(ui, frame, pad, events);
            });
        }
        if response.has_focus() && crate::focus::outlined(ui.ctx()) {
            painter.rect_stroke(
                rects.pad.expand(1.5),
                theme.sizes.corner,
                Stroke::new(1.0, c.focus.0),
                StrokeKind::Outside,
            );
        }
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, pad.selected, pad_label(pad))
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(pad_label(pad));
            node.set_selected(pad.selected);
            node.set_description(pad.hover.clone());
        });
        if pad.kind == PadKind::Empty {
            return;
        }
        self.pad_switch(ui, theme, frame, pad, rects.mute, "mute", pad.mute, events);
        self.pad_switch(ui, theme, frame, pad, rects.solo, "solo", pad.solo, events);
        self.pad_preview(ui, theme, frame, pad, rects.preview, events);
        if pad.kind == PadKind::Single {
            self.pad_hot_swap(ui, theme, frame, pad, pads, rects, &response, events);
        }
    }

    /// A pad's chain's name, being typed on the pad.
    fn pad_name(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        pad: &PadState,
        rects: &PadRects,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(renaming) = self.renaming_chain.as_mut() else {
            return;
        };
        let area = Rect::from_min_max(
            pos2(rects.pad.left() + 2.0, rects.pad.top() + 8.0),
            pos2(rects.pad.right() - 2.0, rects.pad.top() + 20.0),
        );
        let edit = ui.put(
            area,
            egui::TextEdit::singleline(&mut renaming.text)
                .id(Id::new(("pad-name", pad.chains.first().copied())))
                .font(theme.small_font())
                .margin(vec2(2.0, 0.0)),
        );
        if !renaming.focused {
            edit.request_focus();
            renaming.focused = true;
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if edit.lost_focus() || enter || escape {
            if enter || (edit.lost_focus() && !escape) {
                invoke(
                    events,
                    "chain.rename",
                    json!({ "chain": renaming.chain, "name": renaming.text }),
                );
            }
            self.renaming_chain = None;
        }
    }

    /// A pad's context menu: Rename and Save as Default Pad for a pad of one
    /// chain, and Delete (drum racks spec, R12, R17).
    fn pad_menu(
        &mut self,
        ui: &mut Ui,
        frame: &DeviceFrame,
        pad: &PadState,
        events: &mut Vec<UiEvent>,
    ) {
        let single = pad.kind == PadKind::Single;
        if ui
            .add_enabled(single, egui::Button::new("Rename"))
            .clicked()
        {
            if let Some(chain) = pad.chains.first() {
                self.start_chain_rename(*chain, &pad.label);
            }
            ui.close();
        }
        if ui.add_enabled(true, egui::Button::new("Delete")).clicked() {
            invoke(
                events,
                "pad.delete",
                json!({ "rack": frame.id, "note": pad.note }),
            );
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(single, egui::Button::new("Save as Default Pad"))
            .clicked()
        {
            invoke(
                events,
                "pad.save_default",
                json!({ "rack": frame.id, "note": pad.note }),
            );
            ui.close();
        }
    }

    /// A pad's mute or solo button.
    #[expect(clippy::too_many_arguments, reason = "the button's inputs")]
    fn pad_switch(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        pad: &PadState,
        at: Rect,
        which: &'static str,
        state: PadSwitch,
        events: &mut Vec<UiEvent>,
    ) {
        let id = pad_control_id(frame.id, pad.slot, which);
        let response = ui.interact(at, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            let command = if which == "mute" {
                "pad.set_mute"
            } else {
                "pad.set_solo"
            };
            invoke(
                events,
                command,
                json!({ "rack": frame.id, "note": pad.note }),
            );
        }
        let on = state == PadSwitch::On;
        let (face, ink) = button_look(theme, on, state == PadSwitch::Mixed, response.hovered());
        let painter = ui.painter_at(at.expand(1.0));
        painter.rect_filled(at, 1.0, face);
        painter.text(
            at.center(),
            Align2::CENTER_CENTER,
            if which == "mute" { "M" } else { "S" },
            egui::FontId::proportional(7.0),
            ink,
        );
        let name = if which == "mute" { "Mute" } else { "Solo" };
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("{name} of {}", pad_label(pad)));
            node.set_value(
                match state {
                    PadSwitch::Off => "off",
                    PadSwitch::On => "on",
                    PadSwitch::Mixed => "mixed",
                }
                .to_owned(),
            );
        });
    }

    /// A pad's Preview button: it plays while it's held.
    fn pad_preview(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        pad: &PadState,
        at: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let id = pad_control_id(frame.id, pad.slot, "preview");
        let response = ui.interact(at, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        self.held_button(ui, &response, id, pad.previewing, |on| {
            invoke(
                events,
                "pad.preview",
                json!({ "rack": frame.id, "note": pad.note, "on": on }),
            );
        });
        let (face, ink) = button_look(theme, pad.previewing, false, response.hovered());
        let painter = ui.painter_at(at.expand(1.0));
        painter.rect_filled(at, 1.0, face);
        painter.add(Shape::convex_polygon(
            vec![
                at.center() + vec2(-1.5, -2.5),
                at.center() + vec2(2.5, 0.0),
                at.center() + vec2(-1.5, 2.5),
            ],
            ink,
            Stroke::NONE,
        ));
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, pad.previewing, "Preview")
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("Preview of {}", pad_label(pad)));
            node.set_description("Plays the pad while it is held".to_owned());
        });
    }

    /// Calls `send(true)` as a pointer presses a button and `send(false)` as
    /// it lets go (or the button's pointer leaves with the press), for the
    /// buttons that act while they are held.
    pub(crate) fn held_button(
        &self,
        ui: &Ui,
        response: &egui::Response,
        id: Id,
        lit: bool,
        mut send: impl FnMut(bool),
    ) {
        let down = response.is_pointer_button_down_on();
        let key = id.with("held");
        let was = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
        if down != was {
            ui.data_mut(|d| d.insert_temp(key, down));
            send(down);
        } else if response.clicked() && !down && !lit && !was {
            // A click that never showed as held, such as a screen reader's:
            // a press and its release in one go.
            send(true);
            send(false);
        }
    }

    /// A pad's Hot-Swap button, and the Hot-Swap Sample button its
    /// pointer-over shows.
    #[expect(clippy::too_many_arguments, reason = "the buttons' inputs")]
    fn pad_hot_swap(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        pad: &PadState,
        pads: &PadFrame,
        rects: &PadRects,
        pad_response: &egui::Response,
        events: &mut Vec<UiEvent>,
    ) {
        let on = pads.hot_swap == Some(pad.note);
        let id = pad_control_id(frame.id, pad.slot, "hot_swap");
        let response = ui.interact(rects.hot_swap, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            invoke(
                events,
                "pad.hot_swap",
                json!({ "rack": frame.id, "note": pad.note }),
            );
        }
        let (face, ink) = button_look(theme, on, false, response.hovered());
        let painter = ui.painter_at(rects.hot_swap.expand(1.0));
        painter.circle_filled(rects.hot_swap.center(), SMALL / 2.0, face);
        painter.circle_stroke(rects.hot_swap.center(), SMALL / 4.0, Stroke::new(1.0, ink));
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, "Hot-Swap"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("Hot-Swap of {}", pad_label(pad)));
            node.set_value(if on { "on" } else { "off" }.to_owned());
        });
        // Hot-Swap Sample, while the pointer is over a pad that plays a
        // sample (R19).
        if !pad.has_sample || !(pad_response.hovered() || ui.rect_contains_pointer(rects.sample)) {
            return;
        }
        let id = pad_control_id(frame.id, pad.slot, "sample");
        let sample = ui.interact(rects.sample, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if sample.clicked() || (sample.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            invoke(
                events,
                "pad.hot_swap_sample",
                json!({ "rack": frame.id, "note": pad.note }),
            );
        }
        let (face, ink) = button_look(theme, on, false, sample.hovered());
        let painter = ui.painter_at(rects.sample.expand(1.0));
        painter.rect_filled(rects.sample, 1.0, face);
        // A little waveform.
        let mid = rects.sample.center();
        for (k, h) in [1.0, 3.0, 2.0, 3.5, 1.5].into_iter().enumerate() {
            let x = rects.sample.left() + 1.5 + k as f32 * 1.5;
            painter.line_segment(
                [pos2(x, mid.y - h), pos2(x, mid.y + h)],
                Stroke::new(0.8, ink),
            );
        }
        sample.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Hot-Swap Sample"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("Hot-Swap Sample of {}", pad_label(pad)));
            node.set_description("Opens the sample's folder in the browser".to_owned());
        });
    }

    /// What dropping on a pad does: a pad dragged onto it swaps (Cmd
    /// layers), files and presets land on it, a device moves onto it.
    #[expect(clippy::too_many_arguments, reason = "the drop's inputs")]
    fn pad_drop(
        &self,
        ui: &Ui,
        frame: &DeviceFrame,
        pad: &PadState,
        rects: &PadRects,
        drag: Option<&PadDrag>,
        released: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let _ = rects;
        if let Some(pad_drag) = drag {
            if released && pad_drag.rack == frame.id && pad_drag.note != pad.note {
                DragAndDrop::clear_payload(ui.ctx());
                let layer = ui.input(|i| i.modifiers.command);
                invoke(
                    events,
                    "pad.swap",
                    json!({ "rack": frame.id, "from": pad_drag.note, "to": pad.note, "layer": layer }),
                );
            }
            return;
        }
        if let Some(device) = DragAndDrop::payload::<DeviceDrag>(ui.ctx())
            && released
        {
            DragAndDrop::clear_payload(ui.ctx());
            invoke(
                events,
                "pad.drop",
                json!({ "rack": frame.id, "note": pad.note, "move_device": device.device }),
            );
            return;
        }
        if let Some((paths, layer)) = dropped_items(ui, released) {
            if let Some(preset) = paths.iter().find(|p| p.ends_with(".blpreset")) {
                invoke(
                    events,
                    "pad.drop",
                    json!({ "rack": frame.id, "note": pad.note, "preset": preset }),
                );
            } else {
                invoke(
                    events,
                    "pad.drop",
                    json!({ "rack": frame.id, "note": pad.note, "samples": paths, "layer": layer }),
                );
            }
        }
    }

    /// A drum chain's Input/Output controls and send sliders, and a return
    /// chain's Audio To, in its row (drum racks spec, R6, R8, R21).
    #[expect(clippy::too_many_arguments, reason = "the controls' inputs")]
    pub(crate) fn drum_cells(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        rack: &RackFrame,
        row: &ChainRow,
        rects: &ChainRowRects,
        clip: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let painter = ui.painter_at(clip);
        let _ = &painter;
        if let (Some(drum), Some(io)) = (&row.drum, &rects.io) {
            let preview_id = drum_control_id(row.id, "preview");
            let response = ui.interact(io.preview, preview_id, Sense::click_and_drag());
            crate::focus::register(ui.ctx(), preview_id, Area::Detail, None);
            self.held_button(ui, &response, preview_id, drum.previewing, |on| {
                invoke(
                    events,
                    "chain.preview",
                    json!({ "chain": row.id, "on": on }),
                );
            });
            let (face, ink) = button_look(theme, drum.previewing, false, response.hovered());
            let painter = ui.painter_at(io.preview.expand(1.0));
            painter.rect_filled(io.preview, 1.0, face);
            painter.add(Shape::convex_polygon(
                vec![
                    io.preview.center() + vec2(-2.0, -3.0),
                    io.preview.center() + vec2(3.0, 0.0),
                    io.preview.center() + vec2(-2.0, 3.0),
                ],
                ink,
                Stroke::NONE,
            ));
            response.widget_info(|| {
                WidgetInfo::selected(WidgetType::Button, true, drum.previewing, "Preview")
            });
            ui.ctx().accesskit_node_builder(preview_id, |node| {
                node.set_role(Role::Button);
                node.set_label(format!("Preview of {}", row.name));
                node.set_description("Plays the chain's note while it is held".to_owned());
            });
            let chain = row.id;
            // Receive: All Notes, then each note.
            let by = chooser(
                ui,
                theme,
                drum_control_id(chain, "receive"),
                io.receive,
                &receive_text(drum.receive, &drum.receive_label),
                true,
                &format!("Receive of {}", row.name),
                |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for (receive, label) in bl_model::receive_choices() {
                                let chosen = receive == drum.receive;
                                if ui
                                    .add(egui::Button::selectable(chosen, label.as_str()))
                                    .clicked()
                                {
                                    invoke(
                                        events,
                                        "chain.set_receive",
                                        json!({ "chain": chain, "receive": receive }),
                                    );
                                    ui.close();
                                }
                            }
                        });
                },
            );
            let at = match drum.receive {
                Receive::AllNotes => 0,
                Receive::Note(n) => i32::from(n) + 1,
            };
            let to = (at + by).clamp(0, 128);
            if by != 0 && to != at {
                let receive = if to == 0 {
                    Receive::AllNotes
                } else {
                    Receive::Note(to as u8 - 1)
                };
                invoke(
                    events,
                    "chain.set_receive",
                    json!({ "chain": chain, "receive": receive }),
                );
            }
            let by = chooser(
                ui,
                theme,
                drum_control_id(chain, "play"),
                io.play,
                &bl_model::note_name(drum.play),
                !drum.all_notes,
                &format!("Play of {}", row.name),
                |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for note in 0..128u8 {
                                let label = bl_model::receive_label(Receive::Note(note));
                                if ui
                                    .add(egui::Button::selectable(
                                        note == drum.play,
                                        label.as_str(),
                                    ))
                                    .clicked()
                                {
                                    invoke(
                                        events,
                                        "chain.set_play",
                                        json!({ "chain": chain, "play": note }),
                                    );
                                    ui.close();
                                }
                            }
                        });
                },
            );
            let to = (i32::from(drum.play) + by).clamp(0, 127);
            if by != 0 && to != i32::from(drum.play) {
                invoke(
                    events,
                    "chain.set_play",
                    json!({ "chain": chain, "play": to }),
                );
            }
            let by = chooser(
                ui,
                theme,
                drum_control_id(chain, "choke"),
                io.choke,
                &drum
                    .choke
                    .map_or_else(|| "None".to_owned(), |g| g.to_string()),
                !drum.all_notes,
                &format!("Choke of {}", row.name),
                |ui| {
                    for group in std::iter::once(None).chain((1..=16).map(Some)) {
                        let label = group.map_or_else(|| "None".to_owned(), |g| g.to_string());
                        if ui
                            .add(egui::Button::selectable(group == drum.choke, label))
                            .clicked()
                        {
                            invoke(
                                events,
                                "chain.set_choke",
                                json!({ "chain": chain, "choke": group }),
                            );
                            ui.close();
                        }
                    }
                },
            );
            let at = i32::from(drum.choke.unwrap_or(0));
            let to = (at + by).clamp(0, 16);
            if by != 0 && to != at {
                invoke(
                    events,
                    "chain.set_choke",
                    json!({ "chain": chain, "choke": (to > 0).then_some(to) }),
                );
            }
        }
        if let Some(drum) = &row.drum {
            let returns: Vec<&ChainRow> =
                rack.chains.iter().filter(|c| c.output.is_some()).collect();
            for (n, (at, level)) in rects.sends.iter().zip(&drum.sends).enumerate() {
                if let (Some(ret), Some(at)) = (returns.get(n), at) {
                    self.send_slider(ui, theme, row, ret, n, *at, *level, events);
                }
            }
        }
        if let (Some(output), Some(at)) = (&row.output, rects.audio_to) {
            let chain = row.id;
            let by = chooser(
                ui,
                theme,
                drum_control_id(chain, "audio_to"),
                at,
                &output.audio_to_label,
                true,
                &format!("Audio To of {}", row.name),
                |ui| {
                    if ui
                        .add(egui::Button::selectable(
                            output.audio_to.is_none(),
                            "Rack Output",
                        ))
                        .clicked()
                    {
                        invoke(
                            events,
                            "chain.set_audio_to",
                            json!({ "chain": chain, "track": null }),
                        );
                        ui.close();
                    }
                    for choice in &rack.return_tracks {
                        if ui
                            .add(egui::Button::selectable(
                                output.audio_to == Some(choice.track),
                                choice.label.as_str(),
                            ))
                            .clicked()
                        {
                            invoke(
                                events,
                                "chain.set_audio_to",
                                json!({ "chain": chain, "track": choice.track }),
                            );
                            ui.close();
                        }
                    }
                },
            );
            let options: Vec<Option<bl_model::TrackId>> = std::iter::once(None)
                .chain(rack.return_tracks.iter().map(|c| Some(c.track)))
                .collect();
            let at = options
                .iter()
                .position(|o| *o == output.audio_to)
                .unwrap_or(0) as i32;
            let to = (at + by).clamp(0, options.len() as i32 - 1);
            if by != 0 && to != at {
                invoke(
                    events,
                    "chain.set_audio_to",
                    json!({ "chain": chain, "track": options[to as usize] }),
                );
            }
        }
        let _ = clip;
    }

    /// A drum chain's send to a return chain: a drag sets it where the
    /// pointer is, one undo step; it starts at -inf dB.
    #[expect(clippy::too_many_arguments, reason = "the slider's inputs")]
    fn send_slider(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        row: &ChainRow,
        ret: &ChainRow,
        n: usize,
        at: Rect,
        level: f64,
        events: &mut Vec<UiEvent>,
    ) {
        let id = drum_control_id(row.id, &format!("send{n}"));
        let response = ui.interact(at, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let set = |value: f64| {
            Invocation::new(
                "chain.set_send",
                json!({ "chain": row.id, "to": ret.id, "level": value }),
            )
        };
        let position =
            |x: f32| volume_at(f64::from(((x - at.left()) / at.width()).clamp(0.0, 1.0)));
        let name = "Send \"Change\"";
        let _ = name;
        let (steps, reset) = Self::slider_keys(ui, &response);
        if reset || response.double_clicked() {
            events.push(UiEvent::BeginNamedGesture(format!(
                "Change \"{}\"",
                ret.name
            )));
            events.push(UiEvent::Invoke(set(0.0)));
            events.push(UiEvent::EndGesture);
        } else if steps != 0 {
            let position = bl_model::fader::fader_position(level) + f64::from(steps) * 0.02;
            events.push(UiEvent::BeginNamedGesture(format!(
                "Change \"{}\"",
                ret.name
            )));
            events.push(UiEvent::Invoke(set(volume_at(position.clamp(0.0, 1.0)))));
            events.push(UiEvent::EndGesture);
        }
        if response.drag_started() {
            events.push(UiEvent::BeginNamedGesture(format!(
                "Change \"{}\"",
                ret.name
            )));
        }
        if response.dragged()
            && !response.double_clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            events.push(UiEvent::Invoke(set(position(pointer.x))));
        }
        if response.drag_stopped() {
            events.push(UiEvent::EndGesture);
        } else if response.clicked()
            && !response.double_clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            events.push(UiEvent::BeginNamedGesture(format!(
                "Change \"{}\"",
                ret.name
            )));
            events.push(UiEvent::Invoke(set(position(pointer.x))));
            events.push(UiEvent::EndGesture);
        }
        let c = &theme.colors;
        let painter = ui.painter_at(at.expand(1.0));
        painter.rect_filled(at, theme.sizes.corner, c.window.0);
        let fill = bl_model::fader::fader_position(level) as f32;
        painter.rect_filled(
            Rect::from_min_max(at.min, pos2(at.left() + at.width() * fill, at.bottom())),
            theme.sizes.corner,
            c.accent.0.gamma_multiply(0.8),
        );
        let text = volume_text(level);
        painter.text(
            at.center(),
            Align2::CENTER_CENTER,
            &text,
            egui::FontId::proportional(7.0),
            c.text.0,
        );
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Slider, true, format!("Send to {}", ret.name))
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Slider);
            node.set_label(format!("Send of {} to {}", row.name, ret.name));
            node.set_value(text.clone());
        });
    }

    /// The return section's heading and drop area (drum racks spec, R7).
    pub(crate) fn return_section(
        &self,
        ui: &Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        list: &ChainListRects,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(section) = list.returns else {
            return;
        };
        let c = &theme.colors;
        let painter = ui.painter_at(list.area);
        painter.text(
            pos2(section.heading.left() + 3.0, section.heading.center().y),
            Align2::LEFT_CENTER,
            "Returns",
            theme.small_font(),
            c.text_dim.0,
        );
        painter.line_segment(
            [
                section.heading.left_bottom(),
                section.heading.right_bottom(),
            ],
            Stroke::new(0.5, c.separator.0),
        );
        let id = Id::new(("return-drop-area", frame.id));
        ui.interact(section.drop_area.intersect(list.area), id, Sense::hover());
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Group);
            node.set_label("Return Chains Drop Area".to_owned());
            node.set_description("Drop an audio effect here to add a return chain".to_owned());
        });
        painter.rect_stroke(
            section.drop_area,
            theme.sizes.corner,
            Stroke::new(1.0, c.separator.0),
            StrokeKind::Inside,
        );
        painter.text(
            section.drop_area.center(),
            Align2::CENTER_CENTER,
            if rack.can_add_return {
                "Drop an Audio Effect Here"
            } else {
                "Six return chains at most"
            },
            egui::FontId::proportional(9.0),
            c.text_dim.0,
        );
        let (pointer, released) =
            ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
        let Some(pointer) = pointer else {
            return;
        };
        if !section.drop_area.contains(pointer) || !released || !rack.can_add_return {
            return;
        }
        if let Some(device) = DragAndDrop::payload::<DeviceDrag>(ui.ctx()) {
            DragAndDrop::clear_payload(ui.ctx());
            invoke(
                events,
                "chain.create",
                json!({ "rack": frame.id, "move_device": device.device, "returns": true }),
            );
        } else if let Some(preset) = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_string_lossy().into_owned())
                .find(|p| p.ends_with(".blpreset"))
        }) {
            invoke(
                events,
                "chain.create",
                json!({ "rack": frame.id, "preset": preset, "returns": true }),
            );
        }
    }
}

/// What a pad reads to a screen reader.
fn pad_label(pad: &PadState) -> String {
    match pad.kind {
        PadKind::Empty => format!("Pad {}", pad.name),
        PadKind::Single | PadKind::Multi => format!("Pad {}: {}", pad.name, pad.label),
    }
}

/// What a Receive chooser's box reads.
fn receive_text(receive: Receive, label: &str) -> String {
    match receive {
        Receive::AllNotes => label.to_owned(),
        Receive::Note(note) => bl_model::note_name(note),
    }
}

/// A chooser: a box showing its choice, and a list a click opens.
#[expect(clippy::too_many_arguments, reason = "the chooser's inputs")]
fn chooser(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    text: &str,
    enabled: bool,
    name: &str,
    list: impl FnOnce(&mut Ui),
) -> i32 {
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
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(2.0));
    let face = if response.hovered() && enabled {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.rect(
        rect,
        theme.sizes.corner,
        if enabled {
            face
        } else {
            face.gamma_multiply(0.6)
        },
        Stroke::new(1.0, c.separator.0),
        StrokeKind::Inside,
    );
    let ink = if enabled { c.text.0 } else { c.pending.0 };
    ui.painter_at(Rect::from_min_max(
        rect.min,
        pos2(rect.right() - 8.0, rect.bottom()),
    ))
    .text(
        pos2(rect.left() + 3.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(8.0),
        ink,
    );
    if enabled {
        let a = pos2(rect.right() - 5.0, rect.center().y);
        painter.add(Shape::convex_polygon(
            vec![
                a + vec2(-2.0, -1.0),
                a + vec2(2.0, -1.0),
                a + vec2(0.0, 1.5),
            ],
            ink,
            Stroke::NONE,
        ));
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, enabled, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::ComboBox);
        node.set_label(name.to_owned());
        node.set_value(text.to_owned());
    });
    let mut by = 0;
    if enabled {
        // With the focus on it, Up and Down choose the entry before or after
        // the chosen one, as the In/Out choosers do.
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
            by = ui.input(|i| {
                i32::from(i.key_pressed(Key::ArrowDown)) - i32::from(i.key_pressed(Key::ArrowUp))
            });
        }
        egui::Popup::menu(&response)
            .width(rect.width().max(150.0))
            .show(list);
    }
    by
}
