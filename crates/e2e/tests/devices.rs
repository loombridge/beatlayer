//! Every native device type, and each kind of rack, swept parameter by
//! parameter through the agent API and rendered offline.
//!
//! Each subject goes on a track of its own (`bl_e2e::device_rig`): an
//! instrument plays notes at five velocities, a MIDI effect plays them into
//! Cascade, an audio effect hears the fixture sine and then the drum loop.
//! For every parameter the sweep sets its minimum and maximum (and at full
//! depth its default, 25 %, 75 % and an automated sweep from minimum to
//! maximum), renders a second, and checks, by the names the known-failures
//! list uses:
//!
//! - `output finite`: no render holds a NaN or an infinity;
//! - `level bounded`: no setting raises the level more than 18 dB above the
//!   default setting's (or for an effect, the level without it where that's
//!   louder), unless the parameter is itself a level control;
//! - `value reads back`: the query tool reads the value set, or its clamped
//!   value;
//! - `state survives reopen`: saving and opening the set again keeps every
//!   parameter;
//!
//! and for the device as a whole:
//!
//! - `deterministic`: two renders of the same settings are the same, unless
//!   the device is random by design;
//! - `insert and delete`: deleting it, undoing and redoing that, inserting
//!   it again and undoing and redoing that keep the document's invariants,
//!   give back the documents before and after each step, and keep the
//!   engine running;
//! - for an instrument, `velocity scales`, per velocity-sensitivity
//!   parameter: at its maximum the softest note is no louder than with it
//!   at 0 (the class of bug where a percentage was taken as a fraction and
//!   boosted soft notes).
//!
//! Keys are `<device type id>/<parameter name>`, or the device type's ID
//! alone (`rack:<kind>` for a rack) for what concerns the whole device. The
//! subjects are shared out between several tests by weight, so they run in
//! parallel.

use bl_e2e::audio::Audio;
use bl_e2e::device_rig::{
    self, FLOOR_DB, LEVEL_HEADROOM_DB, LEVEL_RULE, RENDER_LENGTH, Rig, Subject, VELOCITY_RULE,
    is_level_control, is_velocity_sensitivity, level, other_value, points, random_by_design,
};
use bl_e2e::{Driver, Known, Ratchet, document_diff, full_depth};
use bl_model::ParamDescriptor;
use serde_json::json;

/// The suite's name in the known-failures list.
const SUITE: &str = "devices";

/// How many tests share the subjects out between them.
const SHARDS: usize = 8;

/// How much louder the softest note may be with a velocity sensitivity at
/// its maximum than with it at 0, in dB, for rounding.
const VELOCITY_TOLERANCE_DB: f64 = 0.5;

#[test]
fn the_catalogue_lists_every_native_device_type_with_sound_descriptors() {
    let subjects = device_rig::subjects();
    let native = bl_devices::device_types();
    assert_eq!(
        subjects.len(),
        native.len() + 4,
        "every native type and the four kinds of rack"
    );
    let mut roles = std::collections::BTreeMap::<&str, usize>::new();
    let mut params = 0;
    for subject in &subjects {
        *roles
            .entry(device_rig::role_name(subject.role))
            .or_default() += 1;
        params += subject.params.len();
        let mut keys = std::collections::BTreeSet::new();
        for param in &subject.params {
            // Spec: device-framework R3. A descriptor's range isn't empty and
            // holds its default.
            assert!(
                param.min() <= param.max() && param.min().is_finite() && param.max().is_finite(),
                "{}: an empty or infinite range",
                subject.param_key(param)
            );
            assert!(
                param.clamp(param.default) == param.default,
                "{}: the default {} lies outside its range",
                subject.param_key(param),
                param.default
            );
            assert!(
                keys.insert(subject.param_key(param)),
                "{}: two parameters share a key",
                subject.param_key(param)
            );
        }
    }
    eprintln!(
        "{} subjects ({roles:?}), {params} parameters; {SHARDS} sweep tests",
        subjects.len()
    );
    let shared: usize = (0..SHARDS)
        .map(|i| device_rig::shard(device_rig::subjects(), i, SHARDS).len())
        .sum();
    assert_eq!(shared, subjects.len(), "each subject is in one shard");
}

/// One sweep test per shard.
macro_rules! sweeps {
    ($($name:ident = $index:expr;)*) => {
        $(
            #[test]
            fn $name() {
                sweep_shard($index);
            }
        )*
    };
}

sweeps! {
    sweep_1_of_8 = 0;
    sweep_2_of_8 = 1;
    sweep_3_of_8 = 2;
    sweep_4_of_8 = 3;
    sweep_5_of_8 = 4;
    sweep_6_of_8 = 5;
    sweep_7_of_8 = 6;
    sweep_8_of_8 = 7;
}

