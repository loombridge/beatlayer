//! The Clip View's Main Properties panel (WP-6.55; layout.md L-CLIP-004): the
//! left group's first panel, for MIDI and audio clips alike. Start and End
//! with Show and Set, Duplicate, the Loop switch, Position and Length with
//! Show and Set, and the Signature, then the Scale controls; the Groove row
//! is drawn pending until its package lands.
//!
//! Every control runs a registered command an agent runs too: the marker and
//! loop commands of the clips, `clip.set_signature`, `clip.duplicate_loop`,
//! `clip_view.show_marker` and `clip.set_marker_at_playhead`. The behaviour
//! is in the clip properties spec, R1 to R12. The sizes inside the panel are
//! the manual's reading, not pixel parity: the reference gives the panel's
//! place and order only.

use bl_core::time::{Ticks, TimeSignature, TimeSignatureMap};
use bl_model::{AudioData, Clip, ClipData, ClipId, Document};
use bl_shell::clip_properties_commands::{Marker, Properties};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{
    Align2, Event, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType,
    pos2, vec2,
};
use serde_json::{Value, json};

use crate::launch_panel::{self, pending};
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice};

/// A field's, a Show button's and a Set button's sizes, and the step between
/// rows.
const LABEL: f32 = 30.0;
const FIELD: Vec2 = vec2(62.0, 17.0);
const SHOW: Vec2 = vec2(30.0, 17.0);
const SET: Vec2 = vec2(24.0, 17.0);
const ROW: f32 = 19.0;
/// The gap between a row's controls, and the panel's inner margin.
const GAP: f32 = 3.0;
const MARGIN: f32 = 4.0;
/// A signature field's size.
const SIGNATURE: Vec2 = vec2(40.0, 17.0);
/// What one step of a field moves: a sixteenth of the clip's contents, or a
/// hundredth of a second in an unwarped clip's file.
const STEP_SECONDS: f64 = 0.01;

/// The ID of the panel's group.
#[must_use]
pub fn main_panel_id() -> Id {
    Id::new("main-properties")
}

fn focus_request_id() -> Id {
    Id::new("clip-panels-focus-request")
}

/// Asks for the keyboard focus to go to the first control of the panel the
/// left group shows (Navigate ▸ Clip Panels, R11).
pub fn request_focus(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(focus_request_id(), true));
}

/// Takes a request for the focus, once.
pub(crate) fn take_focus_request(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(focus_request_id()))
        .unwrap_or(false)
}

/// Draws the left group of the panels in `rect` for the clip `clip`: the
/// selector row, and Main Properties or Launch Properties under it (R1).
pub fn show_left_group(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);
    let session = shell
        .clip_view()
        .shown_session_clip(doc, shell.arrangement())
        .is_some();
    let shown = shell.clip_view().shown_properties(doc, shell.arrangement());
    launch_panel::selector(ui, theme, rect, shown, session, events);
    match shown {
        Properties::Launch => launch_panel::show(ui, shell, theme, rect, clip, events),
        Properties::Main => show(ui, shell, theme, rect, clip, events),
    }
}

/// How a clip's positions read: bars, beats and sixteenths in a signature,
/// or minutes, seconds and milliseconds of an unwarped clip's file.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Reading {
    Bars(TimeSignature),
    Time,
}

impl Reading {
    fn of(clip: &Clip, doc: &Document) -> Self {
        let own = match &clip.data {
            ClipData::Midi(midi) => midi.signature,
            ClipData::Audio(audio) if audio.warp => audio.signature,
            ClipData::Audio(_) => return Self::Time,
        };
        Self::Bars(own.unwrap_or_else(|| doc.settings().time_signatures.signature_at(clip.start)))
    }

    /// A position as the fields read it: `1. 1. 1` at the clip's beginning,
    /// `-1. 3. 1` before it.
    fn position(self, value: f64) -> String {
        match self {
            Self::Bars(signature) => {
                let at = TimeSignatureMap::constant(signature).to_bbs(Ticks(value.round() as i64));
                format!("{}. {}. {}", at.bar, at.beat, at.sixteenth)
            }
            Self::Time => time_text(value),
        }
    }

