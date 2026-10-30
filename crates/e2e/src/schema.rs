//! Arguments for any registered command, made from its JSON Schema and the
//! objects of a set: the command sweep's synthesiser.
//!
//! A command's argument schema names what it wants by type: a `TrackId`, a
//! `ClipId`, a `DeviceId`, `Ticks`, an enum of strings, an object of
//! fields. [`Synth`] walks the schema, following its `$ref` definitions, and
//! fills each place from [`Refs`], the IDs of the set the sweep runs on. It
//! chooses the kind of object from what the command and the field say of
//! themselves: a note command wants a MIDI clip, a warp command an audio
//! clip, a send's `to` a return track, a multisampler command the
//! multisampler. Where a command makes an object, its ID is a `{"$new":
//! name}` placeholder, which the agent API's `run` tool fills. Files a
//! command writes go in [`Refs::dir`]; files it reads are the repository's
//! fixtures, or files the sweep wrote first.
//!
//! [`Synth::candidates`] gives a command's argument sets in the order a
//! sweep tries them: the required fields with the optional ones a person's
//! selection would otherwise supply, then only the required ones, then
//! every field. [`Synth::variants`] gives more for a full run: every enum
//! variant and the bounds of every bounded number, one field at a time.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::fixture::{Clips, Ids};
use crate::{BAR, BEAT};

/// The objects of the set a sweep runs on, by ID, and the files it can
/// read: what [`Synth`] fills arguments from.
#[derive(Clone, Debug, Default)]
pub struct Refs {
    /// MIDI tracks, the one with an instrument and notes first.
    pub midi_tracks: Vec<u64>,
    /// Audio tracks, the one with the drum loop first.
    pub audio_tracks: Vec<u64>,
    /// Group tracks.
    pub groups: Vec<u64>,
    /// Return tracks.
    pub returns: Vec<u64>,
    /// The Main track.
    pub main: u64,
    /// The Arrangement's clips, by kind.
    pub arrangement: Clips,
    /// The Session's clips, by kind.
    pub session: Clips,
    /// Devices by type ID; racks by `rack:<kind>`.
    pub devices: BTreeMap<String, Vec<u64>>,
    /// The type of each device in [`devices`](Self::devices).
    pub device_types: BTreeMap<u64, String>,
    /// Native device types' parameters: ID, lowest value, highest value
    /// and default, in the descriptors' order.
    pub params: BTreeMap<String, Vec<(u32, f64, f64, f64)>>,
    /// Chains of the audio effect rack.
    pub chains: Vec<u64>,
    /// Chains of the drum rack.
    pub drum_chains: Vec<u64>,
    /// Scenes, top to bottom.
    pub scenes: Vec<u64>,
    /// Locators, in order.
    pub locators: Vec<u64>,
    /// Grooves in the Groove Pool.
    pub grooves: Vec<u64>,
    /// Take lanes.
    pub take_lanes: Vec<u64>,
    /// Layers of the multisampler.
    pub layers: Vec<u64>,
    /// Link groups.
    pub link_groups: Vec<u64>,
    /// Content hashes of media the set uses.
    pub media: Vec<String>,
    /// Files a command can read, by kind: `wav`, `blpreset`,
    /// `chain_preset`, `blgroove`, `mid`, `sfz`, `scl`, `dawproject`,
    /// `blclip`, `blset`, `backup`, `blpack`.
    pub files: BTreeMap<String, PathBuf>,
    /// The folder new files go in.
    pub dir: PathBuf,
}

impl Refs {
    /// The objects the rich set holds, as [`crate::fixture::build`] made
    /// them, with new files going in `dir`.
    #[must_use]
    pub fn from_ids(ids: &Ids, dir: PathBuf) -> Self {
        let mut device_types = BTreeMap::new();
        for (kind, list) in &ids.devices {
            for id in list {
                device_types.insert(*id, kind.clone());
            }
        }
        let mut params = BTreeMap::new();
        for device_type in bl_devices::device_types() {
            if let bl_model::DeviceKind::Native { device_type: id } = &device_type.kind {
                params.insert(
                    id.clone(),
                    device_type
                        .description
                        .params
                        .iter()
                        .map(|p| (p.id.0, p.min(), p.max(), p.default))
                        .collect(),
                );
            }
        }
        Self {
            midi_tracks: ids.midi_tracks.clone(),
            audio_tracks: ids.audio_tracks.clone(),
            groups: ids.groups.clone(),
            returns: ids.returns.clone(),
            main: ids.main,
            arrangement: ids.arrangement.clone(),
            session: ids.session.clone(),
            devices: ids.devices.clone(),
            device_types,
            params,
            chains: ids.chains.clone(),
            scenes: ids.scenes.clone(),
            locators: ids.locators.clone(),
            dir,
            ..Self::default()
        }
    }

