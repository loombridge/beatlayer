//! Analog (WP-8.14 and WP-8.15; spec: Analog): two oscillators and a noise
//! generator into two multimode filters and two amplifiers, four envelopes
//! with loop modes, two LFOs, the global section (volume, vibrato, unison,
//! glide, the keyboard, voices and priority), the MPE section, and the
//! shell-and-display panel with its displays.
//!
//! The voices are the instruments' voice section's ([`crate::instrument`]),
//! the oscillators the plug-in kit's band-limited wavetable oscillators and
//! the filters its zero-delay-feedback filters. The mix, the drive stage,
//! the envelopes, the LFOs, the vibrato and the amplifiers are this
//! module's. Each voice's unison copies are lanes of its own, `voice ×
//! COPIES + copy`, in every per-lane array below.

pub mod display;
mod global;
mod waves;

pub use display::display;
pub use global::*;

use core::f32::consts::{FRAC_PI_2, TAU};
use std::ops::Range;

use bl_dsp::random::Random;
use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT,
    Widget,
};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use lspk_dsp::filter::{FilterBank, FilterType, VoiceParams, pitch_of};
use lspk_voice::unison::pan_gains;
use lspk_voice::{Start, VoiceBlock, VoiceEvent, VoiceEventKind};
use lspk_wavetable::osc::{Inputs, Layout as OscLayout, Onset, Oscillator, PhaseMode};
use lspk_wavetable::shaper::{Mode, ShapedOscillator, ShaperInputs, Slot};
use lspk_wavetable::{Cache, FRAME_SIZE, SharedTable, Wavetable};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::matrix::Matrix;
use crate::instrument::{self, Instrument, Section, Sources, State};
use waves::{Lfo, Vibrato};

/// Analog's device type.
pub const ANALOG: &str = "beatlayer.analog";

/// Voices at most, the copies each has at most, and the lanes of every
/// per-lane buffer: a voice's copies side by side (R2, R20, R23).
const VOICES: usize = global::MAX_VOICES;
const LANES: usize = VOICES * COPIES;

/// Frames rendered in one go, so the per-lane buffers stay small however
/// long a block is.
const CHUNK: usize = 512;

/// Spec: analog R20. Unison's reach at Detune 100 %, in cents, and at Delay
/// 100 %, in milliseconds.
const UNISON_CENTS: f32 = 50.0;
const UNISON_MS: f32 = 20.0;

/// Spec: analog R24. A pitch destination's reach at an amount of 100 % and
/// a source of 1, in semitones: an octave, as Pitch Mod's.
const MPE_PITCH_SEMITONES: f64 = 12.0;

/// The seed of the voice section's random draws.
const SEED: u64 = 0x00A4_A106;

/// The parameter IDs: each section a block of them, with the LFOs
/// (190 to 229), the global section (230 to 259) and MPE (260 to 289).
const OSC_BASE: [u32; 2] = [0, 20];
const NOISE_BASE: u32 = 40;
const FILTER_BASE: [u32; 2] = [50, 70];
const AMP_BASE: [u32; 2] = [90, 100];
const ENV_BASE: [u32; 4] = [110, 130, 150, 170];
/// One past the last ID part 2 will use.
const ID_LIMIT: usize = 290;

/// Spec: analog R23. PB Range, the first of the global section's.
pub const PB_RANGE: ParamId = ParamId(230);

/// An oscillator's parameters, as offsets in its block (R2 to R5).
pub mod osc {
    use bl_model::ParamId;
    pub const ON: u32 = 0;
    pub const LEVEL: u32 = 1;
    pub const BALANCE: u32 = 2;
    pub const SHAPE: u32 = 3;
    pub const OCTAVE: u32 = 4;
    pub const SEMI: u32 = 5;
    pub const DETUNE: u32 = 6;
    pub const PULSE_WIDTH: u32 = 7;
    pub const WIDTH_MOD: u32 = 8;
    pub const PITCH_MOD: u32 = 9;
    pub const KEY: u32 = 10;
    pub const ENV_INITIAL: u32 = 11;
    pub const ENV_TIME: u32 = 12;
    pub const SUB_SYNC: u32 = 13;
    pub const SUB_SYNC_AMOUNT: u32 = 14;

    /// Oscillator `n`'s (0 or 1) parameter at `offset`.
    #[must_use]
    pub const fn id(n: usize, offset: u32) -> ParamId {
        ParamId(super::OSC_BASE[n] + offset)
    }
}

/// The noise generator's parameters (R6).
pub mod noise {
    use bl_model::ParamId;
    pub const ON: u32 = 0;
    pub const LEVEL: u32 = 1;
    pub const BALANCE: u32 = 2;
    pub const COLOR: u32 = 3;

    #[must_use]
    pub const fn id(offset: u32) -> ParamId {
        ParamId(super::NOISE_BASE + offset)
    }
}

/// A filter's parameters (R7 to R11). `EXTRA` is Filter 1's To F2 and
/// Filter 2's Follow.
pub mod filter {
    use bl_model::ParamId;
    pub const ON: u32 = 0;
    pub const MODE: u32 = 1;
    pub const FREQ: u32 = 2;
    pub const RESO: u32 = 3;
    pub const DRIVE: u32 = 4;
    pub const FREQ_LFO: u32 = 5;
    pub const FREQ_KEY: u32 = 6;
    pub const FREQ_ENV: u32 = 7;
    pub const RES_LFO: u32 = 8;
    pub const RES_KEY: u32 = 9;
    pub const RES_ENV: u32 = 10;
    pub const EXTRA: u32 = 11;

    #[must_use]
    pub const fn id(n: usize, offset: u32) -> ParamId {
        ParamId(super::FILTER_BASE[n] + offset)
    }
}

/// An amplifier's parameters (R12).
pub mod amp {
    use bl_model::ParamId;
    pub const ON: u32 = 0;
    pub const LEVEL: u32 = 1;
    pub const PAN: u32 = 2;
    pub const LEVEL_LFO: u32 = 3;
    pub const LEVEL_KEY: u32 = 4;
    pub const LEVEL_ENV: u32 = 5;
    pub const PAN_LFO: u32 = 6;
    pub const PAN_KEY: u32 = 7;
    pub const PAN_ENV: u32 = 8;

    #[must_use]
    pub const fn id(n: usize, offset: u32) -> ParamId {
        ParamId(super::AMP_BASE[n] + offset)
    }
}

/// An envelope's parameters (R13 to R15). Envelopes 0 and 1 are the
/// filters', 2 and 3 the amplifiers'.
pub mod env {
    use bl_model::ParamId;
    pub const ATTACK: u32 = 0;
    pub const DECAY: u32 = 1;
    pub const SUSTAIN: u32 = 2;
    pub const RELEASE: u32 = 3;
    pub const SLOPE: u32 = 4;
    pub const ATT_VEL: u32 = 5;
    pub const ENV_VEL: u32 = 6;
    pub const S_TIME: u32 = 7;
    pub const LEGATO: u32 = 8;
    pub const FREE: u32 = 9;
    pub const LOOP: u32 = 10;

    #[must_use]
    pub const fn id(n: usize, offset: u32) -> ParamId {
        ParamId(super::ENV_BASE[n] + offset)
    }
}

/// The oscillator shapes (R2).
pub const SHAPES: [&str; 4] = ["Sine", "Saw", "Rectangle", "White Noise"];
const SINE: usize = 0;
const SAW: usize = 1;
const RECTANGLE: usize = 2;
const WHITE_NOISE: usize = 3;

/// Sub/Sync (R5).
pub const SUB_SYNC: [&str; 3] = ["Off", "Sub", "Sync"];
const SUB: usize = 1;
const SYNC: usize = 2;

/// The filter modes (R7): the twelves second order, the twenty-fours fourth.
pub const MODES: [&str; 10] = [
    "LP12",
    "LP24",
    "BP12",
    "BP24",
    "Notch12",
    "Notch24",
    "HP12",
    "HP24",
    "Formant12",
    "Formant24",
];
pub(crate) const FIRST_FORMANT: usize = 8;

/// Drive (R11).
pub const DRIVES: [&str; 7] = [
    "Off", "Sym 1", "Sym 2", "Sym 3", "Asym 1", "Asym 2", "Asym 3",
];

/// The envelope slopes (R13).
pub const SLOPES: [&str; 2] = ["Linear", "Exponential"];

/// The loop modes (R15).
pub const LOOPS: [&str; 4] = ["Off", "AD-R", "ADR-R", "ADS-R"];
const AD_R: usize = 1;
const ADR_R: usize = 2;
const ADS_R: usize = 3;

/// The sections' names: the pages of the shell (R16).
pub const SECTIONS: [&str; 10] = [
    "Oscillator 1",
    "Oscillator 2",
    "Noise",
    "Filter 1",
    "Filter 2",
    "Amplifier 1",
    "Amplifier 2",
    "LFO 1",
    "LFO 2",
    "Global",
];

/// The pages node's name.
pub const PAGES: &str = "Analog";

/// The note the keys follow from: C3 (R4).
const MIDDLE_C: f32 = 60.0;
const REFERENCE: f32 = 440.0;

/// Spec: analog R3. The rectangle's table holds one frame for each duty
/// cycle from 1 % to 99 %.
const PULSE_FRAMES: usize = 99;

/// Spec: analog R4. Pitch Mod's reach at 100 %, in semitones.
const PITCH_MOD_SEMITONES: f32 = 12.0;

/// Spec: analog R3. Width's reach at 100 %, in frames of the pulse table:
/// half the duty cycle's range.
const WIDTH_FRAMES: f32 = (PULSE_FRAMES - 1) as f32 / 2.0;

/// Spec: analog R5. The ratio Sync reaches at 100 %, against the kit's
/// sixteen at the top of its own scale.
const SYNC_RATIO: f32 = 8.0;
const KIT_SYNC_RATIO: f32 = 16.0;

/// Spec: analog R10. The filters' modulation depths: an LFO moves the cutoff
/// by four octaves, the envelope by six.
const FILTER_LFO_SEMITONES: f32 = 48.0;
const FILTER_ENV_SEMITONES: f32 = 72.0;

/// Spec: analog R8. The vowels' first two formants in hertz, A, E, I, O, U:
/// the average adult male values of Peterson and Barney (1952), in the order
/// Reso moves through them.
const VOWELS: [(f32, f32); 5] = [
    (730.0, 1090.0),
    (530.0, 1840.0),
    (270.0, 2290.0),
    (570.0, 840.0),
    (300.0, 870.0),
];

/// The resonance of a formant's band-pass resonator.
pub(crate) const FORMANT_RESONANCE: f32 = 0.9;

/// Spec: analog R11. The drive stage's gain into the curve, in decibels, for
/// each of the three strengths, and the asymmetric modes' bias.
const DRIVE_DB: [f32; 3] = [6.0, 12.0, 18.0];
const DRIVE_BIAS: f32 = 0.4;

/// Spec: analog R12. The key's reach on an amplifier's level, in decibels,
/// from C3 to four octaves up at 100 %.
const LEVEL_KEY_DB: f32 = 12.0;

/// Spec: analog R13. How much faster velocity 127 makes an attack at 100 %.
const ATTACK_VELOCITY: f32 = 16.0;

/// How sharply an exponential segment bends: the curve's time constants over
/// the segment.
const EXPONENT: f32 = 4.0;

