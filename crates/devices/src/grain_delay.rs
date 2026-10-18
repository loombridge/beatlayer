//! Scatter, the grain delay (WP-5.25; the grain delay spec): the input is
//! read back in short overlapping grains, each at its own pitch and a
//! little off the delay time, and the result can feed back.

use std::f64::consts::TAU;

use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::delay::{Line, MAX_DELAY_SECONDS, sixteenth_frames};
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Scatter's device type.
pub const GRAIN_DELAY: &str = "bl.device.scatter";

/// Sync: beat divisions on, free milliseconds off.
pub const SYNC: ParamId = ParamId(0);
/// The synced time, in sixteenth notes.
pub const DIVISION: ParamId = ParamId(1);
/// How much the synced time is shortened or lengthened, in per cent of it.
pub const PERCENT: ParamId = ParamId(2);
/// The free time, in milliseconds.
pub const TIME: ParamId = ParamId(3);
/// How often a grain starts, in hertz; a grain lasts two periods.
pub const FREQUENCY: ParamId = ParamId(4);
/// The grains' pitch, in semitones.
pub const PITCH: ParamId = ParamId(5);
/// How far each grain's pitch strays at random, in per cent of an octave.
pub const RANDOM_PITCH: ParamId = ParamId(6);
/// How far later than the delay time each grain may start, in milliseconds.
pub const SPRAY: ParamId = ParamId(7);
/// How much of the output feeds back into the delay.
pub const FEEDBACK: ParamId = ParamId(8);
/// The dry/wet balance.
pub const DRY_WET: ParamId = ParamId(9);
/// The parameter on the X–Y display's horizontal axis, an index into
/// [`AXES`].
pub const X_AXIS: ParamId = ParamId(10);
/// The parameter on its vertical axis.
pub const Y_AXIS: ParamId = ParamId(11);

/// How many parameters the device has.
const PARAMS: usize = 12;
/// The parameters the X–Y display's axes can take, in the choosers' order.
/// Delay Time stands for the time control shown: the division while Sync is
/// on, the milliseconds otherwise.
pub const AXES: [(&str, ParamId); 7] = [
    ("Delay Time", TIME),
    ("Frequency", FREQUENCY),
    ("Pitch", PITCH),
    ("Random Pitch", RANDOM_PITCH),
    ("Spray", SPRAY),
    ("Feedback", FEEDBACK),
    ("Dry/Wet", DRY_WET),
];
/// The widest spray, in milliseconds.
const MAX_SPRAY_MS: f64 = 500.0;
/// What a Random Pitch of 100 % strays by each way, in semitones.
const RANDOM_PITCH_SEMITONES: f64 = 12.0;
/// Grains sounding at once, at the most: two overlap, the rest are older
/// ones that outlast a rise in Frequency.
const GRAINS: usize = 8;

/// Scatter's type: Beatlayer's own name and look, Live's parameters (the
/// grain delay spec).
#[must_use]
pub fn grain_delay_type() -> DeviceType {
    let names: Vec<&str> = AXES.iter().map(|(name, _)| *name).collect();
    let params = vec![
        ParamDescriptor::toggle(SYNC, "Sync", false)
            .describe("Sets the delay time in beat divisions rather than milliseconds."),
        ParamDescriptor::integer(DIVISION, "Division", (1, 16), 4, ParamUnit::None)
            .describe("The delay time in sixteenth notes while Sync is on."),
        ParamDescriptor::continuous(PERCENT, "Time %", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("Shortens or lengthens the synced time, for a swing."),
        ParamDescriptor::continuous(TIME, "Time", (1.0, 5_000.0), 100.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("The delay time in milliseconds while Sync is off."),
        ParamDescriptor::continuous(FREQUENCY, "Frequency", (1.0, 150.0), 20.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("How often a grain starts; higher means smaller, quicker grains."),
        ParamDescriptor::continuous(PITCH, "Pitch", (-36.0, 36.0), 0.0, ParamUnit::Semitones)
            .describe("Transposes the grains, as a crude pitch shifter."),
        ParamDescriptor::continuous(
            RANDOM_PITCH,
            "Random Pitch",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Varies each grain's pitch at random."),
        ParamDescriptor::continuous(
            SPRAY,
            "Spray",
            (0.0, MAX_SPRAY_MS),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("Starts each grain a random time later, smearing the signal."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much of the output feeds back into the delay."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The balance between the dry and the grains."),
        ParamDescriptor::choice(X_AXIS, "X Axis", &names, 0)
            .describe("The parameter the X–Y display's horizontal axis moves."),
        ParamDescriptor::choice(Y_AXIS, "Y Axis", &names, AXES.len() - 1)
            .describe("The parameter the X–Y display's vertical axis moves."),
    ];
    DeviceType::native(GRAIN_DELAY, "Scatter", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

fn layout() -> DeviceLayout {
    let time = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (SYNC, Widget::Toggle),
            (DIVISION, Widget::Number),
            (PERCENT, Widget::Knob),
            (TIME, Widget::Knob),
        ],
    );
    let display = DisplayCell::new("X-Y", 160.0, BODY_HEIGHT, 1.0).with_top(ControlBand::stacked(
        40.0,
        &[(X_AXIS, Widget::Chooser), (Y_AXIS, Widget::Chooser)],
    ));
    let grains = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (FREQUENCY, Widget::Knob),
            (PITCH, Widget::Knob),
            (RANDOM_PITCH, Widget::Knob),
            (SPRAY, Widget::Knob),
            (FEEDBACK, Widget::Knob),
            (DRY_WET, Widget::Knob),
        ],
    );
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(time),
        LayoutNode::Cell(display),
        LayoutNode::Band(grains),
    ]))
}