    /// Records a device of type `kind` (a rack: `rack:<kind>`).
    pub fn add_device(&mut self, kind: &str, id: u64) {
        self.devices.entry(kind.to_owned()).or_default().push(id);
        self.device_types.insert(id, kind.to_owned());
    }

    /// The first device of type `kind`, if there is one.
    #[must_use]
    pub fn device(&self, kind: &str) -> Option<u64> {
        self.devices
            .get(kind)
            .and_then(|list| list.first().copied())
    }

    fn file(&self, kind: &str) -> String {
        self.files
            .get(kind)
            .map_or_else(|| self.dir.join(format!("missing.{kind}")), Clone::clone)
            .to_string_lossy()
            .into_owned()
    }
}

/// The kind of clip or track a command is to act on, when its first
/// arguments were refused for being the other kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    /// Audio clips and audio tracks.
    Audio,
    /// MIDI clips and MIDI tracks.
    Midi,
}

/// Which optional fields an argument set fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fill {
    /// The required fields, and the optional ones a selection or a
    /// context would otherwise supply: the objects acted on and the span.
    Selection,
    /// Only the required fields.
    Required,
    /// Every field.
    All,
}

/// Optional fields that stand for what a person has selected or what a view
/// shows, which a sweep gives so a command doesn't depend on the view.
const SELECTION: &[&str] = &[
    "clip",
    "clips",
    "track",
    "tracks",
    "device",
    "chain",
    "rack",
    "scene",
    "notes",
    "lanes",
    "lane",
    "start",
    "end",
    "locator",
    "locators",
    "layer",
    "layers",
    "groove",
    "at",
    "chains",
    "devices",
    "pad",
    "macro_number",
    "index",
    "path",
    "target",
    "kind",
    "window",
];

/// Makes argument sets for commands from their schemas and a set's
/// [`Refs`].
#[derive(Debug)]
pub struct Synth {
    /// The set's objects.
    pub refs: Refs,
    /// The kind of clip and track to choose, overriding what the command
    /// says of itself; `None` to go by what it says.
    pub prefer: Option<Prefer>,
    fresh: u64,
}

/// One walk through a command's schema.
struct Walk<'a> {
    refs: &'a Refs,
    root: &'a Value,
    command: &'a str,
    /// The command's ID and description, lower-cased.
    about: String,
    fill: Fill,
    prefer: Option<Prefer>,
    fresh: &'a mut u64,
    /// The device chosen last, whose parameters a `ParamId` names.
    device: Option<u64>,
    /// The track chosen last.
    track: Option<u64>,
    /// How deep in the arguments the walk is: 0 for the top-level fields.
    depth: usize,
    /// Which branch of a `oneOf` or `anyOf`, or which enum variant, to
    /// take at the top level of the field being varied.
    branch: usize,
}

impl Synth {
    /// A synthesiser over `refs`.
    #[must_use]
    pub fn new(refs: Refs) -> Self {
        Self {
            refs,
            prefer: None,
            fresh: 0,
        }
    }

