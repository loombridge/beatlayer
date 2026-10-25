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
    AutomationTarget, Breakpoint, ClipParam, Device, DeviceId, Document, Envelope, MixerParam,
    TrackId,
};
use bl_shell::arrangement::automation::{Travel as LaneScale, selected_breakpoints, travel};
pub(crate) use bl_shell::arrangement::lanes::cycle_target;
use bl_shell::arrangement::rows::Row;
use bl_shell::arrangement::{ArrangementView, Horizontal};
use bl_shell::arrangement_commands as view_ids;
use bl_shell::{GesturePhase, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Modifiers, Painter, Pos2, Rect, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
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

/// How close to a handle of a hovered time selection, in points, a press
/// grabs it (the automation editing spec, R14).
pub(crate) const HANDLE_GRAB: f32 = 6.0;

/// The size of a handle's square, in points.
const HANDLE_SIZE: f32 = 6.0;

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
    /// The lane's number among the track's: 0 is the main lane, which draws
    /// over the track's clips, and 1 and up are its lanes of its own.
    pub number: usize,
    pub target: AutomationTarget,
    /// Whether Snap Automation to Grid is on (the automation spec, R31).
    pub snap: bool,
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

    /// The box round the time selection in the lane: the span across the
    /// parameter's whole range (the automation editing spec, R14).
    pub(super) fn span_box(&self) -> Option<Rect> {
        let (start, end) = self.span?;
        Some(Rect::from_min_max(
            pos2(
                self.rect.left() + self.horizontal.x(start) as f32,
                self.rect.top(),
            ),
            pos2(
                self.rect.left() + self.horizontal.x(end) as f32,
                self.rect.bottom(),
            ),
        ))
    }

    /// The handle of the time selection's box under a point, if any.
    fn handle_at(&self, pos: Pos2) -> Option<Handle> {
        Handle::at(self.span_box()?, pos)
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

/// The rectangle of lane `number` of a track's row on screen: the main lane
/// (0) over the track's own lane, the others under it (the automation spec,
/// R24, R25).
pub(super) fn lane_rect(row: &Row, number: usize, lanes: Rect, scroll_y: f32) -> Rect {
    let (top, bottom) = row.lane_span(number);
    let y = lanes.top() - scroll_y;
    Rect::from_min_max(pos2(lanes.left(), y + top), pos2(lanes.right(), y + bottom))
}

/// The ID of a track's automation lane in the accessibility tree.
#[must_use]
pub(super) fn lane_id(track: TrackId, number: usize) -> egui::Id {
    egui::Id::new(("arrangement-automation-lane", track, number))
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
    /// The automation lanes of `row` that draw an envelope: the main lane,
    /// then the lanes of its own, while Automation Mode is on (a lane whose
    /// choosers show None has none to edit).
    pub(super) fn lanes_of(&self, row: &Row, shell: &Shell) -> Vec<Lane<'_>> {
        self.view
            .shown_lane_targets(row.track)
            .into_iter()
            .map(|(number, target)| Lane {
                doc: self.doc,
                track: row.track,
                number,
                target,
                snap: shell.snap_automation_to_grid(),
                rect: lane_rect(row, number, self.rect, self.scroll_y),
                horizontal: self.horizontal,
                scale: travel(self.doc, shell.devices(), target),
                selected: selected_breakpoints(self.doc, self.view, row.track, target),
                span: self.view.time_selection().filter(|_| {
                    self.view
                        .time_selection_lanes()
                        .iter()
                        .any(|lane| lane.track() == row.track && lane.target() == Some(target))
                }),
            })
            .collect()
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

    /// The automation lane under a point, if any: a lane of a track's own
    /// under it, or its main lane, which covers the track's own lane. A
    /// clip's title bar and edges in the main lane keep the clip's gestures
    /// (R24), which the caller tells apart.
    pub(super) fn lane_at(&self, pos: Pos2, shell: &Shell) -> Option<Lane<'_>> {
        let row = self.row_at(pos)?;
        self.lanes_of(row, shell)
            .into_iter()
            .find(|lane| lane.rect.contains(pos))
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
    /// An audio clip's sound settings: Gain, Transposition, Sample Offset and
    /// the rest (WP-4.8, the warp modes spec R29).
    Clip,
    /// No envelope: the lane stays, with nothing drawn.
    None,
    /// The device chooser's last entry, Show Automated Parameters Only (the
    /// automation spec, R27): a switch, never the chooser's value.
    AutomatedOnly,
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
        AutomationTarget::Clip { .. } => AutomationDevice::Clip,
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
        AutomationDevice::Clip => choices.extend(ClipParam::ALL.map(clip_choice)),
        AutomationDevice::None | AutomationDevice::AutomatedOnly => {}
    }
    choices
}

/// The chooser's entry for a clip sound setting. The manual calls the
/// envelope that adds to Transpose "Transposition" (R29).
fn clip_choice(param: ClipParam) -> Choice<AutomationTarget> {
    // The chooser lists one mode's controls at a time, so Grain Size needs no
    // mode before it.
    let label = match param {
        ClipParam::Transpose => "Transposition".to_owned(),
        ClipParam::TonesGrainSize | ClipParam::TextureGrainSize => "Grain Size".to_owned(),
        _ => param.descriptor().name,
    };
    Choice {
        label,
        value: AutomationTarget::Clip { param },
        leds: NO_LEDS,
    }
}

/// The controls the "Clip" entry offers for an audio clip (R29): Gain,
/// Transposition and, in Beats, Sample Offset, then Detune, HiQ and the
/// controls of the clip's warp mode. A mode's other controls keep their
/// envelopes with the clip, unlisted.
pub fn clip_choices(audio: &bl_model::AudioData) -> Vec<Choice<AutomationTarget>> {
    use bl_model::WarpMode;
    let mode: &[ClipParam] = match audio.warp_mode {
        WarpMode::Beats => &[
            ClipParam::Preserve,
            ClipParam::TransientLoopMode,
            ClipParam::TransientEnvelope,
        ],
        WarpMode::Tones => &[ClipParam::TonesGrainSize],
        WarpMode::Texture => &[ClipParam::TextureGrainSize, ClipParam::TextureFlux],
        WarpMode::ComplexPro => &[ClipParam::Formants, ClipParam::ComplexProEnvelope],
        WarpMode::RePitch | WarpMode::Complex => &[],
    };
    let beats = audio.warp_mode == WarpMode::Beats;
    [ClipParam::Gain, ClipParam::Transpose]
        .into_iter()
        .chain(beats.then_some(ClipParam::SampleOffset))
        .chain([ClipParam::Detune, ClipParam::HighQuality])
        .chain(mode.iter().copied())
        .map(clip_choice)
        .collect()
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
        AutomationDevice::Clip => Some(AutomationTarget::Clip {
            param: ClipParam::Gain,
        }),
        AutomationDevice::None | AutomationDevice::AutomatedOnly => None,
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

/// Keeps the device chooser's entries that have automation on `track`: the
/// mixer and the devices with an envelope, and None (the automation spec,
/// R27).
pub(crate) fn keep_automated_devices(
    choices: &mut Vec<Choice<AutomationDevice>>,
    doc: &Document,
    track: TrackId,
) {
    let Some(owner) = doc.track(track) else {
        return;
    };
    choices.retain(|choice| {
        choice.value == AutomationDevice::None
            || owner
                .automation()
                .any(|(target, _)| device_of(*target) == choice.value)
    });
}

/// Keeps the control chooser's entries that have an envelope on `track` (R27).
pub(crate) fn keep_automated_controls(
    choices: &mut Vec<Choice<AutomationTarget>>,
    doc: &Document,
    track: TrackId,
) {
    let Some(owner) = doc.track(track) else {
        return;
    };
    choices.retain(|choice| owner.automation_for(&choice.value).is_some());
}

/// Adds the device chooser's last entry, Show Automated Parameters Only,
/// ticked while the lane's choosers are filtered (R27).
pub(crate) fn push_automated_only(choices: &mut Vec<Choice<AutomationDevice>>, on: bool) {
    choices.push(Choice {
        label: format!(
            "{}Show Automated Parameters Only",
            if on { "✔ " } else { "" }
        ),
        value: AutomationDevice::AutomatedOnly,
        leds: NO_LEDS,
    });
}

/// One of the eight handles on a hovered time selection's box (the
/// automation editing spec, R14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handle {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Handle {
    pub(crate) const ALL: [Self; 8] = [
        Self::Top,
        Self::Bottom,
        Self::Left,
        Self::Right,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// The handle of the box `rect` that a press at `pos` grabs, if any.
    pub(crate) fn at(rect: Rect, pos: Pos2) -> Option<Self> {
        Self::ALL
            .into_iter()
            .map(|handle| (handle, handle.pos(rect).distance(pos)))
            .filter(|(_, away)| *away <= HANDLE_GRAB)
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(handle, _)| handle)
    }

    /// Where the handle sits on `rect`.
    fn pos(self, rect: Rect) -> Pos2 {
        let c = rect.center();
        match self {
            Self::Top => pos2(c.x, rect.top()),
            Self::Bottom => pos2(c.x, rect.bottom()),
            Self::Left => pos2(rect.left(), c.y),
            Self::Right => pos2(rect.right(), c.y),
            Self::TopLeft => rect.left_top(),
            Self::TopRight => rect.right_top(),
            Self::BottomLeft => rect.left_bottom(),
            Self::BottomRight => rect.right_bottom(),
        }
    }

    /// The pointer that shows what dragging the handle does.
    fn cursor(self) -> egui::CursorIcon {
        match self {
            Self::Top | Self::Bottom => egui::CursorIcon::ResizeVertical,
            Self::Left | Self::Right => egui::CursorIcon::ResizeHorizontal,
            Self::TopLeft | Self::BottomRight => egui::CursorIcon::ResizeNwSe,
            Self::TopRight | Self::BottomLeft => egui::CursorIcon::ResizeNeSw,
        }
    }
}

/// The box round a time selection while a handle drags it: the span, and
/// where the bottom and top edges run at its start and end, as shares of
/// the parameter's travel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoxShape {
    start: Ticks,
    end: Ticks,
    bottom: (f64, f64),
    top: (f64, f64),
}

