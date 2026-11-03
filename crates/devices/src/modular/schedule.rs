//! A patch compiled into a schedule (WP-9.36; the modular device spec, R3 to
//! R5, R7): the blocks in an order where each follows the blocks it reads,
//! with everything they need made off the audio thread, so running it
//! allocates nothing.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use bl_model::modular::{self, BlockId, MAX_VOICES, Patch, PortType};

use super::blocks::{Frame, MAX_IN, MAX_OUT, MAX_SET, State};

/// Where an input reads from.
#[derive(Clone, Copy)]
pub(super) enum Src {
    /// Nothing is connected: the block uses its own setting.
    Open,
    /// Output slot `slot`, made by a block that runs per voice or not.
    Slot { slot: usize, voice: bool },
}

/// A setting that is a patch parameter: the parameter at `index` in the
/// device's bank, whose value runs across the setting's range.
#[derive(Clone, Copy)]
pub(super) struct Exposed {
    pub(super) index: usize,
    pub(super) min: f64,
    pub(super) max: f64,
    pub(super) choice: bool,
}

/// One block of the schedule.
pub(super) struct Step {
    pub(super) id: BlockId,
    pub(super) kind: String,
    /// Whether it runs once per voice (R5).
    pub(super) voice: bool,
    pub(super) ins: [Src; MAX_IN],
    /// Its first output slot, and how many outputs it has.
    pub(super) out: usize,
    pub(super) outs: usize,
    /// Whether `tick` makes its outputs (the blocks that take the device's
    /// input or notes in have theirs set from outside).
    pub(super) computed: bool,
    /// Its settings as the patch has them, and as they stand this frame.
    pub(super) base: [f64; MAX_SET],
    pub(super) now: [f64; MAX_SET],
    pub(super) exposed: Vec<(usize, Exposed)>,
    /// One state per voice lane, or one.
    pub(super) states: Vec<State>,
}

/// A patch ready to run.
pub(super) struct Schedule {
    pub(super) steps: Vec<Step>,
    /// The voice lanes: how many voices the schedule can play; 1 with none.
    pub(super) lanes: usize,
    /// Whether a cable joins a Note In to a Note Out (a MIDI effect passes
    /// its notes on).
    pub(super) notes_through: bool,
    /// The first slots of the Note In blocks, and which step is each.
    pub(super) note_ins: Vec<usize>,
    /// The Audio In and Sidechain In blocks: their steps.
    pub(super) audio_ins: Vec<usize>,
    /// The Audio Out blocks: their steps.
    pub(super) audio_outs: Vec<usize>,
    /// The Delay blocks: their steps, whose input is written last.
    pub(super) delays: Vec<usize>,
    /// The steps with a setting that is a patch parameter.
    pub(super) exposed: Vec<usize>,
    /// Every output slot's value, a run of `lanes` per slot.
    pub(super) values: Vec<f32>,
}

