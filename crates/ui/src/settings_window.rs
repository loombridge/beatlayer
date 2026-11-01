//! The Settings window (L-SET-001 to L-SET-003): the page chooser at the
//! left, and the page at the right. The Audio page (L-SET-012), the theme
//! choice (L-SET-011), the interface zoom (L-SET-010, WP-1.17), and Create
//! Fades on Clip Edges (L-SET-018, WP-1.4b), and Display & Input's
//! keyboard settings (Outline View in Focus, Use Tab Key to Move Focus,
//! Wrap Tab Navigation; WP-1.14), and Plug-Ins' Plug-In Sources (WP-2.9b)
//! and Auto-Hide Plug-In Windows (L-SET-017, WP-2.5b) are built; the other pages are
//! in the chooser and say which package builds them. The Agents page
//! (L-SET-020) is Beatlayer's own: the agent server's settings (WP-1.15),
//! which Live doesn't have. It follows Plug-Ins, the page whose plug-ins
//! are the other way another program drives the app.

use bl_model::{LaunchMode, LaunchQuantization, TakeoverMode};
use bl_project::registry as recovery_ids;
use bl_shell::assistant::AssistantProvider;
use bl_shell::audio::{
    AudioDevices, AudioStatus, COMPENSATION_LIMIT_MS, DeviceListing, TONE_MAX_DBFS, TONE_MAX_HZ,
    TONE_MIN_DBFS, TONE_MIN_HZ,
};
use bl_shell::control_surface_commands as control_surface;
use bl_shell::keymap::Origin;
use bl_shell::link_commands as link_ids;
use bl_shell::midi_io_commands as midi_ids;
use bl_shell::settings::{
    Appearance, BRIGHTNESS, COLOR_HUE, COLOR_INTENSITY, ClipColour, DefaultWarpMode,
    GRID_LINE_INTENSITY, LoopWarpShortSamples, SessionAutomationTracks, SettingsPage, Tone,
    ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, zoom_step,
};
use bl_shell::tempo_follower_commands as tempo_follower_ids;
use bl_shell::{Invocation, Shell, agent, assistant, commands as ids};
use egui::accesskit::Role;
use egui::{
    Align2, CentralPanel, Frame, Id, Key, RawInput, Rect, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::keymap::{Platform, binding_text, is_modifier, mods_from_egui};
use crate::layout::SettingsLayout;
use crate::main_window::{UiEvent, sample_rate_text};
use crate::theme::Theme;
use crate::widgets::{Choice, NO_LEDS, chooser, number_box, switch};

/// The commands a key can run, by the context each belongs to, as the
/// Shortcuts page lists them (R7).
fn shortcut_rows(shell: &Shell, platform: Platform) -> Vec<ShortcutRow> {
    let state = shell.keymap_state();
    let mut rows: Vec<ShortcutRow> = Vec::new();
    // Where each command's row is, so the rows are built in one pass.
    let mut at: std::collections::BTreeMap<(String, String), usize> =
        std::collections::BTreeMap::new();
    for shortcut in &state.shortcuts {
        let (keys, origin) = match platform {
            Platform::Mac => (&shortcut.mac.keys, shortcut.mac.origin),
            Platform::Other => (&shortcut.win.keys, shortcut.win.origin),
        };
        for command in &shortcut.commands {
            let label = shell
                .registry()
                .get(command)
                .map_or_else(|| command.clone(), |spec| spec.label.to_string());
            let key = (command.clone(), shortcut.context.clone());
            match at.get(&key).map(|index| &mut rows[*index]) {
                Some(row) => {
                    for key in keys {
                        if !row.keys.contains(key) {
                            row.keys.push(key.clone());
                        }
                    }
                    row.own = row.own || origin == Origin::User;
                    row.changed = row.changed || origin != Origin::Live;
                }
                None => {
                    at.insert(key, rows.len());
                    rows.push(ShortcutRow {
                        command: command.clone(),
                        label,
                        context: shortcut.context.clone(),
                        keys: keys.clone(),
                        own: origin == Origin::User,
                        changed: origin != Origin::Live,
                    });
                }
            }
        }
    }
    // The commands a menu item runs, which the keymap gives no keys: a
    // person can give them keys (R2).
    for menu in bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current()) {
        for item in menu.walk() {
            let bl_shell::menus::Action::Command(id) = &item.action else {
                continue;
            };
            let id = *id;
            if rows.iter().any(|row| row.command == id) {
                continue;
            }
            let spec = shell.registry().get(id);
            let context = spec
                .and_then(|spec| spec.contexts.first())
                .map_or_else(|| "global".to_owned(), |context| context.id().to_owned());
            rows.push(ShortcutRow {
                command: id.to_owned(),
                label: spec.map_or_else(|| id.to_owned(), |spec| spec.label.to_string()),
                context,
                keys: Vec::new(),
                own: false,
                changed: false,
            });
        }
    }
    rows
}

/// Whether a row matches the search, as the command search matches it:
/// every word typed starts a word of the label, or is in the command's
/// description (R7).
fn matches_search(row: &ShortcutRow, shell: &Shell, search: &str) -> bool {
    let words: Vec<String> = search
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .collect();
    if words.is_empty() {
        return true;
    }
    let label: Vec<String> = row
        .label
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .collect();
    let description = shell
        .registry()
        .get(&row.command)
        .map(|spec| spec.description.to_lowercase())
        .unwrap_or_default();
    words.iter().all(|word| {
        label.iter().any(|part| part.starts_with(word)) || description.contains(word.as_str())
    })
}

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
    /// The Shortcuts page's state: the search text, the command whose keys
    /// are being recorded, and the questions the page asks (the custom
    /// shortcuts spec, R7 to R10).
    shortcuts: ShortcutsPage,
}

/// The Shortcuts page's state between frames.
#[derive(Clone, Debug, Default)]
pub struct ShortcutsPage {
    /// The text in the search field, which narrows the list (R7).
    pub search: String,
    /// The command whose keys are being recorded (R8).
    pub recording: Option<Recording>,
    /// The question about keys that already run another command (R9).
    pub conflict: Option<Conflict>,
    /// Whether the question about resetting every shortcut is asked (R10).
    pub ask_reset_all: bool,
}

/// The command whose keys are being recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// The command's ID.
    pub command: String,
    /// The context it runs in.
    pub context: String,
}

/// The keys that already run another command, and the question about them.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// The command being recorded.
    pub command: String,
    /// The context it runs in.
    pub context: String,
    /// The keys pressed.
    pub keys: Vec<String>,
    /// The command the keys already run.
    pub other: String,
    /// The context the other command runs in.
    pub other_context: String,
}

