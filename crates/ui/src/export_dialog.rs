//! The Export Audio/Video dialog (WP-6.71; the export spec, R28 to R45) and
//! the progress window of the export it starts: a modal over the main
//! window.
//!
//! The dialog holds, in the manual's order, the Selection, Rendering
//! Options, Encoding and Video sections, then Cancel and Export. Its
//! controls are the arguments of `export.audio`; the shell keeps their
//! values (`export.status`) and each change runs `export.set_choices`.
//! Export runs `export.start`, which asks for the file in a save dialog.
//! Esc closes the dialog and Return presses Export when it is enabled.
//! While an export runs, a window shows the file being written, how much
//! is rendered, and Cancel, which Esc presses too.

use bl_shell::Shell;
use bl_shell::export_commands::{
    self as ids, Disabled, ExportDialogStatus, FileType, RenderedTrack, RunningStatus, SAMPLE_RATES,
};
use bl_shell::export_dialog::{self as fields, Field, Range};
use egui::accesskit::{self, Role};
use egui::{
    Context, Event, Id, Key, Modifiers, Response, RichText, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// Points of vertical drag per step of a field, as the Control Bar's.
const DRAG_PER_STEP: f32 = 3.0;

/// The longest value a field takes typed in, in characters.
const MAX_TYPED: usize = 8;

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

fn set(events: &mut Vec<UiEvent>, patch: Value) {
    invoke(events, ids::SET_CHOICES, patch);
}

/// What is typed into one of the two fields, if anything: the field and
/// what was typed into each of its parts.
#[derive(Clone, Debug, Default)]
struct Entry {
    field: Option<(bool, [Option<String>; 3])>,
    /// Points dragged that haven't made a whole step yet.
    pending: f32,
}

fn entry_id() -> Id {
    Id::new("export-dialog-entry")
}

/// The ID of a part of Render Start or Render Length.
fn part_id(length: bool, part: usize) -> Id {
    Id::new(("export-dialog-part", length, part))
}

/// Draws the dialog if it's open, or the progress window while an export
/// runs.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    if let Some(status) = shell.export_running().then(|| shell.export_status()) {
        if let Some(running) = status.export {
            progress(ctx, theme, &running, events);
        }
        return;
    }
    let Some(dialog) = shell.export_dialog() else {
        ctx.data_mut(|d| d.remove::<Entry>(entry_id()));
        return;
    };
    // A value typed into a field that has lost focus is dropped, as Esc
    // would drop it.
    let typed_in = ctx
        .data(|d| d.get_temp::<Entry>(entry_id()))
        .and_then(|entry| entry.field.map(|(length, _)| length));
    let typing = typed_in.is_some_and(|length| {
        (0..3).any(|part| ctx.memory(|m| m.has_focus(part_id(length, part))))
    });
    if typed_in.is_some() && !typing {
        ctx.data_mut(|d| d.remove::<Entry>(entry_id()));
    }
    // Return presses Export and Esc closes the dialog (R28), unless a value
    // is being typed, which they set and drop.
    if !typing {
        if !egui::Popup::is_any_open(ctx) && take_key(ctx, Key::Enter) && dialog.can_export {
            invoke(events, ids::START, json!({}));
            return;
        }
        // An open chooser takes Esc first.
        if !egui::Popup::is_any_open(ctx) && take_key(ctx, Key::Escape) {
            invoke(events, ids::CLOSE_DIALOG, json!({}));
            return;
        }
    }
    egui::Modal::new(Id::new("export-dialog")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Export Audio/Video")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        contents(ui, shell, theme, &dialog, events);
    });
}

/// Takes a key press out of the frame's input, so a focused control doesn't
/// act on it too, and says whether it was pressed.
fn take_key(ctx: &Context, key: Key) -> bool {
    ctx.input_mut(|i| {
        let before = i.events.len();
        i.events.retain(|event| {
            !matches!(
                event,
                Event::Key { key: k, pressed: true, modifiers, .. }
                    if *k == key && !modifiers.command && !modifiers.ctrl && !modifiers.alt
            )
        });
        i.events.len() != before
    })
}

/// A section: its name, and its controls as a group assistive technology
/// reads by that name (R45).
fn section(ui: &mut Ui, theme: &Theme, name: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.label(
        RichText::new(name)
            .size(theme.sizes.heading * 0.85)
            .color(theme.colors.text_dim.0)
            .strong(),
    );
    // The group's node comes first, so the controls drawn in it hang from it.
    let group = Id::new(("export-dialog-section", name));
    ui.ctx().accesskit_node_builder(group, |node| {
        node.set_role(Role::Group);
        node.set_label(name.to_owned());
    });
    let inner = ui.scope_builder(UiBuilder::new().accessibility_parent(group), add);
    ui.interact(inner.response.rect, group, Sense::hover());
}

