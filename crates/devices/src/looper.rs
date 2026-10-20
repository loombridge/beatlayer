//! Looper (WP-5.26b; the looper spec): a device that records what it is fed
//! and loops it, layering overdubs, with a Multi-Purpose Transport Button,
//! Clear, Undo/Redo, ×2 and ÷2, Speed, Reverse, Feedback, Input → Output,
//! Quantization, Record Length, Tempo Control and Song Control.
//!
//! The manual gives no range, default, unit or buffer length for any of its
//! controls, and says nothing about how a device asks Live's transport for a
//! change; those are Beatlayer's own and are in `docs/specs/looper.md`. The
//! device asks through the display data it publishes (the looper spec, R24):
//! its node never writes the song's tempo or transport itself.

use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignatureMap};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    LaunchQuantization, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Looper's device type.
pub const LOOPER: &str = "beatlayer.looper";

/// Transport: Record, Overdub, Play or Stop.
pub const TRANSPORT: ParamId = ParamId(0);
/// Quantization: the launch quantisation grid its buttons wait for.
pub const QUANTIZATION: ParamId = ParamId(1);
/// Record Length: "x bars", or a fixed number of bars.
pub const RECORD_LENGTH: ParamId = ParamId(2);
/// Play/Overdub: what Transport becomes after a fixed-length recording.
pub const AFTER_RECORD: ParamId = ParamId(3);
/// Speed: the buffer's playback rate (and so its pitch).
pub const SPEED: ParamId = ParamId(4);
/// Reverse: plays the buffer backwards.
pub const REVERSE: ParamId = ParamId(5);
/// Feedback: the share of the buffer that survives each overdub repetition.
pub const FEEDBACK: ParamId = ParamId(6);
/// Input → Output: when the input is heard.
pub const INPUT_OUTPUT: ParamId = ParamId(7);
/// Tempo Control: what Looper's recorded tempo means.
pub const TEMPO_CONTROL: ParamId = ParamId(8);
/// Song Control: what Looper's transport does to the song's.
pub const SONG_CONTROL: ParamId = ParamId(9);

const TRANSPORT_INDEX: usize = 0;
const QUANTIZATION_INDEX: usize = 1;
const RECORD_LENGTH_INDEX: usize = 2;
const AFTER_RECORD_INDEX: usize = 3;
const SPEED_INDEX: usize = 4;
const REVERSE_INDEX: usize = 5;
const FEEDBACK_INDEX: usize = 6;
const INPUT_OUTPUT_INDEX: usize = 7;
const TEMPO_CONTROL_INDEX: usize = 8;
const SONG_CONTROL_INDEX: usize = 9;

/// The buffer's maximum length in seconds, per channel (Beatlayer's own; the
/// manual gives none). The looper spec states it.
pub const MAX_SECONDS: f64 = 30.0;

/// The Transport choice's index of each state.
const RECORD: usize = 0;
const OVERDUB: usize = 1;
const PLAY: usize = 2;
const STOP: usize = 3;

/// The Record Length choice's index of each fixed length, in bars.
const RECORD_LENGTH_BARS: [i64; 7] = [0, 1, 2, 4, 8, 16, 32];

/// The Input → Output modes' indices.
const IO_ALWAYS: usize = 0;
const IO_NEVER: usize = 1;
const IO_REC_OVR: usize = 2;
const IO_REC_OVR_STOP: usize = 3;

/// The Tempo Control choice's indices.
const TEMPO_NONE: usize = 0;

/// The Song Control choice's indices.
const SONG_START: usize = 1;
const SONG_START_STOP: usize = 2;

/// The display data's layout (the looper spec, R26): the state, whether the
/// buffer is empty, whether Undo and Redo are waiting, the position and
/// length as bar, beat and sixteenth, the recorded tempo, and the request.
const DISPLAY_LEN: usize = 18;

