//! Gestures: drags that are one undo step, and the permission of a call.

use super::{
    Gesture, GesturePhase, Invocation, Shell, ShellError, arrangement_clip_command,
    automation_label, clip_marker_command, edit_error, is_automation_edit, marker_label,
    sample_edit_command, to_value,
};
use crate::WindowKind;
use crate::arrangement::ArrangementView;
use crate::arrangement::automation;
use crate::arrangement::automation::EnvelopeClipboard;
use crate::arrangement::editing;
use crate::arrangement::editing::Clipboard;
use crate::arrangement::editing::EditFailure;
use crate::clip_drag::{ClipDrag, ClipDragEvent};
use crate::clip_view;
use crate::clip_view::ClipView;
use crate::devices::DeviceView;
use crate::midi_tools;
use crate::permission::CallPermission;
use bl_model::Editor;
use bl_model::registry as model;
use bl_shell_spec::audio_editing as audio_ids;
use bl_shell_spec::automation as automation_ids;
use bl_shell_spec::bounce as bounce_ids;
use bl_shell_spec::browser as browser_ids;
use bl_shell_spec::clip_envelope_editing as clip_envelope_ids;
use bl_shell_spec::comping as comping_ids;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::freeze as freeze_ids;
use bl_shell_spec::locator as locator_ids;
use bl_shell_spec::midi_files as midi_ids;
use bl_shell_spec::midi_tools as tool_ids;
use bl_shell_spec::multisampler as multisampler_ids;
use bl_shell_spec::notes as note_ids;
use bl_shell_spec::tracks as track_ids;
use serde_json::Value;

/// Whether `id` is a comping command a drag runs: Draw Mode comping and a
/// highlight's edge (the comping spec, R31 and R34).
fn comping_drag(id: &str) -> bool {
    matches!(
        id,
        comping_ids::COPY_TO_MAIN_LANE | comping_ids::MOVE_COMP_EDGE
    )
}

impl Shell {
    /// Whether a gesture that edits the set is under way.
    #[must_use]
    pub fn gesture_open(&self) -> bool {
        self.edit_gesture.is_some()
    }

    /// Whether the person's hand is on the set: a gesture that edits it
    /// (a drag of clips, [`Shell::gesture`]) or one that drags a value
    /// ([`Shell::begin_gesture`]) is under way. Its edits become one undo
    /// step when it ends, so an agent's edits wait until then (WP-1.15b).
    #[must_use]
    pub fn gesture_under_way(&self) -> bool {
        // The edit a selected breakpoint opens isn't one: it can last
        // minutes, and another command ends it (automation-editing R23).
        self.edit_gesture.is_some()
            || (self.gesture && self.key_edit.is_none())
            // Spec: second-window R23. Clips dragged between the views.
            || self.clip_drag.is_some()
    }

    /// Runs a command as [`Shell::run`] does, recording what it changes in
    /// the set as one undo step named `label`, such as an agent's edit
    /// labelled with the agent's name, and made by `maker`, so the history
    /// says who made it. A command that changes nothing in the set records
    /// nothing. Undo and redo can't be labelled, nor can a command that
    /// replaces the set (opening or making one).
    ///
    /// # Errors
    ///
    /// As [`Shell::run`]; the set is unchanged.
    pub fn run_labelled(
        &mut self,
        maker: bl_model::Maker,
        label: impl Into<std::borrow::Cow<'static, str>>,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let labelled = !matches!(id, model::UNDO | model::REDO | model::GO_TO_STEP)
            && (model::handles(id)
                || audio_ids::handles(id)
                || freeze_ids::handles(id)
                || bounce_ids::handles(id)
                || edit_ids::handles(id)
                || device_ids::handles(id)
                || multisampler_ids::handles(id)
                || self.is_device_command(id)
                || track_ids::handles(id)
                || note_ids::handles(id)
                || (tool_ids::handles(id) && self.audio_apply(id, &args).is_none())
                || midi_ids::handles(id)
                || matches!(
                    id,
                    browser_ids::ADD_TO_ARRANGEMENT | browser_ids::LOAD_SELECTED
                ));
        if !labelled || self.edit_gesture.is_some() {
            return self.run(id, args);
        }
        let name: std::borrow::Cow<'static, str> = label.into();
        self.editor.begin_by(maker, name.clone());
        let result = self.run(id, args);
        if result.is_ok() {
            if self.editor.commit().unwrap_or(false) {
                self.dirty = true;
                // Spec: crash-recovery R1. The step is journalled under the
                // name the undo history gives it.
                self.journal_edit(Some(&name));
            }
        } else {
            self.editor.cancel().ok();
        }
        self.touch();
        result
    }

