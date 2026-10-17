//! The Arrangement's automation lanes (WP-2.12, part 2): drawing a track's
//! envelope, its breakpoints and its segments, the device and parameter
//! choosers that pick the parameter it shows, and the mouse edits that draw,
//! move, curve and delete breakpoints.
//!
//! Every edit becomes one of automation's registered commands
//! (`automation.set_breakpoint`, `automation.set_curve`,
//! `automation.move_breakpoint`, `automation.delete_breakpoint`), the same
//! ones agents run. A drag sends its command on each frame as a
//! [`UiEvent::Gesture`], measured from where the drag began, and the host
//! runs the frames as one transaction, so the whole drag is one undo step
//! (the automation spec, R13).
//!
//! The behaviour is in the automation spec.

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Breakpoint, Device, DeviceId, Document, Envelope, MixerParam, TrackId,
};
use bl_shell::arrangement::rows::Row;
use bl_shell::arrangement::{ArrangementView, Horizontal};
use bl_shell::arrangement_commands as view_ids;
use bl_shell::{GesturePhase, Invocation, Shell, mixer_controls};
use egui::{Modifiers, Painter, Pos2, Rect, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice};

/// How close to a breakpoint, in points, a click deletes it or a press drags
/// it.
pub(crate) const BREAKPOINT_GRAB: f32 = 5.0;

/// How close to a segment, in points, a click adds a breakpoint to it or an
/// Alt/Option press curves it.
pub(crate) const SEGMENT_GRAB: f32 = 4.0;

/// The size of a breakpoint's square, in points.
pub(crate) const BREAKPOINT_SIZE: f32 = 5.0;

/// How far Option/Alt drags a segment, per point, to move its bend: the
/// bend's own law is Beatlayer's (the automation spec, R12), because Live's
/// isn't measured, and so is this gesture's mapping to it.
pub(crate) const CURVE_PER_POINT: f64 = 0.02;

/// How much Shift scales a breakpoint drag's value: finer steps
/// (shortcuts.toml, S-ENV-002).
pub(crate) const FINE_VALUE: f64 = 0.2;

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, id: &str, args: Value) {
    events.push(UiEvent::Gesture {
        phase,
        invocation: Some(Invocation::new(id, args)),
    });
}

/// A track's automation lane on screen: its rectangle, the parameter it
/// shows, and the mapping between the parameter's values and the lane's
/// vertical axis.
#[derive(Clone, Copy, Debug)]
pub(super) struct Lane<'a> {
    pub doc: &'a Document,
    pub track: TrackId,
    pub target: AutomationTarget,
    pub rect: Rect,
    pub horizontal: Horizontal,
    pub scale: LaneScale,
}

impl Lane<'_> {
    /// The track's envelope for the parameter, if it has one.
    fn envelope(&self) -> Option<&Envelope> {
        self.doc
            .track(self.track)?
            .automation_for(&self.target)
            .map(|automation| &automation.envelope)
    }

    /// The position under a point.
    fn at(&self, pos: Pos2) -> Ticks {
        self.horizontal.at(f64::from(pos.x - self.rect.left()))
    }

    /// The value at a point's height.
    fn value_at(&self, pos: Pos2) -> f64 {
        let t = ((self.rect.bottom() - pos.y) / self.rect.height()).clamp(0.0, 1.0);
        self.scale.value(f64::from(t))
    }

    /// The height of a value.
    fn y_of_value(&self, value: f64) -> f32 {
        let t = self.scale.position(value).clamp(0.0, 1.0);
        self.rect.bottom() - (t as f32) * self.rect.height()
    }

    /// The point a breakpoint sits at, on screen.
    fn point_pos(&self, point: Breakpoint) -> Pos2 {
        pos2(
            self.rect.left() + self.horizontal.x(point.at) as f32,
            self.y_of_value(point.value),
        )
    }

    /// What is under a point: a breakpoint, a segment, or the background.
    pub(super) fn hit(&self, pos: Pos2) -> LaneHit {
        let at = self.at(pos);
        if let Some(envelope) = self.envelope() {
            if let Some(point) = envelope
                .points()
                .find(|point| self.point_pos(*point).distance(pos) <= BREAKPOINT_GRAB)
            {
                return LaneHit::Breakpoint(point.at);
            }
            if let Some(value) = envelope.value_at(at)
                && (self.y_of_value(value) - pos.y).abs() <= SEGMENT_GRAB
            {
                let start = envelope
                    .points()
                    .take_while(|point| point.at <= at)
                    .last()
                    .map(|point| point.at);
                if let Some(start) = start {
                    return LaneHit::Segment(start);
                }
            }
        }
        LaneHit::Empty(at)
    }
}