/// The reason a control is disabled, if it is.
fn reason<'a>(dialog: &'a ExportDialogStatus, control: &str) -> Option<&'a str> {
    dialog
        .disabled
        .iter()
        .find(|d: &&Disabled| d.control == control)
        .map(|d| d.reason.as_str())
}

/// Tells assistive technology why a control is unavailable (R45) and shows
/// the reason beside it.
fn explain(ui: &mut Ui, theme: &Theme, response: &Response, why: Option<&str>) {
    if let Some(why) = why {
        let why = why.to_owned();
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_description(why.clone());
        });
        ui.label(
            RichText::new(why)
                .font(theme.small_font())
                .color(theme.colors.text_dim.0),
        );
    }
}

/// Names a chooser for assistive technology: its label, which egui draws
/// beside it as text of its own.
fn named(ui: &Ui, response: &Response, label: &str) {
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label.to_owned());
    });
}

/// A switch: on or off, with its label.
fn switch(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    on: bool,
    why: Option<&str>,
    events: &mut Vec<UiEvent>,
    field: &str,
) {
    let mut value = on;
    let response = ui.add_enabled(why.is_none(), egui::Checkbox::new(&mut value, label));
    if response.changed() {
        set(events, json!({ field: value }));
    }
    explain(ui, theme, &response, why);
}

/// A chooser: its label and the option shown, with the options to choose
/// from (an option can be disabled) and the reason when the whole chooser
/// is.
fn chooser<T: Clone + PartialEq>(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    shown: &str,
    options: &[(T, String, bool)],
    current: &T,
    why: Option<&str>,
) -> Option<T> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(why.is_none(), |ui| {
            let response = egui::ComboBox::from_label(label)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (value, text, enabled) in options {
                        let item = ui
                            .add_enabled_ui(*enabled, |ui| {
                                ui.selectable_label(value == current, text)
                            })
                            .inner;
                        if item.clicked() {
                            chosen = Some(value.clone());
                        }
                    }
                })
                .response;
            named(ui, &response, label);
            explain(ui, theme, &response, why);
        });
    });
    chosen
}

