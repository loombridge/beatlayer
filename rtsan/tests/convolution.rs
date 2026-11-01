//! The partitioned convolver on the audio thread under RealtimeSanitizer
//! (WP-8.29): true-stereo blocks of every size, a swap in the middle of one,
//! the crossfade, the retired response handed out, and a reset. RTSan stops
//! the process at the first allocation, free, lock, or blocking call in any
//! of it, and the allocation trap, installed as in every Beatlayer test
//! binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_dsp::convolution::{Convolver, prepare};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A decaying ramp of `frames` frames, different for each `seed`.
fn tail(frames: usize, seed: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| ((i as f32 * (0.01 + seed)).sin()) * (-6.0 * i as f32 / frames as f32).exp())
        .collect()
}

#[test]
fn convolving_and_swapping_pass_under_rtsan() {
    let long: Vec<Vec<f32>> = (0..4).map(|c| tail(30_000, 0.003 * c as f32)).collect();
    let short = tail(9000, 0.02);
    let slices: Vec<&[f32]> = long.iter().map(Vec::as_slice).collect();
    let first = prepare(&slices, 48_000).unwrap();
    let second = prepare(&[&short, &short], 48_000).unwrap();
    let mut conv = Convolver::with_response(first);
    let mut second = Some(second);
    let mut left = tail(4096, 0.05);
    let mut right = tail(4096, 0.07);
    // What the audio thread retires goes to the drop thread's queue.
    let (mut to_drop, mut dropper) = rtrb::RingBuffer::new(4);
    for (round, block) in [1, 32, 64, 100, 500, 4096]
        .into_iter()
        .cycle()
        .take(60)
        .enumerate()
    {
        bl_rt::rt_test!({
            if round == 20 {
                conv.swap(second.take().unwrap(), 0.05).unwrap();
            }
            conv.process(&mut left[..block], &mut right[..block]);
            while let Some(old) = conv.take_retired() {
                to_drop.push(old).unwrap();
            }
        });
    }
    assert!(!conv.swapping());
    assert!(dropper.pop().is_ok(), "the old response was never retired");
    bl_rt::rt_test!({ conv.reset() });
    bl_rt::rt_test!({ conv.process(&mut left[..100], &mut right[..100]) });
}
