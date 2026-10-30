//! Simpler, the native sampler (WP-2.16; the sampler spec): playing a
//! sample at the right pitch and length, the region and its loop's
//! crossfade, the envelope's times, velocity and pan, glide and
//! portamento, One-Shot, voice stealing, Slicing's slices, the filter's
//! types, key tracking and envelope, the LFO's filter route and its tempo
//! sync, and the sample the engine gets from the set. Every block runs as
//! a real-time section under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::time::{Bpm, Frames, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_devices::sampler::{
    ATTACK, DECAY, FADE, FADE_OUT, FILTER, FILTER_ENV_AMOUNT, FILTER_ENV_DECAY, FILTER_ENV_SUSTAIN,
    FILTER_FREQ, FILTER_KEY, FILTER_TYPE, GAIN, GLIDE, GLIDE_MODE, GLIDE_TIME, LENGTH, LFO,
    LFO_DIVISION, LFO_FILTER, LFO_RATE, LFO_SYNC, LFO_VOLUME, LOOP, LOOP_LENGTH, MODE, PAN,
    PITCH_ENV_AMOUNT, PITCH_ENV_ATTACK, PITCH_ENV_DECAY, PITCH_ENV_RELEASE, PITCH_ENV_SUSTAIN,
    PLAYBACK, RELEASE, RETRIG, SAMPLER, SLICE_BY, SLICE_DIVISION, SLICE_REGIONS, SLICE_SENSITIVITY,
    START, SUSTAIN, TRIGGER, VELOCITY, VOICES, VOLUME, sampler_type,
};
use bl_engine::{
    Block, Collector, DeviceType, EngineConfig, MediaMap, Node, NodeEvent, NoteBuffer, NoteEvent,
    NoteKind, Offline, ParamValue, Registry, SlotSample, TransportCommand, TransportInfo,
};
use bl_model::param::db_to_gain;
use bl_model::{
    AudioData, Clip, ClipData, Command, Device, DeviceKind, DeviceRole, DeviceWarp, Editor,
    MidiData, Note, ParamId, TrackId, TrackKind, WarpMarker, WarpMode, WarpSettings,
};
use common::{CHANNELS, RATE, device, node};

/// A registry with the sampler alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::sampler::register(&mut registry);
    registry
}

/// A sampler of `params`, up to `max_frames` at a time.
fn sampler(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = sampler_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// Decoded audio of `frames` frames at the engine's rate, one channel.
fn sample(frames: usize, value: impl Fn(usize) -> f32) -> Arc<bl_media::SampleData> {
    Arc::new(bl_media::SampleData::new(RATE.hz(), vec![(0..frames).map(value).collect()]).unwrap())
}

/// Gives `node` the sample, through the collector the engine would.
fn give(node: &mut dyn Node, collector: &Collector, data: &Arc<bl_media::SampleData>) {
    node.receive_data(
        collector
            .handle()
            .data(SlotSample::new(0, Arc::clone(data))),
    );
}

/// Gives `node` the sample with its Start and End flags at `region`.
fn give_region(
    node: &mut dyn Node,
    collector: &Collector,
    data: &Arc<bl_media::SampleData>,
    region: (u64, u64),
) {
    let mut slot = SlotSample::new(0, Arc::clone(data));
    slot.region = region;
    node.receive_data(collector.handle().data(slot));
}

fn note(offset: u32, kind: NoteKind, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key,
        velocity,
        note_id: None,
    }
}

/// Runs one block of `frames` frames through `node`, silent at the start,
/// playing `notes`, as a real-time section, and returns each channel.
fn run_notes(node: &mut dyn Node, frames: usize, notes: &[NoteEvent]) -> Vec<Vec<f32>> {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut out = NoteBuffer::with_capacity(notes.len().max(1));
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// The first frame of `samples` where `wanted` holds, or the end.
fn first(samples: &[f32], wanted: impl Fn(&f32) -> bool) -> usize {
    samples.iter().position(wanted).unwrap_or(samples.len())
}

/// The sample rate the tests run at, in hertz.
const HZ: f64 = 48_000.0;

/// Runs one block of `frames` frames through `node` at `tempo`, silent at
/// the start, playing `notes`, as a real-time section, and returns each
/// channel.
fn run_notes_at(
    node: &mut dyn Node,
    frames: usize,
    notes: &[NoteEvent],
    tempo: &TempoMap,
) -> Vec<Vec<f32>> {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut out = NoteBuffer::with_capacity(notes.len().max(1));
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out)
        .with_tempo(tempo);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// The amplitude of `hz` in `signal`, by a windowed DFT.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n).cos();
        let phase = core::f64::consts::TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
    }
    2.0 * re.hypot(im) / n
}

/// A gain in decibels.
fn db(gain: f64) -> f64 {
    20.0 * gain.log10()
}

// Spec: sampler R1
#[test]
fn the_sampler_is_described_and_laid_out() {
    let device_type = sampler_type();
    assert_eq!(device_type.name, "Simpler");
    assert_eq!(device_type.role, DeviceRole::Instrument);
    // Every parameter is described once and placed in the layout.
    device_type.description.check().unwrap();
    assert!(device_type.description.params.len() >= 20);
    // Spec: sampler R3. S Start, S Length and S Loop Length are the
    // parameters Start, Length and Loop Length were.
    let name = |id: ParamId| {
        device_type
            .description
            .params
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.to_string())
    };
    assert_eq!(name(START).as_deref(), Some("S Start"));
    assert_eq!(name(LENGTH).as_deref(), Some("S Length"));
    assert_eq!(name(LOOP_LENGTH).as_deref(), Some("S Loop Length"));
}

// Spec: sampler R2
#[test]
fn a_note_plays_the_sample_at_its_own_pitch_and_an_octave_up_plays_it_twice_as_fast() {
    // A one-frame impulse at sample frame 1000.
    let impulse = sample(4800, |i| if i == 1000 { 1.0 } else { 0.0 });
    let collector = Collector::new();

    let mut at_c3 = sampler(&[], 4800);
    give(at_c3.as_mut(), &collector, &impulse);
    let out = run_notes(at_c3.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    assert_eq!(first(&out[0], |s| s.abs() > 0.9), 1000, "C3 plays at 1x");

    let mut at_c4 = sampler(&[], 4800);
    give(at_c4.as_mut(), &collector, &impulse);
    let out = run_notes(at_c4.as_mut(), 4800, &[note(0, NoteKind::On, 72, 1.0)]);
    assert_eq!(first(&out[0], |s| s.abs() > 0.9), 500, "an octave up, 2x");
}

// Spec: sampler R2
#[test]
fn a_sampler_without_a_sample_is_silent() {
    let mut node = sampler(&[], 512);
    let out = run_notes(node.as_mut(), 512, &[note(0, NoteKind::On, 60, 1.0)]);
    assert!(out.iter().flatten().all(|s| *s == 0.0));
}

// Spec: sampler R3
#[test]
fn the_region_plays_from_its_start_for_its_length() {
    let whole = sample(4800, |_| 1.0);
    let collector = Collector::new();
    let mut node = sampler(&[(START, 25.0), (LENGTH, 25.0)], 4800);
    give(node.as_mut(), &collector, &whole);
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    // 25 % of 4800 is 1200: the voice plays sample frames 1200 to 2400,
    // which are 1.0, then ends.
    assert!(out[0][..1200].iter().all(|s| (s - 1.0).abs() < 1e-6));
    assert!(out[0][1200..].iter().all(|s| *s == 0.0));
}

// Spec: sampler R3, R22
#[test]
fn classic_plays_s_start_and_s_length_inside_the_flags_and_loops_the_end_of_that_part() {
    // A ramp of one frame per step, the flags at a quarter and three
    // quarters: the region is frames 1200 to 3600.
    let ramp = sample(4800, |i| i as f32 / 4800.0);
    let collector = Collector::new();
    let wanted = |from: usize, i: usize| (from + i) as f32 / 4800.0;

    // S Start 50 % and S Length 25 % of the region: its third quarter,
    // frames 2400 to 3000.
    let mut node = sampler(&[(START, 50.0), (LENGTH, 25.0)], 4800);
    give_region(node.as_mut(), &collector, &ramp, (1200, 3600));
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    for (i, played) in out[0][..600].iter().enumerate() {
        assert!((played - wanted(2400, i)).abs() < 1e-6, "frame {i}");
    }
    assert!(out[0][600..].iter().all(|s| *s == 0.0), "then it ends");

    // With the loop on, S Loop Length 50 % repeats the part's last half,
    // frames 2700 to 3000.
    let mut node = sampler(
        &[
            (START, 50.0),
            (LENGTH, 25.0),
            (LOOP, 1.0),
            (LOOP_LENGTH, 50.0),
        ],
        4800,
    );
    give_region(node.as_mut(), &collector, &ramp, (1200, 3600));
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    for i in [0, 300, 599] {
        assert!((out[0][i] - wanted(2400, i)).abs() < 1e-6, "frame {i}");
    }
    for i in [600, 750, 899, 900, 1500, 4799] {
        let looped = 2700 + (i - 600) % 300;
        assert!((out[0][i] - wanted(looped, 0)).abs() < 1e-6, "frame {i}");
    }
}

// Spec: sampler R10, R13, R22
#[test]
fn one_shot_and_slice_play_flag_to_flag_whatever_s_start_says() {
    let ramp = sample(4800, |i| i as f32 / 4800.0);
    let collector = Collector::new();

    let mut node = sampler(&[(MODE, 1.0), (START, 50.0), (LENGTH, 25.0)], 4800);
    give_region(node.as_mut(), &collector, &ramp, (1200, 3600));
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    for i in [0, 1000, 2399] {
        assert!(
            (out[0][i] - (1200 + i) as f32 / 4800.0).abs() < 1e-6,
            "frame {i}"
        );
    }
    assert!(
        out[0][2400..].iter().all(|s| *s == 0.0),
        "it ends at the End flag"
    );

    // Slice by Region cuts only inside the flags: four slices of 600
    // frames, the last ending at End.
    let mut node = sampler(&[(MODE, 2.0), (SLICE_BY, 2.0), (SLICE_REGIONS, 4.0)], 4800);
    give_region(node.as_mut(), &collector, &ramp, (1200, 3600));
    for (index, start) in [1200usize, 1800, 2400, 3000].into_iter().enumerate() {
        let out = run_notes(
            node.as_mut(),
            4800,
            &[note(0, NoteKind::On, 60 + index as u8, 1.0)],
        );
        assert!(
            (out[0][10] - (start + 10) as f32 / 4800.0).abs() < 1e-6,
            "slice {index}"
        );
        assert!(
            out[0][600..].iter().all(|s| *s == 0.0),
            "slice {index} ends"
        );
    }
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 64, 1.0)]);
    assert!(out.iter().flatten().all(|s| *s == 0.0), "no fifth slice");
}

