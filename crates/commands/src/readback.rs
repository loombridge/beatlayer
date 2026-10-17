//! Where each command's effect reads back (agent readback).
//!
//! The rule: anything an agent can set, it can read back. Every command that
//! isn't `read` names the place its effect shows up, in [`READBACK`]: a read
//! command and the field in its result, or the query tool's path for an
//! object and the field in its full result. An action with nothing to read
//! back says so, with a reason. [`problems`] checks the table against the
//! registry, and [`markdown`] renders it for the command reference.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::Value;

use crate::{Permission, Registry};

/// An object the query tool reads, by the path that reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    /// `song`: the song's overview and, under `saved`, its settings and scenes.
    Song,
    /// `track`.
    Track,
    /// `clip`.
    Clip,
    /// `device`.
    Device,
    /// `scene`.
    Scene,
}

impl Object {
    /// The query path that reaches the object.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Track => "track",
            Self::Clip => "clip",
            Self::Device => "device",
            Self::Scene => "scene",
        }
    }

    /// The fields of the object's full result that sit beside `saved` rather
    /// than in it. A test in `bl-agent` holds these against the real result.
    #[must_use]
    pub const fn listed(self) -> &'static [&'static str] {
        match self {
            Self::Song => &[
                "set.name",
                "set.path",
                "set.unsaved_changes",
                "tempo.bpm_at_start",
                "tempo.segments",
                "time_signature_at_start",
                "counts.tracks",
                "counts.clips",
                "counts.devices",
                "undo.next_undo",
                "undo.next_redo",
                "undo.steps",
                "scheduled",
                "checkpoints",
                "tracks",
            ],
            _ => &[],
        }
    }
}

/// Where a command's effect reads back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readback {
    /// A read command, and a field in its result: dotted names, with `[]`
    /// after a list (`agents.clients[].autonomous`).
    Command {
        /// The read command's ID.
        command: &'static str,
        /// The field in its result.
        field: &'static str,
    },
    /// The query tool, and a field of the object's full result. A field not
    /// listed beside `saved` is a field of the object as the set saves it.
    Query {
        /// The object.
        object: Object,
        /// The field.
        field: &'static str,
    },
    /// Nothing to read back, and why.
    Nothing {
        /// One line: why there's nothing to read.
        reason: &'static str,
    },
}

/// Reads back from `command`'s result.
const fn read(command: &'static str, field: &'static str) -> Readback {
    Readback::Command { command, field }
}

/// Reads back from the query tool.
const fn query(object: Object, field: &'static str) -> Readback {
    Readback::Query { object, field }
}

/// Has nothing to read back.
const fn nothing(reason: &'static str) -> Readback {
    Readback::Nothing { reason }
}

use Object::{Clip, Device, Scene, Song, Track};

/// Why the Browser's commands read back through nothing a read command gives.
const BROWSER: &str = "the Browser's state is read with `browser.state`, which is `external` \
                       because it lists files outside the project";
/// Why Save Continuously has nothing to read.
const CONTINUOUS: &str = "the app doesn't host the autosave service yet, so no read command \
                          reports Save Continuously; the command's own result does";
/// Why a command that writes a file has nothing to read.
const FILE: &str = "writes a file outside the set; the set doesn't change";
/// Why a token has nothing to read.
const TOKEN: &str = "a new random token, which an agent is never told: the Settings window \
                     shows it to the person";

