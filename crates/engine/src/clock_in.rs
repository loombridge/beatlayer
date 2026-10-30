//! MIDI clock in (`docs/specs/midi-clock.md`, R14 to R24, R26): the Timing
//! Clock, Start, Continue, Stop and Song Position Pointer an input with Sync
//! on sends, followed as the transport spec's R24 and R25 follow an outside
//! tempo.
//!
//! [`MidiIn::take`](crate::midi::MidiIn) hands every sync message it takes
//! off the input queue to [`ClockIn::push`], with its arrival time and the
//! input's Sync Delay added; once per block [`ClockIn::block`] reads them in
//! the order they came. The clocks' times are fitted against their count by
//! a second-order loop ([`Fit`]), which gives the tempo and, with the count
//! since the last Start, Continue or Song Position Pointer, the song
//! position the clock stands at. The follower plays on the fitted tempo, a
//! little faster or slower to close a difference in position, and jumps
//! when the position is more than a sixteenth note away.
//!
//! An input set to MIDI Timecode sends its quarter frames and full frames
//! the same way (`docs/specs/midi-timecode.md`, R2 to R10): they follow the
//! song's position and start and stop it, and never its tempo.
//!
//! Times are nanoseconds after the block clock's epoch, as
//! [`MidiIn`](crate::midi::MidiIn) keeps them. Nothing here allocates, locks
//! or blocks: the pending messages and the inputs are in storage made with
//! the engine.

