//! The Mixer view (WP-1.9; the mixer view spec): the Session View's columns
//! with their mixer, the Arrangement's mixer under its lanes, and the
//! controls a strip and a track header share: the volume fader and field,
//! pan, the meter, the activator, Solo, and Arm.
//!
//! Placement comes from the layout layer ([`crate::layout::mixer`]). Every
//! control runs a registered command: the values are `bl-model`'s `mixer.*`
//! commands, worked out by `bl_shell::mixer_controls` from the set and the
//! selection, and a drag runs them inside one gesture
//! ([`UiEvent::BeginGesture`]), so it's one undo step however far it goes.
//! Showing, hiding, scrolling, and focusing the mixer are the
//! `mixer_view.*` commands. Only the columns in view are built (R17).

use bl_model::{Document, TrackId, TrackKind};
use bl_shell::mixer_controls::{
    self as controls, Control, DRAG_TRAVEL, FINE, Switch, meter_position, pan_text, volume_spoken,
    volume_text,
};
use bl_shell::mixer_view::MixerOf;
use bl_shell::mixer_view_commands as ids;
use bl_shell::{Invocation, Shell};
use egui::accesskit::{self, Action, Role};
use egui::{
    Align2, Color32, Id, Key, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::arrangement::scene::track_colour;
use crate::layout::mixer::{
    Column, ColumnKind, MixerLayout, Sections, SessionLayout, StripLayout, in_out_row, send_knob,
};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The mixer's sections as the shell has them.
#[must_use]
pub fn sections(shell: &Shell) -> Sections {
    let view = shell.mixer_view();
    Sections {
        in_out: view.in_out(),
        sends: view.sends(),
        volume: view.volume(),
        returns: view.return_tracks(),
    }
}

/// The ID of a volume field: in a mixer's strip, or a header's.
#[must_use]
pub fn volume_field_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-volume", place, track))
}

/// The ID of a fader.
#[must_use]
pub fn fader_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-fader", place, track))
}

/// The ID of a pan control.
#[must_use]
pub fn pan_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-pan", place, track))
}

/// The ID of a switch.
#[must_use]
pub fn switch_id(place: Place, switch: Switch, track: TrackId) -> Id {
    Id::new(("mixer-switch", place, switch, track))
}

/// The ID of a meter.
#[must_use]
pub fn meter_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-meter", place, track))
}

/// Where a control is: one view's mixer, or the Arrangement's headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// A mixer's strip.
    Mixer(MixerOf),
    /// A track header in the Arrangement.
    Header,
}

/// A drag on a value control: where each target started, and how far it has
/// gone, in the control's units.
#[derive(Clone, Debug, Default)]
struct ValueDrag {
    starts: Vec<(TrackId, f64)>,
    moved: f64,
}

/// The Mixer view's state between frames: the focus requests it has
/// carried out. Drags keep their state in egui's memory, by control.
#[derive(Debug, Default)]
pub struct MixerUi {
    focus_seen: u64,
    /// The columns the last frame built: tracks, returns, Main (R17).
    built: usize,
}

impl MixerUi {
    /// A mixer with nothing under way.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many columns the last frame built.
    #[must_use]
    pub fn built_columns(&self) -> usize {
        self.built
    }

