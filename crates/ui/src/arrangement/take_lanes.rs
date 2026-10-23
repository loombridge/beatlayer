//! Take lanes in the Arrangement (WP-6.22; the comping spec, R23 to R30):
//! each shown lane's header in the header column (its name, then its
//! Audition Take Lane button), the Show/Hide Take Lanes toggle in the main
//! lane's header, the header's menu, and the name typed in a header.
//!
//! The lanes' rows and clips are drawn with the track lanes
//! ([`super::scene`]). Everything here runs the commands the keys and agents
//! run (`take_lane.*`, `arrangement.select_take_lanes`,
//! `arrangement.show_take_lanes`).

use bl_commands::Context;
use bl_model::{TakeLaneId, TrackId};
use bl_shell::arrangement_commands as ids;
use bl_shell::context_menus;
use bl_shell::menus::shown;
use bl_shell::{Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, EventFilter, Id, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::keymap::{Keymap, Platform};
use crate::layout::header_columns;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How far a lane's name is indented from the title's left edge, under the
/// fold button's width.
const INDENT: f32 = 16.0;

/// How wide the Audition Take Lane button and the Show/Hide Take Lanes
/// toggle are, in points.
pub const BUTTON: f32 = 14.0;

/// The ID of a take lane header's title.
#[must_use]
pub fn title_id(lane: TakeLaneId) -> Id {
    Id::new(("arrangement-take-lane", lane))
}

/// The ID of a take lane's Audition Take Lane button.
#[must_use]
pub fn audition_id(lane: TakeLaneId) -> Id {
    Id::new(("arrangement-take-lane-audition", lane))
}

/// The ID of a track's Show/Hide Take Lanes toggle.
#[must_use]
pub fn toggle_id(track: TrackId) -> Id {
    Id::new(("arrangement-take-lanes-toggle", track))
}

/// The ID of a take lane row's node in the accessibility tree.
#[must_use]
pub fn row_id(lane: TakeLaneId) -> Id {
    Id::new(("arrangement-take-lane-row", lane))
}

/// A name that Tab can move to: a track's or a take lane's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    /// A track's title.
    Track(TrackId),
    /// A take lane's header.
    Lane(TakeLaneId),
}

/// The menu a take lane's header opens (C-TAKE).
#[derive(Debug)]
pub struct TakeLaneMenus {
    platform: Platform,
}

impl Default for TakeLaneMenus {
    fn default() -> Self {
        Self {
            platform: Platform::current(),
        }
    }
}

impl TakeLaneMenus {
    /// Draws the open menu's items. They act on the take lanes selected,
    /// whatever has the keyboard.
    fn show(&self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        // The keymap in use, so the menu shows the keys in use.
        let keymap = Keymap::from_data(shell.keymap().clone());
        let contexts = [Context::TakeLanes, Context::Arrangement, Context::Global];
        let menu = context_menus::take_lane();
        for item in shown(&menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            let response =
                crate::menus::item_in(ui, item, shell, &keymap, self.platform, &contexts, events);
            crate::menus::take_focus_first(ui, &response);
        }
    }
}

