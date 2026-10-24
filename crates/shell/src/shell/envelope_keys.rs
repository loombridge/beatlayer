//! Editing envelopes from the keyboard (WP-6.26; the automation editing
//! spec, R20 to R29): the insert marker along the envelope of the automation
//! lane that has the keyboard or of the Clip View's Envelope Editor, and
//! making, selecting, moving and deleting its breakpoints.
//!
//! Selecting a breakpoint opens an edit: a transaction held open, so every
//! change made until the breakpoint is let go (Enter keeps it, Esc drops it)
//! is one undo step. It also ends when another command edits the set or the
//! keyboard focus leaves. The changes themselves are the document's own
//! breakpoint commands, run as they are for the pointer.

use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Breakpoint, ClipId, Document, ParamDescriptor, TrackId};
use bl_shell_spec::envelope_keys::{
    self as ids, ArrowKey, EnvelopeKeyState, EnvelopeKeys, MarkerStep, MoveMarkerArgs,
    MoveSelectedArgs, SelectAdjacentArgs, SelectBreakpointArgs, SelectEnvelopeBreakpointArgs,
    SetValueArgs,
};
use bl_shell_spec::notes::ClipTab;

use super::*;
use crate::arrangement::automation::{Travel, rest_of, selected_breakpoints, share_scale, travel};
use crate::mixer_controls;

/// The name of the undo step an edit that begins with selecting a
/// breakpoint makes.
const EDIT_STEP: &str = "Edit Breakpoint";

/// The envelope the envelope keys act on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Keyed {
    /// A track's automation lane.
    Lane {
        track: TrackId,
        target: AutomationTarget,
    },
    /// The envelope the Clip View's Envelope Editor shows.
    Clip {
        clip: ClipId,
        target: AutomationTarget,
    },
}

impl Keyed {
    fn kind(self) -> EnvelopeKeys {
        match self {
            Self::Lane { .. } => EnvelopeKeys::Lane,
            Self::Clip { .. } => EnvelopeKeys::Clip,
        }
    }
}

/// How an envelope's values read, parse and step with an arrow key: what its
/// parameter's control shows.
enum Units {
    Volume,
    Send,
    Pan,
    Switch,
    /// A stepped macro, whose positions are `k / (steps − 1)`.
    Steps(u8),
    /// A parameter with a descriptor: a device's, or a clip setting's.
    Param(Box<ParamDescriptor>),
    /// A clip envelope's modulation, a share of the parameter's travel.
    Share,
    /// A MIDI controller, in its own numbers.
    Midi,
}

impl Units {
    /// The units of a lane's parameter.
    fn of_lane(
        doc: &Document,
        devices: &crate::devices::DeviceView,
        target: AutomationTarget,
    ) -> Self {
        match travel(doc, devices, target) {
            Travel::Volume => Self::Volume,
            Travel::Send => Self::Send,
            Travel::Pan => Self::Pan,
            Travel::Switch => Self::Switch,
            Travel::Steps { steps } => Self::Steps(steps),
            Travel::Linear { .. } => match target {
                AutomationTarget::Device { device, param } => doc
                    .device(device)
                    .and_then(|device| devices.description(&device.kind))
                    .and_then(|description| description.param(param))
                    .map_or(Self::Share, |descriptor| {
                        Self::Param(Box::new(descriptor.clone()))
                    }),
                _ => Self::Share,
            },
        }
    }

    /// The units of a clip envelope's values.
    fn of_clip(target: AutomationTarget) -> Self {
        match target {
            AutomationTarget::Clip { param } => Self::Param(Box::new(param.descriptor())),
            _ if target.is_midi() => Self::Midi,
            _ => Self::Share,
        }
    }

    fn format(&self, value: f64) -> String {
        match self {
            Self::Volume | Self::Send => mixer_controls::volume_spoken(value),
            Self::Pan => mixer_controls::pan_text(value),
            Self::Switch => (if value >= 0.5 { "off" } else { "on" }).to_owned(),
            Self::Steps(_) | Self::Share => format!("{:.0}%", value * 100.0),
            Self::Param(descriptor) => descriptor.format(value),
            Self::Midi => format!("{value:.0}"),
        }
    }

