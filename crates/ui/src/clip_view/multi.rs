//! Several MIDI clips in the note editor (WP-6.6): the title bar with the
//! Focus button, a loop bar for each clip, and every clip's notes on one
//! shared time axis. The clips shown, the foreground clip, the selected
//! bars and Focus Mode are the shell's (`ClipView`); the commands this
//! sends are the ones agents run. Spec: multi-clip-editing.
//!
//! The expression lanes and the footer under the note area edit the
//! foreground clip's notes alone (R3): they are given its notes moved onto
//! the shared axis, which leaves their places as they are.

use bl_model::Clip;
use bl_shell::multi_clip::Window;
use egui::Color32;

use super::*;

/// How tall a loop bar is to begin with, in points.
pub const BAR_HEIGHT: f32 = 12.0;
/// The shortest and tallest a loop bar may be made.
const BAR_LIMITS: (f32, f32) = (5.0, 40.0);
/// How much of the Clip View the loop bars may take at most.
const BARS_SHARE: f32 = 0.35;
/// How far above the title bar a drag makes the loop bars taller or
/// shorter, and how thick the strip that takes it is.
const HANDLE: f32 = 3.0;
/// How wide the Focus button is.
const FOCUS_WIDTH: f32 = 52.0;
/// How far a loop bar's brace end is grabbed from.
const BRACE_GRAB: f32 = 4.0;

/// Where the multi-clip editor's parts sit.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// The editor's title bar, over the loop bars (L-CLIP-021).
    pub title: Rect,
    /// The Focus button at the title bar's right end.
    pub focus: Rect,
    /// The strip above the title bar that is dragged to make the bars
    /// taller or shorter (R2).
    pub handle: Rect,
    /// A row of loop bars for each clip (Session) or track (Arrangement).
    pub rows: Vec<Rect>,
    /// The note area, the lanes and the footer.
    pub layout: NoteLayout,
}

/// The height the loop bars take: `wanted`, held between the limits and to
/// their share of the view.
fn bar_height(view: Rect, rows: usize, wanted: f32) -> f32 {
    let share = view.height() * BARS_SHARE / rows.max(1) as f32;
    wanted
        .clamp(BAR_LIMITS.0, BAR_LIMITS.1)
        .min(share.max(BAR_LIMITS.0))
}

/// Where the parts of the editor sit in `view` with `rows` rows of loop bars
/// each `bar_height` tall and the lanes at `lane_heights`.
#[must_use]
pub fn geometry(
    view: Rect,
    rows: usize,
    bar_height_wanted: f32,
    lane_heights: &[(NoteField, f32)],
) -> Geometry {
    let height = bar_height(view, rows, bar_height_wanted);
    let top = view.top() + TOOLBAR_HEIGHT;
    let title = Rect::from_min_max(
        pos2(view.left(), top),
        pos2(view.right(), top + TITLE_BAR_HEIGHT),
    );
    let focus = Rect::from_min_size(
        pos2(title.right() - FOCUS_WIDTH - 3.0, title.top() + 1.0),
        vec2(FOCUS_WIDTH, (title.height() - 2.0).max(1.0)),
    );
    let handle = Rect::from_min_max(
        pos2(view.left(), top - HANDLE),
        pos2(view.right(), top + HANDLE),
    );
    let rows_rects: Vec<Rect> = (0..rows)
        .map(|row| {
            let y = title.bottom() + height * row as f32;
            Rect::from_min_max(pos2(view.left(), y), pos2(view.right(), y + height))
        })
        .collect();
    // The note area starts under the last row: note_layout leaves room
    // for the toolbar and one marker strip, so it's given a view whose top
    // is lower by what the title bar and the rows take beyond that.
    let shift = TITLE_BAR_HEIGHT + height * rows as f32 - STRIP_HEIGHT;
    let layout = note_lanes::note_layout(
        Rect::from_min_max(pos2(view.left(), view.top() + shift), view.max),
        lane_heights,
    );
    Geometry {
        title,
        focus,
        handle,
        rows: rows_rects,
        layout,
    }
}

/// A clip the editor shows, with what the frame needs of it.
struct Shown {
    clip: ClipId,
    name: String,
    colour: Color32,
    midi: MidiData,
    clip_start: Ticks,
    length: Ticks,
    notes: Vec<Note>,
    selected: Vec<u32>,
    windows: Vec<Window>,
    /// The row of loop bars it sits in.
    row: usize,
}

impl Shown {
    /// How far the shared axis is ahead of the clip's contents where its
    /// first stretch plays.
    fn shift(&self) -> Ticks {
        self.windows
            .first()
            .map_or(self.clip_start - self.midi.start, |w| w.at - w.from)
    }

    /// The loop brace's end, in contents ticks.
    fn loop_end(&self) -> Ticks {
        self.midi.loop_end.unwrap_or(self.midi.start + self.length)
    }
}