/// One take lane's header to draw.
pub struct LaneHeader<'a> {
    /// The shell.
    pub shell: &'a Shell,
    /// The lane's track.
    pub track: TrackId,
    /// The lane.
    pub lane: TakeLaneId,
    /// Its name.
    pub name: &'a str,
    /// The header's rectangle: the whole column for this row.
    pub rect: Rect,
    /// The track's colour.
    pub colour: Color32,
    /// Whether the header is selected.
    pub selected: bool,
    /// Whether the lane is auditioned.
    pub auditioned: bool,
    /// Whether the header shows beside the In/Out column.
    pub show_in_out: bool,
    /// Whether the keys open the menu for this header now.
    pub keys: bool,
    /// The take lanes in the order the headers are drawn, for Shift-click
    /// to take in those between.
    pub order: &'a [TakeLaneId],
    /// The names Tab moves between while one is typed.
    pub names: &'a [Named],
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// A speaker, lit while the lane is auditioned.
fn speaker(painter: &egui::Painter, rect: Rect, ink: Color32, lit: bool) {
    let c = rect.center();
    painter.add(Shape::convex_polygon(
        vec![
            pos2(c.x - 4.5, c.y - 2.0),
            pos2(c.x - 2.0, c.y - 2.0),
            pos2(c.x + 1.0, c.y - 4.5),
            pos2(c.x + 1.0, c.y + 4.5),
            pos2(c.x - 2.0, c.y + 2.0),
            pos2(c.x - 4.5, c.y + 2.0),
        ],
        ink,
        Stroke::NONE,
    ));
    if lit {
        for radius in [3.0, 5.0] {
            let points: Vec<Pos2> = (-4..=4)
                .map(|i| {
                    let angle = i as f32 * 0.17;
                    pos2(c.x + 1.5 + radius * angle.cos(), c.y + radius * angle.sin())
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(1.0, ink)));
        }
    }
}

/// Draws one take lane's header and adds what the person did to `events`.
pub fn header(
    ui: &mut Ui,
    theme: &Theme,
    menus: &TakeLaneMenus,
    h: &LaneHeader<'_>,
    events: &mut Vec<UiEvent>,
) {
    let LaneHeader {
        shell,
        track,
        lane,
        name,
        rect,
        colour,
        selected,
        auditioned,
        show_in_out,
        keys,
        order,
        names,
    } = *h;
    let colors = &theme.colors;
    let doc = shell.editor().document();
    let painter = ui.painter_at(rect);
    let title = Rect::from_min_max(
        pos2(rect.left() + header_columns::TITLE.0, rect.top()),
        pos2(
            rect.left() + header_columns::title_width(show_in_out),
            rect.bottom() - 1.0,
        ),
    );
    painter.rect_filled(
        title,
        0.0,
        colour.gamma_multiply(if selected { 0.85 } else { 0.5 }),
    );
    if selected {
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
    let response = ui.interact(title, title_id(lane), Sense::click());
    crate::focus::register(ui.ctx(), response.id, bl_shell::Area::MainView, None);

    // The Audition Take Lane button, after the name (L-ARR-020).
    let button = Rect::from_min_size(
        pos2(title.right() - BUTTON - 3.0, title.top() + 2.0),
        vec2(BUTTON, BUTTON),
    );
    let show_button = rect.height() >= BUTTON + 4.0;
    // The name, or the name being typed.
    let name_rect = Rect::from_min_max(
        pos2(title.left() + INDENT, title.top() + 1.0),
        pos2(
            if show_button {
                button.left() - 2.0
            } else {
                title.right() - 2.0
            },
            title.top() + 15.0,
        ),
    );
    if rect.height() >= 12.0 && !rename_edit(ui, theme, name_rect, lane, name, names, events) {
        painter.with_clip_rect(name_rect).text(
            pos2(name_rect.left(), name_rect.center().y),
            Align2::LEFT_CENTER,
            name,
            theme.font(),
            colors.clip_text.0,
        );
    }

    if response.clicked() {
        let modifiers = ui.input(|i| i.modifiers);
        let view = shell.arrangement();
        if modifiers.shift {
            // Shift-click takes in the lanes between the last selected and
            // this one.
            let at = |id: &TakeLaneId| order.iter().position(|held| held == id);
            let anchor = view.selected_take_lanes().last().and_then(at).or(at(&lane));
            if let (Some(a), Some(b)) = (anchor, at(&lane)) {
                let (a, b) = (a.min(b), a.max(b));
                invoke(
                    events,
                    ids::SELECT_TAKE_LANES,
                    json!({ "lanes": &order[a..=b] }),
                );
            }
        } else {
            invoke(
                events,
                ids::SELECT_TAKE_LANES,
                json!({ "lanes": [lane], "toggle": modifiers.command }),
            );
        }
    }
    // Right-clicking selects the header unless it's selected already
    // (R29), then opens the lane's menu.
    if response.secondary_clicked() && !selected {
        invoke(events, ids::SELECT_TAKE_LANES, json!({ "lanes": [lane] }));
    }
    response.context_menu(|ui| menus.show(ui, shell, events));
    if keys && crate::menus::context_menu_keys(ui) {
        crate::menus::open_keyboard_context_menu(ui.ctx(), response.id, title.left_bottom());
    }
    crate::menus::keyboard_context_menu(&response, |ui| menus.show(ui, shell, events));

    let track_name = doc.track(track).map_or("", |t| t.name());
    let label = format!("{track_name}, take lane {name}");
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, label.as_str())
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::RowHeader);
        node.set_selected(selected);
        node.set_description(format!(
            "Take lane of {track_name}{}",
            if auditioned { ", auditioned" } else { "" }
        ));
    });

    if show_button {
        let audition = ui.interact(button, audition_id(lane), Sense::click());
        let fill = if auditioned {
            colors.accent.0
        } else if audition.hovered() {
            colors.control_hover.0
        } else {
            colors.control.0
        };
        let ink = if auditioned {
            colors.accent_text.0
        } else {
            colors.text.0
        };
        painter.rect_filled(button, theme.sizes.corner, fill);
        speaker(&painter, button, ink, auditioned);
        if audition.clicked() {
            // I-CLICK-040.
            invoke(
                events,
                "take_lane.set_audition",
                json!({ "lane": lane, "on": !auditioned }),
            );
        }
        crate::focus::register(ui.ctx(), audition.id, bl_shell::Area::MainView, None);
        let switch = format!("Audition Take Lane {name}");
        audition.widget_info(|| {
            WidgetInfo::selected(WidgetType::Checkbox, true, auditioned, switch.as_str())
        });
        ui.ctx().accesskit_node_builder(audition.id, |node| {
            node.set_role(Role::Switch);
            node.set_label(switch.clone());
            node.set_toggled(if auditioned {
                egui::accesskit::Toggled::True
            } else {
                egui::accesskit::Toggled::False
            });
        });
    }
}