// Spec: sampler R4
#[test]
fn the_loop_crossfades_without_a_click() {
    // A cosine at 15 Hz over 4800 frames: its end is near −1 and its start
    // 1, so a bare wrap clicks.
    let cosine = sample(4800, |i| {
        (std::f64::consts::TAU * 15.0 * i as f64 / 48_000.0).cos() as f32
    });
    let collector = Collector::new();
    let step = |out: &[f32]| {
        out.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };

    let mut looped = sampler(&[(LOOP, 1.0), (FADE, 50.0), (ATTACK, 5.0)], 14_400);
    give(looped.as_mut(), &collector, &cosine);
    let out = run_notes(looped.as_mut(), 14_400, &[note(0, NoteKind::On, 60, 1.0)]);
    assert!(
        step(&out[0]) < 0.05,
        "three laps of the loop stay smooth: {}",
        step(&out[0])
    );

    // Without the crossfade the wrap jumps: the test would catch a click.
    let mut bare = sampler(&[(LOOP, 1.0), (FADE, 0.0), (ATTACK, 5.0)], 14_400);
    give(bare.as_mut(), &collector, &cosine);
    let out = run_notes(bare.as_mut(), 14_400, &[note(0, NoteKind::On, 60, 1.0)]);
    assert!(step(&out[0]) > 1.0, "a bare wrap clicks");
}

// Spec: sampler R4
#[test]
fn a_full_length_fade_still_crossfades_the_wrap_without_a_click_or_a_swell() {
    // The cosine of the test above, its whole length the loop and the fade.
    let cosine = sample(4800, |i| {
        (std::f64::consts::TAU * 15.0 * i as f64 / 48_000.0).cos() as f32
    });
    let collector = Collector::new();
    let mut looped = sampler(
        &[
            (LOOP, 1.0),
            (LOOP_LENGTH, 100.0),
            (FADE, 100.0),
            (ATTACK, 5.0),
        ],
        14_400,
    );
    give(looped.as_mut(), &collector, &cosine);
    let out = run_notes(looped.as_mut(), 14_400, &[note(0, NoteKind::On, 60, 1.0)]);
    let step = out[0]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(step < 0.05, "no click at the wrap: {step}");
    let peak = out[0].iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!(peak < 1.01, "no swell mid-loop: {peak}");
}

// Spec: sampler R5
#[test]
fn the_envelope_stages_take_their_times_within_five_percent() {
    let flat = sample(48_000, |_| 1.0);
    let collector = Collector::new();
    let mut node = sampler(
        &[
            (ATTACK, 10.0),
            (DECAY, 20.0),
            (SUSTAIN, 50.0),
            (RELEASE, 30.0),
        ],
        48_000,
    );
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(
        node.as_mut(),
        48_000,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(24_000, NoteKind::Off, 60, 1.0),
        ],
    );
    let left = &out[0];
    // 10 ms at 48 kHz: the peak lands at frame 480, within 5 %.
    let peak = first(left, |s| *s >= 0.999);
    assert!(
        (peak as f64 - 480.0).abs() <= 480.0 * 0.05,
        "attack: {peak}"
    );
    // 20 ms of decay to 50 %: held by frame 2400.
    assert!((left[2400] - 0.5).abs() < 0.01, "sustain: {}", left[2400]);
    // 30 ms of release from the note-off at 24000: silence at 25440,
    // within 5 %.
    let silent = 24_000 + first(&left[24_000..], |s| *s == 0.0);
    assert!(
        (silent as f64 - 25_440.0).abs() <= 1440.0 * 0.05,
        "release: {silent}"
    );
}

// Spec: sampler R6
#[test]
fn velocity_scales_the_level_and_pan_places_the_voice() {
    let flat = sample(4800, |_| 1.0);
    let collector = Collector::new();
    let mut node = sampler(&[(VELOCITY, 100.0), (PAN, -50.0)], 4800);
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 0.5)]);
    assert!(
        (out[0][1000] - 0.5).abs() < 0.01,
        "velocity: {}",
        out[0][1000]
    );
    assert!(out[1][1000].abs() < 0.01, "hard left: {}", out[1][1000]);

    let mut node = sampler(&[(PAN, 50.0)], 4800);
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    assert!(out[0][1000].abs() < 0.01, "hard right: {}", out[0][1000]);
    assert!(
        (out[1][1000] - 1.0).abs() < 0.01,
        "hard right: {}",
        out[1][1000]
    );
}

// Spec: sampler R7
#[test]
fn the_output_level_follows_volume_and_gain() {
    let flat = sample(4800, |_| 1.0);
    let collector = Collector::new();
    // Gain at its bottom (rule G1): silence, though a note plays.
    let mut node = sampler(&[(GAIN, 0.0)], 4800);
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    assert!(out.iter().flatten().all(|s| *s == 0.0));

    // Volume 6 dB down scales the output by that gain.
    let mut node = sampler(&[(VOLUME, db_to_gain(-6.0))], 4800);
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(node.as_mut(), 4800, &[note(0, NoteKind::On, 60, 1.0)]);
    let wanted = db_to_gain(-6.0) as f32;
    assert!((out[0][1000] - wanted).abs() < 0.01);
}

// Spec: sampler R8
#[test]
fn a_stolen_voice_fades_out_without_a_click() {
    let flat = sample(48_000, |_| 1.0);
    let collector = Collector::new();
    // One voice, and the new note's attack as long as the steal's fade:
    // the two cross, so the level never dips or jumps.
    let mut node = sampler(&[(VOICES, 1.0), (ATTACK, 5.0)], 48_000);
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(
        node.as_mut(),
        48_000,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(24_000, NoteKind::On, 64, 1.0),
        ],
    );
    let left = &out[0];
    assert!((left[20_000] - 1.0).abs() < 0.01, "one voice before");
    assert!(
        left[24_000..24_240].iter().all(|s| (s - 1.0).abs() < 0.05),
        "the steal crosses without a dip or a jump"
    );
    assert!((left[30_000] - 1.0).abs() < 0.01, "one voice after");
    let biggest = left[1..]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(biggest < 0.05, "no click: {biggest}");
}

// Spec: sampler R9
#[test]
fn glide_slides_from_the_last_note_and_portamento_keeps_the_voices() {
    let flat = sample(48_000, |_| 1.0);
    let collector = Collector::new();

    // Glide's monophonic mode takes the voice over: the level stays one
    // voice's.
    let mut mono = sampler(
        &[(GLIDE, 1.0), (GLIDE_MODE, 0.0), (GLIDE_TIME, 10.0)],
        48_000,
    );
    give(mono.as_mut(), &collector, &flat);
    let out = run_notes(
        mono.as_mut(),
        48_000,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(1000, NoteKind::On, 72, 1.0),
        ],
    );
    assert!(
        (out[0][2000] - 1.0).abs() < 0.01,
        "one voice: {}",
        out[0][2000]
    );

    // Portamento is polyphonic: the two notes' voices overlap.
    let mut poly = sampler(
        &[(GLIDE, 1.0), (GLIDE_MODE, 1.0), (GLIDE_TIME, 10.0)],
        48_000,
    );
    give(poly.as_mut(), &collector, &flat);
    let out = run_notes(
        poly.as_mut(),
        48_000,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(1000, NoteKind::On, 72, 1.0),
        ],
    );
    assert!(
        (out[0][2000] - 2.0).abs() < 0.01,
        "two voices: {}",
        out[0][2000]
    );

    // A glided note moves off the last note's rate: an impulse at half the
    // sample comes out later than at 2x and earlier than at 1x.
    let impulse = sample(9600, |i| if i == 4800 { 1.0 } else { 0.0 });
    let mut glided = sampler(
        &[
            (GLIDE, 1.0),
            (GLIDE_MODE, 1.0),
            (GLIDE_TIME, 10.0),
            (RELEASE, 0.01),
        ],
        9600,
    );
    give(glided.as_mut(), &collector, &impulse);
    let out = run_notes(
        glided.as_mut(),
        9600,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(500, NoteKind::Off, 60, 1.0),
            note(1000, NoteKind::On, 72, 1.0),
        ],
    );
    let hit = first(&out[0], |s| s.abs() > 0.9);
    assert!(
        (3400..4800).contains(&hit),
        "the glide starts at C3's rate: {hit}"
    );

    // Without the glide the same note hits at 2x: frame 3400.
    let mut plain = sampler(&[(RELEASE, 0.01)], 9600);
    give(plain.as_mut(), &collector, &impulse);
    let out = run_notes(
        plain.as_mut(),
        9600,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(500, NoteKind::Off, 60, 1.0),
            note(1000, NoteKind::On, 72, 1.0),
        ],
    );
    assert_eq!(first(&out[0], |s| s.abs() > 0.9), 3400);
}