/// A command a key can run, as the Shortcuts page lists it (R7).
struct ShortcutRow {
    /// The command's ID.
    command: String,
    /// The command's label.
    label: String,
    /// The context it runs in.
    context: String,
    /// Its keys on this platform.
    keys: Vec<String>,
    /// Whether the keys are the person's own.
    own: bool,
    /// Whether the keys differ from Live's.
    changed: bool,
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
            shortcuts: ShortcutsPage::default(),
        }
    }

    /// Whether the Shortcuts page is recording a command's keys, so the
    /// keys it takes are the person's and run nothing (R8).
    #[must_use]
    pub fn recording(&self) -> bool {
        self.shortcuts.recording.is_some()
    }

    /// Takes the key press that ends a recording, and returns the
    /// command it binds. The keys that already run another command are
    /// asked about first (R9), and nothing is saved until the person
    /// chooses (R9, R11).
    pub fn take_recorded_key(&mut self, input: &mut RawInput, shell: &Shell) -> Option<Invocation> {
        let recording = self.shortcuts.recording.clone()?;
        let platform = Platform::current();
        let mut pressed = None;
        let mut events = Vec::new();
        for event in input.events.drain(..) {
            match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if pressed.is_none() && !is_modifier(key) => {
                    if key == Key::Escape {
                        // Esc stops the recording and changes nothing (R8).
                        self.shortcuts.recording = None;
                    } else if key != Key::Tab {
                        pressed = Some((key, modifiers));
                    }
                }
                // The keys being recorded are the person's, so they reach
                // nothing else. Tab still moves the focus.
                egui::Event::Key { key: Key::Tab, .. } => events.push(event),
                egui::Event::Key { .. } => {}
                other => events.push(other),
            }
        }
        input.events = events;
        let (key, modifiers) = pressed?;
        let binding = binding_text(key, mods_from_egui(modifiers, platform), platform);
        let chords = bl_shell::keymap::parse_binding(&binding, platform).unwrap_or_default();
        if let Some((other, other_context)) =
            shell
                .keymap()
                .conflict(&chords, &recording.context, &recording.command, platform)
        {
            self.shortcuts.conflict = Some(Conflict {
                command: recording.command,
                context: recording.context,
                keys: vec![binding],
                other,
                other_context,
            });
            self.shortcuts.recording = None;
            return None;
        }
        self.shortcuts.recording = None;
        Some(Invocation::new(
            ids::KEYMAP_BIND,
            json!({
                "command": recording.command,
                "keys": [binding],
                "platform": platform.id(),
                "context": recording.context,
            }),
        ))
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
                SettingsPage::Shortcuts => {
                    self.shortcuts_page(ui, shell, theme, &layout, &mut events);
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
                SettingsPage::Link => link_page(ui, shell, theme, &mut rows, &mut events),
                SettingsPage::TempoMidi => {
                    tempo_midi_page(ui, shell, theme, &mut rows, &mut events);
                }
                SettingsPage::FileFolder => {
                    file_folder_page(ui, shell, theme, &mut rows, &mut events);
                }
                SettingsPage::Library => {
                    library_page(ui, shell, theme, &mut rows, &mut events);
                }
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

    /// The Shortcuts page (the custom shortcuts spec, R7 to R13): every
    /// command a key can run, its context and its keys, and the ways to
    /// change them.
    fn shortcuts_page(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &SettingsLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let platform = Platform::current();
        let top = layout.first_page.top();
        // The search field, which narrows the list by the label (R7).
        ui.painter().text(
            pos2(layout.label_x, top + 9.0),
            Align2::LEFT_CENTER,
            "Search",
            theme.font(),
            theme.colors.text_dim.0,
        );
        let field = Rect::from_min_size(pos2(layout.control_x, top), vec2(194.0, 18.0));
        let scope = egui::UiBuilder::new().max_rect(field);
        ui.scope_builder(scope, |ui| {
            let mut search = self.shortcuts.search.clone();
            let response = ui.add_sized(
                [194.0, 18.0],
                egui::TextEdit::singleline(&mut search).hint_text("Command"),
            );
            if response.changed() {
                self.shortcuts.search = search;
            }
        });
        // The list, by the menu or context each command belongs to.
        let rows = shortcut_rows(shell, platform);
        let shown: Vec<&ShortcutRow> = rows
            .iter()
            .filter(|row| matches_search(row, shell, &self.shortcuts.search))
            .collect();
        let list = Rect::from_min_max(
            pos2(layout.page.left(), top + 24.0),
            pos2(layout.page.right(), layout.page.bottom() - 26.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
        egui::ScrollArea::vertical()
            .id_salt(child.id().with("shortcuts"))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                for row in shown {
                    self.shortcut_row(ui, theme, layout, row, events);
                }
            });
        // Reset All, at the page's foot, which asks first (R10).
        let foot = Rect::from_min_size(
            pos2(layout.control_x, layout.page.bottom() - 24.0),
            vec2(108.0, 18.0),
        );
        let scope = egui::UiBuilder::new().max_rect(foot);
        ui.scope_builder(scope, |ui| {
            if ui
                .add_sized([108.0, 18.0], egui::Button::new("Reset All"))
                .clicked()
            {
                self.shortcuts.ask_reset_all = true;
            }
        });
        self.shortcuts_questions(ui, shell, events);
    }

    /// One command's row: its label, its keys, and the ways to change
    /// them (R8 and R10).
    fn shortcut_row(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        layout: &SettingsLayout,
        row: &ShortcutRow,
        events: &mut Vec<UiEvent>,
    ) {
        let platform = Platform::current();
        // The row: the command's label, its keys, and the ways to change
        // them, in Live's columns (L-SET-003).
        let rect = Rect::from_min_size(pos2(layout.label_x, ui.cursor().min.y), vec2(392.0, 18.0));
        let scope = egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center));
        ui.scope_builder(scope, |ui| {
            // The row itself, for assistive technology: the command's
            // label, its context and its keys (R12).
            let response = ui.interact(
                rect,
                Id::new(("shortcut-row", &row.command, &row.context)),
                Sense::hover(),
            );
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(Role::Row);
                node.set_label(row.label.clone());
                node.set_value(format!("{}: {}", row.context, row.keys.join(", ")));
            });
            let label_rect = Rect::from_min_size(rect.left_top(), vec2(196.0, 18.0));
            ui.put(label_rect, egui::Label::new(&row.label).truncate());
            // The keys, which the person clicks to record new ones (R8).
            let recording = self.shortcuts.recording.as_ref().is_some_and(|recording| {
                recording.command == row.command && recording.context == row.context
            });
            let text = if recording {
                "Press keys".to_owned()
            } else {
                row.keys.join(", ")
            };
            let keys_rect =
                Rect::from_min_size(pos2(rect.left() + 200.0, rect.top()), vec2(84.0, 18.0));
            let keys = ui.put(keys_rect, egui::Button::new(text.clone()));
            ui.ctx().accesskit_node_builder(keys.id, |node| {
                node.set_label(format!("{} keys", row.label));
                node.set_value(text);
            });
            if keys.clicked() {
                self.shortcuts.recording = Some(Recording {
                    command: row.command.clone(),
                    context: row.context.clone(),
                });
                self.shortcuts.conflict = None;
            }
            if row.own {
                ui.painter().rect_stroke(
                    keys.rect,
                    0.0,
                    Stroke::new(1.0, theme.colors.accent.0),
                    StrokeKind::Inside,
                );
            }
            // Remove: takes the shortcut away, whether it is Live's or the
            // person's own (R8).
            if !row.keys.is_empty() {
                let remove_rect =
                    Rect::from_min_size(pos2(rect.left() + 288.0, rect.top()), vec2(50.0, 18.0));
                let remove = ui.put(remove_rect, egui::Button::new("Remove"));
                ui.ctx().accesskit_node_builder(remove.id, |node| {
                    node.set_label(format!("Remove {}'s keys", row.label));
                });
                if remove.clicked() {
                    events.push(UiEvent::Invoke(Invocation::new(
                        ids::KEYMAP_UNBIND,
                        json!({
                            "command": row.command,
                            "keys": row.keys,
                            "platform": platform.id(),
                        }),
                    )));
                }
            }
            // Reset: gives the command Live's own keys back (R10).
            if row.changed {
                let reset_rect =
                    Rect::from_min_size(pos2(rect.left() + 342.0, rect.top()), vec2(50.0, 18.0));
                let reset = ui.put(reset_rect, egui::Button::new("Reset"));
                ui.ctx().accesskit_node_builder(reset.id, |node| {
                    node.set_label(format!("Reset {}'s keys", row.label));
                });
                if reset.clicked() {
                    events.push(UiEvent::Invoke(Invocation::new(
                        ids::KEYMAP_RESET,
                        json!({ "command": row.command }),
                    )));
                }
            }
        });
        ui.advance_cursor_after_rect(rect);
    }

    /// The questions the Shortcuts page asks: about keys that already run
    /// another command (R9), and about resetting every shortcut (R10).
    fn shortcuts_questions(&mut self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let platform = Platform::current();
        if let Some(conflict) = self.shortcuts.conflict.clone() {
            let mut open = true;
            egui::Window::new("Replace the shortcut?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    let question = ui.label(format!(
                        "{} already runs {} in {}.",
                        conflict.keys.join(", "),
                        conflict.other,
                        conflict.other_context
                    ));
                    // The question is read as a dialog (R12).
                    ui.ctx().accesskit_node_builder(question.id, |node| {
                        node.set_label(format!(
                            "{} already runs {} in {}.",
                            conflict.keys.join(", "),
                            conflict.other,
                            conflict.other_context
                        ));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Replace").clicked() {
                            events.push(UiEvent::Invoke(Invocation::new(
                                ids::KEYMAP_BIND,
                                json!({
                                    "command": conflict.command,
                                    "keys": conflict.keys,
                                    "platform": platform.id(),
                                    "context": conflict.context,
                                    "replace": true,
                                }),
                            )));
                            self.shortcuts.conflict = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.shortcuts.conflict = None;
                        }
                    });
                });
            if !open {
                self.shortcuts.conflict = None;
            }
        }
        if self.shortcuts.ask_reset_all {
            let mut open = true;
            egui::Window::new("Reset every shortcut?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    let question = ui.label("Every shortcut goes back to Live's own keys.");
                    // The question is read as a dialog (R12).
                    ui.ctx().accesskit_node_builder(question.id, |node| {
                        node.set_label("Every shortcut goes back to Live's own keys.");
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Reset").clicked() {
                            events.push(UiEvent::Invoke(Invocation::new(
                                ids::KEYMAP_RESET,
                                json!({}),
                            )));
                            self.shortcuts.ask_reset_all = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.shortcuts.ask_reset_all = false;
                        }
                    });
                });
            if !open {
                self.shortcuts.ask_reset_all = false;
            }
        }
        let _ = shell;
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
                leds: NO_LEDS,
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
            leds: NO_LEDS,
        })
        .chain(
            devices
                .devices
                .iter()
                .filter(|d| d.input_channels > 0)
                .map(|d| Choice {
                    label: d.name.clone(),
                    value: d.id.clone(),
                    leds: NO_LEDS,
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
            leds: NO_LEDS,
        })
        .chain(
            devices
                .devices
                .iter()
                .filter(|d| d.output_channels > 0)
                .map(|d| Choice {
                    label: d.name.clone(),
                    value: d.id.clone(),
                    leds: NO_LEDS,
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
            leds: NO_LEDS,
        })
        .chain((1..=most.min(64)).map(|n| Choice {
            label: n.to_string(),
            value: n,
            leds: NO_LEDS,
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
            leds: NO_LEDS,
        })
        .chain(
            device
                .into_iter()
                .flat_map(|d| d.sample_rates.iter())
                .map(|r| Choice {
                    label: format!("{r} Hz"),
                    value: *r,
                    leds: NO_LEDS,
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
            leds: NO_LEDS,
        })
        .chain(
            device
                .map(DeviceListing::buffer_choices)
                .unwrap_or_default()
                .into_iter()
                .map(|b| Choice {
                    label: format!("{b} Samples"),
                    value: b,
                    leds: NO_LEDS,
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
            json!({ "percent": next as u16, "window": "main" }),
        )));
    }
    // Spec: interface-zoom R12. While the second window is open, its zoom
    // is a row of its own after Zoom.
    if shell.second_window_open() {
        let percent = shell.zoom_percent_of(bl_shell::WindowKind::Second);
        let rect = rows.row(ui, "Second Window Zoom", SettingsLayout::NUMBER);
        if let Some(next) = number_box(
            ui,
            theme,
            "second-window-zoom",
            "Second Window Zoom",
            rect,
            f64::from(percent),
            (f64::from(ZOOM_MIN_PERCENT), f64::from(ZOOM_MAX_PERCENT)),
            &|value, steps| f64::from(zoom_step(value as u16, steps)),
            &format!("{percent} %"),
        ) {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::VIEW_SET_ZOOM,
                json!({ "percent": next as u16, "window": "second" }),
            )));
        }
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
    // Spec: time-editing R26 (L-SET-010): Permanent Scrub Areas, the one
    // Mouse and Pen Input setting built so far.
    rows.heading(ui, "Mouse and Pen Input");
    let rect = rows.row(ui, "Permanent Scrub Areas", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "permanent-scrub-areas",
        "Permanent Scrub Areas",
        rect,
        shell.permanent_scrub_areas(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_PERMANENT_SCRUB_AREAS,
            json!({ "on": on }),
        )));
    }
    rows.note(
        ui,
        "The zoom scales the main window and this one; View > Zoom and its keys change the \
         zoom of the window in front. The rest of the Mouse and Pen Input settings and the \
         Application Options settings come with later packages.",
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

/// The Record section of Record, Warp & Launch, as observed in Live 12.4.6,
/// without Record Session automation in (WP-3.12), which is built and sits
/// between its fifth row and its last. None of the rest is built.
const RECORD: [Pending; 6] = [
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
    switch_row("Start Transport With Record", "WP-2.13").showing("On"),
];

/// Record, Warp & Launch (L-SET-018, as observed in Live 12.4.6): the
/// Record, Warp/Fades, Launch, and Tap Tempo sections in Live's order, with
/// the Warp/Fades rows (WP-4.9, WP-1.4b) with Warp recorded audio, a
/// Beatlayer addition after them (IMP-02), the Launch section's rows
/// (WP-3.4), and Start Transport With Tap Tempo (WP-7.31), the only settings
/// built. The other rows keep their places,
/// pending.
fn record_warp_launch_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Record");
    let (before, after) = RECORD.split_at(RECORD.len() - 1);
    for row in before {
        rows.pending(ui, row);
    }
    let choices = [
        (SessionAutomationTracks::ArmedTracks, "Armed Tracks"),
        (SessionAutomationTracks::AllTracks, "All Tracks"),
    ]
    .map(|(value, label)| Choice {
        label: label.to_owned(),
        value,
        leds: NO_LEDS,
    });
    let rect = rows.row(ui, "Record Session automation in", SettingsLayout::CHOOSER);
    if let Some(tracks) = chooser(
        ui,
        theme,
        "record-session-automation-in",
        "Record Session automation in",
        rect,
        &choices,
        Some(&shell.record_session_automation_in()),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_RECORD_SESSION_AUTOMATION_IN,
            json!({ "tracks": tracks }),
        )));
    }
    for row in after {
        rows.pending(ui, row);
    }
    rows.heading(ui, "Warp/Fades");
    warp_rows(ui, shell, theme, rows, events);
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
    rows.heading(ui, "Launch");
    launch_rows(ui, shell, theme, rows, events);
    rows.heading(ui, "Tap Tempo");
    // Spec: control-bar-transport R20.
    let rect = rows.row(ui, "Start Transport With Tap Tempo", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "start-transport-with-tap-tempo",
        "Start Transport With Tap Tempo",
        rect,
        shell.start_transport_with_tap_tempo(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_START_TRANSPORT_WITH_TAP_TEMPO,
            json!({ "on": on }),
        )));
    }
}

