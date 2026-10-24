//! A MIDI clip's per-note pitch through Drift (WP-6.17; the MPE spec, R6 and
//! the polysynth spec, R22): a note with a Pitch envelope of +12 semitones
//! sounds an octave up with Drift's PB Range at its default of 48, and six
//! semitones up at 24, each within a cent: a per-note bend is a share of 48
//! semitones, and the instrument scales it to its own range.

use bl_core::time::{SampleRate, Ticks};
use bl_devices::polysynth::{NOTE_PB, OSC1_GAIN, OSC1_WAVE, PB_RANGE, POLYSYNTH};
use bl_engine::{EngineConfig, Offline, Registry, TransportCommand};
use bl_model::{
    Breakpoint, Clip, ClipData, Command, Device, DeviceKind, Editor, MidiData, Note, TrackKind,
};

const RATE: SampleRate = SampleRate::HZ_48000;

/// Renders a four-beat A4 on Drift with `pb_range`, its Pitch envelope a
/// constant `pitch` semitones, and returns the left channel.
fn render(pb_range: f64, pitch: Option<f64>) -> Vec<f32> {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Drift".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut drift = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: POLYSYNTH.to_owned(),
        },
    );
    drift.params.insert(OSC1_WAVE, 0.0);
    drift.params.insert(OSC1_GAIN, 0.5);
    drift.params.insert(PB_RANGE, pb_range);
    drift.params.insert(NOTE_PB, 1.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: drift,
        })
        .unwrap();
    let mut note = Note::new(69, Ticks::ZERO, Ticks::QUARTER * 4, 100);
    if let Some(value) = pitch {
        note.pitch = vec![Breakpoint {
            at: Ticks::ZERO,
            value,
            bend: 0.0,
        }];
    }
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::QUARTER * 8,
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(72_000, |output| out.extend_from_slice(output.channel(0)));
    out
}

/// The frequency of a steady tone, from 300 to 1,000 Hz: the strongest
/// frequency of a Hann-windowed Fourier sum of `signal`, found at a step of a
/// hertz and then refined to a hundredth of one.
fn frequency(signal: &[f32]) -> f64 {
    let n = signal.len();
    let windowed: Vec<f64> = signal
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            f64::from(x) * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        })
        .collect();
    let power = |hz: f64| {
        let step = std::f64::consts::TAU * hz / RATE.as_f64();
        let (mut re, mut im) = (0.0, 0.0);
        for (i, x) in windowed.iter().enumerate() {
            let phase = step * i as f64;
            re += x * phase.cos();
            im += x * phase.sin();
        }
        re * re + im * im
    };
    let coarse = (300..1_000)
        .map(f64::from)
        .max_by(|&a, &b| power(a).total_cmp(&power(b)))
        .unwrap();
    (-100..=100)
        .map(|k| coarse + f64::from(k) / 100.0)
        .max_by(|&a, &b| power(a).total_cmp(&power(b)))
        .unwrap()
}

fn cents(heard: f64, wanted: f64) -> f64 {
    1200.0 * (heard / wanted).log2()
}

// Spec: mpe R6; polysynth R22.
#[test]
fn a_notes_pitch_of_twelve_semitones_is_an_octave_at_48_and_six_semitones_at_24() {
    // From well after the attack, a second of the tone.
    let heard = |range, pitch| {
        let signal = render(range, pitch);
        assert!(signal.iter().any(|&s| s.abs() > 0.01), "Drift sounds");
        frequency(&signal[12_000..36_000])
    };
    let plain = heard(48.0, None);
    assert!(cents(plain, 440.0).abs() < 1.0, "A is {plain:.3} Hz");
    let octave = heard(48.0, Some(12.0));
    assert!(
        cents(octave, 880.0).abs() < 1.0,
        "+12 at PB Range 48 is {octave:.3} Hz, {:.3} cents off 880",
        cents(octave, 880.0)
    );
    let tritone = heard(24.0, Some(12.0));
    let wanted = 440.0 * 2f64.powf(6.0 / 12.0);
    assert!(
        cents(tritone, wanted).abs() < 1.0,
        "+12 at PB Range 24 is {tritone:.3} Hz, {:.3} cents off {wanted:.3}",
        cents(tritone, wanted)
    );
}