    /// A length as the fields read it, counted from 0: a one-bar loop reads
    /// `1. 0. 0`.
    fn length(self, value: f64) -> String {
        match self {
            Self::Bars(signature) => {
                let ticks = value.round() as i64;
                let (bar, beat) = (signature.bar_length().get(), signature.beat_length().get());
                format!(
                    "{}. {}. {}",
                    ticks / bar,
                    ticks % bar / beat,
                    ticks % beat / Ticks::SIXTEENTH.get()
                )
            }
            Self::Time => time_text(value),
        }
    }

    /// The position typed as `text`: `0.3.1` is `-1. 3. 1`, as the position
    /// parser reads bar 0.
    fn parse_position(self, text: &str) -> Option<f64> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        match self {
            Self::Bars(signature) => TimeSignatureMap::constant(signature)
                .parse_position(&text)
                .ok()
                .map(|ticks| ticks.get() as f64),
            Self::Time => parse_time(&text),
        }
    }

    /// The length typed as `text`: bars, beats and sixteenths from 0.
    fn parse_length(self, text: &str) -> Option<f64> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        match self {
            Self::Bars(signature) => {
                let mut fields = text.split('.');
                let mut next = |unit: Ticks| -> Option<i64> {
                    match fields.next() {
                        None => Some(0),
                        Some(field) => field.parse::<u32>().ok().map(|n| i64::from(n) * unit.get()),
                    }
                };
                let bars = next(signature.bar_length())?;
                let beats = next(signature.beat_length())?;
                let sixteenths = next(Ticks::SIXTEENTH)?;
                fields
                    .next()
                    .is_none()
                    .then_some((bars + beats + sixteenths) as f64)
            }
            Self::Time => parse_time(&text),
        }
    }

    /// How far one step of a field moves a value.
    fn step(self) -> f64 {
        match self {
            Self::Bars(_) => Ticks::SIXTEENTH.get() as f64,
            Self::Time => STEP_SECONDS,
        }
    }
}

/// Seconds as `minutes:seconds.milliseconds`.
fn time_text(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as i64;
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// `minutes:seconds.milliseconds`, `seconds.milliseconds` or seconds.
fn parse_time(text: &str) -> Option<f64> {
    let (minutes, seconds) = match text.split_once(':') {
        Some((minutes, seconds)) => (minutes.parse::<u32>().ok()?, seconds),
        None => (0, text),
    };
    let seconds: f64 = seconds
        .parse()
        .ok()
        .filter(|s: &f64| s.is_finite() && *s >= 0.0)?;
    Some(f64::from(minutes) * 60.0 + seconds)
}

/// What the person did to a field.
enum Entry {
    /// Typed a value and pressed Return.
    Typed(String),
    /// Dragged or pressed Up or Down by this many steps.
    Steps(i32),
}

/// A clip's four markers on its own axis: ticks of a MIDI or warped clip's
/// contents, seconds in an unwarped clip's file.
struct Markers {
    start: f64,
    end: f64,
    loop_start: f64,
    loop_end: f64,
    looping: bool,
}

fn markers(shell: &Shell, clip: &Clip) -> Markers {
    match &clip.data {
        ClipData::Midi(midi) => {
            let start = midi.start.get() as f64;
            let loop_start = midi.loop_start.get() as f64;
            let length = clip.length.get() as f64;
            Markers {
                start,
                end: midi.end.map_or(start + length, |end| end.get() as f64),
                loop_start,
                loop_end: midi
                    .loop_end
                    .map_or(loop_start + length, |end| end.get() as f64),
                looping: midi.looping,
            }
        }
        ClipData::Audio(audio) => {
            let file = file_end(shell, clip.id, audio);
            Markers {
                start: audio.start,
                end: audio.end.unwrap_or(file),
                loop_start: audio.loop_start,
                loop_end: audio.loop_end.unwrap_or(file),
                looping: audio.looping,
            }
        }
    }
}

/// The end of an audio clip's file on the clip's axis, where the store knows
/// the file.
fn file_end(shell: &Shell, clip: ClipId, audio: &AudioData) -> f64 {
    let seconds = shell.clip_file_seconds(clip).unwrap_or(0.0);
    bl_shell::sample_editor::SampleView::beat_of(audio, seconds)
        .map_or(seconds, |beat| beat.get() as f64)
}

/// A button: a box with a name on it. Returns whether it was clicked.
fn button(ui: &mut Ui, theme: &Theme, name: &str, text: &str, rect: Rect) -> bool {
    let id = Id::new(("button", name));
    let response = ui.interact(rect, id, Sense::click());
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        c.text.0,
    );
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if response.clicked() {
        response.request_focus();
    }
    response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
}