/// The requests a node publishes for the main thread (the looper spec, R24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RequestKind {
    /// Nothing to ask.
    None = 0,
    /// Start the song.
    Start = 1,
    /// Stop the song at the position given in ticks.
    Stop = 2,
    /// Set the song's tempo to the value given in BPM.
    Tempo = 3,
}

/// Looper's transport state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Record,
    Overdub,
    Play,
    Stop,
}

impl State {
    fn of(choice: usize) -> Self {
        match choice {
            RECORD => Self::Record,
            OVERDUB => Self::Overdub,
            PLAY => Self::Play,
            _ => Self::Stop,
        }
    }

    fn choice(self) -> usize {
        match self {
            Self::Record => RECORD,
            Self::Overdub => OVERDUB,
            Self::Play => PLAY,
            Self::Stop => STOP,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Record => "Record",
            Self::Overdub => "Overdub",
            Self::Play => "Playing",
            Self::Stop => "Stopped",
        }
    }
}

/// The Looper device type.
#[must_use]
pub fn looper_type() -> DeviceType {
    let params =
        vec![
        ParamDescriptor::choice(
            TRANSPORT,
            "Transport",
            &["Record", "Overdub", "Play", "Stop"],
            STOP,
        )
        .describe(
            "Records, overdubs, plays back or stops the buffer. While the song plays a change \
             waits for the next line of the quantisation grid; while it is stopped it takes \
             effect at once.",
        ),
        ParamDescriptor::choice(QUANTIZATION, "Quantization", &quantization_labels(), 4).describe(
            "The grid Looper's buttons wait for while the song plays. 1 Bar by default \
             (Beatlayer's own; the manual names the chooser, not what it starts on).",
        ),
        ParamDescriptor::choice(
            RECORD_LENGTH,
            "Record Length",
            &["x bars", "1 Bar", "2 Bars", "4 Bars", "8 Bars", "16 Bars", "32 Bars"],
            0,
        )
        .describe(
            "How long a recording runs: open-ended, until another button is pressed, or a \
             fixed number of bars after which Looper switches to Play or Overdub.",
        ),
        ParamDescriptor::choice(AFTER_RECORD, "Play/Overdub", &["Play", "Overdub"], 0).describe(
            "What Transport becomes when a fixed-length recording ends. The manual calls it \
             the button beside the Record Length chooser.",
        ),
        ParamDescriptor::continuous(SPEED, "Speed", (0.125, 8.0), 1.0, ParamUnit::None)
            .logarithmic()
            .describe("The buffer's playback rate, and so its pitch."),
        ParamDescriptor::toggle(REVERSE, "Reverse", false)
            .describe("Plays the recorded material backwards from where it is."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe(
                "The share of the layers that survives each repetition while overdubbing; at \
                 100 % they never fall. It does nothing in Play.",
            ),
        ParamDescriptor::choice(
            INPUT_OUTPUT,
            "Input \u{2192} Output",
            &["Always", "Never", "Rec/OVR", "Rec/OVR/Stop"],
            IO_ALWAYS,
        )
        .describe("When the input is heard beside the buffer, and in which states."),
        ParamDescriptor::choice(
            TEMPO_CONTROL,
            "Tempo Control",
            &["None", "Follow Song Tempo", "Set & Follow Song Tempo"],
            1,
        )
        .describe(
            "What the tempo the buffer was recorded at means: nothing, the speed its playback \
             follows, or the song's tempo itself.",
        ),
        ParamDescriptor::choice(
            SONG_CONTROL,
            "Song Control",
            &["None", "Start Song", "Start & Stop Song"],
            0,
        )
        .describe("Whether entering Play or Overdub starts the song, and Stop stops it."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (TRANSPORT, Widget::Chooser),
                (QUANTIZATION, Widget::Chooser),
            ],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Loop", 120.0, BODY_HEIGHT, 1.0).with_bottom(ControlBand::stacked(
                BODY_HEIGHT,
                &[
                    (RECORD_LENGTH, Widget::Chooser),
                    (AFTER_RECORD, Widget::Chooser),
                    (SPEED, Widget::Knob),
                    (REVERSE, Widget::Toggle),
                    (FEEDBACK, Widget::Knob),
                ],
            )),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (INPUT_OUTPUT, Widget::Chooser),
                (TEMPO_CONTROL, Widget::Chooser),
                (SONG_CONTROL, Widget::Chooser),
            ],
        )),
    ]));
    DeviceType::native(LOOPER, "Looper", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// The Quantization chooser's choices: the launch quantisation menu's steps.
fn quantization_labels() -> Vec<&'static str> {
    LaunchQuantization::ALL
        .iter()
        .map(|step| step.label())
        .collect()
}

