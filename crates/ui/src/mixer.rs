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

use bl_engine::{MeterOwner, MeterSite};
use bl_model::modulator::ModTarget;
use bl_model::{
    AutomationTarget, ChainId, CrossfadeAssign, Document, MAX_TRACK_DELAY_MS,
    MAX_TRACK_DELAY_SAMPLES, MixerParam, PanMode, RackKind, TrackDelay, TrackId, TrackKind,
};
use bl_shell::arrangement_commands::SELECT_TRACKS;
use bl_shell::mixer_controls::{
    self as controls, Control, FINE, Switch, meter_position, pan_text, volume_readout,
    volume_spoken, volume_text,
};
use bl_shell::mixer_view::{ChainStrip, MeterValue, MixerOf, RackFold};
use bl_shell::mixer_view_commands as ids;
use bl_shell::{InfoTarget, Invocation, Shell};
use egui::accesskit::{self, Action, Role};
use egui::{
    Align2, Color32, Event, Id, Key, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::arrangement::scene::track_colour;
use crate::layout::mixer::{
    ARRANGEMENT_TITLE_ROW, ChainColumn, ChainStripLayout, Column, ColumnKind, MIN_STRIP_HEIGHT,
    MixerLayout, SCALE_LABELS, Sections, SessionLayout, StripLayout, TrackColumn, arrangement_room,
    crossfade_buttons, crossfader_slider, device_row, device_row_parts, in_out_row,
    performance_impact, send_knob, split_pans, track_options,
};
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::track_menu::{TrackMenus, rename_edit};
use crate::widgets::{modulated_text, paint_ring_segment, ring_segment};

/// The sections of one view's mixer as the shell has them (R37), with the
/// strip section as tall as that view's is (R32).
#[must_use]
pub fn sections(shell: &Shell, of: MixerOf) -> Sections {
    let view = shell.mixer_view();
    let mixer = view.of(of);
    Sections {
        in_out: mixer.in_out,
        sends: mixer.sends,
        volume: mixer.volume,
        strip: mixer.height,
        crossfader: mixer.crossfader,
        returns: mixer.return_tracks,
        track_options: mixer.track_options,
        performance_impact: mixer.performance_impact,
        devices: mixer.devices,
        return_tracks_scroll: view.return_tracks_scroll(),
    }
}

/// A view's mixer laid out in `area`: its columns at the widths the set
/// keeps for them (R36), its sections as that view shows them (R37), and
/// the view's own scroll.
pub(crate) fn mixer_layout(shell: &Shell, area: Rect, of: MixerOf, folds: &Folds) -> MixerLayout {
    let doc = shell.editor().document();
    let view = shell.mixer_view();
    let returns: Vec<f32> = shell
        .shown_returns()
        .into_iter()
        .map(|track| view.column_width(doc, track))
        .collect();
    MixerLayout::compute_with_widths(
        area,
        &folds.columns,
        &returns,
        view.column_width(doc, doc.main_id()),
        sections(shell, of),
        view.of(of).scroll as f32,
    )
}

/// The dB below 0 that the extended meters mark with a tick every 6 dB
/// (R33).
const TICKS: [i8; 11] = [0, 6, 12, 18, 24, 30, 36, 42, 48, 54, 60];

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

/// Which half of a split stereo pan a control places.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SplitSide {
    /// The left input channel.
    Left,
    /// The right input channel.
    Right,
}

/// The ID of a split stereo pan control.
#[must_use]
pub fn split_pan_id(place: Place, track: TrackId, side: SplitSide) -> Id {
    Id::new(("mixer-split-pan", place, track, side))
}

/// The ID of a track's Track Delay field.
#[must_use]
pub fn track_delay_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-track-delay", place, track))
}

/// The ID of a track's Track Delay unit switch.
#[must_use]
pub fn delay_unit_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-delay-unit", place, track))
}

/// The ID of a track's Keep Monitoring Latency switch.
#[must_use]
pub fn keep_monitoring_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-keep-monitoring", place, track))
}

/// The ID of a meter.
#[must_use]
pub fn meter_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-meter", place, track))
}

/// The ID of a strip's gain-reduction bar (R40).
#[must_use]
pub fn gain_reduction_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-gain-reduction", place, track))
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

/// Where egui's memory notes that a value drag holds a gesture open.
fn open_drag_id() -> Id {
    Id::new("mixer-open-value-drag")
}

/// Ends a value drag whose control was scrolled out of view, and so never
/// drawn again, before the button came up: its own `drag_stopped` is never
/// seen. Called at the end of every frame.
pub(crate) fn end_orphaned_drag(ctx: &egui::Context, events: &mut Vec<UiEvent>) {
    if ctx.input(|i| !i.pointer.any_down())
        && ctx
            .data_mut(|d| d.remove_temp::<bool>(open_drag_id()))
            .is_some()
    {
        events.push(UiEvent::EndGesture);
    }
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
    /// The menu a title bar opens (WP-4.22).
    track_menus: TrackMenus,
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

    /// The Session View's slot the pointer was over in the last frame, for
    /// clips dragged from the Arrangement to drop on (second-window R21).
    #[must_use]
    pub fn drop_spot(&self) -> Option<bl_shell::clip_drag::DropSpot> {
        self.grid.drop_spot()
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
        let sections = sections(shell, MixerOf::Session);
        let shown = shell.mixer_view().of(MixerOf::Session).shown;
        let layout = SessionLayout::compute(rect, shown.then_some(sections));
        // Spec: command-palette R11. With the Session mixer hidden, the
        // grid's columns scroll to a track the search field selected.
        if !shown && let Some((MixerOf::Session, track)) = self.reveal {
            self.reveal = None;
            let columns = shell.mixer_columns();
            let folds = folds(shell, &columns);
            let columns_layout = mixer_layout(shell, layout.slots, MixerOf::Session, &folds);
            if let Some(index) = columns.iter().position(|c| *c == track) {
                scroll_into_view(
                    &columns_layout,
                    &folds.columns,
                    index,
                    MixerOf::Session,
                    events,
                );
            }
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.colors.panel.0);
        title_bars(
            ui,
            &self.track_menus,
            shell,
            theme,
            layout.title_bars,
            MixerOf::Session,
            events,
        );
        // The clip slots and scenes (M3, WP-3.7).
        self.grid.show(ui, shell, theme, &layout, events);
        painter.line_segment(
            [layout.status_row.left_top(), layout.status_row.right_top()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        if let Some(mixer) = layout.mixer {
            self.show_mixer(ui, shell, theme, mixer, rect, MixerOf::Session, events);
        }
    }

    /// Draws one view's mixer in `area`, in the main view `view`, and adds
    /// what the person did to `events` (R1 to R3, R5, R10, R13, R14, R32).
    /// The Arrangement's has a row of title bars along its top.
    #[expect(clippy::too_many_arguments, reason = "the mixer's inputs")]
    pub fn show_mixer(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        area: Rect,
        view: Rect,
        of: MixerOf,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let columns = shell.mixer_columns();
        let returns: Vec<TrackId> = shell.shown_returns();
        let folds = folds(shell, &columns);
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
            title_bars(ui, &self.track_menus, shell, theme, row, of, events);
            area.with_min_y(row.bottom())
        } else {
            area
        };
        let layout = mixer_layout(shell, sections_area, of, &folds);
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
            if let ColumnKind::Chain(chain) = column.kind {
                let Some(strip) = folds.strips.iter().find(|s| s.chain == chain.chain) else {
                    continue;
                };
                self.chain_column(&mut ui, shell, theme, column, strip, &layout, events);
                continue;
            }
            let track = column_track(doc, &columns, &returns, column);
            let Some(t) = doc.track(track) else {
                continue;
            };
            let clip = layout.clip_of(column.kind, column.rect);
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
                title_colour(theme, doc, track, t.kind()),
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
                    // A row past the section's seven is left out: Bypass
                    // Tuning has no row of its own in the mixer yet.
                    |i| {
                        let row = in_out_row(column.rect, span, i);
                        (row.bottom() <= span.1).then_some(row)
                    },
                    events,
                );
            }
            if let Some(span) = layout.devices {
                let parts = StripParts {
                    shell,
                    theme,
                    place,
                    track,
                    activator: &label,
                };
                parts.devices_section(&mut column_ui, column.rect, span, events);
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
            if let Some(span) = layout.track_options {
                let parts = StripParts {
                    shell,
                    theme,
                    place,
                    track,
                    activator: &label,
                };
                parts.track_options_section(&mut column_ui, column.rect, span, events);
            }
            if let Some(span) = layout.performance_impact {
                let parts = StripParts {
                    shell,
                    theme,
                    place,
                    track,
                    activator: &label,
                };
                parts.performance_impact_section(&mut column_ui, column.rect, span);
            }
        }
        self.scroll(&ui, sections_area, of, events);
        // Spec: view-options R1, mixer-view R23. A middle-button drag pans
        // the mixer's columns sideways, as the wheel scrolls them.
        if let Some(delta) = crate::middle::pan(&ui, sections_area) {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SCROLL,
                json!({ "view": of, "x": -delta.x }),
            )));
        }
        self.focus(shell, &layout, &folds.columns, &columns, of, events);
        // A chain dropped out of the mixer's tracks (drum racks spec, R23,
        // R24).
        chain_extract(&ui, shell, &layout, events);
        height_drag(&ui, shell, &layout, area, view, of, events);
    }

    /// A chain's strip in the mixer (racks spec, R27): the column, the
    /// chain's colour along its bottom, and the chain list's controls for
    /// the chain.
    #[expect(clippy::too_many_arguments, reason = "the strip's inputs")]
    fn chain_column(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        column: &Column,
        strip: &ChainStrip,
        layout: &MixerLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let group = crate::chain_strips::strip_id(strip.chain);
        let clip = column.rect.intersect(layout.tracks);
        // The column's group, in the mixer's own group.
        let _ = ui.interact(clip, group, Sense::hover());
        ui.ctx().accesskit_node_builder(group, |node| {
            node.set_role(Role::Group);
            node.set_label(format!("Chain Strip {} of {}", strip.name, strip.rack_name));
            node.set_bounds(bounds(clip));
        });
        let mut column_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(clip)
                .accessibility_parent(group),
        );
        column_ui.set_clip_rect(clip.intersect(layout.area));
        let painter = column_ui.painter();
        painter.rect_filled(
            column.rect.shrink2(vec2(0.5, 0.0)),
            0.0,
            theme.colors.lane.0,
        );
        painter.line_segment(
            [column.rect.right_top(), column.rect.right_bottom()],
            Stroke::new(1.0, theme.colors.lane_line.0),
        );
        // The chain's colour along the column's bottom, where a track's own
        // colour runs.
        if let Some(colour) = strip.colour {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(column.rect.left() + 0.5, layout.colour.0),
                    pos2(column.rect.right() - 0.5, layout.colour.1),
                ),
                0.0,
                theme.swatch(colour),
            );
        }
        if let Some(span) = layout.strip {
            let strip_layout = ChainStripLayout::compute(column.rect, span);
            crate::chain_strips::strip(&mut column_ui, shell, theme, &strip_layout, strip, events);
        }
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
        tracks: &[TrackColumn],
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
                scroll_into_view(layout, tracks, index, of, events);
            }
        }
        let Some(target) = self.focus_to else {
            return;
        };
        let Some(index) = columns.iter().position(|c| *c == target) else {
            self.focus_to = None;
            return;
        };
        scroll_into_view(layout, tracks, index, of, events);
    }
}