/// Spec: analog R16, R17. Analog's device type: parts 1's controls with the
/// IDs of part 2 reserved.
#[must_use]
pub fn analog_type() -> DeviceType {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 20_000.0), default, ParamUnit::Milliseconds)
            .power(3)
    };
    let signed = |id, name: &str, default: f64, text: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), default, ParamUnit::Percent)
            .describe(text)
    };
    let percent = |id, name: &str, default: f64, text: &str| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
            .describe(text)
    };
    let mut params = Vec::new();
    for n in 0..2 {
        let at = |offset| osc::id(n, offset);
        let name = |what: &str| format!("Osc {} {what}", n + 1);
        params.extend([
            ParamDescriptor::toggle(at(osc::ON), &format!("Osc {}", n + 1), n == 0)
                .describe("Turns the oscillator on."),
            ParamDescriptor::gain(at(osc::LEVEL), &name("Level"), 0.0, -6.0)
                .describe("Sets the oscillator's level, down to silence."),
            percent(
                at(osc::BALANCE),
                &name("F1/F2"),
                50.0,
                "Shares the oscillator between Filter 1 at 0 % and Filter 2 at 100 %.",
            ),
            ParamDescriptor::choice(at(osc::SHAPE), &name("Shape"), &SHAPES, SAW)
                .describe("The oscillator's waveform."),
            ParamDescriptor::integer(at(osc::OCTAVE), &name("Octave"), (-3, 3), 0, ParamUnit::None)
                .decimals(0)
                .describe("Transposes the oscillator in octaves."),
            ParamDescriptor::integer(at(osc::SEMI), &name("Semi"), (-12, 12), 0, ParamUnit::Semitones)
                .decimals(0)
                .describe("Transposes the oscillator in semitones."),
            ParamDescriptor::continuous(
                at(osc::DETUNE),
                &name("Detune"),
                (-300.0, 300.0),
                0.0,
                ParamUnit::Cents,
            )
            .describe("Detunes the oscillator, in cents."),
            percent(
                at(osc::PULSE_WIDTH),
                &name("Pulse Width"),
                50.0,
                "Sets the rectangle's duty cycle, from 1 % to 99 %.",
            ),
            signed(
                at(osc::WIDTH_MOD),
                &name("Width Mod"),
                0.0,
                "How far the LFO moves the pulse width.",
            ),
            signed(
                at(osc::PITCH_MOD),
                &name("Pitch Mod"),
                0.0,
                "How far the LFO moves the oscillator's pitch, up to an octave.",
            ),
            ParamDescriptor::continuous(
                at(osc::KEY),
                &name("Key"),
                (-100.0, 200.0),
                100.0,
                ParamUnit::Percent,
            )
            .describe("How the pitch follows the key from C3: 100 % is a semitone a key, 0 % holds one pitch."),
            ParamDescriptor::continuous(
                at(osc::ENV_INITIAL),
                &name("Env Initial"),
                (-48.0, 48.0),
                0.0,
                ParamUnit::Semitones,
            )
            .describe("Where the pitch envelope starts a note, in semitones from its pitch."),
            ParamDescriptor::continuous(
                at(osc::ENV_TIME),
                &name("Env Time"),
                (0.0, 10_000.0),
                0.0,
                ParamUnit::Milliseconds,
            )
            .power(3)
            .describe("How long the pitch takes to glide from its starting pitch to the note's."),
            ParamDescriptor::choice(at(osc::SUB_SYNC), &name("Sub/Sync"), &SUB_SYNC, 0)
                .describe("Adds a square wave an octave below, or syncs the oscillator to a hidden one at the note's pitch."),
            percent(
                at(osc::SUB_SYNC_AMOUNT),
                &name("Sub/Sync Amount"),
                0.0,
                "The sub oscillator's level, or the synced oscillator's ratio, from 1 to 8.",
            ),
        ]);
    }
    params.extend([
        ParamDescriptor::toggle(noise::id(noise::ON), "Noise", false)
            .describe("Turns the noise generator on."),
        ParamDescriptor::gain(noise::id(noise::LEVEL), "Noise Level", 0.0, -6.0)
            .describe("Sets the noise generator's level, down to silence."),
        percent(
            noise::id(noise::BALANCE),
            "Noise F1/F2",
            50.0,
            "Shares the noise between Filter 1 at 0 % and Filter 2 at 100 %.",
        ),
        ParamDescriptor::continuous(
            noise::id(noise::COLOR),
            "Noise Color",
            (20.0, 20_000.0),
            20_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The frequency above which the noise falls, at 6 dB per octave."),
    ]);
    for n in 0..2 {
        let at = |offset| filter::id(n, offset);
        let name = |what: &str| format!("Filter {} {what}", n + 1);
        params.extend([
            ParamDescriptor::toggle(at(filter::ON), &format!("Filter {}", n + 1), n == 0)
                .describe("Turns the filter on; off passes its sources unfiltered."),
            ParamDescriptor::choice(at(filter::MODE), &name("Mode"), &MODES, 0)
                .describe("The filter's response and order; the formant modes sound vowels."),
            ParamDescriptor::continuous(
                at(filter::FREQ),
                &name("Freq"),
                (20.0, 20_000.0),
                20_000.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The filter's cutoff; in a formant mode it transposes the vowels."),
            percent(
                at(filter::RESO),
                &name("Reso"),
                0.0,
                "The filter's resonance; in a formant mode it moves through the vowels A, E, I, O, U.",
            ),
            ParamDescriptor::choice(at(filter::DRIVE), &name("Drive"), &DRIVES, 0)
                .describe("Distorts the filter's input, symmetrically or asymmetrically; higher numbers distort more."),
            signed(at(filter::FREQ_LFO), &name("Freq Mod LFO"), 0.0, "How far the LFO moves the cutoff."),
            signed(at(filter::FREQ_KEY), &name("Freq Mod Key"), 0.0, "How far the key moves the cutoff."),
            signed(at(filter::FREQ_ENV), &name("Freq Mod Env"), 0.0, "How far the filter's envelope moves the cutoff."),
            signed(at(filter::RES_LFO), &name("Res Mod LFO"), 0.0, "How far the LFO moves the resonance."),
            signed(at(filter::RES_KEY), &name("Res Mod Key"), 0.0, "How far the key moves the resonance."),
            signed(at(filter::RES_ENV), &name("Res Mod Env"), 0.0, "How far the filter's envelope moves the resonance."),
        ]);
        params.push(if n == 0 {
            percent(
                at(filter::EXTRA),
                "Filter 1 To F2",
                0.0,
                "How much of Filter 1's output goes into Filter 2.",
            )
        } else {
            ParamDescriptor::toggle(at(filter::EXTRA), "Filter 2 Follow", false).describe(
                "Makes Filter 2's cutoff follow Filter 1's, its own Freq setting as an offset of up to five octaves either way.",
            )
        });
    }
    for n in 0..2 {
        let at = |offset| amp::id(n, offset);
        let name = |what: &str| format!("Amp {} {what}", n + 1);
        params.extend([
            ParamDescriptor::toggle(at(amp::ON), &format!("Amp {}", n + 1), true)
                .describe("Turns the amplifier on."),
            ParamDescriptor::gain(at(amp::LEVEL), &name("Level"), 0.0, 0.0)
                .describe("Sets the amplifier's level, down to silence."),
            signed(at(amp::PAN), &name("Pan"), 0.0, "Pans the amplifier's output."),
            signed(at(amp::LEVEL_LFO), &name("Level Mod LFO"), 0.0, "How far the LFO moves the level."),
            signed(at(amp::LEVEL_KEY), &name("Level Mod Key"), 0.0, "How far the key moves the level, up to 12 dB over four octaves."),
            signed(at(amp::LEVEL_ENV), &name("Level Mod Env"), 100.0, "How far the amplifier's envelope moves the level; 100 % is the envelope's own shape."),
            signed(at(amp::PAN_LFO), &name("Pan Mod LFO"), 0.0, "How far the LFO moves the pan."),
            signed(at(amp::PAN_KEY), &name("Pan Mod Key"), 0.0, "How far the key moves the pan."),
            signed(at(amp::PAN_ENV), &name("Pan Mod Env"), 0.0, "How far the amplifier's envelope moves the pan."),
        ]);
    }
    let owners = ["Filter 1", "Filter 2", "Amp 1", "Amp 2"];
    for (n, owner) in owners.iter().enumerate() {
        let at = |offset| env::id(n, offset);
        let name = |what: &str| format!("{owner} {what}");
        params.extend([
            time(at(env::ATTACK), &name("Attack"), 1.0)
                .describe("How long the envelope takes to rise to full."),
            time(at(env::DECAY), &name("Decay"), 600.0)
                .describe("How long the envelope takes to fall to its sustain level."),
            percent(at(env::SUSTAIN), &name("Sustain"), 100.0, "The level the envelope holds while the key is down."),
            time(at(env::RELEASE), &name("Release"), 50.0)
                .describe("How long the envelope takes to fall to silence once the key is let go."),
            ParamDescriptor::choice(at(env::SLOPE), &name("Slope"), &SLOPES, 0)
                .describe("Makes each segment of the envelope straight or exponential."),
            percent(at(env::ATT_VEL), &name("Att < Vel"), 0.0, "How much velocity shortens the attack, by up to sixteen times."),
            percent(at(env::ENV_VEL), &name("Env < Vel"), 0.0, "How much velocity scales the envelope's level."),
            ParamDescriptor::continuous(
                at(env::S_TIME),
                &name("S.Time"),
                (0.0, 60_000.0),
                0.0,
                ParamUnit::Milliseconds,
            )
            .power(3)
            .describe("How long the sustain level takes to fall to silence while the key is down; 0 holds it."),
            ParamDescriptor::toggle(at(env::LEGATO), &name("Legato"), false)
                .describe("Starts a new note's envelope from the level the previous note's had reached."),
            ParamDescriptor::toggle(at(env::FREE), &name("Free"), false)
                .describe("Skips the sustain: attack and decay, then release, however long the note."),
            ParamDescriptor::choice(at(env::LOOP), &name("Loop"), &LOOPS, 0)
                .describe("Repeats the envelope while the key is down: AD-R repeats attack and decay, ADR-R attack, decay and release, ADS-R plays the attack again when the key is let go."),
        ]);
    }
    params.push(
        ParamDescriptor::integer(PB_RANGE, "PB Range", (0, 24), 2, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the instrument's pitch bend range, in semitones."),
    );
    params.extend(global::descriptors());
    let banks = bank_lists();
    let named: Vec<(&str, Vec<&str>)> = banks
        .iter()
        .map(|(name, list)| (name.as_str(), list.iter().map(String::as_str).collect()))
        .collect();
    let slices: Vec<(&str, &[&str])> = named
        .iter()
        .map(|(name, list)| (*name, list.as_slice()))
        .collect();
    DeviceType::native(ANALOG, "Analog", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&slices),
    )
    .with_matrix(global::matrix_layout())
}

/// The width of a display cell docked, in points at 100 %. (Beatlayer's own
/// size.)
const CELL_WIDTH: f32 = 110.0;

/// The shell: a section for each page, each its main controls under a
/// button, beside the display that holds the page's further controls
/// (R16).
fn layout() -> DeviceLayout {
    const SECTION: f32 = BODY_HEIGHT - TAB_HEIGHT;
    let band = |height, controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(height, controls))
    };
    let knob = |id| (id, Widget::Knob);
    let toggle = |id| (id, Widget::Toggle);
    let chooser = |id| (id, Widget::Chooser);
    let number = |id| (id, Widget::Number);
    let envelope = |n: usize| {
        let at = |offset| env::id(n, offset);
        [
            band(
                BODY_HEIGHT,
                &[
                    knob(at(env::ATTACK)),
                    knob(at(env::DECAY)),
                    knob(at(env::SUSTAIN)),
                    knob(at(env::RELEASE)),
                    chooser(at(env::SLOPE)),
                    knob(at(env::ATT_VEL)),
                    knob(at(env::ENV_VEL)),
                    knob(at(env::S_TIME)),
                    toggle(at(env::LEGATO)),
                    toggle(at(env::FREE)),
                    chooser(at(env::LOOP)),
                ],
            ),
            LayoutNode::Cell(DisplayCell::new(
                display::ENVELOPES[n],
                CELL_WIDTH,
                BODY_HEIGHT,
                1.0,
            )),
        ]
    };
    let mut pages = Vec::new();
    for (n, name) in SECTIONS.iter().take(2).enumerate() {
        let at = |offset| osc::id(n, offset);
        pages.push(
            Page::new(
                name,
                band(
                    BODY_HEIGHT,
                    &[
                        knob(at(osc::PULSE_WIDTH)),
                        knob(at(osc::WIDTH_MOD)),
                        knob(at(osc::PITCH_MOD)),
                        knob(at(osc::KEY)),
                        knob(at(osc::ENV_INITIAL)),
                        knob(at(osc::ENV_TIME)),
                        chooser(at(osc::SUB_SYNC)),
                        knob(at(osc::SUB_SYNC_AMOUNT)),
                    ],
                ),
            )
            .with_section(band(
                SECTION,
                &[
                    toggle(at(osc::ON)),
                    chooser(at(osc::SHAPE)),
                    number(at(osc::OCTAVE)),
                    number(at(osc::SEMI)),
                    knob(at(osc::DETUNE)),
                    knob(at(osc::LEVEL)),
                    knob(at(osc::BALANCE)),
                ],
            )),
        );
    }
    pages.push(
        Page::new(SECTIONS[2], band(BODY_HEIGHT, &[])).with_section(band(
            SECTION,
            &[
                toggle(noise::id(noise::ON)),
                knob(noise::id(noise::LEVEL)),
                knob(noise::id(noise::BALANCE)),
                knob(noise::id(noise::COLOR)),
            ],
        )),
    );
    for n in 0..2 {
        let at = |offset| filter::id(n, offset);
        let [controls, cell] = envelope(n);
        pages.push(
            Page::new(
                SECTIONS[3 + n],
                LayoutNode::Row(vec![
                    band(
                        BODY_HEIGHT,
                        &[
                            knob(at(filter::FREQ_LFO)),
                            knob(at(filter::FREQ_KEY)),
                            knob(at(filter::FREQ_ENV)),
                            knob(at(filter::RES_LFO)),
                            knob(at(filter::RES_KEY)),
                            knob(at(filter::RES_ENV)),
                        ],
                    ),
                    controls,
                    LayoutNode::Cell(DisplayCell::new(
                        display::RESPONSES[n],
                        CELL_WIDTH,
                        BODY_HEIGHT,
                        1.0,
                    )),
                    cell,
                ]),
            )
            .with_section(band(
                SECTION,
                &[
                    toggle(at(filter::ON)),
                    chooser(at(filter::MODE)),
                    knob(at(filter::FREQ)),
                    knob(at(filter::RESO)),
                    chooser(at(filter::DRIVE)),
                    if n == 0 {
                        knob(at(filter::EXTRA))
                    } else {
                        toggle(at(filter::EXTRA))
                    },
                ],
            )),
        );
    }
    for n in 0..2 {
        let at = |offset| amp::id(n, offset);
        let [controls, cell] = envelope(2 + n);
        pages.push(
            Page::new(
                SECTIONS[5 + n],
                LayoutNode::Row(vec![
                    band(
                        BODY_HEIGHT,
                        &[
                            knob(at(amp::LEVEL_LFO)),
                            knob(at(amp::LEVEL_KEY)),
                            knob(at(amp::LEVEL_ENV)),
                            knob(at(amp::PAN_LFO)),
                            knob(at(amp::PAN_KEY)),
                            knob(at(amp::PAN_ENV)),
                        ],
                    ),
                    controls,
                    cell,
                ]),
            )
            .with_section(band(
                SECTION,
                &[
                    toggle(at(amp::ON)),
                    knob(at(amp::LEVEL)),
                    knob(at(amp::PAN)),
                ],
            )),
        );
    }
    pages.extend(global::pages());
    DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        PAGES,
        PageStyle::Shell,
        pages,
    )))
}

