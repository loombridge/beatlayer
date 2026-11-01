//! Link Audio between two Link instances in one process on the loopback
//! network (the Link Audio spec, R1, R3, R4, R11): a channel one offers
//! appears in the other's list, comes and goes with its sink, is renamed with
//! it, and its buffers arrive with the moment they were sent on. The tests
//! wait for discovery with a deadline, the one place they wait on the wall
//! clock, which `cargo xtask check`'s allowlist names.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bl_link::{Audio, Link, QUANTUM};

fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Two instances with Link and Link Audio on, named `a` and `b`, that have
/// found each other: the first runs alone for a while so it is the older
/// session, as the Link tests do.
fn pair() -> (Link, Link) {
    let a = Link::new(120.0);
    a.enable(true, 120.0);
    a.set_audio_enabled(true);
    a.set_audio_name("alpha");
    std::thread::sleep(Duration::from_millis(1500));
    let b = Link::new(120.0);
    b.enable(true, 120.0);
    b.set_audio_enabled(true);
    b.set_audio_name("beta");
    wait("a peer", || a.peers() == 1 && b.peers() == 1);
    (a, b)
}

/// Sends a block of `value` from `audio` as the audio thread would, at the
/// session's beat now.
fn send_block(sink: &bl_link::Sink, audio: &mut Audio, value: f32) -> bool {
    audio.capture();
    let now = audio.clock_micros();
    let beats = audio.beat_at_time(now, QUANTUM);
    let block = [value; 256];
    sink.send(audio, beats, &block, &block, 48_000)
}

#[test]
fn the_name_and_switch_are_kept() {
    let link = Link::new(120.0);
    assert!(!link.audio_enabled());
    link.set_audio_enabled(true);
    assert!(link.audio_enabled());
    link.set_audio_name("Studio Mac");
    // Link takes the name on its own thread.
    wait("the name", || link.audio_name() == "Studio Mac");
}

#[test]
fn a_sink_is_a_channel_on_the_other_peer_until_it_is_dropped_and_is_renamed_with_it() {
    let (a, b) = pair();
    let sink = a.sink("Drums");
    wait("the channel", || {
        b.channels()
            .iter()
            .any(|channel| channel.name == "Drums" && channel.peer_name == "alpha")
    });
    sink.set_name("Kick");
    wait("the new name", || {
        let channels = b.channels();
        channels.iter().any(|channel| channel.name == "Kick") && channels.len() == 1
    });
    drop(sink);
    wait("the channel to go", || b.channels().is_empty());
}

#[test]
fn a_source_receives_the_blocks_a_sink_sends_with_the_moment_they_were_sent_on() {
    let (a, b) = pair();
    let sink = a.sink("Tone");
    let channel = {
        let mut found = None;
        wait("the channel", || {
            found = b.channels().into_iter().find(|c| c.name == "Tone");
            found.is_some()
        });
        found.unwrap()
    };
    let got = Arc::new(AtomicUsize::new(0));
    let first = Arc::new(Mutex::new(None));
    let source = {
        let (got, first) = (Arc::clone(&got), Arc::clone(&first));
        // Link may cut a block into smaller buffers, so only the whole is
        // checked; a panic here would abort the process.
        b.source(channel.id, move |buffer| {
            let whole = buffer.channels == 2
                && buffer.rate == 48_000
                && buffer.samples.len() == buffer.frames * 2
                && buffer.samples.iter().all(|&s| s == 16384);
            first
                .lock()
                .unwrap()
                .get_or_insert((whole, buffer.begin_micros));
            got.fetch_add(1, Ordering::Relaxed);
        })
    };
    let mut audio = a.audio();
    let clock = b.clock_micros();
    wait("buffers", || {
        send_block(&sink, &mut audio, 0.5);
        got.load(Ordering::Relaxed) >= 3
    });
    let (whole, began) = first.lock().unwrap().unwrap();
    assert!(whole);
    let began = began.expect("same session");
    // The moment is on Link's clock, between when the test began and now.
    assert!(began > clock - 1_000_000 && began < b.clock_micros() + 1_000);
    drop(source);
}
