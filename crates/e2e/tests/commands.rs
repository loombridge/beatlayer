//! The command sweep: every registered command, run through the agent API
//! from the same rich set, with arguments made from its JSON Schema
//! (`bl_e2e::schema`), against the properties every command keeps.
//!
//! | Property | What it says |
//! |---|---|
//! | `no panic` | A command never panics, whatever its arguments |
//! | `failure changes nothing` | A command that fails leaves the set and its unsaved-changes state as they were |
//! | `read changes nothing` | A `read` command leaves the set and its unsaved-changes state as they were |
//! | `invariants hold` | After a command succeeds the set keeps every rule a loaded set keeps |
//! | `undo restores` | Undo after a command's step gives the set as it was before |
//! | `redo reapplies` | Redo then gives the set as the command left it |
//! | `readback runs` | The read command or query that shows the command's effect succeeds |
//! | `survives save and load` | The set a command left saves and opens again unchanged |
//! | `value clamped to the parameter's range` | A device parameter, an automation breakpoint or a volume far out of range is refused or stored within it |
//!
//! Each command runs from the same set: the sweep builds the rich set once,
//! adds a drum rack, a multisampler, a groove, a take lane, a tuning, a
//! macro mapping and variation, warps the drum loop and writes the files
//! loaders read, saves it, and puts it back exactly, unsaved-changes state
//! and all, before each command. The fixtures it loads and hands to
//! commands are copies in a temporary folder. A command that ran on a
//! track runs again on Main and a return (and at full depth a group). A
//! command whose arguments are refused at every try, after the repairs its
//! errors suggest, counts as rejected; the coverage table says how many of
//! each namespace ran. Failures the suite has found in Beatlayer and not fixed
//! yet are listed in `tests/known-failures.toml`, so the sweep stays green
//! until one changes.

use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use bl_commands::Permission;
use bl_commands::readback::{Object, READBACK, Readback};
use bl_e2e::fixture::{self, Ids};
use bl_e2e::schema::{Prefer, Refs, Synth};
use bl_e2e::{BAR, Driver, Host, Known, Ratchet, document_diff, full_depth};
use bl_model::Document;
use serde_json::{Value, json};

/// The suite's name in the known-failures list.
const SUITE: &str = "commands";

/// Commands the sweep doesn't run, and why.
const SKIPPED: &[(&str, &str)] = &[
    ("app.quit", "quits the app"),
    ("agent.set_server", "opens a network port"),
    ("link.set_enabled", "joins a Link session on the network"),
    ("stems.install", "downloads packages from the network"),
    (
        "stems.separate",
        "needs the stem separation packages installed",
    ),
    ("assistant.send", "sends a message to a model provider"),
    ("plugin.scan", "scans the machine's real plug-ins"),
    ("plugin.rescan", "scans the machine's real plug-ins"),
    ("plugin.load", "needs a real plug-in"),
    ("plugin.load_au", "needs a real Audio Unit"),
    ("browser.reveal", "opens the system's file manager"),
    ("clip.edit_sample", "opens an external sample editor"),
    ("file_manager.edit_file", "opens an external sample editor"),
    ("cv.calibrate", "needs hardware patched to a CV output"),
    (
        "file_manager.delete_unused",
        "moves files to the system's trash, outside the test's folder",
    ),
    ("control_surface.dump", "sends to a hardware controller"),
];

/// Why the sweep doesn't run `command`, if it doesn't.
fn skip_reason(command: &str, args: &Value) -> Option<&'static str> {
    if let Some((_, reason)) = SKIPPED.iter().find(|(id, _)| *id == command) {
        return Some(reason);
    }
    // A plug-in device's own commands need a plug-in to act on.
    if command.starts_with("plugin.")
        && args
            .get("properties")
            .and_then(|p| p.get("device"))
            .is_some()
    {
        return Some("needs a real plug-in device");
    }
    None
}

/// The most argument sets the sweep tries for one command.
const ATTEMPTS: usize = 12;

/// What Beatlayer answers an agent that runs a command only a person may.
const PERSON_ONLY: &str = "so only a person can run it";

/// Argument sets to try after `args` failed with `error`: the other kind
/// of clip and track when the error says the kind was wrong, and, for each
/// field the error names in backticks, `args` without it if it was given
/// or with it (from `every`, the set with every field) if it wasn't.
fn repairs(synth: &mut Synth, spec: &Spec, args: &Value, every: &Value, error: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let lower = error.to_lowercase();
    let wants_audio = [
        "needs an audio",
        "isn't an audio",
        "not an audio",
        "only an audio",
    ]
    .iter()
    .any(|w| lower.contains(w));
    let wants_midi = [
        "needs a midi",
        "isn't a midi",
        "not a midi",
        "only a midi",
        "holds a midi",
    ]
    .iter()
    .any(|w| lower.contains(w));
    let prefer = if wants_audio {
        Some(Prefer::Audio)
    } else if wants_midi {
        Some(Prefer::Midi)
    } else {
        None
    };
    if prefer.is_some() && synth.prefer.is_none() {
        synth.prefer = prefer;
        out.extend(synth.candidates(&spec.id, &spec.description, &spec.args));
        synth.prefer = None;
    }
    let names = error
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    for name in names {
        let mut next = args.clone();
        if let Some(fields) = next.as_object_mut()
            && fields.remove(name).is_none()
        {
            match every.get(name) {
                Some(value) => {
                    fields.insert(name.to_owned(), value.clone());
                }
                None => continue,
            }
        }
        out.push(next);
    }
    out
}