/// Spec: analog R17. The banks: every parameter, eight at most to a bank.
fn bank_lists() -> Vec<(String, Vec<String>)> {
    fn bank(name: String, names: Vec<String>) -> (String, Vec<String>) {
        (name, names)
    }
    let list = |prefix: &str, what: &[&str]| -> Vec<String> {
        what.iter().map(|w| format!("{prefix} {w}")).collect()
    };
    let mut banks = Vec::new();
    for n in 1..=2 {
        let prefix = format!("Osc {n}");
        let mut main = vec![prefix.clone()];
        main.extend(list(
            &prefix,
            &[
                "Level",
                "F1/F2",
                "Shape",
                "Octave",
                "Semi",
                "Detune",
                "Pulse Width",
            ],
        ));
        banks.push(bank(format!("Oscillator {n}"), main));
        banks.push(bank(
            format!("Oscillator {n} Detail"),
            list(
                &prefix,
                &[
                    "Width Mod",
                    "Pitch Mod",
                    "Key",
                    "Env Initial",
                    "Env Time",
                    "Sub/Sync",
                    "Sub/Sync Amount",
                ],
            ),
        ));
    }
    banks.push(bank(
        "Noise".to_owned(),
        ["Noise", "Noise Level", "Noise F1/F2", "Noise Color"]
            .map(str::to_owned)
            .to_vec(),
    ));
    for n in 1..=2 {
        let prefix = format!("Filter {n}");
        let mut main = vec![prefix.clone()];
        main.extend(list(&prefix, &["Mode", "Freq", "Reso", "Drive"]));
        main.push(if n == 1 {
            "Filter 1 To F2".to_owned()
        } else {
            "Filter 2 Follow".to_owned()
        });
        banks.push(bank(format!("Filter {n}"), main));
        banks.push(bank(
            format!("Filter {n} Mod"),
            list(
                &prefix,
                &[
                    "Freq Mod LFO",
                    "Freq Mod Key",
                    "Freq Mod Env",
                    "Res Mod LFO",
                    "Res Mod Key",
                    "Res Mod Env",
                ],
            ),
        ));
    }
    for n in 1..=2 {
        let prefix = format!("Amp {n}");
        let mut main = vec![prefix.clone()];
        main.extend(list(&prefix, &["Level", "Pan"]));
        banks.push(bank(format!("Amplifier {n}"), main));
        banks.push(bank(
            format!("Amplifier {n} Mod"),
            list(
                &prefix,
                &[
                    "Level Mod LFO",
                    "Level Mod Key",
                    "Level Mod Env",
                    "Pan Mod LFO",
                    "Pan Mod Key",
                    "Pan Mod Env",
                ],
            ),
        ));
    }
    for (owner, title) in [
        ("Filter 1", "Filter 1 Envelope"),
        ("Filter 2", "Filter 2 Envelope"),
        ("Amp 1", "Amplifier 1 Envelope"),
        ("Amp 2", "Amplifier 2 Envelope"),
    ] {
        banks.push(bank(
            title.to_owned(),
            list(
                owner,
                &[
                    "Attack",
                    "Decay",
                    "Sustain",
                    "Release",
                    "Slope",
                    "Att < Vel",
                    "Env < Vel",
                    "S.Time",
                ],
            ),
        ));
        banks.push(bank(
            format!("{title} Loop"),
            list(owner, &["Legato", "Free", "Loop"]),
        ));
    }
    for n in 1..=2 {
        let prefix = format!("LFO {n}");
        let mut main = vec![prefix.clone()];
        main.extend(list(
            &prefix,
            &["Wave", "Rate", "Sync", "Division", "Width", "Delay", "Attack"],
        ));
        banks.push(bank(format!("LFO {n}"), main));
        banks.push(bank(
            format!("LFO {n} Phase"),
            list(&prefix, &["Retrig", "Offset"]),
        ));
    }
    let names = |what: &[&str]| what.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    banks.push(bank("Global".to_owned(), names(&["Volume", "Voices", "Priority"])));
    banks.push(bank(
        "Vibrato".to_owned(),
        names(&[
            "Vibrato",
            "Vibrato Amount",
            "Vibrato Rate",
            "Vibrato Delay",
            "Vibrato Attack",
            "Vibrato Error",
            "Vibrato Amt < MW",
        ]),
    ));
    banks.push(bank(
        "Unison and Glide".to_owned(),
        names(&[
            "Unison",
            "Unison Voices",
            "Unison Detune",
            "Unison Delay",
            "Glide",
            "Glide Time",
            "Glide Legato",
            "Glide Mode",
        ]),
    ));
    banks.push(bank(
        "Keyboard".to_owned(),
        names(&[
            "PB Range",
            "Keyboard Octave",
            "Keyboard Semi",
            "Keyboard Detune",
            "Keyboard Stretch",
            "Keyboard Error",
        ]),
    ));
    banks.push(bank(
        "MPE".to_owned(),
        names(&[
            "MPE",
            "Pressure Dest 1",
            "Pressure Amount 1",
            "Pressure Dest 2",
            "Pressure Amount 2",
            "Slide Dest 1",
            "Slide Amount 1",
            "Slide Dest 2",
        ]),
    ));
    banks.push(bank(
        "MPE Slide".to_owned(),
        names(&["Slide Amount 2", "Note PB"]),
    ));
    banks
}

/// Adds Analog to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, analog_type(), |_, config| make(config));
}

/// Makes an Analog for `config`'s rate and block size. Every buffer and
/// table it needs is made here, off the audio thread.
#[must_use]
pub fn make(config: &NodeConfig) -> Analog {
    Analog::new(config)
}

/// A table of `frames` frames, each `wave(frame, phase)` over a cycle,
/// band-limited into mipmaps by the kit.
fn table(frames: usize, wave: impl Fn(usize, f64) -> f64) -> SharedTable {
    let source = Wavetable::from_fn(frames, |frame, sample| {
        wave(frame, sample as f64 / FRAME_SIZE as f64) as f32
    })
    .expect("a table of whole frames");
    Cache::global().share(source)
}

/// Spec: analog R3. Frame `frame` of the rectangle's table: the duty cycle
/// (`frame` + 1) %, with its mean taken out so a narrow pulse carries no
/// offset.
fn pulse(frame: usize, phase: f64) -> f64 {
    let duty = (frame + 1) as f64 / 100.0;
    (if phase < duty { 1.0 } else { -1.0 }) - (2.0 * duty - 1.0)
}

/// Analog: the voice section and the voices' sound engine.
pub struct Analog {
    section: Section,
    synth: Synth,
    ids: Vec<ParamId>,
}

impl Analog {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let ty = analog_type();
        Self {
            section: Section::new(global::parts(), VOICES, COPIES, rate, SEED),
            synth: Synth::new(rate, config.max_frames, &ty),
            ids: ty.description.params.iter().map(|p| p.id).collect(),
        }
    }
}

