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
    AutomationTarget, Breakpoint, Device, DeviceId, Document, Envelope, MixerParam,
    TrackId,
};
use bl_shell::arrangement::automation::{Travel as LaneScale, selected_breakpoints, travel};
use bl_shell::arrangement::rows::Row;
use bl_shell::arrangement::{ArrangementView, Horizontal};
use bl_shell::arrangement_commands as view_ids;
use bl_shell::{GesturePhase, Invocation, Shell};
use egui::{Modifiers, Painter, Pos2, Rect, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice, Leds, NO_LEDS};

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

/// How close to a neighbouring breakpoint, in points, a dragged breakpoint
/// snaps to it (the automation editing spec, R3).
pub(crate) const NEIGHBOUR_GRAB: f64 = 6.0;

/// How close to the envelope's line, in points, a click counts as on it:
/// on it, it adds a breakpoint; near it, it selects the segment (the
/// automation editing spec, R4).
pub(crate) const SEGMENT_ON: f32 = 1.0;

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
#[derive(Clone, Debug)]
pub(super) struct Lane<'a> {
    pub doc: &'a Document,
    pub track: TrackId,
    pub target: AutomationTarget,
    pub rect: Rect,
    pub horizontal: Horizontal,
    pub scale: LaneScale,
    /// The breakpoints that are selected in this lane (the automation
    /// editing spec, R2).
    pub selected: Vec<Ticks>,
    /// The time selection covering the lane, if any (R4).
    pub span: Option<(Ticks, Ticks)>,
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

    /// How far a point is from the envelope's line, if it is near it.
    pub(super) fn segment_distance(&self, pos: Pos2) -> Option<f32> {
        let value = self.envelope()?.value_at(self.at(pos))?;
        Some((self.y_of_value(value) - pos.y).abs())
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
            scale: travel(self.doc, shell.devices(), target),
            selected: selected_breakpoints(self.doc, self.view, row.track, target),
            span: self.view.time_selection(),
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

/// The device chooser's options: the track mixer, each device in the track's
/// chain, and None to show no envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationDevice {
    /// The track's mixer: volume, pan, the Track Activator, and its sends.
    Mixer,
    /// One of the track's devices.
    Device(DeviceId),
    /// A MIDI clip's controllers: pitch bend, channel pressure and the
    /// continuous controllers (WP-2.29). Only a clip's envelopes name them.
    MidiCtrl,
    /// No envelope: the lane stays, with nothing drawn.
    None,
}