/// Sweeps every subject in shard `index`, then settles the ratchet.
fn sweep_shard(index: usize) {
    let full = full_depth();
    let subjects = device_rig::shard(device_rig::subjects(), index, SHARDS);
    eprintln!(
        "level bounded: within {LEVEL_HEADROOM_DB} dB of the default setting, where {LEVEL_RULE}"
    );
    eprintln!("velocity scales: {VELOCITY_RULE}");
    let mut ratchet = Known::load().ratchet(SUITE);
    for subject in &subjects {
        sweep(subject, full, &mut ratchet);
    }
    ratchet.finish();
}

/// What one parameter's sweep found, property by property.
#[derive(Default)]
struct Findings {
    finite: Vec<String>,
    bounded: Vec<String>,
    reads: Vec<String>,
    /// The largest rise above the reference, in dB, and where, for a
    /// parameter that isn't a level control.
    loudest: Option<(f64, String)>,
}

impl Findings {
    /// Records each property's outcome for `key`.
    fn record(self, ratchet: &mut Ratchet, key: &str) -> Option<(f64, String)> {
        for (property, found) in [
            ("output finite", self.finite),
            ("value reads back", self.reads),
            ("level bounded", self.bounded),
        ] {
            let outcome = if found.is_empty() {
                Ok(())
            } else {
                Err(found.join("; "))
            };
            ratchet.check(key, property, outcome);
        }
        self.loudest
            .map(|(rise, what)| (rise, format!("{key} at {what}")))
    }

    /// Checks a render made at `what`: finite, and within the headroom of
    /// `reference` unless `param` is a level control.
    fn judge(&mut self, audio: &Audio, what: &str, param: &ParamDescriptor, reference: f64) {
        if let Some((channel, frame)) = audio.first_non_finite() {
            self.finite.push(format!(
                "at {what}, channel {channel} frame {frame} isn't finite"
            ));
            return;
        }
        let got = level(audio);
        if !is_level_control(param)
            && self
                .loudest
                .as_ref()
                .is_none_or(|(rise, _)| got - reference > *rise)
        {
            self.loudest = Some((got - reference, what.to_owned()));
        }
        if !is_level_control(param) && got > reference + LEVEL_HEADROOM_DB {
            self.bounded.push(format!(
                "at {what} the level is {got:.1} dBFS, {:+.1} dB against the default \
                 setting's {reference:.1} dBFS",
                got - reference
            ));
        }
    }
}

/// Sweeps one subject (see the module's documentation).
fn sweep(subject: &Subject, full: bool, ratchet: &mut Ratchet) {
    let mut rig = Rig::new(subject);
    let mut renders = 0usize;
    let default = rig.render();
    let again = rig.render();
    renders += 2;
    let key = subject.key.clone();
    ratchet.check(
        &key,
        "output finite",
        match default.first_non_finite() {
            Some((channel, frame)) => Err(format!(
                "at its defaults, channel {channel} frame {frame} isn't finite"
            )),
            None => Ok(()),
        },
    );
    // Spec: device-framework R29. A device's randomness follows a seed, so
    // a render repeats run after run.
    let deterministic = if default == again {
        Ok(())
    } else if let Some(why) = random_by_design(&key) {
        eprintln!("{key}: renders differ, random by design: {why}");
        Ok(())
    } else {
        Err(difference(&default, &again))
    };
    ratchet.check(&key, "deterministic", deterministic);
    // An effect passing its input through is never a boost: the level
    // without it is a floor for the reference.
    let reference = if subject.is_instrument() {
        level(&default)
    } else {
        rig.set_enabled(false);
        let dry = rig.render();
        renders += 1;
        rig.set_enabled(true);
        level(&default).max(level(&dry))
    };

    let mut loudest: Option<(f64, String)> = None;
    let mut changed = 0usize;
    let mut peak = f64::NEG_INFINITY;
    for param in &subject.params {
        let mut found = Findings::default();
        for (label, value) in points(param, full) {
            let what = format!("its {label} ({value})");
            if let Err(error) = rig.set(param, value) {
                found.reads.push(format!("setting {what} failed: {error}"));
                continue;
            }
            // Spec: device-framework R14. What an agent sets, it reads back.
            match rig.read(param) {
                Ok(read) if read == value || read == param.clamp(value) => {}
                Ok(read) => found.reads.push(format!("set {what}, read {read}")),
                Err(error) => found.reads.push(format!("reading {what}: {error}")),
            }
            if value == param.default {
                // The default setting's render is the reference already.
                continue;
            }
            let audio = rig.render();
            renders += 1;
            if audio != default {
                changed += 1;
            }
            peak = peak.max(audio.peak_dbfs() - default.peak_dbfs());
            found.judge(&audio, &what, param, reference);
        }
        if let Err(error) = rig.set(param, param.default) {
            found
                .reads
                .push(format!("setting the default back failed: {error}"));
        }
        if full {
            renders += automated_sweep(&mut rig, param, reference, &mut found);
        }
        if let Some((rise, what)) = found.record(ratchet, &subject.param_key(param))
            && loudest.as_ref().is_none_or(|(most, _)| rise > *most)
        {
            loudest = Some((rise, what));
        }
    }

    if subject.is_instrument() {
        renders += velocity_scales(&mut rig, ratchet);
    }
    state_survives_reopen(&mut rig, ratchet);
    ratchet.check(&key, "insert and delete", insert_and_delete(&mut rig));
    let loudest = loudest.map_or_else(
        || "no render".to_owned(),
        |(rise, what)| format!("{rise:+.1} dB, {what}"),
    );
    eprintln!(
        "{key} ({}, {}): {} parameters, {renders} renders, reference level {reference:.1} dBFS, \
         loudest {loudest}; {changed} renders differ from the default's; peak rise {peak:+.1} dB",
        subject.name,
        device_rig::role_name(subject.role),
        subject.params.len()
    );
}

