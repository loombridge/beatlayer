//! The instruments' modulation matrix at run time (WP-8.4; spec: instrument
//! modulation). A device type declares a [`MatrixLayout`] (sources,
//! destinations and route slots); a device's [`Matrix`] reads the slots'
//! amounts, choosers and free routes at the start of each stretch, and then
//! moves a destination's value by what its sources read, per voice, frame by
//! frame.
//!
//! The laws are Beatlayer's own, in `f64`: a route's contribution is its
//! amount times its source (R3), routes into a destination add, or multiply
//! where the destination is multiplicative (R4), and the result is clamped to
//! the travel (R5). The plug-in kit's modulation engine isn't used: it holds
//! a compiled program, which can't switch a chosen slot's source or
//! destination on a frame, and it works in `f32`, which Drift's render
//! can't keep.

use std::cell::Cell;
use std::sync::Arc;

use bl_model::ParamId;
use bl_model::matrix::FreeRoutes;
pub use bl_model::matrix::{
    DestDef, Kind, MAX_SLOTS, MatrixLayout, ModRoute, Pick, Range, Rate, Scope, SlotDef, SlotKind,
    SourceDef,
};
use lspk_voice::MAX_VOICES;

use crate::framework::ParamBank;

/// Spec: instrument modulation R15. A matrix Time control's factor on the
/// rate of every envelope and LFO the matrix reads: 2^(−2t) at Time t from
/// −1 to 1, negative being faster. Beatlayer's own law.
#[must_use]
pub fn time_factor(time: f64) -> f64 {
    (-2.0 * time.clamp(-1.0, 1.0)).exp2()
}

/// One route of a stretch, resolved from its slot.
#[derive(Clone, Copy)]
struct Route {
    source: usize,
    dest: usize,
    /// −1 to 1, after the matrix's Amount.
    amount: f64,
}

/// What a source reads for a voice at a frame of the stretch: its
/// place in the layout's sources, the voice, and the frame.
pub trait Read {
    /// The source's value, in its own range.
    fn read(&self, source: usize, voice: usize, frame: usize) -> f64;
}

impl<F: Fn(usize, usize, usize) -> f64> Read for F {
    fn read(&self, source: usize, voice: usize, frame: usize) -> f64 {
        self(source, voice, frame)
    }
}

/// A device's matrix: the type's layout, the free routes it holds, and the
/// routes of the stretch being played.
pub struct Matrix {
    layout: Arc<MatrixLayout>,
    free: FreeRoutes,
    routes: Vec<Route>,
    amount: f64,
    /// A control-rate destination's worked-out movement for each voice, and
    /// whether it holds one: the movement is held between grid points (R7).
    held: Vec<Cell<(f64, bool)>>,
    oldest: Option<usize>,
}

impl Matrix {
    /// A matrix for `layout` with no free routes. Allocates: make it with
    /// the device (R13).
    #[must_use]
    pub fn new(layout: Arc<MatrixLayout>) -> Self {
        let dests = layout.dests.len();
        Self {
            layout,
            free: FreeRoutes::default(),
            routes: Vec::with_capacity(MAX_SLOTS),
            amount: 1.0,
            held: vec![Cell::new((0.0, false)); dests * MAX_VOICES],
            oldest: None,
        }
    }

    /// A matrix holding `device`'s free routes.
    #[must_use]
    pub fn of(layout: Arc<MatrixLayout>, device: &bl_model::Device) -> Self {
        let mut matrix = Self::new(layout);
        matrix.free = matrix.layout.free_routes(device);
        matrix
    }

    /// The layout.
    #[must_use]
    pub fn layout(&self) -> &MatrixLayout {
        &self.layout
    }

    /// Takes the free routes a change sent (R10): on the audio thread, at a
    /// block's start, with nothing allocated.
    pub fn set_free(&mut self, free: FreeRoutes) {
        self.free = free;
    }

    /// The matrix's Amount, 0 to 1 (R15): every route's amount is scaled by
    /// it. A device without the control leaves it at 1.
    pub fn set_amount(&mut self, amount: f64) {
        self.amount = amount.clamp(0.0, 1.0);
    }

    /// Reads the slots for the stretch: each slot's source and destination
    /// from its type, its choosers or its free route, and its amount. A slot
    /// that's Off, empty or at 0 % has no route (R2).
    pub fn begin(&mut self, params: &ParamBank) {
        self.routes.clear();
        let value = |id: ParamId| params.index(id).map_or(0.0, |at| params.value(at));
        for (slot, def) in self.layout.slots.iter().enumerate() {
            let pick = |pick: Pick, free: u8, len: usize| {
                match pick {
                    Pick::Fixed(n) => Some(usize::from(n)),
                    Pick::Chosen(id) => (value(id) as usize).checked_sub(1),
                    Pick::Free => (free != u8::MAX).then_some(usize::from(free)),
                }
                .filter(|&n| n < len)
            };
            let [free_source, free_dest] = self.free.0[slot];
            let (Some(source), Some(dest)) = (
                pick(def.source, free_source, self.layout.sources.len()),
                pick(def.dest, free_dest, self.layout.dests.len()),
            ) else {
                continue;
            };
            let amount = (value(def.amount) / 100.0).clamp(-1.0, 1.0) * self.amount;
            if amount != 0.0 {
                self.routes.push(Route {
                    source,
                    dest,
                    amount,
                });
            }
        }
    }