impl Schedule {
    /// Compiles `patch` for `rate` and `voices` lanes (R4). Blocks Beatlayer
    /// doesn't know make nothing (R10). Allocates: not for the audio
    /// thread.
    pub(super) fn build(patch: &Patch, rate: f64, voices: usize, instrument: bool) -> Self {
        let known: Vec<_> = patch.blocks.iter().filter(|b| b.spec().is_some()).collect();
        // The cables that decide order: a Delay's audio input is written
        // after the frame, so it decides nothing (R3).
        let decides = |c: &modular::Cable| {
            c.to_port != "in" || patch.block(c.to).is_none_or(|b| b.kind != "delay")
        };
        let mut waiting: BTreeMap<BlockId, usize> = known.iter().map(|b| (b.id, 0)).collect();
        for c in patch.cables.iter().filter(|c| decides(c)) {
            if waiting.contains_key(&c.from)
                && let Some(n) = waiting.get_mut(&c.to)
            {
                *n += 1;
            }
        }
        let mut order = Vec::with_capacity(known.len());
        let mut pending: Vec<_> = known.iter().map(|b| b.id).collect();
        while !pending.is_empty() {
            let at = pending.iter().position(|id| waiting[id] == 0).unwrap_or(0);
            let id = pending.remove(at);
            for c in patch.cables.iter().filter(|c| c.from == id && decides(c)) {
                if let Some(n) = waiting.get_mut(&c.to) {
                    *n = n.saturating_sub(1);
                }
            }
            order.push(id);
        }
        // Per voice: what depends on a Note In, but not a block that ends
        // the patch (R5).
        let mut per_voice: Vec<BlockId> = Vec::new();
        if instrument {
            let mut front: Vec<BlockId> = known
                .iter()
                .filter(|b| b.kind == "note_in")
                .map(|b| b.id)
                .collect();
            while let Some(id) = front.pop() {
                if per_voice.contains(&id) {
                    continue;
                }
                per_voice.push(id);
                front.extend(
                    patch
                        .cables
                        .iter()
                        .filter(|c| c.from == id)
                        .map(|c| c.to)
                        .filter(|to| {
                            patch.block(*to).is_some_and(|b| {
                                !matches!(b.kind.as_str(), "audio_out" | "note_out")
                            })
                        }),
                );
            }
        }
        let lanes = if per_voice.is_empty() {
            1
        } else {
            voices.clamp(1, MAX_VOICES as usize)
        };
        // Output slots, in step order.
        let mut first = BTreeMap::new();
        let mut slots = 0;
        for id in &order {
            first.insert(*id, slots);
            slots += patch
                .block(*id)
                .and_then(|b| b.spec())
                .map_or(0, |s| s.outputs.len());
        }
        let mut schedule = Self {
            steps: Vec::with_capacity(order.len()),
            lanes,
            notes_through: false,
            note_ins: Vec::new(),
            audio_ins: Vec::new(),
            audio_outs: Vec::new(),
            delays: Vec::new(),
            exposed: Vec::new(),
            values: vec![0.0; slots * lanes],
        };
        for (n, id) in order.iter().enumerate() {
            let Some(block) = patch.block(*id) else {
                continue;
            };
            let Some(spec) = block.spec() else { continue };
            let voice = per_voice.contains(id);
            let mut ins = [Src::Open; MAX_IN];
            for c in patch.cables.iter().filter(|c| c.to == *id) {
                let Some(at) = spec.inputs.iter().position(|p| p.id == c.to_port) else {
                    continue;
                };
                let Some(from) = patch.block(c.from).and_then(|b| b.spec().map(|s| (b, s))) else {
                    continue;
                };
                let Some(port) = from.1.outputs.iter().position(|p| p.id == c.from_port) else {
                    continue;
                };
                if spec.inputs[at].ty == PortType::Notes {
                    schedule.notes_through |= block.kind == "note_out" && from.0.kind == "note_in";
                } else if at < MAX_IN {
                    ins[at] = Src::Slot {
                        slot: first[&c.from] + port,
                        voice: per_voice.contains(&c.from),
                    };
                }
            }
            let mut base = [0.0; MAX_SET];
            let mut exposed = Vec::new();
            for (k, setting) in spec.settings.iter().enumerate().take(MAX_SET) {
                base[k] = block.setting(setting.id).unwrap_or(setting.default);
                if let Some(param) = patch.param_of(*id, setting.id) {
                    exposed.push((
                        k,
                        Exposed {
                            index: usize::from(param.slot) - 1,
                            min: setting.min,
                            max: setting.max,
                            choice: setting.is_choice(),
                        },
                    ));
                }
            }
            match block.kind.as_str() {
                "note_in" => schedule.note_ins.push(first[id]),
                "audio_in" | "sidechain_in" => schedule.audio_ins.push(n),
                "audio_out" => schedule.audio_outs.push(n),
                "delay" => schedule.delays.push(n),
                _ => {}
            }
            if !exposed.is_empty() {
                schedule.exposed.push(n);
            }
            let count = if voice { lanes } else { 1 };
            schedule.steps.push(Step {
                id: *id,
                kind: block.kind.clone(),
                voice,
                ins,
                out: first[id],
                outs: spec.outputs.len(),
                computed: !matches!(block.kind.as_str(), "note_in" | "audio_in" | "sidechain_in"),
                base,
                now: base,
                exposed,
                states: (0..count)
                    .map(|lane| {
                        State::new(
                            &block.kind,
                            rate,
                            id.0.wrapping_mul(2_654_435_761) ^ lane as u32,
                        )
                    })
                    .collect(),
            });
        }
        schedule
    }

    /// Takes the state of every block `old` and `self` both hold, of the
    /// same type, so a patch edit carries phases, filter memories, envelope
    /// stages and delay lines over (R4). Nothing allocates: states trade
    /// places, and `old` ends up holding the fresh ones.
    pub(super) fn take_state_from(&mut self, old: &mut Self) {
        for step in &mut self.steps {
            if let Some(was) = old
                .steps
                .iter_mut()
                .find(|s| s.id == step.id && s.kind == step.kind)
            {
                for (new, old) in step.states.iter_mut().zip(was.states.iter_mut()) {
                    std::mem::swap(new, old);
                }
                // A Note In's pitch, velocity and gate belong to the voices
                // playing, so they carry over too.
                if step.kind == "note_in" {
                    for k in 0..step.outs {
                        for lane in 0..self.lanes.min(old.lanes) {
                            self.values[at(step.out + k, self.lanes, lane)] =
                                old.values[at(was.out + k, old.lanes, lane)];
                        }
                    }
                }
            }
        }
    }

    /// One frame of values for the settings that are patch parameters:
    /// `value(index)` is the parameter's value at this frame.
    pub(super) fn refresh(&mut self, value: impl Fn(usize) -> f64) {
        for &n in &self.exposed {
            let step = &mut self.steps[n];
            step.now = step.base;
            for &(k, e) in &step.exposed {
                let v = e.min + (e.max - e.min) * value(e.index).clamp(0.0, 1.0);
                step.now[k] = if e.choice { v.round() } else { v };
            }
        }
    }
}