/// The Warp/Fades rows before Create Fades on Clip Edges (WP-4.9), in
/// Live's order.
fn warp_rows(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    let prefs = shell.warp_prefs();
    let shorts: Vec<Choice<LoopWarpShortSamples>> = LoopWarpShortSamples::ALL
        .into_iter()
        .map(|value| Choice {
            label: value.label().to_owned(),
            value,
            leds: NO_LEDS,
        })
        .collect();
    let rect = rows.row(ui, "Loop/Warp Short Samples", SettingsLayout::CHOOSER);
    if let Some(mode) = chooser(
        ui,
        theme,
        "loop-warp-short-samples",
        "Loop/Warp Short Samples",
        rect,
        &shorts,
        Some(&prefs.loop_warp_short_samples),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_LOOP_WARP_SHORT_SAMPLES,
            json!({ "mode": mode }),
        )));
    }
    let rect = rows.row(ui, "Auto-Warp Long Samples", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "auto-warp-long-samples",
        "Auto-Warp Long Samples",
        rect,
        prefs.auto_warp_long_samples,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_AUTO_WARP_LONG_SAMPLES,
            json!({ "on": on }),
        )));
    }
    let modes: Vec<Choice<DefaultWarpMode>> = DefaultWarpMode::ALL
        .into_iter()
        .map(|value| Choice {
            label: value.label().to_owned(),
            value,
            leds: NO_LEDS,
        })
        .collect();
    let rect = rows.row(ui, "Default Warp Mode", SettingsLayout::CHOOSER);
    if let Some(mode) = chooser(
        ui,
        theme,
        "default-warp-mode",
        "Default Warp Mode",
        rect,
        &modes,
        Some(&prefs.default_warp_mode),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_DEFAULT_WARP_MODE,
            json!({ "mode": mode }),
        )));
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
            leds: NO_LEDS,
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
        leds: NO_LEDS,
    })
    .chain(LaunchQuantization::ALL.into_iter().map(|step| Choice {
        label: step.label().to_owned(),
        value: Some(step),
        leds: NO_LEDS,
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

/// Plug-Ins (L-SET-017): Plug-In Sources (WP-2.9b): Rescan, which scans
/// the formats switched on afresh in the background, and the switches that
/// say which formats are scanned and listed; Live's VST2 rows are left out,
/// as Beatlayer doesn't host VST2 (the parity matrix: Out), and so is the
/// VST3 custom folder, not built yet. Then Plug-In Windows, whose Auto-Hide
/// Plug-In Windows is built (WP-2.5b): floating devices' windows follow it
/// as Live's plug-in windows do. Multiple and Auto-Open keep their places,
/// pending.
fn plug_ins_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Plug-In Sources");
    let rect = rows.row(ui, "Rescan Plug-Ins", SettingsLayout::SWITCH);
    if push_button(
        ui,
        theme,
        "rescan-plug-ins",
        "Rescan Plug-Ins",
        "Rescan",
        rect,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::plugin_commands::RESCAN,
            json!({ "background": true }),
        )));
    }
    let sources = shell.plugin_settings();
    let switches = [
        (
            "Use Audio Units v2",
            "use-audio-units-v2",
            sources.use_audio_units_v2,
            "use_audio_units_v2",
        ),
        (
            "Use Audio Units v3",
            "use-audio-units-v3",
            sources.use_audio_units_v3,
            "use_audio_units_v3",
        ),
        (
            "Use VST3 Plug-In System Folders",
            "vst3-system-folders",
            sources.vst3_system_folders,
            "vst3_system_folders",
        ),
    ];
    for (label, id_salt, value, field) in switches {
        let rect = rows.row(ui, label, SettingsLayout::SWITCH);
        if let Some(on) = switch(ui, theme, id_salt, label, rect, value) {
            events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::plugin_commands::SETTINGS,
                json!({ field: on }),
            )));
        }
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