impl NativeDevice for Analog {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        self.synth.snapshot(frames, params, &self.ids);
        // Spec: analog R24. With the MPE switch off, Pressure and Slide do
        // nothing.
        let mpe = params
            .index(global::MPE_ON)
            .is_some_and(|at| params.value(at) >= 0.5);
        self.synth.matrix.set_amount(if mpe { 1.0 } else { 0.0 });
        self.synth.matrix.begin(params);
        self.synth.bpm = audio.bpm();
        self.section.process(audio, params, &mut self.synth);
        let channels = audio.channels();
        audio
            .channel_mut(0)
            .copy_from_slice(&self.synth.left[..frames]);
        if channels > 1 {
            audio
                .channel_mut(1)
                .copy_from_slice(&self.synth.right[..frames]);
            for channel in 2..channels {
                audio
                    .channel_mut(channel)
                    .copy_from_slice(&self.synth.right[..frames]);
            }
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        self.section.reset();
        self.synth.silence();
    }

    /// Spec: instrument modulation R12. What the newest voice plays for each
    /// parameter the matrix moves.
    fn matrix_played(&self, report: &mut dyn FnMut(ParamId, f64)) {
        let matrix = &self.synth.matrix;
        for (dest, def) in matrix.layout().dests.iter().enumerate() {
            if let (Some(param), Some(descriptor), Some(position)) =
                (def.param, &self.synth.dests[dest], matrix.shown(dest))
            {
                report(param, descriptor.denormalize(position));
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
    }
}

/// An envelope's stage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
    /// ADS-R's attack after the key is let go.
    Rise,
}

/// An envelope's settings for a stretch, in frames and fractions.
#[derive(Clone, Copy)]
struct EnvSettings {
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    /// How far the sustain level falls each frame; 0 holds it.
    fall: f32,
    exponential: bool,
    free: bool,
    legato: bool,
    looping: usize,
    attack_velocity: f32,
    level_velocity: f32,
}

/// One envelope's voices (R13 to R15).
struct Envelope {
    stage: [Stage; LANES],
    level: [f32; LANES],
    from: [f32; LANES],
    /// How many frames `voice` has spent in its stage.
    elapsed: [f32; LANES],
    held: [bool; LANES],
    /// How many times faster velocity made this note's attack.
    faster: [f32; LANES],
    /// This note's level scale, from Env < Vel.
    scale: [f32; LANES],
}

impl Envelope {
    fn new() -> Self {
        Self {
            stage: [Stage::Idle; LANES],
            level: [0.0; LANES],
            from: [0.0; LANES],
            elapsed: [0.0; LANES],
            held: [false; LANES],
            faster: [1.0; LANES],
            scale: [1.0; LANES],
        }
    }

    /// Spec: analog R13, R14. Starts `voice`'s note at velocity `velocity`:
    /// from the level it had reached with Legato, else from silence.
    fn trigger(&mut self, voice: usize, set: &EnvSettings, from_current: bool, velocity: f32) {
        let level = if from_current { self.level[voice] } else { 0.0 };
        self.stage[voice] = Stage::Attack;
        self.level[voice] = level;
        self.from[voice] = level;
        self.elapsed[voice] = 0.0;
        self.held[voice] = true;
        self.faster[voice] = ATTACK_VELOCITY.powf(set.attack_velocity * velocity);
        self.scale[voice] = 1.0 - set.level_velocity + set.level_velocity * velocity;
    }

    /// Spec: analog R14, R15. The key is let go: release, or with ADS-R
    /// attack once more first. With Free the note's length doesn't matter.
    fn release(&mut self, voice: usize, set: &EnvSettings) {
        self.held[voice] = false;
        if set.free {
            return;
        }
        match self.stage[voice] {
            Stage::Idle | Stage::Release | Stage::Rise => {}
            _ if set.looping == ADS_R => self.enter(voice, Stage::Rise),
            _ => self.enter(voice, Stage::Release),
        }
    }

    fn reset(&mut self, voice: usize) {
        self.stage[voice] = Stage::Idle;
        self.level[voice] = 0.0;
        self.held[voice] = false;
    }

    fn idle(&self, voice: usize) -> bool {
        self.stage[voice] == Stage::Idle
    }

    fn enter(&mut self, voice: usize, stage: Stage) {
        self.stage[voice] = stage;
        self.from[voice] = self.level[voice];
        self.elapsed[voice] = 0.0;
    }

    /// Moves `voice`'s level one frame towards `to` over `frames`; whether it
    /// got there.
    fn advance(&mut self, voice: usize, to: f32, frames: f32, exponential: bool) -> bool {
        self.elapsed[voice] += 1.0;
        let progress = self.elapsed[voice] / frames.max(1.0);
        if progress >= 1.0 {
            self.level[voice] = to;
            return true;
        }
        let warped = if exponential {
            (1.0 - (-EXPONENT * progress).exp()) / (1.0 - (-EXPONENT).exp())
        } else {
            progress
        };
        self.level[voice] = self.from[voice] + (to - self.from[voice]) * warped;
        false
    }

    /// One frame of `voice`'s envelope, scaled by its velocity.
    fn step(&mut self, voice: usize, set: &EnvSettings) -> f32 {
        match self.stage[voice] {
            Stage::Idle => return 0.0,
            Stage::Attack | Stage::Rise => {
                let frames = set.attack / self.faster[voice];
                if self.advance(voice, 1.0, frames, set.exponential) {
                    let next = if self.stage[voice] == Stage::Rise {
                        Stage::Release
                    } else {
                        Stage::Decay
                    };
                    self.enter(voice, next);
                }
            }
            Stage::Decay => {
                if self.advance(voice, set.sustain, set.decay, set.exponential) {
                    let next = if set.free || set.looping == ADR_R {
                        Stage::Release
                    } else if set.looping == AD_R && self.held[voice] {
                        Stage::Attack
                    } else if self.held[voice] {
                        Stage::Sustain
                    } else {
                        Stage::Release
                    };
                    self.enter(voice, next);
                }
            }
            // A held note follows the Sustain knob; with S.Time it falls.
            Stage::Sustain if set.fall > 0.0 => {
                self.level[voice] = (self.level[voice] - set.fall).max(0.0);
            }
            Stage::Sustain => self.level[voice] = set.sustain,
            Stage::Release => {
                if self.advance(voice, 0.0, set.release, set.exponential) {
                    if set.looping == ADR_R && self.held[voice] && !set.free {
                        self.enter(voice, Stage::Attack);
                    } else {
                        self.stage[voice] = Stage::Idle;
                    }
                }
            }
        }
        self.level[voice] * self.scale[voice]
    }
}

/// A random number from 0 to 1 in each call, from the voice's own state.
fn noise_sample(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as f32) * (2.0 / u32::MAX as f32) - 1.0
}

/// Spec: analog R11. `x` through drive `mode` (1 to 6) with its DC taken out
/// by `dc`, which holds the one-pole high-pass's last input and output.
fn drive(mode: usize, x: f32, dc: &mut [f32; 2], pole: f32) -> f32 {
    let gain = 10f32.powf(DRIVE_DB[(mode - 1) % 3] / 20.0);
    let bias = if mode > 3 { DRIVE_BIAS } else { 0.0 };
    let shaped = (gain * x + bias).tanh() - bias.tanh();
    let out = shaped - dc[0] + pole * dc[1];
    *dc = [shaped, out];
    out
}

/// Spec: analog R7, R8. The kit's filter type that plays `mode` (a place in
/// [`MODES`]): a formant mode is two band-passes of the same order.
pub(crate) fn kit_type(mode: usize) -> FilterType {
    match mode {
        0 => FilterType::Lp12,
        1 => FilterType::Lp24,
        2 | 8 => FilterType::Bp12,
        3 | 9 => FilterType::Bp24,
        4 => FilterType::Notch12,
        5 => FilterType::Notch24,
        6 => FilterType::Hp12,
        _ => FilterType::Hp24,
    }
}

/// Spec: analog R8. The first two formants of Reso `reso` (0 to 1), in
/// hertz, with Freq `freq` transposing both: 1 kHz leaves them at the table.
pub(crate) fn formants(reso: f32, freq: f32) -> (f32, f32) {
    let position = reso.clamp(0.0, 1.0) * (VOWELS.len() - 1) as f32;
    let from = (position as usize).min(VOWELS.len() - 2);
    let t = position - from as f32;
    let (a, b) = (VOWELS[from], VOWELS[from + 1]);
    let scale = freq / 1_000.0;
    (
        (a.0 + (b.0 - a.0) * t) * scale,
        (a.1 + (b.1 - a.1) * t) * scale,
    )
}

/// Every parameter's value for each frame of the block, by ID.
struct Values {
    ordinal: Vec<u16>,
    max_frames: usize,
    values: Vec<f32>,
}

impl Values {
    fn new(description: &DeviceDescription, max_frames: usize) -> Self {
        let mut ordinal = vec![u16::MAX; ID_LIMIT];
        for (i, param) in description.params.iter().enumerate() {
            ordinal[param.id.0 as usize] = i as u16;
        }
        Self {
            ordinal,
            max_frames,
            values: vec![0.0; description.params.len() * max_frames],
        }
    }

    /// `id`'s value at `frame` of the block.
    fn at(&self, id: ParamId, frame: usize) -> f32 {
        self.values[usize::from(self.ordinal[id.0 as usize]) * self.max_frames + frame]
    }

    /// Envelope `n`'s settings at `frame` of the block (R13 to R15).
    fn envelope(&self, n: usize, frame: usize, rate: f32) -> EnvSettings {
        let get = |offset| self.at(env::id(n, offset), frame);
        let frames = |ms: f32| ms * 1e-3 * rate;
        let sustain = get(env::SUSTAIN) / 100.0;
        let fall_time = frames(get(env::S_TIME));
        EnvSettings {
            attack: frames(get(env::ATTACK)),
            decay: frames(get(env::DECAY)),
            sustain,
            release: frames(get(env::RELEASE)),
            fall: if fall_time > 1.0 {
                sustain / fall_time
            } else {
                0.0
            },
            exponential: get(env::SLOPE) >= 0.5,
            free: get(env::FREE) >= 0.5,
            legato: get(env::LEGATO) >= 0.5,
            looping: get(env::LOOP) as usize,
            attack_velocity: get(env::ATT_VEL) / 100.0,
            level_velocity: get(env::ENV_VEL) / 100.0,
        }
    }

    /// Spec: analog R10. Filter `k`'s resonance at `at`, from 0 to 1: the
    /// knob's value `knob` (in per cent, as the matrix may have moved it), the LFO, the key (−1 to 1) and the envelope's level moving it by
    /// up to its whole travel.
    fn resonance(&self, k: usize, at: usize, knob: f32, lfo: f32, key: f32, env: f32) -> f32 {
        let get = |offset| self.at(filter::id(k, offset), at);
        (knob / 100.0
            + lfo * get(filter::RES_LFO) / 100.0
            + key * get(filter::RES_KEY) / 100.0
            + env * get(filter::RES_ENV) / 100.0)
            .clamp(0.0, 1.0)
    }

    /// Spec: analog R10. How far Freq Mod moves filter `k`'s cutoff at `at`,
    /// in semitones, for a voice playing `note`.
    fn cutoff_move(&self, k: usize, at: usize, lfo: f32, note: f32, env: f32) -> f32 {
        let get = |offset| self.at(filter::id(k, offset), at) / 100.0;
        lfo * get(filter::FREQ_LFO) * FILTER_LFO_SEMITONES
            + get(filter::FREQ_KEY) * (note - MIDDLE_C)
            + get(filter::FREQ_ENV) * FILTER_ENV_SEMITONES * env
    }
}