/// Adds Looper to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, looper_type(), make);
}

/// Makes a Looper for `config`'s rate. Its buffers are made here, off the
/// audio thread.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Looper {
    let transport = device
        .params
        .get(&TRANSPORT)
        .map_or(STOP, |value| (*value as usize).min(STOP));
    let reverse = device
        .params
        .get(&REVERSE)
        .is_some_and(|value| *value >= 0.5);
    Looper::new(config, transport, reverse)
}

/// What the main thread asks a Looper's node to do (the looper spec, R25):
/// the actions that change the buffer, and nothing in the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Empties the buffer, keeping the length while overdubbing (R9).
    Clear,
    /// Puts the buffer back to when Overdub was last entered (R10).
    Undo,
    /// Puts back what the last Undo removed (R10).
    Redo,
    /// Doubles the buffer's length, repeating what it holds (R11).
    Double,
    /// Halves the buffer's length, keeping the half that plays (R11).
    Halve,
}

/// Looper's audio-thread half.
pub struct Looper {
    rate: SampleRate,
    channels: usize,
    max_frames: usize,
    /// The loop, planar: channel `c`, frame `i` is `buffer[c * max_frames + i]`.
    buffer: Vec<f32>,
    /// The other side of Undo/Redo: after an Undo it holds what the Undo took
    /// away, so a Redo is the same swap (R10).
    undo: Vec<f32>,
    has_undo: bool,
    has_redo: bool,
    /// The buffer's length in frames: 0 before anything is recorded.
    length: usize,
    /// Where in the buffer the playback is, as frames, fractional at any
    /// rate but 1.
    pos: f64,
    /// The state Looper is in.
    state: State,
    /// The Transport value last seen, which says when it changed.
    seen: usize,
    /// A transport change waiting for its frame: the state and the song
    /// frame it acts on (R5).
    pending: Option<(State, i64)>,
    /// The Reverse value last seen, which says when it changed.
    seen_reverse: bool,
    /// Whether the song was playing in the stretch last processed, which
    /// Clear's rule (R9) and the quantisation grid (R5) read.
    song_playing: bool,
    /// A Reverse change waiting for its frame (R14, through R5).
    pending_reverse: Option<(bool, i64)>,
    /// The Reverse value in effect.
    reverse: bool,
    /// Where a fixed-length recording ends, as a song frame (R6).
    record_end: Option<i64>,
    /// The song tick a recording began on, which the tempo rules use (R22).
    record_start: Ticks,
    /// The tempo when a recording began, which the open-ended guess uses.
    record_tempo: f64,
    /// The tempo the buffer was recorded at, in BPM (R20).
    recorded_tempo: f64,
    /// What the node is asking the main thread for, if anything (R24).
    request: Option<(RequestKind, u32, i64, f64)>,
    /// The serial the next request takes.
    serial: u32,
    /// What the display published last (R26).
    display: [f32; DISPLAY_LEN],
}

