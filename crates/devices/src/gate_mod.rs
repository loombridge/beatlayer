//! The Gate Mod (WP-5.11; the modulation spec, R25): Beatlayer's equivalent
//! of an Envelope MIDI device. A MIDI effect that passes its notes through
//! untouched and turns them into an ADSR envelope for the parameters it
//! maps.
//!
//! The laws are Beatlayer's own. A note starts the envelope from the level
//! it is at: Attack runs to the peak (1, or the note's velocity with Velocity
//! on), Decay to Sustain's share of the peak, and the last held key's end
//! starts Release from where the envelope is. With Sustain off the envelope
//! runs Attack and Decay and then Release whatever the note does. A segment's
//! slope bends it by the automation law (`curve_value`): a positive slope
//! bows it below the straight line. Global Time scales every time.
//!
//! Loop Mode: Free starts the envelope on each note; Sync starts it again on
//! every Sync Rate division while a key is held; Loop runs Attack, Decay and
//! Release over and over while a key is held, with no Sustain, so its cycle
//! is the three times together; Echo starts it again every Echo Time, each
//! repeat's peak Echo Feedback's share of the last, until it is under a
//! 256th, whether or not the key is still held.

use bl_engine::{DeviceType, NoteEvent, NoteKind, Registry};
use bl_model::automation::curve_value;
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DeviceLayout, DisplayCell, LayoutNode, Widget,
};
use bl_model::modulator::{GATE_MOD, SLOTS};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, DisplayReading, DisplaySeries, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, Modulation, NativeDevice, ParamBank, add};
use crate::modulator::{DIVISIONS, mapping_cells, mapping_params};
use crate::signal_mod::{Held, Indices, SignalOut};

/// Attack, Decay, Sustain and Release.
pub const ATTACK: ParamId = ParamId(0);
pub const DECAY: ParamId = ParamId(1);
pub const SUSTAIN_LEVEL: ParamId = ParamId(2);
pub const RELEASE: ParamId = ParamId(3);
/// The slopes of Attack, Decay and Release.
pub const ATTACK_SLOPE: ParamId = ParamId(4);
pub const DECAY_SLOPE: ParamId = ParamId(5);
pub const RELEASE_SLOPE: ParamId = ParamId(6);
/// The Sustain switch: hold the Sustain level while a key is held.
pub const SUSTAIN_ON: ParamId = ParamId(7);
/// The Velocity switch: scale the peak by the note's velocity.
pub const VELOCITY: ParamId = ParamId(8);
/// Loop Mode.
pub const LOOP_MODE: ParamId = ParamId(9);
/// The division Sync starts the envelope again on.
pub const SYNC_RATE: ParamId = ParamId(10);
/// Global Time: scales every time.
pub const GLOBAL_TIME: ParamId = ParamId(11);
/// Env Echo Time, the time between echoes.
pub const ECHO_TIME: ParamId = ParamId(12);
/// Env Feedback, how much of each echo the next keeps.
pub const ECHO_FEEDBACK: ParamId = ParamId(13);

/// The Loop Mode choices.
pub const LOOP_MODES: [&str; 4] = ["Free", "Sync", "Loop", "Echo"];

/// The peak under which echoes stop.
const ECHO_FLOOR: f64 = 1.0 / 256.0;

