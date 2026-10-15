//! Snapshot, the native sampler (WP-2.16, part 1; the sampler spec):
//! playing a sample at the right pitch and length, the region and its
//! loop's crossfade, the envelope's times, velocity and pan, glide and
//! portamento, One-Shot, voice stealing, and the sample the engine gets
//! from the set. Every block runs as a real-time section under the
//! allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_devices::sampler::{
    ATTACK, DECAY, FADE, FADE_OUT, GAIN, GLIDE, GLIDE_MODE, GLIDE_TIME, LENGTH, LOOP, MODE, PAN,
    RELEASE, SAMPLER, START, SUSTAIN, TRIGGER, VELOCITY, VOICES, VOLUME, sampler_type,
};
use bl_engine::{
    Block, Collector, DeviceType, EngineConfig, MediaMap, Node, NodeEvent, NoteBuffer, NoteEvent,
    NoteKind, Offline, ParamValue, Registry, TransportCommand, TransportInfo,
};
use bl_model::param::db_to_gain;
use bl_model::{
    Clip, ClipData, Command, Device, DeviceKind, DeviceRole, Editor, MidiData, Note, ParamId,
    TrackId, TrackKind,
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
    node.receive_data(collector.handle().data(Arc::clone(data)));
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

// Spec: sampler R1
#[test]
fn the_sampler_is_described_and_laid_out() {
    let device_type = sampler_type();
    assert_eq!(device_type.name, "Snapshot");
    assert_eq!(device_type.role, DeviceRole::Instrument);
    // Every parameter is described once and placed in the layout.
    device_type.description.check().unwrap();
    assert!(device_type.description.params.len() >= 20);
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
        "Snapshot",
        DeviceKind::Native {
            device_type: SAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.media = Some(hash);
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