/// Where a field keeps what is being typed into it.
fn typing_id() -> Id {
    Id::new("clip-field-typing")
}

/// A field that shows a position or a length and takes a typed value (Return
/// to confirm, Esc to cancel) or steps by dragging or Up and Down. The keys
/// go to it while it has the keyboard focus, as to a text field.
fn field(ui: &mut Ui, theme: &Theme, name: &str, rect: Rect, text: &str) -> Option<Entry> {
    let id = Id::new(("number-box", name));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    // Marks the field as one that takes typed values, so the keys go to it
    // and not to shortcuts.
    ui.data_mut(|d| d.insert_temp(id.with("typed-field"), true));
    let mut typed = ui
        .data(|d| d.get_temp::<(Id, String)>(typing_id()))
        .filter(|(field, _)| *field == id)
        .map(|(_, text)| text);
    let mut result = None;
    let mut steps = 0;
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
        let keys: Vec<Key> = ui.input_mut(|i| {
            let mut taken = Vec::new();
            i.events.retain(|event| match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command
                    && !modifiers.ctrl
                    && (digit(*key).is_some()
                        || matches!(
                            key,
                            Key::Minus
                                | Key::Period
                                | Key::Colon
                                | Key::Backspace
                                | Key::ArrowUp
                                | Key::ArrowDown
                                | Key::Escape
                        )
                        || (typed.is_some() && *key == Key::Enter)) =>
                {
                    taken.push(*key);
                    false
                }
                _ => true,
            });
            taken
        });
        for key in keys {
            match key {
                Key::Enter => result = typed.take().map(Entry::Typed),
                Key::Escape => typed = None,
                Key::Backspace => {
                    if let Some(text) = typed.as_mut() {
                        text.pop();
                    }
                }
                Key::ArrowUp => steps += 1,
                Key::ArrowDown => steps -= 1,
                other => {
                    let ch = digit(other).unwrap_or(match other {
                        Key::Minus => '-',
                        Key::Colon => ':',
                        _ => '.',
                    });
                    typed.get_or_insert_with(String::new).push(ch);
                }
            }
        }
    } else {
        typed = None;
    }
    if response.dragged() {
        // Read outside `data_mut`: the drag reads the input.
        let delta = response.drag_delta().y;
        steps += ui.data_mut(|d| {
            let pending = d.get_temp_mut_or(id, 0.0f32);
            *pending -= delta;
            let whole = (*pending / 3.0).trunc();
            *pending -= whole * 3.0;
            whole as i32
        });
    } else if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(id));
    }
    ui.data_mut(|d| match &typed {
        Some(text) => {
            d.insert_temp(typing_id(), (id, text.clone()));
        }
        None => {
            if d.get_temp::<(Id, String)>(typing_id())
                .is_some_and(|(field, _)| field == id)
            {
                d.remove::<(Id, String)>(typing_id());
            }
        }
    });

    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if typed.is_some() || response.hovered() || response.dragged() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        typed.as_deref().unwrap_or(text),
        theme.small_font(),
        c.text.0,
    );
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    steps += ui.input(|i| {
        i.num_accesskit_action_requests(id, accesskit::Action::Increment) as i32
            - i.num_accesskit_action_requests(id, accesskit::Action::Decrement) as i32
    });
    response.widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::SpinButton);
        node.set_value(text.to_owned());
        node.add_action(accesskit::Action::Increment);
        node.add_action(accesskit::Action::Decrement);
    });
    result.or((steps != 0).then_some(Entry::Steps(steps)))
}

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

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// The ticks of a position in a MIDI clip's JSON.
fn ticks(value: f64) -> i64 {
    value.round() as i64
}