/// A push button in the Settings window, as Live's Rescan is: a rounded
/// face with its text. Returns whether it was pressed.
fn push_button(
    ui: &mut Ui,
    theme: &Theme,
    id_salt: &str,
    name: &str,
    text: &str,
    rect: Rect,
) -> bool {
    let response = ui.interact(rect, Id::new(("push-button", id_salt)), Sense::click());
    let pressed = response.clicked()
        || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    let face = if response.hovered() {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.rect_filled(rect, rect.height() / 2.0, face);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.font(),
        c.text.0,
    );
    // Spec: accessibility R7: the outline follows Outline View in Focus.
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            rect.height() / 2.0,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    pressed
}

/// Theme & Colors (L-SET-011): the Theme and Customization sections
/// (themes R1, R2, R3, R4, R5, R7, R13); the Track and Clip Colors section
/// comes with the colours of tracks, clips and scenes (WP-6.30b).
fn theme_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    fn choices<T: Clone>(items: impl IntoIterator<Item = (String, T)>) -> Vec<Choice<T>> {
        items
            .into_iter()
            .map(|(label, value)| Choice {
                label,
                value,
                leds: NO_LEDS,
            })
            .collect()
    }
    let look = shell.look();
    rows.heading(ui, "Theme");
    let rect = rows.row(ui, "Theme", SettingsLayout::CHOOSER);
    let themes = choices(shell.themes().iter().map(|t| (t.clone(), t.clone())));
    if let Some(name) = chooser(
        ui,
        theme,
        "theme",
        "Theme",
        rect,
        &themes,
        Some(&look.theme),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_THEME,
            json!({ "theme": name }),
        )));
    }
    let rect = rows.row(ui, "Appearance", SettingsLayout::CHOOSER);
    let appearances = choices(Appearance::ALL.map(|a| (a.label().to_owned(), a)));
    if let Some(appearance) = chooser(
        ui,
        theme,
        "appearance",
        "Appearance",
        rect,
        &appearances,
        Some(&shell.appearance()),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_APPEARANCE,
            json!({ "appearance": appearance }),
        )));
    }
    let rect = rows.row(ui, "Tone", SettingsLayout::CHOOSER);
    let tones = choices(Tone::ALL.map(|t| (t.label().to_owned(), t)));
    if let Some(tone) = chooser(
        ui,
        theme,
        "tone",
        "Tone",
        rect,
        &tones,
        Some(&look.tone),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_TONE,
            json!({ "tone": tone }),
        )));
    }
    let rect = rows.row(ui, "High Contrast", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "high-contrast",
        "High Contrast",
        rect,
        look.high_contrast,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_HIGH_CONTRAST,
            json!({ "on": on }),
        )));
    }
    rows.heading(ui, "Customization");
    // Each box: its label, command, argument, value, range with step, and
    // the unit its value is shown with.
    type Range = (i16, i16, i16, i16);
    let boxes: [(&str, &str, &str, i16, Range, &str); 4] = [
        (
            "Grid Line Intensity",
            ids::SETTINGS_SET_GRID_LINE_INTENSITY,
            "percent",
            look.grid_line_intensity,
            GRID_LINE_INTENSITY,
            " %",
        ),
        (
            "Brightness",
            ids::SETTINGS_SET_BRIGHTNESS,
            "value",
            look.brightness,
            BRIGHTNESS,
            "",
        ),
        (
            "Color Intensity",
            ids::SETTINGS_SET_COLOR_INTENSITY,
            "percent",
            look.color_intensity,
            COLOR_INTENSITY,
            " %",
        ),
        (
            "Color Hue",
            ids::SETTINGS_SET_COLOR_HUE,
            "degrees",
            look.color_hue,
            COLOR_HUE,
            "°",
        ),
    ];
    for (label, command, argument, value, (min, max, step, _), unit) in boxes {
        let rect = rows.row(ui, label, SettingsLayout::NUMBER);
        if let Some(next) = number_box(
            ui,
            theme,
            command,
            label,
            rect,
            f64::from(value),
            (f64::from(min), f64::from(max)),
            &|value, steps| value + f64::from(steps) * f64::from(step),
            &format!("{value}{unit}"),
        ) {
            events.push(UiEvent::Invoke(Invocation::new(
                command,
                json!({ argument: next as i16 }),
            )));
        }
    }
    // Spec: themes R23, R24 (L-SET-011).
    rows.heading(ui, "Track and Clip Colors");
    let rect = rows.row(ui, "Auto-Assign Track Colors", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "auto-assign-track-colours",
        "Auto-Assign Track Colors",
        rect,
        look.auto_assign_track_colours,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_AUTO_ASSIGN_TRACK_COLOURS,
            json!({ "on": on }),
        )));
    }
    let rect = rows.row(ui, "Default Track Color", SettingsLayout::CHOOSER);
    if let Some(colour) = swatch_chooser(
        ui,
        theme,
        "default-track-colour",
        "Default Track Color",
        rect,
        look.default_track_colour,
        !look.auto_assign_track_colours,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_DEFAULT_TRACK_COLOUR,
            json!({ "colour": colour }),
        )));
    }
    let rect = rows.row(ui, "Reduced Automatic Colors", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "reduced-automatic-colours",
        "Reduced Automatic Colors",
        rect,
        look.reduced_automatic_colours,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_REDUCED_AUTOMATIC_COLOURS,
            json!({ "on": on }),
        )));
    }
    let rect = rows.row(ui, "Clip Color", SettingsLayout::CHOOSER);
    let clip_colours = choices(ClipColour::ALL.map(|c| (c.label().to_owned(), c)));
    if let Some(clip_colour) = chooser(
        ui,
        theme,
        "clip-colour",
        "Clip Color",
        rect,
        &clip_colours,
        Some(&look.clip_colour),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_CLIP_COLOUR,
            json!({ "clip_colour": clip_colour }),
        )));
    }
}

