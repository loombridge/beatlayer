//! The Settings window (L-SET-001 to L-SET-003): the page chooser at the
//! left, and the page at the right. The Audio page (L-SET-012), the theme
//! choice (L-SET-011), the interface zoom (L-SET-010, WP-1.17), and Create
//! Fades on Clip Edges (L-SET-018, WP-1.4b), and Display & Input's
//! keyboard settings (Outline View in Focus, Use Tab Key to Move Focus,
//! Wrap Tab Navigation; WP-1.14), and Plug-Ins' Auto-Hide Plug-In Windows
//! (L-SET-017, WP-2.5b) are built; the other pages are
//! in the chooser and say which package builds them. The Agents page
//! (L-SET-020) is Beatlayer's own: the agent server's settings (WP-1.15),
//! which Live doesn't have. It follows Plug-Ins, the page whose plug-ins
//! are the other way another program drives the app.

use bl_model::{LaunchMode, LaunchQuantization};
use bl_shell::assistant::AssistantProvider;
use bl_shell::audio::{
    AudioDevices, AudioStatus, COMPENSATION_LIMIT_MS, DeviceListing, TONE_MAX_DBFS, TONE_MAX_HZ,
    TONE_MIN_DBFS, TONE_MIN_HZ,
};
use bl_shell::midi_io_commands as midi_ids;
use bl_shell::settings::{
    SettingsPage, ThemeChoice, ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, zoom_step,
};
use bl_shell::{Invocation, Shell, agent, assistant, commands as ids};
use egui::accesskit::Role;
use egui::{Align2, CentralPanel, Frame, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::layout::SettingsLayout;
use crate::main_window::{UiEvent, sample_rate_text};
use crate::theme::Theme;
use crate::widgets::{Choice, chooser, number_box, switch};

/// The Settings window's state between frames.
#[derive(Clone, Debug)]
pub struct SettingsUi {
    /// The devices the Audio page offers, listed when the window opens and
    /// after an audio change.
    devices: Option<AudioDevices>,
    /// Whether to list them again before the next frame.
    stale: bool,
    /// A move to the next or previous control, done at the start of the
    /// next frame.
    move_focus: Option<egui::FocusDirection>,
    /// The assistant's API key being typed on the Agents page, before Set
    /// stores it (WP-2.23, part 4). It's cleared once stored and never
    /// echoed back from the shell.
    assistant_key: String,
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsUi {
    /// A Settings window that lists the devices when it first draws.
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: None,
            stale: true,
            move_focus: None,
            assistant_key: String::new(),
        }
    }

    /// Carries out an effect that moves the keyboard focus in this window
    /// (Tab and Shift-Tab, S-SET-001 and S-SET-002).
    pub fn apply_effect(&mut self, effect: &bl_shell::Effect) {
        if let bl_shell::Effect::MoveFocus { backwards, .. } = effect {
            self.move_focus = Some(crate::main_window::direction(*backwards));
        }
    }

    /// Lists the audio devices again before the next frame, as after the
    /// window opens or an audio setting changes.
    pub fn refresh(&mut self) {
        self.stale = true;
    }

    /// Lists the devices if they're stale. Call before [`SettingsUi::show`].
    pub fn prepare(&mut self, shell: &mut Shell) {
        if self.stale {
            self.devices = Some(shell.audio_mut().devices());
            // The assistant's key lives in the keychain: read whether one is
            // stored when the window opens, not every frame (WP-2.23).
            shell.refresh_assistant_key();
            self.stale = false;
        }
    }

    /// Draws the window and returns what the person did. With `title_bar`,
    /// it draws Live's 23-point title bar along the top, for a window whose
    /// content covers the system's title bar (macOS, D22).
    pub fn show(
        &mut self,
        root: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        title_bar: bool,
    ) -> Vec<UiEvent> {
        if let Some(direction) = self.move_focus.take() {
            root.ctx().memory_mut(|m| m.move_focus(direction));
        }
        let mut events = Vec::new();
        if title_bar {
            crate::main_window::draw_title_bar(root, theme, "Settings");
        }
        let frame = Frame::NONE.fill(theme.colors.panel.0);
        CentralPanel::default().frame(frame).show(root, |ui| {
            let area = ui.max_rect();
            let layout = SettingsLayout::compute(area.size()).translate(area.min.to_vec2());
            page_chooser(ui, shell, theme, &layout, &mut events);
            let page = shell.settings_page();
            let mut rows = Rows::new(&layout, theme);
            match page {
                SettingsPage::Audio => self.audio_page(ui, shell, theme, &mut rows, &mut events),
                SettingsPage::DisplayInput => {
                    display_page(ui, shell, theme, &mut rows, &mut events);
                }
                SettingsPage::ThemeColors => theme_page(ui, shell, theme, &mut rows, &mut events),
                SettingsPage::RecordWarpLaunch => {
                    record_warp_launch_page(ui, shell, theme, &mut rows, &mut events);
                }
                SettingsPage::PlugIns => plug_ins_page(ui, shell, theme, &mut rows, &mut events),
                SettingsPage::Agents => {
                    agents_page(
                        ui,
                        shell,
                        theme,
                        &mut rows,
                        &mut events,
                        &mut self.assistant_key,
                    );
                }
                SettingsPage::TempoMidi => {
                    tempo_midi_page(ui, shell, theme, &mut rows, &mut events);
                }
                SettingsPage::FileFolder => file_folder_page(ui, shell, &mut rows, &mut events),
                other => rows.note(
                    ui,
                    &format!(
                        "The {} settings come with later packages. This page keeps its place in \
                         the list so it's where you'd look for it.",
                        other.label()
                    ),
                ),
            }
        });
        if events
            .iter()
            .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == ids::AUDIO_CONFIGURE))
        {
            self.stale = true;
        }
        events
    }

    fn audio_page(
        &self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rows: &mut Rows<'_>,
        events: &mut Vec<UiEvent>,
    ) {
        let status: AudioStatus = shell.audio().status();
        let devices = self.devices.clone().unwrap_or_default();
        let configure = |events: &mut Vec<UiEvent>, args: Value| {
            events.push(UiEvent::Invoke(Invocation::new(ids::AUDIO_CONFIGURE, args)));
        };

        rows.heading(ui, "Audio Device");
        let drivers: Vec<Choice<String>> = devices
            .drivers
            .iter()
            .map(|d| Choice {
                label: d.name.clone(),
                value: d.id.clone(),
            })
            .collect();
        let rect = rows.row(ui, "Driver Type", SettingsLayout::CHOOSER);
        if let Some(driver) = chooser(
            ui,
            theme,
            "driver",
            "Driver Type",
            rect,
            &drivers,
            Some(&devices.driver),
            true,
        ) {
            configure(events, json!({ "driver": driver }));
        }

        let inputs: Vec<Choice<String>> = std::iter::once(Choice {
            label: "No Device".to_owned(),
            value: "none".to_owned(),
        })
        .chain(
            devices
                .devices
                .iter()
                .filter(|d| d.input_channels > 0)
                .map(|d| Choice {
                    label: d.name.clone(),
                    value: d.id.clone(),
                }),
        )
        .collect();
        let current_input = status
            .settings
            .input_device
            .clone()
            .unwrap_or_else(|| "none".to_owned());
        let rect = rows.row(ui, "Audio Input Device", SettingsLayout::DEVICE_CHOOSER);
        if let Some(input) = chooser(
            ui,
            theme,
            "input",
            "Audio Input Device",
            rect,
            &inputs,
            Some(&current_input),
            true,
        ) {
            configure(events, json!({ "input_device": input }));
        }

        let default_name = devices
            .devices
            .iter()
            .find(|d| d.default_output)
            .map_or_else(
                || "System Default".to_owned(),
                |d| format!("System Default ({})", d.name),
            );
        let outputs: Vec<Choice<String>> = std::iter::once(Choice {
            label: default_name,
            value: "default".to_owned(),
        })
        .chain(
            devices
                .devices
                .iter()
                .filter(|d| d.output_channels > 0)
                .map(|d| Choice {
                    label: d.name.clone(),
                    value: d.id.clone(),
                }),
        )
        .collect();
        let current_output = status
            .settings
            .output_device
            .clone()
            .unwrap_or_else(|| "default".to_owned());
        let rect = rows.row(ui, "Audio Output Device", SettingsLayout::DEVICE_CHOOSER);
        if let Some(output) = chooser(
            ui,
            theme,
            "output",
            "Audio Output Device",
            rect,
            &outputs,
            Some(&current_output),
            true,
        ) {
            configure(events, json!({ "output_device": output }));
        }
        if let Some(error) = &devices.error {
            rows.note(ui, &format!("The devices couldn't be listed: {error}"));
        }

        // The device in use: the chosen one, or the default output.
        let device: Option<&DeviceListing> = status
            .output_device
            .as_ref()
            .and_then(|open| devices.devices.iter().find(|d| d.id == open.id))
            .or_else(|| devices.devices.iter().find(|d| d.default_output));

        rows.heading(ui, "Channels");
        let most = device.map_or(2, |d| d.output_channels);
        let channels: Vec<Choice<u16>> = std::iter::once(Choice {
            label: format!("All ({most})"),
            value: 0,
        })
        .chain((1..=most.min(64)).map(|n| Choice {
            label: n.to_string(),
            value: n,
        }))
        .collect();
        let rect = rows.row(ui, "Output Config", SettingsLayout::CHOOSER);
        let current = status.settings.output_channels.unwrap_or(0);
        if let Some(n) = chooser(
            ui,
            theme,
            "channels",
            "Output Config",
            rect,
            &channels,
            Some(&current),
            true,
        ) {
            configure(events, json!({ "output_channels": n }));
        }

        rows.heading(ui, "Sample Rate");
        let current_rate = device.and_then(|d| d.current_sample_rate);
        let rates: Vec<Choice<u32>> = std::iter::once(Choice {
            label: current_rate
                .map_or_else(|| "Device's Own".to_owned(), |r| format!("{r} (Device)")),
            value: 0,
        })
        .chain(
            device
                .into_iter()
                .flat_map(|d| d.sample_rates.iter())
                .map(|r| Choice {
                    label: format!("{r} Hz"),
                    value: *r,
                }),
        )
        .collect();
        let rect = rows.row(ui, "In/Out Sample Rate", SettingsLayout::CHOOSER);
        let current = status.settings.sample_rate.unwrap_or(0);
        if let Some(rate) = chooser(
            ui,
            theme,
            "rate",
            "In/Out Sample Rate",
            rect,
            &rates,
            Some(&current),
            true,
        ) {
            configure(events, json!({ "sample_rate": rate }));
        }

        rows.heading(ui, "Latency");
        let default_buffer = device.and_then(|d| d.default_buffer_size);
        let buffers: Vec<Choice<u32>> = std::iter::once(Choice {
            label: default_buffer
                .map_or_else(|| "Device's Own".to_owned(), |b| format!("{b} (Device)")),
            value: 0,
        })
        .chain(
            device
                .map(DeviceListing::buffer_choices)
                .unwrap_or_default()
                .into_iter()
                .map(|b| Choice {
                    label: format!("{b} Samples"),
                    value: b,
                }),
        )
        .collect();
        let rect = rows.row(ui, "Buffer Size", SettingsLayout::CHOOSER);
        let current = status.settings.buffer_size.unwrap_or(0);
        if let Some(frames) = chooser(
            ui,
            theme,
            "buffer",
            "Buffer Size",
            rect,
            &buffers,
            Some(&current),
            true,
        ) {
            configure(events, json!({ "buffer_size": frames }));
        }
        let running = match (status.buffer_size, status.sample_rate, status.buffer_ms) {
            (Some(frames), Some(rate), Some(ms)) => {
                format!("{frames} samples at {}: {ms:.2} ms", sample_rate_text(rate))
            }
            _ if !status.engine_on => "The audio engine is off.".to_owned(),
            _ => "No device open.".to_owned(),
        };
        rows.text_row(ui, "Running At", &running);
        // Spec: recording R7. The compensation is added to the device's
        // reported latency when a recording is moved earlier; it changes
        // recording only.
        let rect = rows.row(ui, "Driver Error Compensation", SettingsLayout::NUMBER);
        let compensation = status.settings.driver_error_compensation_ms;
        if let Some(ms) = number_box(
            ui,
            theme,
            "driver-error-compensation",
            "Driver Error Compensation",
            rect,
            compensation,
            (-COMPENSATION_LIMIT_MS, COMPENSATION_LIMIT_MS),
            &|ms, steps| ms + f64::from(steps),
            &format!("{compensation:.2} ms"),
        ) {
            configure(events, json!({ "driver_error_compensation_ms": ms }));
        }
        let overall = status
            .overall_latency_ms
            .map_or_else(|| "No device open.".to_owned(), |ms| format!("{ms:.2} ms"));
        rows.text_row(ui, "Overall Latency", &overall);

        // The test section (D23): Live's tone switch, Tone Volume, and Tone
        // Frequency.
        rows.heading(ui, "Test");
        let tone_args = |events: &mut Vec<UiEvent>, args: Value| {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::AUDIO_SET_TEST_TONE,
                args,
            )));
        };
        let rect = rows.row(ui, "Test Tone", SettingsLayout::SWITCH);
        if let Some(on) = switch(ui, theme, "test-tone", "Test Tone", rect, status.test_tone) {
            tone_args(events, json!({ "on": on }));
        }
        let tone = status.tone;
        let rect = rows.row(ui, "Tone Volume", SettingsLayout::NUMBER);
        if let Some(db) = number_box(
            ui,
            theme,
            "tone-volume",
            "Tone Volume",
            rect,
            tone.volume_db,
            (TONE_MIN_DBFS, TONE_MAX_DBFS),
            &|db, steps| (db + f64::from(steps)).round(),
            &format!("{:.0} dB", tone.volume_db),
        ) {
            tone_args(events, json!({ "volume_db": db }));
        }
        let rect = rows.row(ui, "Tone Frequency", SettingsLayout::NUMBER);
        if let Some(hz) = number_box(
            ui,
            theme,
            "tone-frequency",
            "Tone Frequency",
            rect,
            tone.frequency_hz,
            (TONE_MIN_HZ, TONE_MAX_HZ),
            // A semitone a step, to the nearest hertz.
            &|hz, steps| (hz * 2f64.powf(f64::from(steps) / 12.0)).round(),
            &format!("{:.0} Hz", tone.frequency_hz),
        ) {
            tone_args(events, json!({ "frequency_hz": hz }));
        }
        let cpu = status
            .cpu_load
            .filter(|_| status.engine_on)
            .map_or_else(|| "OFF".to_owned(), |load| format!("{load:.1} %"));
        rows.text_row(ui, "CPU Usage", &cpu);
        if let Some(error) = &status.error {
            rows.note(ui, &format!("The device reported a problem: {error}"));
        }
    }
}

