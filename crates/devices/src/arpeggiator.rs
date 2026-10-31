//! Arpeggiator, the arpeggiator MIDI effect (WP-5.31; the arpeggiator spec):
//! transforms chords and notes into rhythmic sequences across a variety of
//! styles, with synced or free rate, gate, hold, offset, retrigger, repeats,
//! progressive transposition steps, and velocity decay.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_core::id::IdAllocator;
use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::scales::OFF;
use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{
    Device, DeviceDescription, DeviceRole, Groove, GrooveBase, GrooveId, GrooveNote, GroovePlay,
    ParamDescriptor, ParamId, ParamUnit, Scale,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, scale_choices, scale_named, seed, use_current_scale, value};

/// The Arpeggiator device type's ID.
pub const ARPEGGIATOR: &str = "beatlayer.arpeggiator";

/// The style / pattern order.
pub const STYLE: ParamId = ParamId(0);
/// Rate sync: beat divisions (true) vs free milliseconds (false).
pub const SYNC: ParamId = ParamId(1);
/// Beat division for synced rate.
pub const RATE_SYNC: ParamId = ParamId(2);
/// Free rate in milliseconds.
pub const RATE_MS: ParamId = ParamId(3);
/// Note duration as percentage of step rate.
pub const GATE: ParamId = ParamId(4);
/// Hold mode: keeps arpeggio playing after keys are released.
pub const HOLD: ParamId = ParamId(5);
/// Sequence offset shifting the starting step.
pub const OFFSET: ParamId = ParamId(6);
/// Retrigger mode: Off, Note, or Beat.
pub const RETRIGGER: ParamId = ParamId(7);
/// Retrigger interval on the beat grid.
pub const INTERVAL: ParamId = ParamId(8);
/// Number of pattern repeats before stopping (0 = infinite).
pub const REPEATS: ParamId = ParamId(9);
/// Pitch transposition distance per step, in semitones.
pub const DISTANCE: ParamId = ParamId(10);
/// Number of transposition steps.
pub const STEPS: ParamId = ParamId(11);
/// Enables velocity decay slope.
pub const VELOCITY_ON: ParamId = ParamId(12);
/// Velocity decay time in milliseconds.
pub const DECAY: ParamId = ParamId(13);
/// Target velocity at end of decay.
pub const TARGET: ParamId = ParamId(14);
/// Retrigger velocity decay when pattern retriggers.
pub const RETRIGGER_VELOCITY: ParamId = ParamId(15);
/// Counts Distance in degrees of the device's current scale (the arpeggiator
/// spec, R16); the title bar draws it.
pub const USE_SCALE: ParamId = ParamId(16);
/// The Groove chooser (the arpeggiator spec, R18). It replaced a placeholder
/// at `ParamId(17)`, whose saved value is left out on loading, so an
/// Arpeggiator saved with the placeholder's Swing loads with None.
pub const GROOVE: ParamId = ParamId(20);
/// The root of the scale Distance counts degrees of (the arpeggiator spec,
/// R15). A parameter after the placeholder Groove's, so a device saved
/// before keeps its numbers.
pub const ROOT: ParamId = ParamId(18);
/// Chromatic, which counts Distance in semitones, or a scale of the list,
/// whose degrees Distance counts from Root.
pub const SCALE: ParamId = ParamId(19);

/// The pitch classes the Root chooser offers.
pub const ROOTS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The Style choices.
pub const STYLES: [&str; 18] = bl_midi_tools::STYLE_NAMES;

/// The Retrigger choices.
pub const RETRIGGERS: [&str; 3] = ["Off", "Note", "Beat"];