/// A note drawn on the axis: the clip and place it is, where it is, and how
/// it's painted.
struct Drawn {
    clip: ClipId,
    place: u32,
    rect: Rect,
    colour: Color32,
    selected: bool,
    muted: bool,
}

/// What a press on a loop bar's brace is dragging: every selected bar's
/// loop moves together by the same amount (R10).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BarDrag {
    part: StripPart,
    /// The tick of the grabbed clip's contents under the press, and how far
    /// the axis is ahead of them there.
    grab: Ticks,
    shift: Ticks,
    clips: Vec<(ClipId, Ticks, Option<Ticks>, Ticks)>,
}

impl BarDrag {
    /// The command for the pointer at `pos`: each clip's loop brace moved by
    /// the amount the grabbed marker moved, snapped to the grid.
    fn invocation(
        &self,
        arrangement: &ArrangementView,
        doc: &Document,
        keys: EditKeys,
        grid: Rect,
        axis: (Ticks, Ticks),
        pos: Pos2,
    ) -> (&'static str, Value) {
        let at = tick_at(grid, axis.0, axis.1, pos.x);
        let snapped = arrangement.snap_drag(doc, at, keys.bypass);
        let delta = snapped - (self.grab + self.shift);
        let loops: Vec<Value> = self
            .clips
            .iter()
            .map(|(clip, start, end, clip_end)| {
                let end_of = end.unwrap_or(*clip_end);
                let (new_start, new_end) = match self.part {
                    StripPart::LoopStart => (
                        (*start + delta).clamp(Ticks::ZERO, (end_of - Ticks(1)).max(Ticks::ZERO)),
                        *end,
                    ),
                    StripPart::LoopEnd => (*start, Some((end_of + delta).max(*start + Ticks(1)))),
                    _ => {
                        let by = delta.max(-*start);
                        (*start + by, Some(end_of + by))
                    }
                };
                json!({ "clip": clip, "start": new_start, "end": new_end })
            })
            .collect();
        (SET_MIDI_LOOP, json!({ "clips": loops }))
    }
}

/// The part of a bar's brace a press falls on.
fn hit_brace(brace: Rect, pos: Pos2) -> Option<StripPart> {
    if !brace.expand2(vec2(BRACE_GRAB, 0.0)).contains(pos) {
        return None;
    }
    Some(if (pos.x - brace.left()).abs() <= BRACE_GRAB {
        StripPart::LoopStart
    } else if (pos.x - brace.right()).abs() <= BRACE_GRAB {
        StripPart::LoopEnd
    } else {
        StripPart::LoopBody
    })
}

impl ClipViewUi {
    /// How tall the loop bars are (R2).
    #[must_use]
    pub fn bar_height(&self) -> f32 {
        self.bar_height
    }

    /// The clips' data for the frame, in the loop bars' order.
    fn gather(
        shell: &Shell,
        theme: &Theme,
        shown: &[ClipId],
        layout: &bl_shell::multi_clip::Layout,
    ) -> Vec<Shown> {
        let doc = shell.editor().document();
        let view = shell.clip_view();
        let session = view.shows_session();
        let mut tracks: Vec<bl_model::TrackId> = Vec::new();
        shown
            .iter()
            .enumerate()
            .filter_map(|(at, id)| {
                let clip: &Clip = doc.clip(*id)?;
                let ClipData::Midi(midi) = &clip.data else {
                    return None;
                };
                let track = doc.clip_track(*id);
                // A row for each clip in the Session View, for each track in
                // the Arrangement (R2).
                let row = if session {
                    at
                } else {
                    let track = track?;
                    tracks
                        .iter()
                        .position(|other| *other == track)
                        .unwrap_or_else(|| {
                            tracks.push(track);
                            tracks.len() - 1
                        })
                };
                Some(Shown {
                    clip: *id,
                    name: clip.name.clone(),
                    colour: clip
                        .colour
                        .map_or(theme.colors.accent.0, |swatch| theme.swatch(swatch)),
                    midi: midi.clone(),
                    clip_start: clip.start,
                    length: clip.length,
                    notes: notes_in_order(midi),
                    selected: view.selection_of(*id).to_vec(),
                    windows: layout.windows(*id).to_vec(),
                    row,
                })
            })
            .collect()
    }

