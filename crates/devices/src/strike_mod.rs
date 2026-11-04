//! The Shaper MIDI (WP-5.11; the modulation spec, R26): Beatlayer's equivalent
//! of a Shaper MIDI device. A MIDI effect that passes its notes through
//! untouched and plays a breakpoint shape, edited as the Shaper's, each
//! time a note starts.
//!
//! The laws are Beatlayer's own. A note starts the shape from its start and
//! it runs through one cycle at Rate (a length in Hz, or a beat division of
//! the song's tempo); a shape with a sustain breakpoint holds there while a
//! key is held and goes on to its end when the last one comes up. With Loop
//! on, the shape starts again at its end while a key is held, and the
//! sustain breakpoint is passed over. The output rests at the shape's start
//! before the first note and at its end after each one. Velocity scales the
//! output by `1 − v + v · velocity`, `v` being Velocity as a share, so 0 %
//! leaves velocity out and 100 % makes the output the velocity. Echo starts
//! the shape again every Time, each repeat scaled by Echo's share of the
//! last, until it is under a 256th.

use bl_engine::modbus::Shape;
use bl_engine::{DeviceType, NodeData, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{
    Control, ControlBand, DeviceLayout, DisplayCell, LayoutNode, TitlePart, Unit, Widget,
};
use bl_model::modulator::{
    DEPTH, GRID_DIVISIONS, GRIDS, MAX_POINTS, SLOTS, SNAP, STRIKE_MOD, ShapePoint,
    default_shape_of, shape_value,
};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, DisplayReading, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, Modulation, NativeDevice, ParamBank, add};
use crate::midi_fx::seed;
use crate::modulator::{
    DIVISIONS, JITTER, OFFSET, RATE_BEATS, SMOOTH, TIME_MODE, mapped_layout, mapping_params,
    mapping_row, rate_label,
};
use crate::signal_mod::{Held, Indices, Post, SignalOut};

/// Loop: the shape starts again at its end while a key is held.
pub const LOOP: ParamId = ParamId(10);
/// Velocity: how much the note's velocity scales the output.
pub const VELOCITY: ParamId = ParamId(11);
/// Time: the time between echoes.
pub const ECHO_TIME: ParamId = ParamId(15);
/// Echo: how much of each echo the next keeps.
pub const ECHO: ParamId = ParamId(16);

/// T: turning it on plays the shape once, as a note does.
pub const TRIGGER: ParamId = ParamId(18);
/// Rate, as the length of a cycle in milliseconds.
pub const RATE_MS: ParamId = ParamId(17);

/// The peak under which echoes stop.
const ECHO_FLOOR: f64 = 1.0 / 256.0;