/// Synced divisions: their names and beat counts (in quarter notes).
pub const DIVISIONS: [(&str, f64); 17] = [
    ("1/32", 0.125),
    ("1/16T", 1.0 / 6.0),
    ("1/16", 0.25),
    ("1/16.", 0.375),
    ("1/8T", 1.0 / 3.0),
    ("1/8", 0.5),
    ("1/8.", 0.75),
    ("1/4T", 2.0 / 3.0),
    ("1/4", 1.0),
    ("1/4.", 1.5),
    ("1/2T", 4.0 / 3.0),
    ("1/2", 2.0),
    ("1/2.", 3.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
    ("8 bars", 32.0),
];

/// Retrigger intervals on the beat grid.
pub const INTERVALS: [(&str, f64); 7] = [
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
];

/// The Groove choices: None, then the bundled swing grooves in order (the
/// grooves spec, R36).
pub const GROOVE_CHOICES: [&str; 11] = [
    "None",
    "Swing 8ths 55",
    "Swing 8ths 60",
    "Swing 8ths 65",
    "Swing 8ths 70",
    "Swing 8ths 75",
    "Swing 16ths 55",
    "Swing 16ths 60",
    "Swing 16ths 65",
    "Swing 16ths 70",
    "Swing 16ths 75",
];

const MAX_HELD: usize = 128;

/// Arpeggiator's device type.
#[must_use]
pub fn arpeggiator_type() -> DeviceType {
    let div_names: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    let interval_names: Vec<&str> = INTERVALS.iter().map(|d| d.0).collect();
    DeviceType::native(ARPEGGIATOR, "Arpeggiator", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::choice(STYLE, "Style", &STYLES, 0)
                .describe("Determines the sequence order of notes in the arpeggiated pattern."),
            ParamDescriptor::toggle(SYNC, "Sync", true)
                .describe("Sets the rate in beat divisions rather than milliseconds."),
            ParamDescriptor::choice(RATE_SYNC, "Rate Sync", &div_names, 5)
                .describe("The arpeggio step duration in beat divisions."),
            ParamDescriptor::continuous(
                RATE_MS,
                "Rate Free",
                (1.0, 2_000.0),
                200.0,
                ParamUnit::Milliseconds,
            )
            .logarithmic()
            .decimals(0)
            .describe("The arpeggio step duration in milliseconds."),
            ParamDescriptor::continuous(GATE, "Gate", (1.0, 200.0), 100.0, ParamUnit::Percent)
                .decimals(0)
                .describe(
                    "Length of each note as a percentage of the step rate; values above 100 % \
                     overlap into the next step.",
                ),
            ParamDescriptor::toggle(HOLD, "Hold", false).describe(
                "Keeps the pattern playing after keys are released. Pressing a note adds or \
                 removes it from the pattern.",
            ),
            ParamDescriptor::integer(OFFSET, "Offset", (0, 64), 0, ParamUnit::None)
                .describe("Shifts the starting position of the note pattern."),
            ParamDescriptor::choice(RETRIGGER, "Retrigger", &RETRIGGERS, 1).describe(
                "When the pattern restarts: Off (never), Note (when a new key is pressed), or \
                 Beat (at beat intervals).",
            ),
            ParamDescriptor::choice(INTERVAL, "Interval", &interval_names, 4).describe(
                "The bar or beat interval on the song grid at which Beat retriggering occurs.",
            ),
            ParamDescriptor::integer(REPEATS, "Repeats", (0, 64), 0, ParamUnit::None).describe(
                "How many times the pattern repeats before stopping; 0 plays indefinitely (∞).",
            ),
            ParamDescriptor::integer(DISTANCE, "Distance", (-48, 48), 12, ParamUnit::Semitones)
                .describe("The transposition interval in semitones per transposition step."),
            ParamDescriptor::integer(STEPS, "Steps", (0, 8), 1, ParamUnit::None).describe(
                "How many times the pattern is transposed before cycling back to the original \
                 pitch.",
            ),
            ParamDescriptor::toggle(VELOCITY_ON, "Velocity", false)
                .describe("Enables velocity decay towards Target over the Decay time."),
            ParamDescriptor::continuous(
                DECAY,
                "Decay",
                (1.0, 60_000.0),
                1_000.0,
                ParamUnit::Milliseconds,
            )
            .logarithmic()
            .decimals(0)
            .describe("Time taken for velocity to ramp towards the Target value."),
            ParamDescriptor::integer(TARGET, "Target", (1, 127), 64, ParamUnit::None)
                .describe("Target velocity reached at the end of the decay time."),
            ParamDescriptor::toggle(RETRIGGER_VELOCITY, "Retrig Vel", true)
                .describe("Restarts the velocity decay slope when the pattern retriggers."),
            ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
                "Counts Distance in degrees of the current scale instead of the Root and Scale \
                 chosen here.",
            ),
            ParamDescriptor::choice(GROOVE, "Groove", &GROOVE_CHOICES, 0).describe(
                "Swings the pattern's timing: None plays it straight, and Swing 8ths and Swing \
                 16ths put each second step of a pair 55 to 75 % of the way through the pair, \
                 moving it as a clip's note would move, scaled by the Global Amount. Needs Sync \
                 on and the song playing.",
            ),
            ParamDescriptor::choice(ROOT, "Root", &ROOTS, 0)
                .describe("The root of the scale Distance counts degrees of."),
            ParamDescriptor::choice(SCALE, "Scale", &scale_choices("Chromatic"), 0).describe(
                "Chromatic counts Distance in semitones; a scale of the list counts it in \
                 degrees of that scale from Root.",
            ),
        ])
        .with_title_switch(USE_SCALE)
        .with_banks(&[
            (
                "Rhythm",
                &[
                    "Style",
                    "Sync",
                    "Rate Sync",
                    "Rate Free",
                    "Gate",
                    "Hold",
                    "Offset",
                    "Retrigger",
                ],
            ),
            (
                "Pattern",
                &["Interval", "Repeats", "Distance", "Steps", "Groove"],
            ),
            ("Velocity", &["Velocity", "Decay", "Target", "Retrig Vel"]),
            ("Scale", &["Root", "Scale"]),
        ]),
    )
}