/// Scrolls a mixer so the track column at `index` is wholly in view, if it
/// isn't.
fn scroll_into_view(
    layout: &MixerLayout,
    tracks: &[TrackColumn],
    index: usize,
    of: MixerOf,
    events: &mut Vec<UiEvent>,
) {
    let in_view = layout
        .columns
        .iter()
        .any(|c| c.index == index && layout.tracks.contains_rect(c.rect.shrink(0.5)));
    if in_view {
        return;
    }
    let Some((left, right)) = layout.track_span(tracks, index) else {
        return;
    };
    let left = f64::from(left) - f64::from(layout.scroll);
    let x = if left < 0.0 {
        left
    } else {
        f64::from(right) - f64::from(layout.scroll) - f64::from(layout.tracks.width())
    };
    events.push(UiEvent::Invoke(Invocation::new(
        ids::SCROLL,
        json!({ "view": of, "x": x }),
    )));
}

/// A row of title bars over a mixer's columns: the Session View's top row
/// (L-SES-002) and the Arrangement mixer's top row, each track's colour and
/// name.
///
/// Right-clicking a title bar opens the track's menu (WP-4.22, C-TRACK); the
/// keys do the same while the track's column has the keyboard.
fn title_bars(
    ui: &mut Ui,
    menus: &TrackMenus,
    shell: &Shell,
    theme: &Theme,
    row: Rect,
    of: MixerOf,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    let columns = shell.mixer_columns();
    let returns: Vec<TrackId> = shell.shown_returns();
    let folds = folds(shell, &columns);
    let layout = mixer_layout(shell, row, of, &folds);
    for column in layout.every_column() {
        if let ColumnKind::Chain(chain) = column.kind {
            let Some(strip) = folds.strips.iter().find(|s| s.chain == chain.chain) else {
                continue;
            };
            let bar = column.rect.shrink2(vec2(0.5, 0.0)).intersect(layout.tracks);
            crate::chain_strips::title_bar(ui, shell, theme, bar, strip, events);
            continue;
        }
        let track = column_track(doc, &columns, &returns, column);
        let Some(t) = doc.track(track) else {
            continue;
        };
        let bar = layout.clip_of(column.kind, column.rect.shrink2(vec2(0.5, 0.0)));
        let painter = ui.painter_at(bar);
        painter.rect_filled(
            bar,
            0.0,
            title_colour(theme, doc, track, t.kind()).gamma_multiply(0.82),
        );
        // Spec: command-palette R10. The tracks the search field finds are
        // outlined.
        crate::search_field::outline(ui, shell, theme, track, bar);
        // The fold button for the track's rack, at the bar's right.
        let fold = folds.racks.iter().find(|r| r.track == track).map(|r| {
            (
                r.rack,
                r.folded,
                Rect::from_min_max(
                    pos2(bar.right() - 12.0, bar.top() + 2.0),
                    pos2(bar.right() - 2.0, bar.bottom() - 2.0),
                ),
            )
        });
        let name = Rect::from_min_max(
            pos2(bar.left() + 4.0, bar.top()),
            pos2(
                fold.map_or(bar.right(), |(_, _, rect)| rect.left()) - 2.0,
                bar.bottom(),
            ),
        );
        let next = crate::track_menu::next_renamable(shell.editor().document(), track);
        if !rename_edit(ui, theme, name, track, t.name(), next, events) {
            painter.text(
                pos2(name.left(), name.center().y),
                Align2::LEFT_CENTER,
                t.name(),
                theme.small_font(),
                theme.colors.clip_text.0,
            );
        }
        let response = ui.interact(
            bar,
            Id::new(("mixer-title", of, track)),
            Sense::click_and_drag(),
        );
        // The fold button for the track's rack, at the bar's right, over the
        // title bar's own click.
        if let Some((rack, folded, rect)) = fold {
            crate::chain_strips::fold_button(ui, theme, rect, rack, folded, events);
        }
        if response.double_clicked() {
            // Spec: detail-views R4 (I-CLICK-006).
            crate::track_menu::open_device_view(shell, track, events);
        }
        crate::rack_view::focus_track_under_drag(ui, shell, track, bar, events);
        let focused = ui.memory(|m| m.has_focus(column_id(of, track)));
        menus.title(
            ui,
            theme,
            &response,
            track,
            (bar.left_bottom(), focused, false),
            shell,
            events,
        );
        crate::info_text::describe(ui.ctx(), &response, shell, InfoTarget::Track(track));
    }
    // The Session View's title bars are resized by their edges (R36).
    if of == MixerOf::Session {
        width_drags(ui, shell, &layout, &columns, &returns, events);
    }
    // A chain dropped out of the row leaves its rack (drum racks spec,
    // R23, R24) before a drop among the strips moves it (racks spec, R29).
    chain_extract(ui, shell, &layout, events);
    chain_drop(ui, &layout, &folds.strips, events);
}

/// Resizes a column by dragging its title bar's edge (R36, I-DRAG-004,
/// I-DRAG-005): a track's or a scrolling return's right edge, and a pinned
/// return's or Main's left, the edge that moves with the pointer. With
/// Option (Alt) held it resizes every track's column; dragging Main's edge
/// also shows the scene tempo and signature fields. Each step runs
/// `mixer_view.set_column_width`, which is no undo step.
fn width_drags(
    ui: &Ui,
    shell: &Shell,
    layout: &MixerLayout,
    columns: &[TrackId],
    returns: &[TrackId],
    events: &mut Vec<UiEvent>,
) {
    const GRAB: f32 = 3.0;
    let doc = shell.editor().document();
    let view = shell.mixer_view();
    let title = layout.area;
    for column in layout.every_column() {
        let (track, left_edge) = match column.kind {
            ColumnKind::Track => (columns[column.index], false),
            ColumnKind::Return => (returns[column.index], !layout.returns_scroll),
            ColumnKind::Main => (doc.main_id(), true),
            ColumnKind::Chain(_) => continue,
        };
        let x = if left_edge {
            column.rect.left()
        } else {
            column.rect.right()
        };
        let visible = match column.kind {
            ColumnKind::Track => layout.tracks,
            ColumnKind::Return if layout.returns_scroll => layout.tracks,
            _ => title,
        };
        if x < visible.left() || x > visible.right() {
            continue;
        }
        let handle =
            Rect::from_min_max(pos2(x - GRAB, title.top()), pos2(x + GRAB, title.bottom()));
        let id = Id::new(("mixer-width", track));
        let response = ui.interact(handle, id, Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let drag_id = id.with("drag");
        if response.drag_started() {
            let start = view.column_width(doc, track);
            ui.data_mut(|d| d.insert_temp(drag_id, (start, 0.0f32)));
        }
        if response.dragged() {
            let dx = response.drag_delta().x;
            let Some((start, moved)) = ui.data_mut(|d| {
                let drag = d.get_temp_mut_or_default::<(f32, f32)>(drag_id);
                drag.1 += if left_edge { -dx } else { dx };
                Some(*drag)
            }) else {
                continue;
            };
            let every = ui.input(|i| i.modifiers.alt);
            let args = if every {
                json!({ "width": start + moved })
            } else {
                json!({ "track": track, "width": start + moved })
            };
            if dx != 0.0 {
                // Dragging Main's edge reveals the scene tempo and
                // signature fields (I-DRAG-005).
                if column.kind == ColumnKind::Main && !view.scene_tempo() {
                    events.push(UiEvent::Invoke(Invocation::new(
                        ids::SCENE_TEMPO,
                        json!({ "shown": true }),
                    )));
                }
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::SET_COLUMN_WIDTH,
                    args,
                )));
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<(f32, f32)>(drag_id));
        }
    }
}

