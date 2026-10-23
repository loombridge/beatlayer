//! Find and Select Notes' filter bar (WP-6.4): the magnifier in the editor
//! toolbar, and the bar above the editor's ruler with the Filter chooser,
//! the chosen filter's controls, its Invert switch and its Select button
//! (L-CLIP-010, L-CLIP-019).
//!
//! Nothing here owns the filters: every control runs
//! `clip_view.note_filters`, the command agents run (Spec:
//! find-and-select-notes R14), and the bar reads the shell's state.

use bl_core::time::Ticks;
use bl_model::{ClipId, ScaleName};
use bl_shell::clip_view::filter_frame;
use bl_shell::note_commands::{NOTE_FILTERS, SELECT_NOTES};
use bl_shell::note_filter_commands::{NoteFilter, NoteFilters};
use bl_shell::note_filters::{Frame, span_beats};
use bl_shell::{Area, Invocation, Shell};
use egui::{Align2, Id, Rect, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::{Value, json};

use crate::clip_view::{TOOLBAR_HEIGHT, toolbar_rect};
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice, NO_LEDS};

/// How tall the filter bar is, in points.
pub const BAR_HEIGHT: f32 = 16.0;
/// How wide the magnifier in the toolbar is.
const MAGNIFIER_WIDTH: f32 = 18.0;
/// How wide the Filter chooser is.
const CHOOSER_WIDTH: f32 = 84.0;
/// How wide a number box is.
const NUMBER_WIDTH: f32 = 46.0;
/// The space between controls.
const GAP: f32 = 4.0;
/// How wide the Invert switch and the Select button are.
const BUTTON_WIDTH: f32 = 44.0;

/// The magnifier, at the left of the editor toolbar (L-CLIP-010).
#[must_use]
pub fn magnifier_rect(view: Rect) -> Rect {
    let toolbar = toolbar_rect(view);
    Rect::from_min_size(
        pos2(toolbar.left() + 3.0, toolbar.top()),
        vec2(MAGNIFIER_WIDTH, toolbar.height()),
    )
}

/// The filter bar, between the toolbar and the editor ruler (L-CLIP-019).
#[must_use]
pub fn bar_rect(view: Rect) -> Rect {
    let top = view.top() + TOOLBAR_HEIGHT;
    Rect::from_min_max(pos2(view.left(), top), pos2(view.max.x, top + BAR_HEIGHT))
}

/// The editor's rectangle below the toolbar's own height: the whole view,
/// or with the bar showing the view less the bar's height, so the ruler,
/// the note area and the lanes lay out as they do without it.
#[must_use]
pub fn body_rect(view: Rect, bar: bool) -> Rect {
    if bar {
        Rect::from_min_max(pos2(view.left(), view.top() + BAR_HEIGHT), view.max)
    } else {
        view
    }
}

/// A command for the filters, from the bar, a click or a drag.
fn invoke(events: &mut Vec<UiEvent>, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(NOTE_FILTERS, args)));
}

/// A click in the note area or a lane switches every filter off, keeping
/// their settings, and deselects the notes; the click then does what a click
/// there does (Spec: find-and-select-notes R3).
pub fn clear(shell: &Shell, clip: ClipId, events: &mut Vec<UiEvent>) {
    if shell.clip_view().note_filters().any_on() {
        invoke(events, json!({ "clip": clip, "off": NoteFilter::ALL }));
        events.push(UiEvent::Invoke(Invocation::new(
            SELECT_NOTES,
            json!({ "clip": clip, "notes": [], "mode": "replace" }),
        )));
    }
}

/// The Time filter's Start and Length for a Shift-drag from `from` to `to`
/// (I-DRAG-042): where the drag began and the span it covers, in beats to a
/// hundredth. `None` while the drag has covered nothing.
#[must_use]
pub fn time_window(frame: &Frame, from: Ticks, to: Ticks) -> Option<(f64, f64)> {
    let (low, high) = if from <= to { (from, to) } else { (to, from) };
    let beat = frame.signature.beat_length().get() as f64;
    let round = |beats: f64| (beats * 100.0).round() / 100.0;
    let start = round((low - frame.span.0).get() as f64 / beat + 1.0);
    let length = round((high - low).get() as f64 / beat);
    (length > 0.0).then_some((start, length))
}

