//! Spectral Resonator (WP-8.48; the Spectral Resonator spec): the partials'
//! law and the decay against the spec's numbers, the pitch modes, MIDI notes
//! on their frames, Quantize, scales and tunings, the modulation modes,
//! Unison, the latency, the spectrogram, and renders that repeat at every
//! block size. Each block runs as a real-time section under the allocation
//! trap.

use std::f64::consts::TAU;

use crate::common;
use bl_core::id::Id;
use bl_core::time::SampleRate;
use bl_devices::spectral_resonator::{
    self, COPIES, DECAY, DRY_WET, FREQ, GLIDE, HARMONICS, HF_DAMP, INPUT_SEND, LF_DAMP, MIDI_GATE,
    MOD_RATE, MODULATION, MONO_POLY, PB, PCH_MOD, PITCH_MODE, POLYPHONY, QUANTIZE, SD_SHIFT, SHIFT,
    SPECTRAL_RESONATOR, STRETCH, TRANSP, UNI_AMT, UNISON, USE_SCALE, VOICES,
    spectral_resonator_type,
};
use bl_engine::scales::ScaleRuns;
use bl_engine::{
    Block, Collector, KeyPitches, Node, NodeConfig, NodeEvent, NoteBuffer, NoteEvent, NoteKind,
    TransportInfo,
};
use bl_model::{DeviceId, ParamId, Scale, ScaleName, Tuning};

/// The block sizes every render must agree at (R13).
const BLOCKS: [usize; 4] = [1, 64, 512, 4_096];

const RATE: SampleRate = SampleRate::HZ_48000;

/// The frame of the analysis at 48 kHz, and so the latency less one.
const LATENCY: usize = 2_047;

/// The frequency of key `key` of twelve equal steps, A3 = 440 Hz at 69.
fn hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

fn semitones_of(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

fn cents(hz: f64, from: f64) -> f64 {
    1200.0 * (hz / from).log2()
}

fn id(n: u64) -> DeviceId {
    DeviceId(Id::new(n).expect("an ID"))
}

/// A render of the device: its node and what it was given.
struct Rig {
    node: Box<dyn Node>,
    collector: Collector,
}

impl Rig {
    fn new(params: &[(ParamId, f64)]) -> Self {
        Self::with_id(1, params)
    }

    fn with_id(n: u64, params: &[(ParamId, f64)]) -> Self {
        let kind = spectral_resonator_type();
        let mut device = kind.device(id(n));
        for (param, value) in params {
            device.params.insert(*param, *value);
        }
        let config = NodeConfig {
            round_trip_latency: 0,
            sample_rate: RATE,
            max_frames: 4_096,
            channels: 2,
        };
        Self {
            node: common::registry()
                .create(&device, &config)
                .expect("the registry makes Spectral Resonator"),
            collector: Collector::new(),
        }
    }

    fn tune(&mut self, tuning: Option<&Tuning>) {
        self.node
            .receive_data(self.collector.handle().data(KeyPitches::of(tuning)));
    }

    /// Runs `input` (both channels) in blocks of `block` frames, with
    /// `events` and `notes` (frames from the start), `scale_at` giving the
    /// scale at a frame and `hidden` for a set whose tuning hides the scales.
    /// Returns each channel.
    fn render(
        &mut self,
        input: &[f32],
        block: usize,
        events: &[(usize, ParamId, f64)],
        notes: &[NoteEvent],
        scale_at: impl Fn(usize) -> Scale,
        hidden: bool,
    ) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::new(); 2];
        for start in (0..input.len()).step_by(block) {
            let end = (start + block).min(input.len());
            let len = end - start;
            let here: Vec<NodeEvent> = events
                .iter()
                .filter(|e| e.0 >= start && e.0 < end)
                .map(|&(at, param, value)| common::set((at - start) as u32, param, value))
                .collect();
            let heard: Vec<NoteEvent> = notes
                .iter()
                .filter(|n| (n.offset as usize) >= start && (n.offset as usize) < end)
                .map(|n| NoteEvent {
                    offset: n.offset - start as u32,
                    ..*n
                })
                .collect();
            let mut runs = ScaleRuns::of(scale_at(start));
            for offset in 1..len {
                runs.push(offset as u32, scale_at(start + offset));
            }
            let mut data: Vec<f32> = (0..2).flat_map(|_| input[start..end].to_vec()).collect();
            let mut sent = NoteBuffer::with_capacity(8);
            {
                let mut block = Block::new(
                    &mut data,
                    2,
                    len,
                    len,
                    &here,
                    TransportInfo::default(),
                    RATE,
                )
                .with_scale(runs)
                .with_scales_hidden(hidden)
                .with_notes(&heard, &mut sent);
                bl_rt::rt_test!({ self.node.process(&mut block) });
            }
            for (channel, values) in out.iter_mut().zip(data.chunks(len)) {
                channel.extend_from_slice(values);
            }
        }
        out
    }

    /// `input` with nothing else.
    fn play(&mut self, input: &[f32], block: usize) -> Vec<Vec<f32>> {
        self.render(input, block, &[], &[], |_| Scale::default(), false)
    }

    /// The left channel's answer to an impulse, `frames` long.
    fn impulse(&mut self, frames: usize) -> Vec<f32> {
        let mut input = vec![0.0f32; frames];
        input[0] = 1.0;
        self.play(&input, 4_096).swap_remove(0)
    }
}

fn on(offset: usize, key: u8) -> NoteEvent {
    NoteEvent {
        offset: offset as u32,
        kind: NoteKind::On,
        channel: 0,
        key,
        velocity: 0.8,
        note_id: None,
    }
}

fn off(offset: usize, key: u8) -> NoteEvent {
    NoteEvent {
        kind: NoteKind::Off,
        ..on(offset, key)
    }
}

fn bend(offset: usize, value: f32) -> NoteEvent {
    NoteEvent {
        kind: NoteKind::PitchBend { value },
        key: 0,
        ..on(offset, 0)
    }
}

/// The amplitude of `signal` at `at` hertz through a Hann window: a sine of
/// amplitude A at `at` reads A.
fn amplitude(signal: &[f32], at: f64) -> f64 {
    let w = TAU * at / RATE.as_f64();
    let n = signal.len() as f64;
    let (mut re, mut im, mut sum) = (0.0, 0.0, 0.0);
    for (i, sample) in signal.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let angle = w * i as f64;
        re += f64::from(*sample) * window * angle.cos();
        im -= f64::from(*sample) * window * angle.sin();
        sum += window;
    }
    2.0 * re.hypot(im) / sum
}

/// An impulse, then silence.
fn impulse(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[0] = 1.0;
    input
}

/// The frequency near `near` (within `span` cents either side) where
/// `signal`'s spectrum peaks, by golden-section search.
fn peak(signal: &[f32], near: f64, span: f64) -> f64 {
    let (mut low, mut high) = (
        near * (-span / 1200.0).exp2(),
        near * (span / 1200.0).exp2(),
    );
    let ratio = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (high - ratio * (high - low), low + ratio * (high - low));
    let (mut fa, mut fb) = (amplitude(signal, a), amplitude(signal, b));
    for _ in 0..40 {
        if fa < fb {
            low = a;
            (a, fa) = (b, fb);
            b = low + ratio * (high - low);
            fb = amplitude(signal, b);
        } else {
            high = b;
            (b, fb) = (a, fa);
            a = high - ratio * (high - low);
            fa = amplitude(signal, a);
        }
    }
    (low + high) / 2.0
}

