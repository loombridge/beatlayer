//! Link on the audio thread under RealtimeSanitizer (WP-7.14,
//! `docs/specs/link.md` R12): the engine follows a real Link session with a
//! peer in it, capturing it every block, while the transport starts, stops
//! and jumps and the peer changes the tempo. RTSan stops the process at the
//! first allocation, free, lock, or blocking call on the audio thread, and
//! the allocation trap fails the test at any heap use it sees.
//!
//! What RTSan found, 30 October 2026: capture and every read of the session
//! pass. Committing it does not: `abl_link_commit_audio_session_state`
//! signals a condition variable (`pthread_cond_signal`, from
//! `Controller::setClientStateRtSafe`), which RTSan reports as an
//! unsafe library call. The test that commits is ignored until a maintainer
//! decides how to take that call (`docs/specs/link.md`, the real-time rule
//! R12); nothing is suppressed.

bl_rt::install_trap!();

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{
    Collector, Compiler, EngineConfig, LinkFollower, NoInput, TransportCommand, channel,
};
use bl_link::Link;
use bl_model::{Command, Editor};
use bl_shell::link::{LinkSession, PeerTempo};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// Runs the engine under RTSan against a real Link session with a peer in
/// it. With `commit`, Beatlayer also writes the session: Start Stop Sync is
/// on, so its starts and stops go to the session, and its tempo changes.
fn run(commit: bool) {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(
        config,
        testing::registry(&Arc::new(Probe::default())),
        collector.handle(),
    );
    let (mut sender, mut processor, mut monitor) = channel(&config);

    // A peer that is in the session first, so the engine joins it.
    let peer = Link::new(100.0);
    peer.enable(true, 100.0);
    thread::sleep(Duration::from_millis(1500));
    let link = Link::new(120.0);
    link.enable(true, 120.0);
    if commit {
        peer.set_start_stop_sync(true);
        link.set_start_stop_sync(true);
    }
    let session = LinkSession::new(link.audio(), Arc::new(PeerTempo::default()));
    let follower: Box<dyn bl_engine::Follower> = Box::new(LinkFollower::new(session));
    sender.follower(Some(collector.handle().wrap(follower)));

    let mut editor = Editor::new();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());

    for round in 0..1500u32 {
        match round {
            // Beatlayer starts and stops: with peers, a start waits for the
            // session's phase.
            100 | 700 => sender.transport(TransportCommand::Play),
            400 | 1000 => sender.transport(TransportCommand::Stop),
            450 => sender.transport(TransportCommand::Locate(Ticks::WHOLE * 3)),
            // The peer changes the tempo: the engine plays it, and a
            // difference of phase is slewed or jumped.
            200 => peer.set_tempo(133.0),
            600 | 900 => peer.set_tempo(87.0 + f64::from(round) / 20.0),
            1100 if commit => {
                // Beatlayer changes the tempo: it goes to the session.
                editor
                    .apply(Command::SetTempo {
                        tempo: TempoMap::constant(Bpm::new(141.0).unwrap()),
                    })
                    .unwrap();
                sender.send_update(compiler.sync(editor.document()));
            }
            _ => {}
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process_at(256, &NoInput, Instant::now());
        });
        if round % 50 == 0 {
            collector.collect();
        }
        thread::sleep(Duration::from_micros(500));
    }
    assert!(link.peers() >= 1, "the engine's session has the peer");
    assert!(monitor.state().clock >= 1500 * 256);
    eprintln!(
        "1500 blocks following a Link session with {} peer(s) under RealtimeSanitizer; the \
         session's tempo is {:.2}",
        link.peers(),
        link.snapshot(4.0).tempo
    );
}

#[test]
fn link_capture_and_the_follower_pass_under_rtsan() {
    run(false);
}

#[test]
#[ignore = "RTSan reports pthread_cond_signal in Link's commit; a maintainer decides (link spec R12)"]
fn link_commit_under_rtsan() {
    run(true);
}
