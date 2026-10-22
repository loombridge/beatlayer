//! Recording automation (WP-2.12): while Automation Arm is on and the
//! transport records, moving a control writes its values into that
//! parameter's envelope.
//!
//! The moves are gathered here as they happen, at the playback position the
//! engine last showed; when recording stops they are written into the
//! envelopes in one undo step ([`Take::commit`]). A *touch* is a run of
//! moves of one control, from the first until the control is released
//! ([`Take::release`]); the envelope's own breakpoints over the range the
//! touch covers are replaced, and the old envelope resumes at its end
//! ([`bl_model::Envelope::touch`]).
//!
//! The recording's mode (the automation spec, R18) is Touch, Live's, which
//! writes a control's values while it is held. In Latch the value a control
//! was moved to holds until the recording stops, and in Write the envelope is
//! written from where the recording started; both carry on over the
//! Arrangement loop's wrap (R19 to R21).

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Command, Document, Editor, EnvelopeKind, MixerParam, SceneId, TrackId,
};

use crate::settings::AutomationRecordMode;

/// The undo step a finished automation recording makes. Live's name for it
/// isn't observed yet.
pub const UNDO_LABEL: &str = "Record Automation";

/// The undo step the moves made over a Session clip that was already playing
/// make, as Live names an overdub's (observed 17 October 2026).
pub const OVERDUB_UNDO_LABEL: &str = "Overdub Record";

/// One run of moves of a control.
struct Touch {
    /// What the control was moved to, at the positions it was moved at, in
    /// position order.
    written: Vec<(Ticks, f64)>,
    /// Where the control was released, once it has been.
    end: Option<Ticks>,
}

/// Notes a move to `value` at `at` in a control's `touches`.
fn write_touch(touches: &mut Vec<Touch>, at: Ticks, value: f64) {
    if let Some(touch) = touches.last_mut()
        && touch.end.is_none()
    {
        match touch.written.last_mut() {
            Some(last) if last.0 == at => {
                last.1 = value;
                return;
            }
            Some(last) if last.0 < at => {
                touch.written.push((at, value));
                return;
            }
            _ => touch.end = touch.written.last().map(|last| last.0),
        }
    }
    touches.push(Touch {
        written: vec![(at, value)],
        end: None,
    });
}

/// Ends `touch`, if it is open, where the control was released at `at`.
fn release_touch(touch: &mut Touch, at: Ticks) {
    if touch.end.is_none() {
        touch.end = touch.written.last().map(|last| last.0.max(at));
    }
}

/// Notes a move in a Latch or Write touch (automation R19): a later move
/// carries the open touch on, and one at the same position as the last
/// replaces its value.
fn write_latch(touches: &mut Vec<Touch>, at: Ticks, value: f64) {
    if let Some(touch) = touches.last_mut().filter(|touch| touch.end.is_none()) {
        match touch.written.last_mut() {
            Some(last) if last.0 == at => last.1 = value,
            _ => touch.written.push((at, value)),
        }
        return;
    }
    touches.push(Touch {
        written: vec![(at, value)],
        end: None,
    });
}

/// The moves of a recording under way.
#[derive(Default)]
pub struct Take {
    touches: BTreeMap<(TrackId, AutomationTarget), Vec<Touch>>,
    /// The value each control had before its first move, which Write starts
    /// the envelope from (automation R20).
    before: BTreeMap<(TrackId, AutomationTarget), f64>,
    /// The mode the recording writes its moves in (automation R18).
    mode: AutomationRecordMode,
    /// Where the recording started, its first frame, which Write starts the
    /// envelope from (automation R20).
    start: Ticks,
    /// Where the Arrangement loop starts, so that a wrap is worked out
    /// (automation R21).
    loop_start: Ticks,
    /// The playhead's position when last seen, so that a wrap is noticed
    /// (automation R21).
    last: Ticks,
}