use bl_core::midi::{MidiMessage, PortId, SyncInput};
use bl_core::time::{TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_core::timecode::{FrameRate, Timecode};

use crate::follower::{Ask, Follow};
use crate::message::TransportCommand;
use crate::timecode_in::{Assembler, Piece, full_frame, song_ticks};

/// The most input ports with Sync on the engine follows; the list is made
/// with the engine, and any more are ignored and counted.
pub const MAX_SYNC_INPUTS: usize = 32;

/// The most sync messages held between two blocks. At 999 BPM a block of
/// 4,096 frames at 44.1 kHz holds 38 clocks; more than this are dropped and
/// counted.
const PENDING: usize = 512;

/// Ticks in one Timing Clock: a 24th of a quarter note.
const CLOCK_TICKS: f64 = (TICKS_PER_QUARTER / 24) as f64;
/// Ticks in a sixteenth note, the Song Position Pointer's unit and how far
/// apart the positions may be before the song jumps (R19).
const SIXTEENTH_TICKS: f64 = 6.0 * CLOCK_TICKS;
/// A difference in position under this, in ticks, is left alone: rounding.
const DEADBAND: f64 = CLOCK_TICKS / 40.0;
/// The most the speed changes to close a difference in position (R19).
const MAX_CORRECTION: f64 = 0.02;
/// The song position difference, in ticks, that would ask for the greatest
/// correction: two quarter notes.
const CORRECTION_SPAN: f64 = 2.0 * TICKS_PER_QUARTER as f64;
/// How long without a usable clock before the clock counts as silent (R21).
const SILENCE_NS: f64 = 500e6;
/// How long without a quarter frame before the timecode counts as stopped
/// (midi-timecode R8).
const TIMECODE_SILENCE_NS: f64 = 100e6;
/// The fastest and slowest clock intervals the fit takes, in nanoseconds:
/// 999 and 20 BPM.
const MIN_PERIOD: f64 = 60e9 / (24.0 * 999.0);
const MAX_PERIOD: f64 = 60e9 / (24.0 * 20.0);

/// How far the loop's bandwidth is from the clock rate: a fraction of the
/// quarter-note rate (R18).
const BANDWIDTH: f64 = 0.07;
/// How many clocks the prior tempo counts for when a clock begins again
/// after silence.
const PRIOR_WEIGHT: u32 = 12;
/// The fewest clocks the loop's estimate stands on while it listens again.
const LISTENING_WEIGHT: u32 = 8;
/// How many clocks the average of the errors takes in (a fraction of 1/8 of
/// each) and how many the spread of the errors does (1/32).
const BIAS_RATE: f64 = 1.0 / 8.0;
const SPREAD_RATE: f64 = 1.0 / 32.0;
/// How many standard deviations of the average the errors may sit away from
/// zero before the tempo is taken to be changing.
const BIAS_LIMIT: f64 = 3.0;

/// A second-order loop over the clocks' arrival times against their count
/// (the engine's block clock is the same loop, over callbacks): the time of
/// the clock it last took, and the interval between clocks.
///
/// It starts as a growing-memory fit, whose gains give every clock so far
/// the same say, so a few jittered intervals don't set the tempo; once the
/// gains fall to the loop's own, they stay there. Errors that lean one way
/// for long enough, more than jitter would, mean the tempo is changing: the
/// loop's memory is cut, and it grows again as the errors settle.
#[derive(Clone, Copy, Debug)]
struct Fit {
    /// How many clocks the estimate stands on.
    weight: u32,
    /// The filtered time of the last clock.
    t: f64,
    /// The filtered interval between clocks.
    period: f64,
    /// A running average of the errors, and of their squares.
    bias: f64,
    spread: f64,
}

impl Fit {
    const fn new() -> Self {
        Self {
            weight: 0,
            t: 0.0,
            period: 0.0,
            bias: 0.0,
            spread: 0.0,
        }
    }

    /// Begins at a clock at `time`, with the tempo of `period` standing for
    /// `weight` clocks.
    fn lock(&mut self, time: f64, period: f64, weight: u32) {
        *self = Self {
            weight,
            t: time,
            period,
            ..Self::new()
        };
    }

    /// Takes the next clock, at `time`.
    fn push(&mut self, time: f64) {
        let omega = core::f64::consts::TAU * BANDWIDTH / 24.0;
        let predicted = self.t + self.period;
        let error = time - predicted;
        let mut weight = self.weight + 1;
        if self.weight > LISTENING_WEIGHT {
            self.bias += (error - self.bias) * BIAS_RATE;
            // A jump the tempo makes isn't jitter: it widens the spread no
            // more than a few times jitter would.
            let squared = (error * error).min(9.0 * self.spread);
            self.spread += (squared - self.spread) * SPREAD_RATE;
            let deviation = (self.spread * BIAS_RATE / (2.0 - BIAS_RATE)).sqrt();
            if self.bias.abs() > BIAS_LIMIT * deviation {
                weight = (self.weight / 4).max(LISTENING_WEIGHT);
                self.bias = 0.0;
            }
        } else {
            self.spread = error * error;
        }
        let k = f64::from(weight);
        let (alpha, beta) = if weight <= 1 {
            (1.0, 1.0)
        } else {
            (
                2.0 * (2.0 * k - 1.0) / (k * (k + 1.0)),
                6.0 / (k * (k + 1.0)),
            )
        };
        self.t = predicted + alpha.max(core::f64::consts::SQRT_2 * omega) * error;
        self.period = (self.period + beta.max(omega * omega) * error).clamp(MIN_PERIOD, MAX_PERIOD);
        self.weight = weight;
    }

    /// Steps over `clocks` clocks that never came, or came twice, on the
    /// loop's own tempo.
    fn coast(&mut self, clocks: f64) {
        self.t += clocks * self.period;
    }

    /// The tempo in BPM, once there is one.
    fn bpm(&self) -> Option<f64> {
        (self.weight >= 2).then(|| 60e9 / (24.0 * self.period))
    }
}

/// What a sync message says.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sync {
    Clock,
    Start,
    Continue,
    Stop,
    /// A Song Position Pointer, in sixteenth notes.
    Pointer(u32),
    /// A quarter frame's data byte.
    Quarter(u8),
    /// A full frame's hours with the rate, minutes, seconds and frames.
    Full([u8; 4]),
}

