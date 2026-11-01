//! Pedal, the native guitar pedal (WP-8.41; the pedal spec): three circuits
//! (Overdrive, Distortion and Fuzz) at four times the sample rate, or eight
//! times with Hi-Quality, then a DC filter, a four-band EQ, Output and
//! Dry/Wet.
//!
//! Gain drives the circuit; each circuit is a clipper between two filters,
//! and Fuzz's bias follows its own input's envelope. A change of type
//! crossfades. The circuits, ranges and the EQ's Qs are Beatlayer's own: the
//! manual names the controls and the EQ's frequencies but gives no numbers.

use core::f32::consts::FRAC_1_SQRT_2;
use core::f64::consts::PI;

use bl_dsp::{EnvelopeFollower, Oversampler};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::sections::Biquad;

/// Pedal's device type.
pub const PEDAL: &str = "beatlayer.pedal";

/// Type: one of [`TYPES`].
pub const TYPE: ParamId = ParamId(0);
/// Gain, per cent: the circuit's drive (0 to +48 dB).
pub const GAIN: ParamId = ParamId(1);
/// Output: the level after the EQ.
pub const OUTPUT: ParamId = ParamId(2);
/// Bass: a peak at 100 Hz, in dB.
pub const BASS: ParamId = ParamId(3);
/// Mid: a peak at [`MID_FREQUENCY`], in dB.
pub const MID: ParamId = ParamId(4);
/// Mid Frequency: one of [`MID_HZ`].
pub const MID_FREQUENCY: ParamId = ParamId(5);
/// Treble: a high shelf at 3.3 kHz, in dB.
pub const TREBLE: ParamId = ParamId(6);
/// Sub: a low shelf at 250 Hz, in dB.
pub const SUB: ParamId = ParamId(7);
/// Dry/Wet, per cent.
pub const DRY_WET: ParamId = ParamId(8);
/// Hi-Quality, a menu item: eight times the rate instead of four.
pub const HI_QUALITY: ParamId = ParamId(9);

/// The types, in the chooser's order.
pub const TYPES: [&str; 3] = ["Overdrive", "Distortion", "Fuzz"];
const OVERDRIVE: usize = 0;
const DISTORTION: usize = 1;
const FUZZ: usize = 2;

/// The Mid peak's centres, in Hz, in the switch's order.
pub const MID_HZ: [f64; 3] = [500.0, 1_000.0, 2_000.0];

/// Gain's span: 0 to this many dB of drive.
const DRIVE_DB: f32 = 48.0;
/// Doublings of the rate: 4 times, and 8 times with Hi-Quality.
const STAGES: [u32; 2] = [2, 3];
/// Overdrive's filters: a first-order high-pass before the clipper, a
/// first-order low-pass after.
const OVERDRIVE_HP_HZ: f64 = 720.0;
const OVERDRIVE_LP_HZ: f64 = 6_000.0;
/// Distortion's: a first-order high-pass before, a second-order low-pass
/// after.
const DISTORTION_HP_HZ: f64 = 100.0;
const DISTORTION_LP_HZ: f64 = 5_000.0;
/// Fuzz's first-order low-pass after the second stage.
const FUZZ_LP_HZ: f64 = 4_000.0;
/// Fuzz: the bias at rest, how far a full-scale envelope pulls it down, the
/// second stage's gain and the follower's times.
const FUZZ_BIAS: f32 = 0.3;
const FUZZ_BIAS_FALL: f32 = 0.25;
const FUZZ_SECOND_STAGE: f32 = 3.0;
const FUZZ_ATTACK_SECONDS: f64 = 0.002;
const FUZZ_RELEASE_SECONDS: f64 = 0.08;
/// A change of type crossfades over this long.
const FADE_SECONDS: f64 = 0.02;
/// The filter after the circuit that removes its DC, in Hz.
const DC_HZ: f64 = 5.0;
/// The EQ's bands: Bass and Mid peaks, Treble and Sub shelves.
const BASS_HZ: f64 = 100.0;
const BASS_Q: f64 = 0.7;
const MID_Q: f64 = 1.0;
const TREBLE_HZ: f64 = 3_300.0;
const SUB_HZ: f64 = 250.0;
const SHELF_Q: f64 = 0.71;
/// Frames between the EQ's coefficient updates, counted from the device's
/// first frame, so a render doesn't depend on its block size.
const REFRESH_FRAMES: u64 = 16;

/// The circuit's drive at `gain` per cent: g = 10^(Gain × 48 / 20).
#[must_use]
pub fn drive(gain: f32) -> f32 {
    10f32.powf(gain / 100.0 * DRIVE_DB / 20.0)
}

/// Overdrive's clipper, which keeps part of the clean signal: its slope at
/// zero is `g`.
#[must_use]
pub fn overdrive_curve(g: f32, u: f32) -> f32 {
    g * (u + (g * u).tanh()) / (1.0 + g)
}

