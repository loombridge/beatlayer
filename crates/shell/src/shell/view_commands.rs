//! The views: what is showing, the zoom, and the panels the View menu opens.

use super::plugin_list;
use super::{MainView, Shell, to_value};
use crate::arrangement::ArrangementView;
use crate::arrangement::TransportView;
use crate::arrangement::Viewport;
use crate::clip_view::ClipView;
use crate::commands::ViewResult;
use crate::detail::DetailArea;
use crate::mixer_view::MixerView;
use crate::session_view::SessionView;
use crate::settings::ZOOM_MAX_PERCENT;
use crate::settings::ZOOM_MIN_PERCENT;
use crate::settings::zoom_factor;
use crate::settings::zoom_percent;
use bl_shell_spec::plugins as plugin_ids;
use serde_json::Value;

use super::WindowKind;
use super::{Area, Effect, ShellError};
use crate::arrangement::ViewError;
use crate::assistant as assistant_ids;
use crate::commands as ids;
use crate::commands::ClipEnvelopeArgs;
use crate::commands::ClipTabArgs;
use crate::commands::LaneArgs;
use crate::commands::LanesArgs;
use crate::commands::MpeGridArgs;
use crate::commands::MpeLanesArgs;
use crate::commands::SwitchResult;
use crate::commands::ViewState;
use crate::commands::{FoldNotesArgs, HighlightScalesArgs, NoteSpellingArgs};
use bl_commands::NoArgs;
use bl_model::AutomationTarget;
use bl_model::ClipData;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::notes::{ClipTab, RandomizeAmounts};

