//! The Drum Synths' shared parts (WP-8.24; the drum synths spec, R1 to R10):
//! a device that sounds one hit at a time, each hit a synthesised drum
//! sound that runs its own length whatever the note's length.
//!
//! A note on starts a hit on its frame at its velocity. A hit while one
//! sounds cuts the old one with a short fade ([`CUT_MS`]) while the new one
//! starts, so two hits share the device for that fade only. A choke cuts the
//! hit the same way. Decay is the time the hit's master envelope takes to
//! fall 60 dB; every device multiplies its sound by that envelope, and the
//! hit ends when it falls below -90 dB. The pitch control sets the sound's
//! pitch whatever key plays it. Every random part draws from the device's
//! seed, started again with each hit, so a hit sounds the same every time.
//! Every value that changes within a block is read from the parameter
//! bank's ramps, frame by frame, so a render is the same at any block size.

use std::f64::consts::{LN_10, PI};

use bl_engine::{NodeData, NoteKind};
use bl_model::device_layout::{
    BODY_HEIGHT, BandLabel, Control, ControlBand, DisplayCell, FRAMED_WIDTH, LayoutNode, Widget,
};
use bl_model::{DeviceDescription, DeviceId, DeviceLayout, ParamDescriptor, ParamId, ParamUnit};
use bl_model::{DeviceDisplay, DisplayReading, DisplaySeries, OrdMap};

use crate::ds_clap::DS_CLAP;
use crate::ds_kick::DS_KICK;
use crate::ds_snare::DS_SNARE;
use crate::ds_tom::DS_TOM;
use crate::filters::{Circuit, Coef, Stage};
use crate::framework::{Audio, NativeDevice, ParamBank};

/// The hit's time to fall 60 dB from its peak, as a share of the travel
/// from [`DECAY_MIN_MS`] to [`DECAY_MAX_MS`] (see [`decay_ms`]).
pub const DECAY: ParamId = ParamId(0);
/// The output level, up to 0 dB.
pub const VOLUME: ParamId = ParamId(1);

/// How long a hit that a new hit or a choke cuts takes to fade out, in
/// milliseconds. Whether Live's drum synths cut or overlap is to observe;
/// this is the one place to change it (spec R2).
pub const CUT_MS: f64 = 2.0;

/// The shortest and longest Decay, in milliseconds (spec R4).
pub const DECAY_MIN_MS: f64 = 10.0;
pub const DECAY_MAX_MS: f64 = 4_000.0;

/// The level a hit ends below, in decibels (spec R4).
const END_DB: f64 = -90.0;

/// A preview's velocity, as a share of full (spec R6).
const PREVIEW_VELOCITY: f32 = 100.0 / 127.0;

/// How many points a display's envelope has.
const DISPLAY_POINTS: usize = 160;

/// What `device.preview_hit` sends a Drum Synth's node: play a hit at
/// velocity 100, as a click in the upper half of its display does.
#[derive(Clone, Copy, Debug)]
pub struct PreviewHit;

/// Whether `kind` is one of the Drum Synths.
#[must_use]
pub fn is_drum_synth(kind: &bl_model::DeviceKind) -> bool {
    matches!(kind, bl_model::DeviceKind::Native { device_type }
        if [DS_KICK, DS_SNARE, DS_TOM, DS_CLAP].contains(&device_type.as_str()))
}

/// What Decay's `percent` of travel is, in milliseconds: 10 ms to 4 s on a
/// logarithmic travel, as Live shows Decay in per cent (the drum synths spec,
/// R4).
#[must_use]
pub fn decay_ms(percent: f64) -> f64 {
    DECAY_MIN_MS * (DECAY_MAX_MS / DECAY_MIN_MS).powf(percent / 100.0)
}

/// The Decay `ms` milliseconds is, in per cent: [`decay_ms`]'s inverse.
#[must_use]
pub fn decay_percent(ms: f64) -> f64 {
    100.0 * (ms / DECAY_MIN_MS).ln() / (DECAY_MAX_MS / DECAY_MIN_MS).ln()
}