/// A chooser of the palette's 70 swatches (Default Track Color): a button
/// showing the swatch, which opens the palette. Returns the swatch chosen.
fn swatch_chooser(
    ui: &mut Ui,
    theme: &Theme,
    id_salt: &str,
    name: &str,
    rect: Rect,
    current: u8,
    enabled: bool,
) -> Option<u8> {
    let id = Id::new(("chooser", id_salt));
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let mut chosen = None;
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let step = ui.input(|i| {
            i32::from(i.key_pressed(Key::ArrowDown)) - i32::from(i.key_pressed(Key::ArrowUp))
        });
        let next = (i32::from(current) + step).clamp(0, i32::from(bl_model::SWATCHES) - 1) as u8;
        if step != 0 && next != current {
            chosen = Some(next);
        }
    }
    let swatch = theme.swatch(current);
    let painter = ui.painter_at(rect.expand(3.0));
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if enabled {
            swatch
        } else {
            swatch.gamma_multiply(0.45)
        },
    );
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, enabled, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_value(format!(
            "Row {}, colour {}",
            current / crate::palette::COLUMNS + 1,
            current % crate::palette::COLUMNS + 1
        ));
    });
    egui::Popup::menu(&response).show(|ui| {
        if let Some(swatch) = crate::palette::show(ui, theme, id_salt, Some(current)) {
            chosen = Some(swatch);
            ui.close();
        }
    });
    chosen
}

/// File & Folder (L-SET-015): the Sample Editor and Temporary Folder rows,
/// with their Browse buttons and paths (WP-4.10b and WP-4.20), and the Save
/// Continuously switch (WP-6.62, the crash-recovery spec, R24). The page's
/// other rows come with later packages.
fn file_folder_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
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

    rows.heading(ui, "Sample Editor");
    let rect = rows.row(ui, "Sample Editor", SettingsLayout::CHOOSER);
    if ui.put(rect, egui::Button::new("Browse")).clicked() {
        // No path: the host asks for a program.
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_SAMPLE_EDITOR,
            json!({}),
        )));
    }
    let path = shell.sample_editor().unwrap_or_default();
    rows.copyable_row(ui, "Path", "sample-editor", &path);

    rows.heading(ui, "Saving");
    let rect = rows.row(ui, "Save Continuously", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "save-continuously",
        "Save Continuously",
        rect,
        shell.save_continuously(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            recovery_ids::SET_CONTINUOUS_SAVE,
            json!({ "on": on }),
        )));
    }
}

/// Library (L-SET-016): Collect Files on Export and the User Library's
/// folder (WP-6.34; file management R7, browser R24). Show Downloadable
/// Packs, Show Cloud, Show Push, Show Splice, the pack installation folder
/// and the Splice download folder are left out (D21).
fn library_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    use bl_shell::settings::CollectFiles;
    rows.heading(ui, "Collect Files on Export");
    let modes: Vec<Choice<CollectFiles>> = [
        ("Always", CollectFiles::Always),
        ("Never", CollectFiles::Never),
        ("Ask", CollectFiles::Ask),
    ]
    .into_iter()
    .map(|(label, value)| Choice {
        label: label.to_owned(),
        value,
        leds: NO_LEDS,
    })
    .collect();
    let rect = rows.row(ui, "Collect Files on Export", SettingsLayout::CHOOSER);
    let current = shell.collect_files_on_export();
    if let Some(mode) = chooser(
        ui,
        theme,
        "collect-files-on-export",
        "Collect Files on Export",
        rect,
        &modes,
        Some(&current),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_COLLECT_FILES_ON_EXPORT,
            json!({ "mode": mode }),
        )));
    }
    rows.heading(ui, "Locations");
    let rect = rows.row(ui, "User Library", SettingsLayout::CHOOSER);
    if ui.put(rect, egui::Button::new("Browse")).clicked() {
        // No path: the host asks for a folder.
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_USER_LIBRARY,
            json!({}),
        )));
    }
    let path = shell
        .browser_view()
        .user_library()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    rows.copyable_row(ui, "Path", "user-library", &path);
}

/// Link (L-SET-013; the Link spec, R20): the Link section's Show Link Toggle
/// and Start Stop Sync, then Link Audio's section (the Link Audio spec, R2):
/// Audio, Name, Latency, Sync to Incoming Audio and the peers list. While no
/// audio engine runs it holds only a message, and Link stays as it is set.
fn link_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    if !shell.audio().engine_running() {
        rows.read_note(
            ui,
            "Link needs the audio engine running. Switch it on in the Audio settings.",
        );
        return;
    }
    rows.heading(ui, "Link");
    let choices = [("Show", true), ("Hide", false)].map(|(label, value)| Choice {
        label: label.to_owned(),
        value,
        leds: NO_LEDS,
    });
    let rect = rows.row(ui, "Show Link Toggle", SettingsLayout::CHOOSER);
    if let Some(on) = chooser(
        ui,
        theme,
        "show-link-toggle",
        "Show Link Toggle",
        rect,
        &choices,
        Some(&shell.link_toggle_shown()),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            link_ids::SET_SHOW_TOGGLE,
            json!({ "on": on }),
        )));
    }
    let rect = rows.row(ui, "Start Stop Sync", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "link-start-stop-sync",
        "Start Stop Sync",
        rect,
        shell.link_start_stop_sync(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            link_ids::SET_START_STOP_SYNC,
            json!({ "on": on }),
        )));
    }
    link_audio_section(ui, shell, theme, rows, events);
}

/// The width of the Latency slider and the Name field.
const LINK_AUDIO_FIELD: f32 = 160.0;

