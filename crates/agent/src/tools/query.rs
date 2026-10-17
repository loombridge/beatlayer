//! Query: read the project by ID or by a readable path, with summaries and
//! pages for large projects.
//!
//! A path is segments joined by `/`. Each segment is a kind and a
//! selector: `track "Bass"`, `clip #2`, `device 41`, `param 0`. A quoted
//! selector is a name, a number is an ID (or, for a parameter, its number),
//! `#n` is the nth in order, counting from 1, and `track main` is the Main
//! track. A clip or device after a track is looked for on that track; on
//! its own, anywhere in the song. `scene 1` and `slot 3` count scenes from
//! 1 as the Session View numbers them; `track "Bass" / slot 3` is that
//! track's slot, and `track "Bass" / automation` its envelopes, one of them
//! picked by its parameter (`automation "volume"`) or its number. The
//! keywords `song`, `tracks`, `tempo`,
//! `time signatures`, `history`, and `checkpoints` read those parts.

use bl_core::time::{Ticks, TimeSignatureMap};
use bl_model::automation::{Automation, AutomationTarget};
use bl_model::{
    Clip, ClipData, ClipId, Device, DeviceId, Document, Scene, SceneId, Track, TrackId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Ctx, ToolOutcome, args, done, failed, step, tool_definition};

pub(crate) const NAME: &str = "query";

/// How much a query returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Detail {
    /// Counts and the main facts, with long lists paged.
    #[default]
    Summary,
    /// Everything the set saves for what you read, as the set file writes it
    /// (under `saved`), such as every note of a clip, still paged.
    Full,
}

/// Arguments of `query`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryArgs {
    /// What to read, as a readable path such as `track "Bass"`,
    /// `track "Bass" / clip #1`, `device "EQ" / param 0`, `tracks`, or
    /// `tempo`. Leave it and `id` out for an overview of the whole song.
    #[serde(default)]
    path: Option<String>,
    /// What to read, by its ID: any track, clip, or device.
    #[serde(default)]
    id: Option<u64>,
    /// How much to return.
    #[serde(default)]
    detail: Detail,
    /// Where a long list starts, counting from 0.
    #[serde(default)]
    offset: usize,
    /// The most entries a list returns, from 1 to 500; 50 unless given.
    #[serde(default)]
    #[schemars(range(min = 1, max = 500))]
    limit: Option<usize>,
}

pub(crate) fn definition() -> Value {
    tool_definition::<QueryArgs>(
        NAME,
        "Query the project",
        "Reads the open project without changing it. With no arguments: an overview of the \
         song (its set, tempo, time signatures, counts, the tracks in order, and the undo \
         history). With `id`: the track, clip, or device with that ID. With `path`: a readable \
         path such as `track \"Bass\"`, `track \"Bass\" / clip #1` (the first clip by start), \
         `track main`, `device \"EQ\" / param 0`, `clip \"Verse\"`, `tracks`, `tempo`, `time \
         signatures`, `history`, or `checkpoints`. Positions are given as bar.beat.sixteenth \
         (as Live shows them, from 1.1.1) and as ticks (705,600 to the quarter note). Long \
         lists come a page at a time: pass the `next_offset` a result gives as `offset`. Also \
         `scene 1` or `scene \"Verse\"`, `track \"Bass\" / slot 3` (a Session slot, scenes \
         counted from 1), and `track \"Bass\" / automation` (its envelopes; \
         `automation \"volume\"` reads one, with its points). With `detail: \"full\"` a \
         track, clip, device, scene, and the song (`song`) also return `saved`: everything \
         the set file saves for it, in the file\'s own form, so a setting you can change is \
         readable there; a list inside a list shows only its length, which the item\'s own \
         path reads in full. The default `summary` stays compact.",
        true,
    )
}