impl Looper {
    fn new(config: &NodeConfig, transport: usize, reverse: bool) -> Self {
        let rate = config.sample_rate;
        let channels = config.channels.clamp(1, 2);
        let max_frames = (MAX_SECONDS * rate.as_f64()) as usize;
        let samples = max_frames * channels;
        Self {
            rate,
            channels,
            max_frames,
            buffer: vec![0.0; samples],
            undo: vec![0.0; samples],
            has_undo: false,
            has_redo: false,
            length: 0,
            pos: 0.0,
            state: State::of(transport),
            seen: transport,
            pending: None,
            seen_reverse: reverse,
            song_playing: false,
            pending_reverse: None,
            reverse,
            record_end: None,
            record_start: Ticks::ZERO,
            record_tempo: 120.0,
            recorded_tempo: 120.0,
            request: None,
            serial: 0,
            display: [0.0; DISPLAY_LEN],
        }
    }

    /// The quantisation grid Looper's buttons wait for (R17).
    fn quantization(&self, params: &ParamBank) -> LaunchQuantization {
        let index =
            (params.value(QUANTIZATION_INDEX) as usize).min(LaunchQuantization::ALL.len() - 1);
        LaunchQuantization::ALL[index]
    }

    /// Whether the Input → Output mode hears the input in `state` (R16).
    fn input_heard(&self, mode: usize, state: State, recording_or_overdubbing: bool) -> bool {
        match mode {
            IO_NEVER => false,
            IO_REC_OVR => recording_or_overdubbing,
            IO_REC_OVR_STOP => !matches!(state, State::Play),
            _ => true,
        }
    }

    /// The song frame a change acts on (R5): the next line of the
    /// quantisation grid while the song plays, and now while it is stopped.
    fn due(
        &self,
        song: i64,
        playing: bool,
        tempo: &TempoMap,
        params: &ParamBank,
        signatures: &TimeSignatureMap,
    ) -> i64 {
        if playing {
            let from = tempo.frames_to_ticks(Frames(song), self.rate);
            let line = self.quantization(params).next_line(from, signatures);
            tempo.ticks_to_frames(line, self.rate).get()
        } else {
            song
        }
    }

    /// Takes a transport change: while the song plays it waits for the next
    /// grid line, and while it is stopped it acts at once (R5). A change back
    /// to the state in effect takes back a queued one.
    fn change(
        &mut self,
        wanted: State,
        song: i64,
        playing: bool,
        tempo: &TempoMap,
        params: &ParamBank,
        signatures: &TimeSignatureMap,
    ) {
        if wanted == self.state {
            self.pending = None;
            return;
        }
        let due = self.due(song, playing, tempo, params, signatures);
        self.pending = Some((wanted, due));
    }

    /// Takes a Reverse change, which waits like a transport change (R14). A
    /// change back to the value in effect takes back a queued one.
    fn change_reverse(
        &mut self,
        wanted: bool,
        song: i64,
        playing: bool,
        tempo: &TempoMap,
        params: &ParamBank,
        signatures: &TimeSignatureMap,
    ) {
        if wanted == self.reverse {
            self.pending_reverse = None;
            return;
        }
        let due = self.due(song, playing, tempo, params, signatures);
        self.pending_reverse = Some((wanted, due));
    }

    /// Puts a transport change into effect on `song`, and asks the song for a
    /// transport change if Song Control says so (R23).
    #[allow(clippy::too_many_arguments, reason = "the rules each need one")]
    fn apply(
        &mut self,
        state: State,
        song: i64,
        playing: bool,
        tempo: &TempoMap,
        signatures: &TimeSignatureMap,
        song_control: usize,
        tempo_control: usize,
        record_length: i64,
    ) {
        // Leaving Record keeps what was recorded (R6).
        if self.state == State::Record && state != State::Record {
            self.end_recording(playing, signatures, tempo_control, record_length);
        }
        match state {
            State::Record => {
                self.length = 0;
                self.pos = 0.0;
                self.record_start = tempo.frames_to_ticks(Frames(song), self.rate);
                self.record_tempo = tempo.bpm_at(self.record_start).get();
                self.record_end = if playing && record_length > 0 {
                    let bar = signatures.signature_at(self.record_start).bar_length();
                    let end = self.record_start + bar * record_length;
                    Some(tempo.ticks_to_frames(end, self.rate).get())
                } else {
                    None
                };
            }
            State::Overdub => {
                self.snapshot();
            }
            State::Play | State::Stop => {}
        }
        self.state = state;
        // R23: entering Play or Overdub starts a stopped song; entering Stop
        // with Start & Stop stops a playing one.
        if matches!(state, State::Play | State::Overdub) && song_control >= SONG_START && !playing {
            self.ask(RequestKind::Start, 0, 0.0);
        } else if state == State::Stop && song_control >= SONG_START_STOP && playing {
            let at = tempo.frames_to_ticks(Frames(song), self.rate);
            self.ask(RequestKind::Stop, at.get(), 0.0);
        }
    }

