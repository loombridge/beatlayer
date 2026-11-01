//! The Session View's context menus and the Clip View title bar's (WP-6.56;
//! menus.md C-SLOT, C-SCENE, C-CLIP-SES and C-CLIPTITLE), and the names
//! typed in place by their Rename items.
//!
//! A right-click on a slot or a scene that isn't selected selects it first;
//! with several selected, the one clicked keeps the selection and an item
//! acts on all of them as its command does. S-CTX-002 opens the menu of the
//! selection. Each item runs what its shortcut or command runs; the ones whose
//! packages aren't built are drawn disabled and name theirs.

use bl_commands::Context;
use bl_model::{ClipData, ClipId, SceneId, TrackId, TrackKind};
use bl_shell::context_menus::{self, SceneMenu, SessionClipMenu, SlotMenu};
use bl_shell::menus::{MenuItem, shown};
use bl_shell::scene_fields::SceneField;
use bl_shell::session_view_commands as ids;
use bl_shell::{Invocation, Shell};
use egui::{Context as Egui, Id, Key, Pos2, Rect, Response, Ui};
use serde_json::json;

use crate::keymap::{Keymap, Platform};
use crate::main_window::UiEvent;
use crate::menus::{ItemState, draw_item, item_state_in};
use crate::theme::Theme;

/// The command that selects, which Select All Slots in Scene runs for each
/// slot.
const SELECT: &str = ids::SELECT;

/// What a menu was opened on.
#[derive(Clone, Copy, Debug, Default)]
struct On {
    /// The slot, for a slot's or a clip's menu.
    slot: Option<(TrackId, SceneId)>,
    /// The scene, for a scene's menu.
    scene: Option<SceneId>,
}

/// What a menu's items act on: the selection, with what was clicked joined
/// to it when the selection didn't hold it yet.
#[derive(Debug, Default)]
struct Selection {
    /// The selected slots.
    slots: Vec<(TrackId, SceneId)>,
    /// The tracks of the selected slots, each once.
    tracks: Vec<TrackId>,
    /// The clips in the selected slots.
    clips: Vec<ClipId>,
    /// The selected scenes.
    scenes: Vec<SceneId>,
}

fn selection(shell: &Shell, on: On) -> Selection {
    let doc = shell.editor().document();
    let state = shell.session().state();
    let mut slots: Vec<(TrackId, SceneId)> =
        state.slots.iter().map(|s| (s.track, s.scene)).collect();
    if let Some(slot) = on.slot
        && !slots.contains(&slot)
    {
        slots = vec![slot];
    }
    let mut scenes = state.scenes;
    if let Some(scene) = on.scene
        && !scenes.contains(&scene)
    {
        scenes = vec![scene];
    }
    let mut tracks = Vec::new();
    for (track, _) in &slots {
        if !tracks.contains(track) {
            tracks.push(*track);
        }
    }
    Selection {
        tracks,
        clips: slots
            .iter()
            .filter_map(|(track, scene)| doc.slot(*track, *scene))
            .map(|clip| clip.id)
            .collect(),
        scenes,
        slots,
    }
}

/// The contexts a Session menu's items run in whichever area has the
/// keyboard: the Session View's first, so the shortcuts the items show are
/// the Session View's.
fn contexts(shell: &Shell) -> Vec<Context> {
    let mut contexts = vec![Context::Session];
    for context in shell.active_contexts() {
        if !contexts.contains(&context) {
            contexts.push(context);
        }
    }
    if !contexts.contains(&Context::Global) {
        contexts.push(Context::Global);
    }
    contexts
}

/// The Session View's menus, with the keymap they take their keys from.
#[derive(Debug)]
pub struct SessionMenus {
    platform: Platform,
}

impl Default for SessionMenus {
    fn default() -> Self {
        Self {
            platform: Platform::current(),
        }
    }
}

impl SessionMenus {
    /// Gives a slot its context menu: the empty slot's (C-SLOT) or the clip's
    /// (C-CLIP-SES). `response` is the slot's, `at` where the keyboard opens
    /// the menu, and `first` whether the slot is the one the keys open it for
    /// (the first selected, with the grid focused).
    #[expect(clippy::too_many_arguments, reason = "the slot's inputs")]
    pub fn slot(
        &self,
        ui: &Ui,
        theme: &Theme,
        response: &Response,
        shell: &Shell,
        (track, scene): (TrackId, SceneId),
        (at, first): (Pos2, bool),
        events: &mut Vec<UiEvent>,
    ) {
        if response.secondary_clicked() {
            select_slot(shell, track, scene, events);
        }
        response.context_menu(|ui| self.show_slot(ui, theme, shell, track, scene, events));
        if first && crate::menus::context_menu_keys(ui) {
            crate::menus::open_keyboard_context_menu(ui.ctx(), response.id, at);
        }
        crate::menus::keyboard_context_menu(response, |ui| {
            self.show_slot(ui, theme, shell, track, scene, events);
        });
    }