/// Display & Input (L-SET-010): its Display section, Language then Zoom.
/// The zoom box steps through the zoom's steps by mouse, keyboard, or
/// assistive technology, and the main window follows straight away; this
/// window follows when the mouse button is released (interface-zoom R4).
fn display_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Display");
    // Beatlayer speaks only English until its translations (M9).
    rows.text_row(ui, "Language", "English");
    let percent = shell.zoom_percent();
    let rect = rows.row(ui, "Zoom", SettingsLayout::NUMBER);
    if let Some(next) = number_box(
        ui,
        theme,
        "zoom",
        "Zoom",
        rect,
        f64::from(percent),
        (f64::from(ZOOM_MIN_PERCENT), f64::from(ZOOM_MAX_PERCENT)),
        &|value, steps| f64::from(zoom_step(value as u16, steps)),
        &format!("{percent} %"),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::VIEW_SET_ZOOM,
            json!({ "percent": next as u16 }),
        )));
    }
    // Spec: accessibility R6, R7 (L-SET-010): the Display Options and
    // Navigation and Keyboard sections, with the keyboard's settings built
    // and the others in their places, pending.
    rows.heading(ui, "Display Options");
    let rect = rows.row(ui, "Outline View in Focus", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "outline-view-in-focus",
        "Outline View in Focus",
        rect,
        shell.outline_view_in_focus(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS,
            json!({ "on": on }),
        )));
    }
    for row in &DISPLAY_OPTIONS {
        rows.pending(ui, row);
    }
    rows.heading(ui, "Navigation and Keyboard");
    for (label, salt, on, command) in [
        (
            "Use Tab Key to Move Focus",
            "use-tab-key",
            shell.tab_navigation(),
            ids::FOCUS_TOGGLE_TAB_NAVIGATION,
        ),
        (
            "Wrap Tab Navigation",
            "wrap-tab-navigation",
            shell.wrap_navigation(),
            ids::FOCUS_TOGGLE_WRAP_NAVIGATION,
        ),
    ] {
        let rect = rows.row(ui, label, SettingsLayout::SWITCH);
        if let Some(next) = switch(ui, theme, salt, label, rect, on)
            && next != on
        {
            events.push(UiEvent::Invoke(Invocation::new(command, json!({}))));
        }
    }
    for row in &NAVIGATION {
        rows.pending(ui, row);
    }
    rows.note(
        ui,
        "The zoom scales every window; View > Zoom and its keys change it too. The Mouse and \
         Pen Input and Application Options settings come with later packages.",
    );
}

