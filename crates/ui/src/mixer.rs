//! The Mixer view (WP-1.9; the mixer view spec): the Session View's columns
//! with their mixer, the Arrangement's mixer under its lanes, and the
//! controls a strip and a track header share: the volume fader and the
//! header's volume field, pan, the meter, the activator, Solo, and Arm.
//!
//! Placement comes from the layout layer ([`crate::layout::mixer`]). Every
//! control runs a registered command: the values are `bl-model`'s `mixer.*`
//! commands, worked out by `bl_shell::mixer_controls` from the set and the
//! selection, and a drag runs them inside one gesture
//! ([`UiEvent::BeginGesture`]), so it's one undo step however far it goes.
//! Showing, hiding, scrolling, and focusing the mixer are the
//! `mixer_view.*` commands. Only the columns in view are built (R17).

use bl_model::{CrossfadeAssign, Document, TrackId, TrackKind};
use bl_shell::arrangement_commands::SELECT_TRACKS;
use bl_shell::mixer_controls::{
    self as controls, Control, FINE, Switch, meter_position, pan_text, volume_readout,
    volume_spoken, volume_text,
};
use bl_shell::mixer_view::MixerOf;
use bl_shell::mixer_view_commands as ids;
use bl_shell::{Invocation, Shell};
use egui::accesskit::{self, Action, Role};
use egui::{
    Align2, Color32, Event, Id, Key, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::arrangement::scene::track_colour;
use crate::layout::mixer::{
    ARRANGEMENT_TITLE_ROW, Column, ColumnKind, MixerLayout, SCALE_LABELS, Sections, SessionLayout,
    StripLayout, crossfade_buttons, crossfader_slider, in_out_row, send_knob,
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
        crossfader: view.crossfader(),
        returns: view.return_tracks(),
    }
}

/// The ID of a track header's volume field.
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

/// The ID of a send knob.
#[must_use]
pub fn send_id(place: Place, track: TrackId, to: TrackId) -> Id {
    Id::new(("mixer-send", place, track, to))
}

/// The ID of a meter.
#[must_use]
pub fn meter_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-meter", place, track))
}

/// The ID of a mixer's column, which takes the keyboard focus that
/// Navigate ▸ Mixer gives the mixer (R13).
#[must_use]
pub fn column_id(of: MixerOf, track: TrackId) -> Id {
    Id::new(("mixer-column", of, track))
}

/// Where the text the Status Bar shows while a fader is dragged is kept
/// for the frame (R6).
fn readout_id() -> Id {
    Id::new("mixer-status-readout")
}

/// Where a value typed into a header's volume field is kept: the field and
/// what's been typed.
fn typing_id() -> Id {
    Id::new("mixer-typing")
}

/// What the Status Bar shows this frame instead of its message, while a
/// fader is dragged (observed: `Track Volume: -21.0 dB`). Taking it clears
/// it for the next frame.
#[must_use]
pub fn take_readout(ctx: &egui::Context) -> Option<String> {
    ctx.data_mut(|d| d.remove_temp::<String>(readout_id()))
}

/// Puts `text` in the Status Bar for this frame instead of its message, as
/// while a device's control is dragged (WP-2.5).
pub(crate) fn show_readout(ctx: &egui::Context, text: String) {
    ctx.data_mut(|d| d.insert_temp(readout_id(), text));
}

/// Whether a track header's volume field has the keyboard, so a value can
/// be typed into it: the keys then go to the field, as to a text field, not
/// to shortcuts (so Minus starts a negative value rather than zooming). Esc,
/// or a click elsewhere, gives them back.
#[must_use]
pub fn is_typing(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<(Id, String)>(typing_id()).is_some())
        || ctx.memory(|m| {
            m.focused().is_some_and(|id| {
                ctx.data(|d| d.get_temp::<bool>(id.with("typed-field")).is_some())
            })
        })
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

/// A drag on the crossfader: where it started, and how far the pointer has
/// moved since, in points.
#[derive(Clone, Copy, Debug, Default)]
struct CrossfadeDrag {
    start: f64,
    moved: f32,
}

/// The Mixer view's state between frames: the focus requests it has
/// carried out, and a column the focus moves to. Drags keep their state in
/// egui's memory, by control.
#[derive(Debug, Default)]
pub struct MixerUi {
    focus_seen: u64,
    /// The track whose column takes the focus once it's in view.
    focus_to: Option<TrackId>,
    /// The columns the last frame built: tracks, returns, Main (R17).
    built: usize,
    /// A track to scroll into view in a mixer, for a keyboard focus move to
    /// one of its controls (the accessibility spec, R4).
    reveal: Option<(MixerOf, TrackId)>,
    /// The Session View grid's slots (WP-3.7).
    grid: crate::session_view::GridUi,
}

