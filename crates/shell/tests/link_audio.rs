//! Link Audio in the app (WP-7.16, `docs/specs/link-audio.md` R1 to R12, R14,
//! `docs/specs/routing.md` R37): its switches are kept between starts, the
//! Audio From chooser lists the peers and their channels, a track keeps a
//! peer and channel by name, and two Beatlayer sessions in one process, on
//! the loopback network, send a track from one and record it on the other on
//! the beat it was sent on. Link instances find each other across the
//! process, and the devices run in real time, so these tests wait for them
//! with a deadline and run one at a time with `bl-link`'s
//! (`.config/nextest.toml`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_engine::nodes::{BEAT_TONE, beat_tone};
use bl_link::Link;
use bl_media::MediaStore;
use bl_model::{DeviceId, Input, TrackId};
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::routing::{self, Entry, InOut};
use bl_shell::settings::Preferences;
use bl_shell::Shell;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-link-audio-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn arg(&self, relative: &str) -> String {
        self.0.join(relative).to_string_lossy().into_owned()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell with its preferences in `dir`, as the app starts, on a null
/// device that runs in real time.
fn start(dir: &Path) -> Shell {
    let prefs = Preferences::load(dir).unwrap();
    Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(AudioSettings {
            sample_rate: Some(48_000),
            ..prefs.audio
        }),
        Box::new(ProjectFiles::new()),
    )
}

/// Polls every shell and waits until `done` holds, up to twenty seconds.
fn pump(shells: &mut [&mut Shell], what: &str, mut done: impl FnMut(&mut [&mut Shell]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for shell in shells.iter_mut() {
            shell.poll();
            shell.poll_engine();
        }
        if done(shells) {
            return;
        }
        if Instant::now() >= deadline {
            let states: Vec<Value> = shells.iter_mut().map(|shell| state(shell)).collect();
            let recordings: Vec<Value> = shells
                .iter_mut()
                .map(|shell| shell.run("transport.state", json!({})).unwrap()["last_recording"].clone())
                .collect();
            panic!("timed out waiting for {what}: {recordings:#?} {states:#?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn state(shell: &mut Shell) -> Value {
    shell.run("link.state", json!({})).unwrap()
}

/// A shell in the Link session with Link Audio on under `name`, whose audio
/// runs.
fn in_session(dir: &Path, name: &str) -> Shell {
    let mut shell = start(dir);
    shell.start_audio();
    shell
        .run("link.set_audio_name", json!({ "name": name }))
        .unwrap();
    shell.run("link.set_enabled", json!({ "on": true })).unwrap();
    shell.run("link.set_audio", json!({ "on": true })).unwrap();
    shell
}

/// An audio track called `name`.
fn audio_track(shell: &mut Shell, name: &str) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": name }))
        .unwrap();
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == name)
        .map(bl_model::Track::id)
        .expect("the new track")
}

/// The beat tone on `track`, at full level: a sine locked to the song's
/// beat whose level jumps at each quarter note.
fn beat_tone_on(shell: &mut Shell, track: TrackId) {
    let device: DeviceId = shell.new_id();
    shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 0, "device": {
                "id": device, "name": "Beat Tone", "enabled": true,
                "params": [[beat_tone::LEVEL.0, 0.0]],
                "kind": { "native": { "device_type": BEAT_TONE } } } }),
        )
        .unwrap();
}

fn take_link_audio(shell: &mut Shell, track: TrackId, peer: &str, channel: &str) {
    shell
        .run(
            "routing.set_input",
            json!({ "track": track, "input": {
                "type": "link_audio", "peer": peer, "channel": channel } }),
        )
        .unwrap();
}

fn input_of(shell: &Shell, track: TrackId) -> Input {
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .routing()
        .input
        .clone()
}

/// The peer called `name` in `link.state`, if the session lists one.
fn peer(shell: &mut Shell, name: &str) -> Option<Value> {
    state(shell)["audio"]["peers"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == name)
        .cloned()
}

/// Whether `shell` lists a peer called `peer` whose channels include (or, with
/// `has` false, don't include) `channel`.
fn lists(shell: &mut Shell, peer: &str, channel: &str, has: bool) -> bool {
    state(shell)["audio"]["peers"]
        .as_array()
        .is_some_and(|peers| {
            peers.iter().any(|p| {
                p["name"] == peer
                    && p["channels"]
                        .as_array()
                        .is_some_and(|c| c.iter().any(|c| c == channel) == has)
            })
        })
}

/// A track's Audio From and Input Channel choosers as the In/Out section
/// lists them.
fn choosers(shell: &Shell, track: TrackId) -> (routing::Chooser, routing::Chooser) {
    let section = routing::in_out(shell.editor().document(), track, &shell.interface()).unwrap();
    let InOut::Clips {
        input,
        input_channel,
        ..
    } = section
    else {
        panic!("an audio track's section");
    };
    (input, input_channel)
}

