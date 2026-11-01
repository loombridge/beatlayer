//! Granulator III, part 1 (WP-8.26; the Granulator III spec R1 to R17): the
//! three modes' grain onsets on frames, seeded randomness, the grain pool,
//! the window, the envelope, filter and Volume, MPE per note, and what the
//! display reads. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::id::Id;
use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::granulator_iii::{
    ATTACK, CLASSIC, CLOUD, DENSITY, DISPLAY_LEN, FILTER, FILTER_FREQ, LOOP, MODE, MODES, PAGES,
    POOL, POSITION, PRESSURE_SIZE, RATE, RELEASE, SHAPE, SIZE, SLIDE_AMOUNT, SPRAY, SUSTAIN,
    VARIATION, VOICES, VOLUME, WIDTH, display, granulator_iii_type,
};
use bl_engine::{
    Block, Collector, DisplayReader, Node, NoteBuffer, NoteEvent, NoteExpression, NoteKind,
    Registry, SlotSample, TransportInfo,
};
use bl_media::SampleData;
use bl_model::device_layout::LayoutNode;
use bl_model::{DeviceId, DeviceRole, OrdMap, ParamId};
use common::{CHANNELS, node};

const MAX_FRAMES: usize = 8_192;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::granulator_iii::register(&mut registry);
    registry
}

fn tempo() -> TempoMap {
    TempoMap::constant(Bpm::new(120.0).unwrap())
}

/// Audio of `frames` frames at the engine's rate, one channel.
fn source(frames: usize, value: impl Fn(usize) -> f32) -> Arc<SampleData> {
    Arc::new(SampleData::new(48_000, vec![(0..frames).map(value).collect()]).unwrap())
}

/// One sample of 1 at `at`: a grain that reads it past its first
/// millisecond plays it at the grain's start plus `at`, at the window's top.
fn impulse(frames: usize, at: usize) -> Arc<SampleData> {
    source(frames, |n| if n == at { 1.0 } else { 0.0 })
}

/// Where in a grain [`impulse`] is read: past the 1 ms fade of the flat
/// window.
const AT: usize = 100;

/// A Granulator III with `params`, given `sample`, and the reader of its
/// display.
fn granulator(
    params: &[(ParamId, f64)],
    sample: Option<&Arc<SampleData>>,
) -> (Box<dyn Node>, DisplayReader) {
    let mut device = granulator_iii_type().device(DeviceId(Id::new(1).unwrap()));
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    let mut node = node(&registry(), &device, MAX_FRAMES);
    if let Some(sample) = sample {
        node.receive_data(
            Collector::new()
                .handle()
                .data(SlotSample::new(0, Arc::clone(sample))),
        );
    }
    let reader = node.display().expect("Granulator III has a display");
    (node, reader)
}

fn note(offset: u32, kind: NoteKind, key: u8, id: u32) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key,
        velocity: 1.0,
        note_id: Some(id),
    }
}

fn on(offset: u32, key: u8) -> NoteEvent {
    note(offset, NoteKind::On, key, u32::from(key))
}

fn off(offset: u32, key: u8) -> NoteEvent {
    note(offset, NoteKind::Off, key, u32::from(key))
}

fn expression(offset: u32, key: u8, expression: NoteExpression, value: f32) -> NoteEvent {
    note(
        offset,
        NoteKind::Expression { expression, value },
        key,
        u32::from(key),
    )
}

/// How a render is played: the block size, the song's position if it
/// plays, and the display's last data.
struct Play {
    block: usize,
    song_from: Option<i64>,
}

const STOPPED: Play = Play {
    block: 512,
    song_from: None,
};

impl Play {
    fn blocks(block: usize) -> Self {
        Self { block, ..STOPPED }
    }

    fn song(block: usize, from: i64) -> Self {
        Self {
            block,
            song_from: Some(from),
        }
    }
}