    /// Gives a scene its context menu (C-SCENE), from a response of its row
    /// in the Main column or of the Scene View's title bar.
    #[expect(clippy::too_many_arguments, reason = "the scene's inputs")]
    pub fn scene(
        &self,
        ui: &Ui,
        theme: &Theme,
        response: &Response,
        shell: &Shell,
        scene: SceneId,
        (at, first): (Pos2, bool),
        events: &mut Vec<UiEvent>,
    ) {
        if response.secondary_clicked() {
            select_scene(shell, scene, events);
        }
        response.context_menu(|ui| self.show_scene(ui, theme, shell, scene, events));
        if first && crate::menus::context_menu_keys(ui) {
            crate::menus::open_keyboard_context_menu(ui.ctx(), response.id, at);
        }
        crate::menus::keyboard_context_menu(response, |ui| {
            self.show_scene(ui, theme, shell, scene, events);
        });
    }

    /// Gives a scene's tempo or signature field its menu: Return to Default
    /// (C-SCENE-016), enabled while the field holds the scene's own value.
    pub fn scene_field(
        &self,
        response: &Response,
        shell: &Shell,
        scene: SceneId,
        field: SceneField,
        events: &mut Vec<UiEvent>,
    ) {
        response.context_menu(|ui| {
            let keymap = Keymap::from_data(shell.keymap().clone());
            let contexts = contexts(shell);
            let menu = context_menus::scene_field(field == SceneField::Tempo);
            for item in shown(&menu) {
                let mut state = item_state_in(item, shell, &keymap, self.platform, &contexts);
                // The change the field's own gestures make: a double-click and
                // Delete turn it off as well.
                state.invocation = bl_shell::scene_fields::off(shell, scene, field);
                state.enabled = state.invocation.is_some();
                let _ = draw_item(ui, state, self.platform, events);
            }
        });
    }

    /// Gives the Clip View's title bar its menu (C-CLIPTITLE): it acts on
    /// the clips the Clip View shows. `focused` says the Clip View has the
    /// keyboard, which S-CTX-002 then opens the menu for.
    pub fn clip_title(
        &self,
        ui: &Ui,
        theme: &Theme,
        response: &Response,
        shell: &Shell,
        focused: bool,
        events: &mut Vec<UiEvent>,
    ) {
        response.context_menu(|ui| self.show_clip_title(ui, theme, shell, events));
        if focused && crate::menus::context_menu_keys(ui) {
            crate::menus::open_keyboard_context_menu(
                ui.ctx(),
                response.id,
                response.rect.left_bottom(),
            );
        }
        crate::menus::keyboard_context_menu(response, |ui| {
            self.show_clip_title(ui, theme, shell, events);
        });
    }

    fn show_slot(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        track: TrackId,
        scene: SceneId,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let Some(held) = doc.track(track) else {
            return;
        };
        let on = On {
            slot: Some((track, scene)),
            scene: None,
        };
        let (paste, _) = shell.session_clipboard();
        let stop_removed = !held.has_stop_button(scene);
        let menu = match doc.slot(track, scene) {
            Some(clip) => context_menus::session_clip(SessionClipMenu {
                paste,
                midi: matches!(clip.data, ClipData::Midi(_)),
                audio: matches!(clip.data, ClipData::Audio(_)),
                stop_removed,
            }),
            None => context_menus::slot(SlotMenu {
                paste,
                midi: held.kind() == TrackKind::Midi,
                group: held.kind() == TrackKind::Group,
                stop_removed,
            }),
        };
        self.show(ui, theme, shell, &menu, on, events);
    }

    fn show_scene(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        scene: SceneId,
        events: &mut Vec<UiEvent>,
    ) {
        let (_, paste) = shell.session_clipboard();
        let menu = context_menus::scene(SceneMenu { paste });
        let on = On {
            slot: None,
            scene: Some(scene),
        };
        self.show(ui, theme, shell, &menu, on, events);
    }