/// Every command that isn't `read`, and where its effect reads back. One line
/// each, so a command's entry is easy to find.
#[rustfmt::skip]
pub const READBACK: &[(&str, Readback)] = &[
    ("agent.approve_client", read("settings.state", "agents.clients")),
    ("agent.forget_client", read("settings.state", "agents.clients")),
    ("agent.new_client_token", nothing(TOKEN)),
    ("agent.new_token", nothing(TOKEN)),
    ("agent.set_autonomous", read("settings.state", "agents.clients[].autonomous")),
    ("agent.set_server", read("settings.state", "agents.server")),
    ("app.quit", nothing("the app ends, so there is nothing left to read from")),
    ("arrangement.cut", query(Track, "clips")),
    ("arrangement.delete", query(Track, "clips")),
    ("arrangement.duplicate", query(Track, "clips")),
    ("arrangement.loop_selection", read("transport.state", "loop_start")),
    ("arrangement.move_clips", query(Track, "clips")),
    ("arrangement.nudge", query(Track, "clips")),
    ("arrangement.paste", query(Track, "clips")),
    ("arrangement.resize_clips", query(Track, "clips")),
    ("arrangement.split", query(Track, "clips")),
    ("assistant.forget_key", read("settings.state", "assistant.key_set")),
    ("assistant.set_base_url", read("settings.state", "assistant.base_url")),
    ("assistant.set_key", read("settings.state", "assistant.key_set")),
    ("assistant.set_model", read("settings.state", "assistant.model")),
    ("assistant.set_provider", read("settings.state", "assistant.provider")),
    ("audio.configure", read("audio.status", "settings")),
    ("audio.set_test_tone", read("audio.status", "test_tone")),
    ("audio.toggle_engine", read("audio.status", "engine_on")),
    ("automation.delete", query(Track, "automation")),
    ("automation.delete_breakpoint", query(Track, "automation")),
    ("automation.move_breakpoint", query(Track, "automation")),
    ("automation.re_enable", query(Track, "automation")),
    ("automation.re_enable_for", query(Track, "automation")),
    ("automation.set_breakpoint", query(Track, "automation")),
    ("automation.set_curve", query(Track, "automation")),
    ("browser.add_folder", nothing(BROWSER)),
    ("browser.add_to_arrangement", query(Track, "clips")),
    ("browser.load_selected", query(Track, "clips")),
    ("browser.open_folder", nothing(BROWSER)),
    ("browser.open_place", nothing(BROWSER)),
    ("browser.preview", nothing(BROWSER)),
    ("browser.remove_folder", nothing(BROWSER)),
    ("browser.search", nothing(BROWSER)),
    ("browser.select", nothing(BROWSER)),
    ("browser.set_preview", nothing(BROWSER)),
    (
        "browser.state",
        nothing(
            "a query of its own: it reads the Browser, and is `external` for the files it lists",
        ),
    ),
    ("clip.create", query(Track, "clips")),
    ("clip.create_in_slot", query(Track, "slots")),
    ("clip.delete", query(Track, "clips")),
    ("clip.delete_envelope", query(Clip, "envelopes")),
    ("clip.delete_envelope_breakpoint", query(Clip, "envelopes")),
    ("clip.move", query(Clip, "start")),
    ("clip.move_by", query(Clip, "start")),
    ("clip.move_envelope_breakpoint", query(Clip, "envelopes")),
    ("clip.move_to_slot", query(Track, "slots")),
    ("clip.rename", query(Clip, "name")),
    ("clip.set_data", query(Clip, "data")),
    ("clip.set_envelope_breakpoint", query(Clip, "envelopes")),
    ("clip.set_envelope_curve", query(Clip, "envelopes")),
    ("clip.set_envelope_linked", query(Clip, "envelopes")),
    ("clip.set_fades", query(Clip, "data.audio.fade_in")),
    ("clip.set_gain", query(Clip, "data.audio.gain")),
    ("clip.set_launch_quantization", query(Clip, "launch.quantization")),
    ("clip.set_length", query(Clip, "length")),
    ("clip.set_legato", query(Clip, "launch.legato")),
    ("clip.set_loop", query(Clip, "data.audio.loop_start")),
    ("clip.set_loop_enabled", query(Clip, "data.audio.looping")),
    ("clip.set_midi_loop", query(Clip, "data.midi.loop_start")),
    ("clip.set_midi_region", query(Clip, "data.midi.start")),
    ("clip.set_mute", query(Clip, "mute")),
    ("clip.set_region", query(Clip, "data.audio.start")),
    ("clip.set_velocity_amount", query(Clip, "launch.velocity_amount")),
    ("clip.trim_start", query(Clip, "offset")),
    ("device.add", query(Track, "devices")),
    ("device.cut", query(Track, "devices")),
    ("device.duplicate", query(Track, "devices")),
    ("device.insert", query(Track, "devices")),
    ("device.load_preset", query(Device, "params")),
    ("device.load_sample", query(Device, "media")),
    ("device.move", query(Track, "devices")),
    ("device.paste", query(Track, "devices")),
    ("device.remove", query(Track, "devices")),
    ("device.rename", query(Device, "name")),
    ("device.save_default_preset", nothing(FILE)),
    ("device.save_preset", nothing(FILE)),
    ("device.set_enabled", query(Device, "enabled")),
    ("device.set_floating", query(Device, "floating")),
    ("device.set_param", query(Device, "params")),
    ("device.set_sample", query(Device, "media")),
    ("device.set_show_preset_name", query(Device, "show_preset_name")),
    ("device.set_sidechain_enabled", query(Device, "sidechain.enabled")),
    ("device.set_sidechain_gain", query(Device, "sidechain.gain")),
    ("device.set_sidechain_input", query(Device, "sidechain.input")),
    ("device.set_sidechain_listen", query(Device, "sidechain.listen")),
    ("device.set_sidechain_mix", query(Device, "sidechain.mix")),
    ("device.set_state", query(Device, "params")),
    ("edit.redo", query(Song, "undo.next_redo")),
    ("edit.undo", query(Song, "undo.next_undo")),
    ("export.audio", nothing(FILE)),
    ("midi.export_clip", nothing(FILE)),
    ("midi.import_file", query(Track, "clips")),
    ("midi.set_port", read("midi.ports", "ports")),
    ("midi.toggle_computer_keyboard", read("midi.ports", "keyboard.on")),
    ("mixer.arm_exclusive", query(Track, "mixer.arm")),
    ("mixer.set_arm", query(Track, "mixer.arm")),
    ("mixer.set_crossfade_assign", query(Track, "mixer.crossfade")),
    ("mixer.set_crossfader", query(Song, "settings.crossfader")),
    ("mixer.set_mute", query(Track, "mixer.mute")),
    ("mixer.set_pan", query(Track, "mixer.pan")),
    ("mixer.set_send", query(Track, "sends")),
    ("mixer.set_send_tap", query(Track, "mixer.tap")),
    ("mixer.set_solo", query(Track, "mixer.solo")),
    ("mixer.set_solo_in_place", query(Song, "settings.solo_in_place")),
    ("mixer.set_volume", query(Track, "mixer.volume")),
    ("mixer.solo_exclusive", query(Track, "mixer.solo")),
    ("note.add", query(Clip, "data.midi.notes")),
    ("note.delete", query(Clip, "data.midi.notes")),
    ("note.move", query(Clip, "data.midi.notes")),
    ("note.quantize", query(Clip, "data.midi.notes")),
    ("note.ramp_value", query(Clip, "data.midi.notes")),
    ("note.resize", query(Clip, "data.midi.notes")),
    ("note.set_value", query(Clip, "data.midi.notes")),
    ("plugin.load", query(Track, "devices")),
    ("plugin.load_au", query(Track, "devices")),
    ("plugin.rescan", read("plugin.list", "plugins")),
    ("plugin.scan", read("plugin.list", "plugins")),
    ("plugin.set_panel_param", read("device_view.state", "devices")),
    ("plugin.settings", read("settings.state", "plug_ins")),
    ("routing.set_cue_out", query(Track, "routing.cue_out")),
    ("routing.set_input", query(Track, "routing.input")),
    ("routing.set_main_out", query(Track, "routing.main_out")),
    ("routing.set_midi_output", query(Track, "routing.midi_output")),
    ("routing.set_monitor", query(Track, "routing.monitor")),
    ("routing.set_output", query(Track, "routing.output")),
    ("scene.create", query(Song, "scenes")),
    ("scene.delete", query(Song, "scenes")),
    ("scene.duplicate", query(Song, "scenes")),
    ("scene.move", query(Song, "scenes")),
    ("scene.rename", query(Scene, "name")),
    ("session.copy_clip_to_slot", query(Track, "slots")),
    ("session.deactivate", query(Clip, "mute")),
    ("session.delete", query(Track, "slots")),
    ("session.drop_files", query(Track, "slots")),
    ("session.duplicate_scene", query(Song, "scenes")),
    ("session.insert_empty_midi_clip", query(Track, "slots")),
    ("session.insert_scene", query(Song, "scenes")),
    ("session.overdub", read("transport.state", "recording.session_slots")),
    ("session.record", read("transport.state", "recording.session_slots")),
    ("session.record_slot", read("transport.state", "recording.session_slots")),
    ("set.collect_all_and_save", query(Song, "set.unsaved_changes")),
    ("set.discard_changes", query(Song, "set.unsaved_changes")),
    ("set.discard_recovery", nothing("deletes a recovery file outside the set")),
    ("set.new", query(Song, "set.name")),
    ("set.open", query(Song, "set.path")),
    ("set.recover", query(Song, "set.path")),
    ("set.relink_missing", nothing("finds files outside the set; the clips that use them read their media")),
    ("set.save", query(Song, "set.unsaved_changes")),
    ("set.save_as", query(Song, "set.path")),
    ("set.save_as_default", nothing(FILE)),
    ("set.save_as_template", nothing(FILE)),
    ("settings.set_auto_hide_plug_in_windows", read("settings.state", "auto_hide_plug_in_windows")),
    ("settings.set_continuous_save", nothing(CONTINUOUS)),
    ("settings.set_create_fades_on_clip_edges", read("settings.state", "create_fades_on_clip_edges")),
    ("settings.set_outline_view_in_focus", read("settings.state", "display_input.outline_view_in_focus")),
    ("settings.set_default_launch_mode", read("settings.state", "launch.default_mode")),
    ("settings.set_default_launch_quantization", read("settings.state", "launch.default_quantization")),
    ("settings.set_select_next_scene_on_launch", read("settings.state", "launch.select_next_scene_on_launch")),
    ("settings.set_select_on_launch", read("settings.state", "launch.select_on_launch")),
    ("settings.set_start_recording_on_scene_launch", read("settings.state", "launch.start_recording_on_scene_launch")),
    ("settings.set_theme", read("settings.state", "theme")),
    ("settings.set_warp_recorded_audio", read("settings.state", "warp_recorded_audio")),
    ("slot.set_stop_button", query(Track, "removed_stop_buttons")),
    ("song.delete_tempo_breakpoint", query(Song, "settings.tempo")),
    ("song.delete_time_signature_change", query(Song, "settings.time_signatures")),
    ("song.move_tempo_breakpoint", query(Song, "settings.tempo")),
    ("song.move_time_signature_change", query(Song, "settings.time_signatures")),
    ("song.set_tempo", query(Song, "settings.tempo")),
    ("song.set_tempo_breakpoint", query(Song, "settings.tempo")),
    ("song.set_time_signature_change", query(Song, "settings.time_signatures")),
    ("song.set_time_signatures", query(Song, "settings.time_signatures")),
    ("track.add_take_lane", query(Track, "take_lanes")),
    ("track.create", query(Song, "tracks")),
    ("track.delete", query(Song, "tracks")),
    ("track.duplicate_without_clips", query(Song, "tracks")),
    ("track.group", query(Song, "tracks")),
    ("track.move", query(Song, "tracks")),
    ("track.rename", query(Track, "name")),
    ("track.ungroup", query(Song, "tracks")),
    ("transport.set_loop", read("transport.state", "loop_start")),
    ("transport.set_loop_enabled", read("transport.state", "loop_on")),
    ("transport.set_metronome_count_in", read("transport.state", "metronome.count_in")),
    ("transport.set_metronome_enabled", read("transport.state", "metronome.enabled")),
    ("transport.set_metronome_only_while_recording", read("transport.state", "metronome.only_while_recording")),
    ("transport.set_metronome_rhythm", read("transport.state", "metronome.rhythm")),
    ("transport.set_metronome_sound", read("transport.state", "metronome.sound")),
    ("transport.set_midi_overdub", read("transport.state", "midi_overdub")),
    ("transport.set_punch_in", read("transport.state", "punch_in")),
    ("transport.set_punch_out", read("transport.state", "punch_out")),
    ("transport.set_recording", read("transport.state", "recording")),
    ("view.set_zoom", read("view.state", "window.zoom")),
    ("view.zoom_in", read("view.state", "window.zoom")),
    ("view.zoom_out", read("view.state", "window.zoom")),
    ("view.zoom_reset", read("view.state", "window.zoom")),
];