    /// Whether any route reaches `dest`.
    #[must_use]
    pub fn has(&self, dest: usize) -> bool {
        self.routes.iter().any(|route| route.dest == dest)
    }

    /// Whether any route reads `source`, so its values are needed.
    #[must_use]
    pub fn uses(&self, source: usize) -> bool {
        self.routes.iter().any(|route| route.source == source)
    }

    /// The voice a global destination reads per-voice sources from (R6): the
    /// oldest voice still sounding, or none.
    pub fn set_oldest(&mut self, oldest: Option<usize>) {
        if self.oldest != oldest {
            self.oldest = oldest;
            for (dest, def) in self.layout.dests.iter().enumerate() {
                if def.scope == Scope::Global {
                    self.held[dest * MAX_VOICES].set((0.0, false));
                }
            }
        }
    }

    /// `voice` starts a note: its control-rate movements are worked out at
    /// once, on the frame of its note and not at the next grid point.
    pub fn start(&mut self, voice: usize) {
        for dest in 0..self.layout.dests.len() {
            self.held[dest * MAX_VOICES + voice].set((0.0, false));
        }
    }

    /// `base` with every route into `dest` added in turn, as a travel
    /// position and not yet clamped: voice `voice`'s at `frame` (R3). The
    /// order is the slots'.
    pub fn add(&self, dest: usize, base: f64, voice: usize, frame: usize, read: &impl Read) -> f64 {
        let mut value = base;
        for route in self.routes.iter().filter(|route| route.dest == dest) {
            value += route.amount * read.read(route.source, voice, frame);
        }
        value
    }

    /// The factor every route into `dest` gives (R4): 1 − a × (1 − s) for a
    /// positive amount a and 1 + a × s for a negative one, s being the
    /// source's 0-to-1 reading, never below 0, multiplied.
    fn factor(&self, dest: usize, voice: usize, frame: usize, read: &impl Read) -> f64 {
        let mut factor = 1.0;
        for route in self.routes.iter().filter(|route| route.dest == dest) {
            let x = read.read(route.source, voice, frame);
            let s = match self.layout.sources[route.source].range {
                Range::Unipolar => x,
                Range::Bipolar => (x + 1.0) / 2.0,
            };
            let a = route.amount;
            factor *= if a >= 0.0 {
                1.0 - a * (1.0 - s)
            } else {
                1.0 + a * s
            }
            .max(0.0);
        }
        factor
    }

    /// Voice `voice`'s travel position for `dest` at `frame`, given the
    /// parameter's own `base` position (the value it plays, R5), after every
    /// route and clamped to the travel (R3 to R7). `due` says the frame is
    /// on the control-rate grid ([`State::control_due`](super::State::control_due)).
    /// A global destination reads per-voice sources from the oldest voice
    /// ([`set_oldest`](Self::set_oldest)); `voice` then only names the
    /// voice whose value is wanted.
    pub fn position(
        &self,
        dest: usize,
        base: f64,
        voice: usize,
        frame: usize,
        due: bool,
        read: &impl Read,
    ) -> f64 {
        let def = self.layout.dests[dest];
        let from = if def.scope == Scope::Global {
            match self.oldest {
                Some(oldest) => oldest,
                None => return base,
            }
        } else {
            voice
        };
        let multiplicative = def.kind == Kind::Multiplicative;
        match def.rate {
            // Worked out every frame, from the base: the order Drift's routes
            // have always been summed in.
            Rate::Audio if !multiplicative => {
                self.add(dest, base, from, frame, read).clamp(0.0, 1.0)
            }
            Rate::Audio => (base * self.factor(dest, from, frame, read)).clamp(0.0, 1.0),
            Rate::Control => {
                let at = dest * MAX_VOICES + if def.scope == Scope::Global { 0 } else { voice };
                if due || !self.held[at].get().1 {
                    let moved = if multiplicative {
                        self.factor(dest, from, frame, read)
                    } else {
                        self.add(dest, 0.0, from, frame, read)
                    };
                    self.held[at].set((moved, true));
                }
                let moved = self.held[at].get().0;
                if multiplicative {
                    base * moved
                } else {
                    base + moved
                }
                .clamp(0.0, 1.0)
            }
        }
    }
}
