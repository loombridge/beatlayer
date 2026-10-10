//! The editor: the current document, transactions, and the undo history.

use core::fmt;
use std::borrow::Cow;
use std::collections::{BTreeSet, VecDeque};

use bl_core::id::{Id, IdAllocator};

use crate::changes::ChangeSet;
use crate::command::Command;
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::TrackId;

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
    start: Document,
}

/// A step in the history: its label and one version of the document. On the
/// undo stack that's the version before the step; on the redo stack, the
/// version after it. The current document is the other end of the newest
/// step on either stack, so no version is stored twice. The version is boxed
/// to keep the stacks' unused capacity small.
#[derive(Debug)]
struct Step {
    label: Cow<'static, str>,
    document: Box<Document>,
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
    /// An editor for a new, empty song (just the Main track) with an empty
    /// history.
    #[must_use]
    pub fn new() -> Self {
        let mut ids = IdAllocator::new();
        let main = TrackId(ids.allocate());
        Self::from_document(Document::new(main), ids)
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
    pub fn apply(&mut self, command: Command) -> Result<ChangeSet, EditError> {
        let created = command.created_id();
        if let Some(id) = created
            && !self.unused_ids.contains(&id)
        {
            return Err(EditError::IdNotIssued(id));
        }
        let before = self.document.clone();
        self.document.apply(&command)?;
        if let Some(id) = created {
            self.unused_ids.remove(&id);
        }
        let changes = ChangeSet::between(&before, &self.document);
        if self.open.is_empty() && !changes.is_empty() {
            self.record(command.label().into(), before);
        }
        Ok(changes)
    }

    /// Opens a transaction. Its label names the undo step, such as "Move
    /// clips". Inside another transaction, the outer one's label is used.
    pub fn begin(&mut self, label: impl Into<Cow<'static, str>>) {
        self.open.push(Open {
            label: label.into(),
            start: self.document.clone(),
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
        if !self.open.is_empty() || open.start == self.document {
            return Ok(false);
        }
        self.record(open.label, open.start);
        Ok(true)
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
        self.begin(label);
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
        let Step { label, document } = self.undo.pop_back().ok_or(EditError::NothingToUndo)?;
        let changes = ChangeSet::between(&self.document, &document);
        let after = std::mem::replace(&mut self.document, *document);
        self.redo.push(Step {
            label,
            document: Box::new(after),
        });
        Ok(changes)
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
        let Step { label, document } = self.redo.pop().ok_or(EditError::NothingToRedo)?;
        let changes = ChangeSet::between(&self.document, &document);
        let before = std::mem::replace(&mut self.document, *document);
        self.undo.push_back(Step {
            label,
            document: Box::new(before),
        });
        Ok(changes)
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

    fn record(&mut self, label: Cow<'static, str>, before: Document) {
        self.redo.clear();
        self.undo.push_back(Step {
            label,
            document: Box::new(before),
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