    /// Shows the Clip View for several clips (WP-6.6).
    #[expect(clippy::too_many_lines, reason = "the editor's whole frame")]
    pub(super) fn show_multi(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        shown: &[ClipId],
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let arrangement = shell.arrangement();
        let state = shell.clip_view();
        let colours = &theme.colors;
        let layout = state.layout(doc, arrangement);
        let clips = Self::gather(shell, theme, shown, &layout);
        let Some(foreground) = state
            .shown_clip(doc, arrangement)
            .and_then(|fg| clips.iter().position(|c| c.clip == fg))
        else {
            return;
        };
        let rows = clips.iter().map(|c| c.row).max().map_or(1, |row| row + 1);
        let lanes = state.lanes().shown();
        let geometry = geometry(rect, rows, self.bar_height, &self.lanes.heights(&lanes));
        let grid = geometry.layout.grid;
        if grid.width() < 1.0 || grid.height() < KEY_HEIGHT {
            return;
        }
        let axis = (layout.from, (layout.to - layout.from).max(Ticks(1)));
        let focus = state.focus_active(doc, arrangement);
        let bars = state.loop_bars(doc, arrangement);
        let session = state.shows_session();
        toolbar(ui, shell, theme, rect, false, events);

        // The key rows hold every shown clip's notes (R9).
        let fold = state.fold(doc, clips[foreground].clip);
        let keys_held: Vec<Note> = {
            let mut keys: Vec<u8> = clips
                .iter()
                .flat_map(|c| c.notes.iter().map(|note| note.key))
                .collect();
            keys.sort_unstable();
            keys.dedup();
            keys.into_iter()
                .map(|key| Note::new(key, Ticks::ZERO, Ticks(1), 1))
                .collect()
        };
        let rows_of_keys = KeyRows::of(fold, &keys_held, self.top, self.first);
        self.first = rows_of_keys.first;

        // Where each note is drawn, the foreground clip's last so it wins.
        let hovered_bar = ui
            .input(|i| i.pointer.hover_pos())
            .and_then(|pos| {
                clips
                    .iter()
                    .find(|c| bar_rect(&geometry, grid, axis, c, session).contains(pos))
            })
            .map(|c| c.clip);
        let mut order: Vec<usize> = (0..clips.len()).filter(|at| *at != foreground).collect();
        order.push(foreground);
        let mut drawn: Vec<Drawn> = Vec::new();
        for at in order {
            let c = &clips[at];
            let active = !focus || at == foreground || hovered_bar == Some(c.clip);
            let face = if active {
                c.colour
            } else {
                colours.clip_muted.0
            };
            for (place, note) in c.notes.iter().enumerate() {
                let Some(y) = rows_of_keys.y_of(grid, note.key) else {
                    continue;
                };
                for w in &c.windows {
                    let from = note.start.max(w.from);
                    let to = (note.start + note.length).min(w.to);
                    if to <= from {
                        continue;
                    }
                    let x0 = x_of_tick(grid, axis.0, axis.1, w.at + (from - w.from));
                    let x1 = x_of_tick(grid, axis.0, axis.1, w.at + (to - w.from));
                    let rect =
                        Rect::from_min_max(pos2(x0, y), pos2(x1.max(x0 + 2.0), y + KEY_HEIGHT));
                    if rect.right() < grid.left() || rect.left() > grid.right() {
                        continue;
                    }
                    drawn.push(Drawn {
                        clip: c.clip,
                        place: u32::try_from(place).unwrap_or(u32::MAX),
                        rect,
                        colour: crate::theme::note_colour(face, note.velocity),
                        selected: c
                            .selected
                            .contains(&u32::try_from(place).unwrap_or(u32::MAX)),
                        muted: note.mute,
                    });
                }
            }
        }

        self.paint_multi(
            ui,
            shell,
            theme,
            &geometry,
            &clips,
            foreground,
            &rows_of_keys,
            axis,
            &drawn,
            &bars,
            focus,
            hovered_bar,
        );

        // The lanes and the footer edit the foreground clip's notes alone,
        // given on the shared axis (R3).
        let fg = &clips[foreground];
        let shift = fg.shift();
        let moved: Vec<Note> = fg
            .notes
            .iter()
            .map(|note| Note {
                start: note.start + shift,
                ..note.clone()
            })
            .collect();
        let chosen_mask = note_lanes::selection_mask(moved.len(), &fg.selected);
        let frame = note_lanes::Frame {
            clip: fg.clip,
            layout: &geometry.layout,
            start: axis.0,
            length: axis.1,
            notes: &moved,
            groups: &fg.midi.groups,
            selected: &fg.selected,
            selected_mask: &chosen_mask,
            face: fg.colour,
            hovered: None,
            step: arrangement.grid_ticks(doc),
            snap: arrangement.grid().snap,
            tuning: super::tuning_of_clip(shell, fg.clip),
        };
        self.lanes.paint(ui, shell, theme, &frame);
        self.input_multi(
            ui,
            shell,
            &geometry,
            &clips,
            foreground,
            &rows_of_keys,
            axis,
            &drawn,
            &bars,
            focus,
            events,
        );
        self.lanes.input(ui, shell, &frame, events);
        self.lanes.footer(ui, shell, theme, &frame, events);
        // The note-selection keys act on the notes selected in every clip
        // the edits reach.
        let any: Vec<u32> = clips
            .iter()
            .filter(|c| !focus || c.clip == fg.clip)
            .flat_map(|c| c.selected.iter().copied())
            .collect();
        let before = events.len();
        self.keys(
            ui,
            shell,
            doc,
            arrangement,
            &frame,
            fg.length,
            &fg.midi,
            &fg.notes,
            &any,
            &rows_of_keys,
            events,
        );
        for event in &mut events[before..] {
            across(event);
        }
    }

