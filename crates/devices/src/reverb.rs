//! Reverb, the algorithmic reverb (WP-2.18b, part 1; the reverb spec):
//! an input filter, early reflections, a diffusion network, a chorus
//! section, the global predelay, size, decay and freeze controls, a stereo
//! width, and the reflect, diffuse and dry/wet output controls.
//!
//! Its algorithm is Beatlayer's own: a predelay line feeds a bank of early
//! reflection taps, shaped by the Shape control, and a diffusion network of
//! allpass filters into a bank of damped combs whose feedbacks come from
//! the Decay setting (RT60). The Size control scales the delay lines, the
//! Chorus and Spin controls move them, Freeze holds the network's tail, and
//! the Stereo control sets the width of the output. Its parameters, ranges
//! and defaults are Beatlayer's reading of Live 12's Reverb
//! (`docs/specs/reverb.md`); the manual doesn't record the ranges or
//! defaults, so those are Beatlayer's and are listed to observe.

use core::f64::consts::TAU;

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Reverb's device type.
pub const REVERB: &str = "beatlayer.rotunda";

/// Low Cut: whether the input's low-cut filter runs.
pub const LOW_CUT: ParamId = ParamId(0);
/// The low-cut filter's frequency.
pub const LOW_CUT_HZ: ParamId = ParamId(1);
/// High Cut: whether the input's high-cut filter runs.
pub const HIGH_CUT: ParamId = ParamId(2);
/// The high-cut filter's frequency.
pub const HIGH_CUT_HZ: ParamId = ParamId(3);
/// Spin: whether the early reflections are modulated.
pub const SPIN: ParamId = ParamId(4);
/// How far Spin moves the early reflections.
pub const SPIN_AMOUNT: ParamId = ParamId(5);
/// How fast Spin moves them.
pub const SPIN_RATE: ParamId = ParamId(6);
/// Shape: how quickly the early reflections decay.
pub const SHAPE: ParamId = ParamId(7);
/// High: whether the tail's high frequencies are damped.
pub const HIGH_DAMP: ParamId = ParamId(8);
/// How much the tail's high frequencies are damped.
pub const HIGH: ParamId = ParamId(9);
/// Low: whether the tail's low frequencies are damped.
pub const LOW_DAMP: ParamId = ParamId(10);
/// How much the tail's low frequencies are damped.
pub const LOW: ParamId = ParamId(11);
/// How much the allpass stages diffuse the tail.
pub const DIFFUSION: ParamId = ParamId(12);
/// Scale: the diffusion's coarseness.
pub const SCALE: ParamId = ParamId(13);
/// Chorus: whether the tail is modulated.
pub const CHORUS: ParamId = ParamId(14);
/// How far Chorus moves the tail.
pub const CHORUS_AMOUNT: ParamId = ParamId(15);
/// How fast Chorus moves it.
pub const CHORUS_RATE: ParamId = ParamId(16);
/// Predelay: the time before the first early reflection.
pub const PREDELAY: ParamId = ParamId(17);
/// Smooth: how a Size change is smoothed.
pub const SMOOTH: ParamId = ParamId(18);
/// Size: the room's volume.
pub const SIZE: ParamId = ParamId(19);
/// Decay: the time the tail takes to fall by 60 dB.
pub const DECAY: ParamId = ParamId(20);
/// Freeze: hold the tail.
pub const FREEZE: ParamId = ParamId(21);
/// Flat: bypass the damping filters while frozen.
pub const FLAT: ParamId = ParamId(22);
/// Cut: stop the input adding to the frozen tail.
pub const CUT: ParamId = ParamId(23);
/// Stereo: the width of the output's stereo image, in degrees.
pub const STEREO: ParamId = ParamId(24);
/// Density: the tradeoff between quality and CPU.
pub const DENSITY: ParamId = ParamId(25);
/// The amplitude of the early reflections.
pub const REFLECT: ParamId = ParamId(26);
/// The amplitude of the diffuse tail.
pub const DIFFUSE: ParamId = ParamId(27);
/// The dry/wet balance.
pub const DRY_WET: ParamId = ParamId(28);