/// Adds Arpeggiator to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, arpeggiator_type(), |device, _| {
        Arpeggiator::new(device)
    });
}

/// Swing `percent` on a grid of eighths or sixteenths, one 4/4 bar long (the
/// grooves spec, R36), worked out for playing at a Global Amount of 100 %.
fn swing_groove(eighths: bool, percent: i64) -> GroovePlay {
    let base = if eighths {
        GrooveBase::Eighth
    } else {
        GrooveBase::Sixteenth
    };
    let pair = 2 * base.ticks().get();
    let bar = 4 * TICKS_PER_QUARTER;
    let notes = (0..bar / pair)
        .flat_map(|n| [n * pair, n * pair + pair * percent / 100])
        .map(|position| GrooveNote {
            position: Ticks(position),
            velocity: if position % TICKS_PER_QUARTER == 0 {
                127
            } else {
                100
            },
        })
        .collect();
    let mut groove = Groove::new(
        GrooveId(IdAllocator::new().allocate()),
        String::new(),
        Ticks(bar),
        notes,
    );
    groove.base = base;
    GroovePlay::new(&groove, 100.0)
}

/// How many frames a step at song frame `song_frame` is delayed by `groove`
/// (the arpeggiator spec, R19): where a clip's note starting there would
/// play, less where it starts. The grooves are late, never early, so the
/// step waits in the device's queue.
fn groove_delay(groove: &GroovePlay, tempo: &TempoMap, rate: SampleRate, song_frame: i64) -> i64 {
    let start = tempo.frames_to_ticks(Frames(song_frame), rate);
    let moved = Ticks(groove.note_start(start, 0, 0, 0).round() as i64);
    (tempo.ticks_to_frames(moved, rate).0 - tempo.ticks_to_frames(start, rate).0).max(0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HeldNote {
    key: u8,
    channel: u8,
    note_id: Option<u32>,
    velocity: f32,
    order: u32,
    physically_held: bool,
}

/// A lightweight, deterministic linear congruential generator for random styles.
#[derive(Debug, Clone, Copy)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x5851_F42D_4C95_7F2D
            } else {
                seed
            },
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 32) as u32
    }

    fn gen_range(&mut self, max: usize) -> usize {
        if max == 0 {
            0
        } else {
            (self.next_u32() as usize) % max
        }
    }
}

/// What one block's parameters make of the step scheduler.
struct Cfg {
    style: usize,
    /// The step length in ticks while Sync is on and the song plays: steps
    /// sit on the song's grid as the tempo map places it, not on a rounded
    /// frame count.
    grid: Option<i64>,
    step_frames: i64,
    gate_frames: i64,
    offset: usize,
    retrigger_mode: usize,
    interval_ticks: i64,
    repeats: i32,
    distance: i32,
    steps: i32,
    use_scale: bool,
    chosen_scale: Scale,
    vel_on: bool,
    decay_frames: f64,
    target_vel: f32,
    retrigger_vel: bool,
    swing: Option<usize>,
    is_playing: bool,
    /// The router clock and the song position at the block's first frame.
    block_start: i64,
    song_start: i64,
}