/// Whether `field` is in the schema `root`: dotted names through its
/// properties, `[]` after a list, following references and the branches of
/// `oneOf`, `anyOf` and `allOf`.
#[must_use]
pub fn has_field(root: &Value, field: &str) -> bool {
    let mut here = vec![root];
    for segment in field.split('.') {
        let (name, is_list) = segment
            .strip_suffix("[]")
            .map_or((segment, false), |name| (name, true));
        let mut next: Vec<&Value> = here
            .iter()
            .flat_map(|schema| branches(root, schema))
            .filter_map(|branch| branch.get("properties")?.get(name))
            .collect();
        if is_list {
            next = next
                .iter()
                .flat_map(|schema| branches(root, schema))
                .filter_map(|branch| branch.get("items"))
                .collect();
        }
        if next.is_empty() {
            return false;
        }
        here = next;
    }
    true
}

/// `schema` and every schema it stands for: what it references, and the
/// branches of its combinators.
fn branches<'a>(root: &'a Value, schema: &'a Value) -> Vec<&'a Value> {
    fn walk<'a>(root: &'a Value, schema: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
        if depth > 16 {
            return;
        }
        out.push(schema);
        if let Some(target) = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| root.pointer(reference.strip_prefix('#')?))
        {
            walk(root, target, depth + 1, out);
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            for branch in schema
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(root, branch, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, schema, 0, &mut out);
    out
}