pub(crate) fn query(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let query: QueryArgs = step!(args(raw));
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let page = Page {
        offset: query.offset,
        limit,
    };
    let document = ctx.shell.editor().document();
    let view = View {
        shell: &*ctx.shell,
        document,
        signatures: &document.settings().time_signatures,
        detail: query.detail,
        page,
    };
    if let Some(id) = query.id {
        return match find_id(document, id) {
            Some(found) => done(view.found(found)),
            None => failed(format!("nothing in the project has the ID {id}")),
        };
    }
    let Some(path) = query.path.filter(|p| !p.trim().is_empty()) else {
        return done(overview(ctx, &view));
    };
    let segments = match parse_path(&path) {
        Ok(segments) => segments,
        Err(message) => return failed(format!("can't read the path `{path}`: {message}")),
    };
    match segments.as_slice() {
        [Segment::Keyword(keyword)] => match keyword.as_str() {
            "song" => done(overview(ctx, &view)),
            "tracks" => done(view.track_list()),
            "tempo" => done(json!({ "tempo": document.settings().tempo })),
            "time signatures" => done(json!({
                "time_signatures": document.settings().time_signatures,
            })),
            "history" => {
                let editor = ctx.shell.editor();
                done(json!({
                    "undo": page.slice(editor.undo_history().map(str::to_owned).collect()),
                    "redo": page.slice(editor.redo_history().map(str::to_owned).collect()),
                }))
            }
            "checkpoints" => done(ctx.state.checkpoints.list()),
            _ => failed(format!("`{keyword}` isn't something a path can read")),
        },
        [Segment::Keyword(keyword), ..] => failed(format!("`{keyword}` must be the whole path")),
        [before @ .., Segment::Pick(Kind::Param, selector)] => param(&view, before, selector),
        _ => match resolve(document, &segments) {
            Ok(found) => done(view.found(found)),
            Err(message) => failed(message),
        },
    }
}

/// A page of a long list.
#[derive(Clone, Copy, Debug)]
struct Page {
    offset: usize,
    limit: usize,
}

impl Page {
    /// One page of `items`, with where the next one starts.
    fn slice(self, items: Vec<impl serde::Serialize>) -> Value {
        let total = items.len();
        let end = self.offset.saturating_add(self.limit).min(total);
        let shown: Vec<_> = items
            .into_iter()
            .skip(self.offset)
            .take(end.saturating_sub(self.offset))
            .collect();
        json!({
            "total": total,
            "offset": self.offset,
            "items": shown,
            "next_offset": (end < total).then_some(end),
        })
    }
}

