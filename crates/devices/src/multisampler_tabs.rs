//! The multisampler's Pitch/Osc, Filter/Global, Modulation and MIDI tabs
//! (WP-5.14; the multisampler spec, R32 to R46): their parameters, their
//! layout, and the tab indicators of R1. The voices that run them are in
//! [`crate::multisampler`].

use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{ParamDescriptor, ParamId, ParamUnit, SamplerTab};

use crate::instrument_filter::CIRCUITS;

/// The longest envelope time, in milliseconds.
const ENVELOPE_MAX_MS: f64 = 20_000.0;

/// The ids of one envelope's parameters (R24, R25, R26): the volume envelope
/// has no End level.
#[derive(Clone, Copy)]
pub struct EnvIds {
    pub initial: ParamId,
    pub peak: ParamId,
    pub sustain: ParamId,
    pub end: Option<ParamId>,
    pub attack: ParamId,
    pub decay: ParamId,
    pub release: ParamId,
    pub attack_slope: ParamId,
    pub decay_slope: ParamId,
    pub release_slope: ParamId,
    pub mode: ParamId,
    pub loop_time: ParamId,
    pub repeat: ParamId,
    pub time_vel: ParamId,
}

impl EnvIds {
    /// An envelope whose fourteen parameters follow `base` in order.
    const fn at(base: u32) -> Self {
        Self {
            initial: ParamId(base),
            peak: ParamId(base + 1),
            sustain: ParamId(base + 2),
            end: Some(ParamId(base + 3)),
            attack: ParamId(base + 4),
            decay: ParamId(base + 5),
            release: ParamId(base + 6),
            attack_slope: ParamId(base + 7),
            decay_slope: ParamId(base + 8),
            release_slope: ParamId(base + 9),
            mode: ParamId(base + 10),
            loop_time: ParamId(base + 11),
            repeat: ParamId(base + 12),
            time_vel: ParamId(base + 13),
        }
    }
}

/// The pitch envelope's parameters (R35).
pub const PITCH_ENV: EnvIds = EnvIds::at(100);
/// The filter envelope's (R38).
pub const FILTER_ENV: EnvIds = EnvIds::at(114);
/// The auxiliary envelope's (R40).
pub const AUX_ENV: EnvIds = EnvIds::at(128);
/// The modulation oscillator's envelope's (R36).
pub const OSC_ENV: EnvIds = EnvIds::at(142);

// ----- Pitch/Osc ----------------------------------------------------------

/// Transpose, in semitones (R32).
pub const TRANSPOSE: ParamId = ParamId(160);
/// Detune, in cents (R32).
pub const DETUNE: ParamId = ParamId(161);
/// Spread: two detuned voices per layer (R32).
pub const SPREAD: ParamId = ParamId(162);
/// Key Zone Shift, in semitones (R33).
pub const KEY_ZONE_SHIFT: ParamId = ParamId(163);
/// Glide: Off, Glide or Portamento (R34).
pub const GLIDE: ParamId = ParamId(164);
/// Glide Time (R34).
pub const GLIDE_TIME: ParamId = ParamId(165);
/// The pitch envelope's Amount, in semitones (R35).
pub const PITCH_ENV_AMOUNT: ParamId = ParamId(166);
/// The modulation oscillator's switch (R36).
pub const OSC_ON: ParamId = ParamId(167);
/// FM or AM (R36).
pub const OSC_MODE: ParamId = ParamId(168);
/// The modulation oscillator's waveform (R36).
pub const OSC_TYPE: ParamId = ParamId(169);
/// Its depth (R36).
pub const OSC_VOLUME: ParamId = ParamId(170);
/// How much velocity scales its depth (R36).
pub const OSC_VOL_VEL: ParamId = ParamId(171);
/// Whether it runs at a fixed frequency (R36).
pub const OSC_FIXED: ParamId = ParamId(172);
/// Its frequency while Fixed is on (R36).
pub const OSC_FREQ: ParamId = ParamId(173);
/// Fixed's multiplier (R36).
pub const OSC_MULTI: ParamId = ParamId(174);
/// Its coarse frequency ratio while Fixed is off (R36).
pub const OSC_COARSE: ParamId = ParamId(175);
/// Its fine frequency ratio while Fixed is off (R36).
pub const OSC_FINE: ParamId = ParamId(176);

// ----- Filter/Global --------------------------------------------------------

