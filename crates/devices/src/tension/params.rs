//! Tension's parameters, layout and banks (spec: tension R1, R18). A control
//! that velocity, the key or the MPE routings move is a [`C`]: its value, and
//! its Vel and Key amounts where it has them, take three IDs from `3 × C`.
//! The switches, choosers and shared parts have IDs of their own from 200.

use bl_engine::DeviceType;
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT, Widget,
};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use lspk_voice::MAX_VOICES;

use super::TENSION;
use crate::analog::MODES;
use crate::instrument;

/// How a control's travel runs.
#[derive(Clone, Copy)]
enum Taper {
    Linear,
    Log,
    Power(u8),
}

/// One control: its name, range, default, unit and taper, the default of its
/// Vel amount if it has one, whether it has a Key amount, whether a matrix
/// route can target it, and whether it is one the whole device shares.
struct Control {
    name: &'static str,
    range: (f64, f64),
    default: f64,
    unit: ParamUnit,
    taper: Taper,
    vel: Option<f64>,
    key: bool,
    dest: bool,
    global: bool,
}

macro_rules! controls {
    ($($variant:ident: $name:literal, $lo:literal..$hi:literal, $default:literal, $unit:ident, $taper:expr, vel $vel:expr, key $key:literal, $kind:ident, $text:literal;)*) => {
        /// A control velocity, the key or an MPE route can move.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        pub enum C { $($variant),* }

        impl C {
            /// Every control, in the order of the table.
            pub const ALL: &[C] = &[$(C::$variant),*];
        }

        const TABLE: &[(Control, &str)] = &[$((Control {
            name: $name,
            range: ($lo as f64, $hi as f64),
            default: $default as f64,
            unit: ParamUnit::$unit,
            taper: $taper,
            vel: $vel,
            key: $key,
            dest: controls!(@dest $kind),
            global: controls!(@global $kind),
        }, $text)),*];
    };
    (@dest amount) => { false };
    (@dest $other:ident) => { true };
    (@global shared) => { true };
    (@global $other:ident) => { false };
}

use Taper::{Linear, Log, Power};