/// Runs the marker or loop command that puts `marker` of each of `targets`
/// at `value` (a length for `LoopLength`), as one undo step for MIDI clips
/// (R3, R7, R11).
fn edit_marker(events: &mut Vec<UiEvent>, targets: &[&Clip], marker: Marker, value: f64) {
    let midi = targets
        .iter()
        .all(|clip| matches!(clip.data, ClipData::Midi(_)));
    if midi {
        let own: Vec<Value> = targets
            .iter()
            .filter_map(|clip| {
                let ClipData::Midi(data) = &clip.data else {
                    return None;
                };
                Some(match marker {
                    Marker::Start => json!({ "clip": clip.id, "end": data.end }),
                    Marker::End => json!({ "clip": clip.id, "start": data.start }),
                    Marker::LoopPosition => json!({ "clip": clip.id, "end": data.loop_end }),
                    Marker::LoopLength => json!({
                        "clip": clip.id,
                        "start": data.loop_start,
                        "end": data.loop_start.get() + ticks(value),
                    }),
                })
            })
            .collect();
        let (id, shared) = match marker {
            Marker::Start => ("clip.set_midi_region", json!({ "start": ticks(value) })),
            Marker::End => ("clip.set_midi_region", json!({ "end": ticks(value) })),
            Marker::LoopPosition => ("clip.set_midi_loop", json!({ "start": ticks(value) })),
            Marker::LoopLength => ("clip.set_midi_loop", json!({})),
        };
        let mut args = shared;
        args["clips"] = Value::Array(own);
        invoke(events, id, args);
        return;
    }
    for clip in targets {
        let ClipData::Audio(audio) = &clip.data else {
            continue;
        };
        let args = match marker {
            Marker::Start => (
                "clip.set_region",
                json!({ "start": value, "end": audio.end }),
            ),
            Marker::End => (
                "clip.set_region",
                json!({ "start": audio.start, "end": value }),
            ),
            Marker::LoopPosition => (
                "clip.set_loop",
                json!({ "start": value, "end": audio.loop_end }),
            ),
            Marker::LoopLength => (
                "clip.set_loop",
                json!({ "start": audio.loop_start, "end": audio.loop_start + value }),
            ),
        };
        let (id, mut args) = args;
        args["clip"] = json!(clip.id);
        args["ticks"] = json!(audio.warp);
        invoke(events, id, args);
    }
}