/// Link Audio (the Link Audio spec, R2, R3, R7, R8, R9, R13): Audio, Name,
/// Latency and Sync to Incoming Audio, and the peers list, a table of each
/// peer's name, state and buffered audio.
fn link_audio_section(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Link Audio");
    let rect = rows.row(ui, "Audio", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "link-audio",
        // The page list has an Audio of its own, so the switch's name says
        // what it switches.
        "Link Audio",
        rect,
        shell.link_audio_enabled(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            link_ids::SET_AUDIO,
            json!({ "on": on }),
        )));
    }
    let rect = rows.row(ui, "Name", LINK_AUDIO_FIELD);
    if let Some(name) = committed_field(
        ui,
        rect,
        "link-audio-name",
        "Name",
        &shell.link_audio_name(),
        |edit| edit,
        Some("The name other apps in the Link session see this app by."),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            link_ids::SET_AUDIO_NAME,
            json!({ "name": name }),
        )));
    }
    let rect = rows.row(ui, "Latency", LINK_AUDIO_FIELD);
    let mut ms = shell.link_audio_latency_ms();
    let slider = ui.put(
        rect,
        egui::Slider::new(&mut ms, 0.0..=link_ids::MAX_LATENCY_MS)
            .integer()
            .suffix(" ms"),
    );
    ui.ctx().accesskit_node_builder(slider.id, |node| {
        node.set_label("Latency".to_owned());
    });
    if slider.changed() {
        events.push(UiEvent::Invoke(Invocation::new(
            link_ids::SET_AUDIO_LATENCY,
            json!({ "ms": ms }),
        )));
    }
    let rect = rows.row(ui, "Sync to Incoming Audio", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "link-sync-to-incoming-audio",
        "Sync to Incoming Audio",
        rect,
        shell.link_sync_to_incoming_audio(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            link_ids::SET_SYNC_TO_INCOMING_AUDIO,
            json!({ "on": on }),
        )));
    }
    link_audio_peers(ui, shell, theme, rows);
}

/// The peers list: a table whose rows are the peers with Link Audio on, each
/// by name, with its state and the audio buffered from it in milliseconds
/// (the Link Audio spec, R9, R13).
fn link_audio_peers(ui: &mut Ui, shell: &Shell, theme: &Theme, rows: &mut Rows<'_>) {
    let widths = [140.0, 90.0, 70.0];
    let table = |ui: &mut Ui, role: Role, label: &str, text: &str, rect: Rect| {
        let response = ui.interact(rect, Id::new(("link-peers", label, text)), Sense::hover());
        ui.painter().text(
            pos2(rect.left() + 2.0, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            theme.font(),
            if role == Role::ColumnHeader {
                theme.colors.text_dim.0
            } else {
                theme.colors.text.0
            },
        );
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(role);
            node.set_label(text.to_owned());
        });
    };
    let cells = rows.table_row(ui, "Peers", &widths);
    for (rect, heading) in cells.iter().zip(["Name", "State", "Buffered"]) {
        table(ui, Role::ColumnHeader, "head", heading, *rect);
    }
    let peers = shell.link_audio_peers();
    if peers.is_empty() {
        rows.read_note(ui, "No other app in the session has Link Audio on.");
    }
    for peer in peers {
        let cells = rows.table_row(ui, "", &widths);
        let buffered = format!("{:.0} ms", peer.buffered_ms);
        for (rect, text) in
            cells
                .iter()
                .zip([peer.name.as_str(), peer.state.label(), buffered.as_str()])
        {
            table(ui, Role::Cell, &peer.name, text, *rect);
        }
    }
}

/// The Tempo Follower section (L-SET-014; the Tempo Follower spec, R1): Show
/// Tempo Follower Toggle, and Input Channel, the audio input's single
/// channels then its stereo pairs, as the Routing choosers list them. A
/// channel the device hasn't, which the preferences keep, is listed too.
fn tempo_follower_rows(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    rows.heading(ui, "Tempo Follower");
    let choices = [("Show", true), ("Hide", false)].map(|(label, value)| Choice {
        label: label.to_owned(),
        value,
        leds: NO_LEDS,
    });
    let rect = rows.row(ui, "Show Tempo Follower Toggle", SettingsLayout::CHOOSER);
    if let Some(on) = chooser(
        ui,
        theme,
        "show-tempo-follower-toggle",
        "Show Tempo Follower Toggle",
        rect,
        &choices,
        Some(&shell.tempo_follower_toggle_shown()),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            tempo_follower_ids::SET_SHOW_TOGGLE,
            json!({ "on": on }),
        )));
    }
    let inputs = shell.audio().channels().0;
    let current = shell.tempo_follower_input();
    let mut channels: Vec<bl_model::Channel> = (1..=inputs)
        .map(bl_model::Channel::Mono)
        .chain((1..inputs).step_by(2).map(bl_model::Channel::Pair))
        .collect();
    if !channels.contains(&current) {
        channels.push(current);
    }
    let channels: Vec<Choice<bl_model::Channel>> = channels
        .into_iter()
        .map(|value| Choice {
            label: value.label(),
            value,
            leds: NO_LEDS,
        })
        .collect();
    let rect = rows.row(ui, "Input Channel", SettingsLayout::CHOOSER);
    if let Some(channel) = chooser(
        ui,
        theme,
        "tempo-follower-input-channel",
        "Input Channel",
        rect,
        &channels,
        Some(&current),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            tempo_follower_ids::SET_INPUT,
            json!({ "channel": channel }),
        )));
    }
}

/// Tempo & MIDI (L-SET-014): the second section's Show Resync Button, and the
/// MIDI ports table (WP-2.2), each input and output with its Track, Sync, and
/// Remote switches, and an input its MPE switch after them, as the MIDI I/O
/// spec R2 and R27 give it. An output with Sync on has a disclosure triangle
/// after its switches that opens its MIDI Clock Sync Delay, MIDI Clock Type
/// and Hardware Resync (the MIDI clock spec, R1). Takeover Mode comes before
/// the ports (the remote mapping spec, R28). The page starts with the Tempo
/// Follower section: Show Tempo Follower Toggle and Input Channel (the Tempo
/// Follower spec, R1). Remote is kept and acts when MIDI mapping is built
/// (R5).
fn tempo_midi_page(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    tempo_follower_rows(ui, shell, theme, rows, events);
    // Spec: midi-clock R10
    let rect = rows.row(ui, "Show Resync Button", SettingsLayout::SWITCH);
    if let Some(on) = switch(
        ui,
        theme,
        "show-resync-button",
        "Show Resync Button",
        rect,
        shell.resync_button_shown(),
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            midi_ids::SET_SHOW_RESYNC_BUTTON,
            json!({ "on": on }),
        )));
    }
    rows.heading(ui, "MIDI");
    control_surfaces_table(ui, shell, theme, rows, events);
    // Spec: remote-mapping R28. Takeover Mode sits where Live has it: after
    // the Control Surfaces table's place and before the MIDI ports table.
    let modes: Vec<Choice<TakeoverMode>> = TakeoverMode::ALL
        .into_iter()
        .map(|mode| Choice {
            label: mode.label().to_owned(),
            value: mode,
            leds: NO_LEDS,
        })
        .collect();
    let rect = rows.row(ui, "Takeover Mode", SettingsLayout::CHOOSER);
    let current = shell.takeover_mode();
    if let Some(mode) = chooser(
        ui,
        theme,
        "takeover-mode",
        "Takeover Mode",
        rect,
        &modes,
        Some(&current),
        true,
    ) {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SETTINGS_SET_TAKEOVER_MODE,
            json!({ "mode": mode }),
        )));
    }
    rows.heading(ui, "MIDI Ports");
    let ports = shell.midi().ports();
    let matches = shell.template_matches();
    if ports.is_empty() {
        rows.note(ui, "The computer offers no MIDI ports.");
    }
    for port in ports {
        let label = format!("{}: {}", port.direction.label(), port.name);
        let output = port.direction == midi_ids::PortDirection::Output;
        // Spec: midi-io R27. Only an input has an MPE switch, after Remote.
        let mut switches = vec![
            ("Track", port.track),
            ("Sync", port.sync),
            ("Remote", port.remote),
        ];
        if !output {
            switches.push(("MPE", port.mpe));
        }
        // Spec: midi-clock R1, R14. A port with Sync on has the triangle: an
        // output's after its switches, an input's before them, where an
        // input's row, a switch longer, has no room.
        let triangle = port.sync;
        let mut widths = vec![SettingsLayout::SWITCH; switches.len()];
        if triangle && output {
            widths.push(SettingsLayout::TRIANGLE);
        }
        let all = rows.row_of(ui, &label, &widths);
        let before = (triangle && !output).then(|| {
            Rect::from_min_size(
                pos2(all[0].left() - 22.0, all[0].top()),
                vec2(SettingsLayout::TRIANGLE, 18.0),
            )
        });
        let mut rects = all.into_iter();
        for ((name, on), rect) in switches.into_iter().zip(rects.by_ref()) {
            let salt = format!("midi-port-{}-{}-{name}", port.direction as u8, port.name);
            if let Some(on) = switch(ui, theme, &salt, &format!("{label} {name}"), rect, on) {
                let mut args = json!({ "direction": port.direction, "name": port.name });
                args[name.to_lowercase()] = json!(on);
                events.push(UiEvent::Invoke(Invocation::new(midi_ids::SET_PORT, args)));
            }
        }
        if triangle {
            let open = match before.or_else(|| rects.next()) {
                Some(rect) => disclosure(ui, &format!("{label} Sync Options"), rect),
                None => false,
            };
            if open {
                sync_options(ui, theme, rows, events, &port);
            }
        }
        // Spec: control-surfaces R45. A template that names the port is
        // noted under it.
        for (_, template) in matches.iter().filter(|(name, _)| *name == port.name) {
            rows.read_note(ui, &format!("Matches {template}"));
        }
        if let Some(error) = &port.error {
            rows.note(ui, &format!("{label} couldn't be opened: {error}"));
        }
    }
}

