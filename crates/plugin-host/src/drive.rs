//! What a plug-in's node plays of its parameters' automation and of clip
//! modulation (the modulation spec, R10), the same for every format: a
//! plug-in plays its own parameters, so the node works out each driven
//! parameter's base and played value and hands them to the plug-in as the
//! format takes them. Everything is made with the node, so nothing here
//! allocates on the audio thread.

use bl_engine::{AutomationList, TransportInfo};
use bl_model::modulation::Travel;
use bl_model::{ParamDescriptor, ParamId};

/// The song frames between the points a driven parameter is worked out at:
/// control rate, fixed on the song so a render is the same at any block
/// size.
const GRID: i64 = 32;

/// A value for the plug-in, `offset` frames into the block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Play {
    pub(crate) offset: u32,
    pub(crate) id: u32,
    /// The parameter's value before modulation.
    pub(crate) base: f64,
    /// The value it plays.
    pub(crate) played: f64,
    /// The plug-in takes modulation as events of its own (CLAP's
    /// modulatable parameters): the base is its value, and the difference
    /// to `played` its modulation. Otherwise it takes `played`.
    pub(crate) modulatable: bool,
}

struct Slot {
    id: u32,
    descriptor: ParamDescriptor,
    modulatable: bool,
    /// The control's value: the last one set, or the plug-in's at its
    /// activation.
    control: f64,
    /// What the plug-in was last given, while it's driven.
    sent: Option<(f64, f64)>,
    /// The base and value played at the last point worked out.
    last: (f64, f64),
    driven: bool,
    modulated: bool,
}

/// A plug-in's driven parameters.
pub(crate) struct Driver {
    /// By parameter ID.
    slots: Vec<Slot>,
    /// The values one point works out, with room for every parameter.
    scratch: Vec<(u32, f64, f64)>,
    /// The parameter values given as the parameters' own in this block,
    /// which the plug-in may report back (R10).
    echoes: Vec<(u32, f64)>,
    /// Some parameter is driven or still to be handed back.
    live: bool,
}

/// How many values a block's echoes keep; past that a value reported back
/// isn't known.
const ECHOES: usize = 1024;

fn find(slots: &[Slot], id: u32) -> Option<usize> {
    slots.binary_search_by_key(&id, |slot| slot.id).ok()
}

impl Driver {
    /// A driver for `params`: each with its descriptor, whether the plug-in
    /// takes modulation itself, and its control's value now.
    pub(crate) fn new(params: impl IntoIterator<Item = (ParamDescriptor, bool, f64)>) -> Self {
        let mut slots: Vec<Slot> = params
            .into_iter()
            .map(|(descriptor, modulatable, control)| Slot {
                id: descriptor.id.0,
                descriptor,
                modulatable,
                control,
                sent: None,
                last: (control, control),
                driven: false,
                modulated: false,
            })
            .collect();
        slots.sort_by_key(|slot| slot.id);
        slots.dedup_by_key(|slot| slot.id);
        let scratch = Vec::with_capacity(slots.len());
        Self {
            slots,
            scratch,
            echoes: Vec::with_capacity(ECHOES),
            live: false,
        }
    }

    /// Notes that the control of `id` was set to `value`.
    pub(crate) fn set_control(&mut self, id: u32, value: f64) {
        if let Some(at) = find(&self.slots, id) {
            self.slots[at].control = value;
        }
    }

    /// Works out which parameters the block drives: while the song plays,
    /// those a track envelope or a clip's modulation envelope holds. Call
    /// it before the block's changes and [`play`](Self::play).
    pub(crate) fn prepare(&mut self, list: Option<&AutomationList>, transport: TransportInfo) {
        self.echoes.clear();
        let list = Self::active(list, transport);
        if list.is_none() && !self.live {
            return;
        }
        for slot in &mut self.slots {
            let id = ParamId(slot.id);
            slot.modulated = list.is_some_and(|list| list.modulates(id));
            slot.driven = slot.modulated || list.is_some_and(|list| list.envelope(id).is_some());
        }
    }