    /// Paints the title bar, the loop bars, the key track, the notes and the
    /// lanes.
    #[expect(clippy::too_many_arguments, reason = "the editor's whole layout")]
    fn paint_multi(
        &self,
        ui: &Ui,
        shell: &Shell,
        theme: &Theme,
        geometry: &Geometry,
        clips: &[Shown],
        foreground: usize,
        keys: &KeyRows,
        axis: (Ticks, Ticks),
        drawn: &[Drawn],
        bars: &[ClipId],
        focus: bool,
        hovered_bar: Option<ClipId>,
    ) {
        let painter = ui.painter();
        let colours = &theme.colors;
        let grid = geometry.layout.grid;
        let session = shell.clip_view().shows_session();
        // The title bar: the foreground clip's name, or its track's, and
        // how many clips are shown.
        let fg = &clips[foreground];
        painter.rect_filled(geometry.title, 0.0, colours.panel.0);
        let doc = shell.editor().document();
        let name = if fg.name.is_empty() {
            doc.clip_track(fg.clip)
                .and_then(|track| doc.track(track))
                .map_or_else(String::new, |track| track.name().to_owned())
        } else {
            fg.name.clone()
        };
        painter.text(
            pos2(geometry.title.left() + 6.0, geometry.title.center().y),
            Align2::LEFT_CENTER,
            format!("{name}  ({} clips)", clips.len()),
            theme.small_font(),
            fg.colour,
        );
        // The Focus button.
        let on = shell.clip_view().focus_mode();
        painter.rect_filled(
            geometry.focus,
            theme.sizes.corner,
            if on {
                colours.control_pressed.0
            } else {
                colours.control.0
            },
        );
        painter.text(
            geometry.focus.center(),
            Align2::CENTER_CENTER,
            "Focus",
            theme.small_font(),
            colours.text.0,
        );
        // The loop bars, each in its clip's colour; the foreground clip's
        // carries an LED, a selected one is black, and with Focus Mode on the
        // inactive ones are grey (R2, R3, R10, R14).
        for row in &geometry.rows {
            painter.rect_filled(*row, 0.0, colours.lane.0);
        }
        for (at, c) in clips.iter().enumerate() {
            let body = bar_rect(geometry, grid, axis, c, session);
            let selected = bars.contains(&c.clip);
            let active = !focus || at == foreground;
            let face = if selected {
                Color32::BLACK
            } else if active || hovered_bar == Some(c.clip) {
                c.colour
            } else {
                colours.clip_muted.0
            };
            painter.rect_filled(body, 1.0, face.gamma_multiply(0.45));
            for brace in braces(c, session, axis, grid) {
                let on = c.midi.looping;
                let rect =
                    Rect::from_min_max(pos2(brace.0, body.top()), pos2(brace.1, body.bottom()));
                painter.rect_filled(rect, 1.0, if on { face } else { colours.loop_off.0 });
            }
            if at == foreground {
                let row = geometry.rows[c.row];
                painter.circle_filled(pos2(grid.left() - 12.0, row.center().y), 3.5, c.colour);
            }
        }

        let style = KeyStyle::of(shell, clips[foreground].clip);
        paint_key_track(ui, theme, grid, keys, &style);
        paint_scale_rows(ui, theme, grid, keys, &style);
        paint_grid_lines(ui, shell, theme, grid, axis.0, axis.1);
        paint_row_lines(ui, theme, grid, keys, style.limit());
        let corner = theme.sizes.corner.min(2.0);
        for note in drawn {
            painter.rect_filled(
                note.rect,
                corner,
                if note.muted {
                    colours.clip_muted.0
                } else {
                    note.colour
                },
            );
            if note.selected {
                painter.rect_stroke(
                    note.rect,
                    corner,
                    Stroke::new(1.5, colours.time_selection.0),
                    StrokeKind::Inside,
                );
            }
        }
        // Spec: R5. In the Session View, the loop start and end of the clip
        // under the pointer are drawn as black lines.
        if session
            && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
            && let Some(over) = drawn.iter().rev().find(|d| d.rect.contains(pos))
            && let Some(c) = clips.iter().find(|c| c.clip == over.clip)
        {
            for w in &c.windows {
                for tick in [w.at, w.at + (w.to - w.from)] {
                    let x = x_of_tick(grid, axis.0, axis.1, tick);
                    painter.line_segment(
                        [pos2(x, grid.top()), pos2(x, grid.bottom())],
                        Stroke::new(1.0, Color32::BLACK),
                    );
                }
            }
        }
        if let Some((from, to)) = self.marquee {
            let marquee = Rect::from_two_pos(from, to).intersect(grid);
            painter.rect_filled(marquee, 0.0, colours.time_selection.0.gamma_multiply(0.2));
            painter.rect_stroke(
                marquee,
                0.0,
                Stroke::new(1.0, colours.time_selection.0),
                StrokeKind::Inside,
            );
        }
    }