/// The filter's switch (R37).
pub const FILTER_ON: ParamId = ParamId(180);
/// Low-pass, high-pass, band-pass, notch or Morph (R37).
pub const FILTER_TYPE: ParamId = ParamId(181);
/// 12 or 24 dB per octave (R37).
pub const FILTER_SLOPE: ParamId = ParamId(182);
/// The cutoff (R37).
pub const FILTER_FREQ: ParamId = ParamId(183);
/// The resonance (R37).
pub const FILTER_RES: ParamId = ParamId(184);
/// Where Morph sits (R37).
pub const FILTER_MORPH: ParamId = ParamId(185);
/// The drive into the filter (R37).
pub const FILTER_DRIVE: ParamId = ParamId(186);
/// The filter envelope's switch (R38).
pub const FENV_ON: ParamId = ParamId(187);
/// The filter envelope's Amount, in semitones (R38).
pub const FENV_AMOUNT: ParamId = ParamId(188);
/// The filter's circuit: Clean, OSR, MS2, SMP or PRD (the instrument filter
/// spec, R2). Older sets lack it and open on Clean.
pub const FILTER_CIRCUIT: ParamId = ParamId(193);
/// The shaper's switch (R39).
pub const SHAPER_ON: ParamId = ParamId(189);
/// Soft, Hard, Sine or 4bit (R39).
pub const SHAPER_TYPE: ParamId = ParamId(190);
/// The shaper's Amount (R39).
pub const SHAPER_AMOUNT: ParamId = ParamId(191);
/// Puts the shaper after the filter (R39).
pub const SHAPER_POST: ParamId = ParamId(192);

// ----- Modulation -------------------------------------------------------------

/// The auxiliary envelope's switch (R40).
pub const AUX_ON: ParamId = ParamId(200);
/// Its first destination (R40, R46).
pub const AUX_DEST_A: ParamId = ParamId(201);
/// Its second destination.
pub const AUX_DEST_B: ParamId = ParamId(202);
/// Its first amount.
pub const AUX_AMOUNT_A: ParamId = ParamId(203);
/// Its second amount.
pub const AUX_AMOUNT_B: ParamId = ParamId(204);

/// One LFO's parameters (R41, R42): the nine that all three have follow
/// `base`; LFO 1 then has its four amounts, LFOs 2 and 3 their
/// destinations, amounts, Phase and Spin.
#[derive(Clone, Copy)]
pub struct LfoIds {
    pub on: ParamId,
    pub wave: ParamId,
    pub rate: ParamId,
    pub sync: ParamId,
    pub division: ParamId,
    pub attack: ParamId,
    pub retrig: ParamId,
    pub offset: ParamId,
    pub key: ParamId,
    /// LFO 1's Vol, Pan, Filter and Pitch, or LFOs 2 and 3's Destination A
    /// and B, Amount A and B, Phase and Spin.
    pub extra: ParamId,
}

impl LfoIds {
    const fn at(base: u32) -> Self {
        Self {
            on: ParamId(base),
            wave: ParamId(base + 1),
            rate: ParamId(base + 2),
            sync: ParamId(base + 3),
            division: ParamId(base + 4),
            attack: ParamId(base + 5),
            retrig: ParamId(base + 6),
            offset: ParamId(base + 7),
            key: ParamId(base + 8),
            extra: ParamId(base + 9),
        }
    }

    /// The `n`th of the parameters after the nine shared ones.
    #[must_use]
    pub const fn extra(self, n: u32) -> ParamId {
        ParamId(self.extra.0 + n)
    }
}

/// The three LFOs.
pub const LFOS: [LfoIds; 3] = [LfoIds::at(210), LfoIds::at(230), LfoIds::at(250)];

/// LFO 1's Vol, Pan, Filter and Pitch, as offsets from its `extra`.
pub const LFO1_VOL: u32 = 0;
pub const LFO1_PAN: u32 = 1;
pub const LFO1_FILTER: u32 = 2;
pub const LFO1_PITCH: u32 = 3;
/// LFOs 2 and 3's Destination A and B, Amount A and B, Phase and Spin, as
/// offsets from their `extra`.
pub const LFO_DEST_A: u32 = 0;
pub const LFO_DEST_B: u32 = 1;
pub const LFO_AMOUNT_A: u32 = 2;
pub const LFO_AMOUNT_B: u32 = 3;
pub const LFO_PHASE: u32 = 4;
pub const LFO_SPIN: u32 = 5;

// ----- MIDI ---------------------------------------------------------------------

/// The MIDI tab's sources (R43), in tab order.
pub const MIDI_SOURCES: [&str; 7] = [
    "Key",
    "Velocity",
    "Release Velocity",
    "Aftertouch",
    "Mod Wheel",
    "Foot Controller",
    "Pitch Bend",
];

