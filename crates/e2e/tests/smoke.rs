//! The harness's own checks: each piece of `bl-e2e` comes up and does what
//! the test families rely on. A failure here is a harness bug, unless it
//! says Beatlayer is at fault, in which case it names the bug.

use bl_e2e::audio::{
    aligned_residual_db, assert_finite, assert_sounds, best_alignment, level_difference_db, overlap,
};
use bl_e2e::fixture::{self, POLYSYNTH};
use bl_e2e::host::HostOptions;
use bl_e2e::{BAR, BEAT, Driver, ExportSpec, Host, Known, StdioHost, document_diff};
use serde_json::json;

#[test]
fn the_host_comes_up_with_the_engine_running_and_the_device_paced_by_hand() {
    let mut host = Host::new();
    assert!(host.shell().audio().engine_running());
    let status = host
        .tool("transport", json!({ "action": "status" }))
        .unwrap();
    assert_eq!(status["engine_plays_here"], true, "{status}");
    assert_eq!(status["playing"], false, "{status}");
    // The device runs only the blocks the host asks for.
    let blocks = |host: &mut Host| host.shell().audio().stats().map_or(0, |s| s.blocks);
    let before = blocks(&mut host);
    host.advance_ms(100);
    let after = blocks(&mut host);
    assert!(after > before, "{before} blocks, then {after}");
    // Nothing plays, so the device plays silence.
    let quiet = host.capture_for(50);
    assert!(quiet.frames() >= 2_400, "{} frames", quiet.frames());
    assert!(quiet.is_silent(-120.0), "{} dBFS", quiet.peak_dbfs());
}

/// The default set's first MIDI track with Cascade on it and a one-bar clip
/// of a held C major chord at velocity 100: the track's ID.
fn one_synth(host: &mut Host) -> u64 {
    let track = host.query(json!({ "path": "track \"1 MIDI\"" }))["id"]
        .as_u64()
        .unwrap();
    let notes = [60, 64, 67].map(|key| (key, 0, BAR - BEAT, 100));
    host.batch_ok(
        "A chord",
        vec![
            json!({ "command": "device.insert", "args": { "track": track, "index": 0,
                    "device": fixture::native("cascade", POLYSYNTH, "instrument") } }),
            json!({ "command": "clip.create", "args": { "track": track,
                    "clip": fixture::midi_clip("Chord", 0, BAR, fixture::midi_data(notes)) } }),
        ],
    );
    host.settle();
    track
}

#[test]
fn a_midi_clip_through_a_native_instrument_plays_live() {
    let mut host = Host::new();
    one_synth(&mut host);
    // A bar at 120 BPM is two seconds; half a second more hears the release.
    let live = host.play_for(2_500);
    assert!(live.frames() >= 120_000, "{} frames", live.frames());
    assert_sounds(&live, "the live capture", -40.0);
    eprintln!(
        "live: peak {:.1} dBFS, rms {:.1} dBFS",
        live.peak_dbfs(),
        live.rms_dbfs()
    );
}

#[test]
fn the_same_bar_exported_offline_matches_the_live_capture_once_aligned() {
    let mut host = Host::new();
    one_synth(&mut host);
    let live = host.play_for(2_500);
    let offline = host.export(0, BAR, &ExportSpec::default());
    assert_eq!(offline.rate, live.rate);
    assert_eq!(offline.frames(), 96_000, "a bar at 120 BPM");
    assert_sounds(&offline, "the export", -40.0);
    let alignment = best_alignment(&offline, &live, 4_800, 8_192);
    let (exported, played) = overlap(&offline, &live, alignment.lag);
    let level = level_difference_db(&exported, &played);
    let residual = aligned_residual_db(&exported, &played, 0);
    eprintln!(
        "live against offline: lag {} frames, correlation {:.4}, level {level:+.2} dB, \
         residual {residual:.1} dB over {} frames",
        alignment.lag,
        alignment.correlation,
        exported.frames()
    );
    assert!(
        alignment.lag.unsigned_abs() < 4_800,
        "the live capture lines up within 100 ms: {alignment:?}"
    );
    assert!(level.abs() < 1.0, "the levels agree within 1 dB: {level}");
    assert!(
        residual < -20.0,
        "the live capture and the export are the same sound: residual {residual:.1} dB, \
         correlation {:.4}",
        alignment.correlation
    );
}