/// Plays `notes` (frames from the render's start) through `node` for
/// `frames`, `play.block` frames at a time, and returns both channels and
/// the display's data after the last block.
fn render(
    (node, reader): &mut (Box<dyn Node>, DisplayReader),
    play: &Play,
    frames: usize,
    notes: &[NoteEvent],
) -> ([Vec<f32>; 2], Vec<f32>) {
    let tempo = tempo();
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut done = 0;
    while done < frames {
        let length = play.block.min(frames - done);
        let here: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= done && (n.offset as usize) < done + length)
            .map(|n| NoteEvent {
                offset: n.offset - done as u32,
                ..*n
            })
            .collect();
        let mut data = vec![0.0f32; CHANNELS * length];
        let mut sent = NoteBuffer::with_capacity(here.len().max(1));
        let transport = match play.song_from {
            Some(from) => TransportInfo::at(true, Frames(from + done as i64)),
            None => TransportInfo::default(),
        };
        {
            let mut block = Block::new(
                &mut data,
                CHANNELS,
                length,
                length,
                &[],
                transport,
                common::RATE,
            )
            .with_notes(&here, &mut sent)
            .with_tempo(&tempo);
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, chunk) in out.iter_mut().zip(data.chunks(length)) {
            channel.extend_from_slice(chunk);
        }
        done += length;
    }
    (out, reader.read().values.clone())
}

/// The frames a stream of grains of [`impulse`] start on: where the output
/// first rises past `level` after being below it for a while, less
/// [`AT`].
fn onsets(samples: &[f32], level: f32) -> Vec<usize> {
    let mut found = Vec::new();
    for (n, s) in samples.iter().enumerate() {
        if s.abs() > level && (n == 0 || samples[n - 1].abs() <= level) {
            found.push(n.saturating_sub(AT));
        }
    }
    found
}

fn near(a: usize, b: usize) -> bool {
    a.abs_diff(b) <= 1
}

// Spec: granulator iii R1, R15
#[test]
fn it_is_an_instrument_named_for_live_with_the_display_and_three_pages() {
    let device_type = granulator_iii_type();
    assert_eq!(device_type.name, "Granulator III");
    assert_eq!(device_type.role, DeviceRole::Instrument);
    device_type.description.check().unwrap();
    for param in &device_type.description.params {
        assert!(param.automatable, "{}", param.name);
    }
    // Spec: granulator iii R3, R5, R12. The defaults.
    let default = |id: ParamId| device_type.description.param(id).unwrap().default;
    assert_eq!(default(MODE), CLASSIC as f64);
    assert_eq!(MODES, ["Classic", "Loop", "Cloud"]);
    assert_eq!(
        (default(POSITION), default(SIZE), default(SHAPE)),
        (0.0, 100.0, 50.0)
    );
    assert_eq!((default(DENSITY), default(VOICES)), (4.0, 8.0));
    // Spec: granulator iii R17. The display is outside the pages, so it
    // is always shown.
    let LayoutNode::Row(parts) = &device_type.description.layout.root else {
        panic!("a row of the display and the pages");
    };
    assert!(matches!(parts[0], LayoutNode::Cell(_)));
    let pages = device_type.description.layout.pages();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].name, PAGES);
    let names: Vec<_> = pages[0].pages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Grain", "Envelope and Filter", "MPE"]);
}

// Spec: granulator iii R2
#[test]
fn without_a_sample_it_is_silent_and_with_one_a_note_plays_it_at_its_own_pitch_at_c3() {
    let notes = [on(0, 60)];
    let mut none = granulator(&[(SIZE, 100.0)], None);
    let (out, _) = render(&mut none, &STOPPED, 4_800, &notes);
    assert!(out[0].iter().chain(&out[1]).all(|s| *s == 0.0));
    // A grain of the ramp at C3 reads the source at 1x, and an octave up at 2x.
    let ramp = source(48_000, |n| n as f32 / 48_000.0);
    let params = [(DENSITY, 1.0), (SHAPE, 0.0), (SIZE, 100.0)];
    let at = |key: u8| {
        let mut node = granulator(&params, Some(&ramp));
        let (out, _) = render(&mut node, &STOPPED, 3_000, &[on(0, key)]);
        out[0][2_000]
    };
    assert!(
        (at(60) - 2_000.0 / 48_000.0).abs() < 1e-4,
        "C3 plays at 1x: {}",
        at(60)
    );
    assert!(
        (at(72) - 4_000.0 / 48_000.0).abs() < 1e-4,
        "C4 plays at 2x: {}",
        at(72)
    );
}