/// Where two renders that should be equal differ.
fn difference(a: &Audio, b: &Audio) -> String {
    if a.frames() != b.frames() || a.channels.len() != b.channels.len() {
        return format!(
            "{} then {} frames, {} then {} channels",
            a.frames(),
            b.frames(),
            a.channels.len(),
            b.channels.len()
        );
    }
    let first = (0..a.frames()).find(|&frame| {
        a.channels
            .iter()
            .zip(&b.channels)
            .any(|(x, y)| x[frame].to_bits() != y[frame].to_bits())
    });
    let residual = bl_e2e::audio::aligned_residual_db(a, b, 0);
    format!(
        "two renders of the default setting differ from frame {first:?}, residual {residual:.1} dB"
    )
}

/// Automates `param` from its minimum at the song's start to its maximum
/// at the render's end, renders, judges it, and takes the automation away
/// again. Gives how many renders it made.
fn automated_sweep(
    rig: &mut Rig,
    param: &ParamDescriptor,
    reference: f64,
    found: &mut Findings,
) -> usize {
    let target = json!({ "type": "device", "device": rig.device, "param": param.id });
    let points = [(0, param.min()), (RENDER_LENGTH, param.max())].map(|(at, value)| {
        json!({ "command": "automation.set_breakpoint", "args": { "track": rig.track,
                "target": target, "at": at, "value": value } })
    });
    if let Err(error) = rig.host.batch("Sweep", points.to_vec()) {
        found.reads.push(format!("automating it failed: {error}"));
        return 0;
    }
    let audio = rig.render();
    found.judge(&audio, "an automated sweep", param, reference);
    rig.host.run_ok("edit.undo", json!({}));
    1
}

/// For each velocity sensitivity (see `is_velocity_sensitivity`), the
/// softest note at its maximum against at 0. Gives how many renders it
/// made.
fn velocity_scales(rig: &mut Rig, ratchet: &mut Ratchet) -> usize {
    let params: Vec<ParamDescriptor> = rig
        .subject
        .params
        .iter()
        .filter(|p| is_velocity_sensitivity(p))
        .cloned()
        .collect();
    let mut renders = 0;
    for param in params {
        let key = rig.subject.param_key(&param);
        let mut levels = Vec::new();
        let mut outcome = Ok(());
        for value in [param.max(), param.clamp(0.0)] {
            if let Err(error) = rig.set(&param, value) {
                outcome = Err(format!("setting {value} failed: {error}"));
                break;
            }
            levels.push(Rig::note_levels(&rig.render()));
            renders += 1;
        }
        let _ = rig.set(&param, param.default);
        if let [at_max, at_zero] = levels.as_slice() {
            let (soft_max, soft_zero) = (at_max[0], at_zero[0]);
            if soft_max.is_nan() || soft_zero.is_nan() {
                outcome = Err("a render isn't finite".to_owned());
            } else if soft_max > soft_zero + VELOCITY_TOLERANCE_DB && soft_max > FLOOR_DB {
                outcome = Err(format!(
                    "at its maximum ({}) the softest note is {soft_max:.1} dBFS, {:+.1} dB \
                     louder than with it at 0; each note at maximum {at_max:.1?}, at 0 \
                     {at_zero:.1?}",
                    param.max(),
                    soft_max - soft_zero
                ));
            }
        }
        if let [at_max, at_zero] = levels.as_slice() {
            eprintln!(
                "{key}: velocity scales, softest note {:.1} dBFS at its maximum, {:.1} at 0",
                at_max[0], at_zero[0]
            );
        }
        ratchet.check(&key, "velocity scales", outcome);
    }
    renders
}

