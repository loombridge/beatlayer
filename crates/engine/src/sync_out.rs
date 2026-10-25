//! MIDI clock out (`docs/specs/midi-clock.md`, R3 to R8, R11 to R13): the
//! Timing Clock, Start, Continue, Stop and Song Position Pointer the engine
//! writes for each output port with Sync on, each for the frame it falls on.
//!
//! The processor tells [`SyncOut`] what the transport did in every part of a
//! block; it works out the messages from what changed since the part before
//! (a start, a stop, a jump of the play position, a move of the insert
//! marker), so every way the transport moves is covered by one place.
//! Positions are converted through the tempo map only, with [`Beats`] as
//! the one place a tick becomes a frame, so a followed tempo can replace it.

use bl_core::midi::{ClockType, HardwareResync, MidiMessage, SyncOutput};
use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignatureMap};

use crate::midi::{MidiIn, Stamped};

/// The most output ports with Sync on the engine sends clock to; the list is
/// made with the engine, and any more are ignored and counted.
pub const MAX_SYNC_OUTPUTS: usize = 32;

/// Ticks in one Timing Clock: a 24th of a quarter note.
const CLOCK_TICKS: i64 = TICKS_PER_QUARTER / 24;
/// Timing Clocks in one MIDI beat, a sixteenth note.
const SIXTEENTH_CLOCKS: i64 = 6;
/// Ticks in a sixteenth note.
const SIXTEENTH_TICKS: i64 = CLOCK_TICKS * SIXTEENTH_CLOCKS;
/// A jump of the play position smaller than this, in ticks, is rounding.
const JUMP_TICKS: i64 = CLOCK_TICKS / 2;
/// The clocks one part of a block writes at most. At 999 BPM a 4,096-frame
/// block at 44.1 kHz holds 37; the bound only protects a cursor gone astray.
const MAX_CLOCKS_PER_PART: usize = 256;

/// What the engine reports of the clock it sends, for `sync.state` and the
/// lower sync lamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncOutState {
    /// Whether clock is going out now: the song plays, past any count-in,
    /// and some output has Sync on.
    pub sending: bool,
    /// Quarter notes of clock sent: the sync lamp lights for 100 ms each
    /// time it grows.
    pub quarters: u64,
    /// Timing Clocks handed to the output queue, one per port.
    pub clocks: u64,
    /// Timing Clocks that were stamped for a moment before the block that
    /// wrote them began, apart from those a start or jump makes
    /// unavoidable under a negative sync delay.
    pub late: u64,
    /// Sync messages dropped because the queue to the output thread was
    /// full, and outputs over [`MAX_SYNC_OUTPUTS`].
    pub dropped: u64,
}

/// Ticks and frames through the tempo map: the one place a tick becomes a
/// frame for clock out.
pub(crate) struct Beats<'a> {
    pub tempo: &'a TempoMap,
    pub rate: SampleRate,
}

impl Beats<'_> {
    fn frame(&self, ticks: Ticks) -> Frames {
        self.tempo.ticks_to_frames(ticks, self.rate)
    }

    fn ticks(&self, frames: Frames) -> Ticks {
        self.tempo.frames_to_ticks(frames, self.rate)
    }

    /// The index of the first Timing Clock whose frame is at or after
    /// `position`.
    fn clock_at_or_after(&self, position: Frames) -> i64 {
        let at = self.ticks(position).get();
        let mut k = at.div_euclid(CLOCK_TICKS) + i64::from(at.rem_euclid(CLOCK_TICKS) != 0);
        k = k.max(0);
        while k > 0 && self.frame(Ticks(k * CLOCK_TICKS - CLOCK_TICKS)) >= position {
            k -= 1;
        }
        while self.frame(Ticks(k * CLOCK_TICKS)) < position {
            k += 1;
        }
        k
    }
}