/// How many parameters Reverb has.
const PARAMS: usize = 29;

/// The Damping chooser's choices.
pub const DENSITIES: [&str; 3] = ["Sparse", "Normal", "High"];
/// The Smooth chooser's choices.
pub const SMOOTHS: [&str; 3] = ["None", "Fast", "Slow"];

/// How many allpass stages diffuse each channel.
const ALLPASSES: usize = 4;
/// The allpass delays at 44.1 kHz, in samples.
const ALLPASS_SAMPLES: [f64; ALLPASSES] = [556.0, 441.0, 341.0, 225.0];
/// How many combs the network runs at its densest.
const COMBS: usize = 12;
/// The comb delays at 44.1 kHz, in samples: the first eight make the
/// Normal bank, and the last four join it at High density.
const COMB_SAMPLES: [f64; COMBS] = [
    1116.0, 1188.0, 1277.0, 1356.0, 1422.0, 1491.0, 1557.0, 1617.0, 691.0, 779.0, 887.0, 897.0,
];
/// How many combs each Density choice runs.
const DENSITY_COMBS: [usize; 3] = [4, 8, COMBS];
/// The right channel's comb delay offset at 44.1 kHz, in samples.
const STEREO_SPREAD: f64 = 23.0;
/// The early reflection tap times, in seconds.
const EARLY_TAPS: [f64; 4] = [0.0073, 0.0111, 0.0137, 0.0179];
/// The right channel's early reflection offset, in seconds.
const EARLY_SPREAD: f64 = 0.0017;
/// The early reflections' own gain.
const EARLY_GAIN: f32 = 0.5;
/// How much of the predelayed input reaches the diffusion network.
const NETWORK_INPUT: f32 = 0.3;
/// Size at its bottom scales the delay lines by this.
const SIZE_SMALL: f64 = 0.3;
/// Size at its top scales them by this.
const SIZE_LARGE: f64 = 2.0;
/// Scale at its bottom scales the allpass delays by this.
const SCALE_SMALL: f64 = 0.5;
/// Scale at its top scales them by this.
const SCALE_LARGE: f64 = 2.0;
/// The highest allpass coefficient, at Diffusion 100 %.
const DIFFUSION_MAX: f64 = 0.7;
/// Spin at 100 % moves the reflection taps by this many milliseconds.
const SPIN_MILLISECONDS: f64 = 3.0;
/// Chorus at 100 % moves the combs by this many milliseconds.
const CHORUS_MILLISECONDS: f64 = 0.5;
/// The highest frequency in the tail, at High 100 %: the damping
/// one-pole's turnover.
const HIGH_TURNOVER_HZ: f64 = 4_000.0;
/// The lowest frequency left in the tail, at Low 100 %.
const LOW_TURNOVER_HZ: f64 = 500.0;
/// Shape at 100 % delays the diffused onset by this many milliseconds.
const SHAPE_MILLISECONDS: f64 = 15.0;
/// The longest predelay the manual's typical values leave room for.
const MAX_PREDELAY_MILLISECONDS: f64 = 250.0;
/// How long a Size change takes with Smooth Fast, in seconds.
const SMOOTH_FAST_SECONDS: f64 = 0.05;
/// The same with Smooth Slow.
const SMOOTH_SLOW_SECONDS: f64 = 1.0;
/// A state value below this is left as zero rather than as a denormal.
const FLUSH: f32 = 1e-30;
/// The most a comb holds, in full-scale units (+18 dBFS), so a frozen tail
/// that keeps taking in input (Cut off) cannot grow to infinity.
const COMB_LIMIT: f32 = 8.0;