/// Display Options after Outline View in Focus (L-SET-010), which later
/// packages build, with the controls and values observed in Live 12.4.6:
/// Show Scroll Bars a chooser, Follow Behavior and Hide Labels buttons that
/// show their value.
const DISPLAY_OPTIONS: [Pending; 3] = [
    chooser_row("Show Scroll Bars", "a later package").showing("Follow System"),
    switch_row("Follow Behavior", "a later package").showing("Scroll"),
    switch_row("Hide Labels", "a later package").showing("Show"),
];

/// Navigation and Keyboard after the Tab settings (L-SET-010): Move Clips
/// With Arrow Keys, On in Live 12.4.6.
const NAVIGATION: [Pending; 1] =
    [switch_row("Move Clips With Arrow Keys", "a later package").showing("On")];

/// A row of a Settings page whose setting no package has built yet: its
/// label, and its control's place drawn quiet, which can't be used and
/// tells assistive technology which package brings it.
struct Pending {
    label: &'static str,
    /// The controls on the row: each one's name and width.
    controls: &'static [(&'static str, f32)],
    package: &'static str,
    /// The value Live shows in a new install, where it's observed, drawn
    /// in the quiet control and read by assistive technology.
    value: &'static str,
}

impl Pending {
    /// The same row, showing the value Live shows in a new install.
    const fn showing(self, value: &'static str) -> Self {
        Self { value, ..self }
    }
}

