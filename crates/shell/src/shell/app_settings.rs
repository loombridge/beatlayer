//! Preferences, appearance and the Settings window.

use super::{Effect, Shell, WindowKind};
use crate::agent::AgentServerState;
use crate::settings::Appearance;
use crate::settings::Preferences;
use crate::settings::SettingsPage;
use crate::settings::ThemeLook;
use crate::settings::WarpPrefs;
use bl_shell_spec::settings::AgentsPageSettings;
use bl_shell_spec::settings::AudioPageSettings;
use bl_shell_spec::settings::DisplayInputSettings;
use bl_shell_spec::settings::FileFolderSettings;
use bl_shell_spec::settings::LaunchPrefs;
use bl_shell_spec::settings::LibrarySettings;
use bl_shell_spec::settings::SettingsState;
use bl_shell_spec::settings::ThemeColorsSettings;
use std::path::PathBuf;

use super::{ShellError, to_value};
use crate::browser;
use crate::commands as ids;
use crate::commands::AppearanceArgs;
use crate::commands::AutomationRecordModeArgs;
use crate::commands::BrightnessArgs;
use crate::commands::CollectFilesArgs;
use crate::commands::DefaultLaunchModeArgs;
use crate::commands::DefaultLaunchQuantizationArgs;
use crate::commands::DefaultWarpModeArgs;
use crate::commands::HueArgs;
use crate::commands::LoopWarpShortSamplesArgs;
use crate::commands::PercentArgs;
use crate::commands::RecordSessionAutomationInArgs;
use crate::commands::SampleEditorArgs;
use crate::commands::SampleEditorResult;
use crate::commands::SwitchArgs;
use crate::commands::SwitchResult;
use crate::commands::TemporaryFolderArgs;
use crate::commands::TemporaryFolderResult;
use crate::commands::ThemeArgs;
use crate::commands::UserLibraryArgs;
use crate::commands::UserLibraryResult;
use crate::commands::{ClipColourArgs, SwatchArgs, ToneArgs};
use crate::settings::OnByDefault;
use bl_commands::NoArgs;
use serde_json::Value;

impl Shell {
    /// The preferences.
    #[must_use]
    pub fn preferences(&self) -> &Preferences {
        &self.prefs
    }

    /// Whether the Settings window is open.
    #[must_use]
    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    /// The Settings page showing, or shown last.
    #[must_use]
    pub fn settings_page(&self) -> SettingsPage {
        self.prefs.settings_page
    }

    /// Shows a Settings page, as clicking it in the page chooser does.
    pub fn show_settings_page(&mut self, page: SettingsPage) {
        if self.prefs.settings_page != page {
            self.prefs.settings_page = page;
            self.save_preferences();
            self.touch();
        }
    }

    /// Record Session automation in (Record, Warp & Launch; WP-3.12): which
    /// tracks a Session recording writes control moves into the clips of.
    #[must_use]
    pub fn record_session_automation_in(&self) -> crate::settings::SessionAutomationTracks {
        self.prefs.record_session_automation_in
    }

    /// The mode an Arrangement recording writes control moves in (the
    /// automation spec, R18): Touch, Latch or Write.
    #[must_use]
    pub fn automation_record_mode(&self) -> crate::settings::AutomationRecordMode {
        self.prefs.automation_record_mode
    }

    /// Whether Warp recorded audio is on (IMP-02). Clips have no warp fields
    /// yet, so recorded clips play unwarped either way.
    #[must_use]
    pub fn warp_recorded_audio(&self) -> bool {
        self.prefs.warp_recorded_audio.0
    }

    /// Whether Create Fades on Clip Edges is on: new audio clips get 4 ms
    /// fades at their edges.
    #[must_use]
    pub fn create_fades_on_clip_edges(&self) -> bool {
        self.prefs.create_fades_on_clip_edges.0
    }

    /// Whether Retune Set On Loading Tuning Systems is on (the tunings spec,
    /// R15).
    #[must_use]
    pub fn retune_on_tuning_load(&self) -> bool {
        self.prefs.retune_on_tuning_load
    }

    /// Whether Snap Automation to Grid is on (the automation spec, R31).
    #[must_use]
    pub fn snap_automation_to_grid(&self) -> bool {
        self.prefs.snap_automation_to_grid.0
    }

    /// Loop/Warp Short Samples, Auto-Warp Long Samples and Default Warp
    /// Mode (WP-4.9).
    #[must_use]
    pub fn warp_prefs(&self) -> WarpPrefs {
        self.prefs.warp
    }