fn labels(chooser: &routing::Chooser) -> Vec<String> {
    chooser
        .entries
        .iter()
        .map(|entry| entry.label().unwrap_or("---").to_owned())
        .collect()
}

// Spec: link-audio R1, R3, R7, R8, R12. Link Audio and Sync to Incoming
// Audio are off in a new install, the Latency is 20 ms and the name the
// computer's; each is kept across a relaunch, the latency within 0 to 200.
#[test]
fn the_link_audio_settings_start_as_the_spec_says_and_are_kept_across_a_relaunch() {
    let dir = TestDir::new("settings");
    let mut shell = start(dir.path());
    let first = state(&mut shell)["audio"].clone();
    assert_eq!(first["enabled"], false);
    assert_eq!(first["sync_to_incoming_audio"], false);
    assert_eq!(first["latency_ms"], 20.0);
    let computer = first["name"].as_str().unwrap().to_owned();
    assert!(!computer.is_empty(), "the computer's name");
    assert_eq!(first["peers"], json!([]));

    assert_eq!(
        shell.run("link.set_audio", json!({})).unwrap()["on"],
        true,
        "left out, it switches to the other state"
    );
    assert_eq!(
        shell
            .run("link.set_audio_name", json!({ "name": "  Studio Mac " }))
            .unwrap()["name"],
        "Studio Mac"
    );
    assert_eq!(
        shell
            .run("link.set_audio_latency", json!({ "ms": 500.0 }))
            .unwrap()["ms"],
        200.0,
        "brought into the range"
    );
    shell
        .run("link.set_audio_latency", json!({ "ms": 55.0 }))
        .unwrap();
    assert_eq!(
        shell
            .run("link.set_sync_to_incoming_audio", json!({ "on": true }))
            .unwrap()["on"],
        true
    );
    drop(shell);

    let mut shell = start(dir.path());
    let kept = state(&mut shell)["audio"].clone();
    assert_eq!(kept["enabled"], true);
    assert_eq!(kept["name"], "Studio Mac");
    assert_eq!(kept["latency_ms"], 55.0);
    assert_eq!(kept["sync_to_incoming_audio"], true);
    // An empty name is the computer's again.
    assert_eq!(
        shell
            .run("link.set_audio_name", json!({ "name": "" }))
            .unwrap()["name"],
        computer
    );
}

// Spec: routing R37; link-audio R5, R10. The Audio From chooser lists the
// peers after Ext. In's and Configure…, before Resampling; choosing one takes
// its first channel, and Input Channel lists its channels in its order. The
// track keeps a peer and channel by name, saved with the set, and lists them
// dimmed and chosen while the peer is gone.
#[test]
fn the_chooser_lists_the_peers_and_their_channels_and_a_track_keeps_them_by_name() {
    let dir = TestDir::new("chooser");
    let other = Link::new(120.0);
    other.enable(true, 120.0);
    other.set_audio_enabled(true);
    other.set_audio_name("Peer");
    let (drums, bass) = (other.sink("Drums"), other.sink("Bass"));
    std::thread::sleep(Duration::from_millis(1500));
    let mut shell = in_session(dir.path(), "Mine");
    let track = audio_track(&mut shell, "Take");
    pump(&mut [&mut shell], "the peer's channels", |shells| {
        peer(shells[0], "Peer").is_some_and(|p| p["channels"].as_array().unwrap().len() == 2)
    });
    let found = peer(&mut shell, "Peer").unwrap();
    let channels: Vec<String> = serde_json::from_value(found["channels"].clone()).unwrap();
    assert_eq!(found["state"], "connected");

    let (from, channel) = choosers(&shell, track);
    let list = labels(&from);
    let at = list.iter().position(|label| label == "Peer").expect("the peer");
    assert_eq!(
        &list[..3],
        ["Ext. In", "Configure…", "---"],
        "the peers are a group of their own after Ext. In's"
    );
    assert_eq!(list[at + 1], "---");
    let resampling = list.iter().position(|label| label == "Resampling").unwrap();
    assert!(at < resampling, "before Resampling: {list:?}");
    assert!(channel.entries.is_empty() || channel.text == "1/2");

    // Choosing the peer takes its first channel; Input Channel lists its
    // channels in its order, the taken one chosen.
    let Entry::Choice { run: Some(run), .. } = &from.entries[at] else {
        panic!("the peer is a choice");
    };
    shell.invoke(&run.id, run.args.clone()).unwrap();
    assert_eq!(
        input_of(&shell, track),
        Input::LinkAudio {
            peer: "Peer".into(),
            channel: channels[0].clone()
        }
    );
    let (from, channel) = choosers(&shell, track);
    assert!(matches!(&from.entries[at], Entry::Choice { chosen: true, .. }));
    assert_eq!(labels(&channel), channels);
    assert_eq!(channel.text, channels[0]);

    // The input is saved with the set by name, and loads by name.
    take_link_audio(&mut shell, track, "Peer", &channels[1]);
    let saved = shell
        .run("set.save_as", json!({ "path": dir.arg("Keep.blset") }))
        .unwrap();
    let set = saved["path"].as_str().unwrap().to_owned();
    shell.run("set.new", json!({})).unwrap();
    shell.run("set.open", json!({ "path": set })).unwrap();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Take")
        .map(bl_model::Track::id)
        .unwrap();
    assert_eq!(
        input_of(&shell, track),
        Input::LinkAudio {
            peer: "Peer".into(),
            channel: channels[1].clone()
        }
    );

    // The peer goes: the track keeps the names, and the chooser lists them
    // dimmed and chosen.
    drop((drums, bass));
    drop(other);
    pump(&mut [&mut shell], "the peer to go", |shells| {
        peer(shells[0], "Peer").is_none()
    });
    assert_eq!(
        input_of(&shell, track),
        Input::LinkAudio {
            peer: "Peer".into(),
            channel: channels[1].clone()
        }
    );
    let (from, channel) = choosers(&shell, track);
    assert!(
        from.entries.iter().any(|entry| matches!(
            entry,
            Entry::Choice { label, run: None, chosen: true, .. } if label == "Peer"
        )),
        "{:?}",
        labels(&from)
    );
    assert!(matches!(
        channel.entries.last(),
        Some(Entry::Choice { run: None, chosen: true, .. })
    ));

    // Link Audio off lists no peer either, and the track still keeps them.
    shell.run("link.set_audio", json!({ "on": false })).unwrap();
    assert_eq!(state(&mut shell)["audio"]["enabled"], false);
    assert!(matches!(input_of(&shell, track), Input::LinkAudio { .. }));
}