/// Distortion's two diodes.
#[must_use]
pub fn distortion_curve(g: f32, u: f32) -> f32 {
    let v = g * u;
    v.signum() * (1.0 - (-v.abs()).exp())
}

/// Fuzz's biased transistor stage and the second stage after it.
#[must_use]
pub fn fuzz_curve(g: f32, bias: f32, u: f32) -> f32 {
    let stage = (g * (u + bias)).tanh() - (g * bias).tanh();
    (FUZZ_SECOND_STAGE * stage).tanh()
}

/// Fuzz's bias while its input's envelope is `envelope` (full scale is 1).
#[must_use]
pub fn fuzz_bias(envelope: f32) -> f32 {
    FUZZ_BIAS - FUZZ_BIAS_FALL * envelope.clamp(0.0, 1.0)
}

/// The Pedal device type.
#[must_use]
pub fn pedal_type() -> DeviceType {
    let boost = |id, name: &str, from| {
        ParamDescriptor::continuous(id, name, (from, 15.0), 0.0, ParamUnit::Decibels)
    };
    let params = vec![
        ParamDescriptor::choice(TYPE, "Type", &TYPES, OVERDRIVE)
            .describe("The circuit: Overdrive is warm and smooth, Distortion tighter and harder, Fuzz unstable and gated."),
        ParamDescriptor::continuous(GAIN, "Gain", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How hard the signal drives the circuit; even 0% distorts a little."),
        ParamDescriptor::gain(OUTPUT, "Output", 12.0, 0.0)
            .describe("The level after the EQ."),
        boost(BASS, "Bass", -15.0).describe("Boosts or cuts around 100 Hz."),
        boost(MID, "Mid", -15.0)
            .describe("Boosts or cuts around the frequency Mid Frequency chooses."),
        ParamDescriptor::choice(MID_FREQUENCY, "Mid Frequency", &["500 Hz", "1 kHz", "2 kHz"], 1)
            .describe("The centre of the Mid control."),
        boost(TREBLE, "Treble", -15.0).describe("Boosts or cuts the highs above about 3.3 kHz."),
        boost(SUB, "Sub", 0.0).describe("Boosts the lows below about 250 Hz."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the pedal's."),
        ParamDescriptor::toggle(HI_QUALITY, "Hi-Quality", false).describe(
            "Runs the circuit at eight times the rate instead of four, so its harmonics alias \
             less; it delays the output a little more.",
        ),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![LayoutNode::Band(
        ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (TYPE, Widget::Chooser),
                (GAIN, Widget::Knob),
                (BASS, Widget::Knob),
                (MID, Widget::Knob),
                (MID_FREQUENCY, Widget::Chooser),
                (TREBLE, Widget::Knob),
                (SUB, Widget::Knob),
                (OUTPUT, Widget::Knob),
                (DRY_WET, Widget::Knob),
            ],
        ),
    )]));
    DeviceType::native(PEDAL, "Pedal", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_menu_options(&[HI_QUALITY])
            .with_banks(&[
                (
                    "Pedal",
                    &[
                        "Type", "Gain", "Bass", "Mid", "Treble", "Sub", "Output", "Dry/Wet",
                    ],
                ),
                ("Mid", &["Mid Frequency"]),
            ]),
    )
}

/// Adds Pedal to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, pedal_type(), make);
}

/// Makes a Pedal for `device`'s values and `config`'s rate and block size.
/// Every buffer is made here, off the audio thread, and its latency comes
/// from the set's Hi-Quality value, so it is reported before the device
/// first runs.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Pedal {
    Pedal::new(device, config)
}

/// A first-order filter, bilinear with its corner prewarped.
#[derive(Clone, Copy, Default)]
struct OnePole {
    b0: f64,
    b1: f64,
    a1: f64,
    x1: f64,
    y1: f64,
}

impl OnePole {
    fn low_pass(hz: f64, rate: f64) -> Self {
        let k = (PI * hz / rate).tan();
        Self {
            b0: k / (1.0 + k),
            b1: k / (1.0 + k),
            a1: (k - 1.0) / (1.0 + k),
            ..Self::default()
        }
    }

    fn high_pass(hz: f64, rate: f64) -> Self {
        let k = (PI * hz / rate).tan();
        Self {
            b0: 1.0 / (1.0 + k),
            b1: -1.0 / (1.0 + k),
            a1: (k - 1.0) / (1.0 + k),
            ..Self::default()
        }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 - self.a1 * self.y1;
        self.x1 = x;
        // Flushed so a silence doesn't decay into denormals.
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        y
    }
}