// Spec: sampler R9
#[test]
fn a_glided_note_is_not_taken_over_into_a_voice_that_is_releasing() {
    let flat = sample(96_000, |_| 1.0);
    let collector = Collector::new();
    let mut mono = sampler(
        &[
            (GLIDE, 1.0),
            (GLIDE_MODE, 0.0),
            (GLIDE_TIME, 10.0),
            (RELEASE, 10.0),
        ],
        24_000,
    );
    give(mono.as_mut(), &collector, &flat);
    // The first note has ended and is releasing when the second starts, and
    // the second is held long after the first's release is over.
    let out = run_notes(
        mono.as_mut(),
        24_000,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(2000, NoteKind::Off, 60, 1.0),
            note(2100, NoteKind::On, 64, 1.0),
        ],
    );
    assert!(
        (out[0][10_000] - 1.0).abs() < 0.01,
        "the held note sounds: {}",
        out[0][10_000]
    );
}

// Spec: sampler R10
#[test]
fn one_shot_ignores_a_note_off_with_trigger_on_and_fades_with_it_off() {
    let flat = sample(4800, |_| 1.0);
    let collector = Collector::new();
    // Trigger on (the default): the note-off at 960 is ignored and the
    // region plays to its end.
    let mut node = sampler(&[(MODE, 1.0)], 9600);
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(
        node.as_mut(),
        9600,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(960, NoteKind::Off, 60, 1.0),
        ],
    );
    assert!(out[0][..4800].iter().all(|s| (s - 1.0).abs() < 1e-6));
    assert!(out[0][4800..].iter().all(|s| *s == 0.0));

    // Trigger off: the note-off fades the region out over Fade Out.
    let mut node = sampler(&[(MODE, 1.0), (TRIGGER, 0.0), (FADE_OUT, 10.0)], 9600);
    give(node.as_mut(), &collector, &flat);
    let out = run_notes(
        node.as_mut(),
        9600,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(960, NoteKind::Off, 60, 1.0),
        ],
    );
    let left = &out[0];
    assert!(left[..960].iter().all(|s| (s - 1.0).abs() < 1e-6));
    // 10 ms at 48 kHz: silent from frame 1440.
    assert!(left[1440..].iter().all(|s| s.abs() < 1e-6));
    assert!(left[960..1440].iter().all(|s| (0.0..=1.0).contains(s)));
}

// Spec: sampler R11
#[test]
fn the_engine_plays_the_sample_the_set_names() {
    let hash = bl_model::ContentHash([7u8; 32]);
    // Two seconds of the sample at 48 kHz: the note's bar.
    let flat = sample(96_000, |_| 1.0);
    let mut media = MediaMap::new();
    media.insert(hash, Arc::clone(&flat));

    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device_id = editor.new_id();
    let mut device = Device::new(
        device_id,
        "Simpler",
        DeviceKind::Native {
            device_type: SAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.set_media(0, Some(hash));
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    let note = Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4), 100);
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 4),
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();

    let registry = registry().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    let unhosted = offline.sync(editor.document());
    assert!(unhosted.is_empty(), "the sampler is hosted: {unhosted:?}");
    assert!(offline.missing_media().is_empty(), "the sample is found");
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(48_000, |block| {
        out.extend_from_slice(block.channel(0));
    });
    assert_eq!(offline.heap_operations(), 0, "no allocation in a block");
    assert!(out[1000] > 0.9, "the note plays the sample: {}", out[1000]);
    assert!(out[47_000] > 0.9, "and holds: {}", out[47_000]);
}

// Spec: sampler R22
#[test]
fn the_engine_counts_the_flags_in_the_samples_own_rate() {
    let hash = bl_model::ContentHash([8u8; 32]);
    // One second of a ramp at 24 kHz, played by an engine at 48 kHz.
    let ramp = Arc::new(
        bl_media::SampleData::new(
            24_000,
            vec![(0..24_000).map(|i| i as f32 / 24_000.0).collect()],
        )
        .unwrap(),
    );
    let heard = |region: Option<bl_model::SampleRegion>| {
        let mut media = MediaMap::new();
        media.insert(hash, Arc::clone(&ramp));
        let mut editor = Editor::new();
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let device_id = editor.new_id();
        let mut device = Device::new(
            device_id,
            "Simpler",
            DeviceKind::Native {
                device_type: SAMPLER.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        device.set_media(0, Some(hash));
        device.params.insert(MODE, 1.0);
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        editor
            .apply(Command::SetDeviceRegion {
                device: device_id,
                region,
            })
            .unwrap();
        let clip: bl_model::ClipId = editor.new_id();
        let note = Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4), 100);
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks(TICKS_PER_QUARTER * 4),
                    ClipData::Midi(MidiData::new([note])),
                ),
            })
            .unwrap();
        let mut offline = Offline::new(
            EngineConfig::new(RATE, 256),
            registry().with_media(Arc::new(media)),
        );
        offline.sync(editor.document());
        offline.sender().transport(TransportCommand::Play);
        let mut out = Vec::new();
        offline.render(60_000, |block| out.extend_from_slice(block.channel(0)));
        out
    };
    // Frames 6,000 to 18,000 of the file are a quarter to three quarters of
    // it: 12,000 to 36,000 of the engine's.
    let out = heard(Some(bl_model::SampleRegion::Frames {
        start: 6_000,
        end: 18_000,
    }));
    assert!(
        (out[100] - 0.25).abs() < 0.01,
        "from the Start flag: {}",
        out[100]
    );
    assert!((out[23_000] - 0.73).abs() < 0.01, "{}", out[23_000]);
    assert!(
        out[24_200..].iter().all(|s| s.abs() < 1e-3),
        "to the End flag"
    );
    // Shares, as an older set holds them, are counted in the engine's frames.
    let out = heard(Some(bl_model::SampleRegion::Shares {
        start_share: 0.5,
        end_share: 0.75,
    }));
    assert!((out[100] - 0.5).abs() < 0.01, "{}", out[100]);
    assert!(
        out[12_300..].iter().all(|s| s.abs() < 1e-3),
        "a quarter long"
    );
}

// Spec: sampler R12
#[test]
fn the_samplers_blocks_run_under_the_allocation_trap() {
    // Every block above ran under the trap; this one walks the audio
    // thread's path once more with a note, a change and data arriving.
    let flat = sample(4800, |_| 1.0);
    let collector = Collector::new();
    let mut node = sampler(&[(LOOP, 1.0), (FADE, 50.0)], 512);
    give(node.as_mut(), &collector, &flat);
    let events = [NodeEvent {
        offset: 10,
        param: VOLUME,
        value: ParamValue::Set(-6.0),
    }];
    let notes = [
        note(0, NoteKind::On, 60, 1.0),
        note(200, NoteKind::Off, 60, 1.0),
    ];
    let mut data = vec![0.0f32; CHANNELS * 512];
    let mut buffer = NoteBuffer::with_capacity(4);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            512,
            512,
            &events,
            TransportInfo::default(),
            RATE,
        )
        .with_notes(&notes, &mut buffer);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
}

// Spec: sampler R13 (one note per slice, in order) and R14 (transient
// slicing)
#[test]
fn slicing_a_drum_loop_by_transient_finds_its_hits_within_a_millisecond() {
    // Four hits, one every 9600 frames: a tone burst with an instant
    // attack, decaying over 2400 frames, each quieter than the one before.
    let drum = sample(38_400, |i| {
        let amplitude = 0.8 - 0.15 * (i / 9600) as f32;
        let at = (i % 9600) as f32;
        if at < 2400.0 {
            amplitude * (1.0 - at / 2400.0) * (std::f32::consts::TAU * 300.0 * at / 48_000.0).cos()
        } else {
            0.0
        }
    });
    let collector = Collector::new();
    let mut node = sampler(&[(MODE, 2.0), (SLICE_SENSITIVITY, 50.0)], 38_400);
    give(node.as_mut(), &collector, &drum);
    // A slice starts reading its sample where its hit is, so the note's
    // first output sample is the hit's: a slice that began a millisecond
    // late would miss the first 48 frames of the burst, and its first
    // sample would be the burst's 48-frames-in value.
    for index in 0..4usize {
        let key = 60 + index as u8;
        let out = run_notes(node.as_mut(), 38_400, &[note(0, NoteKind::On, key, 1.0)]);
        let first = 0.8 - 0.15 * index as f32;
        let late =
            first * (1.0 - 48.0 / 2400.0) * (std::f32::consts::TAU * 300.0 * 48.0 / 48_000.0).cos();
        assert!(
            (late..=first + 1e-6).contains(&out[0][0]),
            "slice {index} plays the sample from {}: its hit's first sample is {first}",
            out[0][0]
        );
    }
    // A key past the slices plays nothing.
    let out = run_notes(node.as_mut(), 38_400, &[note(0, NoteKind::On, 64, 1.0)]);
    assert!(out.iter().flatten().all(|s| *s == 0.0), "no fifth slice");
}