/// One part of a block, as the transport has it at the part's first frame.
pub(crate) struct Part<'a> {
    /// The part's first frame, in frames from the block's start.
    pub offset: u32,
    /// The part's length.
    pub length: u32,
    /// The device's output latency in frames.
    pub output_latency: u32,
    /// The song plays: the transport runs and no count-in sounds.
    pub playing: bool,
    /// A count-in sounds.
    pub counting_in: bool,
    /// The song position of the part's first frame.
    pub position: Frames,
    /// Where Play starts.
    pub insert_marker: Ticks,
    /// The frame the Arrangement loop wraps at, if it is on and ahead.
    pub loop_end: Option<Frames>,
    pub beats: Beats<'a>,
    pub signatures: &'a TimeSignatureMap,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Nothing,
    Started,
    Jumped,
    Stopped,
    MarkerMoved,
}

/// The first bar line at or after `at`.
fn bar_line(signatures: &TimeSignatureMap, at: Ticks) -> Ticks {
    let (bar, into) = signatures.bar_at(at);
    if into == Ticks::ZERO {
        at
    } else {
        signatures.bar_start(bar + 1)
    }
}

/// The first sixteenth note at or after `at`.
fn sixteenth_at_or_after(at: Ticks) -> Ticks {
    Ticks((at.get().max(0) + SIXTEENTH_TICKS - 1) / SIXTEENTH_TICKS * SIXTEENTH_TICKS)
}

/// The engine's clock out.
pub(crate) struct SyncOut {
    outputs: Vec<SyncOutput>,
    /// What the transport did in the part before.
    was_playing: bool,
    /// Where the play position would be if nothing had moved it.
    expect: Option<Ticks>,
    /// The insert marker as the part before had it.
    marker: Option<Ticks>,
    /// The next Timing Clock to write, by index: tick `cursor * CLOCK_TICKS`.
    cursor: Option<i64>,
    /// Song outputs send their first clock on this tick or after it: the
    /// sixteenth note their Song Position Pointer names.
    song_from: Ticks,
    /// Pattern outputs send Start before the clock at this tick.
    pattern_start: Option<Ticks>,
    /// Resync External Hardware goes out before the clock at this tick.
    resync_bar: Option<Ticks>,
    /// Messages for frames before this need a moment already past under a
    /// negative sync delay, so they aren't counted late.
    unavoidable_until: Frames,
    stats: SyncOutState,
}

impl SyncOut {
    pub(crate) fn new() -> Self {
        Self {
            outputs: Vec::with_capacity(MAX_SYNC_OUTPUTS),
            was_playing: false,
            expect: None,
            marker: None,
            cursor: None,
            song_from: Ticks::ZERO,
            pattern_start: None,
            resync_bar: None,
            unavoidable_until: Frames::ZERO,
            stats: SyncOutState::default(),
        }
    }

    /// Sets output `index` of `count` outputs (a count of 0 clears the
    /// list).
    pub(crate) fn set_output(&mut self, index: u8, count: u8, output: Option<SyncOutput>) {
        let (index, count) = (usize::from(index), usize::from(count));
        if count > MAX_SYNC_OUTPUTS {
            self.stats.dropped += 1;
            return;
        }
        self.outputs.truncate(count);
        if let Some(output) = output
            && index < count
        {
            if index < self.outputs.len() {
                self.outputs[index] = output;
            } else if index == self.outputs.len() {
                self.outputs.push(output);
            }
        }
    }

    /// What the engine reports.
    pub(crate) fn state(&self, playing: bool, counting_in: bool) -> SyncOutState {
        SyncOutState {
            sending: playing && !counting_in && !self.outputs.is_empty(),
            ..self.stats
        }
    }

    /// Resync External Hardware: each output's choice goes out before the
    /// clock of the next bar line at or after `position`, while the song
    /// plays (R11).
    pub(crate) fn resync(&mut self, position: Ticks, signatures: &TimeSignatureMap, playing: bool) {
        if playing && self.cursor.is_some() {
            self.resync_bar = Some(bar_line(signatures, position));
        }
    }