/// The Control Surfaces table (L-SET-014; the control surfaces spec, R1, R2,
/// R11 and R14): six numbered rows, each a Control Surface chooser, an Input
/// chooser, an Output chooser and a Dump button, left to right. A port a row
/// names that the system doesn't offer shows by its name, marked missing.
/// Choosing runs `control_surface.set` and Dump runs `control_surface.dump`.
fn control_surfaces_table(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
) {
    let kinds = shell.control_surface_choices();
    let ports = shell.midi().ports();
    let offered = |direction| {
        ports
            .iter()
            .filter(move |port| port.direction == direction)
            .map(|port| port.name.clone())
            .collect::<Vec<_>>()
    };
    let (inputs, outputs) = (
        offered(midi_ids::PortDirection::Input),
        offered(midi_ids::PortDirection::Output),
    );
    // None, then the offered names, then a name the row keeps that isn't
    // offered, marked missing.
    let port_choices = |offered: &[String], kept: Option<&str>| -> Vec<Choice<Option<String>>> {
        let choice = |label: String, value: Option<String>| Choice {
            label,
            value,
            leds: NO_LEDS,
        };
        let mut choices = vec![choice("None".to_owned(), None)];
        choices.extend(offered.iter().map(|n| choice(n.clone(), Some(n.clone()))));
        if let Some(kept) = kept.filter(|kept| !offered.iter().any(|n| n == kept)) {
            choices.push(choice(format!("{kept} (missing)"), Some(kept.to_owned())));
        }
        choices
    };
    for row in shell.control_surface_rows() {
        let n = row.row;
        let rects = rows.table_row(ui, &n.to_string(), &[100.0, 100.0, 100.0, 44.0]);
        let surface_id = row.surface.as_ref().map(|s| s.id.clone());
        let mut surfaces = vec![Choice {
            label: "None".to_owned(),
            value: None,
            leds: NO_LEDS,
        }];
        surfaces.extend(kinds.iter().map(|kind| Choice {
            label: kind.name.clone(),
            value: Some(kind.id.clone()),
            leds: NO_LEDS,
        }));
        if let Some(id) = surface_id
            .as_ref()
            .filter(|id| !kinds.iter().any(|k| &k.id == *id))
        {
            surfaces.push(Choice {
                label: id.clone(),
                value: Some(id.clone()),
                leds: NO_LEDS,
            });
        }
        let current_input = row.input.as_ref().map(|p| p.name.clone());
        let current_output = row.output.as_ref().map(|p| p.name.clone());
        let mut set = |field: &str, value: Option<String>| {
            events.push(UiEvent::Invoke(Invocation::new(
                control_surface::SET,
                json!({ "row": n, field: value }),
            )));
        };
        let name = format!("Control Surface {n}");
        if let Some(chosen) = chooser(
            ui,
            theme,
            &name,
            &name,
            rects[0],
            &surfaces,
            Some(&surface_id),
            true,
        ) {
            set("surface", chosen);
        }
        let name = format!("Input {n}");
        if let Some(chosen) = chooser(
            ui,
            theme,
            &name,
            &name,
            rects[1],
            &port_choices(&inputs, current_input.as_deref()),
            Some(&current_input),
            true,
        ) {
            set("input", chosen);
        }
        let name = format!("Output {n}");
        if let Some(chosen) = chooser(
            ui,
            theme,
            &name,
            &name,
            rects[2],
            &port_choices(&outputs, current_output.as_deref()),
            Some(&current_output),
            true,
        ) {
            set("output", chosen);
        }
        let dump = ui
            .add_enabled_ui(row.can_dump, |ui| {
                ui.put(rects[3], egui::Button::new("Dump"))
            })
            .inner;
        let label = format!("Dump {n}");
        ui.ctx().accesskit_node_builder(dump.id, |node| {
            node.set_label(label);
        });
        if dump.clicked() {
            events.push(UiEvent::Invoke(Invocation::new(
                control_surface::DUMP,
                json!({ "row": n }),
            )));
        }
        if let Some(error) = &row.error {
            rows.note(ui, &format!("Row {n} stopped: {error}"));
        }
    }
}

/// A disclosure triangle in `rect` that opens and closes on a click, kept in
/// the context's memory; returns whether it is open.
fn disclosure(ui: &mut Ui, name: &str, rect: Rect) -> bool {
    let id = Id::new(("disclosure", name));
    let response = ui.interact(rect, id, Sense::click());
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    let ink = ui.visuals().text_color();
    let c = rect.center();
    let points = if open {
        vec![
            c + vec2(-4.0, -2.0),
            c + vec2(4.0, -2.0),
            c + vec2(0.0, 3.0),
        ]
    } else {
        vec![
            c + vec2(-2.0, -4.0),
            c + vec2(3.0, 0.0),
            c + vec2(-2.0, 4.0),
        ]
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(points, ink, egui::Stroke::NONE));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::DisclosureTriangle);
        node.set_label(name.to_owned());
        node.set_expanded(open);
    });
    open
}