/// Resizes the strip section by dragging the mixer's top edge or the strip
/// section's top border (R32, I-DRAG-030): up makes it taller, down
/// shorter, between 64 points and what leaves the view the rest of its
/// rows. Each step runs `mixer_view.set_height` for this view.
fn height_drag(
    ui: &Ui,
    shell: &Shell,
    layout: &MixerLayout,
    area: Rect,
    view: Rect,
    of: MixerOf,
    events: &mut Vec<UiEvent>,
) {
    const GRAB: f32 = 3.0;
    let Some(strip) = layout.strip else {
        return;
    };
    let sections = sections(shell, of);
    let room = match of {
        MixerOf::Session => SessionLayout::room(view),
        MixerOf::Arrangement => arrangement_room(view),
    };
    let most = Sections {
        strip: f32::MAX,
        ..sections
    }
    .fitted(room)
    .strip;
    for (edge, y) in [("top", area.top() + 1.0), ("strip", strip.0)] {
        let handle = Rect::from_min_max(
            pos2(area.left(), y - GRAB),
            pos2(layout.tracks.right().max(area.left() + 1.0), y + GRAB / 2.0),
        );
        let id = Id::new(("mixer-height", of, edge));
        let response = ui.interact(handle, id, Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let drag_id = id.with("drag");
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(drag_id, (strip.1 - strip.0, 0.0f32)));
        }
        if response.dragged() {
            let dy = response.drag_delta().y;
            let (start, moved) = ui.data_mut(|d| {
                let drag = d.get_temp_mut_or_default::<(f32, f32)>(drag_id);
                drag.1 += dy;
                *drag
            });
            if dy != 0.0 {
                let height = (start - moved).clamp(MIN_STRIP_HEIGHT, most.max(MIN_STRIP_HEIGHT));
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::SET_HEIGHT,
                    json!({ "view": of, "height": height }),
                )));
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<(f32, f32)>(drag_id));
        }
    }
}

/// Moves a chain dragged from its strip among its rack's chains (racks
/// spec, R29), where it lands among the rack's strips.
fn chain_drop(ui: &Ui, layout: &MixerLayout, strips: &[ChainStrip], events: &mut Vec<UiEvent>) {
    let Some(drag) = egui::DragAndDrop::payload::<crate::rack_view::ChainDrag>(ui.ctx()) else {
        return;
    };
    let Some(chain) = drag.chains.first().copied() else {
        return;
    };
    let Some(strip) = strips.iter().find(|s| s.chain == chain) else {
        return;
    };
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    // The chain lands where it is dropped, in the row of title bars that
    // holds the strips.
    if !layout.area.contains(pointer) {
        return;
    }
    // The rack's strips, in order, and where each is.
    let siblings: Vec<(ChainId, f32)> = strips
        .iter()
        .filter(|s| s.rack == strip.rack)
        .filter_map(|s| {
            let column = layout
                .columns
                .iter()
                .find(|c| matches!(c.kind, ColumnKind::Chain(k) if k.chain == s.chain))?;
            Some((s.chain, column.rect.center().x))
        })
        .collect();
    // Where the chain would land: the rack's strips left of the pointer,
    // the dragged one out of the way.
    let at = siblings
        .iter()
        .filter(|(other, x)| *other != chain && *x < pointer.x)
        .count();
    if ui.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ui.ctx());
        events.push(UiEvent::BeginNamedGesture("Move Chain".to_owned()));
        events.push(UiEvent::Invoke(Invocation::new(
            "chain.move",
            json!({ "chain": chain, "rack": strip.rack, "index": at }),
        )));
        events.push(UiEvent::EndGesture);
    }
}

/// Extracts a chain dragged out of the mixer (drum racks spec, R23, R24): a
/// drum chain dropped on the drop area beside the tracks leaves its rack
/// for a new MIDI track, and a return chain dropped on the returns leaves
/// it for a new return track. Either is one undo step, named for the drag.
fn chain_extract(ui: &Ui, shell: &Shell, layout: &MixerLayout, events: &mut Vec<UiEvent>) {
    let Some(drag) = egui::DragAndDrop::payload::<crate::rack_view::ChainDrag>(ui.ctx()) else {
        return;
    };
    let Some(chain) = drag.chains.first().copied() else {
        return;
    };
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    let doc = shell.editor().document();
    let Some(row) = doc.chain(chain) else {
        return;
    };
    let drum = doc.chain_rack(chain).and_then(|r| r.rack_kind()) == Some(RackKind::Drum);
    let command =
        if drum && !row.is_return && layout.drop_area.is_some_and(|drop| drop.contains(pointer)) {
            "chain.extract"
        } else if row.is_return && layout.returns.iter().any(|c| c.rect.contains(pointer)) {
            "chain.extract_return"
        } else {
            return;
        };
    if !ui.input(|i| i.pointer.any_released()) {
        return;
    }
    egui::DragAndDrop::clear_payload(ui.ctx());
    let name = if command == "chain.extract" {
        "Extract Chain"
    } else {
        "Extract Return Chain"
    };
    events.push(UiEvent::BeginNamedGesture(name.to_owned()));
    events.push(UiEvent::Invoke(Invocation::new(
        command,
        json!({ "chain": chain }),
    )));
    events.push(UiEvent::EndGesture);
}

/// The mixer's columns and the racks and chains folded out in them (racks
/// spec, R27): each track's column, then the chain strips folded out of
/// its rack.
pub(crate) struct Folds {
    /// The columns, in order.
    pub(crate) columns: Vec<TrackColumn>,
    /// The racks whose chains fold out.
    pub(crate) racks: Vec<RackFold>,
    /// The chains folded out as strips.
    pub(crate) strips: Vec<ChainStrip>,
}

/// The mixer's columns and the racks and chains folded out in them.
pub(crate) fn folds(shell: &Shell, tracks: &[TrackId]) -> Folds {
    let doc = shell.editor().document();
    let (racks, strips) = shell.mixer_view().folds(doc, shell.arrangement());
    let mut columns: Vec<TrackColumn> = Vec::with_capacity(tracks.len() + strips.len());
    for track in tracks {
        columns.push(TrackColumn::track().wide(shell.mixer_view().column_width(doc, *track)));
        for strip in strips.iter().filter(|s| s.track == *track) {
            columns.push(TrackColumn::chain(ChainColumn {
                chain: strip.chain,
                rack: strip.rack,
            }));
        }
    }
    Folds {
        columns,
        racks,
        strips,
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
        ColumnKind::Chain(_) => columns[column.index],
        ColumnKind::Return => returns[column.index],
        ColumnKind::Main => doc.main_id(),
    }
}

