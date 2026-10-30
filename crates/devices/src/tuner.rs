//! Tuner, the native pitch detection and tuning device (WP-5.28; the tuner spec):
//! monophonic pitch detection from 41 Hz to 2 kHz within 1 cent, Classic
//! (Target/Strobe) and Histogram views, Hz/cents readout, Reference tuning
//! (410–480 Hz), and note spelling menu options.
//!
//! Audio passes through untouched with zero latency. No allocation, lock or
//! blocking call occurs on the audio thread: all ring buffers and analysis
//! tables are pre-allocated when the node is constructed.

use bl_engine::{DeviceType, KeyPitches, NodeConfig, NodeData, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Tuner's device type.
pub const TUNER: &str = "beatlayer.tuner";

/// Concert pitch reference: 410.0 to 480.0 Hz, default 440.0 Hz.
pub const REFERENCE: ParamId = ParamId(0);
/// Readout chooser: Hz or Cents.
pub const HERTZ_CENTS: ParamId = ParamId(1);
/// Classic mode chooser: Target or Strobe.
pub const CLASSIC_MODE: ParamId = ParamId(2);
/// Auto centring toggle for Histogram view.
pub const AUTO: ParamId = ParamId(3);
/// Note spelling option: Sharps (C#).
pub const SPELLING_SHARPS: ParamId = ParamId(4);
/// Note spelling option: Flats (D♭).
pub const SPELLING_FLATS: ParamId = ParamId(5);
/// Note spelling option: Sharps and Flats (C#/D♭).
pub const SPELLING_BOTH: ParamId = ParamId(6);

/// Readout options.
pub const READOUT_MODES: [&str; 2] = ["Hz", "Cents"];
/// Classic mode options.
pub const CLASSIC_MODES: [&str; 2] = ["Target", "Strobe"];
/// Tuner's display views.
pub const VIEWS: [&str; 2] = ["Classic", "Histogram"];

/// Analysis window size for difference function.
const WINDOW_LEN: usize = 2048;
/// How many new samples arrive before the pitch is measured again: one window,
/// about 43 ms at 48 kHz. The difference function costs more than a block's
/// worth of time at the lowest pitches, and a tuner's needle doesn't need to
/// move faster than this. (Beatlayer's own response time, the tuner spec.)
const ANALYSIS_INTERVAL: usize = 2048;
/// Number of history samples for Histogram view.
pub const HIST_LEN: usize = 32;
/// Where the display data says the note is a key of the set's tuning, not a
/// twelve-step note (the tunings spec, R33): 1 when it is.
const TUNED_AT: usize = 6 + HIST_LEN;
/// Display data length published to the triple buffer:
/// [pitch_hz, midi_note (or the tuning's key), cents, in_tune, valid,
/// strobe_phase, ..history, tuned].
pub const TUNER_DISPLAY_LEN: usize = 7 + HIST_LEN;

/// Returns the descriptor and layout for Tuner.
#[must_use]
pub fn tuner_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(
            REFERENCE,
            "Reference",
            (410.0, 480.0),
            440.0,
            ParamUnit::Hertz,
        )
        .describe("Tuning reference frequency, 440 Hz by default."),
        ParamDescriptor::choice(HERTZ_CENTS, "Readout", &READOUT_MODES, 1)
            .describe("Displays frequency in Hertz or distance in cents."),
        ParamDescriptor::choice(CLASSIC_MODE, "Classic Mode", &CLASSIC_MODES, 0)
            .describe("Classic tuner display mode: Target or Strobe."),
        ParamDescriptor::toggle(AUTO, "Auto", true)
            .describe("Automatically centres the incoming pitch in Histogram view."),
        ParamDescriptor::toggle(SPELLING_SHARPS, "Sharps (C#)", true)
            .describe("Spell chromatic notes with sharps."),
        ParamDescriptor::toggle(SPELLING_FLATS, "Flats (D♭)", false)
            .describe("Spell chromatic notes with flats."),
        ParamDescriptor::toggle(SPELLING_BOTH, "Sharps and Flats (C#/D♭)", false)
            .describe("Spell chromatic notes with both sharps and flats."),
    ];

    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (REFERENCE, Widget::Knob),
                (HERTZ_CENTS, Widget::Chooser),
                (CLASSIC_MODE, Widget::Chooser),
                (AUTO, Widget::Toggle),
            ],
        )),
        LayoutNode::Cell(DisplayCell::new("Tuner", 200.0, BODY_HEIGHT, 1.0)),
    ]));

    DeviceType::native(TUNER, "Tuner", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_views(&["Classic", "Histogram"])
            .with_menu_options(&[SPELLING_SHARPS, SPELLING_FLATS, SPELLING_BOTH])
            .with_banks(&[("Tuner", &["Reference", "Readout", "Classic Mode", "Auto"])]),
    )
}

