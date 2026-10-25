//! The browser's Tuning section (WP-6.10; the tunings spec, R26 to R28): the
//! loaded tuning's name, a triangle that opens its Octave, Note, Ref. Pitch,
//! Lowest Note and Highest Note, and the save and link buttons, each control
//! running a registered command. A tuning file dropped on it loads.

use bl_model::{Tuning, TuningNote};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, DragAndDrop, Id, Key, Painter, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::layout::TUNING_ROW;
use crate::main_window::{InfoViewContent, UiEvent};
use crate::theme::Theme;
use crate::widgets::{Choice, NO_LEDS, chooser, number_box};

/// What dragging a tuning from the browser's list carries: its path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuningDrag {
    /// The tuning file, or a bundled tuning's path.
    pub path: String,
}

/// The name's ID: the control Navigate ▸ Tuning focuses, and the one Delete
/// removes the tuning from.
#[must_use]
pub fn name_id() -> Id {
    Id::new("browser-tuning-name")
}

/// The triangle's ID.
#[must_use]
pub fn triangle_id() -> Id {
    Id::new("browser-tuning-triangle")
}

fn request_id() -> Id {
    Id::new("browser-tuning-focus-request")
}

fn info_id() -> Id {
    Id::new("browser-tuning-info")
}

/// Asks for the keyboard focus to go to the section's name (Navigate ▸
/// Tuning).
pub fn request_focus(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(request_id(), true));
}

/// The Info View's content for the tuning's name, if the pointer is over it
/// (R26).
pub(crate) fn take_info(ctx: &egui::Context) -> Option<InfoViewContent> {
    ctx.data_mut(|d| d.remove_temp::<InfoViewContent>(info_id()))
}

/// How many rows of [`TUNING_ROW`] points the section has: none while it
/// hides, the name alone while the set plays twelve equal steps, and with a
/// tuning the name, the five controls under an open triangle, and the
/// buttons.
#[must_use]
pub fn rows(shell: &Shell, open: bool) -> u8 {
    if !shell.tuning_section_shown() {
        0
    } else if shell.editor().document().settings().tuning.is_none() {
        1
    } else if open {
        7
    } else {
        2
    }
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// Draws the section in `rect` and adds what the person did to `events`.
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    open: &mut bool,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, c.panel.0);
    painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, c.control.0));
    let tuning = shell.editor().document().settings().tuning.as_ref();
    let row = |i: usize| {
        Rect::from_min_size(
            pos2(rect.left(), rect.top() + i as f32 * TUNING_ROW),
            vec2(rect.width(), TUNING_ROW),
        )
        .shrink2(vec2(4.0, 1.0))
    };

    // The name row: the triangle, then the name.
    let name_row = row(0);
    let triangle = Rect::from_min_size(name_row.min, vec2(14.0, name_row.height()));
    let name = Rect::from_min_max(pos2(triangle.right() + 2.0, name_row.top()), name_row.max);
    if tuning.is_some() {
        let response = ui.interact(triangle, triangle_id(), Sense::click());
        if response.clicked() {
            *open = !*open;
        }
        let a = triangle.center();
        let corners = if *open {
            [
                a + vec2(-3.5, -2.0),
                a + vec2(3.5, -2.0),
                a + vec2(0.0, 3.0),
            ]
        } else {
            [
                a + vec2(-2.0, -3.5),
                a + vec2(-2.0, 3.5),
                a + vec2(3.0, 0.0),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            corners.to_vec(),
            c.text.0,
            Stroke::NONE,
        ));
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, *open, "Tuning Controls")
        });
        crate::focus::register(ui.ctx(), triangle_id(), Area::Browser, None);
    }
    let response = ui.interact(name, name_id(), Sense::click());
    crate::focus::register(ui.ctx(), name_id(), Area::Browser, None);
    if ui.ctx().data_mut(|d| d.remove_temp::<bool>(request_id())) == Some(true) {
        response.request_focus();
    }
    if response.clicked() {
        response.request_focus();
    }
    let text = tuning.map_or("Twelve equal steps", |t| t.name.as_str());
    painter.rect_filled(
        name,
        theme.sizes.corner,
        if response.has_focus() || response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    painter.with_clip_rect(name).text(
        pos2(name.left() + 6.0, name.center().y),
        Align2::LEFT_CENTER,
        text,
        theme.font(),
        if tuning.is_some() {
            c.text.0
        } else {
            c.text_dim.0
        },
    );
    if response.has_focus() && crate::focus::outlined(ui.ctx()) {
        painter.rect_stroke(
            name,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Tuning"));
    ui.ctx()
        .accesskit_node_builder(name_id(), |node| node.set_value(text.to_owned()));
    if let Some(tuning) = tuning {
        if response.hovered() {
            let body = bl_shell::browser::tunings::body(tuning);
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    info_id(),
                    InfoViewContent {
                        title: tuning.name.clone(),
                        body,
                        shortcut: None,
                    },
                );
            });
        }
        // Delete on the name removes the tuning (R28).
        if response.has_focus() && ui.input(|i| i.key_pressed(Key::Delete)) {
            invoke(events, "tuning.remove", json!({}));
        }
        if *open {
            controls(ui, theme, tuning, &row, &mut |id, args| {
                invoke(events, id, args)
            });
        }
        buttons(ui, theme, &painter, tuning, row(if *open { 6 } else { 1 }));
    }
    drop_target(ui, theme, rect, events);
}