    fn show_clip_title(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        events: &mut Vec<UiEvent>,
    ) {
        let keymap = Keymap::from_data(shell.keymap().clone());
        let mut contexts = vec![Context::ClipView];
        contexts.extend(shell.active_contexts());
        let doc = shell.editor().document();
        let clips = title_clips(shell);
        let menu = context_menus::clip_title();
        for item in shown(&menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            if item.row == context_menus::CLIP_TITLE_COLOUR {
                let current = clips
                    .first()
                    .and_then(|clip| doc.clip(*clip))
                    .and_then(|clip| clip.colour);
                if let Some(colour) = crate::palette::show(ui, theme, "clip-title-colour", current)
                {
                    events.push(UiEvent::Invoke(Invocation::new(
                        "clip.set_colour",
                        json!({ "clips": clips, "colour": colour }),
                    )));
                    ui.close();
                }
                continue;
            }
            let mut state = item_state_in(item, shell, &keymap, self.platform, &contexts);
            if item.row == context_menus::CLIP_TITLE_RENAME {
                // Rename types the name in the title bar (R9): the item is
                // the menu's own, not a command's.
                state.enabled = !clips.is_empty();
                state.invocation = None;
                let response = draw_item(ui, state, self.platform, events);
                if response.clicked() {
                    start_rename(ui.ctx(), Rename::Title(clips.clone()));
                    ui.close();
                }
                crate::menus::take_focus_first(ui, &response);
                continue;
            }
            // Edit Info Text opens the Info View's field for the first clip.
            if item.row == "C-CLIPTITLE-002"
                && let (Some(clip), Some(invocation)) = (clips.first(), state.invocation.as_mut())
            {
                invocation.args = json!({ "clip": clip });
                state.enabled = shell.can_invoke_in(&invocation.id, &invocation.args, &contexts);
            }
            let response = draw_item(ui, state, self.platform, events);
            let response = pending(ui, response, item.row);
            crate::menus::take_focus_first(ui, &response);
        }
    }