/// The reverb's type: Beatlayer's own look, Live's controls
/// (the reverb spec, "The behaviour").
#[must_use]
pub fn reverb_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::toggle(LOW_CUT, "Low Cut", false)
            .describe("Filters frequencies below Low Cut Out of the input."),
        ParamDescriptor::continuous(
            LOW_CUT_HZ,
            "Low Cut Frequency",
            (20.0, 500.0),
            100.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The frequency below which the input's low-cut filter removes the input."),
        ParamDescriptor::toggle(HIGH_CUT, "High Cut", false)
            .describe("Filters frequencies above High Cut out of the input."),
        ParamDescriptor::continuous(
            HIGH_CUT_HZ,
            "High Cut Frequency",
            (1_000.0, 20_000.0),
            10_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The frequency above which the input's high-cut filter removes the input."),
        ParamDescriptor::toggle(SPIN, "Spin", false)
            .describe("Modulates the early reflections, for a less coloured tail."),
        ParamDescriptor::continuous(
            SPIN_AMOUNT,
            "Spin Amount",
            (0.0, 100.0),
            20.0,
            ParamUnit::Percent,
        )
        .describe("How far Spin moves the early reflections."),
        ParamDescriptor::continuous(SPIN_RATE, "Spin Rate", (0.01, 10.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("How fast Spin moves the early reflections."),
        ParamDescriptor::continuous(SHAPE, "Shape", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe(
                "How quickly the early reflections decay and how soon the diffusion follows.",
            ),
        ParamDescriptor::toggle(HIGH_DAMP, "High", true)
            .describe("Damps the tail's high frequencies, as air and soft furnishings do."),
        ParamDescriptor::continuous(HIGH, "High Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How much the tail's high frequencies are damped."),
        ParamDescriptor::toggle(LOW_DAMP, "Low", false)
            .describe("Damps the tail's low frequencies, for a thinner decay."),
        ParamDescriptor::continuous(LOW, "Low Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How much the tail's low frequencies are damped."),
        ParamDescriptor::continuous(
            DIFFUSION,
            "Diffusion",
            (0.0, 100.0),
            50.0,
            ParamUnit::Percent,
        )
        .describe("How much the allpass stages diffuse the tail."),
        ParamDescriptor::continuous(SCALE, "Scale", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The diffusion's coarseness: the lower the finer."),
        ParamDescriptor::toggle(CHORUS, "Chorus", false)
            .describe("Adds modulation and motion to the tail."),
        ParamDescriptor::continuous(
            CHORUS_AMOUNT,
            "Chorus Amount",
            (0.0, 100.0),
            20.0,
            ParamUnit::Percent,
        )
        .describe("How far Chorus moves the tail."),
        ParamDescriptor::continuous(
            CHORUS_RATE,
            "Chorus Rate",
            (0.01, 10.0),
            0.5,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("How fast Chorus moves the tail."),
        ParamDescriptor::continuous(
            PREDELAY,
            "Predelay",
            (0.0, MAX_PREDELAY_MILLISECONDS),
            10.0,
            ParamUnit::Milliseconds,
        )
        .describe("The time before the onset of the first early reflection."),
        ParamDescriptor::choice(SMOOTH, "Smooth", &SMOOTHS, 0).describe(
            "How a change of Size is smoothed: not at all, or over a short or long time.",
        ),
        ParamDescriptor::continuous(SIZE, "Size", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The room's volume, which scales the delay lines."),
        ParamDescriptor::continuous(DECAY, "Decay", (0.1, 60.0), 2.0, ParamUnit::Seconds)
            .logarithmic()
            .describe("The time the tail takes to fall by 60 dB."),
        ParamDescriptor::toggle(FREEZE, "Freeze", false)
            .describe("Holds the tail, letting the reverberation sustain."),
        ParamDescriptor::toggle(FLAT, "Flat", false)
            .describe("Bypasses the damping filters while Freeze is on."),
        ParamDescriptor::toggle(CUT, "Cut", true)
            .describe("Stops the input adding to the frozen tail."),
        ParamDescriptor::continuous(STEREO, "Stereo", (0.0, 120.0), 120.0, ParamUnit::None)
            .decimals(0)
            .describe("The width of the output, in degrees: 0 is mono."),
        ParamDescriptor::choice(DENSITY, "Density", &DENSITIES, 1)
            .describe("The tradeoff between the tail's quality and the device's cost."),
        ParamDescriptor::continuous(REFLECT, "Reflect", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The amplitude of the early reflections."),
        ParamDescriptor::continuous(DIFFUSE, "Diffuse", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The amplitude of the diffuse tail."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry and the reverberated signal."),
    ];
    DeviceType::native(REVERB, "Reverb", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// A layout of fixed control bands, in the device's sections.
fn layout() -> DeviceLayout {
    let input = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (LOW_CUT, Widget::Toggle),
            (LOW_CUT_HZ, Widget::Knob),
            (HIGH_CUT, Widget::Toggle),
            (HIGH_CUT_HZ, Widget::Knob),
        ],
    );
    let early = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (SPIN, Widget::Toggle),
            (SPIN_AMOUNT, Widget::Knob),
            (SPIN_RATE, Widget::Knob),
            (SHAPE, Widget::Knob),
        ],
    );
    let diffusion = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (HIGH_DAMP, Widget::Toggle),
            (HIGH, Widget::Knob),
            (LOW_DAMP, Widget::Toggle),
            (LOW, Widget::Knob),
            (DIFFUSION, Widget::Knob),
            (SCALE, Widget::Knob),
        ],
    );
    let chorus = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (CHORUS, Widget::Toggle),
            (CHORUS_AMOUNT, Widget::Knob),
            (CHORUS_RATE, Widget::Knob),
        ],
    );
    let global = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (PREDELAY, Widget::Knob),
            (SIZE, Widget::Knob),
            (DECAY, Widget::Knob),
            (SMOOTH, Widget::Chooser),
        ],
    );
    let freeze = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (FREEZE, Widget::Toggle),
            (FLAT, Widget::Toggle),
            (CUT, Widget::Toggle),
            (DENSITY, Widget::Chooser),
            (STEREO, Widget::Knob),
        ],
    );
    let output = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (REFLECT, Widget::Knob),
            (DIFFUSE, Widget::Knob),
            (DRY_WET, Widget::Knob),
        ],
    );
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(input),
        LayoutNode::Band(early),
        LayoutNode::Band(diffusion),
        LayoutNode::Band(chorus),
        LayoutNode::Band(global),
        LayoutNode::Band(freeze),
        LayoutNode::Band(output),
    ]))
}

