//! Bass, the native monophonic virtual analogue synthesiser (WP-8.16; the
//! bass and Poli spec, R1 to R8, R15, R18): one voice with legato glide, an
//! oscillator that morphs from saw to square with a sub and noise, the
//! instruments' low-pass at 24 dB per octave with its circuits and an
//! attack-decay envelope, the amplifier envelope, and a distortion stage.
//!
//! It plays through the instruments' voice section, which keeps the one
//! voice, the glide and the bends, and filters through the instruments'
//! filter. The pack page names no controls, so every control, range and
//! law is Beatlayer's own, as the spec says.

use core::ops::Range;

use bl_dsp::random::Random;
use bl_engine::{DeviceType, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT, Widget,
};
use bl_model::param::db_to_gain;
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};
use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams};
use lspk_dsp::filter::pitch_of;
use lspk_voice::{VoiceBlock, VoiceEvent, VoiceEventKind};
use lspk_wavetable::osc::{
    Inputs, Layout as OscLayout, Oscillator, Params as OscParams, PhaseMode,
};
use lspk_wavetable::{Cache, FRAME_SIZE, SharedTable, Wavetable};

use crate::analogue::{self, Filtering, MIDDLE_C, REFERENCE};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::{Instrument, Parts, Section, State};
use crate::instrument_filter::{CIRCUITS, Setup};

/// Bass's device type.
pub const BASS: &str = "beatlayer.bass";

/// The oscillator's morph from saw to square, in per cent.
pub const SHAPE: ParamId = ParamId(0);
/// The oscillator's octave.
pub const OCTAVE: ParamId = ParamId(1);
/// The oscillator's tuning, in semitones.
pub const TUNE: ParamId = ParamId(2);
/// The sub oscillator's gain.
pub const SUB: ParamId = ParamId(3);
/// The noise's gain.
pub const NOISE: ParamId = ParamId(4);
/// The glide time, in milliseconds.
pub const GLIDE: ParamId = ParamId(5);
/// The filter's circuit.
pub const CIRCUIT: ParamId = ParamId(6);
/// The filter's cutoff, in hertz.
pub const FREQ: ParamId = ParamId(7);
/// The filter's resonance, in per cent.
pub const RES: ParamId = ParamId(8);
/// The filter's drive, in per cent.
pub const DRIVE: ParamId = ParamId(9);
/// How much the key moves the cutoff, in per cent.
pub const KEY: ParamId = ParamId(10);
/// The filter envelope's attack, in milliseconds.
pub const FILTER_ATTACK: ParamId = ParamId(11);
/// The filter envelope's decay, in milliseconds.
pub const FILTER_DECAY: ParamId = ParamId(12);
/// How far the filter envelope moves the cutoff, in per cent of six octaves.
pub const ENV_AMOUNT: ParamId = ParamId(13);
/// The amplifier envelope's attack, in milliseconds.
pub const ATTACK: ParamId = ParamId(14);
/// The amplifier envelope's decay, in milliseconds.
pub const DECAY: ParamId = ParamId(15);
/// The amplifier envelope's sustain, in per cent.
pub const SUSTAIN: ParamId = ParamId(16);
/// The amplifier envelope's release, in milliseconds.
pub const RELEASE: ParamId = ParamId(17);
/// How much the velocity moves the volume, in per cent.
pub const VEL_VOL: ParamId = ParamId(18);
/// The output level.
pub const VOLUME: ParamId = ParamId(19);
/// The distortion after the amplifier, in per cent.
pub const DISTORTION: ParamId = ParamId(20);
/// The pitch wheel's range, in semitones.
pub const PB_RANGE: ParamId = ParamId(21);

/// Spec: bass and Poli R7. The filter envelope's reach at full Env Amount,
/// in semitones: six octaves.
const ENV_SEMITONES: f32 = 72.0;
/// Spec: bass and Poli R8. The distortion's drive at 100 %, in decibels.
const DRIVE_DB: f32 = 36.0;
/// The one voice.
const VOICES: usize = 1;