impl Sync {
    fn of(message: &MidiMessage) -> Option<Self> {
        match message.status() {
            0xF8 => Some(Self::Clock),
            0xFA => Some(Self::Start),
            0xFB => Some(Self::Continue),
            0xFC => Some(Self::Stop),
            0xF2 => Some(Self::Pointer(
                u32::from(message.data1()) | u32::from(message.data2()) << 7,
            )),
            0xF1 => Some(Self::Quarter(message.data1())),
            0xF0 => match message.bytes() {
                &[_, hours, minutes, seconds, frames] => {
                    Some(Self::Full([hours, minutes, seconds, frames]))
                }
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    port: PortId,
    time: f64,
    what: Sync,
}

/// A start or continue from the host waiting for its first clock.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Begin {
    Start,
    Continue,
}

/// What the engine reports of the clock it follows, for `sync.state` and the
/// upper sync lamp.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SyncInState {
    /// Whether the song follows the clock now: EXT is on and a tempo was
    /// found.
    pub following: bool,
    /// Whether usable clocks arrive now: one came in the last 500 ms.
    pub receiving: bool,
    /// The input that leads, while it does.
    pub lead: Option<PortId>,
    /// The smoothed tempo of the clock in BPM, once there is one.
    pub tempo: Option<f64>,
    /// Quarter notes of clock received: the sync lamp lights for 100 ms
    /// each time it grows.
    pub quarters: u64,
    /// Timing Clocks that arrived from the leading input.
    pub clocks: u64,
    /// Timing Clocks set aside as glitches.
    pub glitches: u64,
    /// Sync messages dropped because the audio thread's list was full, and
    /// inputs over [`MAX_SYNC_INPUTS`].
    pub dropped: u64,
    /// The last whole time from an input set to MIDI Timecode, if one came.
    pub timecode: Option<TimecodeState>,
    /// Whether timecode leads the song with EXT on, so the Arrangement loop
    /// doesn't wrap it (midi-timecode R9).
    pub timecode_leads: bool,
}

/// The last whole time of the timecode the engine follows (midi-timecode
/// R12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimecodeState {
    /// The time, as its message spelled it.
    pub time: Timecode,
    /// The rate it is counted at.
    pub rate: FrameRate,
    /// Whether the timecode runs: whole times arrive in order and the song
    /// follows them.
    pub running: bool,
    /// The song position the time gives, in ticks, less the offset (R4).
    pub position: Ticks,
}

/// Where the engine's block stands on the monotonic clock, as the block
/// clock estimated it: what puts a message's time on a frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// The block's start, in nanoseconds after the epoch.
    pub start: f64,
    /// Nanoseconds in one frame.
    pub ns_per_frame: f64,
    /// The frames between the engine handing the device a sound and it
    /// coming out.
    pub latency: u32,
}

/// The engine's clock follower.
pub(crate) struct ClockIn {
    pending: Vec<Pending>,
    inputs: Vec<SyncInput>,
    lead: Option<PortId>,
    fit: Fit,
    /// The tempo of the last fit, kept over silence.
    prior: Option<f64>,
    /// When the last clock came.
    last: f64,
    /// Whether `position` is the song position at the fit's time.
    anchored: bool,
    /// The song position, in ticks, at the fit's last clock.
    position: f64,
    /// Where the next clock stands, when a Start, Continue or pointer says.
    anchor: Option<f64>,
    /// A host Start or Continue waiting for the clock that begins it.
    begin: Option<(Begin, f64)>,
    /// A pointer received while stopped: where Continue goes on from.
    pointer: Option<f64>,
    /// The engine frame of the last start this follower asked for, until the
    /// block after it.
    launch: Option<u64>,
    /// The latest frame a command of this block was asked for on: a later
    /// one never runs before an earlier.
    latest: u64,
    /// Clocks since the lamp's last quarter note began.
    lamp: u32,
    /// Whether the input that leads sends timecode, not clock.
    lead_timecode: bool,
    /// The quarter frames being read, and the input they come from.
    pieces: Assembler,
    pieces_port: Option<PortId>,
    /// When the last quarter frame or full frame of the leading input came.
    timecode_last: f64,
    /// Whether whole times arrive in order and the song follows them: once
    /// the time reaches the offset.
    timecode_running: bool,
    /// Whether this block's timecode already jumped the song, so a later
    /// time in it isn't compared against a position from before the jump.
    jumped: bool,
    state: SyncInState,
}

