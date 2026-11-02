//! The sidebar's Collections, Library and Places (L-BRW-002 to L-BRW-005;
//! spec: browser R2, browser-tags R24 to R26): saved labels, the Library
//! section's Edit mode, dragging labels, renaming them and giving them
//! icons.

use bl_shell::browser_commands::{self as ids, LABEL_ICONS, Label, LabelInfo, Part, Section};
use bl_shell::{Area, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, DragAndDrop, Id, Key, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind,
    Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use super::{
    BROWSER_ROW, BrowserUi, ClipDropTarget, SideRow, add_folder_id, bounds, collection_colour,
    drop_chains, drop_tracks, invoke, label_id, note_clip_target, scrolled, sidebar_id,
};
use crate::layout::BrowserLayout;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// What a drag of a label carries.
#[derive(Clone, Debug)]
struct LabelDrag(Label);

/// Where a label's row lies in the sidebar: how deep, whether it holds
/// nested collections, and the lists a drag reads its place from.
struct Place<'a> {
    depth: u16,
    holds: bool,
    /// The Library labels in the person's order.
    order: &'a [Label],
    /// The collections, nested ones under their parents.
    nesting: &'a [LabelInfo],
}

/// How far a nested collection's row is indented, per level.
const NEST_INDENT: f32 = 12.0;

/// The Library heading's Edit and Done button.
#[must_use]
pub fn labels_edit_id() -> Id {
    Id::new("browser-labels-edit")
}

/// The Collections heading's Edit and Done button.
#[must_use]
pub fn collections_edit_id() -> Id {
    Id::new("browser-collections-edit")
}

/// The field a label's name is edited in.
#[must_use]
pub fn label_rename_id() -> Id {
    Id::new("browser-label-rename")
}

/// A label's check box, in the Library section's Edit mode.
#[must_use]
pub fn label_check_id(label: &Label) -> Id {
    Id::new(("browser-label-check", label))
}

/// Draws one of Beatlayer's label icons (spec: browser-tags R25) in `rect`.
fn icon(painter: &Painter, rect: Rect, name: &str, colour: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.42;
    let stroke = Stroke::new(1.0, colour);
    let polygon = |points: Vec<Pos2>| {
        painter.add(Shape::convex_polygon(points, colour, Stroke::NONE));
    };
    match name {
        "star" => {
            let points: Vec<Pos2> = (0..10)
                .map(|i| {
                    let angle =
                        std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
                    let radius = if i % 2 == 0 { r } else { r * 0.45 };
                    pos2(c.x + radius * angle.cos(), c.y + radius * angle.sin())
                })
                .collect();
            painter.add(Shape::Path(egui::epaint::PathShape::convex_polygon(
                points,
                colour,
                Stroke::NONE,
            )));
        }
        "heart" => {
            painter.circle_filled(c + vec2(-r * 0.5, -r * 0.3), r * 0.55, colour);
            painter.circle_filled(c + vec2(r * 0.5, -r * 0.3), r * 0.55, colour);
            polygon(vec![
                c + vec2(-r * 1.0, -r * 0.1),
                c + vec2(r * 1.0, -r * 0.1),
                c + vec2(0.0, r),
            ]);
        }
        "circle" => {
            painter.circle_filled(c, r, colour);
        }
        "square" => {
            painter.rect_filled(
                Rect::from_center_size(c, vec2(r * 1.8, r * 1.8)),
                1.0,
                colour,
            );
        }
        "triangle" => polygon(vec![
            c + vec2(0.0, -r),
            c + vec2(r, r * 0.8),
            c + vec2(-r, r * 0.8),
        ]),
        "diamond" => polygon(vec![
            c + vec2(0.0, -r),
            c + vec2(r, 0.0),
            c + vec2(0.0, r),
            c + vec2(-r, 0.0),
        ]),
        "bolt" => {
            painter.add(Shape::line(
                vec![
                    c + vec2(r * 0.3, -r),
                    c + vec2(-r * 0.5, r * 0.1),
                    c + vec2(r * 0.4, -r * 0.1),
                    c + vec2(-r * 0.3, r),
                ],
                Stroke::new(1.5, colour),
            ));
        }
        "note" => {
            painter.circle_filled(c + vec2(-r * 0.3, r * 0.6), r * 0.45, colour);
            painter.line_segment([c + vec2(r * 0.1, r * 0.6), c + vec2(r * 0.1, -r)], stroke);
        }
        "wave" => {
            for (i, height) in [0.4f32, 1.0, 0.6, 0.9, 0.3].iter().enumerate() {
                let x = c.x - r + i as f32 * r * 0.5;
                painter.line_segment(
                    [pos2(x, c.y - r * height), pos2(x, c.y + r * height)],
                    stroke,
                );
            }
        }
        _ => {
            painter.line_segment([c + vec2(-r, 0.0), c + vec2(r, 0.0)], stroke);
            painter.line_segment([c + vec2(0.0, -r), c + vec2(0.0, r)], stroke);
        }
    }
}