/// A saw and a square: the oscillator's two frames, which Shape fades
/// between. The saw falls, so its fundamental and the square's agree in
/// sign and the morph never cancels one against the other.
fn morph_table() -> SharedTable {
    let table = Wavetable::from_fn(2, |frame, sample| {
        let p = sample as f64 / FRAME_SIZE as f64;
        if frame == 0 {
            (1.0 - 2.0 * p) as f32
        } else if p < 0.5 {
            1.0
        } else {
            -1.0
        }
    })
    .expect("a table of whole frames");
    Cache::global().share(table)
}

/// One sine, the sub oscillator's.
fn sine_table() -> SharedTable {
    let table = Wavetable::from_fn(1, |_, sample| {
        (core::f64::consts::TAU * sample as f64 / FRAME_SIZE as f64).sin() as f32
    })
    .expect("a table of whole frames");
    Cache::global().share(table)
}

/// Bass's device type.
#[must_use]
pub fn bass_type() -> DeviceType {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 10_000.0), default, ParamUnit::Milliseconds)
            .power(3)
    };
    let percent = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let params = vec![
        percent(SHAPE, "Shape", 0.0)
            .describe("Morphs the oscillator from a saw (0 %) to a square (100 %)."),
        ParamDescriptor::integer(OCTAVE, "Octave", (-2, 1), -1, ParamUnit::None)
            .decimals(0)
            .describe("Transposes the oscillator in octaves."),
        ParamDescriptor::continuous(TUNE, "Tune", (-12.0, 12.0), 0.0, ParamUnit::Semitones)
            .describe("Tunes the oscillator, in semitones."),
        ParamDescriptor::gain(SUB, "Sub", 0.0, f64::NEG_INFINITY)
            .describe("Sets the level of a sine an octave below the oscillator, down to silence."),
        ParamDescriptor::gain(NOISE, "Noise", 0.0, f64::NEG_INFINITY)
            .describe("Sets the level of white noise, down to silence."),
        ParamDescriptor::continuous(
            GLIDE,
            "Glide",
            (0.0, 2_000.0),
            60.0,
            ParamUnit::Milliseconds,
        )
        .power(2)
        .describe("How long a note played over a held one takes to glide to its pitch."),
        ParamDescriptor::choice(CIRCUIT, "Circuit", &CIRCUITS, 0)
            .describe("The filter's circuit: how it behaves when pushed."),
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 20_000.0), 400.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("Sets the low-pass cutoff."),
        percent(RES, "Res", 0.0).describe("Raises the filter's resonance."),
        percent(DRIVE, "Drive", 0.0).describe("Pushes the signal into the filter's circuit."),
        percent(KEY, "Key", 0.0)
            .describe("How much the played note moves the cutoff: one octave per octave at 100 %."),
        time(FILTER_ATTACK, "Filter Attack", 0.0)
            .describe("How long the filter envelope takes to rise."),
        time(FILTER_DECAY, "Filter Decay", 300.0)
            .describe("How long the filter envelope takes to fall."),
        ParamDescriptor::continuous(
            ENV_AMOUNT,
            "Env Amount",
            (-100.0, 100.0),
            50.0,
            ParamUnit::Percent,
        )
        .describe("How far the filter envelope moves the cutoff, up to six octaves."),
        time(ATTACK, "Attack", 1.0).describe("How long the amplifier envelope takes to rise."),
        time(DECAY, "Decay", 300.0)
            .describe("How long the amplifier envelope takes to fall to its sustain."),
        percent(SUSTAIN, "Sustain", 100.0)
            .describe("The level the amplifier envelope holds while a key is held."),
        time(RELEASE, "Release", 100.0)
            .describe("How long the amplifier envelope takes to fall to silence."),
        percent(VEL_VOL, "Vel > Vol", 0.0)
            .describe("How much the notes' velocity moves the volume."),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, -6.0)
            .describe("Sets the instrument's output level, down to silence."),
        percent(DISTORTION, "Distortion", 0.0)
            .describe("Drives a soft clipper after the amplifier, by up to 36 dB."),
        ParamDescriptor::integer(PB_RANGE, "PB Range", (0, 24), 2, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the pitch wheel's range, in semitones."),
    ];
    let height = BODY_HEIGHT - TAB_HEIGHT;
    let page = |name: &str, controls: &[(ParamId, Widget)]| {
        Page::new(
            name,
            LayoutNode::Band(ControlBand::stacked(height, controls)),
        )
    };
    let layout = DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        "Pages",
        PageStyle::Tabs,
        vec![
            page(
                "Oscillator",
                &[
                    (SHAPE, Widget::Knob),
                    (OCTAVE, Widget::Number),
                    (TUNE, Widget::Knob),
                    (SUB, Widget::Knob),
                    (NOISE, Widget::Knob),
                    (GLIDE, Widget::Knob),
                ],
            ),
            page(
                "Filter",
                &[
                    (CIRCUIT, Widget::Chooser),
                    (FREQ, Widget::Knob),
                    (RES, Widget::Knob),
                    (DRIVE, Widget::Knob),
                    (KEY, Widget::Knob),
                    (FILTER_ATTACK, Widget::Knob),
                    (FILTER_DECAY, Widget::Knob),
                    (ENV_AMOUNT, Widget::Knob),
                ],
            ),
            page(
                "Amplifier",
                &[
                    (ATTACK, Widget::Knob),
                    (DECAY, Widget::Knob),
                    (SUSTAIN, Widget::Knob),
                    (RELEASE, Widget::Knob),
                    (VEL_VOL, Widget::Knob),
                    (VOLUME, Widget::Knob),
                    (DISTORTION, Widget::Knob),
                    (PB_RANGE, Widget::Number),
                ],
            ),
        ],
    )));
    DeviceType::native(BASS, "Bass", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Oscillator",
                    &["Shape", "Octave", "Tune", "Sub", "Noise", "Glide"],
                ),
                (
                    "Filter",
                    &[
                        "Circuit",
                        "Freq",
                        "Res",
                        "Drive",
                        "Key",
                        "Filter Attack",
                        "Filter Decay",
                        "Env Amount",
                    ],
                ),
                (
                    "Amplifier",
                    &[
                        "Attack",
                        "Decay",
                        "Sustain",
                        "Release",
                        "Vel > Vol",
                        "Volume",
                        "Distortion",
                        "PB Range",
                    ],
                ),
            ]),
    )
}