    /// Stop to every output, at once, off the audio thread: a stream that
    /// closes while the song plays.
    pub(crate) fn park(&mut self, midi: &mut MidiIn) {
        if std::mem::take(&mut self.was_playing) {
            for output in &self.outputs {
                midi.send_now(output.port, MidiMessage::STOP);
            }
        }
        self.cursor = None;
        self.expect = None;
    }

    /// Writes what the transport did in this part, and every clock whose
    /// frame falls in it (or, for a negative sync delay beyond the output
    /// latency, in the time just after it).
    pub(crate) fn part(&mut self, p: &Part<'_>, midi: &mut MidiIn) {
        let marker = self.marker.replace(p.insert_marker);
        if p.counting_in {
            // Spec: midi-clock R8. Nothing goes out until the song starts.
            self.stop_running();
            return;
        }
        let at = if p.playing {
            p.beats.ticks(p.position)
        } else {
            Ticks::ZERO
        };
        let change = match (self.was_playing, p.playing) {
            (false, true) => Change::Started,
            (true, false) => Change::Stopped,
            (true, true)
                if self
                    .expect
                    .is_some_and(|e| (at - e).abs().get() > JUMP_TICKS) =>
            {
                Change::Jumped
            }
            (false, false) if marker.is_some_and(|m| m != p.insert_marker) => Change::MarkerMoved,
            _ => Change::Nothing,
        };
        self.was_playing = p.playing;
        let ahead = self.lookahead(p);
        if !self.outputs.is_empty() {
            self.announce(p, change, at, midi);
        }
        match change {
            Change::Started | Change::Jumped => {
                self.cursor = None;
                self.song_from = sixteenth_at_or_after(at);
                self.resync_bar = None;
                self.unavoidable_until = p.position + Frames(ahead);
                if change == Change::Started || self.pattern_start.is_some() {
                    let pattern = self
                        .outputs
                        .iter()
                        .any(|o| o.clock_type == ClockType::Pattern);
                    self.pattern_start = pattern.then(|| bar_line(p.signatures, at));
                }
            }
            Change::Stopped => self.stop_running(),
            Change::MarkerMoved | Change::Nothing => {}
        }
        if !p.playing {
            return;
        }
        let end = p.position + Frames(i64::from(p.length));
        self.expect = Some(p.beats.ticks(end));
        if self.outputs.is_empty() {
            self.cursor = None;
            return;
        }
        let cursor = *self
            .cursor
            .get_or_insert_with(|| p.beats.clock_at_or_after(p.position));
        let mut horizon = end + Frames(ahead);
        if let Some(wrap) = p.loop_end {
            horizon = horizon.min(wrap);
        }
        self.clocks(p, cursor, horizon, midi);
    }

    fn stop_running(&mut self) {
        self.was_playing = false;
        self.cursor = None;
        self.expect = None;
        self.pattern_start = None;
        self.resync_bar = None;
    }

    /// How many frames beyond a part its clocks are written ahead: the
    /// longest negative sync delay, less what the output latency already
    /// gives (R4).
    fn lookahead(&self, p: &Part<'_>) -> i64 {
        let rate = p.beats.rate.as_f64();
        self.outputs
            .iter()
            .map(|o| {
                let early = (f64::from(-o.delay_ms) * rate / 1000.0).ceil() as i64;
                (early - i64::from(p.output_latency)).max(0)
            })
            .max()
            .unwrap_or(0)
    }