impl BoxShape {
    /// The box's four corners on screen: top left, top right, bottom right,
    /// bottom left.
    pub(crate) fn corners(self, rect: Rect, x: impl Fn(Ticks) -> f32) -> [Pos2; 4] {
        let y = |share: f64| rect.bottom() - share as f32 * rect.height();
        [
            pos2(x(self.start), y(self.top.0)),
            pos2(x(self.end), y(self.top.1)),
            pos2(x(self.end), y(self.bottom.1)),
            pos2(x(self.start), y(self.bottom.0)),
        ]
    }
}

/// Where a handle drag's pointer is in time: on the editing grid, unless Cmd
/// (Alt on Windows) does the opposite of the grid's setting (S-ENV-006).
fn handle_time(
    view: &ArrangementView,
    doc: &Document,
    horizontal: Horizontal,
    rect: Rect,
    pointer: Pos2,
    modifiers: Modifiers,
) -> Ticks {
    let invert = if cfg!(target_os = "macos") {
        modifiers.mac_cmd
    } else {
        modifiers.alt
    };
    view.snap_drag(
        doc,
        horizontal.at(f64::from(pointer.x - rect.left())),
        invert,
    )
}

/// The box a handle's drag makes of the one that began it, for the pointer
/// at `pointer`, which is at `at` in time once snapped (the automation
/// editing spec, R15 to R17): Shift makes a vertical drag finer, and Option
/// (Alt on Windows) moves the opposite handle the opposite way.
pub(crate) fn drag_box(
    handle: Handle,
    from: (Ticks, Ticks),
    rect: Rect,
    at: Ticks,
    pointer: Pos2,
    modifiers: Modifiers,
) -> BoxShape {
    let (start, end) = from;
    let mut shape = BoxShape {
        start,
        end,
        bottom: (0.0, 0.0),
        top: (1.0, 1.0),
    };
    // How far up the travel the pointer is, finer with Shift, from where
    // the handle began. The drag may leave the lane: values past the travel
    // are clipped (R15).
    let height = |began: f64| {
        let t = (f64::from(rect.bottom() - pointer.y) / f64::from(rect.height())).clamp(-1.0, 2.0);
        if modifiers.shift {
            began + (t - began) * FINE_VALUE
        } else {
            t
        }
    };
    let mirror = modifiers.alt;
    match handle {
        Handle::Top => {
            let top = height(1.0);
            if mirror {
                let d = (top - 1.0).max(-0.5);
                shape.bottom = (-d, -d);
                shape.top = (1.0 + d, 1.0 + d);
            } else {
                shape.top = (top.max(0.0), top.max(0.0));
            }
        }
        Handle::Bottom => {
            let bottom = height(0.0);
            if mirror {
                let d = bottom.min(0.5);
                shape.bottom = (d, d);
                shape.top = (1.0 - d, 1.0 - d);
            } else {
                shape.bottom = (bottom.min(1.0), bottom.min(1.0));
            }
        }
        Handle::Left => {
            shape.start = at.clamp(Ticks::ZERO, end - Ticks(1));
            if mirror {
                shape.end = (end - (shape.start - start)).max(shape.start + Ticks(1));
            }
        }
        Handle::Right => {
            shape.end = at.max(start + Ticks(1));
            if mirror {
                shape.start = (start - (shape.end - end)).clamp(Ticks::ZERO, shape.end - Ticks(1));
            }
        }
        Handle::TopLeft | Handle::TopRight => {
            let moved = height(1.0);
            let d = if mirror {
                (moved - 1.0).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let (left, right) = if handle == Handle::TopLeft {
                (moved.max(0.0), if mirror { 1.0 - d } else { 1.0 })
            } else {
                (if mirror { 1.0 - d } else { 1.0 }, moved.max(0.0))
            };
            shape.top = (left, right);
        }
        Handle::BottomLeft | Handle::BottomRight => {
            let moved = height(0.0);
            let d = if mirror { moved.clamp(-1.0, 1.0) } else { 0.0 };
            let (left, right) = if handle == Handle::BottomLeft {
                (moved.min(1.0), if mirror { -d } else { 0.0 })
            } else {
                (if mirror { -d } else { 0.0 }, moved.min(1.0))
            };
            shape.bottom = (left, right);
        }
    }
    shape
}

/// The commands a handle's drag runs: the stretch of values, the stretch in
/// time and the skew.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BoxCommands {
    pub stretch_values: &'static str,
    pub stretch_time: &'static str,
    pub skew: &'static str,
}