/// The onsets of a beat tone in `samples`: its level jumps from a quarter to
/// the full at each quarter note, so an onset is the first frame past 0.3
/// after at least 192 frames at or under it (the tone's cycles are shorter,
/// and its level has fallen under 0.3 by the end of the beat).
fn onsets(samples: &[f32]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut quiet = 0;
    for (i, sample) in samples.iter().enumerate() {
        if sample.abs() > 0.3 {
            if quiet >= 192 {
                found.push(i);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    found
}

// Spec: link-audio R4, R5, R6, R10, R14. Two sessions on the loopback
// network, Link and Link Audio on in both: A's track playing the beat tone is
// offered by its name; B's audio track takes it, armed, and records it, each
// onset within 3 ms of its beat plus the Latency; renaming A's track renames
// the channel; quitting A leaves B's input named and silent, and B hears A
// again when A returns.
#[test]
fn a_track_sent_from_one_session_is_recorded_on_the_beat_in_the_other() {
    const LATENCY_MS: f64 = 20.0;
    let (dir_a, dir_b) = (TestDir::new("a"), TestDir::new("b"));
    let mut a = in_session(dir_a.path(), "alpha");
    // The session's tempo is whatever its first peer set.
    // Alone for well over the half second within which Link breaks a tie
    // between sessions, so A is the older session B joins.
    std::thread::sleep(Duration::from_millis(1500));
    let mut b = in_session(dir_b.path(), "beta");
    let beat = audio_track(&mut a, "Beat");
    beat_tone_on(&mut a, beat);
    let saved = b
        .run("set.save_as", json!({ "path": dir_b.arg("Take.blset") }))
        .unwrap();
    let folder = PathBuf::from(saved["path"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let take = audio_track(&mut b, "Take");
    b.run("link.set_audio_latency", json!({ "ms": LATENCY_MS }))
        .unwrap();

    // A's track is offered by its name.
    pump(&mut [&mut a, &mut b], "A's channel on B", |shells| {
        lists(shells[1], "alpha", "Beat", true)
    });
    take_link_audio(&mut b, take, "alpha", "Beat");
    b.run("mixer.set_arm", json!({ "track": take, "arm": true }))
        .unwrap();
    b.run("routing.set_monitor", json!({ "track": take, "monitor": "in" }))
        .unwrap();

    // Both play; B records. Each starts on the session's bar.
    b.run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    a.run("transport.play", json!({})).unwrap();
    b.run("transport.play", json!({})).unwrap();
    pump(&mut [&mut a, &mut b], "both to play", |shells| {
        shells[0].run("transport.state", json!({})).unwrap()["playing"] == true
            && shells[1].run("transport.state", json!({})).unwrap()["playing"] == true
    });
    pump(&mut [&mut a, &mut b], "audio to arrive on B", |shells| {
        peer(shells[1], "alpha").is_some_and(|p| p["state"] == "receiving")
    });
    let until = Instant::now() + Duration::from_secs(6);
    pump(&mut [&mut a, &mut b], "the take", |_| Instant::now() > until);
    b.run("transport.stop", json!({})).unwrap();
    a.run("transport.stop", json!({})).unwrap();
    let mut clip = None;
    pump(&mut [&mut a, &mut b], "B's clip", |shells| {
        clip = shells[1]
            .editor()
            .document()
            .track(take)
            .and_then(|t| t.clips().next().cloned());
        clip.is_some()
    });
    let clip = clip.unwrap();

    // The file holds the tone; each onset is on a beat, the Latency later.
    let file = std::fs::read_dir(folder.join("Samples").join("Recorded"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|e| e == "wav"))
        .expect("B's recording");
    let media = MediaStore::new();
    let audio = media.load(media.import(&file).unwrap()).unwrap();
    let left = audio.channel(0).unwrap();
    let tempo = b.editor().document().settings().tempo.clone();
    let bpm = state(&mut b)["tempo"].as_f64().unwrap();
    assert!(
        (bpm - tempo.bpm_at(bl_core::time::Ticks::ZERO).get()).abs() < 0.01,
        "the tempo held through the take: {bpm}"
    );
    let beat_frames = 48_000.0 * 60.0 / bpm;
    assert!(left.len() as f64 > 4.0 * beat_frames, "recorded for a few beats");
    let rate = bl_core::time::SampleRate::HZ_48000;
    let started = tempo.ticks_to_frames(clip.start, rate).get() as f64;
    let latency = LATENCY_MS / 1000.0 * 48_000.0;
    let found = onsets(left);
    assert!(found.len() >= 5, "{} onsets", found.len());
    // The first sound is where the channel's first buffer began, which is
    // wherever in a beat the connection was made; every beat after is an
    // onset of the tone.
    for onset in &found[1..] {
        // The session's beats, in B's song: its frames from the clip's start.
        let song = started + *onset as f64 - latency;
        let off = song.rem_euclid(beat_frames);
        let off = off.min(beat_frames - off);
        assert!(
            off <= 144.0,
            "an onset at file frame {onset} is {off:.0} frames ({:.2} ms) from its beat plus the Latency",
            off / 48.0
        );
    }

    // Sending and receiving stayed off the heap on both audio threads.
    for shell in [&a, &b] {
        let stats = shell.audio().stats().expect("a stream is open");
        assert!(stats.blocks > 0);
        assert_eq!(stats.heap_operations, 0, "{stats:?}");
    }

    // Renaming A's track renames the channel; B's input keeps the old name.
    a.run("track.rename", json!({ "track": beat, "name": "Kick" }))
        .unwrap();
    pump(&mut [&mut a, &mut b], "the new channel name", |shells| {
        lists(shells[1], "alpha", "Kick", true) && lists(shells[1], "alpha", "Beat", false)
    });
    assert_eq!(
        input_of(&b, take),
        Input::LinkAudio {
            peer: "alpha".into(),
            channel: "Beat".into()
        },
        "the track keeps the name it took"
    );
    a.run("track.rename", json!({ "track": beat, "name": "Beat" }))
        .unwrap();
    pump(&mut [&mut a, &mut b], "the old name back", |shells| {
        lists(shells[1], "alpha", "Beat", true) && lists(shells[1], "alpha", "Kick", false)
    });

    // Quitting A leaves B's input named and silent.
    drop(a);
    pump(&mut [&mut b], "A to go", |shells| {
        peer(shells[0], "alpha").is_none()
    });
    assert_eq!(
        input_of(&b, take),
        Input::LinkAudio {
            peer: "alpha".into(),
            channel: "Beat".into()
        }
    );

    // A returns, with a track of that name: B hears it again.
    let dir_c = TestDir::new("c");
    let mut a = in_session(dir_c.path(), "alpha");
    let beat = audio_track(&mut a, "Beat");
    beat_tone_on(&mut a, beat);
    pump(&mut [&mut a, &mut b], "A's channel to return", |shells| {
        lists(shells[1], "alpha", "Beat", true)
    });
    a.run("transport.play", json!({})).unwrap();
    b.run("transport.play", json!({})).unwrap();
    pump(&mut [&mut a, &mut b], "B to hear A again", |shells| {
        peer(shells[1], "alpha").is_some_and(|p| p["state"] == "receiving")
    });
}
