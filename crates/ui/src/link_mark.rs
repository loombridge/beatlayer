//! The link mark of a linked clip (WP-6.44; the linked clips spec, R10, R11):
//! a small chain glyph at the right end of the clip's title bar in the
//! Arrangement, at the right of its slot in the Session View and beside its
//! name in the Clip View. Pointing at it says how many other clips the clip
//! is linked to in the Status Bar, and clicking it selects the link's clips.

use bl_model::{ClipId, Document};
use bl_shell::Invocation;
use egui::{
    CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::main_window::UiEvent;

/// How wide the mark's hit area is, in points.
pub const WIDTH: f32 = 14.0;

/// What the Status Bar says while the pointer is over the mark.
#[must_use]
pub fn hint(others: usize) -> String {
    match others {
        1 => "Linked to 1 other clip".to_owned(),
        n => format!("Linked to {n} other clips"),
    }
}

/// How assistive technology reads a linked clip's name: `name`, then that
/// it's linked and how many clips its link holds (R11).
#[must_use]
pub fn described(name: &str, others: usize) -> String {
    format!("{name}, linked, {} clips", others + 1)
}

/// The mark's rectangle at the right end of `row`, or `None` where the row
/// is too narrow to give it room beside a name.
#[must_use]
pub fn rect(row: Rect) -> Option<Rect> {
    (row.width() >= 2.0 * WIDTH)
        .then(|| Rect::from_min_max(pos2(row.right() - WIDTH, row.top()), row.right_bottom()))
}

/// Draws the mark in `area` in `ink` and reads the pointer on it: the Status
/// Bar's hint while the pointer is over it, and whether it was clicked.
/// `id` names the mark; `others` is how many other clips the clip's link
/// holds.
pub fn show(
    ui: &Ui,
    id: Id,
    area: Rect,
    ink: egui::Color32,
    others: usize,
    events: &mut Vec<UiEvent>,
) -> bool {
    let response = ui.interact(area, id, Sense::click());
    let centre = area.center();
    let (half_width, half_height) = (4.5, 2.5);
    let stroke = Stroke::new(1.2, ink);
    let radius = CornerRadius::same(2);
    // Two rings, each an open rounded rectangle, overlapping: a chain link.
    for shift in [-2.5, 2.5] {
        let link = Rect::from_center_size(
            pos2(centre.x + shift, centre.y),
            vec2(half_width * 2.0 - 1.0, half_height * 2.0),
        );
        ui.painter()
            .rect_stroke(link, radius, stroke, StrokeKind::Inside);
    }
    let label = format!("Select the {} clips linked with this one", others + 1);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
    // The hint stays up while the pointer does, and goes when it leaves.
    let key = Id::new(("link-mark-hint", id));
    let shown = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    if response.hovered() != shown {
        ui.data_mut(|d| d.insert_temp(key, response.hovered()));
        events.push(UiEvent::StatusHint(
            response.hovered().then(|| hint(others)),
        ));
    }
    response.clicked()
}

/// Selects every clip of `clip`'s link that sits where `clip` does: the
/// Arrangement's clips, or the Session View's slots (R10).
pub fn select_link(doc: &Document, clip: ClipId, events: &mut Vec<UiEvent>) {
    let slot_of = |id: ClipId| {
        let owner = doc.track(doc.clip_track(id)?)?;
        let (scene, _) = owner.slots().find(|(_, held)| held.id == id)?;
        Some((owner.id(), scene))
    };
    let members: Vec<ClipId> = std::iter::once(clip)
        .chain(doc.linked_clip_ids(clip))
        .collect();
    if slot_of(clip).is_some() {
        for (at, (track, scene)) in members.into_iter().filter_map(slot_of).enumerate() {
            let mode = if at == 0 { "replace" } else { "add" };
            events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::session_view_commands::SELECT,
                json!({ "track": track, "scene": scene, "mode": mode }),
            )));
        }
    } else {
        let in_slots = |id: &ClipId| slot_of(*id).is_none();
        let clips: Vec<ClipId> = members.into_iter().filter(in_slots).collect();
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::editing_commands::SELECT_CLIPS,
            json!({ "clips": clips, "linked": false }),
        )));
    }
}