/// The automation lane's.
const LANE_BOX: BoxCommands = BoxCommands {
    stretch_values: "automation.stretch_values",
    stretch_time: "automation.stretch_time",
    skew: "automation.skew",
};

/// The command that makes the box a handle's drag drew.
pub(crate) fn box_command(
    handle: Handle,
    shape: BoxShape,
    carry: bool,
    commands: BoxCommands,
) -> (&'static str, Value) {
    match handle {
        Handle::Top | Handle::Bottom => (
            commands.stretch_values,
            json!({ "bottom": shape.bottom.0, "top": shape.top.0 }),
        ),
        Handle::Left | Handle::Right => (
            commands.stretch_time,
            json!({ "to_start": shape.start, "to_end": shape.end, "carry": carry }),
        ),
        Handle::TopLeft | Handle::TopRight => (
            commands.skew,
            json!({ "edge": "top", "at_start": shape.top.0, "at_end": shape.top.1 }),
        ),
        Handle::BottomLeft | Handle::BottomRight => (
            commands.skew,
            json!({ "edge": "bottom", "at_start": shape.bottom.0, "at_end": shape.bottom.1 }),
        ),
    }
}

/// A drag on an automation lane.
#[derive(Clone, Debug, PartialEq)]
enum LaneDrag {
    /// Dragging a handle of the time selection's box, to stretch the
    /// automation in value or in time, or to skew it (R14 to R17).
    Handle {
        handle: Handle,
        /// The span the box began with.
        span: (Ticks, Ticks),
        rect: Rect,
        horizontal: Horizontal,
    },
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
        /// Whether Snap Automation to Grid was on when the drag began.
        snap: bool,
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
    /// The track and number of the lane a drag is in.
    lane: Option<(TrackId, usize)>,
    drag: Option<LaneDrag>,
}

