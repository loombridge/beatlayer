//! The Settings window's pages and the theme choice, as the shell's
//! commands name them.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use bl_model::{LaunchMode, LaunchQuantization};

use crate::agent::AllowedAgent;
use crate::assistant::AssistantState;
use crate::audio::{AudioSettings, ToneSettings};
use crate::midi_io::MidiPortsResult;
use crate::plugins::PluginSettings;

/// `settings.state`: every preference the Settings window shows with a
/// value.
pub const STATE: &str = "settings.state";

/// A page of the Settings window, in Live's order (L-SET-002).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsPage {
    /// Display & Input (L-SET-010).
    DisplayInput,
    /// Shortcuts. Beatlayer's own page: the person's own keyboard
    /// shortcuts (IMP-17; the custom shortcuts spec). It follows Display &
    /// Input, the page Live keeps its keyboard settings in (L-SET-002).
    Shortcuts,
    /// Theme & Colors (L-SET-011).
    ThemeColors,
    /// Audio (L-SET-012). The page the window first opens on.
    #[default]
    Audio,
    /// Link (L-SET-013).
    Link,
    /// Tempo & MIDI (L-SET-014).
    TempoMidi,
    /// File & Folder (L-SET-015).
    FileFolder,
    /// Library (L-SET-016).
    Library,
    /// Plug-Ins (L-SET-017).
    PlugIns,
    /// Agents (L-SET-020). Beatlayer's own page: the agent server's
    /// settings (WP-1.15), which Live doesn't have. It follows Plug-Ins in
    /// the chooser (a Beatlayer addition, decided 14 October 2026).
    Agents,
    /// Record, Warp & Launch (L-SET-018).
    RecordWarpLaunch,
    /// Licenses & Updates (L-SET-019).
    LicensesUpdates,
}

impl SettingsPage {
    /// Every page, in the page chooser's order.
    pub const ALL: [Self; 12] = [
        Self::DisplayInput,
        Self::Shortcuts,
        Self::ThemeColors,
        Self::Audio,
        Self::Link,
        Self::TempoMidi,
        Self::FileFolder,
        Self::Library,
        Self::PlugIns,
        Self::Agents,
        Self::RecordWarpLaunch,
        Self::LicensesUpdates,
    ];

    /// The page's name in the page chooser.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DisplayInput => "Display & Input",
            Self::Shortcuts => "Shortcuts",
            Self::ThemeColors => "Theme & Colors",
            Self::Audio => "Audio",
            Self::Link => "Link",
            Self::TempoMidi => "Tempo & MIDI",
            Self::FileFolder => "File & Folder",
            Self::Library => "Library",
            Self::PlugIns => "Plug-Ins",
            Self::Agents => "Agents",
            Self::RecordWarpLaunch => "Record, Warp & Launch",
            Self::LicensesUpdates => "Licenses & Updates",
        }
    }

    /// The UI parity reference's row for the page.
    #[must_use]
    pub const fn row(self) -> &'static str {
        match self {
            Self::DisplayInput => "L-SET-010",
            Self::Shortcuts => "L-SET-002",
            Self::ThemeColors => "L-SET-011",
            Self::Audio => "L-SET-012",
            Self::Link => "L-SET-013",
            Self::TempoMidi => "L-SET-014",
            Self::FileFolder => "L-SET-015",
            Self::Library => "L-SET-016",
            Self::PlugIns => "L-SET-017",
            Self::Agents => "L-SET-020",
            Self::RecordWarpLaunch => "L-SET-018",
            Self::LicensesUpdates => "L-SET-019",
        }
    }

    /// Whether this build has any of the page's settings, or only its place
    /// in the chooser. The shell (WP-1.7) builds Audio and the theme choice,
    /// WP-1.17 Display & Input's Zoom, WP-1.4b Record, Warp & Launch's
    /// Create Fades on Clip Edges, WP-2.5b Plug-Ins' Auto-Hide Plug-In
    /// Windows, WP-1.15 the Agents page's agent server settings, WP-2.2 the
    /// Tempo & MIDI page's MIDI ports table, and WP-4.20 File & Folder's
    /// Temporary Folder.
    #[must_use]
    pub const fn is_built(self) -> bool {
        matches!(
            self,
            Self::Audio
                | Self::ThemeColors
                | Self::DisplayInput
                | Self::Shortcuts
                | Self::RecordWarpLaunch
                | Self::PlugIns
                | Self::Agents
                | Self::TempoMidi
                | Self::FileFolder
                | Self::Link
        )
    }

    /// The page `steps` places on, wrapping round.
    #[must_use]
    pub fn step(self, steps: isize) -> Self {
        let len = Self::ALL.len() as isize;
        let index = Self::ALL.iter().position(|p| *p == self).unwrap_or(0) as isize;
        Self::ALL[(index + steps).rem_euclid(len) as usize]
    }
}