/// What is under the pointer in an automation lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaneHit {
    /// A breakpoint at this position.
    Breakpoint(Ticks),
    /// A segment starting at the breakpoint at this position.
    Segment(Ticks),
    /// The lane's background, at this position.
    Empty(Ticks),
}

/// The lane's rectangle on screen, under a track's row.
pub(super) fn lane_rect(row: &Row, lanes: Rect, scroll_y: f32) -> Rect {
    let top = lanes.top() + row.top + row.height - scroll_y;
    Rect::from_min_max(
        pos2(lanes.left(), top),
        pos2(lanes.right(), top + row.automation),
    )
}

/// The ID of a track's automation lane in the accessibility tree.
#[must_use]
pub(super) fn lane_id(track: TrackId) -> egui::Id {
    egui::Id::new(("arrangement-automation-lane", track))
}

/// The lanes in a frame: the rows the shell gave, and the geometry to look a
/// lane up.
pub(super) struct Lanes<'a> {
    pub view: &'a ArrangementView,
    pub doc: &'a Document,
    pub rect: Rect,
    pub horizontal: Horizontal,
    pub rows: &'a [Row],
    pub scroll_y: f32,
}

impl Lanes<'_> {
    /// The automation lane of `row`, if the row has one and its track shows a
    /// parameter.
    pub(super) fn lane(&self, row: &Row, shell: &Shell) -> Option<Lane<'_>> {
        if !self.view.automation_mode() || row.automation <= 0.0 {
            return None;
        }
        let target = self.view.automation_lane(row.track)?;
        Some(Lane {
            doc: self.doc,
            track: row.track,
            target,
            rect: lane_rect(row, self.rect, self.scroll_y),
            horizontal: self.horizontal,
            scale: lane_scale(shell, self.doc, target),
        })
    }

    /// The row a point is in.
    pub(super) fn row_at(&self, pos: Pos2) -> Option<&Row> {
        if !self.rect.contains(pos) {
            return None;
        }
        let y = pos.y - self.rect.top() + self.scroll_y;
        let index = bl_shell::arrangement::rows::first_row_at(self.rows, y);
        let row = self.rows.get(index)?;
        (y < row.bottom()).then_some(row)
    }

    /// The automation lane under a point, if any.
    pub(super) fn lane_at(&self, pos: Pos2, shell: &Shell) -> Option<Lane<'_>> {
        let row = self.row_at(pos)?;
        let y = pos.y - self.rect.top() + self.scroll_y;
        if y < row.top + row.height {
            return None;
        }
        self.lane(row, shell)
    }
}

/// How a parameter's values map onto its lane's vertical axis: the axis Live
/// draws it on, so a fader's useful range isn't squashed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LaneScale {
    /// A track's volume fader, which is dB-like: [`mixer_controls`]'s travel.
    Volume,
    /// A send level, whose travel matches the faders'.
    Send,
    /// Pan, from −1 to 1.
    Pan,
    /// A switch or radio button, whose axis is discrete (0 or 1).
    Switch,
    /// A device parameter, from its descriptor's low value to its high one.
    Linear { min: f64, max: f64 },
}

impl LaneScale {
    /// Where a value sits on the axis, from 0 (the bottom) to 1 (the top).
    pub(crate) fn position(self, value: f64) -> f64 {
        let t = match self {
            Self::Volume => mixer_controls::fader_position(value),
            Self::Send => mixer_controls::send_position(value),
            Self::Pan => (value + 1.0) / 2.0,
            Self::Switch => value,
            Self::Linear { min, max } => {
                if max > min {
                    (value - min) / (max - min)
                } else {
                    0.5
                }
            }
        };
        t.clamp(0.0, 1.0)
    }