/// A MIDI source's four parameters (Destination A and B, Amount A and B)
/// follow `MIDI_BASE + 4 × source`.
pub const MIDI_BASE: u32 = 270;
/// Pitch Bend Range, in semitones (R44).
pub const BEND_RANGE: ParamId = ParamId(298);

/// Source `source`'s Destination A, Destination B, Amount A or Amount B
/// (`part` 0 to 3).
#[must_use]
pub const fn midi_param(source: usize, part: u32) -> ParamId {
    ParamId(MIDI_BASE + 4 * source as u32 + part)
}

// ----- Choices ------------------------------------------------------------------

/// The destinations of R46, in the order their choosers list them.
pub const DESTINATIONS: [&str; 21] = [
    "None",
    "Volume",
    "Pan",
    "Pitch",
    "Sample Start",
    "Loop Length",
    "Sample Selector",
    "Filter Freq",
    "Filter Res",
    "Filter Morph",
    "Filter Drive",
    "Shaper Amount",
    "Osc Volume",
    "Osc Pitch",
    "Pitch Env Amount",
    "Filter Env Amount",
    "LFO 1 Rate",
    "LFO 2 Rate",
    "LFO 3 Rate",
    "LFO 1 Amount",
    "Time",
];

/// Destination numbers (R46).
pub mod dest {
    pub const NONE: usize = 0;
    pub const VOLUME: usize = 1;
    pub const PAN: usize = 2;
    pub const PITCH: usize = 3;
    pub const SAMPLE_START: usize = 4;
    pub const LOOP_LENGTH: usize = 5;
    pub const SELECTOR: usize = 6;
    pub const FILTER_FREQ: usize = 7;
    pub const FILTER_RES: usize = 8;
    pub const FILTER_MORPH: usize = 9;
    pub const FILTER_DRIVE: usize = 10;
    pub const SHAPER: usize = 11;
    pub const OSC_VOLUME: usize = 12;
    pub const OSC_PITCH: usize = 13;
    pub const PITCH_ENV: usize = 14;
    pub const FILTER_ENV: usize = 15;
    pub const LFO1_RATE: usize = 16;
    pub const LFO2_RATE: usize = 17;
    pub const LFO3_RATE: usize = 18;
    pub const LFO1_AMOUNT: usize = 19;
    pub const TIME: usize = 20;
}

/// The Glide choices (R34).
pub const GLIDES: [&str; 3] = ["Off", "Glide", "Portamento"];
/// The modulation oscillator's modes (R36).
pub const OSC_MODES: [&str; 2] = ["FM", "AM"];
/// Its waveforms (R36).
pub const OSC_TYPES: [&str; 13] = [
    "Sine",
    "Triangle",
    "Saw",
    "Square",
    "Noise",
    "Saw 4",
    "Saw 8",
    "Saw 16",
    "Saw 32",
    "Square 4",
    "Square 8",
    "Square 16",
    "Square 32",
];
/// The stops Morph's context menu snaps it to (R37): the four types it
/// sweeps through, with its value at each.
pub const MORPH_STOPS: [(&str, f64); 4] = [
    ("Low-pass", 0.0),
    ("Band-pass", 25.0),
    ("High-pass", 50.0),
    ("Notch", 75.0),
];
/// Fixed's multipliers (R36).
pub const OSC_MULTIS: [&str; 4] = ["0.01", "0.1", "1", "10"];
/// The filter's types (R37).
pub const FILTER_TYPES: [&str; 5] = ["Low-pass", "High-pass", "Band-pass", "Notch", "Morph"];
/// The filter's slopes (R37).
pub const FILTER_SLOPES: [&str; 2] = ["12 dB", "24 dB"];
/// The shaper's curves (R39).
pub const SHAPER_TYPES: [&str; 4] = ["Soft", "Hard", "Sine", "4bit"];
/// An LFO's waveforms (R41).
pub const LFO_WAVES: [&str; 6] = ["Sine", "Square", "Triangle", "Saw Down", "Saw Up", "S&H"];

// ----- Descriptors -----------------------------------------------------------------

fn time(id: ParamId, name: &str, default: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(
        id,
        name,
        (0.0, ENVELOPE_MAX_MS),
        default,
        ParamUnit::Milliseconds,
    )
}

fn percent(id: ParamId, name: &str, range: (f64, f64), default: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(id, name, range, default, ParamUnit::Percent)
}