    /// Draws a slot's or a scene's menu: `menu`'s items, each acting on what
    /// the menu was opened on and the selection with it.
    fn show(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        menu: &[MenuItem],
        on: On,
        events: &mut Vec<UiEvent>,
    ) {
        let keymap = Keymap::from_data(shell.keymap().clone());
        let contexts = contexts(shell);
        let doc = shell.editor().document();
        let chosen = selection(shell, on);
        let clicked_clip = on
            .slot
            .and_then(|(track, scene)| doc.slot(track, scene))
            .map(|clip| clip.id);
        for item in shown(menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            let row = item.row;
            if row == "C-CLIP-SES-018" {
                // Spec: themes R16. The palette colours the selected clips.
                let current = clicked_clip
                    .and_then(|clip| doc.clip(clip))
                    .and_then(|clip| clip.colour);
                if let Some(colour) =
                    crate::palette::show(ui, theme, "session-clip-colour", current)
                {
                    events.push(UiEvent::Invoke(Invocation::new(
                        "clip.set_colour",
                        json!({ "clips": chosen.clips, "colour": colour }),
                    )));
                    ui.close();
                }
                continue;
            }
            if row == "C-SCENE-014" {
                self.scene_colours(ui, theme, shell, on, &chosen, events);
                continue;
            }
            if row == "C-SCENE-009" {
                let state = ItemState {
                    label: item.label.to_owned(),
                    enabled: !chosen.scenes.is_empty(),
                    checked: None,
                    chord: None,
                    invocation: None,
                };
                let response = draw_item(ui, state, self.platform, events);
                if response.clicked() {
                    select_all_slots(shell, &chosen.scenes, events);
                    ui.close();
                }
                crate::menus::take_focus_first(ui, &response);
                continue;
            }
            if row == "C-SCENE-010" {
                // Cancel Scene Launch: one command for each selected scene
                // with a launch waiting.
                let queued: Vec<SceneId> = chosen
                    .scenes
                    .iter()
                    .copied()
                    .filter(|scene| shell.scene_launch_queued(*scene))
                    .collect();
                let state = ItemState {
                    label: item.label.to_owned(),
                    enabled: !queued.is_empty(),
                    checked: None,
                    chord: None,
                    invocation: None,
                };
                let response = draw_item(ui, state, self.platform, events);
                if response.clicked() {
                    for scene in queued {
                        events.push(UiEvent::Invoke(Invocation::new(
                            context_menus::CANCEL_SCENE_LAUNCH,
                            json!({ "scene": scene }),
                        )));
                    }
                    ui.close();
                }
                crate::menus::take_focus_first(ui, &response);
                continue;
            }
            let mut state = item_state_in(item, shell, &keymap, self.platform, &contexts);
            // The switches of the Settings window: ticked while on, and
            // choosing one turns it the other way.
            let prefs = shell.launch_prefs();
            let switch = match row {
                "C-CLIP-SES-017" => Some(prefs.select_on_launch),
                "C-SCENE-011" => Some(prefs.select_next_scene_on_launch),
                "C-SCENE-012" => Some(prefs.start_recording_on_scene_launch),
                _ => None,
            };
            if let Some(on_now) = switch {
                let command = match row {
                    "C-CLIP-SES-017" => "settings.set_select_on_launch",
                    "C-SCENE-011" => "settings.set_select_next_scene_on_launch",
                    _ => "settings.set_start_recording_on_scene_launch",
                };
                state.invocation = Some(Invocation::new(command, json!({ "on": !on_now })));
                state.enabled = true;
                state.checked = Some(on_now);
            }
            if row == "C-SCENE-013" {
                state.checked = Some(shell.mixer_view().scene_tempo());
            }
            // Items that take the selection's tracks or clips, or the slot
            // clicked, which a key would take from another view's selection.
            let args = match row {
                "C-SLOT-001" | "C-SLOT-002" | "C-CLIP-SES-010" | "C-CLIP-SES-011" => {
                    Some(json!({ "tracks": chosen.tracks }))
                }
                "C-SLOT-003" | "C-SLOT-009" | "C-CLIP-SES-012" => {
                    let mut args = json!({ "tracks": chosen.tracks });
                    if !chosen.clips.is_empty() {
                        args["clips"] = json!(chosen.clips);
                    }
                    Some(args)
                }
                "C-SLOT-004" | "C-CLIP-SES-013" => on
                    .slot
                    .map(|(track, scene)| json!({ "track": track, "scene": scene })),
                "C-CLIP-SES-007" => Some(json!({ "clips": chosen.clips })),
                "C-CLIP-SES-006" | "C-CLIP-SES-009" | "C-CLIP-SES-014" | "C-CLIP-SES-019a"
                | "C-CLIP-SES-020" => clicked_clip.map(|clip| json!({ "clip": clip })),
                "C-SCENE-006" => on.scene.map(|scene| json!({ "scene": scene })),
                _ => None,
            };
            if let (Some(args), Some(invocation)) = (args, state.invocation.as_mut()) {
                invocation.args = args;
                state.enabled = shell.can_invoke_in(&invocation.id, &invocation.args, &contexts);
            }
            if matches!(row, "C-SLOT-006" | "C-CLIP-SES-015")
                && let Some((track, scene)) = on.slot
            {
                let add = shell
                    .editor()
                    .document()
                    .track(track)
                    .is_some_and(|t| !t.has_stop_button(scene));
                state.invocation = Some(stop_button(track, scene, add));
                state.enabled = true;
            }
            // Crop Clip is disabled in an empty slot's menu (R6), and a
            // scene's Rename shows no key (C-SCENE-005).
            if row == "C-SLOT-005" {
                state.enabled = false;
            }
            if row == "C-SCENE-005" {
                state.chord = None;
            }
            // Several clips selected: Crop Clip crops each, and the stop
            // button of every selected slot goes the way the clicked one's
            // does (both commands take one slot or clip).
            let more: Vec<Invocation> = match row {
                "C-CLIP-SES-014" => chosen
                    .clips
                    .iter()
                    .filter(|clip| Some(**clip) != clicked_clip)
                    .map(|clip| Invocation::new("clip.crop", json!({ "clip": clip })))
                    .collect(),
                "C-SLOT-006" | "C-CLIP-SES-015" => {
                    let add = state
                        .invocation
                        .as_ref()
                        .is_some_and(|i| i.args["stop_button"] == json!(true));
                    chosen
                        .slots
                        .iter()
                        .filter(|slot| Some(**slot) != on.slot)
                        .map(|(track, scene)| stop_button(*track, *scene, add))
                        .collect()
                }
                _ => Vec::new(),
            };
            let response = draw_item(ui, state, self.platform, events);
            if response.clicked() {
                events.extend(more.into_iter().map(UiEvent::Invoke));
            }
            let response = pending(ui, response, row);
            crate::menus::take_focus_first(ui, &response);
        }
    }