/// A factory preset: a name and the parameters it sets, the rest staying
/// at their defaults.
pub struct Preset {
    pub name: &'static str,
    pub values: Vec<(ParamId, f64)>,
}

/// Bass's factory presets, Beatlayer's own.
#[must_use]
pub fn presets() -> Vec<Preset> {
    let db = db_to_gain;
    vec![
        Preset {
            name: "Deep Sub",
            values: vec![
                (SHAPE, 0.0),
                (OCTAVE, -1.0),
                (SUB, db(-3.0)),
                (FREQ, 220.0),
                (ENV_AMOUNT, 20.0),
                (FILTER_DECAY, 200.0),
                (SUSTAIN, 100.0),
                (RELEASE, 120.0),
                (GLIDE, 40.0),
            ],
        },
        Preset {
            name: "Acid Line",
            values: vec![
                (SHAPE, 100.0),
                (OCTAVE, 0.0),
                (CIRCUIT, 4.0),
                (FREQ, 300.0),
                (RES, 70.0),
                (DRIVE, 30.0),
                (ENV_AMOUNT, 80.0),
                (FILTER_DECAY, 180.0),
                (DECAY, 250.0),
                (SUSTAIN, 60.0),
                (GLIDE, 90.0),
            ],
        },
        Preset {
            name: "Gritty Drive",
            values: vec![
                (SHAPE, 30.0),
                (NOISE, db(-30.0)),
                (SUB, db(-6.0)),
                (CIRCUIT, 2.0),
                (FREQ, 600.0),
                (RES, 30.0),
                (DISTORTION, 70.0),
                (VEL_VOL, 60.0),
            ],
        },
    ]
}

