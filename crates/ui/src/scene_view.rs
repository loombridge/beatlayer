//! The Scene View (WP-3.5; layout.md L-SES-011): the detail area's contents
//! while a double-click on a scene's number has it open. One column with a
//! title bar headed "Scene" and the scene's name or number at its right,
//! its Tempo and Signature fields under their headings, then the
//! follow-action controls (WP-3.10).

use bl_model::SceneId;
use bl_shell::Shell;
use bl_shell::scene_fields::SceneField;
use egui::accesskit::Role;
use egui::{Align2, Id, Rect, Sense, Ui, pos2, vec2};

use crate::main_window::UiEvent;
use crate::scene_fields;
use crate::theme::Theme;

/// The column's width, and the padding inside it.
const WIDTH: f32 = 190.0;
const PAD: f32 = 6.0;
/// The title bar's height, as the other views' title bars.
const TITLE: f32 = 17.0;
/// The Tempo field's size, and a part of the time signature's.
const FIELD: egui::Vec2 = vec2(86.0, 17.0);
const PART: egui::Vec2 = vec2(36.0, 17.0);

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
    let title = Rect::from_min_size(area.min, vec2(WIDTH.min(area.width()), TITLE));
    painter.rect_filled(title, 0.0, c.lane.0);
    painter.text(
        pos2(title.left() + PAD, title.center().y),
        Align2::LEFT_CENTER,
        "Scene",
        theme.small_font(),
        c.text.0,
    );
    painter.text(
        pos2(title.right() - PAD, title.center().y),
        Align2::RIGHT_CENTER,
        label.strip_prefix("Scene ").unwrap_or(&label),
        theme.small_font(),
        c.text.0,
    );
    // Spec: session-context-menus R3. A right-click on the title bar opens
    // the scene's menu for the selected scenes.
    let bar = ui.interact(title, Id::new(("scene-view-title", scene)), Sense::click());
    crate::session_menus::SessionMenus::default().scene(
        ui,
        theme,
        &bar,
        shell,
        scene,
        (title.left_bottom(), false),
        events,
    );
    ui.ctx().accesskit_node_builder(scene_view_id(), |node| {
        node.set_role(Role::Group);
        node.set_label(format!("Scene View: {label}"));
    });

    // Tempo and Signature, each heading over its field.
    let top = area.min + vec2(PAD, TITLE + 4.0);
    let fields = top + vec2(0.0, 13.0);
    for (heading, x) in [("Tempo", 0.0), ("Signature", FIELD.x + 5.0)] {
        painter.text(
            pos2(top.x + x + 1.0, top.y + 5.0),
            Align2::LEFT_CENTER,
            heading,
            theme.small_font(),
            c.text_dim.0,
        );
    }
    scene_fields::show(
        ui,
        shell,
        theme,
        Rect::from_min_size(fields, FIELD),
        scene,
        SceneField::Tempo,
        &label,
        events,
    );
    let numerator = Rect::from_min_size(fields + vec2(FIELD.x + 5.0, 0.0), PART);
    let denominator = Rect::from_min_size(numerator.right_top() + vec2(12.0, 0.0), PART);
    painter.text(
        pos2(numerator.right() + 6.0, numerator.center().y),
        Align2::CENTER_CENTER,
        "/",
        theme.small_font(),
        c.text_dim.0,
    );
    for (rect, part) in [
        (numerator, SceneField::Numerator),
        (denominator, SceneField::Denominator),
    ] {
        scene_fields::show(ui, shell, theme, rect, scene, part, &label, events);
    }

    // Spec: follow-actions R14. The follow-action controls, under them.
    let follow = shell
        .editor()
        .document()
        .scene(scene)
        .map_or_else(Default::default, |scene| scene.follow);
    crate::follow_controls::show(
        ui,
        theme,
        fields + vec2(0.0, FIELD.y + 8.0),
        crate::follow_controls::Owner::Scene(scene),
        &follow,
        &format!("{label} "),
        shell.editor().document().scenes().len(),
        events,
    );
}
