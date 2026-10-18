//! The note routing of a drum rack's chains (the drum racks spec, R1 to R5).
//!
//! Each drum chain has one [`DrumNotes`]: it passes the notes its Receive
//! answers on to the chain's devices at the chain's Play note, remembers the
//! pitch each held note got so its note-off follows, and remembers which
//! keys the chain has started, so a note in another chain of its choke group
//! can silence them. It runs on the rack's own thread before the chains, so
//! nothing in it waits on another chain.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use bl_model::{ChainId, ParamId};

use crate::node::{NodeEvent, NoteBuffer, NoteEvent, NoteKind, ParamValue};

/// How many note-ons have reached each drum chain since its note routing
/// was made, which the shell reads to select the chain a note reaches (Auto
/// Select; the drum racks spec, R20). Only the compiler and the shell take
/// its lock; the audio thread bumps the counters it already holds.
#[derive(Clone, Default)]
pub struct ChainHits(Arc<Mutex<BTreeMap<ChainId, Arc<AtomicU64>>>>);

impl ChainHits {
    /// The counter of `chain`, made if it has none.
    pub(crate) fn counter(&self, chain: ChainId) -> Arc<AtomicU64> {
        Arc::clone(self.lock().entry(chain).or_default())
    }

    /// Forgets the chains `keep` refuses: those the program no longer has.
    pub(crate) fn retain(&self, mut keep: impl FnMut(ChainId) -> bool) {
        self.lock().retain(|chain, _| keep(*chain));
    }

    /// Copies each chain's count into `out`.
    pub fn read_all(&self, out: &mut BTreeMap<ChainId, u64>) {
        out.clear();
        out.extend(
            self.lock()
                .iter()
                .map(|(chain, n)| (*chain, n.load(Relaxed))),
        );
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<ChainId, Arc<AtomicU64>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl std::fmt::Debug for ChainHits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainHits").finish_non_exhaustive()
    }
}

/// The parameters of a chain's [`DrumNotes`], set by the compiler with the
/// document's values.
pub mod param {
    use bl_model::ParamId;

    /// The note the chain answers, 0 to 127, or [`ALL_NOTES`].
    pub const RECEIVE: ParamId = ParamId(0);
    /// The note the chain's devices get, 0 to 127.
    pub const PLAY: ParamId = ParamId(1);
    /// The choke group, 1 to 16, or 0 for none.
    pub const CHOKE: ParamId = ParamId(2);
    /// Whether the chain's Preview button is held: 1 or 0 (the drum racks
    /// spec, R5). Sent by the shell, not the document.
    pub const PREVIEW: ParamId = ParamId(3);
    /// The value of [`RECEIVE`] that answers every note.
    pub const ALL_NOTES: f64 = 128.0;
}

/// The velocity a preview plays at: the computer keyboard's 100 (R5).
const PREVIEW_VELOCITY: f32 = 100.0 / 127.0;
/// The note a chain on All Notes previews: C3.
const PREVIEW_ALL_NOTES: u8 = 60;
/// A key that isn't held.
const NOT_HELD: u8 = u8::MAX;

/// One drum chain's note routing.
#[derive(Clone, Debug)]
pub(crate) struct DrumNotes {
    /// The note answered, or [`param::ALL_NOTES`].
    receive: u8,
    play: u8,
    /// The choke group, or 0 for none.
    choke: u8,
    /// For each key that came in and went on, the key it went on as, until
    /// its note-off: what a note-off follows (R2).
    held: [u8; 128],
    /// The keys the chain started since a choke ended them, as bits.
    sounded: u128,
    /// For each sounded key, the MIDI channel its note-on came on, which
    /// its choke goes out on.
    channels: [u8; 128],
    /// Whether Preview is held, and whether its note-on has gone in.
    preview: bool,
    previewing: Option<u8>,
    /// Counts the note-ons that reach the chain, for the shell.
    hits: Arc<AtomicU64>,
}