/// Adds Reverb to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, reverb_type(), |_, config| Reverb::new(config));
}

/// The tail's damping: the shelves' amounts and their one-poles'
/// coefficients, fixed for one stretch.
#[derive(Clone, Copy, Default)]
struct Damping {
    high_on: bool,
    high: f32,
    low_on: bool,
    low: f32,
    high_a: f32,
    low_a: f32,
}

/// One stretch's settings: the toggles and choices, the worked-out
/// coefficients, and each parameter's place in the bank.
struct Settings {
    low_cut: bool,
    low_a: f32,
    high_cut: bool,
    high_a: f32,
    spin: bool,
    chorus: bool,
    freeze: bool,
    cut: bool,
    count: usize,
    /// The early reflection taps' gains, worked out from Shape.
    early_gains: [f32; 4],
    /// Shape's delay of the diffused onset, in seconds.
    shape_shift: f64,
    spin_rate: f64,
    spin_depth: f64,
    chorus_rate: f64,
    chorus_depth: f64,
    /// The diffusion's allpass coefficient.
    diffusion: f32,
    /// The damping the combs apply.
    damping: Damping,
    /// Each comb's feedback, from Decay and its delay time.
    feedback: [f32; COMBS],
    /// How fast a Size change moves, as a fraction of the travel per frame.
    size_step: f64,
    rate: f64,
    index: [usize; PARAMS],
}

