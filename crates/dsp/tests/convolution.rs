//! The partitioned convolver against a direct convolution (WP-8.29): every
//! layout and length, block sizes, the work per tick, swaps, the limits, and
//! the real-time rules.

use std::sync::Arc;
use std::thread;

use bl_dsp::Random;
use bl_dsp::convolution::{
    Convolver, Layout, MAX_PREPARED_BYTES, PrepareError, PreparedIr, SwapError, TICK, prepare,
};

lspk_rt::install_trap!();

const RATE: u32 = 48_000;
const LAYOUTS: [Layout; 3] = [Layout::Mono, Layout::Stereo, Layout::TrueStereo];

type Stereo = [Vec<f32>; 2];

fn noise(frames: usize, seed: u64) -> Vec<f32> {
    let mut random = Random::seeded(seed);
    (0..frames).map(|_| random.next_bipolar() as f32).collect()
}

/// Noise that falls 60 dB over `frames`, as a room's tail does.
fn decaying(frames: usize, seed: u64) -> Vec<f32> {
    noise(frames, seed)
        .into_iter()
        .enumerate()
        .map(|(i, v)| v * (-6.9 * i as f32 / frames as f32).exp())
        .collect()
}

fn response(layout: Layout, frames: usize, seed: u64) -> Vec<Vec<f32>> {
    (0..layout.channels() as u64)
        .map(|c| decaying(frames, seed + c))
        .collect()
}

fn prepared(channels: &[Vec<f32>]) -> Arc<PreparedIr> {
    let slices: Vec<&[f32]> = channels.iter().map(Vec::as_slice).collect();
    prepare(&slices, RATE).unwrap()
}

fn stereo_noise(frames: usize, seed: u64) -> Stereo {
    [noise(frames, seed), noise(frames, seed + 100)]
}

/// One frame of a direct convolution, in double precision.
fn direct_at(x: &[f32], h: &[f32], n: usize) -> f64 {
    (0..=n.min(h.len() - 1))
        .map(|t| f64::from(h[t]) * f64::from(x[n - t]))
        .sum()
}

/// The direct convolution of the stereo `x` with a response's channels at
/// frame `n`, for each output.
fn reference_at(layout: Layout, h: &[Vec<f32>], x: &Stereo, n: usize) -> [f64; 2] {
    let at = |input: usize, chan: usize| direct_at(&x[input], &h[chan], n);
    match layout {
        Layout::Mono => [at(0, 0), at(1, 0)],
        Layout::Stereo => [at(0, 0), at(1, 1)],
        Layout::TrueStereo => [at(0, 0) + at(1, 2), at(0, 1) + at(1, 3)],
    }
}

fn run(conv: &mut Convolver, input: &Stereo, block: usize) -> Stereo {
    let mut out = input.clone();
    let [left, right] = &mut out;
    for (l, r) in left.chunks_mut(block).zip(right.chunks_mut(block)) {
        conv.process(l, r);
    }
    out
}

/// The frames the tests compare: the start, then a spread.
fn points(frames: usize) -> Vec<usize> {
    let mut at: Vec<usize> = (0..frames.min(3000)).collect();
    at.extend((3000..frames).step_by(997));
    at.extend(frames.saturating_sub(100)..frames);
    at
}

/// The error in dB of `got` against the direct convolution at the frames
/// `from` on, relative to its level.
fn error_db(layout: Layout, h: &[Vec<f32>], x: &Stereo, got: &Stereo, from: usize) -> f64 {
    let (mut err, mut level) = (0.0, 0.0);
    for n in points(x[0].len()).into_iter().filter(|&n| n >= from) {
        let want = reference_at(layout, h, x, n);
        for (o, w) in want.iter().enumerate() {
            err += (f64::from(got[o][n]) - w).powi(2);
            level += w * w;
        }
    }
    20.0 * (err / level.max(1e-30)).sqrt().log10()
}