/// A port's sync options, the rows under its port row: an output's three, and
/// an input's Sync Type, Sync Delay, and its timecode rows while the type is
/// MIDI Timecode (the MIDI clock spec, R1, R14; the MIDI Timecode spec, R1).
fn sync_options(
    ui: &mut Ui,
    theme: &Theme,
    rows: &mut Rows<'_>,
    events: &mut Vec<UiEvent>,
    port: &midi_ids::PortInfo,
) {
    let set = |events: &mut Vec<UiEvent>, field: &str, value: Value| {
        let mut args = json!({ "direction": port.direction, "name": port.name });
        args[field] = value;
        events.push(UiEvent::Invoke(Invocation::new(midi_ids::SET_PORT, args)));
    };
    let name = |option: &str| format!("{}: {} {option}", port.direction.label(), port.name);
    let delay = i32::from(port.sync_delay_ms);
    let input = port.direction == midi_ids::PortDirection::Input;
    if input {
        // Spec: midi-timecode R1. An input follows clock or timecode, and
        // timecode has a frame rate and an offset.
        let types: Vec<Choice<midi_ids::SyncTypeChoice>> = midi_ids::SyncTypeChoice::ALL
            .into_iter()
            .map(|value| Choice {
                label: value.label().to_owned(),
                value,
                leds: NO_LEDS,
            })
            .collect();
        let rect = rows.row(ui, "MIDI Sync Type", SettingsLayout::CHOOSER);
        if let Some(value) = chooser(
            ui,
            theme,
            &name("sync type"),
            &name("MIDI Sync Type"),
            rect,
            &types,
            Some(&port.sync_type),
            true,
        ) {
            set(events, "sync_type", json!(value));
        }
    }
    let delay_label = if input {
        "Sync Delay"
    } else {
        "MIDI Clock Sync Delay"
    };
    let rect = rows.row(ui, delay_label, SettingsLayout::NUMBER);
    if let Some(ms) = number_box(
        ui,
        theme,
        &name("delay"),
        &name(delay_label),
        rect,
        f64::from(delay),
        (
            f64::from(*midi_ids::SYNC_DELAY_RANGE.start()),
            f64::from(*midi_ids::SYNC_DELAY_RANGE.end()),
        ),
        &|ms, steps| ms + f64::from(steps),
        &format!("{delay} ms"),
    ) {
        set(events, "sync_delay_ms", json!(ms.round() as i16));
    }
    if input {
        if port.sync_type == midi_ids::SyncTypeChoice::MidiTimecode {
            let rates: Vec<Choice<midi_ids::TimecodeRateChoice>> =
                midi_ids::TimecodeRateChoice::ALL
                    .into_iter()
                    .map(|value| Choice {
                        label: value.label().to_owned(),
                        value,
                        leds: NO_LEDS,
                    })
                    .collect();
            let rect = rows.row(ui, "MIDI Timecode Frame Rate", SettingsLayout::CHOOSER);
            if let Some(value) = chooser(
                ui,
                theme,
                &name("timecode frame rate"),
                &name("MIDI Timecode Frame Rate"),
                rect,
                &rates,
                Some(&port.timecode_rate),
                true,
            ) {
                set(events, "timecode_rate", json!(value));
            }
            let rect = rows.row(ui, "MIDI Timecode Offset", SettingsLayout::NUMBER + 30.0);
            if let Some(text) = committed_field(
                ui,
                rect,
                &name("timecode offset"),
                &name("MIDI Timecode Offset"),
                &port.timecode_offset,
                |edit| edit.hint_text("hh:mm:ss:ff"),
                Some("The time at which the Arrangement starts, as hours:minutes:seconds:frames."),
            ) {
                set(events, "timecode_offset", json!(text));
            }
        }
        return;
    }
    let types: Vec<Choice<midi_ids::ClockTypeChoice>> = midi_ids::ClockTypeChoice::ALL
        .into_iter()
        .map(|value| Choice {
            label: value.label().to_owned(),
            value,
            leds: NO_LEDS,
        })
        .collect();
    let rect = rows.row(ui, "MIDI Clock Type", SettingsLayout::CHOOSER);
    if let Some(value) = chooser(
        ui,
        theme,
        &name("clock type"),
        &name("MIDI Clock Type"),
        rect,
        &types,
        Some(&port.clock_type),
        true,
    ) {
        set(events, "clock_type", json!(value));
    }
    let resyncs: Vec<Choice<midi_ids::HardwareResyncChoice>> = midi_ids::HardwareResyncChoice::ALL
        .into_iter()
        .map(|value| Choice {
            label: value.label().to_owned(),
            value,
            leds: NO_LEDS,
        })
        .collect();
    let rect = rows.row(ui, "Hardware Resync", SettingsLayout::CHOOSER);
    if let Some(value) = chooser(
        ui,
        theme,
        &name("hardware resync"),
        &name("Hardware Resync"),
        rect,
        &resyncs,
        Some(&port.hardware_resync),
        true,
    ) {
        set(events, "hardware_resync", json!(value));
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

/// A text field that holds what is typed while it has the keyboard and
/// returns the text once, when it loses focus (Enter, a click elsewhere):
/// the shell trims what it is sent and saves the preferences, so sending on
/// every key would drop a typed space and write the file for each key. Esc
/// abandons the edit.
fn committed_field(
    ui: &mut Ui,
    rect: Rect,
    salt: &str,
    label: &str,
    current: &str,
    style: impl FnOnce(egui::TextEdit<'_>) -> egui::TextEdit<'_>,
    hover: Option<&str>,
) -> Option<String> {
    let id = ui.id().with(salt);
    let mut draft = ui.data(|d| d.get_temp::<String>(id));
    let mut text = match &draft {
        Some(draft) if ui.memory(|m| m.has_focus(id)) => draft.clone(),
        _ => current.to_owned(),
    };
    let mut response = ui.put(rect, style(egui::TextEdit::singleline(&mut text).id(id)));
    if let Some(hover) = hover {
        response = response.on_hover_text(hover);
    }
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label.to_owned());
    });
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
        return None;
    }
    if response.changed() {
        draft = Some(text);
    }
    ui.data_mut(|d| d.remove_temp::<String>(id));
    let escaped = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape));
    draft.filter(|draft| response.lost_focus() && !escaped && draft.trim() != current)
}

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
            leds: NO_LEDS,
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
    if let Some(base_url) = committed_field(
        ui,
        rect,
        "assistant-base-url",
        "Assistant Base URL",
        &settings.base_url,
        |edit| {
            edit.hint_text("The provider's own")
                .interactive(settings.provider.has_base_url())
        },
        Some(
            "The OpenAI-compatible server's address, such as http://localhost:11434/v1 for \
             Ollama. Empty uses the provider's own.",
        ),
    ) {
        invoke(
            events,
            assistant::ASSISTANT_SET_BASE_URL,
            json!({ "base_url": base_url }),
        );
    }

    let rect = rows.row(ui, "Model", ASSISTANT_FIELD);
    if let Some(model) = committed_field(
        ui,
        rect,
        "assistant-model",
        "Assistant Model",
        &settings.model,
        |edit| edit,
        None,
    ) {
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

    /// A row of a table: a short label in the label column, then controls
    /// of these widths side by side right after it, where they fit the
    /// window (the option column starts too far right for a row of three
    /// choosers and a button).
    fn table_row(&mut self, ui: &Ui, label: &str, widths: &[f32]) -> Vec<Rect> {
        self.label(ui, label);
        let mut x = self.layout.label_x + 18.0;
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

    /// A note that assistive technology can read, as a value row is: the
    /// text, labelled by itself.
    fn read_note(&mut self, ui: &Ui, text: &str) {
        let top = self.y + 4.0;
        self.note(ui, text);
        let rect = Rect::from_min_max(
            pos2(self.layout.label_x, top),
            pos2(self.right(), self.y - 6.0),
        );
        let response = ui.interact(
            rect,
            Id::new(("settings-note", text, top as i32)),
            Sense::hover(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Label);
            node.set_label(text.to_owned());
        });
    }
}