impl Settings {
    fn new(params: &ParamBank, rate: f64, size_now: f64) -> Self {
        let index = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let value = |id: ParamId| params.value(index[id.0 as usize]);
        let toggle = |id: ParamId| value(id) >= 0.5;
        let freeze = toggle(FREEZE);
        let flat = toggle(FLAT);
        let high_on = toggle(HIGH_DAMP) && !(freeze && flat);
        let low_on = toggle(LOW_DAMP) && !(freeze && flat);
        let shape = value(SHAPE) / 100.0;
        let early_gains = core::array::from_fn(|k| {
            let decay = 0.4 + 2.0 * shape;
            (((-decay * k as f64).exp()) * f64::from(EARLY_GAIN)) as f32
        });
        let decay = value(DECAY);
        let feedback = core::array::from_fn(|k| {
            if freeze {
                return 1.0;
            }
            let seconds = COMB_SAMPLES[k] * rate / 44_100.0 * size_now / rate;
            (10f64.powf(-3.0 * seconds / decay)) as f32
        });
        let smooth = match value(SMOOTH) as usize {
            1 => SMOOTH_FAST_SECONDS,
            2 => SMOOTH_SLOW_SECONDS,
            _ => 0.0,
        };
        let size_step = if smooth > 0.0 {
            1.0 / (smooth * rate)
        } else {
            1.0
        };
        Self {
            low_cut: toggle(LOW_CUT),
            low_a: one_pole(value(LOW_CUT_HZ), rate),
            high_cut: toggle(HIGH_CUT),
            high_a: one_pole(value(HIGH_CUT_HZ), rate),
            spin: toggle(SPIN),
            chorus: toggle(CHORUS),
            freeze,
            cut: toggle(CUT),
            count: DENSITY_COMBS[(value(DENSITY) as usize).min(2)],
            early_gains,
            shape_shift: shape * SHAPE_MILLISECONDS / 1_000.0,
            spin_rate: value(SPIN_RATE),
            spin_depth: SPIN_MILLISECONDS * rate / 1_000.0,
            chorus_rate: value(CHORUS_RATE),
            chorus_depth: CHORUS_MILLISECONDS * rate / 1_000.0,
            diffusion: (value(DIFFUSION) / 100.0 * DIFFUSION_MAX) as f32,
            damping: Damping {
                high_on,
                high: (value(HIGH) / 100.0) as f32,
                low_on,
                low: (value(LOW) / 100.0) as f32,
                high_a: one_pole(HIGH_TURNOVER_HZ, rate),
                low_a: one_pole(LOW_TURNOVER_HZ, rate),
            },
            feedback,
            size_step,
            rate,
            index,
        }
    }

    /// A continuous parameter's value at each frame of the stretch.
    fn ramp<'a>(&self, params: &'a ParamBank, id: ParamId) -> &'a [f32] {
        params.ramp(self.index[id.0 as usize])
    }

    /// The allpass delays' factor, from Scale.
    fn scale_factor(&self, params: &ParamBank, frame: usize) -> f64 {
        let percent = f64::from(self.ramp(params, SCALE)[frame]) / 100.0;
        SCALE_SMALL * (SCALE_LARGE / SCALE_SMALL).powf(percent)
    }
}

/// The one-pole coefficient reaching a turnover of `hz`.
fn one_pole(hz: f64, rate: f64) -> f32 {
    (1.0 - (-TAU * hz / rate).exp()) as f32
}

/// A value far below the audio's noise floor is left as zero rather than as
/// a denormal, whose arithmetic is slow.
fn flush(x: f32) -> f32 {
    if x.abs() < FLUSH { 0.0 } else { x }
}

/// The reverb's audio-thread state: the two channels' filters, predelay
/// lines, diffusion and comb networks, and the two modulation LFOs.
struct Reverb {
    input: [InputFilter; 2],
    predelay: [Line; 2],
    diffusion: [[Allpass; ALLPASSES]; 2],
    combs: [[Comb; COMBS]; 2],
    spin: [Lfo; 2],
    chorus: Lfo,
    rate: f64,
    /// The delay lines' size factor, moving to the Size setting.
    size_now: f64,
}

