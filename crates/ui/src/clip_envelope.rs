//! The Clip View's Envelopes tab (WP-3.16): a clip's envelope for one of its
//! track's parameters, drawn over the clip's contents, with the device and
//! control choosers and the Linked switch in the row under it (L-CLIP-017).
//!
//! Every edit is one of the clip envelope commands (`clip.set_envelope_*`,
//! `clip.move_envelope_breakpoint`, `clip.delete_envelope_breakpoint`,
//! `clip.set_envelope_linked`), the same ones agents run, and the gestures
//! are the Arrangement's automation lanes': a click on a breakpoint deletes
//! it, one on a segment adds a breakpoint there, a double-click off the line
//! adds one, dragging a breakpoint moves it, and Option (Alt) with a drag on
//! a segment curves it. A drag sends its command on each frame as a
//! [`UiEvent::Gesture`], measured from where it began, so the whole drag is
//! one undo step. Which parameter shows is the shell's (`view.clip_envelope`).
//!
//! The time axis is the one the clip plays the envelope on (clip-envelopes
//! R2): a linked envelope runs from the clip's Start marker, as the notes do,
//! and an unlinked one from the clip's start. The behaviour is in the clip
//! envelopes spec.

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Breakpoint, Clip, ClipData, ClipId, Document, Envelope, MidiData,
};
use bl_shell::{Area, GesturePhase, Invocation, Shell};
use egui::{
    Align2, Id, Key, Modifiers, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::arrangement::automation::{
    AutomationDevice, BREAKPOINT_GRAB, BREAKPOINT_SIZE, CURVE_PER_POINT, FINE_VALUE, SEGMENT_GRAB,
    belongs_to, clip_choices, control_choices, default_target, device_choices, device_of,
    mark_controls, mark_devices, target_label,
};
use crate::arrangement::editing::EditKeys;
use crate::clip_view::{grid_rect, paint_grid_lines, tick_at, x_of_tick};
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice, NO_LEDS};
use bl_shell::arrangement::automation::Travel as LaneScale;

/// How tall the row under the editor is, in points: the device chooser, the
/// control chooser and the Linked switch (L-CLIP-017).
pub const FOOTER_HEIGHT: f32 = 16.0;
/// How wide the device chooser is, in points.
const DEVICE_WIDTH: f32 = 110.0;
/// How wide the control chooser is, in points.
const CONTROL_WIDTH: f32 = 150.0;
/// How wide the Linked switch is, in points.
const LINKED_WIDTH: f32 = 56.0;
/// The gap between the footer's controls, in points.
const GAP: f32 = 3.0;

pub(crate) use bl_shell::arrangement::automation::{rest_of, share_scale};

/// The device chooser's choices for a clip, in the order it lists them
/// (L-CLIP-017): an audio clip's sound settings or a MIDI clip's controllers
/// first, then the track's mixer and each device in its chain.
fn editor_devices(doc: &Document, clip: &Clip) -> Vec<Choice<AutomationDevice>> {
    let mut devices: Vec<Choice<AutomationDevice>> = doc
        .clip_track(clip.id)
        .map(|track| device_choices(doc, track))
        .unwrap_or_default()
        .into_iter()
        .filter(|choice| choice.value != AutomationDevice::None)
        .collect();
    // The device chooser's first choice reads "Mixer" (L-CLIP-017).
    if let Some(mixer) = devices.first_mut() {
        mixer.label = "Mixer".to_owned();
    }
    // An audio clip's sound settings come first (L-CLIP-017).
    // Spec: warp-modes R29
    if clip.data.is_audio() {
        devices.insert(
            0,
            Choice {
                label: "Clip".to_owned(),
                value: AutomationDevice::Clip,
                leds: NO_LEDS,
            },
        );
    }
    // A MIDI clip's controllers come first, before the Mixer.
    // Spec: midi-controller-data R8
    if !clip.data.is_audio() {
        devices.insert(
            0,
            Choice {
                label: "MIDI Ctrl".to_owned(),
                value: AutomationDevice::MidiCtrl,
                leds: NO_LEDS,
            },
        );
    }
    devices
}