impl AutomationGestures {
    /// A press at `press` in a lane: on a breakpoint it drags it, with the
    /// selected breakpoints moving with it, and with Option/Alt on a segment
    /// it curves the segment. Returns whether it took the press; anywhere
    /// else the lanes' other gestures apply.
    pub fn begin(&mut self, lane: &Lane<'_>, press: Pos2, modifiers: Modifiers) -> bool {
        // Spec: automation-editing R14. A press on a handle of the time
        // selection's box takes it, unless a breakpoint is there too.
        if let (Some(handle), Some(span)) = (lane.handle_at(press), lane.span)
            && !matches!(lane.hit(press), LaneHit::Breakpoint(_))
        {
            self.lane = Some((lane.track, lane.number));
            self.drag = Some(LaneDrag::Handle {
                handle,
                span,
                rect: lane.rect,
                horizontal: lane.horizontal,
            });
            return true;
        }
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
                self.lane = Some((lane.track, lane.number));
                self.drag = Some(LaneDrag::Breakpoint {
                    target: lane.target,
                    at,
                    grabbed,
                    value,
                    neighbours,
                    pointer: press,
                    rect: lane.rect,
                    scale: lane.scale,
                    snap: lane.snap,
                });
                true
            }
            LaneHit::Segment(at) if modifiers.alt => {
                let bend = lane
                    .envelope()
                    .and_then(|envelope| envelope.point(at))
                    .map_or(0.0, |point| point.bend);
                self.lane = Some((lane.track, lane.number));
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
                self.lane = Some((lane.track, lane.number));
                self.drag = Some(LaneDrag::Breakpoint {
                    target: lane.target,
                    at,
                    grabbed,
                    value,
                    neighbours,
                    pointer: press,
                    rect: lane.rect,
                    scale: lane.scale,
                    snap: lane.snap,
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
        let (track, _) = self.lane?;
        match self.drag.as_ref()? {
            LaneDrag::Handle {
                handle,
                span,
                rect,
                horizontal,
            } => {
                let at = handle_time(view, doc, *horizontal, *rect, pointer, modifiers);
                let shape = drag_box(*handle, *span, *rect, at, pointer, modifiers);
                Some(box_command(*handle, shape, modifiers.shift, LANE_BOX))
            }
            LaneDrag::Breakpoint {
                target,
                at,
                grabbed,
                value: old_value,
                neighbours,
                pointer: from,
                rect,
                scale,
                snap,
            } => {
                let by = horizontal.at(f64::from(pointer.x - rect.left()))
                    - horizontal.at(f64::from(from.x - rect.left()));
                // Spec: automation-editing R3. Shift keeps the drag to the
                // axis it started along, and makes a vertical one finer.
                let vertical =
                    modifiers.shift && (pointer.y - from.y).abs() > (pointer.x - from.x).abs();
                let by = if vertical { Ticks::ZERO } else { by };
                let snapped = breakpoint_time(view, doc, *grabbed + by, modifiers, *snap);
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

    /// The outline of the box a handle's drag has made of the time
    /// selection in `track`'s lane for the pointer at `pointer`, if one is
    /// being dragged (R14).
    pub fn outline(
        &self,
        lane: (TrackId, usize),
        view: &ArrangementView,
        doc: &Document,
        pointer: Pos2,
        modifiers: Modifiers,
    ) -> Option<[Pos2; 4]> {
        let LaneDrag::Handle {
            handle,
            span,
            rect,
            horizontal,
        } = self.drag.as_ref()?
        else {
            return None;
        };
        (self.lane == Some(lane)).then(|| {
            let at = handle_time(view, doc, *horizontal, *rect, pointer, modifiers);
            drag_box(*handle, *span, *rect, at, pointer, modifiers)
                .corners(*rect, |at| rect.left() + horizontal.x(at) as f32)
        })
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
            // Spec: automation-editing R22 (S-A11Y-021). Option-click
            // (Alt-click) selects the breakpoint and puts the insert marker
            // on it.
            LaneHit::Breakpoint(at) if modifiers.alt => {
                invoke(
                    events,
                    bl_shell::envelope_commands::SELECT_BREAKPOINT,
                    json!({ "track": lane.track, "target": lane.target, "at": at }),
                );
                true
            }
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

/// Whether Cmd (Alt on Windows) is held, which does the opposite of what the
/// grid's snapping would (S-ENV-006).
fn bypass(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.mac_cmd
    } else {
        modifiers.alt
    }
}

/// Where a breakpoint made or moved with the mouse lands in time (the
/// automation spec, R31): with Snap Automation to Grid on, on the editing
/// grid unless Cmd (Alt on Windows) is held to bypass it (S-ENV-006); with it
/// off, where the pointer is, unless Cmd is held to snap that drag. The
/// editing grid's own Snap to Grid turns both off.
pub(super) fn breakpoint_time(
    view: &ArrangementView,
    doc: &Document,
    at: Ticks,
    modifiers: Modifiers,
    snap: bool,
) -> Ticks {
    if snap {
        view.snap_drag(doc, at, bypass(modifiers))
    } else if bypass(modifiers) && view.grid().snap {
        view.snap(doc, at)
    } else {
        at
    }
}

/// The position a click lands on (R31).
pub(super) fn click_at(
    view: &ArrangementView,
    doc: &Document,
    lane: &Lane<'_>,
    pos: Pos2,
    modifiers: Modifiers,
) -> Ticks {
    breakpoint_time(view, doc, lane.at(pos), modifiers, lane.snap)
}

/// Paints a lane's background and its top edge.
pub(super) fn paint_lane(painter: &Painter, rect: Rect, theme: &Theme) {
    painter.rect_filled(rect, 0.0, theme.colors.lane.0.gamma_multiply(0.86));
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, theme.colors.lane_line.0),
    );
}

/// Paints a lane's envelope and its breakpoints. With `keys`, the lane has
/// the keyboard: the focused breakpoint is drawn in the hover colour and a
/// selected one pulses (the automation editing spec, R21, R22); `time` is
/// the clock that pulse runs on.
pub(super) fn paint_envelope(
    painter: &Painter,
    lane: &Lane<'_>,
    theme: &Theme,
    keys: Option<&bl_shell::envelope_commands::EnvelopeKeyState>,
    time: f64,
) {
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
        // The main lane draws over the clips (R24): a dark edge keeps the line
        // readable on a waveform.
        if lane.number == 0 {
            painter.add(egui::Shape::line(
                points.clone(),
                Stroke::new(3.5, theme.colors.window.0.gamma_multiply(0.6)),
            ));
        }
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
        let keyed = keys
            .and_then(|keys| crate::envelope_keys::breakpoint_colour(theme, point.at, keys, time));
        painter.rect_filled(
            Rect::from_center_size(
                lane.point_pos(point),
                vec2(BREAKPOINT_SIZE, BREAKPOINT_SIZE),
            ),
            0.0,
            keyed.unwrap_or(if selected {
                theme.colors.time_selection.0
            } else {
                theme.colors.text.0
            }),
        );
    }
}

/// Paints the handles of the time selection's box in a lane while the
/// pointer is over it or a handle is being dragged, and the outline of the
/// box a drag has made (the automation editing spec, R14).
pub(super) fn paint_handles(
    painter: &Painter,
    ui: &Ui,
    lane: &Lane<'_>,
    theme: &Theme,
    outline: Option<[Pos2; 4]>,
) {
    if let Some(rect) = lane.span_box() {
        paint_handles_at(painter, ui, rect, theme, outline);
    }
}

/// Paints the handles of a time selection's box, `rect`, while the pointer
/// is over it or a handle is being dragged, and the outline of the box a drag
/// has made.
pub(crate) fn paint_handles_at(
    painter: &Painter,
    ui: &Ui,
    rect: Rect,
    theme: &Theme,
    outline: Option<[Pos2; 4]>,
) {
    let hover = ui.input(|i| i.pointer.hover_pos());
    if outline.is_none() && !hover.is_some_and(|pos| rect.expand(HANDLE_GRAB).contains(pos)) {
        return;
    }
    if let Some(corners) = outline {
        let mut line = corners.to_vec();
        line.push(corners[0]);
        painter.add(egui::Shape::line(
            line,
            Stroke::new(1.0, theme.colors.time_selection.0),
        ));
    }
    for handle in Handle::ALL {
        let at = handle.pos(rect);
        painter.rect(
            Rect::from_center_size(at, vec2(HANDLE_SIZE, HANDLE_SIZE)),
            0.0,
            theme.colors.time_selection.0,
            Stroke::new(1.0, theme.colors.text.0),
            egui::StrokeKind::Inside,
        );
        if hover.is_some_and(|pos| pos.distance(at) <= HANDLE_GRAB) {
            ui.ctx().set_cursor_icon(handle.cursor());
        }
    }
}

/// The ID of the accessible node of the breakpoint at `at` in a track's
/// automation lane.
fn breakpoint_id(track: TrackId, number: usize, at: Ticks) -> egui::Id {
    egui::Id::new(("arrangement-automation-breakpoint", track, number, at.get()))
}

/// Draws what the keyboard has in a lane: the insert marker with the value
/// beside it (or what is being typed), and the node of the focused or
/// selected breakpoint, which is returned for the lane's group to name as
/// its active descendant (the automation editing spec, R21, R28).
pub(super) fn paint_keys(
    ui: &Ui,
    painter: &Painter,
    theme: &Theme,
    shell: &Shell,
    lane: &Lane<'_>,
    keys: &bl_shell::envelope_commands::EnvelopeKeyState,
    typed: Option<&str>,
) -> Option<egui::Id> {
    let x = lane.rect.left() + lane.horizontal.x(keys.marker) as f32;
    if (lane.rect.left()..=lane.rect.right()).contains(&x) {
        crate::envelope_keys::paint_marker(
            painter,
            theme,
            lane.rect,
            x,
            typed.unwrap_or(&keys.value),
            typed.is_some(),
        );
    }
    let at = keys.selected.unwrap_or(keys.marker);
    let point = lane.envelope()?.point(at)?;
    let id = breakpoint_id(lane.track, lane.number, at);
    let rect = Rect::from_center_size(
        lane.point_pos(point),
        vec2(BREAKPOINT_SIZE * 2.0, BREAKPOINT_SIZE * 2.0),
    );
    crate::envelope_keys::describe_breakpoint(
        ui,
        id,
        rect,
        lane.doc.settings().time_signatures.to_bbs(at).to_string(),
        shell.keyboard_breakpoint_text(at).unwrap_or_default(),
        keys.selected == Some(at),
    );
    Some(id)
}

/// How wide the lane header's small buttons are, in points.
pub(super) const BUTTON: f32 = 14.0;

/// How tall the choosers are, in points.
pub(super) const CHOOSER_HEIGHT: f32 = 15.0;

/// Whether Alt (Windows) or Cmd (Mac) is held: the lane and hide buttons then
/// act on every lane (the automation spec, R25, R26).
fn all_key(ui: &Ui) -> bool {
    bypass(ui.input(|i| i.modifiers))
}

/// A small header button with a glyph, `lit` while it stands for something
/// switched on. Returns whether it was clicked.
fn small_button(
    ui: &mut Ui,
    theme: &Theme,
    id: egui::Id,
    rect: Rect,
    (glyph, name): (&str, &str),
    lit: Option<bool>,
) -> bool {
    let c = &theme.colors;
    let response = ui.interact(rect, id, Sense::click());
    let on = lit == Some(true);
    let fill = if on {
        c.accent.0
    } else if response.hovered() {
        c.control_hover.0
    } else {
        c.control.0
    };
    let ink = if on { c.accent_text.0 } else { c.text.0 };
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, theme.sizes.corner, fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme.small_font(),
        ink,
    );
    crate::focus::register(ui.ctx(), response.id, bl_shell::Area::MainView, None);
    match lit {
        Some(on) => {
            response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, name))
        }
        None => response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name)),
    }
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(name.to_owned());
        if let Some(on) = lit {
            node.set_role(Role::Switch);
            node.set_toggled(if on {
                egui::accesskit::Toggled::True
            } else {
                egui::accesskit::Toggled::False
            });
        }
    });
    response.clicked()
}