impl DrumNotes {
    pub(crate) fn new(receive: u8, play: u8, choke: u8, hits: Arc<AtomicU64>) -> Self {
        Self {
            hits,
            receive,
            play,
            choke,
            held: [NOT_HELD; 128],
            sounded: 0,
            channels: [0; 128],
            preview: false,
            previewing: None,
        }
    }

    /// Sets a parameter.
    fn set(&mut self, param: ParamId, value: ParamValue) {
        let ParamValue::Set(value) = value else {
            return;
        };
        let value = if value.is_finite() { value } else { 0.0 };
        match param {
            param::RECEIVE => self.receive = value.clamp(0.0, param::ALL_NOTES) as u8,
            param::PLAY => self.play = value.clamp(0.0, 127.0) as u8,
            param::CHOKE => self.choke = value.clamp(0.0, 16.0) as u8,
            param::PREVIEW => self.preview = value != 0.0,
            _ => {}
        }
    }

    /// Takes the parameter changes of a block that the chain's notes aren't
    /// routed in: a switched-off rack's.
    pub(crate) fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event.param, event.value);
        }
    }

    /// Whether the chain answers every note.
    fn all_notes(&self) -> bool {
        self.receive > 127
    }

    /// The choke group the chain is in, 0 for none: a chain on All Notes is
    /// in none (R2).
    pub(crate) fn group(&self) -> u8 {
        if self.all_notes() { 0 } else { self.choke }
    }

    /// Notes the chain started: takes them, as a choke ends them.
    pub(crate) fn take_sounded(&mut self) -> u128 {
        std::mem::take(&mut self.sounded)
    }

    /// The MIDI channel a sounded key's note-on came on.
    pub(crate) fn channel(&self, key: u8) -> u8 {
        self.channels[usize::from(key & 0x7F)]
    }

    /// Records that a note-on of `key` on `channel` reached the chain's
    /// devices.
    pub(crate) fn sound(&mut self, key: u8, channel: u8) {
        self.sounded |= 1u128 << (key & 0x7F);
        self.channels[usize::from(key & 0x7F)] = channel;
    }

    /// Fills `out` with the notes of `input` that reach the chain, as they
    /// reach it: a note-on the chain answers at its Play note, the note-off
    /// of each at the pitch its note-on got, and every message other than
    /// a note as it came (R2). A held Preview button plays its note too
    /// (R5). `events` are the parameter changes of the block.
    pub(crate) fn route(
        &mut self,
        input: &[NoteEvent],
        out: &mut NoteBuffer,
        events: &[NodeEvent],
    ) {
        self.flush(events);
        out.clear();
        let wanted = self.preview.then(|| {
            if self.all_notes() {
                PREVIEW_ALL_NOTES
            } else {
                self.receive
            }
        });
        // The preview note goes in on the block's first frame, ahead of the
        // notes that came.
        if let (None, Some(key)) = (self.previewing, wanted) {
            self.previewing = Some(key);
            self.take(&preview_note(NoteKind::On, key), out);
        }
        for note in input {
            self.take(note, out);
        }
        if let (Some(key), None) = (self.previewing, wanted) {
            self.previewing = None;
            self.take(&preview_note(NoteKind::Off, key), out);
        }
    }

    /// Passes one note on, if it reaches the chain.
    fn take(&mut self, note: &NoteEvent, out: &mut NoteBuffer) {
        match note.kind {
            NoteKind::On => {
                if self.all_notes() || note.key == self.receive {
                    let key = if self.all_notes() {
                        note.key
                    } else {
                        self.play
                    };
                    self.held[usize::from(note.key & 0x7F)] = key;
                    self.hits.fetch_add(1, Relaxed);
                    out.push(NoteEvent { key, ..*note });
                }
            }
            NoteKind::Off | NoteKind::Choke | NoteKind::Expression { .. } => {
                let held = self.held[usize::from(note.key & 0x7F)];
                if held != NOT_HELD {
                    out.push(NoteEvent { key: held, ..*note });
                    if !matches!(note.kind, NoteKind::Expression { .. }) {
                        self.held[usize::from(note.key & 0x7F)] = NOT_HELD;
                    }
                }
            }
            NoteKind::Control { .. }
            | NoteKind::PitchBend { .. }
            | NoteKind::ChannelPressure { .. } => out.push(*note),
        }
    }
}