// Spec: sampler R15 (Region and Beat slicing; Manual places no slices)
#[test]
fn region_and_beat_slicing_cut_the_region_by_parameter_and_tempo() {
    // A ramp of one frame per sample: a slice's output begins with its own
    // start's value.
    let ramp = sample(48_000, |i| i as f32 / 48_000.0);
    let collector = Collector::new();

    // Region slicing: four equal slices.
    let mut node = sampler(
        &[(MODE, 2.0), (SLICE_BY, 2.0), (SLICE_REGIONS, 4.0)],
        48_000,
    );
    give(node.as_mut(), &collector, &ramp);
    for (index, start) in [0u64, 12_000, 24_000, 36_000].into_iter().enumerate() {
        let key = 60 + index as u8;
        let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, key, 1.0)]);
        let wanted = (start + 10) as f32 / 48_000.0;
        assert!(
            (out[0][10] - wanted).abs() < 1e-4,
            "slice {index} starts at {start}: {}",
            out[0][10]
        );
    }

    // Beat slicing: a quarter note is 24000 frames at 120 BPM, so the
    // region cuts in two; at 60 BPM the division is the whole sample, one
    // slice.
    let mut node = sampler(
        &[(MODE, 2.0), (SLICE_BY, 1.0), (SLICE_DIVISION, 4.0)],
        48_000,
    );
    give(node.as_mut(), &collector, &ramp);
    let at_120 = TempoMap::constant(Bpm::new(120.0).expect("120 BPM"));
    let out = run_notes_at(
        node.as_mut(),
        48_000,
        &[note(0, NoteKind::On, 61, 1.0)],
        &at_120,
    );
    let wanted = 24_010.0 / 48_000.0;
    assert!(
        (out[0][10] as f64 - wanted).abs() < 1e-4,
        "the second slice at 120 BPM: {}",
        out[0][10]
    );
    let at_60 = TempoMap::constant(Bpm::new(60.0).expect("60 BPM"));
    let out = run_notes_at(
        node.as_mut(),
        48_000,
        &[note(0, NoteKind::On, 61, 1.0)],
        &at_60,
    );
    assert!(
        out.iter().flatten().all(|s| *s == 0.0),
        "at 60 BPM the division is the whole sample: one slice"
    );

    // Manual slicing places no slices automatically.
    let mut node = sampler(&[(MODE, 2.0), (SLICE_BY, 3.0)], 48_000);
    give(node.as_mut(), &collector, &ramp);
    let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 60, 1.0)]);
    assert!(
        out.iter().flatten().all(|s| *s == 0.0),
        "Manual has no slices to play"
    );
}

// Spec: sampler R16 (the filter's types match their prototypes)
#[test]
fn the_filters_types_match_their_prototypes_within_point_two_db() {
    let frequencies = [
        200.0, 400.0, 700.0, 1_000.0, 1_400.0, 2_000.0, 4_000.0, 8_000.0,
    ];
    // One sample holding every test frequency at once.
    let sines = sample(24_000, |i| {
        let t = i as f64 / HZ;
        let sum: f64 = frequencies
            .iter()
            .map(|hz| (core::f64::consts::TAU * hz * t).sin())
            .sum();
        (sum / frequencies.len() as f64 * 0.8) as f32
    });
    let collector = Collector::new();
    let cutoff = 1_000.0;
    let s_at = |hz: f64, cutoff: f64| {
        (core::f64::consts::PI * hz / HZ).tan() / (core::f64::consts::PI * cutoff / HZ).tan()
    };
    // The kit's prototypes, written out independently (the kit's own
    // filter tests do the same): a second-order section with damping k.
    // The low-pass and high-pass run at a Butterworth's damping and the
    // band-pass and notch at 1 (unity at the centre).
    let second = |k: f64, s: f64| 1.0 / ((1.0 - s * s).powi(2) + (k * s).powi(2)).sqrt();
    let prototype = |kind: usize, s: f64| -> f64 {
        let k = 2f64.sqrt();
        let lp = |k: f64| second(k, s);
        let hp = |k: f64| s * s * second(k, s);
        let bp = |k: f64| s * second(k, s);
        let notch = |k: f64| (1.0 - s * s).abs() * second(k, s);
        match kind {
            0 => lp(k),
            // The fourth-order pair: 2 sin(3π/8) then 2 sin(π/8).
            1 => lp(1.847_759) * second(0.765_366_9, s),
            2 => hp(k),
            3 => hp(1.847_759) * s * s * second(0.765_366_9, s),
            4 => bp(1.0),
            5 => bp(1.0).powi(2),
            // Peak and notch run at a damping of 2 at resonance 0: the
            // peak is flat there, the notch's centre is its zero.
            6 => notch(2.0),
            7 => notch(2.0).powi(2),
            _ => unreachable!(),
        }
    };
    for kind in 0..8usize {
        let mut plain = sampler(&[], 24_000);
        give(plain.as_mut(), &collector, &sines);
        let plain = run_notes(plain.as_mut(), 24_000, &[note(0, NoteKind::On, 60, 1.0)]);
        let mut filtered = sampler(
            &[
                (FILTER, 1.0),
                (FILTER_TYPE, kind as f64),
                (FILTER_FREQ, cutoff),
            ],
            24_000,
        );
        give(filtered.as_mut(), &collector, &sines);
        let filtered = run_notes(filtered.as_mut(), 24_000, &[note(0, NoteKind::On, 60, 1.0)]);
        let mut worst = (0.0f64, 0.0f64);
        for hz in frequencies {
            let measured = db(amplitude_at(&filtered[0][8_000..16_000], hz)
                / amplitude_at(&plain[0][8_000..16_000], hz));
            let expected = db(prototype(kind, s_at(hz, cutoff)));
            println!("type {kind} at {hz} Hz: {measured:.3} dB (prototype {expected:.3} dB)");
            // At the notch's centre the prototype's response is zero; the
            // measured one is a deep notch instead.
            if expected.is_infinite() {
                assert!(
                    measured < -30.0,
                    "type {kind} at {hz} Hz: a notch of only {measured:.1} dB"
                );
                continue;
            }
            if (measured - expected).abs() > worst.0 {
                worst = ((measured - expected).abs(), hz);
            }
        }
        assert!(
            worst.0 <= 0.2,
            "type {kind}: off by {:.3} dB at {:.0} Hz",
            worst.0,
            worst.1
        );
    }
}

// Spec: sampler R16 (Key tracks the note; the filter envelope moves the
// cutoff)
#[test]
fn key_tracking_and_the_filter_envelope_move_the_cutoff() {
    // A 4 kHz tone; it plays twice as fast an octave up.
    let tone = sample(24_000, |i| {
        ((core::f64::consts::TAU * 4_000.0 * i as f64 / HZ).sin() * 0.8) as f32
    });
    let collector = Collector::new();
    let s_at = |hz: f64, cutoff: f64| {
        (core::f64::consts::PI * hz / HZ).tan() / (core::f64::consts::PI * cutoff / HZ).tan()
    };
    let lp12 = |s: f64| 1.0 / ((1.0 - s * s).powi(2) + 2.0 * s * s).sqrt();
    let play = |params: &[(ParamId, f64)], key: u8| -> Vec<f32> {
        let mut node = sampler(params, 24_000);
        give(node.as_mut(), &collector, &tone);
        run_notes(node.as_mut(), 24_000, &[note(0, NoteKind::On, key, 1.0)]).remove(0)
    };

    // The reference: the tone at C4, 8 kHz, unfiltered.
    let plain = play(&[], 72);
    let plain = amplitude_at(&plain[8_000..16_000], 8_000.0);

    // Key 0: the cutoff stays at 1 kHz, an octave below the keyed one.
    let no_key = play(
        &[(FILTER, 1.0), (FILTER_FREQ, 1_000.0), (FILTER_KEY, 0.0)],
        72,
    );
    let measured = db(amplitude_at(&no_key[8_000..16_000], 8_000.0) / plain);
    let expected = db(lp12(s_at(8_000.0, 1_000.0)));
    assert!(
        (measured - expected).abs() <= 0.3,
        "Key 0: {measured:.3} dB, wanted {expected:.3} dB"
    );

    // Key 100 %: the cutoff follows the note an octave up, to 2 kHz.
    let keyed = play(
        &[(FILTER, 1.0), (FILTER_FREQ, 1_000.0), (FILTER_KEY, 100.0)],
        72,
    );
    let measured = db(amplitude_at(&keyed[8_000..16_000], 8_000.0) / plain);
    let expected = db(lp12(s_at(8_000.0, 2_000.0)));
    assert!(
        (measured - expected).abs() <= 0.3,
        "Key 100 %: {measured:.3} dB, wanted {expected:.3} dB"
    );

    // The filter envelope at full amount opens the cutoff four octaves
    // and its own Decay falls back to the knob's frequency: loud at first,
    // then the low-pass's attenuation at 1 kHz.
    let enveloped = play(
        &[
            (FILTER, 1.0),
            (FILTER_FREQ, 1_000.0),
            (FILTER_ENV_AMOUNT, 100.0),
            (FILTER_ENV_DECAY, 100.0),
            (FILTER_ENV_SUSTAIN, 0.0),
        ],
        60,
    );
    let plain_c3 = play(&[], 60);
    let plain_c3 = amplitude_at(&plain_c3[8_000..16_000], 4_000.0);
    let early = db(amplitude_at(&enveloped[1_000..2_000], 4_000.0) / plain_c3);
    assert!(early > -3.0, "open at first: {early:.3} dB");
    let late = db(amplitude_at(&enveloped[16_000..24_000], 4_000.0) / plain_c3);
    let expected = db(lp12(s_at(4_000.0, 1_000.0)));
    assert!(
        (late - expected).abs() <= 0.3,
        "closed after the decay: {late:.3} dB, wanted {expected:.3} dB"
    );
}

// Spec: sampler R17 (the LFO's Filter amount moves the cutoff)
#[test]
fn the_lfos_filter_amount_moves_the_cutoff() {
    // A 4 kHz tone through a 1 kHz low-pass: with the LFO's Filter amount
    // at full depth the cutoff swings four octaves, so the level swings.
    let tone = sample(48_000, |i| {
        ((core::f64::consts::TAU * 4_000.0 * i as f64 / HZ).sin() * 0.8) as f32
    });
    let collector = Collector::new();
    let peak = |window: &[f32]| window.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
    // At 2 Hz a cycle is 24000 frames: the first window is the top of the
    // swing, around frame 6000, and the second its bottom, around 18000.
    let measure = |depth: f64| -> (f32, f32) {
        let mut node = sampler(
            &[
                (FILTER, 1.0),
                (FILTER_FREQ, 1_000.0),
                (LFO, 1.0),
                (LFO_RATE, 2.0),
                (LFO_FILTER, depth),
            ],
            48_000,
        );
        give(node.as_mut(), &collector, &tone);
        let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 60, 1.0)]);
        (peak(&out[0][4_000..8_000]), peak(&out[0][16_000..20_000]))
    };
    let (top, bottom) = measure(0.0);
    assert!(
        (top - bottom).abs() < 0.01 * bottom,
        "without depth the level holds: {top} against {bottom}"
    );
    let (top, bottom) = measure(100.0);
    assert!(top > 0.5, "the top of the swing passes the tone: {top}");
    assert!(
        bottom < 0.05 * top,
        "the bottom closes the cutoff: {bottom} against {top}"
    );
}

