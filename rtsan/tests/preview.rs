//! The browser's preview voice under RealtimeSanitizer (WP-1.10; spec:
//! browser R12): taking previews off its queue, playing them, replacing one
//! with another, fading a stopped one out, following the volume, and letting
//! go of finished audio, every block a real-time section. RTSan stops the
//! process at the first allocation, free, lock, or blocking call in any of
//! it, and the allocation trap fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_engine::Collector;
use bl_engine::preview::{PreviewBuffer, PreviewShared, preview_channel};

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn a_hundred_previews_play_replace_and_stop_under_rtsan() {
    let mut collector = Collector::new();
    let (mut control, mut voice) =
        preview_channel(collector.handle(), Arc::new(PreviewShared::default()));
    let mut left = vec![0.0f32; 128];
    let mut right = vec![0.0f32; 128];
    let mut heard = 0usize;
    for round in 0..100u32 {
        // Made and wrapped off the audio thread, as the browser does.
        let level = 0.001 * (1 + round % 10) as f32;
        let buffer = PreviewBuffer::new(vec![vec![level; 1_000]; 1 + (round % 2) as usize])
            .expect("channels");
        control.play(Box::new(buffer)).expect("queued");
        control.set_gain(if round % 4 == 0 { 0.0 } else { 0.5 });
        for block in 0..10 {
            if round % 3 == 1 && block == 3 {
                control.stop();
            }
            bl_rt::rt_test!({
                left.fill(0.0);
                right.fill(0.0);
                voice.render(128, [&mut left[..], &mut right[..]].into_iter());
            });
            if left.iter().any(|s| *s != 0.0) {
                heard += 1;
            }
        }
        if round % 10 == 0 {
            collector.collect();
        }
    }
    assert!(heard > 0, "the previews played");
    drop(voice);
    collector.collect();
    eprintln!("100 previews under RealtimeSanitizer: {heard} blocks heard");
}
