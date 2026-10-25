//! What the native MIDI effects share (WP-5.30; the device framework spec,
//! R30 to R34): a table of the notes each input note started, so an input note-off
//! ends the notes it made and its per-note expressions follow them, a queue
//! of notes held for later, and a note-off for everything still sounding
//! when the device is switched off, moved or removed. Both are made with the
//! device and fixed in size: a note past their room is dropped and counted.

use bl_engine::scales::OFF;
use bl_engine::{NoteBuffer, NoteEvent, NoteExpression, NoteKind};
use bl_model::{Device, ParamId, Scale, ScaleName};

use crate::framework::{Audio, ParamBank};

/// How many notes a device tracks at once.
const TABLE_ROOM: usize = 1024;
/// How many notes a device holds for later at once.
const QUEUE_ROOM: usize = 1024;
/// Generated notes take IDs from here up, apart from the IDs controllers
/// give their own notes.
const FIRST_ID: u32 = 0x8000_0000;

/// The value of parameter `id`, which the device's description holds.
pub(crate) fn value(params: &ParamBank, id: ParamId) -> f64 {
    params.value(params.index(id).expect("the description holds it"))
}

/// Whether a scale-aware device's Use Current Scale switch `switch` is on and
/// acts: it acts as off while the set's tuning hides the scales (the tunings
/// spec, R16).
pub(crate) fn use_current_scale(
    audio: &Audio<'_, '_>,
    params: &ParamBank,
    switch: ParamId,
) -> bool {
    value(params, switch) >= 0.5 && !audio.scales_hidden()
}

/// The scale a scale-aware device counts degrees of, `offset` frames into
/// its stretch (the scales spec, R15, R16): its current scale while its Use
/// Current Scale switch `switch` is on, otherwise the chromatic one, whose
/// degree is a semitone.
pub(crate) fn scale_in_use(
    audio: &Audio<'_, '_>,
    params: &ParamBank,
    switch: ParamId,
    offset: u32,
) -> Scale {
    if use_current_scale(audio, params, switch) {
        audio.scale_at(offset)
    } else {
        OFF
    }
}