/// The three circuits' filters and Fuzz's follower, for one channel at the
/// high rate.
struct Circuit {
    rate: f64,
    overdrive: [OnePole; 2],
    distortion_hp: OnePole,
    distortion_lp: Biquad,
    fuzz_lp: OnePole,
    envelope: EnvelopeFollower,
}

impl Circuit {
    fn new(rate: f64) -> Self {
        let mut envelope = EnvelopeFollower::new(rate);
        envelope.set(FUZZ_ATTACK_SECONDS, FUZZ_RELEASE_SECONDS, false);
        let mut distortion_lp = Biquad::default();
        distortion_lp.c = coeffs(
            Type::HighCut12,
            DISTORTION_LP_HZ,
            0.0,
            f64::from(FRAC_1_SQRT_2),
            rate,
        );
        Self {
            rate,
            overdrive: [
                OnePole::high_pass(OVERDRIVE_HP_HZ, rate),
                OnePole::low_pass(OVERDRIVE_LP_HZ, rate),
            ],
            distortion_hp: OnePole::high_pass(DISTORTION_HP_HZ, rate),
            distortion_lp,
            fuzz_lp: OnePole::low_pass(FUZZ_LP_HZ, rate),
            envelope,
        }
    }

    /// Clears what `kind` remembers, as it starts to play.
    fn reset(&mut self, kind: usize) {
        let fresh = Self::new(self.rate);
        match kind {
            OVERDRIVE => self.overdrive = fresh.overdrive,
            DISTORTION => {
                self.distortion_hp = fresh.distortion_hp;
                self.distortion_lp = fresh.distortion_lp;
            }
            _ => {
                self.fuzz_lp = fresh.fuzz_lp;
                self.envelope = fresh.envelope;
            }
        }
    }

    /// One sample through circuit `kind` at drive `g`.
    fn tick(&mut self, kind: usize, g: f32, x: f32) -> f32 {
        match kind {
            OVERDRIVE => {
                let u = self.overdrive[0].process(f64::from(x)) as f32;
                self.overdrive[1].process(f64::from(overdrive_curve(g, u))) as f32
            }
            DISTORTION => {
                let u = self.distortion_hp.process(f64::from(x)) as f32;
                self.distortion_lp.process(distortion_curve(g, u))
            }
            _ => {
                let bias = fuzz_bias(self.envelope.process(x) as f32);
                self.fuzz_lp.process(f64::from(fuzz_curve(g, bias, x))) as f32
            }
        }
    }
}

/// One channel's circuit, and the crossfade from the type before.
struct Voice {
    circuit: Circuit,
    kind: usize,
    from: usize,
    /// Samples of the fade still to play.
    left: f64,
}

impl Voice {
    fn new(kind: usize, rate: f64) -> Self {
        Self {
            circuit: Circuit::new(rate),
            kind,
            from: kind,
            left: 0.0,
        }
    }

    /// Notes the type the stretch plays: a change starts a fade of `fade`
    /// samples from the type before.
    fn sync(&mut self, kind: usize, fade: f64) {
        if kind != self.kind {
            self.from = self.kind;
            self.kind = kind;
            self.circuit.reset(kind);
            self.left = fade;
        }
    }

    fn tick(&mut self, g: f32, x: f32, fade: f64) -> f32 {
        let now = self.circuit.tick(self.kind, g, x);
        if self.left <= 0.0 {
            return now;
        }
        let from = self.circuit.tick(self.from, g, x);
        let t = (1.0 - self.left / fade) as f32;
        self.left -= 1.0;
        from + (now - from) * t
    }
}

/// One channel's filters after the circuit: the DC filter and the EQ.
#[derive(Clone, Copy, Default)]
struct Tone {
    dc: OnePole,
    /// Bass, Mid, Treble, Sub.
    bands: [Biquad; 4],
}

/// Pedal's audio-thread half.
pub struct Pedal {
    rate: f64,
    /// By quality (0 for four times the rate, 1 for eight), then channel.
    oversamplers: [[Oversampler; 2]; 2],
    voices: [[Voice; 2]; 2],
    tones: [Tone; 2],
    /// The input as it came, for the dry signal, and the signal on its way
    /// through the circuit.
    dry: [Vec<f32>; 2],
    work: [Vec<f32>; 2],
    /// The drive at each frame of the stretch.
    gains: Vec<f32>,
    delay: [DelayLine; 2],
    quality: usize,
    latency: u32,
    /// Frames processed so far, which the EQ's refresh counts.
    ticks: u64,
}