impl BrowserUi {
    fn side_rows(&self, shell: &Shell) -> Vec<SideRow> {
        let labels = shell.browser_view().labels();
        let editing = self.editing_labels;
        let mut rows = vec![SideRow::CollectionsHeading];
        // Spec: browser-tags R45. A nested collection shows while every
        // collection it lies in is open.
        let collections: Vec<&LabelInfo> = labels
            .iter()
            .filter(|l| l.section == Section::Collections)
            .collect();
        let mut expanded: Vec<(&Label, u16)> = Vec::new();
        for info in &collections {
            let (visible, depth) = match &info.parent {
                None => (info.shown || self.editing_collections, 0),
                Some(parent) => expanded
                    .iter()
                    .find(|(label, _)| *label == parent)
                    .map_or((false, 0), |(_, depth)| (true, depth + 1)),
            };
            if visible {
                let holds = collections
                    .iter()
                    .any(|child| child.parent.as_ref() == Some(&info.label));
                rows.push(SideRow::Label((*info).clone(), depth, holds));
                if info.open {
                    expanded.push((&info.label, depth));
                }
            }
        }
        rows.push(SideRow::LibraryHeading);
        rows.extend(
            labels
                .iter()
                .filter(|l| l.section == Section::Library && (l.shown || editing))
                .cloned()
                .map(|l| SideRow::Label(l, 0, false)),
        );
        rows.push(SideRow::Heading("Places"));
        rows.extend(
            labels
                .into_iter()
                .filter(|l| l.section == Section::Places)
                .map(|l| SideRow::Label(l, 0, false)),
        );
        rows.push(SideRow::AddFolder);
        rows
    }