/// Draws a track's Show/Hide Take Lanes toggle in its main lane's header
/// (L-ARR-021, I-CLICK-043) at `rect`.
pub fn toggle(
    ui: &mut Ui,
    theme: &Theme,
    track: TrackId,
    rect: Rect,
    shown: bool,
    events: &mut Vec<UiEvent>,
) {
    let colors = &theme.colors;
    let response = ui.interact(rect, toggle_id(track), Sense::click());
    let fill = if shown {
        colors.accent.0
    } else if response.hovered() {
        colors.control_hover.0
    } else {
        colors.control.0
    };
    let ink = if shown {
        colors.accent_text.0
    } else {
        colors.text.0
    };
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, theme.sizes.corner, fill);
    // Three lines, the lanes.
    for i in 0..3 {
        let y = rect.top() + 3.5 + 3.5 * i as f32;
        painter.line_segment(
            [pos2(rect.left() + 3.0, y), pos2(rect.right() - 3.0, y)],
            Stroke::new(1.0, ink),
        );
    }
    if response.clicked() {
        invoke(
            events,
            ids::SHOW_TAKE_LANES,
            json!({ "tracks": [track], "on": !shown }),
        );
    }
    crate::focus::register(ui.ctx(), response.id, bl_shell::Area::MainView, None);
    let label = if shown {
        "Hide Take Lanes"
    } else {
        "Show Take Lanes"
    };
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, shown, label));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Switch);
        node.set_label(label.to_owned());
        node.set_toggled(if shown {
            egui::accesskit::Toggled::True
        } else {
            egui::accesskit::Toggled::False
        });
    });
}

/// A name being typed in a take lane's header.
#[derive(Clone, Debug)]
struct Renaming {
    lane: TakeLaneId,
    /// The text, once it has been taken from the lane's name.
    text: Option<String>,
    focused: bool,
    /// The pass that last drew it, so a header that isn't drawn doesn't keep
    /// the keys.
    drawn: Option<u64>,
}