impl Reverb {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let predelay = ((MAX_PREDELAY_MILLISECONDS + SHAPE_MILLISECONDS) / 1_000.0
            + (EARLY_TAPS[3] + EARLY_SPREAD) * SIZE_LARGE
            + 0.01)
            * rate;
        let predelay = predelay.ceil() as usize + 4;
        // Every comb's line must hold the longest delay at the largest size.
        let longest = COMB_SAMPLES.iter().copied().fold(f64::MIN, f64::max);
        let comb = (longest + STEREO_SPREAD) * rate / 44_100.0 * SIZE_LARGE
            + CHORUS_MILLISECONDS * rate / 1_000.0
            + 4.0;
        let comb = comb.ceil() as usize;
        let allpass = (ALLPASS_SAMPLES[0] * rate / 44_100.0 * SCALE_LARGE + 4.0).ceil() as usize;
        Self {
            input: [InputFilter::default(); 2],
            predelay: [Line::new(predelay), Line::new(predelay)],
            diffusion: [
                core::array::from_fn(|_| Allpass::new(allpass)),
                core::array::from_fn(|_| Allpass::new(allpass)),
            ],
            combs: [
                core::array::from_fn(|_| Comb::new(comb)),
                core::array::from_fn(|_| Comb::new(comb)),
            ],
            spin: [Lfo::new(0.0), Lfo::new(0.37)],
            chorus: Lfo::new(0.0),
            rate,
            size_now: SIZE_SMALL + (SIZE_LARGE - SIZE_SMALL) / 2.0,
        }
    }
}

impl NativeDevice for Reverb {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        if audio.frames() == 0 {
            return;
        }
        let settings = Settings::new(params, self.rate, self.size_now);
        let mut channels = audio.channels_mut();
        let Some(first) = channels.next() else {
            return;
        };
        // shortcut: the reverb is stereo, as the manual's is; any channel
        // past the second passes through. Upgrade with a surround layout.
        let second = channels.next();
        self.render(first, second, &settings, params);
    }
}

impl Reverb {
    /// Renders the first channel and, when there is one, the second.
    fn render(
        &mut self,
        first: &mut [f32],
        mut second: Option<&mut [f32]>,
        settings: &Settings,
        params: &ParamBank,
    ) {
        let frames = first.len();
        let rate_scale = self.rate / 44_100.0;
        for i in 0..frames {
            // Size moves toward its setting at the Smooth choice's pace.
            let percent = f64::from(settings.ramp(params, SIZE)[i]) / 100.0;
            let target = SIZE_SMALL + (SIZE_LARGE - SIZE_SMALL) * percent;
            let step = (target - self.size_now).abs().min(settings.size_step);
            self.size_now += step * (target - self.size_now).signum();
            let amount = settings.ramp(params, CHORUS_AMOUNT)[i] as f64 / 100.0;
            let chorus = if settings.chorus {
                self.chorus.step(settings.chorus_rate, settings.rate)
                    * amount
                    * settings.chorus_depth
            } else {
                0.0
            };
            let spin_amount = settings.ramp(params, SPIN_AMOUNT)[i] as f64 / 100.0;
            let spin: [f64; 2] = if settings.spin {
                [0, 1].map(|channel| {
                    self.spin[channel].step(settings.spin_rate, settings.rate)
                        * spin_amount
                        * settings.spin_depth
                })
            } else {
                [0.0; 2]
            };
            let predelay = f64::from(settings.ramp(params, PREDELAY)[i]) / 1_000.0 * self.rate;
            // The dry signal and the input filter.
            let dry_left = first[i];
            let dry_right = second.as_ref().map_or(dry_left, |right| right[i]);
            let filtered_left = self.input[0].process(
                dry_left,
                settings.low_cut,
                settings.low_a,
                settings.high_cut,
                settings.high_a,
            );
            let filtered_right = self.input[1].process(
                dry_right,
                settings.low_cut,
                settings.low_a,
                settings.high_cut,
                settings.high_a,
            );
            self.predelay[0].push(filtered_left);
            self.predelay[1].push(filtered_right);
            // The early reflections, on both channels.
            let early = [
                self.early(0, settings, spin[0], predelay),
                self.early(1, settings, spin[1], predelay),
            ];
            // The diffusion network's input, delayed by Shape's onset.
            let net_delay = predelay + settings.shape_shift * self.rate;
            let net = if settings.freeze && settings.cut {
                [0.0f32; 2]
            } else {
                [
                    self.predelay[0].read(net_delay) * NETWORK_INPUT,
                    self.predelay[1].read(net_delay) * NETWORK_INPUT,
                ]
            };
            let scale = settings.scale_factor(params, i) as f32;
            let mut tail = [0.0f32; 2];
            for channel in 0..2 {
                let mut diffused = net[channel];
                for (k, base) in ALLPASS_SAMPLES.iter().enumerate() {
                    diffused = self.diffusion[channel][k].process(
                        diffused,
                        base * rate_scale * f64::from(scale),
                        settings.diffusion,
                    );
                }
                let mut sum = 0.0;
                for (k, base) in COMB_SAMPLES.iter().enumerate().take(settings.count) {
                    let delay = base * rate_scale * self.size_now
                        + if channel == 1 {
                            STEREO_SPREAD * rate_scale
                        } else {
                            0.0
                        }
                        + chorus;
                    sum += self.combs[channel][k].process(
                        diffused,
                        delay,
                        settings.feedback[k],
                        settings.damping,
                    );
                }
                tail[channel] = sum / settings.count as f32;
            }
            // The Stereo control's width, then the output controls.
            let degrees = f64::from(settings.ramp(params, STEREO)[i]) / 120.0;
            let reflect = settings.ramp(params, REFLECT)[i] / 100.0;
            let diffuse = settings.ramp(params, DIFFUSE)[i] / 100.0;
            let dry_wet = settings.ramp(params, DRY_WET)[i] / 100.0;
            let wet_left = reflect * early[0] + diffuse * tail[0];
            let wet_right = reflect * early[1] + diffuse * tail[1];
            let mid = 0.5 * (wet_left + wet_right);
            let side = 0.5 * (wet_left - wet_right) * degrees as f32;
            first[i] = dry_left * (1.0 - dry_wet) + dry_wet * (mid + side);
            if let Some(right) = second.as_deref_mut() {
                right[i] = dry_right * (1.0 - dry_wet) + dry_wet * (mid - side);
            }
        }
    }