    /// What calling `id` with `args` needs, decided from its arguments
    /// against the set as it is now: its registered class, and destructive
    /// as well when it would delete or shorten clips it doesn't act on, as
    /// moving, resizing, nudging, duplicating, or pasting clips over others,
    /// or dropping files on them, does (WP-1.15b). Nothing changes. An
    /// unknown command is external, the most a command can need.
    pub fn call_permission(&mut self, id: &str, args: &Value) -> CallPermission {
        let registered = self
            .registry
            .get(id)
            .and_then(|spec| spec.permission)
            .unwrap_or(bl_commands::Permission::External);
        let shown = self
            .clip_view
            .shown_any_clip(self.editor.document(), &self.arrangement);
        crate::permission::assess(
            id,
            args,
            registered,
            crate::permission::Scene {
                insert_marker: self.arrangement.insert_marker(),
                selected_track: self.arrangement.selected_tracks().first().copied(),
                shown_clip: shown,
                clip_view: &self.clip_view,
                editor: &mut self.editor,
                view: &self.arrangement,
                clipboard: &self.clipboard,
                session: &self.session,
            },
        )
    }

    /// Runs one step of a gesture that edits the set, such as dragging
    /// clips or notes (WP-1.8b; arrangement-editing R13, piano-roll R11). A
    /// gesture is one undo step however many frames it lasts: its first
    /// update opens a transaction, each update runs `invocation`, an editing
    /// command with arguments measured from where the gesture began, on the
    /// set as it was then, and the commit closes the transaction. The set
    /// follows the gesture as it goes, so the view and the engine show it. A
    /// frame whose command is refused (a clip pushed before 1.1.1, a note
    /// past key 127) keeps the last one that worked. Cancelling puts the set
    /// and the selection back.
    ///
    /// # Errors
    ///
    /// [`ShellError`] if the command isn't an editing command, or the
    /// frame's command was refused; the gesture stays open either way
    /// until it's committed or cancelled.
    pub fn gesture(
        &mut self,
        phase: GesturePhase,
        invocation: Option<&Invocation>,
    ) -> Result<Value, ShellError> {
        // A drag ends the edit a selected breakpoint opened, keeping it.
        self.end_key_edit();
        if phase == GesturePhase::Cancel {
            if let Some(gesture) = self.edit_gesture.take() {
                self.editor.cancel().ok();
                self.arrangement.restore_selection(&gesture.view);
                self.clip_view = gesture.clip_view;
                self.document_changed();
                self.touch();
            }
            return Ok(Value::Null);
        }
        let mut outcome = Ok(Value::Null);
        if let Some(invocation) = invocation {
            if !edit_ids::handles(&invocation.id)
                && !locator_ids::handles(&invocation.id)
                && !note_ids::handles(&invocation.id)
                && !tool_ids::handles(&invocation.id)
                && !is_automation_edit(&invocation.id)
                && !automation_ids::handles(&invocation.id)
                && !clip_envelope_ids::handles(&invocation.id)
                && !clip_marker_command(&invocation.id)
                && !arrangement_clip_command(&invocation.id)
                && !comping_drag(&invocation.id)
            {
                return Err(ShellError::Unavailable {
                    command: invocation.id.clone(),
                    reason: "only the Arrangement's and the Clip View's editing commands run as \
                             gestures"
                        .to_owned(),
                });
            }
            outcome = self.gesture_step(invocation);
        }
        if phase == GesturePhase::Commit
            && self.edit_gesture.take().is_some()
            && self.editor.commit().unwrap_or(false)
        {
            self.dirty = true;
            // Spec: crash-recovery R1. The gesture's transaction is
            // journalled as the one edit it was.
            self.journal_edit(None);
        }
        self.document_changed();
        self.touch();
        outcome
    }