    /// The number `text` reads as, within the control's range, or `None`.
    fn parse(&self, text: &str, scale: Travel) -> Option<f64> {
        let text = text.trim();
        let number = |text: &str| text.trim().parse::<f64>().ok().filter(|n| n.is_finite());
        let value = match self {
            Self::Volume | Self::Send => {
                let lower = text.to_lowercase();
                let db = lower.trim_end_matches("db").trim();
                if db.ends_with("inf") {
                    0.0
                } else {
                    mixer_controls::volume_gain(number(db)?)
                }
            }
            Self::Pan => {
                let upper = text.to_uppercase();
                match upper.as_str() {
                    "C" => 0.0,
                    _ => {
                        let side = upper.chars().last()?;
                        let amount = number(&upper[..upper.len() - side.len_utf8()])?;
                        match side {
                            'L' => -amount / mixer_controls::PAN_STEPS,
                            'R' => amount / mixer_controls::PAN_STEPS,
                            _ => return None,
                        }
                    }
                }
            }
            Self::Switch => match text.to_lowercase().as_str() {
                "on" | "1" => 0.0,
                "off" | "0" => 1.0,
                _ => return None,
            },
            Self::Steps(_) | Self::Share => number(text.trim_end_matches('%'))? / 100.0,
            Self::Param(descriptor) => descriptor.parse(text)?,
            Self::Midi => number(text)?,
        };
        Some(scale.value(scale.position(value)))
    }

    /// The value `steps` arrow-key presses from `value`: one step of a
    /// stepped control, or 1% of its travel (the device framework spec,
    /// R17), within its range.
    fn step(&self, value: f64, steps: i32, scale: Travel) -> f64 {
        let by = f64::from(steps);
        match self {
            Self::Volume => mixer_controls::step_volume(value, steps, false),
            Self::Send => mixer_controls::step_send(value, steps, false),
            Self::Pan => mixer_controls::step_pan(value, steps),
            Self::Switch => f64::from(u8::from(steps > 0)),
            Self::Steps(count) => {
                let at = scale.position(value) + by / f64::from((*count).max(2) - 1);
                scale.value(at)
            }
            Self::Param(descriptor) => {
                if descriptor.is_stepped() {
                    descriptor.clamp(value + by)
                } else {
                    descriptor.denormalize(descriptor.normalize(value) + by * 0.01)
                }
            }
            Self::Share | Self::Midi => scale.value(scale.position(value) + by * 0.01),
        }
    }
}

impl Shell {
    /// The envelope the envelope keys act on: the Envelope Editor while the
    /// Clip View's Envelopes tab has the keyboard, else the automation lane
    /// that is selected, else the Envelope Editor if it shows (the keys
    /// work for an agent whatever has the focus).
    pub(super) fn keyed(&self) -> Option<Keyed> {
        let doc = self.editor.document();
        let clip = || {
            if self.clip_view.tab() != ClipTab::Envelopes {
                return None;
            }
            let clip = self.shown_clip()?;
            let target = self.clip_view.envelope()?;
            let track = doc.clip_track(clip)?;
            doc.check_clip_target(track, doc.clip(clip)?, target).ok()?;
            Some(Keyed::Clip { clip, target })
        };
        let lane = || {
            self.arrangement
                .keyboard_lane()
                .map(|(track, target)| Keyed::Lane { track, target })
        };
        // The keys are the window in front's (second-window R10).
        let window = self.acting_window();
        if self.focused_area_of(window) == Area::Detail && self.detail_of(window).focus_in_clip() {
            clip().or_else(lane)
        } else {
            lane().or_else(clip)
        }
    }