/// Draws the magnifier, the Find and Select Notes switch, which shows and
/// hides the bar (Spec: find-and-select-notes R1).
pub fn magnifier(ui: &mut Ui, shell: &Shell, theme: &Theme, view: Rect, events: &mut Vec<UiEvent>) {
    let rect = magnifier_rect(view);
    let id = Id::new("note-filters-magnifier");
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    let on = shell.clip_view().note_filters().visible;
    let colours = &theme.colors;
    let face = if on {
        colours.control_pressed.0
    } else if response.hovered() {
        colours.control_hover.0
    } else {
        colours.control.0
    };
    ui.painter().rect_filled(rect, theme.sizes.corner, face);
    let lens = rect.center() + vec2(-1.5, -1.0);
    let ink = Stroke::new(1.2, colours.text.0);
    ui.painter().circle_stroke(lens, 3.5, ink);
    ui.painter()
        .line_segment([lens + vec2(2.5, 2.5), lens + vec2(5.5, 5.5)], ink);
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::Checkbox, true, on, "Find and Select Notes")
    });
    if response.clicked() {
        invoke(events, json!({ "toggle": true }));
    }
}

/// Lays controls out left to right along the bar.
struct Row {
    x: f32,
    top: f32,
    bottom: f32,
}

impl Row {
    fn take(&mut self, width: f32) -> Rect {
        let rect = Rect::from_min_max(pos2(self.x, self.top), pos2(self.x + width, self.bottom));
        self.x += width + GAP;
        rect
    }
}

/// A switch that reads its state: `on` lights it. Returns whether it was
/// clicked.
fn toggle(ui: &mut Ui, theme: &Theme, row: &mut Row, name: &str, width: f32, on: bool) -> bool {
    let rect = row.take(width);
    let id = Id::new(("note-filters-toggle", name));
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    let colours = &theme.colors;
    let face = if on {
        colours.accent.0
    } else if response.hovered() {
        colours.control_hover.0
    } else {
        colours.control.0
    };
    ui.painter().rect_filled(rect, theme.sizes.corner, face);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        name,
        theme.small_font(),
        if on {
            colours.accent_text.0
        } else {
            colours.text.0
        },
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, name));
    response.clicked()
}

/// A labelled number box. Returns the new value when it's dragged or
/// stepped.
#[expect(
    clippy::too_many_arguments,
    reason = "a number box has this many parts"
)]
fn number(
    ui: &mut Ui,
    theme: &Theme,
    row: &mut Row,
    name: &str,
    value: f64,
    range: (f64, f64),
    step: f64,
    text: &str,
) -> Option<f64> {
    let label =
        ui.painter()
            .layout_no_wrap(name.to_owned(), theme.small_font(), theme.colors.text_dim.0);
    let at = pos2(
        row.x,
        row.top + (row.bottom - row.top - label.size().y) / 2.0,
    );
    ui.painter()
        .galley(at, label.clone(), theme.colors.text_dim.0);
    row.x += label.size().x + GAP;
    let rect = row.take(NUMBER_WIDTH);
    crate::focus::register(ui.ctx(), Id::new(("number-box", name)), Area::Detail, None);
    widgets::number_box(
        ui,
        theme,
        name,
        name,
        rect,
        value,
        range,
        &|v, n| ((v + step * f64::from(n)) * 100.0).round() / 100.0,
        text,
    )
}

/// A chooser among `choices`, returning the one picked.
#[expect(clippy::too_many_arguments, reason = "a chooser has this many parts")]
fn choose<T: Clone + PartialEq>(
    ui: &mut Ui,
    theme: &Theme,
    row: &mut Row,
    name: &str,
    width: f32,
    choices: &[Choice<T>],
    current: &T,
    enabled: bool,
) -> Option<T> {
    let rect = row.take(width);
    crate::focus::register(ui.ctx(), Id::new(("chooser", name)), Area::Detail, None);
    widgets::chooser(ui, theme, name, name, rect, choices, Some(current), enabled)
}

fn plain<T>(label: impl Into<String>, value: T) -> Choice<T> {
    Choice {
        label: label.into(),
        value,
        leds: NO_LEDS,
    }
}