    /// Whether the block drives `id`: its values come from `play`, and a
    /// change of its control only moves its base.
    pub(crate) fn is_driven(&self, id: u32) -> bool {
        find(&self.slots, id).is_some_and(|at| self.slots[at].driven)
    }

    /// Whether `value` is one the plug-in was given for `id` in its last
    /// block, which it reports back because the host sent it: neither an
    /// undo step nor an edit (R10).
    pub(crate) fn is_echo(&self, id: u32, value: f64) -> bool {
        self.echoes.contains(&(id, value))
    }

    fn active(list: Option<&AutomationList>, transport: TransportInfo) -> Option<&AutomationList> {
        list.filter(|list| transport.playing && !list.is_empty())
    }

    /// Calls `send` for each value the plug-in is to be given in a block of
    /// `frames`: at each song frame that is a multiple of the grid, and at
    /// the block's first frame for a parameter it hasn't been given a value
    /// for in this run, each only when it differs from the last one given;
    /// and, for a parameter no longer driven, its control's value once.
    // Spec: modulation R6, R10
    pub(crate) fn play(
        &mut self,
        list: Option<&AutomationList>,
        transport: TransportInfo,
        frames: usize,
        mut send: impl FnMut(Play),
    ) {
        if let Some(list) = Self::active(list, transport) {
            self.live = true;
            let frames = i64::try_from(frames).unwrap_or(i64::MAX);
            let mut offset = 0;
            while offset < frames {
                let frame = transport.position.0 + offset;
                let on_grid = frame.rem_euclid(GRID) == 0;
                self.visit(
                    list,
                    frame,
                    u32::try_from(offset).unwrap_or(0),
                    on_grid,
                    &mut send,
                );
                offset += GRID - frame.rem_euclid(GRID);
            }
        } else if !self.live {
            return;
        }
        // Spec: modulation R6. A parameter that stops being driven plays its
        // control again, and a CLAP one's modulation goes to nothing.
        self.live = false;
        for slot in &mut self.slots {
            if slot.driven {
                self.live = true;
            } else if slot.sent.take().is_some() {
                if self.echoes.len() < ECHOES {
                    self.echoes.push((slot.id, slot.control));
                }
                send(Play {
                    offset: 0,
                    id: slot.id,
                    base: slot.control,
                    played: slot.control,
                    modulatable: slot.modulatable,
                });
            }
        }
    }

    /// Works out the driven parameters at song `frame` and sends the ones
    /// due: all that differ on the grid, else the ones not yet sent.
    fn visit(
        &mut self,
        list: &AutomationList,
        frame: i64,
        offset: u32,
        on_grid: bool,
        send: &mut impl FnMut(Play),
    ) {
        let Self {
            slots,
            scratch,
            echoes,
            ..
        } = self;
        scratch.clear();
        {
            let slots: &[Slot] = slots;
            list.drive(
                frame,
                |id| find(slots, id.0).map(|at| Travel::Param(&slots[at].descriptor)),
                |id| find(slots, id.0).map_or(0.0, |at| slots[at].control),
                |id, base, played| scratch.push((id.0, base, played)),
            );
        }
        for &(id, base, played) in scratch.iter() {
            let Some(at) = find(slots, id) else {
                continue;
            };
            let slot = &mut slots[at];
            let base = slot.descriptor.clamp(base);
            let played = slot.descriptor.clamp(played);
            slot.last = (base, played);
            let given = if slot.modulatable {
                (base, played)
            } else {
                (played, played)
            };
            if slot.sent.is_some() && (!on_grid || slot.sent == Some(given)) {
                continue;
            }
            slot.sent = Some(given);
            if echoes.len() < ECHOES {
                echoes.push((id, given.0));
            }
            send(Play {
                offset,
                id,
                base,
                played,
                modulatable: slot.modulatable,
            });
        }
    }

    /// Calls `report` with the id, base and played value of each parameter
    /// a clip's modulation acts on, as of the last block (R10).
    pub(crate) fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        for slot in &self.slots {
            if slot.modulated && slot.sent.is_some() {
                report(ParamId(slot.id), slot.last.0, slot.last.1);
            }
        }
    }
}
