//! Clip tempo leaders and followers in the app (WP-4.11): the Lead/Follow
//! switch, the tempo map it makes the set play, and Unfollow Tempo
//! Automation. The rules are in the warping spec, R43 to R46.

use bl_core::time::{TempoMap, Ticks};
use bl_model::{AudioData, Clip, ClipData, ClipId, ContentHash, TrackId, TrackKind, WarpMarker};
use bl_shell::Shell;
use serde_json::{Value, json};

const Q: i64 = 705_600;

/// A shell with a new audio track, and that track's ID.
fn new_shell(name: &str) -> (Shell, TrackId) {
    let mut shell = Shell::headless();
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": TrackKind::Audio, "name": name, "index": 0 }),
        )
        .unwrap();
    (shell, track)
}

/// A warped Arrangement clip of `beats` quarter notes on `track`, its file
/// running from 0 to its last marker's time, and its ID.
fn add_clip(
    shell: &mut Shell,
    track: TrackId,
    at: i64,
    beats: i64,
    markers: &[(f64, i64)],
) -> ClipId {
    let id: ClipId = shell.new_id();
    let data = AudioData {
        // A warped clip's End marker is a beat of its contents.
        end: Some((beats * Q) as f64),
        tempo: Some(120.0),
        warp: true,
        warp_markers: markers
            .iter()
            .map(|&(time, beat)| WarpMarker {
                time,
                beat: Ticks(beat),
            })
            .collect(),
        ..AudioData::new(ContentHash([3; 32]))
    };
    let clip = Clip::new(id, Ticks(at), Ticks(beats * Q), ClipData::Audio(data));
    shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    id
}

/// A leader clip whose one segment is `bpm`, over one bar.
fn add_leader(shell: &mut Shell, track: TrackId, at: i64, bpm: f64) -> ClipId {
    // Four quarter notes over a file of this many seconds is `bpm`.
    let seconds = 240.0 / bpm;
    let clip = add_clip(shell, track, at, 4, &[(0.0, 0), (seconds, 4 * Q)]);
    set(
        shell,
        "clip.set_tempo_leader",
        json!({ "clip": clip, "leader": true }),
    );
    clip
}

fn set(shell: &mut Shell, command: &str, args: Value) {
    shell.run(command, args).unwrap();
}

/// The tempo of the set's effective map at `at`.
fn bpm(shell: &Shell, at: i64) -> f64 {
    shell
        .editor()
        .document()
        .tempo_map()
        .bpm_at(Ticks(at))
        .get()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.5
}

// Spec: warping R43, R44
#[test]
fn a_leader_sets_the_tempo_over_its_span_and_followers_follow() {
    let (mut shell, track) = new_shell("Lead");
    // One bar whose two segments are 120 and 60 BPM.
    let clip = add_clip(
        &mut shell,
        track,
        0,
        4,
        &[(0.0, 0), (1.0, 2 * Q), (3.0, 4 * Q)],
    );
    // A follower leaves the song's own tempo alone.
    set(
        &mut shell,
        "clip.set_tempo_leader",
        json!({ "clip": clip, "leader": false }),
    );
    assert_eq!(bpm(&shell, 0), 120.0);
    assert_eq!(bpm(&shell, 2 * Q), 120.0);
    // As a leader the set takes the clip's tempo over its span, its own
    // outside it.
    set(
        &mut shell,
        "clip.set_tempo_leader",
        json!({ "clip": clip, "leader": true }),
    );
    assert_eq!(bpm(&shell, 0), 120.0);
    assert_eq!(bpm(&shell, 2 * Q + 1), 60.0);
    assert_eq!(bpm(&shell, 3 * Q), 60.0);
    assert_eq!(bpm(&shell, 4 * Q), 120.0);
    let document = shell.editor().document();
    assert!(document.any_tempo_leader());
    assert!(document.tempo_leader_covers(Ticks(Q)));
    assert!(!document.tempo_leader_covers(Ticks(4 * Q)));
    // And it is one undo step.
    set(&mut shell, "edit.undo", json!({}));
    assert!(!shell.editor().document().any_tempo_leader());
    set(&mut shell, "edit.redo", json!({}));
    assert!(shell.editor().document().any_tempo_leader());
    // An unwarped clip can't lead.
    let plain: ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({
                "track": track,
                "clip": Clip::new(plain, Ticks(4 * Q), Ticks(4 * Q),
                    ClipData::Audio(AudioData::new(ContentHash([4; 32])))),
            }),
        )
        .unwrap();
    assert!(
        shell
            .run(
                "clip.set_tempo_leader",
                json!({ "clip": plain, "leader": true }),
            )
            .is_err()
    );
}