impl Cfg {
    fn scale_at(&self, audio: &Audio<'_, '_>, offset: u32) -> Scale {
        if self.use_scale {
            audio.scale_at(offset)
        } else {
            self.chosen_scale
        }
    }
}

/// The largest `k` for which `k * unit` ticks come at or before song frame
/// `frame`.
fn grid_index(tempo: &TempoMap, rate: SampleRate, unit: i64, frame: i64) -> i64 {
    let frame = frame.max(0);
    let mut k = tempo
        .frames_to_ticks(Frames(frame), rate)
        .0
        .div_euclid(unit)
        + 1;
    while tempo.ticks_to_frames(Ticks(k * unit), rate).0 > frame {
        k -= 1;
    }
    k
}

pub struct Arpeggiator {
    router: Router,
    held: Vec<HeldNote>,
    order_counter: u32,
    step_in_pattern: usize,
    transposition_step: i32,
    repeats_counted: i32,
    pattern_len: usize,
    /// The earliest frame on `router.now()` the next step may fire: on the
    /// song's grid the step is the first grid frame from here.
    next_step_frame: i64,
    /// The frame of the last step fired.
    last_step_frame: i64,
    was_grid: bool,
    /// The Interval boundary the last step was in, while Beat retriggers.
    last_interval: Option<i64>,
    /// Clock frame when velocity decay slope started.
    velocity_start_frame: i64,
    initial_velocity: f32,
    rng: Rng,
    random_once_pattern: Vec<usize>,
    last_random_other_idx: Option<usize>,
    random_other_pool: Vec<usize>,
    scratch_notes: Vec<HeldNote>,
    pattern_buf: Vec<usize>,
    scratch_pool: Vec<usize>,
    was_playing: bool,
    was_held_mode: bool,
    /// The ten swing grooves of the Groove chooser after None, in its order.
    grooves: Vec<GroovePlay>,
}

impl Arpeggiator {
    fn new(device: &Device) -> Self {
        Self {
            router: Router::new(),
            held: Vec::with_capacity(MAX_HELD),
            order_counter: 0,
            step_in_pattern: 0,
            transposition_step: 0,
            repeats_counted: 0,
            pattern_len: 0,
            next_step_frame: 0,
            last_step_frame: 0,
            was_grid: false,
            last_interval: None,
            velocity_start_frame: 0,
            initial_velocity: 0.8,
            rng: Rng::new(seed(device)),
            random_once_pattern: Vec::with_capacity(MAX_HELD),
            last_random_other_idx: None,
            random_other_pool: Vec::with_capacity(MAX_HELD),
            scratch_notes: Vec::with_capacity(MAX_HELD),
            pattern_buf: Vec::with_capacity(MAX_HELD * 4),
            scratch_pool: Vec::with_capacity(MAX_HELD),
            was_playing: false,
            was_held_mode: false,
            grooves: (0..10)
                .map(|n| swing_groove(n < 5, 55 + 5 * (n % 5)))
                .collect(),
        }
    }

    fn retrigger(&mut self, now: i64, retrigger_vel: bool) {
        self.step_in_pattern = 0;
        self.transposition_step = 0;
        self.repeats_counted = 0;
        if retrigger_vel {
            self.velocity_start_frame = now;
        }
    }

    fn remove_physically_released(&mut self) {
        self.held.retain(|h| h.physically_held);
    }