// Spec: granulator iii R7
#[test]
fn classic_starts_a_grain_every_size_over_density_from_the_notes_frame() {
    let click = impulse(48_000, AT);
    let params = [(SHAPE, 0.0), (SIZE, 100.0), (DENSITY, 4.0), (ATTACK, 0.0)];
    let mut node = granulator(&params, Some(&click));
    let (out, _) = render(&mut node, &STOPPED, 9_000, &[on(50, 60)]);
    let found = onsets(&out[0], 0.25);
    assert!(found.len() >= 7, "{found:?}");
    for (k, at) in found.iter().enumerate() {
        assert!(near(*at, 50 + 1_200 * k), "grain {k} starts on {at}");
    }
    // The same frames at any block size, and run after run.
    for block in [1, 64, 512, 4_096] {
        let mut again = granulator(&params, Some(&click));
        let (same, _) = render(&mut again, &Play::blocks(block), 9_000, &[on(50, 60)]);
        assert_eq!(same, out, "block size {block}");
    }
}

// Spec: granulator iii R8
#[test]
fn loop_starts_grains_on_the_songs_grid_at_any_block_size() {
    let click = impulse(48_000, AT);
    // 120 BPM at 48 kHz: a beat is 24,000 frames and 1/16 of a bar's note
    // (Rate 1/16) 6,000.
    let params = [
        (MODE, LOOP as f64),
        (RATE, 2.0),
        (SHAPE, 0.0),
        (SIZE, 20.0),
        (ATTACK, 0.0),
    ];
    let play = |block: usize, from: i64| {
        let mut node = granulator(&params, Some(&click));
        render(&mut node, &Play::song(block, from), 30_000, &[on(0, 60)]).0
    };
    let at_start = play(512, 1_000);
    let found: Vec<_> = onsets(&at_start[0], 0.5)
        .iter()
        .map(|f| f + 1_000)
        .collect();
    assert_eq!(found.len(), 5, "{found:?}");
    for (k, song_frame) in found.iter().enumerate() {
        assert!(
            near(*song_frame, 6_000 * (k + 1)),
            "grid line {k} on {song_frame}"
        );
    }
    for block in [1, 64, 4_096] {
        assert_eq!(play(block, 1_000), at_start, "block size {block}");
    }
    // With the transport stopped, the grid runs from the note's frame.
    let mut node = granulator(&params, Some(&click));
    let (out, _) = render(&mut node, &STOPPED, 20_000, &[on(300, 60)]);
    let found = onsets(&out[0], 0.5);
    assert!(found.len() >= 3, "{found:?}");
    for (k, at) in found.iter().enumerate() {
        assert!(near(*at, 300 + 6_000 * k), "grain {k} on {at}");
    }
}

// Spec: granulator iii R9, R10
#[test]
fn cloud_scatters_grains_from_a_seed_that_repeats_with_each_note_and_every_run() {
    let click = impulse(48_000, AT);
    let params = [
        (MODE, CLOUD as f64),
        (SHAPE, 0.0),
        (SIZE, 20.0),
        (DENSITY, 4.0),
        (WIDTH, 100.0),
        (VARIATION, 7.0),
        (SPRAY, 30.0),
    ];
    let run = |block: usize| {
        let mut node = granulator(&params, Some(&click));
        render(&mut node, &Play::blocks(block), 96_000, &[on(0, 60)]).0
    };
    let first = run(512);
    // About Density × 10 grains a second: 80 in two seconds.
    let count = onsets(&first[0], 0.0001)
        .len()
        .max(onsets(&first[1], 0.0001).len());
    assert!((40..160).contains(&count), "{count} grains");
    assert_eq!(run(512), first, "run after run");
    for block in [1, 64, 4_096] {
        // Cloud's grains start on the same frames at any block size.
        let other = run(block);
        assert_eq!(other, first, "block size {block}");
    }
    // The two channels differ: the grains are panned within Width.
    assert_ne!(first[0], first[1]);
    // A second note of the same key draws the same stream from its own frame.
    let mut node = granulator(&params, Some(&click));
    let ([left, _], _) = render(
        &mut node,
        &STOPPED,
        96_000,
        &[on(0, 60), off(20_000, 60), on(40_000, 60)],
    );
    let second: Vec<f32> = left[40_000..40_000 + 20_000].to_vec();
    let again: Vec<f32> = first[0][..20_000].to_vec();
    // Release overlap aside, the second note's grains follow the first's.
    assert_eq!(onsets(&second, 0.0001)[..3], onsets(&again, 0.0001)[..3]);
}