// Spec: impulse-responses R3 (the layouts' order); architecture, `bl_dsp::convolution`
#[test]
fn every_layout_and_length_matches_a_direct_convolution() {
    let lengths = [
        1, 2, 63, 64, 65, 500, 1023, 1024, 1025, 4095, 4096, 4097, 6143, 6144, 10_000, 70_000,
    ];
    for layout in LAYOUTS {
        for frames in lengths {
            let h = response(layout, frames, 11);
            let x = stereo_noise(frames + 3000, 1);
            let mut conv = Convolver::with_response(prepared(&h));
            let got = run(&mut conv, &x, 256);
            let db = error_db(layout, &h, &x, &got, 0);
            assert!(db < -110.0, "{layout:?}, {frames} frames: {db:.1} dB");
        }
    }
}

// Spec: architecture, `bl_dsp::convolution` (responses up to 30 seconds)
#[test]
fn a_response_of_thirty_seconds_plays_and_a_longer_one_is_refused() {
    let frames = 30 * RATE as usize;
    let h = response(Layout::TrueStereo, frames, 21);
    let ir = prepared(&h);
    assert!(ir.bytes() < MAX_PREPARED_BYTES);
    let x = stereo_noise(60_000, 3);
    let mut conv = Convolver::with_response(ir);
    let got = run(&mut conv, &x, 480);
    let db = error_db(Layout::TrueStereo, &h, &x, &got, 0);
    assert!(db < -110.0, "{db:.1} dB");

    let long = vec![0.1; frames + 2];
    let err = prepare(&[&long], RATE).unwrap_err();
    assert!(matches!(err, PrepareError::TooLong { .. }));
    assert!(err.to_string().contains("30 seconds"), "{err}");
}

// Spec: architecture, `bl_dsp::convolution` (the budget)
#[test]
fn a_response_past_the_memory_budget_is_refused_with_its_reason() {
    // 30 seconds of true stereo at 192 kHz takes 184 MiB prepared.
    let long = vec![0.1; 30 * 192_000];
    let err = prepare(&[&long, &long, &long, &long], 192_000).unwrap_err();
    assert!(matches!(err, PrepareError::TooLarge { .. }), "{err}");
    assert!(err.to_string().contains("MiB"), "{err}");
    // The same at stereo takes 92 MiB.
    assert!(prepare(&[&long, &long], 192_000).is_ok());
    assert_eq!(
        prepare(&[&long, &long, &long], 48_000).unwrap_err(),
        PrepareError::Channels(3)
    );
    assert_eq!(prepare(&[], 48_000).unwrap_err(), PrepareError::Channels(0));
    assert_eq!(prepare(&[&[]], 48_000).unwrap_err(), PrepareError::Empty);
}

// Spec: architecture, `bl_dsp::convolution` (no latency)
#[test]
fn the_first_output_frame_carries_the_first_tap() {
    for layout in LAYOUTS {
        let mut h = response(layout, 9000, 5);
        for c in &mut h {
            c[0] = 0.5;
        }
        let mut conv = Convolver::with_response(prepared(&h));
        assert_eq!(conv.latency(), 0);
        let mut left = [1.0];
        let mut right = [1.0];
        conv.process(&mut left, &mut right);
        let want = reference_at(layout, &h, &[vec![1.0], vec![1.0]], 0);
        assert_eq!(
            [left[0], right[0]],
            [want[0] as f32, want[1] as f32],
            "{layout:?}"
        );
    }
}

// Spec: architecture, `bl_dsp::convolution` (the output doesn't depend on the block size)
#[test]
fn output_is_bit_identical_at_any_block_size() {
    let h = response(Layout::TrueStereo, 20_000, 31);
    let ir = prepared(&h);
    let x = stereo_noise(30_000, 9);
    let reference = run(&mut Convolver::with_response(ir.clone()), &x, 64);
    for block in [1, 32, 500, 4096] {
        let got = run(&mut Convolver::with_response(ir.clone()), &x, block);
        assert!(got == reference, "block size {block}");
    }
}

