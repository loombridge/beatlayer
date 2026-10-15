//! Rotunda, the algorithmic reverb (WP-2.18b, part 1; the reverb spec):
//! the input filter, the early reflections with Spin and Shape, the
//! diffusion network with its damping, Chorus, Predelay, Size, Decay
//! (RT60), Freeze with Flat and Cut, the Stereo width, and the Reflect,
//! Diffuse and Dry/Wet output controls.

bl_rt::install_trap!();

mod common;

use bl_devices::reverb::{
    CHORUS, CHORUS_AMOUNT, CHORUS_RATE, CUT, DECAY, DENSITY, DIFFUSE, DIFFUSION, DRY_WET, FLAT,
    FREEZE, HIGH, HIGH_CUT, HIGH_CUT_HZ, HIGH_DAMP, LOW, LOW_CUT, LOW_CUT_HZ, LOW_DAMP, PREDELAY,
    REFLECT, SCALE, SHAPE, SIZE, SMOOTH, SPIN, SPIN_AMOUNT, SPIN_RATE, STEREO, reverb_type,
};
use bl_engine::{Block, DeviceType, Node, NodeEvent, Registry, TransportInfo};
use bl_model::ParamId;

use common::{RATE, device, node, noise, set};

/// A registry with the reverb alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::reverb::register(&mut registry);
    registry
}