/// What is wrong with `table` as the readback of `registry`'s commands, one
/// line each: a command that changes something and has no entry, an entry
/// for a command that isn't registered, is `read`, or has two, a readback
/// through a command that isn't registered or isn't `read`, a field its
/// result schema lacks, a query field its object's saved form lacks (`saved`
/// gives the schema of the form: see [`Object::path`]), and an exemption
/// without a reason.
#[must_use]
pub fn problems(
    registry: &Registry,
    table: &[(&str, Readback)],
    saved: impl Fn(Object) -> Option<Value>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for (id, readback) in table {
        if !seen.insert(*id) {
            problems.push(format!("`{id}` has two readbacks"));
        }
        match registry.get(id) {
            None => problems.push(format!(
                "a readback is declared for `{id}`, which isn't registered"
            )),
            Some(spec) if spec.permission == Some(Permission::Read) => problems.push(format!(
                "a readback is declared for `{id}`, which is a read command and changes nothing"
            )),
            Some(_) => {}
        }
        match *readback {
            Readback::Command { command, field } => match registry.get(command) {
                None => problems.push(format!(
                    "`{id}` reads back from `{command}`, which isn't registered"
                )),
                Some(spec) if spec.permission != Some(Permission::Read) => problems.push(format!(
                    "`{id}` reads back from `{command}`, which isn't a read command"
                )),
                Some(spec) => {
                    let found = spec
                        .result
                        .as_ref()
                        .is_some_and(|schema| has_field(schema.as_value(), field));
                    if !found {
                        problems.push(format!(
                            "`{id}` reads back from `{field}` in `{command}`'s result, which has no such field"
                        ));
                    }
                }
            },
            Readback::Query { object, field } => {
                let found = object.listed().contains(&field)
                    || saved(object).is_some_and(|schema| has_field(&schema, field));
                if !found {
                    problems.push(format!(
                        "`{id}` reads back from `{field}` of a `{}` query, which has no such field",
                        object.path()
                    ));
                }
            }
            Readback::Nothing { reason } => {
                if reason.trim().is_empty() {
                    problems.push(format!(
                        "`{id}` has nothing to read back and gives no reason"
                    ));
                }
            }
        }
    }
    for spec in registry.iter() {
        if spec.permission != Some(Permission::Read) && !seen.contains(spec.id.as_ref()) {
            problems.push(format!(
                "`{}` changes something and names no place it reads back; add it to the readback table",
                spec.id
            ));
        }
    }
    problems
}