// Spec: architecture, `bl_dsp::convolution` (flat work)
#[test]
fn each_tick_does_the_same_share_of_each_stages_transforms() {
    let ir = prepared(&response(Layout::TrueStereo, 60_000, 41));
    let mut conv = Convolver::with_response(ir);
    // Per tick and stage: two inputs' forward transforms and two outputs'
    // inverse transforms, a window's steps spread over its ticks. The
    // stages' windows are 1, 8 and 32 ticks, with 24, 64 and 128 steps in a
    // transform.
    let share = [96, 32, 16];
    let mut last = conv.sub_ffts();
    let mut counts = Vec::new();
    let x = stereo_noise(TICK * 200, 5);
    let [mut left, mut right] = x.clone();
    for (l, r) in left.chunks_mut(TICK).zip(right.chunks_mut(TICK)) {
        conv.process(l, r);
        let now = conv.sub_ffts();
        counts.push(std::array::from_fn::<u64, 3, _>(|k| now[k] - last[k]));
        last = now;
    }
    for (tick, delta) in counts.iter().enumerate() {
        for k in 0..3 {
            assert!(
                delta[k] <= share[k],
                "tick {tick}, stage {k}: {} transforms, share {}",
                delta[k],
                share[k]
            );
        }
    }
    // Once every window is running, each tick does exactly its share.
    for delta in &counts[80..] {
        assert_eq!(delta, &[96, 32, 16]);
    }
}