/// The frequency of a stretch of a clean sine from its positive-going zero
/// crossings, in Hz.
fn crossing_hz(signal: &[f32]) -> f64 {
    let mut crossings = Vec::new();
    for (i, pair) in signal.windows(2).enumerate() {
        if pair[0] < 0.0 && pair[1] >= 0.0 {
            crossings.push(i as f64 + f64::from(-pair[0]) / f64::from(pair[1] - pair[0]));
        }
    }
    assert!(crossings.len() > 2, "a sine in the stretch");
    (crossings.len() - 1) as f64 * RATE.as_f64() / (crossings[crossings.len() - 1] - crossings[0])
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

/// How long the ring of `ir` takes to fall 60 dB, from the slope of its
/// level over windows of `window` frames, between −10 and −40 dB of its
/// loudest window.
fn ring_time(ir: &[f32], window: usize) -> f64 {
    let level: Vec<f64> = ir
        .chunks_exact(window)
        .map(|w| 10.0 * (rms(w).powi(2)).max(1e-30).log10())
        .collect();
    let top = level.iter().copied().fold(f64::MIN, f64::max);
    let peak_at = level.iter().position(|l| *l == top).unwrap();
    let at = |target: f64| {
        peak_at
            + level[peak_at..]
                .iter()
                .position(|l| *l < top + target)
                .unwrap()
    };
    let (a, b) = (at(-10.0), at(-40.0));
    let slope = (level[a] - level[b]) / ((b - a) * window) as f64 * RATE.as_f64();
    60.0 / slope
}

fn sine(at: f64, amplitude: f32, frames: usize) -> Vec<f32> {
    common::sine(at, amplitude, frames)
}

/// One partial, alone: Harmonics 1, so the output is a single sine.
const ONE: [(ParamId, f64); 1] = [(HARMONICS, 1.0)];

fn with(base: &[(ParamId, f64)], more: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut all = base.to_vec();
    all.extend_from_slice(more);
    all
}

fn d_minor() -> Scale {
    Scale::new(2, ScaleName::Minor)
}

fn c_major() -> Scale {
    Scale::new(0, ScaleName::Major)
}

/// 19 equal steps to the octave, with the default reference and range.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

// Spec: spectral-resonator R1, R17. The device is an audio effect with a
// tapped MIDI sidechain, Use Current Scale in the title bar, off in a new
// device, one spectrogram, and banks of eight that hold every parameter.
#[test]
fn the_description_has_a_midi_sidechain_the_switch_in_the_title_bar_and_banks_of_eight() {
    let kind = spectral_resonator_type();
    assert!(kind.midi_sidechain && kind.midi_tap && !kind.sidechain);
    let description = &kind.description;
    description.check().unwrap();
    assert_eq!(description.title_switch, Some(USE_SCALE));
    let switch = description.param(USE_SCALE).unwrap();
    assert_eq!(
        (switch.name.as_str(), switch.default),
        ("Use Current Scale", 0.0)
    );
    assert_eq!(description.spectrogram, Some(spectral_resonator::GRID));
    let banks = description.banks();
    let named: Vec<(&str, usize)> = banks
        .iter()
        .map(|b| (b.name.as_str(), b.params.len()))
        .collect();
    assert_eq!(named[0], ("Main", 8));
    assert_eq!(named[1], ("Damping and Modulation", 8));
    assert_eq!(named[2], ("MIDI", 6));
    let banked: Vec<ParamId> = banks.iter().flat_map(|b| b.params.clone()).collect();
    for param in &description.params {
        assert!(banked.contains(&param.id), "{}", param.name);
    }
    for (param, name) in [(FREQ, "Freq"), (TRANSP, "Transp."), (PCH_MOD, "Pch. Mod")] {
        assert_eq!(description.param(param).unwrap().name, name);
    }
    // Voices come from a pool of 16, Unison's copies are 8 to a partial.
    assert_eq!((VOICES, COPIES), (16, 8));
}

// Spec: spectral-resonator R2. Each partial's resonance is within 5 cents of
// k times the fundamental, from 55 Hz to 1.76 kHz.
#[test]
fn every_partial_rings_within_5_cents_of_its_frequency() {
    for f0 in [55.0, 110.0, 220.0, 440.0, 1_760.0] {
        let mut rig = Rig::new(&[(FREQ, f0), (HARMONICS, 6.0), (DECAY, 3.0)]);
        let ir = rig.impulse(3 * 48_000);
        let ring = &ir[8_000..];
        for k in 1..=6 {
            let expected = f0 * f64::from(k);
            let found = peak(ring, expected, 100.0);
            assert!(
                cents(found, expected).abs() < 5.0,
                "{f0} Hz, partial {k}: {} cents",
                cents(found, expected)
            );
            assert!(amplitude(ring, expected) > 1e-5, "{f0} Hz, partial {k}");
        }
    }
}

// Spec: spectral-resonator R2. Stretch moves the partials by the law
// f_k = f0 * k^(1 + s): squeezed together below 0, apart above, and at +100 %
// only the odd ones sound.
#[test]
fn stretch_moves_the_partials_by_its_law_and_leaves_the_odd_ones_at_100_percent() {
    let f0 = 110.0;
    let ring = |stretch: f64| {
        let mut rig = Rig::new(&[
            (FREQ, f0),
            (HARMONICS, 4.0),
            (DECAY, 3.0),
            (STRETCH, stretch),
        ]);
        rig.impulse(3 * 48_000)[8_000..].to_vec()
    };
    for stretch in [-0.5, 0.5] {
        let signal = ring(stretch * 100.0);
        for k in 1..=4 {
            let expected = f0 * f64::from(k).powf(1.0 + stretch);
            let found = peak(&signal, expected, 100.0);
            assert!(
                cents(found, expected).abs() < 5.0,
                "stretch {stretch}, partial {k}: {} cents",
                cents(found, expected)
            );
        }
    }
    // At −100 % every partial is on the fundamental: none sounds at 2 f0.
    let squeezed = ring(-100.0);
    assert!(amplitude(&squeezed, f0) > 1e-5);
    assert!(amplitude(&squeezed, 2.0 * f0) < 0.02 * amplitude(&squeezed, f0));
    // At +100 % the even partials are gone and the odd ones sit at k^2 f0.
    let odd = ring(100.0);
    let first = amplitude(&odd, f0);
    assert!(first > 1e-5, "partial 1");
    assert!(
        amplitude(&odd, 2.0 * f0) < 0.02 * first,
        "partial 2 is gone"
    );
    assert!(amplitude(&odd, 4.0 * f0) < 0.02 * first, "partial 2 at k^2");
    assert!(amplitude(&odd, 9.0 * f0) > 0.1 * first, "partial 3");
}

// Spec: spectral-resonator R2. Squeezing the partials together doesn't make
// the resonance louder: at Stretch -100 % it is within 9 dB of the level at 0.
#[test]
fn squeezing_the_partials_together_keeps_the_level() {
    let level = |stretch: f64| {
        let mut rig = Rig::new(&[(FREQ, 110.0), (STRETCH, stretch), (DECAY, 1.0)]);
        rms(&rig.play(&common::noise(3 * 48_000), 512)[0][48_000..])
    };
    let (open, squeezed) = (level(0.0), level(-100.0));
    assert!(open > 1e-4);
    let difference = 20.0 * (squeezed / open).log10();
    assert!(difference.abs() < 9.0, "{difference} dB");
}

// Spec: spectral-resonator R2. Harmonics sets how many partials sound.
#[test]
fn harmonics_limits_the_number_of_partials() {
    for count in [1usize, 3, 7] {
        let mut rig = Rig::new(&[(FREQ, 110.0), (HARMONICS, count as f64), (DECAY, 3.0)]);
        let ring = rig.impulse(3 * 48_000)[8_000..].to_vec();
        let first = amplitude(&ring, 110.0);
        assert!(first > 1e-5);
        for k in 2..=count + 1 {
            let level = amplitude(&ring, 110.0 * k as f64);
            if k <= count {
                assert!(level > 0.2 * first, "{count} partials: {k} sounds");
            } else {
                assert!(
                    level < 0.02 * first,
                    "{count} partials: {k} doesn't: {level}"
                );
            }
        }
    }
}

// Spec: spectral-resonator R2. A partial above the Nyquist frequency is
// dropped: it doesn't fold back below it.
#[test]
fn partials_above_nyquist_are_dropped() {
    let mut rig = Rig::new(&[(FREQ, 5_000.0), (HARMONICS, 8.0), (DECAY, 1.0)]);
    let ring = rig.impulse(2 * 48_000)[8_000..].to_vec();
    // 5 kHz and 10, 15, 20 kHz sound; 25 kHz would fold to 23 kHz.
    let first = amplitude(&ring, 5_000.0);
    assert!(first > 1e-6);
    for k in 2..=4 {
        assert!(
            amplitude(&ring, 5_000.0 * f64::from(k)) > 0.2 * first,
            "{k}"
        );
    }
    assert!(
        amplitude(&ring, 23_000.0) < 0.02 * first,
        "nothing folds back"
    );
    assert!(ring.iter().all(|s| s.is_finite()));
}

// Spec: spectral-resonator R3. A partial at 1 kHz rings for Decay to fall 60
// dB, within 10 %; HF Damp and LF Damp shorten it above and below by up to 8
// times at 20 kHz and 20 Hz.
#[test]
fn decay_and_the_damps_set_the_ring_time_within_10_percent() {
    let time = |f0: f64, decay: f64, rest: &[(ParamId, f64)]| {
        let params = with(&ONE, &[(FREQ, f0), (DECAY, decay)]);
        let ir = Rig::new(&with(&params, rest)).impulse(4 * 48_000);
        ring_time(&ir[1_024..], 1_024)
    };
    for decay in [0.5, 1.0, 2.0] {
        let found = time(1_000.0, decay, &[]);
        assert!((found / decay - 1.0).abs() < 0.1, "decay {decay}: {found}");
    }
    // 4 kHz is a factor of ln 4 / ln 20 of the way to 20 kHz.
    let hf = 1.0 + 7.0 * 4f64.ln() / 20f64.ln();
    let found = time(4_000.0, 1.0, &[(HF_DAMP, 100.0)]);
    assert!((found * hf - 1.0).abs() < 0.1, "HF Damp: {found}");
    // 100 Hz is ln 10 / ln 50 of the way to 20 Hz.
    let lf = 1.0 + 7.0 * 10f64.ln() / 50f64.ln();
    let found = time(100.0, 1.0, &[(LF_DAMP, 100.0)]);
    assert!((found * lf - 1.0).abs() < 0.1, "LF Damp: {found}");
    // The damps leave a partial on the other side of 1 kHz alone.
    let found = time(100.0, 1.0, &[(HF_DAMP, 100.0)]);
    assert!((found - 1.0).abs() < 0.1, "HF Damp below 1 kHz: {found}");
}

// Spec: spectral-resonator R4. Shift moves the input's spectrum before it
// reaches the resonances, and the partials stay where they are.
#[test]
fn shift_moves_the_input_spectrum_and_not_the_partials() {
    let input = sine(220.0, 0.5, 2 * 48_000);
    let ring = |shift: f64| {
        Rig::new(&with(&ONE, &[(FREQ, 440.0), (SHIFT, shift), (DECAY, 0.3)]))
            .play(&input, 512)
            .swap_remove(0)[24_000..]
            .to_vec()
    };
    let unshifted = ring(0.0);
    let shifted = ring(12.0);
    assert!(amplitude(&shifted, 440.0) > 0.1, "an octave up reaches 440");
    assert!(amplitude(&unshifted, 440.0) < 0.02, "220 Hz doesn't");
    assert!(amplitude(&shifted, 220.0) < 0.01, "the input isn't heard");
    let found = peak(&shifted, 440.0, 50.0);
    assert!(cents(found, 440.0).abs() < 5.0, "the partial stays");
}

// Spec: spectral-resonator R5. In Internal the fundamental is Freq plus
// Transp.; with Use Current Scale on Freq becomes SD Shift, a degree of the
// scale from its root in the octave of C3, and Transp. counts degrees.
#[test]
fn internal_pitch_is_freq_and_transp_or_a_scale_degree_with_use_current_scale() {
    let ring = |params: &[(ParamId, f64)], scale: Scale| {
        let mut rig = Rig::new(&with(&with(&ONE, &[(DECAY, 3.0)]), params));
        let input = impulse(3 * 48_000);
        rig.render(&input, 512, &[], &[], |_| scale, false)
            .swap_remove(0)[8_000..]
            .to_vec()
    };
    let found = |signal: &[f32], key: f64| cents(peak(signal, hz(key), 100.0), hz(key));
    let plain = ring(&[(FREQ, 220.0), (TRANSP, 12.0)], Scale::default());
    assert!(cents(peak(&plain, 440.0, 100.0), 440.0).abs() < 5.0);
    // D minor, SD Shift 2: the root D3 (key 62) and two degrees up is F3.
    let degrees = ring(
        &[(USE_SCALE, 1.0), (SD_SHIFT, 2.0), (FREQ, 880.0)],
        d_minor(),
    );
    assert!(
        found(&degrees, 65.0).abs() < 5.0,
        "{}",
        found(&degrees, 65.0)
    );
    // Transp. counts degrees too: one more is G3.
    let more = ring(
        &[(USE_SCALE, 1.0), (SD_SHIFT, 2.0), (TRANSP, 1.0)],
        d_minor(),
    );
    assert!(found(&more, 67.0).abs() < 5.0);
    // With no scale the degree is a semitone from the root, C3.
    let chromatic = ring(&[(USE_SCALE, 1.0), (SD_SHIFT, 2.0)], Scale::default());
    assert!(found(&chromatic, 62.0).abs() < 5.0);
}

/// Noise through the MIDI device with `notes`, `events` and `params`.
fn midi(
    params: &[(ParamId, f64)],
    notes: &[NoteEvent],
    events: &[(usize, ParamId, f64)],
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let mut rig = Rig::new(&with(&ONE, &with(params, &[(PITCH_MODE, 1.0)])));
    rig.render(
        &common::noise(frames),
        block,
        events,
        notes,
        |_| Scale::default(),
        false,
    )
    .swap_remove(0)
}

// Spec: spectral-resonator R6. In MIDI the notes of the source tune it on
// their frames, at every block size: Mono's newest held note leads, and a
// note-off hands the pitch back to the one still held.
#[test]
fn notes_tune_the_resonance_on_their_frames_at_every_block_size() {
    let notes = [
        on(4_000, 57),
        on(30_000, 64),
        off(50_000, 64),
        off(60_000, 57),
    ];
    let params = [(MIDI_GATE, 0.0), (DECAY, 5.0)];
    let golden = midi(&params, &notes, &[], 70_000, 512);
    let at = |from: usize, to: usize| crossing_hz(&golden[from..to]);
    // Before the first note nothing sounds.
    assert!(rms(&golden[..4_000]) < 1e-6);
    assert!(cents(at(20_000, 29_990), hz(57.0)).abs() < 5.0);
    // The new note retunes it from its frame: a window of its first 600
    // frames is already at the new pitch, and the one just before isn't.
    assert!(cents(at(30_000, 30_600), hz(64.0)).abs() < 10.0);
    assert!(cents(at(29_300, 29_990), hz(57.0)).abs() < 10.0);
    // The newest note leads while it is held; letting it go hands the pitch
    // back to the older one that still is.
    assert!(cents(at(40_000, 49_990), hz(64.0)).abs() < 5.0);
    assert!(cents(at(50_300, 50_900), hz(57.0)).abs() < 10.0);
    for block in BLOCKS {
        assert_eq!(
            midi(&params, &notes, &[], 70_000, block),
            golden,
            "blocks of {block}"
        );
    }
}

// Spec: spectral-resonator R6. Mono hands the pitch back to a note still
// held when the newest one ends.
#[test]
fn mono_returns_to_the_older_note_that_is_still_held() {
    let notes = [on(2_000, 57), on(20_000, 64), off(40_000, 64)];
    let params = [(MIDI_GATE, 1.0), (DECAY, 5.0)];
    let signal = midi(&params, &notes, &[], 70_000, 512);
    assert!(cents(crossing_hz(&signal[10_000..19_990]), hz(57.0)).abs() < 5.0);
    assert!(cents(crossing_hz(&signal[30_000..39_990]), hz(64.0)).abs() < 5.0);
    assert!(cents(crossing_hz(&signal[50_000..59_990]), hz(57.0)).abs() < 5.0);
}

// Spec: spectral-resonator R6. With MIDI Gate on, the resonances sound only
// while a note is held and ring out after it; with it off they ring at the
// last note.
#[test]
fn midi_gate_makes_the_resonances_sound_only_while_a_note_is_held() {
    let notes = [on(10_000, 57), off(40_000, 57)];
    let gated = midi(&[(MIDI_GATE, 1.0), (DECAY, 0.2)], &notes, &[], 100_000, 512);
    let open = midi(&[(MIDI_GATE, 0.0), (DECAY, 0.2)], &notes, &[], 100_000, 512);
    assert!(rms(&gated[..9_000]) < 1e-6, "silent before the note");
    let held = rms(&gated[20_000..39_000]);
    assert!(held > 1e-3, "it sounds while held: {held}");
    // It rings out after the note-off, over about Decay, and then is gone.
    assert!(rms(&gated[40_500..41_500]) > 0.3 * held, "it rings out");
    assert!(rms(&gated[60_000..100_000]) < 1e-3 * held, "and is gone");
    // With the gate off it rings on at the last note.
    assert!(rms(&open[60_000..100_000]) > 0.5 * held);
    assert!(cents(crossing_hz(&open[70_000..99_000]), hz(57.0)).abs() < 5.0);
}

// Spec: spectral-resonator R6. Glide slides a Mono note to the next over its
// time.
#[test]
fn glide_slides_mono_to_the_next_note() {
    let notes = [on(2_000, 48), on(30_000, 60)];
    let params = [(MIDI_GATE, 0.0), (GLIDE, 1_000.0), (DECAY, 5.0)];
    let signal = midi(&params, &notes, &[], 100_000, 512);
    // Before the second note the first plays; halfway through the glide the
    // pitch is halfway between them; a second after, it is there.
    assert!(cents(crossing_hz(&signal[20_000..29_990]), hz(48.0)).abs() < 5.0);
    let middle = 30_000 + 24_000;
    let found = crossing_hz(&signal[middle - 480..middle + 480]);
    assert!(
        cents(found, hz(54.0)).abs() < 15.0,
        "{}",
        cents(found, hz(54.0))
    );
    assert!(cents(crossing_hz(&signal[85_000..99_000]), hz(60.0)).abs() < 5.0);
}

// Spec: spectral-resonator R6. PB scales the source's pitch bend, which moves
// every voice; with PB at 0 the bend does nothing.
#[test]
fn pb_scales_the_pitch_bend() {
    let notes = [on(2_000, 57), bend(30_000, 0.5)];
    let signal = midi(&[(PB, 4.0), (DECAY, 5.0)], &notes, &[], 60_000, 512);
    assert!(cents(crossing_hz(&signal[20_000..29_990]), hz(57.0)).abs() < 5.0);
    // Half the wheel at a range of 4 semitones is 2 semitones.
    assert!(cents(crossing_hz(&signal[40_000..59_000]), hz(59.0)).abs() < 5.0);
    let flat = midi(&[(PB, 0.0), (DECAY, 5.0)], &notes, &[], 60_000, 512);
    assert!(cents(crossing_hz(&flat[40_000..59_000]), hz(57.0)).abs() < 5.0);
}

// Spec: spectral-resonator R6. Poly gives each held note a voice, up to
// Polyphony; a note past it takes the oldest voice's place.
#[test]
fn poly_rings_every_held_note_and_a_note_past_polyphony_takes_the_oldest_voice() {
    let keys: [u8; 9] = [48, 52, 55, 60, 64, 67, 72, 76, 79];
    let notes: Vec<NoteEvent> = keys
        .iter()
        .enumerate()
        .map(|(n, &key)| on(2_000 + 1_000 * n, key))
        .collect();
    let rig = |polyphony: f64, held: usize| {
        let mut rig = Rig::new(&with(
            &ONE,
            &[
                (PITCH_MODE, 1.0),
                (MONO_POLY, 1.0),
                (POLYPHONY, polyphony),
                (DECAY, 5.0),
            ],
        ));
        let out = rig
            .render(
                &common::noise(80_000),
                512,
                &[],
                &notes[..held],
                |_| Scale::default(),
                false,
            )
            .swap_remove(0);
        keys[..held]
            .iter()
            .map(|&key| amplitude(&out[50_000..], hz(f64::from(key))))
            .collect::<Vec<f64>>()
    };
    // Eight notes, eight voices: every one rings.
    let eight = rig(8.0, 8);
    assert!(eight.iter().all(|&a| a > 1e-3), "{eight:?}");
    // The ninth takes the first voice: the first note's pitch is gone and
    // the others and the new one ring.
    let nine = rig(8.0, 9);
    assert!(nine[0] < 1e-4, "the oldest is stolen: {nine:?}");
    assert!(nine[1..].iter().all(|&a| a > 1e-3), "{nine:?}");
    // With Polyphony at its least, two voices: the newest two ring.
    let two = rig(2.0, 5);
    assert!(two[..3].iter().all(|&a| a < 1e-4), "{two:?}");
    assert!(two[3..].iter().all(|&a| a > 1e-3), "{two:?}");
}

// Spec: spectral-resonator R6. In Poly a released note's voice rings out and
// is the first to be given to a new note.
#[test]
fn a_released_voice_rings_out_and_is_taken_before_a_held_one() {
    let notes = [
        on(2_000, 48),
        on(3_000, 60),
        off(20_000, 60),
        on(40_000, 72),
    ];
    let mut rig = Rig::new(&with(
        &ONE,
        &[
            (PITCH_MODE, 1.0),
            (MONO_POLY, 1.0),
            (POLYPHONY, 2.0),
            (DECAY, 5.0),
        ],
    ));
    let out = rig
        .render(
            &common::noise(80_000),
            512,
            &[],
            &notes,
            |_| Scale::default(),
            false,
        )
        .swap_remove(0);
    let rings = |key: f64| amplitude(&out[60_000..], hz(key));
    assert!(rings(48.0) > 1e-3, "the held note keeps its voice");
    assert!(rings(72.0) > 1e-3, "the new one takes the released voice");
    assert!(rings(60.0) < 1e-4, "the released note's voice was taken");
}

// Spec: spectral-resonator R7. With Quantize on, each partial moves to the
// nearest note of the current scale, or to the nearest semitone with none.
#[test]
fn quantize_moves_partials_to_the_nearest_note_of_the_scale_or_the_semitone() {
    // 61.4 is nearest key 61, and nearest C major's D (62) of the keys it
    // holds.
    let f0 = hz(61.4);
    let ring = |quantize: f64, scale: Scale| {
        let mut rig = Rig::new(&with(
            &ONE,
            &[(FREQ, f0), (QUANTIZE, quantize), (DECAY, 3.0)],
        ));
        rig.render(&impulse(3 * 48_000), 512, &[], &[], |_| scale, false)
            .swap_remove(0)[8_000..]
            .to_vec()
    };
    let near = |signal: &[f32], key: f64| cents(peak(signal, hz(key), 80.0), hz(key)).abs();
    assert!(near(&ring(0.0, c_major()), 61.4) < 5.0, "off: as set");
    assert!(near(&ring(1.0, Scale::default()), 61.0) < 5.0, "a semitone");
    assert!(near(&ring(1.0, c_major()), 62.0) < 5.0, "C major's D");
}

// Spec: spectral-resonator R7, R8. Under a tuning, Quantize moves partials to
// the tuning's nearest note, and Use Current Scale, which the tuning hides,
// acts as off.
#[test]
fn under_a_19_note_tuning_quantize_and_the_notes_follow_the_tuning() {
    let tuning = nineteen();
    let pitch = |key: u8| tuning.key_pitch(key).expect("the tuning has the key");
    let near = |signal: &[f32], semitones: f64| {
        cents(peak(signal, hz(semitones), 80.0), hz(semitones)).abs()
    };
    // A fundamental between two of the tuning's notes goes to the nearer.
    let (a, b) = (pitch(70), pitch(71));
    let between = a + (b - a) * 0.4;
    let mut rig = Rig::new(&with(
        &ONE,
        &[(FREQ, hz(between)), (QUANTIZE, 1.0), (DECAY, 3.0)],
    ));
    rig.tune(Some(&tuning));
    let quantized = rig
        .render(&impulse(3 * 48_000), 512, &[], &[], |_| c_major(), true)
        .swap_remove(0)[8_000..]
        .to_vec();
    assert!(
        near(&quantized, a) < 5.0,
        "the tuning's note, not the scale's"
    );
    // MIDI notes play the tuning: key 65 is not 65 semitones.
    let mut rig = Rig::new(&with(&ONE, &[(PITCH_MODE, 1.0), (MIDI_GATE, 0.0)]));
    rig.tune(Some(&tuning));
    let played = rig
        .render(
            &impulse(3 * 48_000),
            512,
            &[],
            &[on(2_000, 65)],
            |_| Scale::default(),
            true,
        )
        .swap_remove(0)[8_000..]
        .to_vec();
    assert!(near(&played, pitch(65)) < 5.0);
    assert!(cents(hz(pitch(65)), hz(65.0)).abs() > 10.0, "not 12 equal");
    // Use Current Scale, which a tuning of other than twelve notes disables,
    // acts as off: SD Shift is ignored and Freq sets the pitch.
    let mut rig = Rig::new(&with(
        &ONE,
        &[(USE_SCALE, 1.0), (SD_SHIFT, 5.0), (DECAY, 3.0)],
    ));
    rig.tune(Some(&tuning));
    let hidden = rig
        .render(&impulse(3 * 48_000), 512, &[], &[], |_| d_minor(), true)
        .swap_remove(0)[8_000..]
        .to_vec();
    assert!(
        near(&hidden, semitones_of(110.0)) < 5.0,
        "the scale is hidden"
    );
}

// Spec: spectral-resonator R19. Use Current Scale follows the clip's scale on
// the frame it changes, and a held note keeps its pitch across the change.
#[test]
fn a_scale_change_retunes_from_its_frame_and_a_held_note_keeps_its_pitch() {
    let change = 30_000;
    let changing = |at: usize| {
        if at < change {
            Scale::new(0, ScaleName::Major)
        } else {
            Scale::new(0, ScaleName::Minor)
        }
    };
    let play = |params: &[(ParamId, f64)], block: usize| {
        Rig::new(&with(&ONE, params))
            .render(
                &common::noise(70_000),
                block,
                &[],
                &[on(2_000, 60)],
                changing,
                false,
            )
            .swap_remove(0)
    };
    // Transp. of two degrees from C: E in C major, Eb in C minor.
    let params = [
        (PITCH_MODE, 1.0),
        (MIDI_GATE, 0.0),
        (USE_SCALE, 1.0),
        (TRANSP, 2.0),
        (DECAY, 5.0),
    ];
    let signal = play(&params, 512);
    assert!(cents(crossing_hz(&signal[20_000..29_990]), hz(64.0)).abs() < 5.0);
    assert!(cents(crossing_hz(&signal[change..change + 600]), hz(63.0)).abs() < 10.0);
    for block in BLOCKS {
        assert_eq!(play(&params, block), signal, "blocks of {block}");
    }
    // With no transposition the held note stays where it was struck.
    let kept = play(
        &[
            (PITCH_MODE, 1.0),
            (MIDI_GATE, 0.0),
            (USE_SCALE, 1.0),
            (DECAY, 5.0),
        ],
        512,
    );
    assert!(cents(crossing_hz(&kept[20_000..29_990]), hz(60.0)).abs() < 5.0);
    assert!(cents(crossing_hz(&kept[40_000..69_000]), hz(60.0)).abs() < 5.0);
}

/// The frequency of a sine input of 440 Hz through one partial at 440 Hz,
/// in semitones from 440, in windows of 480 frames.
fn deviations(params: &[(ParamId, f64)], device: u64, seconds: usize) -> Vec<f64> {
    let mut rig = Rig::with_id(
        device,
        &with(
            &ONE,
            &with(params, &[(FREQ, 440.0), (DECAY, 20.0), (MOD_RATE, 2.0)]),
        ),
    );
    let out = rig
        .play(&sine(440.0, 0.5, seconds * 48_000), 512)
        .swap_remove(0);
    out[24_000..]
        .as_chunks::<480>()
        .0
        .iter()
        .map(|w| 12.0 * (crossing_hz(w) / 440.0).log2())
        .collect()
}

// Spec: spectral-resonator R9. Chorus moves each partial's pitch by a sine of
// Pch. Mod depth at Mod Rate.
#[test]
fn chorus_moves_the_pitch_in_a_sine_of_the_depth_at_the_rate() {
    let moved = deviations(&[(MODULATION, 1.0), (PCH_MOD, 1.0)], 1, 4);
    let (high, low) = (
        moved.iter().copied().fold(f64::MIN, f64::max),
        moved.iter().copied().fold(f64::MAX, f64::min),
    );
    assert!((0.9..=1.1).contains(&high), "{high}");
    assert!((-1.1..=-0.9).contains(&low), "{low}");
    // 2 Hz: it crosses its middle four times a second.
    let crossings = moved.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
    let seconds = moved.len() as f64 * 480.0 / RATE.as_f64();
    let rate = crossings as f64 / 2.0 / seconds;
    assert!((rate - 2.0).abs() < 0.3, "{rate} Hz");
    // With no depth it doesn't move.
    let still = deviations(&[(MODULATION, 1.0), (PCH_MOD, 0.0)], 1, 4);
    assert!(still.iter().all(|d| d.abs() < 0.02));
}

// Spec: spectral-resonator R9. Wander moves each partial along its own random
// line within Pch. Mod, and follows the device's seed.
#[test]
fn wander_follows_a_random_line_within_the_depth_and_the_seed() {
    let params = [(MODULATION, 2.0), (PCH_MOD, 1.0)];
    let first = deviations(&params, 1, 6);
    assert!(first.iter().all(|d| d.abs() <= 1.05), "within the depth");
    let (high, low) = (
        first.iter().copied().fold(f64::MIN, f64::max),
        first.iter().copied().fold(f64::MAX, f64::min),
    );
    assert!(high - low > 0.5, "it wanders: {low} to {high}");
    assert_eq!(first, deviations(&params, 1, 6), "run after run");
    assert_ne!(
        first,
        deviations(&params, 2, 6),
        "another seed, another line"
    );
}

// Spec: spectral-resonator R9. Granular gates each partial on and off at
// random at Mod Rate and scatters its pitch by up to Pch. Mod, on the seed.
#[test]
fn granular_gates_and_scatters_the_partials_on_the_seed() {
    let render = |device: u64| {
        let mut rig = Rig::with_id(
            device,
            &with(
                &ONE,
                &[
                    (FREQ, 440.0),
                    (DECAY, 20.0),
                    (MODULATION, 3.0),
                    (MOD_RATE, 8.0),
                    (PCH_MOD, 1.0),
                ],
            ),
        );
        rig.play(&sine(440.0, 0.5, 5 * 48_000), 512).swap_remove(0)[24_000..].to_vec()
    };
    let signal = render(1);
    let levels: Vec<f64> = signal.as_chunks::<480>().0.iter().map(|w| rms(w)).collect();
    let loudest = levels.iter().copied().fold(0.0, f64::max);
    let quiet = levels.iter().filter(|&&l| l < 0.05 * loudest).count();
    let loud = levels.iter().filter(|&&l| l > 0.5 * loudest).count();
    assert!(quiet > 10 && loud > 10, "gated: {quiet} quiet, {loud} loud");
    assert_eq!(signal, render(1), "run after run");
    assert_ne!(signal, render(2), "another seed");
    // Where a grain sounds its pitch is within Pch. Mod of the partial.
    for window in signal
        .as_chunks::<480>()
        .0
        .iter()
        .filter(|w| rms(*w) > 0.5 * loudest)
    {
        let moved = 12.0 * (crossing_hz(window) / 440.0).log2();
        assert!(moved.abs() < 1.1, "{moved}");
    }
}

// Spec: spectral-resonator R10. Unison duplicates each partial, detuned
// evenly across ±Uni. Amt (up to 50 cents), spread between the channels.
#[test]
fn unison_detunes_its_copies_evenly_and_spreads_them_between_the_channels() {
    let params = with(
        &ONE,
        &[
            (FREQ, 440.0),
            (DECAY, 20.0),
            (UNISON, 3.0),
            (UNI_AMT, 100.0),
        ],
    );
    let out = Rig::new(&params).play(&sine(440.0, 0.5, 3 * 48_000), 512);
    let (left, right) = (&out[0][48_000..], &out[1][48_000..]);
    let low = 440.0 * (-50.0f64 / 1200.0).exp2();
    let high = 440.0 * (50.0f64 / 1200.0).exp2();
    for (found, expected) in [(low, low), (440.0, 440.0), (high, high)] {
        let mixed: Vec<f32> = left.iter().zip(right).map(|(l, r)| l + r).collect();
        let at = peak(&mixed, found, 20.0);
        assert!(cents(at, expected).abs() < 1.0, "{expected}: {at}");
    }
    // The lowest copy is on the left, the highest on the right, the middle
    // on both.
    assert!(amplitude(left, low) > 2.0 * amplitude(right, low));
    assert!(amplitude(right, high) > 2.0 * amplitude(left, high));
    let (l, r) = (amplitude(left, 440.0), amplitude(right, 440.0));
    assert!((l / r - 1.0).abs() < 0.05, "{l} {r}");
    // One copy has none of it: a single peak at the partial.
    let single =
        Rig::new(&with(&params, &[(UNISON, 1.0)])).play(&sine(440.0, 0.5, 3 * 48_000), 512);
    assert!(amplitude(&single[0][48_000..], low) < 0.01);
}

// Spec: spectral-resonator R11. The device reports the STFT's latency and
// delays the dry signal by it: at Dry/Wet 0 the input comes out, late.
#[test]
fn dry_is_delayed_by_the_reported_latency_and_dry_wet_balances_it() {
    let mut rig = Rig::new(&[(DRY_WET, 0.0)]);
    assert_eq!(rig.node.latency() as usize, LATENCY);
    let input = common::noise(10_000);
    let out = rig.play(&input, 512);
    assert!(out[0][..LATENCY].iter().all(|&s| s == 0.0));
    assert_eq!(out[0][LATENCY..], input[..input.len() - LATENCY]);
    // At 100 % none of the dry signal is left.
    let mut wet = Rig::new(&[(DRY_WET, 100.0), (FREQ, 5_000.0), (HARMONICS, 1.0)]);
    let out = wet.play(&input, 512);
    let heard = rms(&out[0][LATENCY..]);
    let dry = rms(&input);
    assert!(heard < 0.3 * dry, "{heard} against {dry}");
    // At 50 % there is both.
    let mut half = Rig::new(&[(DRY_WET, 50.0), (FREQ, 5_000.0), (HARMONICS, 1.0)]);
    let both = half.play(&input, 512);
    assert!(rms(&both[0][LATENCY..]) > 0.4 * dry);
}

// Spec: spectral-resonator R2, R16. Input Send reaches minus infinity: with
// it there, nothing is sent and the resonances are silent.
#[test]
fn input_send_at_minus_infinity_leaves_the_resonances_silent() {
    let loud = Rig::new(&[(INPUT_SEND, 1.0)]).play(&common::noise(48_000), 512);
    assert!(rms(&loud[0]) > 1e-4);
    let silent = Rig::new(&[(INPUT_SEND, 0.0)]).play(&common::noise(48_000), 512);
    assert!(silent[0].iter().all(|&s| s == 0.0));
}

/// What the display shows after `input`, a block at a time.
fn shown(params: &[(ParamId, f64)], input: &[f32], notes: &[NoteEvent]) -> bl_model::DeviceDisplay {
    let kind = spectral_resonator_type();
    let mut device = kind.device(id(1));
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    let config = NodeConfig {
        round_trip_latency: 0,
        sample_rate: RATE,
        max_frames: 512,
        channels: 2,
    };
    let mut node = common::registry().create(&device, &config).unwrap();
    let mut reader = node.display().expect("the device has a display");
    for (n, part) in input.chunks(512).enumerate() {
        let mut data: Vec<f32> = (0..2).flat_map(|_| part.to_vec()).collect();
        let heard: Vec<NoteEvent> = notes
            .iter()
            .filter(|note| note.offset as usize / 512 == n)
            .map(|note| NoteEvent {
                offset: note.offset % 512,
                ..*note
            })
            .collect();
        let mut sent = NoteBuffer::with_capacity(8);
        let mut block = Block::new(
            &mut data,
            2,
            part.len(),
            part.len(),
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(&heard, &mut sent);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    let data = reader.read().values.clone();
    spectral_resonator::display(id(1), &kind.description, Some(&data))
}

// Spec: spectral-resonator R15. The spectrogram draws the wet output over the
// last four seconds, with each sounding voice's partials marked; the display
// reads headless.
#[test]
fn the_spectrogram_shows_the_partials_marked_and_reads_headless() {
    let params = [(FREQ, 220.0), (HARMONICS, 8.0), (DECAY, 3.0)];
    let display = shown(&params, &common::noise(3 * 48_000), &[]);
    assert!(display.live);
    let grid = spectral_resonator::GRID;
    let spectrogram = &display.spectrograms[0];
    assert_eq!(spectrogram.name, "Spectrogram");
    assert_eq!(spectrogram.grid, grid);
    assert_eq!(spectrogram.values.len(), grid.columns * grid.rows);
    // Each partial's row is lit in the newest column: the loudest rows sit at
    // multiples of 220 Hz, within a row.
    let newest = spectrogram.column(grid.columns - 1).unwrap();
    let row_width = (grid.frequencies[1] / grid.frequencies[0]).ln() / (grid.rows - 1) as f32;
    for k in 1..=8 {
        let row = ((220.0 * k as f32 / grid.frequencies[0]).ln() / row_width).round() as usize;
        let near = newest[row.saturating_sub(1)..=(row + 1).min(grid.rows - 1)]
            .iter()
            .copied()
            .fold(f32::MIN, f32::max);
        assert!(near > grid.levels[0] + 20.0, "partial {k}: {near} dB");
    }
    // Between them it is darker: row of 330 Hz against the one of 220 and 440.
    let between = ((330.0 / grid.frequencies[0]).ln() / row_width).round() as usize;
    assert!(
        newest[between] < newest[((220.0 / grid.frequencies[0]).ln() / row_width).round() as usize]
    );
    // The partials are marked on their series, at their frequencies.
    assert_eq!(display.markers.len(), 8);
    for (k, marker) in display.markers.iter().enumerate() {
        assert_eq!(marker.series, "Partials");
        assert!(
            (marker.x - 220.0 * (k + 1) as f32).abs() < 0.5,
            "{}",
            marker.x
        );
    }
    assert!(display.series("Partials").is_some());
    // The voice's fundamental is read back, and the lamp is off.
    let voice = display.reading("Voice 1 Fundamental").unwrap();
    assert!((voice.value - 220.0).abs() < 0.5, "{}", voice.value);
    assert_eq!(display.reading("Limiter").unwrap().value, 0.0);
    // Before the engine publishes there is nothing to draw.
    let idle = spectral_resonator::display(id(1), &spectral_resonator_type().description, None);
    assert!(!idle.live && idle.spectrograms.is_empty() && idle.markers.is_empty());
}

// Spec: spectral-resonator R14, R15. Each sounding voice's fundamental is
// read, and in MIDI mode a voice appears with its note and goes with its
// ring.
#[test]
fn the_display_lists_the_sounding_voices_fundamentals() {
    let params = [
        (PITCH_MODE, 1.0),
        (MONO_POLY, 1.0),
        (HARMONICS, 2.0),
        (DECAY, 0.2),
    ];
    let notes = [on(2_000, 57), on(3_000, 64)];
    let display = shown(&params, &common::noise(30_000), &notes);
    let mut found: Vec<f32> = display
        .readings
        .iter()
        .filter(|r| r.name.ends_with("Fundamental"))
        .map(|r| r.value)
        .collect();
    found.sort_by(f32::total_cmp);
    assert_eq!(found.len(), 2, "{:?}", display.readings);
    assert!((f64::from(found[0]) - hz(57.0)).abs() < 0.5);
    assert!((f64::from(found[1]) - hz(64.0)).abs() < 0.5);
    assert_eq!(display.markers.len(), 4, "two partials of each voice");
    // With no note nothing is marked.
    let silent = shown(&params, &common::noise(30_000), &[]);
    assert!(silent.markers.is_empty() && silent.readings.len() == 1);
}

// Spec: spectral-resonator R2. The limiter keeps the input below 0 dBFS and
// its lamp lights while it acts.
#[test]
fn the_limiters_lamp_lights_while_it_acts() {
    let loud: Vec<f32> = sine(300.0, 0.9, 20_000);
    let lit = shown(&[(INPUT_SEND, 4.0)], &loud, &[]);
    assert_eq!(lit.reading("Limiter").unwrap().value, 1.0);
    let quiet = shown(&[(INPUT_SEND, 1.0)], &loud, &[]);
    assert_eq!(quiet.reading("Limiter").unwrap().value, 0.0);
}

/// A scenario that moves every kind of parameter, with MIDI notes in both
/// voice modes: noise through a device whose settings change along the way.
fn scenario(block: usize) -> Vec<Vec<f32>> {
    let params = [
        (HARMONICS, 12.0),
        (UNISON, 3.0),
        (DECAY, 0.4),
        (PITCH_MODE, 1.0),
        (MONO_POLY, 1.0),
        (USE_SCALE, 1.0),
        (QUANTIZE, 1.0),
    ];
    let events: Vec<(usize, ParamId, f64)> = vec![
        (3_000, MODULATION, 1.0),
        (7_000, STRETCH, 40.0),
        (9_001, MODULATION, 2.0),
        (12_000, HF_DAMP, 60.0),
        (14_000, MODULATION, 3.0),
        (16_000, SHIFT, 7.0),
        (18_000, MONO_POLY, 0.0),
        (19_000, GLIDE, 300.0),
        (21_000, MIDI_GATE, 0.0),
        (23_000, PCH_MOD, 1.5),
        (25_000, PITCH_MODE, 0.0),
        (26_000, FREQ, 330.0),
        (28_000, STRETCH, -50.0),
        (30_000, UNI_AMT, 80.0),
    ];
    let notes = [
        on(1_000, 48),
        on(1_500, 55),
        on(5_001, 60),
        off(9_000, 48),
        bend(10_000, 0.4),
        on(11_000, 64),
        off(15_000, 55),
        on(17_000, 67),
        off(20_000, 67),
        on(22_000, 52),
        off(24_000, 52),
    ];
    let scale = |at: usize| {
        if at < 15_000 { d_minor() } else { c_major() }
    };
    Rig::with_id(7, &params).render(&common::noise(34_000), block, &events, &notes, scale, false)
}

// Spec: spectral-resonator R12, R13. A render is the same at every block
// size and from run to run, with every modulation mode and MIDI notes.
#[test]
fn renders_are_bit_identical_at_every_block_size_and_run_after_run() {
    let golden = scenario(512);
    assert!(golden[0].iter().any(|s| s.abs() > 1e-3), "it sounds");
    assert!(golden.iter().flatten().all(|s| s.is_finite()));
    for block in BLOCKS {
        assert_eq!(scenario(block), golden, "blocks of {block}");
    }
    assert_eq!(scenario(512), golden, "run after run");
}

// Spec: spectral-resonator R12. Sixteen voices at eight copies of 64
// partials stay finite and bounded under the allocation trap.
#[test]
fn every_voice_every_copy_and_every_partial_at_once_stay_bounded() {
    let mut rig = Rig::new(&with(
        &[],
        &[
            (PITCH_MODE, 1.0),
            (MONO_POLY, 1.0),
            (POLYPHONY, 16.0),
            (HARMONICS, 64.0),
            (UNISON, 8.0),
            (MODULATION, 2.0),
            (DECAY, 20.0),
            (UNI_AMT, 100.0),
        ],
    ));
    let notes: Vec<NoteEvent> = (0..16)
        .map(|n| on(100 + 10 * n, 40 + 3 * n as u8))
        .collect();
    let out = rig.render(
        &common::noise(20_000),
        512,
        &[],
        &notes,
        |_| Scale::default(),
        false,
    );
    assert!(
        out.iter()
            .flatten()
            .all(|s| s.is_finite() && s.abs() < 64.0)
    );
}

/// A song of a MIDI track playing `key` from beat 1 (frame 24,000 at 120
/// BPM) and an audio track of a 440 Hz tone into Spectral Resonator, whose
/// MIDI source is that track; the audio track's left channel for `frames`
/// frames in blocks of `block`.
fn song(block: usize, key: Option<u8>, frames: usize) -> Vec<f32> {
    use bl_engine::nodes::{TEST_TONE, test_tone};
    use bl_model::{Command, Device, DeviceKind, TrackKind};
    let mut chain = common::chain::Chain::new(block.max(64));
    if let Some(key) = key {
        chain.clip(4, [common::chain::note(key, 1, 2)]);
    }
    let audio = chain.editor.new_id();
    chain
        .editor
        .apply(Command::CreateTrack {
            id: audio,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 1,
        })
        .unwrap();
    let make = |chain: &mut common::chain::Chain, device_type: &str, params: &[(ParamId, f64)]| {
        let id = chain.editor.new_id();
        let mut device = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        );
        for &(param, value) in params {
            device.params.insert(param, value);
        }
        (id, device)
    };
    let (_, tone) = make(&mut chain, TEST_TONE, &[(test_tone::LEVEL, -6.0)]);
    let (resonator, device) = make(
        &mut chain,
        SPECTRAL_RESONATOR,
        &[(PITCH_MODE, 1.0), (HARMONICS, 1.0), (DECAY, 5.0)],
    );
    let device = Device {
        midi_sidechain: spectral_resonator_type().device(resonator).midi_sidechain,
        ..device
    };
    for (index, device) in [tone, device].into_iter().enumerate() {
        chain
            .editor
            .apply(Command::InsertDevice {
                track: audio,
                index,
                device,
            })
            .unwrap();
    }
    chain
        .editor
        .apply(Command::SetMidiSidechainSource {
            device: resonator,
            track: Some(chain.track),
            tap: None,
        })
        .unwrap();
    chain.sync();
    chain.play();
    let mut out = Vec::new();
    while out.len() < frames {
        let n = block.min(frames - out.len());
        let processor = &mut chain.processor;
        bl_rt::rt_test!({ processor.process(n) });
        out.extend_from_slice(chain.processor.output().channel(0));
    }
    out
}

// Spec: routing R32; spectral-resonator R6. A MIDI track's note tunes the
// resonance through the device's sidechain on its frame, at every block size.
#[test]
fn a_midi_tracks_note_tunes_spectral_resonator_through_its_sidechain_on_its_frame() {
    let frames = 60_000;
    let mut reference: Option<Vec<f32>> = None;
    for block in [1, 64, 512] {
        let out = song(block, Some(69), frames);
        // Silent until the note, which starts it on its own frame.
        let first = out.iter().position(|&s| s != 0.0).expect("the note sounds");
        assert_eq!(first, 24_000, "blocks of {block}: the note's frame");
        assert!(cents(crossing_hz(&out[40_000..frames]), 440.0).abs() < 5.0);
        match &reference {
            None => reference = Some(out),
            Some(reference) => assert_eq!(&out, reference, "blocks of {block}"),
        }
    }
    // Another key, another pitch; with no note, nothing sounds.
    let up = song(512, Some(76), frames);
    assert!(cents(crossing_hz(&up[40_000..frames]), hz(76.0)).abs() < 5.0);
    assert!(song(512, None, frames).iter().all(|&s| s == 0.0));
}

// Spec: spectral-resonator R18. A set with the device saves and reopens with
// every parameter and its MIDI source.
#[test]
fn a_set_with_spectral_resonator_saves_and_reopens_with_every_parameter_and_its_midi_source() {
    use bl_model::{Command, Document, Editor, Tap, TrackKind};
    let mut editor = Editor::new();
    let mut track = |kind| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    };
    let (audio, keys) = (track(TrackKind::Audio), track(TrackKind::Midi));
    let kind = spectral_resonator_type();
    let id = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 0,
            device: kind.device(id),
        })
        .unwrap();
    let mut moved = Vec::new();
    for p in &kind.description.params {
        let value = p.clamp(p.min() + 0.3 * (p.max() - p.min()));
        editor
            .apply(Command::SetDeviceParam {
                device: id,
                param: p.id,
                value,
            })
            .unwrap();
        moved.push((p.id, value));
    }
    editor
        .apply(Command::SetMidiSidechainSource {
            device: id,
            track: Some(keys),
            tap: Some(Tap::PostMixer),
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let reopened: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(&reopened, editor.document());
    let device = reopened.device(id).unwrap();
    for (param, value) in moved {
        assert_eq!(device.params.get(&param), Some(&value), "{param:?}");
    }
    let sidechain = device.midi_sidechain.as_ref().unwrap();
    assert_eq!(
        (sidechain.source, sidechain.tap),
        (Some(keys), Some(Tap::PostMixer))
    );
}
