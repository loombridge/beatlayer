//! Drum Buss, the native drum processor (WP-5.22b; the drum-bus spec): Trim,
//! a fixed compressor, three distortions with Drive, Crunch, Damp,
//! Transients, a tuned low-end resonator (Boom, Freq, Force To Note, Decay,
//! Boom Audition, a Bass Meter), Dry/Wet and Output.
//!
//! The signal runs through the trim, the compressor while Comp is on, a
//! Drive gain into the chosen shape, Crunch on the mid-highs, Damp's
//! low-pass, Transients above 100 Hz, and a resonator at Freq added by
//! Boom; Dry/Wet then mixes it with the dry input and Output scales it. The
//! curves, ranges, crossovers and times the manual doesn't give are
//! Beatlayer's own (see `docs/specs/drum-bus.md`).

use core::f64::consts::TAU;

use bl_dsp::EnvelopeFollower;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::dynamics::{curve_db, level_db, step_of};
use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::{Biquad, Coeffs, REFRESH_FRAMES};

/// Drum Buss's device type.
pub const DRUM_BUSS: &str = "beatlayer.buss";

/// Trim: the input level before any processing, a gain to unity.
pub const TRIM: ParamId = ParamId(0);
/// Comp: whether the fixed compressor runs before the distortion.
pub const COMP: ParamId = ParamId(1);
/// Type: Soft, Medium or Hard.
pub const TYPE: ParamId = ParamId(2);
/// Drive: how much the input is driven before distorting.
pub const DRIVE: ParamId = ParamId(3);
/// Crunch: the sine-shaped distortion added to the mid-highs.
pub const CRUNCH: ParamId = ParamId(4);
/// Damp: the low-pass after the distortion.
pub const DAMP: ParamId = ParamId(5);
/// Transients: shapes the band above 100 Hz.
pub const TRANSIENTS: ParamId = ParamId(6);
/// Boom: how much low-end enhancement the resonator adds.
pub const BOOM: ParamId = ParamId(7);
/// Freq: the resonator's frequency.
pub const FREQ: ParamId = ParamId(8);
/// Force To Note: whether Freq snaps to the nearest equal-tempered note.
pub const FORCE_TO_NOTE: ParamId = ParamId(9);
/// Decay: the resonator's ring-down time.
pub const DECAY: ParamId = ParamId(10);
/// Boom Audition: whether the device outputs only the low-end enhancer.
pub const BOOM_AUDITION: ParamId = ParamId(11);
/// Dry/Wet: the balance between the processed and the dry signals.
pub const DRY_WET: ParamId = ParamId(12);
/// Output: the output level, a gain reaching silence.
pub const OUTPUT: ParamId = ParamId(13);

/// The parameters', descriptors', and bank's order.
const TRIM_INDEX: usize = 0;
const COMP_INDEX: usize = 1;
const TYPE_INDEX: usize = 2;
const DRIVE_INDEX: usize = 3;
const CRUNCH_INDEX: usize = 4;
const DAMP_INDEX: usize = 5;
const TRANSIENTS_INDEX: usize = 6;
const BOOM_INDEX: usize = 7;
const FREQ_INDEX: usize = 8;
const FORCE_TO_NOTE_INDEX: usize = 9;
const DECAY_INDEX: usize = 10;
const BOOM_AUDITION_INDEX: usize = 11;
const DRY_WET_INDEX: usize = 12;
const OUTPUT_INDEX: usize = 13;

/// The Type choices' indices.
const TYPE_MEDIUM: usize = 1;
const TYPE_HARD: usize = 2;

/// The fixed compressor's settings (Beatlayer's own, the manual's
/// "fast attack, medium release, a moderate ratio, ample makeup").
const COMP_THRESHOLD_DB: f64 = -12.0;
const COMP_RATIO: f64 = 4.0;
const COMP_ATTACK_MS: f64 = 1.0;
const COMP_RELEASE_MS: f64 = 100.0;

/// The Drive gain at 0 % and 100 %, in dB.
const DRIVE_TOP_DB: f64 = 24.0;

/// The Hard distortions' bass boost: a low shelf at 100 Hz, in dB.
const HARD_SHELF_HZ: f64 = 100.0;
const HARD_SHELF_DB: f64 = 6.0;

/// Crunch's drive at 100 %, in dB, and the crossover below which the band
/// passes unchanged, in Hz.
const CRUNCH_TOP_DB: f64 = 18.0;
const CRUNCH_HZ: f64 = 200.0;