/// Draws the Main Properties panel for the clip `clip` in `rect`, under the
/// selector row.
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    let Some(value) = doc.clip(clip) else {
        return;
    };
    let c = &theme.colors;
    let group = ui.interact(rect, main_panel_id(), Sense::hover());
    ui.ctx().accesskit_node_builder(group.id, |node| {
        node.set_role(Role::Group);
        node.set_label("Main Properties".to_owned());
        node.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });

    // The clips the panel acts on (R11): the MIDI clips the note editor
    // shows, or the selected audio clips.
    let audio = matches!(value.data, ClipData::Audio(_));
    let target_ids = if audio {
        crate::audio_panel::targets(shell, doc, clip)
    } else {
        shell.clip_view().shown(doc, shell.arrangement())
    };
    let targets: Vec<&Clip> = target_ids
        .iter()
        .filter_map(|id| doc.clip(*id))
        .collect::<Vec<_>>();
    let targets = if targets.is_empty() {
        vec![value]
    } else {
        targets
    };
    let ids: Vec<ClipId> = targets.iter().map(|clip| clip.id).collect();

    let reading = Reading::of(value, doc);
    let m = markers(shell, value);
    let unwarped = reading == Reading::Time;
    let playing = shell.clip_playing(clip);
    let first = take_focus_request(ui.ctx());

    let painter = ui.painter_at(rect);
    let left = rect.left() + MARGIN;
    let y = std::cell::Cell::new(rect.top() + launch_panel::SELECTOR + 6.0);
    let row = |label: &str| {
        painter.text(
            pos2(left, y.get() + FIELD.y / 2.0),
            Align2::LEFT_CENTER,
            label,
            theme.small_font(),
            c.text_dim.0,
        );
        let at = pos2(left + LABEL + GAP, y.get());
        y.set(y.get() + ROW);
        at
    };

    // A marker row: its field, Show and Set (R3 to R5, R7, R8).
    let marker_row = |events: &mut Vec<UiEvent>,
                      ui: &mut Ui,
                      label: &str,
                      marker: Marker,
                      current: f64,
                      is_length: bool,
                      first_control: bool| {
        let at = row(label);
        let text = if is_length {
            reading.length(current)
        } else {
            reading.position(current)
        };
        if first_control && first {
            ui.memory_mut(|mem| mem.request_focus(Id::new(("number-box", label))));
        }
        if let Some(entry) = field(ui, theme, label, Rect::from_min_size(at, FIELD), &text) {
            let next = match entry {
                Entry::Steps(steps) => Some(current + f64::from(steps) * reading.step()),
                Entry::Typed(typed) if is_length => reading.parse_length(&typed),
                Entry::Typed(typed) => reading.parse_position(&typed),
            };
            if let Some(next) = next {
                edit_marker(events, &targets, marker, next);
            }
        }
        let shown = at.x + FIELD.x + GAP;
        if button(
            ui,
            theme,
            &format!("Show {label}"),
            "Show",
            Rect::from_min_size(pos2(shown, at.y), SHOW),
        ) {
            invoke(
                events,
                bl_shell::clip_properties_commands::SHOW_MARKER,
                json!({ "clip": clip, "marker": marker }),
            );
        }
        let set = Rect::from_min_size(pos2(shown + SHOW.x + GAP, at.y), SET);
        let name = format!("Set {label}");
        if playing {
            if button(ui, theme, &name, "Set", set) {
                invoke(
                    events,
                    bl_shell::clip_properties_commands::SET_MARKER_AT_PLAYHEAD,
                    json!({ "clip": clip, "marker": marker }),
                );
            }
        } else {
            pending(
                ui,
                theme,
                &name,
                set,
                "Set",
                "Set works while the clip plays.",
            );
        }
    };

    marker_row(events, ui, "Start", Marker::Start, m.start, false, true);
    marker_row(events, ui, "End", Marker::End, m.end, false, false);

    // Duplicate (R9): Duplicate Loop on the clips the Clip View shows.
    let at = row("");
    let duplicate = Rect::from_min_size(pos2(left, at.y), vec2(LABEL + GAP + FIELD.x, FIELD.y));
    if audio {
        pending(
            ui,
            theme,
            "Duplicate",
            duplicate,
            "Duplicate",
            "Not built yet: Duplicate Loop works on MIDI clips.",
        );
    } else if button(ui, theme, "Duplicate", "Duplicate", duplicate) {
        invoke(events, "clip.duplicate_loop", json!({ "clips": ids }));
    }

    // The Loop switch (R6): disabled on an unwarped audio clip.
    let at = row("Loop");
    let switch = Rect::from_min_size(at, vec2(FIELD.x, FIELD.y));
    if unwarped {
        pending(
            ui,
            theme,
            "Loop",
            switch,
            if m.looping { "On" } else { "Off" },
            "Only a warped clip loops: switch Warp on first.",
        );
    } else {
        crate::focus::register(ui.ctx(), Id::new(("switch", "Loop")), Area::Detail, None);
        if let Some(on) = widgets::switch(ui, theme, "Loop", "Loop", switch, m.looping) {
            invoke(
                events,
                "clip.set_loop_enabled",
                json!({ "clips": ids, "enabled": on }),
            );
        }
    }

    marker_row(
        events,
        ui,
        "Position",
        Marker::LoopPosition,
        m.loop_start,
        false,
        false,
    );
    marker_row(
        events,
        ui,
        "Length",
        Marker::LoopLength,
        m.loop_end - m.loop_start,
        true,
        false,
    );

    // The Signature (R10): its numerator, and its denominator.
    let at = row("Sig.");
    let signature = match reading {
        Reading::Bars(signature) => Some(signature),
        Reading::Time => None,
    };
    let numerator = Rect::from_min_size(at, SIGNATURE);
    let denominator = Rect::from_min_size(at + vec2(SIGNATURE.x + GAP, 0.0), SIGNATURE);
    if let Some(signature) = signature {
        let (n, d) = (signature.numerator(), signature.denominator());
        crate::focus::register(
            ui.ctx(),
            Id::new(("number-box", "Signature Numerator")),
            Area::Detail,
            None,
        );
        if let Some(next) = widgets::number_box(
            ui,
            theme,
            "Signature Numerator",
            "Signature Numerator",
            numerator,
            f64::from(n),
            (1.0, 99.0),
            &|value, steps| value + f64::from(steps),
            &n.to_string(),
        ) {
            invoke(
                events,
                "clip.set_signature",
                json!({ "clips": ids, "signature": { "numerator": next.round() as u8, "denominator": d } }),
            );
        }
        const DENOMINATORS: [u8; 5] = [1, 2, 4, 8, 16];
        let at = DENOMINATORS.iter().position(|&x| x == d).unwrap_or(2);
        crate::focus::register(
            ui.ctx(),
            Id::new(("number-box", "Signature Denominator")),
            Area::Detail,
            None,
        );
        if let Some(next) = widgets::number_box(
            ui,
            theme,
            "Signature Denominator",
            "Signature Denominator",
            denominator,
            at as f64,
            (0.0, 4.0),
            &|value, steps| value + f64::from(steps),
            &d.to_string(),
        ) {
            invoke(
                events,
                "clip.set_signature",
                json!({ "clips": ids, "signature": { "numerator": n, "denominator": DENOMINATORS[next.round() as usize] } }),
            );
        }
    } else {
        let reason = "An unwarped clip's positions read in time: switch Warp on first.";
        pending(ui, theme, "Signature Numerator", numerator, "-", reason);
        pending(ui, theme, "Signature Denominator", denominator, "-", reason);
    }

    // The Groove and Scale rows wait for their own packages.
    let at = row("Groove");
    pending(
        ui,
        theme,
        "Groove",
        Rect::from_min_size(at, vec2(FIELD.x + GAP + SHOW.x, FIELD.y)),
        "None",
        "Not built yet: the Groove controls come with the Groove Pool.",
    );
    scale_rows(ui, shell, theme, &ids, &row, events);
}