controls! {
    ExcVelocity: "Exciter Velocity", 0..100, 50, Percent, Linear, vel Some(100.0), key true, voice,
        "How fast the exciter moves when it meets the string.";
    ExcPosition: "Exciter Position", 0..50, 20, Percent, Linear, vel Some(0.0), key true, voice,
        "Where the exciter meets the string, from the termination at 0 % to the middle at 50 %.";
    ExcForce: "Bow Force", 0..100, 50, Percent, Linear, vel Some(0.0), key true, voice,
        "How hard the bow presses on the string.";
    ExcFriction: "Bow Friction", 0..100, 50, Percent, Linear, vel Some(0.0), key true, voice,
        "How much more the bow grips a string that sticks than one that slips.";
    ExcMass: "Hammer Mass", 0..100, 50, Percent, Linear, vel Some(0.0), key true, voice,
        "How heavy the hammer is.";
    ExcStiffness: "Exciter Stiffness", 0..100, 50, Percent, Linear, vel Some(0.0), key true, voice,
        "How hard the hammer's or plectrum's contact is: a harder one is brighter.";
    ExcProtrusion: "Plectrum Protrusion", 0..100, 50, Percent, Linear, vel Some(0.0), key true, voice,
        "How far the plectrum pulls the string before it lets go.";
    ExcDamping: "Exciter Damping", 0..100, 20, Percent, Linear, vel Some(0.0), key true, voice,
        "How much the hammer or plectrum takes out of the string while it touches it.";
    DampMass: "Damper Mass", 0..100, 30, Percent, Linear, vel None, key true, voice,
        "How heavy the damper is: a heavier one is slower to land and lift.";
    DampStiffness: "Damper Stiffness", 0..100, 30, Percent, Linear, vel None, key true, voice,
        "From soft felt to metal: felt takes more out of the string than metal does.";
    DampVelocity: "Damper Velocity", 0..100, 50, Percent, Linear, vel None, key true, voice,
        "How fast the damper lands and lifts, when it is gated.";
    DampPosition: "Damper Position", 0..50, 10, Percent, Linear, vel None, key true, voice,
        "Where the damper rests on the string, from the termination at 0 % to the middle at 50 %.";
    DampDamping: "Damper Damping", 0..100, 50, Percent, Linear, vel None, key true, voice,
        "How much the damper takes out of the string while it rests on it.";
    StrDecay: "String Decay", 50..30000, 3000, Milliseconds, Log, vel None, key true, voice,
        "How long the string takes to fall 60 dB while the note is held.";
    StrRatio: "String Ratio", 0..100, 30, Percent, Linear, vel None, key false, voice,
        "How much faster the string decays after the note ends: 0 % decays alike.";
    StrInharm: "String Inharm", 0..100, 0, Percent, Linear, vel None, key false, voice,
        "How far the string's stiffness stretches its upper partials.";
    StrDamping: "String Damping", 0..100, 40, Percent, Linear, vel None, key true, voice,
        "How much faster the string's high harmonics die than its low ones.";
    VibDelay: "Vibrato Delay", 0..10000, 300, Milliseconds, Power(3), vel None, key false, voice,
        "How long after the note starts the vibrato begins.";
    VibAttack: "Vibrato Attack", 0..10000, 500, Milliseconds, Power(3), vel None, key false, voice,
        "How long the vibrato takes to fade in.";
    VibRate: "Vibrato Rate", 0.1..20, 5, Hertz, Log, vel None, key false, voice,
        "How fast the vibrato moves the pitch.";
    VibAmount: "Vibrato Amount", 0..100, 20, Percent, Linear, vel None, key false, voice,
        "How far the vibrato moves the pitch: a semitone at 100 %.";
    VibMod: "Vibrato Mod", 0..100, 0, Percent, Linear, vel None, key false, voice,
        "How much of the amount the mod wheel adds.";
    VibError: "Vibrato Error", 0..100, 0, Percent, Linear, vel None, key false, voice,
        "How much each note's vibrato varies from the others.";
    FingerMass: "Finger Mass", 0..100, 50, Percent, Linear, vel Some(0.0), key true, voice,
        "How heavy the finger is: a heavier one takes more of the string's high harmonics.";
    FingerStiff: "Finger Stiff", 0..100, 50, Percent, Linear, vel None, key false, voice,
        "How hard the finger's flesh is: a harder one takes less.";
    FretStiff: "Fret Stiff", 0..100, 50, Percent, Linear, vel None, key false, voice,
        "How sharply the fret reflects the wave.";
    PickupPosition: "Pickup Position", 0..50, 30, Percent, Linear, vel None, key false, voice,
        "Where the pickup hears the string: near the termination is brighter and thinner, near the middle fuller.";
    BodyDecay: "Body Decay", 10..2000, 200, Milliseconds, Log, vel None, key false, shared,
        "How long the body's resonances ring.";
    BodyStrBody: "Body Str/Body", -100..100, 0, Percent, Linear, vel None, key false, shared,
        "The balance of the string heard directly and the body: -100 % is the string alone, 100 % the body alone.";
    BodyLowCut: "Body Low Cut", 20..2000, 20, Hertz, Log, vel None, key false, shared,
        "Where the body's low frequencies are cut.";
    BodyHighCut: "Body High Cut", 1000..20000, 20000, Hertz, Log, vel None, key false, shared,
        "Where the body's high frequencies are cut.";
    FilterFreq: "Filter Freq", 20..20000, 2000, Hertz, Log, vel None, key false, voice,
        "The filter's cutoff; in a formant mode it transposes the vowels.";
    FilterRes: "Filter Res", 0..100, 20, Percent, Linear, vel None, key false, voice,
        "The filter's resonance; in a formant mode it moves through the vowels A, E, I, O, U.";
    FreqLfo: "Filter Freq LFO", -100..100, 0, Percent, Linear, vel None, key false, amount,
        "How far the filter's LFO moves the cutoff, up to four octaves.";
    FreqEnv: "Filter Freq Env", -100..100, 0, Percent, Linear, vel None, key false, amount,
        "How far the filter's envelope moves the cutoff, up to six octaves.";
    FreqKey: "Filter Freq Key", -100..100, 0, Percent, Linear, vel None, key false, amount,
        "How far the key moves the cutoff: 100 % follows it one octave per octave from C3.";
    ResLfo: "Filter Res LFO", -100..100, 0, Percent, Linear, vel None, key false, amount,
        "How far the filter's LFO moves the resonance.";
    ResEnv: "Filter Res Env", -100..100, 0, Percent, Linear, vel None, key false, amount,
        "How far the filter's envelope moves the resonance.";
    ResKey: "Filter Res Key", -100..100, 0, Percent, Linear, vel None, key false, amount,
        "How far the key moves the resonance.";
    EnvAttack: "Filter Env Attack", 0..20000, 5, Milliseconds, Power(3), vel Some(0.0), key false, voice,
        "How long the filter envelope takes to rise.";
    EnvDecay: "Filter Env Decay", 0..20000, 500, Milliseconds, Power(3), vel None, key false, voice,
        "How long the filter envelope takes to fall to its sustain level.";
    EnvSustain: "Filter Env Sustain", 0..100, 50, Percent, Linear, vel Some(0.0), key false, voice,
        "The level the filter envelope holds while the key is down.";
    EnvRelease: "Filter Env Release", 0..20000, 200, Milliseconds, Power(3), vel None, key false, voice,
        "How long the filter envelope takes to fall after the key is let go.";
    LfoRate: "Filter LFO Rate", 0.01..30, 1, Hertz, Log, vel None, key false, voice,
        "How fast the filter's LFO runs, when it isn't synced.";
    LfoDelay: "Filter LFO Delay", 0..10000, 0, Milliseconds, Power(3), vel None, key false, voice,
        "How long after the note starts the filter's LFO begins.";
    LfoAttack: "Filter LFO Attack", 0..10000, 0, Milliseconds, Power(3), vel None, key false, voice,
        "How long the filter's LFO takes to fade in.";
}

