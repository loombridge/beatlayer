//! What the menus show: contexts, switches, labels and whether a command can run.

use super::info_text;
use super::{
    Area, CUE_MODE, DELAY_COMPENSATION, LOCK_ENVELOPES, MainView, REDUCED_LATENCY_MONITORING,
    SOLO_IN_PLACE, Shell, WindowKind,
};
use crate::arrangement::automation;
use crate::arrangement::editing;
use crate::arrangement::editing::Clipboard;
use crate::assistant as assistant_ids;
use crate::commands as ids;
use crate::devices;
use crate::midi_tools;
use crate::settings::ZOOM_MAX_PERCENT;
use crate::settings::ZOOM_MIN_PERCENT;
use bl_commands::Context;
use bl_engine::session as engine_session;
use bl_engine::transport as engine_transport;
use bl_model::ClipData;
use bl_model::TrackId;
use bl_model::TrackKind;
use bl_model::registry as model;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::audio_editing as audio_ids;
use bl_shell_spec::automation as automation_ids;
use bl_shell_spec::autosampler as autosampler_ids;
use bl_shell_spec::bounce as bounce_ids;
use bl_shell_spec::browser as browser_ids;
use bl_shell_spec::capture_midi as capture_ids;
use bl_shell_spec::clip_envelope_editing as clip_envelope_ids;
use bl_shell_spec::clip_properties as panel_ids;
use bl_shell_spec::commands::DetailView;
use bl_shell_spec::comping as comping_ids;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::envelope_keys as envelope_key_ids;
use bl_shell_spec::export as export_ids;
use bl_shell_spec::freeze as freeze_ids;
use bl_shell_spec::locator as locator_ids;
use bl_shell_spec::midi_tools as tool_ids;
use bl_shell_spec::mixer_view as mixer_ids;
use bl_shell_spec::notes as note_ids;
use bl_shell_spec::notes::ClipTab;
use bl_shell_spec::session_launch as session_launch_ids;
use bl_shell_spec::session_record as session_record_ids;
use bl_shell_spec::session_view as session_ids;
use bl_shell_spec::slice as slice_ids;
use bl_shell_spec::stems as stems_ids;
use bl_shell_spec::tracks as track_ids;
use bl_shell_spec::transport as transport_ids;
use serde_json::Value;
use serde_json::json;

impl Shell {
    /// The contexts that apply to a key press now, narrowest first: the
    /// focused area's, then `tab-focus` while Tab moves the focus, then
    /// `global`. In the Settings window: `settings`, `dialog`, `global`.
    #[must_use]
    pub fn active_contexts(&self) -> Vec<Context> {
        match self.focused_window {
            WindowKind::Settings => vec![Context::Settings, Context::Dialog, Context::Global],
            // The floating assistant: only the global keys work in it.
            WindowKind::Assistant => vec![Context::Global],
            // A floating device's window: its device is selected, so the
            // Device View's keys act on it (Float's among them), and the
            // global ones work.
            WindowKind::Device(_) => vec![Context::DeviceView, Context::Global],
            window @ (WindowKind::Main | WindowKind::Second) => {
                let mut contexts = Vec::with_capacity(3);
                // Spec: crash-recovery R23. While the recovery prompt is
                // open, it has the keyboard: the main window's own keys
                // don't run, so Tab moves between the prompt's buttons and
                // Esc closes it.
                if self.recovery_prompt_open() {
                    return contexts;
                }
                let detail = self.detail_of(window);
                match self.focused_area_of(window) {
                    Area::MainView => match self.main_view_of(window) {
                        MainView::Arrangement => {
                            // A selected title bar's keys come first
                            // (shortcuts.md, `track-headers`).
                            if self.arrangement.title_bars_selected() {
                                contexts.push(Context::TrackHeaders);
                            }
                            // Spec: comping R28. A take lane is selected: its
                            // header, the time on it or a clip on it.
                            if !self.arrangement.title_bars_selected()
                                && self.arrangement.take_lane_selected()
                            {
                                contexts.push(Context::TakeLanes);
                            }
                            // Spec: automation-editing R20. A selected
                            // automation lane's keys come ahead of the
                            // Arrangement's.
                            if self.arrangement.keyboard_lane().is_some()
                                && !self.envelope_control_focused
                            {
                                contexts.push(Context::Envelope);
                            }
                            contexts.push(Context::Arrangement);
                        }
                        MainView::Session => contexts.push(Context::Session),
                    },
                    Area::Browser => {
                        // The Groove Pool's keys are its own: only the
                        // global ones apply (grooves R24).
                        let part = self.browser_view.focus().context();
                        if part != Context::Global {
                            contexts.push(part);
                        }
                    }
                    Area::Detail if self.value_focused => contexts.push(Context::ValueField),
                    // Spec: piano-roll. With the Clip View open, its MIDI
                    // Note Editor holds the detail area's keys.
                    Area::Detail if detail.focus_in_clip() => {
                        // Spec: multi-clip-editing R11. With loop bars
                        // selected, Cmd-D duplicates their loops.
                        if !self
                            .clip_view
                            .loop_bars(self.editor.document(), &self.arrangement)
                            .is_empty()
                        {
                            contexts.push(Context::LoopBrace);
                        }
                        contexts.push(Context::ClipView);
                        if self.clip_view.tab() == ClipTab::Envelopes {
                            if !self.envelope_control_focused {
                                contexts.push(Context::Envelope);
                            }
                        } else if self
                            .clip_view
                            .shown_audio_clip(self.editor.document(), &self.arrangement)
                            .is_some()
                        {
                            // Spec: sample-editor R1. An audio clip's editor
                            // is the Sample Editor.
                            contexts.push(Context::SampleEditor);
                        } else {
                            contexts.push(Context::NoteEditor);
                            if self
                                .clip_view
                                .active_clips(self.editor.document(), &self.arrangement)
                                .iter()
                                .any(|clip| !self.clip_view.selection_of(*clip).is_empty())
                            {
                                contexts.push(Context::NoteSelection);
                            }
                        }
                    }
                    Area::Detail => {
                        // Spec: racks R22. A chain list with the focus
                        // takes S and the edit keys.
                        if self.devices.chain_list_focus() {
                            contexts.push(Context::RackChains);
                        }
                        contexts.push(Context::DeviceView);
                    }
                    Area::ControlBar | Area::StatusBar | Area::Assistant | Area::UndoHistory => {}
                }
                if self.prefs.tab_navigation {
                    contexts.push(Context::TabFocus);
                }
                contexts.push(Context::Global);
                contexts
            }
        }
    }