/// The control chooser's choices for a clip's chosen device.
fn editor_controls(
    shell: &Shell,
    doc: &Document,
    clip: &Clip,
    device: AutomationDevice,
) -> Vec<Choice<AutomationTarget>> {
    match (device, &clip.data) {
        (AutomationDevice::Clip, ClipData::Audio(audio)) => clip_choices(audio),
        _ => control_choices(shell, doc, device),
    }
}

/// The invocation that shows the previous or next parameter in the Envelope
/// Editor (S-A11Y-028, S-A11Y-029): the one the clip has an envelope for, or
/// with `all`, any the choosers list, in the order they list them.
fn cycle(
    shell: &Shell,
    clip: &Clip,
    current: Option<AutomationTarget>,
    forward: bool,
    all: bool,
) -> Option<Invocation> {
    let doc = shell.editor().document();
    let list: Vec<AutomationTarget> = editor_devices(doc, clip)
        .into_iter()
        .flat_map(|device| editor_controls(shell, doc, clip, device.value))
        .map(|choice| choice.value)
        .collect();
    let target = crate::arrangement::automation::cycle_target(&list, current, forward, |target| {
        all || clip.envelope(target).is_some()
    })?;
    Some(Invocation::new(
        "view.clip_envelope",
        json!({ "target": target }),
    ))
}

/// The ID of the accessible node of a breakpoint in the Envelope Editor.
fn breakpoint_id(clip: ClipId, at: Ticks) -> Id {
    Id::new(("clip-envelope-breakpoint", clip, at.get()))
}

/// What a right-click in the editor offered (C-TIME-026).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    /// Edit Value, on a breakpoint.
    Edit(Ticks),
    /// Add Value, on the line, at a position.
    Add(Ticks),
}

/// The editor's area: the note area's width, down to the footer.
#[must_use]
pub fn lane_rect(view: Rect) -> Rect {
    let grid = grid_rect(view);
    let bottom = (view.bottom() - FOOTER_HEIGHT).max(grid.top());
    Rect::from_min_max(grid.min, pos2(grid.right(), bottom))
}

/// The row under the editor, which holds the choosers and the Linked switch.
#[must_use]
pub fn footer_rect(view: Rect) -> Rect {
    Rect::from_min_max(pos2(view.left(), view.bottom() - FOOTER_HEIGHT), view.max)
}

/// The parameter whose envelope the tab shows: the shell's choice while the
/// clip's track still has it.
#[must_use]
pub fn shown_target(shell: &Shell, doc: &Document, clip: ClipId) -> Option<AutomationTarget> {
    let target = shell.clip_view().envelope()?;
    let track = doc.clip_track(clip)?;
    doc.check_clip_target(track, doc.clip(clip)?, target).ok()?;
    Some(target)
}

/// The contents tick at the editor's left edge: the Start marker for a
/// linked envelope, which plays what the clip's contents play, and the
/// clip's start for an unlinked one, which runs once from there (R2).
#[must_use]
pub fn axis_start(clip: &Clip, midi: &MidiData, target: Option<AutomationTarget>) -> Ticks {
    match target.and_then(|target| clip.envelope(&target)) {
        Some(envelope) if !envelope.linked => Ticks::ZERO,
        _ => midi.start,
    }
}

/// The ID of the editor's accessible node for a clip.
#[must_use]
pub fn lane_id(clip: ClipId) -> Id {
    Id::new(("clip-envelope-lane", clip))
}

/// The ID of the Linked switch for a clip.
#[must_use]
pub fn linked_id(clip: ClipId) -> Id {
    Id::new(("clip-envelope-linked", clip))
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, id: &str, args: Value) {
    events.push(UiEvent::Gesture {
        phase,
        invocation: Some(Invocation::new(id, args)),
    });
}