/// The entries of a chooser of scale names: `first`, then the scale list in
/// its order (the scales spec, R1).
pub(crate) fn scale_choices(first: &'static str) -> Vec<&'static str> {
    std::iter::once(first)
        .chain(ScaleName::ALL.iter().map(|name| name.label()))
        .collect()
}

/// The scale a chooser entry names, counting from the entry after `first`:
/// `None` for the first entry itself and for one past the list.
pub(crate) fn scale_named(entry: usize) -> Option<ScaleName> {
    entry
        .checked_sub(1)
        .and_then(|n| ScaleName::ALL.get(n))
        .copied()
}

/// A seed for the random choices of `device`: its ID, so a render repeats
/// and two devices differ.
pub(crate) fn seed(device: &Device) -> u64 {
    device.id.0.get().wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// A note's identity: its channel, key and ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    pub(crate) channel: u8,
    pub(crate) key: u8,
    pub(crate) id: Option<u32>,
}

impl Key {
    pub(crate) fn of(note: &NoteEvent) -> Self {
        Self {
            channel: note.channel,
            key: note.key,
            id: note.note_id,
        }
    }

    /// Whether `note` is about the note this key names: the same channel
    /// and key, and the same ID unless one side has none.
    pub(crate) fn names(&self, note: &NoteEvent) -> bool {
        self.channel == note.channel
            && self.key == note.key
            && (self.id.is_none() || note.note_id.is_none() || self.id == note.note_id)
    }
}

/// One note an input note started, or an input note that started none.
#[derive(Clone, Copy, Debug)]
struct Entry {
    source: Key,
    /// The note it made, or `None` for an input note that was dropped.
    made: Option<Key>,
    /// Whether the note-on went out: a note held for later isn't sounding.
    sounding: bool,
    /// Whether the source's per-note expressions reach the note.
    follows: bool,
}

/// A note held for later, on the device's own clock.
#[derive(Clone, Copy, Debug)]
struct Due {
    at: i64,
    note: NoteEvent,
    source: Key,
}

/// The table and the queue.
#[derive(Debug)]
pub(crate) struct Router {
    entries: Vec<Entry>,
    queue: Vec<Due>,
    /// Frames the device has processed: the clock the queue runs on, which
    /// keeps going while the transport is stopped.
    now: i64,
    next_id: u32,
    dropped: u64,
}

impl Router {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::with_capacity(TABLE_ROOM),
            queue: Vec::with_capacity(QUEUE_ROOM),
            now: 0,
            next_id: FIRST_ID,
            dropped: 0,
        }
    }

    /// Notes dropped since the device was made, for lack of room.
    #[must_use]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The device's clock at the stretch's first frame.
    #[must_use]
    pub(crate) fn now(&self) -> i64 {
        self.now
    }

    /// An ID for a note the device makes.
    pub(crate) fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = FIRST_ID | (self.next_id.wrapping_add(1) & !FIRST_ID);
        id
    }

    /// Records that `source` made `made` (or nothing), not yet sounding.
    /// Returns `false`, and records nothing, if the table is full.
    pub(crate) fn track(&mut self, source: &NoteEvent, made: Option<Key>) -> bool {
        self.track_with(source, made, true)
    }

    /// As [`track`](Self::track), saying whether the source's per-note
    /// expressions follow the note it made.
    pub(crate) fn track_with(
        &mut self,
        source: &NoteEvent,
        made: Option<Key>,
        follows: bool,
    ) -> bool {
        if self.entries.len() == TABLE_ROOM {
            self.dropped += 1;
            return false;
        }
        self.entries.push(Entry {
            source: Key::of(source),
            made,
            sounding: false,
            follows,
        });
        true
    }

    /// Whether a note on `channel` and `key` is sounding or held for later.
    pub(crate) fn active(&self, channel: u8, key: u8) -> bool {
        self.entries
            .iter()
            .any(|e| e.made.is_some_and(|m| m.channel == channel && m.key == key))
    }

    /// Whether the table holds anything made from a note `note` names.
    pub(crate) fn knows(&self, note: &NoteEvent) -> bool {
        self.entries.iter().any(|e| e.source.names(note))
    }

    /// Sends `note` now, marking its entry sounding if it is a note-on.
    pub(crate) fn send_now(&mut self, audio: &mut Audio<'_, '_>, note: NoteEvent, source: Key) {
        self.mark(&note, source);
        audio.send(note);
    }

    /// Starts `on` at the device's clock `at` and ends it with `off`
    /// `length` frames later: now if `at` is this stretch's frame, or from the
    /// queue.
    pub(crate) fn sound(
        &mut self,
        audio: &mut Audio<'_, '_>,
        on: NoteEvent,
        off: NoteEvent,
        source: Key,
        at: i64,
        length: i64,
    ) {
        if at <= self.now + i64::from(on.offset) {
            self.send_now(audio, on, source);
        } else {
            self.hold(at, on, source);
        }
        self.hold(at + length, off, source);
    }

    /// Holds `note` until the device's clock reaches `at` (at least the
    /// stretch's first frame). Returns `false`, and drops it, if the queue
    /// is full.
    pub(crate) fn hold(&mut self, at: i64, note: NoteEvent, source: Key) -> bool {
        if self.queue.len() == QUEUE_ROOM {
            self.dropped += 1;
            return false;
        }
        self.queue.push(Due { at, note, source });
        true
    }

    /// Sends what is due before `offset` frames into the stretch. Call it
    /// before handling an input note there, so the note sees the queue as it
    /// stands on its own frame, whatever the block size.
    pub(crate) fn advance(&mut self, audio: &mut Audio<'_, '_>, offset: u32) {
        self.send_due(audio, self.now + i64::from(offset));
    }

    /// Sends what is due in the stretch, and moves the clock past it. Call
    /// it once at the end of each stretch.
    pub(crate) fn finish(&mut self, audio: &mut Audio<'_, '_>) {
        self.send_due(audio, self.now + audio.frames() as i64);
        self.now += audio.frames() as i64;
    }

    /// Sends what is due before `end` on the device's clock.
    fn send_due(&mut self, audio: &mut Audio<'_, '_>, end: i64) {
        let start = self.now;
        let mut kept = 0;
        for i in 0..self.queue.len() {
            let due = self.queue[i];
            if due.at < end {
                let offset = (due.at - start).max(0) as u32;
                self.send_now(audio, NoteEvent { offset, ..due.note }, due.source);
            } else {
                self.queue[kept] = due;
                kept += 1;
            }
        }
        self.queue.truncate(kept);
    }

    /// Marks the entry `note` belongs to: a note-on is sounding, a
    /// note-off removes it.
    fn mark(&mut self, note: &NoteEvent, source: Key) {
        let made = Some(Key::of(note));
        let is_on = note.kind == NoteKind::On;
        // The first of its kind: a note-on starts the first that isn't
        // sounding yet, a note-off ends the first that is.
        let at = self
            .entries
            .iter()
            .position(|e| e.source == source && e.made == made && e.sounding != is_on);
        match (is_on, at) {
            (true, Some(i)) => self.entries[i].sounding = true,
            (false, Some(i)) if matches!(note.kind, NoteKind::Off | NoteKind::Choke) => {
                self.entries.remove(i);
            }
            _ => {}
        }
    }

    /// Takes everything made from the note `note` names off the table and
    /// off the queue, calling `f` with each note that is sounding. Returns
    /// whether there was anything on the table for it.
    pub(crate) fn end_source(&mut self, note: &NoteEvent, mut f: impl FnMut(Key)) -> bool {
        let mut found = false;
        let mut kept = 0;
        for i in 0..self.entries.len() {
            let entry = self.entries[i];
            if entry.source.names(note) {
                found = true;
                if let (Some(made), true) = (entry.made, entry.sounding) {
                    f(made);
                }
            } else {
                self.entries[kept] = entry;
                kept += 1;
            }
        }
        self.entries.truncate(kept);
        // What was held for them never sounds.
        let mut kept = 0;
        for i in 0..self.queue.len() {
            let due = self.queue[i];
            if !(due.source.names(note) && due.note.kind == NoteKind::On) {
                self.queue[kept] = due;
                kept += 1;
            }
        }
        self.queue.truncate(kept);
        found
    }

    /// Ends everything made from the note `note` names, with release
    /// velocity `velocity`; a note-off for a note the table doesn't know
    /// passes through as it is.
    pub(crate) fn end(&mut self, audio: &mut Audio<'_, '_>, note: &NoteEvent, velocity: f32) {
        let found = self.end_source(note, |made| {
            audio.send(NoteEvent {
                channel: made.channel,
                key: made.key,
                note_id: made.id,
                velocity,
                ..*note
            });
        });
        if !found {
            audio.send(*note);
        }
    }

    /// Passes a per-note expression on to the notes made from the note it
    /// names, or as it is if the table doesn't know that note.
    pub(crate) fn expression(&self, audio: &mut Audio<'_, '_>, note: &NoteEvent) {
        self.expression_by(audio, note, |_, _, bend| bend);
    }

    /// As [`expression`](Self::expression), carrying a per-note bend through
    /// `bend`, which takes the source's key, the key of the note made and the
    /// bend, and gives the bend the note made gets (the scales spec, R18).
    pub(crate) fn expression_by(
        &self,
        audio: &mut Audio<'_, '_>,
        note: &NoteEvent,
        bend: impl Fn(u8, u8, f32) -> f32,
    ) {
        if self.knows(note) {
            self.follow(audio, note, bend);
        } else {
            audio.send(*note);
        }
    }

    /// Sends the expression `note` carries to everything sounding that its
    /// source made, so a bend, pressure or slide follows the note it
    /// belongs to; the bend goes through `bend`.
    fn follow(
        &self,
        audio: &mut Audio<'_, '_>,
        note: &NoteEvent,
        bend: impl Fn(u8, u8, f32) -> f32,
    ) {
        for entry in &self.entries {
            if let (true, Some(made), true, true) = (
                entry.source.names(note),
                entry.made,
                entry.sounding,
                entry.follows,
            ) {
                let kind = match note.kind {
                    NoteKind::Expression {
                        expression: NoteExpression::Bend,
                        value,
                    } => NoteKind::Expression {
                        expression: NoteExpression::Bend,
                        value: bend(entry.source.key, made.key, value),
                    },
                    kind => kind,
                };
                audio.send(NoteEvent {
                    channel: made.channel,
                    key: made.key,
                    note_id: made.id,
                    kind,
                    ..*note
                });
            }
        }
    }

    /// Ends, at `offset` frames into the stretch, every sounding note whose
    /// key `ends` picks, and forgets what was held to end it later.
    pub(crate) fn cut(
        &mut self,
        audio: &mut Audio<'_, '_>,
        offset: u32,
        ends: impl Fn(Key) -> bool,
    ) {
        let mut kept = 0;
        for i in 0..self.entries.len() {
            let entry = self.entries[i];
            match entry.made {
                Some(made) if entry.sounding && ends(made) => audio.send(NoteEvent {
                    offset,
                    kind: NoteKind::Off,
                    channel: made.channel,
                    key: made.key,
                    velocity: 0.5,
                    note_id: made.id,
                }),
                _ => {
                    self.entries[kept] = entry;
                    kept += 1;
                }
            }
        }
        self.entries.truncate(kept);
        let mut kept = 0;
        for i in 0..self.queue.len() {
            let due = self.queue[i];
            if !(due.note.kind == NoteKind::Off && ends(Key::of(&due.note))) {
                self.queue[kept] = due;
                kept += 1;
            }
        }
        self.queue.truncate(kept);
    }

    /// Forgets all held notes in the queue and drops entries that have not yet sounded.
    pub(crate) fn clear_queue(&mut self) {
        self.queue.clear();
        self.entries.retain(|e| e.sounding);
    }

    /// Writes a note-off at offset 0 for every sounding note, and forgets
    /// them and the queue (the device framework spec, R32).
    pub(crate) fn release(&mut self, out: &mut NoteBuffer) {
        for entry in &self.entries {
            if let (Some(made), true) = (entry.made, entry.sounding) {
                out.push(NoteEvent {
                    offset: 0,
                    kind: NoteKind::Off,
                    channel: made.channel,
                    key: made.key,
                    velocity: 0.0,
                    note_id: made.id,
                });
            }
        }
        self.entries.clear();
        self.queue.clear();
    }
}