impl Take {
    /// Starts a recording in `mode`, whose first frame is at `start`, with
    /// the Arrangement loop starting at `loop_start` (automation R18 to R21).
    pub fn start(&mut self, mode: AutomationRecordMode, start: Ticks, loop_start: Ticks) {
        self.mode = mode;
        self.start = start;
        self.loop_start = loop_start;
        self.last = start;
    }

    /// Notes that the playhead is at `at`. When it went back (the loop brace
    /// wrapped it), each open Latch or Write touch carries on from the
    /// loop's start with the value it held, and what earlier passes wrote
    /// from there on is written over (automation R21).
    pub fn advance(&mut self, at: Ticks) {
        let wrapped = at < self.last;
        self.last = at;
        if !wrapped || self.mode == AutomationRecordMode::Touch {
            return;
        }
        let keep_to = self.loop_start.min(at);
        for touch in self
            .touches
            .values_mut()
            .filter_map(|touches| touches.last_mut())
        {
            if touch.end.is_some() {
                continue;
            }
            let Some(&(_, carried)) = touch.written.last() else {
                continue;
            };
            touch.written.retain(|(at, _)| *at < keep_to);
            touch.written.push((keep_to, carried));
        }
    }

    /// Notes that `target` of `track` was moved to `value` at `at`. `before`
    /// is the value the control had before the move, which Write starts the
    /// envelope from. A move after the control's release, or before the
    /// previous move (the loop brace wrapped the playhead), starts a new
    /// touch in Touch; in Latch and Write the touch carries on.
    pub fn write(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        at: Ticks,
        value: f64,
        before: f64,
    ) {
        let key = (track, target);
        if self.mode != AutomationRecordMode::Touch {
            self.advance(at);
        }
        let touches = self.touches.entry(key).or_default();
        match self.mode {
            AutomationRecordMode::Touch => write_touch(touches, at, value),
            AutomationRecordMode::Latch | AutomationRecordMode::Write => {
                write_latch(touches, at, value);
            }
        }
        if self.mode == AutomationRecordMode::Write {
            self.before.entry(key).or_insert(before);
        }
    }

    /// Notes that every control being moved was released at `at`. In Latch
    /// and Write a release doesn't end the touch: its value holds until the
    /// recording stops (automation R19).
    pub fn release(&mut self, at: Ticks) {
        if self.mode != AutomationRecordMode::Touch {
            return;
        }
        for touch in self.touches.values_mut().flatten() {
            release_touch(touch, at);
        }
    }

    /// Writes the take into the set as one undo step, with the controls
    /// still held released at `stop`, and puts every envelope it wrote under
    /// automation again. Returns whether it wrote anything.
    ///
    /// # Errors
    ///
    /// The editor's refusal (a device deleted during the take, say); the
    /// set is unchanged.
    pub fn commit(&self, editor: &mut Editor, stop: Ticks) -> Result<bool, bl_model::EditError> {
        if self.touches.is_empty() {
            return Ok(false);
        }
        editor.transact(UNDO_LABEL, |editor| {
            for ((track, target), touches) in &self.touches {
                for touch in touches {
                    let envelope = editor
                        .document()
                        .track(*track)
                        .and_then(|track| track.automation_for(target))
                        .map(|automation| automation.envelope.clone())
                        .unwrap_or_default();
                    let (removed, set) = match self.mode {
                        AutomationRecordMode::Touch => {
                            envelope.touch(&touch.written, touch.end.unwrap_or(stop))
                        }
                        AutomationRecordMode::Latch | AutomationRecordMode::Write => envelope
                            .touch(&self.held(touch, *track, *target, stop), stop + Ticks(1)),
                    };
                    for at in removed {
                        editor.apply(Command::DeleteAutomationBreakpoint {
                            track: *track,
                            target: *target,
                            at,
                        })?;
                    }
                    for (at, value) in set {
                        editor.apply(Command::SetAutomationBreakpoint {
                            track: *track,
                            target: *target,
                            at,
                            value,
                        })?;
                    }
                }
                editor.apply(Command::ReEnableAutomationFor {
                    track: *track,
                    target: *target,
                })?;
            }
            Ok(true)
        })
    }