    /// The loop bars', the note area's and the Focus button's clicks and
    /// drags, and the wheel.
    #[expect(clippy::too_many_arguments, reason = "the editor's whole input")]
    #[expect(clippy::too_many_lines, reason = "one pass over the editor's input")]
    fn input_multi(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        geometry: &Geometry,
        clips: &[Shown],
        foreground: usize,
        keys_rows: &KeyRows,
        axis: (Ticks, Ticks),
        drawn: &[Drawn],
        bars: &[ClipId],
        focus: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let arrangement = shell.arrangement();
        let grid = geometry.layout.grid;
        let session = shell.clip_view().shows_session();
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        let invoke = |events: &mut Vec<UiEvent>, id: &str, args: Value| {
            events.push(UiEvent::Invoke(Invocation::new(id, args)));
        };

        // The Focus button.
        let button = ui.interact(geometry.focus, Id::new("clip-view-focus"), Sense::click());
        button.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::Checkbox,
                true,
                shell.clip_view().focus_mode(),
                "Focus",
            )
        });
        if button.clicked() {
            invoke(events, ids::FOCUS_MODE, json!({}));
        }

        // Dragging just above the title bar makes the bars taller (up) or
        // shorter (down) (R2).
        let handle = ui.interact(
            geometry.handle,
            Id::new("clip-view-bars-height"),
            Sense::drag(),
        );
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if handle.dragged() {
            let rows = geometry.rows.len().max(1) as f32;
            self.bar_height =
                (self.bar_height - handle.drag_delta().y / rows).clamp(BAR_LIMITS.0, BAR_LIMITS.1);
        }

        // The loop bars.
        for (at, c) in clips.iter().enumerate() {
            let body = bar_rect(geometry, grid, axis, c, session);
            let id = Id::new(("clip-view-loop-bar", c.clip));
            let response = ui.interact(body.expand2(vec2(0.0, 0.0)), id, Sense::click_and_drag());
            let selected = bars.contains(&c.clip);
            let label = format!(
                "Loop bar: {}",
                if c.name.is_empty() { "clip" } else { &c.name }
            );
            response
                .widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, &label));
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_description(if at == foreground {
                    "Foreground clip".to_owned()
                } else {
                    String::new()
                });
            });
            let brace_rects: Vec<Rect> = braces(c, session, axis, grid)
                .into_iter()
                .map(|(x0, x1)| Rect::from_min_max(pos2(x0, body.top()), pos2(x1, body.bottom())))
                .collect();
            if response.drag_started()
                && let Some(press) = ui.input(|i| i.pointer.press_origin())
                && let Some(part) = brace_rects.iter().find_map(|b| hit_brace(*b, press))
            {
                let shift = c.shift();
                let at_axis = tick_at(grid, axis.0, axis.1, press.x);
                let chosen: Vec<ClipId> = if selected {
                    bars.to_vec()
                } else {
                    vec![c.clip]
                };
                self.bar_drag = Some(BarDrag {
                    part,
                    grab: at_axis - shift,
                    shift,
                    clips: clips
                        .iter()
                        .filter(|other| chosen.contains(&other.clip))
                        .map(|other| {
                            (
                                other.clip,
                                other.midi.loop_start,
                                other.midi.loop_end,
                                other.midi.start + other.length,
                            )
                        })
                        .collect(),
                });
            }
            if response.clicked() {
                let order: Vec<ClipId> = clips.iter().map(|other| other.clip).collect();
                if focus && at != foreground {
                    // Spec: R14. Clicking another clip's bar makes it the
                    // active clip.
                    invoke(events, ids::FOREGROUND, json!({ "clip": c.clip }));
                } else if keys.toggle && !focus {
                    invoke(
                        events,
                        ids::SELECT_LOOP_BARS,
                        json!({ "clips": [c.clip], "toggle": true }),
                    );
                } else if keys.extend && !focus {
                    // Shift-click selects the bars between the first selected
                    // bar (else the foreground clip's) and this one
                    // (I-DRAG-041).
                    let anchor = bars
                        .first()
                        .and_then(|bar| order.iter().position(|clip| clip == bar))
                        .unwrap_or(foreground);
                    let (a, b) = (anchor.min(at), anchor.max(at));
                    invoke(
                        events,
                        ids::SELECT_LOOP_BARS,
                        json!({ "clips": &order[a..=b] }),
                    );
                } else {
                    invoke(events, ids::SELECT_LOOP_BARS, json!({ "clips": [c.clip] }));
                }
            }
            response.context_menu(|ui| {
                // Spec: R11, C-LOOPBAR. Duplicate acts on the selected bars,
                // or on this one when it isn't selected.
                if !selected {
                    invoke(events, ids::SELECT_LOOP_BARS, json!({ "clips": [c.clip] }));
                }
                self.menus
                    .show(ui, shell, &context_menus::loop_bar(), events);
            });
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if let (Some(drag), Some(pos)) = (self.bar_drag.as_ref(), pointer) {
            if ui.input(|i| i.pointer.primary_down()) {
                let (id, args) = drag.invocation(arrangement, doc, keys, grid, axis, pos);
                gesture(events, GesturePhase::Update, id, args);
            } else {
                let (id, args) = drag.invocation(arrangement, doc, keys, grid, axis, pos);
                self.bar_drag = None;
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: Some(Invocation::new(id, args)),
                });
            }
        }
        if self.bar_drag.is_some() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.bar_drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }

        // The note area.
        let response = ui.interact(
            grid,
            Id::new("clip-view-multi-grid"),
            Sense::click_and_drag(),
        );
        let draw_mode = shell.clip_view().draw_mode();
        let hit = |pos: Pos2| -> Option<(ClipId, u32, NoteZone)> {
            drawn.iter().rev().find(|d| d.rect.contains(pos)).map(|d| {
                let grab = EDGE_GRAB.min(d.rect.width() / 4.0);
                let zone = if d.rect.right() - pos.x <= grab {
                    NoteZone::End
                } else {
                    NoteZone::Body
                };
                (d.clip, d.place, zone)
            })
        };
        if response.clicked() || response.drag_started() {
            self.lanes.leave_focus();
        }
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && !draw_mode
        {
            match hit(press) {
                Some((clip, place, zone)) => {
                    let c = clips.iter().find(|c| c.clip == clip);
                    let grabbed = c.is_some_and(|c| c.selected.contains(&place));
                    // The selected notes of every clip the edits reach, when
                    // the grabbed note is one of them; else the note alone.
                    let targets: Vec<(ClipId, Vec<u32>)> = if grabbed {
                        clips
                            .iter()
                            .filter(|c| !focus || c.clip == clips[foreground].clip)
                            .filter(|c| !c.selected.is_empty())
                            .map(|c| (c.clip, c.selected.clone()))
                            .collect()
                    } else {
                        invoke(
                            events,
                            ids::SELECT_NOTES,
                            json!({ "clip": clip, "notes": [place], "mode": "replace" }),
                        );
                        vec![(clip, vec![place])]
                    };
                    if clip != clips[foreground].clip {
                        invoke(events, ids::FOREGROUND, json!({ "clip": clip }));
                    }
                    let all: Vec<Note> = targets
                        .iter()
                        .flat_map(|(clip, places)| {
                            clips
                                .iter()
                                .find(|c| c.clip == *clip)
                                .map(|c| notes_at(&c.notes, places))
                                .unwrap_or_default()
                        })
                        .collect();
                    let note = c.and_then(|c| c.notes.get(place as usize));
                    let at_press = tick_at(grid, axis.0, axis.1, press.x);
                    let origin = c
                        .zip(note)
                        .map_or(at_press, |(c, note)| axis_start(c, note, at_press));
                    self.drag = Some(if zone == NoteZone::End {
                        let shortest = if arrangement.grid().snap {
                            arrangement.grid_ticks(doc)
                        } else {
                            Ticks(1)
                        };
                        let end = origin + note.map_or(Ticks::ZERO, |note| note.length);
                        NoteDrag::Resize {
                            limit: resize_limit(&all, shortest),
                            targets,
                            end,
                            grab: at_press - end,
                        }
                    } else {
                        let moving = NoteDrag::Move {
                            limits: move_limits(&all),
                            start: origin,
                            grab_key: keys_rows
                                .key_at(grid, press.y)
                                .unwrap_or_else(|| f32::from(note.map_or(60, |note| note.key))),
                            rows: keys_rows.keys.clone(),
                            grab: at_press - origin,
                            copy: keys.copy,
                            targets: targets.clone(),
                            grabbed: note.map_or((60, 100), |note| (note.key, note.velocity)),
                        };
                        if keys.bypass {
                            NoteDrag::Undecided {
                                press,
                                moving: Box::new(moving),
                                velocity: Box::new(NoteDrag::Velocity {
                                    targets,
                                    from: press.y,
                                }),
                            }
                        } else {
                            moving
                        }
                    });
                }
                None => self.marquee = Some((press, press)),
            }
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        if let (Some(NoteDrag::Undecided { press, .. }), Some(pos)) = (self.drag.as_ref(), pointer)
            && (pos - *press).length() >= 3.0
        {
            let across = (pos - *press).x.abs() >= (pos - *press).y.abs();
            if let Some(NoteDrag::Undecided {
                moving, velocity, ..
            }) = self.drag.take()
            {
                self.drag = Some(if across { *moving } else { *velocity });
            }
        }
        if response.dragged() {
            if let (Some(drag), Some(pos)) = (self.drag.as_ref(), pointer) {
                let (id, args) =
                    drag.invocation(arrangement, doc, keys, grid, keys_rows, axis.0, axis.1, pos);
                gesture(events, GesturePhase::Update, id, args);
            } else if let (Some((from, _)), Some(pos)) = (self.marquee, pointer) {
                self.marquee = Some((from, pos));
                let marquee = Rect::from_two_pos(from, pos);
                let inside: Vec<Value> = clips
                    .iter()
                    .filter(|c| !focus || c.clip == clips[foreground].clip)
                    .map(|c| {
                        let mut places: Vec<u32> = drawn
                            .iter()
                            .filter(|d| d.clip == c.clip && d.rect.intersects(marquee))
                            .map(|d| d.place)
                            .collect();
                        places.sort_unstable();
                        places.dedup();
                        json!({ "clip": c.clip, "notes": places })
                    })
                    .collect();
                invoke(
                    events,
                    ids::SELECT_NOTES,
                    json!({ "clips": inside, "mode": "replace" }),
                );
            }
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) && self.bar_drag.is_none() {
            let note = self.drag.take().is_some();
            let lanes = self.lanes.cancel();
            if note || lanes == Cancelled::Gesture {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Cancel,
                    invocation: None,
                });
            } else if lanes != Cancelled::Typed
                && shell.active_contexts().contains(&Context::NoteEditor)
            {
                // Esc deselects the notes in every clip (piano-roll R24).
                invoke(
                    events,
                    ids::SELECT_NOTES,
                    json!({ "clips": [{ "clip": clips[foreground].clip, "notes": [] }], "mode": "replace" }),
                );
            }
        }
        if response.drag_stopped() {
            let last = match (self.drag.as_ref(), pointer) {
                (Some(drag), Some(pos)) => {
                    let (id, args) = drag.invocation(
                        arrangement,
                        doc,
                        keys,
                        grid,
                        keys_rows,
                        axis.0,
                        axis.1,
                        pos,
                    );
                    Some(Invocation::new(id, args))
                }
                _ => None,
            };
            if self.drag.take().is_some() {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: last,
                });
            }
            self.marquee = None;
        }
        if let Some(hover) = response.hover_pos()
            && !draw_mode
            && let Some((_, _, zone)) = hit(hover)
        {
            ui.ctx().set_cursor_icon(match zone {
                NoteZone::End => CursorIcon::ResizeHorizontal,
                NoteZone::Body => CursorIcon::Grab,
            });
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let over = hit(pos);
            let at = arrangement.snap_drag(doc, tick_at(grid, axis.0, axis.1, pos.x), keys.bypass);
            match (draw_mode, over) {
                (true, Some((clip, place, _))) => invoke(
                    events,
                    ids::DELETE_NOTES,
                    json!({ "clip": clip, "notes": [place] }),
                ),
                (true, None) => {
                    // Spec: R8. A note belongs to the clip its start lies
                    // in: the foreground clip when it plays there, else the
                    // first that does, and with Focus Mode on the active
                    // clip alone.
                    let plays = |c: &&Shown| c.windows.iter().any(|w| contains(w, at));
                    let fg = &clips[foreground];
                    let owner = if plays(&fg) {
                        Some(fg)
                    } else if focus {
                        None
                    } else {
                        clips.iter().find(plays)
                    };
                    if let Some(c) = owner
                        && let Some(w) = c.windows.iter().find(|w| contains(w, at))
                        && let Some(key) = keys_rows.key_at(grid, pos.y)
                    {
                        let key = key.round().clamp(0.0, 127.0) as u8;
                        invoke(
                            events,
                            ids::ADD_NOTE,
                            json!({
                                "clip": c.clip,
                                "key": key,
                                "start": w.from + (at - w.at),
                                "length": arrangement.grid_ticks(doc),
                            }),
                        );
                    }
                }
                (false, Some((clip, place, _))) => {
                    if clip != clips[foreground].clip {
                        invoke(events, ids::FOREGROUND, json!({ "clip": clip }));
                    }
                    invoke(
                        events,
                        ids::SELECT_NOTES,
                        json!({
                            "clip": clip,
                            "notes": [place],
                            "mode": if keys.toggle || keys.extend { "toggle" } else { "replace" },
                        }),
                    );
                }
                (false, None) => {
                    let fg = &clips[foreground];
                    invoke(
                        events,
                        ids::SELECT_NOTES,
                        json!({ "clips": [{ "clip": fg.clip, "notes": [] }], "mode": "replace" }),
                    );
                    invoke(
                        events,
                        "clip_view.set_insert_marker",
                        json!({ "clip": fg.clip, "beat": at - fg.shift() }),
                    );
                }
            }
        }
        response.context_menu(|ui| {
            self.menus.show(ui, shell, &context_menus::note(), events);
        });
        // The wheel and a middle-button drag scroll the key track.
        if ui.rect_contains_pointer(grid) {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if keys_rows.folded {
                    // Row 0 is the highest key: up shows the rows before.
                    self.first =
                        (self.first - scroll / KEY_HEIGHT).clamp(0.0, keys_rows.scroll_room());
                } else {
                    self.top = (self.top + scroll / KEY_HEIGHT).clamp(0.0, 127.0);
                }
            }
        }
        if let Some(delta) = crate::middle::pan(ui, grid) {
            if keys_rows.folded {
                self.first =
                    (self.first - delta.y / KEY_HEIGHT).clamp(0.0, keys_rows.scroll_room());
            } else {
                self.top = (self.top + delta.y / KEY_HEIGHT).clamp(0.0, 127.0);
            }
        }
    }
}