/// What the sweep found, one outcome per command and property however many
/// times it was checked: a property fails for a command if it failed with
/// any arguments.
#[derive(Debug, Default)]
struct Findings {
    outcomes: BTreeMap<(String, String), Result<(), String>>,
}

impl Findings {
    /// Records one check of `property` for `key`.
    fn check(&mut self, key: &str, property: &str, outcome: Result<(), String>) {
        let entry = self
            .outcomes
            .entry((key.to_owned(), property.to_owned()))
            .or_insert(Ok(()));
        if entry.is_ok() {
            *entry = outcome;
        }
    }

    /// Records a failure of `property` for `key`.
    fn violation(&mut self, key: &str, property: &str, detail: impl Into<String>) {
        self.check(key, property, Err(detail.into()));
    }

    /// Hands every outcome to `ratchet` and finishes it.
    fn finish(self, mut ratchet: Ratchet) {
        for ((key, property), outcome) in self.outcomes {
            ratchet.check(&key, &property, outcome);
        }
        ratchet.finish();
    }
}

/// What happened to one command in the sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    /// It ran: a read, or an edit that succeeded.
    Ran,
    /// Every argument set was refused; the last error.
    Rejected(String),
    /// Not run, for this reason.
    Skipped(&'static str),
    /// It panicked.
    Panicked,
}

/// The sweep's host, the set every command starts from, and what it found.
struct Sweep {
    host: Host,
    base: Document,
    saved: PathBuf,
    synth: Synth,
    findings: Findings,
    /// Edits that succeeded, for the save and load pass.
    edits: Vec<(String, Value)>,
}

/// One command's spec as the sweep needs it.
#[derive(Clone, Debug)]
struct Spec {
    id: String,
    description: String,
    permission: Permission,
    args: Value,
}

fn specs(host: &mut Host) -> Vec<Spec> {
    let mut specs: Vec<Spec> = host
        .shell()
        .registry()
        .iter()
        .map(|spec| Spec {
            id: spec.id.to_string(),
            description: spec.description.to_string(),
            permission: spec.permission.unwrap_or(Permission::Edit),
            args: spec
                .args
                .as_ref()
                .map_or_else(|| json!({}), |s| s.as_value().clone()),
        })
        .collect();
    specs.sort_by(|a, b| a.id.cmp(&b.id));
    specs
}

/// The ID a `run` reply gave `$new` name `name`.
fn new_id(reply: &Value, name: &str) -> u64 {
    reply["ids"][name]
        .as_u64()
        .unwrap_or_else(|| panic!("no ID for `{name}` in {reply}"))
}

