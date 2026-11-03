//! The copy of incoming audio for Tempo Follower on the audio thread under
//! RealtimeSanitizer (WP-9.12, `docs/specs/tempo-follower.md` R11): the audio
//! callback sums the chosen channel pair to mono into the tracker's ring,
//! block after block, past the point where the ring is full and the copy
//! drops and counts blocks. RTSan stops the process at the first allocation,
//! free, lock, or blocking call in the copy, and the allocation trap fails
//! the test at any heap use it sees. The tracker's own thread runs outside
//! the section, as it does in the app.

bl_rt::install_trap!();

use bl_analysis::tempo_tracker::{Channel, ring};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn the_copy_of_incoming_audio_passes_under_rtsan() {
    let (mut copier, mut tracker) = ring(48_000, 120.0, Channel::Pair(0));
    let shared = tracker.shared();
    let block = vec![0.25f32; 2 * 512];
    // The ring holds a second: about 93 blocks. The tracker reads it every
    // 80 blocks for the first 400, which never fills it, and then not at
    // all for 120, which does.
    for round in 0..520 {
        bl_rt::rt_test!({
            copier.copy(&block, 2);
        });
        if round < 400 && round % 80 == 79 {
            assert!(tracker.run() > 0);
        }
    }
    assert!(shared.dropped() > 0, "the ring never filled");
    eprintln!(
        "520 blocks copied under RealtimeSanitizer, {} dropped to a full ring",
        shared.dropped()
    );
}