/// Damp's corner at 0 % and 100 %, in Hz.
const DAMP_OPEN_HZ: f64 = 20_000.0;
const DAMP_CLOSED_HZ: f64 = 200.0;

/// Transients' crossover, in Hz, its two followers' times, in seconds, and
/// how much the attack and the sustain are shaped at full settings.
const TRANSIENT_HZ: f64 = 100.0;
const TRANSIENT_FAST_ATTACK: f64 = 0.001;
const TRANSIENT_FAST_RELEASE: f64 = 0.020;
const TRANSIENT_SLOW_ATTACK: f64 = 0.030;
const TRANSIENT_SLOW_RELEASE: f64 = 0.150;
const TRANSIENT_ATTACK_GAIN: f32 = 2.0;
const TRANSIENT_SUSTAIN_GAIN: f32 = 0.5;
/// The most either follower's level shapes the band.
const TRANSIENT_GAIN_LIMITS: (f32, f32) = (0.0, 8.0);

/// The Bass Meter's floor, in dB, for a low-end enhancer at rest.
const METER_FLOOR_DB: f32 = -120.0;

/// The linkwitz-Riley fourth-order crossover's section Qs.
const LR4_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// The Drum Buss device type.
#[must_use]
pub fn drum_buss_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::gain(TRIM, "Trim", 0.0, 0.0)
            .describe("Sets the input level before any processing; down to silence."),
        ParamDescriptor::toggle(COMP, "Comp", false).describe(
            "Runs a fixed compressor before the distortion, with a fast attack, a \
             medium release, a moderate ratio and ample makeup gain.",
        ),
        ParamDescriptor::choice(TYPE, "Type", &["Soft", "Medium", "Hard"], 0).describe(
            "The distortion's character: Soft waveshapes, Medium limits, Hard clips \
             with a bass boost.",
        ),
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much the input is driven before distorting."),
        ParamDescriptor::continuous(CRUNCH, "Crunch", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Adds a sine-shaped distortion to mid-high frequencies."),
        ParamDescriptor::continuous(DAMP, "Damp", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Low-passes the signal after the distortion, removing the highs it added."),
        ParamDescriptor::continuous(
            TRANSIENTS,
            "Transients",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe(
            "Shapes the transients above 100 Hz: positive adds attack and sustain, \
             negative adds attack with less sustain.",
        ),
        ParamDescriptor::continuous(BOOM, "Boom", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much low-end enhancement the tuned resonator adds."),
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 200.0), 60.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The frequency of the low-end enhancer."),
        ParamDescriptor::toggle(FORCE_TO_NOTE, "Force To Note", false)
            .describe("Tunes the low-end enhancer to the nearest note at A = 440 Hz."),
        ParamDescriptor::continuous(
            DECAY,
            "Decay",
            (10.0, 2_000.0),
            200.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The decay rate of the low frequencies, and the enhancer's ring-down time."),
        ParamDescriptor::toggle(BOOM_AUDITION, "Boom Audition", false)
            .describe("Solo the result of the low-frequency enhancer."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the processed and the dry signals."),
        ParamDescriptor::gain(OUTPUT, "Output", 24.0, 0.0)
            .describe("The output level, down to silence."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(TRIM, Widget::Knob), (COMP, Widget::Toggle)],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Bass Meter", 150.0, BODY_HEIGHT, 1.0)
                .with_top(ControlBand::stacked(
                    60.0,
                    &[(BOOM, Widget::Knob), (FREQ, Widget::Knob)],
                ))
                .with_bottom(ControlBand::stacked(
                    76.0,
                    &[
                        (FORCE_TO_NOTE, Widget::Toggle),
                        (BOOM_AUDITION, Widget::Toggle),
                    ],
                )),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (TYPE, Widget::Chooser),
                (DRIVE, Widget::Knob),
                (CRUNCH, Widget::Knob),
                (DAMP, Widget::Knob),
                (TRANSIENTS, Widget::Knob),
                (DECAY, Widget::Knob),
                (DRY_WET, Widget::Knob),
                (OUTPUT, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(DRUM_BUSS, "Drum Buss", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Trim", "Comp", "Type", "Drive", "Crunch", "Damp", "Dry/Wet", "Output",
                    ],
                ),
                (
                    "Boom",
                    &[
                        "Transients",
                        "Boom",
                        "Freq",
                        "Force To Note",
                        "Decay",
                        "Boom Audition",
                    ],
                ),
            ]),
    )
}

/// Adds Drum Buss to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, drum_buss_type(), make);
}