/// What is under the pointer in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    /// A breakpoint at this position.
    Breakpoint(Ticks),
    /// A segment starting at the breakpoint at this position.
    Segment(Ticks),
    /// The background.
    Empty,
}

/// The editor on screen: its area, the axis, and the parameter's scale.
struct Lane<'a> {
    rect: Rect,
    start: Ticks,
    length: Ticks,
    scale: LaneScale,
    /// Whether the envelope is a MIDI controller's, in the controller's own
    /// units rather than shares.
    midi: bool,
    /// The share that modulates nothing: 100 % for a level, 0 for the rest.
    rest: f64,
    envelope: Option<&'a Envelope>,
}

impl Lane<'_> {
    /// The contents position under a point, 0 or later.
    fn at(&self, pos: Pos2) -> Ticks {
        tick_at(self.rect, self.start, self.length, pos.x).max(Ticks::ZERO)
    }

    /// The value at a height.
    fn value_at(&self, y: f32) -> f64 {
        let t = ((self.rect.bottom() - y) / self.rect.height()).clamp(0.0, 1.0);
        self.scale.value(f64::from(t))
    }

    /// The height of a value.
    fn y_of_value(&self, value: f64) -> f32 {
        let t = self.scale.position(value).clamp(0.0, 1.0);
        self.rect.bottom() - (t as f32) * self.rect.height()
    }

    /// Where a breakpoint sits on screen.
    fn point_pos(&self, point: Breakpoint) -> Pos2 {
        pos2(
            x_of_tick(self.rect, self.start, self.length, point.at),
            self.y_of_value(point.value),
        )
    }

    fn hit(&self, pos: Pos2) -> Hit {
        let Some(envelope) = self.envelope else {
            return Hit::Empty;
        };
        if let Some(point) = envelope
            .points()
            .find(|point| self.point_pos(*point).distance(pos) <= BREAKPOINT_GRAB)
        {
            return Hit::Breakpoint(point.at);
        }
        let at = self.at(pos);
        if let Some(value) = envelope.value_at(at)
            && (self.y_of_value(value) - pos.y).abs() <= SEGMENT_GRAB
            && let Some(point) = envelope.points().take_while(|point| point.at <= at).last()
        {
            return Hit::Segment(point.at);
        }
        Hit::Empty
    }
}

/// A drag in the editor.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Moving a breakpoint to another position and value.
    Breakpoint {
        /// Where it was when the drag began.
        at: Ticks,
        /// The value it had then, for finer (Shift) moves.
        value: f64,
        /// Where the pointer went down.
        from: Pos2,
    },
    /// Curving the segment that starts at a breakpoint.
    Curve {
        /// The breakpoint the segment starts at.
        at: Ticks,
        /// Its bend when the drag began.
        bend: f64,
        /// Where the pointer went down.
        from: Pos2,
    },
}

/// The drag's command for the pointer at `pos`, measured from where the
/// drag began.
#[expect(clippy::too_many_arguments, reason = "the drag's whole geometry")]
fn drag_command(
    shell: &Shell,
    doc: &Document,
    clip: ClipId,
    target: AutomationTarget,
    lane: &Lane<'_>,
    drag: Drag,
    pos: Pos2,
    modifiers: Modifiers,
) -> (&'static str, Value) {
    match drag {
        Drag::Breakpoint { at, value, from } => {
            let by = tick_at(lane.rect, lane.start, lane.length, pos.x)
                - tick_at(lane.rect, lane.start, lane.length, from.x);
            let to = shell
                .arrangement()
                .snap_drag(doc, at + by, EditKeys::from(modifiers).bypass)
                .max(Ticks::ZERO);
            let raw = lane.value_at(pos.y);
            // S-ENV-002: Shift drags the value in finer steps.
            let value = if modifiers.shift {
                value + (raw - value) * FINE_VALUE
            } else {
                raw
            };
            (
                "clip.move_envelope_breakpoint",
                json!({ "clip": clip, "target": target, "at": at, "to": to, "value": value }),
            )
        }
        Drag::Curve { at, bend, from } => {
            let bend = (bend + f64::from(from.y - pos.y) * CURVE_PER_POINT).clamp(-1.0, 1.0);
            (
                "clip.set_envelope_curve",
                json!({ "clip": clip, "target": target, "at": at, "bend": bend }),
            )
        }
    }
}