/// A pending row with a chooser.
const fn chooser_row(label: &'static str, package: &'static str) -> Pending {
    Pending {
        label,
        controls: &[("", SettingsLayout::CHOOSER)],
        package,
        value: "",
    }
}

/// A pending row with an On/Off switch.
const fn switch_row(label: &'static str, package: &'static str) -> Pending {
    Pending {
        label,
        controls: &[("", SettingsLayout::SWITCH)],
        package,
        value: "",
    }
}

/// The Record section of Record, Warp & Launch, as observed in Live 12.4.6:
/// none of it is built.
const RECORD: [Pending; 7] = [
    chooser_row("File Type", "WP-2.13").showing("WAV"),
    chooser_row("Bit Depth", "WP-2.13").showing("24"),
    chooser_row("Count-In", "WP-2.15").showing("None"),
    Pending {
        label: "Exclusive",
        controls: &[
            ("Exclusive Arm", SettingsLayout::SWITCH),
            ("Exclusive Solo", SettingsLayout::SWITCH),
        ],
        package: "WP-2.13",
        value: "",
    },
    chooser_row("Clip Update Rate", "WP-2.13").showing("1/16"),
    chooser_row("Record Session automation in", "M3").showing("Armed Tracks"),
    switch_row("Start Transport With Record", "WP-2.13").showing("On"),
];

/// The Warp/Fades rows before Create Fades on Clip Edges, which come with
/// warping.
const WARP: [Pending; 3] = [
    chooser_row("Loop/Warp Short Samples", "M4"),
    switch_row("Auto-Warp Long Samples", "M4"),
    chooser_row("Default Warp Mode", "M4"),
];

/// The Tap Tempo section.
const TAP_TEMPO: [Pending; 1] = [switch_row("Start Transport With Tap Tempo", "M2")];

/// Record, Warp & Launch (L-SET-018, as observed in Live 12.4.6): the
/// Record, Warp/Fades, Launch, and Tap Tempo sections in Live's order, with
/// Create Fades on Clip Edges, the fourth row of Warp/Fades (WP-1.4b), and
/// Warp recorded audio, a Beatlayer addition after it (IMP-02), and the
/// Launch section's rows (WP-3.4), the only settings built. The other rows
/// keep their places, pending.
fn record_warp_launch_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Record");
    for row in &RECORD {
        rows.pending(ui, row);
    }
    rows.heading(ui, "Warp/Fades");
    for row in &WARP {
        rows.pending(ui, row);
    }
    let rect = rows.row(ui, "Create Fades on Clip Edges", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "create-fades-on-clip-edges",
        "Create Fades on Clip Edges",
        rect,
        shell.create_fades_on_clip_edges(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
            json!({ "on": on }),
        )));
    }
    let rect = rows.row(ui, "Warp recorded audio", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "warp-recorded-audio",
        "Warp recorded audio",
        rect,
        shell.warp_recorded_audio(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_WARP_RECORDED_AUDIO,
            json!({ "on": on }),
        )));
    }
    rows.note(
        ui,
        "Warp recorded audio applies once warping arrives (milestone M4): until then recorded \
         clips play unwarped, whatever it says.",
    );
    rows.heading(ui, "Launch");
    launch_rows(ui, shell, theme, rows, events);
    rows.heading(ui, "Tap Tempo");
    for row in &TAP_TEMPO {
        rows.pending(ui, row);
    }
}

/// The Launch section's rows (WP-3.4), in Live's order.
fn launch_rows(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    let prefs = shell.launch_prefs();
    let modes: Vec<Choice<LaunchMode>> = LaunchMode::ALL
        .into_iter()
        .map(|value| Choice {
            label: value.label().to_owned(),
            value,
        })
        .collect();
    let rect = rows.row(ui, "Default Launch Mode", SettingsLayout::CHOOSER);
    if let Some(mode) = chooser(
        ui,
        theme,
        "default-launch-mode",
        "Default Launch Mode",
        rect,
        &modes,
        Some(&prefs.default_mode),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_DEFAULT_LAUNCH_MODE,
            json!({ "mode": mode }),
        )));
    }
    let steps: Vec<Choice<Option<LaunchQuantization>>> = std::iter::once(Choice {
        label: "Global".to_owned(),
        value: None,
    })
    .chain(LaunchQuantization::ALL.into_iter().map(|step| Choice {
        label: step.label().to_owned(),
        value: Some(step),
    }))
    .collect();
    let rect = rows.row(ui, "Default Launch Quantization", SettingsLayout::CHOOSER);
    if let Some(quantization) = chooser(
        ui,
        theme,
        "default-launch-quantization",
        "Default Launch Quantization",
        rect,
        &steps,
        Some(&prefs.default_quantization),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_DEFAULT_LAUNCH_QUANTIZATION,
            json!({ "quantization": quantization }),
        )));
    }
    let switches = [
        (
            "Select on Launch",
            "select-on-launch",
            prefs.select_on_launch,
            ids::SETTINGS_SET_SELECT_ON_LAUNCH,
        ),
        (
            "Select Next Scene on Launch",
            "select-next-scene-on-launch",
            prefs.select_next_scene_on_launch,
            ids::SETTINGS_SET_SELECT_NEXT_SCENE_ON_LAUNCH,
        ),
        (
            "Start Recording on Scene Launch",
            "start-recording-on-scene-launch",
            prefs.start_recording_on_scene_launch,
            ids::SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH,
        ),
    ];
    for (label, id_salt, value, command) in switches {
        let rect = rows.row(ui, label, SettingsLayout::SWITCH);
        if let Some(on) = switch(ui, theme, id_salt, label, rect, value) {
            events.push(UiEvent::Invoke(Invocation::new(
                command,
                json!({ "on": on }),
            )));
        }
    }
}