/// Makes a Drum Buss for `config`'s rate. Every filter state and buffer is
/// made here, off the audio thread.
#[must_use]
pub fn make(_device: &Device, config: &NodeConfig) -> Buss {
    Buss::new(config)
}

/// The Drive gain for `percent`.
fn drive_gain(percent: f32) -> f32 {
    db_to_gain(DRIVE_TOP_DB * f64::from(percent) / 100.0) as f32
}

/// The driven gain for Crunch `amount` from 0 to 1.
fn crunch_gain(amount: f32) -> f32 {
    db_to_gain(CRUNCH_TOP_DB * f64::from(amount)) as f32
}

/// Damp's corner for `percent`.
fn damp_corner(percent: f64) -> f64 {
    DAMP_OPEN_HZ * (DAMP_CLOSED_HZ / DAMP_OPEN_HZ).powf(percent / 100.0)
}

/// The nearest equal-tempered note's frequency to `hz`, at A = 440 Hz.
#[must_use]
pub fn nearest_note_hz(hz: f64) -> f64 {
    let semitones = (12.0 * (hz / 440.0).log2()).round();
    440.0 * (semitones / 12.0).exp2()
}

/// One distortion's shape for the driven `x`.
fn shape(kind: usize, x: f32) -> f32 {
    match kind {
        TYPE_MEDIUM => x / (1.0 + x.abs()),
        TYPE_HARD => x.clamp(-1.0, 1.0),
        _ => x.tanh(),
    }
}

/// A linkwitz-Riley fourth-order split at one frequency: a low-pass and a
/// high-pass of two Butterworth sections each.
#[derive(Clone, Copy, Default)]
struct Split {
    low: [Biquad; 2],
    high: [Biquad; 2],
}

impl Split {
    fn set(&mut self, hz: f64, rate: f64) {
        for i in 0..2 {
            self.low[i].c = coeffs(Type::HighCut12, hz, 0.0, LR4_Q, rate);
            self.high[i].c = coeffs(Type::LowCut12, hz, 0.0, LR4_Q, rate);
        }
    }

    /// The low and high bands of `x`; sum flat in magnitude.
    fn split(&mut self, x: f32) -> (f32, f32) {
        let first = self.low[0].process(x);
        let low = self.low[1].process(first);
        let first = self.high[0].process(x);
        let high = self.high[1].process(first);
        (low, high)
    }
}

/// The resonator's coefficients: a band-pass of 0 dB peak gain at `hz`
/// whose ring-down time constant is `decay` seconds (its Q is π·f·decay, so
/// its poles' radius gives the time constant).
fn resonator_coeffs(hz: f64, decay_seconds: f64, rate: f64) -> Coeffs {
    crate::sections::count_design();
    let w = TAU * (hz / rate).clamp(1.0e-6, 0.49);
    let q = std::f64::consts::PI * hz * decay_seconds.max(1.0e-5);
    let (sin, cos) = w.sin_cos();
    let alpha = sin / (2.0 * q.max(1.0e-4));
    let a0 = 1.0 + alpha;
    Coeffs {
        b0: (alpha / a0) as f32,
        b1: 0.0,
        b2: (-alpha / a0) as f32,
        a1: (-2.0 * cos / a0) as f32,
        a2: ((1.0 - alpha) / a0) as f32,
    }
}

/// Drum Buss's audio-thread half.
pub struct Buss {
    rate: f64,
    /// The fixed compressor's slope, its makeup gain and its envelope.
    comp_slope: f64,
    makeup: f32,
    comp_attack: f32,
    comp_release: f32,
    comp_gain: f32,
    /// The Damp low-pass and the Hard distortions' bass shelf per channel.
    damp: [Biquad; 2],
    hard_shelf: [Biquad; 2],
    /// The Crunch and Transients splits, and the Transients followers.
    crunch_split: [Split; 2],
    transient_split: [Split; 2],
    transient_fast: [EnvelopeFollower; 2],
    transient_slow: [EnvelopeFollower; 2],
    /// The low-end enhancer's resonators.
    resonators: [Biquad; 2],
    /// The Damp corner and the resonator's (Hz, seconds) the sections were
    /// last designed for; a refresh that finds them unchanged keeps them.
    last_corner: Option<f64>,
    last_resonator: Option<(f64, f64)>,
    /// Frames processed so far, which the coefficient refresh counts.
    ticks: u64,
    /// The low-end enhancer's loudest sample since the display last
    /// published.
    meter: f32,
}