/// The dialog's controls, in R29's order.
fn contents(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    dialog: &ExportDialogStatus,
    events: &mut Vec<UiEvent>,
) {
    let choices = &dialog.choices;
    let document = shell.editor().document();
    section(ui, theme, "Selection", |ui| {
        // R30: Main, All Individual Tracks and Selected Tracks Only, then
        // each audio, MIDI and group track in display order, then the
        // returns.
        let mut options: Vec<(RenderedTrack, String, bool)> = vec![
            (RenderedTrack::Main, "Main".to_owned(), true),
            (
                RenderedTrack::AllTracks,
                "All Individual Tracks".to_owned(),
                true,
            ),
            (
                RenderedTrack::SelectedTracks,
                "Selected Tracks Only".to_owned(),
                true,
            ),
        ];
        let order = document.track_order();
        let returns: Vec<_> = document.returns().collect();
        for id in order
            .iter()
            .filter(|id| !returns.contains(id))
            .chain(returns.iter())
        {
            if let Some(track) = document.track(*id)
                && track.kind() != bl_model::TrackKind::Main
            {
                options.push((RenderedTrack::Track(*id), track.name().to_owned(), true));
            }
        }
        let shown = options
            .iter()
            .find(|(value, _, _)| *value == choices.rendered_track)
            .map_or("Main", |(_, text, _)| text.as_str())
            .to_owned();
        if let Some(track) = chooser(
            ui,
            theme,
            "Rendered Track",
            &shown,
            &options,
            &choices.rendered_track,
            None,
        ) {
            set(events, json!({ "rendered_track": track }));
        }
        range_field(ui, shell, theme, Field::Start, dialog, events);
        range_field(ui, shell, theme, Field::Length, dialog, events);
    });
    section(ui, theme, "Rendering Options", |ui| {
        switch(
            ui,
            theme,
            "Include Return and Main Effects",
            choices.include_return_and_main_effects,
            reason(dialog, "Include Return and Main Effects"),
            events,
            "include_return_and_main_effects",
        );
        switch(
            ui,
            theme,
            "Render as Loop",
            choices.render_as_loop,
            None,
            events,
            "render_as_loop",
        );
        switch(
            ui,
            theme,
            "Convert to Mono",
            choices.convert_to_mono,
            None,
            events,
            "convert_to_mono",
        );
        switch(
            ui,
            theme,
            "Normalize",
            choices.normalize,
            None,
            events,
            "normalize",
        );
        switch(
            ui,
            theme,
            "Create Analysis File",
            choices.create_analysis_file,
            None,
            events,
            "create_analysis_file",
        );
        let rate = choices.sample_rate.unwrap_or(SAMPLE_RATES[3]);
        let rates: Vec<(u32, String, bool)> = SAMPLE_RATES
            .iter()
            .map(|r| (*r, format!("{r} Hz"), true))
            .collect();
        if let Some(rate) = chooser(
            ui,
            theme,
            "Sample Rate",
            &format!("{rate} Hz"),
            &rates,
            &rate,
            None,
        ) {
            set(events, json!({ "sample_rate": rate }));
        }
    });
    section(ui, theme, "Encoding", |ui| {
        switch(
            ui,
            theme,
            "Encode PCM",
            choices.encode_pcm,
            None,
            events,
            "encode_pcm",
        );
        let types = [
            (FileType::Wav, "WAV".to_owned(), true),
            (FileType::Aiff, "AIFF".to_owned(), true),
            (FileType::Flac, "FLAC".to_owned(), true),
        ];
        if let Some(file_type) = chooser(
            ui,
            theme,
            "File Type",
            choices.file_type.name(),
            &types,
            &choices.file_type,
            reason(dialog, "File Type"),
        ) {
            set(events, json!({ "file_type": file_type }));
        }
        let depths = [
            (16u8, "16".to_owned(), true),
            (24, "24".to_owned(), true),
            (
                32,
                "32".to_owned(),
                reason(dialog, "Bit Depth 32").is_none(),
            ),
        ];
        if let Some(bits) = chooser(
            ui,
            theme,
            "Bit Depth",
            &choices.bit_depth.to_string(),
            &depths,
            &choices.bit_depth,
            reason(dialog, "Bit Depth"),
        ) {
            set(events, json!({ "bit_depth": bits }));
        }
        let dithers = [
            (
                bl_shell::export_commands::Dither::None,
                "No Dither".to_owned(),
                true,
            ),
            (
                bl_shell::export_commands::Dither::Rectangular,
                "Rectangular".to_owned(),
                true,
            ),
            (
                bl_shell::export_commands::Dither::Triangular,
                "Triangular".to_owned(),
                true,
            ),
            (
                bl_shell::export_commands::Dither::NoiseShaped,
                "Noise Shaped".to_owned(),
                true,
            ),
        ];
        let shown = dithers
            .iter()
            .find(|(value, _, _)| *value == choices.dither)
            .map_or("Triangular", |(_, text, _)| text.as_str())
            .to_owned();
        if let Some(dither) = chooser(
            ui,
            theme,
            "Dither Options",
            &shown,
            &dithers,
            &choices.dither,
            reason(dialog, "Dither Options"),
        ) {
            set(events, json!({ "dither": dither }));
        }
        switch(
            ui,
            theme,
            "Encode MP3",
            choices.encode_mp3,
            None,
            events,
            "encode_mp3",
        );
    });
    section(ui, theme, "Video", |ui| {
        // R38: drawn disabled, as for a set without video clips.
        let why = reason(dialog, "Create Video");
        let mut off = false;
        let response = ui.add_enabled(false, egui::Checkbox::new(&mut off, "Create Video"));
        explain(ui, theme, &response, why);
        let response = ui
            .add_enabled_ui(false, |ui| {
                egui::ComboBox::from_label("Video Encoder")
                    .selected_text("")
                    .show_ui(ui, |_| {})
                    .response
            })
            .inner;
        named(ui, &response, "Video Encoder");
        explain(ui, theme, &response, reason(dialog, "Video Encoder"));
        ui.horizontal(|ui| {
            ui.add_enabled(false, egui::Label::new("Encoder Settings"));
            let response = ui.add_enabled(false, egui::Button::new("Edit"));
            explain(ui, theme, &response, reason(dialog, "Encoder Settings"));
        });
    });
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            invoke(events, ids::CLOSE_DIALOG, json!({}));
        }
        let export = ui.add_enabled(dialog.can_export, egui::Button::new("Export"));
        if export.clicked() {
            invoke(events, ids::START, json!({}));
        }
        if let Some(why) = dialog.reason.as_deref().filter(|_| !dialog.can_export) {
            let why = why.to_owned();
            ui.ctx().accesskit_node_builder(export.id, |node| {
                node.set_description(why.clone());
            });
            ui.label(
                RichText::new(why)
                    .font(theme.small_font())
                    .color(theme.colors.error_text.0),
            );
        }
    });
}