/// Plug-Ins' Plug-In Sources section (L-SET-017): the plug-in hosts build
/// it. Live's VST2 rows are left out, as Beatlayer doesn't host VST2 (the
/// parity matrix: Out).
const PLUG_IN_SOURCES: [Pending; 4] = [
    Pending {
        label: "Plug-In Sources",
        controls: &[("Rescan", SettingsLayout::CHOOSER)],
        package: "WP-2.6",
        value: "Rescan",
    },
    switch_row("Use Audio Units v2", "WP-2.8"),
    switch_row("Use Audio Units v3", "WP-2.8"),
    switch_row("VST3 Plug-In System Folders", "WP-2.7"),
];

/// Plug-Ins (L-SET-017): Plug-In Sources, pending until the plug-in hosts,
/// then Plug-In Windows, whose Auto-Hide Plug-In Windows is built
/// (WP-2.5b): floating devices' windows follow it as Live's plug-in windows
/// do. Multiple and Auto-Open keep their places, pending.
fn plug_ins_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Plug-In Sources");
    for row in &PLUG_IN_SOURCES {
        rows.pending(ui, row);
    }
    rows.heading(ui, "Plug-In Windows");
    rows.pending(
        ui,
        &switch_row("Multiple Plug-In Windows", "WP-2.6").showing("On"),
    );
    let rect = rows.row(ui, "Auto-Hide Plug-In Windows", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "auto-hide-plug-in-windows",
        "Auto-Hide Plug-In Windows",
        rect,
        shell.auto_hide_plug_in_windows(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS,
            json!({ "on": on }),
        )));
    }
    rows.pending(
        ui,
        &switch_row("Auto-Open Plug-In Windows", "WP-2.6").showing("On"),
    );
}

fn theme_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Theme");
    let choices: Vec<Choice<ThemeChoice>> = [ThemeChoice::Dark, ThemeChoice::Light]
        .into_iter()
        .map(|t| Choice {
            label: t.label().to_owned(),
            value: t,
        })
        .collect();
    let rect = rows.row(ui, "Theme", SettingsLayout::CHOOSER);
    if let Some(choice) = chooser(
        ui,
        theme,
        "theme",
        "Theme",
        rect,
        &choices,
        Some(&shell.theme()),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_THEME,
            json!({ "theme": choice }),
        )));
    }
    rows.note(
        ui,
        "Beatlayer's own dark and light themes. More of the Theme & Colors settings come in \
         milestone M6.",
    );
}

/// Tempo & MIDI (L-SET-014): the MIDI ports table (WP-2.2), each input and
/// output with its Track, Sync, and Remote switches, as the MIDI I/O spec
/// R2 gives it. The Tempo Follower, control surfaces, and Takeover Mode come
/// with later packages. Sync and Remote are kept and act when sync and MIDI
/// mapping are built (R5).
/// File & Folder (L-SET-015): the Temporary Folder row, with its Browse
/// button and path (WP-4.20). The page's other rows come with later
/// packages.
fn file_folder_page(ui: &mut Ui, shell: &Shell, rows: &mut Rows<'_>, events: &mut Vec<UiEvent>) {
    rows.heading(ui, "Temporary Folder");
    let rect = rows.row(ui, "Temporary Folder", SettingsLayout::CHOOSER);
    if ui.put(rect, egui::Button::new("Browse")).clicked() {
        // No path: the host asks for a folder.
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_TEMPORARY_FOLDER,
            json!({}),
        )));
    }
    let path = shell.temporary_folder();
    rows.copyable_row(ui, "Path", "temporary-folder", &path.to_string_lossy());
}

fn tempo_midi_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "MIDI");
    rows.note(
        ui,
        "The Tempo Follower, control surface, and Takeover Mode settings come with later \
         packages.",
    );
    rows.heading(ui, "MIDI Ports");
    let ports = shell.midi().ports();
    if ports.is_empty() {
        rows.note(ui, "The computer offers no MIDI ports.");
    }
    for port in ports {
        let label = format!("{}: {}", port.direction.label(), port.name);
        let rects = rows.row_of(ui, &label, &[SettingsLayout::SWITCH; 3]);
        for (rect, (name, on)) in rects.into_iter().zip([
            ("Track", port.track),
            ("Sync", port.sync),
            ("Remote", port.remote),
        ]) {
            let salt = format!("midi-port-{}-{}-{name}", port.direction as u8, port.name);
            if let Some(on) = switch(ui, theme, &salt, &format!("{label} {name}"), rect, on) {
                let mut args = json!({ "direction": port.direction, "name": port.name });
                args[name.to_lowercase()] = json!(on);
                events.push(UiEvent::Invoke(Invocation::new(midi_ids::SET_PORT, args)));
            }
        }
        if let Some(error) = &port.error {
            rows.note(ui, &format!("{label} couldn't be opened: {error}"));
        }
    }
}

