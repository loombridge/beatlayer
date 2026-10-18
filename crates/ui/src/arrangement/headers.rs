//! The Arrangement's track headers (L-ARR-006 to 009): each track's title
//! in its colour with the fold button and name, then the In/Out and mixer
//! columns and the meter (arrangement-view R3, R14). The mixer column's
//! activator, Solo, Arm, volume, and pan, and the meter, are the Mixer
//! view's controls (WP-1.9; the mixer view spec, R4), so they work as a
//! strip's do. The In/Out column's choosers and Monitor switch and the
//! sends are the mixer's too (WP-2.19; the routing spec).

use bl_model::{Track, TrackId, TrackKind};
use bl_shell::Invocation;
use bl_shell::Shell;
use bl_shell::arrangement_commands as ids;
use bl_shell::mixer_controls::Switch;
use egui::accesskit::Role;
use egui::{
    Align2, Color32, CursorIcon, Id, Rect, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::layout::header_columns::{IN_OUT, METER, MIXER, TITLE};
use crate::main_window::UiEvent;
use crate::mixer::{Place, StripParts};
use crate::theme::Theme;
use crate::track_menu::{TrackMenus, rename_edit};

/// One header to draw.
pub struct Header<'a> {
    /// The shell, whose mixer commands the mixer column runs.
    pub shell: &'a Shell,
    /// The track.
    pub track: &'a Track,
    /// The header's rectangle: the whole column for this row.
    pub rect: Rect,
    /// What its activator shows: the track's number, counting from 1, or a
    /// return track's letter (L-MIX-003).
    pub activator: String,
    /// Its colour.
    pub colour: Color32,
    /// Whether it's folded.
    pub folded: bool,
    /// Whether it's selected.
    pub selected: bool,
    /// Whether its title bar is selected.
    pub title_selected: bool,
    /// Whether it can fold (the pinned Main track can't).
    pub foldable: bool,
    /// Whether its height can be dragged.
    pub resizable: bool,
    /// The menu its title bar opens.
    pub menus: &'a TrackMenus,
    /// Whether the keys open that menu for this title bar now (S-CTX-002).
    pub keys: bool,
}

/// The ID of a track's title, for tests and focus.
#[must_use]
pub fn title_id(track: TrackId) -> Id {
    Id::new(("arrangement-title", track))
}

/// The ID of a track's fold button.
#[must_use]
pub fn fold_id(track: TrackId) -> Id {
    Id::new(("arrangement-fold", track))
}