// Spec: sampler R17 (the LFO syncs to the tempo)
#[test]
fn the_lfo_syncs_to_the_tempo() {
    let flat = sample(96_000, |_| 0.5);
    let collector = Collector::new();
    // A volume LFO at full amount swings the level from 0 to 1: the gaps
    // between the crossings of the unmodulated level are half a cycle.
    let gaps = |bpm: f64| -> Vec<f64> {
        let mut node = sampler(
            &[
                (LFO, 1.0),
                (LFO_SYNC, 1.0),
                (LFO_DIVISION, 4.0),
                (LFO_VOLUME, 100.0),
            ],
            96_000,
        );
        give(node.as_mut(), &collector, &flat);
        let tempo = TempoMap::constant(Bpm::new(bpm).expect("a tempo"));
        let out = run_notes_at(
            node.as_mut(),
            96_000,
            &[note(0, NoteKind::On, 60, 1.0)],
            &tempo,
        );
        let crossings: Vec<usize> = out[0]
            .windows(2)
            .enumerate()
            .filter(|(_, w)| (w[0] > 0.5) != (w[1] > 0.5))
            .map(|(i, _)| i)
            .collect();
        crossings.windows(2).map(|w| (w[1] - w[0]) as f64).collect()
    };
    // A quarter note at 120 BPM is 24000 frames, at 60 BPM 48000; half a
    // cycle is half that.
    for (bpm, half) in [(120.0, 12_000.0), (60.0, 24_000.0)] {
        let gaps = gaps(bpm);
        assert!(gaps.len() >= 2, "{bpm} BPM: only {} crossings", gaps.len());
        for gap in &gaps {
            assert!(
                (gap - half).abs() < 100.0,
                "{bpm} BPM: half a cycle is {gap} frames, wanted {half}"
            );
        }
    }
}

/// A set with a Simpler on a MIDI track holding a held C3 for two beats,
/// playing `hash` with `edits`; the registry finds `media`.
fn edited_set(
    hash: bl_model::ContentHash,
    edits: Vec<bl_model::SampleEdit>,
) -> (Editor, bl_model::DeviceId) {
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device_id = editor.new_id();
    let mut device = Device::new(
        device_id,
        "Simpler",
        DeviceKind::Native {
            device_type: SAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.set_media(0, Some(hash));
    device.sample_edits = edits;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    let note = Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 2), 100);
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 2),
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();
    (editor, device_id)
}

/// Renders a second of the set from its start.
fn render_second(offline: &mut Offline) -> Vec<f32> {
    offline.sender().transport(TransportCommand::Stop);
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(48_000, |block| out.extend_from_slice(block.channel(0)));
    out
}

// Spec: multisampler R57
#[test]
fn an_edited_sample_reaches_the_node_off_the_audio_thread() {
    use bl_model::{SampleEdit, SampleEditKind, SampleRange};
    let hash = bl_model::ContentHash([8u8; 32]);
    // A rising ramp over two seconds at 48 kHz.
    let ramp = sample(96_000, |i| i as f32 / 96_000.0);
    let mut media = MediaMap::new();
    media.insert(hash, Arc::clone(&ramp));
    let cut = SampleEdit {
        kind: SampleEditKind::Cut,
        range: Some(SampleRange {
            from: 0,
            to: 48_000,
        }),
    };
    let (mut editor, device_id) = edited_set(hash, vec![cut]);
    let registry = registry().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    assert!(offline.sync(editor.document()).is_empty());
    assert!(offline.missing_media().is_empty());
    // The cut removed the first second, so the note starts at the ramp's
    // middle: the engine plays the edited sample.
    let out = render_second(&mut offline);
    assert!(
        (out[1000] - (0.5 + 1000.0 / 96_000.0)).abs() < 1e-3,
        "{}",
        out[1000]
    );
    assert_eq!(offline.heap_operations(), 0, "no allocation in a block");
    // An edit made while the set is open reaches the running node as data:
    // trim to the first 100 frames of what is left, which then ends.
    editor
        .apply(Command::AddSampleEdit {
            device: device_id,
            layer: None,
            edit: SampleEdit {
                kind: SampleEditKind::Trim,
                range: Some(SampleRange { from: 0, to: 100 }),
            },
        })
        .unwrap();
    assert!(offline.sync(editor.document()).is_empty());
    let out = render_second(&mut offline);
    assert!((out[50] - 0.5).abs() < 1e-2, "{}", out[50]);
    assert!(
        out[1000].abs() < 1e-6,
        "the trimmed sample has ended: {}",
        out[1000]
    );
    assert_eq!(offline.heap_operations(), 0);
    // Reverting the edits brings the source back.
    editor
        .apply(Command::ClearSampleEdits {
            device: device_id,
            layer: None,
            length: 0,
        })
        .unwrap();
    assert!(offline.sync(editor.document()).is_empty());
    let out = render_second(&mut offline);
    assert!(
        (out[1000] - 1000.0 / 96_000.0).abs() < 1e-3,
        "{}",
        out[1000]
    );
}

// Spec: multisampler R54
#[test]
fn a_click_auditions_from_its_point_and_ending_it_fades_the_voice_out() {
    use bl_engine::Audition;
    let ramp = sample(48_000, |i| 0.25 + i as f32 / 96_000.0);
    let collector = Collector::new();
    let mut node = sampler(&[], 512);
    give(node.as_mut(), &collector, &ramp);
    let audition = |down| {
        collector.handle().data(Audition {
            layer: None,
            from: 0.5,
            down,
        })
    };
    // Nothing plays until the button goes down.
    assert!(
        run_notes(node.as_mut(), 256, &[])[0]
            .iter()
            .all(|v| *v == 0.0)
    );
    node.receive_data(audition(true));
    let played = run_notes(node.as_mut(), 512, &[]);
    // It starts at half the sample, at its own pitch.
    assert!(
        (played[0][10] - (0.25 + 24_010.0 / 96_000.0)).abs() < 1e-3,
        "{}",
        played[0][10]
    );
    assert!((played[0][400] - (0.25 + 24_400.0 / 96_000.0)).abs() < 1e-3);
    // Releasing the button fades it out, whatever the mode's Trigger says.
    node.receive_data(audition(false));
    let faded = run_notes(node.as_mut(), 512, &[]);
    let silent = run_notes(node.as_mut(), 512, &[]);
    assert!(faded[0][0] > 0.0, "it faded rather than cut");
    assert!(
        silent[0].iter().all(|v| *v == 0.0),
        "and it ended after the steal fade"
    );
}

// Spec: multisampler R54
#[test]
fn with_glide_on_a_click_starts_a_voice_of_its_own() {
    use bl_engine::Audition;
    let ramp = sample(48_000, |i| 0.25 + i as f32 / 96_000.0);
    let collector = Collector::new();
    let mut node = sampler(&[(GLIDE, 1.0), (GLIDE_MODE, 0.0), (GLIDE_TIME, 10.0)], 512);
    give(node.as_mut(), &collector, &ramp);
    let audition = |down| {
        collector.handle().data(Audition {
            layer: None,
            from: 0.5,
            down,
        })
    };
    // A held note an octave up, which the click mustn't take over.
    run_notes(node.as_mut(), 512, &[note(0, NoteKind::On, 72, 1.0)]);
    node.receive_data(audition(true));
    let both = run_notes(node.as_mut(), 512, &[]);
    node.receive_data(audition(false));
    run_notes(node.as_mut(), 512, &[]);
    run_notes(node.as_mut(), 512, &[]);
    let held = run_notes(node.as_mut(), 512, &[]);
    // The held note plays on alone once the audition has faded, so the
    // audition was a voice of its own.
    assert!(held[0].iter().all(|v| *v > 0.0), "the held note sounds on");
    let at = |frame: f64| 0.25 + frame / 96_000.0;
    // The held voice is two blocks in at an octave up when the click lands;
    // the audition plays at the sample's own pitch from half way.
    let expected = at(2.0 * 522.0) + at(24_010.0);
    assert!(
        (f64::from(both[0][10]) - expected).abs() < 1e-2,
        "{} against {expected}",
        both[0][10]
    );
}

// Spec: multisampler R54
#[test]
fn slice_boundaries_are_what_the_device_cuts() {
    use bl_devices::slice_boundaries;
    // A click every 4,800 frames, the transients Slice By finds.
    let clicks = sample(19_200, |i| if i % 4_800 < 20 { 1.0 } else { 0.0 });
    let value = |id: ParamId| match id {
        SLICE_BY => 0.0,
        SLICE_SENSITIVITY => 50.0,
        SLICE_DIVISION => 4.0,
        SLICE_REGIONS => 4.0,
        _ => 0.0,
    };
    let bounds = slice_boundaries(&clicks, (0, 19_200), &value, 6_000.0, &[]);
    assert_eq!(bounds.first(), Some(&0));
    assert_eq!(bounds, [0, 4_800, 9_600, 14_400, 19_200]);
    // Region slicing cuts four equal parts; Manual places none.
    let regions = |by: f64| {
        slice_boundaries(
            &clicks,
            (0, 19_200),
            &|id| if id == SLICE_BY { by } else { value(id) },
            6_000.0,
            &[],
        )
    };
    assert_eq!(regions(2.0), [0, 4_800, 9_600, 14_400, 19_200]);
    assert!(regions(3.0).is_empty());
    // Manual slicing with manual slices provided.
    let manual = slice_boundaries(
        &clicks,
        (0, 19_200),
        &|id| if id == SLICE_BY { 3.0 } else { value(id) },
        6_000.0,
        &[5_000, 10_000],
    );
    assert_eq!(manual, [0, 5_000, 10_000, 19_200]);
    // The Start and End flags bound the cut (the sampler spec, R22).
    let inside = slice_boundaries(&clicks, (4_800, 14_400), &value, 6_000.0, &[]);
    assert_eq!(inside, [4_800, 9_600, 14_400]);
}