    /// Draws the Session View in the main view `rect`: the title bars, the
    /// clip slots and status row (M3, drawn as a placeholder), and the
    /// mixer under them while it shows (R1).
    pub fn show_session(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let sections = sections(shell);
        let shown = shell.mixer_view().of(MixerOf::Session).shown;
        let layout = SessionLayout::compute(rect, shown.then_some(sections));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.colors.panel.0);
        // The title bars, over every column (L-SES-002).
        let doc = shell.editor().document();
        let columns = shell.mixer_columns();
        let returns: Vec<TrackId> = doc.returns().collect();
        let scroll = shell.mixer_view().of(MixerOf::Session).scroll as f32;
        let title_layout = MixerLayout::compute(
            layout.title_bars,
            columns.len(),
            returns.len(),
            sections,
            scroll,
        );
        for column in title_layout.every_column() {
            let track = column_track(doc, &columns, &returns, column);
            let Some(t) = doc.track(track) else {
                continue;
            };
            let bar = column.rect.shrink2(vec2(0.5, 0.0));
            let bar = if column.kind == ColumnKind::Track {
                bar.intersect(title_layout.tracks)
            } else {
                bar
            };
            let painter = ui.painter_at(bar);
            painter.rect_filled(
                bar,
                0.0,
                title_colour(theme, track, t.kind()).gamma_multiply(0.82),
            );
            painter.text(
                pos2(bar.left() + 4.0, bar.center().y),
                Align2::LEFT_CENTER,
                t.name(),
                theme.small_font(),
                theme.colors.clip_text.0,
            );
        }
        // The slots and the status row arrive with the Session View (M3).
        painter.text(
            layout.slots.center(),
            Align2::CENTER_CENTER,
            "The Session View's clip slots and scenes arrive in milestone M3.",
            theme.small_font(),
            theme.colors.pending.0,
        );
        painter.line_segment(
            [layout.status_row.left_top(), layout.status_row.right_top()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        if let Some(mixer) = layout.mixer {
            self.show_mixer(ui, shell, theme, mixer, MixerOf::Session, events);
        }
    }

    /// Draws one view's mixer in `area` and adds what the person did to
    /// `events` (R1 to R3, R5, R10, R14).
    pub fn show_mixer(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        area: Rect,
        of: MixerOf,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let columns = shell.mixer_columns();
        let returns: Vec<TrackId> = doc.returns().collect();
        let sections = sections(shell);
        let scroll = shell.mixer_view().of(of).scroll as f32;
        let layout = MixerLayout::compute(area, columns.len(), returns.len(), sections, scroll);
        let region = Id::new(("mixer", of));
        let _ = ui.interact(area, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label(match of {
                MixerOf::Session => "Session Mixer".to_owned(),
                MixerOf::Arrangement => "Arrangement Mixer".to_owned(),
            });
            node.set_bounds(bounds(area));
        });
        ui.painter_at(area)
            .rect_filled(area, 0.0, theme.colors.panel.0);
        if let Some(drop) = layout.drop_area {
            ui.painter_at(drop).text(
                drop.center(),
                Align2::CENTER_CENTER,
                "Drop Files and Devices Here",
                theme.small_font(),
                theme.colors.text_dim.0,
            );
        }
        let selected = shell.arrangement().selected_tracks();
        let place = Place::Mixer(of);
        self.built = 0;
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .accessibility_parent(region),
        );
        for column in layout.every_column() {
            self.built += 1;
            let track = column_track(doc, &columns, &returns, column);
            let Some(t) = doc.track(track) else {
                continue;
            };
            let clip = if column.kind == ColumnKind::Track {
                column.rect.intersect(layout.tracks)
            } else {
                column.rect
            };
            let mut column_ui = ui.new_child(egui::UiBuilder::new().max_rect(clip));
            column_ui.set_clip_rect(clip.intersect(area));
            let group = Id::new(("mixer-column", of, track));
            let _ = column_ui.interact(clip, group, Sense::hover());
            column_ui.ctx().accesskit_node_builder(group, |node| {
                node.set_role(Role::Group);
                node.set_label(t.name().to_owned());
                node.set_bounds(bounds(clip));
            });
            let mut column_ui = column_ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(clip)
                    .accessibility_parent(group),
            );
            let painter = column_ui.painter();
            let fill = if selected.contains(&track) {
                theme.colors.lane_selected.0
            } else {
                theme.colors.lane.0
            };
            painter.rect_filled(column.rect.shrink2(vec2(0.5, 0.0)), 0.0, fill);
            painter.line_segment(
                [column.rect.right_top(), column.rect.right_bottom()],
                Stroke::new(1.0, theme.colors.lane_line.0),
            );
            let label = activator_label(column);
            if let Some(span) = layout.in_out {
                in_out_section(&mut column_ui, theme, column, span, t.kind());
            }
            if let Some(span) = layout.sends {
                sends_section(&mut column_ui, theme, column, span, returns.len(), t.kind());
            }
            if let Some(span) = layout.strip {
                let strip = StripLayout::compute(column.rect, span);
                let parts = StripParts {
                    shell,
                    theme,
                    place,
                    track,
                    activator: &label,
                };
                parts.strip(&mut column_ui, &strip, column.kind, events);
            }
        }
        self.scroll(&ui, area, of, events);
        self.focus(&ui, shell, &layout, &columns, of, events);
    }

    /// The wheel over the mixer scrolls its columns (R14).
    fn scroll(&self, ui: &Ui, area: Rect, of: MixerOf, events: &mut Vec<UiEvent>) {
        let over = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pos| area.contains(pos));
        if !over {
            return;
        }
        let delta = ui.input(egui::InputState::smooth_scroll_delta);
        let x = -(delta.x + delta.y);
        if x.abs() > 0.01 {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SCROLL,
                json!({ "view": of, "x": f64::from(x) }),
            )));
        }
    }

    /// Carries out `mixer_view.focus` in the Arrangement's mixer (R13).
    fn focus(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        layout: &MixerLayout,
        columns: &[TrackId],
        of: MixerOf,
        events: &mut Vec<UiEvent>,
    ) {
        let requests = shell.mixer_view().focus_requests();
        if of != MixerOf::Arrangement || requests == self.focus_seen {
            return;
        }
        let target = shell
            .arrangement()
            .selected_tracks()
            .iter()
            .find_map(|t| columns.iter().position(|c| c == t))
            .unwrap_or(0);
        if columns.is_empty() || layout.strip.is_none() {
            self.focus_seen = requests;
            return;
        }
        let in_view = layout
            .columns
            .iter()
            .any(|c| c.index == target && layout.tracks.contains_rect(c.rect.shrink(1.0)));
        if in_view {
            self.focus_seen = requests;
            ui.memory_mut(|m| {
                m.request_focus(volume_field_id(Place::Mixer(of), columns[target]));
            });
        } else {
            let x = f64::from(crate::layout::mixer::COLUMN_WIDTH) * target as f64
                - f64::from(layout.scroll);
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SCROLL,
                json!({ "view": of, "x": x }),
            )));
        }
    }
}