/// The interface zoom's smallest setting, in percent (WP-1.17; spec
/// interface-zoom R3).
pub const ZOOM_MIN_PERCENT: u16 = 50;

/// The interface zoom's largest setting, in percent.
pub const ZOOM_MAX_PERCENT: u16 = 200;

/// The steps Zoom In and Zoom Out move through, and the Display & Input
/// page's Zoom box steps through, in percent: every 10% from 50% to 200%.
/// Live's steps aren't recorded (interface-zoom, "To observe in Live").
pub const ZOOM_STEPS: [u16; 16] = [
    50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200,
];

/// A zoom factor (1 is 100%) as a whole percentage within the zoom's range.
/// Anything that isn't a finite number is 100%.
#[must_use]
pub fn zoom_percent(factor: f32) -> u16 {
    if !factor.is_finite() {
        return 100;
    }
    let percent = (factor * 100.0).round();
    percent.clamp(f32::from(ZOOM_MIN_PERCENT), f32::from(ZOOM_MAX_PERCENT)) as u16
}

/// A percentage as a zoom factor, brought within the zoom's range.
#[must_use]
pub fn zoom_factor(percent: u16) -> f32 {
    f32::from(percent.clamp(ZOOM_MIN_PERCENT, ZOOM_MAX_PERCENT)) / 100.0
}

/// The zoom `steps` steps up (or, negative, down) from `percent`: each step
/// goes to the next of [`ZOOM_STEPS`] in its direction, so a zoom between
/// steps goes to the nearest step that way first. It stops at 50% and 200%.
#[must_use]
pub fn zoom_step(percent: u16, steps: i32) -> u16 {
    let mut percent = percent.clamp(ZOOM_MIN_PERCENT, ZOOM_MAX_PERCENT);
    for _ in 0..steps.unsigned_abs() {
        let next = if steps > 0 {
            ZOOM_STEPS.iter().copied().find(|&step| step > percent)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|&step| step < percent)
        };
        match next {
            Some(next) => percent = next,
            None => break,
        }
    }
    percent
}

/// The bundled theme's name (themes R2).
pub const BUNDLED_THEME: &str = "Beatlayer";

/// Which of a theme's two sets of colours is drawn (themes R3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    /// Dark text on light panels.
    Light,
    /// Light text on dark panels, the default.
    #[default]
    Dark,
    /// The operating system's mode, light or dark, as it changes.
    FollowSystem,
}

impl Appearance {
    /// Every appearance, in the chooser's order.
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::FollowSystem];

    /// The appearance's name in the Settings window.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::FollowSystem => "Follow System",
        }
    }
}

/// How a theme's neutral colours are tinted (themes R4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Tone {
    /// The theme's neutral colours as its file gives them.
    #[default]
    Neutral,
    /// Tinted towards the hue 60°.
    Warm,
    /// Tinted towards the hue 250°.
    Cool,
}

impl Tone {
    /// Every tone, in the chooser's order.
    pub const ALL: [Self; 3] = [Self::Neutral, Self::Warm, Self::Cool];

    /// The tone's name in the Settings window.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Neutral => "Neutral",
            Self::Warm => "Warm",
            Self::Cool => "Cool",
        }
    }
}

/// What Clip Color gives a new clip (themes R23).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ClipColour {
    /// The clip takes its track's colour, as it does in Live.
    #[default]
    TrackColor,
    /// The clip takes swatch `h mod 70`, `h` being a hash of its ID.
    Random,
}