/// Render Start or Render Length: three parts, for bars, beats, and
/// sixteenths, each typed, dragged, or stepped as the Control Bar's Loop
/// Start and Loop Length fields are (R31).
fn range_field(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    field: Field,
    dialog: &ExportDialogStatus,
    events: &mut Vec<UiEvent>,
) {
    let (default_start, default_length) = shell.export_default_range();
    let default = Range {
        start: default_start,
        length: default_length,
    };
    let signatures = shell.editor().document().settings().time_signatures.clone();
    let range = Range {
        start: dialog.start,
        length: dialog.length,
    };
    let is_length = field == Field::Length;
    let shown = fields::parts(&signatures, field, range);
    let whole = shown.map(|part| part.to_string()).join(".");
    let mut entry: Entry = ui
        .ctx()
        .data(|d| d.get_temp(entry_id()))
        .unwrap_or_default();
    let typed = entry
        .field
        .as_ref()
        .filter(|(length, _)| *length == is_length)
        .map(|(_, typed)| typed.clone());
    let mut next: Option<Range> = None;
    ui.horizontal(|ui| {
        ui.label(field.name());
        for part in 0..3 {
            let text = typed
                .as_ref()
                .and_then(|typed| typed[part].clone())
                .unwrap_or_else(|| shown[part].to_string());
            let (rect, _) = ui.allocate_exact_size(Vec2::new(34.0, 20.0), Sense::hover());
            let response = ui.interact(rect, part_id(is_length, part), Sense::click_and_drag());
            let c = &theme.colors;
            let fill = if typed.as_ref().is_some_and(|t| t[part].is_some()) {
                c.control_hover.0
            } else {
                c.control.0
            };
            let painter = ui.painter_at(rect.expand(2.0));
            painter.rect_filled(rect, theme.sizes.corner, fill);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &text,
                theme.font(),
                c.text.0,
            );
            if response.has_focus() && crate::focus::outlined(ui.ctx()) {
                painter.rect_stroke(
                    rect,
                    theme.sizes.corner,
                    Stroke::new(1.5, c.focus.0),
                    StrokeKind::Outside,
                );
            }
            let name = format!("{} {}", field.name(), ["bars", "beats", "sixteenths"][part]);
            let value = shown[part] as f64;
            response.widget_info(|| egui::WidgetInfo::slider(true, value, &name));
            let description = format!(
                "{} {whole}. Type a value, or use Up and Down.",
                field.name()
            );
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(Role::SpinButton);
                node.set_value(shown[part].to_string());
                node.set_description(description.clone());
                node.set_numeric_value(value);
                node.add_action(accesskit::Action::Increment);
                node.add_action(accesskit::Action::Decrement);
            });
            if let Some(range) = interact(
                ui,
                &signatures,
                (field, range, default),
                part,
                &response,
                &mut entry,
            ) {
                next = Some(range);
            }
            if part < 2 {
                ui.label(".");
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(entry_id(), entry));
    if let Some(range) = next {
        let args = if is_length {
            json!({ "length": range.length })
        } else {
            json!({ "start": range.start })
        };
        set(events, args);
    }
}