/// The track a column shows.
fn column_track(
    doc: &Document,
    columns: &[TrackId],
    returns: &[TrackId],
    column: &Column,
) -> TrackId {
    match column.kind {
        ColumnKind::Track => columns[column.index],
        ColumnKind::Return => returns[column.index],
        ColumnKind::Main => doc.main_id(),
    }
}

/// What a column's activator shows: its number, or a return's letter.
fn activator_label(column: &Column) -> String {
    match column.kind {
        ColumnKind::Track => (column.index + 1).to_string(),
        ColumnKind::Return => char::from(b'A' + (column.index % 26) as u8).to_string(),
        ColumnKind::Main => String::new(),
    }
}

fn bounds(rect: Rect) -> accesskit::Rect {
    accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The In/Out section's rows, drawn and disabled until routing and
/// recording (R5).
fn in_out_section(ui: &mut Ui, theme: &Theme, column: &Column, span: (f32, f32), kind: TrackKind) {
    let rows: &[(&str, &str, &str)] = match column.kind {
        ColumnKind::Track if matches!(kind, TrackKind::Audio | TrackKind::Midi) => &[
            ("From", "", ""),
            (
                "Input Type",
                if kind == TrackKind::Midi {
                    "All Ins"
                } else {
                    "Ext. In"
                },
                "WP-2.13",
            ),
            (
                "Input Channel",
                if kind == TrackKind::Midi {
                    "All Channels"
                } else {
                    "1/2"
                },
                "WP-2.13",
            ),
            ("Monitor", "In Auto Off", "WP-2.13"),
            ("To", "", ""),
            ("Output Type", "Main", "WP-2.19"),
            ("Output Channel", "", "WP-2.19"),
        ],
        ColumnKind::Track => &[
            ("To", "", ""),
            ("Output Type", "Main", "WP-2.19"),
            ("Output Channel", "", "WP-2.19"),
        ],
        ColumnKind::Return => &[("Audio To", "", ""), ("Output Type", "Main", "WP-2.19")],
        ColumnKind::Main => &[
            ("Cue Out", "", ""),
            ("Cue Output", "1/2", "Phase 3"),
            ("Main Out", "", ""),
            ("Main Output", "1/2", "WP-2.19"),
        ],
    };
    for (i, (name, text, package)) in rows.iter().enumerate() {
        let rect = in_out_row(column.rect, span, i);
        if package.is_empty() {
            ui.painter_at(rect).text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                *name,
                theme.small_font(),
                theme.colors.text_dim.0,
            );
        } else {
            pending(
                ui,
                theme,
                rect,
                Id::new((
                    "mixer-in-out",
                    column.rect.left().to_bits(),
                    i,
                    span.0.to_bits(),
                )),
                Role::ComboBox,
                name,
                text,
                package,
            );
        }
    }
}