    fn handle_note_on(
        &mut self,
        note: &NoteEvent,
        hold: bool,
        now: i64,
        retrigger_mode: usize,
        retrigger_vel: bool,
    ) {
        if hold {
            // In hold mode: if key is already present, toggle it off!
            if let Some(pos) = self
                .held
                .iter()
                .position(|h| h.key == note.key && h.channel == note.channel)
            {
                self.held.remove(pos);
            } else if self.held.len() < MAX_HELD {
                self.order_counter = self.order_counter.wrapping_add(1);
                self.held.push(HeldNote {
                    key: note.key,
                    channel: note.channel,
                    note_id: note.note_id,
                    velocity: note.velocity,
                    order: self.order_counter,
                    physically_held: true,
                });
            }
        } else {
            // Normal mode
            if let Some(h) = self
                .held
                .iter_mut()
                .find(|h| h.key == note.key && h.channel == note.channel)
            {
                h.velocity = note.velocity;
                h.physically_held = true;
                h.note_id = note.note_id;
            } else if self.held.len() < MAX_HELD {
                self.order_counter = self.order_counter.wrapping_add(1);
                self.held.push(HeldNote {
                    key: note.key,
                    channel: note.channel,
                    note_id: note.note_id,
                    velocity: note.velocity,
                    order: self.order_counter,
                    physically_held: true,
                });
            }
        }

        self.initial_velocity = note.velocity;
        self.random_once_pattern.clear();
        self.random_other_pool.clear();

        if retrigger_mode == 1 {
            // Note retrigger
            self.retrigger(now, retrigger_vel);
        }
    }

    fn handle_note_off(&mut self, note: &NoteEvent, hold: bool) {
        if hold {
            // Mark not physically held, but keep in pattern
            if let Some(h) = self
                .held
                .iter_mut()
                .find(|h| h.key == note.key && h.channel == note.channel)
            {
                h.physically_held = false;
            }
        } else {
            self.held
                .retain(|h| !(h.key == note.key && h.channel == note.channel));
            self.random_once_pattern.clear();
            self.random_other_pool.clear();
        }
    }

    /// Builds the pattern indices into `pattern_buf` for `k` held notes
    /// (the style order is `bl_midi_tools::style_order`, which Arpeggiate
    /// shares).
    fn update_pattern(&mut self, style: usize, k: usize) {
        self.pattern_buf.clear();
        if k == 0 {
            return;
        }
        if style == bl_midi_tools::RANDOM_ONCE {
            if self.random_once_pattern.len() != k {
                let rng = &mut self.rng;
                bl_midi_tools::style_order(
                    style,
                    k,
                    &mut self.random_once_pattern,
                    &mut self.scratch_pool,
                    &mut |n| rng.gen_range(n),
                );
            }
            self.pattern_buf
                .extend_from_slice(&self.random_once_pattern);
        } else {
            bl_midi_tools::style_order(
                style,
                k,
                &mut self.pattern_buf,
                &mut self.scratch_pool,
                &mut |_| 0,
            );
        }
    }
}

impl Arpeggiator {
    /// Ends what sounds, drops what waits and rewinds the pattern, `offset`
    /// frames into the stretch: nothing is held any more.
    fn stop(&mut self, audio: &mut Audio<'_, '_>, offset: u32) {
        self.router.cut(audio, offset, |_| true);
        // A step still waiting for its grooved frame never sounds.
        self.router.clear_queue();
        self.step_in_pattern = 0;
        self.transposition_step = 0;
        self.repeats_counted = 0;
        self.last_interval = None;
    }

    /// The router-clock frame of the next step. On the song's grid it is the
    /// first grid frame at or after `next_step_frame`, placed by the tempo
    /// map, so a step lands on the same frame at any block size and tempo
    /// (Spec: arpeggiator R6, R20); otherwise it is `next_step_frame`.
    fn next_step(&self, audio: &Audio<'_, '_>, c: &Cfg) -> i64 {
        let Some(unit) = c.grid else {
            return self.next_step_frame;
        };
        let (tempo, rate) = (audio.tempo(), audio.sample_rate());
        let shift = c.song_start - c.block_start;
        let from = self.next_step_frame + shift;
        let k = grid_index(tempo, rate, unit, from);
        let k = if tempo.ticks_to_frames(Ticks(k * unit), rate).0 < from {
            k + 1
        } else {
            k
        };
        tempo.ticks_to_frames(Ticks(k * unit), rate).0 - shift
    }