/// The Decay control: 0 to 100 % of a logarithmic travel from 10 ms to 4 s.
#[must_use]
pub fn decay(default_percent: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(
        DECAY,
        "Decay",
        (0.0, 100.0),
        default_percent,
        ParamUnit::Percent,
    )
    .decimals(1)
    .describe(
        "How long the hit takes to fall 60 dB from its peak, and so how long it sounds: 0 % is \
         10 ms and 100 % is 4 s, on a logarithmic travel. The note's length changes nothing.",
    )
}

/// The Volume control: from silence up to 0 dB (rule G1), so a hit never
/// plays above it.
#[must_use]
pub fn volume() -> ParamDescriptor {
    ParamDescriptor::gain(VOLUME, "Volume", 0.0, -6.0).describe(
        "The device's output level, up to 0 dB; velocity 127 plays a hit at this level and \
         softer notes play it lower. Reaches silence at its bottom.",
    )
}

/// A control from 0 to 100 %.
#[must_use]
pub fn percent(id: ParamId, name: &str, default: f64, what: &str) -> ParamDescriptor {
    ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent).describe(what)
}

/// The semitones a [`tune`] control moves the pitch each way from its middle.
pub const TUNE_SEMITONES: f64 = 24.0;

/// What a pitch control's `percent` is, in semitones: -24 at 0 %, 0 at 50 %
/// and +24 at 100 %.
#[must_use]
pub fn tune_semitones(percent: f64) -> f64 {
    (percent - 50.0) / 50.0 * TUNE_SEMITONES
}

/// The pitch control that Live shows in per cent, 0 to 100 %: 50 % is the
/// sound's own pitch and the ends are two octaves away (spec R5). The key
/// doesn't change it.
#[must_use]
pub fn tune(id: ParamId, name: &str, default_percent: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(id, name, (0.0, 100.0), default_percent, ParamUnit::Percent)
        .decimals(1)
        .describe(
            "Sets the sound's pitch whatever key plays it, from two octaves down at 0 % to two \
             octaves up at 100 %, the sound's own pitch at 50 %: the key chooses a pad, not a \
             pitch.",
        )
}

/// The size of a Drum Synth's display, in points, and the clear space round
/// it (observed in Live 12.4.16, 2 November 2026; layout L-DEV-047).
const CELL_WIDTH: f32 = 137.0;
const CELL_HEIGHT: f32 = 101.0;
const INSET: f32 = 4.0;

/// Where the display's three columns sit, from its left: a large number's
/// left edge and a box's, and where its labels and its rows start.
const LARGE_X: [f32; 3] = [3.5, 48.0, 92.5];
const FRAMED_X: [f32; 3] = [3.0, 47.5, 92.0];
const LABEL_Y: f32 = 58.0;
const ROW_Y: [f32; 2] = [70.0, 85.0];

/// Where the three knobs under the display sit, in the body's band under it.
const KNOB_X: [f32; 3] = [11.0, 55.5, 100.0];
const KNOB_Y: f32 = 2.5;

/// A large number in the display's column `column`.
#[must_use]
pub(crate) fn large(param: ParamId, column: usize) -> Control {
    Control::new(param, Widget::Large, LARGE_X[column], LABEL_Y)
}

/// A box in the display's column `column` and row `row`.
#[must_use]
pub(crate) fn framed(param: ParamId, column: usize, row: usize) -> Control {
    Control::new(param, Widget::Framed, FRAMED_X[column], ROW_Y[row])
}

/// A name over the display's column `column` of boxes.
#[must_use]
pub(crate) fn label(text: &str, column: usize) -> BandLabel {
    BandLabel::new(text, FRAMED_X[column], LABEL_Y, FRAMED_WIDTH)
}

/// The panel every Drum Synth has, as Live draws it: a display with the
/// instrument's drawing and its `cell` controls inside, then three knobs.
#[must_use]
pub(crate) fn panel(
    (cell, labels): (Vec<Control>, Vec<BandLabel>),
    knobs: [ParamId; 3],
) -> DeviceLayout {
    let display = DisplayCell::new(ENVELOPE, CELL_WIDTH, CELL_HEIGHT, 1.0)
        .with_inset(INSET)
        .with_top(ControlBand::free(CELL_WIDTH, CELL_HEIGHT, cell).with_labels(labels));
    let under = BODY_HEIGHT - CELL_HEIGHT - 2.0 * INSET;
    let knobs = knobs
        .into_iter()
        .zip(KNOB_X)
        .map(|(param, x)| Control::new(param, Widget::SmallKnob, x, KNOB_Y))
        .collect();
    DeviceLayout::new(LayoutNode::Column(vec![
        LayoutNode::Cell(display),
        LayoutNode::Band(ControlBand::free(CELL_WIDTH + 2.0 * INSET, under, knobs)),
    ]))
}