impl Shell {
    /// Runs the chord lane's view commands (WP-6.48; the chord lane spec, R1,
    /// R3): showing the lane and selecting a region. Neither is an undo step.
    pub(super) fn run_chord_lane(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        use bl_shell_spec::chord_lane::{self as chord_ids, ChordLaneArgs, SelectRegionArgs};
        match id {
            chord_ids::VIEW_CHORD_LANE => {
                let ChordLaneArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.arrangement.chord_lane());
                self.arrangement.set_chord_lane(on);
                self.sync_window_views();
            }
            _ => {
                let SelectRegionArgs { region } = Self::args(id, args)?;
                if let Some(region) = region
                    && self.editor.document().chord_region(region).is_none()
                {
                    return Err(ShellError::Failed {
                        command: id.to_owned(),
                        message: format!("the chord lane holds no {region}"),
                    });
                }
                self.arrangement.select_chord_region(region);
            }
        }
        self.touch();
        Ok(to_value(&self.arrangement.state(self.editor.document())))
    }

    /// Whether the Undo History panel shows (WP-6.28).
    #[must_use]
    pub fn undo_history_open(&self) -> bool {
        self.undo_history_open
    }

    /// The Undo History panel's state, for the command's result.
    #[must_use]
    pub fn undo_history_state(&self) -> bl_shell_spec::commands::UndoHistoryState {
        let editor = self.editor();
        bl_shell_spec::commands::UndoHistoryState {
            open: self.undo_history_open,
            steps: editor.undo_history().len() + editor.redo_history().len(),
            current: editor.current_index(),
        }
    }

    /// The history's rows, from the top: the steps that can be redone
    /// first, then the ones that can be undone.
    #[must_use]
    pub fn undo_history_rows(&self) -> Vec<bl_model::HistoryStep> {
        self.editor.history()
    }

    /// The main view in front.
    #[must_use]
    pub fn main_view(&self) -> MainView {
        self.main_view
    }

    /// Whether the browser shows.
    #[must_use]
    pub fn browser_visible(&self) -> bool {
        self.browser
    }

    /// Whether the Info View shows (L-INFO-001): view state only.
    #[must_use]
    pub fn info_view_visible(&self) -> bool {
        self.info_view
    }

    /// Whether the main window is in full-screen mode.
    #[must_use]
    pub fn full_screen(&self) -> bool {
        self.full_screen
    }

    /// The interface zoom, where 1 is 100%: a whole percentage from 50% to
    /// 200% (WP-1.17). A preferences file with anything else starts at the
    /// nearest limit, or at 100% if it isn't a number (interface-zoom R6).
    #[must_use]
    pub fn zoom(&self) -> f32 {
        zoom_factor(self.zoom_percent())
    }

    /// The interface zoom as a whole percentage, as the Display & Input
    /// page's Zoom shows it.
    #[must_use]
    pub fn zoom_percent(&self) -> u16 {
        zoom_percent(self.prefs.zoom.unwrap_or(1.0))
    }

    /// Sets the interface zoom and keeps it in the preferences: none for
    /// 100%, so a new install's file has no zoom in it.
    pub(super) fn set_zoom_percent(&mut self, percent: u16) {
        let percent = percent.clamp(ZOOM_MIN_PERCENT, ZOOM_MAX_PERCENT);
        let zoom = (percent != 100).then(|| zoom_factor(percent));
        if self.prefs.zoom != zoom {
            self.prefs.zoom = zoom;
            self.save_preferences();
        }
    }

    /// What the Arrangement View shows (WP-1.8a).
    #[must_use]
    pub fn arrangement(&self) -> &ArrangementView {
        &self.arrangement
    }

    /// What the Clip View shows (WP-2.3).
    #[must_use]
    pub fn clip_view(&self) -> &ClipView {
        &self.clip_view
    }

    /// Which of the Clip View and the Device View show, and where the
    /// detail area's focus is (WP-6.27).
    #[must_use]
    pub fn detail(&self) -> &DetailArea {
        &self.detail
    }

    /// The list of the set's plug-ins (IMP-29, WP-6.41).
    #[must_use]
    pub fn plugin_list(&self) -> &plugin_list::PluginList {
        &self.plugin_list
    }

    /// The set's plug-in list, as `plugin.list` reports it.
    #[must_use]
    pub fn set_plugin_list(&self) -> plugin_ids::SetPluginList {
        crate::plugins::set_plugin_list(self.editor.document(), &self.scanner)
    }

    /// What the mixers show (WP-1.9).
    #[must_use]
    pub fn mixer_view(&self) -> &MixerView {
        &self.mixer
    }

    /// What the Session View grid has selected and how far it's scrolled
    /// (WP-3.7).
    #[must_use]
    pub fn session(&self) -> &SessionView {
        &self.session
    }

    /// Records the size of the Arrangement's scrolling lanes, as the main
    /// window lays them out, so its view commands (zooming around the
    /// middle, Optimize Height and Width, Follow) work with what's on
    /// screen.
    pub fn set_arrangement_viewport(&mut self, width: f32, height: f32) {
        if self
            .arrangement
            .set_viewport(Viewport { width, height }, self.editor.document())
        {
            self.touch();
        }
    }

    /// Takes the transport's latest state (the playhead, whether it plays,
    /// the loop brace), for the Arrangement View to draw and, with Follow
    /// on, to keep in view. [`Shell::poll_engine`] calls it with the
    /// engine's.
    pub fn set_transport(&mut self, transport: TransportView) {
        if self.arrangement.set_transport(transport) {
            self.touch();
        }
    }

    /// Whether the browser's Tuning section shows (the tunings spec, R28).
    #[must_use]
    pub fn tuning_section_shown(&self) -> bool {
        self.tuning_section
    }

    pub(super) fn view_window(&self) -> ViewResult {
        self.view_result_of(WindowKind::Main)
    }

    pub(super) fn view_result(&self) -> Value {
        to_value(&self.view_window())
    }

    /// Records that the window left full-screen mode by other means, such
    /// as the window's own button.
    pub fn set_full_screen_state(&mut self, on: bool) {
        if self.full_screen != on {
            self.full_screen = on;
            self.touch();
        }
    }

    /// Runs the `view` commands that show or hide something.
    pub(super) fn run_view(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => {
                // Spec: device-chains R33. It hides and shows windows only.
                let ids::TogglePlugInWindowsArgs { shown } = Self::args(id, args)?;
                if !self.any_plug_in_window() {
                    return Err(unavailable("no window to show or hide"));
                }
                self.plug_in_windows = shown.unwrap_or(!self.plug_in_windows);
                self.apply_plug_in_windows();
                Ok(to_value(&SwitchResult {
                    on: self.plug_in_windows,
                }))
            }
            ids::CLOSE_PLUG_IN_WINDOWS => {
                // Spec: device-chains R40. It closes editors only, so it
                // changes nothing in the set and is never an undo step.
                Self::args::<NoArgs>(id, args)?;
                let closed = self.close_plug_in_windows();
                self.touch();
                Ok(to_value(&ids::ClosedPlugInWindows { closed }))
            }
            ids::VIEW_TOGGLE_FULL_HEIGHT_BROWSER => {
                // Spec: browser R26. The column runs down to the Status Bar;
                // kept between launches.
                Self::args::<NoArgs>(id, args)?;
                let on = !self.browser_view.full_height();
                self.browser_view.set_full_height(on);
                self.prefs.browser = self
                    .browser_view
                    .prefs(self.prefs.browser.user_library.clone());
                self.save_preferences();
                Ok(self.view_result())
            }
            ids::VIEW_SET_BROWSER_WIDTH => {
                // Spec: browser R29. Both windows' browsers take the width,
                // kept between launches; no undo step.
                let ids::BrowserWidthArgs { points } = Self::args(id, args)?;
                if !points.is_finite() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "`points` must be a number".to_owned(),
                    });
                }
                self.browser_view.set_width(points as f32);
                self.prefs.browser = self
                    .browser_view
                    .prefs(self.prefs.browser.user_library.clone());
                self.save_preferences();
                self.touch();
                Ok(self.view_result())
            }
            ids::VIEW_TOGGLE_PLUGIN_LIST => {
                // Spec: plugin-list R1, R3. View state only.
                Self::args::<NoArgs>(id, args)?;
                self.plugin_list.toggle();
                // Spec: file-management R10. The File Manager and the list of
                // the set's plug-ins take the same place: showing either
                // hides the other.
                if self.plugin_list.visible() {
                    self.file_manager.open = false;
                }
                self.touch();
                Ok(to_value(&self.plugin_list.state()))
            }
            ids::VIEW_SET_PLUGIN_LIST_ONLY_MISSING => {
                // Spec: plugin-list R3, R10. View state only.
                let ids::PluginListOnlyMissingArgs { only_missing } = Self::args(id, args)?;
                self.plugin_list
                    .set_only_missing(only_missing.unwrap_or(!self.plugin_list.only_missing()));
                self.touch();
                Ok(to_value(&self.plugin_list.state()))
            }
            ids::VIEW_DRAW_MODE => {
                // Spec: piano-roll R2; S-GRID-001, M-OPT-017. View state only.
                Self::args::<NoArgs>(id, args)?;
                self.clip_view.toggle_draw_mode();
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_FOLD_NOTES | ids::VIEW_FOLD_SCALE => {
                // Spec: piano-roll; M-VIEW-039, S-NOTE-038; scales R23,
                // M-VIEW-040, S-NOTE-039. View state only, saved with the
                // set.
                let scale = id == ids::VIEW_FOLD_SCALE;
                let FoldNotesArgs { clip, folded } = Self::args(id, args)?;
                let args_clip_named = clip.is_some();
                let clip = match clip {
                    Some(clip) => clip,
                    None => self
                        .clip_view
                        .shown_clip(self.editor.document(), &self.arrangement)
                        .ok_or_else(|| unavailable("no MIDI clip is shown in the Clip View"))?,
                };
                match self.editor.document().clip(clip) {
                    None => {
                        return Err(failed(format!("no clip has the ID {}", clip.0)));
                    }
                    Some(value) if !matches!(value.data, ClipData::Midi(_)) => {
                        return Err(failed("the clip isn't a MIDI clip".to_owned()));
                    }
                    Some(value) if scale && !value.scale.enabled => {
                        return Err(unavailable("the clip's Scale Mode is off"));
                    }
                    Some(_) => {}
                }
                let is_folded = |view: &ClipView, clip| {
                    if scale {
                        view.is_scale_folded(clip)
                    } else {
                        view.is_folded(clip)
                    }
                };
                let folded = folded.unwrap_or(!is_folded(&self.clip_view, clip));
                let mut clips = vec![clip];
                if !args_clip_named {
                    clips.extend(
                        self.clip_view
                            .shown(self.editor.document(), &self.arrangement),
                    );
                }
                for clip in clips {
                    let doc = self.editor.document();
                    if scale && doc.clip(clip).is_none_or(|value| !value.scale.enabled) {
                        continue;
                    }
                    if scale {
                        self.clip_view.set_scale_folded(clip, folded);
                    } else {
                        self.clip_view.set_folded(clip, folded);
                    }
                }
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_HIGHLIGHT_SCALES => {
                // Spec: scales R22; M-OPT-019, S-NOTE-035. A preference.
                let HighlightScalesArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.clip_view.highlight_scales());
                self.clip_view.set_highlight_scales(on);
                self.prefs.highlight_scales = on;
                self.save_preferences();
                self.touch();
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_NOTE_SPELLING => {
                // Spec: scales R24; C-KEYS-002. A preference.
                let NoteSpellingArgs { spelling } = Self::args(id, args)?;
                self.clip_view.set_note_spelling(spelling);
                self.prefs.note_spelling = spelling;
                self.save_preferences();
                self.touch();
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_CLIP_TAB => {
                // Spec: piano-roll R1; S-CLIPMODE-001 to 004. View state only.
                let ClipTabArgs { tab } = Self::args(id, args)?;
                // Spec: mpe R22. An audio clip has no MPE tab: it cycles
                // through the other two.
                let audio = self
                    .clip_view
                    .shown_audio_clip(self.editor.document(), &self.arrangement)
                    .is_some();
                if audio && tab == Some(ClipTab::Mpe) {
                    return Err(unavailable("an audio clip has no MPE tab"));
                }
                let next = match self.clip_view.tab().next() {
                    ClipTab::Mpe if audio => ClipTab::Notes,
                    next => next,
                };
                self.clip_view.set_tab(tab.unwrap_or(next));
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_CLIP_ENVELOPE => {
                // Spec: clip-envelopes R8. View state only.
                let ClipEnvelopeArgs { target } = Self::args(id, args)?;
                if let Some(target) = target {
                    let doc = self.editor.document();
                    let clip = self
                        .clip_view
                        .shown_any_clip(doc, &self.arrangement)
                        .ok_or_else(|| unavailable("no clip is shown in the Clip View"))?;
                    let track = doc
                        .clip_track(clip)
                        .ok_or_else(|| failed("no track holds the clip".to_owned()))?;
                    let shown = doc.clip(clip).ok_or_else(|| failed(format!("no {clip}")))?;
                    doc.check_clip_target(track, shown, target)
                        .map_err(|error| failed(error.to_string()))?;
                    // Spec: clap-hosting R5 (WP-5.40). Choosing a plug-in
                    // parameter in the clip-envelope choosers makes its
                    // temporary entry permanent.
                    if let AutomationTarget::Device { device, param } = target {
                        self.configure_on_use(device, param);
                    }
                }
                self.clip_view.set_envelope(target);
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_LANE => {
                // Spec: piano-roll; the lane shows one of a note's values.
                let LaneArgs { lane } = Self::args(id, args)?;
                self.clip_view.set_lane(lane);
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_LANES => {
                // Spec: note-editing R1, R2, R13; the lanes under the note
                // editor and their Randomize amounts.
                let LanesArgs {
                    lane,
                    show,
                    swap,
                    amount,
                    all,
                } = Self::args(id, args)?;
                if lane.is_none() && (show.is_some() || swap.is_some() || amount.is_some()) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "`show`, `swap` and `amount` need a `lane`".to_owned(),
                    });
                }
                if let Some(lane) = lane {
                    if let Some(amount) = amount {
                        let limit = RandomizeAmounts::limit(lane);
                        if amount > limit {
                            return Err(ShellError::InvalidArgs {
                                command: id.to_owned(),
                                message: format!("`amount` must be from 0 to {limit}"),
                            });
                        }
                        self.clip_view.set_randomize_amount(lane, amount);
                    }
                    if swap == Some(true) {
                        self.clip_view.swap_lane(lane);
                    } else if let Some(on) = show {
                        self.clip_view.show_lane(lane, on);
                    } else if amount.is_none() && swap.is_none() {
                        let on = !self.clip_view.lanes().is_shown(lane);
                        self.clip_view.show_lane(lane, on);
                    }
                }
                if let Some(on) = all {
                    self.clip_view.show_all_lanes(on);
                }
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_MPE_LANES => {
                // Spec: mpe R23. View state saved with the set; no undo step.
                let MpeLanesArgs {
                    lane,
                    show,
                    swap,
                    height,
                    visible,
                    all,
                } = Self::args(id, args)?;
                if lane.is_none() && (show.is_some() || swap.is_some() || height.is_some()) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "`show`, `swap` and `height` need a `lane`".to_owned(),
                    });
                }
                if height.is_some_and(|h| !(8.0..=400.0).contains(&h)) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "`height` must be from 8 to 400 points".to_owned(),
                    });
                }
                if lane == Some(bl_model::MpeLane::PitchBend) && (show.is_some() || swap.is_some())
                {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the Pitch Bend lane has no toggle: it shows first on a Drum \
                                  Rack's track"
                            .to_owned(),
                    });
                }
                let mpe = self.clip_view.mpe_mut();
                if let Some(lane) = lane {
                    if let Some(height) = height {
                        mpe.heights.insert(lane, height);
                    }
                    if swap == Some(true) {
                        mpe.swap(lane);
                    } else if let Some(on) = show {
                        mpe.show(lane, on);
                    } else if height.is_none() {
                        let on = !mpe.shown.contains(&lane);
                        mpe.show(lane, on);
                    }
                }
                if let Some(visible) = visible {
                    mpe.hidden = !visible;
                }
                if all == Some(true) {
                    mpe.show_all();
                }
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_MPE_GRID => {
                // Spec: mpe R27. The tab's own grid, saved with the set.
                let MpeGridArgs { clip, on } = Self::args(id, args)?;
                let doc = self.editor.document();
                let clip = clip
                    .or_else(|| self.clip_view.shown_clip(doc, &self.arrangement))
                    .ok_or_else(|| unavailable("no MIDI clip is shown in the Clip View"))?;
                if !doc
                    .clip(clip)
                    .is_some_and(|c| matches!(c.data, ClipData::Midi(_)))
                {
                    return Err(unavailable("the clip isn't a MIDI clip"));
                }
                let mpe = self.clip_view.mpe_mut();
                let on = on.unwrap_or(!mpe.grid_on(clip));
                mpe.set_grid(clip, on);
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_STATE => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&ViewState {
                    window: self.view_window(),
                    second_window: self.second_window_state(),
                    focused_window: if self.acting_window() == WindowKind::Second {
                        ids::WindowName::Second
                    } else {
                        ids::WindowName::Main
                    },
                    clip_view: self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                    detail: self.detail.state(),
                    plugin_list: self.plugin_list.state(),
                    assistant: self.assistant_panel_state(),
                    undo_history: self.undo_history_open,
                    save_history: self.save_history.open,
                    plug_in_windows: self.plug_in_windows,
                }))
            }
            ids::VIEW_TOGGLE_UNDO_HISTORY => {
                // Spec: undo-history R1, R3. The panel is view state: it
                // isn't saved with the set and isn't an undo step.
                Self::args::<NoArgs>(id, args)?;
                self.undo_history_open = !self.undo_history_open;
                if self.undo_history_open {
                    // They share the right-hand area (crash-recovery R12).
                    self.save_history.open = false;
                }
                self.touch();
                Ok(to_value(&self.undo_history_state()))
            }
            ids::VIEW_TOGGLE_SAVE_HISTORY => {
                // Spec: crash-recovery R12. View state: not saved with the
                // set and no undo step.
                Self::args::<NoArgs>(id, args)?;
                Ok(self.toggle_save_history())
            }
            ids::VIEW_TOGGLE_INFO_VIEW => {
                Self::args::<NoArgs>(id, args)?;
                self.info_view = !self.info_view;
                Ok(self.view_result())
            }
            ids::VIEW_TUNING_SECTION => {
                // Spec: tunings R28. View state: not saved with the set and
                // no undo step.
                Self::args::<NoArgs>(id, args)?;
                self.tuning_section = !self.tuning_section;
                Ok(self.view_result())
            }
            // Spec: interface-zoom R3, R5, R6.
            ids::VIEW_TOGGLE_SECOND_WINDOW
            | ids::VIEW_FULL_SCREEN
            | ids::VIEW_TOGGLE_BROWSER
            | ids::VIEW_TOGGLE_CLIP_VIEW
            | ids::VIEW_TOGGLE_DEVICE_VIEW
            | ids::VIEW_SHOW_DETAIL_VIEWS
            | ids::VIEW_SWITCH_DETAIL_VIEW
            | ids::VIEW_TOGGLE_DETAIL_AREA
            | ids::VIEW_EXPAND_CLIP_VIEW
            | ids::VIEW_SET_DETAIL_HEIGHT
            | ids::FOCUS_CLIP_VIEW
            | ids::FOCUS_DEVICE_VIEW
            | ids::VIEW_TOGGLE_SESSION_ARRANGEMENT
            | ids::VIEW_ZOOM_IN
            | ids::VIEW_ZOOM_OUT
            | ids::VIEW_ZOOM_RESET
            | ids::VIEW_SET_ZOOM
            | ids::FOCUS_CONTROL_BAR
            | ids::FOCUS_ARRANGEMENT => self.run_window_view(id, args),
            assistant_ids::VIEW_TOGGLE_ASSISTANT => {
                Self::args::<NoArgs>(id, args)?;
                self.assistant_open = !self.assistant_open;
                if self.assistant_open && !self.prefs.assistant_panel.floating {
                    // The keyboard goes to the pane's prompt field, as
                    // the browser's focus command takes it into the browser.
                    self.focused_area = Area::Assistant;
                    self.effects.push(Effect::FocusArea(Area::Assistant));
                }
                self.leave_hidden_assistant();
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Runs the Arrangement View's view commands.
    pub(super) fn run_arrangement_view(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        // Spec: clap-hosting R5 (WP-5.40). Choosing a plug-in parameter
        // in the automation choosers makes its temporary entry permanent.
        let chosen = (id == view_ids::SHOW_AUTOMATION)
            .then(|| {
                Self::args::<view_ids::ShowAutomationArgs>(id, args.clone())
                    .ok()
                    .and_then(|a| a.target)
            })
            .flatten()
            .and_then(|target| match target {
                AutomationTarget::Device { device, param } => Some((device, param)),
                _ => None,
            });
        let to_error = |error: ViewError| match error {
            ViewError::InvalidArgs(message) => ShellError::InvalidArgs {
                command: id.to_owned(),
                message,
            },
            ViewError::Unavailable(reason) => ShellError::Unavailable {
                command: id.to_owned(),
                reason,
            },
        };
        // Spec: automation R25. Adding a lane needs the choosers' order,
        // which the devices give.
        let result = if id == view_ids::ADD_AUTOMATION_LANE {
            Self::args::<view_ids::AddAutomationLaneArgs>(id, args).and_then(|args| {
                let doc = self.editor.document();
                let order =
                    crate::arrangement::lanes::chooser_order(doc, &self.devices, args.track);
                self.arrangement
                    .add_lane(doc, &order, args)
                    .map_err(to_error)?;
                Ok(self.arrangement.state(doc))
            })
        } else {
            self.arrangement
                .run(id, args, self.editor.document())
                .map_err(to_error)
        };
        if id != view_ids::STATE {
            // A query changes nothing a window shows.
            self.touch();
        }
        if result.is_ok()
            && id == view_ids::SELECT_TAKE_LANES
            && (self.detail.clip_shown() || self.detail.device_shown())
        {
            // Spec: comping R26. With the Clip View or the Device View
            // open, selecting a take lane's header selects its clips.
            self.arrangement.select_header_clips(self.editor.document());
        }
        if result.is_ok() && id == view_ids::FOCUS_TITLE_BAR {
            // Spec: accessibility R4. Esc from a track's control gives
            // the keyboard back to the Arrangement, at the title bar.
            self.focus_area(Area::MainView);
        }
        if result.is_ok() && matches!(id, view_ids::SET_INSERT_MARKER | view_ids::SELECT_TIME) {
            // Spec: control-bar-transport R15. A click or a time
            // selection in the Arrangement moves the engine's insert
            // marker, where Play starts, and playback carries on.
            let at = self.arrangement.insert_marker();
            self.audio.set_insert_marker(at);
        }
        if result.is_ok()
            && let Some((device, param)) = chosen
        {
            self.configure_on_use(device, param);
        }
        result.map(|state| to_value(&state))
    }
}