    fn gesture_step(&mut self, invocation: &Invocation) -> Result<Value, ShellError> {
        let listed = self.chain_status();
        let label = if note_ids::handles(&invocation.id) {
            clip_view::label(&invocation.id)
        } else if automation_ids::handles(&invocation.id) {
            automation::label(&invocation.id)
        } else if clip_envelope_ids::handles(&invocation.id) {
            crate::clip_envelope_edit::label(&invocation.id)
        } else if locator_ids::handles(&invocation.id) {
            crate::arrangement::locator::label(&invocation.id)
        } else if is_automation_edit(&invocation.id) {
            automation_label(&invocation.id)
        } else if clip_marker_command(&invocation.id) || arrangement_clip_command(&invocation.id) {
            marker_label(&invocation.id)
        } else if comping_drag(&invocation.id) {
            if invocation.id == comping_ids::MOVE_COMP_EDGE {
                "Move Comp Edge"
            } else {
                "Copy Selection to Main Lane"
            }
        } else {
            editing::label(&invocation.id, &invocation.args)
        };
        // Spec: midi-tools R22. A tool's control drag is one undo step
        // named after the tool.
        let label: std::borrow::Cow<'static, str> = midi_tools::gesture_label(
            &invocation.id,
            &invocation.args,
            self.editor.document(),
            &self.clip_view,
            &self.arrangement,
        )
        .map_or(std::borrow::Cow::Borrowed(label), std::borrow::Cow::Owned);
        // Sample editor R8: a frame that inserts a warp marker begins the
        // gesture's base, on which the frames that follow move it.
        let makes_base = invocation.id == "clip.insert_warp_marker";
        let base = if makes_base {
            None
        } else {
            self.edit_gesture.as_ref().and_then(|g| g.base.clone())
        };
        match &self.edit_gesture {
            None => {
                self.edit_gesture = Some(Gesture {
                    view: self.arrangement.clone(),
                    clip_view: self.clip_view.clone(),
                    last: None,
                    base: None,
                });
            }
            Some(gesture) => {
                // Back to where the gesture began, then the whole gesture
                // again.
                self.editor.cancel().ok();
                let view = gesture.view.clone();
                self.arrangement.restore_selection(&view);
                self.clip_view = gesture.clip_view.clone();
            }
        }
        self.editor.begin(label.clone());
        let launch = self.launch_settings();
        let edge_fades = self.create_fades_on_clip_edges();
        let run = |editor: &mut Editor,
                   arrangement: &mut ArrangementView,
                   clip_view: &mut ClipView,
                   clipboard: &mut Option<Clipboard>,
                   envelope_clipboard: &mut Option<EnvelopeClipboard>,
                   devices: &DeviceView,
                   invocation: &Invocation| {
            if clip_marker_command(&invocation.id) || arrangement_clip_command(&invocation.id) {
                // The document's own clip commands, run per frame like the
                // Clip View's note commands.
                model::run(editor, &invocation.id, invocation.args.clone())
                    .map(|changes| to_value(&changes))
                    .map_err(|error| EditFailure::Refused(error.to_string()))
            } else if comping_drag(&invocation.id) {
                // Spec: comping R31, R34. Draw Mode comping and a highlight's
                // edge, run per frame as the Arrangement's drags are.
                crate::arrangement::comping::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    edge_fades,
                )
            } else if note_ids::handles(&invocation.id) {
                clip_view::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    clip_view,
                    arrangement,
                )
            } else if tool_ids::handles(&invocation.id) {
                midi_tools::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    clip_view,
                    arrangement,
                )
            } else if automation_ids::handles(&invocation.id) {
                // Spec: automation-editing R2. A drag that moves several
                // breakpoints of an envelope at once.
                automation::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    envelope_clipboard,
                    devices,
                )
            } else if clip_envelope_ids::handles(&invocation.id) {
                // Spec: clip-envelopes R12, R13. A drag in the Envelope
                // Editor moves breakpoints, or a handle of its time
                // selection stretches or skews them.
                crate::clip_envelope_edit::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    clip_view,
                    arrangement,
                    envelope_clipboard,
                    devices,
                )
            } else if is_automation_edit(&invocation.id) {
                // WP-2.12: a drag on an automation lane moves, curves, or
                // deletes breakpoints, as one undo step.
                model::run(editor, &invocation.id, invocation.args.clone())
                    .map(|changes| to_value(&changes))
                    .map_err(|error| {
                        crate::arrangement::editing::EditFailure::Refused(error.to_string())
                    })
            } else if locator_ids::handles(&invocation.id) {
                // The locators' own commands (WP-6.24).
                crate::arrangement::locator::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    &launch,
                    None,
                )
            } else {
                editing::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    clipboard,
                    edge_fades,
                )
            }
        };
        // The warp commands check the files' lengths and fill in the
        // transients, which only the shell can (warping R11, R25, R26).
        let mut steps = Vec::with_capacity(2);
        for step in base.iter().chain(std::iter::once(invocation)) {
            match self.prepare_gesture_step(step) {
                Ok(step) => steps.push(step),
                Err(error) => {
                    let failure = EditFailure::Refused(error.to_string());
                    return self.gesture_refused(&base, &invocation.id, label, failure);
                }
            }
        }
        let mut result = Ok(Value::Null);
        for step in &steps {
            result = run(
                &mut self.editor,
                &mut self.arrangement,
                &mut self.clip_view,
                &mut self.clipboard,
                &mut self.envelope_clipboard,
                &self.devices,
                step,
            );
            if result.is_err() {
                break;
            }
        }
        let Some(gesture) = self.edit_gesture.as_mut() else {
            return Ok(Value::Null);
        };
        match result {
            Ok(value) => {
                gesture.last = Some(invocation.clone());
                if makes_base {
                    gesture.base = Some(invocation.clone());
                }
                if sample_edit_command(&invocation.id) {
                    self.clip_view.sample_follow(true);
                }
                if tool_ids::handles(&invocation.id) {
                    self.announce_chain(listed);
                }
                Ok(value)
            }
            Err(failure) => self.gesture_refused(&base, &invocation.id, label, failure),
        }
    }

    /// A gesture frame that was refused: the set goes back to the last frame
    /// that worked.
    fn gesture_refused(
        &mut self,
        base: &Option<Invocation>,
        id: &str,
        label: std::borrow::Cow<'static, str>,
        failure: EditFailure,
    ) -> Result<Value, ShellError> {
        let Some(last) = self.edit_gesture.as_ref().and_then(|g| g.last.clone()) else {
            return Err(edit_error(id, failure));
        };
        // Back to where the gesture began, then the last frame that worked.
        self.editor.cancel().ok();
        self.editor.begin(label);
        for step in base.iter().chain(std::iter::once(&last)) {
            let Ok(step) = self.prepare_gesture_step(step) else {
                break;
            };
            let outcome = if clip_marker_command(&step.id) || arrangement_clip_command(&step.id) {
                model::run(&mut self.editor, &step.id, step.args.clone())
                    .map(|_| ())
                    .map_err(|error| EditFailure::Refused(error.to_string()))
            } else if comping_drag(&step.id) {
                crate::arrangement::comping::run(
                    &step.id,
                    step.args.clone(),
                    &mut self.editor,
                    &mut self.arrangement,
                    self.prefs.create_fades_on_clip_edges.0,
                )
                .map(|_| ())
            } else if note_ids::handles(&step.id) {
                clip_view::run(
                    &step.id,
                    step.args.clone(),
                    &mut self.editor,
                    &mut self.clip_view,
                    &self.arrangement,
                )
                .map(|_| ())
            } else if tool_ids::handles(&step.id) {
                midi_tools::run(
                    &step.id,
                    step.args.clone(),
                    &mut self.editor,
                    &mut self.clip_view,
                    &self.arrangement,
                )
                .map(|_| ())
            } else if clip_envelope_ids::handles(&step.id) {
                crate::clip_envelope_edit::run(
                    &step.id,
                    step.args.clone(),
                    &mut self.editor,
                    &mut self.clip_view,
                    &self.arrangement,
                    &mut self.envelope_clipboard,
                    &self.devices,
                )
                .map(|_| ())
            } else if is_automation_edit(&step.id) {
                model::run(&mut self.editor, &step.id, step.args.clone())
                    .map(|_| ())
                    .map_err(|error| EditFailure::Refused(error.to_string()))
            } else {
                editing::run(
                    &step.id,
                    step.args.clone(),
                    &mut self.editor,
                    &mut self.arrangement,
                    &mut self.clipboard,
                    self.prefs.create_fades_on_clip_edges.0,
                )
                .map(|_| ())
            };
            if outcome.is_err() {
                break;
            }
        }
        Err(edit_error(id, failure))
    }

    /// Opens a gesture, such as a drag on a value field: the document
    /// commands it runs until [`Shell::end_gesture`] are one undo step,
    /// named `label` (such as "Change tempo"). A gesture already open
    /// carries on.
    pub fn begin_gesture(&mut self, label: &str) {
        // A drag on a value ends the edit a selected breakpoint opened.
        self.end_key_edit();
        if !self.gesture {
            self.editor.begin(label.to_owned());
            self.gesture = true;
        }
    }

    /// Closes the open gesture, recording its changes as one undo step.
    pub fn end_gesture(&mut self) {
        if self.gesture {
            self.gesture = false;
            if self.editor.commit().unwrap_or(false) {
                // Spec: crash-recovery R1. The gesture's transaction is
                // journalled as the one edit it was.
                self.journal_edit(None);
            }
            self.touch();
        }
        // The control a drag moved is let go (automation R17).
        self.automation_take
            .release(self.arrangement.transport().position);
        self.release_session_moves();
    }
}