impl ClipColour {
    /// Every choice, in the chooser's order.
    pub const ALL: [Self; 2] = [Self::TrackColor, Self::Random];

    /// The choice's name in the Settings window.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TrackColor => "Track Color",
            Self::Random => "Random",
        }
    }
}

/// The swatch a new install's Default Track Color is (themes R23).
pub const DEFAULT_TRACK_COLOUR: u8 = 12;

/// The ranges of the Customization number boxes: smallest, largest, step
/// and the value of a new install (themes R7).
pub const GRID_LINE_INTENSITY: (i16, i16, i16, i16) = (0, 200, 5, 100);
/// Brightness, in points of CIELAB lightness.
pub const BRIGHTNESS: (i16, i16, i16, i16) = (-50, 50, 1, 0);
/// Color Intensity, in percent.
pub const COLOR_INTENSITY: (i16, i16, i16, i16) = (0, 200, 5, 100);
/// Color Hue, in degrees.
pub const COLOR_HUE: (i16, i16, i16, i16) = (-180, 180, 5, 0);

/// What the Theme & Colors page's Theme and Customization sections set
/// besides the appearance: the preferences the colour pipeline reads
/// (themes R1, R4, R5, R7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ThemeLook {
    /// The chosen theme's name.
    pub theme: String,
    /// Tone.
    pub tone: Tone,
    /// High Contrast.
    pub high_contrast: bool,
    /// Grid Line Intensity, in percent.
    pub grid_line_intensity: i16,
    /// Brightness.
    pub brightness: i16,
    /// Color Intensity, in percent.
    pub color_intensity: i16,
    /// Color Hue, in degrees.
    pub color_hue: i16,
    /// Auto-Assign Track Colors (themes R23).
    pub auto_assign_track_colours: bool,
    /// Default Track Color, a swatch from 0 to 69, which new tracks take
    /// while Auto-Assign Track Colors is off.
    pub default_track_colour: u8,
    /// Reduced Automatic Colors.
    pub reduced_automatic_colours: bool,
    /// Clip Color.
    pub clip_colour: ClipColour,
}

impl ThemeLook {
    /// The swatch a new track with this ID takes (themes R23).
    #[must_use]
    pub fn new_track_colour(&self, id: bl_model::TrackId) -> u8 {
        if !self.auto_assign_track_colours {
            return self.default_track_colour;
        }
        let raw = id.0.get();
        if self.reduced_automatic_colours {
            // The top row's even-numbered swatches, 0 to 12.
            (raw % 7) as u8 * 2
        } else {
            (raw % u64::from(bl_model::TRACK_COLOURS)) as u8
        }
    }

    /// The swatch a new clip with this ID is given, or none to let it take
    /// its track's colour (themes R23).
    #[must_use]
    pub fn new_clip_colour(&self, id: bl_model::ClipId) -> Option<u8> {
        match self.clip_colour {
            ClipColour::TrackColor => None,
            // SplitMix64's finalizer: a fixed hash, so a clip's swatch never changes.
            ClipColour::Random => {
                let mut h = id.0.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
                h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                Some(((h ^ (h >> 31)) % u64::from(bl_model::SWATCHES)) as u8)
            }
        }
    }
}

impl Default for ThemeLook {
    fn default() -> Self {
        Self {
            theme: BUNDLED_THEME.to_owned(),
            tone: Tone::Neutral,
            high_contrast: false,
            grid_line_intensity: GRID_LINE_INTENSITY.3,
            brightness: BRIGHTNESS.3,
            color_intensity: COLOR_INTENSITY.3,
            color_hue: COLOR_HUE.3,
            auto_assign_track_colours: true,
            default_track_colour: DEFAULT_TRACK_COLOUR,
            reduced_automatic_colours: false,
            clip_colour: ClipColour::default(),
        }
    }
}