    /// Start, Continue, Stop and Song Position Pointer for a change of the
    /// transport, on the part's first frame, in R7's order.
    fn announce(&mut self, p: &Part<'_>, change: Change, at: Ticks, midi: &mut MidiIn) {
        if change == Change::Nothing {
            return;
        }
        let pointer =
            |ticks: Ticks| MidiMessage::song_position((ticks.get() / SIXTEENTH_TICKS) as u32);
        let from = sixteenth_at_or_after(at);
        for i in 0..self.outputs.len() {
            let output = self.outputs[i];
            let song = output.clock_type == ClockType::Song;
            let (first, second) = match change {
                Change::Started if song && at == Ticks::ZERO => (Some(MidiMessage::START), None),
                Change::Started | Change::Jumped if song => {
                    (Some(pointer(from)), Some(MidiMessage::CONTINUE))
                }
                Change::Stopped => (Some(MidiMessage::STOP), None),
                Change::MarkerMoved if song => {
                    (Some(pointer(sixteenth_at_or_after(p.insert_marker))), None)
                }
                _ => (None, None),
            };
            for message in first.into_iter().chain(second) {
                self.send(midi, p, output, p.position, message, false);
            }
        }
    }

    /// Writes the clocks (and the Start and resync due before one) whose
    /// frames lie before `horizon`.
    fn clocks(&mut self, p: &Part<'_>, mut cursor: i64, horizon: Frames, midi: &mut MidiIn) {
        for _ in 0..MAX_CLOCKS_PER_PART {
            let tick = Ticks(cursor * CLOCK_TICKS);
            // A clock never falls before the part: a tempo change moves
            // frames under the cursor.
            let frame = p.beats.frame(tick).max(p.position);
            if frame >= horizon {
                break;
            }
            if self.pattern_start.is_some_and(|bar| tick >= bar) {
                self.pattern_start = None;
                for i in 0..self.outputs.len() {
                    let output = self.outputs[i];
                    if output.clock_type == ClockType::Pattern {
                        self.send(midi, p, output, frame, MidiMessage::START, true);
                    }
                }
            }
            if self.resync_bar.is_some_and(|bar| tick >= bar) {
                self.resync_bar = None;
                for i in 0..self.outputs.len() {
                    let output = self.outputs[i];
                    let (stop, start) = match output.resync {
                        HardwareResync::StopAndStart => (true, true),
                        HardwareResync::StartOnly => (false, true),
                        HardwareResync::Off => (false, false),
                    };
                    for (wanted, message) in
                        [(stop, MidiMessage::STOP), (start, MidiMessage::START)]
                    {
                        if wanted {
                            self.send(midi, p, output, frame, message, true);
                        }
                    }
                }
            }
            let mut sent = false;
            for i in 0..self.outputs.len() {
                let output = self.outputs[i];
                if output.clock_type == ClockType::Song && tick < self.song_from {
                    continue;
                }
                sent |= self.send(midi, p, output, frame, MidiMessage::CLOCK, true);
            }
            if sent && cursor % 24 == 0 {
                self.stats.quarters += 1;
            }
            cursor += 1;
        }
        self.cursor = Some(cursor);
    }

    /// Stamps `message` for `frame` of the song and queues it for `output`.
    /// Returns whether the queue took it. A `steady` message is one a late
    /// stamp counts against, which a start's own messages aren't.
    fn send(
        &mut self,
        midi: &mut MidiIn,
        p: &Part<'_>,
        output: SyncOutput,
        frame: Frames,
        message: MidiMessage,
        steady: bool,
    ) -> bool {
        let from_block = i64::from(p.offset) + (frame - p.position).get();
        let stamped = midi.emit_sync(
            output.port,
            from_block + i64::from(p.output_latency),
            output.delay_ms,
            message,
        );
        let is_clock = message == MidiMessage::CLOCK;
        match stamped {
            Stamped::Offline => false,
            Stamped::Dropped => {
                self.stats.dropped += 1;
                false
            }
            Stamped::Queued { past } => {
                if is_clock {
                    self.stats.clocks += 1;
                }
                if past && steady && frame >= self.unavoidable_until {
                    self.stats.late += 1;
                }
                true
            }
        }
    }
}