/// The five controls under the triangle, each running R7's or R8's command.
fn controls(
    ui: &mut Ui,
    theme: &Theme,
    tuning: &Tuning,
    row: &dyn Fn(usize) -> Rect,
    run: &mut dyn FnMut(&str, serde_json::Value),
) {
    let degrees = tuning.degrees();
    let reference = tuning.reference;
    let labelled = |ui: &Ui, i: usize, label: &str| {
        let area = row(i);
        ui.painter_at(area).text(
            pos2(area.left() + 2.0, area.center().y),
            Align2::LEFT_CENTER,
            label,
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        Rect::from_min_max(pos2(area.left() + 72.0, area.top()), area.max)
    };

    let area = labelled(ui, 1, "Octave");
    if let Some(octave) = number_box(
        ui,
        theme,
        "tuning-octave",
        "Octave",
        area,
        f64::from(reference.octave),
        (-1000.0, 1000.0),
        &|v, n| v + f64::from(n),
        &reference.octave.to_string(),
    ) {
        run("tuning.set_reference", json!({ "octave": octave as i32 }));
    }

    let area = labelled(ui, 2, "Note");
    let choices: Vec<Choice<u32>> = (0..degrees as u32)
        .map(|degree| Choice {
            label: tuning.degree_name(degree),
            value: degree,
            leds: NO_LEDS,
        })
        .collect();
    if let Some(degree) = chooser(
        ui,
        theme,
        "tuning-note",
        "Note",
        area,
        &choices,
        Some(&reference.degree),
        true,
    ) {
        run("tuning.set_reference", json!({ "degree": degree }));
    }

    let area = labelled(ui, 3, "Ref. Pitch");
    if let Some(frequency) = number_box(
        ui,
        theme,
        "tuning-ref-pitch",
        "Ref. Pitch",
        area,
        reference.frequency,
        (1.0, 100_000.0),
        &|v, n| (v + f64::from(n) * 0.1).max(1.0),
        &format!("{:.3} Hz", reference.frequency),
    ) {
        run("tuning.set_reference", json!({ "frequency": frequency }));
    }

    for (i, label, note, key) in [
        (4, "Lowest Note", tuning.range.lowest, "lowest"),
        (5, "Highest Note", tuning.range.highest, "highest"),
    ] {
        let area = labelled(ui, i, label);
        let index = note.index(degrees);
        if let Some(next) = number_box(
            ui,
            theme,
            &format!("tuning-{key}"),
            label,
            area,
            index as f64,
            (-1.0e6, 1.0e6),
            &|v, n| v + f64::from(n),
            &tuning.note_name(note),
        ) && let Some(TuningNote { octave, degree }) =
            TuningNote::from_index(next as i64, degrees)
        {
            run(
                "tuning.set_range",
                json!({ key: { "octave": octave, "degree": degree } }),
            );
        }
    }
}

/// The save and link buttons (R28). Save waits for the ASCL writer; the link
/// opens the tuning's link in the system's web browser and is unavailable
/// where the tuning has none, or one that isn't a web address.
fn buttons(ui: &mut Ui, theme: &Theme, painter: &Painter, tuning: &Tuning, area: Rect) {
    let half = (area.width() - 4.0) / 2.0;
    let save = Rect::from_min_size(area.min, vec2(half, area.height()));
    let link = Rect::from_min_size(
        pos2(save.right() + 4.0, area.top()),
        vec2(half, area.height()),
    );
    // Spec: tunings R10. `tuning.save` is not built yet, so the button is
    // drawn, and unavailable.
    button(
        ui,
        theme,
        painter,
        "tuning-save",
        "Save Tuning",
        "Save",
        save,
        false,
    );
    let url = tuning
        .link
        .as_deref()
        .filter(|l| l.starts_with("https://") || l.starts_with("http://"));
    if button(
        ui,
        theme,
        painter,
        "tuning-link",
        "Open Tuning Link",
        "Link",
        link,
        url.is_some(),
    )
    .clicked()
        && let Some(url) = url
    {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

#[expect(clippy::too_many_arguments, reason = "a button has this many parts")]
fn button(
    ui: &mut Ui,
    theme: &Theme,
    painter: &Painter,
    salt: &str,
    name: &str,
    text: &str,
    rect: Rect,
    enabled: bool,
) -> Response {
    let c = &theme.colors;
    let id = Id::new(("tuning-button", salt));
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let face = if response.hovered() && enabled {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if enabled {
            face
        } else {
            face.gamma_multiply(0.55)
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        if enabled { c.text.0 } else { c.pending.0 },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
    });
    crate::focus::register(ui.ctx(), id, Area::Browser, None);
    response
}

/// Loads a tuning file dropped on the section, from the browser's list or
/// from the operating system (R27; I-DROP-013).
fn drop_target(ui: &mut Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
    let ctx = ui.ctx().clone();
    let payload = DragAndDrop::payload::<TuningDrag>(&ctx);
    let (dropped, pointer, released) = ctx.input(|i| {
        let dropped: Vec<String> = i
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_string_lossy().into_owned())
            .filter(|path| {
                bl_shell::browser::listing::path_kind(std::path::Path::new(path))
                    == Some(bl_shell::browser_commands::ItemKind::Tuning)
            })
            .collect();
        (
            dropped,
            i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()),
            i.pointer.any_released(),
        )
    });
    let over = pointer.is_some_and(|p| rect.contains(p));
    if payload.is_some() && over {
        ui.painter_at(rect).rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(2.0, theme.colors.accent.0),
            StrokeKind::Inside,
        );
    }
    if over && let Some(path) = dropped.first() {
        invoke(events, "tuning.load", json!({ "path": path }));
    }
    if released && let Some(drag) = payload {
        DragAndDrop::clear_payload(&ctx);
        if over {
            invoke(events, "tuning.load", json!({ "path": drag.path }));
        }
    }
}