/// Theme & Colors in `settings.state` (themes R12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThemeColorsSettings {
    /// The Theme, Tone, High Contrast and Customization settings.
    #[serde(flatten)]
    pub look: ThemeLook,
    /// The Appearance chosen.
    pub appearance: Appearance,
    /// The appearance drawn now, `light` or `dark`: Follow System resolved.
    pub drawn_appearance: Appearance,
    /// The themes the Theme chooser offers, in its order.
    pub themes: Vec<String>,
}

/// The Display & Input page's settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayInputSettings {
    /// The interface zoom in percent, as `view.set_zoom` takes it.
    pub zoom_percent: u16,
    /// The second window's zoom in percent, as `view.set_zoom` takes it
    /// with `window` set to `second`: only while the second window is open,
    /// when the page shows Second Window Zoom (interface-zoom R12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_window_zoom_percent: Option<u16>,
    /// Outline View in Focus, as `settings.set_outline_view_in_focus` sets it.
    pub outline_view_in_focus: bool,
    /// Permanent Scrub Areas, as `settings.set_permanent_scrub_areas` sets
    /// it (the time editing spec, R26).
    pub permanent_scrub_areas: bool,
    /// Use Tab Key to Move Focus, as `focus.toggle_tab_navigation` toggles it.
    pub use_tab_key_to_move_focus: bool,
    /// Wrap Tab Navigation, as `focus.toggle_wrap_navigation` toggles it.
    pub wrap_tab_navigation: bool,
    /// Adjust Values with Mouse Wheel, as `settings.set_wheel_adjusts_values`
    /// sets it (the view options spec, R10).
    pub wheel_adjusts_values: bool,
}

/// The Audio page's settings: the device choices as `audio.configure` takes
/// them, and the test tone's level and frequency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioPageSettings {
    /// Driver, devices, sample rate, buffer size, output channels and Driver
    /// Error Compensation, as set (null is the platform's or the device's
    /// own choice).
    pub device: AudioSettings,
    /// Tone Volume and Tone Frequency, as `audio.set_test_tone` takes them.
    pub tone: ToneSettings,
}

/// The Launch section of Record, Warp & Launch (L-SET-018), as observed in
/// Live 12.4.6: what a new clip starts with and what launching does. They
/// are preferences, kept between starts and not saved with a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LaunchPrefs {
    /// Default Launch Mode, as `settings.set_default_launch_mode` sets it.
    pub default_mode: LaunchMode,
    /// Default Launch Quantization, as
    /// `settings.set_default_launch_quantization` sets it: `null` is
    /// "Global".
    pub default_quantization: Option<LaunchQuantization>,
    /// Select on Launch: a launched clip selects itself.
    pub select_on_launch: bool,
    /// Select Next Scene on Launch: a launched scene selects the next one.
    pub select_next_scene_on_launch: bool,
    /// Start Recording on Scene Launch.
    pub start_recording_on_scene_launch: bool,
}

impl Default for LaunchPrefs {
    fn default() -> Self {
        Self {
            default_mode: LaunchMode::Trigger,
            default_quantization: None,
            select_on_launch: true,
            select_next_scene_on_launch: true,
            start_recording_on_scene_launch: false,
        }
    }
}

/// Loop/Warp Short Samples (Record, Warp & Launch, Warp/Fades, L-SET-018):
/// what a new clip of a short sample gets (the warping spec, R29 and R32).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopWarpShortSamples {
    /// Warp and the loop off.
    UnwarpedOneShot,
    /// Warped, the loop off.
    WarpedOneShot,
    /// Warped, the loop on over the whole file.
    WarpedLoop,
    /// A loop when its analysis finds a steady beat that fits whole bars, a
    /// one-shot otherwise. What a new install has.
    #[default]
    Auto,
}

impl LoopWarpShortSamples {
    /// Every choice, in the order Live lists them.
    pub const ALL: [Self; 4] = [
        Self::UnwarpedOneShot,
        Self::WarpedOneShot,
        Self::WarpedLoop,
        Self::Auto,
    ];

    /// The choice's name in the Settings window.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::UnwarpedOneShot => "Unwarped One Shot",
            Self::WarpedOneShot => "Warped One Shot",
            Self::WarpedLoop => "Warped Loop",
            Self::Auto => "Auto",
        }
    }
}