/// The Gate Mod's type.
#[must_use]
pub fn gate_mod_type() -> DeviceType {
    let names: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    let time = |id, name: &str, default, what: &str| {
        ParamDescriptor::continuous(id, name, (1.0, 10_000.0), default, ParamUnit::Milliseconds)
            .logarithmic()
            .decimals(0)
            .describe(what)
    };
    let slope = |id, name: &str, what: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe(what)
    };
    let mut params = vec![
        time(ATTACK, "Attack", 10.0, "The time the envelope takes to rise to its peak."),
        time(DECAY, "Decay", 300.0, "The time the envelope takes to fall to the Sustain level."),
        ParamDescriptor::continuous(
            SUSTAIN_LEVEL,
            "Sustain Level",
            (0.0, 100.0),
            60.0,
            ParamUnit::Percent,
        )
        .decimals(0)
        .describe("The share of the peak the envelope holds while a key is held."),
        time(RELEASE, "Release", 300.0, "The time the envelope takes to fall to nothing."),
        slope(ATTACK_SLOPE, "Attack Slope", "Bends the attack: positive holds it back, negative brings it forward."),
        slope(DECAY_SLOPE, "Decay Slope", "Bends the decay."),
        slope(RELEASE_SLOPE, "Release Slope", "Bends the release."),
        ParamDescriptor::toggle(SUSTAIN_ON, "Sustain", true).describe(
            "On, the envelope holds the Sustain level while a key is held; off, it releases after its decay.",
        ),
        ParamDescriptor::toggle(VELOCITY, "Velocity", false)
            .describe("Scales the envelope's peak by the note's velocity."),
        ParamDescriptor::choice(LOOP_MODE, "Loop Mode", &LOOP_MODES, 0).describe(
            "Free starts the envelope on each note, Sync again on every Sync Rate division while a key is held, Loop runs it over and over while a key is held, and Echo repeats it every Env Echo Time.",
        ),
        ParamDescriptor::choice(SYNC_RATE, "Sync Rate", &names, 3)
            .describe("The beat division Sync starts the envelope again on."),
        ParamDescriptor::continuous(GLOBAL_TIME, "Global Time", (0.1, 10.0), 1.0, ParamUnit::None)
            .logarithmic()
            .decimals(2)
            .describe("Scales every time of the envelope; 1.00 leaves them as set."),
        ParamDescriptor::continuous(
            ECHO_TIME,
            "Env Echo Time",
            (10.0, 2_000.0),
            250.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .decimals(0)
        .describe("The time between echoes in Echo mode."),
        ParamDescriptor::continuous(
            ECHO_FEEDBACK,
            "Env Feedback",
            (0.0, 100.0),
            50.0,
            ParamUnit::Percent,
        )
        .decimals(0)
        .describe("How much of each echo the next one keeps, in Echo mode."),
    ];
    params.extend(mapping_params(
        SLOTS,
        (
            "Amount",
            "How far the modulator reaches: 0 % none, 100 % full.",
        ),
    ));
    DeviceType::native(GATE_MOD, "Gate Mod", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

fn layout() -> DeviceLayout {
    let times = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (ATTACK, Widget::Knob),
            (DECAY, Widget::Knob),
            (SUSTAIN_LEVEL, Widget::Knob),
            (RELEASE, Widget::Knob),
            (ATTACK_SLOPE, Widget::Knob),
            (DECAY_SLOPE, Widget::Knob),
            (RELEASE_SLOPE, Widget::Knob),
        ],
    );
    let modes = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (SUSTAIN_ON, Widget::Toggle),
            (VELOCITY, Widget::Toggle),
            (LOOP_MODE, Widget::Chooser),
            (SYNC_RATE, Widget::Chooser),
            (GLOBAL_TIME, Widget::Knob),
            (ECHO_TIME, Widget::Knob),
            (ECHO_FEEDBACK, Widget::Knob),
        ],
    );
    let output = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (bl_model::modulator::MODE, Widget::Chooser),
            (bl_model::modulator::POLARITY, Widget::Chooser),
            (bl_model::modulator::DEPTH, Widget::Knob),
        ],
    );
    let (first, rest) = mapping_cells(SLOTS);
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(times),
        LayoutNode::Cell(DisplayCell::new("Envelope", 150.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(modes),
        first,
        LayoutNode::Band(output),
    ]))
    .with_expanded(rest)
}

/// Adds the Gate Mod to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, gate_mod_type(), |_, config| {
        GateMod::new(config.sample_rate.as_f64())
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

struct GateMod {
    out: SignalOut,
    rate: f64,
    held: Held,
    stage: Stage,
    /// How far through the stage, from 0 to 1.
    progress: f64,
    /// The level the stage started from, and the one now.
    from: f64,
    level: f64,
    /// The peak the envelope runs to.
    peak: f64,
    /// Frames since Sync last started the envelope.
    sync_clock: f64,
    /// The next echo's peak, while echoes run, and frames since the last.
    echo: Option<f64>,
    echo_clock: f64,
    /// Whether the envelope started since the display last read.
    flashed: bool,
}

/// The settings of a frame.
struct Settings {
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
    bends: [f64; 3],
    hold: bool,
    velocity: bool,
    mode: usize,
    global: f64,
    echo_frames: f64,
    feedback: f64,
}

impl GateMod {
    fn new(rate: f64) -> Self {
        Self {
            out: SignalOut::new(),
            rate,
            held: Held::new(),
            stage: Stage::Idle,
            progress: 0.0,
            from: 0.0,
            level: 0.0,
            peak: 1.0,
            sync_clock: 0.0,
            echo: None,
            echo_clock: 0.0,
            flashed: false,
        }
    }