impl Shell {
    /// The drag of clips between the views, if one is under way.
    #[must_use]
    pub fn clip_drag(&self) -> Option<&ClipDrag> {
        self.clip_drag.as_ref()
    }

    /// Takes a step of the drag of clips between the views, from `window`: a
    /// drop runs its command (errors show in the Status Bar) and ends the
    /// drag.
    ///
    /// # Errors
    ///
    /// [`ShellError`] if the drop's command failed; the drag has ended.
    pub fn clip_drag_event(
        &mut self,
        window: WindowKind,
        event: ClipDragEvent,
    ) -> Result<(), ShellError> {
        match event {
            ClipDragEvent::Begin(source) => {
                self.clip_drag = Some(ClipDrag {
                    source,
                    window,
                    over: None,
                });
                self.touch();
            }
            ClipDragEvent::Over(spot) => {
                if let Some(drag) = &mut self.clip_drag {
                    match spot {
                        Some(spot) => drag.over = Some((window, spot)),
                        // Only the window the pointer was over forgets the place.
                        None if drag.over.is_some_and(|(over, _)| over == window) => {
                            drag.over = None;
                        }
                        None => {}
                    }
                }
            }
            ClipDragEvent::End { drop } => {
                let Some(drag) = self.clip_drag.take() else {
                    return Ok(());
                };
                self.touch();
                if drop && let Some((id, args)) = drag.drop_command() {
                    self.invoke(id, args)?;
                }
            }
        }
        Ok(())
    }
}