    /// The Launch section of Record, Warp & Launch (WP-3.4).
    #[must_use]
    pub fn launch_prefs(&self) -> LaunchPrefs {
        self.prefs.launch
    }

    /// What `settings.state` reports: the Settings window's preferences by
    /// page.
    pub(super) fn settings_state(&mut self) -> SettingsState {
        self.refresh_midi();
        let agent = &self.prefs.agent;
        let server = AgentServerState::from(agent);
        SettingsState {
            page: self.prefs.settings_page,
            display_input: DisplayInputSettings {
                zoom_percent: self.zoom_percent(),
                second_window_zoom_percent: self
                    .second
                    .open
                    .then(|| self.zoom_percent_of(WindowKind::Second)),
                outline_view_in_focus: self.outline_view_in_focus(),
                use_tab_key_to_move_focus: self.tab_navigation(),
                wrap_tab_navigation: self.wrap_navigation(),
            },
            theme_colors: ThemeColorsSettings {
                look: self.prefs.look.clone(),
                appearance: self.prefs.appearance,
                drawn_appearance: if self.theme_dark() {
                    Appearance::Dark
                } else {
                    Appearance::Light
                },
                themes: self.themes.clone(),
            },
            audio: AudioPageSettings {
                device: self.audio.settings().clone(),
                tone: self.audio.tone(),
            },
            tempo_midi: self.midi.report(),
            create_fades_on_clip_edges: self.create_fades_on_clip_edges(),
            retune_on_tuning_load: self.retune_on_tuning_load(),
            snap_automation_to_grid: self.snap_automation_to_grid(),
            warp_recorded_audio: self.warp_recorded_audio(),
            warp: self.prefs.warp,
            file_folder: FileFolderSettings {
                temporary_folder: self.temporary_folder().to_string_lossy().into_owned(),
                sample_editor: self.sample_editor().unwrap_or_default(),
                save_continuously: self.save_continuously(),
            },
            launch: self.prefs.launch,
            record_session_automation_in: self.prefs.record_session_automation_in,
            library: LibrarySettings {
                collect_files_on_export: self.prefs.collect_files_on_export,
                user_library: self
                    .browser_view
                    .user_library()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            automation_record_mode: self.prefs.automation_record_mode,
            auto_hide_plug_in_windows: self.auto_hide_plug_in_windows(),
            plug_ins: self.plugin_settings(),
            agents: AgentsPageSettings {
                server: server.server,
                port: agent.port,
                url: server.url,
                token_stored: !agent.token.is_empty(),
                clients: server.clients,
            },
            assistant: self.assistant_state(),
        }
    }

    /// The Appearance chosen (themes R3).
    #[must_use]
    pub fn appearance(&self) -> Appearance {
        self.prefs.appearance
    }

    /// The theme, tone, high contrast and Customization chosen (themes R1).
    #[must_use]
    pub fn look(&self) -> &ThemeLook {
        &self.prefs.look
    }

    /// Tells the shell the 14 track colours of the theme being drawn, so
    /// that a DAWproject file's colours are placed on the palette the person
    /// sees (the DAWproject spec, R23).
    pub fn set_track_palette(&mut self, colours: Vec<[u8; 3]>) {
        if !colours.is_empty() {
            self.track_palette = colours;
        }
    }

    /// Whether the dark colours are drawn now: Follow System resolved
    /// (themes R3).
    #[must_use]
    pub fn theme_dark(&self) -> bool {
        match self.prefs.appearance {
            Appearance::Light => false,
            Appearance::Dark => true,
            Appearance::FollowSystem => self.system_dark,
        }
    }

    /// Tells the shell the operating system's mode changed; the host calls
    /// it at start and on winit's theme event (themes R3).
    pub fn set_system_dark(&mut self, dark: bool) {
        self.system_dark = dark;
    }

    /// The User Library's `Themes` folder, where theme files are read
    /// from (themes R9), if the User Library is known.
    #[must_use]
    pub fn themes_folder(&self) -> Option<PathBuf> {
        self.browser_view
            .user_library()
            .map(|library| library.join("Themes"))
    }

    /// The themes the Theme chooser offers, Beatlayer first.
    #[must_use]
    pub fn themes(&self) -> &[String] {
        &self.themes
    }

    /// Replaces the list of themes the chooser offers (themes R2); the
    /// host calls it after reading the theme files.
    pub fn set_themes(&mut self, names: Vec<String>) {
        self.themes = names;
        self.touch();
    }

    /// Whether Tab moves the keyboard focus in the main window.
    #[must_use]
    pub fn tab_navigation(&self) -> bool {
        self.prefs.tab_navigation
    }

    /// Whether Tab and Shift-Tab wrap round at the ends of the focused area
    /// (Wrap Tab Navigation; the accessibility spec, R5).
    #[must_use]
    pub fn wrap_navigation(&self) -> bool {
        self.prefs.wrap_navigation.0
    }

    /// Whether a border is drawn round the focused area (Outline View in
    /// Focus; the accessibility spec, R7).
    #[must_use]
    pub fn outline_view_in_focus(&self) -> bool {
        self.prefs.outline_view_in_focus.0
    }

    /// Uses `path` as the browser's User Library, as the app does at
    /// start-up with the preference or the default place (browser R3).
    pub fn set_user_library(&mut self, path: Option<PathBuf>) {
        self.browser_view.set_user_library(path);
        self.touch();
    }

    /// What Collect Files on Export is set to (Settings ▸ Library).
    #[must_use]
    pub fn collect_files_on_export(&self) -> bl_shell_spec::settings::CollectFiles {
        self.prefs.collect_files_on_export
    }

    /// Writes the preferences to the config directory, if there is one, and
    /// the agent server's tokens to their keychain, if one is in use
    /// ([`Shell::use_agent_keychain`]); the file never holds the tokens. A
    /// failure shows in the Status Bar.
    pub fn save_preferences(&mut self) {
        self.store_agent_tokens();
        let Some(dir) = &self.config_dir else {
            return;
        };
        if let Err(error) = self.prefs.save(dir) {
            self.set_message(error, true);
        }
    }

    /// Closes the Settings window, as its close button or Esc does: the test
    /// tone stops, and the focus returns to the main window.
    pub fn close_settings(&mut self) {
        if self.settings_open {
            self.settings_open = false;
            self.audio.set_test_tone(false);
            self.focused_window = WindowKind::Main;
            self.effects.push(Effect::CloseSettings);
            self.touch();
        }
    }

    /// Runs the `settings` commands.
    pub(super) fn run_settings(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            // Spec: accessibility R7.
            ids::SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.outline_view_in_focus = OnByDefault(on);
                self.save_preferences();
                self.touch();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_NEXT_PAGE | ids::SETTINGS_PREVIOUS_PAGE => {
                Self::args::<NoArgs>(id, args)?;
                if !self.settings_open {
                    return Err(unavailable("the Settings window isn't open"));
                }
                let step = if id == ids::SETTINGS_NEXT_PAGE { 1 } else { -1 };
                self.prefs.settings_page = self.prefs.settings_page.step(step);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_THEME => {
                let ThemeArgs { theme } = Self::args(id, args)?;
                match theme.as_str() {
                    // What earlier versions took (themes R12).
                    "dark" => self.prefs.appearance = Appearance::Dark,
                    "light" => self.prefs.appearance = Appearance::Light,
                    name if self.themes.iter().any(|t| t == name) => {
                        self.prefs.look.theme = theme;
                    }
                    _ => {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: format!(
                                "there is no theme `{theme}`; the themes are {}",
                                self.themes.join(", ")
                            ),
                        });
                    }
                }
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_APPEARANCE => {
                let AppearanceArgs { appearance } = Self::args(id, args)?;
                self.prefs.appearance = appearance;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_TONE => {
                let ToneArgs { tone } = Self::args(id, args)?;
                self.prefs.look.tone = tone;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_HIGH_CONTRAST => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.look.high_contrast = on;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_GRID_LINE_INTENSITY => {
                let PercentArgs { percent } = Self::args(id, args)?;
                self.prefs.look.grid_line_intensity = percent.clamp(0, 200);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_BRIGHTNESS => {
                let BrightnessArgs { value } = Self::args(id, args)?;
                self.prefs.look.brightness = value.clamp(-50, 50);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_COLOR_INTENSITY => {
                let PercentArgs { percent } = Self::args(id, args)?;
                self.prefs.look.color_intensity = percent.clamp(0, 200);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_COLOR_HUE => {
                let HueArgs { degrees } = Self::args(id, args)?;
                self.prefs.look.color_hue = degrees.clamp(-180, 180);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_AUTO_ASSIGN_TRACK_COLOURS => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.look.auto_assign_track_colours = on;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_DEFAULT_TRACK_COLOUR => {
                let SwatchArgs { colour } = Self::args(id, args)?;
                if colour >= bl_model::SWATCHES {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("swatch {colour} isn't in the palette, which has 0 to 69"),
                    });
                }
                self.prefs.look.default_track_colour = colour;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_REDUCED_AUTOMATIC_COLOURS => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.look.reduced_automatic_colours = on;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_CLIP_COLOUR => {
                let ClipColourArgs { clip_colour } = Self::args(id, args)?;
                self.prefs.look.clip_colour = clip_colour;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.create_fades_on_clip_edges = OnByDefault(on);
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_RETUNE_ON_TUNING_LOAD => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.retune_on_tuning_load = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_SNAP_AUTOMATION_TO_GRID => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.snap_automation_to_grid = OnByDefault(on);
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_TEMPORARY_FOLDER => {
                let TemporaryFolderArgs { path } = Self::args(id, args)?;
                let path = path.map(PathBuf::from);
                if path.as_deref().is_some_and(|path| !path.is_absolute()) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the Temporary Folder's path must be absolute".to_owned(),
                    });
                }
                self.prefs.temporary_folder = path;
                self.save_preferences();
                Ok(to_value(&TemporaryFolderResult {
                    path: self.temporary_folder().to_string_lossy().into_owned(),
                }))
            }
            ids::SETTINGS_SET_COLLECT_FILES_ON_EXPORT => {
                let CollectFilesArgs { mode } = Self::args(id, args)?;
                self.prefs.collect_files_on_export = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_USER_LIBRARY => {
                let UserLibraryArgs { path } = Self::args(id, args)?;
                let chosen = path.map(PathBuf::from);
                if chosen.as_deref().is_some_and(|path| !path.is_absolute()) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the User Library's path must be absolute".to_owned(),
                    });
                }
                let library = chosen
                    .clone()
                    .or_else(browser::places::default_user_library);
                if let Some(library) = &library {
                    browser::places::make_user_library(library).map_err(|e| {
                        ShellError::Failed {
                            command: id.to_owned(),
                            message: format!("couldn't make {}: {e}", library.display()),
                        }
                    })?;
                }
                self.prefs.browser.user_library = chosen;
                self.browser_view.set_user_library(library.clone());
                self.save_preferences();
                Ok(to_value(&UserLibraryResult {
                    path: library
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                }))
            }
            ids::SETTINGS_SET_SAMPLE_EDITOR => {
                let SampleEditorArgs { path } = Self::args(id, args)?;
                let path = path
                    .map(|path| path.trim().to_owned())
                    .filter(|path| !path.is_empty());
                self.prefs.sample_editor = path;
                self.save_preferences();
                Ok(to_value(&SampleEditorResult {
                    path: self.sample_editor().unwrap_or_default(),
                }))
            }
            ids::SETTINGS_SET_LOOP_WARP_SHORT_SAMPLES => {
                let LoopWarpShortSamplesArgs { mode } = Self::args(id, args)?;
                self.prefs.warp.loop_warp_short_samples = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_AUTO_WARP_LONG_SAMPLES => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.warp.auto_warp_long_samples = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_DEFAULT_WARP_MODE => {
                let DefaultWarpModeArgs { mode } = Self::args(id, args)?;
                self.prefs.warp.default_warp_mode = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_WARP_RECORDED_AUDIO => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.warp_recorded_audio = OnByDefault(on);
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_DEFAULT_LAUNCH_MODE => {
                let DefaultLaunchModeArgs { mode } = Self::args(id, args)?;
                self.prefs.launch.default_mode = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_DEFAULT_LAUNCH_QUANTIZATION => {
                let DefaultLaunchQuantizationArgs { quantization } = Self::args(id, args)?;
                self.prefs.launch.default_quantization = quantization;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_SELECT_ON_LAUNCH => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.launch.select_on_launch = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_SELECT_NEXT_SCENE_ON_LAUNCH => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.launch.select_next_scene_on_launch = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.launch.start_recording_on_scene_launch = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_RECORD_SESSION_AUTOMATION_IN => {
                let RecordSessionAutomationInArgs { tracks } = Self::args(id, args)?;
                self.prefs.record_session_automation_in = tracks;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_AUTOMATION_RECORD_MODE => {
                let AutomationRecordModeArgs { mode } = Self::args(id, args)?;
                self.prefs.automation_record_mode = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.auto_hide_plug_in_windows = OnByDefault(on);
                self.apply_plug_in_windows();
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}