/// Default Warp Mode (Record, Warp & Launch, Warp/Fades, L-SET-018): the
/// warp mode a new warped clip takes (the warping spec, R29 and R33).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DefaultWarpMode {
    /// Beats. What a new install has.
    #[default]
    Beats,
    /// Tones.
    Tones,
    /// Texture.
    Texture,
    /// Re-Pitch.
    RePitch,
    /// Complex.
    Complex,
    /// Complex Pro.
    ComplexPro,
}

impl DefaultWarpMode {
    /// Every choice, in the order Live lists them.
    pub const ALL: [Self; 6] = [
        Self::Beats,
        Self::Tones,
        Self::Texture,
        Self::RePitch,
        Self::Complex,
        Self::ComplexPro,
    ];

    /// The choice's name in the Settings window.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Beats => "Beats",
            Self::Tones => "Tones",
            Self::Texture => "Texture",
            Self::RePitch => "Re-Pitch",
            Self::Complex => "Complex",
            Self::ComplexPro => "Complex Pro",
        }
    }
}

/// The first three rows of the Warp/Fades section (L-SET-018; WP-4.9), with
/// the defaults observed in Live 12.4.6. They are preferences, kept between
/// starts and not saved with a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WarpPrefs {
    /// Loop/Warp Short Samples, as `settings.set_loop_warp_short_samples`
    /// sets it.
    pub loop_warp_short_samples: LoopWarpShortSamples,
    /// Auto-Warp Long Samples, as `settings.set_auto_warp_long_samples` sets
    /// it.
    pub auto_warp_long_samples: bool,
    /// Default Warp Mode, as `settings.set_default_warp_mode` sets it.
    pub default_warp_mode: DefaultWarpMode,
}

impl Default for WarpPrefs {
    fn default() -> Self {
        Self {
            loop_warp_short_samples: LoopWarpShortSamples::Auto,
            auto_warp_long_samples: true,
            default_warp_mode: DefaultWarpMode::Beats,
        }
    }
}

/// Record Session automation in (Record, Warp & Launch, L-SET-018; WP-3.12):
/// which tracks a Session recording writes control moves into the clips of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionAutomationTracks {
    /// The armed tracks only: Live's default, as observed.
    #[default]
    ArmedTracks,
    /// Every track with a Session clip playing.
    AllTracks,
}

/// The mode an Arrangement recording writes its control moves in (the
/// automation spec, R18): Touch, Live's; Latch; and Write (IMP-23). The
/// names are Beatlayer's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRecordMode {
    /// Touch, Live's mode: a control writes its values while it is held.
    #[default]
    Touch,
    /// Latch: the last value a control was moved to holds until the
    /// recording stops.
    Latch,
    /// Write: a control's envelope is written from where the recording
    /// started.
    Write,
}

impl AutomationRecordMode {
    /// Every mode, in the order the Automation Arm button's context menu
    /// lists them.
    pub const ALL: [Self; 3] = [Self::Touch, Self::Latch, Self::Write];

    /// The mode's name in the Automation Arm button's context menu.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Touch => "Touch",
            Self::Latch => "Latch",
            Self::Write => "Write",
        }
    }
}

pub use bl_model::TakeoverMode;

/// The Agents page's settings. The server's token and each allowed agent's
/// are never reported, only whether the server has one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentsPageSettings {
    /// Whether the agent server runs, as `agent.set_server` sets it.
    pub server: bool,
    /// The port it listens on.
    pub port: u16,
    /// The address agents connect to.
    pub url: String,
    /// Whether the server has a token stored.
    pub token_stored: bool,
    /// The agents allowed to connect, with their autonomous mode.
    pub clients: Vec<AllowedAgent>,
}

/// File & Folder's settings in `settings.state`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileFolderSettings {
    /// The Temporary Folder in use: the setting, or the default.
    pub temporary_folder: String,
    /// The sample editor the Edit button in the Clip View's Audio Utilities
    /// panel opens a clip's file in: its path, empty when none is named.
    pub sample_editor: String,
    /// Save Continuously: whether every finished edit of a set that has a
    /// file is saved within a second, as `settings.set_continuous_save`
    /// sets it.
    pub save_continuously: bool,
}

