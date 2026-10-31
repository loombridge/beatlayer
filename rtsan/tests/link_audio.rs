//! Link Audio on the audio thread under RealtimeSanitizer (WP-7.16,
//! `docs/specs/link-audio.md` R11): the engine sends a track to a real peer
//! and plays a channel the peer sends, both on a real Link session, while
//! the transport starts and stops and the peer renames what it offers. The
//! sending goes through Beatlayer's own shim over Link's sink, because Link's
//! C interface allocates a buffer handle for each block it sends (the shim's
//! comment says so); the receiving reads a ring Link's thread fills. RTSan
//! stops the process at the first allocation, free, lock, or blocking call on
//! the audio thread, and the allocation trap fails the test at any heap use
//! it sees.

bl_rt::install_trap!();

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bl_core::time::SampleRate;
use bl_engine::testing::{self, Probe};
use bl_engine::{
    Collector, Compiler, EngineConfig, LinkAudio, LinkFollower, NoInput, TransportCommand, channel,
};
use bl_link::{Link, QUANTUM};
use bl_model::{Command, Editor, Input, Monitor, TrackKind};
use bl_shell::link::{LinkSession, PeerTempo};
use bl_shell::link_audio::{Bank, LinkAudioHost, Wanted};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn link_audio_sends_and_receives_under_rtsan() {
    let rate = SampleRate::HZ_48000;
    let config = EngineConfig::new(rate, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(
        config,
        testing::registry(&Arc::new(Probe::default())),
        collector.handle(),
    );
    let (mut sender, mut processor, mut monitor) = channel(&config);

    // A peer that is in the session first, with Link Audio on, which sends a
    // channel and takes the one Beatlayer offers.
    let peer = Link::new(120.0);
    peer.enable(true, 120.0);
    peer.set_audio_enabled(true);
    peer.set_audio_name("peer");
    let tone = peer.sink("Tone");
    thread::sleep(Duration::from_millis(1500));
    let link = Link::new(120.0);
    link.enable(true, 120.0);
    link.set_audio_enabled(true);
    link.set_audio_name("beatlayer");

    // The document: a track offered to the peer, and a track that takes the
    // peer's channel, monitoring it.
    let mut editor = Editor::new();
    let offered = editor.new_id();
    let taking = editor.new_id();
    for (id, name) in [(offered, "Beat"), (taking, "Take")] {
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: name.into(),
                parent: None,
                index: 0,
            })
            .unwrap();
    }
    editor
        .apply(Command::SetInput {
            track: taking,
            input: Input::LinkAudio {
                peer: "peer".into(),
                channel: "Tone".into(),
            },
            channel: None,
        })
        .unwrap();
    editor
        .apply(Command::SetMonitor {
            track: taking,
            monitor: Monitor::In,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));

    // Wait for the peer's channel to reach this side, then take it and
    // offer the track: the plan the audio thread gets. The peer takes the
    // track Beatlayer offers, so its sink is connected and its commits run.
    let mut host = LinkAudioHost::default();
    host.settings().set_latency_ms(20.0);
    let wanted = Wanted {
        on: true,
        rate: rate.hz(),
        tracks: vec![(offered, "Beat".into())],
        inputs: vec![("peer".into(), "Tone".into())],
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.update(&link, 0, &wanted);
        if host.peers().iter().any(|p| p.name == "peer") {
            break;
        }
        assert!(Instant::now() < deadline, "the peer's channel never came");
        thread::sleep(Duration::from_millis(20));
    }
    // Taken now, with the channel in the list.
    host.update(&link, 1, &wanted);
    let plan = host.plan();
    let beat_channel = loop {
        if let Some(channel) = peer.channels().into_iter().find(|c| c.name == "Beat") {
            break channel;
        }
        assert!(Instant::now() < deadline, "the offered track never reached the peer");
        thread::sleep(Duration::from_millis(20));
    };
    let received = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let _source = {
        let received = Arc::clone(&received);
        peer.source(beat_channel.id, move |buffer| {
            received.fetch_add(buffer.frames as u64, Ordering::Relaxed);
        })
    };

    // The peer sends its tone from a thread of its own, as an app would.
    let stop = Arc::new(AtomicBool::new(false));
    let sender_thread = {
        let stop = Arc::clone(&stop);
        let link = peer.handle();
        thread::spawn(move || {
            let mut audio = link.audio();
            let block = [0.25f32; 240];
            while !stop.load(Ordering::Relaxed) {
                audio.capture();
                let now = audio.clock_micros();
                let beats = audio.beat_at_time(now, QUANTUM);
                tone.send(&audio, beats, &block, &block, 48_000);
                thread::sleep(Duration::from_millis(5));
            }
        })
    };

    let handles = link.handle();
    let settings = Arc::clone(host.settings());
    let session = LinkSession::with_settings(
        handles.audio(),
        Arc::new(PeerTempo::default()),
        Arc::clone(&settings),
    );
    let follower: Box<dyn bl_engine::Follower> = Box::new(LinkFollower::new(session));
    sender.follower(Some(collector.handle().wrap(follower)));
    let bank: Box<dyn LinkAudio> = Box::new(Bank::new(&handles, Arc::clone(&plan), 256));
    sender.link_audio(Some(collector.handle().wrap(bank)));
    assert!(sender.flush());

    for round in 0..1500u32 {
        match round {
            100 | 700 => sender.transport(TransportCommand::Play),
            400 | 1000 => sender.transport(TransportCommand::Stop),
            // Sync to Incoming Audio on and off, and the Latency moving.
            300 => settings.set_sync(true),
            350 | 600 => settings.set_latency_ms(f64::from(round) / 10.0),
            900 => settings.set_sync(false),
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
    stop.store(true, Ordering::Relaxed);
    sender_thread.join().unwrap();
    assert!(monitor.state().clock >= 1500 * 256);
    let heard = received.load(Ordering::Relaxed);
    assert!(heard > 0, "the peer received what the engine offered");
    eprintln!(
        "1500 blocks sending a track to a peer ({heard} frames reached it) and playing its \
         channel on a real Link session under RealtimeSanitizer"
    );
}