/// The Agents page (L-SET-020, WP-1.15): the switch, where agents connect,
/// the server's token, and the agents the person has allowed, each with its
/// autonomous switch and its own token (WP-1.15c). Beatlayer's own page,
/// straight after Plug-Ins; the whole section moved here unchanged from the
/// Tempo & MIDI page.
fn agents_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
    assistant_key: &mut String,
) {
    let settings = shell.agent_settings();
    let status = shell.agent_status();
    let invoke = |events: &mut Vec<UiEvent>, id: &str, args: Value| {
        events.push(UiEvent::Invoke(Invocation::new(id, args)));
    };
    rows.heading(ui, "Agents");
    let rect = rows.row(ui, "Agent Server", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "agent-server",
        "Agent Server",
        rect,
        settings.server,
    ) {
        invoke(events, agent::AGENT_SET_SERVER, json!({ "on": on }));
    }
    let state = match (&status.listening, &status.error) {
        _ if !settings.server => "Off".to_owned(),
        (_, Some(error)) => format!("Couldn't start: {error}"),
        (Some(_), None) if status.sessions == 1 => "Listening, 1 agent session".to_owned(),
        (Some(_), None) => format!("Listening, {} agent sessions", status.sessions),
        (None, None) => "Starting".to_owned(),
    };
    rows.text_row(ui, "Status", &state);
    rows.copyable_row(ui, "Address", "agent-address", &settings.url());
    if !settings.token.is_empty() {
        let rect = rows.row(ui, "Token", SettingsLayout::CHOOSER);
        if ui.put(rect, egui::Button::new("New Token")).clicked() {
            invoke(events, agent::AGENT_NEW_TOKEN, json!({}));
        }
        rows.token_field(ui, "agent-token", "Token", &settings.token);
    }
    if !settings.clients.is_empty() {
        rows.heading(ui, "Allowed Agents");
        for client in &settings.clients {
            let rect = rows.row(ui, &client.name, SettingsLayout::SWITCH);
            let name = format!("Autonomous: {}", client.name);
            if let Some(on) = switch(
                ui,
                theme,
                &format!("agent-autonomous-{}", client.name),
                &name,
                rect,
                client.autonomous,
            ) {
                invoke(
                    events,
                    agent::AGENT_SET_AUTONOMOUS,
                    json!({ "client": client.name, "on": on }),
                );
            }
            let forget = Rect::from_min_size(
                rect.right_top() + vec2(8.0, 0.0),
                vec2(SettingsLayout::SWITCH + 6.0, rect.height()),
            );
            let response = ui.put(forget, egui::Button::new("Forget"));
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_label(format!("Forget {}", client.name));
            });
            if response.clicked() {
                invoke(
                    events,
                    agent::AGENT_FORGET_CLIENT,
                    json!({ "client": client.name }),
                );
            }
            // Spec: agent-permissions R12. The agent's own token under its
            // name, shown and replaced as the server's is.
            if !client.token.is_empty() {
                let new_token = rows.row(ui, "Token", SettingsLayout::CHOOSER);
                let response = ui.put(new_token, egui::Button::new("New Token"));
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_label(format!("New Token: {}", client.name));
                });
                if response.clicked() {
                    invoke(
                        events,
                        agent::AGENT_NEW_CLIENT_TOKEN,
                        json!({ "client": client.name }),
                    );
                }
                rows.token_field(
                    ui,
                    &format!("agent-token-{}", client.name),
                    &format!("Token: {}", client.name),
                    &client.token,
                );
            }
        }
    }
    rows.note(
        ui,
        "Agent clients on this computer, such as Claude Code, can read and edit the open set \
         through the agent API at the address above, sending the token as a bearer token. \
         An agent you allow gets its own token and isn't asked again. Clients using the \
         server's token are asked each time. Deleting or reaching outside the project asks \
         you first unless the agent is autonomous (the switch beside its name). Web pages \
         can't connect.",
    );
    assistant_section(ui, shell, theme, rows, events, assistant_key);
}

/// The width of an assistant setting's text field on the Agents page.
const ASSISTANT_FIELD: f32 = 240.0;

/// The built-in assistant's settings on the Agents page (WP-2.23, part 4):
/// the provider, its base URL and model, the API key entry (stored to the
/// OS keychain, shown masked and never echoed), and the activity log. The
/// assistant is Beatlayer's own, so this is a Beatlayer addition on the
/// Agents page.
fn assistant_section(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
    key: &mut String,
) {
    let settings = shell.assistant_settings();
    let state = shell.assistant_state();
    let invoke = |events: &mut Vec<UiEvent>, id: &str, args: Value| {
        events.push(UiEvent::Invoke(Invocation::new(id, args)));
    };
    rows.heading(ui, "Assistant");

    let rect = rows.row(ui, "Provider", SettingsLayout::DEVICE_CHOOSER);
    let choices: Vec<Choice<AssistantProvider>> = AssistantProvider::ALL
        .into_iter()
        .map(|provider| Choice {
            label: provider.label().to_owned(),
            value: provider,
        })
        .collect();
    if let Some(provider) = chooser(
        ui,
        theme,
        "assistant-provider",
        "Assistant Provider",
        rect,
        &choices,
        Some(&settings.provider),
        true,
    ) {
        invoke(
            events,
            assistant::ASSISTANT_SET_PROVIDER,
            json!({ "provider": provider }),
        );
    }

    // The base URL is only the OpenAI-compatible provider's: it's where
    // OpenRouter and local servers are reached.
    let rect = rows.row(ui, "Base URL", ASSISTANT_FIELD);
    let mut base_url = settings.base_url.clone();
    let response = ui
        .put(
            rect,
            egui::TextEdit::singleline(&mut base_url)
                .id_salt("assistant-base-url")
                .hint_text("The provider's own")
                .interactive(settings.provider.has_base_url()),
        )
        .on_hover_text(
            "The OpenAI-compatible server's address, such as http://localhost:11434/v1 for \
             Ollama. Empty uses the provider's own.",
        );
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label("Assistant Base URL".to_owned());
    });
    if response.changed() {
        invoke(
            events,
            assistant::ASSISTANT_SET_BASE_URL,
            json!({ "base_url": base_url }),
        );
    }

    let rect = rows.row(ui, "Model", ASSISTANT_FIELD);
    let mut model = settings.model.clone();
    let response = ui.put(
        rect,
        egui::TextEdit::singleline(&mut model).id_salt("assistant-model"),
    );
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label("Assistant Model".to_owned());
    });
    if response.changed() {
        invoke(
            events,
            assistant::ASSISTANT_SET_MODEL,
            json!({ "model": model }),
        );
    }

    // The API key: typed into a masked field, written to the keychain by
    // `assistant.set_key` and never read back (WP-2.23, part 4).
    rows.label(ui, "API Key");
    let rect = Rect::from_min_size(
        pos2(rows.layout.control_x, rows.y),
        vec2(ASSISTANT_FIELD - COPY_BUTTON - 6.0, 18.0),
    );
    let response = ui.put(
        rect,
        egui::TextEdit::singleline(key)
            .id_salt("assistant-key")
            .password(true),
    );
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label("Assistant API Key".to_owned());
    });
    let set = Rect::from_min_size(
        pos2(rect.right() + 6.0, rect.top()),
        vec2(COPY_BUTTON, rect.height()),
    );
    let response = ui.put(set, egui::Button::new("Set"));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label("Set Assistant API Key".to_owned());
    });
    if response.clicked() && !key.is_empty() {
        invoke(
            events,
            assistant::ASSISTANT_SET_KEY,
            json!({ "key": key.clone() }),
        );
        key.clear();
    }
    rows.y += SettingsLayout::OPTION_ROW;
    rows.text_row(
        ui,
        "Key",
        if state.key_set {
            "Stored in the keychain"
        } else {
            "None stored"
        },
    );
    if state.key_set {
        let rect = rows.row(ui, "Forget", SettingsLayout::CHOOSER);
        let response = ui.put(rect, egui::Button::new("Forget Key"));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("Forget Assistant API Key".to_owned());
        });
        if response.clicked() {
            invoke(events, assistant::ASSISTANT_FORGET_KEY, json!({}));
        }
    }

    let rect = rows.row(ui, "Activity Log", SettingsLayout::CHOOSER);
    let open = shell.assistant_log_open();
    let response = ui.put(rect, egui::Button::new(if open { "Close" } else { "Open" }));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label("Open Assistant Activity Log".to_owned());
    });
    if response.clicked() {
        invoke(events, assistant::ASSISTANT_TOGGLE_ACTIVITY_LOG, json!({}));
    }
    if open {
        rows.heading(ui, "Recent Activity");
        if shell.assistant_log().is_empty() {
            rows.text_row(
                ui,
                "Activity Log",
                "The assistant hasn't run a command yet.",
            );
        }
        for entry in shell.assistant_log() {
            let result = outcome_text(&entry.outcome);
            rows.text_row(
                ui,
                &clock(entry.at),
                &format!("{}: {result}", entry.command),
            );
        }
    }

    rows.note(
        ui,
        "The built-in assistant answers a prompt by running commands on the open set, the \
         same tools the agent API offers. Its provider, base URL and model are kept with the \
         preferences; its API key is stored only in your computer's keychain and never shown \
         again. A command that deletes or reaches outside the project asks you first, unless \
         you chose to always allow it.",
    );
}