/// What happens to the audio files a clip or a set uses when it is saved to
/// the browser (file-management R7): Collect Files on Export.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollectFiles {
    /// Copy the files into the saved file's project. A new install's choice.
    #[default]
    Always,
    /// Copy nothing: the saved file names each file where it is.
    Never,
    /// Ask first, with Copy, Don't Copy and Cancel.
    Ask,
}

/// The Library page's settings (L-SET-016).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LibrarySettings {
    /// Collect Files on Export, as `settings.set_collect_files_on_export`
    /// sets it.
    pub collect_files_on_export: CollectFiles,
    /// The User Library's folder, as `settings.set_user_library` sets it;
    /// empty when there is none.
    pub user_library: String,
}

/// What `settings.state` returns: the Settings window's preferences by
/// page. Pages whose settings no package has built yet are left out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsState {
    /// The page the Settings window shows, or showed last.
    pub page: SettingsPage,
    /// Display & Input.
    pub display_input: DisplayInputSettings,
    /// Theme & Colors: the Theme and Customization sections, as the
    /// `settings.set_theme` to `settings.set_color_hue` commands set them.
    pub theme_colors: ThemeColorsSettings,
    /// Audio.
    pub audio: AudioPageSettings,
    /// Tempo & MIDI: the MIDI ports' switches and the computer MIDI
    /// keyboard, the same as `midi.ports` reports.
    pub tempo_midi: MidiPortsResult,
    /// Tempo & MIDI: Takeover Mode, as `settings.set_takeover_mode` sets it
    /// (the remote mapping spec, R28).
    pub takeover_mode: TakeoverMode,
    /// Record, Warp & Launch: Create Fades on Clip Edges, as
    /// `settings.set_create_fades_on_clip_edges` sets it.
    pub create_fades_on_clip_edges: bool,
    /// Options ▸ Retune Set On Loading Tuning Systems, as
    /// `settings.set_retune_on_tuning_load` sets it (the tunings spec, R15).
    pub retune_on_tuning_load: bool,
    /// Options ▸ Snap Automation to Grid, as
    /// `settings.set_snap_automation_to_grid` sets it (the automation spec,
    /// R31).
    pub snap_automation_to_grid: bool,
    /// Record, Warp & Launch: Warp Recorded Audio, as
    /// `settings.set_warp_recorded_audio` sets it.
    pub warp_recorded_audio: bool,
    /// Record, Warp & Launch: Start Transport With Tap Tempo, as
    /// `settings.set_start_transport_with_tap_tempo` sets it (the control bar
    /// transport spec, R20).
    pub start_transport_with_tap_tempo: bool,
    /// Record, Warp & Launch: Loop/Warp Short Samples, Auto-Warp Long
    /// Samples and Default Warp Mode.
    pub warp: WarpPrefs,
    /// Record, Warp & Launch: the Launch section, as the
    /// `settings.set_default_launch_*`, `settings.set_select_on_launch`,
    /// `settings.set_select_next_scene_on_launch` and
    /// `settings.set_start_recording_on_scene_launch` commands set it.
    pub launch: LaunchPrefs,
    /// Record, Warp & Launch: Record Session automation in, as
    /// `settings.set_record_session_automation_in` sets it.
    pub record_session_automation_in: SessionAutomationTracks,
    /// The mode an Arrangement recording writes control moves in, as
    /// `settings.set_automation_record_mode` sets it (the automation spec,
    /// R18).
    pub automation_record_mode: AutomationRecordMode,
    /// File & Folder: the Temporary Folder, as `settings.set_temporary_folder`
    /// sets it.
    pub file_folder: FileFolderSettings,
    /// Library: Collect Files on Export and the User Library's folder.
    pub library: LibrarySettings,
    /// Plug-Ins: Auto-Hide Plug-In Windows, as
    /// `settings.set_auto_hide_plug_in_windows` sets it.
    pub auto_hide_plug_in_windows: bool,
    /// Plug-Ins: how the Browser shows plug-ins, as `plugin.settings` sets
    /// it.
    pub plug_ins: PluginSettings,
    /// Agents.
    pub agents: AgentsPageSettings,
    /// Agents, the assistant: provider, base URL, model, and whether an API
    /// key is stored for the provider (the key itself is never reported).
    pub assistant: AssistantState,
}