/// The number of controls.
pub const CONTROLS: usize = TABLE.len();

impl C {
    /// The control's index in [`C::ALL`].
    #[must_use]
    pub const fn n(self) -> usize {
        self as usize
    }

    /// The control's value.
    #[must_use]
    pub const fn id(self) -> ParamId {
        ParamId(3 * self as u32)
    }

    /// Its Vel amount, where it has one.
    #[must_use]
    pub const fn vel(self) -> ParamId {
        ParamId(3 * self as u32 + 1)
    }

    /// Its Key amount, where it has one.
    #[must_use]
    pub const fn key(self) -> ParamId {
        ParamId(3 * self as u32 + 2)
    }

    fn control(self) -> &'static Control {
        &TABLE[self as usize].0
    }

    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.control().name
    }

    /// Whether a matrix route can target it.
    #[must_use]
    pub fn is_dest(self) -> bool {
        self.control().dest
    }

    /// Whether the whole device shares it, so a route reads the oldest
    /// voice's sources.
    #[must_use]
    pub fn is_global(self) -> bool {
        self.control().global
    }

    /// Whether it has a Vel amount, and whether it has a Key amount.
    #[must_use]
    pub fn amounts(self) -> (bool, bool) {
        (self.control().vel.is_some(), self.control().key)
    }
}