    /// Works out what a finished recording leaves: the frames recorded, the
    /// tempo it was recorded at, and the tempo the song is asked for (R6,
    /// R22). It doesn't change the state.
    fn end_recording(
        &mut self,
        playing: bool,
        signatures: &TimeSignatureMap,
        tempo_control: usize,
        record_length: i64,
    ) {
        let frames = (self.pos.floor() as usize).min(self.max_frames).max(1);
        self.length = frames;
        let seconds = frames as f64 / self.rate.as_f64();
        let bar = signatures
            .signature_at(self.record_start)
            .bar_length()
            .get() as f64;
        let bars = if record_length > 0 {
            record_length as f64
        } else {
            // R22's guess: the whole number of bars nearest to what Looper's
            // tempo was when the recording began.
            let bar_seconds = bar / TICKS_PER_QUARTER as f64 * 60.0 / self.record_tempo;
            (seconds / bar_seconds).round().max(1.0)
        };
        let quarters = bar * bars / TICKS_PER_QUARTER as f64;
        let recorded = quarters / seconds * 60.0;
        if recorded.is_finite() && recorded > 0.0 {
            self.recorded_tempo = recorded;
        }
        self.record_end = None;
        self.pos = 0.0;
        self.snapshot_clear();
        if tempo_control != TEMPO_NONE && !playing {
            // R22: Set & Follow asks the song for the tempo the recording
            // gives, once it is known. Only while the song is stopped: while
            // it plays, the recording already took the song's tempo.
            self.ask(
                RequestKind::Tempo,
                self.record_start.get(),
                self.recorded_tempo,
            );
        }
    }

    /// Keeps the buffer as the other side of Undo, so an Undo can put it
    /// back exactly (R10).
    fn snapshot(&mut self) {
        if self.length == 0 {
            self.has_undo = false;
            self.has_redo = false;
            return;
        }
        for channel in 0..self.channels {
            let start = channel * self.max_frames;
            let end = start + self.length;
            self.undo[start..end].copy_from_slice(&self.buffer[start..end]);
        }
        self.has_undo = true;
        self.has_redo = false;
    }

    /// No undo or redo is waiting any more.
    fn snapshot_clear(&mut self) {
        self.has_undo = false;
        self.has_redo = false;
    }

    /// Swaps the buffer with the other side of Undo/Redo (R10).
    fn swap(&mut self) {
        std::mem::swap(&mut self.buffer, &mut self.undo);
    }

    /// Empties the buffer (R9): while overdubbing, the samples alone.
    fn clear(&mut self, keep_length: bool) {
        self.buffer.fill(0.0);
        self.snapshot_clear();
        if !keep_length {
            self.length = 0;
            self.pos = 0.0;
            self.recorded_tempo = self.record_tempo;
            self.record_end = None;
            self.state = State::Stop;
            self.seen = STOP;
        }
    }

    /// Doubles the buffer, repeating what it holds (R11).
    fn double(&mut self) {
        if self.length == 0 || self.length * 2 > self.max_frames {
            return;
        }
        let old = self.length;
        for channel in 0..self.channels {
            let start = channel * self.max_frames;
            self.buffer.copy_within(start..start + old, start + old);
        }
        self.length = old * 2;
    }