#[test]
fn an_export_the_person_allows_in_the_app_answers_its_agent() {
    // Spec: export R44. In the app an agent that isn't autonomous asks
    // before it exports (an external command), and once the person allows
    // it the call waits for the export and answers with its result.
    let mut host = Host::with_options(HostOptions {
        autonomous: false,
        ..HostOptions::default()
    });
    one_synth(&mut host);
    let outcome = host.export_file(0, BAR, &ExportSpec::default());
    assert!(
        host.app().asked.iter().any(|q| q.contains("Export Audio")),
        "the person was asked: {:?}",
        host.app().asked
    );
    let mut ratchet = Known::load().ratchet("smoke");
    ratchet.check(
        "export.audio in the app, allowed by the person",
        "the call answers with the export's result",
        outcome.map(drop),
    );
    ratchet.finish();
}

#[test]
fn the_stdio_host_answers_runs_a_command_and_a_query_and_builds_the_rich_set() {
    let mut host = StdioHost::start();
    assert_eq!(host.request("ping", json!({})).unwrap(), json!({}));
    let overview = host.query(json!({}));
    assert_eq!(overview["counts"]["tracks"], 7, "{overview}");
    let made = host
        .tool(
            "run",
            json!({ "commands": [{ "command": "track.create", "args": {
                "id": { "$new": "Lead" }, "kind": "midi", "name": "Lead", "index": 0 } }] }),
        )
        .unwrap();
    let lead = made["ids"]["Lead"].as_u64().unwrap();
    let read = host.query(json!({ "path": "track \"Lead\"" }));
    assert_eq!(read["id"], lead, "{read}");
    // The rich set builds headless too, as `bl-cli mcp` serves it.
    let ids = fixture::build(&mut host);
    let overview = host.query(json!({}));
    assert_eq!(
        overview["counts"]["tracks"].as_u64().unwrap(),
        ids.tracks.len() as u64,
        "{overview}"
    );
    host.finish().unwrap();
}

#[test]
fn the_rich_set_builds_through_the_agent_api_and_keeps_its_invariants() {
    let mut host = Host::new();
    let ids = fixture::build(&mut host);
    host.check();
    assert_eq!(ids.tracks.len(), 8);
    assert_eq!(ids.midi_tracks.len(), 2);
    assert_eq!(ids.audio_tracks.len(), 2);
    assert_eq!((ids.groups.len(), ids.returns.len()), (1, 2));
    assert_eq!(ids.arrangement.midi.len(), 2);
    assert_eq!(ids.arrangement.audio.len(), 2);
    assert_eq!(ids.session.midi.len(), 2);
    assert_eq!(ids.session.audio.len(), 1);
    assert_eq!(ids.devices.values().map(Vec::len).sum::<usize>(), 12);
    assert_eq!(ids.locators.len(), 2);
    assert_eq!(ids.scenes.len(), 9, "the default eight and Drop");
    // Every ID it gives reads back.
    for id in ids
        .tracks
        .values()
        .chain(ids.clips.values())
        .copied()
        .chain(ids.all_devices())
    {
        let read = host.tool("query", json!({ "id": id }));
        assert!(read.is_ok(), "{id}: {read:?}");
    }
    // The audio clips' media is in, and the first two bars sound.
    let first = host.export(0, 2 * BAR, &ExportSpec::default());
    assert_sounds(&first, "the rich set's first two bars", -30.0);
}

#[test]
fn the_rich_set_saved_and_reopened_is_the_same_document() {
    let mut host = Host::new();
    fixture::build(&mut host);
    let dir = host.dir().to_owned();
    let path = host.save_to(&dir);
    assert!(path.exists(), "{}", path.display());
    assert!(!host.is_dirty());
    let saved = host.document().clone();
    host.reopen().unwrap();
    let reopened = host.document().clone();
    let diff = document_diff(&saved, &reopened, 10);
    assert!(
        saved == reopened,
        "the reopened set differs from the saved one:\n{}",
        diff.join("\n")
    );
    host.check();
}

