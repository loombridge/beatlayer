//! CV Clock Out (WP-7.18; the CV tools spec, R21, R22, R30): an audio effect
//! that leaves its audio alone and sends a clock locked to the song on one CV
//! output. A pulse rises on the frame at which each multiple of 1 / Rate of a
//! quarter note is played, holds Level for Width of the time to the next
//! multiple and falls; while the transport is stopped the output is 0 V.
//!
//! What the output is on a frame depends only on the song position, the tempo
//! map and the controls, so it is the same at any block size.

use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::{DeviceType, NoteBuffer, Registry};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::cv::{sample, volts_display};
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// CV Clock Out's type id.
pub const CV_CLOCK_OUT: &str = "beatlayer.cv_clock_out";

/// Rate: pulses to the quarter note.
pub const RATE: ParamId = ParamId(0);
/// Width: how much of a pulse's period it is high, from 1 % to 99 %.
pub const WIDTH: ParamId = ParamId(1);
/// Level: the voltage a pulse is high at, from 0 to 10 V.
pub const LEVEL: ParamId = ParamId(2);

/// The pulses to the quarter note Rate offers, in the order of its chooser
/// (R21), and the names it shows.
pub const RATES: [(&str, f64); 8] = [
    ("1", 1.0),
    ("2", 2.0),
    ("4", 4.0),
    ("8", 8.0),
    ("12", 12.0),
    ("16", 16.0),
    ("24", 24.0),
    ("48", 48.0),
];

/// The index of the default Rate, 4.
pub const DEFAULT_RATE: usize = 2;

/// The output's name in the device's routing.
pub const OUTPUT_NAMES: [&str; 1] = ["Clock"];

/// CV Clock Out's type.
#[must_use]
pub fn cv_clock_out_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::choice(RATE, "Rate", &RATES.map(|(name, _)| name), DEFAULT_RATE)
            .describe("Pulses to the quarter note."),
        ParamDescriptor::continuous(WIDTH, "Width", (1.0, 99.0), 50.0, ParamUnit::Percent)
            .decimals(0)
            .describe("How much of a pulse's period the clock is high."),
        ParamDescriptor::continuous(LEVEL, "Level", (0.0, 10.0), 5.0, ParamUnit::None)
            .decimals(2)
            .describe("The voltage a pulse is high at, in volts."),
    ];
    DeviceType::native(CV_CLOCK_OUT, "CV Clock Out", DeviceRole::AudioEffect)
        .with_cv(&OUTPUT_NAMES, &[])
        .with_description(
            DeviceDescription::new(params).with_banks(&[("Clock", &["Rate", "Width", "Level"])]),
        )
}

/// Adds CV Clock Out to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cv_clock_out_type(), |_, _| CvClockOut::default());
}

/// CV Clock Out's display (R9, R30): the voltage the output last wrote.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    volts_display(device, OUTPUT_NAMES.map(str::to_owned), data)
}

/// One period of the clock: the multiple it starts on, the one after, and
/// where it falls, in song frames.
#[derive(Clone, Copy)]
struct Period {
    per_quarter: u32,
    width: u32,
    start: i64,
    end: i64,
    fall: i64,
}

impl Period {
    /// The period of the clock that song frame `song` falls in, on `tempo`:
    /// from the frame of the last multiple of 1 / `per_quarter` of a quarter
    /// note at or before it to the frame of the next (R22).
    fn of(tempo: &TempoMap, rate: SampleRate, per_quarter: u32, width: u32, song: i64) -> Self {
        let step = TICKS_PER_QUARTER / i64::from(per_quarter);
        let frame_of = |k: i64| tempo.ticks_to_frames(Ticks(k * step), rate).get();
        let mut k = tempo
            .frames_to_ticks(Frames(song), rate)
            .get()
            .div_euclid(step);
        while frame_of(k) > song {
            k -= 1;
        }
        while frame_of(k + 1) <= song {
            k += 1;
        }
        let (start, end) = (frame_of(k), frame_of(k + 1));
        let high = ((end - start) as f64 * f64::from(width) / 100.0).round() as i64;
        Self {
            per_quarter,
            width,
            start,
            end,
            fall: start + high.clamp(1, (end - start - 1).max(1)),
        }
    }
}

#[derive(Default)]
struct CvClockOut {
    /// The song frame the clock started counting from: the first frame it
    /// played, or the first after a jump. A multiple before it never pulses
    /// (R22): a start between multiples waits for the next.
    origin: i64,
    /// The song frame of the last frame, to find a jump by.
    last: i64,
    playing: bool,
    period: Option<Period>,
    shown: f32,
}

impl NativeDevice for CvClockOut {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if !audio.playing() {
            // Spec: cv-tools R22. Stopped, the clock is 0 V.
            self.playing = false;
            self.shown = 0.0;
            if let Some(out) = audio.cv_out(0) {
                out.fill(0.0);
            }
            return;
        }
        let ramp = |id| params.ramp(params.index(id).expect("the description holds it"));
        let (rate, width, level) = (ramp(RATE), ramp(WIDTH), ramp(LEVEL));
        let (tempo, sample_rate) = (audio.tempo(), audio.sample_rate());
        let reach = audio.speed().ceil() as i64 + 1;
        for f in 0..frames {
            let song = audio.song_frame(f);
            // A start, or a jump (a loop's, a locate): the clock begins here.
            if !self.playing || song < self.last || song - self.last > reach {
                self.origin = song;
                self.period = None;
            }
            self.playing = true;
            self.last = song;
            let per_quarter = RATES[(rate[f] as usize).min(RATES.len() - 1)].1 as u32;
            let width = width[f].round() as u32;
            let period = match self.period {
                Some(p)
                    if p.per_quarter == per_quarter
                        && p.width == width
                        && (p.start..p.end).contains(&song) =>
                {
                    p
                }
                _ => Period::of(tempo, sample_rate, per_quarter, width, song),
            };
            self.period = Some(period);
            let high = period.start >= self.origin && song < period.fall;
            self.shown = if high { level[f].clamp(0.0, 10.0) } else { 0.0 };
            if let Some(out) = audio.cv_out(0) {
                out[f] = sample(f64::from(self.shown));
            }
        }
    }

    fn release(&mut self, _: &mut NoteBuffer) {
        // Switched off, removed or moved: the clock begins again.
        self.playing = false;
        self.shown = 0.0;
    }

    fn display_len(&self) -> usize {
        OUTPUT_NAMES.len()
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.shown;
    }
}