/// What a column's activator shows: its number, or a return's letter.
fn activator_label(column: &Column) -> String {
    match column.kind {
        ColumnKind::Track => (column.index + 1).to_string(),
        ColumnKind::Chain(_) => String::new(),
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

/// Whether a track's strip is a MIDI track's without an instrument or audio
/// effect: a dotted MIDI meter and the switches, without volume or pan
/// (observed; the MIDI meters spec, R1).
fn midi_only(doc: &Document, track: TrackId) -> bool {
    doc.track(track).is_some_and(|t| t.shows_midi_meter())
}

/// Headphones in `rect`: a band over two ear cups.
fn headphones(painter: &egui::Painter, rect: Rect, ink: Color32) {
    let (centre, size) = (rect.center(), rect.height().min(rect.width()) * 0.7);
    let r = size * 0.4;
    let band: Vec<egui::Pos2> = (0..=12)
        .map(|i| {
            let angle = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
            pos2(
                centre.x + r * angle.cos(),
                centre.y + size * 0.1 + r * angle.sin(),
            )
        })
        .collect();
    painter.add(Shape::line(band, Stroke::new(1.5, ink)));
    for side in [-1.0, 1.0] {
        let cup = Rect::from_center_size(
            pos2(centre.x + side * r, centre.y + size * 0.15),
            egui::vec2(size * 0.22, size * 0.36),
        );
        painter.rect_filled(cup, 1.0, ink);
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
                ColumnKind::Chain(_) => {}
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
        self.send_knob(ui, rect, to, index, events);
        // Spec: modulation R15. A lit Map button maps the send's level.
        crate::mapping::overlay(
            ui,
            self.theme,
            rect,
            send_id(self.place, self.track, to),
            ModTarget::Send {
                track: self.track,
                to,
            },
            events,
        );
        // Spec: automation R29. The control's automation items.
        crate::param_menu::ParamMenu::new(self.shell, self.track, AutomationTarget::Send { to })
            .overlay(ui, rect, send_id(self.place, self.track, to), events);
    }

    fn send_knob(
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
        let modulated = self.played(control);
        let base = modulated.map_or(level, |(base, _)| base);
        let response = ui.interact(
            rect,
            send_id(self.place, self.track, to),
            Sense::click_and_drag(),
        );
        self.register(ui, response.id, crate::focus::Slot::Send(index));
        self.escape_to_title(ui, &response);
        self.value_drag(ui, &response, control, control.drag_travel(), events);
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
            let position = controls::send_position(base) as f32;
            // Spec: modulation R7: the pointer stays at the base.
            if let Some(segment) = modulated.and_then(|(_, played)| {
                ring_segment(position, controls::send_position(played) as f32)
            }) {
                paint_ring_segment(&painter, self.theme, segment, |at| {
                    ring_point(centre, radius, at)
                });
            }
            let tip = ring_point(centre, radius, position);
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
            modulated_text(
                format!("{} dB", controls::send_text(base)),
                modulated.map(|(_, played)| format!("{} dB", controls::send_text(played))),
            ),
            base,
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
            self.meter(ui, strip.meter);
        } else {
            // A column narrower than 94 points, or a strip shorter than
            // 100, leaves out the readout and the pan knob (R32, R36).
            if strip.peak_shown {
                self.peak_field(ui, strip, events);
            }
            if strip.pan_shown {
                self.pan(ui, strip.pan, Some(strip.pan_triangle), events);
                // In Split Stereo Pan Mode two controls sit beside the pan
                // knob, one placing each input channel (the mixer spec,
                // R29).
                if self.split_mode() {
                    let [left, right] = split_pans(strip.pan);
                    self.split_pan(ui, left, SplitSide::Left, events);
                    self.split_pan(ui, right, SplitSide::Right, events);
                }
            }
            self.fader(ui, strip.fader, events);
            self.meter(ui, strip.meter);
            self.gain_reduction(ui, strip.gain_reduction);
            if strip.extended {
                self.ticks(ui, strip);
                // The volume field under the fader (R33).
                if let Some(field) = strip.volume_field {
                    self.volume_field(ui, field, events);
                    let id = volume_field_id(self.place, self.track);
                    ui.ctx()
                        .accesskit_node_builder(id, |node| node.set_role(Role::SpinButton));
                }
            }
            if strip.labels_shown {
                self.scale(ui, strip);
            }
        }
        match kind {
            ColumnKind::Main => {
                // Main has no activator or Solo: its cue volume and Solo/Cue
                // button take their places in the Tab order.
                self.cue_volume(ui, strip.activator, events);
                self.cue_mode_button(ui, strip.solo, events);
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

    /// Whether the track's pan mode is Split Stereo Pan Mode.
    fn split_mode(&self) -> bool {
        self.doc()
            .track(self.track)
            .is_some_and(|t| t.mixer().pan_mode == PanMode::Split)
    }

    /// Whether Delay Compensation is on, so a Track Options control works
    /// (the mixer spec, R28; delay compensation R7).
    fn delay_compensation(&self) -> bool {
        self.doc().settings().delay_compensation
    }

    /// The Track Options section (L-MIX-005; the mixer spec, R28, R30): a
    /// track's Track Delay with its ms/samples unit switch, and its Keep
    /// Monitoring Latency switch. Both are disabled while Delay
    /// Compensation is off.
    pub fn track_options_section(
        &self,
        ui: &mut Ui,
        column: Rect,
        span: (f32, f32),
        events: &mut Vec<UiEvent>,
    ) {
        let [delay, unit, keep] = track_options(column, span);
        self.track_delay(ui, delay, events);
        self.delay_unit(ui, unit, events);
        self.keep_monitoring(ui, keep, events);
    }

    /// The Performance Impact section (L-MIX-007): the six segments under
    /// the strip, lit from the left with the track's share of the set's
    /// processing time (the mixer spec, R31).
    pub fn performance_impact_section(&self, ui: &mut Ui, column: Rect, span: (f32, f32)) {
        let lit = self.impact_segments();
        let segments = performance_impact(column, span);
        let painter = ui.painter();
        for (i, rect) in segments.iter().enumerate() {
            let colour = if i < lit {
                self.theme.colors.accent.0
            } else {
                self.theme.colors.inactive.0
            };
            painter.rect_filled(*rect, self.theme.sizes.corner, colour);
        }
        let id = Id::new(("mixer-impact", self.place, self.track));
        let rect = Rect::from_min_max(pos2(column.left(), span.0), pos2(column.right(), span.1));
        let response = ui.interact(rect, id, Sense::hover());
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::ProgressIndicator, true, "Performance Impact")
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Group);
            node.set_label("Performance Impact".to_owned());
            node.set_value(format!("{lit} of 6 segments"));
            node.set_bounds(bounds(rect));
        });
    }

    /// The Devices section (IMP-25, the mixer view spec R26 to R30): the
    /// track's top-level devices in chain order, a rack as one row. A row
    /// holds the device's activator, a square lit while it is on, and its
    /// name; a click on the name selects the device and shows the Device
    /// View, a double-click on a plug-in's name opens or closes its editor,
    /// and a click on the activator switches the device. The rows scroll
    /// with the wheel over the section, on their own.
    fn devices_section(
        &self,
        ui: &mut Ui,
        column: Rect,
        span: (f32, f32),
        events: &mut Vec<UiEvent>,
    ) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let area = Rect::from_min_max(pos2(column.left(), span.0), pos2(column.right(), span.1));
        let list = Id::new(("mixer-devices", self.place, self.track));
        let scroll_id = list.with("scroll");
        let content = crate::layout::mixer::DEVICE_ROW * track.devices().len() as f32;
        let most = (content - area.height()).max(0.0);
        let mut scroll = ui
            .data(|d| d.get_temp::<f32>(scroll_id))
            .unwrap_or(0.0)
            .clamp(0.0, most);
        // Spec: mixer-view R27. The wheel over a section that holds more
        // than it shows scrolls its rows, and not the columns.
        let over = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pos| area.intersect(ui.clip_rect()).contains(pos));
        if over && most > 0.0 {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy != 0.0 {
                scroll = (scroll - dy).clamp(0.0, most);
                ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            }
        }
        ui.data_mut(|d| d.insert_temp(scroll_id, scroll));
        let _ = ui.interact(area, list, Sense::hover());
        // The list's node comes first, so the rows' container hangs from it.
        ui.ctx().accesskit_node_builder(list, |node| {
            node.set_role(Role::List);
            node.set_label("Devices".to_owned());
            node.set_bounds(bounds(area));
        });
        let ui = &mut ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .accessibility_parent(list),
        );
        ui.set_clip_rect(area.intersect(ui.clip_rect()));
        let c = &self.theme.colors;
        for (index, device) in track.devices().enumerate() {
            let row = device_row(column, span, index, scroll);
            if row.bottom() <= area.top() || row.top() >= area.bottom() {
                continue;
            }
            let (square, name) = device_row_parts(row);
            let shown = row.intersect(area);
            let activator = Id::new(("mixer-device-activator", self.place, device.id));
            let button = Id::new(("mixer-device", self.place, device.id));
            let response = ui.interact(square.intersect(area), activator, Sense::click());
            if response.clicked() {
                // Spec: mixer-view R29. Only this device, whatever tracks
                // are selected; one undo step.
                events.push(UiEvent::Invoke(Invocation::new(
                    "device.set_enabled",
                    json!({ "device": device.id, "enabled": !device.enabled }),
                )));
            }
            let painter = ui.painter_at(shown);
            let lit = if device.enabled {
                c.accent.0
            } else {
                c.window.0
            };
            painter.rect_filled(square, self.theme.sizes.corner, lit);
            painter.rect_stroke(
                square,
                self.theme.sizes.corner,
                Stroke::new(1.0, c.separator.0),
                StrokeKind::Inside,
            );
            focus_ring(&painter, self.theme, square, &response);
            let state = if device.enabled { "on" } else { "off" };
            ui.ctx().accesskit_node_builder(activator, |node| {
                node.set_role(Role::Switch);
                node.set_label("Device Activator".to_owned());
                node.set_value(format!("{}, {state}", device.title()));
                node.set_toggled(if device.enabled {
                    accesskit::Toggled::True
                } else {
                    accesskit::Toggled::False
                });
                node.set_bounds(bounds(square));
            });
            let response = ui.interact(name.intersect(area), button, Sense::click());
            if response.double_clicked() && self.shell.plugin_info(device.id).is_some() {
                // Spec: mixer-view R28. As the device's window button does.
                events.push(UiEvent::Invoke(Invocation::new(
                    bl_shell::plugin_commands::SHOW_WINDOW,
                    json!({ "device": device.id }),
                )));
            } else if response.clicked() {
                // Spec: mixer-view R28. Selecting the device shows its
                // track's chain; the Device View opens if it was hidden,
                // and a floating device's window comes to the front.
                events.push(UiEvent::Invoke(Invocation::new(
                    bl_shell::device_commands::SELECT,
                    json!({ "device": device.id }),
                )));
                events.push(UiEvent::Invoke(Invocation::new(
                    bl_shell::commands::FOCUS_DEVICE_VIEW,
                    json!({}),
                )));
            }
            if response.hovered() {
                painter.rect_filled(name, 0.0, c.control_hover.0.gamma_multiply(0.4));
            }
            let mut job = egui::text::LayoutJob::simple_singleline(
                device.title().to_owned(),
                self.theme.small_font(),
                if device.enabled {
                    c.text.0
                } else {
                    c.text_dim.0
                },
            );
            job.wrap = egui::text::TextWrapping {
                max_width: name.width() - 2.0,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            let galley = painter.layout_job(job);
            painter.galley(
                pos2(name.left(), name.center().y - galley.size().y / 2.0),
                galley,
                c.text.0,
            );
            focus_ring(&painter, self.theme, name, &response);
            ui.ctx().accesskit_node_builder(button, |node| {
                node.set_role(Role::Button);
                node.set_label(device.title().to_owned());
                node.set_bounds(bounds(name));
            });
        }
    }

    /// How many of the six Performance Impact segments the track lights:
    /// its share of the set's processing time, a sixth at a time (the mixer
    /// spec, R31).
    fn impact_segments(&self) -> usize {
        self.shell
            .meter(self.track)
            .map(|meter| bl_shell::mixer_view::impact_segments(meter.impact))
            .unwrap_or(0) as usize
    }

    /// The Track Delay field (R28): the delay in its current unit, dragged,
    /// stepped, reset, or typed (`-12` and Return sets the delay); the Status
    /// Bar names it while it's dragged.
    fn track_delay(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let delay = track.delay();
        let enabled = self.delay_compensation();
        let id = track_delay_id(self.place, self.track);
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            },
        );
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(1.0));
        if !enabled {
            painter.rect_filled(
                rect,
                self.theme.sizes.corner,
                c.control.0.gamma_multiply(0.6),
            );
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                delay_text(delay),
                self.theme.small_font(),
                c.pending.0,
            );
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, false, "Track Delay"));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Slider);
                node.set_label("Track Delay".to_owned());
                node.set_value(delay_spoken(delay));
                node.set_disabled();
                node.set_description("Unavailable while Delay Compensation is off".to_owned());
            });
            return;
        }
        self.register(ui, id, crate::focus::Slot::TrackDelay);
        self.escape_to_title(ui, &response);
        let drag_id = id.with("drag");
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(drag_id, (delay, 0.0f64)));
            events.push(UiEvent::BeginGesture(TRACK_DELAY_GESTURE));
        }
        if response.dragged() {
            let dy = response.drag_delta().y;
            let fine = ui.input(|i| i.modifiers.shift);
            let (start, moved) = ui.data_mut(|d| {
                let drag = d.get_temp_mut_or_default::<(TrackDelay, f64)>(drag_id);
                if dy != 0.0 {
                    drag.1 += f64::from(-dy / if fine { FINE } else { 1.0 });
                }
                *drag
            });
            if dy != 0.0 {
                let now = delay_moved(start, moved);
                if now != delay {
                    ui.data_mut(|d| d.insert_temp(readout_id(), delay_readout(now)));
                    events.push(UiEvent::Invoke(Invocation::new(
                        "mixer.set_track_delay",
                        json!({ "track": self.track, "delay": now }),
                    )));
                }
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<(TrackDelay, f64)>(drag_id));
            events.push(UiEvent::EndGesture);
        }
        let typed = self.delay_typing(ui, &response, delay, events);
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
                        escape: true,
                        ..Default::default()
                    },
                );
            });
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
        let change = if reset {
            Some(TrackDelay::default())
        } else if steps != 0 {
            Some(delay_stepped(delay, steps, fine))
        } else {
            None
        };
        if let Some(now) = change
            && now != delay
        {
            events.push(UiEvent::BeginGesture(TRACK_DELAY_GESTURE));
            events.push(UiEvent::Invoke(Invocation::new(
                "mixer.set_track_delay",
                json!({ "track": self.track, "delay": now }),
            )));
            events.push(UiEvent::EndGesture);
        }
        let face = if typed.is_some() {
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
            typed.unwrap_or_else(|| delay_text(delay)),
            self.theme.small_font(),
            c.text.0,
        );
        focus_ring(&painter, self.theme, rect, &response);
        self.describe_value(
            ui,
            &response,
            "Track Delay",
            delay_spoken(delay),
            delay_number(delay),
            delay_range(delay),
        );
    }

    /// Typing into the focused Track Delay field, in its current unit.
    fn delay_typing(
        &self,
        ui: &Ui,
        response: &Response,
        delay: TrackDelay,
        events: &mut Vec<UiEvent>,
    ) -> Option<String> {
        let id = response.id;
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
                    let now = match delay {
                        TrackDelay::Milliseconds { .. } => text
                            .parse::<f64>()
                            .ok()
                            .filter(|v| v.abs() <= MAX_TRACK_DELAY_MS)
                            .map(|value| TrackDelay::Milliseconds { value }),
                        TrackDelay::Samples { .. } => text
                            .parse::<i64>()
                            .ok()
                            .filter(|v| v.abs() <= MAX_TRACK_DELAY_SAMPLES)
                            .map(|value| TrackDelay::Samples { value }),
                    };
                    if let Some(now) = now {
                        events.push(UiEvent::BeginGesture(TRACK_DELAY_GESTURE));
                        events.push(UiEvent::Invoke(Invocation::new(
                            "mixer.set_track_delay",
                            json!({ "track": self.track, "delay": now }),
                        )));
                        events.push(UiEvent::EndGesture);
                    }
                }
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
                    if text.len() < 9 {
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

    /// The Track Delay unit switch (R28): a click converts the value to the
    /// other unit at the current sample rate, as Hardware Latency's does.
    fn delay_unit(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let delay = track.delay();
        let enabled = self.delay_compensation();
        let id = delay_unit_id(self.place, self.track);
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let text = delay_unit_text(delay);
        let c = &self.theme.colors;
        let face = if !enabled {
            c.control.0.gamma_multiply(0.6)
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        };
        let painter = ui.painter_at(rect.expand(1.0));
        painter.rect_filled(rect, self.theme.sizes.corner, face);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            self.theme.small_font(),
            if enabled { c.text.0 } else { c.pending.0 },
        );
        focus_ring(&painter, self.theme, rect, &response);
        if enabled {
            self.register(ui, id, crate::focus::Slot::TrackDelayUnit);
            self.escape_to_title(ui, &response);
            let pressed = response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
            if pressed {
                let rate = self.shell.audio().sample_rate().unwrap_or(48_000);
                events.push(UiEvent::Invoke(Invocation::new(
                    "mixer.set_track_delay",
                    json!({ "track": self.track, "delay": delay.switched(rate) }),
                )));
            }
        }
        response
            .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, "Track Delay Unit"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label("Track Delay Unit".to_owned());
            node.set_value(text.to_owned());
            node.set_description(
                if enabled {
                    "Switches the Track Delay between milliseconds and samples"
                } else {
                    "Unavailable while Delay Compensation is off"
                }
                .to_owned(),
            );
            if !enabled {
                node.set_disabled();
            }
        });
    }

    /// A track's Keep Monitoring Latency switch (R30): a click sets the
    /// other state.
    fn keep_monitoring(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let on = track.keep_monitoring_latency();
        let enabled = self.delay_compensation();
        let id = keep_monitoring_id(self.place, self.track);
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let c = &self.theme.colors;
        let lit = c.accent.0;
        let face = if !enabled {
            c.control.0.gamma_multiply(0.6)
        } else if on {
            lit
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        };
        let painter = ui.painter_at(rect.expand(1.0));
        painter.rect_filled(rect, self.theme.sizes.corner, face);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "K",
            self.theme.small_font(),
            if on && enabled {
                c.accent_text.0
            } else {
                c.text.0
            },
        );
        focus_ring(&painter, self.theme, rect, &response);
        if enabled {
            self.register(ui, id, crate::focus::Slot::KeepMonitoring);
            self.escape_to_title(ui, &response);
            let pressed = response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
            if pressed {
                events.push(UiEvent::Invoke(Invocation::new(
                    "mixer.set_keep_monitoring_latency",
                    json!({ "track": self.track, "enabled": !on }),
                )));
            }
        }
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, enabled, on, "Keep Monitoring Latency")
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label("Keep Monitoring Latency".to_owned());
            node.set_value(if on { "on" } else { "off" }.to_owned());
            node.set_description(
                if enabled {
                    "Keeps a monitored track's delay compensation while Reduced Latency When Monitoring is on"
                } else {
                    "Unavailable while Delay Compensation is off"
                }
                .to_owned(),
            );
            if !enabled {
                node.set_disabled();
            }
        });
    }

    /// One of a strip's two split stereo pan controls (R29): a vertical
    /// slider, dragged relative to the pointer, reset to centre on a
    /// double-click.
    pub fn split_pan(&self, ui: &mut Ui, rect: Rect, side: SplitSide, events: &mut Vec<UiEvent>) {
        self.split_pan_control(ui, rect, side, events);
        let param = match side {
            SplitSide::Left => MixerParam::SplitLeft,
            SplitSide::Right => MixerParam::SplitRight,
        };
        // Spec: automation R29. The control's automation items.
        crate::param_menu::ParamMenu::new(
            self.shell,
            self.track,
            AutomationTarget::Mixer { param },
        )
        .overlay(ui, rect, split_pan_id(self.place, self.track, side), events);
    }

    fn split_pan_control(
        &self,
        ui: &mut Ui,
        rect: Rect,
        side: SplitSide,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let pan = match side {
            SplitSide::Left => track.mixer().split_left,
            SplitSide::Right => track.mixer().split_right,
        };
        let id = split_pan_id(self.place, self.track, side);
        let response = ui.interact(rect, id, Sense::click_and_drag());
        self.register(
            ui,
            id,
            match side {
                SplitSide::Left => crate::focus::Slot::SplitPanLeft,
                SplitSide::Right => crate::focus::Slot::SplitPanRight,
            },
        );
        self.escape_to_title(ui, &response);
        let drag_id = id.with("drag");
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(drag_id, (pan, 0.0f64)));
            events.push(UiEvent::BeginGesture(split_gesture(side)));
        }
        if response.dragged() {
            let dy = response.drag_delta().y;
            let (start, moved) = ui.data_mut(|d| {
                let drag = d.get_temp_mut_or_default::<(f64, f64)>(drag_id);
                if dy != 0.0 {
                    drag.1 += f64::from(-dy) * SPLIT_PAN_PER_POINT;
                }
                *drag
            });
            if dy != 0.0 {
                let now = (start + moved).clamp(-1.0, 1.0);
                if now != pan {
                    events.push(UiEvent::Invoke(self.set_split_pan(side, now)));
                }
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<(f64, f64)>(drag_id));
            events.push(UiEvent::EndGesture);
        }
        if response.double_clicked() && pan != 0.0 {
            events.push(UiEvent::BeginGesture(split_gesture(side)));
            events.push(UiEvent::Invoke(self.set_split_pan(side, 0.0)));
            events.push(UiEvent::EndGesture);
        }
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let x = rect.center().x;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.5, c.separator.0),
        );
        let y = rect.bottom() - rect.height() * ((pan as f32 + 1.0) / 2.0);
        let handle = Rect::from_center_size(pos2(x, y), vec2(rect.width().max(3.0), 3.0));
        let face = if response.dragged() {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.text_dim.0
        };
        painter.rect_filled(handle, 1.0, face);
        focus_ring(&painter, self.theme, rect, &response);
        let name = match side {
            SplitSide::Left => "Split Pan Left",
            SplitSide::Right => "Split Pan Right",
        };
        self.describe_value(ui, &response, name, pan_text(pan), pan, (-1.0, 1.0));
    }

    /// The command that sets one split stereo pan side.
    fn set_split_pan(&self, side: SplitSide, pan: f64) -> Invocation {
        Invocation::new(
            match side {
                SplitSide::Left => "mixer.set_split_pan_left",
                SplitSide::Right => "mixer.set_split_pan_right",
            },
            json!({ "track": self.track, "pan": pan }),
        )
    }

    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    /// A control's base and the value it plays while a clip modulates it.
    fn played(&self, control: Control) -> Option<(f64, f64)> {
        let target = match control {
            Control::Volume => AutomationTarget::Mixer {
                param: MixerParam::Volume,
            },
            Control::Pan => AutomationTarget::Mixer {
                param: MixerParam::Pan,
            },
            Control::Send(to) => AutomationTarget::Send { to },
        };
        self.shell.played(self.track, &target)
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
            ui.data_mut(|d| d.insert_temp(open_drag_id(), true));
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
                    drag.moved += f64::from(-dy / travel / if fine { FINE } else { 1.0 });
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
            ui.data_mut(|d| {
                d.remove::<ValueDrag>(id);
                d.remove::<bool>(open_drag_id());
            });
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
        self.volume_field_control(ui, rect, events);
        // Spec: automation R29. The control's automation items.
        crate::param_menu::ParamMenu::new(
            self.shell,
            self.track,
            AutomationTarget::Mixer {
                param: MixerParam::Volume,
            },
        )
        .overlay(ui, rect, volume_field_id(self.place, self.track), events);
    }

    fn volume_field_control(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let modulated = self.played(Control::Volume);
        let volume = modulated.map_or(track.mixer().volume, |(base, _)| base);
        let id = volume_field_id(self.place, self.track);
        let response = ui.interact(rect, id, Sense::click_and_drag());
        self.register(ui, id, crate::focus::Slot::Volume);
        self.value_drag(
            ui,
            &response,
            Control::Volume,
            Control::Volume.drag_travel(),
            events,
        );
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
            modulated_text(
                volume_spoken(volume),
                modulated.map(|(_, played)| volume_spoken(played)),
            ),
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
        self.fader_control(ui, rect, events);
        // Spec: modulation R15. A lit Map button maps the track's volume.
        crate::mapping::overlay(
            ui,
            self.theme,
            rect,
            fader_id(self.place, self.track),
            ModTarget::Mixer {
                track: self.track,
                param: MixerParam::Volume,
            },
            events,
        );
        // Spec: automation R29. The control's automation items.
        crate::param_menu::ParamMenu::new(
            self.shell,
            self.track,
            AutomationTarget::Mixer {
                param: MixerParam::Volume,
            },
        )
        .overlay(ui, rect, fader_id(self.place, self.track), events);
    }

    fn fader_control(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let modulated = self.played(Control::Volume);
        let volume = modulated.map_or(track.mixer().volume, |(base, _)| base);
        let response = ui.interact(
            rect,
            fader_id(self.place, self.track),
            Sense::click_and_drag(),
        );
        self.register(ui, response.id, crate::focus::Slot::Volume);
        self.escape_to_title(ui, &response);
        // The handle moves a point per point along the scale, however far
        // the strip's height stretches it (R6, R32): a drag of the pointer
        // covers that many points of the default scale.
        let stretch = rect.height() / controls::FADER_TRAVEL_POINTS as f32;
        self.value_drag(ui, &response, Control::Volume, stretch, events);
        self.value_keys(ui, &response, Control::Volume, events);
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(3.0));
        let x = rect.center().x;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, c.separator.0),
        );
        let y_of = |gain| rect.bottom() - rect.height() * controls::fader_position(gain) as f32;
        // Spec: modulation R7: a mark under the thumb at the level it plays.
        if let Some((_, played)) = modulated {
            painter.rect_filled(
                Rect::from_center_size(pos2(x, y_of(played)), vec2(rect.width(), 3.0)),
                1.0,
                c.modulation.0,
            );
        }
        let y = y_of(volume);
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
            modulated_text(
                volume_spoken(volume),
                modulated.map(|(_, played)| volume_spoken(played)),
            ),
            controls::gain_db(volume).max(-1000.0),
            (-1000.0, controls::TOP_DB),
        );
    }

    /// The pan control (R6), with its reset triangle above it in a strip.
    pub fn pan(&self, ui: &mut Ui, rect: Rect, triangle: Option<Rect>, events: &mut Vec<UiEvent>) {
        self.pan_control(ui, rect, triangle, events);
        // Spec: modulation R15. A lit Map button maps the track's pan.
        crate::mapping::overlay(
            ui,
            self.theme,
            rect,
            pan_id(self.place, self.track),
            ModTarget::Mixer {
                track: self.track,
                param: MixerParam::Pan,
            },
            events,
        );
    }

    fn pan_control(
        &self,
        ui: &mut Ui,
        rect: Rect,
        triangle: Option<Rect>,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(track) = self.doc().track(self.track) else {
            return;
        };
        let modulated = self.played(Control::Pan);
        let pan = modulated.map_or(track.mixer().pan, |(base, _)| base);
        let response = ui.interact(
            rect,
            pan_id(self.place, self.track),
            Sense::click_and_drag(),
        );
        self.register(ui, response.id, crate::focus::Slot::Pan);
        self.escape_to_title(ui, &response);
        self.value_drag(
            ui,
            &response,
            Control::Pan,
            Control::Pan.drag_travel(),
            events,
        );
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
            let position = |pan: f64| (pan as f32 + 1.0) / 2.0;
            // Spec: modulation R7: the pointer stays at the base.
            if let Some(segment) =
                modulated.and_then(|(_, played)| ring_segment(position(pan), position(played)))
            {
                paint_ring_segment(&painter, self.theme, segment, |at| {
                    ring_point(centre, radius, at)
                });
            }
            let tip = ring_point(centre, radius, position(pan));
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
        let spoken = modulated_text(pan_text(pan), modulated.map(|(_, played)| pan_text(played)));
        self.describe_value(ui, &response, name, spoken, pan, (-1.0, 1.0));
        // The pan control's context menu (C-PAN-001, C-PAN-002): two
        // radio-ticked items, as a device parameter's menu is.
        let mode = track.mixer().pan_mode;
        response.context_menu(|ui| {
            for (item, wanted) in [
                ("Stereo Pan Mode", PanMode::Stereo),
                ("Split Stereo Pan Mode", PanMode::Split),
            ] {
                if ui.selectable_label(mode == wanted, item).clicked() {
                    events.push(UiEvent::Invoke(Invocation::new(
                        "mixer.set_pan_mode",
                        json!({ "track": self.track, "mode": wanted }),
                    )));
                    ui.close();
                }
            }
            // Spec: automation R29. The automation items come after the
            // control's own.
            crate::param_menu::ParamMenu::new(
                self.shell,
                self.track,
                AutomationTarget::Mixer {
                    param: MixerParam::Pan,
                },
            )
            .items(ui, true, events);
        });
        // A click on the control while Automation Mode is on shows its
        // envelope.
        crate::param_menu::ParamMenu::new(
            self.shell,
            self.track,
            AutomationTarget::Mixer {
                param: MixerParam::Pan,
            },
        )
        .click(ui, rect, events);
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

    /// The Preview/Cue Volume knob on Main's strip (L-MIX-004): a drag moves
    /// it along the fader's scale, Up and Down step it, and a double-click or
    /// Delete resets it to 0 dB (observed 18 October 2026).
    fn cue_volume(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let volume = self.doc().settings().cue_volume;
        let id = Id::new(("mixer-cue-volume", self.place));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        self.register(ui, id, crate::focus::Slot::Activator);
        self.escape_to_title(ui, &response);
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        let drag = id.with("drag");
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(drag, (volume, 0.0f64)));
            events.push(UiEvent::BeginGesture(controls::CUE_VOLUME_GESTURE));
        }
        if response.dragged() {
            let dy = response.drag_delta().y;
            let fine = ui.input(|i| i.modifiers.shift);
            let (start, moved) = ui.data_mut(|d| {
                let drag = d.get_temp_mut_or_default::<(f64, f64)>(drag);
                if dy != 0.0 {
                    drag.1 += f64::from(-dy / if fine { FINE } else { 1.0 });
                }
                *drag
            });
            if dy != 0.0 {
                let change = controls::cue_volume_moved(start, moved);
                if let Some(gain) = change.args["volume"].as_f64() {
                    ui.data_mut(|d| {
                        d.insert_temp(readout_id(), controls::cue_volume_readout(gain))
                    });
                }
                events.push(UiEvent::Invoke(change));
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<(f64, f64)>(drag));
            events.push(UiEvent::EndGesture);
        }
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
                        escape: true,
                        ..Default::default()
                    },
                );
            });
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
        let change = if reset {
            Some(controls::set_cue_volume(1.0))
        } else if steps != 0 {
            Some(controls::cue_volume_stepped(volume, steps, fine))
        } else {
            None
        };
        if let Some(change) = change {
            events.push(UiEvent::BeginGesture(controls::CUE_VOLUME_GESTURE));
            events.push(UiEvent::Invoke(change));
            events.push(UiEvent::EndGesture);
        }
        // The knob: a ring, and a line from its centre to where the volume
        // sits on the fader's scale, in the accent colour (Live's is blue).
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let centre = rect.center();
        let radius = (rect.height().min(rect.width()) * 0.5 - 1.0).max(2.0);
        painter.circle_stroke(centre, radius, Stroke::new(1.0, c.separator.0));
        let tip = ring_point(centre, radius, controls::fader_position(volume) as f32);
        painter.line_segment([centre, tip], Stroke::new(2.0, c.accent.0));
        focus_ring(&painter, self.theme, rect, &response);
        self.describe_value(
            ui,
            &response,
            "Cue Volume",
            controls::volume_spoken(volume),
            volume,
            (0.0, bl_model::MAX_CUE_VOLUME),
        );
    }

    /// The global Solo/Cue button on Main's strip (L-MIX-004): it reads Solo
    /// or Cue and toggles the mode. It does nothing, and is dimmed, while Cue
    /// Out and Main Out share a channel (observed 18 October 2026).
    fn cue_mode_button(&self, ui: &mut Ui, rect: Rect, events: &mut Vec<UiEvent>) {
        let doc = self.doc();
        let (available, cue) = (doc.cue_available(), doc.cue_active());
        let id = Id::new(("mixer-solo-cue", self.place));
        let response = ui.interact(
            rect,
            id,
            if available {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        self.register(ui, id, crate::focus::Slot::Solo);
        self.escape_to_title(ui, &response);
        let pressed = available
            && (response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))));
        if pressed {
            events.push(UiEvent::Invoke(Invocation::new(
                "mixer.set_cue_mode",
                json!({ "enabled": !cue }),
            )));
        }
        let c = &self.theme.colors;
        let text = if cue { "Cue" } else { "Solo" };
        let painter = ui.painter_at(rect.expand(2.0));
        let face = if !available {
            c.control.0.gamma_multiply(0.6)
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
            if available { c.text.0 } else { c.pending.0 },
        );
        focus_ring(&painter, self.theme, rect, &response);
        response
            .widget_info(|| WidgetInfo::labeled(WidgetType::Button, available, "Solo/Cue Mode"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label("Solo/Cue Mode".to_owned());
            node.set_value(text.to_owned());
            node.set_description(
                if available {
                    "Switches the tracks' Solo switches to Cue switches, which feed the Cue Out"
                } else {
                    "Needs a Cue Out that shares no output channel with Main Out"
                }
                .to_owned(),
            );
            if !available {
                node.set_disabled();
            }
        });
    }

    /// A switch (R7, R8): the activator, Solo, or Arm. Tracks without it
    /// show nothing in its place; a deactivated activator shows its number
    /// on grey (observed).
    pub fn switch(&self, ui: &mut Ui, switch: Switch, rect: Rect, events: &mut Vec<UiEvent>) {
        self.switch_control(ui, switch, rect, events);
        // Spec: modulation R15. A lit Map button maps the Track Activator.
        if switch == Switch::Activator && switch.on_track(self.doc(), self.track) {
            crate::mapping::overlay(
                ui,
                self.theme,
                rect,
                switch_id(self.place, switch, self.track),
                ModTarget::Mixer {
                    track: self.track,
                    param: MixerParam::Mute,
                },
                events,
            );
            crate::param_menu::ParamMenu::new(
                self.shell,
                self.track,
                AutomationTarget::Mixer {
                    param: MixerParam::Mute,
                },
            )
            .overlay(ui, rect, switch_id(self.place, switch, self.track), events);
        }
    }

    fn switch_control(&self, ui: &mut Ui, switch: Switch, rect: Rect, events: &mut Vec<UiEvent>) {
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
        // In Cue mode the Solo switch is a Cue switch and shows headphones
        // (observed 18 October 2026).
        let cue = switch == Switch::Solo && doc.cue_active();
        let (name, text, lit) = match switch {
            Switch::Activator => ("Track Activator", self.activator, c.accent.0),
            Switch::Solo if cue => ("Cue", "", c.accent.0),
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
        let ink = if on { c.accent_text.0 } else { c.text.0 };
        if cue {
            headphones(&painter, rect, ink);
        } else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                self.theme.small_font(),
                ink,
            );
        }
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
    /// meter's peak; while the extended meters show it is a button that
    /// holds the highest peak until a click sets it back to the current one
    /// (R33, R39).
    fn peak_field(&self, ui: &mut Ui, strip: &StripLayout, events: &mut Vec<UiEvent>) {
        let rect = strip.peak_field;
        let (peak, _) = self.levels();
        let shown = if strip.extended {
            self.shell.mixer_view().held_peak(self.track)
        } else {
            peak[0].max(peak[1])
        };
        let loudest = f64::from(shown);
        let id = Id::new(("mixer-peak", self.place, self.track));
        // In the strip's Tab order (observed in Live 12.4.6).
        let sense = if strip.extended {
            Sense::click()
        } else {
            Sense::focusable_noninteractive()
        };
        let response = ui.interact(rect, id, sense);
        if strip.extended && response.clicked() {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::RESET_PEAK,
                json!({ "track": self.track }),
            )));
        }
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
        let widget = if strip.extended {
            WidgetType::Button
        } else {
            WidgetType::Label
        };
        response.widget_info(|| WidgetInfo::labeled(widget, true, "Peak Level"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(if strip.extended {
                Role::Button
            } else {
                Role::Label
            });
            node.set_label("Peak Level".to_owned());
            node.set_value(volume_spoken(loudest));
        });
    }

    /// The meter's scale beside it: 0, 12, 24, 36, 48, 60 dB below 0, where
    /// the fader's handle sits at those values (observed).
    fn scale(&self, ui: &Ui, strip: &StripLayout) {
        let (left, right) = strip.labels;
        let painter = ui.painter();
        let font = egui::FontId::proportional(self.theme.sizes.small_text * 0.8);
        // While the extended meters show in a wide column, every tick mark
        // has its value beside it (R35); otherwise the labels are R3's,
        // left out where a short strip crowds them.
        let marks: Vec<(String, f32)> = if strip.tick_labels {
            TICKS
                .iter()
                .map(|db| (db.to_string(), strip.tick_y(-f64::from(*db))))
                .collect()
        } else {
            SCALE_LABELS
                .iter()
                .map(|(label, y)| ((*label).to_owned(), strip.scale_y(*y)))
                .collect()
        };
        let mut last = f32::NEG_INFINITY;
        for (label, y) in marks {
            if y - last < self.theme.sizes.small_text * 0.8 {
                continue;
            }
            last = y;
            painter.text(
                pos2((left + right) / 2.0, y),
                Align2::CENTER_CENTER,
                label,
                font.clone(),
                self.theme.colors.text_dim.0,
            );
        }
    }

    /// The extended meters' tick marks across the meter, one every 6 dB
    /// from 0 to −60 dB (R33).
    fn ticks(&self, ui: &Ui, strip: &StripLayout) {
        let painter = ui.painter_at(strip.meter);
        for db in TICKS {
            let y = strip.tick_y(-f64::from(db));
            painter.line_segment(
                [pos2(strip.meter.left(), y), pos2(strip.meter.right(), y)],
                Stroke::new(1.0, self.theme.colors.text_dim.0.gamma_multiply(0.7)),
            );
        }
    }

    /// The track's meter: a MIDI meter while its chain holds no instrument
    /// and no audio effect, otherwise the level meter (the MIDI meters spec,
    /// R1); the change is as soon as the chain is.
    pub fn meter(&self, ui: &mut Ui, rect: Rect) {
        if midi_only(self.doc(), self.track) {
            let value = self
                .shell
                .chain_meter(MeterOwner::Track(self.track), MeterSite::End)
                .unwrap_or(MeterValue::Midi {
                    velocity: 0.0,
                    channel: 0.0,
                    expression: 0.0,
                });
            let id = meter_id(self.place, self.track);
            crate::meters::midi(ui, self.theme, id, rect, value);
        } else {
            self.level_meter(ui, rect);
        }
    }

    /// The level meter (R10): left and right side by side on the fader's
    /// scale, the peak and the RMS level drawn as the reference describes
    /// them (both shown); how Live draws the two together with a signal
    /// isn't observed yet.
    fn level_meter(&self, ui: &mut Ui, rect: Rect) {
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
            // The line marking the channel's highest recent peak, which
            // holds for two seconds (R34).
            let line = self.shell.mixer_view().peak_line(self.track)[side];
            if line > 0.0 {
                let y = level(line).clamp(bar.top(), bar.bottom() - 1.0);
                painter.line_segment(
                    [pos2(bar.left(), y + 0.5), pos2(bar.right(), y + 0.5)],
                    Stroke::new(1.0, c.text.0),
                );
            }
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

    /// The gain-reduction bar (R40; IMP-25): a thin bar beside the meter
    /// showing the most reduction any of the track's compressors or
    /// limiters reported for the last block, on the meter's own 0-to-60 dB
    /// scale, filled from the top.
    fn gain_reduction(&self, ui: &mut Ui, rect: Rect) {
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, c.window.0);
        let reduction = self.shell.gain_reduction(self.track);
        if reduction < 0.0 {
            let position = meter_position(controls::db_gain(f64::from(reduction)) as f32);
            painter.rect_filled(
                Rect::from_min_max(
                    rect.min,
                    pos2(rect.right(), rect.top() + rect.height() * position),
                ),
                0.0,
                c.meter_hot.0,
            );
        }
        let id = gain_reduction_id(self.place, self.track);
        let response = ui.interact(rect, id, Sense::hover());
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::ProgressIndicator, true, "Gain Reduction")
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Meter);
            node.set_label("Gain Reduction".to_owned());
            node.set_value(format!("{reduction:.1} dB"));
            node.set_numeric_value(f64::from(reduction));
        });
    }
}