    /// The value at a place on the axis, from 0 (the bottom) to 1 (the top).
    pub(crate) fn value(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Volume => mixer_controls::fader_gain(t),
            Self::Send => mixer_controls::send_level(t),
            Self::Pan => t * 2.0 - 1.0,
            Self::Switch => t.round(),
            Self::Linear { min, max } => min + (max - min) * t,
        }
    }
}

/// The scale a parameter's lane draws on.
pub(crate) fn lane_scale(shell: &Shell, doc: &Document, target: AutomationTarget) -> LaneScale {
    match target {
        AutomationTarget::Mixer {
            param: MixerParam::Volume,
        } => LaneScale::Volume,
        AutomationTarget::Mixer {
            param: MixerParam::Pan,
        } => LaneScale::Pan,
        AutomationTarget::Mixer {
            param: MixerParam::Mute,
        } => LaneScale::Switch,
        AutomationTarget::Send { .. } => LaneScale::Send,
        AutomationTarget::Device { device, param } => {
            let range = doc
                .device(device)
                .and_then(|device| shell.devices().description(&device.kind))
                .and_then(|description| description.param(param))
                .map(|descriptor| (descriptor.min(), descriptor.max()));
            match range {
                Some((min, max)) => LaneScale::Linear { min, max },
                None => LaneScale::Linear { min: 0.0, max: 1.0 },
            }
        }
    }
}

/// The device chooser's options: the track mixer, each device in the track's
/// chain, and None to show no envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationDevice {
    /// The track's mixer: volume, pan, the Track Activator, and its sends.
    Mixer,
    /// One of the track's devices.
    Device(DeviceId),
    /// No envelope: the lane stays, with nothing drawn.
    None,
}

/// The device a target belongs to, as the device chooser sees it.
pub(crate) fn device_of(target: AutomationTarget) -> AutomationDevice {
    match target {
        AutomationTarget::Mixer { .. } | AutomationTarget::Send { .. } => AutomationDevice::Mixer,
        AutomationTarget::Device { device, .. } => AutomationDevice::Device(device),
    }
}

/// A device's title, or "Device" if it hasn't been given one.
fn device_title(device: &Device) -> String {
    let title = device.title();
    if title.is_empty() {
        "Device".to_owned()
    } else {
        title.to_owned()
    }
}

/// The device chooser's choices for a track.
pub fn device_choices(doc: &Document, track: TrackId) -> Vec<Choice<AutomationDevice>> {
    let mut choices = vec![Choice {
        label: "Track Mixer".to_owned(),
        value: AutomationDevice::Mixer,
    }];
    if let Some(track) = doc.track(track) {
        choices.extend(track.devices().map(|device| Choice {
            label: device_title(device),
            value: AutomationDevice::Device(device.id),
        }));
    }
    choices.push(Choice {
        label: "None".to_owned(),
        value: AutomationDevice::None,
    });
    choices
}

/// The parameter chooser's choices for a track's chosen device: the mixer's
/// controls and sends, or a device's automatable parameters.
pub fn control_choices(
    shell: &Shell,
    doc: &Document,
    device: AutomationDevice,
) -> Vec<Choice<AutomationTarget>> {
    let mut choices = Vec::new();
    match device {
        AutomationDevice::Mixer => {
            for (label, param) in [
                ("Volume", MixerParam::Volume),
                ("Pan", MixerParam::Pan),
                ("Track Activator", MixerParam::Mute),
            ] {
                choices.push(Choice {
                    label: label.to_owned(),
                    value: AutomationTarget::Mixer { param },
                });
            }
            for return_track in doc.returns() {
                let name = doc
                    .track(return_track)
                    .map_or_else(String::new, |track| track.name().to_owned());
                let label = if name.is_empty() {
                    "Send".to_owned()
                } else {
                    format!("Send {name}")
                };
                choices.push(Choice {
                    label,
                    value: AutomationTarget::Send { to: return_track },
                });
            }
        }
        AutomationDevice::Device(device_id) => {
            let description = doc
                .device(device_id)
                .and_then(|device| shell.devices().description(&device.kind));
            if let Some(description) = description {
                choices.extend(
                    description
                        .params
                        .iter()
                        .filter(|param| param.automatable)
                        .map(|param| Choice {
                            label: param.name.clone(),
                            value: AutomationTarget::Device {
                                device: device_id,
                                param: param.id,
                            },
                        }),
                );
            }
        }
        AutomationDevice::None => {}
    }
    choices
}