/// Adds Tuner to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, tuner_type(), |_, config| Tuner::new(config));
}

/// Tuner audio-thread processor.
pub struct Tuner {
    rate: f64,
    ring_len: usize,
    ring_buffer: Vec<f32>,
    ring_pos: usize,
    since_analysis: usize,
    diff_buffer: Vec<f32>,
    cmnd_buffer: Vec<f32>,
    detected_pitch: Option<f64>,
    cents: Option<f64>,
    nearest_note_midi: Option<i32>,
    /// The pitch of each key under the set's tuning, which the compiler sends
    /// to a Tuner on a track that follows it (the tunings spec, R33).
    keys: KeyPitches,
    spelling: Spelling,
    strobe_phase: f32,
    pitch_history: [f32; HIST_LEN],
    history_pos: usize,
    history_count: usize,
    display_data: [f32; TUNER_DISPLAY_LEN],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    Sharps,
    Flats,
    Both,
}

const SHARP_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const FLAT_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];
const BOTH_NAMES: [&str; 12] = [
    "C", "C#/Db", "D", "D#/Eb", "E", "F", "F#/Gb", "G", "G#/Ab", "A", "A#/Bb", "B",
];

impl Tuner {
    /// Constructs a new Tuner processor.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        let max_tau = (config.sample_rate.as_f64() / 38.0).ceil() as usize;
        // The window plus the longest lag it is compared against, so the
        // lowest pitch the tuner reads is a whole period inside the buffer at
        // every rate the engine runs at.
        let ring_len = WINDOW_LEN + max_tau + 2;
        Self {
            rate: config.sample_rate.as_f64(),
            ring_len,
            ring_buffer: vec![0.0f32; ring_len],
            ring_pos: 0,
            since_analysis: 0,
            diff_buffer: vec![0.0f32; max_tau + 4],
            cmnd_buffer: vec![1.0f32; max_tau + 4],
            detected_pitch: None,
            cents: None,
            nearest_note_midi: None,
            keys: KeyPitches::twelve(),
            spelling: Spelling::Sharps,
            strobe_phase: 0.0,
            pitch_history: [0.0; HIST_LEN],
            history_pos: 0,
            history_count: 0,
            display_data: [0.0f32; TUNER_DISPLAY_LEN],
        }
    }

    /// The detected pitch in Hz, if a monophonic tone is present.
    #[must_use]
    pub fn detected_pitch(&self) -> Option<f64> {
        self.detected_pitch
    }

    /// Distance in cents from the nearest semitone (-50.0 to +50.0).
    #[must_use]
    pub fn cents(&self) -> Option<f64> {
        self.cents
    }

    /// The nearest note name according to the active spelling (e.g. "A4", "C#4").
    #[must_use]
    pub fn note_name(&self) -> Option<String> {
        if self.keys.is_tuned() {
            return None;
        }
        let midi = self.nearest_note_midi?;
        let pc = (midi.rem_euclid(12)) as usize;
        let octave = (midi / 12) - 1;
        let name = match self.spelling {
            Spelling::Sharps => SHARP_NAMES[pc],
            Spelling::Flats => FLAT_NAMES[pc],
            Spelling::Both => BOTH_NAMES[pc],
        };
        Some(format!("{name}{octave}"))
    }

    /// The key of the set's tuning nearest the pitch heard, while the Tuner
    /// follows a tuning (the tunings spec, R33).
    #[must_use]
    pub fn tuned_key(&self) -> Option<u8> {
        self.keys
            .is_tuned()
            .then(|| {
                self.nearest_note_midi
                    .and_then(|key| u8::try_from(key).ok())
            })
            .flatten()
    }

    /// Hears `samples` (mono), measuring the pitch once per analysis
    /// interval as the device does. Returns `Some` when a measurement was
    /// made in this call, holding the pitch it found, or none if no tone was
    /// heard; with more than one interval in `samples`, the last. For the CV
    /// Instrument's calibration, which listens with the same detector
    /// (`docs/specs/cv-tools.md`, R14).
    pub fn listen(&mut self, samples: &[f32]) -> Option<Option<f64>> {
        let mut heard = None;
        for &sample in samples {
            self.ring_buffer[self.ring_pos] = sample;
            self.ring_pos = (self.ring_pos + 1) % self.ring_len;
            self.since_analysis += 1;
            if self.since_analysis >= ANALYSIS_INTERVAL {
                self.since_analysis = 0;
                self.detect_pitch(440.0);
                heard = Some(self.detected_pitch);
            }
        }
        heard
    }

    fn detect_pitch(&mut self, reference: f64) {
        let min_tau = (self.rate / 2150.0).floor() as usize;
        let max_tau = (self.rate / 39.0).ceil() as usize;
        let w = WINDOW_LEN;

        // Check signal energy
        let mut energy = 0.0f32;
        for i in 0..w {
            let idx = (self.ring_pos + self.ring_len - w + i) % self.ring_len;
            let s = self.ring_buffer[idx];
            energy += s * s;
        }
        let rms = (energy / w as f32).sqrt();
        if rms < 0.002 {
            self.detected_pitch = None;
            self.cents = None;
            self.nearest_note_midi = None;
            self.update_display_data(None);
            return;
        }

        // Compute difference function d(tau)
        let ring = self.ring_len;
        for tau in (min_tau.saturating_sub(1))..=(max_tau + 1).min(self.diff_buffer.len() - 1) {
            let mut diff_sum = 0.0f32;
            for j in 0..w {
                let back = (self.ring_pos + ring - w - tau + j) % ring;
                let now = (self.ring_pos + ring - w + j) % ring;
                let d = self.ring_buffer[back] - self.ring_buffer[now];
                diff_sum += d * d;
            }
            self.diff_buffer[tau] = diff_sum;
        }

        // Cumulative mean normalized difference
        self.cmnd_buffer[0] = 1.0;
        let mut running_sum = 0.0f32;
        for tau in 1..=max_tau {
            running_sum += self.diff_buffer[tau];
            if running_sum > 1e-12 {
                self.cmnd_buffer[tau] = self.diff_buffer[tau] * (tau as f32) / running_sum;
            } else {
                self.cmnd_buffer[tau] = 1.0;
            }
        }

        // Find first minimum below threshold
        let threshold = 0.15f32;
        let mut best_tau = None;
        for tau in min_tau..max_tau {
            if self.cmnd_buffer[tau] < threshold
                && self.cmnd_buffer[tau] < self.cmnd_buffer[tau - 1]
                && self.cmnd_buffer[tau] <= self.cmnd_buffer[tau + 1]
            {
                best_tau = Some(tau);
                break;
            }
        }

        // Fallback to absolute minimum if none below threshold
        if best_tau.is_none() {
            let mut min_val = f32::MAX;
            for tau in min_tau..max_tau {
                if self.diff_buffer[tau] < self.diff_buffer[tau - 1]
                    && self.diff_buffer[tau] <= self.diff_buffer[tau + 1]
                    && self.diff_buffer[tau] < min_val
                {
                    min_val = self.diff_buffer[tau];
                    best_tau = Some(tau);
                }
            }
        }

        let Some(tau) = best_tau else {
            self.detected_pitch = None;
            self.cents = None;
            self.nearest_note_midi = None;
            self.update_display_data(None);
            return;
        };

        // Parabolic interpolation on the difference function
        let alpha = self.diff_buffer[tau - 1];
        let beta = self.diff_buffer[tau];
        let gamma = self.diff_buffer[tau + 1];
        let denom = 2.0 * (alpha - 2.0 * beta + gamma);
        let delta = if denom.abs() > 1e-12 {
            (alpha - gamma) / denom
        } else {
            0.0
        };

        let period = tau as f64 + delta as f64;
        let f = self.rate / period;

        self.detected_pitch = Some(f);
        // Spec: tunings R33. Under a tuning the notes are the tuning's, placed
        // by its own reference pitch, so the Reference is set aside.
        let (m, c) = if self.keys.is_tuned() {
            match self.keys.nearest(bl_model::pitch_of(f)) {
                Some((key, cents)) => (i32::from(key), cents),
                None => {
                    self.detected_pitch = None;
                    self.cents = None;
                    self.nearest_note_midi = None;
                    self.update_display_data(None);
                    return;
                }
            }
        } else {
            let p = 69.0 + 12.0 * (f / reference).log2();
            let m = p.round() as i32;
            (m, (p - m as f64) * 100.0)
        };
        self.nearest_note_midi = Some(m);
        self.cents = Some(c);

        self.pitch_history[self.history_pos] = c as f32;
        self.history_pos = (self.history_pos + 1) % HIST_LEN;
        self.history_count = (self.history_count + 1).min(HIST_LEN);

        self.update_display_data(Some((f, m, c)));
    }

    fn update_display_data(&mut self, current: Option<(f64, i32, f64)>) {
        if let Some((f, m, c)) = current {
            self.display_data[0] = f as f32;
            self.display_data[1] = m as f32;
            self.display_data[2] = c as f32;
            self.display_data[3] = if c.abs() < 5.0 { 1.0 } else { 0.0 };
            self.display_data[4] = 1.0; // Valid signal
        } else {
            self.display_data[0] = 0.0;
            self.display_data[1] = 0.0;
            self.display_data[2] = 0.0;
            self.display_data[3] = 0.0;
            self.display_data[4] = 0.0;
        }
        self.display_data[5] = self.strobe_phase;
        self.display_data[TUNED_AT] = if self.keys.is_tuned() { 1.0 } else { 0.0 };
        for i in 0..HIST_LEN {
            let idx = if self.history_count < HIST_LEN {
                i
            } else {
                (self.history_pos + i) % HIST_LEN
            };
            self.display_data[6 + i] = self.pitch_history[idx];
        }
    }
}