/// The reference's "Readback" section: where each command that isn't `read`
/// reads back, then the commands with nothing to read back and why.
#[must_use]
pub fn markdown(registry: &Registry, table: &[(&str, Readback)]) -> String {
    let anchor = |id: &str| format!("[`{id}`](#{})", id.replace('.', ""));
    let mut reads = String::new();
    let mut nothing = String::new();
    for spec in registry.iter() {
        let Some((_, readback)) = table.iter().find(|(id, _)| *id == spec.id) else {
            continue;
        };
        let id = anchor(&spec.id);
        match *readback {
            Readback::Command { command, field } => {
                let _ = writeln!(reads, "| {id} | `{command}` | `{field}` |");
            }
            Readback::Query { object, field } => {
                let _ = writeln!(
                    reads,
                    "| {id} | the query tool's `{}` | `{field}` |",
                    object.path()
                );
            }
            Readback::Nothing { reason } => {
                let _ = writeln!(nothing, "| {id} | {reason} |");
            }
        }
    }
    format!(
        "## Readback\n\n\
         Anything an agent can set, it can read back. Each command that isn't `read` names where \
         its effect shows: a read command and the field in its result, or the query tool's path \
         for an object and the field in its full result. A query field is one the set saves for \
         the object, or one of the song's overview. `cargo xtask commands` fails when a command \
         that changes something names none, or names a field that isn't there.\n\n\
         | Command | Read it with | Field |\n|---|---|---|\n{reads}\n\
         ### Nothing to read back\n\n\
         | Command | Why |\n|---|---|\n{nothing}\n"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{CommandSpec, NoArgs};

    /// A registry of a setter, a read command whose result has a field
    /// `loop_on` and a list `clients` of objects with a `name`, and an action.
    fn registry() -> Registry {
        let state = crate::schemars::json_schema!({
            "type": "object",
            "properties": {
                "loop_on": { "type": "boolean" },
                "clients": { "type": "array", "items": { "$ref": "#/$defs/Client" } }
            },
            "$defs": { "Client": { "type": "object", "properties": { "name": { "type": "string" } } } }
        });
        let command = |id: &'static str, permission| {
            CommandSpec::new(id, id)
                .description("A test command.")
                .permission(permission)
                .context(crate::Context::Global)
                .args::<NoArgs>()
        };
        let mut registry = Registry::new();
        for spec in [
            command("test.state", Permission::Read).result_schema(state),
            command("test.set_loop", Permission::Edit).result::<()>(),
            command("test.quit", Permission::Destructive).result::<()>(),
        ] {
            registry.register(spec).unwrap();
        }
        registry
    }

    fn saved(object: Object) -> Option<Value> {
        (object == Object::Track).then(|| {
            json!({ "properties": { "routing": { "$ref": "#/$defs/Routing" } },
                    "$defs": { "Routing": { "properties": { "output": {} } } } })
        })
    }

    fn check(table: &[(&str, Readback)]) -> Vec<String> {
        problems(&registry(), table, saved)
    }

    #[test]
    fn a_complete_table_has_no_problems() {
        let table = [
            ("test.set_loop", read("test.state", "loop_on")),
            ("test.quit", nothing("ends the app")),
        ];
        assert_eq!(check(&table), Vec::<String>::new());
        let by_list = [
            ("test.set_loop", read("test.state", "clients[].name")),
            ("test.quit", query(Track, "routing.output")),
        ];
        assert_eq!(check(&by_list), Vec::<String>::new());
    }

    #[test]
    fn a_setter_with_no_declaration_fails() {
        let table = [("test.quit", nothing("ends the app"))];
        let found = check(&table);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("`test.set_loop` changes something"));
    }

    #[test]
    fn a_declaration_naming_a_missing_or_non_read_command_fails() {
        let table = [
            ("test.set_loop", read("test.gone", "loop_on")),
            ("test.quit", read("test.set_loop", "loop_on")),
            ("test.ghost", nothing("not registered")),
            ("test.state", nothing("reads")),
        ];
        let found = check(&table).join("\n");
        assert!(
            found.contains("`test.gone`, which isn't registered"),
            "{found}"
        );
        assert!(
            found.contains("`test.set_loop`, which isn't a read command"),
            "{found}"
        );
        assert!(
            found.contains("declared for `test.ghost`, which isn't registered"),
            "{found}"
        );
        assert!(
            found.contains("declared for `test.state`, which is a read command"),
            "{found}"
        );
    }

    #[test]
    fn a_declaration_naming_a_field_the_schema_lacks_fails() {
        let table = [
            ("test.set_loop", read("test.state", "loop_off")),
            ("test.quit", query(Track, "routing.input")),
        ];
        let found = check(&table).join("\n");
        assert!(
            found.contains("`loop_off` in `test.state`'s result"),
            "{found}"
        );
        assert!(
            found.contains("`routing.input` of a `track` query"),
            "{found}"
        );
    }

    #[test]
    fn an_exemption_needs_a_reason() {
        let table = [
            ("test.set_loop", read("test.state", "loop_on")),
            ("test.quit", nothing(" ")),
        ];
        assert!(check(&table)[0].contains("gives no reason"));
    }

    #[test]
    fn the_reference_lists_readbacks_and_exemptions() {
        let table = [
            ("test.set_loop", read("test.state", "loop_on")),
            ("test.quit", nothing("ends the app")),
        ];
        let text = markdown(&registry(), &table);
        assert!(text.contains("| [`test.set_loop`](#testset_loop) | `test.state` | `loop_on` |"));
        assert!(text.contains("| [`test.quit`](#testquit) | ends the app |"));
    }

    #[test]
    fn the_table_names_each_command_once() {
        let mut ids: Vec<&str> = READBACK.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }
}
