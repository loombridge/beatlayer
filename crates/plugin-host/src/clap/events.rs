//! The event lists a CLAP plug-in reads and writes in each block, with
//! room made before the audio thread uses them: an event past that room
//! is dropped and counted (real-time rule 4), never allocated for.

use bl_engine::{NoteEvent, NoteKind};
use clack_host::events::event_types::{
    NoteChokeEvent, NoteOffEvent, NoteOnEvent, ParamModEvent, ParamValueEvent,
};
use clack_host::events::io::{InputEventBuffer, OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::{Match, Pckn, UnknownEvent};
use clack_host::prelude::ClapId;

/// How many events a block's lists hold each way.
pub(crate) const EVENT_CAPACITY: usize = 1024;

/// One event for the plug-in.
#[derive(Clone, Copy)]
enum In {
    Param(ParamValueEvent),
    Mod(ParamModEvent),
    On(NoteOnEvent),
    Off(NoteOffEvent),
    Choke(NoteChokeEvent),
}

impl In {
    fn time(&self) -> u32 {
        self.as_unknown().header().time()
    }

    fn as_unknown(&self) -> &UnknownEvent {
        match self {
            Self::Param(e) => e.as_ref(),
            Self::Mod(e) => e.as_ref(),
            Self::On(e) => e.as_ref(),
            Self::Off(e) => e.as_ref(),
            Self::Choke(e) => e.as_ref(),
        }
    }
}

/// The events a plug-in reads in a block, in order of time.
pub(crate) struct InputList {
    events: Vec<In>,
    pub(crate) dropped: u64,
}

impl InputList {
    pub(crate) fn new() -> Self {
        Self {
            events: Vec::with_capacity(EVENT_CAPACITY),
            dropped: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.events.clear();
    }

    fn push(&mut self, event: In) {
        if self.events.len() < self.events.capacity() {
            self.events.push(event);
        } else {
            self.dropped += 1;
        }
    }

    /// Adds a parameter change.
    pub(crate) fn param(&mut self, time: u32, id: u32, value: f64) {
        if let Some(id) = ClapId::from_raw(id) {
            self.push(In::Param(ParamValueEvent::new(
                time,
                id,
                Pckn::match_all(),
                value,
            )));
        }
    }

    /// Adds a modulation of a parameter marked modulatable: its `amount`, in
    /// the parameter's units, on top of its value, for every note and
    /// channel.
    pub(crate) fn param_mod(&mut self, time: u32, id: u32, amount: f64) {
        if let Some(id) = ClapId::from_raw(id) {
            self.push(In::Mod(ParamModEvent::new(
                time,
                id,
                Pckn::match_all(),
                amount,
            )));
        }
    }

    /// Adds a note.
    pub(crate) fn note(&mut self, note: &NoteEvent) {
        let pckn = Pckn::new(
            0u16,
            u16::from(note.channel.min(15)),
            u16::from(note.key.min(127)),
            note.note_id.map_or(Match::All, Match::Specific),
        );
        let velocity = f64::from(note.velocity.clamp(0.0, 1.0));
        self.push(match note.kind {
            NoteKind::On => In::On(NoteOnEvent::new(note.offset, pckn, velocity)),
            NoteKind::Off => In::Off(NoteOffEvent::new(note.offset, pckn, velocity)),
            NoteKind::Choke => In::Choke(NoteChokeEvent::new(note.offset, pckn)),
            // Per-note expressions wait for MPE hosting (the parity
            // matrix's note): a plug-in's note port takes them then.
            NoteKind::Expression { .. } => return,
        });
    }

    /// Puts the events in order of time, keeping the order of events at
    /// one time (parameter changes before the notes they were added
    /// before). An insertion sort, which allocates nothing; the lists are
    /// nearly in order already.
    pub(crate) fn sort(&mut self) {
        for i in 1..self.events.len() {
            let mut j = i;
            while j > 0 && self.events[j - 1].time() > self.events[j].time() {
                self.events.swap(j - 1, j);
                j -= 1;
            }
        }
    }
}

impl InputEventBuffer for InputList {
    fn len(&self) -> u32 {
        u32::try_from(self.events.len()).unwrap_or(u32::MAX)
    }

    fn get(&self, index: u32) -> Option<&UnknownEvent> {
        self.events.get(index as usize).map(In::as_unknown)
    }
}

/// What a plug-in sent out in a block that Beatlayer takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Out {
    /// A note, which the node sends on.
    Note(NoteEvent),
    /// A parameter's new value, from the plug-in itself (its editor).
    Param(u32, f64),
    /// A person started changing a parameter in its editor.
    Begin(u32),
    /// They finished.
    End(u32),
}

/// The events a plug-in writes in a block.
pub(crate) struct OutputList {
    pub(crate) events: Vec<Out>,
    pub(crate) dropped: u64,
}

impl OutputList {
    pub(crate) fn new() -> Self {
        Self {
            events: Vec::with_capacity(EVENT_CAPACITY),
            dropped: 0,
        }
    }

    fn take(&mut self, event: &UnknownEvent) -> bool {
        let time = event.header().time();
        let note = |kind, pckn: Pckn, velocity: f64| {
            let key = pckn.key.into_specific()?;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "clamped to 0..=127 and 0..=15"
            )]
            Some(Out::Note(NoteEvent {
                offset: time,
                kind,
                channel: pckn.channel.into_specific().unwrap_or(0).min(15) as u8,
                key: key.min(127) as u8,
                velocity: velocity.clamp(0.0, 1.0) as f32,
                note_id: pckn.note_id.into_specific(),
            }))
        };
        let out = match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(e)) => note(NoteKind::On, e.pckn(), e.velocity()),
            Some(CoreEventSpace::NoteOff(e)) => note(NoteKind::Off, e.pckn(), e.velocity()),
            Some(CoreEventSpace::NoteChoke(e)) => note(NoteKind::Choke, e.pckn(), 0.0),
            Some(CoreEventSpace::ParamValue(e)) => e
                .param_id()
                .filter(|_| e.value().is_finite())
                .map(|id| Out::Param(id.get(), e.value())),
            Some(CoreEventSpace::ParamGestureBegin(e)) => {
                e.param_id().map(|id| Out::Begin(id.get()))
            }
            Some(CoreEventSpace::ParamGestureEnd(e)) => e.param_id().map(|id| Out::End(id.get())),
            _ => None,
        };
        let Some(out) = out else {
            // Events Beatlayer doesn't take are accepted and ignored.
            return true;
        };
        if self.events.len() < self.events.capacity() {
            self.events.push(out);
            true
        } else {
            self.dropped += 1;
            false
        }
    }
}

impl OutputEventBuffer for OutputList {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        if self.take(event) {
            Ok(())
        } else {
            Err(TryPushError::new())
        }
    }
}
