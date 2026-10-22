//! The menu a track's title bar opens (WP-4.22; menus.md C-TRACK) in the
//! Arrangement's headers, the Session View and the mixers, and the name
//! typed in a title bar by Rename.
//!
//! A right-click selects the title bar first unless it's selected already,
//! so several selected tracks keep their selection and every item acts on
//! all of them as its command does. The keys (S-CTX-002) open it for the
//! title bar that has the keyboard. Its items run in the `track-headers`
//! context whichever view they're in.

use bl_model::TrackId;
use bl_shell::arrangement_commands::SELECT_TRACKS;
use bl_shell::context_menus::{self, TrackMenu};
use bl_shell::menus::shown;
use bl_shell::{Invocation, MainView, Shell};
use egui::accesskit::Role;
use egui::{Color32, Context, Id, Key, Pos2, Rect, Response, Sense, Ui, WidgetInfo, WidgetType};
use serde_json::json;

use crate::keymap::{Keymap, Platform};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The colour palette's swatches: 5 rows of 14 (C-TRACK-015).
const PALETTE: (usize, usize) = (5, 14);

/// The keymap the track menus' keys come from.
#[derive(Debug)]
pub struct TrackMenus {
    platform: Platform,
}

impl Default for TrackMenus {
    fn default() -> Self {
        Self {
            platform: Platform::current(),
        }
    }
}

impl TrackMenus {
    /// Gives a track's title bar its context menu. `title` is the bar's
    /// response, `at` where the keyboard opens the menu, `focused` whether
    /// the bar (or the column that stands for it) has the keyboard, and
    /// `arrangement` whether it's a header of the Arrangement.
    #[expect(clippy::too_many_arguments, reason = "the title bar's inputs")]
    pub fn title(
        &self,
        ui: &Ui,
        theme: &Theme,
        title: &Response,
        track: TrackId,
        (at, focused, arrangement): (Pos2, bool, bool),
        shell: &Shell,
        events: &mut Vec<UiEvent>,
    ) {
        if title.secondary_clicked() {
            select(shell, track, events);
        }
        title.context_menu(|ui| self.show(ui, theme, shell, track, arrangement, events));
        if focused && crate::menus::context_menu_keys(ui) {
            select(shell, track, events);
            crate::menus::open_keyboard_context_menu(ui.ctx(), title.id, at);
        }
        crate::menus::keyboard_context_menu(title, |ui| {
            self.show(ui, theme, shell, track, arrangement, events);
        });
    }

    /// Draws the open menu's items.
    fn show(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        track: TrackId,
        arrangement: bool,
        events: &mut Vec<UiEvent>,
    ) {
        // The keymap in use, so the menu shows the keys in use (R10).
        let keymap = Keymap::from_data(shell.keymap().clone());
        let view = shell.arrangement();
        let doc = shell.editor().document();
        let in_arrangement = arrangement && shell.main_view() == MainView::Arrangement;
        let menu = context_menus::track(TrackMenu {
            group: doc
                .track(track)
                .is_some_and(|t| t.kind() == bl_model::TrackKind::Group),
            several: view.selected_tracks().len() > 1,
            arrangement: in_arrangement,
            time_selection: in_arrangement
                && view.time_selection().is_some()
                && view.selected_tracks().contains(&track),
            // IMP-25: Deactivate All Devices reads Activate All Devices when
            // every top-level device on the selection is already off.
            devices_off: {
                let mut any = false;
                let mut all_off = true;
                for &selected in view.selected_tracks() {
                    if let Some(track) = doc.track(selected) {
                        for device in track.devices() {
                            any = true;
                            all_off &= !device.enabled;
                        }
                    }
                }
                any && all_off
            },
        });
        let contexts = shell.track_menu_contexts();
        for item in shown(&menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            let package = context_menus::pending_package(item.row);
            if item.row == "C-TRACK-015" {
                palette(ui, theme, package.unwrap_or_default());
                continue;
            }
            let response =
                crate::menus::item_in(ui, item, shell, &keymap, self.platform, &contexts, events);
            let response = match package {
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
    }
}

/// Selects the title bar a menu opens on, unless it's selected already
/// (with the others it's selected with).
fn select(shell: &Shell, track: TrackId, events: &mut Vec<UiEvent>) {
    let view = shell.arrangement();
    if !(view.title_bars_selected() && view.selected_tracks().contains(&track)) {
        events.push(UiEvent::Invoke(Invocation::new(
            SELECT_TRACKS,
            json!({ "tracks": [track], "title_bars": true }),
        )));
    }
}

/// Double-clicking a track's title bar (detail-views R4): selects the track
/// and shows the Device View, as a click on its selector does.
pub fn open_device_view(shell: &Shell, track: TrackId, events: &mut Vec<UiEvent>) {
    events.push(UiEvent::Invoke(Invocation::new(
        SELECT_TRACKS,
        json!({ "tracks": [track], "title_bars": true }),
    )));
    events.extend(
        crate::main_window::open_view(shell, bl_shell::commands::DetailView::Device)
            .into_iter()
            .map(UiEvent::Invoke),
    );
}

/// The colour palette (C-TRACK-015), drawn in place and disabled until
/// tracks have colours: its swatches dimmed, as one control that names the
/// package.
fn palette(ui: &mut Ui, theme: &Theme, package: &str) {
    let (rows, columns) = PALETTE;
    let size = egui::vec2(14.0 * columns as f32, 14.0 * rows as f32);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    for row in 0..rows {
        for column in 0..columns {
            let hue = column as f32 / columns as f32;
            let value = 1.0 - row as f32 * 0.12;
            let swatch = Rect::from_min_size(
                rect.min + egui::vec2(column as f32, row as f32) * 14.0,
                egui::vec2(14.0, 14.0),
            );
            let colour = Color32::from(egui::ecolor::Hsva::new(hue, 0.5, value, 1.0));
            painter.rect_filled(swatch.shrink(1.0), 0.0, colour.gamma_multiply(0.4));
        }
    }
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, theme.colors.separator.0),
        egui::StrokeKind::Inside,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, false, "Colour palette"));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Group);
        node.set_label("Colour palette".to_owned());
        node.set_disabled();
        node.set_description(format!("Not built yet: arrives with {package}."));
    });
}