/// Builds the sweep's set on a new host: the rich set, then a drum rack, a
/// multisampler, a groove and a take lane, and the files loaders read; then
/// saves it, so it starts with no unsaved changes.
fn build(dir: &std::path::Path) -> (Host, Refs, PathBuf) {
    // Copies of the fixtures, which the set loads and the commands are
    // handed, so no command writes beside or deletes the repository's own.
    // A new copy for each build: a command may have left files beside the
    // last one's (a sample's default clip settings).
    let copy = (1..)
        .map(|n| dir.join(format!("fixtures {n}")))
        .find(|path| !path.exists())
        .expect("a free folder name");
    let fixtures =
        bl_e2e::copy_fixtures(&copy, &["drums", "media", "sfz", "tunings", "dawproject"]);
    let mut host = Host::new();
    let ids: Ids = fixture::build(&mut host);
    let saved = host.save_to(dir);
    let files = host.dir().join("files");
    std::fs::create_dir_all(&files).expect("a folder for the sweep's files");
    let library = host.dir().join("User Library");
    std::fs::create_dir_all(&library).expect("a User Library");
    host.run_ok(
        "settings.set_user_library",
        json!({ "path": library.to_string_lossy() }),
    );
    let mut refs = Refs::from_ids(&ids, files.clone());
    let kick = PathBuf::from(fixture::drums("kick.wav"));
    let snare = PathBuf::from(fixture::drums("snare.wav"));
    let sfz = fixtures.join("sfz/Sine Bank.sfz");
    let scl = fixtures.join("tunings/equal-19.scl");
    let dawproject = fixtures.join("dawproject/minimal.dawproject");

    // A drum rack and a multisampler, each on a MIDI track of its own.
    let made = host.batch_ok(
        "More instruments",
        vec![
            json!({ "command": "track.create", "args": { "id": { "$new": "Drums" },
                    "kind": "midi", "name": "Drums", "index": 2 } }),
            json!({ "command": "track.create", "args": { "id": { "$new": "Multi" },
                    "kind": "midi", "name": "Multi", "index": 3 } }),
            json!({ "command": "device.insert", "args": { "track": { "$ref": "Drums" },
                    "index": 0, "device": {
                        "id": { "$new": "drum rack" }, "name": "Drum Rack", "enabled": true,
                        "params": [], "role": "instrument", "kind": { "rack": { "kind": "drum" } },
                        "chains": [{ "id": { "$new": "kick pad" }, "name": "Kick",
                                     "receive": { "note": 36 },
                                     "devices": [fixture::native("kick sampler",
                                         bl_devices::drum_sampler::DRUM_SAMPLER,
                                         "instrument")] }] } } }),
            json!({ "command": "device.insert", "args": { "track": { "$ref": "Multi" },
                    "index": 0, "device": fixture::native("mosaic",
                        bl_devices::multisampler::MULTISAMPLER, "instrument") } }),
            json!({ "command": "clip.create", "args": { "track": { "$ref": "Drums" },
                    "clip": fixture::midi_clip("Beat", 0, BAR, fixture::midi_data(
                        [(36, 0, BAR / 8, 100), (36, BAR / 2, BAR / 8, 90)])) } }),
        ],
    );
    let drums = new_id(&made, "Drums");
    let multi = new_id(&made, "Multi");
    refs.midi_tracks.extend([drums, multi]);
    refs.add_device("rack:drum", new_id(&made, "drum rack"));
    refs.add_device(
        bl_devices::drum_sampler::DRUM_SAMPLER,
        new_id(&made, "kick sampler"),
    );
    refs.drum_chains.push(new_id(&made, "kick pad"));
    let mosaic = new_id(&made, "mosaic");
    refs.add_device(bl_devices::multisampler::MULTISAMPLER, mosaic);
    refs.arrangement.midi.push(new_id(&made, "Beat"));
    let layers = host.run_ok(
        "device.add_layers",
        json!({ "device": mosaic,
                "paths": [kick.to_string_lossy(), snare.to_string_lossy()] }),
    );
    refs.layers = layers["layers"]
        .as_array()
        .map(|l| l.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();

    // A groove from the drum loop, and a take lane on its track.
    let grooves = host.run_ok("groove.extract", json!({ "clips": [ids.clip("Loop")] }));
    refs.grooves = grooves["grooves"]
        .as_array()
        .map(|l| l.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    let lanes = host.run_ok("take_lane.insert", json!({ "tracks": [ids.track("Loop")] }));
    refs.take_lanes = lanes["lanes"]
        .as_array()
        .map(|l| l.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    let loop_clip = host.query(json!({ "id": ids.clip("Loop"), "detail": "full" }));
    if let Some(hash) = loop_clip["saved"]["data"]["audio"]["media"].as_str() {
        refs.media.push(hash.to_owned());
    }

    // A tuning, a macro variation and a macro mapping, which the commands
    // that change them need.
    host.run_ok(
        "tuning.load",
        json!({ "path": scl.to_string_lossy(), "retune": false }),
    );
    let rack = ids.device("rack:audio_effect");
    let saturator = ids.device(bl_devices::saturator::SATURATOR);
    let drive = refs.params[bl_devices::saturator::SATURATOR][0].0;
    host.run_ok(
        "rack.map_macro",
        json!({ "rack": rack, "macro_number": 1,
                "target": { "type": "device", "device": saturator, "param": drive } }),
    );
    host.run_ok("rack.store_variation", json!({ "rack": rack }));

    // The drum loop warped, as warp commands need.
    for clip in [ids.clip("Loop"), ids.clip("Loop slot")] {
        host.run_ok("clip.set_warp", json!({ "clip": clip, "warp": true }));
    }

    // The files the loaders read.
    let file = |name: &str| files.join(name).to_string_lossy().into_owned();
    host.run_ok(
        "device.save_preset",
        json!({ "device": ids.device(bl_devices::auto_filter::AUTO_FILTER),
                "path": file("filter.blpreset") }),
    );
    host.run_ok(
        "chain.save_preset",
        json!({ "chain": ids.chains[0], "path": file("chain.blpreset") }),
    );
    host.run_ok(
        "midi.export_clip",
        json!({ "clip": ids.clip("Chords"), "path": file("chords.mid") }),
    );
    if let Some(groove) = refs.grooves.first() {
        host.run_ok(
            "groove.save",
            json!({ "groove": groove, "path": file("loop.blgroove") }),
        );
    }
    let clip_file = host.run_ok(
        "clip.save_file",
        json!({ "clip": ids.clip("Chords"), "folder": files.to_string_lossy(), "copy": true }),
    );
    for (kind, path) in [
        ("wav", kick.clone()),
        ("blpreset", files.join("filter.blpreset")),
        ("chain_preset", files.join("chain.blpreset")),
        ("mid", files.join("chords.mid")),
        ("blgroove", files.join("loop.blgroove")),
        ("sfz", sfz),
        ("scl", scl),
        ("dawproject", dawproject),
    ] {
        refs.files.insert(kind.to_owned(), path);
    }
    if let Some(path) = clip_file["path"].as_str() {
        refs.files.insert("blclip".to_owned(), PathBuf::from(path));
    }

    // Saved again, so the set has a backup.
    host.shell_mut()
        .run("set.save", json!({}))
        .expect("the set saves again");
    refs.files.insert("blset".to_owned(), saved.clone());
    let backups = host.run_ok("set.backups", json!({}));
    if let Some(path) = backups["backups"][0]["path"].as_str() {
        refs.files.insert("backup".to_owned(), PathBuf::from(path));
    }
    refs.scenes = fixture::scenes(&mut host);
    host.settle();
    assert!(!host.is_dirty(), "the sweep's set starts saved");
    (host, refs, saved)
}

impl Sweep {
    fn new(dir: &std::path::Path) -> Self {
        let (mut host, refs, saved) = build(dir);
        let mut sweep = Self {
            base: host.document().clone(),
            host,
            saved,
            synth: Synth::new(refs),
            findings: Findings::default(),
            edits: Vec::new(),
        };
        sweep.restore();
        sweep
    }

    /// A new host with the same set, after a panic may have left the last
    /// one broken. The IDs come out the same, so the synthesiser's stand.
    fn rebuild(&mut self, dir: &std::path::Path) {
        let (host, refs, saved) = build(dir);
        assert_eq!(
            refs.midi_tracks, self.synth.refs.midi_tracks,
            "the rebuilt set has the same IDs"
        );
        self.host = host;
        self.saved = saved;
        self.base = self.host.document().clone();
        self.restore();
    }

    /// Puts the sweep's set back as it was built: the document, an empty
    /// undo history, no unsaved changes, the transport stopped and the
    /// engine running.
    fn restore(&mut self) {
        let host = &mut self.host;
        if host.is_playing() {
            let _ = host.shell_mut().run("transport.stop", json!({}));
        }
        if !host.shell().audio().engine_on() {
            let _ = host.shell_mut().run("audio.toggle_engine", json!({}));
        }
        if !host.shell().audio().engine_running() {
            host.shell_mut().start_audio();
        }
        assert!(
            host.shell().audio().engine_running(),
            "the sweep's engine runs again: {:?}",
            host.shell().message()
        );
        // A gesture a command left open (a selected breakpoint the
        // Envelope Editor keeps changes to) ends unkept.
        if host.shell().editor().in_transaction() {
            host.shell_mut().edit(|editor| {
                let _ = editor.cancel();
            });
        }
        if *host.document() != self.base {
            let base = self.base.clone();
            host.shell_mut()
                .edit(|editor| editor.replace("Restore the sweep's set", base));
        }
        host.shell_mut().edit(|editor| {
            editor.clear_history().expect("no transaction is open");
        });
        if host.is_dirty() && host.shell_mut().run("set.save", json!({})).is_err() {
            // A set with no file any more (a new set replaced it): saved
            // where the sweep's set was.
            let path = self.saved.to_string_lossy().into_owned();
            let _ = host.shell_mut().run("set.save_as", json!({ "path": path }));
        }
        host.pump();
        assert!(
            *host.document() == self.base,
            "the sweep's set restores exactly: {:?}",
            document_diff(&self.base, &host.document().clone(), 5)
        );
    }

    /// Selects what a person would have selected before running `command`
    /// with `args`, as the commands that act on the selection expect: the
    /// clip it names (the chord clip otherwise), so the Clip View shows it;
    /// the device it names; and for automation, the main automation lane of
    /// the clip's track. Selecting changes nothing in the set.
    fn prelude(&mut self, command: &str, args: &Value) {
        let refs = &self.synth.refs;
        let named = |field: &str| args.get(field).and_then(Value::as_u64);
        let clip = named("clip").or_else(|| refs.arrangement.midi.first().copied());
        let track = named("track").or_else(|| refs.midi_tracks.first().copied());
        let device = named("device").or_else(|| named("rack"));
        let shell = self.host.shell_mut();
        if let Some(clip) = clip {
            let _ = shell.run("arrangement.select_clips", json!({ "clips": [clip] }));
        }
        if let Some(device) = device {
            let _ = shell.run("device.select", json!({ "device": device }));
        }
        if (command.starts_with("automation.") || command.starts_with("envelope."))
            && let Some(track) = track
        {
            let _ = shell.run("arrangement.automation_mode", json!({ "on": true }));
            let _ = shell.run(
                "arrangement.show_automation",
                json!({ "track": track, "target": { "type": "mixer", "param": "volume" } }),
            );
            let _ = shell.run(
                "arrangement.select_tracks",
                json!({ "tracks": [track], "automation_lane": true }),
            );
        }
        self.host.pump();
    }

    /// Lets the engine take the set as it is now, under `catch_unwind`:
    /// `false`, with a `no panic` violation for `key` and a new host, if
    /// the engine or the app panicked on what `key` with `args` left.
    fn settled(&mut self, key: &str, args: &Value, dir: &std::path::Path) -> bool {
        let host = &mut self.host;
        if catch_unwind(AssertUnwindSafe(|| host.settle())).is_ok() {
            self.findings.check(key, "no panic", Ok(()));
            return true;
        }
        self.findings.violation(
            key,
            "no panic",
            format!("the engine never took the set {key} left with {args}"),
        );
        self.rebuild(dir);
        false
    }

    /// Runs `command` with `args` through the agent API, under
    /// `catch_unwind`: the whole `run` reply, the error, or `None` if it
    /// panicked.
    fn call(&mut self, command: &str, args: &Value) -> Option<Result<Value, String>> {
        let host = &mut self.host;
        catch_unwind(AssertUnwindSafe(|| {
            host.tool(
                "run",
                json!({ "commands": [{ "command": command, "args": args }] }),
            )
        }))
        .ok()
    }

    /// Runs one argument set of `spec` from the sweep's set and checks every
    /// property; `Some(error)` if the command failed.
    fn attempt(&mut self, spec: &Spec, args: &Value, key: &str, dir: &std::path::Path) -> Attempt {
        self.prelude(&spec.id, args);
        let before = self.host.document().clone();
        let dirty = self.host.is_dirty();
        let steps = self.host.shell().editor().undo_history().len();
        let Some(reply) = self.call(&spec.id, args) else {
            self.findings
                .violation(key, "no panic", format!("panicked with {args}"));
            self.rebuild(dir);
            return Attempt::Panicked;
        };
        // Background work a command starts lands before it's judged.
        if !self.settled(key, args, dir) {
            return Attempt::Panicked;
        }
        let after = self.host.document().clone();
        let changed = |host: &mut Host, before: &Document| {
            let mut out = document_diff(before, host.document(), 3);
            if host.is_dirty() != dirty {
                out.push(format!(
                    "unsaved changes went from {dirty} to {}",
                    host.is_dirty()
                ));
            }
            out
        };
        let reply = match reply {
            Err(error) => {
                let diff = changed(&mut self.host, &before);
                self.findings.check(
                    key,
                    "failure changes nothing",
                    if diff.is_empty() {
                        Ok(())
                    } else {
                        Err(format!(
                            "failed ({error}) with {args}, yet changed {diff:?}"
                        ))
                    },
                );
                self.restore();
                return Attempt::Failed(error);
            }
            Ok(reply) => reply,
        };
        if spec.permission == Permission::Read {
            let mut diff = changed(&mut self.host, &before);
            // A view the set keeps (a rack's pads, its variations view) is
            // documented to count as an unsaved change, though no undo step.
            if spec.description.contains("the set keeps") && after == before {
                diff.retain(|change| !change.starts_with("unsaved changes"));
            }
            self.findings.check(
                key,
                "read changes nothing",
                if diff.is_empty() {
                    Ok(())
                } else {
                    Err(format!("with {args} it changed {diff:?}"))
                },
            );
            self.restore();
            return Attempt::Ran;
        }
        // Spec: tracks-and-undo. The set a command leaves is one a set can be.
        self.findings.check(
            key,
            "invariants hold",
            after
                .check_invariants()
                .map_err(|e| format!("with {args}: {e}")),
        );
        let stepped = self.host.shell().editor().undo_history().len() > steps;
        if stepped
            && !matches!(
                spec.id.as_str(),
                "edit.undo" | "edit.redo" | "edit.go_to_step"
            )
        {
            // Spec: tracks-and-undo. Undo returns the song to the state
            // before the newest step, and redo reapplies it.
            let undone = self.call("edit.undo", &json!({}));
            if !self.settled(key, args, dir) {
                return Attempt::Panicked;
            }
            let outcome = match undone {
                Some(Ok(_)) => {
                    let diff = document_diff(&before, self.host.document(), 3);
                    if *self.host.document() == before {
                        Ok(())
                    } else {
                        Err(format!("after {args}, undo left {diff:?}"))
                    }
                }
                Some(Err(e)) => Err(format!("undo failed after {args}: {e}")),
                None => Err(format!("undo panicked after {args}")),
            };
            self.findings.check(key, "undo restores", outcome);
            let redone = self.call("edit.redo", &json!({}));
            if !self.settled(key, args, dir) {
                return Attempt::Panicked;
            }
            let outcome = match redone {
                Some(Ok(_)) => {
                    let diff = document_diff(&after, self.host.document(), 3);
                    if *self.host.document() == after {
                        Ok(())
                    } else {
                        Err(format!("after {args}, redo left {diff:?}"))
                    }
                }
                Some(Err(e)) => Err(format!("redo failed after {args}: {e}")),
                None => Err(format!("redo panicked after {args}")),
            };
            self.findings.check(key, "redo reapplies", outcome);
        }
        let readback = self.readback(spec, args, &reply);
        if let Some(outcome) = readback {
            self.findings.check(key, "readback runs", outcome);
        }
        let edited = after != before;
        self.restore();
        if edited {
            Attempt::Edited
        } else {
            Attempt::Ran
        }
    }

    /// Runs the readback the table names for `spec`, after it ran with
    /// `args` and answered `reply`; `None` when there's nothing to read.
    fn readback(&mut self, spec: &Spec, args: &Value, reply: &Value) -> Option<Result<(), String>> {
        let (_, readback) = READBACK.iter().find(|(id, _)| *id == spec.id)?;
        // An argument's value, with a `$new` placeholder's ID filled in.
        let arg = |name: &str| -> Option<Value> {
            let value = args.get(name)?;
            let value = match value {
                Value::Array(list) => list.first()?.clone(),
                other => other.clone(),
            };
            if let Some(placeholder) = value.get("$new").and_then(Value::as_str) {
                return reply["ids"].get(placeholder).cloned();
            }
            Some(value)
        };
        match *readback {
            Readback::Nothing { .. } => None,
            Readback::Query { object, .. } => {
                let query = match object {
                    Object::Song => json!({ "path": "song", "detail": "full" }),
                    Object::Scene => {
                        let scene = arg("scene").and_then(|s| s.as_u64());
                        let number = scene
                            .and_then(|s| {
                                self.host
                                    .document()
                                    .scenes()
                                    .position(|scene| scene.id.0.get() == s)
                            })
                            .unwrap_or(0);
                        json!({ "path": format!("scene {}", number + 1), "detail": "full" })
                    }
                    Object::Track | Object::Clip | Object::Device => {
                        let names: &[&str] = match object {
                            Object::Track => &["track", "tracks", "to", "into"],
                            Object::Clip => &["clip", "clips"],
                            _ => &["device", "rack", "devices"],
                        };
                        let doc = self.host.document();
                        // A track field's value is a track's ID only if
                        // one has it (`to` may be a position).
                        let real = |id: &Value| {
                            object != Object::Track
                                || id
                                    .as_u64()
                                    .is_some_and(|id| doc.tracks().any(|t| t.id().0.get() == id))
                        };
                        let id = names
                            .iter()
                            .find_map(|n| arg(n).filter(|v| v.is_u64() && real(v)))
                            .or_else(|| {
                                // The object the command made, if it made one.
                                reply["ids"].as_object()?.values().next().cloned()
                            });
                        json!({ "id": id?, "detail": "full" })
                    }
                };
                Some(
                    self.host
                        .tool("query", query.clone())
                        .map(drop)
                        .map_err(|e| format!("query {query} failed: {e}")),
                )
            }
            Readback::Command { command, .. } => {
                let read = self.host.shell().registry().get(command).map(|s| {
                    (
                        s.description.to_string(),
                        s.args
                            .as_ref()
                            .map_or_else(|| json!({}), |a| a.as_value().clone()),
                    )
                })?;
                let mut last = String::new();
                for mut candidate in self.synth.candidates(command, &read.0, &read.1) {
                    // The objects the command acted on are the ones to read.
                    if let Some(fields) = candidate.as_object_mut() {
                        for (name, value) in fields.iter_mut() {
                            if let Some(given) = args.get(name)
                                && given.get("$new").is_none()
                            {
                                *value = given.clone();
                            }
                        }
                    }
                    match self.host.run(command, candidate.clone()) {
                        Ok(_) => return Some(Ok(())),
                        Err(error) => last = format!("`{command}` {candidate}: {error}"),
                    }
                }
                Some(Err(last))
            }
        }
    }
}

/// How one argument set went.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Attempt {
    /// A read ran, or an edit succeeded without changing the set.
    Ran,
    /// An edit succeeded and changed the set.
    Edited,
    /// It failed, with this error.
    Failed(String),
    /// It panicked.
    Panicked,
}

/// One row of the coverage table.
#[derive(Clone, Debug, Default)]
struct Row {
    reads: usize,
    reads_run: usize,
    edits: usize,
    edits_run: usize,
    rejected: usize,
    skipped: usize,
    edits_skipped: usize,
    panicked: usize,
}

fn print_coverage(outcomes: &BTreeMap<String, (Permission, Outcome)>) -> (usize, usize) {
    let mut rows: BTreeMap<&str, Row> = BTreeMap::new();
    for (id, (permission, outcome)) in outcomes {
        let row = rows.entry(id.split('.').next().unwrap_or(id)).or_default();
        let read = *permission == Permission::Read;
        if read {
            row.reads += 1;
        } else {
            row.edits += 1;
        }
        match outcome {
            Outcome::Ran if read => row.reads_run += 1,
            Outcome::Ran => row.edits_run += 1,
            Outcome::Rejected(_) => row.rejected += 1,
            Outcome::Skipped(_) => {
                row.skipped += 1;
                if !read {
                    row.edits_skipped += 1;
                }
            }
            Outcome::Panicked => row.panicked += 1,
        }
    }
    eprintln!(
        "{:<16} {:>6} {:>6} {:>6} {:>6} {:>8} {:>7} {:>7}",
        "namespace", "reads", "ran", "edits", "ran", "rejected", "skipped", "panics"
    );
    let mut total = Row::default();
    for (namespace, row) in &rows {
        eprintln!(
            "{:<16} {:>6} {:>6} {:>6} {:>6} {:>8} {:>7} {:>7}",
            namespace,
            row.reads,
            row.reads_run,
            row.edits,
            row.edits_run,
            row.rejected,
            row.skipped,
            row.panicked
        );
        total.reads += row.reads;
        total.reads_run += row.reads_run;
        total.edits += row.edits;
        total.edits_run += row.edits_run;
        total.rejected += row.rejected;
        total.skipped += row.skipped;
        total.edits_skipped += row.edits_skipped;
        total.panicked += row.panicked;
    }
    eprintln!(
        "{:<16} {:>6} {:>6} {:>6} {:>6} {:>8} {:>7} {:>7}",
        "all",
        total.reads,
        total.reads_run,
        total.edits,
        total.edits_run,
        total.rejected,
        total.skipped,
        total.panicked
    );
    for (id, (_, outcome)) in outcomes {
        match outcome {
            Outcome::Skipped(reason) => eprintln!("  skipped {id}: {reason}"),
            Outcome::Rejected(error) => {
                let error: String = error.chars().take(240).collect();
                eprintln!("  rejected {id}: {error}");
            }
            _ => {}
        }
    }
    (total.edits_run, total.edits - total.edits_skipped)
}

#[test]
fn every_command_keeps_its_properties_from_the_rich_set() {
    let full = full_depth();
    let dir = bl_e2e::TempDir::new("commands");
    let mut sweep = Sweep::new(dir.path());
    let specs = specs(&mut sweep.host);
    let mut outcomes: BTreeMap<String, (Permission, Outcome)> = BTreeMap::new();
    for spec in &specs {
        if let Some(reason) = skip_reason(&spec.id, &spec.args) {
            outcomes.insert(spec.id.clone(), (spec.permission, Outcome::Skipped(reason)));
            continue;
        }
        let mut candidates: VecDeque<Value> = sweep
            .synth
            .candidates(&spec.id, &spec.description, &spec.args)
            .into();
        let every = candidates.back().cloned().unwrap_or_else(|| json!({}));
        let mut tried: Vec<Value> = Vec::new();
        let mut outcome = Outcome::Rejected(String::new());
        let mut chosen = None;
        while let Some(args) = candidates.pop_front() {
            if tried.len() >= ATTEMPTS || tried.contains(&args) {
                continue;
            }
            tried.push(args.clone());
            let args = &args;
            match sweep.attempt(spec, args, &spec.id, dir.path()) {
                Attempt::Ran => {
                    outcome = Outcome::Ran;
                    chosen = Some(args.clone());
                    break;
                }
                Attempt::Edited => {
                    outcome = Outcome::Ran;
                    chosen = Some(args.clone());
                    sweep.edits.push((spec.id.clone(), args.clone()));
                    break;
                }
                Attempt::Failed(error) if error.contains(PERSON_ONLY) => {
                    outcome = Outcome::Skipped("only a person can run it, in Settings");
                    break;
                }
                Attempt::Failed(error) => {
                    candidates.extend(repairs(&mut sweep.synth, spec, args, &every, &error));
                    outcome = Outcome::Rejected(error);
                }
                Attempt::Panicked => {
                    outcome = Outcome::Panicked;
                    break;
                }
            }
        }
        // The same command on the tracks that are unlike the rest: Main and
        // a return and, at full depth, a group.
        if let Some(base) = &chosen
            && let Some(track) = base.get("track").and_then(Value::as_u64)
        {
            let refs = &sweep.synth.refs;
            let mut others = vec![refs.main];
            others.extend(refs.returns.first().copied());
            if full {
                others.extend(refs.groups.first().copied());
            }
            for other in others.into_iter().filter(|t| *t != track) {
                let mut args = base.clone();
                args["track"] = json!(other);
                let _ = sweep.attempt(spec, &args, &spec.id, dir.path());
            }
        }
        // At full depth, every variant of each field and each bound too.
        if full && let Some(base) = chosen {
            for (_, args) in sweep
                .synth
                .variants(&spec.id, &spec.description, &spec.args, &base)
            {
                let _ = sweep.attempt(spec, &args, &spec.id, dir.path());
            }
        }
        outcomes.insert(spec.id.clone(), (spec.permission, outcome));
    }
    let (ran, edits) = print_coverage(&outcomes);
    eprintln!(
        "{ran} of {edits} commands that change something and aren't skipped ran ({:.0} %)",
        100.0 * ran as f64 / edits.max(1) as f64
    );
    values_stay_in_range(&mut sweep);
    save_and_load(&mut sweep, dir.path(), full);
    let Sweep { findings, .. } = sweep;
    findings.finish(Known::load().ratchet(SUITE));
    assert!(
        ran * 4 >= edits * 3,
        "at least 75 % of the commands that change something run: {ran} of {edits}"
    );
}

/// The property for a value set beyond a parameter's range.
const IN_RANGE: &str = "value clamped to the parameter's range";

/// A value far outside a parameter's range is refused, or stored within
/// it: a device parameter set directly or by an automation breakpoint, and
/// a track's volume, whose fader tops out at +6 dB.
fn values_stay_in_range(sweep: &mut Sweep) {
    let refs = &sweep.synth.refs;
    let kind = bl_devices::auto_filter::AUTO_FILTER;
    let (Some(filter), Some(&keys)) = (refs.device(kind), refs.midi_tracks.first()) else {
        return;
    };
    let (param, lo, hi, _) = refs.params[kind]
        .iter()
        .copied()
        .find(|p| p.0 == bl_devices::auto_filter::FREQ.0)
        .expect("Auto Filter has a frequency");
    let far = hi + (hi - lo) * 1e5;
    let stored_param = |doc: &Document| -> Option<f64> {
        doc.tracks()
            .flat_map(|t| t.devices_deep())
            .find(|d| d.id.0.get() == filter)
            .and_then(|d| d.params.get(&bl_model::ParamId(param)).copied())
    };
    let breakpoints = |doc: &Document| -> Vec<f64> {
        doc.tracks()
            .filter(|t| t.id().0.get() == keys)
            .flat_map(|t| t.automation())
            .filter(|(target, _)| {
                matches!(target, bl_model::automation::AutomationTarget::Device { device, param: p }
                    if device.0.get() == filter && p.0 == param)
            })
            .flat_map(|(_, a)| a.envelope.points().map(|b| b.value).collect::<Vec<_>>())
            .collect()
    };
    let top = bl_model::fader::db_gain(bl_model::fader::TOP_DB);
    let checks: [(&str, Value); 3] = [
        (
            "device.set_param",
            json!({ "device": filter, "param": param, "value": far }),
        ),
        (
            "automation.set_breakpoint",
            json!({ "track": keys, "at": BAR / 2, "value": far,
                    "target": { "type": "device", "device": filter, "param": param } }),
        ),
        ("mixer.set_volume", json!({ "track": keys, "volume": 7.0 })),
    ];
    for (command, args) in checks {
        sweep.restore();
        let outcome = match sweep.host.run(command, args.clone()) {
            // Refused: nothing stored.
            Err(_) => Ok(()),
            Ok(_) => {
                let doc = sweep.host.document().clone();
                let bad: Vec<f64> = match command {
                    "device.set_param" => stored_param(&doc)
                        .into_iter()
                        .filter(|v| !(lo..=hi).contains(v))
                        .collect(),
                    "automation.set_breakpoint" => breakpoints(&doc)
                        .into_iter()
                        .filter(|v| !(lo..=hi).contains(v))
                        .collect(),
                    _ => doc
                        .tracks()
                        .filter(|t| t.id().0.get() == keys)
                        .map(|t| t.mixer().volume)
                        .filter(|v| *v > top + 1e-9)
                        .collect(),
                };
                if bad.is_empty() {
                    Ok(())
                } else {
                    Err(format!("{args} was accepted and stored {bad:?}"))
                }
            }
        };
        sweep.findings.check(command, IN_RANGE, outcome);
    }
    sweep.restore();
}

/// The sweep's edits that succeeded, applied together in chunks without
/// putting the set back between them, then saved and opened again: the set
/// that opens is the one that was saved. A chunk that doesn't survive is
/// tried a command at a time, to name the command.
fn save_and_load(sweep: &mut Sweep, dir: &std::path::Path, full: bool) {
    let replaces = |id: &str| id.starts_with("set.") || id.starts_with("edit.");
    let edits: Vec<(String, Value)> = sweep
        .edits
        .iter()
        .filter(|(id, _)| !replaces(id))
        .cloned()
        .collect();
    let chunk = if full { 10 } else { 60 };
    let mut round = 0;
    let mut survive = |sweep: &mut Sweep, list: &[(String, Value)]| -> Result<(), String> {
        sweep.restore();
        for (command, args) in list {
            let _ = sweep.call(command, args);
            sweep.host.settle();
        }
        round += 1;
        let folder = dir.join(format!("round {round}"));
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let saved_path = sweep
            .host
            .run(
                "set.save_as",
                json!({ "path": folder.join("E2E.blset").to_string_lossy() }),
            )
            .map_err(|e| format!("the save failed: {e}"))?;
        let saved = sweep.host.document().clone();
        let path = saved_path["path"].as_str().unwrap_or_default().to_owned();
        sweep
            .host
            .run("set.open", json!({ "path": path }))
            .map_err(|e| format!("the saved set didn't open: {e}"))?;
        sweep.host.pump();
        let opened = sweep.host.document().clone();
        if opened == saved {
            Ok(())
        } else {
            Err(format!(
                "the set that opened differs from the one saved: {:?}",
                document_diff(&saved, &opened, 5)
            ))
        }
    };
    for list in edits.chunks(chunk) {
        if survive(sweep, list).is_ok() {
            for (command, _) in list {
                sweep
                    .findings
                    .check(command, "survives save and load", Ok(()));
            }
            continue;
        }
        for one in list {
            let outcome = survive(sweep, std::slice::from_ref(one));
            sweep
                .findings
                .check(&one.0, "survives save and load", outcome);
        }
    }
    // The sweep's own file is the set's again.
    let saved = sweep.saved.clone();
    let _ = sweep.host.shell_mut().run("set.discard_changes", json!({}));
    let _ = sweep
        .host
        .run("set.open", json!({ "path": saved.to_string_lossy() }));
    sweep.restore();
}