    /// Halves the buffer, keeping the half that plays (R11).
    fn halve(&mut self) {
        if self.length < 2 {
            return;
        }
        let half = self.length / 2;
        let keep_second = self.pos as usize >= half;
        let from = if keep_second { half } else { 0 };
        for channel in 0..self.channels {
            let start = channel * self.max_frames;
            self.buffer
                .copy_within(start + from..start + from + half, start);
        }
        self.length = half;
        // The playhead keeps its place in the half that plays.
        self.pos = if keep_second {
            self.pos - half as f64
        } else {
            self.pos
        };
    }

    /// Asks the main thread for something (R24), once per request.
    fn ask(&mut self, kind: RequestKind, ticks: i64, bpm: f64) {
        self.serial = self.serial.wrapping_add(1);
        self.request = Some((kind, self.serial, ticks, bpm));
    }

    /// Takes a command from the main thread (R25).
    fn command(&mut self, action: Action) {
        match action {
            Action::Clear => {
                // R9: the length and tempo are kept only in Overdub while
                // the song plays.
                let keep = self.state == State::Overdub && self.song_playing;
                self.clear(keep);
            }
            Action::Undo => {
                if self.has_undo {
                    self.swap();
                    self.has_undo = false;
                    self.has_redo = true;
                }
            }
            Action::Redo => {
                if self.has_redo {
                    self.swap();
                    self.has_redo = false;
                    self.has_undo = true;
                }
            }
            Action::Double => self.double(),
            Action::Halve => self.halve(),
        }
    }

    /// The whole buffer scaled by `gain`, the feedback of one repetition
    /// (R15).
    fn decay(&mut self, gain: f32) {
        for channel in 0..self.channels {
            let start = channel * self.max_frames;
            for sample in &mut self.buffer[start..start + self.length] {
                *sample *= gain;
            }
        }
    }

    /// Works out what the display publishes (R26).
    fn publish(&mut self, signatures: &TimeSignatureMap) {
        let frames = self.length as f64;
        // The loop's own length and position, in the musical time of the
        // tempo it was recorded at.
        let seconds = frames / self.rate.as_f64();
        let length_ticks =
            (seconds * self.recorded_tempo / 60.0 * TICKS_PER_QUARTER as f64).round() as i64;
        let position_frames = self.pos.min(frames);
        let position_seconds = position_frames / self.rate.as_f64();
        let position_ticks = (position_seconds * self.recorded_tempo / 60.0
            * TICKS_PER_QUARTER as f64)
            .round() as i64;
        let position = signatures.to_bbs(Ticks(position_ticks));
        let (bar, beat, sixteenth) = length_labels(Ticks(length_ticks), signatures);
        let (kind, serial, ticks, bpm) = match self.request {
            Some((kind, serial, ticks, bpm)) => (kind, serial, ticks, bpm),
            None => (RequestKind::None, 0, 0, 0.0),
        };
        let lo = ticks.rem_euclid(1 << 24) as f32;
        let hi = (ticks >> 24) as f32;
        self.display = [
            self.state.choice() as f32,
            f32::from(self.length == 0),
            f32::from(self.has_undo),
            f32::from(self.has_redo),
            position.bar as f32,
            position.beat as f32,
            position.sixteenth as f32,
            bar as f32,
            beat as f32,
            sixteenth as f32,
            self.recorded_tempo as f32,
            kind as u8 as f32,
            serial as f32,
            lo,
            hi,
            bpm as f32,
            self.length as f32,
            self.pos.min(self.length as f64) as f32,
        ];
    }
}

