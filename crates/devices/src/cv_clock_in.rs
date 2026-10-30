//! CV Clock In (WP-7.18; the CV tools spec, R23 to R25, R30): an audio
//! effect that leaves its audio alone and lets a clock on its CV input lead
//! the song's tempo. The engine asks it, before any track runs, to follow
//! the pulses of the block ([`NativeDevice::follow_cv`]); the follower is
//! [`CvClock`], which fits the pulses' times as a MIDI clock's are fitted and
//! keeps the song's beat on them. The panel shows the tempo it measures, a
//! light that blinks with each pulse and what leads.

use bl_engine::cv_clock::{CvClock, Cx, Others, Settings};
use bl_engine::{DeviceType, Follow, Registry};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, DisplayReading, ParamDescriptor,
    ParamId, ParamUnit,
};

use crate::cv::VOLTS_PER_SAMPLE;
use crate::cv_clock_out::{DEFAULT_RATE, RATES};
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// CV Clock In's type id.
pub const CV_CLOCK_IN: &str = "beatlayer.cv_clock_in";

/// Sync: whether the device leads the song's tempo.
pub const SYNC: ParamId = ParamId(0);
/// Rate: pulses to the quarter note.
pub const RATE: ParamId = ParamId(1);
/// Threshold: the voltage a rise has to reach to be a pulse.
pub const THRESHOLD: ParamId = ParamId(2);

/// The input's name in the device's routing.
pub const INPUT_NAMES: [&str; 1] = ["Clock"];

/// What the display data holds: the measured tempo (0 for none), whether the
/// device leads, the last pulse's frame as its low 24 bits and the rest (−1
/// for none), whether the light is lit, and who else leads.
const DISPLAY: usize = 6;
const FRAME_BITS: u32 = 24;

/// CV Clock In's type.
#[must_use]
pub fn cv_clock_in_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::toggle(SYNC, "Sync", true)
            .describe("Lets the clock lead the song's tempo."),
        ParamDescriptor::choice(RATE, "Rate", &RATES.map(|(name, _)| name), DEFAULT_RATE)
            .describe("Pulses to the quarter note."),
        ParamDescriptor::continuous(THRESHOLD, "Threshold", (0.0, 10.0), 1.0, ParamUnit::None)
            .decimals(2)
            .describe("The voltage a rising input has to reach to count as a pulse, in volts."),
    ];
    DeviceType::native(CV_CLOCK_IN, "CV Clock In", DeviceRole::AudioEffect)
        .with_cv(&[], &INPUT_NAMES)
        .with_description(
            DeviceDescription::new(params).with_banks(&[("Clock", &["Sync", "Rate", "Threshold"])]),
        )
}

/// Adds CV Clock In to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cv_clock_in_type(), |_, _| CvClockIn::default());
}

/// CV Clock In's display (R24, R30): the tempo it measures, whether it
/// leads, the frame of its last pulse, the pulse light and what leads.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    let get = |at: usize| data.and_then(|data| data.get(at)).copied().unwrap_or(0.0);
    let tempo = get(0);
    let leads = get(1) >= 0.5;
    let frame = data
        .filter(|_| get(2) >= 0.0)
        .map(|_| get(2) as u64 + ((get(3) as u64) << FRAME_BITS));
    let reading = |name: &str, value: f32, unit: &str, text: String| DisplayReading {
        name: name.to_owned(),
        value,
        unit: unit.to_owned(),
        text,
    };
    let who = match get(5) as u8 {
        _ if leads => "This device",
        1 => "A MIDI clock",
        2 => "Another CV Clock In",
        _ => "Nothing",
    };
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: vec![
            reading(
                "Tempo",
                tempo,
                "BPM",
                if tempo > 0.0 {
                    format!("{tempo:.1} BPM")
                } else {
                    "No tempo".to_owned()
                },
            ),
            reading(
                "Leads",
                f32::from(u8::from(leads)),
                "",
                if leads { "Leads" } else { "Follows nothing" }.to_owned(),
            ),
            reading(
                "Last pulse",
                frame.map_or(-1.0, |frame| frame as f32),
                "frames",
                frame.map_or_else(|| "None".to_owned(), |frame| frame.to_string()),
            ),
            reading(
                "Pulse",
                get(4),
                "",
                if get(4) >= 0.5 { "On" } else { "Off" }.to_owned(),
            ),
            reading("Leader", get(5), "", who.to_owned()),
        ],
    }
}

#[derive(Default)]
struct CvClockIn {
    clock: CvClock,
    others: Others,
}

impl NativeDevice for CvClockIn {
    fn process(&mut self, _: &mut Audio<'_, '_>, _: &ParamBank) {}

    fn follow_cv(&mut self, params: &ParamBank, cx: &Cx<'_>, answer: &mut Follow) -> Option<f64> {
        // shortcut: the controls stand as the block began, so an automated
        // or changed Rate or Threshold takes hold at the next block.
        let value = |id| params.value(params.index(id).expect("the description holds it"));
        let settings = Settings {
            sync: value(SYNC) >= 0.5,
            per_quarter: RATES[(value(RATE) as usize).min(RATES.len() - 1)].1,
            threshold: (value(THRESHOLD) / VOLTS_PER_SAMPLE) as f32,
        };
        self.others = cx.others;
        self.clock.block(settings, cx, answer)
    }

    fn display_len(&self) -> usize {
        DISPLAY
    }

    fn display(&mut self, out: &mut [f32]) {
        let frame = self.clock.last_pulse();
        out[0] = self.clock.measured().map_or(0.0, |bpm| bpm as f32);
        out[1] = f32::from(u8::from(self.clock.leads()));
        out[2] = frame.map_or(-1.0, |frame| (frame & ((1 << FRAME_BITS) - 1)) as f32);
        out[3] = frame.map_or(-1.0, |frame| (frame >> FRAME_BITS) as f32);
        out[4] = f32::from(u8::from(self.clock.lit()));
        out[5] = match self.others {
            Others::Nobody => 0.0,
            Others::Midi => 1.0,
            Others::Cv => 2.0,
        };
    }
}