/// The device a target belongs to, as the device chooser sees it.
pub(crate) fn device_of(target: AutomationTarget) -> AutomationDevice {
    match target {
        AutomationTarget::Mixer { .. }
        | AutomationTarget::Send { .. }
        // The Device View's chain list picks chain controls (WP-5.2).
        | AutomationTarget::Chain { .. }
        | AutomationTarget::ChainSend { .. } => AutomationDevice::Mixer,
        AutomationTarget::Device { device, .. } => AutomationDevice::Device(device),
        AutomationTarget::MidiCc { .. }
        | AutomationTarget::MidiPitchBend
        | AutomationTarget::MidiChannelPressure => AutomationDevice::MidiCtrl,
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
        leds: NO_LEDS,
    }];
    if let Some(track) = doc.track(track) {
        choices.extend(track.devices().map(|device| Choice {
            label: device_title(device),
            value: AutomationDevice::Device(device.id),
            leds: NO_LEDS,
        }));
    }
    choices.push(Choice {
        label: "None".to_owned(),
        value: AutomationDevice::None,
        leds: NO_LEDS,
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
                ("Split Pan Left", MixerParam::SplitLeft),
                ("Split Pan Right", MixerParam::SplitRight),
            ] {
                choices.push(Choice {
                    label: label.to_owned(),
                    value: AutomationTarget::Mixer { param },
                    leds: NO_LEDS,
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
                    leds: NO_LEDS,
                });
            }
        }
        AutomationDevice::Device(device_id) => {
            if let Some(device) = doc.device(device_id)
                && let Some(description) = shell.devices().description(&device.kind)
            {
                // Spec: clap-hosting R5, WP-5.40. A plug-in lists the
                // parameters it is configured to show, then the one its
                // editor moved last (the temporary entry, manual 24.1.2);
                // any other device lists them all.
                let shown = shell.devices().plugin_configured(device, description);
                choices.extend(
                    shown
                        .iter()
                        .filter_map(|id| description.param(*id))
                        .filter(|param| param.automatable)
                        .map(|param| Choice {
                            label: param.name.clone(),
                            value: AutomationTarget::Device {
                                device: device_id,
                                param: param.id,
                            },
                            leds: NO_LEDS,
                        }),
                );
            }
        }
        AutomationDevice::MidiCtrl => {
            let controls = [
                ("Pitch Bend".to_owned(), AutomationTarget::MidiPitchBend),
                (
                    "Channel Pressure".to_owned(),
                    AutomationTarget::MidiChannelPressure,
                ),
            ]
            .into_iter()
            .chain(AutomationTarget::clip_controllers().map(|number| {
                (
                    format!("{number} {}", AutomationTarget::controller_name(number)),
                    AutomationTarget::MidiCc { number },
                )
            }));
            choices.extend(controls.map(|(label, value)| Choice {
                label,
                value,
                leds: NO_LEDS,
            }));
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
        AutomationDevice::MidiCtrl => Some(AutomationTarget::MidiPitchBend),
        AutomationDevice::None => None,
    }
}

/// Whether a target belongs to a chosen device, for the control chooser's
/// selection.
pub(crate) fn belongs_to(target: AutomationTarget, device: AutomationDevice) -> bool {
    device_of(target) == device
}

/// What the parameters `has` picks have on `track`: an LED in the automation
/// colour while any has an envelope (grey while every such envelope is
/// overridden), and one in the modulation colour while any is modulated by
/// a clip's envelope.
// Spec: modulation R8
fn leds(
    doc: &Document,
    track: TrackId,
    theme: &Theme,
    has: impl Fn(&AutomationTarget) -> bool,
) -> Leds {
    let Some(track) = doc.track(track) else {
        return NO_LEDS;
    };
    let c = &theme.colors;
    let mut envelopes = track.automation().filter(|(target, _)| has(target));
    let automation = envelopes.next().map(|first| {
        if first.1.enabled || envelopes.any(|(_, automation)| automation.enabled) {
            c.automation.0
        } else {
            c.inactive.0
        }
    });
    let modulation = track
        .clips()
        .chain(track.slots().map(|(_, clip)| clip))
        .any(|clip| clip.envelopes().any(|envelope| has(&envelope.target)))
        .then_some(c.modulation.0);
    [automation, modulation]
}

/// Marks the device chooser's choices with their LEDs.
pub(crate) fn mark_devices(
    choices: &mut [Choice<AutomationDevice>],
    doc: &Document,
    track: TrackId,
    theme: &Theme,
) {
    for choice in choices {
        choice.leds = leds(doc, track, theme, |target| {
            device_of(*target) == choice.value
        });
    }
}

/// Marks the control chooser's choices with their LEDs.
pub(crate) fn mark_controls(
    choices: &mut [Choice<AutomationTarget>],
    doc: &Document,
    track: TrackId,
    theme: &Theme,
) {
    for choice in choices {
        choice.leds = leds(doc, track, theme, |target| *target == choice.value);
    }
}