// The switches, choosers and shared parts.
pub const EXC_ON: ParamId = ParamId(200);
pub const EXC_TYPE: ParamId = ParamId(201);
pub const EXC_FIX: ParamId = ParamId(202);
pub const DAMP_ON: ParamId = ParamId(203);
pub const DAMP_GATED: ParamId = ParamId(204);
pub const DAMP_FIX: ParamId = ParamId(205);
pub const VIB_ON: ParamId = ParamId(206);
pub const TERM_ON: ParamId = ParamId(207);
pub const PICKUP_ON: ParamId = ParamId(208);
pub const BODY_ON: ParamId = ParamId(209);
pub const BODY_TYPE: ParamId = ParamId(210);
pub const BODY_SIZE: ParamId = ParamId(211);
pub const FILTER_ON: ParamId = ParamId(212);
pub const FILTER_MODE: ParamId = ParamId(213);
pub const ENV_ON: ParamId = ParamId(214);
pub const LFO_ON: ParamId = ParamId(215);
pub const LFO_WAVE: ParamId = ParamId(216);
pub const LFO_SYNC: ParamId = ParamId(217);
pub const LFO_DIV: ParamId = ParamId(218);
pub const MPE_ON: ParamId = ParamId(219);
/// Pressure's and Slide's two destinations and amounts, in that order.
pub const MPE: [(ParamId, ParamId); 4] = [
    (ParamId(220), ParamId(221)),
    (ParamId(222), ParamId(223)),
    (ParamId(224), ParamId(225)),
    (ParamId(226), ParamId(227)),
];
pub const PB_RANGE: ParamId = ParamId(228);
pub const NOTE_PB: ParamId = ParamId(229);
pub const OCTAVE: ParamId = ParamId(230);
pub const SEMI: ParamId = ParamId(231);
pub const DETUNE: ParamId = ParamId(232);
pub const STRETCH: ParamId = ParamId(233);
pub const ERROR: ParamId = ParamId(234);
pub const VOICES: ParamId = ParamId(235);
pub const PRIORITY: ParamId = ParamId(236);
pub const PORT_ON: ParamId = ParamId(237);
pub const PORT_TIME: ParamId = ParamId(238);
pub const PORT_LEGATO: ParamId = ParamId(239);
pub const PORT_PROP: ParamId = ParamId(240);
pub const UNI_ON: ParamId = ParamId(241);
pub const UNI_VOICES: ParamId = ParamId(242);
pub const UNI_DETUNE: ParamId = ParamId(243);
pub const UNI_DELAY: ParamId = ParamId(244);
pub const VOLUME: ParamId = ParamId(245);

/// The exciter types, in the chooser's order (R2).
pub const EXCITERS: [&str; 4] = ["Bow", "Hammer", "Hammer (bouncing)", "Plectrum"];
/// The body types (R12).
pub const BODY_TYPES: [&str; 5] = ["Piano", "Guitar", "Violin", "Cello", "Bass"];
/// The body sizes (R12).
pub const BODY_SIZES: [&str; 5] = ["XS", "S", "M", "L", "XL"];
/// The filter LFO's waves (R13).
pub const LFO_WAVES: [&str; 5] = ["Sine", "Triangle", "Rectangle", "S&H", "Noise"];
/// The filter LFO's synced divisions, and each one's length in beats (R13).
pub const DIVISIONS: [&str; 10] = [
    "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars", "4 bars", "8 bars",
];
pub const DIVISION_BEATS: [f64; 10] = [0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0];
/// The unison's copies (R17), by the chooser's index.
pub const UNISON_COPIES: [&str; 2] = ["2", "4"];

/// The most unison copies a voice has.
pub const COPIES: usize = 4;

/// The descriptor of `c`'s value.
fn value_of(c: C) -> ParamDescriptor {
    let control = c.control();
    let mut p = ParamDescriptor::continuous(
        c.id(),
        control.name,
        control.range,
        control.default,
        control.unit,
    );
    p = match control.taper {
        Taper::Linear => p,
        Taper::Log => p.logarithmic(),
        Taper::Power(k) => p.power(k),
    };
    p.describe(TABLE[c.n()].1)
}

fn amount(id: ParamId, name: String, default: f64, text: String) -> ParamDescriptor {
    ParamDescriptor::continuous(id, &name, (-100.0, 100.0), default, ParamUnit::Percent)
        .describe(&text)
}