    /// Whether a menu item that runs `id` with `args` is the chosen one of
    /// its group now, for the radio pairs a menu shows with a check mark
    /// (Options: Solo Switches and Cue Switches). `None` for commands that
    /// aren't choices.
    #[must_use]
    pub fn choice_state(&self, id: &str, args: &Value) -> Option<bool> {
        match id {
            CUE_MODE => Some(self.editor.document().cue_active() == args["enabled"].as_bool()?),
            panel_ids::ARRANGE_PANELS => Some(
                self.clip_view.panel_arrangement()
                    == serde_json::from_value(args["arrangement"].clone()).ok()?,
            ),
            // Spec: recording R17. The submenu's ticked item is the set's
            // value.
            transport_ids::SET_RECORD_QUANTIZATION => Some(
                self.editor.document().settings().record_quantization
                    == serde_json::from_value(args["quantization"].clone()).ok()?,
            ),
            // Spec: map-modes R1. Edit MIDI Map and Edit Key Map are ticked
            // while their mode is on.
            bl_shell_spec::remote::MAP_MODE => {
                Some(self.remote.map.mode == serde_json::from_value(args["mode"].clone()).ok()?)
            }
            ids::VIEW_NOTE_SPELLING => Some(
                self.clip_view.note_spelling()
                    == serde_json::from_value(args["spelling"].clone()).ok()?,
            ),
            // Spec: time-editing R10. The loop brace's menu item is ticked
            // while the song starts at the brace's start.
            locator_ids::SET_START_TIME if args["loop_brace"].as_bool() == Some(true) => Some(
                self.editor.document().settings().song_start
                    == Some(bl_model::SongStart::LoopBrace),
            ),
            _ => None,
        }
    }