/// A length's bars, beats and sixteenths, counted from 0, as Live's length
/// fields show them (the musical-time spec, "What Live does").
fn length_labels(ticks: Ticks, signatures: &TimeSignatureMap) -> (i64, u32, u32) {
    let bar = signatures.signature_at(ticks).bar_length();
    if bar.get() <= 0 {
        return (0, 0, 0);
    }
    let bars = ticks.get().div_euclid(bar.get());
    let rest = ticks.get().rem_euclid(bar.get());
    let signature = signatures.signature_at(ticks);
    let beat = signature.beat_length();
    let beats = rest.div_euclid(beat.get().max(1));
    let sixteenth = rest.rem_euclid(beat.get().max(1)) / (TICKS_PER_QUARTER / 4).max(1);
    (bars, beats as u32, sixteenth as u32)
}

impl NativeDevice for Looper {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        let rate = self.rate;
        let playing = audio.playing();
        self.song_playing = playing;
        let tempo = audio.tempo();
        let signatures = audio.signatures();
        let song0 = audio.song_position().get();
        let channels = audio.channels().min(self.channels).max(1);

        // The Transport change, and the other choices, are read at the
        // stretch's start: a stepped value changes only between stretches
        // (R5).
        let record_length = RECORD_LENGTH_BARS
            [(params.value(RECORD_LENGTH_INDEX) as usize).min(RECORD_LENGTH_BARS.len() - 1)];
        let after = if (params.value(AFTER_RECORD_INDEX) as usize) == 0 {
            State::Play
        } else {
            State::Overdub
        };
        let input_output = (params.value(INPUT_OUTPUT_INDEX) as usize).min(IO_REC_OVR_STOP);
        let tempo_control = (params.value(TEMPO_CONTROL_INDEX) as usize).min(2);
        let song_control = (params.value(SONG_CONTROL_INDEX) as usize).min(2);

        // A change that has come due acts before a new one is taken, so two
        // in a row in one stretch keep their order.
        if let Some((state, due)) = self.pending
            && (!playing || song0 >= due)
        {
            self.pending = None;
            self.apply(
                state,
                song0,
                playing,
                tempo,
                signatures,
                song_control,
                tempo_control,
                record_length,
            );
        }
        if let Some((value, due)) = self.pending_reverse
            && (!playing || song0 >= due)
        {
            self.pending_reverse = None;
            self.reverse = value;
        }
        let wanted = (params.value(TRANSPORT_INDEX) as usize).min(STOP);
        if wanted != self.seen {
            self.seen = wanted;
            self.change(State::of(wanted), song0, playing, tempo, params, signatures);
        }
        // A Reverse change waits like a transport change (R14).
        let wanted_reverse = params.value(REVERSE_INDEX) >= 0.5;
        if wanted_reverse != self.seen_reverse {
            self.seen_reverse = wanted_reverse;
            self.change_reverse(wanted_reverse, song0, playing, tempo, params, signatures);
        }