    /// Starts the envelope from the level it is at, towards `peak`.
    fn start(&mut self, peak: f64) {
        self.stage = Stage::Attack;
        self.progress = 0.0;
        self.from = self.level;
        self.peak = peak;
        self.flashed = true;
    }

    /// Starts the release from where the envelope is.
    fn begin_release(&mut self) {
        if self.stage != Stage::Idle && self.stage != Stage::Release {
            self.stage = Stage::Release;
            self.progress = 0.0;
            self.from = self.level;
        }
    }

    fn note(&mut self, note: &NoteEvent, settings: &Settings) {
        match note.kind {
            NoteKind::On => {
                self.held.on(note);
                let peak = if settings.velocity {
                    f64::from(note.velocity)
                } else {
                    1.0
                };
                self.start(peak);
                self.sync_clock = 0.0;
                // Spec: modulation R25. A note starts the echoes again.
                self.echo = (settings.mode == 3).then_some(peak * settings.feedback);
                self.echo_clock = 0.0;
            }
            NoteKind::Off | NoteKind::Choke => {
                // A key that went down while it was switched off is not ours.
                let ours = self.held.off(note);
                if ours && self.held.is_empty() && (settings.hold || settings.mode == 2) {
                    self.begin_release();
                }
            }
            _ => {}
        }
    }

    /// Runs the envelope one frame on.
    fn step(&mut self, settings: &Settings, sync_frames: f64) {
        let held = !self.held.is_empty();
        // Sync and Echo start the envelope again.
        if settings.mode == 1 && held {
            self.sync_clock += 1.0;
            if self.sync_clock >= sync_frames {
                self.sync_clock = 0.0;
                self.start(self.peak);
            }
        }
        if let Some(peak) = self.echo {
            self.echo_clock += 1.0;
            if self.echo_clock >= settings.echo_frames {
                self.echo_clock = 0.0;
                if peak > ECHO_FLOOR {
                    self.start(peak);
                    self.echo = Some(peak * settings.feedback);
                } else {
                    self.echo = None;
                }
            }
        }
        let rate = self.rate;
        let frames = |ms: f64| (ms * settings.global * rate / 1000.0).max(1.0);
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.progress += 1.0 / frames(settings.attack);
                let t = self.progress.min(1.0);
                self.level = curve_value(self.from, self.peak, t, settings.bends[0]);
                if self.progress >= 1.0 {
                    self.stage = Stage::Decay;
                    self.progress = 0.0;
                }
            }
            Stage::Decay => {
                self.progress += 1.0 / frames(settings.decay);
                let t = self.progress.min(1.0);
                let target = settings.sustain * self.peak;
                self.level = curve_value(self.peak, target, t, settings.bends[1]);
                if self.progress >= 1.0 {
                    if settings.hold && settings.mode != 2 && held {
                        self.stage = Stage::Sustain;
                    } else {
                        // No key held, no Sustain, or Loop: on to Release.
                        self.stage = Stage::Decay;
                        self.begin_release();
                    }
                }
            }
            Stage::Sustain => {
                self.level = settings.sustain * self.peak;
                if !held {
                    self.begin_release();
                }
            }
            Stage::Release => {
                self.progress += 1.0 / frames(settings.release);
                let t = self.progress.min(1.0);
                self.level = curve_value(self.from, 0.0, t, settings.bends[2]);
                if self.progress >= 1.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                    // Loop: round again while a key is held.
                    if settings.mode == 2 && held {
                        self.start(self.peak);
                    }
                }
            }
        }
    }
}

impl NativeDevice for GateMod {
    fn is_signal_modulator(&self) -> bool {
        true
    }

    fn lag(&mut self, modulation: &mut Modulation<'_, '_>) {
        self.out.lag(modulation);
    }