    /// Whether a switch-like command is on now: the state a menu shows as a
    /// check mark. `None` for commands that aren't switches.
    #[must_use]
    pub fn switch_state(&self, id: &str) -> Option<bool> {
        match id {
            // Spec: time-editing R10. A locator's menu item is ticked while
            // the song starts at that locator.
            locator_ids::SET_START_TIME => {
                let selected = self.arrangement.selected_locators().first().copied();
                Some(
                    self.editor.document().settings().song_start
                        == selected.map(bl_model::SongStart::Locator),
                )
            }
            ids::VIEW_FULL_SCREEN => Some(self.full_screen_of(self.acting_window())),
            ids::VIEW_TOGGLE_SECOND_WINDOW => Some(self.second.open),
            ids::VIEW_TOGGLE_BROWSER => Some(self.browser_of(self.acting_window())),
            ids::VIEW_TOGGLE_FULL_HEIGHT_BROWSER => Some(self.browser_view.full_height()),
            ids::VIEW_TUNING_SECTION => Some(self.tuning_section),
            bl_shell_spec::groove::VIEW_TOGGLE_GROOVE_POOL => Some(self.groove_pool.shown),
            ids::VIEW_TOGGLE_CLIP_VIEW => Some(self.detail_of(self.acting_window()).clip_shown()),
            ids::VIEW_TOGGLE_DEVICE_VIEW => {
                Some(self.detail_of(self.acting_window()).device_shown())
            }
            ids::VIEW_EXPAND_CLIP_VIEW => Some(self.detail_of(self.acting_window()).expanded()),
            ids::VIEW_TOGGLE_PLUGIN_LIST => Some(self.plugin_list.visible()),
            ids::VIEW_DRAW_MODE => Some(self.clip_view.draw_mode()),
            note_ids::SET_PREVIEW => Some(self.clip_view.preview()),
            ids::VIEW_FOLD_NOTES => Some(
                self.clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .is_some_and(|clip| self.clip_view.is_folded(clip)),
            ),
            ids::VIEW_FOLD_SCALE => Some(
                self.clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .is_some_and(|clip| {
                        self.clip_view
                            .fold(self.editor.document(), clip)
                            .scale
                            .is_some()
                    }),
            ),
            ids::VIEW_HIGHLIGHT_SCALES => Some(self.clip_view.highlight_scales()),
            panel_ids::FOLD_PANELS => Some(self.clip_view.panels_folded()),
            ids::VIEW_TOGGLE_INFO_VIEW => Some(self.info_view),
            assistant_ids::VIEW_TOGGLE_ASSISTANT => Some(self.assistant_open),
            ids::VIEW_TOGGLE_UNDO_HISTORY => Some(self.undo_history_open),
            ids::VIEW_TOGGLE_SAVE_HISTORY => Some(self.save_history.open),
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => Some(self.plug_in_windows),
            ids::FOCUS_TOGGLE_TAB_NAVIGATION => Some(self.prefs.tab_navigation),
            ids::FOCUS_TOGGLE_WRAP_NAVIGATION => Some(self.prefs.wrap_navigation.0),
            ids::SETTINGS_SET_RETUNE_ON_TUNING_LOAD => Some(self.prefs.retune_on_tuning_load),
            ids::SETTINGS_SET_SNAP_AUTOMATION_TO_GRID => Some(self.prefs.snap_automation_to_grid.0),
            ids::SETTINGS_SET_START_TRANSPORT_WITH_TAP_TEMPO => {
                Some(self.prefs.start_transport_with_tap_tempo.0)
            }
            session_launch_ids::SET_QUANTIZATION_ENABLED => Some(self.store.launch().enabled),
            SOLO_IN_PLACE => Some(self.editor.document().settings().solo_in_place),
            DELAY_COMPENSATION => Some(self.editor.document().settings().delay_compensation),
            REDUCED_LATENCY_MONITORING => {
                Some(self.editor.document().settings().reduced_latency_monitoring)
            }
            LOCK_ENVELOPES => Some(self.editor.document().settings().lock_envelopes),
            "transport.set_metronome_enabled" => {
                Some(self.editor.document().settings().metronome.enabled)
            }
            "transport.set_midi_overdub" => Some(self.editor.document().settings().midi_overdub),
            "transport.set_metronome_only_while_recording" => Some(
                self.editor
                    .document()
                    .settings()
                    .metronome
                    .only_while_recording,
            ),
            ids::AUDIO_TOGGLE_ENGINE => Some(self.audio.engine_on()),
            browser_ids::SET_PREVIEW => Some(self.browser_view.preview_on()),
            browser_ids::SET_FILTER_VIEW => Some(self.browser_view.filter_view_shown()),
            "device.set_show_preset_name" => self
                .devices
                .selected()
                .and_then(|d| self.editor.document().device(d))
                .map(|d| d.show_preset_name),
            // IMP-14: Float is ticked while the selected device floats.
            device_ids::SET_FLOATING => self
                .devices
                .selected()
                .and_then(|d| self.editor.document().device(d))
                .map(|d| d.floating),
            _ if mixer_ids::handles(id) => self.mixer.switch_state(id, self.front_mixer()),
            _ => self.arrangement.switch_state(id),
        }
    }

    /// Whether a submenu opens now: Arrangement Track Controls is disabled
    /// while the Session View is in front (observed in Live 12.4.6; the
    /// mixer view spec, R12), and the set's backups are disabled while the
    /// set has none (file-management R18).
    #[must_use]
    pub fn can_open_submenu(&self, row: &str) -> bool {
        if row == "M-VIEW-033" && self.main_view == MainView::Session {
            return false;
        }
        if row == "M-FILE-008" {
            return !self.backups().is_empty();
        }
        true
    }