    /// Fires every step due before router-clock frame `until`, with what is
    /// held now.
    fn run_steps(&mut self, audio: &mut Audio<'_, '_>, c: &Cfg, until: i64) {
        while !self.held.is_empty() {
            let step_time = self.next_step(audio, c);
            if step_time >= until {
                break;
            }
            self.fire_step(audio, c, step_time);
            self.last_step_frame = step_time;
            self.next_step_frame = if c.grid.is_some() {
                step_time + 1
            } else {
                step_time + c.step_frames
            };
        }
    }

    fn fire_step(&mut self, audio: &mut Audio<'_, '_>, c: &Cfg, step_time: i64) {
        let offset_in_block = (step_time - c.block_start).max(0) as u32;
        // Spec: arpeggiator R20. What waits for its frame and is due
        // before this step goes out first, whatever the block size.
        self.router.advance(audio, offset_in_block);
        let song_frame = c.song_start + i64::from(offset_in_block);
        let delay = c.swing.map_or(0, |n| {
            groove_delay(
                &self.grooves[n],
                audio.tempo(),
                audio.sample_rate(),
                song_frame,
            )
        });

        // Spec: arpeggiator R6. Beat restarts the pattern when a step is the
        // first at or after a boundary of the Interval.
        if c.retrigger_mode == 2 && c.is_playing {
            let idx = grid_index(
                audio.tempo(),
                audio.sample_rate(),
                c.interval_ticks,
                song_frame,
            );
            if self.last_interval.is_some_and(|last| last != idx) {
                self.retrigger(step_time, c.retrigger_vel);
            }
            self.last_interval = Some(idx);
        }

        // Check if repeats limit reached
        if !(c.repeats == 0 || self.repeats_counted < c.repeats) || self.held.is_empty() {
            return;
        }
        // Calculate velocity with decay if enabled
        let vel = if c.vel_on {
            let elapsed = (step_time - self.velocity_start_frame).max(0) as f64;
            let frac = (elapsed / c.decay_frames).clamp(0.0, 1.0) as f32;
            (self.initial_velocity + (c.target_vel - self.initial_velocity) * frac)
                .clamp(1.0 / 127.0, 1.0)
        } else {
            self.initial_velocity
        };

        let channel = self.held[0].channel;
        let scale = c.scale_at(audio, offset_in_block);

        if c.style == 14 {
            // Chord Trigger: play all held notes simultaneously
            let trans = self.transposition_step * c.distance;
            for h in &self.held {
                let final_key = scale.move_off(h.key, trans).clamp(0, 127) as u8;
                let src_key = Key {
                    channel: h.channel,
                    key: h.key,
                    id: h.note_id,
                };
                let note_id = Some(self.router.fresh_id());
                let on = NoteEvent {
                    offset: offset_in_block,
                    kind: NoteKind::On,
                    channel,
                    key: final_key,
                    velocity: vel,
                    note_id,
                };
                let off = NoteEvent {
                    offset: offset_in_block,
                    kind: NoteKind::Off,
                    channel,
                    key: final_key,
                    velocity: 0.5,
                    note_id,
                };
                let dummy_src = NoteEvent {
                    offset: offset_in_block,
                    kind: NoteKind::On,
                    channel: h.channel,
                    key: h.key,
                    velocity: h.velocity,
                    note_id: h.note_id,
                };
                self.router.cut(audio, offset_in_block, |k| {
                    k.key == final_key && k.channel == channel
                });
                if self.router.track(&dummy_src, Some(Key::of(&on))) {
                    self.router
                        .sound(audio, on, off, src_key, step_time + delay, c.gate_frames);
                }
            }

            // Advance transposition and repeats
            self.transposition_step += 1;
            if self.transposition_step > c.steps {
                self.transposition_step = 0;
                self.repeats_counted += 1;
            }
        } else {
            // Single note per step
            // Sort notes based on style into scratch_notes
            self.scratch_notes.clear();
            self.scratch_notes.extend_from_slice(&self.held);
            if c.style == 13 {
                // Play Order
                self.scratch_notes.sort_by_key(|n| n.order);
            } else {
                self.scratch_notes.sort_by_key(|n| n.key);
            }
            let k = self.scratch_notes.len();

            let picked_idx = if c.style == 15 {
                // Random
                self.rng.gen_range(k)
            } else if c.style == 16 {
                // Random Other
                if self.random_other_pool.is_empty() {
                    for i in 0..k {
                        self.random_other_pool.push(i);
                    }
                    // Shuffle
                    for i in (1..self.random_other_pool.len()).rev() {
                        let j = self.rng.gen_range(i + 1);
                        self.random_other_pool.swap(i, j);
                    }
                    // Avoid repeating previous last
                    if self.random_other_pool.len() > 1
                        && self.random_other_pool.last().copied() == self.last_random_other_idx
                    {
                        let last_idx = self.random_other_pool.len() - 1;
                        self.random_other_pool.swap(0, last_idx);
                    }
                }
                let p = self.random_other_pool.pop().unwrap_or(0);
                self.last_random_other_idx = Some(p);
                p
            } else {
                self.update_pattern(c.style, k);
                self.pattern_len = self.pattern_buf.len();
                if self.pattern_buf.is_empty() {
                    0
                } else {
                    let eff = (self.step_in_pattern + c.offset) % self.pattern_buf.len();
                    self.pattern_buf[eff]
                }
            };

            let chosen = self.scratch_notes[picked_idx.min(self.scratch_notes.len() - 1)];
            let trans = self.transposition_step * c.distance;
            let final_key = scale.move_off(chosen.key, trans).clamp(0, 127) as u8;

            let src_key = Key {
                channel: chosen.channel,
                key: chosen.key,
                id: chosen.note_id,
            };
            let note_id = Some(self.router.fresh_id());
            let on = NoteEvent {
                offset: offset_in_block,
                kind: NoteKind::On,
                channel,
                key: final_key,
                velocity: vel,
                note_id,
            };
            let off = NoteEvent {
                offset: offset_in_block,
                kind: NoteKind::Off,
                channel,
                key: final_key,
                velocity: 0.5,
                note_id,
            };
            let dummy_src = NoteEvent {
                offset: offset_in_block,
                kind: NoteKind::On,
                channel: chosen.channel,
                key: chosen.key,
                velocity: chosen.velocity,
                note_id: chosen.note_id,
            };

            self.router.cut(audio, offset_in_block, |k| {
                k.key == final_key && k.channel == channel
            });
            if self.router.track(&dummy_src, Some(Key::of(&on))) {
                self.router
                    .sound(audio, on, off, src_key, step_time + delay, c.gate_frames);
            }

            // Advance pattern step
            if c.style != 15 && c.style != 16 {
                self.step_in_pattern += 1;
                if self.pattern_len > 0 && self.step_in_pattern >= self.pattern_len {
                    self.step_in_pattern = 0;
                    self.transposition_step += 1;
                    if self.transposition_step > c.steps {
                        self.transposition_step = 0;
                        self.repeats_counted += 1;
                    }
                }
            }
        }
    }
}

