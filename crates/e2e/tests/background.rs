//! Background work, finished as an agent finishes it: every command that
//! starts work off the main thread (a render, a processed file, an
//! analysis, an export, a pack, the autosampler) runs on the rich set and
//! lands its result while the agent does nothing but call tools.
//!
//! The first host is [`StdioHost`], the agent API exactly as `bl-cli mcp`
//! serves it, with no window: nothing in the harness polls the shell for it,
//! so work that lands, lands because the stdio transport made it land. The
//! same cases run again on [`Host`], the app's wiring with its window, so a
//! command that only finishes in one of them shows. Work a command returns
//! before finishing is polled through its own status command or a query, a
//! bounded number of calls, as an agent would poll it.
//!
//! Each case is checked through the known-failures ratchet: the key is the
//! command, the property `finishes without a window` on the stdio host and
//! `finishes in the app` on the app's.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use bl_e2e::audio::assert_sounds;
use bl_e2e::fixture::{self, POLYSYNTH, SAMPLER};
use bl_e2e::{Audio, BAR, BEAT, Driver, Host, Ids, Known, StdioHost, TempDir};
use serde_json::{Value, json};

/// The ratchet's suite.
const SUITE: &str = "background";

/// The property on the stdio host.
const HEADLESS: &str = "finishes without a window";

/// The property on the app's host.
const IN_APP: &str = "finishes in the app";

/// The most tool calls a poll makes before it gives up. Each call polls
/// the shell on the stdio host and runs a frame on the app's, so this is
/// far more than any of the small jobs here needs; it fails a job that
/// never lands rather than hanging the suite.
const POLLS: usize = 200_000;

/// What a case works with: the rich set's IDs, the folder the set is saved
/// in, and a folder for files the case writes.
struct Ctx {
    ids: Ids,
    project: PathBuf,
    out: PathBuf,
}

impl Ctx {
    /// A path in the output folder.
    fn file(&self, name: &str) -> String {
        self.out.join(name).to_string_lossy().into_owned()
    }
}

/// One command that starts background work, and how its result is checked.
struct Case {
    /// The command, which is the ratchet's key.
    key: &'static str,
    /// Runs it and checks what landed.
    run: fn(&mut dyn Driver, &Ctx) -> Result<(), String>,
}

/// Runs `command` through the `run` tool, as an error message on failure.
fn run(driver: &mut dyn Driver, command: &str, args: Value) -> Result<Value, String> {
    driver
        .run(command, args)
        .map_err(|error| format!("`{command}` failed: {error}"))
}

/// The `query` tool, as an error message on failure.
fn query(driver: &mut dyn Driver, args: Value) -> Result<Value, String> {
    driver
        .tool("query", args.clone())
        .map_err(|error| format!("query {args} failed: {error}"))
}

/// Calls `look` until it gives `Some`, at most [`POLLS`] times: the agent's
/// way of waiting for background work, one tool call after another, with
/// the thread yielding between them.
fn poll<T>(
    driver: &mut dyn Driver,
    what: &str,
    mut look: impl FnMut(&mut dyn Driver) -> Result<Option<T>, String>,
) -> Result<T, String> {
    for calls in 1..=POLLS {
        if let Some(found) = look(driver)? {
            eprintln!("{what}: landed after {calls} calls");
            return Ok(found);
        }
        std::thread::yield_now();
    }
    Err(format!("{what} never landed in {POLLS} tool calls"))
}