/// One envelope's parameters. `bipolar` envelopes (the pitch, filter and
/// auxiliary ones) have levels from −100 to 100 %, the oscillator's from 0 to
/// 100 %; `prefix` names them.
fn envelope(ids: EnvIds, prefix: &str, bipolar: bool) -> Vec<ParamDescriptor> {
    let levels = if bipolar {
        (-100.0, 100.0)
    } else {
        (0.0, 100.0)
    };
    let name = |part: &str| format!("{prefix} {part}");
    let slope = |id, part: &str, of: &str| {
        percent(id, &name(part), (-100.0, 100.0), 0.0)
            .describe(&format!("The {prefix} envelope's {of} curve: positive moves fast then slowly, negative slowly then fast, zero in a straight line."))
    };
    vec![
        percent(ids.initial, &name("Init"), levels, 0.0)
            .describe(&format!("The {prefix} envelope's level when a note starts.")),
        percent(ids.peak, &name("Peak"), levels, 100.0)
            .describe(&format!("The level the {prefix} envelope's attack reaches.")),
        percent(ids.sustain, &name("Sust"), levels, 100.0)
            .describe(&format!("The level the {prefix} envelope holds while the note sounds.")),
        percent(ids.end.expect("a modulation envelope has an End"), &name("End"), levels, 0.0)
            .describe(&format!("The level the {prefix} envelope falls to after the note ends.")),
        time(ids.attack, &name("Attack"), 0.0)
            .describe(&format!("The time the {prefix} envelope takes from Init to Peak.")),
        time(ids.decay, &name("Decay"), 0.0)
            .describe(&format!("The time the {prefix} envelope takes from Peak to Sust.")),
        time(ids.release, &name("Release"), 50.0)
            .describe(&format!("The time the {prefix} envelope takes to reach End after the note ends.")),
        slope(ids.attack_slope, "A Slope", "attack"),
        slope(ids.decay_slope, "D Slope", "decay"),
        slope(ids.release_slope, "R Slope", "release"),
        ParamDescriptor::choice(ids.mode, &name("Loop"), &crate::multisampler::ENV_LOOPS, 0)
            .describe(&format!("What the {prefix} envelope does while a note is held: Loop, Beat, Sync or Trigger repeat it as the volume envelope's Loop does.")),
        time(ids.loop_time, &name("Loop Time"), 0.0)
            .describe(&format!("How long the looping {prefix} envelope takes to go back from Sust to Init.")),
        ParamDescriptor::choice(ids.repeat, &name("Repeat"), &crate::multisampler::REPEATS, 4)
            .describe(&format!("How often a Beat or Sync {prefix} envelope starts again.")),
        percent(ids.time_vel, &name("Time < Vel"), (-100.0, 100.0), 0.0)
            .describe(&format!("Shortens the {prefix} envelope's segments for hard notes.")),
    ]
}

fn destination(id: ParamId, name: &str) -> ParamDescriptor {
    ParamDescriptor::choice(id, name, &DESTINATIONS, 0)
        .describe("Where this modulation source goes; None sends it nowhere.")
}

fn amount(id: ParamId, name: &str) -> ParamDescriptor {
    percent(id, name, (-100.0, 100.0), 0.0).describe(
        "How far the source moves its destination, across the destination's span; negative \
         values move it the other way.",
    )
}

