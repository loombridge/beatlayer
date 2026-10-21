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
    Chain, ChainId, Clip, ClipData, ClipId, Device, DeviceId, Document, SavedSong, Scene, SceneId,
    TakeLane, Track, TrackId,
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
    /// What to read, by its ID: any track, clip, device, or rack chain.
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
         history). With `id`: the track, clip, device, or rack chain with that ID (a rack's `chains`, each with its mixer and devices, read with the rack). With `path`: a readable \
         path such as `track \"Bass\"`, `track \"Bass\" / clip #1` (the first clip by start), \
         `track main`, `device \"EQ\" / param 0`, `clip \"Verse\"`, `tracks`, `tempo`, `time \
         signatures`, `history`, or `checkpoints`. Positions are given as bar.beat.sixteenth \
         (as Live shows them, from 1.1.1) and as ticks (705,600 to the quarter note). Long \
         lists come a page at a time: pass the `next_offset` a result gives as `offset`. The \
         `history` path lists the set's steps from the top, each with its label, whether it is \
         undone, who made it, and its index counted up from the set's start, and says which row \
         is current. Also \
         `scene 1` or `scene \"Verse\"`, `track \"Bass\" / slot 3` (a Session slot, scenes \
         counted from 1), `track \"Bass\" / lane 1` (a take lane, with its \
         passes), and `track \"Bass\" / automation` (its envelopes; \
         `automation \"volume\"` reads one, with its points). With `detail: \"full\"` a \
         track, clip, device, chain, scene, and the song (`song`) also return `saved`: everything \
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
                let mut rows: Vec<Value> = editor
                    .history()
                    .iter()
                    .map(|step| {
                        json!({
                            "label": step.label.as_ref(),
                            "undone": step.undone,
                            "made_by": maker(&step.maker),
                            "index": step.index,
                        })
                    })
                    .collect();
                // The set's start is the bottom row, which no step undoes.
                rows.push(json!({
                    "label": ctx.shell.set_start_label(),
                    "undone": false,
                    "made_by": Value::Null,
                    "index": 0,
                }));
                done(json!({
                    "undo": page.slice(editor.undo_history().map(str::to_owned).collect()),
                    "redo": page.slice(editor.redo_history().map(str::to_owned).collect()),
                    "rows": page.slice(rows),
                    "current": editor.current_index(),
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
    /// A rack's chain (WP-5.1).
    Chain(&'a Chain),
    Scene(&'a Scene),
    /// A track's Session slot at a scene.
    Slot(&'a Track, &'a Scene),
    /// One of a track's take lanes, with its place among them.
    Lane(&'a Track, &'a TakeLane, usize),
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
    } else if let Some(chain) = document.chain(ChainId(id)) {
        Some(Found::Chain(chain))
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
                "pan_mode": mixer.pan_mode,
                "split_left": mixer.split_left,
                "split_right": mixer.split_right,
                "mute": mixer.mute,
                "solo": mixer.solo,
                "arm": mixer.arm,
            },
            "delay": track.delay(),
            "keep_monitoring_latency": track.keep_monitoring_latency(),
            "clips": track.clips().len(),
            "devices": track.devices().len(),
            "frozen": track.frozen(),
            "tracks_inside": track.children().len(),
        })
    }

    fn found(&self, found: Found<'_>) -> Value {
        match found {
            Found::Track(track) => self.track(track),
            Found::Clip(clip) => self.clip(clip),
            Found::Device(device) => self.device(device),
            Found::Chain(chain) => self.chain(chain),
            Found::Scene(scene) => self.scene(scene),
            Found::Slot(track, scene) => self.slot(track, scene),
            Found::Lane(track, lane, number) => self.lane(track, lane, number),
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
        // Take lanes show only on a track that has some, so a track without
        // them reads as it did.
        if track.take_lanes().len() > 0 {
            let lanes: Vec<Value> = track
                .take_lanes()
                .map(|lane| {
                    let clips: Vec<Value> = lane
                        .clips
                        .iter()
                        .map(|clip| {
                            json!({
                                "name": clip.name,
                                "start": self.position(clip.start),
                                "end": self.position(clip.start.saturating_add(clip.length)),
                                "length": Self::length(clip.length),
                                "media": clip.data.media,
                            })
                        })
                        .collect();
                    json!({ "name": lane.name, "clips": clips })
                })
                .collect();
            value["take_lanes"] = json!(lanes);
        }
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
            let mut notes: Vec<_> = midi.notes.iter().cloned().collect();
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
        let mut value = json!({
            "id": device.id,
            "name": device.name,
            "kind": device.kind,
            "role": device.role,
            "enabled": device.enabled,
            "params_set": device.params.len(),
        });
        // A rack's chains, each with its mixer and its devices, racks inside
        // included (the racks spec, R17).
        if device.rack_kind().is_some() {
            let chains: Vec<Value> = device
                .chains
                .iter()
                .filter(|c| !c.is_return)
                .map(|c| self.chain_summary(c))
                .collect();
            value["chains"] = json!(chains);
            // A drum rack's return chains (drum racks spec, R27).
            if device.rack_kind() == Some(bl_model::RackKind::Drum) {
                let returns: Vec<Value> = device
                    .chains
                    .iter()
                    .filter(|c| c.is_return)
                    .map(|c| self.chain_summary(c))
                    .collect();
                value["returns"] = json!(returns);
            }
            // Spec: macros R19. The rack's macros and mappings, as
            // `rack.macros` reports them.
            let count = self.shell.devices().views_of(device.id).macro_count;
            if let Some(macros) = self
                .shell
                .devices()
                .macros_of(self.document, device.id, count)
            {
                value["macros"] = json!(macros);
            }
        }
        value
    }

    fn chain_summary(&self, chain: &Chain) -> Value {
        let zone = |z: bl_model::Zone| {
            json!({
                "low": z.low,
                "high": z.high,
                "fade_low": z.fade_low,
                "fade_high": z.fade_high,
            })
        };
        let mixer = &chain.mixer;
        let mut value = json!({
            "id": chain.id,
            "name": chain.name,
            "colour": chain.colour,
            "info": chain.info,
            "mixer": {
                "volume": mixer.volume,
                "volume_db": decibels(mixer.volume),
                "pan": mixer.pan,
                "enabled": mixer.enabled,
                "solo": mixer.solo,
            },
            "zones": {
                "key": zone(chain.zones.key),
                "velocity": zone(chain.zones.velocity),
                "select": zone(chain.zones.select),
            },
            "devices": chain.devices.iter().map(|d| self.device_summary(d)).collect::<Vec<_>>(),
        });
        // What only a drum rack's chains have (drum racks spec, R27); a
        // chain of any other rack holds the defaults.
        if self
            .document
            .chain_rack(chain.id)
            .and_then(Device::rack_kind)
            == Some(bl_model::RackKind::Drum)
        {
            if chain.is_return {
                value["audio_to"] = json!(chain.audio_to);
            } else {
                value["receive"] = json!(chain.receive);
                value["play"] = json!(chain.play);
                value["choke"] = json!(chain.choke);
                value["sends"] = json!(chain.sends);
            }
        }
        value
    }

    fn chain(&self, chain: &Chain) -> Value {
        let mut value = self.chain_summary(chain);
        value["rack"] = json!(self.document.chain_rack(chain.id).map(|rack| rack.id));
        value["track"] = json!(self.document.chain_track(chain.id));
        if self.detail == Detail::Full {
            value["saved"] = self.saved(chain);
        }
        value
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
        // A multisampler's layers and the options of its layer list
        // (WP-5.13, R30).
        if !device.layers.is_default() {
            let layers: Vec<Value> = device.layers.list.iter().map(|l| json!(l)).collect();
            value["layers"] = json!({
                "count": layers.len(),
                "ram_mode": device.layers.ram_mode,
                "snap": device.layers.snap,
                "linear_loop_fades": device.layers.linear_loop_fades,
                "list": self.page.slice(layers),
            });
        }
        // Spec: modulation R18. A modulator's mappings, which
        // `modulator.map` and `modulator.unmap` change, and a Shaper's
        // shape.
        if bl_model::modulator::is_modulator_kind(&device.kind) {
            value["mappings"] = json!(device.mappings);
            if let Some(shape) = &device.shape {
                value["shape"] = json!(shape);
            }
        }
        if device.can_compare() {
            // Spec: device-compare R11.
            let pairs = |params: &bl_model::OrdMap<bl_model::ParamId, f64>| {
                params
                    .iter()
                    .map(|(param, value)| json!({ "param": param, "value": value }))
                    .collect::<Vec<_>>()
            };
            let (a, b) = if device.b_active {
                (device.other_params(), &device.params)
            } else {
                (&device.params, device.other_params())
            };
            value["compare"] = json!({
                "active": if device.b_active { "B" } else { "A" },
                "a": pairs(a),
                "b": pairs(b),
            });
        }
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

    fn lane(&self, track: &Track, lane: &TakeLane, number: usize) -> Value {
        let passes: Vec<Value> = lane.clips.iter().map(|pass| self.saved(pass)).collect();
        json!({
            "track": track.id(),
            "name": lane.name,
            "number": number,
            "passes": self.page.slice(passes),
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
            "redo_steps": editor.redo_history().len(),
            "current": editor.current_index(),
        },
        "scheduled": ctx.state.schedule.waiting(),
        "checkpoints": ctx.state.checkpoints.len(),
    });
    value["tracks"] = view.track_list()["tracks"].take();
    if view.detail == Detail::Full {
        // The set file's own fields for the song: the settings (tempo map,
        // time signatures, loop, punch, metronome and record switches), the
        // launch quantisation saved beside them, and the scenes.
        value["saved"] = view.saved(&SavedSong {
            settings,
            launch: shell.launch_settings(),
            scenes: document.scenes().collect(),
        });
    }
    value
}

/// Who made a step, for the history's rows: the person, an agent by name,
/// or the built-in assistant (undo history R9).
fn maker(maker: &bl_model::Maker) -> Value {
    match maker {
        bl_model::Maker::Person => json!({ "kind": "person" }),
        bl_model::Maker::Agent(name) => json!({ "kind": "agent", "name": name }),
        bl_model::Maker::Assistant => json!({ "kind": "assistant" }),
    }
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
        AutomationTarget::Chain { chain, param } => format!(
            "chain {} {}",
            chain.0.get(),
            serde_json::to_value(param)
                .ok()
                .and_then(|name| name.as_str().map(str::to_owned))
                .unwrap_or_default()
        ),
        AutomationTarget::ChainSend { chain, to } => {
            format!("chain {} send {}", chain.0.get(), to.0.get())
        }
        AutomationTarget::MidiCc { number } => format!("midi cc {number}"),
        AutomationTarget::MidiPitchBend => "midi pitch bend".to_owned(),
        AutomationTarget::MidiChannelPressure => "midi channel pressure".to_owned(),
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
    Lane,
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
            Self::Lane => "lane",
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
                "lane" => Kind::Lane,
                "automation" => Kind::Automation,
                other => {
                    return Err(format!(
                        "`{other}` isn't a kind; use track, clip, device, param, scene, slot, \
                         lane, or automation, or one of the keywords {}",
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
            Kind::Scene | Kind::Slot | Kind::Lane => match usize::try_from(number) {
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
                        track.devices_deep().collect()
                    }
                    Some(_) => return Err("a device can follow only a track".to_owned()),
                    None => document.tracks().flat_map(Track::devices_deep).collect(),
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
            Kind::Lane => {
                let Some(track) = track.filter(|_| matches!(found, Some(Found::Track(_)))) else {
                    return Err("a lane can follow only a track".to_owned());
                };
                let lanes: Vec<&TakeLane> = track.take_lanes().collect();
                let lane = pick(*kind, selector, lanes.clone(), |_| 0, |l| l.name.clone())?;
                let number = lanes
                    .iter()
                    .position(|l| std::ptr::eq(*l, lane))
                    .map_or(0, |at| at + 1);
                Found::Lane(track, lane, number)
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