/// Adds `settings.state` to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Settings State")
            .description(
                "Reports every preference the Settings window shows with a value, by page, in \
                 the form its own command takes it: Theme & Colors (`settings.set_theme`), Display & \
                 Input (zoom, Outline View in Focus, the Tab key settings), the audio device \
                 settings with Driver Error Compensation and the test tone (`audio.configure`, \
                 `audio.set_test_tone`), the MIDI ports and computer keyboard (`midi.set_port`), Takeover Mode \
                 (`settings.set_takeover_mode`), \
                 Create Fades on Clip Edges, Warp Recorded Audio, the Launch section, the \
                 automation recording mode (`settings.set_automation_record_mode`), the File & \
                 Folder page's Temporary Folder, sample editor and Save Continuously \
                 (`settings.set_temporary_folder`, `settings.set_sample_editor` and \
                 `settings.set_continuous_save`), Auto-Hide Plug-In Windows, the plug-ins the \
                 Browser hides and prefers (`plugin.settings`), the agent server and the \
                 assistant. The server's and the agents' tokens and the assistant's API key are \
                 never reported, only whether one is stored. Preferences aren't saved with the \
                 set. Changes nothing. Use it to check a `settings.set_*` command took effect.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SettingsState>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }

    #[test]
    fn pages_step_round_in_order() {
        assert_eq!(SettingsPage::Audio.step(1), SettingsPage::Link);
        assert_eq!(
            SettingsPage::DisplayInput.step(-1),
            SettingsPage::LicensesUpdates
        );
        assert_eq!(
            SettingsPage::LicensesUpdates.step(1),
            SettingsPage::DisplayInput
        );
        let ids: Vec<String> = SettingsPage::ALL
            .iter()
            .map(|p| {
                serde_json::to_value(p)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(ids[1], "shortcuts");
        assert_eq!(ids[3], "audio");
        assert_eq!(ids[9], "agents");
        assert_eq!(ids[10], "record-warp-launch");
    }

    #[test]
    fn zoom_steps_run_every_ten_percent_from_50_to_200() {
        // Spec: interface-zoom R3.
        assert_eq!(ZOOM_STEPS.first(), Some(&ZOOM_MIN_PERCENT));
        assert_eq!(ZOOM_STEPS.last(), Some(&ZOOM_MAX_PERCENT));
        assert!(ZOOM_STEPS.windows(2).all(|pair| pair[1] - pair[0] == 10));
        assert!(ZOOM_STEPS.contains(&100));
        assert_eq!(zoom_step(100, 1), 110);
        assert_eq!(zoom_step(100, -1), 90);
        assert_eq!(zoom_step(115, 1), 120, "between steps, to the next one up");
        assert_eq!(
            zoom_step(115, -1),
            110,
            "between steps, to the next one down"
        );
        assert_eq!(zoom_step(190, 3), 200, "stops at the top");
        assert_eq!(zoom_step(60, -3), 50, "stops at the bottom");
        assert_eq!(zoom_step(130, 0), 130);
    }

    #[test]
    fn zoom_factors_and_percentages_stay_in_range() {
        assert_eq!(zoom_percent(1.0), 100);
        assert_eq!(zoom_percent(1.254), 125);
        assert_eq!(zoom_percent(0.1), 50);
        assert_eq!(zoom_percent(9.0), 200);
        assert_eq!(zoom_percent(f32::NAN), 100);
        assert_eq!(zoom_percent(f32::INFINITY), 100);
        assert!((zoom_factor(150) - 1.5).abs() < f32::EPSILON);
        assert!((zoom_factor(20) - 0.5).abs() < f32::EPSILON);
        assert!((zoom_factor(500) - 2.0).abs() < f32::EPSILON);
        for step in ZOOM_STEPS {
            assert_eq!(zoom_percent(zoom_factor(step)), step);
        }
    }
}