fn renaming_id() -> Id {
    Id::new("take-lane-renaming")
}

fn renaming(ctx: &egui::Context) -> Option<Renaming> {
    ctx.data(|d| d.get_temp::<Option<Renaming>>(renaming_id()))
        .flatten()
}

/// Starts typing a new name for `lane` in its header (Rename, Cmd+R,
/// S-EDIT-008).
pub fn start_rename(ctx: &egui::Context, lane: TakeLaneId) {
    ctx.data_mut(|d| {
        d.insert_temp(
            renaming_id(),
            Some(Renaming {
                lane,
                text: None,
                focused: false,
                drawn: None,
            }),
        );
    });
}

/// Whether a take lane's name is being typed, so keys go to it. Called after
/// the pass has drawn: a name whose header wasn't drawn is dropped, as Esc
/// would.
pub fn is_renaming(ctx: &egui::Context) -> bool {
    let Some(renaming) = renaming(ctx) else {
        return false;
    };
    if renaming.drawn == Some(ctx.cumulative_pass_nr()) {
        return true;
    }
    ctx.data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
    false
}

/// Draws the name being typed over `rect` if it's `lane`'s, and says so.
/// Return keeps it, Esc leaves the name, and Tab and Shift+Tab keep it and
/// move to the next or previous lane or track's name (R28).
fn rename_edit(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    lane: TakeLaneId,
    name: &str,
    names: &[Named],
    events: &mut Vec<UiEvent>,
) -> bool {
    let Some(mut renaming) = renaming(ui.ctx()).filter(|r| r.lane == lane) else {
        return false;
    };
    renaming.drawn = Some(ui.ctx().cumulative_pass_nr());
    let text = renaming.text.get_or_insert_with(|| name.to_owned());
    let id = Id::new(("take-lane-rename-edit", lane));
    let edit = ui.put(
        rect,
        egui::TextEdit::singleline(text)
            .id(id)
            .font(theme.small_font())
            .margin(vec2(2.0, 0.0)),
    );
    if !renaming.focused {
        edit.request_focus();
        renaming.focused = true;
    }
    // Tab belongs to the field while it has the keyboard.
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            EventFilter {
                tab: true,
                ..EventFilter::default()
            },
        );
    });
    let (enter, escape, tab, shift_tab) = ui.input(|i| {
        let tab = i.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. }
                    if !modifiers.shift
            )
        });
        let shift_tab = i.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. }
                    if modifiers.shift
            )
        });
        (
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
            tab,
            shift_tab,
        )
    });
    let moving = tab || shift_tab;
    // The keys that end the name are the field's: no other header's name, and
    // not the keys the Arrangement reads, see them as well.
    ui.input_mut(|i| {
        for (modifiers, key, used) in [
            (egui::Modifiers::NONE, Key::Enter, enter),
            (egui::Modifiers::NONE, Key::Escape, escape),
            (egui::Modifiers::NONE, Key::Tab, tab),
            (egui::Modifiers::SHIFT, Key::Tab, shift_tab),
        ] {
            if used {
                i.consume_key(modifiers, key);
            }
        }
    });
    if edit.lost_focus() || enter || escape || moving {
        if (enter || moving || (edit.lost_focus() && !escape)) && !text.is_empty() && text != name {
            invoke(
                events,
                "take_lane.rename",
                json!({ "lane": lane, "name": text }),
            );
        }
        ui.ctx()
            .data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
        if moving && let Some(at) = names.iter().position(|n| *n == Named::Lane(lane)) {
            let next = if tab {
                names.get(at + 1)
            } else {
                at.checked_sub(1).and_then(|before| names.get(before))
            };
            match next {
                Some(Named::Lane(next)) => start_rename(ui.ctx(), *next),
                Some(Named::Track(next)) => crate::track_menu::start_rename(ui.ctx(), *next),
                None => {}
            }
        }
    } else {
        ui.ctx()
            .data_mut(|d| d.insert_temp(renaming_id(), Some(renaming)));
    }
    true
}