/// Every track's ID, name and kind, top to bottom.
fn tracks(driver: &mut dyn Driver) -> Result<Vec<(u64, String, String)>, String> {
    let read = query(driver, json!({ "path": "tracks", "limit": 500 }))?;
    let items = read["tracks"]["items"]
        .as_array()
        .ok_or_else(|| format!("no tracks in {read}"))?;
    Ok(items
        .iter()
        .map(|t| {
            (
                t["id"].as_u64().unwrap_or_default(),
                t["name"].as_str().unwrap_or_default().to_owned(),
                t["kind"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect())
}

/// The track called `name`, read in full.
fn track(driver: &mut dyn Driver, name: &str) -> Result<Value, String> {
    query(
        driver,
        json!({ "path": format!("track \"{name}\""), "detail": "full" }),
    )
}

/// An object read in full by ID.
fn object(driver: &mut dyn Driver, id: u64) -> Result<Value, String> {
    query(driver, json!({ "id": id, "detail": "full" }))
}

/// Fails unless `path` is a file with something in it.
fn written(path: &str) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("{path} wasn't written: {error}"))?
        .len();
    if size == 0 {
        return Err(format!("{path} is empty"));
    }
    Ok(())
}

/// Fails unless the audio file at `path` decodes and sounds.
fn sounds(path: &str, what: &str) -> Result<Audio, String> {
    written(path)?;
    let audio = Audio::decode(Path::new(path))?;
    catch_unwind(AssertUnwindSafe(|| assert_sounds(&audio, what, -60.0)))
        .map_err(|_| format!("{what} ({path}) is silent or not finite"))?;
    Ok(audio)
}

/// Every string in `value` that names an existing file, depth first.
fn paths_in(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::String(text) if Path::new(text).is_file() => out.push(text.clone()),
            Value::Array(items) => stack.extend(items),
            Value::Object(fields) => stack.extend(fields.values()),
            _ => {}
        }
    }
    out
}

/// The clips a track holds in the Arrangement, by ID, from a full read.
fn clip_ids(track: &Value) -> Vec<u64> {
    track["clips"]["items"]
        .as_array()
        .map(|items| items.iter().filter_map(|c| c["id"].as_u64()).collect())
        .unwrap_or_default()
}