    /// No Color, then the colour palette (C-SCENE-014), for the selected
    /// scenes.
    fn scene_colours(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        on: On,
        chosen: &Selection,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let colour_of = |scene: SceneId| doc.scene(scene).and_then(|s| s.colour);
        let state = ItemState {
            label: "No Color".to_owned(),
            enabled: !chosen.scenes.is_empty(),
            checked: None,
            chord: None,
            invocation: Some(Invocation::new(
                "scene.set_colour",
                json!({ "scenes": chosen.scenes, "colour": null }),
            )),
        };
        let response = draw_item(ui, state, self.platform, events);
        crate::menus::take_focus_first(ui, &response);
        let current = on.scene.and_then(colour_of);
        if let Some(colour) = crate::palette::show(ui, theme, "scene-colour", current) {
            events.push(UiEvent::Invoke(Invocation::new(
                "scene.set_colour",
                json!({ "scenes": chosen.scenes, "colour": colour }),
            )));
            ui.close();
        }
    }
}

/// The clips the Clip View's title bar acts on: the clips it shows, or the
/// audio clip its Sample Editor shows.
pub(crate) fn title_clips(shell: &Shell) -> Vec<ClipId> {
    let doc = shell.editor().document();
    let shown = shell.clip_view().shown(doc, shell.arrangement());
    if shown.is_empty() {
        shell
            .clip_view()
            .shown_any_clip(doc, shell.arrangement())
            .into_iter()
            .collect()
    } else {
        shown
    }
}

/// Names the package an item that isn't built waits for in its AccessKit
/// description and its hover text (R13).
fn pending(ui: &Ui, response: Response, row: &str) -> Response {
    match context_menus::pending_package(row) {
        Some(package) => {
            let text = format!("Not built yet: arrives with {package}.");
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_description(text.clone());
            });
            response.on_disabled_hover_text(text)
        }
        None => response,
    }
}

/// `slot.set_stop_button` for one slot: `add` gives it its button, and
/// otherwise removes it.
fn stop_button(track: TrackId, scene: SceneId, add: bool) -> Invocation {
    Invocation::new(
        "slot.set_stop_button",
        json!({ "track": track, "scene": scene, "stop_button": add }),
    )
}

/// Selects a slot unless it's selected already (with the others it's
/// selected with), and gives the Session View the keyboard.
fn select_slot(shell: &Shell, track: TrackId, scene: SceneId, events: &mut Vec<UiEvent>) {
    if !shell
        .session()
        .selected_slots()
        .iter()
        .any(|slot| slot.track == track && slot.scene == scene)
    {
        events.push(UiEvent::Invoke(Invocation::new(
            SELECT,
            json!({ "track": track, "scene": scene, "mode": "replace" }),
        )));
    }
    events.push(UiEvent::FocusArea(bl_shell::Area::MainView));
}

/// Selects a scene unless it's selected already.
fn select_scene(shell: &Shell, scene: SceneId, events: &mut Vec<UiEvent>) {
    if !shell.session().state().scenes.contains(&scene) {
        events.push(UiEvent::Invoke(Invocation::new(
            SELECT,
            json!({ "scene": scene, "mode": "replace" }),
        )));
    }
    events.push(UiEvent::FocusArea(bl_shell::Area::MainView));
}

/// Select All Slots in Scene (C-SCENE-009): every slot of the scenes, on the
/// tracks that have slots, through `session.select`.
fn select_all_slots(shell: &Shell, scenes: &[SceneId], events: &mut Vec<UiEvent>) {
    let doc = shell.editor().document();
    let tracks: Vec<TrackId> = shell
        .mixer_columns()
        .into_iter()
        .filter(|track| {
            doc.track(*track).is_some_and(|t| {
                matches!(
                    t.kind(),
                    TrackKind::Audio | TrackKind::Midi | TrackKind::Group
                )
            })
        })
        .collect();
    let mut mode = "replace";
    for scene in scenes {
        for track in &tracks {
            events.push(UiEvent::Invoke(Invocation::new(
                SELECT,
                json!({ "track": track, "scene": scene, "mode": mode }),
            )));
            mode = "add";
        }
    }
}

/// What a name is being typed for.
#[derive(Clone, Debug, PartialEq)]
pub enum Rename {
    /// A Session clip, in its slot.
    Clip(ClipId),
    /// A scene, in its name field in the Main column.
    Scene(SceneId),
    /// The clips the Clip View shows, in its title bar.
    Title(Vec<ClipId>),
}