/// The engine behind the voice section.
struct Synth {
    rate: f32,
    /// The tempo of the block being played, for synced LFOs.
    bpm: f64,
    values: Values,
    /// Where each parameter is in the bank, in description order.
    index: Vec<usize>,
    mapped: bool,
    block_frames: usize,
    /// Frames rendered in one go: a block's, up to [`CHUNK`].
    chunk: usize,
    matrix: Matrix,
    /// The descriptor of the parameter each matrix destination moves, if it
    /// moves one.
    dests: Vec<Option<ParamDescriptor>>,
    tables: [SharedTable; 3],
    pulses: SharedTable,
    shape_of: [usize; 2],
    osc: [ShapedOscillator; 2],
    sub: [Oscillator; 2],
    filters: [FilterBank; 2],
    seconds: [FilterBank; 2],
    mode_of: [usize; 2],
    resonance_of: [[f32; LANES]; 2],
    envelopes: [Envelope; 4],
    noise: [u32; LANES],
    noise_low: [f32; LANES],
    wild: [[u32; LANES]; 2],
    dc: [[[f32; 2]; LANES]; 2],
    /// How many notes have started: each note's randomness is seeded from it.
    notes: u64,
    /// How many unison copies each voice's note started with.
    copies_of: [usize; VOICES],
    lfos: [[Lfo; VOICES]; 2],
    vibratos: [Vibrato; VOICES],
    /// The frame each lane's copy starts to sound (unison's Delay), and
    /// whether its envelopes still wait to start, from what velocity and
    /// whether continuing the level they had (R20).
    audible: [u64; LANES],
    pending: [bool; LANES],
    pending_velocity: [f32; LANES],
    pending_legato: [bool; LANES],
    /// Where each lane's pitch is at the start of the chunk being rendered.
    lane_base: Vec<f32>,
    /// The per-lane buffers of a chunk, `frames × LANES` each.
    pitch: [Vec<f32>; 2],
    width: [Vec<f32>; 2],
    sync: [Vec<f32>; 2],
    oscs: [Vec<f32>; 2],
    subs: [Vec<f32>; 2],
    audio: [Vec<f32>; 2],
    second: [Vec<f32>; 2],
    cutoff: [Vec<f32>; 2],
    cutoff_second: [Vec<f32>; 2],
    follow: Vec<f32>,
    envelope_out: [Vec<f32>; 4],
    /// The per-voice buffers of a chunk, `frames × VOICES` each: the note's
    /// pitch, the vibrato's move and the two LFOs.
    note_out: Vec<f32>,
    vibrato_out: Vec<f32>,
    lfo_out: [Vec<f32>; 2],
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Synth {
    fn new(rate: f64, max_frames: usize, ty: &DeviceType) -> Self {
        let max_frames = max_frames.max(1);
        let chunk = max_frames.min(CHUNK);
        let rate = rate as f32;
        let description = &ty.description;
        let matrix = ty.matrix.clone().expect("Analog has a matrix");
        let dests = matrix
            .dests
            .iter()
            .map(|dest| dest.param.and_then(|id| description.param(id).cloned()))
            .collect();
        let tables = [
            table(1, |_, p| (f64::from(TAU) * p).sin()),
            table(1, |_, p| 2.0 * p - 1.0),
            table(1, |_, p| if p < 0.5 { 1.0 } else { -1.0 }),
        ];
        let pulses = table(PULSE_FRAMES, pulse);
        let layout = OscLayout {
            voices: VOICES,
            lanes_per_voice: COPIES,
        };
        // The kit's oversampled Sync works in chunks of at least four frames,
        // so its oscillators are sized for blocks of at least 128 whatever the
        // engine's.
        let oscillator = |table: &SharedTable, seed: u64, octave: i32| {
            let mut osc = Oscillator::new(table.clone(), layout, rate, chunk.max(128), seed);
            let params = osc.params_mut();
            params.level = 100.0;
            params.phase = 0.0;
            params.phase_random = 0.0;
            params.phase_mode = PhaseMode::NoteStart;
            params.octave = octave;
            osc
        };
        let buffer = || vec![0.0; chunk * LANES];
        let both = || [buffer(), buffer()];
        let per_voice = || vec![0.0; chunk * VOICES];
        Self {
            rate,
            bpm: 120.0,
            values: Values::new(description, max_frames),
            index: vec![0; description.params.len()],
            mapped: false,
            block_frames: 0,
            chunk,
            matrix: Matrix::new(matrix),
            dests,
            shape_of: [SAW; 2],
            osc: [
                ShapedOscillator::new(oscillator(&tables[SAW], 0xA4A1, 0)),
                ShapedOscillator::new(oscillator(&tables[SAW], 0xA4A2, 0)),
            ],
            sub: [
                oscillator(&tables[RECTANGLE], 0xA4B1, -1),
                oscillator(&tables[RECTANGLE], 0xA4B2, -1),
            ],
            tables,
            pulses,
            filters: [0, 1].map(|_| {
                let mut bank = FilterBank::new(rate, LANES, 1);
                bank.set_type(FilterType::Lp12);
                bank
            }),
            seconds: [0, 1].map(|_| {
                let mut bank = FilterBank::new(rate, LANES, 1);
                bank.set_type(FilterType::Lp12);
                bank
            }),
            mode_of: [0; 2],
            resonance_of: [[0.0; LANES]; 2],
            envelopes: [0, 1, 2, 3].map(|_| Envelope::new()),
            noise: core::array::from_fn(|l| 0x9E37_79B9u32.wrapping_mul(l as u32 + 1) | 1),
            noise_low: [0.0; LANES],
            wild: [0, 1].map(|n| {
                core::array::from_fn(|l| {
                    0x85EB_CA6Bu32.wrapping_mul((l + LANES * n) as u32 + 1) | 1
                })
            }),
            dc: [[[0.0; 2]; LANES]; 2],
            notes: 0,
            copies_of: [1; VOICES],
            lfos: [0, 1].map(|_| core::array::from_fn(|_| Lfo::new())),
            vibratos: [Vibrato::new(); VOICES],
            audible: [0; LANES],
            pending: [false; LANES],
            pending_velocity: [0.0; LANES],
            pending_legato: [false; LANES],
            lane_base: vec![0.0; LANES],
            pitch: both(),
            width: both(),
            sync: both(),
            oscs: both(),
            subs: both(),
            audio: both(),
            second: both(),
            cutoff: both(),
            cutoff_second: both(),
            follow: buffer(),
            envelope_out: [buffer(), buffer(), buffer(), buffer()],
            note_out: per_voice(),
            vibrato_out: per_voice(),
            lfo_out: [per_voice(), per_voice()],
            left: vec![0.0; max_frames],
            right: vec![0.0; max_frames],
        }
    }

    /// Reads every parameter's values for the block, and clears its output.
    fn snapshot(&mut self, frames: usize, params: &ParamBank, description_ids: &[ParamId]) {
        if !self.mapped {
            for (slot, id) in self.index.iter_mut().zip(description_ids) {
                *slot = params.index(*id).expect("the description holds it");
            }
            self.mapped = true;
        }
        let max = self.values.max_frames;
        for (i, &at) in self.index.iter().enumerate() {
            self.values.values[i * max..][..frames].copy_from_slice(params.ramp(at));
        }
        self.block_frames = frames;
        self.left[..frames].fill(0.0);
        self.right[..frames].fill(0.0);
    }

    fn silence(&mut self) {
        for env in &mut self.envelopes {
            *env = Envelope::new();
        }
        self.noise_low = [0.0; LANES];
        self.dc = [[[0.0; 2]; LANES]; 2];
        self.pending = [false; LANES];
        self.copies_of = [1; VOICES];
        self.notes = 0;
        for bank in self.filters.iter_mut().chain(&mut self.seconds) {
            bank.reset();
        }
        self.left.fill(0.0);
        self.right.fill(0.0);
    }

    /// Spec: analog R24. `base`, the value of the parameter that matrix
    /// destination `dest` moves, as `voice` plays it at `frame` of the
    /// chunk: moved by the routes into it, and `base` itself where none
    /// reach it.
    fn moved(
        &self,
        dest: usize,
        base: f32,
        voice: usize,
        (frame, due): (usize, bool),
        read: &impl Fn(usize, usize, usize) -> f64,
    ) -> f32 {
        let Some(descriptor) = &self.dests[dest] else {
            return base;
        };
        if !self.matrix.has(dest) {
            return base;
        }
        let position = self.matrix.position(
            dest,
            descriptor.normalize(f64::from(base)),
            voice,
            frame,
            due,
            read,
        );
        descriptor.denormalize(position) as f32
    }

    /// Spec: analog R18, R19, R20, R29. A note's randomness: the seed of
    /// its LFOs' and vibrato's random values, from how many notes have
    /// started.
    fn seed(&self) -> u64 {
        SEED ^ self.notes.wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }
}

impl Instrument for Synth {
    fn voice_event(&mut self, voices: &mut VoiceBlock, state: &State, event: VoiceEvent) {
        let voice = event.voice;
        let at = (event.time as usize).min(self.block_frames.saturating_sub(1));
        let first = voice * COPIES;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                let key = voices.pitch()[first];
                let onset = match start {
                    Start::Fresh => Onset::Fresh,
                    Start::Steal => Onset::Steal,
                    Start::Retrigger => Onset::Retrigger,
                    Start::Legato => Onset::Legato,
                    Start::Join { .. } => Onset::Join,
                };
                let velocity = voices.velocity()[first];
                let fresh = matches!(start, Start::Fresh | Start::Join { .. });
                let legato = matches!(start, Start::Legato);
                let now = state.clock(event.time as usize);
                let copies = state.config().copies.clamp(1, COPIES);
                let delay_ms = self.values.at(global::UNI_DELAY, at) / 100.0 * UNISON_MS;
                self.matrix.start(voice);
                self.copies_of[voice] = copies;
                for k in 0..2 {
                    self.osc[k].start(voice, key, onset);
                    self.sub[k].start(voice, key, onset);
                }
                for copy in 0..COPIES {
                    let lane = first + copy;
                    if copy >= copies {
                        self.pending[lane] = false;
                        for env in &mut self.envelopes {
                            env.reset(lane);
                        }
                        continue;
                    }
                    // Spec: analog R20, instrument voices R12. A copy starts
                    // a share of the Delay after the first.
                    let wait = if legato {
                        0
                    } else {
                        instrument::copy_delay_frames(copy, copies, delay_ms, f64::from(self.rate))
                    };
                    self.audible[lane] = now + wait;
                    if wait == 0 {
                        self.pending[lane] = false;
                        for n in 0..4 {
                            let set = self.values.envelope(n, at, self.rate);
                            self.envelopes[n].trigger(lane, &set, !fresh && set.legato, velocity);
                        }
                    } else {
                        self.pending[lane] = true;
                        self.pending_velocity[lane] = velocity;
                        self.pending_legato[lane] = !fresh;
                        for env in &mut self.envelopes {
                            env.reset(lane);
                        }
                    }
                    if fresh {
                        self.noise_low[lane] = 0.0;
                        self.dc[0][lane] = [0.0; 2];
                        self.dc[1][lane] = [0.0; 2];
                        for k in 0..2 {
                            let formant = self.mode_of[k] >= FIRST_FORMANT;
                            let resonance = if formant {
                                FORMANT_RESONANCE
                            } else {
                                self.values.at(filter::id(k, filter::RESO), at) / 100.0
                            };
                            self.resonance_of[k][lane] = resonance;
                            let params = VoiceParams {
                                resonance,
                                ..VoiceParams::default()
                            };
                            self.filters[k].reset_voice(lane, &params);
                            self.seconds[k].reset_voice(lane, &params);
                        }
                    }
                }
                // Spec: analog R18, R19, instrument voices R7. A note starts
                // its LFOs and vibrato afresh, unless it is a legato one,
                // which only moves the voice's pitch.
                if !legato {
                    self.notes += 1;
                    let seed = self.seed();
                    let rate = f64::from(self.rate);
                    for k in 0..2 {
                        let (hz, offset) = self.lfo_start(k, at);
                        let free = if self.values.at(lfo::id(k, lfo::RETRIG), at) >= 0.5 {
                            0.0
                        } else {
                            now as f64 * hz / rate
                        };
                        self.lfos[k][voice].start(seed ^ (k as u64 + 1), free + offset);
                    }
                    let error = f64::from(self.values.at(global::VIB_ERROR, at)) / 100.0;
                    self.vibratos[voice].start(error, &mut Random::seeded(seed ^ 0x5B));
                }
            }
            VoiceEventKind::Release => {
                for lane in first..first + COPIES {
                    // A copy that hasn't begun to sound never will.
                    if self.pending[lane] {
                        self.pending[lane] = false;
                        continue;
                    }
                    for n in 0..4 {
                        let set = self.values.envelope(n, at, self.rate);
                        self.envelopes[n].release(lane, &set);
                    }
                }
            }
            VoiceEventKind::End { .. } => {
                self.copies_of[voice] = 1;
                for lane in first..first + COPIES {
                    self.pending[lane] = false;
                    for env in &mut self.envelopes {
                        env.reset(lane);
                    }
                }
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let mut from = frames.start;
        while from < frames.end {
            let to = (from + self.chunk).min(frames.end);
            self.render_chunk(voices, state, &frames, (from, to));
            from = to;
        }
    }
}

impl Synth {
    /// Spec: analog R18. LFO `k`'s rate in hertz at `at` (a division of the
    /// tempo when synced, the knob's setting otherwise) and its offset in
    /// cycles.
    fn lfo_start(&self, k: usize, at: usize) -> (f64, f64) {
        let v = &self.values;
        let hz = if v.at(lfo::id(k, lfo::SYNC), at) >= 0.5 {
            let division = (v.at(lfo::id(k, lfo::DIVISION), at) as usize).min(DIVISIONS.len() - 1);
            self.bpm / 60.0 / DIVISION_BEATS[division]
        } else {
            f64::from(v.at(lfo::id(k, lfo::RATE), at))
        };
        (hz, f64::from(v.at(lfo::id(k, lfo::OFFSET), at)) / 360.0)
    }

