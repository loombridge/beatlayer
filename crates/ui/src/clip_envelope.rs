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
use bl_model::modulation::Travel;
use bl_model::{
    AutomationTarget, Breakpoint, Clip, ClipId, Document, Envelope, MidiData, MixerParam,
};
use bl_shell::{Area, GesturePhase, Invocation, Shell};
use egui::{
    Align2, Id, Key, Modifiers, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::arrangement::automation::{
    AutomationDevice, BREAKPOINT_GRAB, BREAKPOINT_SIZE, CURVE_PER_POINT, FINE_VALUE, LaneScale,
    SEGMENT_GRAB, belongs_to, control_choices, default_target, device_choices, device_of,
    mark_controls, mark_devices, target_label,
};
use crate::arrangement::editing::EditKeys;
use crate::clip_view::{grid_rect, paint_grid_lines, tick_at, x_of_tick};
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice};

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

/// The scale a clip's modulation envelope is drawn and edited on: shares of
/// the parameter's travel, 0 to 100 % for a level (Volume and sends) and
/// −100 % to 100 % for the rest, not the parameter's own units.
// Spec: modulation R9
pub(crate) fn share_scale(target: AutomationTarget) -> LaneScale {
    let travel = if is_level(target) {
        Travel::Fader
    } else {
        Travel::Pan
    };
    let (min, max) = travel.clip_range();
    LaneScale::Linear { min, max }
}

/// Whether a parameter is a level, whose modulation is a share of its base.
fn is_level(target: AutomationTarget) -> bool {
    matches!(
        target,
        AutomationTarget::Mixer {
            param: MixerParam::Volume
        } | AutomationTarget::Send { .. }
    )
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
    doc.check_automation_target(track, target).ok()?;
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
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(egui::accesskit::Role::Group);
            node.set_label("Clip Envelope".to_owned());
            node.set_value(name.clone());
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
                rest: if is_level(target) { 1.0 } else { 0.0 },
                envelope,
            };
            self.paint(ui, theme, &lane);
            self.input(ui, shell, doc, frame, target, &lane, &response, events);
        }
        self.footer(ui, shell, theme, frame, events);
    }

    fn paint(&self, ui: &Ui, theme: &Theme, lane: &Lane<'_>) {
        let painter = ui.painter_at(lane.rect);
        let colours = &theme.colors;
        // Spec: modulation R9: the left axis reads in percent.
        let low = lane.scale.value(0.0);
        let ticks = [(1.0, Align2::LEFT_TOP), (low, Align2::LEFT_BOTTOM)]
            .into_iter()
            .chain((low < 0.0).then_some((0.0, Align2::LEFT_CENTER)));
        for (value, align) in ticks {
            painter.text(
                pos2(lane.rect.left() + 2.0, lane.y_of_value(value)),
                align,
                format!("{:.0}%", value * 100.0),
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
                colours.text.0,
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
        let mut devices: Vec<Choice<AutomationDevice>> = track
            .map(|track| device_choices(doc, track))
            .unwrap_or_default()
            .into_iter()
            .filter(|choice| choice.value != AutomationDevice::None)
            .collect();
        // The device chooser's first choice reads "Mixer" (L-CLIP-017).
        if let Some(mixer) = devices.first_mut() {
            mixer.label = "Mixer".to_owned();
        }
        let mut controls = device
            .map(|device| control_choices(shell, doc, device))
            .unwrap_or_default();
        if let Some(track) = track {
            mark_devices(&mut devices, doc, track, theme);
            mark_controls(&mut controls, doc, track, theme);
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
            invoke(
                events,
                "view.clip_envelope",
                json!({ "target": default_target(shell, doc, chosen) }),
            );
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