    /// What a Latch or Write touch writes: its moves, the value it holds
    /// from the last of them to the stop, and, for Write, the control's
    /// value from where the recording started until its first move
    /// (automation R19 and R20).
    fn held(
        &self,
        touch: &Touch,
        track: TrackId,
        target: AutomationTarget,
        stop: Ticks,
    ) -> Vec<(Ticks, f64)> {
        let mut written = touch.written.clone();
        if let Some(&(at, value)) = written.last()
            && stop > at
        {
            written.push((stop, value));
        }
        if self.mode == AutomationRecordMode::Write
            && let Some(before) = self.before.get(&(track, target))
        {
            let first = written.first().map(|(at, _)| *at);
            let start = first.map_or(self.start, |at| self.start.min(at));
            if first.is_none_or(|at| at > start) {
                written.insert(0, (start, *before));
            }
        }
        written
    }
}

/// The take's first frame: where Arrangement Record started playback, or the
/// punch-in point when the recording punches in later (automation R20).
#[must_use]
pub fn take_start(document: &Document, position: Ticks) -> Ticks {
    let settings = document.settings();
    if settings.punch_in && settings.arrangement_loop.start > position {
        settings.arrangement_loop.start
    } else {
        position
    }
}

/// The control a command moves and the value it moves it to, if it is a
/// control automation can record: a track's volume, pan, mute or send, or a
/// device's parameter.
#[must_use]
pub fn moved(document: &Document, command: &Command) -> Option<(TrackId, AutomationTarget, f64)> {
    let mixer = |param| AutomationTarget::Mixer { param };
    Some(match *command {
        Command::SetVolume { track, volume } => (track, mixer(MixerParam::Volume), volume),
        Command::SetPan { track, pan } => (track, mixer(MixerParam::Pan), pan),
        Command::SetSplitPanLeft { track, pan } => (track, mixer(MixerParam::SplitLeft), pan),
        Command::SetSplitPanRight { track, pan } => (track, mixer(MixerParam::SplitRight), pan),
        Command::SetMute { track, mute } => (track, mixer(MixerParam::Mute), f64::from(mute)),
        Command::SetSend { track, to, level } => (track, AutomationTarget::Send { to }, level),
        Command::SetDeviceParam {
            device,
            param,
            value,
        } => (
            document.device_track(device)?,
            AutomationTarget::Device { device, param },
            value,
        ),
        _ => return None,
    })
}

/// Every control move `command` makes that automation can record: one move
/// for a control's own command, and one per macro a variation launch moves
/// (macros spec, R27), each of which is a hand move on that macro.
#[must_use]
pub fn moved_all(document: &Document, command: &Command) -> Vec<(TrackId, AutomationTarget, f64)> {
    if let Command::LaunchVariation { rack, index } = command {
        let Some(device) = document.device(*rack) else {
            return Vec::new();
        };
        let Some(variation) = device.macros.variations.get(*index) else {
            return Vec::new();
        };
        let Some(track) = document.device_track(*rack) else {
            return Vec::new();
        };
        return (1..=bl_model::MACROS)
            .filter(|number| !device.macros.setting(*number).variation_excluded)
            .filter_map(|number| {
                variation.values.get(usize::from(number - 1)).map(|value| {
                    (
                        track,
                        AutomationTarget::Device {
                            device: *rack,
                            param: bl_model::macro_param(number),
                        },
                        *value,
                    )
                })
            })
            .collect();
    }
    moved(document, command).into_iter().collect()
}

/// The slot a Session recording writes a control's moves into (WP-3.12).
pub type SlotKey = (TrackId, SceneId, AutomationTarget);

/// One control's moves in a Session clip.
struct SlotTouches {
    /// The control's value when it was first moved, which a clip with no
    /// envelope for it starts from.
    before: f64,
    /// Whether the clip is one the recording makes, so that its moves are
    /// written with it.
    new_clip: bool,
    touches: Vec<Touch>,
}