impl NativeDevice for Arpeggiator {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let sample_rate = audio.sample_rate().as_f64();
        let bpm = audio.bpm();

        let hold_mode = value(params, HOLD) >= 0.5;
        if self.was_held_mode && !hold_mode {
            self.remove_physically_released();
        }
        self.was_held_mode = hold_mode;

        let is_sync = value(params, SYNC) >= 0.5;
        let is_playing = audio.playing();
        let (step_frames, step_ticks) = if is_sync {
            let div_idx = (value(params, RATE_SYNC) as usize).min(DIVISIONS.len() - 1);
            let beats = DIVISIONS[div_idx].1;
            (
                (beats * 60.0 / bpm * sample_rate).round().max(1.0) as i64,
                (beats * TICKS_PER_QUARTER as f64).round() as i64,
            )
        } else {
            (
                (value(params, RATE_MS) * sample_rate / 1000.0)
                    .round()
                    .max(1.0) as i64,
                0,
            )
        };

        let gate = (value(params, GATE) / 100.0).clamp(0.01, 2.0);
        let interval_idx = (value(params, INTERVAL) as usize).min(INTERVALS.len() - 1);
        let decay_ms = value(params, DECAY);
        let block_start = self.router.now();
        let song_start = audio.song_position().0;

        // Spec: arpeggiator R19. A groove moves steps only while the song
        // plays on its grid, with Sync on.
        let swing = (is_sync && is_playing)
            .then(|| (value(params, GROOVE) as usize).checked_sub(1))
            .flatten()
            .filter(|n| *n < self.grooves.len());
        if let Some(n) = swing {
            self.grooves[n].set_global_amount(f64::from(audio.groove_amount()) * 100.0);
        }