    /// One channel's early reflections: four taps off its predelay line,
    /// their times scaled by Size and moved by Spin.
    fn early(&self, channel: usize, settings: &Settings, spin: f64, predelay: f64) -> f32 {
        let mut sum = 0.0;
        for (k, tap) in EARLY_TAPS.iter().enumerate() {
            let spread = if channel == 1 { EARLY_SPREAD } else { 0.0 };
            let time = (tap + spread) * self.size_now * self.rate + spin + predelay;
            sum += self.predelay[channel].read(time) * settings.early_gains[k];
        }
        sum
    }
}

/// One channel's input filter: a low cut (a one-pole high-pass) and a high
/// cut (a one-pole low-pass), either switched off.
#[derive(Clone, Copy, Default)]
struct InputFilter {
    low: f32,
    high: f32,
}

impl InputFilter {
    fn process(&mut self, x: f32, low_cut: bool, low_a: f32, high_cut: bool, high_a: f32) -> f32 {
        let mut y = x;
        if low_cut {
            self.low = flush(self.low + low_a * (y - self.low));
            y -= self.low;
        }
        if high_cut {
            self.high = flush(self.high + high_a * (y - self.high));
            y = self.high;
        }
        y
    }
}

/// A delay line: a ring buffer read at a fractional delay.
struct Line {
    buffer: Vec<f32>,
    write: usize,
}

impl Line {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
            write: 0,
        }
    }

    fn push(&mut self, x: f32) {
        self.buffer[self.write] = x;
        self.write = (self.write + 1) % self.buffer.len();
    }

    /// The sample `delay` frames back, interpolating between samples.
    fn read(&self, delay: f64) -> f32 {
        let len = self.buffer.len();
        let delay = delay.clamp(1.0, (len - 2) as f64);
        let position = (self.write as f64 - delay).rem_euclid(len as f64);
        let low = position.floor();
        let i = low as usize % len;
        let fraction = (position - low) as f32;
        let a = self.buffer[i];
        let b = self.buffer[(i + 1) % len];
        a + (b - a) * fraction
    }
}