impl Rename {
    /// The command Return runs with `name`.
    fn invocation(&self, name: &str) -> Invocation {
        match self {
            Self::Clip(clip) => {
                Invocation::new("clip.rename", json!({ "clip": clip, "name": name }))
            }
            Self::Scene(scene) => {
                Invocation::new("scene.rename", json!({ "scene": scene, "name": name }))
            }
            Self::Title(clips) => {
                Invocation::new("clip.rename", json!({ "clips": clips, "name": name }))
            }
        }
    }
}

/// A name being typed.
#[derive(Clone, Debug)]
struct Renaming {
    target: Rename,
    /// The text, once it has been taken from the current name.
    text: Option<String>,
    focused: bool,
    /// The pass that last drew it, so a name whose place isn't drawn doesn't
    /// keep the keys.
    drawn: Option<u64>,
}

fn renaming_id() -> Id {
    Id::new("session-renaming")
}

fn renaming(ctx: &Egui) -> Option<Renaming> {
    ctx.data(|d| d.get_temp::<Option<Renaming>>(renaming_id()))
        .flatten()
}

/// Starts typing a new name for `target` (Rename, S-EDIT-008).
pub fn start_rename(ctx: &Egui, target: Rename) {
    ctx.data_mut(|d| {
        d.insert_temp(
            renaming_id(),
            Some(Renaming {
                target,
                text: None,
                focused: false,
                drawn: None,
            }),
        );
    });
}

/// Whether a name is being typed here, so keys go to it. Called after the
/// pass has drawn: a name whose place wasn't drawn (scrolled out of view) is
/// dropped, as Esc would.
pub fn is_renaming(ctx: &Egui) -> bool {
    let Some(renaming) = renaming(ctx) else {
        return false;
    };
    if renaming.drawn == Some(ctx.cumulative_pass_nr()) {
        return true;
    }
    ctx.data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
    false
}

/// The scene Tab moves on to while `scene`'s name is being typed (S-EDIT-012).
#[must_use]
pub fn next_scene(doc: &bl_model::Document, scene: SceneId) -> Option<Rename> {
    let mut scenes = doc.scenes().map(|s| s.id).skip_while(|s| *s != scene);
    scenes.nth(1).map(Rename::Scene)
}

/// Draws the name being typed over `rect` if it's for `target`, and says so;
/// Return runs its command, Esc leaves the name as it was. Tab keeps the
/// name and moves on to `next`'s (S-EDIT-012; I-FOCUS-011).
pub fn rename_edit(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    target: &Rename,
    name: &str,
    next: Option<Rename>,
    events: &mut Vec<UiEvent>,
) -> bool {
    let Some(mut renaming) = renaming(ui.ctx()).filter(|r| &r.target == target) else {
        return false;
    };
    renaming.drawn = Some(ui.ctx().cumulative_pass_nr());
    let text = renaming.text.get_or_insert_with(|| name.to_owned());
    let edit = ui.put(
        rect,
        egui::TextEdit::singleline(text)
            .id(Id::new(("session-rename-edit", format!("{target:?}"))))
            .font(theme.small_font())
            .margin(egui::vec2(2.0, 0.0)),
    );
    if !renaming.focused {
        edit.request_focus();
        renaming.focused = true;
    }
    let (enter, escape, tab) = ui.input(|i| {
        (
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::Tab),
        )
    });
    if edit.lost_focus() || enter || escape {
        if enter || (edit.lost_focus() && !escape) {
            events.push(UiEvent::Invoke(target.invocation(text)));
        }
        ui.ctx()
            .data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
        if let Some(next) = next.filter(|_| tab && edit.lost_focus()) {
            start_rename(ui.ctx(), next);
        }
    } else {
        ui.ctx()
            .data_mut(|d| d.insert_temp(renaming_id(), Some(renaming)));
    }
    true
}

/// A value being typed in a scene's field, so Delete and the other keys
/// stay with it rather than the Session View's.
fn field_focus_id() -> Id {
    Id::new("scene-field-focus")
}

/// Notes that a scene field has the keyboard this pass.
pub(crate) fn note_field_focus(ctx: &Egui) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(field_focus_id(), pass));
}

/// Whether a scene's field had the keyboard in the pass just drawn.
pub fn field_has_focus(ctx: &Egui) -> bool {
    ctx.data(|d| d.get_temp::<u64>(field_focus_id()))
        .is_some_and(|pass| pass == ctx.cumulative_pass_nr())
}