    fn release(&mut self, _: &mut bl_engine::NoteBuffer) {
        // Spec: device-framework R32. Switched off, it forgets the keys.
        self.held.clear();
        self.begin_release();
    }

    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if self.out.begin(audio.playing()) && self.held.is_empty() {
            // Spec: modulation R20. It begins as a render from here does,
            // unless a key is already down.
            self.stage = Stage::Idle;
            self.progress = 0.0;
            self.from = 0.0;
            self.level = 0.0;
            self.peak = 1.0;
            self.sync_clock = 0.0;
            self.echo = None;
            self.echo_clock = 0.0;
        }
        let index = |id: ParamId| params.index(id).expect("the description holds it");
        let at = |id: ParamId, f: usize| f64::from(params.ramp(index(id))[f]);
        let tempo = audio.tempo();
        let bpm = tempo
            .bpm_at(tempo.frames_to_ticks(audio.song_position(), audio.sample_rate()))
            .get();
        let ix = Indices::new(params, SLOTS);
        let mut notes = audio.notes().peekable();
        for f in 0..frames {
            let settings = Settings {
                attack: at(ATTACK, f),
                decay: at(DECAY, f),
                sustain: at(SUSTAIN_LEVEL, f) / 100.0,
                release: at(RELEASE, f),
                bends: [
                    at(ATTACK_SLOPE, f) / 100.0,
                    at(DECAY_SLOPE, f) / 100.0,
                    at(RELEASE_SLOPE, f) / 100.0,
                ],
                hold: at(SUSTAIN_ON, f) >= 0.5,
                velocity: at(VELOCITY, f) >= 0.5,
                mode: at(LOOP_MODE, f) as usize,
                global: at(GLOBAL_TIME, f),
                echo_frames: (at(ECHO_TIME, f) * self.rate / 1000.0).max(1.0),
                feedback: at(ECHO_FEEDBACK, f) / 100.0,
            };
            while let Some(note) = notes.next_if(|n| n.offset as usize == f) {
                audio.send(note);
                self.note(&note, &settings);
            }
            let division = DIVISIONS[(at(SYNC_RATE, f) as usize).min(DIVISIONS.len() - 1)].1;
            self.step(&settings, division * 60.0 / bpm * self.rate);
            self.out
                .frame(audio, &ix, params, f, &[self.level.clamp(0.0, 1.0)]);
        }
    }

    fn display_len(&self) -> usize {
        2
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.level as f32;
        out[1] = f32::from(u8::from(self.flashed));
        self.flashed = false;
    }
}

/// The Gate Mod's display (R25): the envelope as its controls set it, with
/// a held key's sustain drawn as long as the attack and decay together, and
/// its level now and trigger LED from what its node published.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    const POINTS: usize = 96;
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let global = value(GLOBAL_TIME);
    let (attack, decay, release) = (
        value(ATTACK) * global,
        value(DECAY) * global,
        value(RELEASE) * global,
    );
    let hold = if value(SUSTAIN_ON) >= 0.5 {
        (attack + decay) * 0.5
    } else {
        0.0
    };
    let total = attack + decay + hold + release;
    let sustain = value(SUSTAIN_LEVEL) / 100.0;
    let bends = [
        value(ATTACK_SLOPE) / 100.0,
        value(DECAY_SLOPE) / 100.0,
        value(RELEASE_SLOPE) / 100.0,
    ];
    let samples = (0..=POINTS)
        .map(|i| {
            let t = total * i as f64 / POINTS as f64;
            let y = if t < attack {
                curve_value(0.0, 1.0, t / attack, bends[0])
            } else if t < attack + decay {
                curve_value(1.0, sustain, (t - attack) / decay, bends[1])
            } else if t < attack + decay + hold {
                sustain
            } else {
                curve_value(
                    sustain,
                    0.0,
                    (t - attack - decay - hold) / release,
                    bends[2],
                )
            };
            y.clamp(0.0, 1.0) as f32
        })
        .collect();
    let mut out = DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: "Envelope".to_owned(),
            unit: String::new(),
            x_unit: "ms".to_owned(),
            x_range: [0.0, total as f32],
            log_x: false,
            y_range: [0.0, 1.0],
            detail: false,
            values: samples,
        }],
        markers: Vec::new(),
        readings: Vec::new(),
    };
    if let Some([level, trigger]) = data.and_then(|d| <[f32; 2]>::try_from(d).ok()) {
        out.readings = vec![
            DisplayReading {
                name: "Level".to_owned(),
                value: level * 100.0,
                unit: "%".to_owned(),
                text: format!("{:.0} %", level * 100.0),
            },
            DisplayReading {
                name: "Trigger".to_owned(),
                value: trigger,
                unit: String::new(),
                text: if trigger > 0.5 { "on" } else { "off" }.to_owned(),
            },
        ];
    }
    out
}