/// What a query reads, by kind.
enum Found<'a> {
    Track(&'a Track),
    Clip(&'a Clip),
    Device(&'a Device),
    Scene(&'a Scene),
    /// A track's Session slot at a scene.
    Slot(&'a Track, &'a Scene),
    /// A track's envelopes, or one of them.
    Automation(&'a Track, Option<(&'a AutomationTarget, &'a Automation)>),
}

fn find_id(document: &Document, raw: u64) -> Option<Found<'_>> {
    let id = bl_core::id::Id::new(raw)?;
    if let Some(track) = document.track(TrackId(id)) {
        Some(Found::Track(track))
    } else if let Some(clip) = document.clip(ClipId(id)) {
        Some(Found::Clip(clip))
    } else if let Some(device) = document.device(DeviceId(id)) {
        Some(Found::Device(device))
    } else {
        document.scene(SceneId(id)).map(Found::Scene)
    }
}

/// Builds the JSON a query returns.
struct View<'a> {
    shell: &'a bl_shell::Shell,
    document: &'a Document,
    signatures: &'a TimeSignatureMap,
    detail: Detail,
    page: Page,
}

impl View<'_> {
    fn position(&self, ticks: Ticks) -> Value {
        json!({ "bbs": self.signatures.to_bbs(ticks).to_string(), "ticks": ticks.get() })
    }

    fn length(ticks: Ticks) -> Value {
        json!({ "quarters": ticks.as_quarters(), "ticks": ticks.get() })
    }

    fn track_summary(&self, track: &Track) -> Value {
        let mixer = track.mixer();
        json!({
            "id": track.id(),
            "kind": track.kind(),
            "name": track.name(),
            "group": self.document.parent(track.id()),
            "mixer": {
                "volume": mixer.volume,
                "volume_db": decibels(mixer.volume),
                "pan": mixer.pan,
                "mute": mixer.mute,
                "solo": mixer.solo,
                "arm": mixer.arm,
            },
            "clips": track.clips().len(),
            "devices": track.devices().len(),
            "tracks_inside": track.children().len(),
        })
    }

    fn found(&self, found: Found<'_>) -> Value {
        match found {
            Found::Track(track) => self.track(track),
            Found::Clip(clip) => self.clip(clip),
            Found::Device(device) => self.device(device),
            Found::Scene(scene) => self.scene(scene),
            Found::Slot(track, scene) => self.slot(track, scene),
            Found::Automation(track, envelope) => self.automation(track, envelope),
        }
    }

    /// The saved form of `object`, as the set file writes it, with its lists
    /// bounded by the page.
    fn saved(&self, object: &impl Serialize) -> Value {
        let mut value = serde_json::to_value(object).unwrap_or_default();
        bound(&mut value, self.page, false);
        value
    }

    fn track(&self, track: &Track) -> Value {
        let mut value = self.track_summary(track);
        if self.detail == Detail::Full {
            value["saved"] = self.saved(&self.document.saved_track(track.id()));
        }
        let mut clips: Vec<&Clip> = track.clips().collect();
        clips.sort_by_key(|clip| (clip.start, clip.id));
        let clips: Vec<Value> = clips.iter().map(|clip| self.clip_summary(clip)).collect();
        let devices: Vec<Value> = track.devices().map(|d| self.device_summary(d)).collect();
        let children: Vec<Value> = track
            .children()
            .filter_map(|id| self.document.track(id))
            .map(|child| json!({ "id": child.id(), "name": child.name(), "kind": child.kind() }))
            .collect();
        value["clips"] = self.page.slice(clips);
        value["devices"] = json!(devices);
        value["tracks_inside"] = json!(children);
        value
    }

    fn clip_summary(&self, clip: &Clip) -> Value {
        let (kind, content) = match &clip.data {
            ClipData::Audio(audio) => ("audio", json!({ "media": audio.media })),
            ClipData::Midi(midi) => ("midi", json!({ "notes": midi.notes.len() })),
        };
        json!({
            "id": clip.id,
            "name": clip.name,
            "kind": kind,
            "track": self.document.clip_track(clip.id),
            "start": self.position(clip.start),
            "end": self.position(clip.end()),
            "length": Self::length(clip.length),
            "mute": clip.mute,
            "content": content,
        })
    }

    fn clip(&self, clip: &Clip) -> Value {
        let mut value = self.clip_summary(clip);
        if let ClipData::Midi(midi) = &clip.data
            && self.detail == Detail::Full
        {
            let mut notes: Vec<_> = midi.notes.iter().copied().collect();
            notes.sort_by_key(|note| (note.start, note.key));
            value["content"]["notes"] = self.page.slice(notes);
        }
        let slot = self
            .document
            .clip_track(clip.id)
            .and_then(|id| self.document.track(id))
            .and_then(|track| track.slots().find(|(_, held)| held.id == clip.id));
        if let Some((scene, _)) = slot {
            value["scene"] = json!(scene);
        }
        if self.detail == Detail::Full {
            value["saved"] = self.saved(clip);
        }
        value
    }

    fn device_summary(&self, device: &Device) -> Value {
        json!({
            "id": device.id,
            "name": device.name,
            "kind": device.kind,
            "enabled": device.enabled,
            "params_set": device.params.len(),
        })
    }

    fn device(&self, device: &Device) -> Value {
        let mut value = self.device_summary(device);
        value["track"] = json!(self.document.device_track(device.id));
        let params: Vec<Value> = device
            .params
            .iter()
            .map(|(param, value)| json!({ "param": param, "value": value }))
            .collect();
        value["params"] = self.page.slice(params);
        if self.detail == Detail::Full {
            value["saved"] = self.saved(device);
        }
        // A plug-in's own commands, scoped to this device, which `run` and
        // `schedule` take like any other.
        let commands = self.shell.device_command_ids(device.id);
        if !commands.is_empty() {
            value["commands"] = json!(commands);
        }
        value
    }

    fn scene_summary(&self, scene: &Scene) -> Value {
        let number = self.document.scene_position(scene.id).map(|at| at + 1);
        json!({ "id": scene.id, "name": scene.name, "number": number })
    }

    fn scene(&self, scene: &Scene) -> Value {
        let mut value = self.scene_summary(scene);
        let slots: Vec<Value> = self
            .document
            .tracks()
            .filter_map(|track| {
                let clip = track.slot(scene.id)?;
                Some(json!({ "track": track.id(), "clip": self.clip_summary(clip) }))
            })
            .collect();
        value["slots"] = self.page.slice(slots);
        if self.detail == Detail::Full {
            value["saved"] = self.saved(scene);
        }
        value
    }

    fn slot(&self, track: &Track, scene: &Scene) -> Value {
        let clip = track.slot(scene.id).map(|clip| self.clip(clip));
        json!({
            "track": track.id(),
            "scene": self.scene_summary(scene),
            "stop_button": !track.removed_stop_buttons().any(|id| id == scene.id),
            "clip": clip,
        })
    }

    fn automation(
        &self,
        track: &Track,
        envelope: Option<(&AutomationTarget, &Automation)>,
    ) -> Value {
        let Some((target, automation)) = envelope else {
            let envelopes: Vec<Value> = track
                .automation()
                .map(|(target, automation)| envelope_summary(target, automation))
                .collect();
            return json!({ "track": track.id(), "automation": self.page.slice(envelopes) });
        };
        let mut value = envelope_summary(target, automation);
        value["track"] = json!(track.id());
        if self.detail == Detail::Full {
            let points: Vec<_> = automation.envelope.points().collect();
            value["points"] = self.page.slice(points);
        }
        value
    }

    fn track_list(&self) -> Value {
        let tracks: Vec<Value> = self
            .document
            .tracks()
            .map(|track| self.track_summary(track))
            .collect();
        json!({ "tracks": self.page.slice(tracks) })
    }
}

/// A linear gain in decibels, or `null` for silence.
fn decibels(gain: f64) -> Option<f64> {
    (gain > 0.0).then(|| (20.0 * gain.log10() * 100.0).round() / 100.0)
}

fn overview(ctx: &Ctx<'_>, view: &View<'_>) -> Value {
    let shell = &*ctx.shell;
    let editor = shell.editor();
    let document = view.document;
    let settings = document.settings();
    let start_tempo = settings.tempo.bpm_at(Ticks::ZERO).get();
    let mut value = json!({
        "set": {
            "name": shell.set_name(),
            "path": shell.set_path().map(|p| p.to_string_lossy().into_owned()),
            "unsaved_changes": shell.is_dirty(),
        },
        "tempo": {
            "bpm_at_start": start_tempo,
            "segments": settings.tempo.segments().len(),
        },
        "time_signature_at_start": settings.time_signatures.signature_at(Ticks::ZERO).to_string(),
        "counts": {
            "tracks": document.track_count(),
            "clips": document.clip_count(),
            "devices": document.device_count(),
        },
        "undo": {
            "next_undo": editor.undo_history().next(),
            "next_redo": editor.redo_history().next(),
            "steps": editor.undo_history().len(),
        },
        "scheduled": ctx.state.schedule.waiting(),
        "checkpoints": ctx.state.checkpoints.len(),
    });
    value["tracks"] = view.track_list()["tracks"].take();
    if view.detail == Detail::Full {
        // The set file's own fields for the song: the settings (tempo map,
        // time signatures, loop, punch, metronome and record switches), the
        // launch quantisation saved beside them, and the scenes.
        let scenes: Vec<&Scene> = document.scenes().collect();
        value["saved"] = view.saved(&json!({
            "settings": settings,
            "launch": shell.launch_settings(),
            "scenes": scenes,
        }));
    }
    value
}

/// One envelope of a track, without its points.
fn envelope_summary(target: &AutomationTarget, automation: &Automation) -> Value {
    json!({
        "parameter": target_label(target),
        "target": target,
        "enabled": automation.enabled,
        "points": automation.envelope.points().len(),
    })
}

/// The name an automation path picks an envelope by: `volume`, `send 12`,
/// `device 41 param 3`.
fn target_label(target: &AutomationTarget) -> String {
    match target {
        AutomationTarget::Mixer { param } => serde_json::to_value(param)
            .ok()
            .and_then(|name| name.as_str().map(str::to_owned))
            .unwrap_or_default(),
        AutomationTarget::Send { to } => format!("send {}", to.0.get()),
        AutomationTarget::Device { device, param } => {
            format!("device {} param {}", device.0.get(), param.0)
        }
    }
}

/// Bounds a saved form for a reply. The first list on each branch is paged
/// like any other; a list of objects or lists inside a paged item shrinks to
/// its length, which that item's own path reads in full.
fn bound(value: &mut Value, page: Page, nested: bool) {
    match value {
        Value::Object(map) => map.values_mut().for_each(|v| bound(v, page, nested)),
        Value::Array(items) if nested => {
            if items.iter().any(|item| item.is_object() || item.is_array()) {
                *value = json!({ "total": items.len() });
            }
        }
        Value::Array(items) => {
            let mut shown = page.slice(std::mem::take(items));
            for item in shown["items"].as_array_mut().into_iter().flatten() {
                bound(item, page, true);
            }
            *value = shown;
        }
        _ => {}
    }
}

/// One part of a path.
#[derive(Clone, Debug, PartialEq)]
enum Segment {
    /// A whole-path keyword, such as `tempo`.
    Keyword(String),
    /// A kind and how to pick one.
    Pick(Kind, Selector),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Track,
    Clip,
    Device,
    Param,
    Scene,
    Slot,
    Automation,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Clip => "clip",
            Self::Device => "device",
            Self::Param => "param",
            Self::Scene => "scene",
            Self::Slot => "slot",
            Self::Automation => "automation",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Selector {
    Name(String),
    Id(u64),
    Position(usize),
    Main,
    /// Everything of the kind: `automation` on its own.
    All,
}

const KEYWORDS: &[&str] = &[
    "song",
    "tracks",
    "tempo",
    "time signatures",
    "history",
    "checkpoints",
];

fn parse_path(path: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut rest = path.trim();
    while !rest.is_empty() {
        // ASCII only: the keywords are ASCII, and a Unicode lowercase copy
        // can differ in length (the Kelvin sign becomes `k`), which would
        // put the slices below inside a character.
        let lower = rest.to_ascii_lowercase();
        let ends_segment = |after: &str| {
            let after = after.trim_start();
            after.is_empty() || after.starts_with('/')
        };
        if let Some(keyword) = KEYWORDS
            .iter()
            .find(|k| lower.starts_with(**k) && ends_segment(&lower[k.len()..]))
        {
            segments.push(Segment::Keyword((*keyword).to_owned()));
            rest = rest[keyword.len()..].trim_start();
        } else {
            let word_end = rest
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(rest.len());
            let kind = match rest[..word_end].to_lowercase().as_str() {
                "track" => Kind::Track,
                "clip" => Kind::Clip,
                "device" => Kind::Device,
                "param" | "parameter" => Kind::Param,
                "scene" => Kind::Scene,
                "slot" => Kind::Slot,
                "automation" => Kind::Automation,
                other => {
                    return Err(format!(
                        "`{other}` isn't a kind; use track, clip, device, param, scene, slot, \
                         or automation, or one of the keywords {}",
                        KEYWORDS.join(", ")
                    ));
                }
            };
            rest = rest[word_end..].trim_start();
            let (selector, after) =
                if kind == Kind::Automation && (rest.is_empty() || rest.starts_with('/')) {
                    (Selector::All, rest)
                } else {
                    parse_selector(rest, kind)?
                };
            segments.push(Segment::Pick(kind, selector));
            rest = after.trim_start();
        }
        if let Some(after) = rest.strip_prefix('/') {
            rest = after.trim_start();
            if rest.is_empty() {
                return Err("it ends with `/`".to_owned());
            }
        } else if !rest.is_empty() {
            return Err(format!("expected `/` before `{rest}`"));
        }
    }
    Ok(segments)
}

fn parse_selector(text: &str, kind: Kind) -> Result<(Selector, &str), String> {
    let quotes = [('"', '"'), ('\u{201c}', '\u{201d}'), ('\'', '\'')];
    if let Some((open, close)) = quotes.iter().find(|(open, _)| text.starts_with(*open)) {
        let body = &text[open.len_utf8()..];
        let mut name = String::new();
        let mut chars = body.char_indices();
        while let Some((at, c)) = chars.next() {
            if c == '\\' {
                if let Some((_, escaped)) = chars.next() {
                    name.push(escaped);
                }
            } else if c == *close {
                return Ok((Selector::Name(name), &body[at + c.len_utf8()..]));
            } else {
                name.push(c);
            }
        }
        return Err(format!(
            "the name after `{}` has no closing quote",
            kind.name()
        ));
    }
    let end = text
        .find(|c: char| c.is_whitespace() || c == '/')
        .unwrap_or(text.len());
    let word = &text[..end];
    let selector = if let Some(n) = word.strip_prefix('#') {
        match n.parse::<usize>() {
            Ok(n) if n >= 1 => Selector::Position(n),
            _ => return Err(format!("`{word}`: positions count from #1")),
        }
    } else if word.eq_ignore_ascii_case("main") && kind == Kind::Track {
        Selector::Main
    } else if let Ok(number) = word.parse::<u64>() {
        // Scenes are counted as the Session View numbers them, from 1.
        match kind {
            Kind::Scene | Kind::Slot => match usize::try_from(number) {
                Ok(n) if n >= 1 => Selector::Position(n),
                _ => return Err(format!("`{word}`: scenes count from 1")),
            },
            _ => Selector::Id(number),
        }
    } else if word.is_empty() {
        return Err(format!(
            "`{}` needs a name in quotes, an ID, or #n",
            kind.name()
        ));
    } else {
        return Err(format!(
            "`{word}`: put names in double quotes, such as {} \"{word}\"",
            kind.name()
        ));
    };
    Ok((selector, &text[end..]))
}

/// Picks the one item `selector` names among `items`, each with its ID and
/// name, for messages.
fn pick<'a, T>(
    kind: Kind,
    selector: &Selector,
    items: Vec<&'a T>,
    id_of: impl Fn(&T) -> u64,
    name_of: impl Fn(&T) -> String,
) -> Result<&'a T, String> {
    let found: Vec<&'a T> = match selector {
        Selector::Id(id) => items.into_iter().filter(|t| id_of(t) == *id).collect(),
        Selector::Position(n) => items.into_iter().skip(n - 1).take(1).collect(),
        Selector::Name(name) => items.into_iter().filter(|t| name_of(t) == *name).collect(),
        Selector::Main | Selector::All => Vec::new(),
    };
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no {} matches {}", kind.name(), describe(selector))),
        many => Err(format!(
            "{} {}s match {}; pick one by ID: {}",
            many.len(),
            kind.name(),
            describe(selector),
            many.iter()
                .map(|t| id_of(t).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn describe(selector: &Selector) -> String {
    match selector {
        Selector::Name(name) => format!("the name \u{201c}{name}\u{201d}"),
        Selector::Id(id) => format!("the ID {id}"),
        Selector::Position(n) => format!("#{n}"),
        Selector::Main => "main".to_owned(),
        Selector::All => "all".to_owned(),
    }
}

fn resolve<'a>(document: &'a Document, segments: &[Segment]) -> Result<Found<'a>, String> {
    let mut track: Option<&Track> = None;
    let mut found: Option<Found<'a>> = None;
    for segment in segments {
        let Segment::Pick(kind, selector) = segment else {
            return Err("keywords must be the whole path".to_owned());
        };
        found = Some(match kind {
            Kind::Track => {
                if found.is_some() {
                    return Err("a track must come first in a path".to_owned());
                }
                let picked = if *selector == Selector::Main {
                    document.main()
                } else {
                    let tracks: Vec<&Track> = document.tracks().collect();
                    pick(
                        *kind,
                        selector,
                        tracks,
                        |t| t.id().0.get(),
                        |t| t.name().to_owned(),
                    )?
                };
                track = Some(picked);
                Found::Track(picked)
            }
            Kind::Clip => {
                let mut clips: Vec<&Clip> = match track {
                    Some(track) if matches!(found, Some(Found::Track(_))) => {
                        track.clips().collect()
                    }
                    Some(_) => return Err("a clip can follow only a track".to_owned()),
                    None => document
                        .clip_ids()
                        .filter_map(|id| document.clip(id))
                        .collect(),
                };
                clips.sort_by_key(|clip| (clip.start, clip.id));
                Found::Clip(pick(
                    *kind,
                    selector,
                    clips,
                    |c| c.id.0.get(),
                    |c| c.name.clone(),
                )?)
            }
            Kind::Device => {
                let devices: Vec<&Device> = match track {
                    Some(track) if matches!(found, Some(Found::Track(_))) => {
                        track.devices().collect()
                    }
                    Some(_) => return Err("a device can follow only a track".to_owned()),
                    None => document.tracks().flat_map(Track::devices).collect(),
                };
                Found::Device(pick(
                    *kind,
                    selector,
                    devices,
                    |d| d.id.0.get(),
                    |d| d.name.clone(),
                )?)
            }
            Kind::Scene => {
                if found.is_some() {
                    return Err("a scene must be the whole path".to_owned());
                }
                let scenes: Vec<&Scene> = document.scenes().collect();
                Found::Scene(pick(
                    *kind,
                    selector,
                    scenes,
                    |s| s.id.0.get(),
                    |s| s.name.clone(),
                )?)
            }
            Kind::Slot => {
                let Some(track) = track.filter(|_| matches!(found, Some(Found::Track(_)))) else {
                    return Err("a slot can follow only a track".to_owned());
                };
                if !track.kind().holds_clips() {
                    return Err(format!("a {} track has no Session slots", track.kind()));
                }
                let scenes: Vec<&Scene> = document.scenes().collect();
                let scene = pick(
                    *kind,
                    selector,
                    scenes,
                    |s| s.id.0.get(),
                    |s| s.name.clone(),
                )?;
                Found::Slot(track, scene)
            }
            Kind::Automation => {
                let Some(track) = track.filter(|_| matches!(found, Some(Found::Track(_)))) else {
                    return Err("automation can follow only a track".to_owned());
                };
                let envelope = match selector {
                    Selector::All => None,
                    Selector::Name(name) => Some(
                        track
                            .automation()
                            .find(|(target, _)| target_label(target) == *name)
                            .ok_or_else(|| format!("no envelope drives {}", describe(selector)))?,
                    ),
                    Selector::Position(n) => Some(
                        track
                            .automation()
                            .nth(n - 1)
                            .ok_or_else(|| format!("no envelope matches {}", describe(selector)))?,
                    ),
                    _ => {
                        return Err(
                            "pick an envelope by its parameter, such as automation \"volume\", \
                             or by #n"
                                .to_owned(),
                        );
                    }
                };
                Found::Automation(track, envelope)
            }
            Kind::Param => return Err("a param must come last in a path".to_owned()),
        });
    }
    found.ok_or_else(|| "the path is empty".to_owned())
}

/// A device parameter's value: the device `before` names, then the
/// parameter `selector` picks.
fn param(view: &View<'_>, before: &[Segment], selector: &Selector) -> ToolOutcome {
    let device = match resolve(view.document, before) {
        Ok(Found::Device(device)) => device,
        Ok(_) => return failed("a param must follow a device"),
        Err(message) => return failed(message),
    };
    let param = match selector {
        Selector::Id(n) => match u32::try_from(*n) {
            Ok(n) => bl_model::ParamId(n),
            Err(_) => return failed(format!("parameter {n} is out of range")),
        },
        Selector::Name(name) => {
            return failed(format!(
                "parameters can't be named yet (the names come with device descriptions, \
                 WP-2.5); give \u{201c}{name}\u{201d}'s number instead, such as param 0"
            ));
        }
        _ => return failed("give a parameter's number, such as param 0"),
    };
    let value = device.params.get(&param).copied();
    done(json!({
        "device": device.id,
        "param": param,
        "value": value,
        "set": value.is_some(),
        "note": if value.is_some() {
            "The value the person or an agent set, in the parameter's own units."
        } else {
            "Not set, so the parameter has its default."
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_parse_into_kinds_and_selectors() {
        assert_eq!(
            parse_path("track \"Bass\" / clip #2").unwrap(),
            vec![
                Segment::Pick(Kind::Track, Selector::Name("Bass".to_owned())),
                Segment::Pick(Kind::Clip, Selector::Position(2)),
            ]
        );
        assert_eq!(
            parse_path("Track \u{201c}Lead \\\"Vox\\\"\u{201d}/device 41/param 0").unwrap(),
            vec![
                Segment::Pick(Kind::Track, Selector::Name("Lead \"Vox\"".to_owned())),
                Segment::Pick(Kind::Device, Selector::Id(41)),
                Segment::Pick(Kind::Param, Selector::Id(0)),
            ]
        );
        assert_eq!(
            parse_path("track main").unwrap(),
            vec![Segment::Pick(Kind::Track, Selector::Main)]
        );
        assert_eq!(
            parse_path(" time signatures ").unwrap(),
            vec![Segment::Keyword("time signatures".to_owned())]
        );
        for bad in [
            "track Bass",
            "bus \"A\"",
            "track \"A",
            "track #0",
            "track \"A\" clip #1",
            "track \"A\" /",
        ] {
            assert!(parse_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn keywords_match_ascii_only_so_other_letters_never_split_a_character() {
        // The Kelvin sign lowercases to an ASCII `k` but takes three bytes,
        // so matching keywords on a Unicode-lowercased copy would slice the
        // path inside it.
        for odd in ["trac\u{212a}s", "chec\u{212a}points / track #1"] {
            assert!(parse_path(odd).is_err(), "{odd}");
        }
        assert_eq!(
            parse_path("TRACKS").unwrap(),
            vec![Segment::Keyword("tracks".to_owned())]
        );
    }

    #[test]
    fn pages_say_where_the_next_starts() {
        let page = Page {
            offset: 2,
            limit: 2,
        };
        assert_eq!(
            page.slice(vec![1, 2, 3, 4, 5]),
            json!({ "total": 5, "offset": 2, "items": [3, 4], "next_offset": 4 })
        );
        let last = Page {
            offset: 4,
            limit: 2,
        };
        assert_eq!(last.slice(vec![1, 2, 3, 4, 5])["next_offset"], Value::Null);
    }

    #[test]
    fn levels_read_in_decibels() {
        assert_eq!(decibels(1.0), Some(0.0));
        assert_eq!(decibels(0.5), Some(-6.02));
        assert_eq!(decibels(0.0), None);
    }
}