/// The target a newly chosen device shows: the mixer's volume, a device's
/// first automatable parameter, or nothing.
pub(crate) fn default_target(
    shell: &Shell,
    doc: &Document,
    device: AutomationDevice,
) -> Option<AutomationTarget> {
    match device {
        AutomationDevice::Mixer => Some(AutomationTarget::Mixer {
            param: MixerParam::Volume,
        }),
        AutomationDevice::Device(_) => control_choices(shell, doc, device)
            .into_iter()
            .next()
            .map(|choice| choice.value),
        AutomationDevice::None => None,
    }
}

/// Whether a target belongs to a chosen device, for the control chooser's
/// selection.
pub(crate) fn belongs_to(target: AutomationTarget, device: AutomationDevice) -> bool {
    device_of(target) == device
}

/// A drag on an automation lane.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LaneDrag {
    /// Dragging a breakpoint to another position and value.
    Breakpoint {
        target: AutomationTarget,
        /// Where it was when the drag began.
        at: Ticks,
        /// The value it had then, for finer (Shift) moves.
        value: f64,
        /// Where the pointer went down.
        pointer: Pos2,
        rect: Rect,
        scale: LaneScale,
    },
    /// Holding Option/Alt while dragging a segment, to curve it.
    Curve {
        target: AutomationTarget,
        /// The breakpoint the segment starts at.
        at: Ticks,
        /// Its bend when the drag began.
        bend: f64,
        pointer: Pos2,
    },
}

/// The automation gestures under way between frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AutomationGestures {
    lane: Option<TrackId>,
    drag: Option<LaneDrag>,
}

impl AutomationGestures {
    /// A press at `press` in a lane: on a breakpoint it drags it, and with
    /// Option/Alt on a segment it curves the segment. Returns whether it
    /// took the press; anywhere else the lanes' other gestures apply.
    pub fn begin(&mut self, lane: &Lane<'_>, press: Pos2, modifiers: Modifiers) -> bool {
        match lane.hit(press) {
            LaneHit::Breakpoint(at) => {
                let value = lane
                    .envelope()
                    .and_then(|envelope| envelope.point(at))
                    .map_or(0.0, |point| point.value);
                self.lane = Some(lane.track);
                self.drag = Some(LaneDrag::Breakpoint {
                    target: lane.target,
                    at,
                    value,
                    pointer: press,
                    rect: lane.rect,
                    scale: lane.scale,
                });
                true
            }
            LaneHit::Segment(at) if modifiers.alt => {
                let bend = lane
                    .envelope()
                    .and_then(|envelope| envelope.point(at))
                    .map_or(0.0, |point| point.bend);
                self.lane = Some(lane.track);
                self.drag = Some(LaneDrag::Curve {
                    target: lane.target,
                    at,
                    bend,
                    pointer: press,
                });
                true
            }
            _ => false,
        }
    }