impl Buss {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let follower = |attack, release| {
            let mut follower = EnvelopeFollower::new(rate);
            follower.set(attack, release, false);
            follower
        };
        let shelf = coeffs(Type::LowShelf, HARD_SHELF_HZ, HARD_SHELF_DB, LR4_Q, rate);
        let mut hard_shelf = [Biquad::default(); 2];
        let mut crunch_split = [Split::default(); 2];
        let mut transient_split = [Split::default(); 2];
        for channel in 0..2 {
            hard_shelf[channel].c = shelf;
            crunch_split[channel].set(CRUNCH_HZ, rate);
            transient_split[channel].set(TRANSIENT_HZ, rate);
        }
        Self {
            rate,
            comp_slope: 1.0 - 1.0 / COMP_RATIO,
            makeup: db_to_gain(COMP_THRESHOLD_DB.abs() * (1.0 - 1.0 / COMP_RATIO)) as f32,
            comp_attack: step_of(COMP_ATTACK_MS, rate),
            comp_release: step_of(COMP_RELEASE_MS, rate),
            comp_gain: 1.0,
            damp: [Biquad::default(); 2],
            hard_shelf,
            crunch_split,
            transient_split,
            transient_fast: [
                follower(TRANSIENT_FAST_ATTACK, TRANSIENT_FAST_RELEASE),
                follower(TRANSIENT_FAST_ATTACK, TRANSIENT_FAST_RELEASE),
            ],
            transient_slow: [
                follower(TRANSIENT_SLOW_ATTACK, TRANSIENT_SLOW_RELEASE),
                follower(TRANSIENT_SLOW_ATTACK, TRANSIENT_SLOW_RELEASE),
            ],
            resonators: [Biquad::default(); 2],
            last_corner: None,
            last_resonator: None,
            ticks: 0,
            meter: 0.0,
        }
    }
}

impl NativeDevice for Buss {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let ramp = |index: usize| params.ramp(index);
        let (trim, drive, crunch, damp, transients, boom, freq, decay, mix, out) = (
            ramp(TRIM_INDEX),
            ramp(DRIVE_INDEX),
            ramp(CRUNCH_INDEX),
            ramp(DAMP_INDEX),
            ramp(TRANSIENTS_INDEX),
            ramp(BOOM_INDEX),
            ramp(FREQ_INDEX),
            ramp(DECAY_INDEX),
            ramp(DRY_WET_INDEX),
            ramp(OUTPUT_INDEX),
        );
        let kind = (params.value(TYPE_INDEX) as usize).min(TYPE_HARD);
        let comp = params.value(COMP_INDEX) >= 0.5;
        let force_to_note = params.value(FORCE_TO_NOTE_INDEX) >= 0.5;
        let audition = params.value(BOOM_AUDITION_INDEX) >= 0.5;