/// Every parameter, in the order the description lists them.
fn descriptors() -> Vec<ParamDescriptor> {
    let mut params = Vec::new();
    for &c in C::ALL {
        params.push(value_of(c));
        let (name, control) = (c.name(), c.control());
        if let Some(default) = control.vel {
            params.push(amount(
                c.vel(),
                format!("{name} Vel"),
                default,
                format!("How far the note's velocity moves {name}."),
            ));
        }
        if control.key {
            params.push(amount(
                c.key(),
                format!("{name} Key"),
                0.0,
                format!("How far the key moves {name}: positive raises it on higher keys."),
            ));
        }
    }
    let toggle =
        |id, name: &str, on, text: &str| ParamDescriptor::toggle(id, name, on).describe(text);
    let mut dests = vec!["Off"];
    dests.extend(C::ALL.iter().filter(|c| c.is_dest()).map(|c| c.name()));
    let routing = |(dest, amount_id): (ParamId, ParamId), source: &str, n: usize| {
        [
            ParamDescriptor::choice(dest, &format!("{source} Dest {n}"), &dests, 0).describe(
                &format!("The control {source} moves, by its amount; Off leaves it alone."),
            ),
            amount(
                amount_id,
                format!("{source} Amount {n}"),
                0.0,
                format!("How far {source} moves its destination, in per cent of its travel."),
            ),
        ]
    };
    params.extend([
        toggle(EXC_ON, "Exciter", true, "Turns the exciter on; off, only the damper sets the string moving."),
        ParamDescriptor::choice(EXC_TYPE, "Exciter Type", &EXCITERS, 1)
            .describe("A bow that keeps the string sounding, a hammer that strikes once, a hammer that bounces, or a plectrum that plucks."),
        toggle(EXC_FIX, "Exciter Fix. Pos", false, "Keeps the exciter's position a fixed distance along strings of every length, as at C3."),
        toggle(DAMP_ON, "Damper", true, "Turns the damper on."),
        toggle(DAMP_GATED, "Damper Gated", true, "Lets the damper's velocity set how fast it lands and lifts."),
        toggle(DAMP_FIX, "Damper Fix. Pos", false, "Keeps the damper's position a fixed distance along strings of every length, as at C3."),
        toggle(VIB_ON, "Vibrato", false, "Turns the vibrato on."),
        toggle(TERM_ON, "Termination", true, "Turns the finger and fret at the string's end on."),
        toggle(PICKUP_ON, "Pickup", true, "Turns the pickup on; off, the string is heard at its middle."),
        toggle(BODY_ON, "Body", false, "Turns the body on."),
        ParamDescriptor::choice(BODY_TYPE, "Body Type", &BODY_TYPES, 1).describe("The kind of instrument body the string is heard through."),
        ParamDescriptor::choice(BODY_SIZE, "Body Size", &BODY_SIZES, 2)
            .describe("The body's size: each step larger lowers its resonances by a fourth."),
        toggle(FILTER_ON, "Filter", false, "Turns the filter on."),
        ParamDescriptor::choice(FILTER_MODE, "Filter Mode", &MODES, 0)
            .describe("The filter's response and order; the formant modes sound vowels."),
        toggle(ENV_ON, "Filter Env", false, "Turns the filter's envelope on."),
        toggle(LFO_ON, "Filter LFO", false, "Turns the filter's LFO on."),
        ParamDescriptor::choice(LFO_WAVE, "Filter LFO Wave", &LFO_WAVES, 0).describe("The filter LFO's wave."),
        toggle(LFO_SYNC, "Filter LFO Sync", false, "Runs the filter's LFO at a division of the tempo instead of in hertz."),
        ParamDescriptor::choice(LFO_DIV, "Filter LFO Division", &DIVISIONS, 4)
            .describe("The tempo division the filter's LFO runs at when it is synced."),
        toggle(MPE_ON, "MPE", true, "Turns the pressure and slide routings on."),
    ]);
    for (n, (pair, source)) in MPE
        .into_iter()
        .zip(["Pressure", "Pressure", "Slide", "Slide"])
        .enumerate()
    {
        params.extend(routing(pair, source, n % 2 + 1));
    }
    params.extend([
        ParamDescriptor::integer(PB_RANGE, "Pitch Bend", (0, 24), 2, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the pitch bend wheel's range, in semitones."),
        ParamDescriptor::integer(NOTE_PB, "Note PB", (0, 48), 48, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the range of a per-note pitch bend, in semitones."),
        ParamDescriptor::integer(OCTAVE, "Octave", (-3, 3), 0, ParamUnit::None)
            .decimals(0)
            .describe("Transposes the instrument in octaves."),
        ParamDescriptor::integer(SEMI, "Semi", (-12, 12), 0, ParamUnit::Semitones)
            .decimals(0)
            .describe("Transposes the instrument in semitones."),
        ParamDescriptor::integer(DETUNE, "Detune", (-50, 50), 0, ParamUnit::Cents)
            .decimals(0)
            .describe("Tunes the instrument finely, in cents."),
        instrument::stretch(STRETCH),
        instrument::error(ERROR),
        instrument::voices(VOICES, MAX_VOICES as i64, 8),
        instrument::priority(PRIORITY),
        toggle(PORT_ON, "Portamento", false, "Turns the portamento on."),
        ParamDescriptor::continuous(
            PORT_TIME,
            "Portamento Time",
            (0.0, 10_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .power(2)
        .describe("How long a note takes to glide from the pitch before it."),
        toggle(
            PORT_LEGATO,
            "Portamento Legato",
            false,
            "Glides only into a note played while another key is held.",
        ),
        toggle(
            PORT_PROP,
            "Portamento Prop.",
            false,
            "Scales the glide's time with the interval, Time being an octave's.",
        ),
        toggle(UNI_ON, "Unison", false, "Turns the unison on."),
        ParamDescriptor::choice(UNI_VOICES, "Unison Voices", &UNISON_COPIES, 0)
            .describe("How many detuned copies play each note."),
        ParamDescriptor::continuous(
            UNI_DETUNE,
            "Unison Detune",
            (0.0, 50.0),
            10.0,
            ParamUnit::Cents,
        )
        .describe("How far the outermost copies sit from the note, in cents."),
        ParamDescriptor::continuous(
            UNI_DELAY,
            "Unison Delay",
            (0.0, 20.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long after the first copy the last one starts."),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, -6.0)
            .describe("Sets the instrument's output level, down to silence."),
    ]);
    params
}

/// A control's parameters: its value, then its Vel and Key amounts.
fn control_knobs(c: C) -> Vec<(ParamId, Widget)> {
    let (vel, key) = c.amounts();
    let mut knobs = vec![(c.id(), Widget::Knob)];
    if vel {
        knobs.push((c.vel(), Widget::Knob));
    }
    if key {
        knobs.push((c.key(), Widget::Knob));
    }
    knobs
}

/// A section: its name, and its controls in the order its band and its banks
/// hold them. A section's switch comes first.
type Section = (&'static str, Vec<(ParamId, Widget)>);
fn sections() -> [Vec<Section>; 2] {
    let controls = |list: &[C]| -> Vec<(ParamId, Widget)> {
        list.iter().flat_map(|&c| control_knobs(c)).collect()
    };
    let with = |head: &[(ParamId, Widget)], list: &[C]| -> Vec<(ParamId, Widget)> {
        head.iter().copied().chain(controls(list)).collect()
    };
    let (toggle, chooser, number) = (Widget::Toggle, Widget::Chooser, Widget::Number);
    let knob = Widget::Knob;
    let mpe: Vec<(ParamId, Widget)> = std::iter::once((MPE_ON, toggle))
        .chain(MPE.iter().flat_map(|&(d, a)| [(d, chooser), (a, knob)]))
        .chain([(PB_RANGE, number), (NOTE_PB, number)])
        .collect();
    let string_tab = vec![
        (
            "Exciter",
            with(
                &[(EXC_ON, toggle), (EXC_TYPE, chooser), (EXC_FIX, toggle)],
                &[
                    C::ExcVelocity,
                    C::ExcPosition,
                    C::ExcForce,
                    C::ExcFriction,
                    C::ExcMass,
                    C::ExcStiffness,
                    C::ExcProtrusion,
                    C::ExcDamping,
                ],
            ),
        ),
        (
            "Damper",
            with(
                &[(DAMP_ON, toggle), (DAMP_GATED, toggle), (DAMP_FIX, toggle)],
                &[
                    C::DampMass,
                    C::DampStiffness,
                    C::DampVelocity,
                    C::DampPosition,
                    C::DampDamping,
                ],
            ),
        ),
        (
            "String",
            controls(&[C::StrDecay, C::StrRatio, C::StrInharm, C::StrDamping]),
        ),
        (
            "Vibrato",
            with(
                &[(VIB_ON, toggle)],
                &[
                    C::VibDelay,
                    C::VibAttack,
                    C::VibRate,
                    C::VibAmount,
                    C::VibMod,
                    C::VibError,
                ],
            ),
        ),
        (
            "Termination",
            with(
                &[(TERM_ON, toggle)],
                &[C::FingerMass, C::FingerStiff, C::FretStiff],
            ),
        ),
        ("Pickup", with(&[(PICKUP_ON, toggle)], &[C::PickupPosition])),
        (
            "Body",
            with(
                &[
                    (BODY_ON, toggle),
                    (BODY_TYPE, chooser),
                    (BODY_SIZE, chooser),
                ],
                &[C::BodyDecay, C::BodyStrBody, C::BodyLowCut, C::BodyHighCut],
            ),
        ),
    ];
    let filter_tab = vec![
        (
            "Filter",
            with(
                &[(FILTER_ON, toggle), (FILTER_MODE, chooser)],
                &[
                    C::FilterFreq,
                    C::FreqLfo,
                    C::FreqEnv,
                    C::FreqKey,
                    C::FilterRes,
                    C::ResLfo,
                    C::ResEnv,
                    C::ResKey,
                ],
            ),
        ),
        (
            "Filter Envelope",
            with(
                &[(ENV_ON, toggle)],
                &[C::EnvAttack, C::EnvDecay, C::EnvSustain, C::EnvRelease],
            ),
        ),
        (
            "Filter LFO",
            with(
                &[
                    (LFO_ON, toggle),
                    (LFO_WAVE, chooser),
                    (LFO_SYNC, toggle),
                    (LFO_DIV, chooser),
                ],
                &[C::LfoRate, C::LfoDelay, C::LfoAttack],
            ),
        ),
        ("MPE", mpe),
        (
            "Keyboard",
            vec![
                (OCTAVE, number),
                (SEMI, number),
                (DETUNE, number),
                (STRETCH, knob),
                (ERROR, knob),
                (VOICES, number),
                (PRIORITY, chooser),
            ],
        ),
        (
            "Portamento",
            vec![
                (PORT_ON, toggle),
                (PORT_TIME, knob),
                (PORT_LEGATO, toggle),
                (PORT_PROP, toggle),
            ],
        ),
        (
            "Unison",
            vec![
                (UNI_ON, toggle),
                (UNI_VOICES, chooser),
                (UNI_DETUNE, knob),
                (UNI_DELAY, knob),
            ],
        ),
    ];
    [string_tab, filter_tab]
}

/// Tension's pages (R1): the String and Filter/Global tabs, each a row of its
/// sections' bands, and Volume beside them so both tabs show it.
fn layout() -> DeviceLayout {
    let [string_tab, filter_tab] = sections();
    let tab = |name: &str, sections: Vec<(&str, Vec<(ParamId, Widget)>)>| {
        Page::new(
            name,
            LayoutNode::Row(
                sections
                    .into_iter()
                    .map(|(_, controls)| {
                        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT - TAB_HEIGHT, &controls))
                    })
                    .collect(),
            ),
        )
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Pages(PagesNode::new(
            "Tabs",
            PageStyle::Tabs,
            vec![tab("String", string_tab), tab("Filter/Global", filter_tab)],
        )),
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &[(VOLUME, Widget::Knob)])),
    ]))
}

/// Tension's banks (R18): each section's parameters, eight to a bank.
fn banks() -> Vec<(String, Vec<String>)> {
    let [string_tab, filter_tab] = sections();
    let mut banks = Vec::new();
    for (name, controls) in string_tab.into_iter().chain(filter_tab) {
        let ids: Vec<String> = controls
            .iter()
            .map(|(id, _)| format!("#{}", id.0))
            .collect();
        for (n, chunk) in ids.chunks(bl_model::BANK_SIZE).enumerate() {
            let label = if ids.len() > bl_model::BANK_SIZE {
                format!("{name} {}", n + 1)
            } else {
                name.to_owned()
            };
            banks.push((label, chunk.to_vec()));
        }
    }
    banks.push(("Global".to_owned(), vec![format!("#{}", VOLUME.0)]));
    banks
}

/// Tension's type.
#[must_use]
pub fn tension_type() -> DeviceType {
    let owned = banks();
    let borrowed: Vec<(&str, Vec<&str>)> = owned
        .iter()
        .map(|(name, ids)| (name.as_str(), ids.iter().map(String::as_str).collect()))
        .collect();
    let slices: Vec<(&str, &[&str])> = borrowed.iter().map(|(n, v)| (*n, v.as_slice())).collect();
    DeviceType::native(TENSION, "Tension", DeviceRole::Instrument)
        .with_description(
            DeviceDescription::new(descriptors())
                .with_layout(layout())
                .with_banks(&slices),
        )
        .with_matrix(super::matrix_layout())
}