        let mut input = [0.0f32; 2];
        let mut out = [0.0f32; 2];
        for i in 0..frames {
            let song = song0 + i as i64;
            // A queued transport change acts on its frame (R5).
            if let Some((state, due)) = self.pending
                && (song >= due || (!playing && i == 0))
            {
                self.pending = None;
                self.apply(
                    state,
                    song,
                    playing,
                    tempo,
                    signatures,
                    song_control,
                    tempo_control,
                    record_length,
                );
            }
            // A queued Reverse change acts on its frame too (R14).
            if let Some((value, due)) = self.pending_reverse
                && (song >= due || (!playing && i == 0))
            {
                self.pending_reverse = None;
                self.reverse = value;
            }
            // A fixed-length recording ends after its bars (R6).
            if self.state == State::Record {
                if let Some(end) = self.record_end {
                    if song >= end {
                        self.end_recording(playing, signatures, tempo_control, record_length);
                        self.state = after;
                        self.pending = None;
                    }
                } else if self.pos >= self.max_frames as f64 {
                    self.end_recording(playing, signatures, tempo_control, record_length);
                    self.state = after;
                    self.pending = None;
                }
            }

            let speed = params.ramp(SPEED_INDEX)[i] as f64;
            let reverse = self.reverse;
            let feedback = (params.ramp(FEEDBACK_INDEX)[i] as f64 / 100.0) as f32;
            let rate_now = if tempo_control == TEMPO_NONE || self.length == 0 {
                speed
            } else {
                let tick = tempo.frames_to_ticks(Frames(song), rate);
                let song_bpm = tempo.bpm_at(tick).get();
                speed * song_bpm / self.recorded_tempo.max(1.0)
            };

            for (channel, samples) in audio.channels_mut().enumerate().take(channels) {
                input[channel] = samples[i];
            }

            let length = self.length;
            let idx = self.pos.floor() as usize;
            let overdubbing = self.state == State::Overdub;
            let recording = self.state == State::Record;
            // R8: only Play and Overdub sound the buffer.
            let sounding = matches!(self.state, State::Play | State::Overdub);
            let heard = self.input_heard(input_output, self.state, recording || overdubbing);
            let mut wrapped = false;
            for channel in 0..channels {
                let base = channel * self.max_frames;
                let out_sample = if recording {
                    // R6: the first pass writes the input over the buffer
                    // from its start, and plays no buffer while it does.
                    let at = base + idx.min(self.max_frames - 1);
                    self.buffer[at] = input[channel];
                    0.0
                } else if length == 0 || !sounding {
                    0.0
                } else {
                    // R14: Reverse reads from the other end and writes
                    // forwards.
                    let forward = idx.min(length - 1);
                    let read = if reverse {
                        length - 1 - forward
                    } else {
                        forward
                    };
                    let sample = self.buffer[base + read];
                    if overdubbing {
                        self.buffer[base + forward] += input[channel];
                    }
                    sample
                };
                out[channel] = out_sample + if heard { input[channel] } else { 0.0 };
            }
            for (channel, samples) in audio.channels_mut().enumerate().take(channels) {
                samples[i] = out[channel];
            }

            // The playhead advances by the rate in effect (R13), and each
            // repetition of an overdub scales the layers (R15).
            if self.length > 0 && !recording {
                self.pos += rate_now.max(0.0);
                if self.pos >= self.length as f64 {
                    self.pos %= self.length as f64;
                    if overdubbing {
                        wrapped = true;
                    }
                }
            } else if self.length == 0 && recording {
                self.pos += 1.0;
            }
            if wrapped {
                self.decay(feedback);
            }
        }

        self.publish(signatures);
    }

    fn receive_data(&mut self, data: bl_engine::NodeData) {
        if let Some(action) = data.downcast_ref::<Action>() {
            self.command(*action);
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        let len = out.len().min(DISPLAY_LEN);
        out[..len].copy_from_slice(&self.display[..len]);
    }
}

/// The display a Looper draws from its data (the looper spec, R26).
#[must_use]
pub fn display(
    device: DeviceId,
    _description: &DeviceDescription,
    _values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let Some(data) = data.filter(|d| d.len() >= DISPLAY_LEN) else {
        return empty(device);
    };
    let state = State::of(data[0] as usize);
    let empty_buffer = data[1] >= 0.5;
    let state_text = if state == State::Record {
        "Record"
    } else if empty_buffer {
        "Empty"
    } else {
        state.label()
    };
    let position = format!("{}.{}.{}", data[4] as i64, data[5] as u32, data[6] as u32);
    let length = format!("{}.{}.{}", data[7] as i64, data[8] as u32, data[9] as u32);
    let tempo = format!("{:.1} BPM", data[10]);
    DeviceDisplay {
        device,
        live: true,
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: vec![
            reading("State", data[0], state_text.to_owned()),
            reading("Position", data[4], position),
            reading("Length", data[7], length),
            reading("Tempo", data[10], tempo),
        ],
    }
}

fn reading(name: &str, value: f32, text: String) -> DisplayReading {
    DisplayReading {
        name: name.to_owned(),
        value,
        unit: String::new(),
        text,
    }
}

/// A Looper with no engine running yet draws nothing.
fn empty(device: DeviceId) -> DeviceDisplay {
    DeviceDisplay {
        device,
        live: false,
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    }
}