impl MixerUi {
    /// A mixer with nothing under way.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scrolls a track's column into view in a mixer, without moving the
    /// focus: a focus move to one of its controls follows once it's drawn.
    pub fn reveal(&mut self, of: MixerOf, track: TrackId) {
        self.reveal = Some((of, track));
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
        title_bars(ui, shell, theme, layout.title_bars, MixerOf::Session);
        // The clip slots and scenes (M3, WP-3.7).
        self.grid.show(ui, shell, theme, &layout, events);
        painter.line_segment(
            [layout.status_row.left_top(), layout.status_row.right_top()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        if let Some(mixer) = layout.mixer {
            self.show_mixer(ui, shell, theme, mixer, MixerOf::Session, events);
        }
    }

    /// Draws one view's mixer in `area` and adds what the person did to
    /// `events` (R1 to R3, R5, R10, R13, R14). The Arrangement's has a row
    /// of title bars along its top.
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
        let sections_area = if of == MixerOf::Arrangement {
            let row = Rect::from_min_max(
                area.min,
                pos2(area.right(), area.top() + ARRANGEMENT_TITLE_ROW),
            );
            title_bars(ui, shell, theme, row, of);
            area.with_min_y(row.bottom())
        } else {
            area
        };
        let layout = MixerLayout::compute(
            sections_area,
            columns.len(),
            returns.len(),
            sections,
            scroll,
        );
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
            // The column itself takes the keyboard focus Navigate ▸ Mixer
            // gives the mixer; Left and Right then move the selection.
            let group = column_id(of, track);
            let focus = column_ui.interact(clip, group, Sense::focusable_noninteractive());
            column_ui.ctx().accesskit_node_builder(group, |node| {
                node.set_role(Role::Group);
                node.set_label(t.name().to_owned());
                node.set_bounds(bounds(clip));
            });
            crate::focus::register(
                column_ui.ctx(),
                group,
                bl_shell::Area::MainView,
                Some(crate::focus::TrackControl {
                    place: Place::Mixer(of),
                    track,
                    slot: crate::focus::Slot::Column,
                }),
            );
            if focus.has_focus() {
                self.column_keys(&column_ui, shell, &columns, track, of, events);
            }
            if self.focus_to == Some(track) && clip.width() >= column.rect.width() - 0.5 {
                focus.request_focus();
                self.focus_to = None;
            }
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
            // The track's colour along the column's bottom (L-MIX-009).
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(column.rect.left() + 0.5, layout.colour.0),
                    pos2(column.rect.right() - 0.5, layout.colour.1),
                ),
                0.0,
                title_colour(theme, track, t.kind()),
            );
            let label = activator_label(column);
            if let Some(span) = layout.in_out
                && let Some(section) = bl_shell::routing::in_out(doc, track, &shell.interface())
            {
                let parts = crate::in_out::InOutParts {
                    theme,
                    place,
                    track,
                };
                parts.draw(
                    &mut column_ui,
                    &section,
                    true,
                    |i| Some(in_out_row(column.rect, span, i)),
                    events,
                );
            }
            if let Some(span) = layout.sends {
                let parts = StripParts {
                    shell,
                    theme,
                    place,
                    track,
                    activator: &label,
                };
                parts.sends_section(&mut column_ui, column, span, &returns, events);
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
            if let Some(span) = layout.crossfader {
                let parts = StripParts {
                    shell,
                    theme,
                    place,
                    track,
                    activator: &label,
                };
                parts.crossfade_section(&mut column_ui, column, span, events);
            }
        }
        self.scroll(&ui, sections_area, of, events);
        self.focus(shell, &layout, &columns, of, events);
    }

    /// Left and Right with a column focused move the track selection, and
    /// the focus with it (R13).
    fn column_keys(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        columns: &[TrackId],
        track: TrackId,
        of: MixerOf,
        events: &mut Vec<UiEvent>,
    ) {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                column_id(of, track),
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let step = ui.input(|i| {
            i32::from(i.key_pressed(Key::ArrowRight)) - i32::from(i.key_pressed(Key::ArrowLeft))
        });
        if step == 0 {
            return;
        }
        let Some(at) = columns.iter().position(|c| *c == track) else {
            return;
        };
        let next = (at as i64 + i64::from(step)).clamp(0, columns.len() as i64 - 1) as usize;
        let _ = shell;
        events.push(UiEvent::Invoke(Invocation::new(
            SELECT_TRACKS,
            json!({ "tracks": [columns[next]] }),
        )));
        self.focus_to = Some(columns[next]);
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

    /// Carries out `mixer_view.focus` in the Arrangement's mixer: the focus
    /// goes to the selected track's column, or the first's, scrolled into
    /// view (R13).
    fn focus(
        &mut self,
        shell: &Shell,
        layout: &MixerLayout,
        columns: &[TrackId],
        of: MixerOf,
        events: &mut Vec<UiEvent>,
    ) {
        let requests = shell.mixer_view().focus_requests();
        if of == MixerOf::Arrangement && requests != self.focus_seen {
            self.focus_seen = requests;
            self.focus_to = shell
                .arrangement()
                .selected_tracks()
                .iter()
                .find(|t| columns.contains(t))
                .or(columns.first())
                .copied();
        }
        if let Some((mixer, track)) = self.reveal
            && mixer == of
        {
            self.reveal = None;
            if let Some(index) = columns.iter().position(|c| *c == track) {
                scroll_into_view(layout, index, of, events);
            }
        }
        let Some(target) = self.focus_to else {
            return;
        };
        let Some(index) = columns.iter().position(|c| *c == target) else {
            self.focus_to = None;
            return;
        };
        scroll_into_view(layout, index, of, events);
    }
}