// Spec: warping R44
#[test]
fn the_bottom_leader_wins() {
    let (mut shell, upper) = new_shell("Upper");
    let lower: TrackId = shell.new_id();
    set(
        &mut shell,
        "track.create",
        json!({ "id": lower, "kind": TrackKind::Audio, "name": "Lower", "index": 1 }),
    );
    let top = add_leader(&mut shell, upper, 0, 30.0);
    let bottom = add_leader(&mut shell, lower, 0, 90.0);
    // Both lead the same span; the one on the track lowest in the list wins.
    assert!(near(bpm(&shell, Q), 90.0), "{}", bpm(&shell, Q));
    set(
        &mut shell,
        "clip.set_mute",
        json!({ "clip": bottom, "mute": true }),
    );
    assert!(
        near(bpm(&shell, Q), 30.0),
        "a deactivated clip leads nothing"
    );
    set(
        &mut shell,
        "clip.set_mute",
        json!({ "clip": bottom, "mute": false }),
    );
    set(
        &mut shell,
        "clip.set_mute",
        json!({ "clip": top, "mute": true }),
    );
    assert!(near(bpm(&shell, Q), 90.0));
}

// Spec: warping R45, R46
#[test]
fn the_leaders_tempo_cannot_be_edited_until_unfollowed() {
    let (mut shell, track) = new_shell("Lead");
    let clip = add_clip(
        &mut shell,
        track,
        0,
        4,
        &[(0.0, 0), (1.0, 2 * Q), (3.0, 4 * Q)],
    );
    set(
        &mut shell,
        "clip.set_tempo_leader",
        json!({ "clip": clip, "leader": true }),
    );
    // While a clip leads, the song's tempo and a breakpoint inside its span
    // are refused.
    assert!(
        shell
            .run(
                "song.set_tempo",
                json!({ "tempo": { "segments": [
                    { "kind": "constant", "start": 0, "bpm": 100.0 },
                ] } }),
            )
            .is_err()
    );
    assert!(
        shell
            .run(
                "song.set_tempo_breakpoint",
                json!({ "at": Q, "bpm": 100.0 }),
            )
            .is_err()
    );
    // Outside the span the envelope is editable.
    set(
        &mut shell,
        "song.set_tempo_breakpoint",
        json!({ "at": 8 * Q, "bpm": 140.0 }),
    );
    // Unfollow Tempo Automation makes every leader a follower and writes the
    // tempo they set into the Song Tempo envelope, so the song plays the
    // same, in one undo step.
    let before: TempoMap = shell.editor().document().tempo_map();
    set(&mut shell, "song.unfollow_tempo_automation", json!({}));
    let after = shell.editor().document().tempo_map();
    assert!(!shell.editor().document().any_tempo_leader());
    for tick in [0, Q / 2, Q, 3 * Q / 2, 3 * Q, 5 * Q, 8 * Q, 9 * Q] {
        assert_eq!(
            after.bpm_at(Ticks(tick)),
            before.bpm_at(Ticks(tick)),
            "{tick}"
        );
    }
    // It is one undo step.
    set(&mut shell, "edit.undo", json!({}));
    assert!(
        shell.editor().document().any_tempo_leader(),
        "one step for the unfollow"
    );
    assert_eq!(shell.editor().document().tempo_map(), before);
    // With the leaders back, the envelope is editable again.
    set(&mut shell, "edit.redo", json!({}));
    assert!(
        shell
            .run(
                "song.set_tempo_breakpoint",
                json!({ "at": Q, "bpm": 100.0 }),
            )
            .is_ok()
    );
}