/// A factory preset: the values it sets over a new device's, by parameter.
/// Beatlayer's own sounds; how presets load is the browser's to decide.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    pub params: &'static [(ParamId, f64)],
}

/// The name of the display's cell and of its series.
pub const ENVELOPE: &str = "Envelope";

/// One hit's sound, which the device runs.
pub(crate) trait Hit: Send + 'static {
    /// Starts the hit again from its top, in place (nothing allocates).
    /// `seed` starts its noise.
    fn start(&mut self, seed: u64);

    /// One frame of the hit, left and right, before Volume and velocity. A
    /// device multiplies what it makes by `master`, the envelope that falls
    /// 60 dB over Decay; what it doesn't (the clap's bursts) still ends
    /// when [`min_ms`](Self::min_ms) have passed. Values beyond -1 and 1
    /// are cut, so a hit never plays above Volume.
    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32);

    /// How long the hit runs at least, in milliseconds, whatever Decay is.
    fn min_ms(&self) -> f64 {
        0.0
    }
}

/// One hit's place in the device.
struct Slot<V> {
    hit: V,
    active: bool,
    /// The master envelope, from 1 at the hit's start.
    master: f64,
    /// Frames since the hit started.
    age: u32,
    velocity: f32,
    /// A cut hit's fade.
    fade_left: u32,
    fade_total: u32,
    /// The frames the hit runs at least.
    min_frames: u32,
}

/// A Drum Synth: two slots for hits, so a hit that is cut fades out while
/// the new one starts.
pub(crate) struct DrumSynth<V> {
    slots: [Slot<V>; 2],
    /// The slot the newest hit is in.
    newest: usize,
    seed: u64,
    rate: f64,
    preview: bool,
}

impl<V: Hit> DrumSynth<V> {
    /// A device whose hits are `make()`s, with `seed` for its noise.
    pub(crate) fn new(seed: u64, rate: f64, make: impl Fn() -> V) -> Self {
        let slot = || Slot {
            hit: make(),
            active: false,
            master: 1.0,
            age: 0,
            velocity: 0.0,
            fade_left: 0,
            fade_total: 1,
            min_frames: 0,
        };
        Self {
            slots: [slot(), slot()],
            newest: 0,
            seed,
            rate,
            preview: false,
        }
    }

    /// Fades the sounding hit out over [`CUT_MS`].
    fn cut(&mut self) {
        let frames = ((CUT_MS / 1000.0 * self.rate) as u32).max(1);
        let slot = &mut self.slots[self.newest];
        if slot.active && slot.fade_left == 0 {
            slot.fade_left = frames;
            slot.fade_total = frames;
        }
    }

    /// Starts a hit at `velocity`, cutting the sounding one.
    fn trigger(&mut self, velocity: f32) {
        self.cut();
        self.newest ^= 1;
        let rate = self.rate;
        let seed = self.seed;
        let slot = &mut self.slots[self.newest];
        slot.hit.start(seed);
        slot.active = true;
        slot.master = 1.0;
        slot.age = 0;
        slot.velocity = velocity.clamp(0.0, 1.0);
        slot.fade_left = 0;
        slot.min_frames = (slot.hit.min_ms() / 1000.0 * rate).ceil() as u32;
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize) -> (f32, f32) {
        let decay = decay_ms(f64::from(bank.ramp(DECAY.0 as usize)[frame]));
        // The master envelope's step: -60 dB over Decay.
        let step = (-3.0 * LN_10 / (decay.max(DECAY_MIN_MS) / 1000.0 * self.rate)).exp();
        let end = 10f64.powf(END_DB / 20.0);
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for slot in &mut self.slots {
            if !slot.active {
                continue;
            }
            let (l, r) = slot.hit.frame(bank, frame, slot.master as f32);
            let mut level = slot.velocity;
            if slot.fade_left > 0 {
                level *= slot.fade_left as f32 / slot.fade_total as f32;
                slot.fade_left -= 1;
                if slot.fade_left == 0 {
                    slot.active = false;
                }
            }
            left += l.clamp(-1.0, 1.0) * level;
            right += r.clamp(-1.0, 1.0) * level;
            slot.master *= step;
            slot.age = slot.age.saturating_add(1);
            if slot.master < end && slot.age >= slot.min_frames {
                slot.active = false;
            }
        }
        (left, right)
    }
}

