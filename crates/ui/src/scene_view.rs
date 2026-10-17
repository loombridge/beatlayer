//! The Scene View (WP-3.5; layout.md L-SES-011): the detail area's contents
//! while a scene is selected in the Session View. A title bar with the
//! scene's name or number, then its Tempo and Signature fields, then the
//! follow-action controls, drawn in place and pending until WP-3.10 makes
//! them live.

use bl_model::SceneId;
use bl_shell::Shell;
use bl_shell::scene_fields::SceneField;
use egui::accesskit::Role;
use egui::{Align2, Id, Rect, Ui, pos2, vec2};

use crate::main_window::UiEvent;
use crate::scene_fields;
use crate::theme::Theme;

/// The title bar's height, as the other views' title bars.
const TITLE: f32 = 17.0;
/// A field's size.
const FIELD: egui::Vec2 = vec2(52.0, 18.0);
/// A part of the time signature's size.
const PART: egui::Vec2 = vec2(26.0, 18.0);

/// The ID of the Scene View's group.
#[must_use]
pub fn scene_view_id() -> Id {
    Id::new("scene-view")
}

/// Draws the Scene View for `scene` in `area`.
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    area: Rect,
    scene: SceneId,
    events: &mut Vec<UiEvent>,
) {
    if shell.editor().document().scene(scene).is_none() {
        return;
    }
    let label = scene_fields::scene_label(shell, scene);
    let c = &theme.colors;
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, c.panel.0);
    let title = Rect::from_min_size(area.min, vec2(area.width(), TITLE));
    painter.rect_filled(title, 0.0, c.lane.0);
    painter.text(
        pos2(title.left() + 6.0, title.center().y),
        Align2::LEFT_CENTER,
        &label,
        theme.small_font(),
        c.text.0,
    );
    ui.ctx().accesskit_node_builder(scene_view_id(), |node| {
        node.set_role(Role::Group);
        node.set_label(format!("Scene View: {label}"));
    });

    // Tempo, then Signature.
    let row = area.min + vec2(8.0, TITLE + 8.0);
    painter.text(
        pos2(row.x, row.y + FIELD.y / 2.0),
        Align2::LEFT_CENTER,
        "Tempo",
        theme.small_font(),
        c.text_dim.0,
    );
    let tempo = Rect::from_min_size(row + vec2(60.0, 0.0), FIELD);
    scene_fields::show(
        ui,
        shell,
        theme,
        tempo,
        scene,
        SceneField::Tempo,
        &label,
        events,
    );
    let below = row + vec2(0.0, FIELD.y + 6.0);
    painter.text(
        pos2(below.x, below.y + FIELD.y / 2.0),
        Align2::LEFT_CENTER,
        "Signature",
        theme.small_font(),
        c.text_dim.0,
    );
    let numerator = Rect::from_min_size(below + vec2(60.0, 0.0), PART);
    let denominator = Rect::from_min_size(numerator.right_top() + vec2(10.0, 0.0), PART);
    painter.text(
        pos2(numerator.right() + 5.0, numerator.center().y),
        Align2::CENTER_CENTER,
        "/",
        theme.small_font(),
        c.text_dim.0,
    );
    scene_fields::show(
        ui,
        shell,
        theme,
        numerator,
        scene,
        SceneField::Numerator,
        &label,
        events,
    );
    scene_fields::show(
        ui,
        shell,
        theme,
        denominator,
        scene,
        SceneField::Denominator,
        &label,
        events,
    );

    // The follow-action controls, drawn in place and pending until WP-3.10.
    let follow = row + vec2(200.0, 0.0);
    let id = Id::new("scene-view-follow-action");
    for (index, (what, text)) in [
        ("Follow Action", "Follow Action"),
        ("Follow Action A", "A  Next"),
        ("Follow Action B", "B  No Action"),
        ("Chance A", "100 %"),
        ("Chance B", "0 %"),
        ("Follow Time", "Longest  1x (1.1.0)"),
    ]
    .into_iter()
    .enumerate()
    {
        let at = follow + vec2(0.0, index as f32 * (FIELD.y + 2.0));
        painter.text(
            pos2(at.x, at.y + FIELD.y / 2.0),
            Align2::LEFT_CENTER,
            text,
            theme.small_font(),
            c.text_dim.0,
        );
        let item = id.with(index);
        ui.ctx().accesskit_node_builder(item, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("{label} {what}"));
            node.set_disabled();
            node.set_description("Not built yet: arrives with WP-3.10.".to_owned());
        });
    }
}