    /// The label a menu shows for a command now: Undo and Redo name the step
    /// they act on, or say there's nothing (M-EDIT-001, M-EDIT-002).
    #[must_use]
    /// The command a menu item runs: the one its row names, or the one
    /// that fits the state, such as Create ▸ Add Locator, which reads
    /// Delete Locator and deletes while a locator is selected (the time
    /// editing spec, R4).
    pub fn menu_command<'a>(&self, id: &'a str) -> &'a str {
        // Spec: comping R28. Edit ▸ Rename renames the selected take lanes.
        if id == "clip.rename" && !self.arrangement.selected_take_lanes().is_empty() {
            return "take_lane.rename";
        }
        if self.arrangement.selected_locators().is_empty() {
            return id;
        }
        match id {
            // Spec: time-editing R4. Create ▸ Add Locator reads Delete
            // Locator and deletes while a locator is selected.
            locator_ids::ADD => locator_ids::DELETE,
            // Spec: time-editing R8. Edit ▸ Rename renames the selected
            // locator.
            "clip.rename" => locator_ids::RENAME,
            _ => id,
        }
    }

    pub fn dynamic_label(&self, id: &str) -> Option<String> {
        match id {
            // Spec: time-editing R4. The Create menu's Add Locator reads
            // Delete Locator while a locator is selected.
            locator_ids::DELETE => Some("Delete Locator".to_owned()),
            // Spec: time-editing R30. Create ▸ Add Time Signature Change reads
            // Insert Time Signature Change with the Arrangement focused.
            locator_ids::INSERT_SIGNATURE => self
                .active_contexts()
                .contains(&Context::Arrangement)
                .then(|| "Insert Time Signature Change".to_owned()),
            // Spec: time-editing R17. Create ▸ Insert Scene reads Insert
            // Silence with the Arrangement focused.
            edit_ids::INSERT_SILENCE => Some("Insert Silence".to_owned()),
            // Spec: audio-clips R28. Create ▸ Create Fade reads Create Fade
            // In/Out or Create Crossfade as the selection gives one.
            edit_ids::CREATE_FADE => Some(
                editing::create_fade_label(self.editor.document(), &self.arrangement).to_owned(),
            ),
            model::UNDO => Some(self.editor.undo_history().next().map_or_else(
                || "Nothing to Undo".to_owned(),
                |step| format!("Undo {step}"),
            )),
            model::REDO => Some(self.editor.redo_history().next().map_or_else(
                || "Nothing to Redo".to_owned(),
                |step| format!("Redo {step}"),
            )),
            // Spec: device-compare R3, R4: the items follow the state of the
            // selected device.
            device_ids::COMPARE_SWITCH | device_ids::COMPARE_COPY => {
                let b = self
                    .devices
                    .selected()
                    .and_then(|d| self.editor.document().device(d))
                    .is_some_and(|d| d.b_active);
                Some(
                    match (id == device_ids::COMPARE_SWITCH, b) {
                        (true, false) => "Compare: Switch to B",
                        (true, true) => "Compare: Switch to A",
                        (false, false) => "Compare: Copy A to B",
                        (false, true) => "Compare: Copy B to A",
                    }
                    .to_owned(),
                )
            }
            // IMP-24: the item follows the selected device's position.
            device_ids::SET_ROLE => {
                let selected = self
                    .devices
                    .selected()
                    .and_then(|device| self.editor.document().device(device));
                Some(match selected {
                    Some(device) if device.role == bl_model::DeviceRole::MidiEffect => {
                        let role = self
                            .devices
                            .device_type(&device.kind)
                            .map_or(bl_model::DeviceRole::AudioEffect, |kind| kind.role);
                        format!("Use as {}", role.label())
                    }
                    _ => "Use as MIDI Effect".to_owned(),
                })
            }
            // Spec: detail-views R6. Hide Device View reads for the views it
            // would hide, or the ones it would bring back.
            ids::VIEW_TOGGLE_DETAIL_AREA => {
                let (hiding, views) = self.detail_of(self.acting_window()).toggle_area_views();
                let names = match views[..] {
                    [DetailView::Clip] => "Clip View",
                    [DetailView::Device] => "Device View",
                    _ => "Clip and Device View",
                };
                Some(format!("{} {names}", if hiding { "Hide" } else { "Show" }))
            }
            // Spec: note-editing R18. The item reads Activate Note(s) when
            // every selected note is deactivated already.
            note_ids::DEACTIVATE => Some(
                if self
                    .clip_view
                    .selection_deactivated(self.editor.document(), &self.arrangement)
                {
                    "Activate Note(s)"
                } else {
                    "Deactivate Note(s)"
                }
                .to_owned(),
            ),
            // Spec: note-editing R22. M-EDIT-014 and M-EDIT-015 read Group
            // Notes and Ungroup Notes with notes selected.
            note_ids::GROUP => Some("Group Notes".to_owned()),
            note_ids::UNGROUP => Some("Ungroup Notes".to_owned()),
            // The Playback menu's first item toggles with Stop (M-PLAY-001).
            transport_ids::PLAY_STOP => {
                Some(if self.playing() { "Stop" } else { "Play" }.to_owned())
            }
            _ => None,
        }
    }

    /// Whether a person can run `id` with `args` now, from a menu or a key:
    /// it applies in an active context, the arguments the shell can fill in
    /// fit its schema, and nothing about the state rules it out.
    #[must_use]
    pub fn can_invoke(&self, id: &str, args: &Value) -> bool {
        self.can_invoke_in(id, args, &self.active_contexts())
    }

    /// The contexts the MIDI Note Editor's context menus work in (C-NOTE,
    /// C-KEYS): the note editor's, and the note selection's while notes are
    /// selected, whatever has the keyboard, because the menu acts on what it
    /// was opened on (Spec: piano-roll R30; note-editing R18).
    #[must_use]
    pub fn note_menu_contexts(&self) -> Vec<Context> {
        let mut contexts = vec![Context::NoteEditor];
        if !self
            .clip_view
            .selected_notes(self.editor.document(), &self.arrangement)
            .is_empty()
        {
            contexts.push(Context::NoteSelection);
        }
        contexts.extend([Context::ClipView, Context::Global]);
        contexts
    }

    /// Whether the Envelope Editor has the keyboard: the Clip View's
    /// Envelopes tab, with the detail area focused, among `active`.
    pub(super) fn envelope_editor_keyed(&self, active: &[Context]) -> bool {
        active.contains(&Context::ClipView) && active.contains(&Context::Envelope)
    }

    /// Whether a person can run Envelope Editor command `id` now (the clip
    /// envelopes spec, R12, R13).
    pub(super) fn can_clip_envelope_edit(&self, id: &str, active: &[Context]) -> bool {
        self.clip_view.tab() == ClipTab::Envelopes
            && crate::clip_envelope_edit::can_run(
                id,
                self.editor.document(),
                &self.clip_view,
                &self.arrangement,
                self.envelope_clipboard.as_ref(),
                self.envelope_editor_keyed(active),
            )
    }

    /// [`Shell::can_invoke`] with `active` as the contexts that apply.
    #[must_use]
    pub fn can_invoke_in(&self, id: &str, args: &Value, active: &[Context]) -> bool {
        let Some(spec) = self.registry.get(id) else {
            return false;
        };
        if self.edit_gesture.is_some() {
            // A drag is under way; keys wait for it (Esc cancels it).
            return false;
        }
        // Spec: detail-views R11. The menu item is enabled while the Clip
        // View shows, whichever area has the focus; the key is the Clip
        // View's own.
        if id == ids::VIEW_EXPAND_CLIP_VIEW {
            return self.detail_of(self.window_in(args)).clip_shown();
        }
        if !spec.contexts.iter().any(|context| active.contains(context)) {
            return false;
        }
        // Spec: recording R21. The submenu's items are disabled while a
        // recording holds the value.
        if id == transport_ids::SET_RECORD_QUANTIZATION
            && self.record_quantization_locked().is_some()
        {
            return false;
        }
        if info_text::is_info_command(id) && !Self::has_info_text(id, args) {
            return self.info_target_of(id, args).is_some();
        }
        if id == ids::EDIT_INFO_TEXT {
            return self.info_target_selected().is_some();
        }
        if envelope_key_ids::handles(id) {
            return self.can_envelope_key(id);
        }
        if clip_envelope_ids::handles(id) {
            return self.can_clip_envelope_edit(id, active);
        }
        if freeze_ids::handles(id) {
            return self.can_freeze(id, args);
        }
        if bounce_ids::handles(id) {
            return self.can_bounce(id, args);
        }
        if comping_ids::handles(id) {
            return self.can_comp(id, args);
        }
        if audio_ids::handles(id) {
            // Spec: audio-clip-editing R28. The menus' enabled states.
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.can_audio_edit(id, &args);
        }
        match id {
            model::UNDO => return self.editor.can_undo(),
            model::REDO => return self.editor.can_redo(),
            // Spec: midi-tools R19. Apply Transformation/Generator is
            // disabled while no MIDI clip shows.
            tool_ids::APPLY => {
                return midi_tools::can_apply(
                    self.editor.document(),
                    &self.clip_view,
                    &self.arrangement,
                ) || self.audio_apply(id, args).is_some_and(|clip| {
                    matches!(
                        self.editor.document().clip(clip).map(|clip| &clip.data),
                        Some(ClipData::Audio(audio)) if audio.warp
                    )
                });
            }
            // The whole-track copies (the track context menu spec, R3):
            // Cut, Copy and Duplicate work on the selected tracks, and
            // Paste on the tracks on the clipboard.
            track_ids::COPY | track_ids::CUT | track_ids::DUPLICATE => {
                return self.can_copy_tracks();
            }
            track_ids::PASTE => {
                return self
                    .clipboard
                    .as_ref()
                    .is_some_and(|clipboard| match clipboard {
                        Clipboard::Tracks(copied) => !copied.tracks.is_empty(),
                        Clipboard::Material(_)
                        | Clipboard::Time(_)
                        | Clipboard::Slots(_)
                        | Clipboard::Scenes(_) => false,
                    });
            }
            // Spec: session-context-menus R12, R16. Export MIDI Clip... is for
            // a MIDI clip.
            bl_shell_spec::midi_files::EXPORT => {
                return args
                    .get("clip")
                    .and_then(|clip| serde_json::from_value::<bl_model::ClipId>(clip.clone()).ok())
                    .or_else(|| self.selected_clip())
                    .and_then(|clip| self.editor.document().clip(clip))
                    .is_some_and(|clip| matches!(clip.data, ClipData::Midi(_)));
            }
            ids::APP_HIDE => return cfg!(target_os = "macos"),
            ids::SET_OPEN
            | ids::SET_SAVE
            | ids::SET_SAVE_AS
            | ids::APP_SETTINGS
            | bl_shell_spec::dawproject::EXPORT_DAWPROJECT => return true,
            ids::SET_DISCARD_CHANGES => return self.unsaved_changes(),
            // Options ▸ Reduced Latency When Monitoring is available only
            // while Delay Compensation is on (the delay-compensation spec,
            // R8).
            REDUCED_LATENCY_MONITORING => {
                return self.editor.document().settings().delay_compensation;
            }
            // Spec: scales R23. View ▸ Fold to Scale is enabled while the
            // editor shows a MIDI clip whose Scale Mode is on.
            ids::VIEW_FOLD_SCALE => {
                return self
                    .clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .and_then(|clip| self.editor.document().clip(clip))
                    .is_some_and(|clip| clip.scale.enabled);
            }
            // Spec: find-and-select-notes R13. Edit ▸ Invert Selection is
            // enabled while the editor shows a clip that holds notes.
            note_ids::INVERT_SELECTION => {
                return self
                    .clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .and_then(|clip| self.editor.document().clip(clip))
                    .is_some_and(
                        |clip| matches!(&clip.data, ClipData::Midi(midi) if !midi.notes.is_empty()),
                    );
            }
            // Spec: note-editing R22, R23. Group Notes needs two notes, and
            // Ungroup Notes a grouped one.
            note_ids::GROUP | note_ids::UNGROUP => {
                let named = args.get("notes").and_then(Value::as_array).map(Vec::len);
                return if id == note_ids::GROUP {
                    named.unwrap_or_else(|| {
                        self.clip_view
                            .selected_notes(self.editor.document(), &self.arrangement)
                            .len()
                    }) >= 2
                } else {
                    named.is_some()
                        || self
                            .clip_view
                            .selection_group_kinds(self.editor.document(), &self.arrangement)
                            != (false, false)
                };
            }
            // Spec: step-recording R9. Step Input Mode is available while the
            // editor shows a MIDI clip.
            note_ids::STEP_INPUT_MODE => return self.editor_track().is_some(),
            ids::FOCUS_NEXT_NEIGHBOR => return self.has_neighbor(false),
            ids::FOCUS_PREVIOUS_NEIGHBOR => return self.has_neighbor(true),
            // Spec: time-editing R8. Cmd-R renames the selected locators
            // while there are any.
            "clip.rename" => {
                return self.arrangement.selected_locators().is_empty()
                    && self.arrangement.selected_signature().is_none()
                    && self.arrangement.selected_take_lanes().is_empty();
            }
            // Spec: comping R28, R29. Rename on a take lane's header types the
            // name in place; without a name it needs a header selected or a
            // lane named.
            "take_lane.rename" if args.get("name").is_none() => {
                return args.get("lane").is_some()
                    || !self.arrangement.selected_take_lanes().is_empty();
            }
            // Spec: track-context-menu R10. The two deletes of a track's
            // take lanes act on the selected tracks, and are enabled when one
            // has a take lane.
            "take_lane.delete_all" | "take_lane.delete_unused" if args.get("tracks").is_none() => {
                let doc = self.editor.document();
                return self.arrangement.selected_tracks().iter().any(|track| {
                    doc.track(*track)
                        .is_some_and(|held| held.take_lanes().len() > 0)
                });
            }
            // Spec: mpe R22. The MPE tab is for a MIDI clip: View ▸ Expression
            // Editor is disabled while none shows.
            ids::VIEW_CLIP_TAB if args["tab"].as_str() == Some("mpe") => {
                return self
                    .clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .is_some();
            }
            // Spec: piano-roll R37. The clip markers are the MIDI Note
            // Editor's; the Sample Editor's shortcuts share their keys.
            note_ids::SET_MARKER => {
                return self
                    .clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .is_some();
            }
            // Spec: piano-roll R39. The …Time commands need a span: a time
            // selection, or the call's own.
            note_ids::DUPLICATE_TIME | note_ids::INSERT_TIME | note_ids::DELETE_TIME => {
                return args.get("start").is_some_and(|start| !start.is_null())
                    || self
                        .clip_view
                        .time_selection(self.editor.document(), &self.arrangement)
                        .is_some();
            }
            // Spec: mpe R27, R29, R31. The tab's own keys work in the tab.
            // Cmd-A takes the breakpoints only while an envelope has the
            // keyboard, and selects the notes otherwise.
            ids::VIEW_MPE_GRID | note_ids::MPE_ZOOM | note_ids::SELECT_EXPRESSION_BREAKPOINTS => {
                let clip = self
                    .clip_view
                    .shown_clip(self.editor.document(), &self.arrangement);
                return self.clip_view.tab() == ClipTab::Mpe
                    && clip.is_some_and(|clip| {
                        id != note_ids::SELECT_EXPRESSION_BREAKPOINTS
                            || args.get("envelope").is_some()
                            || self.clip_view.selected_breakpoints(clip).is_some()
                    });
            }
            // Zoom In and Zoom Out stop at the zoom's ends (interface-zoom
            // R3).
            ids::VIEW_ZOOM_IN | ids::VIEW_ZOOM_OUT => {
                // Spec: interface-zoom R13. The ends are those of the window
                // the zoom acts on.
                let window = self.zoom_window(match args["window"].as_str() {
                    Some("main") => Some(ids::WindowName::Main),
                    Some("second") => Some(ids::WindowName::Second),
                    _ => None,
                });
                let percent = self.zoom_percent_of(window);
                return if id == ids::VIEW_ZOOM_IN {
                    percent < ZOOM_MAX_PERCENT
                } else {
                    percent > ZOOM_MIN_PERCENT
                };
            }
            // A person's Zoom setting always names a percentage in range, as
            // the Customization settings do (themes R7).
            ids::VIEW_SET_ZOOM
            | ids::SETTINGS_SET_GRID_LINE_INTENSITY
            | ids::SETTINGS_SET_BRIGHTNESS
            | ids::SETTINGS_SET_COLOR_INTENSITY
            | ids::SETTINGS_SET_COLOR_HUE => {
                return spec
                    .args
                    .as_ref()
                    .is_some_and(|schema| bl_commands::validate(schema, args).is_ok());
            }
            // The frontmost window must be a secondary one: with only the
            // main window open, Close Window is disabled (M-FILE-004).
            // Live's Plug-In Windows is disabled with none open (M-VIEW-028).
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => return self.any_plug_in_window(),
            // IMP-25. Close All Plug-in Windows is disabled with no editor
            // open: it never closes a floating native device window.
            ids::CLOSE_PLUG_IN_WINDOWS => return self.any_plug_in_editor(),
            ids::WINDOW_CLOSE => {
                return match self.focused_window {
                    WindowKind::Settings => self.settings_open,
                    // Closing the floating assistant closes the panel.
                    WindowKind::Assistant => self.assistant_window_open(),
                    // Closing a floating device's window docks it.
                    WindowKind::Device(device) => self
                        .editor
                        .document()
                        .device(device)
                        .is_some_and(|d| d.floating),
                    WindowKind::Second => self.second.open,
                    WindowKind::Main => false,
                };
            }
            _ => {}
        }
        if stems_ids::handles(id) {
            return self.can_stems(id, args);
        }
        if export_ids::handles(id) {
            return self.can_export(id);
        }
        if autosampler_ids::handles(id) {
            return self.can_autosampler(id, args);
        }
        if slice_ids::handles(id) {
            // Spec: slice-to-midi R1. The menus' enabled states: one audio
            // clip selected, in either view.
            return self.can_slice(id, args);
        }
        if id == CUE_MODE {
            // Spec: mixer S10, R24. Solo Switches and Cue Switches, and the
            // Main strip's button, are dimmed while Cue Out and Main Out
            // share a channel.
            return self.editor.document().cue_available();
        }
        if capture_ids::handles(id) {
            // Spec: capture-midi R1. Nothing played, nothing to capture.
            return self.capture_midi_ready() && self.audio.engine_running();
        }
        if id == automation_ids::SIMPLIFY && args.get("lanes").is_none() {
            // Spec: automation-editing R18. Disabled without a time selection
            // in an automation lane, or in the Envelope Editor (clip-envelopes
            // R13).
            return automation::can_run(id, &self.arrangement, None)
                || self.can_clip_envelope_edit(clip_envelope_ids::SIMPLIFY, active);
        }
        if ids::handles(id) || transport_ids::handles(id) {
            return true;
        }
        if id == bl_shell_spec::groove::EXTRACT && args.get("clips").is_none() {
            // Spec: grooves R34. Extract Groove(s) is disabled while no clip
            // is selected.
            return !self.scale_targets().is_empty();
        }
        if id == browser_ids::LOAD_SELECTED
            && (self.devices_load().is_some()
                || self.hot_swapped_layer_file().is_some()
                || self.hot_swap_sample().is_some()
                || self.browser_view.selected_plugin().is_some())
        {
            return true;
        }
        if device_ids::handles(id) {
            let args = Self::object(args.clone());
            let doc = self.editor.document();
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.devices.can_run(
                    id,
                    &args,
                    doc,
                    devices::shown_track(&self.arrangement, doc),
                );
        }
        if track_ids::handles(id) {
            match id {
                track_ids::DUPLICATE_WITHOUT_CLIPS => {
                    // Spec: routing R28. The selected tracks, or those named.
                    let named: Option<Vec<TrackId>> = args
                        .get("tracks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let tracks =
                        named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    return !crate::tracks::duplicated(self.editor.document(), &tracks).is_empty();
                }
                track_ids::GROUP => {
                    let named: Option<Vec<TrackId>> = args
                        .get("tracks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let tracks =
                        named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    return !crate::tracks::groupable(self.editor.document(), &tracks).is_empty();
                }
                track_ids::LINK | track_ids::UNLINK => {
                    let named: Option<Vec<TrackId>> = args
                        .get("tracks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let into = args.get("into").is_some();
                    let tracks =
                        named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    let doc = self.editor.document();
                    let tracks = crate::tracks::linkable(doc, &tracks);
                    return if id == track_ids::LINK {
                        tracks.len() + usize::from(into) >= 2
                    } else {
                        tracks
                            .iter()
                            .any(|t| doc.track(*t).is_some_and(|t| t.link().is_some()))
                    };
                }
                track_ids::SET_DEVICES_ENABLED => {
                    // IMP-25. It works while the selected tracks hold a
                    // top-level device to switch.
                    let named: Option<Vec<TrackId>> = args
                        .get("tracks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let tracks =
                        named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    return tracks.iter().any(|&track| {
                        self.editor
                            .document()
                            .track(track)
                            .is_some_and(|track| track.devices().next().is_some())
                    });
                }
                track_ids::UNGROUP => {
                    let named: Option<TrackId> = args
                        .get("track")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let track =
                        named.or_else(|| self.arrangement.selected_tracks().first().copied());
                    return track
                        .and_then(|t| self.editor.document().track(t))
                        .is_some_and(|t| t.kind() == TrackKind::Group);
                }
                _ => return false,
            }
        }
        if id == "device.rename" && args.get("name").is_none() {
            // A person names the device in its title bar; with a macro or a
            // variation selected, Rename names that instead (macros spec,
            // R14, R29).
            return self.devices.selected().is_some()
                || self.devices.macro_selected().is_some()
                || self.devices.variation_selected().is_some();
        }
        if id == "chain.rename" && args.get("name").is_none() {
            // A person names the chain in its row.
            return self.devices.selected_chains(self.editor.document()).len() == 1;
        }
        if id == "track.delete" && args.get("track").is_none() {
            return !self.deletable_tracks().is_empty();
        }
        if id == "track.rename" && args.get("name").is_none() {
            return self.renamable_track(args).is_some();
        }
        if id == device_ids::SET_FLOATING
            || matches!(id, device_ids::COMPARE_SWITCH | device_ids::COMPARE_COPY)
        {
            // IMP-14: only native devices float; plug-ins open their own
            // editor windows. Spec: device-compare R6: only native devices
            // have A and B states.
            let named = args
                .get("device")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let device = named
                .or(self.devices.selected())
                .and_then(|d| self.editor.document().device(d));
            let fits = if id == device_ids::SET_FLOATING {
                device.is_some_and(bl_model::Device::can_float)
            } else {
                device.is_some_and(bl_model::Device::can_compare)
            };
            if !fits {
                return false;
            }
        }
        if browser_ids::handles(id) {
            // A person adding a folder picks it in a dialog.
            if id == browser_ids::ADD_FOLDER && args.get("path").is_none() {
                return true;
            }
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.browser_view.can_run(id, &args);
        }
        if session_ids::handles(id)
            && let Some(allowed) = self.can_session_view(id, args)
        {
            return allowed;
        }
        if crate::session_clips::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && crate::session_clips::can_run(
                    id,
                    &args,
                    self.editor.document(),
                    &self.session,
                    &self.arrangement,
                    self.clipboard.as_ref(),
                );
        }
        if edit_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                // Spec: time-editing R4. The Delete key deletes the
                // selected locators while there are any.
                && !(id == edit_ids::DELETE
                    && (!self.arrangement.selected_locators().is_empty()
                        || self.arrangement.selected_signature().is_some()
                        // Spec: audio-clips R29. Delete deletes the selected
                        // fade handle's fade instead.
                        || self.arrangement.selected_fade().is_some()))
                && editing::can_run(
                    id,
                    &args,
                    self.editor.document(),
                    &self.arrangement,
                    self.clipboard.as_ref(),
                );
        }
        if engine_transport::handles(id) {
            // The transport plays only while an engine runs on a device. F9
            // names no state: it switches recording (transport R30).
            let keyed = id == "transport.set_recording" && args.get("recording").is_none();
            return self.audio.engine_running()
                && (keyed
                    || engine_transport::command_from_args(id, Self::object(args.clone()))
                        .is_ok());
        }
        if engine_session::handles(id) {
            if let Some(allowed) = self.can_session_view(id, args) {
                return allowed;
            }
            // The Session View launches only while an engine runs on a
            // device, as the transport plays.
            // Back to Arrangement is off in a set no clip has taken over
            // (M-PLAY-012).
            return self.audio.engine_running()
                && (id != "session.back_to_arrangement" || !self.session_over.is_empty())
                && engine_session::command_from_args(id, Self::object(args.clone())).is_ok();
        }
        if session_record_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return self.audio.engine_running()
                && spec
                    .args
                    .as_ref()
                    .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok());
        }
        if session_launch_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && (id != session_launch_ids::STOP_SELECTED
                    || !self.session.selected_slots().is_empty())
                && (id != session_launch_ids::LAUNCH_SELECTED
                    || !self.launch_targets().is_empty());
        }
        if view_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.arrangement.can_run(id, &args, self.editor.document());
        }
        if locator_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            if !spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
            {
                return false;
            }
            return match id {
                locator_ids::DELETE => {
                    args.get("locators").is_some()
                        || !self.arrangement.selected_locators().is_empty()
                }
                // Spec: time-editing R31, R32. Typing a marker's signature
                // and moving it act on the selected marker.
                locator_ids::EDIT_SIGNATURE | locator_ids::MOVE_SIGNATURE => {
                    args.get("at").is_some() || self.arrangement.selected_signature().is_some()
                }
                // Spec: time-editing R13. The jumps need the engine.
                locator_ids::JUMP | locator_ids::JUMP_PREVIOUS | locator_ids::JUMP_NEXT => {
                    self.audio.engine_running()
                }
                _ => true,
            };
        }
        if mixer_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok());
        }
        if id == bl_shell_spec::warping::QUANTIZE_AUDIO {
            if self.is_sample_key(id, args) {
                return self.complete_sample_command(id, args.clone()).is_ok();
            }
            let named: Option<bl_model::ClipId> = args
                .get("clip")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            if let Some(clip) = named {
                let doc = self.editor.document();
                return match doc.clip(clip).map(|c| &c.data) {
                    Some(ClipData::Audio(audio)) => audio.warp,
                    _ => false,
                };
            }
        }
        if self.is_sample_key(id, args) {
            return self.complete_sample_command(id, args.clone()).is_ok();
        }
        let Ok(args) = self.filled_args(id, args.clone()) else {
            return false;
        };
        spec.args
            .as_ref()
            .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
    }
}
