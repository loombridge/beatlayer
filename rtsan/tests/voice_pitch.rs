//! The voice pitch tracker and shifter under RealtimeSanitizer (WP-8.32).
//! A glide of pulses and a burst of noise run through both, block after
//! block, with the ratios moving the whole time. RTSan stops the process at
//! the first allocation, free, lock, or blocking call, and the allocation
//! trap, installed as in every Beatlayer test binary, fails the test at any
//! heap use it sees.

bl_rt::install_trap!();

use bl_dsp::pitch::{Config, Tracker, VoiceShifter};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

const RATE: f64 = 48_000.0;

#[test]
fn the_voice_tracker_and_shifter_pass_under_rtsan() {
    let mut tracker = Tracker::new(&Config::for_range(RATE, 60.0, 2000.0));
    let mut shifter = VoiceShifter::new(RATE, 60.0, 2000.0);

    // A pulse train that glides from 90 Hz to 400 Hz, then noise, then
    // silence, then the pulses again.
    let mut phase = 0.0f64;
    let mut state = 1u32;
    let input: Vec<f32> = (0..200_000usize)
        .map(|i| match i / 50_000 {
            0 | 3 => {
                phase += (90.0 + 310.0 * (i % 50_000) as f64 / 50_000.0) / RATE;
                if phase >= 1.0 {
                    phase -= 1.0;
                }
                (1.0 - 2.0 * phase) as f32 * 0.3
            }
            1 => {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
            }
            _ => 0.0,
        })
        .collect();

    let mut heard = 0usize;
    let mut sum = 0.0f32;
    for (n, block) in input.chunks(256).enumerate() {
        bl_rt::rt_test!({
            for (i, &x) in block.iter().enumerate() {
                let t = (n * 256 + i) as f32;
                tracker.feed(x);
                while let Some(found) = tracker.pop() {
                    heard += usize::from(found.is_some());
                }
                let pitch = 2.0f32.powf((t * 0.00002).sin());
                let formant = 2.0f32.powf((t * 0.00003).cos());
                sum += shifter.process(x, pitch, formant);
            }
        });
    }
    assert!(heard > 100, "the tracker measured {heard} times");
    assert!(sum.is_finite());
}