impl ClockIn {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::with_capacity(PENDING),
            inputs: Vec::with_capacity(MAX_SYNC_INPUTS),
            lead: None,
            fit: Fit::new(),
            prior: None,
            last: f64::NEG_INFINITY,
            anchored: false,
            position: 0.0,
            anchor: None,
            begin: None,
            pointer: None,
            launch: None,
            latest: 0,
            lamp: 0,
            lead_timecode: false,
            pieces: Assembler::new(),
            pieces_port: None,
            timecode_last: f64::NEG_INFINITY,
            timecode_running: false,
            jumped: false,
            state: SyncInState::default(),
        }
    }

    /// Sets input `index` of `count` inputs with Sync on (a count of 0
    /// clears the list).
    pub(crate) fn set_input(&mut self, index: u8, count: u8, input: Option<SyncInput>) {
        let (index, count) = (usize::from(index), usize::from(count));
        if count > MAX_SYNC_INPUTS {
            self.state.dropped += 1;
            return;
        }
        self.inputs.truncate(count);
        if let Some(input) = input
            && index < count
        {
            if index < self.inputs.len() {
                self.inputs[index] = input;
            } else if index == self.inputs.len() {
                self.inputs.push(input);
            }
        }
    }

    /// Takes a message that arrived on `port` at `time`, its Sync Delay
    /// added (R14, R20). Messages that aren't sync's are ignored.
    pub(crate) fn push(&mut self, port: PortId, time: f64, message: &MidiMessage) {
        let Some(what) = Sync::of(message) else {
            return;
        };
        let input = self.inputs.iter().find(|i| i.port == port);
        // Spec: midi-timecode R2. An input follows its clock or its time
        // code, and the other kind of message is ignored.
        let timecode = matches!(what, Sync::Quarter(_) | Sync::Full(_));
        if input.is_some_and(|i| i.timecode.is_some() != timecode) || (input.is_none() && timecode)
        {
            return;
        }
        let delay = input.map_or(0.0, |i| f64::from(i.delay_ms) * 1e6);
        if self.pending.len() < PENDING {
            self.pending.push(Pending {
                port,
                time: time + delay,
                what,
            });
        } else {
            self.state.dropped += 1;
        }
    }

    /// What the engine reports.
    pub(crate) fn state(&self) -> SyncInState {
        self.state
    }

    /// The engine frame at which the moment `time` is heard, no earlier than
    /// the block's first (R20: a frame already past can't be played).
    fn frame_of(&self, timing: &Timing, ask: &Ask, time: f64) -> u64 {
        let heard = timing.start + f64::from(timing.latency) * timing.ns_per_frame;
        ask.clock + ((time - heard) / timing.ns_per_frame).round().max(0.0) as u64
    }

    /// Where the song stands, in ticks, at the moment `time`, going by the
    /// last clock's position and the tempo.
    fn position_at(&self, time: f64) -> f64 {
        self.position + CLOCK_TICKS * (time - self.fit.t) / self.fit.period
    }

    /// Reads the messages that came since the last block and answers for the
    /// block `ask` describes: with `ext` on, the tempo, and the starts,
    /// stops and jumps of the host (R17 to R24); either way it counts what
    /// came for the lamp (R16). `looped` is the Arrangement loop's start and
    /// end in ticks, while it is on.
    pub(crate) fn block(
        &mut self,
        ask: &Ask,
        timing: Option<Timing>,
        ext: bool,
        looped: Option<(f64, f64)>,
        tempo: &TempoMap,
        answer: &mut Follow,
    ) {
        let Some(timing) = timing else {
            self.pending.clear();
            return;
        };
        if !self.lead_timecode && timing.start - self.last > SILENCE_NS {
            self.lead = None;
            self.anchored = false;
        }
        let mut issued = false;
        self.latest = ask.clock;
        self.jumped = false;
        let mut pending = std::mem::take(&mut self.pending);
        for event in pending.drain(..) {
            match event.what {
                Sync::Clock => {
                    if let Some(kind) = self.clock(&event) {
                        issued |= self.begin_song(kind, 0.0, &timing, ask, answer);
                    }
                }
                Sync::Quarter(_) | Sync::Full(_) => {
                    issued |= self.timecode(&event, ext, &timing, ask, tempo, answer);
                }
                what if ext => {
                    issued |= self.message(event.time, what, &timing, ask, looped, answer);
                }
                // With EXT off, the messages only light the lamp.
                _ => self.lamp = 0,
            }
        }
        self.pending = pending;
        // Spec: midi-timecode R8. Timecode that falls silent has stopped.
        if self.lead_timecode && timing.start - self.timecode_last > TIMECODE_SILENCE_NS {
            self.timecode_stopped(ext, ask, answer);
            self.lead = None;
            self.lead_timecode = false;
        }
        let timecode = self.lead_timecode;
        let heard = timing.start + f64::from(timing.latency) * timing.ns_per_frame;
        let silent = timing.start - self.last > SILENCE_NS;
        self.state.receiving = if timecode { true } else { !silent };
        self.state.lead = self.lead;
        self.state.tempo = if timecode {
            None
        } else {
            self.fit.bpm().or(self.prior)
        };
        let launching = self.launch.is_some_and(|frame| frame >= ask.clock);
        if !launching {
            self.launch = None;
        }
        self.state.timecode_leads = ext && timecode;
        if let Some(state) = self.state.timecode.as_mut() {
            state.running = self.timecode_running;
        }
        if !ext {
            self.anchored = false;
            self.anchor = None;
            self.begin = None;
            self.launch = None;
            self.timecode_running = false;
            self.state.following = false;
            return;
        }
        if timecode {
            // Spec: midi-timecode R4. The tempo stays Beatlayer's own.
            self.state.following = self.timecode_running;
            return;
        }
        let Some(bpm) = self.state.tempo else {
            self.state.following = false;
            return;
        };
        self.state.following = true;
        if !silent {
            // Spec: transport R25. Starts wait for the next clock.
            answer.decides_starts();
            let ahead = ((heard - self.fit.t) / self.fit.period).ceil().max(1.0);
            answer.start_at(self.frame_of(&timing, ask, self.fit.t + ahead * self.fit.period));
        }
        let mut speed = 1.0;
        if !ask.playing && !launching {
            self.anchored = false;
        } else if !silent && !issued && !launching {
            if !self.anchored {
                // Spec: midi-clock R19, R21. No position of the host's: the
                // song's own is the clock's from here on.
                self.position = ask.position - CLOCK_TICKS * (heard - self.fit.t) / self.fit.period;
                self.anchored = true;
            }
            let mut target = self.position_at(heard);
            let mut error = target - ask.position;
            if let Some((start, end)) = looped {
                let length = end - start;
                if target >= end && length > 0.0 {
                    target = start + (target - start) % length;
                }
                error = target - ask.position;
                if length > 0.0 && error.abs() > length / 2.0 {
                    error -= length * error.signum();
                }
            }
            if error.abs() > SIXTEENTH_TICKS {
                answer.jump(ask.clock, Ticks(target.round() as i64));
                self.anchored = true;
            } else if error.abs() > DEADBAND {
                let over = error - DEADBAND * error.signum();
                speed += (over / CORRECTION_SPAN).clamp(-MAX_CORRECTION, MAX_CORRECTION);
            }
        }
        answer.tempo(bpm * speed);
    }

    /// Takes a clock of the leading input. Returns the host's Start or
    /// Continue it begins, if one waited.
    fn clock(&mut self, event: &Pending) -> Option<Begin> {
        // Spec: midi-clock R17. The first input to deliver usable clocks
        // leads until it falls silent.
        match self.lead {
            None => {
                self.lead = Some(event.port);
                self.lead_timecode = false;
            }
            Some(lead) if lead != event.port || self.lead_timecode => return None,
            Some(_) => {}
        }
        let time = event.time;
        self.state.clocks += 1;
        let interval = time - self.last;
        if interval > SILENCE_NS || interval < 0.0 {
            // Spec: midi-clock R21. Clocks begin again, keeping the tempo
            // they had until the loop has heard a few.
            match self.prior {
                Some(bpm) => self.fit.lock(time, 60e9 / (24.0 * bpm), PRIOR_WEIGHT),
                None => self.fit.lock(time, MAX_PERIOD, 0),
            }
            self.anchored = false;
            self.lamp = 0;
        } else {
            let ratio = interval / self.fit.period;
            if self.fit.weight >= 2 && !(0.5..=1.5).contains(&ratio) {
                // Spec: midi-clock R18. A glitch is counted, not fitted, and
                // the count goes on by as many 24ths as its time gives.
                self.state.glitches += 1;
                let missed = ratio.round();
                self.fit.coast(missed);
                if self.anchored {
                    self.position += missed * CLOCK_TICKS;
                }
                self.last = time;
                return None;
            }
            if !(MIN_PERIOD..=MAX_PERIOD).contains(&interval) && self.fit.weight < 2 {
                self.state.glitches += 1;
                return None;
            }
            self.fit.push(time);
            if self.anchored {
                self.position += CLOCK_TICKS;
            }
        }
        self.last = time;
        self.prior = self.fit.bpm().or(self.prior);
        self.lamp += 1;
        if self.lamp.is_multiple_of(24) {
            self.state.quarters += 1;
        }
        if let Some(anchor) = self.anchor.take() {
            self.position = anchor;
            self.anchored = true;
            self.lamp = 0;
        }
        self.begin
            .take_if(|&mut (_, at)| time >= at)
            .map(|(kind, _)| kind)
    }

    /// Takes a Start, Continue, Stop or Song Position Pointer with EXT on
    /// (R22). Returns whether it asked the transport to do anything.
    fn message(
        &mut self,
        time: f64,
        what: Sync,
        timing: &Timing,
        ask: &Ask,
        looped: Option<(f64, f64)>,
        answer: &mut Follow,
    ) -> bool {
        match what {
            Sync::Clock | Sync::Quarter(_) | Sync::Full(_) => false,
            Sync::Start | Sync::Continue => {
                self.lamp = 0;
                let kind = if what == Sync::Start {
                    self.pointer = None;
                    Begin::Start
                } else {
                    Begin::Continue
                };
                if self.fit.weight >= 2 && time - self.last < SILENCE_NS {
                    // Clocks run already: the first one after the message is
                    // the next the fit expects.
                    let ahead = ((time - self.fit.t) / self.fit.period).ceil().max(1.0);
                    self.begin_song(kind, ahead, timing, ask, answer)
                } else {
                    self.begin = Some((kind, time));
                    false
                }
            }
            Sync::Stop => {
                self.begin = None;
                self.anchor = None;
                self.anchored = false;
                if ask.playing || self.launch.is_some() {
                    answer.command(self.latest, TransportCommand::Stop);
                    return true;
                }
                false
            }
            Sync::Pointer(sixteenths) => {
                self.lamp = 0;
                let mut at = f64::from(sixteenths) * SIXTEENTH_TICKS;
                if let Some((start, end)) = looped
                    && at >= end
                    && end > start
                {
                    at = start + (at - start) % (end - start);
                }
                if ask.playing || self.launch.is_some() || self.begin.is_some() {
                    self.anchor = Some(at);
                    self.anchored = false;
                    false
                } else {
                    self.pointer = Some(at);
                    answer.jump(self.latest, Ticks(at.round() as i64))
                }
            }
        }
    }

    /// Takes a quarter frame or a full frame from an input set to MIDI
    /// Timecode (midi-timecode R2 to R10). The first input to deliver a
    /// whole time leads until its timecode falls silent. With EXT on, the
    /// first whole time in order starts the song (R6), later ones move it
    /// when it is more than a frame away (R7), a full frame moves it at once
    /// (R5, R8), and pieces in reverse order stop it (R8). Returns whether
    /// it asked the transport to do anything.
    fn timecode(
        &mut self,
        event: &Pending,
        ext: bool,
        timing: &Timing,
        ask: &Ask,
        tempo: &TempoMap,
        answer: &mut Follow,
    ) -> bool {
        let Some(settings) = self
            .inputs
            .iter()
            .find(|i| i.port == event.port)
            .and_then(|i| i.timecode)
        else {
            return false;
        };
        if self.lead.is_some_and(|lead| lead != event.port) {
            return false;
        }
        if self.pieces_port != Some(event.port) {
            self.pieces.reset();
            self.pieces_port = Some(event.port);
        }
        let leading = self.lead == Some(event.port);
        let (time, carried, quarter) = match event.what {
            Sync::Quarter(data) => match self.pieces.push(data) {
                Piece::Whole(time, rate) => (time, rate, true),
                Piece::Part => {
                    if leading {
                        self.timecode_last = event.time;
                    }
                    return false;
                }
                Piece::Reverse => {
                    if leading {
                        self.timecode_last = event.time;
                        return self.timecode_stopped(ext, ask, answer);
                    }
                    return false;
                }
            },
            Sync::Full(data) => {
                let (time, rate) = full_frame(data);
                (time, rate, false)
            }
            _ => return false,
        };
        let rate = settings.rate.of(carried);
        if !time.fits(rate) {
            return false;
        }
        self.lead = Some(event.port);
        self.lead_timecode = true;
        self.timecode_last = event.time;
        // Spec: midi-timecode R10. Each whole time lights the lamp.
        self.state.quarters += 1;
        // Spec: midi-timecode R6. The pieces of a time took two frames to
        // send: the time is that much further on when the eighth arrives.
        let seconds = time.seconds(rate) + if quarter { 2.0 * rate.frame() } else { 0.0 };
        let offset = settings.offset.seconds(rate);
        self.state.timecode = Some(TimecodeState {
            time,
            rate,
            running: self.timecode_running,
            position: Ticks(song_ticks(tempo, seconds, offset).round() as i64),
        });
        if !ext {
            return false;
        }
        // The song starts or moves on the frame this time is heard on (R20),
        // further along by what that frame is after it.
        let playing = ask.playing || self.launch.is_some();
        let frame = self.frame_of(timing, ask, event.time).max(self.latest);
        let heard = timing.start
            + (f64::from(timing.latency) + (frame - ask.clock) as f64) * timing.ns_per_frame;
        let to =
            Ticks(song_ticks(tempo, seconds + (heard - event.time) / 1e9, offset).round() as i64);
        if !quarter {
            // While stopped, a locate goes to the time itself.
            let at = Ticks(song_ticks(tempo, seconds, offset).round() as i64);
            return self.timecode_jump(frame, if playing { to } else { at }, answer);
        }
        if !self.timecode_running {
            // The song waits for the time the Arrangement starts at.
            if seconds < offset {
                return false;
            }
            self.timecode_running = true;
            if !playing {
                answer.command(frame, TransportCommand::Locate(to));
                answer.command(frame, TransportCommand::PlayFromInsertMarker);
                self.latest = frame;
                self.launch = Some(frame);
                return true;
            }
        }
        if ask.playing && self.launch.is_none() && !self.jumped {
            // Spec: midi-timecode R7. Where the song stood when the time
            // was sent, against where the time puts it.
            let start = timing.start + f64::from(timing.latency) * timing.ns_per_frame;
            let own = tempo.ticks_to_seconds_exact(ask.position) + (event.time - start) / 1e9;
            if ((seconds - offset).max(0.0) - own).abs() > rate.frame() {
                return self.timecode_jump(frame, to, answer);
            }
        }
        false
    }

    /// Moves the song to `to` on `frame`, as a locate does (R5, R7).
    fn timecode_jump(&mut self, frame: u64, to: Ticks, answer: &mut Follow) -> bool {
        self.latest = frame;
        self.jumped = true;
        answer.jump(frame, to)
    }

    /// The timecode has stopped (R8): the song stops if it was following.
    fn timecode_stopped(&mut self, ext: bool, ask: &Ask, answer: &mut Follow) -> bool {
        let was = std::mem::take(&mut self.timecode_running);
        if was && ext && (ask.playing || self.launch.is_some()) {
            return answer.command(self.latest, TransportCommand::Stop);
        }
        false
    }

    /// Starts the song the way the host's Start or Continue says, on the
    /// clock that begins it: `ahead` clocks after the fit's last (R20, R22).
    /// The clock stands at the position the message names, and the song
    /// starts on the frame it is heard on, or on the block's first, further
    /// along by what it is late. Returns whether it asked for anything.
    fn begin_song(
        &mut self,
        kind: Begin,
        ahead: f64,
        timing: &Timing,
        ask: &Ask,
        answer: &mut Follow,
    ) -> bool {
        let anchor = match kind {
            Begin::Start => 0.0,
            Begin::Continue => self.pointer.take().unwrap_or(ask.position),
        };
        self.position = anchor - ahead * CLOCK_TICKS;
        self.anchored = true;
        self.anchor = None;
        if ask.playing && kind == Begin::Continue {
            // Already playing: the song's own position is the clock's.
            self.anchored = false;
            return false;
        }
        let at = self.fit.t + ahead * self.fit.period;
        let frame = self.frame_of(timing, ask, at);
        let heard = timing.start
            + (f64::from(timing.latency) + (frame - ask.clock) as f64) * timing.ns_per_frame;
        let late = ((heard - at) / self.fit.period).max(0.0);
        let from = (anchor + late * CLOCK_TICKS).round() as i64;
        answer.command(frame, TransportCommand::Locate(Ticks(from)));
        if !ask.playing {
            answer.command(frame, TransportCommand::PlayFromInsertMarker);
        }
        self.latest = self.latest.max(frame);
        self.launch = Some(frame);
        true
    }
}
