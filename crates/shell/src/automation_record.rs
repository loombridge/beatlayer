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

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Command, Document, Editor, MixerParam, TrackId};

/// The undo step a finished automation recording makes. Live's name for it
/// isn't observed yet.
pub const UNDO_LABEL: &str = "Record Automation";

/// One run of moves of a control.
struct Touch {
    /// What the control was moved to, at the positions it was moved at, in
    /// position order.
    written: Vec<(Ticks, f64)>,
    /// Where the control was released, once it has been.
    end: Option<Ticks>,
}

/// The moves of a recording under way.
#[derive(Default)]
pub struct Take {
    touches: BTreeMap<(TrackId, AutomationTarget), Vec<Touch>>,
}

impl Take {
    /// Notes that `target` of `track` was moved to `value` at `at`. A move
    /// after the control's release, or before the previous move (the loop
    /// brace wrapped the playhead), starts a new touch; one at the same
    /// position as the last replaces its value.
    pub fn write(&mut self, track: TrackId, target: AutomationTarget, at: Ticks, value: f64) {
        let touches = self.touches.entry((track, target)).or_default();
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

    /// Notes that every control being moved was released at `at`.
    pub fn release(&mut self, at: Ticks) {
        for touch in self.touches.values_mut().flatten() {
            if touch.end.is_none() {
                touch.end = touch.written.last().map(|last| last.0.max(at));
            }
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
                    let (removed, set) = envelope.touch(&touch.written, touch.end.unwrap_or(stop));
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