// Spec: sampler R18
#[test]
fn warped_sample_plays_at_tempo_matching_warped_clip_golden_render() {
    let hash = bl_model::ContentHash([9u8; 32]);
    const STEP: f32 = 1.0 / 65_536.0;
    // 4 seconds of ramp at 48 kHz.
    let ramp = Arc::new(
        bl_media::SampleData::new(
            48_000,
            vec![
                (0..192_000).map(|i| (i + 1) as f32 * STEP).collect(),
                (0..192_000).map(|i| -((i + 1) as f32 * STEP)).collect(),
            ],
        )
        .unwrap(),
    );
    let mut media = MediaMap::new();
    media.insert(hash, Arc::clone(&ramp));
    let media: Arc<dyn bl_engine::MediaLibrary> = Arc::new(media);
    let make_registry = || registry().with_media(Arc::clone(&media));

    // Audio clip song: 1 bar at 120 BPM, playing ramp warped at 90 BPM.
    let song_audio = {
        let mut editor = Editor::new();
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let clip_id = editor.new_id();
        let audio = AudioData {
            warp: true,
            warp_markers: vec![WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            }],
            tempo: Some(90.0),
            warp_mode: WarpMode::RePitch,
            high_quality: false,
            ..AudioData::new(hash)
        };
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip_id,
                    Ticks::ZERO,
                    Ticks(TICKS_PER_QUARTER * 4),
                    ClipData::Audio(audio),
                ),
            })
            .unwrap();
        editor
    };

    // Simpler song: 1 bar at 120 BPM, playing ramp warped at 90 BPM via held C3.
    let song_simpler = {
        let mut editor = Editor::new();
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let device_id = editor.new_id();
        let mut device = Device::new(
            device_id,
            "Simpler",
            DeviceKind::Native {
                device_type: SAMPLER.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        device.set_media(0, Some(hash));
        device.warp = DeviceWarp {
            warp: true,
            mode: WarpMode::RePitch,
            settings: WarpSettings::default(),
            bpm: Some(90.0),
        };
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        let clip_id = editor.new_id();
        let note = Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4), 100);
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip_id,
                    Ticks::ZERO,
                    Ticks(TICKS_PER_QUARTER * 4),
                    ClipData::Midi(MidiData::new([note])),
                ),
            })
            .unwrap();
        editor
    };

    // 1 bar at 120 BPM is 2 seconds = 96,000 frames.
    for block in [1, 100, 512, 4_096] {
        let mut offline_clip = Offline::new(EngineConfig::new(RATE, block), make_registry());
        offline_clip.sync(song_audio.document());
        offline_clip.sender().transport(TransportCommand::Play);
        let mut clip_out = Vec::new();
        offline_clip.render(96_000, |b| clip_out.extend_from_slice(b.channel(0)));
        assert_eq!(
            offline_clip.heap_operations(),
            0,
            "no clip allocs at block {block}"
        );

        let mut offline_simpler = Offline::new(EngineConfig::new(RATE, block), make_registry());
        offline_simpler.sync(song_simpler.document());
        offline_simpler.sender().transport(TransportCommand::Play);
        let mut simpler_out = Vec::new();
        offline_simpler.render(96_000, |b| simpler_out.extend_from_slice(b.channel(0)));
        assert_eq!(
            offline_simpler.heap_operations(),
            0,
            "no simpler allocs at block {block}"
        );

        assert_eq!(clip_out.len(), simpler_out.len());
        for frame in 1..96_000 {
            assert!(
                (clip_out[frame] - simpler_out[frame]).abs() < 1e-4,
                "block {block} frame {frame}: clip {} vs simpler {}",
                clip_out[frame],
                simpler_out[frame]
            );
        }
    }
}

// Spec: sampler R18
#[test]
fn held_warped_note_follows_tempo_change() {
    let hash = bl_model::ContentHash([10u8; 32]);
    const STEP: f32 = 1.0 / 65_536.0;
    let ramp = Arc::new(
        bl_media::SampleData::new(
            48_000,
            vec![
                (0..192_000).map(|i| (i + 1) as f32 * STEP).collect(),
                (0..192_000).map(|i| -((i + 1) as f32 * STEP)).collect(),
            ],
        )
        .unwrap(),
    );
    let mut media = MediaMap::new();
    media.insert(hash, Arc::clone(&ramp));
    let registry = registry().with_media(Arc::new(media));

    let mut editor = Editor::new();
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (Ticks(TICKS_PER_QUARTER * 4), Bpm::new(120.0).unwrap()),
        (Ticks(TICKS_PER_QUARTER * 4), Bpm::new(60.0).unwrap()),
    ])
    .unwrap();
    editor.apply(Command::SetTempo { tempo }).unwrap();

    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device_id = editor.new_id();
    let mut device = Device::new(
        device_id,
        "Simpler",
        DeviceKind::Native {
            device_type: SAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.set_media(0, Some(hash));
    device.warp = DeviceWarp {
        warp: true,
        mode: WarpMode::RePitch,
        settings: WarpSettings::default(),
        bpm: Some(90.0),
    };
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let clip_id = editor.new_id();
    // 2 bars of C3: 1 bar at 120 BPM (96,000 frames) then 1 bar at 60 BPM (192,000 frames).
    let note = Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 8), 100);
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip_id,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 8),
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();

    let mut offline = Offline::new(EngineConfig::new(RATE, 512), registry);
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(96_000 + 192_000, |b| out.extend_from_slice(b.channel(0)));
    assert_eq!(offline.heap_operations(), 0);

    // In the first bar (120 BPM): rate = 120 / 90 = 4/3 sample frames per song frame.
    // In the second bar (60 BPM): rate = 60 / 90 = 2/3 sample frames per song frame.
    let pos = |sample_val: f32| (f64::from(sample_val) / f64::from(STEP)) - 1.0;
    let pos_48k = pos(out[48_000]);
    let pos_96k = pos(out[96_000]);
    let pos_192k = pos(out[96_000 + 96_000]);

    // Advance in first bar (48k frames at 120 BPM): 48000 * 4/3 = 64000 frames
    assert!((pos_48k - 64_000.0).abs() < 2.0, "pos at 48k: {pos_48k}");
    // Advance at end of first bar (96k frames at 120 BPM): 96000 * 4/3 = 128000 frames
    assert!((pos_96k - 128_000.0).abs() < 2.0, "pos at 96k: {pos_96k}");
    // Advance in second bar (96k frames at 60 BPM): 128000 + 96000 * 2/3 = 192000 frames
    assert!(
        (pos_192k - 192_000.0).abs() < 2.0,
        "pos at 192k: {pos_192k}"
    );
}

// Spec: sampler R14
#[test]
fn slice_by_beat_follows_the_warp_map() {
    let ramp = sample(96_000, |i| i as f32 / 96_000.0);
    let collector = Collector::new();
    let mut node = sampler(
        &[(MODE, 2.0), (SLICE_BY, 1.0), (SLICE_DIVISION, 4.0)],
        48_000,
    );
    let mut slot = SlotSample::new(0, ramp);
    slot.warp.warp = true;
    slot.warp.bpm = Some(90.0);
    node.receive_data(collector.handle().data(slot));

    // Song tempo is 120 BPM.
    // Unwarped at 120 BPM, 1 beat (division 4) is 24,000 frames.
    // Warped at 90 BPM, 1 beat is 32,000 frames.
    let at_120 = TempoMap::constant(Bpm::new(120.0).expect("120 BPM"));
    let out = run_notes_at(
        node.as_mut(),
        48_000,
        &[note(0, NoteKind::On, 61, 1.0)],
        &at_120,
    );
    // Slice 1 (key 61) starts at 32,000 frames:
    let wanted = (32_000.0 + 10.0) / 96_000.0;
    assert!(
        (out[0][10] as f64 - wanted).abs() < 1e-4,
        "the second slice followed 90 BPM warp map: got {}, wanted {wanted}",
        out[0][10]
    );
}

// Spec: sampler R13, R20
#[test]
fn manual_slices_play_chromatically_from_c3() {
    let ramp = sample(96_000, |i| i as f32 / 96_000.0);
    let collector = Collector::new();
    let mut node = sampler(&[(MODE, 2.0), (SLICE_BY, 3.0)], 48_000);
    let mut slot = SlotSample::new(0, ramp);
    slot.slices = Arc::from([10_000u64, 25_000, 50_000]);
    node.receive_data(collector.handle().data(slot));

    // Key 60 (C3) -> Slice 0 (0..10_000)
    let out0 = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 60, 1.0)]);
    let want0 = 10.0 / 96_000.0;
    assert!(
        (out0[0][10] as f64 - want0).abs() < 1e-4,
        "slice 0: {}",
        out0[0][10]
    );

    // Key 61 (C#3) -> Slice 1 (10_000..25_000)
    let out1 = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 61, 1.0)]);
    let want1 = (10_000.0 + 10.0) / 96_000.0;
    assert!(
        (out1[0][10] as f64 - want1).abs() < 1e-4,
        "slice 1: {}",
        out1[0][10]
    );

    // Key 62 (D3) -> Slice 2 (25_000..50_000)
    let out2 = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 62, 1.0)]);
    let want2 = (25_000.0 + 10.0) / 96_000.0;
    assert!(
        (out2[0][10] as f64 - want2).abs() < 1e-4,
        "slice 2: {}",
        out2[0][10]
    );

    // Key 63 (D#3) -> Slice 3 (50_000..96_000)
    let out3 = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 63, 1.0)]);
    let want3 = (50_000.0 + 10.0) / 96_000.0;
    assert!(
        (out3[0][10] as f64 - want3).abs() < 1e-4,
        "slice 3: {}",
        out3[0][10]
    );

    // Key 64 (E3) -> Past slices, plays silence
    let out4 = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 64, 1.0)]);
    assert!(out4.iter().flatten().all(|s| *s == 0.0), "key 64 is silent");
}