/// The Envelopes tab's state between frames: the drag under way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvelopeEditor {
    drag: Option<Drag>,
    /// What the last right-click offered.
    menu: Option<Menu>,
}

/// What the editor draws and edits in a frame.
pub struct Frame<'a> {
    /// The Clip View's rectangle.
    pub view: Rect,
    /// The clip.
    pub clip: &'a Clip,
    /// What the clip plays.
    pub midi: &'a MidiData,
    /// The parameter whose envelope shows, if one is chosen.
    pub target: Option<AutomationTarget>,
}

impl EnvelopeEditor {
    /// Forgets a drag, as another clip showing does.
    pub fn reset(&mut self) {
        self.drag = None;
        self.menu = None;
    }

    /// Draws the editor and its footer, and turns the pointer's gestures
    /// into commands.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &Frame<'_>,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let clip = frame.clip;
        let rect = lane_rect(frame.view);
        let response = ui.interact(rect, lane_id(clip.id), Sense::click_and_drag());
        let name = frame
            .target
            .map_or_else(|| "None".to_owned(), |t| target_label(shell, doc, t));
        // Spec: automation-editing R20 to R28. While the Envelope Editor has
        // the keyboard, the insert marker, the focused and selected
        // breakpoints and the value at the marker are drawn and read.
        let keys = shell
            .keyboard_envelope()
            .filter(|keys| keys.envelope == bl_shell::envelope_commands::EnvelopeKeys::Clip)
            .filter(|_| {
                shell.focused_area_of(shell.acting_window()) == Area::Detail
                    && frame.target.is_some()
            });
        let mut focused = None;
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(egui::accesskit::Role::Group);
            node.set_label("Clip Envelope".to_owned());
            node.set_value(match &keys {
                Some(keys) => format!("{name}, {}", keys.value),
                None => name.clone(),
            });
        });
        let start = axis_start(clip, frame.midi, frame.target);
        paint_grid_lines(ui, shell, theme, rect, start, clip.length);
        let envelope = frame
            .target
            .and_then(|target| clip.envelope(&target))
            .map(|clip_envelope| &clip_envelope.envelope);
        if let Some(target) = frame.target
            && rect.height() >= 1.0
        {
            let lane = Lane {
                rect,
                start,
                length: clip.length,
                scale: share_scale(target),
                midi: target.is_midi() || target.is_clip(),
                rest: rest_of(target),
                envelope,
            };
            let time = ui.input(|i| i.time);
            self.paint(ui, theme, &lane, keys.as_ref(), time);
            if let Some(keys) = &keys {
                focused = self.paint_keys(ui, shell, theme, frame, &lane, keys);
                self.read_keys(ui, shell, frame, &response, keys, events);
            }
            self.input(ui, shell, doc, frame, target, &lane, &response, events);
            self.context_menu(shell, frame, target, &lane, &response, events);
        }
        if let Some(focused) = focused {
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_active_descendant(focused.accesskit_id());
            });
        }
        self.footer(ui, shell, theme, frame, events);
    }

    /// Draws the insert marker with the value beside it, or what is being
    /// typed, and the node of the focused or selected breakpoint, which is
    /// returned for the editor's group to name as its active descendant
    /// (R21, R28).
    fn paint_keys(
        &self,
        ui: &Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &Frame<'_>,
        lane: &Lane<'_>,
        keys: &bl_shell::envelope_commands::EnvelopeKeyState,
    ) -> Option<Id> {
        let painter = ui.painter_at(lane.rect);
        let typed = crate::envelope_keys::typed(ui.ctx());
        let x = x_of_tick(lane.rect, lane.start, lane.length, keys.marker);
        if (lane.rect.left()..=lane.rect.right()).contains(&x) {
            crate::envelope_keys::paint_marker(
                &painter,
                theme,
                lane.rect,
                x,
                typed.as_ref().map_or(&keys.value, |typed| &typed.text),
                typed.is_some(),
            );
        }
        if keys.selected.is_some() {
            // The selected breakpoint pulses.
            ui.ctx()
                .request_repaint_after(crate::envelope_keys::PULSE_FRAME);
        }
        let at = keys.selected.unwrap_or(keys.marker);
        let point = lane.envelope?.point(at)?;
        let id = breakpoint_id(frame.clip.id, at);
        let rect = Rect::from_center_size(
            lane.point_pos(point),
            vec2(BREAKPOINT_SIZE * 2.0, BREAKPOINT_SIZE * 2.0),
        );
        crate::envelope_keys::describe_breakpoint(
            ui,
            id,
            rect,
            shell
                .editor()
                .document()
                .settings()
                .time_signatures
                .to_bbs(at)
                .to_string(),
            shell.keyboard_breakpoint_text(at).unwrap_or_default(),
            keys.selected == Some(at),
        );
        Some(id)
    }

    /// Reads the keys the keymap leaves to the editor while it has the
    /// keyboard (S-A11Y-018 to S-A11Y-030).
    fn read_keys(
        &self,
        ui: &Ui,
        shell: &Shell,
        frame: &Frame<'_>,
        response: &egui::Response,
        keys: &bl_shell::envelope_commands::EnvelopeKeyState,
        events: &mut Vec<UiEvent>,
    ) {
        // A chooser or the Linked switch with the keyboard keeps its arrows.
        let focus = ui.ctx().memory(egui::Memory::focused);
        if ui.ctx().text_edit_focused()
            || focus.is_some_and(|id| id != response.id)
            || !shell
                .active_contexts()
                .contains(&bl_commands::Context::Envelope)
        {
            return;
        }
        let current = frame.target;
        crate::envelope_keys::read(
            ui,
            keys,
            |forward, all| cycle(shell, frame.clip, current, forward, all),
            events,
        );
    }

    /// Right-clicking a breakpoint offers Edit Value, and right-clicking the
    /// line, Add Value (C-TIME-026; the automation editing spec, R27): each
    /// types a value, which Return sets.
    fn context_menu(
        &mut self,
        shell: &Shell,
        frame: &Frame<'_>,
        target: AutomationTarget,
        lane: &Lane<'_>,
        response: &egui::Response,
        events: &mut Vec<UiEvent>,
    ) {
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.menu = match lane.hit(pos) {
                Hit::Breakpoint(at) => Some(Menu::Edit(at)),
                Hit::Segment(_) => Some(Menu::Add(shell.arrangement().snap_drag(
                    shell.editor().document(),
                    lane.at(pos),
                    false,
                ))),
                Hit::Empty => None,
            };
        }
        let Some(menu) = self.menu else {
            return;
        };
        let clip = frame.clip.id;
        let select = |at: Option<Ticks>| {
            (
                bl_shell::envelope_commands::SELECT_ENVELOPE_BREAKPOINT.to_owned(),
                json!({ "clip": clip, "target": target, "at": at }),
            )
        };
        response.context_menu(|ui| match menu {
            Menu::Edit(at) => {
                if ui.button("Edit Value").clicked() {
                    let (id, args) = select(Some(at));
                    invoke(events, &id, args);
                    crate::envelope_keys::set_typed(
                        ui.ctx(),
                        Some(crate::envelope_keys::Typed {
                            text: String::new(),
                            add_at: None,
                            release: Some(select(None)),
                        }),
                    );
                    ui.close();
                }
            }
            Menu::Add(at) => {
                if ui.button("Add Value").clicked() {
                    crate::envelope_keys::set_typed(
                        ui.ctx(),
                        Some(crate::envelope_keys::Typed {
                            text: String::new(),
                            add_at: Some(at),
                            release: None,
                        }),
                    );
                    ui.close();
                }
            }
        });
    }

    fn paint(
        &self,
        ui: &Ui,
        theme: &Theme,
        lane: &Lane<'_>,
        keys: Option<&bl_shell::envelope_commands::EnvelopeKeyState>,
        time: f64,
    ) {
        let painter = ui.painter_at(lane.rect);
        let colours = &theme.colors;
        // Spec: modulation R9: the left axis reads in percent, and a MIDI
        // controller's in its own numbers.
        let (low, high) = (lane.scale.value(0.0), lane.scale.value(1.0));
        let ticks = [(high, Align2::LEFT_TOP), (low, Align2::LEFT_BOTTOM)]
            .into_iter()
            .chain((low < 0.0).then_some((0.0, Align2::LEFT_CENTER)));
        for (value, align) in ticks {
            painter.text(
                pos2(lane.rect.left() + 2.0, lane.y_of_value(value)),
                align,
                if lane.midi {
                    format!("{value:.0}")
                } else {
                    format!("{:.0}%", value * 100.0)
                },
                theme.small_font(),
                colours.text_dim.0,
            );
        }
        let Some(envelope) = lane.envelope.filter(|envelope| !envelope.is_empty()) else {
            // With no envelope drawn the lane shows a dashed red line
            // (L-CLIP-017), where nothing modulates.
            let y = lane.y_of_value(lane.rest);
            painter.add(Shape::dashed_line(
                &[pos2(lane.rect.left(), y), pos2(lane.rect.right(), y)],
                Stroke::new(1.0, colours.record.0),
                4.0,
                3.0,
            ));
            return;
        };
        let mut points: Vec<Pos2> = Vec::new();
        let mut x = lane.rect.left();
        while x <= lane.rect.right() {
            let at = tick_at(lane.rect, lane.start, lane.length, x);
            if let Some(value) = envelope.value_at(at) {
                points.push(pos2(x, lane.y_of_value(value)));
            }
            x += 2.0;
        }
        if points.len() >= 2 {
            painter.add(Shape::line(points, Stroke::new(1.5, colours.modulation.0)));
        }
        for point in envelope.points() {
            painter.rect_filled(
                Rect::from_center_size(
                    lane.point_pos(point),
                    vec2(BREAKPOINT_SIZE, BREAKPOINT_SIZE),
                ),
                0.0,
                keys.and_then(|keys| {
                    crate::envelope_keys::breakpoint_colour(theme, point.at, keys, time)
                })
                .unwrap_or(colours.text.0),
            );
        }
    }

    #[expect(clippy::too_many_arguments, reason = "the editor's whole frame")]
    fn input(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        doc: &Document,
        frame: &Frame<'_>,
        target: AutomationTarget,
        lane: &Lane<'_>,
        response: &egui::Response,
        events: &mut Vec<UiEvent>,
    ) {
        let clip = frame.clip.id;
        let modifiers = ui.input(|i| i.modifiers);
        let keys = EditKeys::from(modifiers);
        let snap = |pos: Pos2| {
            shell
                .arrangement()
                .snap_drag(doc, lane.at(pos), keys.bypass)
        };
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            self.drag = match lane.hit(press) {
                Hit::Breakpoint(at) => Some(Drag::Breakpoint {
                    at,
                    value: lane
                        .envelope
                        .and_then(|envelope| envelope.point(at))
                        .map_or(0.0, |point| point.value),
                    from: press,
                }),
                Hit::Segment(at) if modifiers.alt => Some(Drag::Curve {
                    at,
                    bend: lane
                        .envelope
                        .and_then(|envelope| envelope.point(at))
                        .map_or(0.0, |point| point.bend),
                    from: press,
                }),
                _ => None,
            };
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        let step = |drag: Drag, pos: Pos2| {
            drag_command(shell, doc, clip, target, lane, drag, pos, modifiers)
        };
        if response.dragged()
            && let (Some(drag), Some(pos)) = (self.drag, pointer)
        {
            let (id, args) = step(drag, pos);
            gesture(events, GesturePhase::Update, id, args);
        }
        if self.drag.is_some() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        if response.drag_stopped() {
            let last = match (self.drag, pointer) {
                (Some(drag), Some(pos)) => {
                    let (id, args) = step(drag, pos);
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
        }
        let Some(pos) = response.interact_pointer_pos() else {
            return;
        };
        let value_on_line = |at: Ticks| {
            lane.envelope
                .and_then(|envelope| envelope.value_at(at))
                .unwrap_or_else(|| lane.value_at(pos.y))
        };
        let breakpoint = |at: Ticks, value: f64| {
            (
                "clip.set_envelope_breakpoint",
                json!({ "clip": clip, "target": target, "at": at, "value": value }),
            )
        };
        let command = if response.double_clicked() {
            match lane.hit(pos) {
                // Option (Alt) double-click straightens the segment.
                Hit::Segment(at) if modifiers.alt => Some((
                    "clip.set_envelope_curve",
                    json!({ "clip": clip, "target": target, "at": at, "bend": 0.0 }),
                )),
                Hit::Empty => Some(breakpoint(snap(pos), lane.value_at(pos.y))),
                _ => None,
            }
        } else if response.clicked() {
            match lane.hit(pos) {
                // Spec: automation-editing R22 (S-A11Y-021). Option-click
                // (Alt-click) selects the breakpoint and puts the insert
                // marker on it.
                Hit::Breakpoint(at) if modifiers.alt => Some((
                    bl_shell::envelope_commands::SELECT_ENVELOPE_BREAKPOINT,
                    json!({ "clip": clip, "target": target, "at": at }),
                )),
                Hit::Breakpoint(at) => Some((
                    "clip.delete_envelope_breakpoint",
                    json!({ "clip": clip, "target": target, "at": at }),
                )),
                // With Option (Alt) held the segment is being curved.
                Hit::Segment(_) if !modifiers.alt => {
                    let at = snap(pos);
                    Some(breakpoint(at, value_on_line(at)))
                }
                _ => None,
            }
        } else {
            None
        };
        if let Some((id, args)) = command {
            invoke(events, id, args);
        }
    }

    /// The row under the editor: the device and control choosers and the
    /// Linked switch.
    fn footer(
        &self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &Frame<'_>,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let clip = frame.clip;
        let footer = footer_rect(frame.view);
        let device_rect = Rect::from_min_size(
            pos2(footer.left() + GAP, footer.top()),
            vec2(DEVICE_WIDTH, footer.height()),
        );
        let control_rect = Rect::from_min_size(
            pos2(device_rect.right() + GAP, footer.top()),
            vec2(CONTROL_WIDTH, footer.height()),
        );
        let linked_rect = Rect::from_min_size(
            pos2(control_rect.right() + GAP, footer.top()),
            vec2(LINKED_WIDTH, footer.height()),
        );
        let track = doc.clip_track(clip.id);
        let device = frame.target.map(device_of);
        let mut devices = editor_devices(doc, clip);
        let mut controls = device
            .map(|device| editor_controls(shell, doc, clip, device))
            .unwrap_or_default();
        if let Some(track) = track {
            mark_devices(&mut devices, doc, track, theme);
            mark_controls(&mut controls, doc, track, theme);
        }
        // A controller is marked by the clip's own envelopes: a dot on each
        // control that has one.
        let dot = |has: bool| [None, has.then_some(theme.colors.modulation.0)];
        for choice in &mut controls {
            if choice.value.is_midi() || choice.value.is_clip() {
                choice.leds = dot(clip.envelope(&choice.value).is_some());
            }
        }
        if let Some(midi) = devices
            .iter_mut()
            .find(|choice| choice.value == AutomationDevice::MidiCtrl)
        {
            midi.leds = dot(clip.envelopes().any(|envelope| envelope.target.is_midi()));
        }
        if let Some(settings) = devices
            .iter_mut()
            .find(|choice| choice.value == AutomationDevice::Clip)
        {
            settings.leds = dot(clip.envelopes().any(|envelope| envelope.target.is_clip()));
        }
        let current_control = frame
            .target
            .filter(|target| device.is_some_and(|device| belongs_to(*target, device)));
        for salt in ["clip-envelope-device", "clip-envelope-control"] {
            crate::focus::register(ui.ctx(), Id::new(("chooser", salt)), Area::Detail, None);
        }
        if let Some(chosen) = widgets::chooser(
            ui,
            theme,
            "clip-envelope-device",
            "Envelope Device",
            device_rect,
            &devices,
            device.as_ref(),
            true,
        ) {
            // MIDI Ctrl shows the first control that has an envelope, else
            // Pitch Bend (midi-controller-data R8).
            let target = if chosen == AutomationDevice::MidiCtrl {
                control_choices(shell, doc, chosen)
                    .into_iter()
                    .map(|choice| choice.value)
                    .find(|target| clip.envelope(target).is_some())
                    .or_else(|| default_target(shell, doc, chosen))
            } else {
                default_target(shell, doc, chosen)
            };
            invoke(events, "view.clip_envelope", json!({ "target": target }));
        }
        if let Some(chosen) = widgets::chooser(
            ui,
            theme,
            "clip-envelope-control",
            "Envelope Control",
            control_rect,
            &controls,
            current_control.as_ref(),
            device.is_some(),
        ) {
            invoke(events, "view.clip_envelope", json!({ "target": chosen }));
        }
        let linked = frame
            .target
            .and_then(|target| clip.envelope(&target))
            .is_none_or(|envelope| envelope.linked);
        if linked_switch(
            ui,
            theme,
            clip.id,
            linked_rect,
            linked,
            frame.target.is_some(),
        ) && let Some(target) = frame.target
        {
            invoke(
                events,
                "clip.set_envelope_linked",
                json!({ "clip": clip.id, "target": target, "linked": !linked }),
            );
        }
    }
}

/// The Linked switch (L-CLIP-017): on by default. Returns whether it was
/// clicked, or Enter pressed on it.
fn linked_switch(
    ui: &mut Ui,
    theme: &Theme,
    clip: ClipId,
    rect: Rect,
    on: bool,
    enabled: bool,
) -> bool {
    let id = linked_id(clip);
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let pressed = enabled
        && (response.clicked()
            || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))));
    let colours = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    let face = if on {
        colours.accent.0
    } else {
        colours.control.0
    };
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if enabled {
            face
        } else {
            face.gamma_multiply(0.55)
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Linked",
        theme.font(),
        if on {
            colours.accent_text.0
        } else {
            colours.text.0
        },
    );
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, colours.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, "Linked"));
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: midi-controller-data R8
    #[test]
    fn pitch_bend_is_drawn_centred_and_the_others_from_the_bottom() {
        let bend = share_scale(AutomationTarget::MidiPitchBend);
        assert!((bend.position(0.0) - 0.5).abs() < 0.001);
        assert_eq!(bend.position(-8192.0), 0.0);
        assert_eq!(bend.position(8191.0), 1.0);
        let wheel = share_scale(AutomationTarget::MidiCc { number: 1 });
        assert_eq!(wheel.position(0.0), 0.0);
        assert_eq!(wheel.position(127.0), 1.0);
    }
}