/// A comb filter with the tail's damping shelves: its output feeds back
/// through a high-frequency and a low-frequency one-pole.
struct Comb {
    buffer: Vec<f32>,
    write: usize,
    /// The high damping one-pole's state.
    high: f32,
    /// The low damping one-pole's state.
    low: f32,
    /// The fractional read's allpass state.
    last: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
            write: 0,
            high: 0.0,
            low: 0.0,
            last: 0.0,
        }
    }

    /// One sample through it: the delayed output, and `x` plus the damped
    /// output at `feedback` written in.
    fn process(&mut self, x: f32, delay: f64, feedback: f32, damping: Damping) -> f32 {
        let delayed = self.read(delay);
        let mut sink = delayed;
        if damping.high_on {
            self.high = flush(self.high + damping.high_a * (sink - self.high));
            sink += damping.high * (self.high - sink);
        }
        if damping.low_on {
            self.low = flush(self.low + damping.low_a * (sink - self.low));
            sink -= damping.low * self.low;
        }
        // A frozen tail with Cut off takes in input at a feedback of 1: the clamp keeps it finite.
        let write = flush((x + feedback * sink).clamp(-COMB_LIMIT, COMB_LIMIT));
        self.buffer[self.write] = write;
        self.write = (self.write + 1) % self.buffer.len();
        delayed
    }

    /// The sample `delay` frames back. A fractional delay is read through a
    /// first-order allpass ([`fractional`]), so a loop keeps its energy and
    /// Freeze has nothing to lose; a whole-sample delay is read directly.
    fn read(&mut self, delay: f64) -> f32 {
        let len = self.buffer.len();
        let delay = delay.clamp(1.0, (len - 2) as f64);
        let whole = delay.floor();
        let fraction = delay - whole;
        let w = whole as usize;
        let new = (self.write + len - w % len) % len;
        if fraction < 1e-6 {
            self.last = 0.0;
            return self.buffer[new];
        }
        let out = fractional(
            self.buffer[new],
            self.buffer[(new + len - 1) % len],
            fraction,
            self.last,
        );
        self.last = flush(out);
        out
    }
}

/// A Schroeder allpass stage: it diffuses without colouring the tail.
struct Allpass {
    buffer: Vec<f32>,
    write: usize,
    /// The fractional read's allpass state.
    last: f32,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
            write: 0,
            last: 0.0,
        }
    }

    fn process(&mut self, x: f32, delay: f64, g: f32) -> f32 {
        let delayed = self.read(delay);
        let out = delayed - g * x;
        let write = flush(x + g * out);
        self.buffer[self.write] = write;
        self.write = (self.write + 1) % self.buffer.len();
        out
    }

    /// The sample `delay` frames back, through the same fractional read as
    /// a comb's.
    fn read(&mut self, delay: f64) -> f32 {
        let len = self.buffer.len();
        let delay = delay.clamp(1.0, (len - 2) as f64);
        let whole = delay.floor();
        let fraction = delay - whole;
        let w = whole as usize;
        let new = (self.write + len - w % len) % len;
        if fraction < 1e-6 {
            self.last = 0.0;
            return self.buffer[new];
        }
        let out = fractional(
            self.buffer[new],
            self.buffer[(new + len - 1) % len],
            fraction,
            self.last,
        );
        self.last = flush(out);
        out
    }
}

/// One fractional-delay read: a first-order allpass that delays `new` by
/// `fraction` samples past `old` without changing any frequency's level, so
/// a delay line in a feedback loop doesn't lose energy (Jaffe and Smith's
/// allpass interpolation; published method).
fn fractional(new: f32, old: f32, fraction: f64, last: f32) -> f32 {
    let eta = ((1.0 - fraction) / (1.0 + fraction)) as f32;
    eta * (new - last) + old
}

/// A sine LFO, starting at `phase`.
struct Lfo {
    phase: f64,
}

impl Lfo {
    fn new(phase: f64) -> Self {
        Self { phase }
    }

    /// Advances one frame and gives its value, from −1 to 1.
    fn step(&mut self, rate_hz: f64, sample_rate: f64) -> f64 {
        self.phase += rate_hz / sample_rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        (TAU * self.phase).sin()
    }
}