/// The media hash an audio clip plays, from a full read.
fn media_of(clip: &Value) -> Result<String, String> {
    clip["saved"]["data"]["audio"]["media"]
        .as_str()
        .or_else(|| clip["content"]["media"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("no media in {clip}"))
}

/// Fails unless every file `result` lists under `field` was written and
/// sounds, and there is at least one.
fn files_sound(result: &Value, field: &str, what: &str) -> Result<(), String> {
    let files = result[field]
        .as_array()
        .ok_or_else(|| format!("no `{field}` in {result}"))?;
    if files.is_empty() {
        return Err(format!("{what} listed no files: {result}"));
    }
    for file in files {
        let path = file
            .as_str()
            .or_else(|| file["path"].as_str())
            .ok_or_else(|| format!("a file that isn't a path in {result}"))?;
        sounds(path, what)?;
    }
    Ok(())
}

/// When the clicks [`clicks`] writes start, in seconds.
const CLICKS: [f64; 6] = [0.1, 0.35, 0.6, 0.85, 1.2, 1.5];

/// Writes a two-second mono WAV file at 48 kHz of short decaying clicks
/// starting at `times` seconds: a file this session hasn't seen, so its
/// analysis starts from nothing.
fn clicks(path: &Path, times: &[f64]) -> Result<(), String> {
    let rate = 48_000u32;
    let mut samples = vec![0.0f32; 2 * rate as usize];
    for &time in times {
        let at = (time * f64::from(rate)) as usize;
        for n in 0..2_400 {
            let decay = (-(n as f32) / 300.0).exp();
            let tone = (n as f32 * 0.37).sin();
            if let Some(sample) = samples.get_mut(at + n) {
                *sample += 0.8 * decay * tone;
            }
        }
    }
    let spec = bl_media::EncodeSpec {
        file_type: bl_media::FileType::Wav,
        bit_depth: bl_media::BitDepth::Int24,
        dither: bl_media::Dither::None,
        sample_rate: rate,
        channels: 1,
    };
    let mut writer = bl_media::AudioWriter::create(path, spec).map_err(|e| e.to_string())?;
    writer.write(&samples).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// Fails unless `audio` lasts the rich set's first two bars. The tempo
/// rises in a straight line from 120 BPM at bar 1 to 128 at bar 5, so
/// the first eight beats take `120 ln(124 / 120)` seconds.
fn two_bars(audio: &Audio) -> Result<(), String> {
    let seconds = 120.0 * (124.0f64 / 120.0).ln();
    let expected = (seconds * f64::from(audio.rate)).round() as i64;
    let frames = audio.frames() as i64;
    // Within a millisecond: the render ends on the block holding the end.
    if (frames - expected).abs() > i64::from(audio.rate) / 1_000 {
        return Err(format!(
            "the export is {frames} frames, not two bars ({expected})"
        ));
    }
    Ok(())
}

/// Fails unless `path` lies inside the project folder's processed files
/// of `kind` (the audio clip editing spec, R1).
fn processed(ctx: &Ctx, path: &str, kind: &str) -> Result<(), String> {
    let folder = ctx.project.join("Samples/Processed").join(kind);
    if Path::new(path).starts_with(&folder) {
        Ok(())
    } else {
        Err(format!("{path} isn't in {}", folder.display()))
    }
}

/// The cases, in the order they run. Each starts from the rich set as
/// saved (a checkpoint restored), so none depends on another.
#[allow(clippy::too_many_lines)]
fn cases() -> Vec<Case> {
    vec![
        Case {
            key: "track.freeze",
            run: |driver, ctx| {
                let keys = ctx.ids.track("Keys");
                let frozen = run(driver, "track.freeze", json!({ "tracks": [keys] }))?;
                if frozen["frozen"] != json!([keys]) {
                    return Err(format!("Keys wasn't frozen: {frozen}"));
                }
                files_sound(&frozen, "files", "the freeze file")?;
                for file in paths_in(&frozen["files"]) {
                    processed(ctx, &file, "Freeze")?;
                }
                let read = object(driver, keys)?;
                if read["frozen"]["clips"].as_array().is_none_or(Vec::is_empty) {
                    return Err(format!("Keys reads back unfrozen: {}", read["frozen"]));
                }
                run(driver, "track.unfreeze", json!({ "tracks": [keys] }))?;
                let read = object(driver, keys)?;
                if !read["frozen"].is_null() {
                    return Err(format!("Keys reads back frozen: {}", read["frozen"]));
                }
                Ok(())
            },
        },
        Case {
            key: "track.bounce_in_place",
            run: |driver, ctx| {
                let bass = ctx.ids.track("Bass");
                let bounced = run(driver, "track.bounce_in_place", json!({ "tracks": [bass] }))?;
                files_sound(&bounced, "files", "the bounced file")?;
                let new = bounced["tracks"][0]
                    .as_u64()
                    .ok_or_else(|| format!("no track in {bounced}"))?;
                let read = object(driver, new)?;
                if read["kind"] != "audio"
                    || read["devices"].as_array().is_some_and(|d| !d.is_empty())
                {
                    return Err(format!("the bounce isn't a bare audio track: {read}"));
                }
                if clip_ids(&read).is_empty() {
                    return Err(format!("the bounced track holds no clips: {read}"));
                }
                Ok(())
            },
        },
        Case {
            key: "track.bounce_to_new_track",
            run: |driver, ctx| {
                let tone = ctx.ids.clip("Tone");
                let bounced = run(
                    driver,
                    "track.bounce_to_new_track",
                    json!({ "tracks": [ctx.ids.track("Tone")], "clips": [tone] }),
                )?;
                files_sound(&bounced, "files", "the bounced file")?;
                let clip = bounced["clips"][0]
                    .as_u64()
                    .ok_or_else(|| format!("no clip in {bounced}"))?;
                let read = object(driver, clip)?;
                media_of(&read)?;
                Ok(())
            },
        },
        Case {
            key: "track.paste_bounced_audio",
            run: |driver, ctx| {
                run(
                    driver,
                    "arrangement.copy",
                    json!({ "clips": [ctx.ids.clip("Chords")] }),
                )?;
                let pasted = run(
                    driver,
                    "track.paste_bounced_audio",
                    json!({ "track": ctx.ids.track("Tone"), "at": 4 * BAR }),
                )?;
                files_sound(&pasted, "files", "the pasted audio")?;
                Ok(())
            },
        },
        Case {
            key: "export.audio",
            run: |driver, ctx| {
                let path = ctx.file("export.wav");
                run(
                    driver,
                    "export.audio",
                    json!({ "path": path, "start": 0, "length": 2 * BAR, "bit_depth": 24 }),
                )?;
                let audio = sounds(&path, "the export")?;
                two_bars(&audio)
            },
        },
        Case {
            key: "export.start",
            run: |driver, ctx| {
                // Spec: export R41. The dialog's export runs on its thread
                // and its end lands while the agent reads export.status.
                let path = ctx.file("dialog export.wav");
                run(driver, "export.open_dialog", json!({}))?;
                run(
                    driver,
                    "export.set_choices",
                    json!({ "start": 0, "length": 2 * BAR, "file_type": "wav", "bit_depth": 24 }),
                )?;
                run(
                    driver,
                    "export.start",
                    json!({ "path": path, "replace": true }),
                )?;
                poll(driver, "the dialog's export", |driver| {
                    let status = run(driver, "export.status", json!({}))?;
                    Ok(status["export"].is_null().then_some(()))
                })?;
                let _ = run(driver, "export.close_dialog", json!({}));
                let audio = sounds(&path, "the dialog's export")?;
                two_bars(&audio)
            },
        },
        Case {
            key: "clip.reverse",
            run: |driver, ctx| {
                let clip = ctx.ids.clip("Loop");
                let before = media_of(&object(driver, clip)?)?;
                let reversed = run(driver, "clip.reverse", json!({ "clip": clip }))?;
                let after = media_of(&object(driver, clip)?)?;
                if before == after {
                    return Err(format!("the clip plays the same file: {reversed}"));
                }
                for file in paths_in(&reversed) {
                    processed(ctx, &file, "Reverse")?;
                    sounds(&file, "the reversed file")?;
                }
                Ok(())
            },
        },
        Case {
            key: "clip.crop",
            run: |driver, ctx| {
                let clip = ctx.ids.clip("Tone");
                let before = media_of(&object(driver, clip)?)?;
                let cropped = run(
                    driver,
                    "clip.crop",
                    // The sine lasts an eighth note from bar 2.
                    json!({ "clip": clip, "start": BAR + BEAT / 8, "end": BAR + 3 * BEAT / 8 }),
                )?;
                let after = media_of(&object(driver, clip)?)?;
                if before == after {
                    return Err(format!("the clip plays the same file: {cropped}"));
                }
                Ok(())
            },
        },
        Case {
            key: "arrangement.reverse",
            run: |driver, ctx| {
                let track = ctx.ids.track("Loop");
                let original = media_of(&object(driver, ctx.ids.clip("Loop"))?)?;
                run(
                    driver,
                    "arrangement.reverse",
                    json!({ "tracks": [track], "start": 0, "end": BAR }),
                )?;
                let after = object(driver, track)?;
                let first = clip_ids(&after)
                    .first()
                    .copied()
                    .ok_or_else(|| format!("no clips left on Loop: {after}"))?;
                let media = media_of(&object(driver, first)?)?;
                if media == original {
                    return Err("the span's first clip plays the original file".to_owned());
                }
                Ok(())
            },
        },
        Case {
            key: "arrangement.consolidate",
            run: |driver, ctx| {
                let made = run(
                    driver,
                    "arrangement.consolidate",
                    json!({ "tracks": [ctx.ids.track("Loop"), ctx.ids.track("Tone")],
                            "start": 0, "end": 2 * BAR }),
                )?;
                files_sound(&made, "files", "the consolidated file")?;
                for file in paths_in(&made["files"]) {
                    processed(ctx, &file, "Consolidate")?;
                }
                Ok(())
            },
        },
        Case {
            key: "arrangement.consolidate_to_scene",
            run: |driver, ctx| {
                let before = fixture_scenes(driver)?;
                run(
                    driver,
                    "arrangement.consolidate_to_scene",
                    json!({ "tracks": [ctx.ids.track("Loop"), ctx.ids.track("Keys")],
                            "start": 0, "end": BAR, "scene": ctx.ids.scenes[0] }),
                )?;
                let after = fixture_scenes(driver)?;
                if after.len() != before.len() + 1 {
                    return Err(format!("{} scenes, then {}", before.len(), after.len()));
                }
                let scene = query(driver, json!({ "path": "scene 2", "detail": "full" }))?;
                if !scene.to_string().contains("\"media\"") {
                    return Err(format!("the new scene holds no audio clip: {scene}"));
                }
                Ok(())
            },
        },
        Case {
            key: "device.reverse_sample",
            run: |driver, ctx| {
                let device = ctx.ids.device(SAMPLER);
                let reversed = run(driver, "device.reverse_sample", json!({ "device": device }))?;
                let files = paths_in(&reversed);
                if files.is_empty() {
                    return Err(format!("no file named in {reversed}"));
                }
                for file in files {
                    processed(ctx, &file, "Reverse")?;
                    sounds(&file, "the reversed sample")?;
                }
                Ok(())
            },
        },
        Case {
            key: "device.crop_sample",
            run: |driver, ctx| {
                let device = ctx.ids.device(SAMPLER);
                let region = run(
                    driver,
                    "device.set_sample_region",
                    json!({ "device": device, "start": 0, "end": 2_000 }),
                )?;
                let cropped = run(driver, "device.crop_sample", json!({ "device": device }))?;
                let files = paths_in(&cropped);
                if files.is_empty() {
                    return Err(format!("no file named in {cropped} (region {region})"));
                }
                for file in files {
                    processed(ctx, &file, "Crop")?;
                    let audio = Audio::decode(Path::new(&file))?;
                    if audio.frames() != 2_000 {
                        return Err(format!("{} frames cropped to 2,000", audio.frames()));
                    }
                }
                Ok(())
            },
        },
        Case {
            key: "clip.transients",
            run: |driver, ctx| {
                // Spec: warping R20, R24. A file no one has analysed yet is
                // analysed in the background; the agent reads the clip's
                // transients until they're ready. Unwarped One Shot adds the
                // clip without waiting for the analysis.
                let path = ctx.out.join("clicks.wav");
                clicks(&path, &CLICKS)?;
                run(
                    driver,
                    "settings.set_loop_warp_short_samples",
                    json!({ "mode": "unwarped_one_shot" }),
                )?;
                let placed = run(
                    driver,
                    "browser.add_to_arrangement",
                    json!({ "paths": [path.to_string_lossy()], "at": 0 }),
                );
                run(
                    driver,
                    "settings.set_loop_warp_short_samples",
                    json!({ "mode": "auto" }),
                )?;
                let placed = placed?;
                let clip = placed["clips"][0]["clip"]
                    .as_u64()
                    .ok_or_else(|| format!("no clip placed: {placed}"))?;
                let found = poll(driver, "the clicks' analysis", |driver| {
                    let read = run(driver, "clip.transients", json!({ "clip": clip }))?;
                    if let Some(error) = read["analysis_error"].as_str() {
                        return Err(format!("the analysis failed: {error}"));
                    }
                    Ok((read["analysis_ready"] == true).then_some(read))
                })?;
                let times: Vec<f64> = found["transients"]
                    .as_array()
                    .map(|items| items.iter().filter_map(|t| t["time"].as_f64()).collect())
                    .unwrap_or_default();
                // Each click is found, within 10 ms.
                for click in CLICKS {
                    if !times.iter().any(|time| (time - click).abs() < 0.01) {
                        return Err(format!("no transient at the click at {click} s: {times:?}"));
                    }
                }
                Ok(())
            },
        },
        Case {
            key: "browser.add_to_arrangement",
            run: |driver, ctx| {
                // Spec: warping R33, R36. A short sample added from the
                // browser under Warped Loop is analysed and warped before
                // the call returns. The drum loop's beat isn't steady enough
                // for Auto to call it a loop, so the setting says so.
                run(
                    driver,
                    "settings.set_loop_warp_short_samples",
                    json!({ "mode": "warped_loop" }),
                )?;
                // A copy: adding a file may write beside it.
                let copy = ctx.out.join("drum-loop.wav");
                std::fs::copy(fixture::drums("drum-loop.wav"), &copy)
                    .map_err(|e| format!("copying the drum loop: {e}"))?;
                let placed = run(
                    driver,
                    "browser.add_to_arrangement",
                    json!({ "paths": [copy.to_string_lossy()], "at": 0 }),
                )?;
                let clip = placed["clips"][0]["clip"]
                    .as_u64()
                    .ok_or_else(|| format!("no clip placed: {placed}"))?;
                let read = object(driver, clip)?;
                run(
                    driver,
                    "settings.set_loop_warp_short_samples",
                    json!({ "mode": "auto" }),
                )?;
                let audio = &read["saved"]["data"]["audio"];
                if audio["warp"] != true {
                    return Err(format!("the loop wasn't warped: {audio}"));
                }
                let found = poll(driver, "the new clip's transients", |driver| {
                    let read = run(driver, "clip.transients", json!({ "clip": clip }))?;
                    Ok((read["analysis_ready"] == true).then_some(read))
                })?;
                if found["transients"].as_array().is_none_or(Vec::is_empty) {
                    return Err(format!("no transients: {found}"));
                }
                Ok(())
            },
        },
        Case {
            key: "clip.slice_to_midi_track",
            run: |driver, ctx| {
                let sliced = run(
                    driver,
                    "clip.slice_to_midi_track",
                    json!({ "clip": ctx.ids.clip("Loop"), "by": "quarter" }),
                )?;
                let slices = sliced["slices"].as_u64().unwrap_or_default();
                if slices < 4 {
                    return Err(format!("{slices} slices of a bar's drum loop: {sliced}"));
                }
                let track = sliced["track"]
                    .as_u64()
                    .ok_or_else(|| format!("no track in {sliced}"))?;
                let read = object(driver, track)?;
                if read["kind"] != "midi" || read["devices"].as_array().is_none_or(Vec::is_empty) {
                    return Err(format!("the sliced track has no instrument: {read}"));
                }
                Ok(())
            },
        },
        Case {
            key: "clip.slice_to_midi_track of a warped clip",
            run: |driver, ctx| {
                // Spec: slice R6. A warped clip is sliced from its warped
                // audio, rendered as Consolidate renders it.
                let clip = ctx.ids.clip("Loop");
                run(
                    driver,
                    "clip.set_warp",
                    json!({ "clip": clip, "warp": true }),
                )?;
                let sliced = run(
                    driver,
                    "clip.slice_to_midi_track",
                    json!({ "clip": clip, "by": "quarter", "preserve_warped_timing": true }),
                )?;
                let slices = sliced["slices"].as_u64().unwrap_or_default();
                if slices < 4 {
                    return Err(format!("{slices} slices of a bar's drum loop: {sliced}"));
                }
                Ok(())
            },
        },
        Case {
            key: "device.autosample",
            run: |driver, ctx| {
                let made = run(
                    driver,
                    "device.autosample",
                    json!({ "device": ctx.ids.device(POLYSYNTH), "lowest": 60, "highest": 66,
                            "step": 6, "note_length": 0.2, "release_tail": 0.1 }),
                )?;
                if made["files"].as_array().map_or(0, Vec::len) != 2 {
                    return Err(format!("two notes asked for: {made}"));
                }
                files_sound(&made, "files", "an autosampled note")?;
                let track = made["track"]
                    .as_u64()
                    .ok_or_else(|| format!("no track in {made}"))?;
                object(driver, track)?;
                Ok(())
            },
        },
        Case {
            key: "device.autosample without waiting",
            run: |driver, ctx| {
                // Spec: multisampler R73. With `wait` off the call returns at
                // once; the new track reads back once the samples are made.
                let before = tracks(driver)?.len();
                let started = run(
                    driver,
                    "device.autosample",
                    json!({ "device": ctx.ids.device(POLYSYNTH), "lowest": 60, "highest": 72,
                            "step": 6, "note_length": 0.2, "release_tail": 0.1,
                            "wait": false, "name": "Sampled" }),
                )?;
                if !started["track"].is_null() {
                    return Err(format!("it didn't return at once: {started}"));
                }
                poll(driver, "the autosampled track", |driver| {
                    let all = tracks(driver)?;
                    Ok((all.len() > before).then_some(()))
                })?;
                let read = track(driver, "Sampled")?;
                if read["kind"] != "midi" || read["devices"].as_array().is_none_or(Vec::is_empty) {
                    return Err(format!("the sampled track has no multisampler: {read}"));
                }
                Ok(())
            },
        },
        Case {
            key: "perception.analyze",
            run: |driver, _ctx| {
                let analysed = run(
                    driver,
                    "perception.analyze",
                    json!({ "start": 0, "length": 2 * BAR, "measure": ["levels", "loudness"] }),
                )?;
                let peak = analysed["levels"]["peak_dbfs"]
                    .as_f64()
                    .or_else(|| analysed["levels"]["sample_peak_dbfs"].as_f64());
                eprintln!("analyse: {}", analysed["summary"]);
                if analysed["levels"].is_null() {
                    return Err(format!("no levels: {analysed} ({peak:?})"));
                }
                Ok(())
            },
        },
        Case {
            key: "set.collect_all_and_save",
            run: |driver, ctx| {
                let collected = run(driver, "set.collect_all_and_save", json!({}))?;
                let files = collected["files"].as_u64().unwrap_or_default();
                if files == 0 {
                    return Err(format!("the fixture's media wasn't collected: {collected}"));
                }
                let imported = ctx.project.join("Samples/Imported");
                let count = std::fs::read_dir(&imported).map_or(0, Iterator::count);
                if count == 0 {
                    return Err(format!("{} is empty", imported.display()));
                }
                Ok(())
            },
        },
        Case {
            key: "set.create_pack",
            run: |driver, ctx| {
                run(driver, "set.save", json!({}))?;
                let pack = ctx.file("Background.blpack");
                let made = run(driver, "set.create_pack", json!({ "path": pack }))?;
                written(&pack)?;
                if made["files"].as_u64().unwrap_or_default() == 0 {
                    return Err(format!("an empty pack: {made}"));
                }
                let unpacked = ctx.out.join("unpacked");
                std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
                let installed = run(
                    driver,
                    "set.install_pack",
                    json!({ "pack": pack, "folder": unpacked.to_string_lossy() }),
                )?;
                let sets = paths_in(&installed["sets"]);
                if sets.is_empty() {
                    return Err(format!("the unpacked project holds no set: {installed}"));
                }
                Ok(())
            },
        },
        Case {
            key: "set.export_dawproject",
            run: |driver, ctx| {
                let path = ctx.file("Background.dawproject");
                run(driver, "set.export_dawproject", json!({ "path": path }))?;
                written(&path)
            },
        },
        Case {
            key: "set.export_tracks",
            run: |driver, ctx| {
                let saved = run(
                    driver,
                    "set.export_tracks",
                    json!({ "tracks": [ctx.ids.track("Loop")], "folder": ctx.out.to_string_lossy(),
                            "copy": true }),
                )?;
                let path = saved["path"]
                    .as_str()
                    .ok_or_else(|| format!("no path in {saved}"))?;
                written(path)
            },
        },
        Case {
            key: "clip.save_file",
            run: |driver, ctx| {
                let saved = run(
                    driver,
                    "clip.save_file",
                    json!({ "clip": ctx.ids.clip("Loop"), "folder": ctx.out.to_string_lossy(),
                            "copy": true }),
                )?;
                let path = saved["path"]
                    .as_str()
                    .ok_or_else(|| format!("no path in {saved}"))?;
                written(path)
            },
        },
        Case {
            key: "midi.export_clip",
            run: |driver, ctx| {
                let path = ctx.file("Chords.mid");
                let saved = run(
                    driver,
                    "midi.export_clip",
                    json!({ "clip": ctx.ids.clip("Chords"), "path": path }),
                )?;
                if saved["notes"] != 15 {
                    return Err(format!("the chords hold 15 notes: {saved}"));
                }
                written(&path)
            },
        },
        Case {
            key: "stems.separate",
            run: |driver, ctx| {
                // Stem separation needs packages a test never downloads: it
                // runs only where they're already installed.
                let status = run(driver, "stems.status", json!({}))?;
                if status["installed"] != true {
                    eprintln!(
                        "stems.separate skipped: its packages aren't installed in {}",
                        status["folder"]
                    );
                    return Ok(());
                }
                let before = tracks(driver)?.len();
                run(
                    driver,
                    "stems.separate",
                    json!({ "clip": ctx.ids.clip("Loop"), "stems": ["drums", "bass"] }),
                )?;
                let status = poll(driver, "the separation", |driver| {
                    let status = run(driver, "stems.status", json!({}))?;
                    Ok((status["separation"]["state"] != "running").then_some(status))
                })?;
                if let Some(error) = status["separation"]["error"].as_str() {
                    return Err(format!("the separation failed: {error}"));
                }
                poll(driver, "the stems' tracks", |driver| {
                    Ok((tracks(driver)?.len() > before).then_some(()))
                })
            },
        },
    ]
}

/// Every scene's ID, top to bottom.
fn fixture_scenes(driver: &mut dyn Driver) -> Result<Vec<u64>, String> {
    let mut scenes = Vec::new();
    for number in 1.. {
        let Ok(scene) = driver.tool("query", json!({ "path": format!("scene {number}") })) else {
            break;
        };
        scenes.push(
            scene["id"]
                .as_u64()
                .ok_or_else(|| format!("scene {number} has no ID"))?,
        );
    }
    Ok(scenes)
}

/// Every file under the repository's fixtures folder, with its size and
/// when it was last changed: a sweep fails if any command touched one.
pub(crate) fn fixtures_state() -> Vec<(PathBuf, u64, Option<std::time::SystemTime>)> {
    let mut out = Vec::new();
    let mut stack = vec![bl_e2e::fixtures()];
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => stack.push(path),
                Ok(meta) => out.push((path, meta.len(), meta.modified().ok())),
                Err(_) => out.push((path, 0, None)),
            }
        }
    }
    out.sort();
    out
}