fn steps_of(v: &[f32]) -> f32 {
    v.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

// Spec: impulse-responses R4 (the previous response plays until the new one crossfades in)
#[test]
fn a_swap_crossfades_between_exact_outputs_and_frees_the_old_response_on_the_drop_thread() {
    let layout = Layout::Stereo;
    let ha = response(layout, 8000, 51);
    let hb = response(layout, 5000, 61);
    let (a, b) = (prepared(&ha), prepared(&hb));
    let weak = Arc::downgrade(&a);
    let x = stereo_noise(48_000, 13);
    let swap_at = 20_000;
    let mut conv = Convolver::with_response(a);
    let (mut tx, mut rx) = rtrb::RingBuffer::new(8);
    let mut out = x.clone();
    let mut b = Some(b);
    let mut fade = None;
    let [left, right] = &mut out;
    let mut done = 0;
    for (l, r) in left.chunks_mut(500).zip(right.chunks_mut(500)) {
        lspk_rt::rt_test!({
            if done == swap_at {
                conv.swap(b.take().unwrap(), 0.05).unwrap();
            }
            conv.process(l, r);
            while let Some(old) = conv.take_retired() {
                tx.push(old).unwrap();
            }
        });
        done += l.len();
        if fade.is_none() {
            fade = conv.fade();
        }
    }
    let (start, len) = fade.unwrap();
    let (start, len) = (start as usize, len as usize);
    assert_eq!(len, 2400);
    assert!(start >= swap_at && start < swap_at + 8192, "{start}");
    assert!(!conv.swapping());

    // The old response is freed by the thread that collects it, not here.
    assert!(weak.upgrade().is_some());
    let drops = thread::spawn(move || rx.pop().map(drop).is_ok());
    assert!(drops.join().unwrap());
    assert!(weak.upgrade().is_none());

    // The output is the old response until the fade, the mix of the two
    // during it, and the new one after.
    let mut worst: f64 = f64::MIN;
    for n in points(x[0].len()).into_iter().filter(|&n| n > 4096) {
        let g = (n.saturating_sub(start) as f64 / len as f64).min(1.0);
        let (ra, rb) = (
            reference_at(layout, &ha, &x, n),
            reference_at(layout, &hb, &x, n),
        );
        for o in 0..2 {
            let want = ra[o] * (1.0 - g) + rb[o] * g;
            worst = worst.max((f64::from(out[o][n]) - want).abs());
        }
    }
    assert!(worst < 3e-5, "{worst}");
    // No step in the crossfade is larger than the signals' own.
    let own = |h: &[Vec<f32>], o: usize| {
        let y: Vec<f32> = (0..x[0].len())
            .map(|n| reference_at(layout, h, &x, n)[o] as f32)
            .collect();
        steps_of(&y[start..start + len])
    };
    for (o, y) in out.iter().enumerate() {
        let step = steps_of(&y[start..=start + len]);
        assert!(step <= own(&ha, o).max(own(&hb, o)) * 1.01, "output {o}");
    }
}

// Spec: architecture, `bl_dsp::convolution` (a swap that can't be played comes back)
#[test]
fn a_swap_that_can_not_be_played_is_handed_back() {
    let short = prepared(&response(Layout::Stereo, 1000, 1));
    let long = prepared(&response(Layout::Stereo, 20_000, 2));
    let other_rate = prepare(&[&[1.0]], 44_100).unwrap();
    let mut conv = Convolver::new(RATE, short.frames());
    let (err, ir) = conv.swap(other_rate, 0.05).unwrap_err();
    assert_eq!(err, SwapError::Rate);
    assert_eq!(ir.rate(), 44_100);
    let (err, _) = conv.swap(long, 0.05).unwrap_err();
    assert_eq!(err, SwapError::TooLong);
    // Swaps pile up in the queue only as far as the retired list has room.
    for _ in 0..4 {
        match conv.swap(short.clone(), 0.01) {
            Ok(()) => {}
            Err((err, _)) => {
                assert_eq!(err, SwapError::Busy);
                break;
            }
        }
    }
    while conv.take_retired().is_some() {}
    assert!(conv.swap(short, 0.01).is_ok());
}

// Spec: architecture, `bl_dsp::convolution` (a later swap replaces one waiting)
#[test]
fn the_newest_of_swaps_that_wait_is_the_one_that_plays() {
    let responses: Vec<Vec<Vec<f32>>> = (0..3)
        .map(|s| response(Layout::Mono, 3000 + 500 * s, 70 + s as u64))
        .collect();
    let irs: Vec<_> = responses.iter().map(|h| prepared(h)).collect();
    let mut conv = Convolver::new(RATE, 4000);
    let x = stereo_noise(30_000, 17);
    let mut out = x.clone();
    let [left, right] = &mut out;
    conv.swap(irs[0].clone(), 0.01).unwrap();
    conv.process(&mut left[..1000], &mut right[..1000]);
    // The second waits behind the first's fade; the third replaces it.
    conv.swap(irs[1].clone(), 0.01).unwrap();
    conv.swap(irs[2].clone(), 0.01).unwrap();
    conv.process(&mut left[1000..], &mut right[1000..]);
    assert!(!conv.swapping());
    let db = error_db(Layout::Mono, &responses[2], &x, &out, 20_000);
    assert!(db < -110.0, "{db:.1} dB");
    // The two responses it replaced and the first that played come back.
    let mut back = 0;
    while conv.take_retired().is_some() {
        back += 1;
    }
    assert_eq!(back, 2);
}

// Spec: architecture, `bl_dsp::convolution`; real-time rules
#[test]
fn processing_and_resetting_allocate_nothing() {
    let ir = prepared(&response(Layout::TrueStereo, 20_000, 81));
    let mut conv = Convolver::with_response(ir.clone());
    let mut left = noise(1024, 1);
    let mut right = noise(1024, 2);
    for _ in 0..40 {
        lspk_rt::rt_test!({ conv.process(&mut left, &mut right) });
    }
    lspk_rt::rt_test!({ conv.reset() });
    lspk_rt::rt_test!({ conv.process(&mut left[..37], &mut right[..37]) });
    // A reset starts the history over: the output is the same as a new
    // convolver's.
    let x = stereo_noise(5000, 3);
    lspk_rt::rt_test!({ conv.reset() });
    let again = run(&mut conv, &x, 700);
    let fresh = run(&mut Convolver::with_response(ir), &x, 700);
    assert!(again == fresh);
}