/// The ID of lane `number`'s lane button (main lane) or hide button.
#[must_use]
pub(crate) fn lane_button_id(track: TrackId, number: usize) -> egui::Id {
    egui::Id::new(("arrangement-automation-lane-button", track, number))
}

/// The ID of lane `number`'s hide button.
#[must_use]
pub(crate) fn hide_button_id(track: TrackId, number: usize) -> egui::Id {
    egui::Id::new(("arrangement-automation-hide-button", track, number))
}

/// The ID of a track's toggle that shows or hides its lanes of its own.
#[must_use]
pub(crate) fn lanes_toggle_id(track: TrackId) -> egui::Id {
    egui::Id::new(("arrangement-automation-lanes-toggle", track))
}

/// A lane's device and parameter choosers, side by side in `rect` (the
/// automation spec, R14, R27): the main lane's keep the salts they always
/// had, the other lanes' add their number.
pub(super) fn lane_choosers(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    (track, number): (TrackId, usize),
    rect: Rect,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    let choice = shell
        .arrangement()
        .lane_choice(track, number)
        .unwrap_or_default();
    let target = choice.target;
    let mut header = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    header.set_clip_rect(rect.expand(3.0));
    let gap = 3.0;
    let each = ((rect.width() - gap) / 2.0).max(10.0);
    let device_rect = Rect::from_min_size(rect.min, vec2(each, rect.height()));
    let control_rect = Rect::from_min_size(
        pos2(rect.left() + each + gap, rect.top()),
        vec2(each, rect.height()),
    );

    let current_device = target.map_or(AutomationDevice::None, device_of);
    let mut devices = device_choices(doc, track);
    if choice.only_automated {
        keep_automated_devices(&mut devices, doc, track);
    }
    mark_devices(&mut devices, doc, track, theme);
    push_automated_only(&mut devices, choice.only_automated);
    // The choosers are Tab-reachable: the Accessibility spec, R4.
    let suffix = if number == 0 {
        track.0.to_string()
    } else {
        format!("{}-{number}", track.0)
    };
    let device_salt = format!("arrangement-automation-device-{suffix}");
    let control_salt = format!("arrangement-automation-control-{suffix}");
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
        if chosen == AutomationDevice::AutomatedOnly {
            invoke(
                events,
                view_ids::SHOW_AUTOMATED_ONLY,
                json!({ "track": track, "lane": number, "on": !choice.only_automated }),
            );
        } else {
            invoke(
                events,
                view_ids::SHOW_AUTOMATION,
                json!({ "track": track, "lane": number, "target": default_target(shell, doc, chosen) }),
            );
        }
    }

    let mut controls = control_choices(shell, doc, current_device);
    if choice.only_automated {
        keep_automated_controls(&mut controls, doc, track);
    }
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
            json!({ "track": track, "lane": number, "target": chosen }),
        );
    }
    // Right-clicking a chooser opens the lane header's menu (R28).
    for salt in [device_salt.as_str(), control_salt.as_str()] {
        let rect = if salt == device_salt {
            device_rect
        } else {
            control_rect
        };
        let response = header.interact(rect, egui::Id::new(("chooser", salt)), Sense::click());
        lane_menu(&header, &response, shell, (track, number), events);
    }
}