/// Builds the rich set on `driver`, saves it in a temporary project, and
/// runs every case from a checkpoint of it, recording each outcome as
/// `property`.
fn sweep(driver: &mut dyn Driver, ids: Ids, property: &str) {
    let dir = TempDir::new("background");
    // The folder as the disk names it: a temporary folder reached through a
    // link (macOS's /var) is a case of its own, in the round trips.
    let root = dir.path().canonicalize().expect("the temporary folder");
    let saved = driver
        .run(
            "set.save_as",
            json!({ "path": root.join("Background.blset").to_string_lossy() }),
        )
        .unwrap_or_else(|error| panic!("set.save_as: {error}"));
    let set = PathBuf::from(saved["path"].as_str().expect("the saved set's path"));
    let project = set.parent().expect("a project folder").to_owned();
    let out = root.join("out");
    std::fs::create_dir_all(&out).expect("an output folder");
    driver
        .tool(
            "checkpoint",
            json!({ "action": "save", "name": "rich set" }),
        )
        .expect("a checkpoint");
    let ctx = Ctx { ids, project, out };
    let mut ratchet = Known::load().ratchet(SUITE);
    let fixtures = fixtures_state();
    for case in cases() {
        let outcome =
            catch_unwind(AssertUnwindSafe(|| (case.run)(driver, &ctx))).unwrap_or_else(|panic| {
                Err(panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_else(|| "it panicked".to_owned()))
            });
        if let Err(error) = &outcome {
            eprintln!("{}: {error}", case.key);
        }
        ratchet.check(case.key, property, outcome);
        // Back to the rich set as saved, for the next case.
        let restored = driver.tool(
            "checkpoint",
            json!({ "action": "restore", "name": "rich set" }),
        );
        if let Err(error) = restored {
            panic!("restoring the rich set after {} failed: {error}", case.key);
        }
        let _ = driver.run("set.save", json!({}));
        assert!(
            fixtures_state() == fixtures,
            "{} changed the repository's fixtures",
            case.key
        );
    }
    ratchet.finish();
}

#[test]
fn background_work_finishes_without_a_window() {
    let mut host = StdioHost::start();
    let ids = fixture::build(&mut host);
    sweep(&mut host, ids, HEADLESS);
    host.finish().expect("the server stops cleanly");
}

#[test]
fn background_work_finishes_in_the_app() {
    let mut host = Host::new();
    let ids = fixture::build(&mut host);
    sweep(&mut host, ids, IN_APP);
    host.check();
}