/// A name being typed in a track's title bar.
#[derive(Clone, Debug)]
struct Renaming {
    track: TrackId,
    /// The text, once it has been taken from the track's name.
    text: Option<String>,
    focused: bool,
    /// The pass that last drew it, so a title bar that isn't drawn doesn't
    /// keep the keys.
    drawn: Option<u64>,
}

fn renaming_id() -> Id {
    Id::new("track-renaming")
}

fn renaming(ctx: &Context) -> Option<Renaming> {
    ctx.data(|d| d.get_temp::<Option<Renaming>>(renaming_id()))
        .flatten()
}

/// Starts typing a new name for `track` in its title bar (Rename,
/// S-TRACK-004).
pub fn start_rename(ctx: &Context, track: TrackId) {
    ctx.data_mut(|d| {
        d.insert_temp(
            renaming_id(),
            Some(Renaming {
                track,
                text: None,
                focused: false,
                drawn: None,
            }),
        );
    });
}

/// Whether a track's name is being typed, so keys go to it. Called after
/// the pass has drawn: a name whose title bar wasn't drawn (scrolled out of
/// view, or in a folded group) is dropped, as Esc would.
pub fn is_renaming(ctx: &Context) -> bool {
    let Some(renaming) = renaming(ctx) else {
        return false;
    };
    if renaming.drawn == Some(ctx.cumulative_pass_nr()) {
        return true;
    }
    ctx.data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
    false
}

/// Draws the name being typed over `rect` if it's `track`'s, and says so;
/// Return runs `track.rename`, Esc leaves the name.
pub fn rename_edit(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    track: TrackId,
    name: &str,
    events: &mut Vec<UiEvent>,
) -> bool {
    let Some(mut renaming) = renaming(ui.ctx()).filter(|r| r.track == track) else {
        return false;
    };
    renaming.drawn = Some(ui.ctx().cumulative_pass_nr());
    let text = renaming.text.get_or_insert_with(|| name.to_owned());
    let edit = ui.put(
        rect,
        egui::TextEdit::singleline(text)
            .id(Id::new(("track-rename-edit", track)))
            .font(theme.small_font())
            .margin(egui::vec2(2.0, 0.0)),
    );
    if !renaming.focused {
        edit.request_focus();
        renaming.focused = true;
    }
    let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    if edit.lost_focus() || enter || escape {
        if enter || (edit.lost_focus() && !escape) {
            events.push(UiEvent::Invoke(Invocation::new(
                "track.rename",
                json!({ "track": track, "name": text }),
            )));
        }
        ui.ctx()
            .data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
    } else {
        ui.ctx()
            .data_mut(|d| d.insert_temp(renaming_id(), Some(renaming)));
    }
    true
}