// Spec: granulator iii R5
#[test]
fn size_shape_and_density_change_the_output_as_specified() {
    let dc = source(48_000, |_| 1.0);
    let peak_at = |params: &[(ParamId, f64)]| {
        let mut node = granulator(params, Some(&dc));
        let (out, _) = render(&mut node, &STOPPED, 4_800, &[on(0, 60)]);
        let top = out[0][..4_000].iter().cloned().fold(0.0f32, f32::max);
        let at = out[0].iter().position(|s| *s == top).unwrap();
        (at, top, out[0].clone())
    };
    // Shape 100 %: a narrow bell whose top is the middle of the grain, so
    // Size is where it sits.
    let (short, _, _) = peak_at(&[(DENSITY, 1.0), (SHAPE, 100.0), (SIZE, 20.0)]);
    let (long, _, _) = peak_at(&[(DENSITY, 1.0), (SHAPE, 100.0), (SIZE, 50.0)]);
    assert!(short.abs_diff(480) <= 2, "size 20 ms peaks at {short}");
    assert!(long.abs_diff(1_200) <= 2, "size 50 ms peaks at {long}");
    // Shape 0: a flat window, full to within its 1 ms fades; Hann at 50 %
    // is lower a quarter of the way in; the bell lower still.
    let quarter = |shape: f64| {
        let (_, _, out) = peak_at(&[(DENSITY, 1.0), (SHAPE, shape), (SIZE, 50.0)]);
        out[600]
    };
    let (flat, hann, bell) = (quarter(0.0), quarter(50.0), quarter(100.0));
    assert!((flat - 1.0).abs() < 1e-3, "flat: {flat}");
    assert!((hann - 0.5).abs() < 1e-2, "Hann at a quarter: {hann}");
    assert!(bell < 0.2, "bell: {bell}");
    // Density: more overlap starts grains closer together.
    let spacing = |density: f64| {
        let click = impulse(48_000, AT);
        let mut node = granulator(
            &[(SHAPE, 0.0), (SIZE, 100.0), (DENSITY, density)],
            Some(&click),
        );
        let (out, _) = render(&mut node, &STOPPED, 12_000, &[on(0, 60)]);
        let found = onsets(&out[0], 0.05);
        found[1] - found[0]
    };
    assert!(spacing(2.0).abs_diff(2_400) <= 1);
    assert!(spacing(8.0).abs_diff(600) <= 1);
    // Position: where grains start in the source.
    let ramp = source(48_000, |n| n as f32 / 48_000.0);
    let start = |position: f64| {
        let mut node = granulator(
            &[(POSITION, position), (SHAPE, 0.0), (DENSITY, 1.0)],
            Some(&ramp),
        );
        let (out, _) = render(&mut node, &STOPPED, 3_000, &[on(0, 60)]);
        out[0][2_000]
    };
    assert!((start(50.0) - start(0.0) - 0.5).abs() < 1e-3);
}

// Spec: granulator iii R4
#[test]
fn a_full_pool_drops_grains_and_counts_them_and_the_display_reports_the_count() {
    let dc = source(48_000, |_| 1.0);
    // 160 grains a second of a second each: far past the pool's 64.
    let params = [(MODE, CLOUD as f64), (SIZE, 1_000.0), (DENSITY, 16.0)];
    let mut node = granulator(&params, Some(&dc));
    let (out, data) = render(&mut node, &STOPPED, 96_000, &[on(0, 60)]);
    assert_eq!(data.len(), DISPLAY_LEN);
    assert!(data[0] > 0.0, "grains were dropped: {}", data[0]);
    assert_eq!(data[1] as usize, POOL, "the pool is full");
    assert!(out[0].iter().all(|s| s.is_finite()));
    // Within the pool, nothing is dropped.
    let mut node = granulator(&[(SIZE, 100.0), (DENSITY, 4.0)], Some(&dc));
    let (_, data) = render(&mut node, &STOPPED, 24_000, &[on(0, 60)]);
    assert_eq!(data[0], 0.0);
}

