//! Two Link instances in one process find each other over the loopback
//! network (the Link spec, R1 to R3, R8, R16). They wait for discovery with
//! a deadline: the one place these tests wait on the wall clock, which
//! `cargo xtask check`'s allowlist names.

use std::time::{Duration, Instant};

use bl_link::{Audio, Link};

/// Waits until `done` holds, up to ten seconds.
fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Two instances that have found each other. The first runs alone for
/// well over the half second within which Link breaks a tie between
/// sessions by their identifiers (a head start of 700 ms still lost one run
/// in four), so it is the older session, the one the second joins.
fn pair(first: f64, second: f64) -> (Link, Link) {
    let a = Link::new(first);
    a.enable(true, first);
    std::thread::sleep(Duration::from_millis(1500));
    let b = Link::new(second);
    b.enable(true, second);
    wait("a peer", || a.peers() == 1 && b.peers() == 1);
    (a, b)
}

/// The session's tempo, as `link`'s audio handle captures it.
fn tempo(audio: &mut Audio) -> f64 {
    audio.capture();
    audio.tempo()
}

/// Commits a tempo from `audio`, as the audio thread does.
fn set_tempo(audio: &mut Audio, bpm: f64) {
    audio.capture();
    let now = audio.clock_micros();
    audio.set_tempo(bpm, now);
    audio.commit();
}

/// Commits a start or stop from `audio`.
fn set_playing(audio: &mut Audio, playing: bool) {
    audio.capture();
    let now = audio.clock_micros();
    audio.set_is_playing(playing, now);
    audio.commit();
}

fn playing(audio: &mut Audio) -> bool {
    audio.capture();
    audio.is_playing()
}

#[test]
fn two_instances_find_each_other() {
    let a = Link::new(120.0);
    let b = Link::new(120.0);
    a.enable(true, 120.0);
    b.enable(true, 120.0);
    wait("a peer", || a.peers() == 1 && b.peers() == 1);
}

#[test]
fn a_tempo_set_on_one_reaches_the_other() {
    let (a, b) = pair(120.0, 120.0);
    let (mut on_a, mut on_b) = (a.audio(), b.audio());
    set_tempo(&mut on_a, 143.0);
    wait("the tempo", || (tempo(&mut on_b) - 143.0).abs() < 1e-3);
    set_tempo(&mut on_b, 87.5);
    wait("the tempo back", || (tempo(&mut on_a) - 87.5).abs() < 1e-3);
}

#[test]
fn enabling_with_no_session_changes_neither_tempo_nor_beat() {
    let link = Link::new(131.0);
    let mut audio = link.audio();
    let later = audio.clock_micros() + 3_000_000;
    audio.capture();
    let beat = audio.beat_at_time(later, 4.0);
    link.enable(true, 131.0);
    audio.capture();
    assert_eq!(audio.tempo(), 131.0);
    assert!((audio.beat_at_time(later, 4.0) - beat).abs() < 1e-6);
    link.enable(false, 131.0);
    audio.capture();
    assert_eq!(audio.tempo(), 131.0);
    assert!((audio.beat_at_time(later, 4.0) - beat).abs() < 1e-6);
}

#[test]
fn joining_takes_the_sessions_tempo() {
    let (a, b) = pair(100.0, 130.0);
    let (mut on_a, mut on_b) = (a.audio(), b.audio());
    wait("b to join", || (tempo(&mut on_b) - 100.0).abs() < 1e-6);
    assert!((tempo(&mut on_a) - 100.0).abs() < 1e-6);
}

#[test]
fn start_and_stop_travel_with_start_stop_sync_on_both() {
    let (a, b) = pair(120.0, 120.0);
    a.set_start_stop_sync(true);
    b.set_start_stop_sync(true);
    let (mut on_a, mut on_b) = (a.audio(), b.audio());
    set_playing(&mut on_a, true);
    wait("the start", || playing(&mut on_b));
    set_playing(&mut on_b, false);
    wait("the stop", || !playing(&mut on_a));
}

#[test]
fn start_and_stop_stay_home_when_one_has_start_stop_sync_off() {
    let (a, b) = pair(120.0, 120.0);
    a.set_start_stop_sync(true);
    b.set_start_stop_sync(false);
    let (mut on_a, mut on_b) = (a.audio(), b.audio());
    set_playing(&mut on_a, true);
    // A tempo change sent after the start arrives on b too, so by the time
    // it's seen, a start that was going to travel has.
    set_tempo(&mut on_a, 99.0);
    wait("the tempo", || (tempo(&mut on_b) - 99.0).abs() < 1e-3);
    assert!(!playing(&mut on_b));
    assert!(playing(&mut on_a));
}

#[test]
fn the_main_threads_snapshot_follows_the_session() {
    let (a, b) = pair(100.0, 130.0);
    wait("b to join", || (b.snapshot(4.0).tempo - 100.0).abs() < 1e-3);
    a.set_tempo(90.0);
    wait("the tempo", || (b.snapshot(4.0).tempo - 90.0).abs() < 1e-3);
}