/// The activity log's result column: what came of a command the assistant
/// ran, short enough for one line.
fn outcome_text(outcome: &bl_shell::assistant::Outcome) -> String {
    use bl_shell::assistant::Outcome;
    const MAX: usize = 60;
    let text = match outcome {
        Outcome::Ran(value) => format!("ran, {value}"),
        Outcome::Denied(message) => format!("denied: {message}"),
        Outcome::Failed(message) => format!("failed: {message}"),
    };
    if text.chars().count() > MAX {
        let short: String = text.chars().take(MAX).collect();
        format!("{short}…")
    } else {
        text
    }
}

/// An activity entry's time as `HH:MM:SS` in UTC, since the app keeps no
/// local-time clock.
fn clock(at: std::time::SystemTime) -> String {
    let seconds = at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let day = seconds % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

/// The page chooser (L-SET-002): one row per page, in Live's order.
fn page_chooser(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    layout: &SettingsLayout,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    ui.painter().rect_filled(layout.chooser, 0.0, c.bar.0);
    let list = Id::new("settings-pages");
    let _ = ui.interact(layout.chooser, list, Sense::hover());
    ui.ctx().accesskit_node_builder(list, |node| {
        node.set_role(Role::TabList);
        node.set_label("Settings Pages".to_owned());
    });
    let scope = egui::UiBuilder::new()
        .max_rect(layout.chooser)
        .accessibility_parent(list);
    ui.scope_builder(scope, |ui| {
        for (index, page) in SettingsPage::ALL.into_iter().enumerate() {
            let rect = layout.page_row(index);
            let response = ui.interact(rect, Id::new(("settings-page", index)), Sense::click());
            let selected = shell.settings_page() == page;
            if selected {
                ui.painter().rect_filled(rect, 0.0, c.accent.0);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 0.0, c.control_hover.0);
            }
            let ink = if selected {
                c.accent_text.0
            } else if page.is_built() {
                c.text.0
            } else {
                c.text_dim.0
            };
            ui.painter().text(
                pos2(rect.left() + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                page.label(),
                theme.font(),
                ink,
            );
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect.shrink(1.0),
                    0.0,
                    Stroke::new(1.5, c.focus.0),
                    StrokeKind::Inside,
                );
            }
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, page.label())
            });
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(Role::Tab);
                node.set_selected(selected);
            });
            if response.clicked() && !selected {
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::APP_SETTINGS,
                    json!({ "page": page }),
                )));
            }
        }
    });
}

/// The width of a Copy button beside an address or a token on the Agents
/// page. Not measured in Live, which has no such button: a switch's width,
/// narrowed to the word.
const COPY_BUTTON: f32 = 44.0;

/// Lays out a page's rows top to bottom (L-SET-003): section headings with
/// a rule, labels at one x, controls at another, rows 22 points apart.
struct Rows<'a> {
    layout: &'a SettingsLayout,
    theme: &'a Theme,
    y: f32,
}

impl<'a> Rows<'a> {
    fn new(layout: &'a SettingsLayout, theme: &'a Theme) -> Self {
        Self {
            layout,
            theme,
            y: layout.first_page.top(),
        }
    }

    fn right(&self) -> f32 {
        self.layout.page.right() - 14.0
    }

    fn heading(&mut self, ui: &Ui, text: &str) {
        if self.y > self.layout.first_page.top() {
            self.y += 6.0;
        }
        let painter = ui.painter();
        let x = self.layout.label_x;
        painter.text(
            pos2(x, self.y + 9.0),
            Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(self.theme.sizes.heading),
            self.theme.colors.text.0,
        );
        painter.line_segment(
            [pos2(x, self.y + 19.0), pos2(self.right(), self.y + 19.0)],
            Stroke::new(1.0, self.theme.colors.separator.0),
        );
        self.y += SettingsLayout::OPTION_ROW + 4.0;
    }

    fn label(&self, ui: &Ui, text: &str) {
        ui.painter().text(
            pos2(self.layout.label_x, self.y + 9.0),
            Align2::LEFT_CENTER,
            text,
            self.theme.font(),
            self.theme.colors.text_dim.0,
        );
    }