    /// Which envelope has the keyboard, for the UI to draw the insert marker
    /// and the selected breakpoint on.
    #[must_use]
    pub fn keyboard_envelope(&self) -> Option<EnvelopeKeyState> {
        self.keyed().map(|keyed| self.key_state(keyed))
    }

    /// The value of the breakpoint at `at` in the envelope that has the
    /// keyboard, as its parameter reads it, for the screen reader.
    #[must_use]
    pub fn keyboard_breakpoint_text(&self, at: Ticks) -> Option<String> {
        let keyed = self.keyed()?;
        let point = self
            .envelope_points(keyed)
            .into_iter()
            .find(|point| point.at == at)?;
        Some(self.key_units(keyed).format(point.value))
    }

    /// The envelope an Undo or Redo of a breakpoint edit follows: the one
    /// that has the keyboard, or the shown lane of the one selected track.
    fn followed_envelope(&self) -> Option<Keyed> {
        self.keyed().or_else(|| {
            let track = *self.arrangement.selected_tracks().first()?;
            let target = self
                .arrangement
                .automation_mode()
                .then(|| self.arrangement.automation_lane(track))
                .flatten()?;
            Some(Keyed::Lane { track, target })
        })
    }

    fn envelope_points(&self, keyed: Keyed) -> Vec<Breakpoint> {
        let doc = self.editor.document();
        match keyed {
            Keyed::Lane { track, target } => doc
                .track(track)
                .and_then(|track| track.automation_for(&target))
                .map(|automation| automation.envelope.points().collect())
                .unwrap_or_default(),
            Keyed::Clip { clip, target } => doc
                .clip(clip)
                .and_then(|clip| clip.envelope(&target))
                .map(|envelope| envelope.envelope.points().collect())
                .unwrap_or_default(),
        }
    }

    fn envelope_marker(&self, keyed: Keyed) -> Ticks {
        match keyed {
            Keyed::Lane { .. } => self.arrangement.insert_marker(),
            Keyed::Clip { clip, .. } => self.clip_view.envelope_marker(clip),
        }
    }

    /// The places the insert marker can go: from 1.1.1, and in the Envelope
    /// Editor to the end of what it shows.
    fn marker_span(&self, keyed: Keyed) -> (Ticks, Ticks) {
        let Keyed::Clip { clip, target } = keyed else {
            return (Ticks::ZERO, Ticks(i64::MAX));
        };
        let doc = self.editor.document();
        let Some(clip) = doc.clip(clip) else {
            return (Ticks::ZERO, Ticks::ZERO);
        };
        // A linked envelope runs from the clip's Start marker, an unlinked
        // one from the clip's start (the clip envelopes spec, R2).
        let linked = clip
            .envelope(&target)
            .is_none_or(|envelope| envelope.linked);
        let start = match &clip.data {
            ClipData::Midi(midi) if linked => midi.start,
            _ => Ticks::ZERO,
        };
        (start, start + clip.length)
    }

    fn set_envelope_marker(&mut self, keyed: Keyed, at: Ticks) {
        let (low, high) = self.marker_span(keyed);
        let at = at.clamp(low, high.max(low));
        match keyed {
            Keyed::Lane { .. } => {
                if self.arrangement.move_insert_marker(at) {
                    // Spec: control-bar-transport R15. The engine's insert
                    // marker is where Play starts.
                    self.audio.set_insert_marker(at);
                }
            }
            Keyed::Clip { clip, .. } => self.clip_view.set_envelope_marker(clip, at),
        }
    }

    fn envelope_selected(&self, keyed: Keyed) -> Option<Ticks> {
        match keyed {
            Keyed::Lane { track, target } => self
                .arrangement
                .selected_breakpoint()
                .filter(|(t, g, _)| *t == track && *g == target)
                .map(|(_, _, at)| at),
            Keyed::Clip { clip, target } => self
                .clip_view
                .envelope_breakpoint()
                .filter(|(c, g, _)| *c == clip && *g == target)
                .map(|(_, _, at)| at),
        }
    }