/// The Scale Mode switch, Root Note chooser and Scale Name chooser, at the
/// panel's end (the scales spec, R19): they show the scale of the clips the
/// panel acts on, `*` where they differ, and run `clip.set_scale` on them.
fn scale_rows(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    ids: &[ClipId],
    row: &dyn Fn(&str) -> egui::Pos2,
    events: &mut Vec<UiEvent>,
) {
    let shown = bl_shell::scales::display(shell.editor().document(), ids);
    let set = |events: &mut Vec<UiEvent>, change: Value| {
        let mut args = serde_json::Map::new();
        args.insert("clips".to_owned(), json!(ids));
        if let Value::Object(change) = change {
            args.extend(change);
        }
        invoke(
            events,
            bl_shell::scales_commands::SET_CLIP_SCALE,
            Value::Object(args),
        );
    };
    let at = row("Scale");
    let switch = Rect::from_min_size(at, FIELD);
    match shown.enabled {
        Some(on) => {
            crate::focus::register(
                ui.ctx(),
                Id::new(("switch", "Clip Scale Mode")),
                Area::Detail,
                None,
            );
            if let Some(on) =
                widgets::switch(ui, theme, "Clip Scale Mode", "Clip Scale Mode", switch, on)
            {
                set(events, json!({ "enabled": on }));
            }
        }
        // The clips differ: the switch reads `*`, and a click turns them all on.
        None => {
            if button(ui, theme, "Clip Scale Mode", "*", switch) {
                set(events, json!({ "enabled": true }));
            }
        }
    }
    fn mixed<T>() -> Choice<Option<T>> {
        Choice {
            label: "*".to_owned(),
            value: None,
            leds: widgets::NO_LEDS,
        }
    }
    let roots: Vec<Choice<Option<u8>>> = (0..12u8)
        .map(|root| Choice {
            label: shell.root_name(root),
            value: Some(root),
            leds: widgets::NO_LEDS,
        })
        .chain(shown.root.is_none().then(mixed))
        .collect();
    crate::focus::register(
        ui.ctx(),
        Id::new(("chooser", "Clip Root Note")),
        Area::Detail,
        None,
    );
    let root = Rect::from_min_size(at + vec2(FIELD.x + GAP, 0.0), vec2(40.0, FIELD.y));
    if let Some(Some(root_note)) = widgets::chooser(
        ui,
        theme,
        "Clip Root Note",
        "Clip Root Note",
        root,
        &roots,
        Some(&shown.root),
        true,
    ) {
        set(events, json!({ "root": root_note }));
    }
    let names: Vec<Choice<Option<bl_model::ScaleName>>> = bl_model::ScaleName::ALL
        .into_iter()
        .map(|name| Choice {
            label: name.label().to_owned(),
            value: Some(name),
            leds: widgets::NO_LEDS,
        })
        .chain(shown.name.is_none().then(mixed))
        .collect();
    let at = row("Name");
    crate::focus::register(
        ui.ctx(),
        Id::new(("chooser", "Clip Scale Name")),
        Area::Detail,
        None,
    );
    let name = Rect::from_min_size(at, vec2(FIELD.x + GAP + 40.0, FIELD.y));
    if let Some(Some(chosen)) = widgets::chooser(
        ui,
        theme,
        "Clip Scale Name",
        "Clip Scale Name",
        name,
        &names,
        Some(&shown.name),
        true,
    ) {
        set(events, json!({ "name": chosen.id() }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn four_four() -> Reading {
        Reading::Bars(TimeSignature::FOUR_FOUR)
    }

    #[test]
    fn positions_read_from_one_and_lengths_from_zero() {
        // Spec: clip-properties R3, R7.
        assert_eq!(four_four().position(0.0), "1. 1. 1");
        assert_eq!(
            four_four().position(-(Ticks::QUARTER * 2 - Ticks::SIXTEENTH).get() as f64),
            "-1. 3. 2"
        );
        assert_eq!(four_four().length(Ticks::WHOLE.get() as f64), "1. 0. 0");
        assert_eq!(
            four_four().length((Ticks::WHOLE * 3).get() as f64),
            "3. 0. 0"
        );
    }

    #[test]
    fn typing_bar_zero_reads_before_the_beginning() {
        // Spec: clip-properties R3. 0.3.1 gives -1. 3. 1.
        let ticks = four_four().parse_position("0.3.1").unwrap();
        assert_eq!(four_four().position(ticks), "-1. 3. 1");
        assert_eq!(
            four_four().parse_position("2 . 1 . 1"),
            Some(Ticks::WHOLE.get() as f64)
        );
        assert_eq!(four_four().parse_position("x"), None);
    }

    #[test]
    fn lengths_and_times_parse() {
        assert_eq!(
            four_four().parse_length("1. 0. 0"),
            Some(Ticks::WHOLE.get() as f64)
        );
        assert_eq!(
            four_four().parse_length("0.1.2"),
            Some((Ticks::QUARTER + Ticks::SIXTEENTH * 2).get() as f64)
        );
        assert_eq!(Reading::Time.position(61.5), "1:01.500");
        assert_eq!(Reading::Time.parse_position("1:01.5"), Some(61.5));
        assert_eq!(Reading::Time.parse_position("2.25"), Some(2.25));
    }
}