    /// `command`'s argument sets (its `args` schema is `schema`, its
    /// description `description`), in the order to try them: the required
    /// fields with those a selection would supply, the required fields
    /// alone, then every field. Duplicates are left out.
    #[must_use]
    pub fn candidates(&mut self, command: &str, description: &str, schema: &Value) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        for fill in [Fill::Selection, Fill::Required, Fill::All] {
            let args = self.walk(command, description, schema, fill, 0, |walk| {
                walk.value(schema, "", "")
            });
            let args = args.unwrap_or_else(|| json!({}));
            if !out.iter().any(|seen| same_but_fresh(seen, &args)) {
                out.push(args);
            }
        }
        out
    }

    /// More argument sets for a full run, each `base` with one top-level
    /// field changed: every variant of an enum or a `oneOf`, every branch
    /// of an `anyOf` (null included), and the lowest and highest value of a
    /// bounded number. Each comes with a label naming the field and value.
    #[must_use]
    pub fn variants(
        &mut self,
        command: &str,
        description: &str,
        schema: &Value,
        base: &Value,
    ) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return out;
        };
        for (name, field) in properties {
            let resolved = resolve(schema, field);
            // Enum variants and union branches.
            let count = branch_count(schema, resolved);
            for branch in 0..count {
                let value = self.walk(command, description, schema, Fill::Required, branch, |w| {
                    w.depth = 1;
                    w.value(field, name, "")
                });
                let Some(value) = value else { continue };
                if base.get(name) == Some(&value) {
                    continue;
                }
                let mut args = base.clone();
                args[name.as_str()] = value.clone();
                out.push((format!("{name} = {}", short(&value)), args));
            }
            // A bounded number's ends.
            if is_number(resolved) {
                for bound in ["minimum", "maximum"] {
                    let Some(limit) = resolved.get(bound).and_then(Value::as_f64) else {
                        continue;
                    };
                    // Integer formats' own bounds (u64's) aren't useful ends.
                    if limit.abs() > 1e9 {
                        continue;
                    }
                    let value = if resolved["type"] == "integer" {
                        json!(limit as i64)
                    } else {
                        json!(limit)
                    };
                    if base.get(name) == Some(&value) {
                        continue;
                    }
                    let mut args = base.clone();
                    args[name.as_str()] = value.clone();
                    out.push((format!("{name} = {value}"), args));
                }
            }
        }
        out
    }

    fn walk<T>(
        &mut self,
        command: &str,
        description: &str,
        schema: &Value,
        fill: Fill,
        branch: usize,
        run: impl FnOnce(&mut Walk<'_>) -> T,
    ) -> T {
        let mut walk = Walk {
            refs: &self.refs,
            root: schema,
            command,
            about: format!("{command} {description}").to_lowercase(),
            fill,
            prefer: self.prefer,
            fresh: &mut self.fresh,
            device: None,
            track: None,
            depth: 0,
            branch,
        };
        run(&mut walk)
    }
}

/// Whether two argument sets are the same but for the names of their
/// `$new` placeholders.
fn same_but_fresh(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            if x.contains_key("$new") && y.contains_key("$new") {
                return true;
            }
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_but_fresh(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same_but_fresh(v, w))
        }
        _ => a == b,
    }
}

/// A value, cut short for a label.
fn short(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 40 {
        format!("{}…", &text[..text.floor_char_boundary(40)])
    } else {
        text
    }
}

/// `schema` with its `$ref`s followed, within `root`'s `$defs`.
#[must_use]
pub fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    let mut here = schema;
    for _ in 0..16 {
        match here
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| root.pointer(r.strip_prefix('#')?))
        {
            Some(target) => here = target,
            None => break,
        }
    }
    here
}

/// The definition name a schema references, such as `TrackId`.
fn ref_name(schema: &Value) -> Option<&str> {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.rsplit('/').next())
}

/// How many variants or branches a field has worth trying one by one.
fn branch_count(root: &Value, schema: &Value) -> usize {
    if let Some(list) = schema.get("enum").and_then(Value::as_array) {
        return list.len();
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(list) = schema.get(key).and_then(Value::as_array) {
            // A reference wrapped in a nullable union counts its target's.
            let inner: usize = list
                .iter()
                .map(|b| {
                    let r = resolve(root, b);
                    if r.get("oneOf").is_some() || r.get("enum").is_some() {
                        branch_count(root, r)
                    } else {
                        1
                    }
                })
                .sum();
            return inner.max(list.len());
        }
    }
    if schema.get("type") == Some(&json!(["boolean", "null"])) || schema["type"] == "boolean" {
        return 2;
    }
    0
}

fn is_number(schema: &Value) -> bool {
    matches!(schema["type"].as_str(), Some("number" | "integer"))
}

/// The types a schema allows, without `null`.
fn types(schema: &Value) -> Vec<&str> {
    match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect(),
        _ => Vec::new(),
    }
}

/// The first `a to b` range a description gives, as numbers.
fn range_in(text: &str) -> Option<(f64, f64)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for window in words.windows(3) {
        if window[1] == "to" {
            let number = |w: &str| {
                w.trim_matches(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '−'))
                    .replace('−', "-")
                    .parse::<f64>()
                    .ok()
            };
            if let (Some(a), Some(b)) = (number(window[0]), number(window[2]))
                && a < b
            {
                return Some((a, b));
            }
        }
    }
    None
}