    /// The drag's command for the pointer at `pointer`.
    fn frame(
        &self,
        view: &ArrangementView,
        doc: &Document,
        horizontal: Horizontal,
        pointer: Pos2,
        modifiers: Modifiers,
    ) -> Option<(&'static str, Value)> {
        let track = self.lane?;
        match self.drag? {
            LaneDrag::Breakpoint {
                target,
                at,
                value,
                pointer: from,
                rect,
                scale,
            } => {
                let by = horizontal.at(f64::from(pointer.x - rect.left()))
                    - horizontal.at(f64::from(from.x - rect.left()));
                // S-ENV-006: Cmd (Alt on Windows) bypasses grid snapping.
                let invert = if cfg!(target_os = "macos") {
                    modifiers.mac_cmd
                } else {
                    modifiers.alt
                };
                let to = view.snap_drag(doc, at + by, invert);
                let t = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);
                let raw = scale.value(f64::from(t));
                // S-ENV-002: Shift drags the value in finer steps.
                let value = if modifiers.shift {
                    value + (raw - value) * FINE_VALUE
                } else {
                    raw
                };
                Some((
                    "automation.move_breakpoint",
                    json!({ "track": track, "target": target, "at": at, "to": to, "value": value }),
                ))
            }
            LaneDrag::Curve {
                target,
                at,
                bend,
                pointer: from,
            } => {
                let dy = f64::from(from.y - pointer.y) * CURVE_PER_POINT;
                let bend = (bend + dy).clamp(-1.0, 1.0);
                Some((
                    "automation.set_curve",
                    json!({ "track": track, "target": target, "at": at, "bend": bend }),
                ))
            }
        }
    }

    /// The drag went on to `pointer`.
    pub fn update(
        &self,
        view: &ArrangementView,
        doc: &Document,
        horizontal: Horizontal,
        pointer: Pos2,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) {
        if let Some((id, args)) = self.frame(view, doc, horizontal, pointer, modifiers) {
            gesture(events, GesturePhase::Update, id, args);
        }
    }

    /// The drag ended at `pointer`: one undo step.
    pub fn end(
        &mut self,
        view: &ArrangementView,
        doc: &Document,
        horizontal: Horizontal,
        pointer: Option<Pos2>,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) {
        let last =
            pointer.and_then(|pointer| self.frame(view, doc, horizontal, pointer, modifiers));
        if self.drag.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: last.map(|(id, args)| Invocation::new(id, args)),
            });
        }
        self.lane = None;
    }

    /// Esc during a drag: the set goes back.
    pub fn cancel(&mut self, events: &mut Vec<UiEvent>) {
        if self.drag.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        self.lane = None;
    }

    /// A click at `pos` in a lane: on a breakpoint it deletes it, and on a
    /// segment (without Option/Alt, which is the curve gesture) it adds one
    /// there. Returns whether it took the click.
    pub fn click(
        &self,
        view: &ArrangementView,
        doc: &Document,
        lane: &Lane<'_>,
        pos: Pos2,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        match lane.hit(pos) {
            LaneHit::Breakpoint(at) => {
                invoke(
                    events,
                    "automation.delete_breakpoint",
                    json!({ "track": lane.track, "target": lane.target, "at": at }),
                );
                true
            }
            // With Option/Alt held the segment is being curved, not clicked.
            LaneHit::Segment(_) if modifiers.alt => false,
            LaneHit::Segment(_) => {
                // Spec: S3. A new breakpoint on the line keeps its shape.
                let at = click_at(view, doc, lane, pos, modifiers);
                let value = lane
                    .envelope()
                    .and_then(|envelope| envelope.value_at(at))
                    .unwrap_or_else(|| lane.value_at(pos));
                invoke(
                    events,
                    "automation.set_breakpoint",
                    json!({ "track": lane.track, "target": lane.target, "at": at, "value": value }),
                );
                true
            }
            LaneHit::Empty(_) => false,
        }
    }

    /// A double-click at `pos` in a lane: with Option/Alt on a segment it
    /// straightens it, and off a segment it adds a breakpoint there at the
    /// clicked value. Returns whether it took the click.
    pub fn double_click(
        &self,
        view: &ArrangementView,
        doc: &Document,
        lane: &Lane<'_>,
        pos: Pos2,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        match lane.hit(pos) {
            LaneHit::Segment(at) if modifiers.alt => {
                // Spec: S3. Alt/Option double-click straightens the segment.
                invoke(
                    events,
                    "automation.set_curve",
                    json!({ "track": lane.track, "target": lane.target, "at": at, "bend": 0.0 }),
                );
                true
            }
            LaneHit::Empty(_) => {
                let at = click_at(view, doc, lane, pos, modifiers);
                let value = lane.value_at(pos);
                invoke(
                    events,
                    "automation.set_breakpoint",
                    json!({ "track": lane.track, "target": lane.target, "at": at, "value": value }),
                );
                true
            }
            _ => false,
        }
    }
}

/// The position a click lands on: on the editing grid, unless Cmd (Alt on
/// Windows) is held to bypass snapping (S-ENV-006).
fn click_at(
    view: &ArrangementView,
    doc: &Document,
    lane: &Lane<'_>,
    pos: Pos2,
    modifiers: Modifiers,
) -> Ticks {
    let invert = if cfg!(target_os = "macos") {
        modifiers.mac_cmd
    } else {
        modifiers.alt
    };
    view.snap_drag(doc, lane.at(pos), invert)
}