/// Adds Bass to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, bass_type(), |device, config| {
        Bass::new(device, config)
    });
}

/// Bass's audio-thread half.
pub struct Bass {
    section: Section,
    voice: Voice,
}

impl Bass {
    fn new(device: &Device, config: &bl_engine::NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let seed = device.id.0.get();
        Self {
            // Spec: bass and Poli R4, R5. One voice, always legato, gliding
            // only into a note played over a held one.
            section: Section::new(
                Parts {
                    voices_one_is_mono: true,
                    always_legato: true,
                    glide_time: Some(GLIDE),
                    glide_only_held: true,
                    pb_range: Some(PB_RANGE),
                    ..Parts::default()
                },
                VOICES,
                1,
                rate,
                seed,
            ),
            voice: Voice::new(rate, config.max_frames, seed),
        }
    }
}

impl NativeDevice for Bass {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        self.voice.snapshot(frames, params);
        self.section.process(audio, params, &mut self.voice);
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.voice.out[..frames]);
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        // A switched-off instrument is silent and holds no notes.
        self.section.reset();
        self.voice.silence();
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
    }
}

/// Everything but the voice section: the [`Instrument`] the voice plays.
struct Voice {
    stride: usize,
    osc: Oscillator,
    sub: Oscillator,
    amp: EnvelopeBank,
    fenv: EnvelopeBank,
    amp_params: EnvelopeParams,
    fenv_params: EnvelopeParams,
    filtering: Filtering,
    /// The device's seed and the number of notes started, which seed the
    /// noise afresh with each note.
    seed: u64,
    notes: u64,
    noise: Random,
    /// The stretch's parameters, frame by frame; Resonance and Drive are
    /// kept from 0 to 1.
    shape: Vec<f32>,
    tune: Vec<f32>,
    sub_gain: Vec<f32>,
    noise_gain: Vec<f32>,
    freq: Vec<f32>,
    res: Vec<f32>,
    drive: Vec<f32>,
    key: Vec<f32>,
    env_amount: Vec<f32>,
    vel_vol: Vec<f32>,
    volume: Vec<f32>,
    distortion: Vec<f32>,
    /// The stretch's per-voice buffers, `frames × stride` each.
    osc_out: Vec<f32>,
    sub_out: Vec<f32>,
    pitch_mod: Vec<f32>,
    frame_mod: Vec<f32>,
    amp_out: Vec<f32>,
    fenv_out: Vec<f32>,
    /// The stretch's mixed output, one value a frame.
    out: Vec<f32>,
    octave: i32,
    setup: Setup,
}

impl Voice {
    fn new(rate: f64, max_frames: usize, seed: u64) -> Self {
        let max_frames = max_frames.max(1);
        let layout = OscLayout {
            voices: VOICES,
            lanes_per_voice: 1,
        };
        let osc = Oscillator::new(morph_table(), layout, rate as f32, max_frames, 0x0BA5_5001);
        let sub = Oscillator::new(sine_table(), layout, rate as f32, max_frames, 0x0BA5_5002);
        let stride = osc.lane_count();
        let ramp = || vec![0.0; max_frames];
        let buffer = || vec![0.0; max_frames * stride];
        Self {
            stride,
            osc,
            sub,
            amp: analogue::envelopes(VOICES, rate),
            fenv: analogue::envelopes(VOICES, rate),
            amp_params: EnvelopeParams::AMP,
            fenv_params: EnvelopeParams::AMP,
            filtering: Filtering::new(rate, VOICES, max_frames),
            seed,
            notes: 0,
            noise: Random::seeded(seed),
            shape: ramp(),
            tune: ramp(),
            sub_gain: ramp(),
            noise_gain: ramp(),
            freq: ramp(),
            res: ramp(),
            drive: ramp(),
            key: ramp(),
            env_amount: ramp(),
            vel_vol: ramp(),
            volume: ramp(),
            distortion: ramp(),
            osc_out: buffer(),
            sub_out: buffer(),
            pitch_mod: buffer(),
            frame_mod: buffer(),
            amp_out: buffer(),
            fenv_out: buffer(),
            out: ramp(),
            octave: -1,
            setup: Setup::new(0, true, 0),
        }
    }

