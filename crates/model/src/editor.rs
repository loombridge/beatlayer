//! The editor: the current document, transactions, and the undo history.

use core::fmt;
use std::borrow::Cow;
use std::collections::{BTreeSet, VecDeque};

use bl_core::id::{Id, IdAllocator};

use crate::changes::ChangeSet;
use crate::command::Command;
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{SceneId, TrackId};
use crate::scale::Scale;
use crate::scene::DEFAULT_SCENE_COUNT;

/// Owns the current document, its undo history, and the project's ID
/// allocator. Every edit on the main thread goes through it.
///
/// # Transactions
///
/// A UI gesture opens a transaction with [`Editor::begin`], applies any
/// number of commands as it goes (a drag moves the clip on every pointer
/// event), and closes it with [`Editor::commit`], which records one undo
/// step, or [`Editor::cancel`], which puts the document back. The document
/// changes with each command, so the UI and the engine follow the gesture
/// live. A command applied with no transaction open is a transaction of its
/// own. Transactions nest; only the outermost one records a step.
///
/// A transaction that leaves the document as it was records nothing.
///
/// # Undo
///
/// The history keeps whole versions of the document. Versions share every
/// node an edit didn't touch, so each step costs memory in proportion to
/// what it changed. A new step discards the steps that could have been
/// redone. The history isn't part of the document and isn't saved.
///
/// # IDs
///
/// A command that creates an object carries the object's ID, which must come
/// from [`Editor::new_id`] and can be used once. The allocator never goes
/// backwards, so an ID is never reused, even after its object is deleted
/// and the deletion undone.
#[derive(Debug)]
pub struct Editor {
    document: Document,
    ids: IdAllocator,
    /// IDs handed out by `new_id` and not yet used by a command.
    unused_ids: BTreeSet<Id>,
    /// Open transactions, outermost first.
    open: Vec<Open>,
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    undo_limit: Option<usize>,
}

#[derive(Debug)]
struct Open {
    label: Cow<'static, str>,
    maker: Maker,
    start: Document,
    /// Whether the step is recorded even if the document ends as it began.
    keep: bool,
}

/// A step in the history: its label, who made it, and one version of the
/// document. On the undo stack that's the version before the step; on the
/// redo stack, the version after it. The current document is the other end
/// of the newest step on either stack, so no version is stored twice. The
/// version is boxed to keep the stacks' unused capacity small.
#[derive(Debug)]
struct Step {
    label: Cow<'static, str>,
    maker: Maker,
    document: Box<Document>,
    /// The set's current scale the step set, when the step changed it:
    /// redoing brings it back. A step that only followed a selection leaves
    /// the scale as it is (the scales spec, R12).
    scale: Option<Scale>,
}

/// Who made a step in the history: the person, an agent by the name the
/// agent API knows it by, or the built-in assistant.
///
/// Spec: undo-history R9.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Maker {
    /// The person, whose own gesture the step came from.
    Person,
    /// An agent, by the name the agent API knows it by.
    Agent(String),
    /// The built-in assistant.
    Assistant,
}

impl Maker {
    /// The name of whoever made the step: the agent's name, "Assistant", or
    /// "the person".
    #[must_use]
    pub fn name(&self) -> Cow<'static, str> {
        match self {
            Self::Person => Cow::Borrowed("the person"),
            Self::Agent(name) => Cow::Owned(name.clone()),
            Self::Assistant => Cow::Borrowed("Assistant"),
        }
    }

    /// Whether an agent or the assistant made the step, rather than the
    /// person.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        !matches!(self, Self::Person)
    }
}

/// A row of the undo history: one step, where it stands in the list, and
/// who made it. The rows come from the top: the steps that can be redone
/// first, then the ones that can be undone, the newest of them the current
/// row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryStep {
    /// The step's label, as the Edit menu names it.
    pub label: Cow<'static, str>,
    /// Who made it.
    pub maker: Maker,
    /// Whether it has been undone: the set is before it, and it can be
    /// redone.
    pub undone: bool,
    /// Its place in the list, counted up from the set's start: 0 is the
    /// set's start, before any step.
    pub index: usize,
}

/// Why the editor refused a request.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum EditError {
    /// The document rejected the command.
    Command(ModelError),
    /// A command creates an object with an ID that [`Editor::new_id`] didn't
    /// hand out, or that a command has already used.
    IdNotIssued(Id),
    /// Commit or cancel with no transaction open.
    NoTransaction,
    /// Undo, redo, or clearing the history while a transaction is open.
    TransactionOpen,
    /// Undo with nothing to undo.
    NothingToUndo,
    /// Redo with nothing to redo.
    NothingToRedo,
    /// A jump to a step the history doesn't hold.
    NoSuchStep {
        /// The row the jump named.
        index: usize,
        /// How many rows the history holds.
        steps: usize,
    },
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => error.fmt(f),
            Self::IdNotIssued(id) => write!(f, "ID {id} wasn't issued for a new object"),
            Self::NoTransaction => f.write_str("no transaction is open"),
            Self::TransactionOpen => f.write_str("a transaction is open"),
            Self::NothingToUndo => f.write_str("nothing to undo"),
            Self::NothingToRedo => f.write_str("nothing to redo"),
            Self::NoSuchStep { index, steps } => {
                write!(f, "there's no step {index} in a history of {steps}")
            }
        }
    }
}