/// The lane button (R25): it moves the envelope the lane's choosers show into
/// a lane of its own, or with Alt/Cmd every automated parameter of the track
/// that has none. Hidden while the choosers show None or a parameter that
/// already has a lane of its own.
pub(super) fn lane_button(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    (track, number): (TrackId, usize),
    rect: Rect,
    events: &mut Vec<UiEvent>,
) {
    let view = shell.arrangement();
    let Some(target) = view
        .lane_choice(track, number)
        .and_then(|choice| choice.target)
    else {
        return;
    };
    if view
        .own_lanes(track)
        .iter()
        .any(|lane| lane.target == Some(target))
    {
        return;
    }
    if small_button(
        ui,
        theme,
        lane_button_id(track, number),
        rect,
        ("+", "Show Automation in New Lane"),
        None,
    ) {
        invoke(
            events,
            view_ids::ADD_AUTOMATION_LANE,
            json!({ "track": track, "lane": number, "all": all_key(ui) }),
        );
    }
}

/// The toggle in a track's header that shows or hides its lanes of its own
/// (R26).
pub(super) fn lanes_toggle(
    ui: &mut Ui,
    theme: &Theme,
    track: TrackId,
    rect: Rect,
    shown: bool,
    events: &mut Vec<UiEvent>,
) {
    let name = if shown {
        "Hide Automation Lanes"
    } else {
        "Show Automation Lanes"
    };
    if small_button(
        ui,
        theme,
        lanes_toggle_id(track),
        rect,
        ("≡", name),
        Some(shown),
    ) {
        invoke(
            events,
            view_ids::SHOW_AUTOMATION_LANES,
            json!({ "tracks": [track], "on": !shown }),
        );
    }
}