/// The Sends section: a knob per return, disabled until WP-2.19 (R5).
fn sends_section(
    ui: &mut Ui,
    theme: &Theme,
    column: &Column,
    span: (f32, f32),
    returns: usize,
    kind: TrackKind,
) {
    for index in 0..returns {
        let rect = send_knob(column.rect, span, index, returns);
        let letter = char::from(b'A' + (index % 26) as u8).to_string();
        let (name, text) = if column.kind == ColumnKind::Main {
            (format!("Return {letter} Pre/Post"), "Post".to_owned())
        } else {
            (format!("Send {letter}"), letter.clone())
        };
        let _ = kind;
        pending(
            ui,
            theme,
            rect,
            Id::new((
                "mixer-send",
                column.rect.left().to_bits(),
                index,
                span.0.to_bits(),
            )),
            Role::Slider,
            &name,
            &text,
            "WP-2.19",
        );
        if column.kind == ColumnKind::Return {
            // A return's own sends are dimmed (L-MIX-002).
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, theme.colors.panel.0.gamma_multiply(0.5));
        }
    }
}

/// A control another package brings: drawn and named where Live has it,
/// disabled, and saying which package brings it.
#[expect(clippy::too_many_arguments, reason = "one control, described in full")]
pub fn pending(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    id: Id,
    role: Role,
    name: &str,
    text: &str,
    package: &str,
) {
    let response = ui.interact(rect, id, Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect(
        rect,
        theme.sizes.corner,
        theme.colors.control.0.gamma_multiply(0.6),
        Stroke::new(1.0, theme.colors.separator.0),
        StrokeKind::Inside,
    );
    if !text.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            theme.small_font(),
            theme.colors.pending.0,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, false, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(role);
        node.set_label(name.to_owned());
        if !text.is_empty() {
            node.set_value(text.to_owned());
        }
        node.set_disabled();
        node.set_description(format!("Not built yet: arrives with {package}."));
    });
}

/// One track's controls, wherever they're drawn.
pub struct StripParts<'a> {
    /// The shell, for the set, the selection, and the meters.
    pub shell: &'a Shell,
    /// The theme.
    pub theme: &'a Theme,
    /// Where the controls are.
    pub place: Place,
    /// The track.
    pub track: TrackId,
    /// What its activator shows.
    pub activator: &'a str,
}