    /// Selects the breakpoint at `at` of `keyed`, or none, moving the insert
    /// marker to it. Selecting opens the edit, and letting go ends it.
    fn select_envelope_breakpoint(&mut self, keyed: Keyed, at: Option<Ticks>) {
        match keyed {
            Keyed::Lane { track, target } => {
                self.arrangement.select_breakpoint(track, target, at);
            }
            Keyed::Clip { clip, target } => {
                self.clip_view.select_envelope_breakpoint(clip, target, at);
            }
        }
        match at {
            Some(at) => {
                self.begin_key_edit(keyed, at);
                self.set_envelope_marker(keyed, at);
            }
            None => self.end_key_edit(),
        }
    }

    /// Opens the edit a selected breakpoint begins: its changes are one undo
    /// step. `origin` is where the insert marker was.
    fn begin_key_edit(&mut self, keyed: Keyed, origin: Ticks) {
        if self.key_edit.is_none() {
            self.begin_gesture(EDIT_STEP);
            self.key_edit = Some((keyed.kind(), origin));
        }
    }

    /// Ends the edit, keeping its changes as one undo step.
    pub(super) fn end_key_edit(&mut self) {
        if self.key_edit.take().is_some() {
            self.end_gesture();
            self.arrangement.deselect_breakpoint();
            self.clip_view.deselect_envelope_breakpoint();
        }
    }

    /// Drops the edit: the set goes back to how it was when the first
    /// breakpoint was selected, and nothing is selected.
    fn cancel_key_edit(&mut self) {
        if self.key_edit.take().is_some() {
            self.gesture = false;
            self.editor.cancel().ok();
            self.document_changed();
            self.touch();
        }
    }

    /// Ends the edit when the selection that began it has gone.
    pub(super) fn sync_key_edit(&mut self) {
        if let Some((kind, _)) = self.key_edit {
            let selected = match kind {
                EnvelopeKeys::Lane => self.arrangement.selected_breakpoint().is_some(),
                EnvelopeKeys::Clip => {
                    self.clip_view
                        .envelope_breakpoint()
                        .is_some_and(|(clip, target, _)| {
                            self.shown_clip() == Some(clip)
                                && self.clip_view.envelope() == Some(target)
                        })
                }
            };
            if !selected {
                self.end_key_edit();
            }
        }
    }

    /// What a command that begins with `id` leaves an edit alone for: the
    /// envelope keys' own, and the ones that change only the view.
    pub(super) fn keeps_key_edit(&self, id: &str) -> bool {
        ids::handles(id)
            || self
                .registry
                .get(id)
                .is_some_and(|spec| spec.permission == Some(bl_commands::Permission::Read))
    }

    /// The value of the envelope at `at` as its parameter reads it, or the
    /// parameter's value where there is no envelope.
    fn envelope_value(&self, keyed: Keyed, at: Ticks) -> f64 {
        let doc = self.editor.document();
        let envelope = match keyed {
            Keyed::Lane { track, target } => doc
                .track(track)
                .and_then(|track| track.automation_for(&target))
                .map(|automation| automation.envelope.clone()),
            Keyed::Clip { clip, target } => doc
                .clip(clip)
                .and_then(|clip| clip.envelope(&target))
                .map(|envelope| envelope.envelope.clone()),
        };
        if let Some(value) = envelope.and_then(|envelope| envelope.value_at(at)) {
            return value;
        }
        match keyed {
            Keyed::Lane { track, target } => {
                crate::modulation::control_value(doc, &self.devices, track, &target)
                    .unwrap_or_else(|| self.key_scale(keyed).value(0.5))
            }
            Keyed::Clip { target, .. } => rest_of(target),
        }
    }

    fn key_scale(&self, keyed: Keyed) -> Travel {
        match keyed {
            Keyed::Lane { target, .. } => travel(self.editor.document(), &self.devices, target),
            Keyed::Clip { target, .. } => share_scale(target),
        }
    }