    /// Reads the stretch's parameters: the ramps into the buffers, and the
    /// switches, choices and envelopes into the fields.
    fn snapshot(&mut self, frames: usize, params: &ParamBank) {
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let ramp = |to: &mut [f32], id: ParamId, scale: f32| {
            for (to, from) in to[..frames].iter_mut().zip(params.ramp(at(id))) {
                *to = from * scale;
            }
        };
        ramp(&mut self.shape, SHAPE, 0.01);
        ramp(&mut self.tune, TUNE, 1.0);
        ramp(&mut self.sub_gain, SUB, 1.0);
        ramp(&mut self.noise_gain, NOISE, 1.0);
        ramp(&mut self.freq, FREQ, 1.0);
        ramp(&mut self.res, RES, 0.01);
        ramp(&mut self.drive, DRIVE, 0.01);
        ramp(&mut self.key, KEY, 0.01);
        ramp(&mut self.env_amount, ENV_AMOUNT, 0.01);
        ramp(&mut self.vel_vol, VEL_VOL, 0.01);
        ramp(&mut self.volume, VOLUME, 1.0);
        ramp(&mut self.distortion, DISTORTION, 0.01);
        self.out[..frames].fill(0.0);

        self.octave = params.value(at(OCTAVE)) as i32;
        // Spec: bass and Poli R7. The 24 dB low-pass in the circuit chosen.
        self.setup = Setup::new(0, true, params.value(at(CIRCUIT)) as usize);
        for (osc, octave) in [
            (&mut self.osc, self.octave),
            (&mut self.sub, self.octave - 1),
        ] {
            let p: &mut OscParams = osc.params_mut();
            p.octave = octave;
            p.phase_mode = PhaseMode::NoteStart;
            p.phase_random = 0.0;
            p.phase = 0.0;
            p.level = 100.0;
            p.reference = REFERENCE;
            p.frame = 1.0;
            p.smooth = true;
        }
        let value = |id: ParamId| params.value(at(id));
        analogue::set_envelope(
            &mut self.amp,
            &mut self.amp_params,
            analogue::adsr(
                value(ATTACK),
                value(DECAY),
                value(SUSTAIN) / 100.0,
                value(RELEASE),
            ),
        );
        // Spec: bass and Poli R7. An attack and a decay: it falls to nothing,
        // and a key let go falls over the same time.
        analogue::set_envelope(
            &mut self.fenv,
            &mut self.fenv_params,
            analogue::adsr(
                value(FILTER_ATTACK),
                value(FILTER_DECAY),
                0.0,
                value(FILTER_DECAY),
            ),
        );
    }

    /// Drops everything the voice holds, for a device switched off.
    fn silence(&mut self) {
        self.amp.reset(0);
        self.fenv.reset(0);
        self.out.fill(0.0);
    }
}

/// Spec: bass and Poli R8. A soft clipper driven by up to 36 dB at
/// `amount` (0 to 1), its output scaled back by half the drive; at 0 the
/// signal passes untouched.
fn distort(x: f32, amount: f32) -> f32 {
    if amount <= 0.0 {
        return x;
    }
    let drive = amount.min(1.0) * DRIVE_DB;
    let gain = (drive * core::f32::consts::LN_10 / 20.0).exp();
    (gain * x).tanh() / gain.sqrt()
}

