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
                "scale",
                "tuning",
                "counts.tracks",
                "counts.clips",
                "counts.devices",
                "undo.next_undo",
                "undo.next_redo",
                "undo.steps",
                "undo.redo_steps",
                "undo.current",
                "scheduled",
                "checkpoints",
                "tracks",
            ],
            Self::Device => &["compare"],
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
/// Why the browser's tag commands read back through nothing a read command
/// gives.
const TAGS: &str = "the browser's tags are read with `browser.tags`, which is `external` because \
                    it lists files outside the project";
/// Why the File Manager's commands read back through nothing a read command
/// gives.
const FILE_MANAGER: &str = "the File Manager's state is read with `file_manager.state`, which \
                             is `external` because it lists files outside the project";
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
    ("arrangement.consolidate", query(Track, "clips")),
    ("arrangement.consolidate_to_scene", query(Song, "scenes")),
    ("arrangement.reverse", query(Track, "clips")),
    ("arrangement.create_fade", query(Clip, "data.audio.fade_in")),
    ("arrangement.cut", query(Track, "clips")),
    ("arrangement.delete_crossfade", query(Clip, "data.audio.fade_in_before")),
    ("arrangement.delete_fades", query(Clip, "data.audio.fade_in")),
    ("arrangement.delete", query(Track, "clips")),
    ("arrangement.duplicate", query(Track, "clips")),
    ("arrangement.loop_selection", read("transport.state", "loop_start")),
    ("arrangement.move_clips", query(Track, "clips")),
    ("arrangement.nudge", query(Track, "clips")),
    ("arrangement.paste", query(Track, "clips")),
    ("arrangement.resize_clips", query(Track, "clips")),
    ("arrangement.set_crossfade", query(Clip, "data.audio.fade_in_before")),
    ("arrangement.split", query(Track, "clips")),
    ("articulation.add", read("articulation.list", "articulations")),
    ("articulation.delete", read("articulation.list", "articulations")),
    ("articulation.move", read("articulation.list", "articulations")),
    ("articulation.set", read("articulation.list", "articulations")),
    ("assistant.forget_key", read("settings.state", "assistant.key_set")),
    ("clip.duplicate_loop", query(Clip, "data.midi.loop_end")),
    ("clip.set_groove", query(Clip, "groove")),
    ("clip.set_signature", query(Clip, "data.midi.signature")),
    ("groove.add", query(Song, "settings.grooves")),
    ("groove.commit", query(Clip, "groove")),
    ("groove.delete", query(Song, "settings.grooves")),
    ("groove.extract", query(Song, "settings.grooves")),
    ("groove.load", query(Song, "settings.grooves")),
    ("groove.rename", query(Song, "settings.grooves")),
    ("groove.save", nothing(FILE)),
    ("groove.set", query(Song, "settings.grooves")),
    ("groove.set_auto_load", query(Song, "settings.grooves")),
    ("groove.set_global_amount", query(Song, "settings.grooves")),
    ("groove.to_clip", query(Track, "clips")),
    ("locator.add", query(Song, "settings.locators")),
    ("locator.delete", query(Song, "settings.locators")),
    ("locator.loop_to_next", query(Song, "settings.arrangement_loop")),
    ("locator.move", query(Song, "settings.locators")),
    ("locator.rename", query(Song, "settings.locators")),
    ("locator.set_info_text", query(Song, "settings.locators")),
    // The document's own locator commands (WP-6.24), which the shell's
    // `locator.*` commands, the UI, and the keys run.
    ("song.add_locator", query(Song, "settings.locators")),
    ("song.delete_locator", query(Song, "settings.locators")),
    ("song.loop_to_next_locator", query(Song, "settings.arrangement_loop")),
    ("song.move_locator", query(Song, "settings.locators")),
    ("song.rename_locator", query(Song, "settings.locators")),
    ("song.set_locator_info_text", query(Song, "settings.locators")),
    ("song.set_start", query(Song, "settings.song_start")),
    ("assistant.set_base_url", read("settings.state", "assistant.base_url")),
    ("assistant.set_key", read("settings.state", "assistant.key_set")),
    ("assistant.set_model", read("settings.state", "assistant.model")),
    ("assistant.set_provider", read("settings.state", "assistant.provider")),
    ("audio.configure", read("audio.status", "settings")),
    ("audio.set_test_tone", read("audio.status", "test_tone")),
    ("audio.toggle_engine", read("audio.status", "engine_on")),
    ("automation.cut", query(Track, "automation")),
    ("automation.delete", query(Track, "automation")),
    ("automation.delete_range", query(Track, "automation")),
    ("automation.duplicate", query(Track, "automation")),
    ("automation.move_breakpoints", query(Track, "automation")),
    ("automation.insert_shape", query(Track, "automation")),
    ("automation.paste", query(Track, "automation")),
    ("automation.simplify", query(Track, "automation")),
    ("automation.skew", query(Track, "automation")),
    ("automation.stretch_time", query(Track, "automation")),
    ("automation.stretch_values", query(Track, "automation")),
    ("automation.delete_breakpoint", query(Track, "automation")),
    ("automation.move_breakpoint", query(Track, "automation")),
    ("automation.re_enable", query(Track, "automation")),
    ("automation.re_enable_for", query(Track, "automation")),
    ("automation.set_breakpoint", query(Track, "automation")),
    ("automation.set_curve", query(Track, "automation")),
    ("automation.set_envelope", query(Track, "automation")),
    ("keymap.bind", read("keymap.state", "shortcuts")),
    ("keymap.reset", read("keymap.state", "shortcuts")),
    ("keymap.unbind", read("keymap.state", "shortcuts")),
    ("browser.add_folder", nothing(BROWSER)),
    ("browser.add_tag", nothing(TAGS)),
    ("browser.add_tag_group", nothing(TAGS)),
    ("browser.add_to_arrangement", query(Track, "clips")),
    ("browser.delete_tag", nothing(TAGS)),
    ("browser.load_selected", query(Track, "clips")),
    ("browser.move_tag", nothing(TAGS)),
    ("browser.open_folder", nothing(BROWSER)),
    ("browser.open_place", nothing(BROWSER)),
    ("browser.preview", nothing(BROWSER)),
    ("browser.remove_folder", nothing(BROWSER)),
    ("browser.rename_file", nothing(BROWSER)),
    ("browser.rename_tag", nothing(TAGS)),
    ("browser.reset_tag_order", nothing(TAGS)),
    ("browser.scrub_preview", nothing(BROWSER)),
    ("browser.search", nothing(BROWSER)),
    ("browser.select", nothing(BROWSER)),
    ("browser.set_preview", nothing(BROWSER)),
    ("browser.set_raw", nothing(BROWSER)),
    ("browser.show_file", nothing(BROWSER)),
    ("browser.show_quick_tags", nothing(BROWSER)),
    ("browser.tag_items", nothing(TAGS)),
    (
        "browser.tags",
        nothing(
            "a query of its own: it reads the tags, and is `external` for the files it lists",
        ),
    ),
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
    ("clip.delete_transients", read("clip.transients", "transients")),
    ("clip.delete_warp_markers", query(Clip, "data.audio.warp_markers")),
    ("clip.edit_sample", query(Clip, "data.audio.media")),
    ("clip.insert_transient", read("clip.transients", "transients")),
    ("clip.insert_warp_marker", query(Clip, "data.audio.warp_markers")),
    ("clip.insert_warp_markers", query(Clip, "data.audio.warp_markers")),
    ("clip.move", query(Clip, "start")),
    ("clip.move_by", query(Clip, "start")),
    ("clip.move_envelope_breakpoint", query(Clip, "envelopes")),
    ("clip.move_to_slot", query(Track, "slots")),
    ("clip.move_transient", read("clip.transients", "transients")),
    ("clip.move_warp_markers", query(Clip, "data.audio.warp_markers")),
    ("clip.quantize_audio", query(Clip, "data.audio.warp_markers")),
    ("clip.replace_sample", query(Clip, "data.audio.media")),
    ("clip.rename", query(Clip, "name")),
    ("clip.reverse", query(Clip, "data")),
    ("clip.reverse_file", query(Clip, "data")),
    ("clip.reset_transients", read("clip.transients", "transients")),
    ("clip.save_default", read("clip.defaults", "defaults")),
    ("clip.save_file", nothing(BROWSER)),
    ("clip.slice_to_midi_track", query(Song, "tracks")),
    ("clip.crop", query(Clip, "data")),
    ("clip.crop_region", query(Clip, "data")),
    ("clip.crop_span", query(Clip, "data")),
    ("clip.set_data", query(Clip, "data")),
    ("clip.set_envelope_breakpoint", query(Clip, "envelopes")),
    ("clip.set_envelope_curve", query(Clip, "envelopes")),
    ("clip.set_envelope_linked", query(Clip, "envelopes")),
    ("clip.set_fades", query(Clip, "data.audio.fade_in")),
    ("clip.set_gain", query(Clip, "data.audio.gain")),
    ("clip.set_follow_actions", query(Clip, "launch.follow")),
    ("clip.set_high_quality", query(Clip, "data.audio.high_quality")),
    ("clip.set_pitch", query(Clip, "data.audio")),
    ("clip.set_launch_mode", query(Clip, "launch.mode")),
    ("clip.set_launch_quantization", query(Clip, "launch.quantization")),
    ("clip.set_length", query(Clip, "length")),
    ("clip.set_legato", query(Clip, "launch.legato")),
    ("clip.set_loop", query(Clip, "data.audio.loop_start")),
    ("clip.set_loop_enabled", query(Clip, "data.audio.looping")),
    ("clip.set_scale", query(Clip, "scale")),
    ("clip.set_midi_loop", query(Clip, "data.midi.loop_start")),
    ("clip.set_midi_region", query(Clip, "data.midi.start")),
    ("clip.set_mute", query(Clip, "mute")),
    ("clip.scale_tempo", query(Clip, "data.audio.tempo")),
    ("clip.set_ram_mode", query(Clip, "data.audio.ram")),
    ("clip.set_region", query(Clip, "data.audio.start")),
    ("clip.set_segment_bpm", query(Clip, "data.audio.warp_markers")),
    ("clip.set_tempo_leader", query(Clip, "data.audio.tempo_leader")),
    ("clip.slide_contents", query(Clip, "data.audio.start")),
    ("clip.stretch", query(Clip, "data.audio.warp_markers")),
    ("clip.set_velocity_amount", query(Clip, "launch.velocity_amount")),
    ("clip.set_warp", query(Clip, "data.audio.warp")),
    ("clip.set_warp_mode", query(Clip, "data.audio.warp_mode")),
    ("clip.set_warp_settings", query(Clip, "data.audio.warp_settings")),
    ("clip.shift_warp_marker", query(Clip, "data.audio.warp_markers")),
    ("clip.trim_start", query(Clip, "offset")),
    ("clip.set_1_1_1_here", query(Clip, "data.audio.warp_markers")),
    ("clip.warp_as_loop", query(Clip, "data.audio.warp_markers")),
    ("clip.warp_from_here", query(Clip, "data.audio.warp_markers")),
    ("clip.warp_selection_as_loop", query(Clip, "data.audio.warp_markers")),
    ("clip_view.set_preview", read("view.state", "clip_view.preview")),
    ("clip_view.step_input_mode", query(Track, "mixer.arm")),
    ("chain.add", query(Device, "chains")),
    ("chain.create", query(Device, "chains")),
    ("chain.cut", query(Device, "chains")),
    ("chain.delete", query(Device, "chains")),
    ("chain.drop_on_track", query(Track, "devices")),
    ("chain.duplicate", query(Device, "chains")),
    ("chain.extract", query(Track, "devices")),
    ("chain.extract_return", query(Track, "devices")),
    ("chain.load_preset", query(Device, "chains")),
    ("chain.preview", read("device_view.state", "devices")),
    ("chain.move", query(Device, "chains")),
    ("chain.paste", query(Device, "chains")),
    ("chain.remove", query(Device, "chains")),
    ("chain.rename", query(Device, "chains")),
    ("chain.save_preset", nothing(FILE)),
    ("chain.set_colour", query(Device, "chains")),
    ("chain.set_enabled", query(Device, "chains")),
    ("chain.set_info", query(Device, "chains")),
    ("chain.set_pan", query(Device, "chains")),
    ("chain.set_solo", query(Device, "chains")),
    ("chain.set_receive", query(Device, "chains")),
    ("chain.set_play", query(Device, "chains")),
    ("chain.set_choke", query(Device, "chains")),
    ("chain.set_send", query(Device, "chains")),
    ("chain.set_audio_to", query(Device, "chains")),
    ("chain.set_gain", query(Device, "chains")),
    ("chain.set_zone", query(Device, "chains")),
    ("chain.solo", query(Device, "chains")),
    ("device.add", query(Track, "devices")),
    ("device.add_preset", query(Track, "devices")),
    ("device.add_layers", query(Device, "layers")),
    ("device.add_sample_edit", query(Device, "sample_edits")),
    ("device.add_slice", query(Device, "slices")),
    ("device.autosample", query(Track, "devices")),
    ("device.autosample_cancel", nothing("it stops a running autosample")),
    ("device.clear_sample_edits", query(Device, "sample_edits")),
    ("device.convert", query(Track, "devices")),
    ("device.crop_sample", query(Device, "media")),
    ("device.compare_copy", query(Device, "compare")),
    ("device.compare_switch", query(Device, "compare")),
    ("device.cut", query(Track, "devices")),
    ("device.delete_layers", query(Device, "layers")),
    ("device.delete_slice", query(Device, "slices")),
    ("device.distribute_layers", query(Device, "layers")),
    ("device.duplicate", query(Track, "devices")),
    ("device.edit_sample", query(Device, "sample_edits")),
    ("device.duplicate_layers", query(Device, "layers")),
    ("device.group", query(Track, "devices")),
    ("device.group_to_drum_rack", query(Track, "devices")),
    ("device.import_sfz", query(Device, "layers")),
    ("device.insert", query(Track, "devices")),
    ("device.insert_in_chain", query(Device, "chains")),
    ("device.insert_layers", query(Device, "layers")),
    ("device.load_preset", query(Device, "params")),
    ("device.load_sample", query(Device, "media")),
    ("device.move", query(Track, "devices")),
    ("device.move_layers", query(Device, "layers")),
    ("device.move_slice", query(Device, "slices")),
    ("device.move_to_chain", query(Device, "chains")),
    ("device.normalize_layers", query(Device, "layers")),
    ("device.paste", query(Track, "devices")),
    ("device.remove", query(Track, "devices")),
    ("device.reverse_sample", query(Device, "media")),
    ("device.rename", query(Device, "name")),
    ("device.rename_layer", query(Device, "layers")),
    ("device.replace", query(Track, "devices")),
    ("device.save_default_preset", nothing(FILE)),
    ("device.save_preset", nothing(FILE)),
    ("device.scale_tempo", query(Device, "warp.bpm")),
    ("device.swap_to_snapshot", query(Track, "devices")),
    ("device.set_enabled", query(Device, "enabled")),
    ("device.set_enabled_all", query(Device, "chains")),
    ("device.set_external", query(Device, "external")),
    ("device.set_floating", query(Device, "floating")),
    ("device.set_layer", query(Device, "layers")),
    ("device.set_layer_media", query(Device, "layers")),
    ("device.set_layer_options", query(Device, "layers")),
    ("device.set_layer_sample", query(Device, "layers")),
    ("device.set_layer_zone", query(Device, "layers")),
    ("device.set_configured_params", read("device.parameters", "params")),
    ("device.set_param", query(Device, "params")),
    ("device.set_processed_layer_sample", query(Device, "layers")),
    ("device.set_sample", query(Device, "media")),
    ("device.set_sample_bpm", query(Device, "warp.bpm")),
    ("device.set_show_preset_name", query(Device, "show_preset_name")),
    ("device.set_role", query(Device, "role")),
    ("device.set_midi_sidechain_source", query(Device, "midi_sidechain.source")),
    ("device.set_sidechain_enabled", query(Device, "sidechain.enabled")),
    ("device.set_sidechain_gain", query(Device, "sidechain.gain")),
    ("device.set_sidechain_input", query(Device, "sidechain.input")),
    ("device.set_sidechain_listen", query(Device, "sidechain.listen")),
    ("device.set_sidechain_mix", query(Device, "sidechain.mix")),
    ("device.set_state", query(Device, "params")),
    ("device.set_warp", query(Device, "warp.warp")),
    ("device.set_warp_mode", query(Device, "warp.mode")),
    ("device.set_warp_settings", query(Device, "warp.settings")),
    ("device.snap_layer_markers", query(Device, "layers")),
    ("device.sort_layers", query(Device, "layers")),
    ("device.ungroup", query(Track, "devices")),
    ("device.unwrap_rack", query(Track, "devices")),
    ("device.update_layers", query(Device, "layers")),
    ("device.wrap_in_rack", query(Track, "devices")),
    ("edit.redo", query(Song, "undo.next_redo")),
    ("edit.undo", query(Song, "undo.next_undo")),
    ("edit.go_to_step", query(Song, "undo.current")),
    ("export.audio", nothing(FILE)),
    (
        "file_manager.delete_unused",
        nothing("moves files outside the set to the trash; the set doesn't change"),
    ),
    ("file_manager.edit_file", query(Clip, "data.audio.media")),
    ("file_manager.hot_swap", nothing(FILE_MANAGER)),
    ("file_manager.replace_file", query(Clip, "data.audio.media")),
    (
        "file_manager.state",
        nothing(
            "a query of its own: it reads the File Manager, and is `external` for the files it \
             lists",
        ),
    ),
    ("export.realtime.auto_restart", read("export.realtime.status", "auto_restart")),
    ("export.realtime.cancel", read("export.realtime.status", "phase")),
    ("export.realtime.restart", read("export.realtime.status", "phase")),
    ("export.realtime.skip", read("export.realtime.status", "phase")),
    ("export.start", read("export.status", "export")),
    ("midi.export_clip", nothing(FILE)),
    ("midi.capture", query(Track, "slots")),
    ("midi.import_file", query(Track, "clips")),
    ("midi.set_port", read("midi.ports", "ports")),
    ("midi.toggle_computer_keyboard", read("midi.ports", "keyboard.on")),
    ("mixer.arm_exclusive", query(Track, "mixer.arm")),
    ("mixer.set_arm", query(Track, "mixer.arm")),
    ("mixer.set_crossfade_assign", query(Track, "mixer.crossfade")),
    ("mixer.set_crossfader", query(Song, "settings.crossfader")),
    ("mixer.set_mute", query(Track, "mixer.mute")),
    ("mixer.set_pan", query(Track, "mixer.pan")),
    ("mixer.set_pan_mode", query(Track, "mixer.pan_mode")),
    ("mixer.set_split_pan_left", query(Track, "mixer.split_left")),
    ("mixer.set_split_pan_right", query(Track, "mixer.split_right")),
    ("mixer.set_track_delay", query(Track, "delay")),
    ("mixer.set_keep_monitoring_latency", query(Track, "keep_monitoring_latency")),
    ("mixer.set_delay_compensation", query(Song, "settings.delay_compensation")),
    (
        "mixer.set_reduced_latency_monitoring",
        query(Song, "settings.reduced_latency_monitoring"),
    ),
    ("mixer.set_send", query(Track, "sends")),
    ("mixer.set_send_tap", query(Track, "mixer.tap")),
    ("mixer.set_solo", query(Track, "mixer.solo")),
    ("mixer.set_solo_in_place", query(Song, "settings.solo_in_place")),
    ("song.set_lock_envelopes", query(Song, "settings.lock_envelopes")),
    ("mixer.set_cue_mode", query(Song, "settings.cue_mode")),
    ("mixer.set_cue_volume", query(Song, "settings.cue_volume")),
    ("mixer.set_volume", query(Track, "mixer.volume")),
    ("mixer.solo_exclusive", query(Track, "mixer.solo")),
    ("modulator.map", query(Device, "mappings")),
    ("modulator.set_shape", query(Device, "shape")),
    ("modulator.unmap", query(Device, "mappings")),
    ("note.add", query(Clip, "data.midi.notes")),
    ("note.adjust_value", query(Clip, "data.midi.notes")),
    ("note.chop", query(Clip, "data.midi.notes")),
    ("note.cut", query(Clip, "data.midi.notes")),
    ("note.deactivate", query(Clip, "data.midi.notes")),
    ("note.delete", query(Clip, "data.midi.notes")),
    ("note.duplicate", query(Clip, "data.midi.notes")),
    ("note.fit_to_time", query(Clip, "data.midi.notes")),
    ("note.group", query(Clip, "data.midi.groups")),
    ("note.join", query(Clip, "data.midi.notes")),
    ("note.move", query(Clip, "data.midi.notes")),
    ("note.paste", query(Clip, "data.midi.notes")),
    ("note.quantize", query(Clip, "data.midi.notes")),
    ("note.ramp_value", query(Clip, "data.midi.notes")),
    ("note.preview", nothing("plays a note and changes nothing in the set")),
    ("note.randomize", query(Clip, "data.midi.notes")),
    ("note.resize", query(Clip, "data.midi.notes")),
    ("note.set_value", query(Clip, "data.midi.notes")),
    ("note.split", query(Clip, "data.midi.notes")),
    ("note.split_clip", query(Track, "clips")),
    ("note.step_record", query(Clip, "data.midi.notes")),
    ("note.stretch", query(Clip, "data.midi.notes")),
    ("note.ungroup", query(Clip, "data.midi.groups")),
    ("pad.delete", query(Device, "chains")),
    ("pad.drop", query(Device, "chains")),
    ("pad.preview", read("device_view.state", "devices")),
    ("pad.rename", query(Device, "chains")),
    (
        "pad.save_default",
        nothing(
            "the default pad is the shell's, kept in memory and in the User Library, not in \
             the set; the next `pad.drop` on an empty pad makes its chain from it",
        ),
    ),
    ("pad.set_mute", query(Device, "chains")),
    ("pad.set_solo", query(Device, "chains")),
    ("pad.swap", query(Device, "chains")),
    ("plugin.configure", read("device_view.state", "devices")),
    ("plugin.configure_param", read("device.parameters", "params")),
    ("plugin.export_list", nothing(FILE)),
    ("plugin.load", query(Track, "devices")),
    ("plugin.load_au", query(Track, "devices")),
    ("plugin.move_configured_param", read("device.parameters", "params")),
    ("plugin.rescan", read("plugin.list", "plugins")),
    ("plugin.scan", read("plugin.list", "plugins")),
    ("plugin.set_panel_param", read("device_view.state", "devices")),
    ("plugin.settings", read("settings.state", "plug_ins")),
    ("plugin.unconfigure_param", read("device.parameters", "params")),
    ("rack.set_auto_select", read("device_view.state", "devices")),
    ("rack.set_macro_count", read("rack.macros", "count")),
    ("rack.map_macro", read("rack.macros", "macros")),
    ("rack.map_mode", read("device_view.state", "map_mode")),
    ("rack.unmap_macro", read("rack.macros", "macros")),
    ("rack.set_mapping_range", read("rack.macros", "macros")),
    ("rack.invert_mapping", read("rack.macros", "macros")),
    ("rack.rename_macro", read("rack.macros", "macros")),
    ("rack.set_macro_colour", read("rack.macros", "macros")),
    ("rack.set_macro_info_text", read("rack.macros", "macros")),
    ("rack.randomize_macros", read("rack.macros", "macros")),
    ("rack.set_macro_randomize_excluded", read("rack.macros", "macros")),
    ("rack.set_macro_style", read("rack.macros", "macros")),
    ("rack.set_mapped_value", read("rack.macros", "macros")),
    ("rack.select_macro", read("rack.macros", "macros")),
    ("rack.store_variation", read("rack.macros", "variations")),
    ("rack.launch_variation", read("rack.macros", "macros")),
    ("rack.overwrite_variation", read("rack.macros", "variations")),
    ("rack.rename_variation", read("rack.macros", "variations")),
    ("rack.duplicate_variation", read("rack.macros", "variations")),
    ("rack.delete_variation", read("rack.macros", "variations")),
    ("rack.set_macro_variation_excluded", read("rack.macros", "macros")),
    ("rack.select_variation", read("rack.macros", "variations")),
    ("rack.set_view", read("device_view.state", "devices")),
    ("rack.set_zone_editor", read("device_view.state", "devices")),
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
    ("scene.set_follow_actions", query(Scene, "follow")),
    ("scene.set_tempo", query(Scene, "tempo")),
    ("scene.set_time_signature", query(Scene, "signature")),
    ("session.capture_and_insert_scene", query(Song, "scenes")),
    ("session.copy_clip_to_slot", query(Track, "slots")),
    ("session.create_follow_action_chain", query(Clip, "launch.follow")),
    ("session.deactivate", query(Clip, "mute")),
    ("session.delete", query(Track, "slots")),
    ("session.drop_files", query(Track, "slots")),
    ("session.duplicate_scene", query(Song, "scenes")),
    ("session.insert_empty_midi_clip", query(Track, "slots")),
    ("session.insert_scene", query(Song, "scenes")),
    ("session.overdub", read("transport.state", "recording.session_slots")),
    ("session.record", read("transport.state", "recording.session_slots")),
    ("session.record_slot", read("transport.state", "recording.session_slots")),
    ("session.toggle_follow_actions", query(Clip, "launch.follow.enabled")),
    ("set.collect_all_and_save", query(Song, "set.unsaved_changes")),
    ("set.discard_changes", query(Song, "set.unsaved_changes")),
    ("set.discard_recovery", nothing("deletes a recovery file outside the set")),
    ("set.import_dawproject", query(Song, "set.name")),
    ("set.new", query(Song, "set.name")),
    ("set.open", query(Song, "set.path")),
    ("set.open_backup", query(Song, "set.unsaved_changes")),
    ("set.recover", query(Song, "set.path")),
    ("set.relink_missing", nothing("finds files outside the set; the clips that use them read their media")),
    ("set.save", query(Song, "set.unsaved_changes")),
    ("set.save_as", query(Song, "set.path")),
    ("set.save_as_default", nothing(FILE)),
    ("set.save_copy", nothing(FILE)),
    ("set.save_as_template", nothing(FILE)),
    ("settings.set_auto_hide_plug_in_windows", read("settings.state", "auto_hide_plug_in_windows")),
    ("settings.set_continuous_save", read("settings.state", "file_folder.save_continuously")),
    ("settings.set_auto_warp_long_samples", read("settings.state", "warp.auto_warp_long_samples")),
    ("settings.set_default_warp_mode", read("settings.state", "warp.default_warp_mode")),
    ("settings.set_loop_warp_short_samples", read("settings.state", "warp.loop_warp_short_samples")),
    ("settings.set_create_fades_on_clip_edges", read("settings.state", "create_fades_on_clip_edges")),
    ("settings.set_retune_on_tuning_load", read("settings.state", "retune_on_tuning_load")),
    ("settings.set_sample_editor", read("settings.state", "file_folder.sample_editor")),
    ("settings.set_collect_files_on_export", read("settings.state", "library.collect_files_on_export")),
    ("settings.set_user_library", read("settings.state", "library.user_library")),
    ("settings.set_temporary_folder", read("settings.state", "file_folder.temporary_folder")),
    ("settings.set_outline_view_in_focus", read("settings.state", "display_input.outline_view_in_focus")),
    ("settings.set_default_launch_mode", read("settings.state", "launch.default_mode")),
    ("settings.set_default_launch_quantization", read("settings.state", "launch.default_quantization")),
    ("settings.set_record_session_automation_in", read("settings.state", "record_session_automation_in")),
    ("settings.set_automation_record_mode", read("settings.state", "automation_record_mode")),
    ("settings.set_select_next_scene_on_launch", read("settings.state", "launch.select_next_scene_on_launch")),
    ("settings.set_select_on_launch", read("settings.state", "launch.select_on_launch")),
    ("settings.set_start_recording_on_scene_launch", read("settings.state", "launch.start_recording_on_scene_launch")),
    ("settings.set_theme", read("settings.state", "theme_colors.theme")),
    ("settings.set_appearance", read("settings.state", "theme_colors.appearance")),
    ("settings.set_tone", read("settings.state", "theme_colors.tone")),
    ("settings.set_high_contrast", read("settings.state", "theme_colors.high_contrast")),
    ("settings.set_grid_line_intensity", read("settings.state", "theme_colors.grid_line_intensity")),
    ("settings.set_brightness", read("settings.state", "theme_colors.brightness")),
    ("settings.set_color_intensity", read("settings.state", "theme_colors.color_intensity")),
    ("settings.set_color_hue", read("settings.state", "theme_colors.color_hue")),
    ("settings.set_warp_recorded_audio", read("settings.state", "warp_recorded_audio")),
    ("slot.set_stop_button", query(Track, "removed_stop_buttons")),
    ("song.delete_tempo_breakpoint", query(Song, "settings.tempo")),
    ("song.delete_time_signature_change", query(Song, "settings.time_signatures")),
    ("song.move_tempo_breakpoint", query(Song, "settings.tempo")),
    ("song.move_time_signature_change", query(Song, "settings.time_signatures")),
    ("song.set_scale", query(Song, "scale")),
    ("tuning.load", read("tuning.state", "tuning")),
    ("tuning.remove", read("tuning.state", "tuning")),
    ("tuning.set", read("tuning.state", "tuning")),
    ("tuning.set_range", read("tuning.state", "tuning")),
    ("tuning.set_reference", read("tuning.state", "tuning")),
    ("song.set_start_time", query(Song, "settings.song_start")),
    ("song.set_tempo", query(Song, "settings.tempo")),
    (
        "song.unfollow_tempo_automation",
        query(Song, "settings.tempo"),
    ),
    ("song.set_tempo_breakpoint", query(Song, "settings.tempo")),
    ("song.set_time_signature_change", query(Song, "settings.time_signatures")),
    ("song.set_time_signatures", query(Song, "settings.time_signatures")),
    ("stems.cancel", read("stems.status", "separation")),
    ("stems.install", read("stems.status", "install")),
    ("stems.separate", query(Song, "tracks")),
    ("take_lane.copy_to_main_lane", query(Track, "clips")),
    ("take_lane.create_clip", query(Track, "take_lanes")),
    ("take_lane.delete", query(Track, "take_lanes")),
    ("take_lane.delete_all", query(Track, "take_lanes")),
    ("take_lane.delete_unused", query(Track, "take_lanes")),
    ("take_lane.duplicate", query(Track, "take_lanes")),
    ("take_lane.insert", query(Track, "take_lanes")),
    ("take_lane.move", query(Track, "take_lanes")),
    ("take_lane.move_comp_edge", query(Track, "clips")),
    ("take_lane.rename", query(Track, "take_lanes")),
    ("take_lane.replace_with_take", query(Track, "clips")),
    ("take_lane.set_audition", query(Track, "audition")),
    ("track.add_take_lane", query(Track, "take_lanes")),
    ("track.bounce_in_place", query(Song, "tracks")),
    ("track.bounce_to_new_track", query(Song, "tracks")),
    ("track.create", query(Song, "tracks")),
    ("track.create_bounced_track", query(Song, "tracks")),
    ("track.delete", query(Song, "tracks")),
    ("track.cut", query(Song, "tracks")),
    ("track.duplicate", query(Song, "tracks")),
    ("track.duplicate_without_clips", query(Song, "tracks")),
    ("track.group", query(Song, "tracks")),
    ("track.move", query(Song, "tracks")),
    ("track.paste", query(Song, "tracks")),
    ("track.paste_bounced_audio", query(Track, "clips")),
    ("track.clear_frozen", query(Track, "frozen")),
    ("track.freeze", query(Track, "frozen")),
    ("track.set_frozen", query(Track, "frozen")),
    ("track.set_bounced", query(Song, "tracks")),
    ("track.unfreeze", query(Track, "frozen")),
    ("track.rename", query(Track, "name")),
    ("track.set_devices_enabled", query(Track, "devices")),
    ("track.ungroup", query(Song, "tracks")),
    ("track.set_tuning_bypass", query(Track, "tuning_bypass")),
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