/// Paints a lane's background and its top edge.
pub(super) fn paint_lane(painter: &Painter, rect: Rect, theme: &Theme) {
    painter.rect_filled(rect, 0.0, theme.colors.lane.0.gamma_multiply(0.86));
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, theme.colors.lane_line.0),
    );
}

/// Paints a lane's envelope and its breakpoints.
pub(super) fn paint_envelope(painter: &Painter, lane: &Lane<'_>, theme: &Theme) {
    let Some(envelope) = lane.envelope() else {
        return;
    };
    if envelope.is_empty() {
        return;
    }
    // The envelope's line, sampled across the lane so a curved segment shows
    // its shape (R12).
    let mut points: Vec<Pos2> = Vec::new();
    let mut x = lane.rect.left();
    while x <= lane.rect.right() {
        let at = lane.horizontal.at(f64::from(x - lane.rect.left()));
        if let Some(value) = envelope.value_at(at) {
            points.push(pos2(x, lane.y_of_value(value)));
        }
        x += 2.0;
    }
    if points.len() >= 2 {
        painter.add(egui::Shape::line(
            points,
            Stroke::new(1.5, theme.colors.accent.0),
        ));
    }
    for point in envelope.points() {
        painter.rect_filled(
            Rect::from_center_size(
                lane.point_pos(point),
                vec2(BREAKPOINT_SIZE, BREAKPOINT_SIZE),
            ),
            0.0,
            theme.colors.text.0,
        );
    }
}

/// Draws a track's automation lane header in the header column: the device
/// chooser and the parameter chooser.
pub(super) fn lane_header(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    track: TrackId,
    target: Option<AutomationTarget>,
    rect: Rect,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    let mut header = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    header.set_clip_rect(rect);
    let gap = 3.0;
    let each = ((rect.width() - gap) / 2.0).max(10.0);
    let device_rect = Rect::from_min_size(rect.min, vec2(each, rect.height()));
    let control_rect = Rect::from_min_size(
        pos2(rect.left() + each + gap, rect.top()),
        vec2(each, rect.height()),
    );

    let current_device = target.map_or(AutomationDevice::None, device_of);
    let devices = device_choices(doc, track);
    // The choosers are Tab-reachable: the Accessibility spec, R4.
    let device_salt = format!("arrangement-automation-device-{}", track.0);
    let control_salt = format!("arrangement-automation-control-{}", track.0);
    for salt in [device_salt.as_str(), control_salt.as_str()] {
        crate::focus::register(
            ui.ctx(),
            egui::Id::new(("chooser", salt)),
            bl_shell::Area::MainView,
            None,
        );
    }
    if let Some(chosen) = widgets::chooser(
        &mut header,
        theme,
        &device_salt,
        "Automation Device",
        device_rect,
        &devices,
        Some(&current_device),
        true,
    ) {
        invoke(
            events,
            view_ids::SHOW_AUTOMATION,
            json!({ "track": track, "target": default_target(shell, doc, chosen) }),
        );
    }

    let controls = control_choices(shell, doc, current_device);
    let current_control = target.filter(|target| belongs_to(*target, current_device));
    if let Some(chosen) = widgets::chooser(
        &mut header,
        theme,
        &control_salt,
        "Automation Control",
        control_rect,
        &controls,
        current_control.as_ref(),
        true,
    ) {
        invoke(
            events,
            view_ids::SHOW_AUTOMATION,
            json!({ "track": track, "target": chosen }),
        );
    }
}

/// A label for a target, for assistive technology.
pub(crate) fn target_label(shell: &Shell, doc: &Document, target: AutomationTarget) -> String {
    match target {
        AutomationTarget::Mixer { param } => match param {
            MixerParam::Volume => "Volume".to_owned(),
            MixerParam::Pan => "Pan".to_owned(),
            MixerParam::Mute => "Track Activator".to_owned(),
        },
        AutomationTarget::Send { to } => doc.track(to).map_or_else(
            || "Send".to_owned(),
            |track| format!("Send {}", track.name()),
        ),
        AutomationTarget::Device { device, param } => doc
            .device(device)
            .and_then(|device| {
                shell
                    .devices()
                    .description(&device.kind)
                    .and_then(|description| description.param(param))
                    .map(|descriptor| descriptor.name.clone())
            })
            .unwrap_or_else(|| "Device parameter".to_owned()),
    }
}