/// A schedule on its way to the audio thread, built off it. The audio thread
/// takes it once and leaves the schedule it replaces in its place, so the
/// one that was running is freed with the handoff, by the engine's drop
/// thread, not by the audio thread.
pub(super) struct Handoff {
    cell: AtomicPtr<Schedule>,
    spent: AtomicBool,
}

impl Handoff {
    pub(super) fn new(schedule: Schedule) -> Self {
        Self {
            cell: AtomicPtr::new(Box::into_raw(Box::new(schedule))),
            spent: AtomicBool::new(false),
        }
    }

    /// The schedule, the first time only.
    pub(super) fn take(&self) -> Option<Box<Schedule>> {
        if self.spent.swap(true, Ordering::AcqRel) {
            return None;
        }
        let pointer = self.cell.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: the pointer came from `Box::into_raw` in `new` or `give`,
        // and `spent` lets one caller past, once, so nothing else owns it.
        (!pointer.is_null()).then(|| unsafe { Box::from_raw(pointer) })
    }

    /// Leaves `schedule` to be freed with the handoff.
    pub(super) fn give(&self, schedule: Box<Schedule>) {
        self.cell.store(Box::into_raw(schedule), Ordering::Release);
    }
}

impl Drop for Handoff {
    fn drop(&mut self) {
        let pointer = *self.cell.get_mut();
        if !pointer.is_null() {
            // SAFETY: as in `take`: `drop` has the only access, and the
            // pointer is a leaked box nobody took.
            drop(unsafe { Box::from_raw(pointer) });
        }
    }
}

/// Voice lane `lane`'s slot `slot` in a table of `lanes` lanes.
#[inline]
pub(super) fn at(slot: usize, lanes: usize, lane: usize) -> usize {
    slot * lanes + lane
}

/// What one frame needs besides the schedule.
pub(super) struct Run<'a> {
    pub(super) frame: Frame,
    /// Which lanes are playing, and the gain each is heard at.
    pub(super) active: u32,
    pub(super) gain: &'a [f32; MAX_VOICES as usize],
    /// How loud each lane was to the rest of the patch this frame.
    pub(super) heard: &'a mut [f32; MAX_VOICES as usize],
}

/// The value of `src` for a block that runs per voice (on `lane`) or once.
#[inline]
fn read(
    values: &[f32],
    lanes: usize,
    run: &mut Run<'_>,
    src: Src,
    per_voice: bool,
    lane: usize,
) -> Option<f32> {
    let Src::Slot { slot, voice } = src else {
        return None;
    };
    Some(match (per_voice, voice) {
        (true, true) => values[at(slot, lanes, lane)],
        (_, false) => values[at(slot, lanes, 0)],
        (false, true) => {
            // A block that runs once hears every voice, summed.
            let mut sum = 0.0;
            let mut bits = run.active;
            while bits != 0 {
                let v = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let x = values[at(slot, lanes, v)];
                run.heard[v] = run.heard[v].max(x.abs());
                sum += x * run.gain[v];
            }
            sum
        }
    })
}

impl Schedule {
    /// Works out every block for one frame, per voice where it runs per
    /// voice, then writes what the Delay blocks take in; returns the Audio
    /// Out blocks' left and right sum.
    pub(super) fn frame(&mut self, run: &mut Run<'_>) -> [f32; 2] {
        let lanes = self.lanes;
        let mut out = [0.0f32; MAX_OUT];
        for step in &mut self.steps {
            let mut bits = if step.voice { run.active } else { 1 };
            while bits != 0 {
                let lane = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if !step.computed {
                    continue;
                }
                let mut ins = [None; MAX_IN];
                for (k, src) in step.ins.iter().enumerate() {
                    ins[k] = read(&self.values, lanes, run, *src, step.voice, lane);
                }
                out.fill(0.0);
                step.states[if step.voice { lane } else { 0 }]
                    .tick(run.frame, ins, &step.now, &mut out);
                for (k, v) in out.iter().take(step.outs).enumerate() {
                    self.values[at(step.out + k, lanes, lane)] = *v;
                }
            }
        }
        // The Delay blocks take their input in, each voice's own where they
        // run per voice.
        for &n in &self.delays {
            let step = &mut self.steps[n];
            let mut bits = if step.voice { run.active } else { 1 };
            while bits != 0 {
                let lane = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let x = read(&self.values, lanes, run, step.ins[0], step.voice, lane);
                step.states[if step.voice { lane } else { 0 }].write(x.unwrap_or(0.0));
            }
        }
        let mut sum = [0.0f32; 2];
        for &n in &self.audio_outs {
            let ins = self.steps[n].ins;
            for (side, total) in sum.iter_mut().enumerate() {
                *total += read(&self.values, lanes, run, ins[side], false, 0).unwrap_or(0.0);
            }
        }
        sum
    }

    /// Latency along the longest path (R12): none of the first blocks
    /// delays the signal it passes.
    pub(super) fn latency(&self) -> u32 {
        0
    }
}