// Spec: granulator iii R13
#[test]
fn mpe_slide_pressure_and_per_note_pitch_move_one_notes_grains_alone() {
    let ramp = source(48_000, |n| n as f32 / 48_000.0);
    let click = impulse(48_000, 2_000);
    let params = [
        (DENSITY, 1.0),
        (SHAPE, 0.0),
        (SIZE, 200.0),
        (SLIDE_AMOUNT, 100.0),
        (PRESSURE_SIZE, 100.0),
    ];
    let play = |sample: &Arc<SampleData>, notes: &[NoteEvent]| {
        let mut node = granulator(&params, Some(sample));
        render(&mut node, &STOPPED, 12_000, notes).0
    };
    let sum = |a: &[Vec<f32>; 2], b: &[Vec<f32>; 2]| -> Vec<f32> {
        a[0].iter().zip(&b[0]).map(|(x, y)| x + y).collect()
    };
    for (what, sample, moving) in [
        (
            "slide",
            &ramp,
            expression(0, 60, NoteExpression::Slide, 1.0),
        ),
        (
            "pressure",
            &ramp,
            expression(0, 60, NoteExpression::Pressure, 1.0),
        ),
        (
            "pitch",
            &click,
            expression(0, 60, NoteExpression::Bend, 0.25),
        ),
    ] {
        let both = play(sample, &[on(0, 60), on(0, 67), moving]);
        let moved = play(sample, &[on(0, 60), moving]);
        let still = play(sample, &[on(0, 67)]);
        let unmoved = play(sample, &[on(0, 60)]);
        assert_ne!(moved, unmoved, "{what} changes its note");
        let want = sum(&moved, &still);
        let got = &both[0];
        assert!(
            got.iter().zip(&want).all(|(g, w)| (g - w).abs() < 1e-5),
            "{what} moves one note alone"
        );
    }
}

// Spec: granulator iii R11
#[test]
fn the_envelope_filter_and_volume_shape_the_output() {
    let dc = source(48_000, |_| 1.0);
    let params = [(SHAPE, 0.0), (DENSITY, 1.0), (SIZE, 100.0)];
    let run = |extra: &[(ParamId, f64)], notes: &[NoteEvent]| {
        let all: Vec<_> = params.iter().chain(extra).copied().collect();
        let mut node = granulator(&all, Some(&dc));
        render(&mut node, &STOPPED, 24_000, notes).0
    };
    let level = |s: &[f32]| s.iter().map(|x| x.abs()).fold(0.0, f32::max);
    // Attack: 100 ms to rise.
    let slow = run(&[(ATTACK, 100.0)], &[on(0, 60)]);
    assert!(
        slow[0][480] < 0.2 * slow[0][4_700],
        "{} {}",
        slow[0][480],
        slow[0][4_700]
    );
    // Release: the note's end fades out over the release.
    let ended = run(
        &[(RELEASE, 100.0), (ATTACK, 0.0)],
        &[on(0, 60), off(9_600, 60)],
    );
    assert!(level(&ended[0][9_700..10_000]) > 0.5 && level(&ended[0][16_000..]) < 1e-3);
    // Sustain at 50 % holds half.
    let held = run(
        &[
            (ATTACK, 0.0),
            (SUSTAIN, 50.0),
            (bl_devices::granulator_iii::DECAY, 10.0),
        ],
        &[on(0, 60)],
    );
    assert!((held[0][12_000] - 0.5).abs() < 0.05, "{}", held[0][12_000]);
    // Volume at -inf is silent.
    assert_eq!(level(&run(&[(VOLUME, 0.0)], &[on(0, 60)])[0]), 0.0);
    // A low-pass at 100 Hz takes a DC-free noise source nearly out.
    let noise = source(48_000, |n| if n % 2 == 0 { 1.0 } else { -1.0 });
    let filtered = |on_: f64| {
        let mut node = granulator(
            &[(SHAPE, 0.0), (FILTER, on_), (FILTER_FREQ, 100.0)],
            Some(&noise),
        );
        let (out, _) = render(&mut node, &STOPPED, 12_000, &[on(0, 60)]);
        level(&out[0][6_000..])
    };
    assert!(
        filtered(1.0) < 0.1 * filtered(0.0),
        "{} {}",
        filtered(1.0),
        filtered(0.0)
    );
    // Voices: two notes sound together up to Voices, and Voices 1 plays one.
    let two = |voices: f64| {
        let mut node = granulator(&[(VOICES, voices), (SHAPE, 0.0), (DENSITY, 1.0)], Some(&dc));
        let (out, _) = render(&mut node, &STOPPED, 4_800, &[on(0, 60), on(0, 64)]);
        out[0][3_000]
    };
    assert!(
        (two(8.0) - 2.0 * two(1.0)).abs() < 0.05,
        "{} {}",
        two(8.0),
        two(1.0)
    );
}