    fn key_units(&self, keyed: Keyed) -> Units {
        match keyed {
            Keyed::Lane { target, .. } => {
                Units::of_lane(self.editor.document(), &self.devices, target)
            }
            Keyed::Clip { target, .. } => Units::of_clip(target),
        }
    }

    fn key_state(&self, keyed: Keyed) -> EnvelopeKeyState {
        let marker = self.envelope_marker(keyed);
        EnvelopeKeyState {
            envelope: keyed.kind(),
            marker,
            selected: self.envelope_selected(keyed),
            value: self
                .key_units(keyed)
                .format(self.envelope_value(keyed, marker)),
        }
    }

    // The document's own commands for an envelope's breakpoints.

    fn set_breakpoint_command(keyed: Keyed, at: Ticks, value: f64) -> (&'static str, Value) {
        match keyed {
            Keyed::Lane { track, target } => (
                "automation.set_breakpoint",
                json!({ "track": track, "target": target, "at": at, "value": value }),
            ),
            Keyed::Clip { clip, target } => (
                "clip.set_envelope_breakpoint",
                json!({ "clip": clip, "target": target, "at": at, "value": value }),
            ),
        }
    }

    fn move_breakpoint_command(keyed: Keyed, at: Ticks, to: Ticks) -> (&'static str, Value) {
        match keyed {
            Keyed::Lane { track, target } => (
                "automation.move_breakpoint",
                json!({ "track": track, "target": target, "at": at, "to": to }),
            ),
            Keyed::Clip { clip, target } => (
                "clip.move_envelope_breakpoint",
                json!({ "clip": clip, "target": target, "at": at, "to": to }),
            ),
        }
    }

    fn delete_breakpoint_command(keyed: Keyed, at: Ticks) -> (&'static str, Value) {
        match keyed {
            Keyed::Lane { track, target } => (
                "automation.delete_breakpoint",
                json!({ "track": track, "target": target, "at": at }),
            ),
            Keyed::Clip { clip, target } => (
                "clip.delete_envelope_breakpoint",
                json!({ "clip": clip, "target": target, "at": at }),
            ),
        }
    }

    fn no_envelope(id: &str) -> ShellError {
        ShellError::Unavailable {
            command: id.to_owned(),
            reason: "no automation lane is selected and the Envelope Editor shows no envelope; \
                     select a lane, or choose an envelope in the Clip View's Envelopes tab"
                .to_owned(),
        }
    }

    fn not_selected(id: &str) -> ShellError {
        ShellError::Unavailable {
            command: id.to_owned(),
            reason: "no breakpoint is selected".to_owned(),
        }
    }

    /// Whether the envelope keys' command `id` can run now: the keys it
    /// answers only apply while their state holds, so the key falls through
    /// to the view's own when they don't (the automation editing spec, R22
    /// to R24).
    pub(super) fn can_envelope_key(&self, id: &str) -> bool {
        if matches!(id, ids::SELECT_BREAKPOINT | ids::SELECT_ENVELOPE_BREAKPOINT) {
            return true;
        }
        let Some(keyed) = self.keyed() else {
            return false;
        };
        let selected = self.envelope_selected(keyed);
        match id {
            ids::ENTER | ids::SET_VALUE => true,
            ids::MOVE_MARKER => selected.is_none(),
            ids::CANCEL | ids::MOVE_SELECTED | ids::SELECT_ADJACENT => selected.is_some(),
            ids::DELETE_BREAKPOINT => {
                let points = self.envelope_points(keyed);
                let at = selected.unwrap_or_else(|| self.envelope_marker(keyed));
                points.iter().any(|point| point.at == at)
                    || (selected.is_none() && self.deletes_range(keyed))
            }
            ids::DELETE => !self.envelope_points(keyed).is_empty(),
            _ => false,
        }
    }

    /// Whether Delete deletes the Envelope Editor's time selection: it has
    /// one, and no breakpoint is selected (the clip envelopes spec, R12).
    fn deletes_range(&self, keyed: Keyed) -> bool {
        matches!(keyed, Keyed::Clip { clip, .. } if self.clip_view.envelope_time(clip).is_some())
    }