/// What the person did to a part this frame: a click or a drag gives it the
/// keyboard; a drag up or down, Up and Down, or a screen reader's increment
/// and decrement step it; digits type a value, Period or Comma on to the
/// next part, Return sets it and Esc drops it; Delete and a double-click
/// give the field its opening value back. Returns the range it makes.
fn interact(
    ui: &mut Ui,
    signatures: &bl_core::time::TimeSignatureMap,
    (field, range, default): (Field, Range, Range),
    part: usize,
    response: &Response,
    entry: &mut Entry,
) -> Option<Range> {
    let is_length = field == Field::Length;
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    let mut result = None;
    if response.double_clicked() {
        entry.field = None;
        result = Some(default);
    }
    if response.dragged() {
        entry.pending -= response.drag_delta().y;
        let steps = (entry.pending / DRAG_PER_STEP).trunc();
        entry.pending -= steps * DRAG_PER_STEP;
        if steps != 0.0 {
            entry.field = None;
            result = Some(fields::step(signatures, field, range, part, steps as i32));
        }
    }
    let requested = ui.input(|i| {
        i.num_accesskit_action_requests(response.id, accesskit::Action::Increment) as i32
            - i.num_accesskit_action_requests(response.id, accesskit::Action::Decrement) as i32
    });
    if requested != 0 {
        result = Some(fields::step(signatures, field, range, part, requested));
    }
    if !response.has_focus() {
        return result;
    }
    let typing = entry
        .field
        .as_ref()
        .is_some_and(|(length, _)| *length == is_length);
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                vertical_arrows: true,
                escape: typing,
                ..Default::default()
            },
        );
    });
    let takes = |key: Key| {
        digit(key).is_some()
            || matches!(
                key,
                Key::Period
                    | Key::Comma
                    | Key::Backspace
                    | Key::ArrowUp
                    | Key::ArrowDown
                    | Key::Delete
            )
            || (typing && matches!(key, Key::Enter | Key::Escape))
    };
    let pressed: Vec<(Key, Modifiers)> = ui.input_mut(|i| {
        let mut taken = Vec::new();
        i.events.retain(|event| match event {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !modifiers.command && !modifiers.ctrl && takes(*key) => {
                taken.push((*key, *modifiers));
                false
            }
            _ => true,
        });
        taken
    });
    for (key, _) in pressed {
        let current = entry
            .field
            .get_or_insert_with(|| (is_length, Default::default()));
        if current.0 != is_length {
            *current = (is_length, Default::default());
        }
        if let Some(digit) = digit(key) {
            let typed = current.1[part].get_or_insert_with(String::new);
            if typed.len() < MAX_TYPED {
                typed.push(digit);
            }
            continue;
        }
        match key {
            Key::Period | Key::Comma if part + 1 < 3 => {
                ui.memory_mut(|m| m.request_focus(part_id(is_length, part + 1)));
            }
            Key::Backspace => {
                if let Some(typed) = current.1[part].as_mut() {
                    typed.pop();
                }
            }
            Key::Enter => {
                if let Some((_, typed)) = entry.field.take() {
                    result = fields::typed(signatures, field, range, &typed).or(result);
                }
            }
            Key::Escape => entry.field = None,
            Key::ArrowUp | Key::ArrowDown => {
                entry.field = None;
                let steps = if key == Key::ArrowUp { 1 } else { -1 };
                result = Some(fields::step(signatures, field, range, part, steps));
            }
            Key::Delete => {
                entry.field = None;
                result = Some(default);
            }
            _ => {}
        }
    }
    result
}

/// The digit a key types, if it's a digit key.
fn digit(key: Key) -> Option<char> {
    Some(match key {
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        _ => return None,
    })
}

/// The progress window (R41): the name of the file being written, a bar of
/// the share rendered across every file, and Cancel, which Esc presses too.
fn progress(ctx: &Context, theme: &Theme, running: &RunningStatus, events: &mut Vec<UiEvent>) {
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        invoke(events, ids::CANCEL, json!({}));
    }
    // The file being written: files render one after another, each a share
    // of the whole.
    let count = running.files.len().max(1);
    let at = ((running.progress * count as f64) as usize).min(count - 1);
    let name = running
        .files
        .get(at)
        .and_then(|file| std::path::Path::new(file).file_name())
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    egui::Modal::new(Id::new("export-progress")).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Exporting")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        ui.label(name);
        ui.add_space(6.0);
        bar(ui, theme, running.progress as f32, "Export progress");
        ui.label(
            RichText::new(format!("{:.0}%", running.progress * 100.0))
                .font(theme.small_font())
                .color(theme.colors.text_dim.0),
        );
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            if ui.button("Cancel").clicked() {
                invoke(events, ids::CANCEL, json!({}));
            }
        });
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

/// A progress bar, painted, read by a screen reader as a progress
/// indicator with its share.
pub(crate) fn bar(ui: &mut Ui, theme: &Theme, fraction: f32, name: &str) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let c = &theme.colors;
    painter.rect_filled(rect, theme.sizes.corner, c.control.0);
    let mut filled = rect;
    filled.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(filled, theme.sizes.corner, c.accent.0);
    let value = f64::from(fraction.clamp(0.0, 1.0)) * 100.0;
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::ProgressIndicator);
        node.set_label(name.to_owned());
        node.set_numeric_value(value);
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(100.0);
    });
}