// Spec: sampler R19
#[test]
fn pitch_envelope_moves_pitch_by_amount_rule_by_rule() {
    let ramp = sample(96_000, |i| i as f32 / 96_000.0);
    let collector = Collector::new();

    // 1. Amount +12 semitones, 0 attack, 100% sustain: plays 2x speed.
    let mut node = sampler(
        &[
            (PITCH_ENV_ATTACK, 0.0),
            (PITCH_ENV_SUSTAIN, 100.0),
            (PITCH_ENV_AMOUNT, 12.0),
        ],
        48_000,
    );
    give(node.as_mut(), &collector, &ramp);
    let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 60, 1.0)]);
    let want = 2000.0 / 96_000.0;
    assert!(
        (out[0][1000] as f64 - want).abs() < 1e-3,
        "+12 st plays at 2x: got {}, wanted {want}",
        out[0][1000]
    );

    // 2. Amount -12 semitones, 0 attack, 100% sustain: plays 0.5x speed.
    let mut node = sampler(
        &[
            (PITCH_ENV_ATTACK, 0.0),
            (PITCH_ENV_SUSTAIN, 100.0),
            (PITCH_ENV_AMOUNT, -12.0),
        ],
        48_000,
    );
    give(node.as_mut(), &collector, &ramp);
    let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 60, 1.0)]);
    let want = 500.0 / 96_000.0;
    assert!(
        (out[0][1000] as f64 - want).abs() < 1e-3,
        "-12 st plays at 0.5x: got {}, wanted {want}",
        out[0][1000]
    );

    // 3. Attack ramp: 10 ms (480 frames) attack, +12 semitones.
    let mut node = sampler(
        &[
            (PITCH_ENV_ATTACK, 10.0),
            (PITCH_ENV_SUSTAIN, 100.0),
            (PITCH_ENV_AMOUNT, 12.0),
        ],
        48_000,
    );
    give(node.as_mut(), &collector, &ramp);
    let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 60, 1.0)]);
    let early_diff = (out[0][2] - out[0][1]) as f64 * 96_000.0;
    assert!((early_diff - 1.0).abs() < 0.1, "starts at 1x: {early_diff}");
    let late_diff = (out[0][1002] - out[0][1001]) as f64 * 96_000.0;
    assert!((late_diff - 2.0).abs() < 0.1, "reaches 2x: {late_diff}");

    // 4. Decay to 0 sustain: attack 0, decay 10 ms (480 frames), sustain 0%, +12 st.
    let mut node = sampler(
        &[
            (PITCH_ENV_ATTACK, 0.0),
            (PITCH_ENV_DECAY, 10.0),
            (PITCH_ENV_SUSTAIN, 0.0),
            (PITCH_ENV_AMOUNT, 12.0),
        ],
        48_000,
    );
    give(node.as_mut(), &collector, &ramp);
    let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 60, 1.0)]);
    let start_diff = (out[0][2] - out[0][1]) as f64 * 96_000.0;
    assert!((start_diff - 2.0).abs() < 0.1, "starts at 2x: {start_diff}");
    let settled_diff = (out[0][2002] - out[0][2001]) as f64 * 96_000.0;
    assert!(
        (settled_diff - 1.0).abs() < 0.1,
        "decays back to 1x: {settled_diff}"
    );

    // 5. Release: note-off releases over pitch release time back to 0 amount.
    let mut node = sampler(
        &[
            (PITCH_ENV_ATTACK, 0.0),
            (PITCH_ENV_SUSTAIN, 100.0),
            (PITCH_ENV_RELEASE, 10.0),
            (PITCH_ENV_AMOUNT, 12.0),
            (RELEASE, 60_000.0),
        ],
        48_000,
    );
    give(node.as_mut(), &collector, &ramp);
    let out = run_notes(
        node.as_mut(),
        48_000,
        &[
            note(0, NoteKind::On, 60, 1.0),
            note(1000, NoteKind::Off, 60, 1.0),
        ],
    );
    let mid_diff = (out[0][502] - out[0][501]) as f64 * 96_000.0;
    assert!((mid_diff - 2.0).abs() < 0.1, "held at 2x: {mid_diff}");
    let rel_diff = (out[0][2502] - out[0][2501]) as f64 * 96_000.0;
    assert!(
        (rel_diff - 1.0).abs() < 0.1,
        "released back to 1x: {rel_diff}"
    );
}

/// Eight Region slices of 1200 frames, slice `k` a constant `(k + 1) / 10`,
/// so the sum of what sounds names which slices play (R13, R23).
fn eight_slices() -> Arc<bl_media::SampleData> {
    sample(9600, |i| (i / 1200 + 1) as f32 / 10.0)
}

/// A Slice-mode sampler of the eight slices with `params`.
fn slicer(params: &[(ParamId, f64)], collector: &Collector, max_frames: usize) -> Box<dyn Node> {
    let mut all = vec![(MODE, 2.0), (SLICE_BY, 2.0), (SLICE_REGIONS, 8.0)];
    all.extend_from_slice(params);
    let mut node = sampler(&all, max_frames);
    give(node.as_mut(), collector, &eight_slices());
    node
}

/// The left channel of `frames` frames played in blocks of `size`, `notes`
/// given as (frame, key) note-ons.
fn play_in_blocks(
    node: &mut dyn Node,
    size: usize,
    frames: usize,
    notes: &[(usize, u8)],
) -> Vec<f32> {
    let mut left = Vec::with_capacity(frames);
    for start in (0..frames).step_by(size) {
        let events: Vec<NoteEvent> = notes
            .iter()
            .filter(|(at, _)| (start..start + size).contains(at))
            .map(|(at, key)| note((at - start) as u32, NoteKind::On, *key, 1.0))
            .collect();
        left.extend_from_slice(&run_notes(node, size, &events)[0]);
    }
    left
}

const BLOCK_SIZES: [usize; 3] = [1, 64, 512];

// Spec: sampler R23
#[test]
fn mono_cuts_the_first_slice_when_the_second_plays() {
    let collector = Collector::new();
    for size in BLOCK_SIZES {
        let mut node = slicer(&[], &collector, size);
        let out = play_in_blocks(node.as_mut(), size, 1200, &[(0, 60), (100, 61)]);
        assert!((out[50] - 0.1).abs() < 1e-6, "slice 0 first, block {size}");
        assert!(
            (out[400] - 0.2).abs() < 1e-6,
            "slice 1 alone after the steal, block {size}"
        );
    }
}

// Spec: sampler R23
#[test]
fn poly_sounds_slices_together_up_to_voices_and_drops_the_oldest() {
    let collector = Collector::new();
    let notes = [(0, 60), (10, 61), (20, 62)];
    for size in BLOCK_SIZES {
        let mut node = slicer(&[(PLAYBACK, 1.0), (VOICES, 3.0)], &collector, size);
        let out = play_in_blocks(node.as_mut(), size, 600, &notes);
        assert!(
            (out[300] - 0.6).abs() < 1e-6,
            "three slices together, block {size}"
        );

        let mut node = slicer(&[(PLAYBACK, 1.0), (VOICES, 2.0)], &collector, size);
        let out = play_in_blocks(node.as_mut(), size, 600, &notes);
        assert!(
            (out[300] - 0.5).abs() < 1e-6,
            "the oldest dropped, block {size}"
        );
    }
}

// Spec: sampler R23
#[test]
fn retrig_under_poly_fades_a_slice_played_twice() {
    let collector = Collector::new();
    let notes = [(0, 60), (100, 60)];
    for size in BLOCK_SIZES {
        for (retrig, want) in [(0.0, 0.2), (1.0, 0.1)] {
            let mut node = slicer(&[(PLAYBACK, 1.0), (RETRIG, retrig)], &collector, size);
            let out = play_in_blocks(node.as_mut(), size, 600, &notes);
            assert!(
                (out[400] - want).abs() < 1e-6,
                "retrig {retrig}, block {size}: {}",
                out[400]
            );
        }
    }
}

// Spec: sampler R23
#[test]
fn thru_plays_a_held_first_slice_on_to_the_end_flag() {
    let collector = Collector::new();
    for size in BLOCK_SIZES {
        let mut mono = slicer(&[], &collector, size);
        let out = play_in_blocks(mono.as_mut(), size, 9600, &[(0, 60)]);
        assert!(
            out[1300..].iter().all(|s| *s == 0.0),
            "Mono ends with its slice, block {size}"
        );

        let mut thru = slicer(&[(PLAYBACK, 2.0)], &collector, size);
        let out = play_in_blocks(thru.as_mut(), size, 9600, &[(0, 60)]);
        assert!((out[600] - 0.1).abs() < 1e-6, "slice 0, block {size}");
        assert!(
            (out[1300] - 0.2).abs() < 1e-6,
            "on into slice 1, block {size}"
        );
        assert!(
            (out[9500] - 0.8).abs() < 1e-6,
            "on to the last slice, block {size}"
        );
    }
}