impl Pedal {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        let rate = config.sample_rate.as_f64();
        let oversamplers =
            STAGES.map(|stages| [(); 2].map(|()| Oversampler::new(stages, 2, frames)));
        let quality = usize::from(device.params.get(&HI_QUALITY).copied().unwrap_or(0.0) >= 0.5);
        let kind = (device.params.get(&TYPE).copied().unwrap_or(0.0) as usize).min(FUZZ);
        let buffer = || vec![0.0f32; frames];
        let max_latency = oversamplers[1][0].latency() as usize;
        let tone = Tone {
            dc: OnePole::high_pass(DC_HZ, rate),
            ..Tone::default()
        };
        Self {
            rate,
            latency: oversamplers[quality][0].latency(),
            voices: [0, 1].map(|q| {
                [(); 2].map(|()| Voice::new(kind, rate * oversamplers[q][0].factor() as f64))
            }),
            oversamplers,
            tones: [tone; 2],
            dry: [buffer(), buffer()],
            work: [buffer(), buffer()],
            gains: buffer(),
            delay: [DelayLine::new(max_latency), DelayLine::new(max_latency)],
            quality,
            ticks: 0,
        }
    }

    /// Switches to `quality`, starting its oversamplers and circuits clean.
    fn change_quality(&mut self, quality: usize, kind: usize) {
        self.quality = quality;
        let high_rate = self.rate * self.oversamplers[quality][0].factor() as f64;
        for oversampler in &mut self.oversamplers[quality] {
            oversampler.reset();
        }
        self.voices[quality] = [(); 2].map(|()| Voice::new(kind, high_rate));
        self.latency = self.oversamplers[quality][0].latency();
    }
}

impl NativeDevice for Pedal {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let value = |id: ParamId| params.value(id.0 as usize);
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        let kind = (value(TYPE) as usize).min(FUZZ);
        let quality = usize::from(value(HI_QUALITY) >= 0.5);
        if quality != self.quality {
            self.change_quality(quality, kind);
        }
        let mid_hz = MID_HZ[(value(MID_FREQUENCY) as usize).min(MID_HZ.len() - 1)];
        let (bass, mid, treble, sub) = (ramp(BASS), ramp(MID), ramp(TREBLE), ramp(SUB));
        let (gain, output, mix) = (ramp(GAIN), ramp(OUTPUT), ramp(DRY_WET));
        for (g, percent) in self.gains[..frames].iter_mut().zip(gain) {
            *g = drive(*percent);
        }

        // 1. The input to the circuit, cleaned of anything that isn't a
        // number.
        for channel in 0..channels {
            let input = audio.channel(channel);
            self.dry[channel][..frames].copy_from_slice(input);
            for (work, x) in self.work[channel][..frames].iter_mut().zip(input) {
                *work = if x.is_finite() { *x } else { 0.0 };
            }
        }

        // 2. The circuit, at four or eight times the rate.
        let oversamplers = &mut self.oversamplers[quality];
        let factor = oversamplers[0].factor();
        let fade = (FADE_SECONDS * self.rate * factor as f64).round();
        let gains = &self.gains[..frames];
        let channel_state = self.voices[quality].iter_mut().zip(oversamplers.iter_mut());
        for (channel, (voice, oversampler)) in channel_state.enumerate().take(channels) {
            voice.sync(kind, fade);
            let mut frame = 0;
            oversampler.process(channel, &mut self.work[channel][..frames], |high| {
                for (k, sample) in high.iter_mut().enumerate() {
                    let g = gains[(frame + k / factor).min(frames - 1)];
                    *sample = voice.tick(g, *sample, fade);
                }
                frame += high.len() / factor;
            });
        }

        // 3. The DC filter, the EQ, Output and the mix.
        for channel in 0..channels {
            let tone = &mut self.tones[channel];
            let work = &self.work[channel][..frames];
            let dry = &self.dry[channel][..frames];
            let delay = &mut self.delay[channel];
            let out = audio.channel_mut(channel);
            for i in 0..frames {
                if (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES) {
                    let rate = self.rate;
                    let [b, m, t, s] = &mut tone.bands;
                    b.c = coeffs(Type::Peak, BASS_HZ, f64::from(bass[i]), BASS_Q, rate);
                    m.c = coeffs(Type::Peak, mid_hz, f64::from(mid[i]), MID_Q, rate);
                    t.c = coeffs(
                        Type::HighShelf,
                        TREBLE_HZ,
                        f64::from(treble[i]),
                        SHELF_Q,
                        rate,
                    );
                    s.c = coeffs(Type::LowShelf, SUB_HZ, f64::from(sub[i]), SHELF_Q, rate);
                }
                let mut v = tone.dc.process(f64::from(work[i])) as f32;
                for band in &mut tone.bands {
                    v = band.process(v);
                }
                v *= output[i];
                let dry_now = delay.read(self.latency as usize);
                delay.write(dry[i]);
                out[i] = dry_now + (v - dry_now) * (mix[i] / 100.0);
            }
        }
        self.ticks += frames as u64;
    }

    fn latency(&self) -> u32 {
        self.latency
    }
}