/// Draws the header of lane `number` (1 and up) of a track in the header
/// column: its choosers, its hide button and its context menu.
pub(super) fn own_lane_header(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    lane: (TrackId, usize),
    rect: Rect,
    events: &mut Vec<UiEvent>,
) {
    let (track, number) = lane;
    let background = ui.interact(
        rect,
        egui::Id::new(("arrangement-automation-header", track, number)),
        Sense::click(),
    );
    let choosers = Rect::from_min_size(
        pos2(rect.left(), rect.top() + 2.0),
        vec2(rect.width(), CHOOSER_HEIGHT),
    );
    lane_choosers(ui, shell, theme, lane, choosers, events);
    let hide = Rect::from_min_size(
        pos2(rect.left(), choosers.bottom() + 5.0),
        vec2(BUTTON, BUTTON),
    );
    if small_button(
        ui,
        theme,
        hide_button_id(track, number),
        hide,
        ("×", "Hide Automation Lane"),
        None,
    ) {
        invoke(
            events,
            view_ids::REMOVE_AUTOMATION_LANE,
            json!({ "track": track, "lane": number, "and_after": all_key(ui) }),
        );
    }
    lane_menu(ui, &background, shell, lane, events);
}

/// The lane header's context menu (R28): its view items, then the two that
/// clear envelopes. Their order and names are Beatlayer's own.
pub(super) fn lane_menu(
    ui: &Ui,
    response: &egui::Response,
    shell: &Shell,
    (track, number): (TrackId, usize),
    events: &mut Vec<UiEvent>,
) {
    let view = shell.arrangement();
    let doc = shell.editor().document();
    let choice = view.lane_choice(track, number).unwrap_or_default();
    let has_lanes = !view.own_lanes(track).is_empty();
    let owner = doc.track(track);
    let device = choice.target.map(device_of);
    // The device the lane shows, for Clear Envelopes: a device, or the
    // track's mixer. A lane showing a clip's controls or None has none.
    let clearing = match device {
        Some(AutomationDevice::Mixer) => Some((None, "Track Mixer".to_owned())),
        Some(AutomationDevice::Device(id)) => doc
            .device(id)
            .map(|device| (Some(id), device_title(device))),
        _ => None,
    };
    let menu = |ui: &mut Ui| {
        if ui
            .selectable_label(choice.only_automated, "Show Automated Parameters Only")
            .clicked()
        {
            invoke(
                events,
                view_ids::SHOW_AUTOMATED_ONLY,
                json!({ "track": track, "lane": number, "on": !choice.only_automated }),
            );
            ui.close();
        }
        for (label, on) in [("Show All Lanes", true), ("Hide All Lanes", false)] {
            if ui
                .add_enabled(has_lanes, egui::Button::new(label))
                .clicked()
            {
                invoke(
                    events,
                    view_ids::SHOW_AUTOMATION_LANES,
                    json!({ "tracks": [track], "on": on }),
                );
                ui.close();
            }
        }
        ui.separator();
        let any = owner.is_some_and(|owner| owner.automation().next().is_some());
        if ui
            .add_enabled(any, egui::Button::new("Clear All Envelopes"))
            .clicked()
        {
            invoke(events, "automation.clear_track", json!({ "track": track }));
            ui.close();
        }
        let label = clearing.as_ref().map_or_else(
            || "Clear Envelopes".to_owned(),
            |(_, name)| format!("Clear Envelopes of {name}"),
        );
        let has = clearing.is_some()
            && owner.is_some_and(|owner| {
                owner
                    .automation()
                    .any(|(target, _)| Some(device_of(*target)) == device)
            });
        if ui.add_enabled(has, egui::Button::new(label)).clicked() {
            invoke(
                events,
                "automation.clear_device",
                json!({ "track": track, "device": clearing.as_ref().and_then(|(id, _)| *id) }),
            );
            ui.close();
        }
    };
    // The menu opens on a right-click, or on the keys that open a focused
    // item's menu (S-CTX-002). The choosers' own popup holds the response's
    // popup ID, so this one has the keyboard menu's.
    if response.secondary_clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        crate::menus::open_keyboard_context_menu(ui.ctx(), response.id, at);
    } else if response.has_focus() && crate::menus::context_menu_keys(ui) {
        crate::menus::open_keyboard_context_menu(
            ui.ctx(),
            response.id,
            response.rect.left_bottom(),
        );
    }
    crate::menus::keyboard_context_menu(response, menu);
}