impl<V: Hit> NativeDevice for DrumSynth<V> {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        if self.preview {
            self.preview = false;
            self.trigger(PREVIEW_VELOCITY);
        }
        let volume = params.ramp(VOLUME.0 as usize);
        let mut notes = audio.notes().peekable();
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let mut right = channels.next();
        for frame in 0..frames {
            while notes
                .peek()
                .is_some_and(|note| note.offset as usize <= frame)
            {
                let note = notes.next().expect("peeked");
                match note.kind {
                    NoteKind::On => self.trigger(note.velocity),
                    NoteKind::Choke => self.cut(),
                    // The hit runs its own length, and the key sets no pitch.
                    _ => {}
                }
            }
            let (l, r) = self.frame(params, frame);
            left[frame] = l * volume[frame];
            if let Some(right) = &mut right {
                right[frame] = r * volume[frame];
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<PreviewHit>().is_some() {
            self.preview = true;
        }
    }
}

/// A state-variable filter: its coefficients and state.
#[derive(Clone, Copy, Default)]
pub(crate) struct Filter {
    stage: Stage,
}

/// What a filter gives of its input.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pass {
    Low,
    High,
    /// A band-pass whose gain at its centre is 1.
    Band,
}

impl Filter {
    pub(crate) fn clear(&mut self) {
        self.stage = Stage::default();
    }

    /// `x` through the filter at `hz` and `q`, at `rate`.
    pub(crate) fn tick(&mut self, x: f64, pass: Pass, hz: f64, q: f64, rate: f64) -> f64 {
        let coef = Coef::new((PI * (hz / rate).clamp(1e-5, 0.49)).tan(), q);
        let [low, band, high] = self.stage.tick(x, &coef, Circuit::Svf);
        match pass {
            Pass::Low => low,
            Pass::High => high,
            Pass::Band => coef.k * band,
        }
    }
}

/// The master envelope at `ms` into a hit of `decay_ms`.
#[must_use]
pub fn master_at(ms: f64, decay_ms: f64) -> f64 {
    10f64.powf(-3.0 * ms / decay_ms.max(DECAY_MIN_MS))
}

/// A Drum Synth's display: its hit's amplitude envelope over the time it
/// sounds, worked out from its parameters (spec R6). `shape` gives the
/// envelope at a time in milliseconds into the hit.
pub(crate) fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    readings: impl Fn(&dyn Fn(ParamId) -> f64) -> Vec<DisplayReading>,
    shape: impl Fn(&dyn Fn(ParamId) -> f64, f64) -> f64,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let length = decay_ms(value(DECAY)) * 1.5;
    let points = (0..DISPLAY_POINTS)
        .map(|n| {
            let ms = length * n as f64 / (DISPLAY_POINTS - 1) as f64;
            shape(&value, ms).clamp(0.0, 1.0) as f32
        })
        .collect();
    DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: false,
        view: None,
        series: vec![DisplaySeries {
            name: ENVELOPE.to_owned(),
            unit: String::new(),
            x_unit: "ms".to_owned(),
            x_range: [0.0, length as f32],
            log_x: false,
            y_range: [0.0, 1.0],
            detail: false,
            values: points,
        }],
        markers: Vec::new(),
        readings: readings(&value),
    }
}

/// A reading of text: the note a pitch is nearest and, for DS Tom, its hertz,
/// which the panel draws in the display and under a large number.
pub(crate) fn reading(name: &str, text: String) -> DisplayReading {
    DisplayReading {
        name: name.to_owned(),
        value: 0.0,
        unit: String::new(),
        text,
    }
}

/// The reading of the note nearest `hz`, in Live's octave numbering.
pub(crate) fn note_reading(hz: f64) -> DisplayReading {
    let note = (69.0 + 12.0 * (hz / 440.0).log2())
        .round()
        .clamp(0.0, 127.0);
    reading("Note", bl_model::note_name(note as u8))
}