    /// A row with a label and a control `width` points wide; returns the
    /// control's rectangle.
    fn row(&mut self, ui: &Ui, label: &str, width: f32) -> Rect {
        self.label(ui, label);
        let rect = Rect::from_min_size(pos2(self.layout.control_x, self.y), vec2(width, 18.0));
        self.y += SettingsLayout::OPTION_ROW;
        rect
    }

    /// A row with a label and controls of these widths side by side, as
    /// Live's Exclusive Arm and Solo are; returns their rectangles.
    fn row_of(&mut self, ui: &Ui, label: &str, widths: &[f32]) -> Vec<Rect> {
        self.label(ui, label);
        let mut x = self.layout.control_x;
        let rects = widths
            .iter()
            .map(|&width| {
                let rect = Rect::from_min_size(pos2(x, self.y), vec2(width, 18.0));
                x += width + 4.0;
                rect
            })
            .collect();
        self.y += SettingsLayout::OPTION_ROW;
        rects
    }

    /// A row that shows a value as text, read by assistive technology as a
    /// labelled value.
    fn text_row(&mut self, ui: &mut Ui, label: &str, value: &str) {
        self.label(ui, label);
        let rect = Rect::from_min_size(
            pos2(self.layout.control_x, self.y),
            vec2(self.right() - self.layout.control_x, 18.0),
        );
        let response = ui.interact(rect, Id::new(("settings-value", label)), Sense::hover());
        ui.painter().text(
            pos2(rect.left() + 2.0, rect.center().y),
            Align2::LEFT_CENTER,
            value,
            self.theme.font(),
            self.theme.colors.text.0,
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Label);
            node.set_label(label.to_owned());
            node.set_value(value.to_owned());
        });
        self.y += SettingsLayout::OPTION_ROW;
    }

    /// A row whose setting isn't built yet ([`Pending`]).
    fn pending(&mut self, ui: &mut Ui, row: &Pending) {
        self.label(ui, row.label);
        let mut x = self.layout.control_x;
        let c = &self.theme.colors;
        for (index, &(name, width)) in row.controls.iter().enumerate() {
            let name = if name.is_empty() { row.label } else { name };
            let rect = Rect::from_min_size(pos2(x, self.y), vec2(width, 18.0));
            let response = ui
                .interact(
                    rect,
                    Id::new(("settings-pending", row.label, index)),
                    Sense::hover(),
                )
                .on_hover_text(format!(
                    "{name}. Not built yet: arrives with {}.",
                    row.package
                ));
            ui.painter().rect_filled(
                rect,
                self.theme.sizes.corner,
                c.control.0.gamma_multiply(0.55),
            );
            if !row.value.is_empty() {
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    row.value,
                    self.theme.font(),
                    c.pending.0,
                );
            }
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, false, name));
            ui.ctx().accesskit_node_builder(response.id, |node| {
                if !row.value.is_empty() {
                    node.set_value(row.value.to_owned());
                }
                node.set_description(format!("Not built yet: arrives with {}.", row.package));
                node.remove_action(egui::accesskit::Action::Click);
                node.remove_action(egui::accesskit::Action::Focus);
            });
            // Side by side, as Live's Exclusive Arm and Solo are.
            x += width + 4.0;
        }
        self.y += SettingsLayout::OPTION_ROW;
    }

    /// A token on a line of its own, so all 64 digits show beside its Copy
    /// button; it can be selected and copied but not edited. This is
    /// where the person sees a token: the only place the window reveals a
    /// [`agent::Secret`].
    fn token_field(&mut self, ui: &mut Ui, id_salt: &str, label: &str, token: &agent::Secret) {
        let rect = Rect::from_min_size(
            pos2(self.layout.label_x, self.y),
            vec2(self.right() - self.layout.label_x - COPY_BUTTON - 6.0, 18.0),
        );
        self.field_and_copy(ui, rect, id_salt, label, token.expose());
        self.y += SettingsLayout::OPTION_ROW;
    }

    /// A row that shows a value in a read-only field that can be selected
    /// and copied, with a Copy button beside it.
    fn copyable_row(&mut self, ui: &mut Ui, label: &str, id_salt: &str, value: &str) {
        self.label(ui, label);
        let rect = Rect::from_min_size(
            pos2(self.layout.control_x, self.y),
            vec2(
                self.right() - self.layout.control_x - COPY_BUTTON - 6.0,
                18.0,
            ),
        );
        self.field_and_copy(ui, rect, id_salt, label, value);
        self.y += SettingsLayout::OPTION_ROW;
    }

    /// A read-only field showing `value`, with a Copy button to its right
    /// that puts the whole value on the clipboard. The field is where the
    /// mouse or the keyboard selects the text, and Cmd+C (Ctrl+C on Windows
    /// and Linux) copies the selection; the button copies the whole value
    /// without a selection. Copying changes nothing in the set, so it isn't
    /// an undo step. `label` names the value and its button for assistive
    /// technology.
    fn field_and_copy(&self, ui: &mut Ui, rect: Rect, id_salt: &str, label: &str, value: &str) {
        let mut text = value;
        let response = ui.put(
            rect,
            egui::TextEdit::singleline(&mut text)
                .id_salt(id_salt)
                .font(egui::FontId::proportional(
                    self.theme.sizes.small_text * 0.9,
                ))
                .clip_text(true),
        );
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label(label.to_owned());
            node.set_read_only();
        });
        let copy = Rect::from_min_size(
            pos2(rect.right() + 6.0, rect.top()),
            vec2(COPY_BUTTON, rect.height()),
        );
        let response = ui.put(copy, egui::Button::new("Copy"));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label(format!("Copy {label}"));
        });
        if response.clicked() {
            ui.ctx().copy_text(value.to_owned());
        }
    }

    fn note(&mut self, ui: &Ui, text: &str) {
        let width = self.right() - self.layout.label_x;
        let galley = ui.painter().layout(
            text.to_owned(),
            self.theme.small_font(),
            self.theme.colors.text_dim.0,
            width,
        );
        let height = galley.size().y;
        ui.painter().galley(
            pos2(self.layout.label_x, self.y + 4.0),
            galley,
            self.theme.colors.text_dim.0,
        );
        self.y += height + 10.0;
    }
}