// Spec: granulator iii R14
#[test]
fn the_display_reads_the_waveform_position_spray_and_each_grain_headless() {
    let description = granulator_iii_type().description;
    let id = DeviceId(Id::new(1).unwrap());
    let mut values = OrdMap::new();
    values.insert(POSITION, 25.0);
    values.insert(SPRAY, 50.0);
    let shown = display(id, &description, &values, None);
    assert!(!shown.live);
    assert_eq!(shown.markers.len(), 1);
    assert_eq!(shown.markers[0].name, "Position");
    assert_eq!(shown.markers[0].x, 25.0);
    // From an engine: a one-second source with a loud first half.
    let half = source(48_000, |n| if n < 24_000 { 1.0 } else { 0.1 });
    let mut node = granulator(
        &[
            (SIZE, 100.0),
            (DENSITY, 4.0),
            (POSITION, 25.0),
            (SPRAY, 50.0),
        ],
        Some(&half),
    );
    let (_, data) = render(&mut node, &STOPPED, 9_000, &[on(0, 60)]);
    let live = display(id, &description, &values, Some(&data));
    assert!(live.live);
    let waveform = live.series("Waveform").unwrap();
    assert_eq!(waveform.values.len(), bl_devices::granulator_iii::PEAKS);
    assert!(waveform.values[10] > 0.9 && waveform.values[100] < 0.2);
    let names: Vec<_> = live.markers.iter().map(|m| m.name.as_str()).collect();
    assert!(
        names.contains(&"Spray Start") && names.contains(&"Spray End"),
        "{names:?}"
    );
    let grains = names.iter().filter(|n| n.starts_with("Grain")).count();
    assert!(grains > 0 && grains == data[1] as usize);
    assert_eq!(live.reading("Source Length").unwrap().text, "1.00 s");
    assert_eq!(live.reading("Dropped Grains").unwrap().text, "0");
}

// Spec: granulator iii R4, R8, R10
#[test]
fn a_stream_that_waited_for_a_sample_or_the_song_starts_where_it_is_not_with_a_burst_of_grains() {
    let dc = source(48_000, |_| 1.0);
    let busy = [(SIZE, 100.0), (DENSITY, 16.0)];
    // A note held for two seconds before its sample arrives.
    let mut node = granulator(&busy, None);
    render(&mut node, &STOPPED, 96_000, &[on(0, 60)]);
    node.0.receive_data(
        Collector::new()
            .handle()
            .data(SlotSample::new(0, Arc::clone(&dc))),
    );
    let (_, data) = render(&mut node, &STOPPED, 2_000, &[]);
    assert_eq!(data[0], 0.0, "no grain was dropped");
    // 2,000 frames at one grain every 300: seven, not the 320 it missed.
    assert!(data[1] <= 8.0, "{} grains at once", data[1]);
    // A Loop note held while the song plays, then the transport stops.
    let params = [(MODE, LOOP as f64), (RATE, 0.0), (SIZE, 100.0)];
    let mut node = granulator(&params, Some(&dc));
    render(&mut node, &Play::song(512, 0), 96_000, &[on(0, 60)]);
    let (_, data) = render(&mut node, &STOPPED, 2_000, &[]);
    assert_eq!(data[0], 0.0, "no grain was dropped");
}