/// The invocation that shows the previous or next parameter in lane `number`
/// of a track (S-A11Y-026, S-A11Y-027): the one with an envelope, or with
/// `all`, any the choosers list.
pub(crate) fn cycle_lane(
    shell: &Shell,
    (track, number): (TrackId, usize),
    forward: bool,
    all: bool,
) -> Option<Invocation> {
    let doc = shell.editor().document();
    let list = bl_shell::arrangement::lanes::chooser_order(doc, shell.devices(), track);
    let owner = doc.track(track)?;
    let target = bl_shell::arrangement::lanes::cycle_target(
        &list,
        shell
            .arrangement()
            .lane_choice(track, number)
            .and_then(|choice| choice.target),
        forward,
        |target| all || owner.automation_for(target).is_some(),
    )?;
    Some(Invocation::new(
        view_ids::SHOW_AUTOMATION,
        json!({ "track": track, "lane": number, "target": target }),
    ))
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
        AutomationTarget::Clip { param } => clip_choice(param).label,
        AutomationTarget::MidiCc { number } => {
            format!("{number} {}", AutomationTarget::controller_name(number))
        }
        AutomationTarget::MidiPitchBend => "Pitch Bend".to_owned(),
        AutomationTarget::MidiChannelPressure => "Channel Pressure".to_owned(),
    }
}