/// Draws the bar for the clip the editor shows, in `view`, and runs the
/// controls it's given (Spec: find-and-select-notes R2).
#[expect(clippy::too_many_lines, reason = "one block per filter's controls")]
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    view: Rect,
    clip: ClipId,
    frame: &Frame,
    events: &mut Vec<UiEvent>,
) {
    let bar = bar_rect(view);
    let colours = &theme.colors;
    ui.painter().rect_filled(bar, 0.0, colours.bar.0);
    let filters: &NoteFilters = shell.clip_view().note_filters();
    // Spec: find-and-select-notes R15. The bar reads out which filters are
    // on, as the chooser's dots show them.
    let on: Vec<&str> = NoteFilter::ALL
        .into_iter()
        .filter(|filter| filters.is_on(*filter))
        .map(NoteFilter::label)
        .collect();
    let region = ui.interact(bar, Id::new("region-note-filters"), Sense::hover());
    ui.ctx().accesskit_node_builder(region.id, |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label("Note Filters".to_owned());
        node.set_description(if on.is_empty() {
            "All off".to_owned()
        } else {
            format!("On: {}", on.join(", "))
        });
    });
    let shown = filters.shown;
    let mut row = Row {
        x: bar.left() + 3.0,
        top: bar.top() + 1.0,
        bottom: bar.bottom() - 1.0,
    };
    // The chooser marks each filter that is on with a dot (L-CLIP-019).
    let choices: Vec<Choice<NoteFilter>> = NoteFilter::ALL
        .iter()
        .map(|filter| Choice {
            label: filter.label().to_owned(),
            value: *filter,
            leds: [None, filters.is_on(*filter).then_some(colours.accent.0)],
        })
        .collect();
    if let Some(chosen) = choose(
        ui,
        theme,
        &mut row,
        "Filter",
        CHOOSER_WIDTH,
        &choices,
        &shown,
        true,
    ) {
        invoke(events, json!({ "clip": clip, "shown": chosen }));
    }
    row.x += GAP;
    let span = span_beats(frame);
    let bar_beats = f64::from(frame.signature.numerator());
    let set = |events: &mut Vec<UiEvent>, args: Value| {
        invoke(
            events,
            json!({ "clip": clip, shown.label().to_lowercase(): args }),
        );
    };
    match shown {
        NoteFilter::Pitch => {
            for class in 0..12u8 {
                let on = filters.pitch.classes.contains(&class);
                let name = bl_model::Scale::root_name(class);
                if toggle(ui, theme, &mut row, name, 20.0, on) {
                    let mut classes = filters.pitch.classes.clone();
                    if on {
                        classes.retain(|c| *c != class);
                    } else {
                        classes.push(class);
                    }
                    set(events, json!({ "classes": classes }));
                }
            }
        }
        NoteFilter::Time => {
            let time = &filters.time;
            let repeat = time.repeat.unwrap_or(bar_beats);
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Start",
                time.start,
                (1.0, 1.0 + span),
                0.25,
                &format!("{:.2}", time.start),
            ) {
                set(events, json!({ "start": v }));
            }
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Length",
                time.length,
                (0.01, span),
                0.25,
                &format!("{:.2}", time.length),
            ) {
                set(events, json!({ "length": v }));
            }
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Repeat",
                repeat,
                (0.0, span),
                0.25,
                &format!("{repeat:.2}"),
            ) {
                set(events, json!({ "repeat": v }));
            }
        }
        NoteFilter::Chance => {
            let chance = &filters.chance;
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Min",
                chance.min,
                (0.0, 100.0),
                1.0,
                &format!("{:.0} %", chance.min),
            ) {
                set(events, json!({ "min": v }));
            }
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Max",
                chance.max,
                (0.0, 100.0),
                1.0,
                &format!("{:.0} %", chance.max),
            ) {
                set(events, json!({ "max": v }));
            }
        }
        NoteFilter::Condition => {
            let condition = &filters.condition;
            for (name, field, on, width) in [
                ("Active", "active", condition.active, 44.0),
                ("Chance", "chance", condition.chance, 44.0),
                (
                    "Velocity Deviation",
                    "velocity_deviation",
                    condition.velocity_deviation,
                    110.0,
                ),
            ] {
                if toggle(ui, theme, &mut row, name, width, on) {
                    set(events, json!({ field: !on }));
                }
            }
        }
        NoteFilter::Count => {
            let count = &filters.count;
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Every",
                f64::from(count.every),
                (1.0, 16.0),
                1.0,
                &count.every.to_string(),
            ) {
                set(events, json!({ "every": v as u8 }));
            }
            let top = f64::from(count.every.saturating_sub(1));
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Offset",
                f64::from(count.offset),
                (0.0, top),
                1.0,
                &count.offset.to_string(),
            ) {
                set(events, json!({ "offset": v as u8 }));
            }
            if toggle(ui, theme, &mut row, "Quantized", 60.0, count.quantized) {
                set(events, json!({ "quantized": !count.quantized }));
            }
        }
        NoteFilter::Duration => {
            let duration = &filters.duration;
            let max = duration.max.unwrap_or(span);
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Min",
                duration.min,
                (0.0, span),
                0.25,
                &format!("{:.2}", duration.min),
            ) {
                set(events, json!({ "min": v }));
            }
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Max",
                max,
                (0.0, span),
                0.25,
                &format!("{max:.2}"),
            ) {
                set(events, json!({ "max": v }));
            }
        }
        NoteFilter::Scale => {
            let scale = &filters.scale;
            if toggle(
                ui,
                theme,
                &mut row,
                "Use Clip Scale",
                84.0,
                scale.use_clip_scale,
            ) {
                set(events, json!({ "use_clip_scale": !scale.use_clip_scale }));
            }
            let roots: Vec<Choice<u8>> = (0..12u8)
                .map(|root| plain(bl_model::Scale::root_name(root), root))
                .collect();
            if let Some(root) = choose(
                ui,
                theme,
                &mut row,
                "Scale Root",
                50.0,
                &roots,
                &scale.root,
                !scale.use_clip_scale,
            ) {
                set(events, json!({ "root": root }));
            }
            let names: Vec<Choice<ScaleName>> = ScaleName::ALL
                .iter()
                .map(|name| plain(name.label(), *name))
                .collect();
            if let Some(name) = choose(
                ui,
                theme,
                &mut row,
                "Scale Name",
                130.0,
                &names,
                &scale.name,
                !scale.use_clip_scale,
            ) {
                set(events, json!({ "name": name }));
            }
        }
        NoteFilter::Velocity => {
            let velocity = &filters.velocity;
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Min",
                f64::from(velocity.min),
                (1.0, 127.0),
                1.0,
                &velocity.min.to_string(),
            ) {
                set(events, json!({ "min": v as u8 }));
            }
            if let Some(v) = number(
                ui,
                theme,
                &mut row,
                "Max",
                f64::from(velocity.max),
                (1.0, 127.0),
                1.0,
                &velocity.max.to_string(),
            ) {
                set(events, json!({ "max": v as u8 }));
            }
        }
    }
    // The Invert switch and the Select button end the bar.
    let invert = match shown {
        NoteFilter::Pitch => filters.pitch.invert,
        NoteFilter::Time => filters.time.invert,
        NoteFilter::Chance => filters.chance.invert,
        NoteFilter::Condition => filters.condition.invert,
        NoteFilter::Count => filters.count.invert,
        NoteFilter::Duration => filters.duration.invert,
        NoteFilter::Scale => filters.scale.invert,
        NoteFilter::Velocity => filters.velocity.invert,
    };
    row.x = bar.right() - 3.0 - 2.0 * BUTTON_WIDTH - GAP;
    if toggle(ui, theme, &mut row, "Invert", BUTTON_WIDTH, invert) {
        set(events, json!({ "invert": !invert }));
    }
    if toggle(ui, theme, &mut row, "Select", BUTTON_WIDTH, false) {
        invoke(events, json!({ "clip": clip, "select": [shown] }));
    }
}

/// What the bar needs of a clip: its filter frame.
#[must_use]
pub fn frame(shell: &Shell, clip: ClipId, midi: &bl_model::MidiData) -> Frame {
    filter_frame(shell.editor().document(), shell.arrangement(), clip, midi)
}