impl StripParts<'_> {
    /// A whole strip (R3): the volume field, pan, the fader and its meter,
    /// and the activator, Solo, and Arm (or Main's pending cue controls).
    fn strip(&self, ui: &mut Ui, strip: &StripLayout, kind: ColumnKind, events: &mut Vec<UiEvent>) {
        self.volume_field(ui, strip.volume_field, events);
        self.pan(ui, strip.pan, Some(strip.pan_triangle), events);
        self.fader(ui, strip.fader, events);
        self.meter(ui, strip.meter);
        match kind {
            ColumnKind::Main => {
                pending(
                    ui,
                    self.theme,
                    strip.buttons[0],
                    Id::new(("mixer-cue-volume", self.place)),
                    Role::Slider,
                    "Cue Volume",
                    "Cue",
                    "Phase 3",
                );
                pending(
                    ui,
                    self.theme,
                    strip.buttons[1],
                    Id::new(("mixer-solo-cue", self.place)),
                    Role::Button,
                    "Solo/Cue Mode",
                    "Solo",
                    "Phase 3",
                );
            }
            _ => {
                for (switch, rect) in [Switch::Activator, Switch::Solo, Switch::Arm]
                    .into_iter()
                    .zip(strip.buttons)
                {
                    self.switch(ui, switch, rect, events);
                }
            }
        }
    }

    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    /// The tracks a change reaches, each with where its control starts.
    fn starts(&self, control: Control) -> Vec<(TrackId, f64)> {
        let doc = self.doc();
        controls::targets(
            self.track,
            self.shell.arrangement().selected_tracks(),
            |t| doc.track(t).is_some(),
        )
        .into_iter()
        .filter_map(|t| control.position(doc, t).map(|p| (t, p)))
        .collect()
    }

    /// Runs `invocations` as one undo step.
    fn run_all(events: &mut Vec<UiEvent>, label: &'static str, invocations: Vec<Invocation>) {
        let several = invocations.len() > 1;
        if several {
            events.push(UiEvent::BeginGesture(label));
        }
        events.extend(invocations.into_iter().map(UiEvent::Invoke));
        if several {
            events.push(UiEvent::EndGesture);
        }
    }

    /// What every value control does besides its drag: a click focuses it;
    /// a double-click, Delete, and a screen reader's or the keyboard's
    /// steps change it (R6, R8).
    fn value_keys(
        &self,
        ui: &Ui,
        response: &Response,
        control: Control,
        events: &mut Vec<UiEvent>,
    ) {
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        let id = response.id;
        let mut steps = ui.input(|i| {
            i.num_accesskit_action_requests(id, Action::Increment) as i32
                - i.num_accesskit_action_requests(id, Action::Decrement) as i32
        });
        let mut fine = false;
        let mut reset = response.double_clicked();
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
            // The keys' own modifiers: Shift with Up or Down steps finely.
            ui.input(|i| {
                for event in &i.events {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    {
                        match key {
                            Key::ArrowUp => steps += 1,
                            Key::ArrowDown => steps -= 1,
                            Key::Delete => reset = true,
                            _ => continue,
                        }
                        fine |= modifiers.shift;
                    }
                }
            });
        }
        let doc = self.doc();
        if reset {
            // Spec: mixer-view R6 (I-CLICK-021, S-VAL-004).
            let invocations = self
                .starts(control)
                .iter()
                .map(|(t, _)| control.set(*t, control.default_position()))
                .collect();
            Self::run_all(events, control.gesture(), invocations);
        } else if steps != 0 {
            // Spec: mixer-view R6 (S-VAL-001, S-VAL-002).
            let invocations = self
                .starts(control)
                .iter()
                .filter_map(|(t, _)| {
                    let mixer = doc.track(*t)?.mixer();
                    Some(match control {
                        Control::Volume => Invocation::new(
                            "mixer.set_volume",
                            json!({ "track": t, "volume": controls::step_volume(mixer.volume, steps, fine) }),
                        ),
                        Control::Pan => control.set(*t, controls::step_pan(mixer.pan, steps)),
                    })
                })
                .collect();
            Self::run_all(events, control.gesture(), invocations);
        }
    }

    /// A drag on a value control, moving every target by the same amount:
    /// `travel` points of drag cover a unit of the control (R6, R8).
    fn value_drag(
        &self,
        ui: &Ui,
        response: &Response,
        control: Control,
        travel: f32,
        events: &mut Vec<UiEvent>,
    ) {
        let id = response.id.with("drag");
        if response.drag_started() {
            ui.data_mut(|d| {
                d.insert_temp(
                    id,
                    ValueDrag {
                        starts: self.starts(control),
                        moved: 0.0,
                    },
                );
            });
            events.push(UiEvent::BeginGesture(control.gesture()));
        }
        if response.dragged() {
            let dy = response.drag_delta().y;
            if dy != 0.0 {
                let fine = ui.input(|i| i.modifiers.shift);
                let step = f64::from(-dy / travel / if fine { FINE } else { 1.0 });
                let drag = ui.data_mut(|d| {
                    let drag = d.get_temp_mut_or_default::<ValueDrag>(id);
                    drag.moved += step;
                    drag.clone()
                });
                events.extend(
                    controls::moved(control, &drag.starts, drag.moved)
                        .into_iter()
                        .map(UiEvent::Invoke),
                );
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<ValueDrag>(id));
            events.push(UiEvent::EndGesture);
        }
    }

    fn describe_value(
        &self,
        ui: &Ui,
        response: &Response,
        name: &str,
        value: String,
        number: f64,
        range: (f64, f64),
    ) {
        response.widget_info(|| WidgetInfo::slider(true, number, name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Slider);
            node.set_label(name.to_owned());
            node.set_value(value);
            node.set_numeric_value(number);
            node.set_min_numeric_value(range.0);
            node.set_max_numeric_value(range.1);
            node.add_action(Action::Increment);
            node.add_action(Action::Decrement);
        });
    }

    /// The volume field (R6): the level in dB, dragged, stepped, or reset.
    pub fn volume_field(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let volume = track.mixer().volume;
        let response = ui.interact(
            rect,
            volume_field_id(self.place, self.track),
            Sense::click_and_drag(),
        );
        self.value_drag(ui, &response, Control::Volume, DRAG_TRAVEL, events);
        self.value_keys(ui, &response, Control::Volume, events);
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let face = if response.dragged() || response.is_pointer_button_down_on() {
            c.control_pressed.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        };
        painter.rect_filled(rect, self.theme.sizes.corner, face);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            volume_text(volume),
            self.theme.small_font(),
            c.text.0,
        );
        focus_ring(&painter, self.theme, rect, &response);
        self.describe_value(
            ui,
            &response,
            "Track Volume",
            volume_spoken(volume),
            controls::gain_db(volume).max(-1000.0),
            (-1000.0, controls::TOP_DB),
        );
    }

    /// The fader (R6): dragged, the handle follows the pointer.
    pub fn fader(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let volume = track.mixer().volume;
        let response = ui.interact(
            rect,
            fader_id(self.place, self.track),
            Sense::click_and_drag(),
        );
        self.value_drag(
            ui,
            &response,
            Control::Volume,
            rect.height().max(1.0),
            events,
        );
        self.value_keys(ui, &response, Control::Volume, events);
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let x = rect.center().x;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, c.separator.0),
        );
        let y = rect.bottom() - rect.height() * controls::fader_position(volume) as f32;
        let handle = Rect::from_center_size(pos2(x, y), vec2(rect.width().min(16.0), 5.0));
        let face = if response.dragged() {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.text_dim.0
        };
        painter.rect_filled(handle, 1.0, face);
        focus_ring(&painter, self.theme, rect, &response);
        self.describe_value(
            ui,
            &response,
            "Track Volume Fader",
            volume_spoken(volume),
            controls::gain_db(volume).max(-1000.0),
            (-1000.0, controls::TOP_DB),
        );
    }

    /// The pan control (R6), with its reset triangle above it in a strip.
    pub fn pan(&self, ui: &mut Ui, rect: Rect, triangle: Option<Rect>, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let pan = track.mixer().pan;
        let response = ui.interact(
            rect,
            pan_id(self.place, self.track),
            Sense::click_and_drag(),
        );
        self.value_drag(ui, &response, Control::Pan, DRAG_TRAVEL, events);
        self.value_keys(ui, &response, Control::Pan, events);
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        if self.place == Place::Header {
            painter.rect_filled(rect, self.theme.sizes.corner, c.control.0);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                pan_text(pan),
                self.theme.small_font(),
                c.text.0,
            );
        } else {
            // A knob: an arc from the top, round to where the pan points.
            let centre = rect.center();
            let radius = (rect.height().min(rect.width()) * 0.5 - 1.0).max(2.0);
            painter.circle_stroke(centre, radius, Stroke::new(1.0, c.separator.0));
            let angle = (pan as f32) * std::f32::consts::FRAC_PI_4 * 3.0;
            let tip = centre + vec2(angle.sin(), -angle.cos()) * radius;
            painter.line_segment([centre, tip], Stroke::new(2.0, c.text.0));
        }
        focus_ring(&painter, self.theme, rect, &response);
        self.describe_value(
            ui,
            &response,
            "Track Panning",
            pan_text(pan),
            pan,
            (-1.0, 1.0),
        );
        if let Some(triangle) = triangle {
            // Spec: mixer-view R6 (I-CLICK-020): the triangle resets pan.
            let id = Id::new(("mixer-pan-reset", self.place, self.track));
            let hit = ui.interact(triangle, id, Sense::click());
            let c2 = triangle.center();
            ui.painter_at(triangle.expand(1.0))
                .add(Shape::convex_polygon(
                    vec![
                        pos2(c2.x - 3.0, c2.y - 2.0),
                        pos2(c2.x + 3.0, c2.y - 2.0),
                        pos2(c2.x, c2.y + 2.0),
                    ],
                    c.text_dim.0,
                    Stroke::NONE,
                ));
            if hit.clicked() {
                let invocations = self
                    .starts(Control::Pan)
                    .iter()
                    .map(|(t, _)| Control::Pan.set(*t, 0.0))
                    .collect();
                Self::run_all(events, Control::Pan.gesture(), invocations);
            }
            hit.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Reset Panning"));
        }
    }

    /// A switch (R7, R8): the activator, Solo, or Arm. Tracks without it
    /// show nothing in its place.
    pub fn switch(&self, ui: &mut Ui, switch: Switch, rect: Rect, events: &mut Vec<UiEvent>) {
        let doc = self.doc();
        if !switch.on_track(doc, self.track) {
            return;
        }
        let on = switch.is_on(doc, self.track);
        let id = switch_id(self.place, switch, self.track);
        let response = ui.interact(rect, id, Sense::click());
        let pressed =
            response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
        if pressed {
            let add = ui.input(|i| i.modifiers.command);
            let invocations = controls::clicked(
                doc,
                switch,
                self.track,
                self.shell.arrangement().selected_tracks(),
                add,
            );
            Self::run_all(events, switch.gesture(), invocations);
        }
        let c = &self.theme.colors;
        let (name, text, lit) = match switch {
            Switch::Activator => ("Track Activator", self.activator, c.accent.0),
            Switch::Solo => ("Solo", "S", c.accent.0),
            Switch::Arm => ("Arm Recording", "●", c.record.0),
        };
        let painter = ui.painter_at(rect.expand(2.0));
        let face = if on {
            lit
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        };
        painter.rect_filled(rect, self.theme.sizes.corner, face);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            self.theme.small_font(),
            if on { c.accent_text.0 } else { c.text.0 },
        );
        focus_ring(&painter, self.theme, rect, &response);
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(name.to_owned());
            node.set_value(if on { "on" } else { "off" }.to_owned());
            node.set_description(match switch {
                Switch::Activator => {
                    format!("Track {}: switches the track off and on", self.activator)
                }
                Switch::Solo | Switch::Arm => {
                    "Click to release the others; with Cmd (Ctrl), add this track".to_owned()
                }
            });
        });
    }

    /// The meter (R10): left and right side by side, peak then RMS over it,
    /// on the fader's scale.
    pub fn meter(&self, ui: &mut Ui, rect: Rect) {
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, c.window.0);
        let meter = self.shell.meter(self.track);
        let (peak, rms) = meter.map_or(([0.0; 2], [0.0; 2]), |m| (m.peak, m.rms));
        let half = rect.width() / 2.0;
        for side in 0..2 {
            let bar = Rect::from_min_max(
                pos2(rect.left() + half * side as f32 + 0.5, rect.top()),
                pos2(rect.left() + half * (side + 1) as f32 - 0.5, rect.bottom()),
            );
            let level = |amplitude: f32| bar.bottom() - bar.height() * meter_position(amplitude);
            let colour = if peak[side] > 1.0 {
                c.meter_hot.0
            } else {
                c.meter.0
            };
            painter.rect_filled(
                Rect::from_min_max(pos2(bar.left(), level(peak[side])), bar.max),
                0.0,
                colour.gamma_multiply(0.45),
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(bar.left(), level(rms[side])), bar.max),
                0.0,
                colour,
            );
        }
        let id = meter_id(self.place, self.track);
        let response = ui.interact(rect, id, Sense::hover());
        let loudest = peak[0].max(peak[1]);
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::ProgressIndicator, true, "Track Meter")
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Meter);
            node.set_label("Track Meter".to_owned());
            node.set_value(format!(
                "peak {}, RMS {}",
                volume_text(f64::from(loudest)),
                volume_text(f64::from(rms[0].max(rms[1])))
            ));
            node.set_numeric_value(controls::gain_db(f64::from(loudest)).max(-1000.0));
        });
    }
}

fn focus_ring(painter: &egui::Painter, theme: &Theme, rect: Rect, response: &Response) {
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
}

/// A colour for a column's title bar.
#[must_use]
pub fn title_colour(theme: &Theme, track: TrackId, kind: TrackKind) -> Color32 {
    if kind == TrackKind::Main {
        theme.colors.control_pressed.0
    } else {
        track_colour(theme, track)
    }
}