/// A Rotunda of `params`, up to `max_frames` at a time.
fn reverb(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = reverb_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// Runs `left` and `right` through `node` as one block with `events`, under
/// the trap.
fn run_stereo_with(
    node: &mut dyn Node,
    left: &[f32],
    right: &[f32],
    events: &[NodeEvent],
) -> [Vec<f32>; 2] {
    let frames = left.len();
    let mut data: Vec<f32> = left.iter().chain(right.iter()).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            2,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    let (l, r) = data.split_at(frames);
    [l.to_vec(), r.to_vec()]
}

/// The same, without events.
fn run_stereo(node: &mut dyn Node, left: &[f32], right: &[f32]) -> [Vec<f32>; 2] {
    run_stereo_with(node, left, right, &[])
}

/// Runs `input` (the same on both channels) through `node` in `block`-sized
/// blocks and returns both channels.
fn stream(node: &mut dyn Node, input: &[f32], block: usize) -> [Vec<f32>; 2] {
    let mut out = [
        Vec::with_capacity(input.len()),
        Vec::with_capacity(input.len()),
    ];
    for chunk in input.chunks(block) {
        let [l, r] = run_stereo(node, chunk, chunk);
        out[0].extend_from_slice(&l);
        out[1].extend_from_slice(&r);
    }
    out
}

/// The same, with an impulse at the first frame.
fn impulse_response(params: &[(ParamId, f64)], frames: usize, block: usize) -> [Vec<f32>; 2] {
    let mut input = vec![0.0f32; frames];
    input[0] = 1.0;
    stream(reverb(params, block).as_mut(), &input, block)
}

/// The settings every measurement starts from: no modulation, no damping,
/// no predelay, fully wet, everything audible.
fn with(more: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = vec![
        (SPIN, 0.0),
        (CHORUS, 0.0),
        (HIGH_DAMP, 0.0),
        (LOW_DAMP, 0.0),
        (FREEZE, 0.0),
        (PREDELAY, 0.0),
        (REFLECT, 100.0),
        (DIFFUSE, 100.0),
        (DRY_WET, 100.0),
    ];
    for (id, value) in more {
        params.retain(|(existing, _)| existing != id);
        params.push((*id, *value));
    }
    params
}

/// The sum of the squares of `samples` in the frames `from` to `to`.
fn energy(samples: &[f32], from: usize, to: usize) -> f64 {
    samples[from.min(samples.len())..to.min(samples.len())]
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum()
}

/// The first frame whose sample rises above `threshold`.
fn arrival(samples: &[f32], threshold: f32) -> usize {
    samples
        .iter()
        .position(|s| s.abs() > threshold)
        .expect("something arrives")
}

// Spec: reverb R9
#[test]
fn decay_time_matches_the_setting_at_three_settings() {
    for decay in [0.5, 2.0, 5.0] {
        let params = with(&[
            (DECAY, decay),
            (REFLECT, 0.0),
            (DIFFUSE, 100.0),
            (SCALE, 0.0),
        ]);
        let frames = (decay * 1.5 * RATE.as_f64()) as usize;
        let [left, _] = impulse_response(&params, frames, 512);
        // Schroeder's energy decay curve: EDC[n] is the energy left after n.
        let mut edc = vec![0.0f64; frames + 1];
        for n in (0..frames).rev() {
            let sample = f64::from(left[n]);
            edc[n] = edc[n + 1] + sample * sample;
        }
        assert!(edc[0] > 0.0, "the impulse made a tail");
        let threshold = edc[0] * 1e-6;
        let measured = edc
            .iter()
            .position(|e| *e <= threshold)
            .expect("the tail falls by 60 dB") as f64
            / RATE.as_f64();
        assert!(
            (measured - decay).abs() <= 0.1 * decay,
            "RT60 {measured:.3} s against {decay} s"
        );
    }
}

// Spec: reverb R10
#[test]
fn freeze_holds_the_tail_without_growing() {
    let params = with(&[(DECAY, 1.0), (FLAT, 1.0), (CUT, 1.0), (REFLECT, 0.0)]);
    let mut node = reverb(&params, 4_800);
    let burst = noise(4_800);
    let _ = run_stereo(node.as_mut(), &burst, &burst);
    let silent = vec![0.0f32; 4_800];
    let mut frozen = Vec::new();
    for block in 0..30 {
        let events = if block == 0 {
            vec![set(0, FREEZE, 1.0)]
        } else {
            vec![]
        };
        let [left, right] = run_stereo_with(node.as_mut(), &silent, &silent, &events);
        frozen.push(energy(&left, 0, 4_800) + energy(&right, 0, 4_800));
    }
    assert!(frozen[0] > 1e-6, "the burst left a tail");
    // The first frozen block still takes in the burst's delayed flight, so
    // the steady state it settles to is the one to hold: it may not grow
    // and it may not decay.
    let steady = frozen[1];
    for (block, e) in frozen.iter().enumerate().skip(1) {
        assert!(
            *e <= steady * 1.1,
            "the frozen tail does not grow in block {block}: {e} against {steady}"
        );
        assert!(
            *e >= steady * 0.9,
            "the frozen tail holds in block {block}: {e} against {steady}"
        );
    }
    // Without Freeze the same tail decays away: 60 dB in a second.
    let silent = vec![0.0f32; 4_800 * 16];
    let mut plain = reverb(&params, 4_800);
    let _ = run_stereo(plain.as_mut(), &burst, &burst);
    let [left, right] = stream(plain.as_mut(), &silent, 4_800);
    let ended = energy(&left, 4_800 * 15, 4_800 * 16) + energy(&right, 4_800 * 15, 4_800 * 16);
    assert!(ended < steady * 1e-3, "unfrozen it decays: {ended}");
}

// Spec: reverb R10
#[test]
fn flat_and_cut_decide_what_a_freeze_keeps() {
    let base = with(&[(DECAY, 1.0), (REFLECT, 0.0), (FLAT, 1.0), (CUT, 1.0)]);
    // A small impulse, then Freeze, then a loud block: with Cut on the block
    // never reaches the frozen network; with Cut off it adds to it.
    let added = |cut: f64| {
        let mut all = base.clone();
        all.push((CUT, cut));
        let mut node = reverb(&with(&all), 4_800);
        let mut impulse = vec![0.0f32; 4_800];
        impulse[0] = 1.0;
        let _ = run_stereo(node.as_mut(), &impulse, &impulse);
        let silent = vec![0.0f32; 4_800];
        let _ = run_stereo_with(node.as_mut(), &silent, &silent, &[set(0, FREEZE, 1.0)]);
        let _ = run_stereo(node.as_mut(), &silent, &silent);
        let loud = noise(4_800);
        let [left, right] = run_stereo(node.as_mut(), &loud, &loud);
        energy(&left, 0, 4_800) + energy(&right, 0, 4_800)
    };
    let shut = added(1.0);
    let open = added(0.0);
    assert!(
        open > shut * 2.0,
        "with Cut off the input adds to the frozen tail: {open} against {shut}"
    );
    // Flat bypasses the damping while frozen: with High damping at 100 % the
    // tail loses its high energy when Flat is off and holds it when on.
    let held = |flat: f64| {
        let mut all = base.clone();
        all.extend([
            (FLAT, flat),
            (HIGH_DAMP, 1.0),
            (HIGH, 100.0),
            (DIFFUSE, 100.0),
        ]);
        let mut node = reverb(&with(&all), 4_800);
        let burst = noise(4_800);
        let _ = run_stereo(node.as_mut(), &burst, &burst);
        let silent = vec![0.0f32; 4_800];
        let mut tail = Vec::new();
        for block in 0..16 {
            let events = if block == 0 {
                vec![set(0, FREEZE, 1.0)]
            } else {
                vec![]
            };
            let [left, right] = run_stereo_with(node.as_mut(), &silent, &silent, &events);
            tail.push(energy(&left, 0, 4_800) + energy(&right, 0, 4_800));
        }
        (tail[1], tail[15])
    };
    let (flat_start, flat_end) = held(1.0);
    let (damped_start, damped_end) = held(0.0);
    assert!(
        flat_end >= flat_start * 0.9,
        "with Flat the damped bands keep their energy: {flat_end} against {flat_start}"
    );
    assert!(
        damped_end < damped_start * 0.8,
        "without Flat the damped bands lose energy: {damped_end} against {damped_start}"
    );
}

// Spec: reverb R15
#[test]
fn the_impulse_response_is_finite_and_ends_in_silence() {
    let params = with(&[(DECAY, 0.2), (DIFFUSE, 100.0)]);
    let frames = 48_000 * 6;
    let [left, right] = impulse_response(&params, frames, 512);
    for (channel, samples) in [("left", &left), ("right", &right)] {
        for (index, sample) in samples.iter().enumerate() {
            assert!(sample.is_finite(), "{channel} finite at {index}: {sample}");
            assert!(sample.abs() < 4.0, "{channel} bounded at {index}: {sample}");
        }
        assert!(
            samples.iter().any(|s| s.abs() > 0.01),
            "{channel}: the reverb is heard"
        );
        assert!(
            samples[48_000 * 4..].iter().all(|s| *s == 0.0),
            "{channel}: the tail flushes to exact silence, with no denormals left"
        );
    }
}

// Spec: reverb R3
#[test]
fn spin_moves_the_early_reflections() {
    let off = with(&[(DIFFUSE, 0.0)]);
    let [plain, _] = impulse_response(&off, 24_000, 512);
    let still = with(&[(DIFFUSE, 0.0), (SPIN, 1.0), (SPIN_AMOUNT, 0.0)]);
    let [same, _] = impulse_response(&still, 24_000, 512);
    assert!(plain == same, "Spin at 0 moves nothing");
    let moving = with(&[
        (DIFFUSE, 0.0),
        (SPIN, 1.0),
        (SPIN_AMOUNT, 100.0),
        (SPIN_RATE, 5.0),
    ]);
    let [moved, _] = impulse_response(&moving, 24_000, 512);
    assert!(
        plain.iter().zip(&moved).any(|(a, b)| (a - b).abs() > 1e-4),
        "Spin moves the reflections"
    );
}

// Spec: reverb R2
#[test]
fn shape_sets_how_the_early_reflections_decay() {
    let early = |shape: f64| {
        let params = with(&[(DIFFUSE, 0.0), (SHAPE, shape)]);
        let [left, _] = impulse_response(&params, 24_000, 512);
        energy(&left, 500, 1_100)
    };
    let gradual = early(0.0);
    let rapid = early(100.0);
    eprintln!("shape gradual {gradual} rapid {rapid}");
    assert!(gradual > 0.0, "the later reflections are heard");
    assert!(
        gradual > rapid * 4.0,
        "a small Shape decays more gradually: {gradual} against {rapid}"
    );
}

// Spec: reverb R4
#[test]
fn diffusion_and_scale_shape_the_tail() {
    let params = with(&[(REFLECT, 0.0), (DECAY, 0.5)]);
    let response = |more: &[(ParamId, f64)]| {
        let mut all = params.clone();
        all.extend_from_slice(more);
        let all = with(&all);
        let [left, _] = impulse_response(&all, 24_000, 512);
        left
    };
    let sparse = response(&[(DIFFUSION, 0.0)]);
    let dense = response(&[(DIFFUSION, 100.0)]);
    assert!(
        sparse.iter().zip(&dense).any(|(a, b)| (a - b).abs() > 1e-4),
        "Diffusion changes the tail"
    );
    let fine = response(&[(SCALE, 0.0)]);
    let coarse = response(&[(SCALE, 100.0)]);
    assert!(
        fine.iter().zip(&coarse).any(|(a, b)| (a - b).abs() > 1e-4),
        "Scale changes the diffusion"
    );
}

// Spec: reverb R5
#[test]
fn the_damping_shelves_thin_the_tail() {
    let alternating: Vec<f32> = (0..48_000)
        .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let high_tail = |more: &[(ParamId, f64)]| {
        let mut all = vec![(REFLECT, 0.0), (DECAY, 2.0), (DIFFUSION, 100.0)];
        all.extend_from_slice(more);
        let all = with(&all);
        let [left, _] = stream(reverb(&all, 512).as_mut(), &alternating, 512);
        energy(&left, 24_000, 48_000)
    };
    let open = high_tail(&[]);
    let damped = high_tail(&[(HIGH_DAMP, 1.0), (HIGH, 100.0)]);
    assert!(open > 1e-6, "the high tail rings: {open}");
    assert!(
        damped < open * 0.7,
        "High takes the high tail out: {damped} against {open}"
    );
    let impulse_tail = |more: &[(ParamId, f64)]| {
        let mut all = vec![(REFLECT, 0.0), (DECAY, 2.0), (DIFFUSION, 100.0)];
        all.extend_from_slice(more);
        let all = with(&all);
        let [left, _] = impulse_response(&all, 48_000, 512);
        energy(&left, 24_000, 48_000)
    };
    let plain = impulse_tail(&[]);
    let thin = impulse_tail(&[
        (HIGH_DAMP, 1.0),
        (HIGH, 100.0),
        (LOW_DAMP, 1.0),
        (LOW, 100.0),
    ]);
    assert!(
        thin < plain,
        "the shelves thin the tail: {thin} against {plain}"
    );
}

// Spec: reverb R6
#[test]
fn chorus_moves_the_tail() {
    let params = with(&[
        (REFLECT, 0.0),
        (DECAY, 0.5),
        (CHORUS_AMOUNT, 100.0),
        (CHORUS_RATE, 2.0),
    ]);
    let [off, _] = impulse_response(&params, 24_000, 512);
    let mut on = params.clone();
    on.push((CHORUS, 1.0));
    let on = with(&on);
    let [moving, _] = impulse_response(&on, 24_000, 512);
    assert!(
        off.iter().zip(&moving).any(|(a, b)| (a - b).abs() > 1e-4),
        "Chorus moves the tail"
    );
    let still = with(&[
        (REFLECT, 0.0),
        (DECAY, 0.5),
        (CHORUS, 1.0),
        (CHORUS_AMOUNT, 0.0),
        (CHORUS_RATE, 2.0),
    ]);
    let [same, _] = impulse_response(&still, 24_000, 512);
    assert!(off == same, "Chorus at 0 moves nothing");
}

// Spec: reverb R8
#[test]
fn size_moves_the_tail_and_smooth_glides_a_size_change() {
    let arrival_for = |size: f64| {
        let params = with(&[(REFLECT, 0.0), (DECAY, 1.0), (SIZE, size), (SCALE, 50.0)]);
        let [left, _] = impulse_response(&params, 48_000, 512);
        arrival(&left, 1e-4)
    };
    let small = arrival_for(0.0);
    let large = arrival_for(100.0);
    assert!(
        large > small + 1_000,
        "a larger size moves the tail later: {large} against {small}"
    );
    // A change of Size with Smooth Slow glides to the new delay lines; with
    // None it jumps. A fresh impulse's tail shows which lines it meets.
    let changed = |smooth: f64| {
        let params = with(&[
            (REFLECT, 0.0),
            (DECAY, 1.0),
            (SIZE, 0.0),
            (SCALE, 50.0),
            (SMOOTH, smooth),
        ]);
        let mut node = reverb(&params, 4_800);
        let silent = vec![0.0f32; 24_000];
        stream(node.as_mut(), &silent, 4_800);
        let mut input = vec![0.0f32; 4_800];
        input[0] = 1.0;
        let [left, _] = run_stereo_with(node.as_mut(), &input, &input, &[set(0, SIZE, 100.0)]);
        arrival(&left, 1e-4)
    };
    let jumped = changed(0.0);
    let glided = changed(2.0);
    assert!(
        jumped > glided + 1_000,
        "Smooth None jumps to the new size, Slow glides: {jumped} against {glided}"
    );
}

// Spec: reverb R12
#[test]
fn density_chooses_how_many_combs_run() {
    let params = with(&[(REFLECT, 0.0), (DECAY, 1.0)]);
    let response = |density: f64| {
        let mut all = params.clone();
        all.push((DENSITY, density));
        let all = with(&all);
        let [left, _] = impulse_response(&all, 24_000, 512);
        left
    };
    let sparse = response(0.0);
    let high = response(2.0);
    assert!(
        sparse.iter().zip(&high).any(|(a, b)| (a - b).abs() > 1e-4),
        "the density changes the tail"
    );
    assert!(sparse.iter().all(|s| s.is_finite()));
}

// Spec: reverb R7
#[test]
fn predelay_delays_the_first_reflection() {
    let first = |predelay: f64| {
        let params = with(&[(DIFFUSE, 0.0), (SIZE, 50.0), (PREDELAY, predelay)]);
        let [left, _] = impulse_response(&params, 48_000, 512);
        arrival(&left, 1e-4)
    };
    let early = first(0.0);
    let late = first(50.0);
    assert!(
        late > early + 1_800,
        "Predelay moves the first reflection: {late} against {early}"
    );
}

// Spec: reverb R13
#[test]
fn reflect_and_diffuse_route_the_early_reflections_and_the_tail() {
    let both_off = with(&[(REFLECT, 0.0), (DIFFUSE, 0.0), (DECAY, 0.5)]);
    let [left, right] = impulse_response(&both_off, 24_000, 512);
    assert!(
        left.iter().chain(&right).all(|s| *s == 0.0),
        "Reflect and Diffuse at 0 leave silence"
    );
    let early_only = with(&[(DIFFUSE, 0.0), (DECAY, 0.5)]);
    let [early, _] = impulse_response(&early_only, 24_000, 512);
    let early_window = energy(&early, 240, 1_440);
    eprintln!(
        "early_window {early_window} early_late {}",
        energy(&early, 9_600, 24_000)
    );
    assert!(early_window > 0.01, "the early reflections are heard");
    let early_late = energy(&early, 9_600, 24_000);
    assert!(
        early_late < early_window * 1e-3,
        "with Diffuse at 0 there is no tail: {early_late} against {early_window}"
    );
    let tail_only = with(&[(REFLECT, 0.0), (DECAY, 0.5)]);
    let [tail, _] = impulse_response(&tail_only, 24_000, 512);
    let tail_late = energy(&tail, 9_600, 24_000);
    eprintln!(
        "tail_late {tail_late} tail_early {}",
        energy(&tail, 240, 1_440)
    );
    assert!(tail_late > 1e-5, "with Reflect at 0 the tail is heard");
    let tail_early = energy(&tail, 240, 1_440);
    assert!(
        tail_early < early_window * 0.2,
        "with Reflect at 0 the early reflections are gone: {tail_early} against {early_window}"
    );
}

// Spec: reverb R14
#[test]
fn dry_wet_balances_the_dry_and_the_wet() {
    let dry = with(&[(DRY_WET, 0.0)]);
    let input = noise(24_000);
    let [left, right] = stream(reverb(&dry, 512).as_mut(), &input, 512);
    assert!(
        left.iter().zip(&input).all(|(a, b)| a == b),
        "at 0 the output is the input"
    );
    assert!(
        right.iter().zip(&input).all(|(a, b)| a == b),
        "on both channels"
    );
    let wet = with(&[(DRY_WET, 100.0)]);
    let [left, _] = stream(reverb(&wet, 512).as_mut(), &input, 512);
    let difference = left
        .iter()
        .zip(&input)
        .fold(0.0f64, |m, (a, b)| m.max(f64::from((a - b).abs())));
    assert!(difference > 0.01, "at 100 the output is reverberated");
}

// Spec: reverb R1
#[test]
fn the_input_filter_shapes_the_input() {
    let sine = |hz: f64| {
        (0..9_600)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
            .collect::<Vec<f32>>()
    };
    let wet = |params: &[(ParamId, f64)], input: &[f32]| {
        let [left, _] = stream(reverb(params, 512).as_mut(), input, 512);
        energy(&left, 0, 4_800).sqrt()
    };
    let base = with(&[(DECAY, 0.5)]);
    let low = sine(50.0);
    let open = wet(&base, &low);
    let cut = with(&[(DECAY, 0.5), (LOW_CUT, 1.0), (LOW_CUT_HZ, 500.0)]);
    let filtered = wet(&cut, &low);
    assert!(open > 0.1, "the 50 Hz sine is heard: {open}");
    assert!(
        filtered < open * 0.3,
        "the low cut takes the 50 Hz sine out: {filtered} against {open}"
    );
    let high = sine(10_000.0);
    let open = wet(&base, &high);
    let cut = with(&[(DECAY, 0.5), (HIGH_CUT, 1.0), (HIGH_CUT_HZ, 1_000.0)]);
    let filtered = wet(&cut, &high);
    assert!(open > 0.1, "the 10 kHz sine is heard: {open}");
    assert!(
        filtered < open * 0.3,
        "the high cut takes the 10 kHz sine out: {filtered} against {open}"
    );
}

// Spec: reverb R11
#[test]
fn zero_stereo_is_mono_and_full_stereo_keeps_the_channels_apart() {
    let mono = with(&[(STEREO, 0.0), (DECAY, 0.5)]);
    let [left, right] = impulse_response(&mono, 24_000, 512);
    assert!(left.iter().any(|s| s.abs() > 0.01), "the reverb is heard");
    assert!(left == right, "at 0 the two channels are one");
    let wide = with(&[(STEREO, 120.0), (DECAY, 0.5)]);
    let [left, right] = impulse_response(&wide, 24_000, 512);
    assert!(
        left.iter().zip(&right).any(|(l, r)| l != r),
        "at 120 the channels differ"
    );
}

// Spec: device-framework R10
#[test]
fn the_layout_places_every_parameter_once() {
    let description = reverb_type().description;
    description.check().expect("a good description");
    description
        .layout
        .check(&description.params)
        .expect("every parameter is placed once");
}