/// Sets every parameter to a value other than its default, saves the set,
/// opens it again, and checks each parameter kept its value.
fn state_survives_reopen(rig: &mut Rig, ratchet: &mut Ratchet) {
    let wanted: Vec<(ParamDescriptor, f64)> = rig
        .subject
        .params
        .iter()
        .map(|p| (p.clone(), other_value(p)))
        .collect();
    let sets: Vec<serde_json::Value> = wanted
        .iter()
        .map(|(param, value)| {
            json!({ "command": "device.set_param",
                    "args": { "device": rig.device, "param": param.id, "value": value } })
        })
        .collect();
    let key = rig.subject.key.clone();
    if !sets.is_empty()
        && let Err(error) = rig.host.batch("Set every parameter", sets)
    {
        ratchet.violation(
            &key,
            "state survives reopen",
            format!("setting every parameter failed: {error}"),
        );
        return;
    }
    let dir = rig.host.dir().join("reopen");
    std::fs::create_dir_all(&dir).expect("a folder for the set");
    rig.host.save_to(&dir);
    if let Err(error) = rig.host.reopen() {
        ratchet.violation(
            &key,
            "state survives reopen",
            format!("reopening failed: {error}"),
        );
        return;
    }
    rig.host.settle();
    let values = match rig.read_all() {
        Ok(values) => values,
        Err(error) => {
            ratchet.violation(
                &key,
                "state survives reopen",
                format!("reading the reopened device failed: {error}"),
            );
            return;
        }
    };
    for (param, value) in &wanted {
        let outcome = match values.get(&param.id.0) {
            Some(read) if *read == param.clamp(*value) => Ok(()),
            Some(read) => Err(format!("set {value}, read {read} after reopening")),
            None => Err("missing after reopening".to_owned()),
        };
        ratchet.check(
            &rig.subject.param_key(param),
            "state survives reopen",
            outcome,
        );
    }
}

/// Deletes the device, undoes and redoes that, inserts it again, undoes
/// and redoes that, checking the document and the engine at each step.
fn insert_and_delete(rig: &mut Rig) -> Result<(), String> {
    fn step(rig: &mut Rig, what: &str) -> Result<bl_model::Document, String> {
        let host = &mut rig.host;
        host.document().check_invariants().map_err(|error| {
            format!("after {what}, the document breaks its invariants: {error}")
        })?;
        if !host.shell().audio().engine_running() {
            return Err(format!("after {what}, the engine stopped"));
        }
        Ok(host.document().clone())
    }
    fn same(a: &bl_model::Document, b: &bl_model::Document, what: &str) -> Result<(), String> {
        if a == b {
            Ok(())
        } else {
            Err(format!("{what}: {:?}", document_diff(a, b, 5)))
        }
    }
    let with = step(rig, "the sweep")?;
    let device = rig.device;
    rig.host
        .run("device.remove", json!({ "device": device }))
        .map_err(|error| format!("device.remove failed: {error}"))?;
    let without = step(rig, "deleting it")?;
    if without == with {
        return Err("deleting it changed nothing".to_owned());
    }
    for (command, want, what) in [
        ("edit.undo", &with, "undoing the delete"),
        ("edit.redo", &without, "redoing the delete"),
    ] {
        rig.host
            .run(command, json!({}))
            .map_err(|error| format!("{what} failed: {error}"))?;
        let now = step(rig, what)?;
        same(&now, want, what)?;
    }
    let device = rig.subject.device("again");
    rig.host
        .run(
            "device.insert",
            json!({ "track": rig.track, "index": 0, "device": device }),
        )
        .map_err(|error| format!("inserting it again failed: {error}"))?;
    let inserted = step(rig, "inserting it again")?;
    for (command, want, what) in [
        ("edit.undo", &without, "undoing the insert"),
        ("edit.redo", &inserted, "redoing the insert"),
    ] {
        rig.host
            .run(command, json!({}))
            .map_err(|error| format!("{what} failed: {error}"))?;
        let now = step(rig, what)?;
        same(&now, want, what)?;
    }
    // The engine plays the chain as it is now.
    let live = rig.host.play_for(100);
    if let Some((channel, frame)) = live.first_non_finite() {
        return Err(format!(
            "after inserting it again, the live output's channel {channel} frame {frame} \
             isn't finite"
        ));
    }
    step(rig, "playing").map(|_| ())
}