/// Adds Scatter to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, grain_delay_type(), Scatter::new);
}

/// One grain: where it is in its length, and what it reads.
#[derive(Clone, Copy, Default)]
struct Grain {
    live: bool,
    /// Frames since it started, and how long it lasts.
    age: f64,
    length: f64,
    /// The delay at its middle, in frames, and the ratio it reads at.
    delay: f64,
    ratio: f64,
}

/// Scatter: a delay line a channel, and the grains reading them.
struct Scatter {
    lines: Vec<Line>,
    /// The longest delay a grain reads at, in frames.
    max_delay: f64,
    grains: [Grain; GRAINS],
    random: Random,
    /// How far through the next grain's start the device is, in cycles.
    phase: f64,
}

impl Scatter {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let len = ((MAX_DELAY_SECONDS + MAX_SPRAY_MS / 1_000.0 + 0.1) * rate).ceil() as usize + 2;
        Self {
            lines: (0..config.channels).map(|_| Line::new(len)).collect(),
            max_delay: (len - 2) as f64,
            grains: [Grain::default(); GRAINS],
            random: Random::seeded(device.id.0.get()),
            phase: 1.0,
        }
    }

    /// Starts a grain at the delay `delay` (frames), `pitch` semitones up,
    /// already `age` frames in, in a slot that's free or else the oldest's.
    fn start(&mut self, age: f64, length: f64, delay: f64, pitch: f64) {
        let slot = self.grains.iter().position(|g| !g.live).unwrap_or_else(|| {
            self.grains
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.age.total_cmp(&b.1.age))
                .map_or(0, |(i, _)| i)
        });
        self.grains[slot] = Grain {
            live: true,
            age,
            length,
            delay,
            ratio: (pitch / 12.0).exp2(),
        };
    }
}

impl NativeDevice for Scatter {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let rate = audio.sample_rate().as_f64();
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let value = |id: ParamId| params.value(ix[id.0 as usize]);
        let sixteenth = sixteenth_frames(audio);
        let synced = value(SYNC) >= 0.5;
        let synced_frames =
            value(DIVISION) * sixteenth * (1.0 + params.target(ix[PERCENT.0 as usize]) / 100.0);
        let (time, frequency, pitch) = (ramp(TIME), ramp(FREQUENCY), ramp(PITCH));
        let (random_pitch, spray) = (ramp(RANDOM_PITCH), ramp(SPRAY));
        let (feedback, dry_wet) = (ramp(FEEDBACK), ramp(DRY_WET));
        let max_delay = self.max_delay;
        for i in 0..audio.frames() {
            let hop = rate / f64::from(frequency[i]);
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                // Two draws a grain whatever the settings, so the stream
                // doesn't depend on them.
                let (later, strays) = (self.random.next_bipolar(), self.random.next_bipolar());
                let delay = if synced {
                    synced_frames
                } else {
                    f64::from(time[i]) * rate / 1_000.0
                } + (later + 1.0) / 2.0 * f64::from(spray[i]) * rate / 1_000.0;
                let pitch = f64::from(pitch[i])
                    + strays * f64::from(random_pitch[i]) / 100.0 * RANDOM_PITCH_SEMITONES;
                self.start(self.phase * hop, 2.0 * hop, delay.min(max_delay), pitch);
            }
            self.phase += 1.0 / hop;
            let mut weights = [(0.0, 0.0); GRAINS];
            for (grain, slot) in self.grains.iter_mut().zip(&mut weights) {
                if !grain.live {
                    continue;
                }
                let window = 0.5 * (1.0 - (TAU * grain.age / grain.length).cos());
                let delay = grain.delay + (1.0 - grain.ratio) * (grain.age - grain.length / 2.0);
                *slot = (window, delay.clamp(1.0, max_delay));
                grain.age += 1.0;
                grain.live = grain.age < grain.length;
            }
            let (feedback, wet_mix) = (
                f64::from(feedback[i]) / 100.0,
                f64::from(dry_wet[i]) / 100.0,
            );
            for (channel, line) in self.lines.iter_mut().enumerate() {
                let input = f64::from(audio.channel_mut(channel)[i]);
                let wet: f64 = weights
                    .iter()
                    .filter(|(window, _)| *window > 0.0)
                    .map(|(window, delay)| window * line.read(*delay))
                    .sum();
                audio.channel_mut(channel)[i] = (input * (1.0 - wet_mix) + wet * wet_mix) as f32;
                line.write((input + feedback * wet) as f32);
            }
        }
    }
}