        let c = Cfg {
            style: value(params, STYLE) as usize,
            grid: (is_sync && is_playing).then_some(step_ticks),
            step_frames,
            gate_frames: ((step_frames as f64) * gate).round().max(1.0) as i64,
            offset: value(params, OFFSET) as usize,
            retrigger_mode: value(params, RETRIGGER) as usize,
            interval_ticks: (INTERVALS[interval_idx].1 * TICKS_PER_QUARTER as f64).round() as i64,
            repeats: value(params, REPEATS) as i32,
            distance: value(params, DISTANCE) as i32,
            steps: value(params, STEPS) as i32,
            // Spec: arpeggiator R15, R16. Distance counts degrees of the
            // current scale, or of the Root and Scale chosen here, or
            // semitones for Chromatic.
            use_scale: use_current_scale(audio, params, USE_SCALE),
            chosen_scale: scale_named(value(params, SCALE) as usize)
                .map_or(OFF, |name| Scale::new(value(params, ROOT) as u8, name)),
            vel_on: value(params, VELOCITY_ON) >= 0.5,
            decay_frames: (decay_ms * sample_rate / 1000.0).max(1.0),
            target_vel: (value(params, TARGET) as f32 / 127.0).clamp(1.0 / 127.0, 1.0),
            retrigger_vel: value(params, RETRIGGER_VELOCITY) >= 0.5,
            swing,
            is_playing,
            block_start,
            song_start,
        };

        // Steps stay in the phase they had when they leave the song's grid.
        if self.was_grid && c.grid.is_none() {
            self.next_step_frame = (self.last_step_frame + step_frames).max(self.next_step_frame);
        }
        self.was_grid = c.grid.is_some();
        // A transport start puts the next step on the song's grid.
        if is_playing && !self.was_playing {
            self.next_step_frame = block_start;
        }
        self.was_playing = is_playing;
        if !is_playing {
            self.last_interval = None;
        }

        // Nothing held (Hold turned off, say): end what still sounds.
        if self.held.is_empty() {
            self.stop(audio, 0);
        }

        // Spec: arpeggiator R20. Each input note is applied on its own frame:
        // the steps before it play with what was held until then, and the
        // note ends or starts a pattern there.
        for note in audio.notes() {
            let note_frame = block_start + i64::from(note.offset);
            self.run_steps(audio, &c, note_frame);
            self.router.advance(audio, note.offset);

            match note.kind {
                NoteKind::On => {
                    let was_empty = self.held.is_empty();
                    self.handle_note_on(
                        &note,
                        hold_mode,
                        note_frame,
                        c.retrigger_mode,
                        c.retrigger_vel,
                    );
                    if was_empty && !self.held.is_empty() {
                        self.next_step_frame = note_frame;
                    }
                }
                NoteKind::Off | NoteKind::Choke => {
                    self.handle_note_off(&note, hold_mode);
                }
                NoteKind::Expression { .. } => {
                    // Forward expression to router; a bend stays in the
                    // scale (R17).
                    let scale = c.scale_at(audio, note.offset);
                    self.router
                        .expression_by(audio, &note, |from, to, bend| scale.bend(from, to, bend));
                }
                _ => {
                    audio.send(note);
                }
            }
            // If no notes are held, cut sounding arpeggiated notes if hold is
            // off, at the frame the last key went up.
            if self.held.is_empty() {
                self.stop(audio, note.offset);
            }
        }

        self.run_steps(audio, &c, block_start + audio.frames() as i64);
        self.router.finish(audio);
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        self.router.release(out);
        self.held.clear();
        self.last_interval = None;
        self.random_once_pattern.clear();
        self.random_other_pool.clear();
        self.scratch_notes.clear();
        self.pattern_buf.clear();
        self.scratch_pool.clear();
    }

    fn dropped_notes(&self) -> u64 {
        self.router.dropped()
    }
}