/// Scrolls a mixer so the track column at `index` is wholly in view, if it
/// isn't.
fn scroll_into_view(layout: &MixerLayout, index: usize, of: MixerOf, events: &mut Vec<UiEvent>) {
    let in_view = layout
        .columns
        .iter()
        .any(|c| c.index == index && layout.tracks.contains_rect(c.rect.shrink(0.5)));
    if in_view {
        return;
    }
    let width = f64::from(crate::layout::mixer::COLUMN_WIDTH);
    let left = width * index as f64 - f64::from(layout.scroll);
    let x = if left < 0.0 {
        left
    } else {
        left + width - f64::from(layout.tracks.width())
    };
    events.push(UiEvent::Invoke(Invocation::new(
        ids::SCROLL,
        json!({ "view": of, "x": x }),
    )));
}

/// A row of title bars over a mixer's columns: the Session View's top row
/// (L-SES-002) and the Arrangement mixer's top row, each track's colour and
/// name.
fn title_bars(ui: &Ui, shell: &Shell, theme: &Theme, row: Rect, of: MixerOf) {
    let doc = shell.editor().document();
    let columns = shell.mixer_columns();
    let returns: Vec<TrackId> = doc.returns().collect();
    let scroll = shell.mixer_view().of(of).scroll as f32;
    let layout = MixerLayout::compute(row, columns.len(), returns.len(), sections(shell), scroll);
    for column in layout.every_column() {
        let track = column_track(doc, &columns, &returns, column);
        let Some(t) = doc.track(track) else {
            continue;
        };
        let bar = column.rect.shrink2(vec2(0.5, 0.0));
        let bar = if column.kind == ColumnKind::Track {
            bar.intersect(layout.tracks)
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

/// Whether a track's strip is a MIDI track's without an instrument: a
/// dotted MIDI meter and the switches, without volume or pan (observed).
/// Beatlayer has no instruments yet, so a MIDI track counts as one without
/// while its chain holds no device that makes sound.
fn midi_only(doc: &Document, track: TrackId) -> bool {
    doc.track(track)
        .is_some_and(|t| t.kind() == TrackKind::Midi && t.devices().len() == 0)
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
    /// The Sends section (R5; the routing spec): a knob per return, in two
    /// staggered rows. A return's own sends are dimmed until WP-2.19b
    /// enables them; Main's column holds each return's Pre/Post switch.
    fn sends_section(
        &self,
        ui: &mut Ui,
        column: &Column,
        span: (f32, f32),
        returns: &[TrackId],
        events: &mut Vec<UiEvent>,
    ) {
        let doc = self.doc();
        for (index, &to) in returns.iter().enumerate() {
            let rect = send_knob(column.rect, span, index, returns.len());
            match column.kind {
                ColumnKind::Main => self.pre_post(ui, rect, to, index, events),
                ColumnKind::Return => {
                    let letter = bl_shell::routing::return_letter(index);
                    let name = doc
                        .track(to)
                        .map_or_else(String::new, |t| t.name().to_owned());
                    pending(
                        ui,
                        self.theme,
                        rect,
                        Id::new(("mixer-return-send", self.place, self.track, index)),
                        Role::Slider,
                        &name,
                        &letter,
                        "WP-2.19b",
                    );
                    // A return's own sends are dimmed (L-MIX-002).
                    ui.painter_at(rect).rect_filled(
                        rect,
                        0.0,
                        self.theme.colors.panel.0.gamma_multiply(0.5),
                    );
                }
                // A MIDI track without an instrument has no sends to
                // show (observed: none in its Tab order).
                ColumnKind::Track if midi_only(doc, self.track) => {}
                ColumnKind::Track => self.send(ui, rect, to, index, events),
            }
        }
    }

    /// A send knob (R5; the routing spec): drags, keys, and a double-click
    /// as the other value controls, from −∞ to 0 dB.
    pub fn send(
        &self,
        ui: &mut Ui,
        rect: Rect,
        to: TrackId,
        index: usize,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = self.doc();
        let control = Control::Send(to);
        let Some(level) = control.value(doc, self.track) else {
            return;
        };
        let response = ui.interact(
            rect,
            send_id(self.place, self.track, to),
            Sense::click_and_drag(),
        );
        self.register(ui, response.id, crate::focus::Slot::Send(index));
        self.escape_to_title(ui, &response);
        self.value_drag(ui, &response, control, events);
        self.value_keys(ui, &response, control, events);
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let letter = bl_shell::routing::return_letter(index);
        if self.place == Place::Header {
            painter.rect_filled(rect, self.theme.sizes.corner, c.control.0);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                &letter,
                self.theme.small_font(),
                c.text.0,
            );
        } else {
            // A knob: a ring, an arc for the level, and the return's letter.
            let centre = rect.center();
            let radius = (rect.height().min(rect.width()) * 0.5 - 1.0).max(2.0);
            painter.circle_stroke(centre, radius, Stroke::new(1.0, c.separator.0));
            let position = controls::send_position(level) as f32;
            let sweep = std::f32::consts::FRAC_PI_4 * 3.0;
            let angle = -sweep + 2.0 * sweep * position;
            let tip = centre + vec2(angle.sin(), -angle.cos()) * radius;
            painter.line_segment([centre, tip], Stroke::new(2.0, c.text.0));
            painter.text(
                pos2(centre.x, rect.bottom()),
                Align2::CENTER_BOTTOM,
                &letter,
                self.theme.small_font(),
                c.text_dim.0,
            );
        }
        focus_ring(&painter, self.theme, rect, &response);
        // Named by its return track, as Live's tree names sends
        // ("A-Reverb"; observed in Live 12.4.6).
        let name = bl_shell::routing::track_label(doc, to);
        self.describe_value(
            ui,
            &response,
            &name,
            format!("{} dB", controls::send_text(level)),
            level,
            (0.0, 1.0),
        );
    }

    /// A return's Pre/Post switch, on Main's column (L-MIX-002): a click
    /// switches it.
    fn pre_post(
        &self,
        ui: &mut Ui,
        rect: Rect,
        to: TrackId,
        index: usize,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(tap) = self.doc().track(to).map(|t| t.mixer().tap) else {
            return;
        };
        let pre = tap == bl_model::SendTap::Pre;
        let id = Id::new(("mixer-pre-post", self.place, to));
        let response = ui.interact(rect, id, Sense::click());
        // In Main's Tab order where a track's sends are (where Live puts it
        // isn't observed).
        self.register(ui, id, crate::focus::Slot::Send(index));
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        painter.rect_filled(
            rect,
            self.theme.sizes.corner,
            if pre { c.accent.0 } else { c.control.0 },
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            if pre { "Pre" } else { "Post" },
            self.theme.small_font(),
            if pre { c.accent_text.0 } else { c.text.0 },
        );
        focus_ring(&painter, self.theme, rect, &response);
        if response.clicked() {
            events.push(UiEvent::Invoke(Invocation::new(
                "mixer.set_send_tap",
                json!({ "track": to, "tap": if pre { "post" } else { "pre" } }),
            )));
        }
        let letter = bl_shell::routing::return_letter(index);
        let name = format!("Return {letter} Pre/Post");
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, pre, &name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_value(if pre { "Pre" } else { "Post" }.to_owned());
        });
    }

    /// A whole strip (R3), where Live has each control: the peak level
    /// readout, pan, the activator, Solo, and Arm in a column, the fader,
    /// the meter, and its scale. A MIDI track without an instrument shows a
    /// MIDI meter and the switches; Main shows the cue volume and the
    /// Solo/Cue button, pending, in place of its switches.
    fn strip(&self, ui: &mut Ui, strip: &StripLayout, kind: ColumnKind, events: &mut Vec<UiEvent>) {
        let midi = midi_only(self.doc(), self.track);
        if midi {
            self.midi_meter(ui, strip.meter);
        } else {
            self.peak_field(ui, strip.peak_field);
            self.pan(ui, strip.pan, Some(strip.pan_triangle), events);
            self.fader(ui, strip.fader, events);
            self.meter(ui, strip.meter);
            self.scale(ui, strip);
        }
        match kind {
            ColumnKind::Main => {
                pending(
                    ui,
                    self.theme,
                    strip.activator,
                    Id::new(("mixer-cue-volume", self.place)),
                    Role::Slider,
                    "Cue Volume",
                    "Cue",
                    "Phase 3",
                );
                pending(
                    ui,
                    self.theme,
                    strip.solo,
                    Id::new(("mixer-solo-cue", self.place)),
                    Role::Button,
                    "Solo/Cue Mode",
                    "Solo",
                    "Phase 3",
                );
            }
            _ => {
                for (switch, rect) in [
                    (Switch::Activator, strip.activator),
                    (Switch::Solo, strip.solo),
                    (Switch::Arm, strip.arm),
                ] {
                    self.switch(ui, switch, rect, events);
                }
            }
        }
    }

    /// The Crossfader section (L-MIX-006; the mixer spec, R23): a track's
    /// or a return's A and B assign buttons under its strip, and the
    /// crossfader itself in Main's column. A track without the buttons (a
    /// MIDI track with no instrument, as observed) shows neither.
    fn crossfade_section(
        &self,
        ui: &mut Ui,
        column: &Column,
        span: (f32, f32),
        events: &mut Vec<UiEvent>,
    ) {
        let doc = self.doc();
        if column.kind == ColumnKind::Main {
            self.crossfader(ui, column.rect, span, events);
            return;
        }
        if midi_only(doc, self.track) {
            return;
        }
        let current = doc
            .track(self.track)
            .map_or(CrossfadeAssign::None, |t| t.mixer().crossfade);
        let [a, b] = crossfade_buttons(column.rect, span);
        for (side, rect) in [(CrossfadeAssign::A, a), (CrossfadeAssign::B, b)] {
            self.assign_button(ui, rect, side, current, events);
        }
    }

    /// One A or B assign button: lit while the track plays on that side. A
    /// click assigns it, or releases the track when it's already lit; the
    /// two sides are exclusive.
    fn assign_button(
        &self,
        ui: &mut Ui,
        rect: Rect,
        side: CrossfadeAssign,
        current: CrossfadeAssign,
        events: &mut Vec<UiEvent>,
    ) {
        let on = current == side;
        let id = Id::new(("mixer-crossfade-assign", self.place, self.track, side));
        let response = ui.interact(rect, id, Sense::click());
        self.escape_to_title(ui, &response);
        let pressed =
            response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
        if pressed
            && let Some(invocation) = controls::crossfade_clicked(self.doc(), self.track, side)
        {
            events.push(UiEvent::Invoke(invocation));
        }
        let c = &self.theme.colors;
        let face = if on {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        };
        let painter = ui.painter_at(rect.expand(2.0));
        painter.rect_filled(rect, self.theme.sizes.corner, face);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            if side == CrossfadeAssign::A { "A" } else { "B" },
            self.theme.small_font(),
            if on { c.accent_text.0 } else { c.text.0 },
        );
        focus_ring(&painter, self.theme, rect, &response);
        // Live's own name for the buttons isn't observed.
        let name = format!(
            "Crossfade {}",
            if side == CrossfadeAssign::A { "A" } else { "B" }
        );
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, &name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(name);
            node.set_value(if on { "on" } else { "off" }.to_owned());
            node.set_description("Assigns the track to this side of the crossfader".to_owned());
        });
    }

    /// Main's crossfader (R23): a horizontal slider, 0 (the A end) at its
    /// left and 1 (the B end) at its right, dragging relative to the
    /// pointer like the mixer's other faders.
    fn crossfader(&self, ui: &mut Ui, column: Rect, span: (f32, f32), events: &mut Vec<UiEvent>) {
        let position = self.doc().settings().crossfader;
        let rect = crossfader_slider(column, span);
        let id = Id::new(("mixer-crossfader", self.place));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        self.escape_to_title(ui, &response);
        let drag_id = id.with("drag");
        if response.drag_started() {
            ui.data_mut(|d| {
                d.insert_temp(
                    drag_id,
                    CrossfadeDrag {
                        start: position,
                        moved: 0.0,
                    },
                );
            });
            events.push(UiEvent::BeginGesture(controls::CROSSFADER_GESTURE));
        }
        if response.dragged() {
            let dx = response.drag_delta().x;
            let drag = ui.data_mut(|d| {
                let drag = d.get_temp_mut_or_default::<CrossfadeDrag>(drag_id);
                drag.moved += dx;
                *drag
            });
            let at = controls::crossfader_moved(drag.start, drag.moved, rect.width());
            ui.data_mut(|d| d.insert_temp(readout_id(), controls::crossfader_readout(at)));
            if at != position {
                events.push(UiEvent::Invoke(Invocation::new(
                    "mixer.set_crossfader",
                    json!({ "position": at }),
                )));
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<CrossfadeDrag>(drag_id));
            events.push(UiEvent::EndGesture);
        }
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(3.0));
        let y = rect.top() + rect.height() * 0.5;
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(2.0, c.separator.0),
        );
        let x = rect.left() + rect.width() * position as f32;
        let handle = Rect::from_center_size(pos2(x, y), vec2(5.0, rect.height()));
        let face = if response.dragged() {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.text_dim.0
        };
        painter.rect_filled(handle, 1.0, face);
        focus_ring(&painter, self.theme, rect, &response);
        // A slider a screen reader can read. Live's own keyboard steps for
        // the crossfader aren't observed, so it offers none.
        response.widget_info(|| WidgetInfo::slider(true, position, "Crossfader"));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Slider);
            node.set_label("Crossfader".to_owned());
            node.set_value(controls::crossfader_text(position));
            node.set_numeric_value(position);
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(1.0);
        });
    }

    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    /// Esc from a Session strip's control goes to the track's title bar,
    /// which the column stands for (observed in Live 12.4.6; the
    /// accessibility spec, R4). In the Arrangement, Esc is
    /// `arrangement.focus_title_bar`'s key.
    fn escape_to_title(&self, ui: &Ui, response: &Response) {
        let Place::Mixer(of) = self.place else {
            return;
        };
        if !response.has_focus() {
            return;
        }
        // Keep the focus through Esc, to move it rather than drop it. The
        // value controls set their own filter, which keeps it too.
        let value = [
            fader_id(self.place, self.track),
            pan_id(self.place, self.track),
        ];
        if !value.contains(&response.id) {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        escape: true,
                        ..Default::default()
                    },
                );
            });
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            ui.memory_mut(|m| m.request_focus(column_id(of, self.track)));
        }
    }

    /// Puts a control in the keyboard's order (the accessibility spec, R3
    /// and R4).
    fn register(&self, ui: &Ui, id: Id, slot: crate::focus::Slot) {
        crate::focus::register(
            ui.ctx(),
            id,
            bl_shell::Area::MainView,
            Some(crate::focus::TrackControl {
                place: self.place,
                track: self.track,
                slot,
            }),
        );
    }

    /// The tracks a change reaches, each with its value: the changed track
    /// first.
    fn starts(&self, control: Control) -> Vec<(TrackId, f64)> {
        let doc = self.doc();
        controls::targets(
            self.track,
            self.shell.arrangement().selected_tracks(),
            |t| doc.track(t).is_some(),
        )
        .into_iter()
        .filter_map(|t| control.value(doc, t).map(|v| (t, v)))
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
    /// steps change it, on every selected track by the same dB (R6, R8).
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
        let entry_open = ui.data(|d| d.get_temp::<(Id, String)>(typing_id()).is_some());
        if response.has_focus() && !entry_open {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        // Esc goes back to the track's title bar, or drops a
                        // typed value, rather than just dropping the focus
                        // (the accessibility spec, R4).
                        escape: true,
                        ..Default::default()
                    },
                );
            });
            // The keys' own modifiers: Shift with Up or Down steps finely.
            ui.input(|i| {
                for event in &i.events {
                    if let Event::Key {
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
            // Spec: mixer-view R6 (I-CLICK-021, S-VAL-004; observed: a
            // double-click resets the fader to 0 dB).
            let invocations = self
                .starts(control)
                .iter()
                .map(|(t, _)| control.set(*t, control.default_value()))
                .collect();
            Self::run_all(events, control.gesture(), invocations);
        } else if steps != 0 {
            // Spec: mixer-view R6, R8 (S-VAL-001, S-VAL-002).
            let invocations = self
                .starts(control)
                .iter()
                .filter_map(|(t, _)| {
                    let value = control.value(doc, *t)?;
                    Some(control.set(*t, control.step(value, steps, fine)))
                })
                .collect();
            Self::run_all(events, control.gesture(), invocations);
        }
    }

    /// A drag on a value control: relative, moving the changed track along
    /// its scale and every other target by the same dB (or pan) (R6, R8).
    /// While volume is dragged, the Status Bar shows it.
    fn value_drag(
        &self,
        ui: &Ui,
        response: &Response,
        control: Control,
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
            // Read before taking egui's data, whose lock the input's would
            // deadlock against.
            let fine = ui.input(|i| i.modifiers.shift);
            let drag = ui.data_mut(|d| {
                let drag = d.get_temp_mut_or_default::<ValueDrag>(id);
                if dy != 0.0 {
                    drag.moved +=
                        f64::from(-dy / control.drag_travel() / if fine { FINE } else { 1.0 });
                }
                drag.clone()
            });
            let changes = controls::moved(control, &drag.starts, drag.moved);
            if control == Control::Volume
                && let Some(volume) = changes
                    .first()
                    .and_then(|first| first.args["volume"].as_f64())
            {
                ui.data_mut(|d| d.insert_temp(readout_id(), volume_readout(volume)));
            }
            if dy != 0.0 {
                events.extend(changes.into_iter().map(UiEvent::Invoke));
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

    /// A track header's volume field (R4, R6): the level in dB, dragged,
    /// stepped, reset, or typed (`-12` and Return gives −12.0 dB).
    pub fn volume_field(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let volume = track.mixer().volume;
        let id = volume_field_id(self.place, self.track);
        let response = ui.interact(rect, id, Sense::click_and_drag());
        self.register(ui, id, crate::focus::Slot::Volume);
        self.value_drag(ui, &response, Control::Volume, events);
        self.value_keys(ui, &response, Control::Volume, events);
        let typed = self.typing(ui, &response, events);
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let face = if typed.is_some() || response.dragged() || response.is_pointer_button_down_on()
        {
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
            typed.unwrap_or_else(|| volume_text(volume)),
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

    /// Typing into the focused volume field (observed in the Arrangement's
    /// headers): digits, Minus, and Period type, Backspace takes back, Return
    /// sets the value for every selected track by the same dB, Esc drops it.
    /// Returns what's been typed.
    fn typing(&self, ui: &Ui, response: &Response, events: &mut Vec<UiEvent>) -> Option<String> {
        let id = response.id;
        // Marks the field as one that takes typed values, for `is_typing`.
        ui.data_mut(|d| d.insert_temp(id.with("typed-field"), true));
        let mut entry = ui
            .data(|d| d.get_temp::<(Id, String)>(typing_id()))
            .filter(|(field, _)| *field == id)
            .map(|(_, text)| text);
        if !response.has_focus() {
            if entry.is_some() {
                ui.data_mut(|d| d.remove::<(Id, String)>(typing_id()));
            }
            return None;
        }
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
                        || matches!(key, Key::Minus | Key::Period | Key::Backspace)
                        || (entry.is_some() && key == &Key::Enter)
                        || key == &Key::Escape) =>
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
                Key::Enter => {
                    let text = entry.take().unwrap_or_default();
                    if let Ok(db) = text.parse::<f64>() {
                        let target = controls::volume_gain(db);
                        let starts = self.starts(Control::Volume);
                        let shift = controls::gain_db(target)
                            - starts.first().map_or(0.0, |(_, g)| {
                                controls::gain_db(*g).max(controls::LOWEST_DB)
                            });
                        let invocations = starts
                            .iter()
                            .enumerate()
                            .map(|(i, (t, g))| {
                                let value = if i == 0 {
                                    target
                                } else if target <= 0.0 {
                                    0.0
                                } else {
                                    controls::shift_volume(*g, shift)
                                };
                                Control::Volume.set(*t, value)
                            })
                            .collect();
                        Self::run_all(events, Control::Volume.gesture(), invocations);
                    }
                }
                // Esc drops what's typed; egui gives the keyboard back.
                // With nothing typed, it goes back to the track's title bar,
                // as from any of a track's controls (the accessibility spec,
                // R4; S-ARR-034).
                Key::Escape => {
                    if entry.is_none() && self.place == Place::Header {
                        events.push(UiEvent::Invoke(Invocation::new(
                            bl_shell::arrangement_commands::FOCUS_TITLE_BAR,
                            json!({}),
                        )));
                    } else {
                        ui.memory_mut(|m| m.surrender_focus(id));
                    }
                    entry = None;
                }
                Key::Backspace => {
                    if let Some(text) = entry.as_mut() {
                        text.pop();
                    }
                }
                key => {
                    let c = digit(key).unwrap_or(match key {
                        Key::Minus => '-',
                        _ => '.',
                    });
                    let text = entry.get_or_insert_with(String::new);
                    if text.len() < 8 {
                        text.push(c);
                    }
                }
            }
        }
        ui.data_mut(|d| match &entry {
            Some(text) => {
                d.insert_temp(typing_id(), (id, text.clone()));
            }
            None => d.remove::<(Id, String)>(typing_id()),
        });
        entry
    }

    /// The fader (R6): a relative drag moves the handle a point per point
    /// of drag along the meter's scale, which the handle sits on.
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
        self.register(ui, response.id, crate::focus::Slot::Volume);
        self.escape_to_title(ui, &response);
        self.value_drag(ui, &response, Control::Volume, events);
        self.value_keys(ui, &response, Control::Volume, events);
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(3.0));
        let x = rect.center().x;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, c.separator.0),
        );
        let y = rect.bottom() - rect.height() * controls::fader_position(volume) as f32;
        let handle = Rect::from_center_size(pos2(x, y), vec2(rect.width(), 5.0));
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
            "Track Volume",
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
        self.register(ui, response.id, crate::focus::Slot::Pan);
        self.escape_to_title(ui, &response);
        self.value_drag(ui, &response, Control::Pan, events);
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
            // A knob: a ring, and a line from its centre to where the pan
            // points.
            let centre = rect.center();
            let radius = (rect.height().min(rect.width()) * 0.5 - 1.0).max(2.0);
            painter.circle_stroke(centre, radius, Stroke::new(1.0, c.separator.0));
            let angle = (pan as f32) * std::f32::consts::FRAC_PI_4 * 3.0;
            let tip = centre + vec2(angle.sin(), -angle.cos()) * radius;
            painter.line_segment([centre, tip], Stroke::new(2.0, c.text.0));
        }
        focus_ring(&painter, self.theme, rect, &response);
        // Live's tree calls it Track Pan, and Main's Main Pan (observed in
        // Live 12.4.6; the accessibility spec, R1).
        let name = if track.kind() == TrackKind::Main {
            "Main Pan"
        } else {
            "Track Pan"
        };
        self.describe_value(ui, &response, name, pan_text(pan), pan, (-1.0, 1.0));
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
    /// show nothing in its place; a deactivated activator shows its number
    /// on grey (observed).
    pub fn switch(&self, ui: &mut Ui, switch: Switch, rect: Rect, events: &mut Vec<UiEvent>) {
        let doc = self.doc();
        if !switch.on_track(doc, self.track) {
            return;
        }
        let on = switch.is_on(doc, self.track);
        let id = switch_id(self.place, switch, self.track);
        let response = ui.interact(rect, id, Sense::click());
        self.register(
            ui,
            id,
            match switch {
                Switch::Activator => crate::focus::Slot::Activator,
                Switch::Solo => crate::focus::Slot::Solo,
                Switch::Arm => crate::focus::Slot::Arm,
            },
        );
        self.escape_to_title(ui, &response);
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

    fn levels(&self) -> ([f32; 2], [f32; 2]) {
        self.shell
            .meter(self.track)
            .map_or(([0.0; 2], [0.0; 2]), |m| (m.peak, m.rms))
    }

    /// The peak level readout at the strip's top left (observed: `-inf`
    /// with nothing playing; the fader doesn't change it). It shows the
    /// meter's peak; whether Live holds it until clicked isn't observed.
    fn peak_field(&self, ui: &mut Ui, rect: Rect) {
        let (peak, _) = self.levels();
        let loudest = f64::from(peak[0].max(peak[1]));
        let id = Id::new(("mixer-peak", self.place, self.track));
        // In the strip's Tab order (observed in Live 12.4.6).
        let response = ui.interact(rect, id, Sense::focusable_noninteractive());
        self.register(ui, id, crate::focus::Slot::Peak);
        self.escape_to_title(ui, &response);
        focus_ring(
            &ui.painter_at(rect.expand(3.0)),
            self.theme,
            rect,
            &response,
        );
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(1.0));
        painter.rect_filled(rect, self.theme.sizes.corner, c.window.0);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            volume_text(loudest),
            self.theme.small_font(),
            c.text_dim.0,
        );
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, "Peak Level"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Label);
            node.set_label("Peak Level".to_owned());
            node.set_value(volume_spoken(loudest));
        });
    }

    /// The meter's scale beside it: 0, 12, 24, 36, 48, 60 dB below 0, where
    /// the fader's handle sits at those values (observed).
    fn scale(&self, ui: &Ui, strip: &StripLayout) {
        let (left, right) = strip.labels;
        let painter = ui.painter();
        for (label, y) in SCALE_LABELS {
            painter.text(
                pos2((left + right) / 2.0, strip.top + y),
                Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(self.theme.sizes.small_text * 0.8),
                self.theme.colors.text_dim.0,
            );
        }
    }

    /// The meter (R10): left and right side by side on the fader's scale,
    /// the peak and the RMS level drawn as the reference describes them
    /// (both shown); how Live draws the two together with a signal isn't
    /// observed yet.
    pub fn meter(&self, ui: &mut Ui, rect: Rect) {
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, c.window.0);
        let (peak, rms) = self.levels();
        let half = rect.width() / 2.0;
        for side in 0..2 {
            let bar = Rect::from_min_max(
                pos2(rect.left() + half * side as f32 + 0.5, rect.top()),
                pos2(rect.left() + half * (side + 1) as f32 - 0.5, rect.bottom()),
            );
            let level = |amplitude: f32| bar.bottom() - bar.height() * meter_position(amplitude);
            painter.rect_filled(
                Rect::from_min_max(pos2(bar.left(), level(peak[side])), bar.max),
                0.0,
                c.meter.0.gamma_multiply(0.45),
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(bar.left(), level(rms[side])), bar.max),
                0.0,
                c.meter.0,
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

    /// A MIDI track's meter (observed on a MIDI track without an
    /// instrument: a dotted meter). It shows no notes until MIDI plays
    /// (WP-2.1).
    fn midi_meter(&self, ui: &mut Ui, rect: Rect) {
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect);
        let mut y = rect.bottom() - 2.0;
        while y > rect.top() {
            painter.circle_filled(pos2(rect.center().x, y), 1.0, c.separator.0);
            y -= 4.0;
        }
        let id = meter_id(self.place, self.track);
        let response = ui.interact(rect, id, Sense::hover());
        response
            .widget_info(|| WidgetInfo::labeled(WidgetType::ProgressIndicator, true, "MIDI Meter"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Meter);
            node.set_label("MIDI Meter".to_owned());
            node.set_value("no notes".to_owned());
        });
    }
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

fn focus_ring(painter: &egui::Painter, theme: &Theme, rect: Rect, response: &Response) {
    // Spec: accessibility R7. Live draws no outline round the focused
    // control unless Outline View in Focus is on.
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
}

/// A colour for a column's title bar and colour strip.
#[must_use]
pub fn title_colour(theme: &Theme, track: TrackId, kind: TrackKind) -> Color32 {
    if kind == TrackKind::Main {
        theme.colors.control_pressed.0
    } else {
        track_colour(theme, track)
    }
}