impl NativeDevice for Tuner {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let reference = params.value(REFERENCE.0 as usize);
        let spelling_flats = params.value(SPELLING_FLATS.0 as usize) >= 0.5;
        let spelling_both = params.value(SPELLING_BOTH.0 as usize) >= 0.5;

        self.spelling = if spelling_both {
            Spelling::Both
        } else if spelling_flats {
            Spelling::Flats
        } else {
            Spelling::Sharps
        };

        let frames = audio.frames();
        let channels = audio.channels();

        // Feed mono mix to ring buffer
        for i in 0..frames {
            let sample = if channels >= 2 {
                let l = audio.channel_mut(0)[i];
                let r = audio.channel_mut(1)[i];
                (l + r) * 0.5
            } else {
                audio.channel_mut(0)[i]
            };
            self.ring_buffer[self.ring_pos] = sample;
            self.ring_pos = (self.ring_pos + 1) % self.ring_len;
        }

        if let Some(c) = self.cents {
            let speed = (c as f32 / 50.0) * (frames as f32 / self.rate as f32) * 5.0;
            self.strobe_phase = (self.strobe_phase + speed).rem_euclid(1.0);
            self.display_data[5] = self.strobe_phase;
        }

        // Spec: tuner R2. The difference function costs more than a block at
        // the lowest pitches, so the pitch is measured once per analysis
        // interval, not once per block; audio is never delayed by it.
        self.since_analysis += frames;
        if self.since_analysis >= ANALYSIS_INTERVAL {
            self.since_analysis = 0;
            self.detect_pitch(reference);
        }
        // Note: audio is unmodified! Bit-identical pass-through.
    }

    fn receive_data(&mut self, data: NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
        }
    }

    fn latency(&self) -> u32 {
        0
    }

    fn display_len(&self) -> usize {
        TUNER_DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.display_data);
    }
}