        let mut input = [0.0f32; 2];
        for i in 0..frames {
            if (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES as u64) {
                let hz = f64::from(freq[i]);
                let hz = if force_to_note {
                    nearest_note_hz(hz)
                } else {
                    hz
                };
                let seconds = f64::from(decay[i]) * 0.001;
                let corner = damp_corner(f64::from(damp[i]));
                let corner_changed = self.last_corner != Some(corner);
                let resonator_changed = self.last_resonator != Some((hz, seconds));
                self.last_corner = Some(corner);
                self.last_resonator = Some((hz, seconds));
                for channel in 0..channels {
                    if corner_changed {
                        self.damp[channel].c =
                            coeffs(Type::HighCut12, corner, 0.0, LR4_Q, self.rate);
                    }
                    if resonator_changed {
                        self.resonators[channel].c = resonator_coeffs(hz, seconds, self.rate);
                    }
                }
            }

            for (slot, samples) in audio.channels_mut().enumerate().take(channels) {
                input[slot] = samples[i];
            }
            let mut work = [0.0f32; 2];
            for (channel, value) in work.iter_mut().enumerate().take(channels) {
                *value = input[channel] * trim[i];
            }

            // The fixed compressor, before the distortion.
            if comp {
                let level = work[..channels]
                    .iter()
                    .fold(0.0f32, |most, value| most.max(value.abs()));
                let db = level_db(f64::from(level));
                let gain_db = curve_db(db, COMP_THRESHOLD_DB, self.comp_slope, 0.0, false);
                let target = db_to_gain(gain_db) as f32 * self.makeup;
                let step = if target < self.comp_gain {
                    self.comp_attack
                } else {
                    self.comp_release
                };
                self.comp_gain += (target - self.comp_gain) * step;
                for value in work.iter_mut().take(channels) {
                    *value *= self.comp_gain;
                }
            }

            // The distortion, then Crunch on the mid-highs and Damp.
            let gain = drive_gain(drive[i]);
            let amount = crunch[i] / 100.0;
            let crunch_gain = crunch_gain(amount);
            for (channel, value) in work.iter_mut().enumerate().take(channels) {
                let driven = gain * *value;
                let shaped = if kind == TYPE_HARD {
                    shape(kind, self.hard_shelf[channel].process(driven))
                } else {
                    shape(kind, driven)
                };
                let mut out = shaped;
                if amount > 0.0 {
                    let (low, high) = self.crunch_split[channel].split(out);
                    // A sine shaper with unity slope at rest, so the band
                    // below the crossover is untouched as the amount rises.
                    let folded = (crunch_gain * high).sin() / crunch_gain;
                    out = low + (1.0 - amount) * high + amount * folded;
                }
                *value = self.damp[channel].process(out);
            }

            // Transients on the band above 100 Hz.
            let t = transients[i] / 100.0;
            for (channel, value) in work.iter_mut().enumerate().take(channels) {
                let (low, high) = self.transient_split[channel].split(*value);
                let fast = self.transient_fast[channel].process(high) as f32;
                let slow = self.transient_slow[channel].process(high) as f32;
                let excess = ((fast - slow) / (slow + 1e-6)).clamp(0.0, 1.0);
                let gain = ((1.0 + t.abs() * TRANSIENT_ATTACK_GAIN * excess)
                    * (1.0 + t * TRANSIENT_SUSTAIN_GAIN))
                    .clamp(TRANSIENT_GAIN_LIMITS.0, TRANSIENT_GAIN_LIMITS.1);
                *value = low + high * gain;
            }

            // The low-end enhancer: the resonator added by Boom.
            let boom_amount = boom[i] / 100.0;
            let mut enhancer = [0.0f32; 2];
            for (channel, value) in enhancer.iter_mut().enumerate().take(channels) {
                let ring = self.resonators[channel].process(work[channel]);
                *value = ring * boom_amount;
                self.meter = self.meter.max(value.abs());
            }

            // Boom Audition, else Dry/Wet, then Output.
            let wet = mix[i] / 100.0;
            for (channel, samples) in audio.channels_mut().enumerate().take(channels) {
                let value = if audition {
                    enhancer[channel]
                } else {
                    (work[channel] + enhancer[channel]) * wet + input[channel] * (1.0 - wet)
                };
                samples[i] = value * out[i];
            }
        }
        self.ticks += frames as u64;
    }

    fn display_len(&self) -> usize {
        1
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = if self.meter > 1e-6 {
            20.0 * self.meter.log10()
        } else {
            METER_FLOOR_DB
        };
        self.meter = 0.0;
    }
}

/// The device's display (the drum-bus spec, R13): the Bass Meter as a
/// reading, once an engine publishes it.
#[must_use]
pub fn display(
    device: DeviceId,
    _description: &DeviceDescription,
    _values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let readings = data
        .filter(|d| d.len() == 1)
        .map(|level| {
            vec![DisplayReading {
                name: "Bass Meter".to_owned(),
                value: level[0],
                unit: "dB".to_owned(),
                text: format!("{:.1} dB", level[0]),
            }]
        })
        .unwrap_or_default();
    DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings,
    }
}

#[cfg(test)]
mod tests {
    use bl_engine::{NodeEvent, ParamValue};

    use super::*;
    use crate::sections::{designs_in_block, test_node};

    // Spec: drum-bus R10
    #[test]
    fn force_to_note_snaps_to_the_nearest_equal_tempered_note() {
        assert!((nearest_note_hz(440.0) - 440.0).abs() < 1e-9);
        assert!((nearest_note_hz(110.0) - 110.0).abs() < 1e-9);
        // 200 Hz is nearer G3 (196 Hz) than G#3 (207.7 Hz).
        assert!((nearest_note_hz(200.0) - 196.0).abs() < 0.1);
    }

    // Spec: drum-bus R15
    #[test]
    fn a_refresh_with_nothing_moved_designs_nothing() {
        let mut node = test_node(register, &drum_buss_type(), &[]);
        designs_in_block(node.as_mut(), &[]);
        for _ in 0..16 {
            assert_eq!(designs_in_block(node.as_mut(), &[]), 0, "static parameters");
        }
        let moved = NodeEvent {
            offset: 0,
            param: FREQ,
            value: ParamValue::Set(90.0),
        };
        assert!(
            designs_in_block(node.as_mut(), &[moved]) > 0,
            "a moved Freq"
        );
    }
}