fn lfo(n: usize) -> Vec<ParamDescriptor> {
    let ids = LFOS[n];
    let name = |part: &str| format!("LFO {} {part}", n + 1);
    let mut params = vec![
        ParamDescriptor::toggle(ids.on, &name("On"), false)
            .describe("Turns this LFO on; it runs per voice."),
        ParamDescriptor::choice(ids.wave, &name("Wave"), &LFO_WAVES, 0)
            .describe("The LFO's waveform; S&H holds a new random value each cycle."),
        ParamDescriptor::continuous(ids.rate, &name("Rate"), (0.01, 30.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in hertz while Sync is off."),
        ParamDescriptor::toggle(ids.sync, &name("Sync"), false)
            .describe("Sets the LFO's rate as a note value of the set's tempo."),
        ParamDescriptor::choice(ids.division, &name("Division"), &crate::multisampler::REPEATS, 4)
            .describe("The LFO's rate while synced, from a 64th note to 8 bars."),
        time(ids.attack, &name("Attack"), 0.0)
            .describe("How long the LFO takes to reach full intensity after a note starts."),
        ParamDescriptor::toggle(ids.retrig, &name("Retrig"), false).describe(
            "Restarts the LFO at Offset on each note; off, a note's LFO starts where the free-running one is.",
        ),
        ParamDescriptor::continuous(ids.offset, &name("Offset"), (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("The phase in degrees a retriggered LFO starts at; it does nothing with Retrig off."),
        percent(ids.key, &name("Key"), (0.0, 100.0), 0.0).describe(
            "How much the played note speeds the LFO up: at 100 % an octave above C3 runs twice as fast.",
        ),
    ];
    // Amounts are parameters of the three LFOs alike, named as the tab shows them.
    if n == 0 {
        for (part, label) in ["Vol", "Pan", "Filter", "Pitch"].into_iter().enumerate() {
            let (range, text) = match label {
                "Vol" => (
                    (0.0, 100.0),
                    "How far the LFO swings the level: at 100 % between full and silence.",
                ),
                "Pan" => (
                    (0.0, 100.0),
                    "How far the LFO swings the pan: at 100 % from hard left to hard right.",
                ),
                "Filter" => (
                    (0.0, 24.0),
                    "How far the LFO moves the filter's cutoff, in semitones each way.",
                ),
                _ => (
                    (0.0, 100.0),
                    "How far the LFO bends the pitch: at 100 % an octave each way.",
                ),
            };
            let unit = if label == "Filter" {
                ParamUnit::Semitones
            } else {
                ParamUnit::Percent
            };
            params.push(
                ParamDescriptor::continuous(ids.extra(part as u32), &name(label), range, 0.0, unit)
                    .describe(text),
            );
        }
    } else {
        params.push(destination(ids.extra(LFO_DEST_A), &name("Dest A")));
        params.push(destination(ids.extra(LFO_DEST_B), &name("Dest B")));
        params.push(amount(ids.extra(LFO_AMOUNT_A), &name("Amount A")));
        params.push(amount(ids.extra(LFO_AMOUNT_B), &name("Amount B")));
        params.push(
            ParamDescriptor::continuous(
                ids.extra(LFO_PHASE),
                &name("Phase"),
                (0.0, 360.0),
                0.0,
                ParamUnit::None,
            )
            .decimals(0)
            .describe("How many degrees the right channel's LFO runs ahead of the left's."),
        );
        params.push(
            percent(ids.extra(LFO_SPIN), &name("Spin"), (0.0, 50.0), 0.0)
                .describe("How much faster the right channel's LFO runs than the left's."),
        );
    }
    params
}

/// The parameters of the four tabs (R32 to R46), in the order the device
/// lists them after the ones of WP-5.13.
#[must_use]
pub fn params() -> Vec<ParamDescriptor> {
    let mut out = Vec::new();
    out.extend(envelope(PITCH_ENV, "Pitch", true));
    out.extend(envelope(FILTER_ENV, "Filter", true));
    out.extend(envelope(AUX_ENV, "Aux", true));
    out.extend(envelope(OSC_ENV, "Osc", false));
    out.extend([
        ParamDescriptor::continuous(TRANSPOSE, "Transpose", (-48.0, 48.0), 0.0, ParamUnit::Semitones)
            .decimals(0)
            .describe("Moves every voice's pitch, in semitones."),
        ParamDescriptor::continuous(DETUNE, "Detune", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .decimals(0)
            .describe("Moves every voice's pitch, in cents."),
        percent(SPREAD, "Spread", (0.0, 100.0), 0.0).describe(
            "Plays each layer as two voices detuned against each other, by up to 25 cents each way at 100 %.",
        ),
        ParamDescriptor::continuous(
            KEY_ZONE_SHIFT,
            "Key Zone Shift",
            (-48.0, 48.0),
            0.0,
            ParamUnit::Semitones,
        )
        .decimals(0)
        .describe("Moves the key that chooses the layers, never the pitch they play at."),
        ParamDescriptor::choice(GLIDE, "Glide", &GLIDES, 0).describe(
            "Glide slides a note's pitch from the sounding note's, monophonically; Portamento slides each note from the last note's pitch, polyphonically.",
        ),
        ParamDescriptor::continuous(GLIDE_TIME, "Glide Time", (0.0, 1000.0), 100.0, ParamUnit::Milliseconds)
            .describe("How long a glide from one pitch to the next takes."),
        ParamDescriptor::continuous(
            PITCH_ENV_AMOUNT,
            "Pitch Env Amount",
            (-48.0, 48.0),
            0.0,
            ParamUnit::Semitones,
        )
        .describe("How far the pitch envelope moves the pitch at its full level, in semitones."),
        ParamDescriptor::toggle(OSC_ON, "Osc On", false)
            .describe("Turns the modulation oscillator on; it is never heard itself."),
        ParamDescriptor::choice(OSC_MODE, "Osc Mode", &OSC_MODES, 0).describe(
            "FM moves the sample's pitch with the oscillator; AM moves its level.",
        ),
        ParamDescriptor::choice(OSC_TYPE, "Osc Type", &OSC_TYPES, 0)
            .describe("The modulation oscillator's waveform."),
        percent(OSC_VOLUME, "Osc Volume", (0.0, 100.0), 0.0)
            .describe("The modulation oscillator's depth, shaped by its envelope."),
        percent(OSC_VOL_VEL, "Osc Vol < Vel", (-100.0, 100.0), 0.0)
            .describe("How much velocity scales the modulation oscillator's depth."),
        ParamDescriptor::toggle(OSC_FIXED, "Osc Fixed", false)
            .describe("Runs the oscillator at a fixed frequency instead of the note's."),
        ParamDescriptor::continuous(OSC_FREQ, "Osc Freq", (1.0, 10_000.0), 100.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The oscillator's frequency while Fixed is on, times Multi."),
        ParamDescriptor::choice(OSC_MULTI, "Osc Multi", &OSC_MULTIS, 2)
            .describe("Multiplies Freq while Fixed is on."),
        ParamDescriptor::continuous(OSC_COARSE, "Osc Coarse", (0.125, 48.0), 1.0, ParamUnit::None)
            .logarithmic()
            .describe("The oscillator's frequency as a multiple of the note's, while Fixed is off."),
        ParamDescriptor::continuous(OSC_FINE, "Osc Fine", (0.0, 1000.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Adds Fine/1000 to Coarse, while Fixed is off."),
        ParamDescriptor::toggle(FILTER_ON, "Filter On", false)
            .describe("Runs each voice through the filter; off bypasses it."),
        ParamDescriptor::choice(FILTER_TYPE, "Filter Type", &FILTER_TYPES, 0).describe(
            "Low-pass, high-pass, band-pass, notch, or Morph, which sweeps through all four.",
        ),
        ParamDescriptor::choice(FILTER_SLOPE, "Filter Slope", &FILTER_SLOPES, 0)
            .describe("The filter's steepness, 12 or 24 dB per octave."),
        ParamDescriptor::choice(FILTER_CIRCUIT, "Filter Circuit", &CIRCUITS, 0).describe(
            "The filter's circuit: Clean has no distortion of its own; OSR, MS2, SMP and PRD \
             part as the signal and Res rise. MS2, SMP and PRD play low-pass and high-pass \
             only; on other types the circuit plays Clean.",
        ),
        ParamDescriptor::continuous(FILTER_FREQ, "Filter Freq", (20.0, 20_000.0), 1000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The filter's cutoff frequency."),
        percent(FILTER_RES, "Filter Res", (0.0, 100.0), 0.0)
            .describe("Raises the filter's resonance at the cutoff."),
        percent(FILTER_MORPH, "Filter Morph", (0.0, 100.0), 0.0).describe(
            "Where the Morph type sits: low-pass at 0, band-pass at 25 %, high-pass at 50 %, notch at 75 %, and low-pass again at 100 %.",
        ),
        ParamDescriptor::continuous(FILTER_DRIVE, "Filter Drive", (0.0, 24.0), 0.0, ParamUnit::Decibels)
            .describe(
                "Drives the signal into the filter's circuit, for low-pass, high-pass and \
                 band-pass; Clean ignores it.",
            ),
        ParamDescriptor::toggle(FENV_ON, "Filter Env On", false)
            .describe("Lets the filter envelope move the cutoff."),
        ParamDescriptor::continuous(FENV_AMOUNT, "Filter Env Amount", (0.0, 72.0), 0.0, ParamUnit::Semitones)
            .describe("How far the filter envelope moves the cutoff at its full level, in semitones."),
        ParamDescriptor::toggle(SHAPER_ON, "Shaper On", false)
            .describe("Shapes each voice with the chosen curve."),
        ParamDescriptor::choice(SHAPER_TYPE, "Shaper Type", &SHAPER_TYPES, 0)
            .describe("The shaper's curve: a soft tanh, a hard clip, a sine fold or four bits."),
        percent(SHAPER_AMOUNT, "Shaper Amount", (0.0, 100.0), 0.0)
            .describe("How hard the shaper is driven, up to 24 dB at 100 %."),
        ParamDescriptor::toggle(SHAPER_POST, "Shaper Post", false)
            .describe("Puts the shaper after the filter instead of before it."),
        ParamDescriptor::toggle(AUX_ON, "Aux On", false)
            .describe("Turns the auxiliary envelope on."),
        destination(AUX_DEST_A, "Aux Dest A"),
        destination(AUX_DEST_B, "Aux Dest B"),
        amount(AUX_AMOUNT_A, "Aux Amount A"),
        amount(AUX_AMOUNT_B, "Aux Amount B"),
    ]);
    for n in 0..3 {
        out.extend(lfo(n));
    }
    for (source, name) in MIDI_SOURCES.iter().enumerate() {
        out.push(destination(
            midi_param(source, 0),
            &format!("{name} Dest A"),
        ));
        out.push(destination(
            midi_param(source, 1),
            &format!("{name} Dest B"),
        ));
        out.push(amount(midi_param(source, 2), &format!("{name} Amount A")));
        out.push(amount(midi_param(source, 3), &format!("{name} Amount B")));
    }
    out.push(
        ParamDescriptor::continuous(
            BEND_RANGE,
            "Pitch Bend Range",
            (0.0, 24.0),
            2.0,
            ParamUnit::Semitones,
        )
        .decimals(0)
        .describe("How far a full pitch bend moves every voice, in semitones."),
    );
    out
}

// ----- Layout ----------------------------------------------------------------------------

fn band(controls: &[(ParamId, Widget)]) -> LayoutNode {
    LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
}

fn envelope_band(amount: Option<ParamId>, ids: EnvIds, on: Option<ParamId>) -> LayoutNode {
    let mut controls = Vec::new();
    controls.extend(on.map(|id| (id, Widget::Toggle)));
    controls.extend(amount.map(|id| (id, Widget::Knob)));
    controls.extend([
        (ids.initial, Widget::Knob),
        (ids.peak, Widget::Knob),
        (ids.sustain, Widget::Knob),
        (
            ids.end.expect("a modulation envelope has an End"),
            Widget::Knob,
        ),
        (ids.attack, Widget::Knob),
        (ids.decay, Widget::Knob),
        (ids.release, Widget::Knob),
        (ids.attack_slope, Widget::Number),
        (ids.decay_slope, Widget::Number),
        (ids.release_slope, Widget::Number),
        (ids.mode, Widget::Chooser),
        (ids.loop_time, Widget::Number),
        (ids.repeat, Widget::Chooser),
        (ids.time_vel, Widget::Number),
    ]);
    band(&controls)
}

fn routes(controls: &mut Vec<(ParamId, Widget)>, dest_a: ParamId, amount_a: ParamId) {
    controls.extend([
        (dest_a, Widget::Chooser),
        (ParamId(dest_a.0 + 1), Widget::Chooser),
        (amount_a, Widget::Knob),
        (ParamId(amount_a.0 + 1), Widget::Knob),
    ]);
}

/// The Pitch/Osc tab's bands (R32 to R36).
#[must_use]
pub fn pitch_osc_bands() -> Vec<LayoutNode> {
    vec![
        band(&[
            (TRANSPOSE, Widget::Knob),
            (DETUNE, Widget::Knob),
            (SPREAD, Widget::Knob),
            (KEY_ZONE_SHIFT, Widget::Knob),
            (GLIDE, Widget::Chooser),
            (GLIDE_TIME, Widget::Knob),
        ]),
        envelope_band(Some(PITCH_ENV_AMOUNT), PITCH_ENV, None),
        band(&[
            (OSC_ON, Widget::Toggle),
            (OSC_MODE, Widget::Chooser),
            (OSC_TYPE, Widget::Chooser),
            (OSC_VOLUME, Widget::Knob),
            (OSC_VOL_VEL, Widget::Knob),
            (OSC_FIXED, Widget::Toggle),
            (OSC_FREQ, Widget::Knob),
            (OSC_MULTI, Widget::Chooser),
            (OSC_COARSE, Widget::Knob),
            (OSC_FINE, Widget::Knob),
        ]),
        envelope_band(None, OSC_ENV, None),
    ]
}

/// The Filter/Global tab's bands that belong to WP-5.14 (R37 to R39); the
/// volume envelope's and the output's follow them.
#[must_use]
pub fn filter_bands() -> Vec<LayoutNode> {
    vec![
        band(&[
            (FILTER_ON, Widget::Toggle),
            (FILTER_TYPE, Widget::Chooser),
            (FILTER_SLOPE, Widget::Chooser),
            (FILTER_CIRCUIT, Widget::Chooser),
            (FILTER_FREQ, Widget::Knob),
            (FILTER_RES, Widget::Knob),
            (FILTER_MORPH, Widget::Knob),
            (FILTER_DRIVE, Widget::Knob),
        ]),
        envelope_band(Some(FENV_AMOUNT), FILTER_ENV, Some(FENV_ON)),
        band(&[
            (SHAPER_ON, Widget::Toggle),
            (SHAPER_TYPE, Widget::Chooser),
            (SHAPER_AMOUNT, Widget::Knob),
            (SHAPER_POST, Widget::Toggle),
        ]),
    ]
}

/// The Modulation tab's bands (R40 to R42).
#[must_use]
pub fn modulation_bands() -> Vec<LayoutNode> {
    let mut aux = vec![(AUX_ON, Widget::Toggle)];
    routes(&mut aux, AUX_DEST_A, AUX_AMOUNT_A);
    // Destination B follows A and Amount B follows Amount A in the numbers,
    // so the two pairs are listed as the tab shows them.
    let mut out = vec![band(&aux), envelope_band(None, AUX_ENV, None)];
    for (n, ids) in LFOS.iter().enumerate() {
        let mut controls = vec![
            (ids.on, Widget::Toggle),
            (ids.wave, Widget::Chooser),
            (ids.rate, Widget::Knob),
            (ids.sync, Widget::Toggle),
            (ids.division, Widget::Chooser),
            (ids.attack, Widget::Knob),
            (ids.retrig, Widget::Toggle),
            (ids.offset, Widget::Knob),
            (ids.key, Widget::Knob),
        ];
        if n == 0 {
            controls.extend((0..4).map(|part| (ids.extra(part), Widget::Knob)));
        } else {
            routes(
                &mut controls,
                ids.extra(LFO_DEST_A),
                ids.extra(LFO_AMOUNT_A),
            );
            controls.push((ids.extra(LFO_PHASE), Widget::Knob));
            controls.push((ids.extra(LFO_SPIN), Widget::Knob));
        }
        out.push(band(&controls));
    }
    out
}

/// The MIDI tab's bands (R43, R44).
#[must_use]
pub fn midi_bands() -> Vec<LayoutNode> {
    let mut out = Vec::new();
    for source in 0..MIDI_SOURCES.len() {
        let mut controls = Vec::new();
        routes(&mut controls, midi_param(source, 0), midi_param(source, 2));
        if source == MIDI_SOURCES.len() - 1 {
            controls.push((BEND_RANGE, Widget::Knob));
        }
        out.push(band(&controls));
    }
    out
}

// ----- Tab indicators ----------------------------------------------------------------------

/// The tab holding the control that destination `dest` moves (R1).
#[must_use]
pub fn dest_tab(dest: usize) -> Option<SamplerTab> {
    Some(match dest {
        dest::NONE => return None,
        dest::SAMPLE_START | dest::LOOP_LENGTH | dest::SELECTOR => SamplerTab::Sample,
        dest::PITCH | dest::OSC_VOLUME | dest::OSC_PITCH | dest::PITCH_ENV => SamplerTab::PitchOsc,
        dest::LFO1_RATE | dest::LFO2_RATE | dest::LFO3_RATE | dest::LFO1_AMOUNT => {
            SamplerTab::Modulation
        }
        _ => SamplerTab::FilterGlobal,
    })
}

/// The five tabs after Zone whose indicator is lit (R1), in the order
/// Sample, Pitch/Osc, Filter/Global, Modulation, MIDI: those with a control
/// that a routing sends to. A routing is a source that is on, with a
/// destination and an amount other than 0; the MIDI sources are always on.
/// `value` reads a parameter.
#[must_use]
pub fn lit_tabs(value: &dyn Fn(ParamId) -> f64) -> [bool; 5] {
    let mut lit = [false; 5];
    let mut mark = |dest_param: ParamId, amount_param: ParamId| {
        let dest = value(dest_param) as usize;
        if value(amount_param) != 0.0
            && let Some(tab) = dest_tab(dest)
        {
            lit[tab_index(tab)] = true;
        }
    };
    let mut sends = |dest_a: ParamId, amount_a: ParamId| {
        mark(dest_a, amount_a);
        mark(ParamId(dest_a.0 + 1), ParamId(amount_a.0 + 1));
    };
    if value(AUX_ON) >= 0.5 {
        sends(AUX_DEST_A, AUX_AMOUNT_A);
    }
    for ids in &LFOS[1..] {
        if value(ids.on) >= 0.5 {
            sends(ids.extra(LFO_DEST_A), ids.extra(LFO_AMOUNT_A));
        }
    }
    for source in 0..MIDI_SOURCES.len() {
        sends(midi_param(source, 0), midi_param(source, 2));
    }
    lit
}

/// Where `tab` sits among the five tabs after Zone.
#[must_use]
pub fn tab_index(tab: SamplerTab) -> usize {
    match tab {
        SamplerTab::Sample => 0,
        SamplerTab::PitchOsc => 1,
        SamplerTab::FilterGlobal => 2,
        SamplerTab::Modulation => 3,
        SamplerTab::Midi => 4,
    }
}