/// The Shaper MIDI's type.
#[must_use]
pub fn strike_mod_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::toggle(LOOP, "Loop", false).describe(
            "On, the shape starts again at its end while a key is held, past its sustain breakpoint.",
        ),
        ParamDescriptor::continuous(VELOCITY, "Velocity", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("0 % leaves the note's velocity out; higher makes soft notes give less and hard ones more."),
        ParamDescriptor::continuous(JITTER, "Jitter", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Adds randomness to the output."),
        ParamDescriptor::continuous(SMOOTH, "Smooth", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Softens sharp changes in the output."),
        ParamDescriptor::choice(TIME_MODE, "Time Mode", &["ms", "Beats"], 1)
            .describe("Sets the rate as a length in milliseconds or in beat divisions of the song's tempo."),
        ParamDescriptor::continuous(RATE_MS, "Rate", (25.0, 100_000.0), 1_000.0, ParamUnit::Milliseconds)
            .logarithmic()
            .decimals(0)
            .describe("The time the shape takes to play through once."),
        ParamDescriptor::choice(
            RATE_BEATS,
            "Beat Rate",
            &DIVISIONS.map(|(name, _)| name),
            3,
        )
        .describe("The length of one cycle, in beat divisions of the song's tempo."),
        ParamDescriptor::continuous(OFFSET, "Offset", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Moves the output's centre; 0 % leaves it centred. The output stays within 0 to 1."),
        ParamDescriptor::continuous(
            ECHO_TIME,
            "Time",
            (10.0, 2_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .decimals(0)
        .describe("The time between echoes."),
        ParamDescriptor::continuous(ECHO, "Echo", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(2)
            .describe("How much of each echo the next one keeps; 0 % makes none."),
        ParamDescriptor::toggle(TRIGGER, "T", false)
            .describe("Turning it on plays the shape from its start, as a note does."),
        ParamDescriptor::choice(GRID_DIVISIONS, "Grid", &GRIDS.map(|(name, _)| name), 1)
            .describe("The display's divisions of the cycle."),
        ParamDescriptor::toggle(SNAP, "Snap", true)
            .describe("Snaps breakpoints to the grid as they are placed and dragged."),
    ];
    params.extend(mapping_params(
        SLOTS,
        (
            "Depth",
            "How far the modulator reaches: 0 % none, 100 % full.",
        ),
    ));
    DeviceType::native(STRIKE_MOD, "Shaper MIDI", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(params)
            .with_alternate_at((TIME_MODE, 1), RATE_MS, RATE_BEATS)
            .with_relabels(vec![rate_label(RATE_BEATS, TIME_MODE)])
            .with_layout(layout())
            .with_banks(&[
                (
                    "Strike",
                    &[
                        "Loop", "Velocity", "Grid", "Snap", "Time", "Echo", "Jitter", "Smooth",
                    ],
                ),
                ("Timing", &["Time Mode", "Rate", "Beat Rate", "Offset", "T"]),
                ("Mapping", &["Mod", "Polarity", "Depth"]),
                (
                    "Targets 1-2",
                    &["Amount 1", "Min 1", "Max 1", "Amount 2", "Min 2", "Max 2"],
                ),
                (
                    "Targets 3-4",
                    &["Amount 3", "Min 3", "Max 3", "Amount 4", "Min 4", "Max 4"],
                ),
                (
                    "Targets 5-6",
                    &["Amount 5", "Min 5", "Max 5", "Amount 6", "Min 6", "Max 6"],
                ),
                (
                    "Targets 7-8",
                    &["Amount 7", "Min 7", "Max 7", "Amount 8", "Min 8", "Max 8"],
                ),
            ]),
    )
}

/// The panel as Live draws it (layout L-DEV-021): the shape display with the
/// mapping row along its top and Grid and Snap along its foot; at the right
/// the output display with T, Jitter and Smooth inside, Velocity, Offset and
/// Loop, Rate with its ms and sync buttons beside Depth, and Echo over Time.
fn layout() -> DeviceLayout {
    let at = Control::new;
    let curve = DisplayCell::new("Curve", 290.0, 150.0, 3.0)
        .with_inset(4.0)
        .with_top(mapping_row())
        .with_bottom(ControlBand::free(
            290.0,
            20.0,
            vec![
                at(GRID_DIVISIONS, Widget::Inline, 207.0, 4.0).sized(46.0, 12.0),
                at(SNAP, Widget::Toggle, 259.0, 4.0).sized(28.0, 12.0),
            ],
        ));
    let output = DisplayCell::new("Output", 146.0, 66.0, 1.0)
        .with_inset(4.0)
        .with_bottom(ControlBand::free(
            146.0,
            24.0,
            vec![
                at(TRIGGER, Widget::Round, 4.0, 6.0),
                at(JITTER, Widget::Inline, 22.0, 6.0).sized(60.0, 12.0),
                at(SMOOTH, Widget::Inline, 86.0, 6.0).sized(58.0, 12.0),
            ],
        ));
    let controls = ControlBand::free(
        154.0,
        84.0,
        vec![
            at(VELOCITY, Widget::Number, 5.0, 3.0).sized(40.0, 21.0),
            at(OFFSET, Widget::Number, 56.0, 3.0).sized(40.0, 21.0),
            at(LOOP, Widget::Toggle, 110.0, 12.0).sized(38.0, 12.0),
            at(RATE_MS, Widget::SmallKnob, 8.0, 30.0)
                .with_unit_right(TIME_MODE, [Unit::Ms, Unit::Sync]),
            at(DEPTH, Widget::SmallKnob, 67.0, 30.0),
            at(ECHO, Widget::Number, 110.0, 30.0).sized(40.0, 21.0),
            at(ECHO_TIME, Widget::Number, 110.0, 58.0).sized(40.0, 21.0),
        ],
    );
    mapped_layout(LayoutNode::Row(vec![
        LayoutNode::Cell(curve),
        LayoutNode::Column(vec![LayoutNode::Cell(output), LayoutNode::Band(controls)]),
    ]))
    .with_title(vec![TitlePart::Mpe])
}

/// Adds the Shaper MIDI to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, strike_mod_type(), |device, config| {
        StrikeMod::new(
            device
                .shape
                .clone()
                .unwrap_or_else(|| default_shape_of(&device.kind)),
            config.sample_rate.as_f64(),
            seed(device),
        )
    });
}

struct StrikeMod {
    out: SignalOut,
    post: Post,
    rate: f64,
    points: Vec<ShapePoint>,
    held: Held,
    /// Where the shape is, from 0 to 1.
    x: f64,
    running: bool,
    /// The velocity's and the echo's share of the output.
    scale: f64,
    /// The next echo's scale, while echoes run, and frames since the last.
    echo: Option<f64>,
    echo_clock: f64,
    /// The velocity of the key that started the shape, as a share.
    velocity: f64,
    flashed: bool,
    level: f64,
    /// Whether T was on at the last frame.
    triggered: bool,
}

impl StrikeMod {
    fn new(mut points: Vec<ShapePoint>, rate: f64, seed: u64) -> Self {
        // Room for the longest shape, so a new one arriving on the audio
        // thread never allocates (R20).
        points.reserve(MAX_POINTS.saturating_sub(points.len()));
        Self {
            out: SignalOut::new(),
            post: Post::new(seed),
            rate,
            points,
            held: Held::new(),
            x: 0.0,
            running: false,
            scale: 1.0,
            echo: None,
            echo_clock: 0.0,
            velocity: 1.0,
            flashed: false,
            level: 0.0,
            triggered: false,
        }
    }

    fn start(&mut self, scale: f64) {
        self.x = 0.0;
        self.running = true;
        self.scale = scale;
        self.flashed = true;
    }

    /// The x of the sustain breakpoint, if the shape has one.
    fn sustain_at(&self) -> Option<f64> {
        self.points.iter().find(|p| p.sustain).map(|p| p.x)
    }
}

/// The settings of a frame.
struct Settings {
    looped: bool,
    velocity: f64,
    echo: f64,
    echo_frames: f64,
    /// Frames in a cycle.
    cycle: f64,
}

impl NativeDevice for StrikeMod {
    fn is_signal_modulator(&self) -> bool {
        true
    }

    fn lag(&mut self, modulation: &mut Modulation<'_, '_>) {
        self.out.lag(modulation);
    }

    fn release(&mut self, _: &mut bl_engine::NoteBuffer) {
        // Spec: device-framework R32. Switched off, it forgets the keys.
        self.held.clear();
        self.out.off();
    }

    fn receive_data(&mut self, data: NodeData) {
        if let Some(shape) = data.downcast_ref::<Shape>() {
            self.points.clone_from(&shape.0);
        }
    }

    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if self.out.begin(audio) {
            // Spec: modulation R20, R21. It begins as a render from here
            // does, unless a key is already down.
            self.post.restart();
            if self.held.is_empty() {
                self.x = 0.0;
                self.running = false;
                self.echo = None;
                self.level = 0.0;
            }
        }
        let index = |id: ParamId| params.index(id).expect("the description holds it");
        let at = |id: ParamId, f: usize| f64::from(params.ramp(index(id))[f]);
        let bpm = audio.bpm();
        let ix = Indices::new(params, SLOTS);
        let mut notes = audio.notes().peekable();
        for f in 0..frames {
            let cycle = if at(TIME_MODE, f) >= 0.5 {
                DIVISIONS[(at(RATE_BEATS, f) as usize).min(DIVISIONS.len() - 1)].1 * 60.0 / bpm
                    * self.rate
            } else {
                at(RATE_MS, f) * self.rate / 1000.0
            };
            let settings = Settings {
                looped: at(LOOP, f) >= 0.5,
                velocity: at(VELOCITY, f) / 100.0,
                echo: at(ECHO, f) / 100.0,
                echo_frames: (at(ECHO_TIME, f) * self.rate / 1000.0).max(1.0),
                cycle: cycle.max(1.0),
            };
            while let Some(note) = notes.next_if(|n| n.offset as usize == f) {
                audio.send(note);
                self.note(&note, &settings);
            }
            // T plays the shape from its start, as a note does.
            let trigger = at(TRIGGER, f) >= 0.5;
            if trigger && !self.triggered {
                self.start(1.0);
            }
            self.triggered = trigger;
            self.step(&settings);
            if self.out.at_grid() {
                // Spec: modulation R26, R22. Jitter, Smooth and Offset.
                let shaped = shape_value(&self.points, self.x) * self.scale;
                self.level = self.post.apply(
                    shaped,
                    at(JITTER, f) / 100.0,
                    at(SMOOTH, f) / 100.0,
                    at(OFFSET, f) / 100.0,
                );
            }
            self.out.frame(audio, &ix, params, f, &[self.level]);
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

impl StrikeMod {
    fn note(&mut self, note: &NoteEvent, settings: &Settings) {
        match note.kind {
            NoteKind::On => {
                self.held.on(note);
                self.velocity = f64::from(note.velocity);
                let scale = 1.0 - settings.velocity + settings.velocity * self.velocity;
                self.start(scale);
                self.echo = (settings.echo > 0.0).then_some(scale * settings.echo);
                self.echo_clock = 0.0;
            }
            NoteKind::Off | NoteKind::Choke => {
                self.held.off(note);
            }
            _ => {}
        }
    }

    fn step(&mut self, settings: &Settings) {
        if let Some(scale) = self.echo {
            self.echo_clock += 1.0;
            if self.echo_clock >= settings.echo_frames {
                self.echo_clock = 0.0;
                if scale > ECHO_FLOOR {
                    self.start(scale);
                    self.echo = Some(scale * settings.echo);
                } else {
                    self.echo = None;
                }
            }
        }
        if !self.running {
            return;
        }
        let held = !self.held.is_empty();
        let next = self.x + 1.0 / settings.cycle;
        // Spec: modulation R26. The sustain breakpoint holds the shape while
        // a key is held, unless Loop passes over it.
        self.x = match self.sustain_at() {
            Some(sustain) if held && !settings.looped && self.x <= sustain && next > sustain => {
                sustain
            }
            _ => next,
        };
        if self.x >= 1.0 {
            if settings.looped && held {
                self.x -= 1.0;
                self.flashed = true;
            } else {
                self.x = 1.0;
                self.running = false;
            }
        }
    }
}

/// The Shaper MIDI's display (R26): its level now and its trigger LED, from
/// what its node published; the shape itself is drawn by the panel.
#[must_use]
pub fn display(
    device: DeviceId,
    _description: &DeviceDescription,
    _values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let mut out = DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
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