#[test]
fn undo_of_an_edit_to_the_rich_set_restores_it_and_redo_reapplies_it() {
    let mut host = Host::new();
    let ids = fixture::build(&mut host);
    let before = host.document().clone();
    host.run_ok(
        "mixer.set_volume",
        json!({ "track": ids.track("Keys"), "volume": 0.25 }),
    );
    host.run_ok(
        "device.set_param",
        json!({ "device": ids.device(POLYSYNTH), "param": 27, "value": 0.5 }),
    );
    let after = host.document().clone();
    assert!(before != after, "the edits changed the document");
    host.run_ok("edit.undo", json!({}));
    host.run_ok("edit.undo", json!({}));
    let undone = host.document().clone();
    assert!(
        undone == before,
        "undo didn't restore the document:\n{}",
        document_diff(&before, &undone, 10).join("\n")
    );
    host.run_ok("edit.redo", json!({}));
    host.run_ok("edit.redo", json!({}));
    let redone = host.document().clone();
    assert!(
        redone == after,
        "redo didn't reapply the edits:\n{}",
        document_diff(&after, &redone, 10).join("\n")
    );
    host.check();
}

#[test]
fn the_known_failures_list_parses_and_its_ratchet_counts() {
    let known = Known::load();
    for entry in known.entries() {
        eprintln!(
            "known: {} / {} / {}: {}",
            entry.suite, entry.key, entry.property, entry.description
        );
    }
    let parsed = Known::parse(
        "[[failure]]\nsuite = \"smoke\"\nkey = \"k\"\nproperty = \"p\"\ndescription = \"d\"\n",
    )
    .unwrap();
    let mut ratchet = parsed.ratchet("smoke");
    ratchet.check("k", "p", Err("still broken".to_owned()));
    ratchet.check("other", "p", Ok(()));
    assert_eq!(ratchet.checked(), 2);
    ratchet.finish();
    // A listed failure that passes fails the ratchet.
    let mut fixed = parsed.ratchet("smoke");
    fixed.check("k", "p", Ok(()));
    assert!(std::panic::catch_unwind(move || fixed.finish()).is_err());
    assert!(Known::parse("[[failure]]\nsuite = \"x\"\n").is_err());
}

#[test]
fn measurements_line_up_a_delayed_copy() {
    // Noise from a small generator, so only one lag matches.
    let mut state = 0x2545_f491_u32;
    let noise: Vec<f32> = (0..48_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32 - 0.5) * 0.5
        })
        .collect();
    let reference = bl_e2e::Audio::new(48_000, vec![noise.clone(), noise]);
    let mut delayed = reference.clone();
    for channel in &mut delayed.channels {
        channel.splice(0..0, std::iter::repeat_n(0.0, 123));
        channel.truncate(48_000);
    }
    assert_finite(&delayed, "the delayed noise");
    let alignment = best_alignment(&reference, &delayed, 1_000, 4_096);
    assert_eq!(alignment.lag, 123, "{alignment:?}");
    assert!(alignment.correlation > 0.999, "{alignment:?}");
    let residual = aligned_residual_db(&reference, &delayed, alignment.lag);
    assert!(residual < -100.0, "residual {residual}");
    // A full-scale sine's RMS is -3 dBFS; at half scale, -9.
    let sine = bl_e2e::audio::sine(48_000, 1_000.0, 0.5, 48_000, 2);
    assert!((sine.rms_dbfs() + 9.03).abs() < 0.05, "{}", sine.rms_dbfs());
    assert!(
        (sine.peak_dbfs() + 6.02).abs() < 0.05,
        "{}",
        sine.peak_dbfs()
    );
    let lufs = sine.integrated_lufs().unwrap();
    assert!(lufs.is_finite(), "{lufs}");
}