/// A drag on an automation lane.
#[derive(Clone, Debug, PartialEq)]
enum LaneDrag {
    /// Dragging breakpoints to another position and value: the ones that
    /// are selected move together.
    Breakpoint {
        target: AutomationTarget,
        /// The breakpoints that move: the selected ones, or the one that
        /// was grabbed.
        at: Vec<Ticks>,
        /// Where the grabbed one was when the drag began.
        grabbed: Ticks,
        /// The value the grabbed one had then, for finer (Shift) moves.
        value: f64,
        /// The positions of the envelope's other breakpoints, which the
        /// drag snaps to (S-ENV-006's neighbours).
        neighbours: Vec<Ticks>,
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
    /// A press at `press` in a lane: on a breakpoint it drags it, with the
    /// selected breakpoints moving with it, and with Option/Alt on a segment
    /// it curves the segment. Returns whether it took the press; anywhere
    /// else the lanes' other gestures apply.
    pub fn begin(&mut self, lane: &Lane<'_>, press: Pos2, modifiers: Modifiers) -> bool {
        match lane.hit(press) {
            LaneHit::Breakpoint(at) => {
                let grabbed = at;
                let value = lane
                    .envelope()
                    .and_then(|envelope| envelope.point(at))
                    .map_or(0.0, |point| point.value);
                // Spec: automation-editing R2. Dragging a selected
                // breakpoint moves every selected breakpoint with it.
                let at = if lane.selected.contains(&at) && !lane.selected.is_empty() {
                    lane.selected.clone()
                } else {
                    vec![at]
                };
                let neighbours: Vec<Ticks> = lane
                    .envelope()
                    .map(|envelope| {
                        envelope
                            .points()
                            .filter(|point| !at.contains(&point.at))
                            .map(|point| point.at)
                            .collect()
                    })
                    .unwrap_or_default();
                self.lane = Some(lane.track);
                self.drag = Some(LaneDrag::Breakpoint {
                    target: lane.target,
                    at,
                    grabbed,
                    value,
                    neighbours,
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
            // Spec: automation-editing R4. Dragging a segment moves its two
            // breakpoints, or the stretch of envelope the time selection
            // covers when the segment lies inside it.
            LaneHit::Segment(at) => {
                let mut at = if lane.selected.contains(&at) {
                    lane.selected.clone()
                } else {
                    vec![at]
                };
                // Spec: automation-editing R4. A segment inside the time
                // selection gets breakpoints at the selection's edges, which
                // move with it.
                if let Some((start, end)) = lane.span
                    && at.first().is_some_and(|at| *at >= start && *at < end)
                {
                    for edge in [start, end] {
                        if !at.contains(&edge) {
                            at.push(edge);
                        }
                    }
                }
                let value = lane
                    .envelope()
                    .and_then(|envelope| envelope.point(at[0]))
                    .map_or(0.0, |point| point.value);
                let neighbours: Vec<Ticks> = lane
                    .envelope()
                    .map(|envelope| {
                        envelope
                            .points()
                            .filter(|point| !at.contains(&point.at))
                            .map(|point| point.at)
                            .collect()
                    })
                    .unwrap_or_default();
                let grabbed = at[0];
                self.lane = Some(lane.track);
                self.drag = Some(LaneDrag::Breakpoint {
                    target: lane.target,
                    at,
                    grabbed,
                    value,
                    neighbours,
                    pointer: press,
                    rect: lane.rect,
                    scale: lane.scale,
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
        match self.drag.as_ref()? {
            LaneDrag::Breakpoint {
                target,
                at,
                grabbed,
                value: old_value,
                neighbours,
                pointer: from,
                rect,
                scale,
            } => {
                let by = horizontal.at(f64::from(pointer.x - rect.left()))
                    - horizontal.at(f64::from(from.x - rect.left()));
                // Spec: automation-editing R3. Shift keeps the drag to the
                // axis it started along, and makes a vertical one finer.
                let vertical = modifiers.shift
                    && (pointer.y - from.y).abs() > (pointer.x - from.x).abs();
                let by = if vertical { Ticks::ZERO } else { by };
                // S-ENV-006: Cmd (Alt on Windows) bypasses grid snapping.
                let invert = if cfg!(target_os = "macos") {
                    modifiers.mac_cmd
                } else {
                    modifiers.alt
                };
                let snapped = view.snap_drag(doc, *grabbed + by, invert);
                // Spec: automation-editing R3. The drag snaps to the
                // positions of the breakpoints next to it.
                let to = if vertical {
                    *grabbed
                } else {
                    neighbours
                        .iter()
                        .map(|at| (*at, (horizontal.x(*at) - horizontal.x(snapped)).abs()))
                        .filter(|(_, away)| *away <= NEIGHBOUR_GRAB)
                        .min_by(|(_, away), (_, other)| away.total_cmp(other))
                        .map_or(snapped, |(at, _)| at)
                };
                let t = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);
                let raw = scale.value(f64::from(t));
                // S-ENV-002: Shift drags the value in finer steps. A Shift
                // drag along the time axis keeps the value where it was.
                let value = if vertical {
                    *old_value + (raw - *old_value) * FINE_VALUE
                } else if modifiers.shift {
                    *old_value
                } else {
                    raw
                };
                // Spec: automation-editing R2. Every breakpoint that moves
                // goes by the same time and the same share of the
                // parameter's travel.
                let share = scale.position(value) - scale.position(*old_value);
                Some((
                    "automation.move_breakpoints",
                    json!({
                        "track": track,
                        "target": target,
                        "at": at,
                        "by": to - *grabbed,
                        "share": share,
                    }),
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
            LaneHit::Segment(at) => {
                // Spec: automation-editing R4. Clicking near a segment but
                // not on it, or Shift-clicking on it, selects the segment:
                // its two breakpoints.
                let on = lane.segment_distance(pos).unwrap_or(0.0) <= SEGMENT_ON;
                if !on || modifiers.shift {
                    invoke(
                        events,
                        "automation.select_segment",
                        json!({ "track": lane.track, "target": lane.target, "at": at }),
                    );
                    return true;
                }
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
            // Spec: modulation R9: the base, in the automation colour.
            Stroke::new(1.5, theme.colors.automation.0),
        ));
    }
    for point in envelope.points() {
        // Spec: automation-editing R2. A selected breakpoint is drawn in
        // the selection colour.
        let selected = lane.selected.contains(&point.at);
        painter.rect_filled(
            Rect::from_center_size(
                lane.point_pos(point),
                vec2(BREAKPOINT_SIZE, BREAKPOINT_SIZE),
            ),
            0.0,
            if selected {
                theme.colors.time_selection.0
            } else {
                theme.colors.text.0
            },
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
    let mut devices = device_choices(doc, track);
    mark_devices(&mut devices, doc, track, theme);
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

    let mut controls = control_choices(shell, doc, current_device);
    mark_controls(&mut controls, doc, track, theme);
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
            MixerParam::SplitLeft => "Split Pan Left".to_owned(),
            MixerParam::SplitRight => "Split Pan Right".to_owned(),
        },
        AutomationTarget::Send { to } => doc.track(to).map_or_else(
            || "Send".to_owned(),
            |track| format!("Send {}", track.name()),
        ),
        AutomationTarget::Chain { chain, param } => {
            let name = doc
                .chain(chain)
                .map_or("Chain", |chain| chain.name.as_str());
            match param {
                MixerParam::Volume => format!("{name} Volume"),
                MixerParam::Pan => format!("{name} Pan"),
                MixerParam::Mute => format!("{name} Activator"),
                MixerParam::SplitLeft => format!("{name} Split Pan Left"),
                MixerParam::SplitRight => format!("{name} Split Pan Right"),
            }
        }
        AutomationTarget::ChainSend { chain, to } => {
            let name = |id| doc.chain(id).map_or("Chain", |chain| chain.name.as_str());
            format!("{} Send to {}", name(chain), name(to))
        }
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
        AutomationTarget::MidiCc { number } => {
            format!("{number} {}", AutomationTarget::controller_name(number))
        }
        AutomationTarget::MidiPitchBend => "Pitch Bend".to_owned(),
        AutomationTarget::MidiChannelPressure => "Channel Pressure".to_owned(),
    }
}