    pub(super) fn sidebar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let area = layout.sidebar;
        let view = shell.browser_view();
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, theme.colors.bar.0);
        let region = ui.interact(area, sidebar_id(), Sense::click());
        crate::focus::register(ui.ctx(), sidebar_id(), Area::Browser, None);
        ui.ctx().accesskit_node_builder(sidebar_id(), |node| {
            node.set_role(Role::List);
            node.set_label("Browser Sidebar".to_owned());
        });
        if region.clicked() {
            region.request_focus();
        }
        let rows = self.side_rows(shell);
        // The Library section in the order the person left it, for a drag
        // to read its place from.
        let order: Vec<Label> = view
            .labels()
            .into_iter()
            .filter(|l| l.section == Section::Library)
            .map(|l| l.label)
            .collect();
        // The collections as the sidebar lists them, for a drag to read its
        // place among the nested ones from.
        let nesting: Vec<LabelInfo> = view
            .labels()
            .into_iter()
            .filter(|l| l.section == Section::Collections)
            .collect();
        let content = rows.len() as f32 * BROWSER_ROW;
        self.sidebar_scroll = scrolled(ui, area, self.sidebar_scroll, content);
        // Spec: view-options R1. A middle-button drag pans the list, the
        // content following the pointer.
        if let Some(delta) = crate::middle::pan(ui, area) {
            self.sidebar_scroll =
                (self.sidebar_scroll - delta.y).clamp(0.0, (content - area.height()).max(0.0));
        }
        let first = (self.sidebar_scroll / BROWSER_ROW).floor() as usize;
        let mut sidebar_ui = ui.new_child(egui::UiBuilder::new().max_rect(area));
        sidebar_ui.set_clip_rect(area);
        for (index, row) in rows.iter().enumerate().skip(first) {
            let top = area.top() + index as f32 * BROWSER_ROW - self.sidebar_scroll;
            if top > area.bottom() {
                break;
            }
            let rect = Rect::from_min_size(pos2(area.left(), top), vec2(area.width(), BROWSER_ROW));
            match row {
                SideRow::Heading(name) => {
                    let mut text = (*name).to_owned();
                    if *name == "Places" && view.index_state().indexing {
                        text.push_str(" \u{b7} indexing");
                    }
                    painter.text(
                        pos2(rect.left() + 4.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        text,
                        theme.small_font(),
                        theme.colors.text_dim.0,
                    );
                    let id = Id::new(("browser-heading", *name));
                    let _ = sidebar_ui.interact(rect, id, Sense::hover());
                    ui.ctx().accesskit_node_builder(id, |node| {
                        node.set_role(Role::Heading);
                        node.set_label((*name).to_owned());
                        if *name == "Places" && view.index_state().indexing {
                            node.set_description("Indexing the places' files".to_owned());
                        }
                    });
                }
                SideRow::CollectionsHeading => {
                    self.section_heading(
                        &mut sidebar_ui,
                        theme,
                        rect,
                        Section::Collections,
                        events,
                    );
                }
                SideRow::LibraryHeading => {
                    self.section_heading(&mut sidebar_ui, theme, rect, Section::Library, events);
                }
                SideRow::Label(info, depth, holds) => {
                    let place = Place {
                        depth: *depth,
                        holds: *holds,
                        order: &order,
                        nesting: &nesting,
                    };
                    self.label_row(&mut sidebar_ui, shell, theme, rect, info, &place, events);
                }
                SideRow::AddFolder => {
                    let response = sidebar_ui.interact(rect, add_folder_id(), Sense::click());
                    if response.hovered() {
                        painter.rect_filled(rect, 0.0, theme.colors.control_hover.0);
                    }
                    painter.text(
                        pos2(rect.left() + 8.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        "Add Folder\u{2026}",
                        theme.small_font(),
                        theme.colors.text.0,
                    );
                    if response.clicked() {
                        invoke(events, ids::ADD_FOLDER, json!({}));
                    }
                    response.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::Button, true, "Add Folder\u{2026}")
                    });
                }
            }
        }
        // A label dragged and released outside the sidebar's rows is
        // dropped.
        if DragAndDrop::has_payload_of_type::<LabelDrag>(ui.ctx())
            && ui.input(|i| i.pointer.any_released())
        {
            DragAndDrop::clear_payload(ui.ctx());
        }
        // Spec: accessibility R7: outlined only with Outline View in Focus.
        if ui.memory(|m| m.has_focus(sidebar_id())) && crate::focus::outlined(ui.ctx()) {
            ui.painter().rect_stroke(
                area.shrink(1.0),
                0.0,
                Stroke::new(1.0, theme.colors.focus.0),
                StrokeKind::Inside,
            );
        }
    }

    /// The Library or Collections heading: Edit (and Done) at its right
    /// while the pointer is there, and on the Library heading a context menu
    /// with Reset Label Order (spec: browser-tags R26, R39).
    fn section_heading(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        section: Section,
        events: &mut Vec<UiEvent>,
    ) {
        let collections = section == Section::Collections;
        let title = if collections {
            "Collections"
        } else {
            "Library"
        };
        let editing = if collections {
            &mut self.editing_collections
        } else {
            &mut self.editing_labels
        };
        let painter = ui.painter_at(rect);
        painter.text(
            pos2(rect.left() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            title,
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        let id = Id::new(("browser-heading", title));
        let heading = ui.interact(rect, id, Sense::click());
        if !collections {
            heading.context_menu(|ui| {
                let item = ui.button("Reset Label Order");
                crate::menus::register_item(ui.ctx(), &item);
                crate::menus::take_focus_first(ui, &item);
                if item.clicked() {
                    invoke(events, ids::RESET_LABEL_ORDER, json!({}));
                    ui.close();
                }
            });
        }
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Heading);
            node.set_label(title.to_owned());
            node.set_bounds(bounds(rect));
        });
        let button = Rect::from_min_max(pos2(rect.right() - 36.0, rect.top()), rect.max);
        let edit_id = if collections {
            collections_edit_id()
        } else {
            labels_edit_id()
        };
        let response = ui.interact(button, edit_id, Sense::click());
        let name = if *editing { "Done" } else { "Edit" };
        // Edit shows when the pointer is to the right of the heading.
        let right_of_heading = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|at| rect.contains(at) && at.x > rect.left() + 44.0);
        if *editing || right_of_heading || response.hovered() {
            let colour = if response.hovered() {
                theme.colors.text.0
            } else {
                theme.colors.text_dim.0
            };
            painter.text(
                pos2(button.right() - 6.0, button.center().y),
                Align2::RIGHT_CENTER,
                name,
                theme.small_font(),
                colour,
            );
        }
        if response.clicked() {
            *editing = !*editing;
        }
        ui.ctx().accesskit_node_builder(edit_id, |node| {
            node.set_role(Role::Button);
            node.set_label(name.to_owned());
            node.set_bounds(bounds(button));
        });
    }

    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn label_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        info: &LabelInfo,
        place: &Place<'_>,
        events: &mut Vec<UiEvent>,
    ) {
        let order = place.order;
        let view = shell.browser_view();
        let id = label_id(&info.label);
        let library = info.section == Section::Library;
        let collection = info.section == Section::Collections;
        let editing = if collection {
            self.editing_collections
        } else {
            self.editing_labels
        };
        let movable = (library && info.label != Label::All) || info.label.is_nested_collection();
        let response = ui.interact(
            rect,
            id,
            if movable {
                Sense::click_and_drag()
            } else {
                Sense::CLICK
            },
        );
        let chosen = *view.label() == info.label;
        let painter = ui.painter_at(rect);
        if chosen {
            painter.rect_filled(rect, 0.0, theme.colors.lane_selected.0);
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, theme.colors.control_hover.0);
        }
        let colour = if !info.available || (editing && !info.shown) {
            theme.colors.pending.0
        } else {
            theme.colors.text.0
        };
        let mut left = rect.left() + 8.0 + f32::from(place.depth) * NEST_INDENT;
        // Spec: browser-tags R45. A collection holding others has a fold
        // triangle, which opens and closes its list.
        if place.holds {
            self.fold_triangle(ui, theme, rect, left - 4.0, info, events);
        }
        // Edit mode: a check box that shows or hides the label (R26, R39).
        if (library || collection) && editing {
            let check =
                Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(9.0, 9.0));
            let box_response = ui.interact(
                check.expand(3.0),
                label_check_id(&info.label),
                Sense::click(),
            );
            painter.rect_stroke(
                check,
                0.0,
                Stroke::new(1.0, theme.colors.text_dim.0),
                StrokeKind::Inside,
            );
            if info.shown {
                painter.text(
                    check.center(),
                    Align2::CENTER_CENTER,
                    "\u{2713}",
                    theme.small_font(),
                    theme.colors.text.0,
                );
            }
            if box_response.clicked() && info.label != Label::All {
                invoke(
                    events,
                    ids::SET_LABEL_SHOWN,
                    json!({ "label": info.label, "shown": !info.shown }),
                );
            }
            let shown = info.shown;
            let name = info.name.clone();
            ui.ctx()
                .accesskit_node_builder(label_check_id(&info.label), |node| {
                    node.set_role(Role::CheckBox);
                    node.set_label(name);
                    node.set_toggled(if shown {
                        egui::accesskit::Toggled::True
                    } else {
                        egui::accesskit::Toggled::False
                    });
                });
            left = rect.left() + 22.0 + f32::from(place.depth) * NEST_INDENT;
        }
        if let Some(name) = &info.icon {
            icon(
                &painter,
                Rect::from_center_size(pos2(left + 5.0, rect.center().y), vec2(11.0, 11.0)),
                name,
                colour,
            );
            left += 14.0;
        } else if let Label::Collection(number) = info.label {
            // Spec: browser-tags R34, R46. A collection's colour; a nested
            // one has the colour of the collection at its top.
            painter.circle_filled(
                pos2(left + 3.5, rect.center().y),
                3.5,
                collection_colour(theme, view.collection_top(number)),
            );
            left += 12.0;
        }
        if view.renaming_label() == Some(&info.label) {
            self.rename_label_field(ui, theme, rect, left, info, events);
        } else {
            painter.text(
                pos2(left, rect.center().y),
                Align2::LEFT_CENTER,
                &info.name,
                theme.small_font(),
                colour,
            );
        }
        if response.clicked() {
            if view.focus() != Part::Sidebar
                || crate::window::focused_area(ui.ctx(), shell) != Area::Browser
            {
                invoke(events, ids::FOCUS_BROWSER, json!({ "part": "sidebar" }));
            }
            invoke(events, ids::OPEN_PLACE, json!({ "label": info.label }));
        }
        // Spec: browser-tags R26. A label dragged within the Library section
        // moves, All staying first (I-DRAG-056).
        if movable && response.drag_started() {
            DragAndDrop::set_payload(ui.ctx(), LabelDrag(info.label.clone()));
        }
        if library
            && let Some(drag) = DragAndDrop::payload::<LabelDrag>(ui.ctx())
            && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
            && rect.contains(pointer)
            && ui.clip_rect().contains(pointer)
        {
            let at = order
                .iter()
                .position(|l| *l == info.label)
                .unwrap_or(0)
                .max(1);
            painter.line_segment(
                [rect.left_top(), rect.right_top()],
                Stroke::new(2.0, theme.colors.accent.0),
            );
            if ui.input(|i| i.pointer.any_released()) {
                let label = drag.0.clone();
                DragAndDrop::clear_payload(ui.ctx());
                if label != info.label {
                    invoke(
                        events,
                        ids::MOVE_LABEL,
                        json!({ "label": label, "position": at }),
                    );
                }
            }
        }
        // Spec: browser-tags R45 (I-DRAG-056). A nested collection's label
        // dropped on a collection moves into it, or before a nested one when
        // dropped on its upper quarter.
        if info.section == Section::Collections
            && let Some(drag) = DragAndDrop::payload::<LabelDrag>(ui.ctx())
            && drag.0.is_nested_collection()
            && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
            && rect.contains(pointer)
            && ui.clip_rect().contains(pointer)
        {
            let before =
                info.label.is_nested_collection() && pointer.y < rect.top() + rect.height() * 0.25;
            if before {
                painter.line_segment(
                    [rect.left_top(), rect.right_top()],
                    Stroke::new(2.0, theme.colors.accent.0),
                );
            } else {
                painter.rect_stroke(
                    rect.shrink(1.0),
                    0.0,
                    Stroke::new(1.0, theme.colors.accent.0),
                    StrokeKind::Inside,
                );
            }
            if ui.input(|i| i.pointer.any_released()) {
                let moved = drag.0.clone();
                DragAndDrop::clear_payload(ui.ctx());
                if moved != info.label {
                    let (parent, index) = match (&info.parent, before) {
                        (Some(parent), true) => (
                            parent.clone(),
                            place
                                .nesting
                                .iter()
                                .filter(|l| l.parent.as_ref() == Some(parent))
                                .position(|l| l.label == info.label),
                        ),
                        _ => (info.label.clone(), None),
                    };
                    invoke(
                        events,
                        ids::MOVE_COLLECTION,
                        json!({
                            "collection": collection_number(&moved),
                            "parent": collection_number(&parent),
                            "index": index,
                        }),
                    );
                }
            }
        }
        // Spec: browser-tags R46 (I-DROP-024). Items dropped on a nested
        // collection's label go into it.
        if info.label.is_nested_collection()
            && let Some(keys) = super::dragged_item_keys(ui.ctx())
            && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
            && rect.contains(pointer)
            && ui.clip_rect().contains(pointer)
        {
            painter.rect_stroke(
                rect.shrink(1.0),
                0.0,
                Stroke::new(1.0, theme.colors.accent.0),
                StrokeKind::Inside,
            );
            if ui.input(|i| i.pointer.any_released()) {
                DragAndDrop::clear_payload(ui.ctx());
                invoke(
                    events,
                    ids::SET_COLLECTION,
                    json!({
                        "collection": collection_number(&info.label),
                        "items": keys,
                        "member": true,
                    }),
                );
            }
        }
        // Spec: racks R46. A chain dropped on a place is saved there; on the
        // User Library, in its folder for the rack's presets.
        match (&info.label, &info.path) {
            (Label::UserLibrary, _) => drop_chains(ui, theme, rect, "", events),
            (label, Some(path)) if label.is_place() => {
                drop_chains(ui, theme, rect, path, events);
            }
            _ => {}
        }
        // Spec: set-exchange R11. Track title bars dropped on a place's label
        // are saved there as a new set.
        if info.label.is_place()
            && let Some(path) = &info.path
        {
            drop_tracks(ui, shell, theme, rect, path, events);
        }
        // Spec: clip files R3. A clip dropped on a place's label is saved in
        // its folder, and on the Clips label in the User Library's `Clips`.
        match (&info.label, &info.path) {
            (Label::Clips, _) => note_clip_target(ui, theme, rect, ClipDropTarget::Clips),
            (label, Some(path)) if label.is_place() => {
                note_clip_target(ui, theme, rect, ClipDropTarget::Folder(path.clone()));
            }
            _ => {}
        }
        // C-BROWSER-017, C-BROWSER-018: the label's context menu.
        let holds = match info.label {
            Label::Collection(number) => view.collection_holds(number),
            _ => 0,
        };
        let menu =
            |ui: &mut Ui, events: &mut Vec<UiEvent>| self.label_menu(ui, info, holds, events);
        if has_menu(&info.label) {
            response.context_menu(|ui| menu(ui, events));
            // S-CTX-002: the keys open the chosen place's menu while the
            // sidebar has the keyboard (the accessibility spec, R8).
            if chosen
                && ui.memory(|m| m.has_focus(sidebar_id()))
                && crate::menus::context_menu_keys(ui)
            {
                crate::menus::open_keyboard_context_menu(ui.ctx(), id, rect.left_bottom());
            }
            crate::menus::keyboard_context_menu(&response, |ui| menu(ui, events));
        }
        response.widget_info(|| WidgetInfo::selected(WidgetType::Other, true, chosen, &info.name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::ListItem);
            node.set_label(info.name.clone());
            node.set_selected(chosen);
            if let Some(path) = &info.path {
                node.set_description(path.clone());
            } else if let Some(note) = info.label.pending() {
                node.set_description(note.to_owned());
            } else if let Some(name) = &info.icon {
                node.set_description(format!("icon: {name}"));
            }
        });
    }

    /// The fold triangle of a collection that holds others, centred at
    /// `x`: it opens and closes the list of the collections nested in it.
    fn fold_triangle(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        x: f32,
        info: &LabelInfo,
        events: &mut Vec<UiEvent>,
    ) {
        let c = pos2(x + 4.0, rect.center().y);
        let id = Id::new(("browser-fold", &info.label));
        let response = ui.interact(
            Rect::from_center_size(c, vec2(10.0, rect.height())),
            id,
            Sense::click(),
        );
        let points = if info.open {
            vec![
                c + vec2(-3.0, -2.0),
                c + vec2(3.0, -2.0),
                c + vec2(0.0, 2.5),
            ]
        } else {
            vec![
                c + vec2(-2.0, -3.0),
                c + vec2(-2.0, 3.0),
                c + vec2(2.5, 0.0),
            ]
        };
        ui.painter_at(rect).add(Shape::convex_polygon(
            points,
            theme.colors.text_dim.0,
            Stroke::NONE,
        ));
        if response.clicked() {
            invoke(
                events,
                ids::OPEN_FOLDER,
                json!({ "path": format!("collection:{}", collection_number(&info.label)) }),
            );
        }
        let name = format!(
            "{} {}",
            if info.open { "Collapse" } else { "Expand" },
            info.name
        );
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(name);
        });
    }

    /// A label's context menu: a folder in Places can be removed from the
    /// sidebar and given an icon; a Library label or a saved label can be
    /// renamed (Cmd-R) and given an icon, and a saved label removed
    /// (C-BROWSER-017, C-BROWSER-018; spec: browser-tags R25).
    fn label_menu(&self, ui: &mut Ui, info: &LabelInfo, holds: usize, events: &mut Vec<UiEvent>) {
        let mut first = true;
        let mut button = |ui: &mut Ui, text: &str| -> egui::Response {
            let response = ui.button(text);
            crate::menus::register_item(ui.ctx(), &response);
            if first {
                crate::menus::take_focus_first(ui, &response);
                first = false;
            }
            response
        };
        if let Label::Folder(path) = &info.label
            && button(ui, "Remove from Sidebar").clicked()
        {
            invoke(events, ids::REMOVE_FOLDER, json!({ "path": path }));
            ui.close();
        }
        let nested = info.label.is_nested_collection();
        if (info.label.is_library() || info.label.is_collection() || nested)
            && button(ui, "Rename").clicked()
        {
            invoke(events, ids::RENAME_LABEL, json!({ "label": info.label }));
            ui.close();
        }
        if (info.label.is_collection() || nested) && button(ui, "New Collection").clicked() {
            invoke(
                events,
                ids::ADD_COLLECTION,
                json!({ "parent": collection_number(&info.label) }),
            );
            ui.close();
        }
        if nested && button(ui, "Delete").clicked() {
            let number = collection_number(&info.label);
            let args = json!({ "collection": number });
            // Spec: browser-tags R47. Asks first when it holds items.
            let held = holds;
            if held == 0 {
                invoke(events, ids::DELETE_COLLECTION, args);
            } else {
                super::ask_delete_collection(ui.ctx(), args, &info.name, held);
            }
            ui.close();
        }
        if info.label.is_collection() || nested {
            // A collection has its colour, not an icon (spec: browser-tags
            // R38).
            return;
        }
        let has_icon = info.icon.is_some();
        ui.menu_button("Choose Icon", |ui| {
            for name in LABEL_ICONS {
                let mark = if info.icon.as_deref() == Some(name) {
                    "\u{2714} "
                } else {
                    ""
                };
                let response = ui.button(format!("{mark}{name}"));
                crate::menus::register_item(ui.ctx(), &response);
                if response.clicked() {
                    invoke(
                        events,
                        ids::SET_LABEL_ICON,
                        json!({ "label": info.label, "icon": name }),
                    );
                    ui.close();
                }
            }
            ui.separator();
            let reset = ui.add_enabled(has_icon, egui::Button::new("Reset Icon"));
            crate::menus::register_item(ui.ctx(), &reset);
            if reset.clicked() {
                invoke(events, ids::SET_LABEL_ICON, json!({ "label": info.label }));
                ui.close();
            }
        });
        if matches!(info.label, Label::Saved(_)) && button(ui, "Remove").clicked() {
            invoke(events, ids::REMOVE_LABEL, json!({ "label": info.label }));
            ui.close();
        }
    }

    /// The text field a label's name is edited in: Enter keeps the name
    /// typed, Esc and a click elsewhere keep the old one.
    fn rename_label_field(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        row: Rect,
        left: f32,
        info: &LabelInfo,
        events: &mut Vec<UiEvent>,
    ) {
        let first = self.label_text_for.as_ref() != Some(&info.label);
        if first {
            self.label_text_for = Some(info.label.clone());
            self.label_text.clone_from(&info.name);
        }
        let rect = Rect::from_min_max(
            pos2(left, row.top() + 1.0),
            pos2((row.right() - 4.0).max(left + 40.0), row.bottom() - 1.0),
        );
        let edit = egui::TextEdit::singleline(&mut self.label_text)
            .id(label_rename_id())
            .font(theme.small_font())
            .margin(vec2(2.0, 0.0));
        let response = ui.put(rect, edit);
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("Label Name".to_owned());
        });
        if first {
            response.request_focus();
            let len = self.label_text.chars().count();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), label_rename_id()) {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(len),
                    )));
                state.store(ui.ctx(), label_rename_id());
            }
        }
        if response.lost_focus() && !first {
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                invoke(
                    events,
                    ids::RENAME_LABEL,
                    json!({ "label": info.label, "name": self.label_text }),
                );
            } else {
                invoke(events, ids::RENAME_LABEL, json!({ "label": info.label }));
            }
            self.label_text_for = None;
        }
    }
}

/// The number of a collection's label.
fn collection_number(label: &Label) -> u32 {
    match label {
        Label::Collection(number) => *number,
        _ => 0,
    }
}

/// Whether a label has a context menu: a folder in Places, a Library label,
/// a saved label and a collection do; All, the User Library and Current
/// Project don't.
fn has_menu(label: &Label) -> bool {
    !matches!(
        label,
        Label::All | Label::UserLibrary | Label::CurrentProject
    )
}