// Spec: sampler R23
#[test]
fn fade_out_under_thru_ends_at_the_regions_end() {
    let collector = Collector::new();
    for size in BLOCK_SIZES {
        // 20 ms of Fade Out: 960 frames, linear, ending at the End flag.
        let mut thru = slicer(&[(PLAYBACK, 2.0), (FADE_OUT, 20.0)], &collector, size);
        let out = play_in_blocks(thru.as_mut(), size, 9600, &[(0, 60)]);
        assert!(
            (out[9600 - 480] - 0.4).abs() < 1e-3,
            "half-way down at the end, block {size}"
        );
        assert!(
            (out[2000] - 0.2).abs() < 1e-6,
            "untouched before it, block {size}"
        );

        // Under Mono the fade ends at the slice's end instead.
        let mut mono = slicer(&[(FADE_OUT, 20.0)], &collector, size);
        let out = play_in_blocks(mono.as_mut(), size, 1200, &[(0, 60)]);
        assert!(
            (out[1200 - 480] - 0.05).abs() < 1e-3,
            "half-way down at the slice's end, block {size}"
        );
    }
}

// Spec: sampler R23
#[test]
fn a_set_saved_before_playback_plays_mono() {
    let collector = Collector::new();
    let device_type = sampler_type();
    let mut old = device(
        &device_type,
        &[(MODE, 2.0), (SLICE_BY, 2.0), (SLICE_REGIONS, 8.0)],
    );
    old.params.remove(&PLAYBACK);
    let mut node = node(&registry(), &old, 1200);
    give(node.as_mut(), &collector, &eight_slices());
    let out = play_in_blocks(node.as_mut(), 1200, 1200, &[(0, 60), (100, 61)]);
    assert!((out[400] - 0.2).abs() < 1e-6, "the first slice was cut");
}

/// Runs one block of `frames` frames through `node` on `transport`, with
/// `notes` and `events`, as a real-time section, and returns each channel.
fn run_block(
    node: &mut dyn Node,
    frames: usize,
    notes: &[NoteEvent],
    events: &[NodeEvent],
    transport: TransportInfo,
) -> Vec<Vec<f32>> {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut out = NoteBuffer::with_capacity(notes.len().max(1));
    {
        let mut block = Block::new(&mut data, CHANNELS, frames, frames, events, transport, RATE)
            .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

// Spec: sampler R18
#[test]
fn a_warped_note_held_while_stopped_keeps_moving_through_the_sample() {
    let ramp = sample(96_000, |i| (i + 1) as f32 / 96_000.0);
    let collector = Collector::new();
    let mut node = sampler(&[(ATTACK, 0.0)], 256);
    let mut slot = SlotSample::new(0, ramp);
    slot.warp.warp = true;
    slot.warp.bpm = Some(120.0);
    node.receive_data(collector.handle().data(slot));
    let at_120 = TempoMap::constant(Bpm::new(120.0).unwrap());
    let b1 = run_notes_at(
        node.as_mut(),
        256,
        &[note(0, NoteKind::On, 60, 1.0)],
        &at_120,
    );
    let b2 = run_notes_at(node.as_mut(), 256, &[], &at_120);
    // The ramp carries on into the second block; it once began again at the
    // note's start.
    assert!(
        b2[0][0] > b1[0][255],
        "block 2 began at {} after block 1 ended at {}",
        b2[0][0],
        b1[0][255]
    );
    assert!(b2[0][255] > b2[0][0]);
}

// Spec: sampler R18
#[test]
fn a_warped_note_goes_on_through_a_locate_back() {
    let ramp = sample(96_000, |i| (i + 1) as f32 / 96_000.0);
    let collector = Collector::new();
    let mut node = sampler(&[(ATTACK, 0.0)], 256);
    let mut slot = SlotSample::new(0, ramp);
    slot.warp.warp = true;
    slot.warp.bpm = Some(120.0);
    node.receive_data(collector.handle().data(slot));
    let on = [note(0, NoteKind::On, 60, 1.0)];
    let at = |frame| TransportInfo::at(true, Frames(frame));
    let b1 = run_block(node.as_mut(), 256, &on, &[], at(48_000));
    let b2 = run_block(node.as_mut(), 256, &[], &[], at(0));
    assert!(b2[0][0] > b1[0][255], "the note goes on after the locate");
    assert!(
        b2[0][1] > b2[0][0],
        "the note carries on, not back to its start"
    );
}

// Spec: sampler R18
#[test]
fn warped_pitch_envelope_never_reads_backwards() {
    let ramp = sample(192_000, |i| (i + 1) as f32 / 192_000.0);
    let collector = Collector::new();
    let mut node = sampler(
        &[
            (ATTACK, 0.0),
            (SUSTAIN, 100.0),
            (PITCH_ENV_AMOUNT, 48.0),
            (PITCH_ENV_ATTACK, 0.0),
            (PITCH_ENV_DECAY, 100.0),
            (PITCH_ENV_SUSTAIN, 0.0),
        ],
        256,
    );
    let mut slot = SlotSample::new(0, ramp);
    slot.warp.warp = true;
    slot.warp.mode = WarpMode::RePitch;
    slot.warp.bpm = Some(120.0);
    node.receive_data(collector.handle().data(slot));
    let mut out = Vec::new();
    for block in 0..57 {
        let notes = [note(0, NoteKind::On, 60, 1.0)];
        let transport = TransportInfo::at(true, Frames(block * 256));
        let notes: &[NoteEvent] = if block == 0 { &notes } else { &[] };
        out.extend(run_block(node.as_mut(), 256, notes, &[], transport).remove(0));
    }
    let first = out.iter().position(|v| *v != 0.0).unwrap();
    assert!(
        out[first..].windows(2).all(|w| w[1] >= w[0] - 1e-6),
        "a pitch envelope only speeds the read up or slows it down"
    );
}

// Spec: sampler R20
#[test]
fn manual_slices_merge_into_automatic_slicing() {
    use bl_devices::slice_boundaries;
    let clicks = sample(48_000, |i| if i % 24_000 < 20 { 1.0 } else { 0.0 });
    let value = |by: f64| {
        move |id: ParamId| match id {
            SLICE_BY => by,
            SLICE_SENSITIVITY => 50.0,
            SLICE_DIVISION => 4.0,
            SLICE_REGIONS => 2.0,
            _ => 0.0,
        }
    };
    let cut = |by: f64| slice_boundaries(&clicks, (0, 48_000), &value(by), 6_000.0, &[12_000]);
    assert_eq!(cut(2.0), [0, 12_000, 24_000, 48_000], "Region");
    assert_eq!(cut(0.0), [0, 12_000, 24_000, 48_000], "Transient");
    assert_eq!(cut(1.0), [0, 12_000, 24_000, 48_000], "Beat");
    // A full list still closes the region.
    let many: Vec<u64> = (1..100).map(|k| k * 400 + 1).collect();
    let full = slice_boundaries(&clicks, (0, 48_000), &value(0.0), 6_000.0, &many);
    assert_eq!(full.len(), 65);
    assert_eq!(full.last(), Some(&48_000));

    // The device plays the merged slices: key 61 starts at the manual one.
    let ramp = sample(48_000, |i| i as f32 / 48_000.0);
    let collector = Collector::new();
    let mut node = sampler(
        &[(MODE, 2.0), (SLICE_BY, 2.0), (SLICE_REGIONS, 2.0)],
        48_000,
    );
    let mut slot = SlotSample::new(0, ramp);
    slot.slices = Arc::from([12_000u64]);
    node.receive_data(collector.handle().data(slot));
    let out = run_notes(node.as_mut(), 48_000, &[note(0, NoteKind::On, 61, 1.0)]);
    let want = (12_000.0 + 10.0) / 48_000.0;
    assert!((out[0][10] as f64 - want).abs() < 1e-4, "{}", out[0][10]);
}

// Spec: sampler R14
#[test]
fn a_long_samples_transient_slices_are_found_over_several_blocks() {
    // A loud click at the start and the middle and a quiet one between:
    // the quiet one is an onset from Sensitivity 51 % up, not at 50 %.
    let quiet = 10f32.powf(-1.01);
    let long = sample(20_000_000, |i| match i {
        0 | 10_000_000 => 1.0,
        5_000_000 => quiet,
        _ => 0.0,
    });
    let collector = Collector::new();
    let mut node = sampler(&[(MODE, 2.0), (SLICE_SENSITIVITY, 50.0)], 128);
    give(node.as_mut(), &collector, &long);
    let on = [note(0, NoteKind::On, 61, 1.0)];
    let play = |node: &mut dyn Node, events: &[NodeEvent]| {
        run_block(node, 128, &on, events, TransportInfo::default())[0][0]
    };
    let idle = |node: &mut dyn Node, events: &[NodeEvent]| {
        run_block(node, 128, &[], events, TransportInfo::default());
    };
    // The first scan takes more than a block, and no slice plays until it
    // is done.
    assert_eq!(play(node.as_mut(), &[]), 0.0);
    for _ in 0..2_000 {
        idle(node.as_mut(), &[]);
    }
    let loud = play(node.as_mut(), &[]);
    assert!(loud > 0.0, "the second slice starts on the loud click");
    // A new Sensitivity doesn't stop the old slices playing, nor does it
    // rescan the sample in the block it arrives in.
    let sensitivity = [NodeEvent {
        offset: 0,
        param: SLICE_SENSITIVITY,
        value: ParamValue::Set(51.0),
    }];
    idle(node.as_mut(), &sensitivity);
    assert_eq!(play(node.as_mut(), &[]), loud, "the old slices still play");
    for _ in 0..2_000 {
        idle(node.as_mut(), &[]);
    }
    let now = play(node.as_mut(), &[]);
    assert!(
        (now / loud - quiet).abs() < 1e-3,
        "the second slice now starts on the quiet click"
    );
}