impl Walk<'_> {
    fn new_name(&mut self, what: &str) -> Value {
        *self.fresh += 1;
        json!({ "$new": format!("e2e {what} {}", self.fresh) })
    }

    /// Whether the command or the field mentions any of `words`.
    fn says(&self, hint: &str, words: &[&str]) -> bool {
        words
            .iter()
            .any(|w| hint.contains(w) || self.about.contains(w))
    }

    /// A value for the field `name`, whose schema is `schema` and whose
    /// description so far is `hint`; `None` when nothing fits.
    fn value(&mut self, schema: &Value, name: &str, hint: &str) -> Option<Value> {
        let own = schema
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let hint = if own.is_empty() {
            hint.to_owned()
        } else {
            format!("{own} {hint}")
        };
        if let Some(def) = ref_name(schema) {
            if let Some(value) = self.reference(def, name, &hint) {
                return Some(value);
            }
            let target = resolve(self.root, schema);
            return self.value(target, name, &hint);
        }
        if let Some(value) = schema.get("const") {
            return Some(value.clone());
        }
        if let Some(list) = schema.get("enum").and_then(Value::as_array) {
            return list.get(self.take_branch(list.len())).cloned();
        }
        if let Some(list) = schema.get("allOf").and_then(Value::as_array) {
            return list.first().and_then(|b| self.value(b, name, &hint));
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(list) = schema.get(key).and_then(Value::as_array) {
                return self.union(list, name, &hint);
            }
        }
        let kinds = types(schema);
        let kind = kinds.first().copied().unwrap_or("object");
        if kinds.is_empty() && schema.get("type") == Some(&json!("null")) {
            return Some(Value::Null);
        }
        match kind {
            "object" => self.object(schema, name, &hint),
            "array" => self.array(schema, name, &hint),
            "string" => Some(json!(self.string(schema, name, &hint))),
            "integer" => Some(self.integer(schema, name, &hint)),
            "number" => Some(self.number(schema, name, &hint)),
            "boolean" => Some(json!(self.boolean(name))),
            _ => None,
        }
    }

    /// Takes the branch the walk was asked for, at the top level of the
    /// varied field, and the first elsewhere.
    fn take_branch(&mut self, count: usize) -> usize {
        if self.depth <= 1 && self.branch > 0 && count > 0 {
            let branch = self.branch.min(count - 1);
            // Taken once: deeper unions take their first.
            self.branch = 0;
            branch
        } else {
            0
        }
    }

    fn union(&mut self, list: &[Value], name: &str, hint: &str) -> Option<Value> {
        // A nullable value: the value, unless the varied branch is the null.
        let non_null: Vec<&Value> = list
            .iter()
            .filter(|b| b.get("type") != Some(&json!("null")))
            .collect();
        let has_null = non_null.len() < list.len();
        if has_null && non_null.len() == 1 {
            if self.depth <= 1 && self.branch == 1 {
                self.branch = 0;
                return Some(Value::Null);
            }
            return self.value(non_null[0], name, hint);
        }
        // A string pattern beside a reference (a position in ticks or as
        // text): the reference.
        let preferred = self.preferred_branch(&non_null, hint);
        let branch = if self.depth <= 1 && self.branch > 0 {
            self.take_branch(non_null.len())
        } else {
            preferred
        };
        let mut order: Vec<usize> = vec![branch];
        order.extend((0..non_null.len()).filter(|i| *i != branch));
        for index in order {
            if let Some(value) = self.value(non_null[index], name, hint) {
                return Some(value);
            }
        }
        if has_null {
            return Some(Value::Null);
        }
        None
    }

    /// The branch of a union that suits the command, by what it says: a
    /// `track` lane, a `device` target, a `mixer` parameter.
    fn preferred_branch(&self, branches: &[&Value], hint: &str) -> usize {
        let tag = |b: &Value| -> Option<String> {
            let b = resolve(self.root, b);
            let props = b.get("properties")?.as_object()?;
            props
                .values()
                .find_map(|p| p.get("const")?.as_str().map(str::to_owned))
        };
        let wants: &[&str] = if self.says(hint, &["device parameter", "device's parameter"]) {
            &["device"]
        } else if self.command.starts_with("clip.") && self.says(hint, &["audio clip"]) {
            &["clip", "mixer"]
        } else {
            &["track", "mixer", "device"]
        };
        for want in wants {
            if let Some(index) = branches
                .iter()
                .position(|b| tag(b).as_deref() == Some(*want))
            {
                return index;
            }
        }
        0
    }

    fn object(&mut self, schema: &Value, _name: &str, hint: &str) -> Option<Value> {
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut out = Map::new();
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return Some(Value::Object(out));
        };
        // The device comes before its parameter, the track before what is on it.
        let mut names: Vec<&String> = properties.keys().collect();
        names.sort_by_key(|n| {
            (
                match n.as_str() {
                    "track" | "tracks" => 0,
                    "device" | "rack" => 1,
                    _ => 2,
                },
                n.to_string(),
            )
        });
        let top = self.depth == 0;
        self.depth += 1;
        for field in names {
            let is_required = required.contains(&field.as_str());
            let fill = if top { self.fill } else { Fill::Required };
            // A list in place of a single object (`clips` beside `clip`):
            // the single one, as a selection gives.
            let instead = field
                .strip_suffix('s')
                .is_some_and(|one| properties.contains_key(one));
            let wanted = is_required
                || match fill {
                    Fill::All => true,
                    Fill::Required => false,
                    Fill::Selection => SELECTION.contains(&field.as_str()) && !instead,
                };
            if !wanted {
                continue;
            }
            let field_schema = &properties[field];
            let field_hint = if top { String::new() } else { hint.to_owned() };
            match self.value(field_schema, field, &field_hint) {
                Some(value) => {
                    out.insert(field.clone(), value);
                }
                None if is_required => {
                    self.depth -= 1;
                    return None;
                }
                None => {}
            }
        }
        self.depth -= 1;
        Some(Value::Object(out))
    }

    fn array(&mut self, schema: &Value, name: &str, hint: &str) -> Option<Value> {
        let min = schema
            .get("minItems")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .max(1);
        // A fixed tuple, as JSON Schema 2020-12 writes one.
        if let Some(list) = schema.get("prefixItems").and_then(Value::as_array) {
            // A pair of one kind of object (`[from, to]`) names an object
            // and the new one made from it.
            let pair = list.len() == 2
                && ref_name(&list[0]).is_some()
                && ref_name(&list[0]) == ref_name(&list[1])
                && ref_name(&list[0]).is_some_and(|d| d.ends_with("Id"));
            let values: Option<Vec<Value>> = list
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let def = ref_name(item).unwrap_or_default();
                    if pair && i == 1 {
                        Some(self.new_name(def.trim_end_matches("Id")))
                    } else {
                        self.value(item, name, hint)
                    }
                })
                .collect();
            return values.map(Value::Array);
        }
        let Some(items) = schema
            .get("items")
            .filter(|i| i.is_object() || i.is_array())
        else {
            return Some(json!([]));
        };
        // A pair or a fixed tuple.
        if let Some(list) = items.as_array() {
            let values: Option<Vec<Value>> =
                list.iter().map(|i| self.value(i, name, hint)).collect();
            return values.map(Value::Array);
        }
        let singular = name.strip_suffix('s').unwrap_or(name);
        let mut out = Vec::new();
        for index in 0..min {
            let mut value = self.value(items, singular, hint)?;
            // Several objects of one kind: the next one each time.
            if index > 0
                && let Some(id) = value.as_u64()
                && let Some(next) = self.another(singular, hint, id)
            {
                value = json!(next);
            }
            out.push(value);
        }
        Some(Value::Array(out))
    }

    /// Another object of the kind `id` is, for a list that wants two.
    fn another(&self, name: &str, _hint: &str, id: u64) -> Option<u64> {
        let pools: [&[u64]; 6] = [
            &self.refs.midi_tracks,
            &self.refs.audio_tracks,
            &self.refs.arrangement.midi,
            &self.refs.arrangement.audio,
            &self.refs.scenes,
            &self.refs.locators,
        ];
        let _ = name;
        pools
            .iter()
            .find(|pool| pool.contains(&id))
            .and_then(|pool| pool.iter().copied().find(|other| *other != id))
    }

    fn boolean(&self, name: &str) -> bool {
        !matches!(
            name,
            "copy" | "dry_run" | "reset" | "revert" | "clear" | "background" | "linked" | "add"
        )
    }

    fn string(&mut self, schema: &Value, name: &str, hint: &str) -> String {
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            if pattern.contains("0-9a-fA-F") {
                return self.refs.media.first().cloned().unwrap_or_default();
            }
            if pattern.contains("[0-9]+") {
                return "2".to_owned();
            }
        }
        match name {
            "path" | "file" => return self.path(hint),
            "folder" | "project" | "library" => {
                return self.refs.dir.to_string_lossy().into_owned();
            }
            "hash" => return self.refs.media.first().cloned().unwrap_or_default(),
            "client" => return crate::host::AGENT_NAME.to_owned(),
            "query" | "words" => return "kick".to_owned(),
            "command" => return "edit.undo".to_owned(),
            "tool" => return "arpeggiate".to_owned(),
            "signature" => return "4/4".to_owned(),
            _ => {}
        }
        format!("E2E {}", self.fresh)
    }

    /// A file path: a new one in the sweep's folder for a command that
    /// writes, or a file to read of the kind the command wants.
    fn path(&mut self, hint: &str) -> String {
        let writes = self.says(hint, &["file to write", "where to write", "where to save"])
            || [
                "save",
                "export",
                "create_pack",
                "collect",
                "bounce",
                "render",
                "record",
            ]
            .iter()
            .any(|w| self.command.contains(w));
        let extension = [
            ("blpreset", "blpreset"),
            ("dawproject", "dawproject"),
            ("blgroove", "blgroove"),
            ("groove", "blgroove"),
            (".mid", "mid"),
            ("midi file", "mid"),
            ("blpack", "blpack"),
            ("pack", "blpack"),
            ("blclip", "blclip"),
            (".sfz", "sfz"),
            (".scl", "scl"),
            ("tuning", "scl"),
            ("backup", "backup"),
            ("blset", "blset"),
            ("set file", "blset"),
            ("preset", "blpreset"),
            ("plug-in list", "txt"),
        ]
        .iter()
        .find(|(word, _)| hint.contains(word) || self.about.contains(word))
        .map_or("wav", |(_, ext)| *ext);
        if writes {
            *self.fresh += 1;
            let file = format!("out-{}.{extension}", self.fresh);
            return self.refs.dir.join(file).to_string_lossy().into_owned();
        }
        if self.command.starts_with("chain.") && extension == "blpreset" {
            return self.refs.file("chain_preset");
        }
        if self.says(hint, &["folder"]) && !self.says(hint, &["file"]) {
            return self.refs.dir.to_string_lossy().into_owned();
        }
        self.refs.file(extension)
    }

    fn integer(&mut self, schema: &Value, name: &str, hint: &str) -> Value {
        let min = schema.get("minimum").and_then(Value::as_f64);
        let max = schema.get("maximum").and_then(Value::as_f64);
        let pick = match name {
            // Notes named by their place in a clip count from 0.
            "note" | "notes" | "place" if hint.contains("place") => Some(0),
            "row" | "column" => Some(1),
            "index" | "offset" | "from" | "slot" | "step" | "lane_index" => Some(0),
            "key" | "note" | "pitch" | "root_key" | "lowest" => Some(60),
            "highest" | "high" => Some(72),
            "low" => Some(48),
            "velocity" => Some(100),
            "channel" => Some(1),
            "numerator" | "denominator" => Some(4),
            "number" | "controller" => Some(1),
            "macro_number" | "macro" => Some(1),
            "to" => Some(48_000),
            "seed" => Some(7),
            "count" | "times" | "parts" | "steps" | "round_robins" | "multiplier" => Some(2),
            "port" => Some(47_123),
            "limit" => Some(10),
            "sample_rate" => Some(48_000),
            "buffer_size" => Some(256),
            "bit_depth" => Some(24),
            "percent" | "amount" => Some(50),
            "frame" => Some(0),
            _ => None,
        };
        let value = pick.map(|v| v as f64).unwrap_or_else(|| {
            if let Some((a, b)) = range_in(hint) {
                a.max(min.unwrap_or(f64::MIN))
                    .min(b.min(max.unwrap_or(f64::MAX)))
            } else {
                min.unwrap_or(0.0).max(if min.is_some_and(|m| m >= 1.0) {
                    min.unwrap_or(1.0)
                } else {
                    1.0
                })
            }
        });
        let value = value
            .max(min.unwrap_or(f64::MIN))
            .min(max.unwrap_or(f64::MAX));
        json!(value as i64)
    }

    fn number(&mut self, schema: &Value, name: &str, hint: &str) -> Value {
        let min = schema.get("minimum").and_then(Value::as_f64);
        let max = schema.get("maximum").and_then(Value::as_f64);
        let named = match name {
            "bpm" | "tempo" => Some(120.0),
            "pan" | "bend" => Some(0.0),
            "volume" | "level" | "gain" => Some(0.5),
            "seconds" | "time" | "position" => Some(0.5),
            "frequency" | "frequency_hz" | "hz" => Some(440.0),
            "factor" | "ratio" => Some(2.0),
            _ => None,
        };
        if name == "value"
            && let Some(device) = self.device
            && let Some(kind) = self.refs.device_types.get(&device)
            && let Some(params) = self.refs.params.get(kind)
            && let Some((_, lo, hi, _)) = params.first()
        {
            return json!(lo + (hi - lo) * 0.25);
        }
        let value = named.unwrap_or_else(|| match (min, max) {
            (Some(a), Some(b)) => a + (b - a) * 0.5,
            (Some(a), None) => (a + 0.5).max(a),
            (None, Some(b)) => (b - 0.5).min(b),
            (None, None) => range_in(hint).map_or(0.5, |(a, b)| a + (b - a) * 0.5),
        });
        json!(
            value
                .max(min.unwrap_or(f64::MIN))
                .min(max.unwrap_or(f64::MAX))
        )
    }

    /// A value for a reference type, by its definition's name: an object
    /// of the set, a position, or `None` to walk the definition.
    fn reference(&mut self, def: &str, name: &str, hint: &str) -> Option<Value> {
        let new = (name == "id" && (self.depth > 1 || hint.contains("new")))
            || (hint.contains("the new ") && hint.contains(" id"));
        let refs = self.refs;
        let first = |list: &[u64]| list.first().map(|id| json!(id));
        match def {
            "TrackId" if new => Some(self.new_name("track")),
            "TrackId" => {
                let id = self.track_for(name, hint);
                self.track = Some(id);
                Some(json!(id))
            }
            "ClipId" if new => Some(self.new_name("clip")),
            "ClipId" => Some(json!(self.clip_for(hint))),
            "DeviceId" if new => Some(self.new_name("device")),
            "DeviceId" => {
                let id = self.device_for(name, hint);
                self.device = Some(id);
                Some(json!(id))
            }
            "SceneId" if new => Some(self.new_name("scene")),
            "SceneId" => first(&refs.scenes),
            "LocatorId" if new => Some(self.new_name("locator")),
            "LocatorId" => first(&refs.locators),
            "ChainId" if new => Some(self.new_name("chain")),
            "ChainId" => {
                if self.says(hint, &["drum", "pad"]) {
                    first(&refs.drum_chains).or_else(|| first(&refs.chains))
                } else {
                    first(&refs.chains)
                }
            }
            "GrooveId" if new => Some(self.new_name("groove")),
            "GrooveId" => first(&refs.grooves),
            "TakeLaneId" if new => Some(self.new_name("lane")),
            "TakeLaneId" => first(&refs.take_lanes),
            "LayerId" => first(&refs.layers),
            "LinkGroupId" => first(&refs.link_groups),
            "ContentHash" => refs.media.first().map(|h| json!(h)),
            "ParamId" => Some(json!(self.param_for())),
            "Ticks" => Some(json!(self.ticks(name, hint))),
            "Bpm" => Some(json!(120.0)),
            _ => None,
        }
    }

    fn ticks(&self, name: &str, hint: &str) -> i64 {
        // The set's tempo change is at bar 5, its signature change at bar 6.
        if self.depth <= 1 && matches!(name, "at" | "from" | "position") {
            if self.says(hint, &["tempo breakpoint"]) {
                return 4 * BAR;
            }
            if self.says(hint, &["time signature", "time-signature", "fragmentary"]) {
                return 5 * BAR;
            }
            // The automation's breakpoints are at the start and at bar 2.
            if self.says(hint, &["breakpoint", "clip envelope"])
                && !self.says(hint, &["add", "insert"])
            {
                return 0;
            }
        }
        // Inside a clip, positions count from its start; a clip is two bars.
        match name {
            "start" | "from" | "loop_start" | "at_start" | "to_start" => 0,
            "end" | "loop_end" | "at_end" | "to_end" => BAR,
            "length" | "by" | "duration" | "grid" => BEAT,
            "at" | "position" | "insert" => {
                if self.says(hint, &["breakpoint", "envelope", "marker"]) {
                    BEAT
                } else {
                    BAR
                }
            }
            "to" => BAR + BEAT,
            _ => BEAT,
        }
    }

    fn track_for(&self, name: &str, hint: &str) -> u64 {
        let refs = self.refs;
        let pick = |list: &[u64]| list.first().copied();
        let text = format!("{name} {hint}");
        let found = if let Some(prefer) =
            self.prefer.filter(|_| name == "track" || name == "tracks")
        {
            pick(match prefer {
                Prefer::Audio => &refs.audio_tracks,
                Prefer::Midi => &refs.midi_tracks,
            })
        } else if name == "to" || text.contains("return track") || self.says(hint, &["return"]) {
            pick(&refs.returns)
        } else if text.contains("main") && self.about.contains("main track") {
            Some(refs.main)
        } else if text.contains("group") || self.says(hint, &["group track", "ungroup"]) {
            pick(&refs.groups)
        } else if text.contains("audio track")
            || self.says(
                hint,
                &[
                    "audio track",
                    "warp",
                    "audio clip",
                    "freeze",
                    "bounce",
                    "take lane",
                    "comp",
                ],
            )
        {
            pick(&refs.audio_tracks)
        } else {
            pick(&refs.midi_tracks)
        };
        found.unwrap_or(refs.main)
    }

    fn clip_for(&self, hint: &str) -> u64 {
        let refs = self.refs;
        let audio = self.says(
            hint,
            &[
                "audio clip",
                "warp",
                "transient",
                "sample",
                "audio",
                "gain",
                "fade",
                "slice",
                "groove",
            ],
        ) && !self.says(hint, &["midi clip", "note"]);
        let audio = match self.prefer {
            Some(Prefer::Audio) => true,
            Some(Prefer::Midi) => false,
            None => audio,
        };
        let session = self.says(
            hint,
            &["slot", "session", "scene", "launch", "follow action"],
        );
        let pool = match (audio, session) {
            (true, true) => &refs.session.audio,
            (true, false) => &refs.arrangement.audio,
            (false, true) => &refs.session.midi,
            (false, false) => &refs.arrangement.midi,
        };
        pool.first()
            .or_else(|| refs.arrangement.midi.first())
            .copied()
            .unwrap_or(1)
    }

    fn device_for(&self, name: &str, hint: &str) -> u64 {
        let refs = self.refs;
        let text = format!("{name} {hint} {}", self.about);
        let order: &[(&[&str], &str)] = &[
            (
                &["multisampler", "layer", "zone", "sfz"],
                bl_devices::multisampler::MULTISAMPLER,
            ),
            (&["drum rack", "pad", "choke"], "rack:drum"),
            (&["instrument rack"], "rack:instrument"),
            (&["sidechain"], bl_devices::compressor::COMPRESSOR),
            (
                &["rack", "macro", "chain", "variation"],
                "rack:audio_effect",
            ),
            (&["modulator"], bl_model::modulator::WAVE_MOD),
            (
                &["sample", "sampler", "snapshot"],
                bl_devices::sampler::SAMPLER,
            ),
            (&["arpeggiator"], bl_devices::arpeggiator::ARPEGGIATOR),
            (&["instrument"], bl_devices::polysynth::POLYSYNTH),
        ];
        for (words, kind) in order {
            if words.iter().any(|w| text.contains(w))
                && let Some(id) = refs.device(kind)
            {
                return id;
            }
        }
        refs.device(bl_devices::auto_filter::AUTO_FILTER)
            .or_else(|| refs.device_types.keys().next().copied())
            .unwrap_or(1)
    }

    fn param_for(&self) -> u32 {
        self.device
            .and_then(|d| self.refs.device_types.get(&d))
            .and_then(|kind| self.refs.params.get(kind))
            .and_then(|params| params.first())
            .map_or(0, |(id, ..)| *id)
    }
}
