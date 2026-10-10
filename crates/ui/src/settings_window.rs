//! The Settings window (L-SET-001 to L-SET-003): the page chooser at the
//! left, and the page at the right. The Audio page (L-SET-012) and the
//! theme choice (L-SET-011) are built; the other pages are in the chooser
//! and say which package builds them.

use bl_shell::audio::{
    AudioDevices, AudioStatus, DeviceListing, TONE_MAX_DBFS, TONE_MAX_HZ, TONE_MIN_DBFS,
    TONE_MIN_HZ,
};
use bl_shell::settings::{SettingsPage, ThemeChoice};
use bl_shell::{Invocation, Shell, commands as ids};
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
        }
    }

    /// Carries out an effect that moves the keyboard focus in this window
    /// (Tab and Shift-Tab, S-SET-001 and S-SET-002).
    pub fn apply_effect(&mut self, effect: &bl_shell::Effect) {
        if let bl_shell::Effect::MoveFocus { backwards } = effect {
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
                SettingsPage::ThemeColors => theme_page(ui, shell, theme, &mut rows, &mut events),
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