/// The control moves of a Session recording, gathered by slot (WP-3.12):
/// positions are ticks into the clip's contents, and the moves are written
/// into the clips' Automation envelopes when the take ends, a touch
/// replacing the envelope from its first move to its release or, with the
/// control not released, to the end of the loop pass.
#[derive(Default)]
pub struct SessionTake {
    slots: BTreeMap<SlotKey, SlotTouches>,
}

impl SessionTake {
    /// Notes that `key`'s control, which stood at `before`, was moved to
    /// `value` at `at` ticks into the clip.
    pub fn write(&mut self, key: SlotKey, new_clip: bool, before: f64, at: Ticks, value: f64) {
        let slot = self.slots.entry(key).or_insert_with(|| SlotTouches {
            before,
            new_clip,
            touches: Vec::new(),
        });
        write_touch(&mut slot.touches, at, value);
    }

    /// Notes that every control being moved was released, each where
    /// `position` says its clip then is (`None`: the touch stays open).
    pub fn release(&mut self, position: impl Fn(&SlotKey) -> Option<Ticks>) {
        for (key, slot) in &mut self.slots {
            if let Some(at) = position(key) {
                for touch in &mut slot.touches {
                    release_touch(touch, at);
                }
            }
        }
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The slots and controls it holds moves for.
    pub fn keys(&self) -> impl Iterator<Item = SlotKey> + '_ {
        self.slots.keys().copied()
    }

    /// Writes the moves into the set as one undo step named `label` (joining
    /// the one open, if there is), and puts every envelope written under
    /// automation again. `end_of` gives the position where a touch not
    /// released ends, for a clip and whether the recording made it: the end
    /// of the take, or of the clip's loop pass. Returns whether it wrote
    /// anything.
    ///
    /// # Errors
    ///
    /// The editor's refusal (a device deleted during the take, say); the set
    /// is unchanged.
    pub fn commit(
        &self,
        editor: &mut Editor,
        label: &'static str,
        end_of: impl Fn(&bl_model::Clip, bool) -> Ticks,
    ) -> Result<bool, bl_model::EditError> {
        if self.slots.is_empty() {
            return Ok(false);
        }
        editor.transact(label, |editor| {
            for (&(track, scene, target), slot) in &self.slots {
                let Some(clip) = editor.document().slot(track, scene).cloned() else {
                    continue;
                };
                let stop = end_of(&clip, slot.new_clip);
                for touch in &slot.touches {
                    let envelope = editor
                        .document()
                        .clip(clip.id)
                        .and_then(|clip| clip.envelope_of(&target, EnvelopeKind::Automation))
                        .map(|e| e.envelope.clone())
                        .unwrap_or_default();
                    let (removed, mut set) =
                        envelope.touch(&touch.written, touch.end.unwrap_or(stop));
                    // A control not released holds its last value to the end
                    // of the pass: the old envelope doesn't resume there.
                    if touch.end.is_none()
                        && let Some(&(at, _)) = set.last()
                        && at == stop
                        && touch.written.last().is_some_and(|last| last.0 < stop)
                    {
                        set.pop();
                    }
                    // A clip with no envelope for the control starts it at
                    // the value it had and holds that until the first move,
                    // which steps to its new value.
                    if envelope.is_empty()
                        && let Some(&(first, _)) = touch.written.first()
                    {
                        if first > Ticks(1) {
                            set.insert(0, (first - Ticks(1), slot.before));
                        }
                        if first > Ticks::ZERO {
                            set.insert(0, (Ticks::ZERO, slot.before));
                        }
                    }
                    for at in removed {
                        editor.apply(Command::DeleteClipEnvelopeBreakpoint {
                            clip: clip.id,
                            target,
                            kind: EnvelopeKind::Automation,
                            at,
                        })?;
                    }
                    for (at, value) in set {
                        editor.apply(Command::SetClipEnvelopeBreakpoint {
                            clip: clip.id,
                            target,
                            kind: EnvelopeKind::Automation,
                            at,
                            value,
                        })?;
                    }
                }
                editor.apply(Command::ReEnableAutomationFor { track, target })?;
            }
            Ok(true)
        })
    }
}