    /// Renders frames `from` to `to` of the stretch, a piece of the render
    /// call `frames`.
    fn render_chunk(
        &mut self,
        voices: &mut VoiceBlock,
        state: &State,
        frames: &Range<usize>,
        (from, to): (usize, usize),
    ) {
        let count = to - from;
        let span = voices.span(16);
        if count == 0 || span == 0 {
            return;
        }
        // How far into the call the chunk starts.
        let skip = from - frames.start;
        let len = count * LANES;
        let active = voices.active_mask();
        let sources = Sources::of(voices);
        let keys: [f32; VOICES] = core::array::from_fn(|v| sources.key(v) as f32);
        let pressure: [f64; VOICES] = core::array::from_fn(|v| sources.pressure(v));
        let slide: [f64; VOICES] = core::array::from_fn(|v| sources.slide(v));
        let wheel = sources.mod_wheel() as f32;
        let read = |source: usize, voice: usize, _frame: usize| {
            if source == 0 {
                pressure[voice]
            } else {
                slide[voice]
            }
        };
        self.matrix.set_oldest(state.oldest(voices));
        self.matrix.set_newest(state.newest(voices));
        let copies = self.copies_of;
        let live_voice = |voice: usize| active & (1 << voice) != 0;
        let live = |lane: usize| live_voice(lane / COPIES) && lane % COPIES < copies[lane / COPIES];
        let clock = |frame: usize| state.clock(from + frame);
        let key_of = |lane: usize| keys[lane / COPIES];
        let v = &self.values;

        // Every sounding voice's note, LFOs and vibrato, frame by frame.
        let rate = f64::from(self.rate);
        let one = 1.0 / rate;
        let mut voice_list = [0usize; VOICES];
        let mut sounding = 0;
        for voice in 0..VOICES {
            if live_voice(voice) {
                voice_list[sounding] = voice;
                sounding += 1;
            }
        }
        let mpe_pitch: [bool; 2] =
            core::array::from_fn(|k| self.matrix.has(global::DEST_OSC_PITCH + k));
        for frame in 0..count {
            let at = from + frame;
            let due = state.control_due(at);
            let keyboard = v.at(global::OCTAVE, at).round() * 12.0
                + v.at(global::SEMI, at).round()
                + v.at(global::DETUNE, at) / 100.0;
            let lfo_on: [bool; 2] = core::array::from_fn(|k| v.at(lfo::id(k, lfo::ON), at) >= 0.5);
            let vibrato_on = v.at(global::VIB_ON, at) >= 0.5;
            for &voice in &voice_list[..sounding] {
                let i = frame * VOICES + voice;
                let note = state.pitch(voices, voice, frames, skip + frame) as f32 + keyboard;
                self.note_out[i] = note;
                let seconds = state.frames_since_start(voice, at) as f64 * one;
                for k in 0..2 {
                    self.lfo_out[k][i] = if lfo_on[k] {
                        let (width, delay, attack) = (
                            v.at(lfo::id(k, lfo::WIDTH), at) / 100.0,
                            f64::from(v.at(lfo::id(k, lfo::DELAY), at)) * 1e-3,
                            f64::from(v.at(lfo::id(k, lfo::ATTACK), at)) * 1e-3,
                        );
                        let synced = v.at(lfo::id(k, lfo::SYNC), at) >= 0.5;
                        let hz = if synced {
                            self.lfo_start(k, at).0
                        } else {
                            f64::from(self.moved(
                                global::DEST_LFO_RATE + k,
                                v.at(lfo::id(k, lfo::RATE), at),
                                voice,
                                (frame, due),
                                &read,
                            ))
                        };
                        let kind = v.at(lfo::id(k, lfo::WAVE), at) as usize;
                        self.lfos[k][voice].step(kind, width, hz * one)
                            * waves::fade(seconds, delay, attack)
                    } else {
                        0.0
                    };
                }
                self.vibrato_out[i] = if vibrato_on {
                    let amount = self.moved(
                        global::DEST_VIBRATO,
                        v.at(global::VIB_AMOUNT, at),
                        voice,
                        (frame, due),
                        &read,
                    ) / 100.0
                        + v.at(global::VIB_MOD_WHEEL, at) / 100.0 * wheel;
                    let (delay, attack) = (
                        f64::from(v.at(global::VIB_DELAY, at)) * 1e-3,
                        f64::from(v.at(global::VIB_ATTACK, at)) * 1e-3,
                    );
                    self.vibratos[voice].step(
                        amount * waves::fade(seconds, delay, attack),
                        f64::from(v.at(global::VIB_RATE, at)),
                        one,
                    )
                } else {
                    0.0
                };
            }
        }

        // The envelopes, a lane at a time: a copy that waited for unison's
        // Delay starts its own when its time comes.
        let sets: [EnvSettings; 4] = core::array::from_fn(|n| v.envelope(n, from, self.rate));
        for frame in 0..count {
            for lane in 0..span {
                if self.pending[lane] && clock(frame) >= self.audible[lane] {
                    self.pending[lane] = false;
                    for n in 0..4 {
                        self.envelopes[n].trigger(
                            lane,
                            &sets[n],
                            self.pending_legato[lane] && sets[n].legato,
                            self.pending_velocity[lane],
                        );
                    }
                }
                let on = live(lane);
                for n in 0..4 {
                    self.envelope_out[n][frame * LANES + lane] = if on {
                        self.envelopes[n].step(lane, &sets[n])
                    } else {
                        0.0
                    };
                }
            }
        }

        // The oscillators' pitch, width and sync, per lane and frame.
        let step = voices.pitch_step();
        let lane_pitch = voices.pitch();
        for lane in 0..LANES {
            self.lane_base[lane] = lane_pitch[lane] + step[lane] * skip as f32;
        }
        let modes: [usize; 2] =
            core::array::from_fn(|k| v.at(osc::id(k, osc::SUB_SYNC), from) as usize);
        let shapes: [usize; 2] =
            core::array::from_fn(|k| v.at(osc::id(k, osc::SHAPE), from) as usize);
        for k in 0..2 {
            for frame in 0..count {
                let at = |offset| v.at(osc::id(k, offset), from + frame);
                let due = state.control_due(from + frame);
                let tune = at(osc::OCTAVE).round() * 12.0
                    + at(osc::SEMI).round()
                    + at(osc::DETUNE) / 100.0;
                let key = at(osc::KEY) / 100.0;
                let (initial, time) = (at(osc::ENV_INITIAL), at(osc::ENV_TIME) * 1e-3);
                let width_mod = at(osc::WIDTH_MOD) / 100.0;
                let pitch_mod = at(osc::PITCH_MOD) / 100.0;
                let sync = at(osc::SUB_SYNC_AMOUNT) / 100.0;
                let unison = v.at(global::UNI_DETUNE, from + frame) / 100.0 * UNISON_CENTS;
                for lane in 0..span {
                    let (voice, copy) = (lane / COPIES, lane % COPIES);
                    let i = frame * LANES + lane;
                    if !live(lane) {
                        self.pitch[k][i] = 0.0;
                        continue;
                    }
                    let vi = frame * VOICES + voice;
                    let note = self.note_out[vi];
                    let carried = note - (lane_pitch[lane] + step[lane] * (skip + frame) as f32);
                    let seconds = state.frames_since_start(voice, from + frame) as f32 / self.rate;
                    let glide = if time > 0.0 {
                        initial * (1.0 - seconds / time).max(0.0)
                    } else {
                        0.0
                    };
                    let wave = self.lfo_out[k][vi];
                    let moved_pitch = if mpe_pitch[k] {
                        (self.matrix.add(
                            global::DEST_OSC_PITCH + k,
                            0.0,
                            voice,
                            frame,
                            &read,
                        ) * MPE_PITCH_SEMITONES) as f32
                    } else {
                        0.0
                    };
                    let spread = instrument::copy_cents(copy, copies[voice], unison) / 100.0;
                    let pulse = self.moved(
                        4 + k,
                        at(osc::PULSE_WIDTH),
                        voice,
                        (frame, due),
                        &read,
                    );
                    let base = (pulse / 100.0).clamp(0.01, 0.99) * 100.0 - 1.0;
                    self.pitch[k][i] = carried
                        + tune
                        + (key - 1.0) * (note - MIDDLE_C)
                        + glide
                        + wave * pitch_mod * PITCH_MOD_SEMITONES
                        + self.vibrato_out[vi]
                        + moved_pitch
                        + spread;
                    self.width[k][i] = (base + wave * width_mod * WIDTH_FRAMES)
                        .clamp(0.0, (PULSE_FRAMES - 1) as f32);
                    // Spec: analog R5. The ratio 1 to 8 on the kit's scale of 1 to 16.
                    self.sync[k][i] = 100.0 * (SYNC_RATIO.powf(sync)).ln() / KIT_SYNC_RATIO.ln();
                }
            }
        }

        // The oscillators, band-limited; the noise shape and the sub aside.
        for k in 0..2 {
            let shape = shapes[k];
            if shape != self.shape_of[k] && shape != WHITE_NOISE {
                self.shape_of[k] = shape;
                let table = match shape {
                    RECTANGLE => &self.pulses,
                    SINE => &self.tables[SINE],
                    _ => &self.tables[SAW],
                };
                self.osc[k].osc_mut().set_table(table.clone());
            }
            if shape == WHITE_NOISE {
                for frame in 0..count {
                    for lane in 0..span {
                        self.oscs[k][frame * LANES + lane] = if live(lane) {
                            noise_sample(&mut self.wild[k][lane])
                        } else {
                            0.0
                        };
                    }
                }
                self.subs[k][..len].fill(0.0);
                continue;
            }
            let rectangle = shape == RECTANGLE;
            {
                let params = self.osc[k].osc_mut().params_mut();
                params.frame = 1.0;
                params.smooth = true;
            }
            let syncing = modes[k] == SYNC;
            self.osc[k].shapers_mut().slots[0] = Slot {
                mode: if syncing { Mode::Sync } else { Mode::Off },
                ..Slot::default()
            };
            let inputs = Inputs {
                lanes: span,
                pitch: &self.lane_base,
                pitch_step: Some(step),
                pitch_mod: Some(&self.pitch[k][..len]),
                frame_mod: rectangle.then(|| &self.width[k][..len]),
            };
            let mods = ShaperInputs {
                amount_mod: [syncing.then(|| &self.sync[k][..len]), None],
                sources: [None, None],
            };
            self.osc[k].process(count, &inputs, &mods, &mut self.oscs[k][..len]);
            if modes[k] == SUB {
                let inputs = Inputs {
                    frame_mod: None,
                    ..inputs
                };
                self.sub[k].process(count, &inputs, &mut self.subs[k][..len]);
            } else {
                self.subs[k][..len].fill(0.0);
            }
        }

        // The sources mixed into each filter's input (R2, R6).
        let coefficient = |hz: f32| 1.0 - (-TAU * hz / self.rate).exp();
        let split = |balance: f32| {
            let (sin, cos) = (balance.clamp(0.0, 100.0) / 100.0 * FRAC_PI_2).sin_cos();
            (cos, sin)
        };
        for frame in 0..count {
            let at = from + frame;
            let due = state.control_due(at);
            let level = |k: usize| {
                if v.at(osc::id(k, osc::ON), at) >= 0.5 {
                    v.at(osc::id(k, osc::LEVEL), at)
                } else {
                    0.0
                }
            };
            let osc_levels = [level(0), level(1)];
            let osc_splits = [0, 1].map(|k| split(v.at(osc::id(k, osc::BALANCE), at)));
            let subs = [0, 1].map(|k| v.at(osc::id(k, osc::SUB_SYNC_AMOUNT), at) / 100.0);
            let noise_level = if v.at(noise::id(noise::ON), at) >= 0.5 {
                v.at(noise::id(noise::LEVEL), at)
            } else {
                0.0
            };
            let noise_split = split(v.at(noise::id(noise::BALANCE), at));
            let low = coefficient(v.at(noise::id(noise::COLOR), at));
            for lane in 0..span {
                let i = frame * LANES + lane;
                // A copy that waits for unison's Delay is silent.
                if !live(lane) || clock(frame) < self.audible[lane] {
                    self.audio[0][i] = 0.0;
                    self.audio[1][i] = 0.0;
                    continue;
                }
                let voice = lane / COPIES;
                let (mut one, mut two) = (0.0, 0.0);
                for k in 0..2 {
                    let sub = if modes[k] == SUB { subs[k] } else { 0.0 };
                    let gain = if osc_levels[k] > 0.0 {
                        self.moved(2 + k, osc_levels[k], voice, (frame, due), &read)
                    } else {
                        0.0
                    };
                    let signal = (self.oscs[k][i] + sub * self.subs[k][i]) * gain;
                    one += signal * osc_splits[k].0;
                    two += signal * osc_splits[k].1;
                }
                if noise_level > 0.0 {
                    let white = noise_sample(&mut self.noise[lane]);
                    self.noise_low[lane] += low * (white - self.noise_low[lane]);
                    let gain = self.moved(6, noise_level, voice, (frame, due), &read);
                    let signal = self.noise_low[lane] * gain;
                    one += signal * noise_split.0;
                    two += signal * noise_split.1;
                }
                self.audio[0][i] = one;
                self.audio[1][i] = two;
            }
        }

        // Filter 1's cutoff after its modulation, which Filter 2's Follow reads.
        let following = v.at(filter::id(1, filter::EXTRA), from) >= 0.5;
        if following {
            for frame in 0..count {
                let at = from + frame;
                let due = state.control_due(at);
                let knob = v.at(filter::id(0, filter::FREQ), at);
                for lane in 0..span {
                    let (voice, i) = (lane / COPIES, frame * LANES + lane);
                    let freq = pitch_of(
                        self.moved(7, knob, voice, (frame, due), &read),
                        REFERENCE,
                    );
                    let vi = frame * VOICES + voice;
                    self.follow[i] = freq
                        + v.cutoff_move(
                            0,
                            at,
                            self.lfo_out[0][vi],
                            self.note_out[vi],
                            self.envelope_out[0][i],
                        );
                }
            }
        }

        // The filters: Filter 1, then Filter 2 with Filter 1's share added.
        let dc_pole = 1.0 - TAU * 10.0 / self.rate;
        for k in 0..2 {
            let on = v.at(filter::id(k, filter::ON), from) >= 0.5;
            let mode = v.at(filter::id(k, filter::MODE), from) as usize;
            let drive_mode = v.at(filter::id(k, filter::DRIVE), from) as usize;
            let formant = mode >= FIRST_FORMANT;
            if mode != self.mode_of[k] {
                self.mode_of[k] = mode;
                let kind = kit_type(mode);
                self.filters[k].set_type(kind);
                self.seconds[k].set_type(kind);
            }
            if !on {
                continue;
            }
            if k == 1 {
                for frame in 0..count {
                    let to_f2 = v.at(filter::id(0, filter::EXTRA), from + frame) / 100.0;
                    if v.at(filter::id(0, filter::ON), from + frame) < 0.5 {
                        continue;
                    }
                    for lane in 0..span {
                        let i = frame * LANES + lane;
                        self.audio[1][i] += to_f2 * self.audio[0][i];
                    }
                }
            }
            // The cutoff, per lane and frame; for a formant, the two
            // resonators' frequencies.
            for frame in 0..count {
                let at = from + frame;
                let due = state.control_due(at);
                let knob = v.at(filter::id(k, filter::FREQ), at);
                let (reso_knob, follow) = (
                    v.at(filter::id(k, filter::RESO), at),
                    k == 1 && following,
                );
                for lane in 0..span {
                    let (voice, i) = (lane / COPIES, frame * LANES + lane);
                    let vi = frame * VOICES + voice;
                    let note = self.note_out[vi];
                    let wave = self.lfo_out[k][vi];
                    let env = self.envelope_out[k][i];
                    let freq = self.moved(7 + 2 * k, knob, voice, (frame, due), &read);
                    let moved = v.cutoff_move(k, at, wave, note, env);
                    let (low, high) = if formant {
                        let reso = self.moved(8 + 2 * k, reso_knob, voice, (frame, due), &read);
                        let reso = v.resonance(k, at, reso, wave, key_of(lane), env);
                        let (f1, f2) = formants(reso, freq);
                        (
                            pitch_of(f1, REFERENCE) + moved,
                            pitch_of(f2, REFERENCE) + moved,
                        )
                    } else if follow {
                        // Spec: analog R9. Freq is an offset of up to five
                        // octaves either way from Filter 1's cutoff.
                        let offset = -5.0 + 10.0 * (freq / 20.0).ln() / 1000f32.ln();
                        (self.follow[i] + 12.0 * offset + moved, 0.0)
                    } else {
                        (pitch_of(freq, REFERENCE) + moved, 0.0)
                    };
                    self.cutoff[k][i] = low;
                    self.cutoff_second[k][i] = high;
                }
            }
            // The drive in front of the filter.
            if drive_mode > 0 {
                for frame in 0..count {
                    for lane in 0..span {
                        let i = frame * LANES + lane;
                        self.audio[k][i] = drive(
                            drive_mode,
                            self.audio[k][i],
                            &mut self.dc[k][lane],
                            dc_pole,
                        );
                    }
                }
            }
            if formant {
                self.second[k][..len].copy_from_slice(&self.audio[k][..len]);
            }
            // The filters, in chunks that start on the control grid, where each
            // lane's resonance is set.
            let mut start = 0;
            while start < count {
                let mut end = start + 1;
                while end < count && !state.control_due(from + end) {
                    end += 1;
                }
                if state.control_due(from + start) {
                    for lane in 0..span {
                        let target = if formant {
                            FORMANT_RESONANCE
                        } else {
                            let at = from + start;
                            let voice = lane / COPIES;
                            let vi = start * VOICES + voice;
                            let env = self.envelope_out[k][start * LANES + lane];
                            let knob = self.moved(
                                8 + 2 * k,
                                v.at(filter::id(k, filter::RESO), at),
                                voice,
                                (start, true),
                                &read,
                            );
                            v.resonance(k, at, knob, self.lfo_out[k][vi], key_of(lane), env)
                        };
                        if (target - self.resonance_of[k][lane]).abs() > f32::EPSILON {
                            self.resonance_of[k][lane] = target;
                            let params = VoiceParams {
                                resonance: target,
                                ..VoiceParams::default()
                            };
                            self.filters[k].set_params(lane, &params);
                            self.seconds[k].set_params(lane, &params);
                        }
                    }
                }
                let (a, b) = (start * LANES, end * LANES);
                self.filters[k].process_voices(
                    span,
                    &self.cutoff[k][a..b],
                    &mut self.audio[k][a..b],
                );
                if formant {
                    self.seconds[k].process_voices(
                        span,
                        &self.cutoff_second[k][a..b],
                        &mut self.second[k][a..b],
                    );
                }
                start = end;
            }
            if formant {
                for i in 0..len {
                    self.audio[k][i] += self.second[k][i];
                }
            }
        }

        // The amplifiers, panned into the output (R12). A voice's copies
        // keep its level between them (instrument voices R13).
        let share: [f32; VOICES] = core::array::from_fn(|n| (copies[n] as f32).sqrt().recip());
        for frame in 0..count {
            let at = from + frame;
            let due = state.control_due(at);
            for k in 0..2 {
                if v.at(amp::id(k, amp::ON), at) < 0.5 {
                    continue;
                }
                let get = |offset| v.at(amp::id(k, offset), at);
                let level = get(amp::LEVEL);
                let (level_lfo, level_key, level_env) = (
                    get(amp::LEVEL_LFO) / 100.0,
                    get(amp::LEVEL_KEY) / 100.0,
                    get(amp::LEVEL_ENV) / 100.0,
                );
                let (pan, pan_lfo, pan_key, pan_env) = (
                    get(amp::PAN) / 100.0,
                    get(amp::PAN_LFO) / 100.0,
                    get(amp::PAN_KEY) / 100.0,
                    get(amp::PAN_ENV) / 100.0,
                );
                for lane in 0..span {
                    if !live(lane) || clock(frame) < self.audible[lane] {
                        continue;
                    }
                    let voice = lane / COPIES;
                    let i = frame * LANES + lane;
                    let env = self.envelope_out[2 + k][i];
                    let wave = self.lfo_out[k][frame * VOICES + voice];
                    // Spec: analog R12. Positive envelope amounts bring the
                    // level in from a flat one; negative ones turn it over.
                    let shaped = if level_env >= 0.0 {
                        1.0 - level_env * (1.0 - env)
                    } else {
                        1.0 + level_env * env
                    };
                    let level = self.moved(11 + k, level, voice, (frame, due), &read);
                    let db = LEVEL_KEY_DB * (level_key * key_of(lane) + level_lfo * wave);
                    let gain = level * shaped * 10f32.powf(db / 20.0) * share[voice];
                    let position = (pan + pan_lfo * wave + pan_key * key_of(lane) + pan_env * env)
                        .clamp(-1.0, 1.0);
                    let (l, r) = pan_gains(position);
                    let signal = self.audio[k][i] * gain;
                    self.left[from + frame] += signal * l;
                    self.right[from + frame] += signal * r;
                }
            }
        }

        // Spec: analog R19. Volume is the device's output.
        for frame in 0..count {
            let volume = v.at(global::VOLUME, from + frame);
            self.left[from + frame] *= volume;
            self.right[from + frame] *= volume;
        }

        // A voice whose amplifier envelopes have ended in every copy is silent.
        let mut mask = active;
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let first = voice * COPIES;
            let done = (first..first + copies[voice]).all(|lane| {
                !self.pending[lane] && self.envelopes[2].idle(lane) && self.envelopes[3].idle(lane)
            });
            if done {
                voices.end(voice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straight stages of 100 frames, a sustain of 0.5 and no loop.
    const SET: EnvSettings = EnvSettings {
        attack: 100.0,
        decay: 100.0,
        sustain: 0.5,
        release: 100.0,
        fall: 0.0,
        exponential: false,
        free: false,
        legato: false,
        looping: 0,
        attack_velocity: 0.0,
        level_velocity: 0.0,
    };

    /// `frames` frames of voice 0's envelope.
    fn run(env: &mut Envelope, set: &EnvSettings, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| env.step(0, set)).collect()
    }

    fn started(set: &EnvSettings, velocity: f32) -> Envelope {
        let mut env = Envelope::new();
        env.trigger(0, set, false, velocity);
        env
    }

    // Spec: analog R13
    #[test]
    fn the_stages_run_straight_to_their_levels_in_their_times() {
        let mut env = started(&SET, 1.0);
        let attack = run(&mut env, &SET, 100);
        assert!(
            (attack[49] - 0.5).abs() < 0.011,
            "half-way up: {}",
            attack[49]
        );
        assert_eq!(attack[99], 1.0, "full at the end of the attack");
        let decay = run(&mut env, &SET, 100);
        assert!(
            (decay[49] - 0.75).abs() < 0.011,
            "half-way down: {}",
            decay[49]
        );
        assert_eq!(decay[99], 0.5, "the sustain level");
        assert!(run(&mut env, &SET, 500).iter().all(|&x| x == 0.5), "held");
        env.release(0, &SET);
        let release = run(&mut env, &SET, 100);
        assert!(
            (release[49] - 0.25).abs() < 0.011,
            "half-way to silence: {}",
            release[49]
        );
        assert_eq!(release[99], 0.0);
        run(&mut env, &SET, 1);
        assert!(env.idle(0), "the voice ends when the release has");
    }

    // Spec: analog R13
    #[test]
    fn the_exponential_slope_bends_every_segment_the_same_way() {
        let set = EnvSettings {
            exponential: true,
            ..SET
        };
        let mut env = started(&set, 1.0);
        let attack = run(&mut env, &set, 100);
        let bent = (1.0 - (-EXPONENT * 0.5).exp()) / (1.0 - (-EXPONENT).exp());
        assert!(
            (attack[49] - bent).abs() < 0.02,
            "{} against {bent}",
            attack[49]
        );
        assert!(attack[49] > 0.8, "fast at first, slow at the top");
        run(&mut env, &set, 600);
        env.release(0, &set);
        let release = run(&mut env, &set, 100);
        assert!(
            (release[49] - 0.5 * (1.0 - bent)).abs() < 0.02,
            "a falling segment bends too"
        );
        assert_eq!(release[99], 0.0);
    }

    // Spec: analog R13
    #[test]
    fn velocity_shortens_the_attack_and_scales_the_level() {
        let set = EnvSettings {
            attack: 1_600.0,
            attack_velocity: 1.0,
            ..SET
        };
        let reach = |velocity: f32| {
            let mut env = started(&set, velocity);
            (1..=2_000)
                .find(|_| env.step(0, &set) >= 1.0)
                .expect("the attack ends")
        };
        assert_eq!(reach(0.0), 1_600);
        assert!(
            (reach(0.5) as f32 - 400.0).abs() <= 2.0,
            "four times faster at half velocity"
        );
        assert!(
            (reach(1.0) as f32 - 100.0).abs() <= 2.0,
            "sixteen times faster at full velocity"
        );
        let half = EnvSettings {
            level_velocity: 0.5,
            ..SET
        };
        let peak = |set: &EnvSettings, velocity: f32| {
            run(&mut started(set, velocity), set, 100)
                .into_iter()
                .fold(0.0, f32::max)
        };
        assert!((peak(&half, 0.5) - 0.75).abs() < 1e-6);
        let full = EnvSettings {
            level_velocity: 1.0,
            ..SET
        };
        assert!((peak(&full, 0.5) - 0.5).abs() < 1e-6);
        assert!((peak(&full, 1.0) - 1.0).abs() < 1e-6);
        assert!(
            (peak(&SET, 0.2) - 1.0).abs() < 1e-6,
            "no amount, no scaling"
        );
    }

    // Spec: analog R13, R17
    #[test]
    fn a_held_note_follows_the_sustain_level() {
        let mut env = started(&SET, 1.0);
        run(&mut env, &SET, 300);
        let lower = EnvSettings {
            sustain: 0.2,
            ..SET
        };
        assert_eq!(env.step(0, &lower), 0.2);
    }

    // Spec: analog R13
    #[test]
    fn s_time_lets_the_sustain_fall_to_silence_while_the_key_is_held() {
        let set = EnvSettings {
            sustain: 1.0,
            fall: 1.0 / 100.0,
            ..SET
        };
        let mut env = started(&set, 1.0);
        run(&mut env, &set, 200);
        let held = run(&mut env, &set, 150);
        assert!((held[49] - 0.5).abs() < 0.03, "half-way down: {}", held[49]);
        assert_eq!(held[149], 0.0, "silent and still held");
        assert!(!env.idle(0));
    }

    // Spec: analog R14
    #[test]
    fn legato_continues_from_the_level_the_last_note_reached() {
        let mut env = started(&SET, 1.0);
        run(&mut env, &SET, 60);
        let reached = env.level[0];
        let mut fresh = Envelope::new();
        fresh.level[0] = reached;
        fresh.trigger(0, &SET, false, 1.0);
        assert!(
            run(&mut fresh, &SET, 1)[0] < 0.02,
            "a new note starts from silence"
        );
        env.trigger(0, &SET, true, 1.0);
        let next = run(&mut env, &SET, 1)[0];
        assert!(
            next > reached && next < reached + 0.02,
            "{next} follows {reached}"
        );
    }

    // Spec: analog R14
    #[test]
    fn free_ignores_the_key_and_runs_attack_decay_and_release() {
        let set = EnvSettings {
            attack: 10.0,
            decay: 10.0,
            release: 10.0,
            free: true,
            ..SET
        };
        let mut env = started(&set, 1.0);
        env.release(0, &set);
        let all = run(&mut env, &set, 30);
        assert_eq!(
            all.iter().copied().fold(0.0, f32::max),
            1.0,
            "the attack isn't cut short"
        );
        assert!(
            all.iter().skip(25).all(|&x| x == 0.0) || env.idle(0),
            "released by itself"
        );
        let mut held = started(&set, 1.0);
        run(&mut held, &set, 40);
        assert!(held.idle(0), "no sustain: it ends with the key still down");
    }

    // Spec: analog R15
    #[test]
    fn ad_r_repeats_attack_and_decay_and_releases_when_the_key_is_let_go() {
        let set = EnvSettings {
            attack: 10.0,
            decay: 10.0,
            release: 10.0,
            looping: AD_R,
            ..SET
        };
        let mut env = started(&set, 1.0);
        let held = run(&mut env, &set, 200);
        let peaks = held
            .windows(2)
            .filter(|w| w[0] < 1.0 && w[1] >= 1.0)
            .count();
        assert!(peaks >= 9, "{peaks} loops");
        assert!(
            held.iter().skip(20).all(|&x| x >= 0.49),
            "it loops between the sustain and the peak"
        );
        env.release(0, &set);
        let release = run(&mut env, &set, 12);
        assert!(release.last() == Some(&0.0) || env.idle(0), "released");
    }

    // Spec: analog R15
    #[test]
    fn adr_r_repeats_attack_decay_and_release_while_the_key_is_held() {
        let set = EnvSettings {
            attack: 10.0,
            decay: 10.0,
            release: 10.0,
            looping: ADR_R,
            ..SET
        };
        let mut env = started(&set, 1.0);
        let held = run(&mut env, &set, 300);
        let peaks = held
            .windows(2)
            .filter(|w| w[0] < 1.0 && w[1] >= 1.0)
            .count();
        assert!(peaks >= 9, "{peaks} loops");
        assert!(
            held.iter().skip(30).any(|&x| x == 0.0),
            "each loop falls to silence"
        );
        env.release(0, &set);
        run(&mut env, &set, 30);
        assert!(env.idle(0), "the loop ends when the key is let go");
    }

    // Spec: analog R15
    #[test]
    fn ads_r_holds_the_sustain_and_replays_the_attack_when_the_key_is_let_go() {
        let set = EnvSettings {
            attack: 10.0,
            decay: 10.0,
            release: 10.0,
            looping: ADS_R,
            ..SET
        };
        let mut env = started(&set, 1.0);
        run(&mut env, &set, 30);
        assert!(
            run(&mut env, &set, 100).iter().all(|&x| x == 0.5),
            "held at the sustain"
        );
        env.release(0, &set);
        let after = run(&mut env, &set, 40);
        assert_eq!(
            after.iter().copied().fold(0.0, f32::max),
            1.0,
            "the attack once more, to the peak"
        );
        assert!(after[0] > 0.5, "from where it was");
        assert!(env.idle(0), "then the release, to silence");
    }

    // Spec: analog R8
    #[test]
    fn the_formants_sit_at_the_tables_vowels_and_move_with_freq() {
        for (reso, at) in [
            (0.0, VOWELS[0]),
            (0.25, VOWELS[1]),
            (0.5, VOWELS[2]),
            (0.75, VOWELS[3]),
            (1.0, VOWELS[4]),
        ] {
            assert_eq!(formants(reso, 1_000.0), at);
        }
        let (a, b) = (formants(0.125, 1_000.0), formants(0.0, 1_000.0));
        assert!(
            a.0 < b.0 && a.0 > VOWELS[1].0,
            "crossfaded between neighbours"
        );
        assert_eq!(formants(0.5, 2_000.0), (540.0, 4_580.0));
    }
}