/// The digit a key types, if it's a digit key.
/// The point at `position` (0 to 1) on the ring of a send or pan knob, which
/// turns 135 degrees either side of straight up.
fn ring_point(centre: egui::Pos2, radius: f32, position: f32) -> egui::Pos2 {
    let sweep = std::f32::consts::FRAC_PI_4 * 3.0;
    let angle = -sweep + 2.0 * sweep * position;
    centre + vec2(angle.sin(), -angle.cos()) * radius
}

/// How far one point of drag moves a split stereo pan control.
const SPLIT_PAN_PER_POINT: f64 = 0.05;

/// The gesture a Track Delay change runs as.
const TRACK_DELAY_GESTURE: &str = "Change track delay";

/// The gesture a split stereo pan change runs as.
fn split_gesture(side: SplitSide) -> &'static str {
    match side {
        SplitSide::Left => "Change \"Split Pan Left\"",
        SplitSide::Right => "Change \"Split Pan Right\"",
    }
}

/// The Track Delay as its field shows it: the number in its current unit.
fn delay_text(delay: TrackDelay) -> String {
    match delay {
        TrackDelay::Milliseconds { value } => format!("{value:.2}"),
        TrackDelay::Samples { value } => value.to_string(),
    }
}

/// The delay as assistive technology reads it, unit named.
fn delay_spoken(delay: TrackDelay) -> String {
    match delay {
        TrackDelay::Milliseconds { value } => format!("{value:.2} ms"),
        TrackDelay::Samples { value } => format!("{value} samples"),
    }
}