/// Builds the device display for Tuner from parameters and publication data for a given view.
#[must_use]
pub fn display_for_view(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    view: Option<&str>,
) -> DeviceDisplay {
    display_in(device, description, values, data, view, None)
}

/// As [`display_for_view`], for a Tuner that follows `tuning`: it names the
/// notes it hears as the note editor names them (the tunings spec, R29 and
/// R33).
#[must_use]
pub fn display_in(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    view: Option<&str>,
    tuning: Option<&bl_model::Tuning>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let spelling_flats = value(SPELLING_FLATS) >= 0.5;
    let spelling_both = value(SPELLING_BOTH) >= 0.5;

    // Note spelling priority: Sharps (C#) is the default when neither Flats nor Both
    // is set. In the three-way spelling menu (Sharps, Flats, Both), the spelling selection
    // checks Both first, then Flats, falling back to Sharps if neither is active.
    let spelling = if spelling_both {
        Spelling::Both
    } else if spelling_flats {
        Spelling::Flats
    } else {
        Spelling::Sharps
    };

    let readout_cents = value(HERTZ_CENTS) >= 0.5; // 0 = Hz, 1 = Cents
    let classic_mode = value(CLASSIC_MODE) as usize; // 0 = Target, 1 = Strobe
    let auto_centring = value(AUTO) >= 0.5;
    let active_view = view.unwrap_or("Classic");

    let mut readings = Vec::new();
    let mut markers = Vec::new();
    let mut series = Vec::new();

    if let Some(d) = data {
        let valid = d.get(4).copied().unwrap_or(0.0) >= 0.5;
        if valid {
            let pitch_hz = d[0];
            let midi = d[1] as i32;
            let cents = d[2];
            let strobe_phase = d.get(5).copied().unwrap_or(0.0);

            let pc = (midi.rem_euclid(12)) as usize;
            let octave = (midi / 12) - 1;
            let name = match spelling {
                Spelling::Sharps => SHARP_NAMES[pc],
                Spelling::Flats => FLAT_NAMES[pc],
                Spelling::Both => BOTH_NAMES[pc],
            };
            // Spec: tunings R33. A note of the set's tuning is named as the
            // note editor names it.
            let note_str = if d.get(TUNED_AT).copied().unwrap_or(0.0) >= 0.5 {
                tuning
                    .zip(u8::try_from(midi).ok())
                    .and_then(|(tuning, key)| {
                        tuning.key_note(key).map(|note| tuning.note_name(note))
                    })
                    .unwrap_or_else(|| format!("key {midi}"))
            } else {
                format!("{name}{octave}")
            };

            readings.push(DisplayReading {
                name: "Note".to_owned(),
                value: midi as f32,
                unit: "st".to_owned(),
                text: note_str.clone(),
            });

            if readout_cents {
                readings.push(DisplayReading {
                    name: "Cents".to_owned(),
                    value: cents,
                    unit: "ct".to_owned(),
                    text: format!("{cents:+.1} ct"),
                });
                readings.push(DisplayReading {
                    name: "Pitch".to_owned(),
                    value: pitch_hz,
                    unit: "Hz".to_owned(),
                    text: format!("{pitch_hz:.1} Hz"),
                });
            } else {
                readings.push(DisplayReading {
                    name: "Pitch".to_owned(),
                    value: pitch_hz,
                    unit: "Hz".to_owned(),
                    text: format!("{pitch_hz:.1} Hz"),
                });
                readings.push(DisplayReading {
                    name: "Cents".to_owned(),
                    value: cents,
                    unit: "ct".to_owned(),
                    text: format!("{cents:+.1} ct"),
                });
            }

            let needle_text = if readout_cents {
                format!("{note_str} {cents:+.1} ct")
            } else {
                format!("{note_str} {pitch_hz:.1} Hz")
            };

            if active_view == "Histogram" {
                let history_values: Vec<f32> = if d.len() >= 6 + HIST_LEN {
                    d[6..6 + HIST_LEN].to_vec()
                } else {
                    vec![cents; HIST_LEN]
                };

                let y_range = if auto_centring {
                    [-50.0, 50.0]
                } else {
                    [-100.0, 100.0]
                };

                series.push(DisplaySeries {
                    name: "Histogram".to_owned(),
                    unit: "ct".to_owned(),
                    x_unit: "step".to_owned(),
                    x_range: [0.0, HIST_LEN as f32],
                    log_x: false,
                    y_range,
                    detail: false,
                    values: history_values,
                });

                markers.push(DisplayMarker {
                    name: "Pitch".to_owned(),
                    series: "Histogram".to_owned(),
                    x: HIST_LEN as f32,
                    y: cents,
                    text: needle_text,
                });
            } else if classic_mode == 1 {
                // Strobe Mode
                let strobe_values: Vec<f32> = (0..32)
                    .map(|i| {
                        let phase = i as f32 / 32.0;
                        (phase * 4.0 * std::f32::consts::TAU + strobe_phase * std::f32::consts::TAU)
                            .sin()
                            * 0.5
                            + 0.5
                    })
                    .collect();

                series.push(DisplaySeries {
                    name: "Strobe".to_owned(),
                    unit: "ct".to_owned(),
                    x_unit: "".to_owned(),
                    x_range: [0.0, 32.0],
                    log_x: false,
                    y_range: [0.0, 1.0],
                    detail: false,
                    values: strobe_values,
                });

                let strobe_direction = if cents.abs() < 1.0 {
                    "In Tune"
                } else if cents > 0.0 {
                    "Rotating Right (Sharp)"
                } else {
                    "Rotating Left (Flat)"
                };

                readings.push(DisplayReading {
                    name: "Strobe".to_owned(),
                    value: cents,
                    unit: "ct".to_owned(),
                    text: strobe_direction.to_owned(),
                });

                markers.push(DisplayMarker {
                    name: "Strobe".to_owned(),
                    series: "Strobe".to_owned(),
                    x: strobe_phase * 32.0,
                    y: 0.5,
                    text: format!("{needle_text} ({strobe_direction})"),
                });
            } else {
                // Target Mode
                series.push(DisplaySeries {
                    name: "Tuning".to_owned(),
                    unit: "ct".to_owned(),
                    x_unit: "".to_owned(),
                    x_range: [-50.0, 50.0],
                    log_x: false,
                    y_range: [-1.0, 1.0],
                    detail: false,
                    values: vec![cents],
                });

                markers.push(DisplayMarker {
                    name: "Needle".to_owned(),
                    series: "Tuning".to_owned(),
                    x: cents,
                    y: 0.0,
                    text: needle_text,
                });
            }
        }
    }

    DeviceDisplay {
        device,
        live: data.is_some(),
        view: Some(active_view.to_owned()),
        series,
        markers,
        readings,
    }
}

/// Builds the device display for Tuner from parameters and publication data.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    display_for_view(device, description, values, data, None)
}