impl std::error::Error for EditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Command(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ModelError> for EditError {
    fn from(error: ModelError) -> Self {
        Self::Command(error)
    }
}

impl Editor {
    /// An editor for a new, empty song (the Main track and the default
    /// scenes) with an empty history.
    #[must_use]
    pub fn new() -> Self {
        let mut ids = IdAllocator::new();
        let main = TrackId(ids.allocate());
        let scenes = (0..DEFAULT_SCENE_COUNT).map(|_| SceneId(ids.allocate()));
        Self::from_document(Document::new(main, scenes), ids)
    }

    /// An editor for a loaded document, with an empty history.
    ///
    /// `ids` is the allocator saved with the document. If it's behind the
    /// document's largest ID, it's moved past it.
    #[must_use]
    pub fn from_document(document: Document, mut ids: IdAllocator) -> Self {
        ids.skip_past(document.max_id());
        Self {
            document,
            ids,
            unused_ids: BTreeSet::new(),
            open: Vec::new(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            undo_limit: None,
        }
    }

    /// The current document. Clone it for a snapshot to send to another
    /// thread.
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The ID allocator, to save with the document.
    #[must_use]
    pub fn ids(&self) -> IdAllocator {
        self.ids
    }

    /// A new ID for an object a command will create, such as a `TrackId`.
    pub fn new_id<T: From<Id>>(&mut self) -> T {
        let id = self.ids.allocate();
        self.unused_ids.insert(id);
        T::from(id)
    }

    /// Applies a command, inside the open transaction or as a transaction of
    /// its own, and returns what it changed.
    ///
    /// # Errors
    ///
    /// [`EditError::Command`] if the document rejects the command, and
    /// [`EditError::IdNotIssued`] if it creates an object with an ID that
    /// [`Editor::new_id`] didn't hand out or that's been used. Either way the
    /// document is as it was and any open transaction stays open.
    pub fn apply(&mut self, mut command: Command) -> Result<ChangeSet, EditError> {
        let document = self.document.clone();
        command.issue_missing_ids(&document, &mut || self.new_id());
        let created = command.created_ids();
        for id in &created {
            if !self.unused_ids.contains(id) {
                return Err(EditError::IdNotIssued(*id));
            }
        }
        let before = self.document.clone();
        self.document.apply(&command)?;
        for id in created {
            self.unused_ids.remove(&id);
        }
        let changes = ChangeSet::between(&before, &self.document);
        if self.open.is_empty() && !changes.is_empty() && command.records_undo() {
            self.record(command.label_in(&before).into(), Maker::Person, before);
        }
        Ok(changes)
    }

    /// Applies a command into the newest undo step, so that step ends with
    /// its result instead of the command recording a step of its own: Tap
    /// Tempo's taps make one step between them (the control bar transport
    /// spec, R21). The caller makes sure the newest step is the one to
    /// amend. If the document ends as that step began, the step goes.
    ///
    /// # Errors
    ///
    /// [`EditError::TransactionOpen`] during a transaction, and
    /// [`EditError::Command`] if the document rejects the command, which
    /// leaves it as it was.
    pub fn apply_into_newest(&mut self, command: Command) -> Result<ChangeSet, EditError> {
        if self.in_transaction() {
            return Err(EditError::TransactionOpen);
        }
        let before = self.document.clone();
        self.document.apply(&command)?;
        if self
            .undo
            .back()
            .is_some_and(|step| *step.document == self.document)
        {
            self.undo.pop_back();
        }
        Ok(ChangeSet::between(&before, &self.document))
    }

    /// Records a plug-in device's own state in the document (WP-2.6)
    /// without an undo step: the running plug-in already has it, so it's a
    /// note for the set file, made when the set is saved or the device
    /// copied. Undoing an earlier step may bring back an older note, which
    /// the running plug-in ignores; the next save takes its state again.
    ///
    /// # Errors
    ///
    /// [`EditError::Command`] if there's no such device.
    pub fn amend_plugin_state(
        &mut self,
        device: crate::DeviceId,
        state: Option<crate::PluginState>,
    ) -> Result<(), EditError> {
        self.document.set_plugin_state(device, state)?;
        Ok(())
    }

    /// Records the ARA plug-ins' document archives in the set (IMP-20)
    /// without an undo step, as [`Editor::amend_plugin_state`] does a
    /// plug-in's own state: the running plug-ins already hold what they
    /// archive, so it is a note for the set file. Returns whether the set
    /// changed.
    pub fn amend_ara_archives(&mut self, archives: Vec<crate::ara::AraArchive>) -> bool {
        if self.document.settings.ara_archives == archives {
            return false;
        }
        self.document.settings.ara_archives = archives;
        true
    }

    /// Records the name of the preset a device holds (the plug-in list spec,
    /// R5) without an undo step: saving a preset names it, and the name is
    /// a note for the set file, not a change to the sound.
    ///
    /// # Errors
    ///
    /// [`EditError::Command`] if there's no such device.
    pub fn amend_device_preset_name(
        &mut self,
        device: crate::DeviceId,
        name: Option<String>,
    ) -> Result<(), EditError> {
        self.document.set_device_preset_name(device, name)?;
        Ok(())
    }

    /// Points the set's current scale at `scale`, as selecting a clip or a
    /// clip that starts playing does (the scales spec, R10 and R11): the
    /// current scale follows, and that is no undo step, though the set is
    /// marked as changed.
    ///
    /// It isn't a command: no person asked for it, so it is the app
    /// following the set's own state, as
    /// [`Editor::amend_plugin_state`] does for a plug-in's.
    pub fn follow_scale(&mut self, scale: Scale) -> ChangeSet {
        let before = self.document.clone();
        self.document.settings.scale = scale;
        ChangeSet::between(&before, &self.document)
    }

    /// Sets the set's tempo to the constant tempo `bpm`, as a peer in a Link
    /// session setting it does (the Link spec, R3): that is no undo step,
    /// though the set is marked as changed.
    ///
    /// It isn't a command: no person asked for it, so it is the app
    /// following the session, as [`Editor::follow_scale`] follows the
    /// scale.
    pub fn follow_tempo(&mut self, bpm: bl_core::time::Bpm) -> ChangeSet {
        let before = self.document.clone();
        self.document.settings.tempo = bl_core::time::TempoMap::constant(bpm);
        ChangeSet::between(&before, &self.document)
    }

    /// Replaces the whole document with another version of it, inside the
    /// open transaction or as an undo step of its own labelled `label`, and
    /// returns what that changed. The agent API restores a checkpoint this
    /// way (WP-1.15), so undo brings back what the restore replaced.
    ///
    /// The allocator moves past the document's largest ID, so no ID in it is
    /// handed out again. A document that's the same as the current one
    /// records nothing.
    pub fn replace(
        &mut self,
        label: impl Into<Cow<'static, str>>,
        document: Document,
    ) -> ChangeSet {
        self.replace_by(Maker::Person, label, document)
    }

    /// [`Editor::replace`], with the step recorded as `maker`'s: the agent
    /// API restores a checkpoint as the agent's own step.
    pub fn replace_by(
        &mut self,
        maker: Maker,
        label: impl Into<Cow<'static, str>>,
        document: Document,
    ) -> ChangeSet {
        self.ids.skip_past(document.max_id());
        let before = std::mem::replace(&mut self.document, document);
        let changes = ChangeSet::between(&before, &self.document);
        if self.open.is_empty() && !changes.is_empty() {
            self.record(label.into(), maker, before);
        }
        changes
    }

    /// Opens a transaction. Its label names the undo step, such as "Move
    /// clips". Inside another transaction, the outer one's label is used.
    pub fn begin(&mut self, label: impl Into<Cow<'static, str>>) {
        self.begin_by(Maker::Person, label);
    }

    /// [`Editor::begin`], with the step recorded as `maker`'s: an agent's
    /// batch and the assistant's turn open their transaction this way.
    pub fn begin_by(&mut self, maker: Maker, label: impl Into<Cow<'static, str>>) {
        self.open.push(Open {
            label: label.into(),
            maker,
            start: self.document.clone(),
            keep: false,
        });
    }

    /// Closes the innermost transaction, keeping its changes. Closing the
    /// outermost one records an undo step if the document changed.
    ///
    /// Returns whether it recorded a step.
    ///
    /// # Errors
    ///
    /// [`EditError::NoTransaction`] if none is open.
    pub fn commit(&mut self) -> Result<bool, EditError> {
        let open = self.open.pop().ok_or(EditError::NoTransaction)?;
        if let Some(outer) = self.open.last_mut() {
            outer.keep |= open.keep;
            return Ok(false);
        }
        if open.start == self.document && !open.keep {
            return Ok(false);
        }
        self.record(open.label, open.maker, open.start);
        Ok(true)
    }

    /// Makes the innermost open transaction record its undo step when it
    /// closes even if the document ends as it began, as a MIDI Tool's
    /// application does when it leaves the notes as they were (the MIDI
    /// Tools spec, R22). Does nothing outside a transaction.
    pub fn keep_step(&mut self) {
        if let Some(open) = self.open.last_mut() {
            open.keep = true;
        }
    }

    /// Closes the innermost transaction, putting the document back as it was
    /// when the transaction opened. Returns what that changed.
    ///
    /// # Errors
    ///
    /// [`EditError::NoTransaction`] if none is open.
    pub fn cancel(&mut self) -> Result<ChangeSet, EditError> {
        let open = self.open.pop().ok_or(EditError::NoTransaction)?;
        let changes = ChangeSet::between(&self.document, &open.start);
        self.document = open.start;
        Ok(changes)
    }

    /// Runs `edit` in a transaction, committing it if `edit` succeeds and
    /// cancelling it if `edit` fails. Transactions `edit` leaves open are
    /// closed with it.
    ///
    /// # Errors
    ///
    /// Whatever `edit` returns.
    pub fn transact<T, E>(
        &mut self,
        label: impl Into<Cow<'static, str>>,
        edit: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        self.transact_by(Maker::Person, label, edit)
    }

    /// [`Editor::transact`], with the step recorded as `maker`'s.
    ///
    /// # Errors
    ///
    /// Whatever `edit` returns.
    pub fn transact_by<T, E>(
        &mut self,
        maker: Maker,
        label: impl Into<Cow<'static, str>>,
        edit: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        self.begin_by(maker, label);
        let depth = self.open.len();
        let result = edit(self);
        if self.open.len() >= depth {
            // Fold any inner transactions into this one, then close it.
            let open = self.open.drain(depth - 1..).next();
            if let Some(open) = open {
                self.open.push(open);
                if result.is_ok() {
                    self.commit().ok();
                } else {
                    self.cancel().ok();
                }
            }
        }
        result
    }

    /// Whether a transaction is open.
    #[must_use]
    pub fn in_transaction(&self) -> bool {
        !self.open.is_empty()
    }

    /// Undoes the newest step and returns what that changed.
    ///
    /// # Errors
    ///
    /// [`EditError::TransactionOpen`] during a transaction, and
    /// [`EditError::NothingToUndo`] if the history is empty.
    pub fn undo(&mut self) -> Result<ChangeSet, EditError> {
        if self.in_transaction() {
            return Err(EditError::TransactionOpen);
        }
        let before = self.document.clone();
        self.undo_step()?;
        Ok(ChangeSet::between(&before, &self.document))
    }

    /// Undoes the newest step: the document goes back to the version before
    /// it, and the step moves to the redo stack.
    fn undo_step(&mut self) -> Result<(), EditError> {
        let Step {
            label,
            maker,
            document,
            scale,
        } = self.undo.pop_back().ok_or(EditError::NothingToUndo)?;
        let mut document = *document;
        // Cue mode is no undo step, so undoing keeps it as it is.
        document.settings.cue_mode = self.document.settings.cue_mode;
        // The current scale follows a selection or a clip that starts, and
        // that is no undo step either, so a step that didn't change the
        // scale leaves it as it is.
        if scale.is_none() {
            document.settings.scale = self.document.settings.scale;
        }
        let after = std::mem::replace(&mut self.document, document);
        self.redo.push(Step {
            label,
            maker,
            document: Box::new(after),
            scale,
        });
        Ok(())
    }

    /// Redoes the newest undone step and returns what that changed.
    ///
    /// # Errors
    ///
    /// [`EditError::TransactionOpen`] during a transaction, and
    /// [`EditError::NothingToRedo`] if nothing has been undone since the last
    /// new step.
    pub fn redo(&mut self) -> Result<ChangeSet, EditError> {
        if self.in_transaction() {
            return Err(EditError::TransactionOpen);
        }
        let before = self.document.clone();
        self.redo_step()?;
        Ok(ChangeSet::between(&before, &self.document))
    }

    /// Redoes the newest undone step: the document goes on to the version
    /// after it, and the step moves back to the undo stack.
    fn redo_step(&mut self) -> Result<(), EditError> {
        let Step {
            label,
            maker,
            document,
            scale,
        } = self.redo.pop().ok_or(EditError::NothingToRedo)?;
        let mut document = *document;
        document.settings.cue_mode = self.document.settings.cue_mode;
        // A step that changed the scale brings it back; one that didn't
        // leaves it as it is.
        if let Some(scale) = scale {
            document.settings.scale = scale;
        }
        let before = std::mem::replace(&mut self.document, document);
        self.undo.push_back(Step {
            label,
            maker,
            document: Box::new(before),
            scale,
        });
        Ok(())
    }

    /// Whether there's a step to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there's a step to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The labels of the steps that can be undone, newest first.
    pub fn undo_history(&self) -> impl ExactSizeIterator<Item = &str> {
        self.undo.iter().rev().map(|step| step.label.as_ref())
    }

    /// The labels of the steps that can be redone, next first.
    pub fn redo_history(&self) -> impl ExactSizeIterator<Item = &str> {
        self.redo.iter().rev().map(|step| step.label.as_ref())
    }

    /// The history's rows, from the top: the steps that can be redone
    /// first, newest first, then the steps that can be undone, the newest
    /// of them the current row.
    ///
    /// The set's start isn't a row here: it has no step, and the panel adds
    /// it at the bottom.
    #[must_use]
    pub fn history(&self) -> Vec<HistoryStep> {
        let total = self.undo.len() + self.redo.len();
        let mut rows = Vec::with_capacity(total);
        for (offset, step) in self.redo.iter().enumerate() {
            rows.push(HistoryStep {
                label: step.label.clone(),
                maker: step.maker.clone(),
                undone: true,
                index: total - offset,
            });
        }
        for (offset, step) in self.undo.iter().rev().enumerate() {
            rows.push(HistoryStep {
                label: step.label.clone(),
                maker: step.maker.clone(),
                undone: false,
                index: self.undo.len() - offset,
            });
        }
        rows
    }

    /// The current row's place in the history, counted up from the set's
    /// start: 0 before any step, and the number of steps that can be
    /// undone when the newest of them is the current row.
    #[must_use]
    pub fn current_index(&self) -> usize {
        self.undo.len()
    }

    /// Returns the set to the state just after the step at `index`, counted
    /// up from the set's start: 0 is the set's start, before any step, and
    /// the newest step is the number of steps there are.
    ///
    /// The jump undoes or redoes as many steps as it takes, and is no step
    /// of its own: the steps above the row it went to stay listed, greyed.
    ///
    /// # Errors
    ///
    /// [`EditError::TransactionOpen`] during a transaction, and
    /// [`EditError::NoSuchStep`] if the history doesn't hold that row.
    pub fn go_to_step(&mut self, index: usize) -> Result<ChangeSet, EditError> {
        if self.in_transaction() {
            return Err(EditError::TransactionOpen);
        }
        let steps = self.undo.len() + self.redo.len();
        if index > steps {
            return Err(EditError::NoSuchStep { index, steps });
        }
        if self.undo.len() == index {
            return Ok(ChangeSet::default());
        }
        let before = self.document.clone();
        // The steps move between the stacks without working out what each
        // one changed: the jump's own change set is the difference between
        // the two versions.
        while self.undo.len() > index {
            self.undo_step()?;
        }
        while self.undo.len() < index {
            self.redo_step()?;
        }
        Ok(ChangeSet::between(&before, &self.document))
    }

    /// Keeps at most `limit` undo steps, dropping the oldest. `None`, the
    /// default, keeps them all.
    pub fn set_undo_limit(&mut self, limit: Option<usize>) {
        self.undo_limit = limit;
        self.trim();
    }

    /// Forgets every undo and redo step, as after loading a project.
    ///
    /// # Errors
    ///
    /// [`EditError::TransactionOpen`] during a transaction.
    pub fn clear_history(&mut self) -> Result<(), EditError> {
        if self.in_transaction() {
            return Err(EditError::TransactionOpen);
        }
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    fn record(&mut self, label: Cow<'static, str>, maker: Maker, before: Document) {
        self.redo.clear();
        let scale = (before.settings.scale != self.document.settings.scale)
            .then_some(self.document.settings.scale);
        self.undo.push_back(Step {
            label,
            maker,
            document: Box::new(before),
            scale,
        });
        self.trim();
    }

    fn trim(&mut self) {
        if let Some(limit) = self.undo_limit {
            while self.undo.len() > limit {
                self.undo.pop_front();
            }
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::Ticks;

    use super::*;
    use crate::clip::{Clip, ClipData, MidiData};
    use crate::ids::ClipId;
    use crate::track::TrackKind;

    fn midi_track(editor: &mut Editor) -> TrackId {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Midi,
                name: "Keys".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    }

    fn clip_on(editor: &mut Editor, track: TrackId) -> ClipId {
        let id = editor.new_id();
        let clip = Clip::new(
            id,
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Midi(MidiData::default()),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        id
    }

    fn start_of(editor: &Editor, clip: ClipId) -> Ticks {
        editor.document().clip(clip).unwrap().start
    }

    #[test]
    fn a_command_applied_into_the_newest_step_ends_that_step_with_its_result() {
        use bl_core::time::{Bpm, TempoMap};
        let tempo = |bpm| TempoMap::constant(Bpm::new(bpm).unwrap());
        let mut editor = Editor::new();
        let original = editor.document().settings().tempo.clone();
        editor.begin("Tap Tempo");
        editor
            .apply(Command::SetTempo {
                tempo: tempo(100.0),
            })
            .unwrap();
        editor.commit().unwrap();
        editor
            .apply_into_newest(Command::SetTempo {
                tempo: tempo(110.0),
            })
            .unwrap();
        assert_eq!(editor.document().settings().tempo, tempo(110.0));
        assert_eq!(editor.undo_history().collect::<Vec<_>>(), ["Tap Tempo"]);
        // Back to where the step began: the step goes.
        editor
            .apply_into_newest(Command::SetTempo {
                tempo: original.clone(),
            })
            .unwrap();
        assert!(!editor.can_undo());
        // During a transaction it refuses.
        editor.begin("Open");
        assert_eq!(
            editor.apply_into_newest(Command::SetTempo { tempo: tempo(90.0) }),
            Err(EditError::TransactionOpen)
        );
    }

    #[test]
    fn a_kept_step_is_recorded_though_the_document_ends_as_it_began() {
        let mut editor = Editor::new();
        editor.begin("Plain");
        assert!(
            !editor.commit().unwrap(),
            "an unchanged transaction records nothing"
        );
        editor.begin("Kept");
        editor.begin("Inside");
        editor.keep_step();
        assert!(
            !editor.commit().unwrap(),
            "the inner one leaves it to the outer"
        );
        assert!(editor.commit().unwrap(), "the outer one records it");
        assert_eq!(editor.undo_history().collect::<Vec<_>>(), ["Kept"]);
        editor.undo().unwrap();
        assert!(!editor.can_undo());
    }

    #[test]
    fn history_starts_empty() {
        let editor = Editor::new();
        assert!(!editor.can_undo());
        assert!(!editor.can_redo());
        assert_eq!(editor.document().track_count(), 1);

        let mut loaded = Editor::from_document(editor.document().clone(), IdAllocator::new());
        assert!(!loaded.can_undo());
        assert_eq!(loaded.undo(), Err(EditError::NothingToUndo));
        // The stale allocator was moved past the Main track's ID.
        assert!(loaded.ids().peek() > loaded.document().main_id().0);
    }

    #[test]
    fn single_commands_are_single_steps() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        let empty = Editor::new();
        assert_eq!(editor.undo_history().collect::<Vec<_>>(), ["Create track"]);
        editor.undo().unwrap();
        assert_eq!(editor.document(), empty.document());
        assert_eq!(editor.redo_history().collect::<Vec<_>>(), ["Create track"]);
        editor.redo().unwrap();
        assert!(editor.document().track(track).is_some());
    }

    // Spec: mixer R24, R27
    #[test]
    fn cue_mode_needs_separate_outputs_and_is_no_undo_step() {
        use crate::{Channel, MAX_CUE_VOLUME};
        let mut editor = Editor::new();
        // Cue Out and Main Out both start at 1/2: Cue mode is refused.
        assert!(!editor.document().cue_available());
        assert!(matches!(
            editor.apply(Command::SetCueMode { enabled: true }),
            Err(EditError::Command(ModelError::CueOverlapsMain))
        ));
        editor
            .apply(Command::SetCueOut {
                channel: Channel::Pair(3),
            })
            .unwrap();
        assert!(editor.document().cue_available());
        let steps = editor.undo_history().len();

        editor.apply(Command::SetCueMode { enabled: true }).unwrap();
        assert!(editor.document().cue_active());
        assert_eq!(editor.undo_history().len(), steps, "no undo step");

        // The volume is a step, named as Live names it, and is bounded.
        editor.apply(Command::SetCueVolume { volume: 0.5 }).unwrap();
        assert_eq!(
            editor.undo_history().next(),
            Some("Change \"Preview Volume\"")
        );
        for volume in [-0.1, MAX_CUE_VOLUME + 0.01, f64::NAN] {
            assert!(editor.apply(Command::SetCueVolume { volume }).is_err());
        }

        // Undoing the volume leaves the mode as it is, and so does redo; the
        // routing step before it brings the overlap back, which ends Cue
        // mode in effect, not in the set.
        editor.undo().unwrap();
        assert_eq!(editor.document().settings().cue_volume, 1.0);
        assert!(editor.document().settings().cue_mode);
        editor.redo().unwrap();
        assert!(editor.document().settings().cue_mode);
        editor.undo().unwrap();
        editor.undo().unwrap();
        assert!(!editor.document().cue_available());
        assert!(editor.document().settings().cue_mode);
        assert!(!editor.document().cue_active());

        // A set saves them only when they differ from a new set's.
        editor
            .apply(Command::SetCueMode { enabled: false })
            .unwrap();
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert!(saved["settings"].get("cue_mode").is_none());
        assert!(saved["settings"].get("cue_volume").is_none());
    }

    #[test]
    fn a_gesture_is_one_step() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        let clip = clip_on(&mut editor, track);
        let steps = editor.undo_history().len();

        editor.begin("Drag clip");
        for bar in 1..=8 {
            editor
                .apply(Command::MoveClip {
                    clip,
                    track,
                    start: Ticks::WHOLE * bar,
                })
                .unwrap();
            // The document follows the gesture as it goes.
            assert_eq!(start_of(&editor, clip), Ticks::WHOLE * bar);
        }
        assert!(editor.commit().unwrap());
        assert_eq!(editor.undo_history().len(), steps + 1);
        assert_eq!(editor.undo_history().next(), Some("Drag clip"));

        editor.undo().unwrap();
        assert_eq!(start_of(&editor, clip), Ticks::ZERO);
        editor.redo().unwrap();
        assert_eq!(start_of(&editor, clip), Ticks::WHOLE * 8);
    }

    #[test]
    fn cancel_puts_the_document_back() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        let clip = clip_on(&mut editor, track);
        let before = editor.document().clone();
        let steps = editor.undo_history().len();

        editor.begin("Drag clip");
        editor
            .apply(Command::MoveClip {
                clip,
                track,
                start: Ticks::WHOLE,
            })
            .unwrap();
        let changes = editor.cancel().unwrap();
        assert_eq!(editor.document(), &before);
        assert!(changes.clips.contains_key(&clip));
        assert_eq!(editor.undo_history().len(), steps);
    }

    #[test]
    fn transactions_nest() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        let clip = clip_on(&mut editor, track);
        let steps = editor.undo_history().len();

        editor.begin("Outer");
        editor.begin("Inner kept");
        editor
            .apply(Command::SetClipMute { clip, mute: true })
            .unwrap();
        assert!(!editor.commit().unwrap());
        editor.begin("Inner dropped");
        editor
            .apply(Command::MoveClip {
                clip,
                track,
                start: Ticks::WHOLE,
            })
            .unwrap();
        editor.cancel().unwrap();
        assert!(editor.commit().unwrap());

        assert_eq!(editor.undo_history().len(), steps + 1);
        assert_eq!(editor.undo_history().next(), Some("Outer"));
        let clip_now = editor.document().clip(clip).unwrap();
        assert!(clip_now.mute);
        assert_eq!(clip_now.start, Ticks::ZERO);
    }

    #[test]
    fn empty_transactions_record_nothing() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        let steps = editor.undo_history().len();

        editor.begin("Click");
        assert!(!editor.commit().unwrap());
        // Setting a value to what it already is changes nothing either.
        let changes = editor
            .apply(Command::SetMute { track, mute: false })
            .unwrap();
        assert!(changes.is_empty());
        editor.begin("Wiggle");
        editor
            .apply(Command::SetVolume { track, volume: 0.5 })
            .unwrap();
        editor
            .apply(Command::SetVolume { track, volume: 1.0 })
            .unwrap();
        assert!(!editor.commit().unwrap());
        assert_eq!(editor.undo_history().len(), steps);
    }

    #[test]
    fn rejected_commands_change_nothing() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        let before = editor.document().clone();
        editor.begin("Gesture");
        let error = editor
            .apply(Command::SetPan { track, pan: 2.0 })
            .unwrap_err();
        assert!(matches!(
            error,
            EditError::Command(ModelError::InvalidValue { what: "pan", .. })
        ));
        assert_eq!(editor.document(), &before);
        assert!(editor.in_transaction());
        assert_eq!(editor.undo(), Err(EditError::TransactionOpen));
        editor.cancel().unwrap();
        assert_eq!(editor.cancel(), Err(EditError::NoTransaction));
        assert_eq!(editor.commit(), Err(EditError::NoTransaction));
    }

    #[test]
    fn a_new_step_discards_redo() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        editor
            .apply(Command::SetVolume { track, volume: 0.5 })
            .unwrap();
        editor.undo().unwrap();
        assert!(editor.can_redo());
        editor.apply(Command::SetPan { track, pan: 0.5 }).unwrap();
        assert!(!editor.can_redo());
        assert_eq!(editor.redo(), Err(EditError::NothingToRedo));
    }

    #[test]
    fn ids_are_never_reused() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        editor.undo().unwrap();
        // Recreating with the undone track's ID is refused...
        let error = editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap_err();
        assert_eq!(error, EditError::IdNotIssued(track.0));
        // ...and so is an ID the editor never handed out.
        let made_up = TrackId(Id::new(1_000).unwrap());
        assert!(matches!(
            editor.apply(Command::CreateTrack {
                id: made_up,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            }),
            Err(EditError::IdNotIssued(_))
        ));
        // A fresh ID comes after every ID handed out before.
        let fresh: TrackId = editor.new_id();
        assert!(fresh > track);
        // Redo still restores the original track with its original ID.
        editor.redo().unwrap();
        assert!(editor.document().track(track).is_some());
    }

    #[test]
    fn transact_commits_or_cancels() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        let steps = editor.undo_history().len();

        editor
            .transact("Set up", |editor| {
                editor.apply(Command::SetVolume {
                    track,
                    volume: 0.25,
                })?;
                editor.begin("Left open");
                editor.apply(Command::SetPan { track, pan: -0.5 })?;
                Ok::<_, EditError>(())
            })
            .unwrap();
        assert!(!editor.in_transaction());
        assert_eq!(editor.undo_history().len(), steps + 1);
        assert_eq!(editor.document().track(track).unwrap().mixer().pan, -0.5);

        let before = editor.document().clone();
        let result = editor.transact("Fails", |editor| {
            editor.apply(Command::SetVolume {
                track,
                volume: 0.75,
            })?;
            editor.apply(Command::SetArm {
                track: editor.document().main_id(),
                arm: true,
            })?;
            Ok::<_, EditError>(())
        });
        assert!(result.is_err());
        assert!(!editor.in_transaction());
        assert_eq!(editor.document(), &before);
        assert_eq!(editor.undo_history().len(), steps + 1);
    }

    #[test]
    fn replacing_the_document_is_one_undoable_step() {
        let mut editor = Editor::new();
        let checkpoint = editor.document().clone();
        let track = midi_track(&mut editor);
        let clip = clip_on(&mut editor, track);
        let edited = editor.document().clone();

        let changes = editor.replace("Restore checkpoint", checkpoint.clone());
        assert_eq!(editor.document(), &checkpoint);
        assert_eq!(changes.tracks.get(&track), Some(&crate::Change::Removed));
        assert_eq!(changes.clips.get(&clip), Some(&crate::Change::Removed));
        assert_eq!(editor.undo_history().next(), Some("Restore checkpoint"));
        editor.undo().unwrap();
        assert_eq!(editor.document(), &edited);
        editor.redo().unwrap();
        assert_eq!(editor.document(), &checkpoint);

        // The same document again changes nothing and records nothing.
        let steps = editor.undo_history().len();
        assert!(editor.replace("Again", checkpoint).is_empty());
        assert_eq!(editor.undo_history().len(), steps);

        // A document from elsewhere moves the allocator past its IDs.
        let mut other = Editor::new();
        for _ in 0..10 {
            midi_track(&mut other);
        }
        editor.replace("Other", other.document().clone());
        let fresh: TrackId = editor.new_id();
        assert!(fresh.0 > other.document().max_id());
    }

    // Spec: undo-history R6, R7, R9
    #[test]
    fn a_jump_walks_the_history_without_recording_a_step() {
        let mut editor = Editor::new();
        let start = editor.document().clone();
        let track = midi_track(&mut editor);
        let one = editor.document().clone();
        editor
            .apply(Command::SetVolume { track, volume: 0.5 })
            .unwrap();
        let two = editor.document().clone();
        editor.apply(Command::SetPan { track, pan: 0.5 }).unwrap();
        let three = editor.document().clone();
        assert_eq!(editor.history().len(), 3);

        // Back to the step before the newest one.
        editor.go_to_step(2).unwrap();
        assert_eq!(editor.document(), &two);
        // The jump is no step of its own.
        assert_eq!(editor.history().len(), 3);
        // The step above it is greyed, and can be redone.
        let rows = editor.history();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].undone, "the newest step is undone");
        assert_eq!(rows[0].label, "Change pan");
        assert!(!rows[1].undone);
        assert_eq!(rows[1].index, 2);
        assert_eq!(rows[2].index, 1);

        // Back to the set's start undoes every step.
        editor.go_to_step(0).unwrap();
        assert_eq!(editor.document(), &start);
        assert_eq!(editor.history().len(), 3);
        assert!(editor.history().iter().all(|row| row.undone));

        // And forward again gives every version back.
        editor.go_to_step(3).unwrap();
        assert_eq!(editor.document(), &three);
        editor.go_to_step(1).unwrap();
        assert_eq!(editor.document(), &one);
        editor.go_to_step(3).unwrap();
        assert_eq!(editor.document(), &three);

        // A row the history doesn't hold is refused.
        assert_eq!(
            editor.go_to_step(4),
            Err(EditError::NoSuchStep { index: 4, steps: 3 })
        );
    }

    // Spec: undo-history R7
    #[test]
    fn a_new_edit_after_a_jump_discards_the_greyed_steps() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        editor
            .apply(Command::SetVolume { track, volume: 0.5 })
            .unwrap();
        editor.apply(Command::SetPan { track, pan: 0.5 }).unwrap();
        editor.go_to_step(1).unwrap();
        assert!(editor.can_redo());

        editor
            .apply(Command::SetMute { track, mute: true })
            .unwrap();
        assert!(!editor.can_redo());
        assert_eq!(editor.history().len(), 2);
        assert_eq!(editor.history()[0].label, "Mute track");
    }

    // Spec: undo-history R9
    #[test]
    fn a_steps_maker_survives_undo_and_redo() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        editor
            .apply(Command::SetVolume { track, volume: 0.5 })
            .unwrap();
        editor.begin_by(Maker::Agent("editor".to_owned()), "editor: Set Volume");
        editor.apply(Command::SetPan { track, pan: 0.5 }).unwrap();
        editor.commit().unwrap();

        let rows = editor.history();
        assert_eq!(rows[0].maker, Maker::Agent("editor".to_owned()));
        assert_eq!(rows[1].maker, Maker::Person);

        // Undoing and redoing keeps the maker with the step.
        editor.undo().unwrap();
        assert_eq!(editor.history()[0].maker, Maker::Agent("editor".to_owned()));
        editor.redo().unwrap();
        assert_eq!(editor.history()[0].maker, Maker::Agent("editor".to_owned()));
    }

    #[test]
    fn undo_limit_drops_the_oldest() {
        let mut editor = Editor::new();
        let track = midi_track(&mut editor);
        for step in 1..=5_u32 {
            editor
                .apply(Command::SetVolume {
                    track,
                    volume: f64::from(step) / 10.0,
                })
                .unwrap();
        }
        editor.set_undo_limit(Some(3));
        assert_eq!(editor.undo_history().len(), 3);
        for _ in 0..3 {
            editor.undo().unwrap();
        }
        assert_eq!(editor.undo(), Err(EditError::NothingToUndo));
        assert_eq!(editor.document().track(track).unwrap().mixer().volume, 0.2);
        editor.clear_history().unwrap();
        assert!(!editor.can_redo());
    }
}