fn preview_note(kind: NoteKind, key: u8) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind,
        channel: 0,
        key,
        velocity: if kind == NoteKind::On {
            PREVIEW_VELOCITY
        } else {
            0.0
        },
        note_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(offset: u32, key: u8) -> NoteEvent {
        NoteEvent {
            offset,
            kind: NoteKind::On,
            channel: 0,
            key,
            velocity: 0.5,
            note_id: None,
        }
    }

    fn off(offset: u32, key: u8) -> NoteEvent {
        NoteEvent {
            kind: NoteKind::Off,
            velocity: 0.0,
            ..on(offset, key)
        }
    }

    fn routed(drum: &mut DrumNotes, input: &[NoteEvent]) -> Vec<(u32, NoteKind, u8)> {
        let mut out = NoteBuffer::with_capacity(16);
        drum.route(input, &mut out, &[]);
        out.notes()
            .iter()
            .map(|n| (n.offset, n.kind, n.key))
            .collect()
    }

    // Spec: drum-racks R2
    #[test]
    fn a_note_goes_on_at_the_play_note_and_its_off_follows() {
        let mut drum = DrumNotes::new(36, 60, 0, Arc::default());
        let got = routed(&mut drum, &[on(5, 36), on(6, 37), off(9, 36), off(10, 37)]);
        assert_eq!(got, [(5, NoteKind::On, 60), (9, NoteKind::Off, 60)]);
    }

    // Spec: drum-racks R2
    #[test]
    fn a_note_off_follows_its_note_on_after_receive_and_play_change() {
        let mut drum = DrumNotes::new(36, 60, 0, Arc::default());
        routed(&mut drum, &[on(0, 36)]);
        drum.set(param::RECEIVE, ParamValue::Set(40.0));
        drum.set(param::PLAY, ParamValue::Set(72.0));
        assert_eq!(routed(&mut drum, &[off(3, 36)]), [(3, NoteKind::Off, 60)]);
    }

    // Spec: drum-racks R2
    #[test]
    fn all_notes_passes_every_note_as_it_came_and_messages_reach_every_chain() {
        let mut drum = DrumNotes::new(36, 60, 4, Arc::default());
        drum.set(param::RECEIVE, ParamValue::Set(param::ALL_NOTES));
        assert_eq!(drum.group(), 0, "all notes is in no choke group");
        let bend = NoteEvent {
            kind: NoteKind::PitchBend { value: 0.5 },
            ..on(1, 0)
        };
        let got = routed(&mut drum, &[on(0, 41), bend, off(2, 41)]);
        assert_eq!(
            got.iter().map(|g| (g.0, g.2)).collect::<Vec<_>>(),
            [(0, 41), (1, 0), (2, 41)]
        );
        let mut other = DrumNotes::new(36, 60, 0, Arc::default());
        assert_eq!(routed(&mut other, &[bend]).len(), 1);
    }

    // Spec: drum-racks R5
    #[test]
    fn preview_plays_the_receive_note_while_held() {
        let mut drum = DrumNotes::new(38, 60, 0, Arc::default());
        drum.set(param::PREVIEW, ParamValue::Set(1.0));
        assert_eq!(routed(&mut drum, &[]), [(0, NoteKind::On, 60)]);
        assert!(routed(&mut drum, &[]).is_empty(), "the note stays on");
        drum.set(param::PREVIEW, ParamValue::Set(0.0));
        assert_eq!(routed(&mut drum, &[]), [(0, NoteKind::Off, 60)]);
    }
}