impl Instrument for Voice {
    fn voice_event(&mut self, voices: &mut VoiceBlock, _state: &State, event: VoiceEvent) {
        let voice = event.voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                let start = analogue::effective(start, self.amp.is_idle(voice));
                if !matches!(start, lspk_voice::Start::Legato) {
                    self.notes += 1;
                    self.noise = Random::seeded(analogue::note_seed(self.seed, self.notes));
                }
                let key = voices.pitch()[voice];
                let onset = analogue::onset(start);
                self.osc.start(voice, key, onset);
                self.sub.start(voice, key, onset);
                analogue::trigger(&mut self.amp, voice, start);
                analogue::trigger(&mut self.fenv, voice, start);
                if matches!(start, lspk_voice::Start::Fresh) {
                    self.filtering.reset_voice(voice);
                }
            }
            VoiceEventKind::Release => {
                self.amp.release(voice);
                self.fenv.release(voice);
            }
            VoiceEventKind::End { .. } => {
                self.amp.reset(voice);
                self.fenv.reset(voice);
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let (from, to) = (frames.start, frames.end);
        let count = to - from;
        let span = voices.span(16);
        if count == 0 || span == 0 {
            return;
        }
        let stride = self.stride;

        // 1. The envelopes: the amplifier's and the filter's.
        self.amp.process(&mut self.amp_out[..count * stride]);
        self.fenv.process(&mut self.fenv_out[..count * stride]);

        // 2. The oscillators, band-limited: Tune and a tuned key's offset
        //    ride on the pitch, Shape on the frame.
        for frame in 0..count {
            for voice in 0..span {
                self.pitch_mod[frame * stride + voice] =
                    self.tune[from + frame] + state.tuned_by(voice);
                self.frame_mod[frame * stride + voice] = self.shape[from + frame];
            }
        }
        let inputs = Inputs {
            lanes: span,
            pitch: voices.pitch(),
            pitch_step: Some(voices.pitch_step()),
            pitch_mod: Some(&self.pitch_mod[..count * stride]),
            frame_mod: Some(&self.frame_mod[..count * stride]),
        };
        self.osc
            .process(count, &inputs, &mut self.osc_out[..count * stride]);
        let sub_inputs = Inputs {
            frame_mod: None,
            ..inputs
        };
        self.sub
            .process(count, &sub_inputs, &mut self.sub_out[..count * stride]);

        // 3. The sources summed, and the filter's cutoff: the knob, the key
        //    and the envelope.
        self.filtering.begin(count);
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            for frame in 0..count {
                let at = from + frame;
                let i = frame * stride + voice;
                let mut sample = self.osc_out[i] + self.sub_out[i] * self.sub_gain[at];
                if self.noise_gain[at] != 0.0 {
                    sample += self.noise.next_bipolar() as f32 * self.noise_gain[at];
                }
                self.filtering.put(frame, voice, sample);
                let note = voices.pitch()[voice]
                    + voices.pitch_step()[voice] * frame as f32
                    + state.tuned_by(voice)
                    - MIDDLE_C;
                self.filtering.set_cutoff(
                    frame,
                    voice,
                    pitch_of(self.freq[at], REFERENCE)
                        + self.key[at] * note
                        + self.fenv_out[i] * self.env_amount[at] * ENV_SEMITONES,
                );
            }
        }

        // 4. The filter.
        self.filtering.run(
            self.setup,
            count,
            &self.res[from..to],
            Some(&self.drive[from..to]),
        );

        // 5. The amplifier, Vel > Vol and Volume, and the distortion.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let velocity = voices.velocity()[voice];
            for frame in 0..count {
                let at = from + frame;
                let level = self.amp_out[frame * stride + voice];
                if level == 0.0 {
                    continue;
                }
                // Spec: bass and Poli R8. Drift's law.
                let vel_vol = self.vel_vol[at];
                let scale = 1.0 - vel_vol + vel_vol * velocity;
                self.out[at] += self.filtering.get(frame, voice) * level * scale * self.volume[at];
            }
        }
        for at in from..to {
            self.out[at] = distort(self.out[at], self.distortion[at]);
        }

        // 6. A voice whose amplifier envelope has ended is silent.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.amp.is_idle(voice) {
                voices.end(voice);
            }
        }
    }
}