/// The Status Bar's readout while the Track Delay is dragged.
fn delay_readout(delay: TrackDelay) -> String {
    format!("Track Delay: {}", delay_spoken(delay))
}

/// The delay's unit switch's label.
fn delay_unit_text(delay: TrackDelay) -> &'static str {
    match delay {
        TrackDelay::Milliseconds { .. } => "ms",
        TrackDelay::Samples { .. } => "smp",
    }
}

/// The delay's numeric value, whatever its unit.
fn delay_number(delay: TrackDelay) -> f64 {
    match delay {
        TrackDelay::Milliseconds { value } => value,
        TrackDelay::Samples { value } => value as f64,
    }
}

/// The delay's range, whatever its unit.
fn delay_range(delay: TrackDelay) -> (f64, f64) {
    match delay {
        TrackDelay::Milliseconds { .. } => (-MAX_TRACK_DELAY_MS, MAX_TRACK_DELAY_MS),
        TrackDelay::Samples { .. } => (
            -MAX_TRACK_DELAY_SAMPLES as f64,
            MAX_TRACK_DELAY_SAMPLES as f64,
        ),
    }
}

/// The delay a drag of `points` points makes from `start`, in `start`'s unit.
fn delay_moved(start: TrackDelay, points: f64) -> TrackDelay {
    match start {
        TrackDelay::Milliseconds { value } => TrackDelay::Milliseconds {
            value: (value + points).clamp(-MAX_TRACK_DELAY_MS, MAX_TRACK_DELAY_MS),
        },
        TrackDelay::Samples { value } => TrackDelay::Samples {
            value: (value + points.round() as i64)
                .clamp(-MAX_TRACK_DELAY_SAMPLES, MAX_TRACK_DELAY_SAMPLES),
        },
    }
}

/// The delay `steps` steps of `from`, in `from`'s unit; Shift makes a step
/// finer.
fn delay_stepped(from: TrackDelay, steps: i32, fine: bool) -> TrackDelay {
    match from {
        TrackDelay::Milliseconds { value } => {
            let step = if fine { 0.01 } else { 0.1 };
            TrackDelay::Milliseconds {
                value: (value + f64::from(steps) * step)
                    .clamp(-MAX_TRACK_DELAY_MS, MAX_TRACK_DELAY_MS),
            }
        }
        TrackDelay::Samples { value } => {
            let step = if fine { 1 } else { 10 };
            TrackDelay::Samples {
                value: (value + i64::from(steps) * step)
                    .clamp(-MAX_TRACK_DELAY_SAMPLES, MAX_TRACK_DELAY_SAMPLES),
            }
        }
    }
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
pub fn title_colour(
    theme: &Theme,
    doc: &bl_model::Document,
    track: TrackId,
    kind: TrackKind,
) -> Color32 {
    if kind == TrackKind::Main {
        theme.colors.control_pressed.0
    } else {
        track_colour(theme, doc, track)
    }
}