/// Whether the stretch `w` covers the axis tick `at`.
fn contains(w: &Window, at: Ticks) -> bool {
    at >= w.at && at < w.at + (w.to - w.from)
}

/// Where `note` starts on the axis, in the stretch nearest the press.
fn axis_start(c: &Shown, note: &Note, near: Ticks) -> Ticks {
    c.windows
        .iter()
        .filter(|w| note.start + note.length > w.from && note.start < w.to)
        .map(|w| w.at + (note.start.max(w.from) - w.from))
        .min_by_key(|at| (*at - near).abs())
        .unwrap_or(note.start + c.shift())
}

/// Where a clip's loop bar sits: its row, from where the clip plays first to
/// where it plays last (Arrangement), or from its first pass to its last
/// (Session).
fn bar_rect(
    geometry: &Geometry,
    grid: Rect,
    axis: (Ticks, Ticks),
    c: &Shown,
    session: bool,
) -> Rect {
    let row = geometry.rows[c.row];
    let (from, to) = if session {
        (
            c.windows.first().map_or(Ticks::ZERO, |w| w.at),
            c.windows.last().map_or(axis.1, |w| w.at + (w.to - w.from)),
        )
    } else {
        (c.clip_start, c.clip_start + c.length)
    };
    Rect::from_min_max(
        pos2(x_of_tick(grid, axis.0, axis.1, from), row.top() + 1.0),
        pos2(
            x_of_tick(grid, axis.0, axis.1, to).max(x_of_tick(grid, axis.0, axis.1, from) + 2.0),
            row.bottom() - 1.0,
        ),
    )
}