/// The ID of the split line under a track's header, dragged to resize it.
#[must_use]
pub fn split_id(track: TrackId) -> Id {
    Id::new(("arrangement-split", track))
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// The volume as the mixer column shows it, in dB.
#[must_use]
pub fn volume_text(volume: f64) -> String {
    bl_shell::mixer_controls::volume_text(volume)
}

/// The pan as the mixer column shows it: C, or a number and L or R.
#[must_use]
pub fn pan_text(pan: f64) -> String {
    bl_shell::mixer_controls::pan_text(pan)
}

/// Draws one track's header and adds what the person did to `events`.
#[expect(clippy::too_many_lines, reason = "the header's columns, left to right")]
pub fn header(
    ui: &mut Ui,
    theme: &Theme,
    header: &Header<'_>,
    every_track: &[TrackId],
    events: &mut Vec<UiEvent>,
) {
    let Header {
        shell,
        track,
        rect,
        ref activator,
        colour,
        folded,
        selected,
        title_selected,
        foldable,
        resizable,
        menus,
        keys,
    } = *header;
    let id = track.id();
    let colors = &theme.colors;
    let left = rect.left();
    let column = |(x0, x1): (f32, f32), top: f32, bottom: f32| {
        Rect::from_min_max(pos2(left + x0, top), pos2(left + x1, bottom))
    };
    let height = rect.height();
    let title = column(TITLE, rect.top(), rect.bottom() - 1.0);
    let painter = ui.painter_at(rect);

    // The title, filled with the track's colour.
    let fill = if selected {
        colour
    } else {
        colour.gamma_multiply(0.82)
    };
    painter.rect_filled(title, 0.0, fill);
    if title_selected {
        painter.rect_stroke(
            title.shrink(0.5),
            0.0,
            Stroke::new(1.0, colors.focus.0),
            StrokeKind::Inside,
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, colors.lane_line.0),
    );
    let name = track.name();
    let title_response = ui.interact(title, title_id(id), Sense::click());
    crate::rack_view::focus_track_under_drag(ui, shell, id, title, events);
    // The title marks the track as drawn for the keyboard's neighbour moves
    // (the accessibility spec, R4); Tab doesn't stop on it.
    crate::focus::register(
        ui.ctx(),
        title_response.id,
        bl_shell::Area::MainView,
        Some(crate::focus::TrackControl {
            place: Place::Header,
            track: id,
            slot: crate::focus::Slot::Title,
        }),
    );
    if title_response.clicked() {
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        invoke(
            events,
            ids::SELECT_TRACKS,
            json!({ "tracks": [id], "add": add, "title_bars": true }),
        );
    }
    // WP-4.22: right-clicking the title, or the keys on it, open C-TRACK.
    menus.title(
        ui,
        theme,
        &title_response,
        id,
        (
            title.left_bottom(),
            keys || title_response.has_focus(),
            true,
        ),
        shell,
        events,
    );
    title_response
        .widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, name));
    ui.ctx().accesskit_node_builder(title_response.id, |node| {
        node.set_role(Role::RowHeader);
        node.set_description(format!(
            "{} track {activator}{}",
            kind_word(track.kind()),
            if folded { ", folded" } else { "" }
        ));
    });

    if height < 12.0 {
        return;
    }
    let row_top = rect.top() + 1.0;
    // The fold button, then the name.
    let mut name_left = title.left() + 4.0;
    if foldable {
        let fold = Rect::from_min_size(pos2(title.left() + 3.0, row_top + 2.0), vec2(11.0, 11.0));
        let response = ui.interact(fold, fold_id(id), Sense::click());
        let ink = colors.clip_text.0;
        let c = fold.center();
        let shape = if folded {
            Shape::convex_polygon(
                vec![
                    pos2(c.x - 2.5, c.y - 4.0),
                    pos2(c.x + 3.5, c.y),
                    pos2(c.x - 2.5, c.y + 4.0),
                ],
                ink,
                Stroke::NONE,
            )
        } else {
            Shape::convex_polygon(
                vec![
                    pos2(c.x - 4.0, c.y - 2.5),
                    pos2(c.x + 4.0, c.y - 2.5),
                    pos2(c.x, c.y + 3.5),
                ],
                ink,
                Stroke::NONE,
            )
        };
        painter.add(shape);
        if response.clicked() {
            let all = ui.input(|i| i.modifiers.alt);
            let tracks: Vec<TrackId> = if all { every_track.to_vec() } else { vec![id] };
            invoke(
                events,
                ids::FOLD_TRACKS,
                json!({ "tracks": tracks, "fold": !folded }),
            );
        }
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, folded, "Fold"));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_description(format!(
                "Folds {name} to its title row; with Option (Alt), every track"
            ));
        });
        name_left = fold.right() + 3.0;
    }
    let name_rect = Rect::from_min_max(
        pos2(name_left, row_top),
        pos2(title.right() - 1.0, row_top + 15.0),
    );
    if !rename_edit(ui, theme, name_rect, id, name, events) {
        painter.with_clip_rect(title.shrink(1.0)).text(
            pos2(name_left, row_top + 7.5),
            Align2::LEFT_CENTER,
            name,
            theme.font(),
            colors.clip_text.0,
        );
    }

    // The split line under the header: drag to resize (I-DRAG-018).
    if resizable && !folded {
        let split = Rect::from_min_max(
            pos2(title.left(), rect.bottom() - 3.0),
            pos2(rect.right(), rect.bottom() + 1.0),
        );
        let response = ui
            .interact(split, split_id(id), Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeVertical);
        if response.drag_started() {
            // The height when the drag began; the drag adds to it.
            ui.data_mut(|d| d.insert_temp(split_id(id), height));
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let all = ui.input(|i| i.modifiers.alt);
            let start = ui
                .data(|d| d.get_temp::<f32>(split_id(id)))
                .unwrap_or(height);
            let new_height = f64::from(start + pointer.y - press.y);
            let tracks: Vec<TrackId> = if all { every_track.to_vec() } else { vec![id] };
            invoke(
                events,
                ids::SET_TRACK_HEIGHT,
                json!({ "tracks": tracks, "height": new_height }),
            );
        }
    }

    // The mixer column's first row: activator, Solo, Arm (L-ARR-009), live
    // since WP-1.9.
    let parts = StripParts {
        shell,
        theme,
        place: Place::Header,
        track: id,
        activator,
    };
    let mixer = column(MIXER, row_top, row_top + 14.0);
    let third = mixer.width() / 3.0;
    let cell = |i: f32| {
        Rect::from_min_max(
            pos2(mixer.left() + third * i + 1.0, mixer.top()),
            pos2(mixer.left() + third * (i + 1.0) - 1.0, mixer.bottom()),
        )
    };
    for (i, switch) in [Switch::Activator, Switch::Solo, Switch::Arm]
        .into_iter()
        .enumerate()
    {
        parts.switch(ui, switch, cell(i as f32), events);
    }

    // The meter (L-ARR-006).
    parts.meter(ui, column(METER, row_top, rect.bottom() - 2.0));

    if height < 36.0 || folded {
        return;
    }
    // Second row: volume and pan; third: the sends (L-ARR-009).
    let row2 = column(MIXER, row_top + 16.0, row_top + 30.0);
    let half = row2.width() / 2.0;
    let left_half = Rect::from_min_max(row2.min, pos2(row2.left() + half - 1.0, row2.bottom()));
    let right_half = Rect::from_min_max(pos2(row2.left() + half + 1.0, row2.top()), row2.max);
    parts.volume_field(ui, left_half, events);
    parts.pan(ui, right_half, None, events);
    let midi_only = track.kind() == TrackKind::Midi && track.devices().len() == 0;
    if height >= 52.0 && track.kind().has_sends() && !midi_only {
        // The sends, one per return, side by side (L-ARR-009; with more
        // returns than fit, the first that do).
        let row3 = column(MIXER, row_top + 32.0, row_top + 46.0);
        let doc = shell.editor().document();
        let returns: Vec<TrackId> = doc.returns().collect();
        let width = (row3.width() / returns.len().max(2) as f32).max(14.0);
        for (index, &to) in returns.iter().enumerate() {
            let left = row3.left() + width * index as f32;
            if left + width > row3.right() + 0.5 {
                break;
            }
            let cell = Rect::from_min_max(
                pos2(left + 1.0, row3.top()),
                pos2(left + width - 1.0, row3.bottom()),
            );
            parts.send(ui, cell, to, index, events);
        }
    }

    // The In/Out column (L-ARR-008), as many rows as fit.
    if let Some(section) =
        bl_shell::routing::in_out(shell.editor().document(), id, &shell.interface())
    {
        let in_out = crate::in_out::InOutParts {
            theme,
            place: Place::Header,
            track: id,
        };
        let bottom = rect.bottom() - 2.0;
        in_out.draw(
            ui,
            &section,
            false,
            |i| {
                let top = row_top + 16.0 * i as f32;
                (top + 14.0 <= bottom).then(|| column(IN_OUT, top, top + 14.0))
            },
            events,
        );
    }
}

/// The word for a track's kind, for assistive technology.
fn kind_word(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Audio => "Audio",
        TrackKind::Midi => "MIDI",
        TrackKind::Group => "Group",
        TrackKind::Return => "Return",
        TrackKind::Main => "Main",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_values_read_as_live_shows_them() {
        assert_eq!(volume_text(1.0), "0.0");
        assert_eq!(volume_text(0.5), "-6.0");
        assert_eq!(volume_text(0.0), "-inf");
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(pan_text(-0.5), "25L");
        assert_eq!(pan_text(1.0), "50R");
    }
}