    /// Runs one of the envelope keys' commands (the automation editing spec,
    /// R20 to R29).
    pub(super) fn run_envelope_keys(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let result = match id {
            ids::SELECT_BREAKPOINT => {
                let SelectBreakpointArgs { track, target, at } = Self::args(id, args)?;
                let doc = self.editor.document();
                if doc.track(track).is_none() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("no track has the ID {}", track.0),
                    });
                }
                if self.arrangement.automation_lane(track) != Some(target) {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "the track's automation lane doesn't show that parameter; show \
                                 it with `arrangement.show_automation`, with Automation Mode on"
                            .to_owned(),
                    });
                }
                let keyed = Keyed::Lane { track, target };
                if self.arrangement.keyboard_lane() != Some((track, target)) {
                    self.run_command(
                        "arrangement.select_tracks",
                        json!({ "tracks": [track], "automation_lane": true }),
                    )?;
                }
                if self.arrangement.keyboard_lane().is_none() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "the track's automation lane isn't showing; turn Automation \
                                 Mode on and unfold the track"
                            .to_owned(),
                    });
                }
                self.select_envelope_breakpoint(keyed, at);
                keyed
            }
            ids::SELECT_ENVELOPE_BREAKPOINT => {
                let SelectEnvelopeBreakpointArgs { clip, target, at } = Self::args(id, args)?;
                let doc = self.editor.document();
                let valid = doc.clip_track(clip).is_some_and(|track| {
                    doc.clip(clip)
                        .is_some_and(|c| doc.check_clip_target(track, c, target).is_ok())
                });
                if !valid {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the clip or its track doesn't have that parameter".to_owned(),
                    });
                }
                if self.shown_clip() != Some(clip) || self.clip_view.envelope() != Some(target) {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "the Envelope Editor isn't showing that clip's envelope for the \
                                 parameter; show it with `view.clip_envelope`"
                            .to_owned(),
                    });
                }
                let keyed = Keyed::Clip { clip, target };
                self.select_envelope_breakpoint(keyed, at);
                keyed
            }
            ids::MOVE_MARKER => {
                let MoveMarkerArgs { direction, by } = Self::args(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                if self.envelope_selected(keyed).is_some() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "a breakpoint is selected; `envelope.move_selected` moves it"
                            .to_owned(),
                    });
                }
                let earlier = match direction {
                    ArrowKey::Left => true,
                    ArrowKey::Right => false,
                    ArrowKey::Up | ArrowKey::Down => {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: "`direction` is `left` or `right`".to_owned(),
                        });
                    }
                };
                let marker = self.envelope_marker(keyed);
                let to = match by {
                    MarkerStep::Grid => {
                        let (before, after) = self
                            .arrangement
                            .lines_around(self.editor.document(), marker);
                        if earlier { before } else { after }
                    }
                    MarkerStep::Breakpoint => {
                        let points = self.envelope_points(keyed);
                        if earlier {
                            points.iter().rev().find(|p| p.at < marker).map(|p| p.at)
                        } else {
                            points.iter().find(|p| p.at > marker).map(|p| p.at)
                        }
                        .unwrap_or(marker)
                    }
                };
                self.set_envelope_marker(keyed, to);
                keyed
            }
            ids::ENTER => {
                Self::args::<bl_commands::NoArgs>(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                if self.envelope_selected(keyed).is_some() {
                    // Keeps the changes and deselects.
                    self.select_envelope_breakpoint(keyed, None);
                } else {
                    let at = self.envelope_marker(keyed);
                    self.begin_key_edit(keyed, at);
                    if !self.envelope_points(keyed).iter().any(|p| p.at == at) {
                        let value = self.envelope_value(keyed, at);
                        let (command, args) = Self::set_breakpoint_command(keyed, at, value);
                        if let Err(error) = self.run_command(command, args) {
                            self.cancel_key_edit();
                            return Err(error);
                        }
                    }
                    self.select_envelope_breakpoint(keyed, Some(at));
                }
                keyed
            }
            ids::CANCEL => {
                Self::args::<bl_commands::NoArgs>(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                if self.envelope_selected(keyed).is_none() {
                    return Err(Self::not_selected(id));
                }
                let origin = self.key_edit.map(|(_, origin)| origin);
                self.cancel_key_edit();
                self.select_envelope_breakpoint(keyed, None);
                if let Some(origin) = origin {
                    self.set_envelope_marker(keyed, origin);
                }
                keyed
            }
            ids::MOVE_SELECTED => {
                let MoveSelectedArgs { direction } = Self::args(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                let at = self
                    .envelope_selected(keyed)
                    .ok_or_else(|| Self::not_selected(id))?;
                let points = self.envelope_points(keyed);
                let point = points
                    .iter()
                    .find(|p| p.at == at)
                    .copied()
                    .ok_or_else(|| Self::not_selected(id))?;
                match direction {
                    ArrowKey::Left | ArrowKey::Right => {
                        let (before, after) =
                            self.arrangement.lines_around(self.editor.document(), at);
                        let to = if direction == ArrowKey::Left {
                            before
                        } else {
                            after
                        };
                        // Not past its neighbours, nor before 1.1.1.
                        let blocked = to < Ticks::ZERO
                            || points
                                .iter()
                                .any(|p| p.at != at && (at.min(to)..=at.max(to)).contains(&p.at));
                        if to != at && !blocked {
                            let (command, args) = Self::move_breakpoint_command(keyed, at, to);
                            self.run_command(command, args)?;
                            self.select_envelope_breakpoint(keyed, Some(to));
                        }
                    }
                    ArrowKey::Up | ArrowKey::Down => {
                        let steps = if direction == ArrowKey::Up { 1 } else { -1 };
                        let value =
                            self.key_units(keyed)
                                .step(point.value, steps, self.key_scale(keyed));
                        if value != point.value {
                            let (command, args) = Self::set_breakpoint_command(keyed, at, value);
                            self.run_command(command, args)?;
                        }
                    }
                }
                keyed
            }
            ids::SELECT_ADJACENT => {
                let SelectAdjacentArgs { reverse } = Self::args(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                let at = self
                    .envelope_selected(keyed)
                    .ok_or_else(|| Self::not_selected(id))?;
                let points = self.envelope_points(keyed);
                let next = if reverse {
                    points.iter().rev().find(|p| p.at < at)
                } else {
                    points.iter().find(|p| p.at > at)
                };
                if let Some(next) = next {
                    self.select_envelope_breakpoint(keyed, Some(next.at));
                }
                keyed
            }
            ids::DELETE_BREAKPOINT => {
                Self::args::<bl_commands::NoArgs>(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                let selected = self.envelope_selected(keyed);
                let at = selected.unwrap_or_else(|| self.envelope_marker(keyed));
                // Spec: clip-envelopes R12. With a time selected in the
                // Envelope Editor and no breakpoint selected, Delete
                // deletes the selection's range.
                if selected.is_none()
                    && self.deletes_range(keyed)
                    && let Keyed::Clip { clip, target } = keyed
                {
                    self.run_command(
                        bl_shell_spec::clip_envelope_editing::DELETE_RANGE,
                        json!({ "clip": clip, "target": target }),
                    )?;
                } else {
                    if !self.envelope_points(keyed).iter().any(|p| p.at == at) {
                        return Err(ShellError::Unavailable {
                            command: id.to_owned(),
                            reason: "no breakpoint is selected or at the insert marker".to_owned(),
                        });
                    }
                    let (command, args) = Self::delete_breakpoint_command(keyed, at);
                    self.run_command(command, args)?;
                    self.select_envelope_breakpoint(keyed, None);
                }
                keyed
            }
            ids::DELETE => {
                Self::args::<bl_commands::NoArgs>(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                // The edit, if one is open, is kept first: this is a step of
                // its own.
                self.select_envelope_breakpoint(keyed, None);
                let (command, args) = match keyed {
                    Keyed::Lane { track, target } => (
                        "automation.delete",
                        json!({ "track": track, "target": target }),
                    ),
                    Keyed::Clip { clip, target } => (
                        "clip.delete_envelope",
                        json!({ "clip": clip, "target": target }),
                    ),
                };
                self.run_command(command, args)?;
                keyed
            }
            ids::SET_VALUE => {
                let SetValueArgs { text, at } = Self::args(id, args)?;
                let keyed = self.keyed().ok_or_else(|| Self::no_envelope(id))?;
                let units = self.key_units(keyed);
                let scale = self.key_scale(keyed);
                let value = units
                    .parse(&text, scale)
                    .ok_or_else(|| ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("`{text}` isn't a value this parameter reads"),
                    })?;
                self.set_typed_value(id, keyed, at, value, scale)?;
                keyed
            }
            other => return Err(ShellError::UnknownCommand(other.to_owned())),
        };
        self.touch();
        Ok(to_value(&self.key_state(result)))
    }

    /// Gives a breakpoint a typed value: the selected one, with the other
    /// selected breakpoints of a lane moving by the same share of the
    /// travel, or a new one at `at`.
    fn set_typed_value(
        &mut self,
        id: &str,
        keyed: Keyed,
        at: Option<Ticks>,
        value: f64,
        scale: Travel,
    ) -> Result<(), ShellError> {
        let opened = self.key_edit.is_none();
        if opened {
            self.begin_key_edit(keyed, self.envelope_marker(keyed));
        }
        let result = match at {
            Some(at) => {
                let (command, args) = Self::set_breakpoint_command(keyed, at, value);
                self.run_command(command, args).map(|_| {
                    self.set_envelope_marker(keyed, at);
                })
            }
            None => match self.envelope_selected(keyed) {
                None => Err(Self::not_selected(id)),
                Some(at) => {
                    let old = self
                        .envelope_points(keyed)
                        .iter()
                        .find(|p| p.at == at)
                        .map_or(value, |p| p.value);
                    let others = match keyed {
                        Keyed::Lane { track, target } => selected_breakpoints(
                            self.editor.document(),
                            &self.arrangement,
                            track,
                            target,
                        ),
                        Keyed::Clip { .. } => Vec::new(),
                    };
                    if let (Keyed::Lane { track, target }, true) =
                        (keyed, others.len() > 1 && others.contains(&at))
                    {
                        self.run_command(
                            "automation.move_breakpoints",
                            json!({
                                "track": track,
                                "target": target,
                                "at": others,
                                "by": 0,
                                "share": scale.position(value) - scale.position(old),
                            }),
                        )
                        .map(|_| ())
                    } else {
                        let (command, args) = Self::set_breakpoint_command(keyed, at, value);
                        self.run_command(command, args).map(|_| ())
                    }
                }
            },
        };
        if opened {
            // A value typed with nothing selected is one undo step of its own.
            if result.is_ok() {
                self.end_key_edit();
            } else {
                self.cancel_key_edit();
            }
        }
        result
    }

    /// After Undo or Redo: the breakpoint it restored is the focused one,
    /// with the insert marker on it (the automation editing spec, R24).
    pub(super) fn follow_restored_breakpoint(&mut self, before: Option<(Keyed, Vec<Breakpoint>)>) {
        let Some((keyed, before)) = before else {
            return;
        };
        let restored = self
            .envelope_points(keyed)
            .into_iter()
            .find(|point| !before.contains(point));
        if let Some(point) = restored {
            self.set_envelope_marker(keyed, point.at);
            self.touch();
        }
    }

    /// The envelope an Undo or Redo is about to change, with its
    /// breakpoints as they are.
    pub(super) fn envelope_before_history(&self) -> Option<(Keyed, Vec<Breakpoint>)> {
        let keyed = self.followed_envelope()?;
        Some((keyed, self.envelope_points(keyed)))
    }
}