/// The x extents of a clip's loop brace on its bar: one for the Arrangement
/// clip, one for each pass of a Session clip.
fn braces(c: &Shown, session: bool, axis: (Ticks, Ticks), grid: Rect) -> Vec<(f32, f32)> {
    let x = |tick: Ticks| x_of_tick(grid, axis.0, axis.1, tick);
    if session {
        return c
            .windows
            .iter()
            .map(|w| (x(w.at), x(w.at + (w.to - w.from))))
            .collect();
    }
    let shift = c.shift();
    vec![(x(c.midi.loop_start + shift), x(c.loop_end() + shift))]
}

/// Lets an event from the note editor's keys reach every clip's selected
/// notes: the keys name the foreground clip, which a command with no clip
/// leaves out (R7, R18).
fn across(event: &mut UiEvent) {
    let UiEvent::Invoke(invocation) = event else {
        return;
    };
    let reaches = matches!(
        invocation.id.as_str(),
        ids::MOVE_NOTES | ids::RESIZE_NOTES | ids::ADJUST_VALUE
    ) || (invocation.id == ids::SELECT_NOTES
        && invocation.args.get("select").is_some());
    if reaches && let Some(fields) = invocation.args.as_object_mut() {
        fields.remove("clip");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_of_loop_bars_take_their_height_and_leave_the_note_area_the_rest() {
        let view = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 400.0));
        let one = geometry(view, 1, BAR_HEIGHT, &[]);
        let three = geometry(view, 3, BAR_HEIGHT, &[]);
        assert_eq!(one.rows.len(), 1);
        assert_eq!(three.rows.len(), 3);
        assert_eq!(three.rows[2].top(), three.rows[0].top() + 2.0 * BAR_HEIGHT);
        assert!((three.layout.grid.top() - one.layout.grid.top() - 2.0 * BAR_HEIGHT).abs() < 0.01);
        assert_eq!(three.layout.grid.top(), three.rows[2].bottom());
        // Taller bars push the note area down; the share of the view caps them.
        let tall = geometry(view, 3, 40.0, &[]);
        assert!(tall.rows[0].height() <= 40.0);
        assert!(
            tall.rows[2].bottom()
                <= view.top() + TOOLBAR_HEIGHT + TITLE_BAR_HEIGHT + 400.0 * BARS_SHARE + 0.01
        );
    }
}
