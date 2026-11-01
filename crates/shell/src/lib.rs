//! The app shell's headless half (WP-1.7): everything the Beatlayer window
//! does that doesn't draw.
//!
//! - [`commands`] and [`menus`]: the shell's commands in the command
//!   registry and the menu bar in Live's order, from `bl-shell-spec`, which
//!   holds them as data so `cargo xtask` can read them without linking an
//!   audio host.
//! - [`Shell`]: the shell's state (the document, the set's file, the view,
//!   the preferences, the audio device) and the one place every command
//!   runs, the document's included. A person's menu choice or key press
//!   goes through [`Shell::invoke`]; an agent or a test through
//!   [`Shell::run`]. Things only the platform can do (dialogs, quitting,
//!   full screen) come back as [`Effect`]s.
//! - [`audio`]: the audio host that applies device, rate, buffer, and
//!   channel changes without a restart, the test tone, and the CPU meter.
//! - [`settings`]: preferences and where they're kept.
//! - [`files`]: set files, through `bl-project`, and the factory default
//!   set.
//! - [`agent`]: the agent server's settings and token (WP-1.15); the app
//!   runs the server itself, from `bl-agent`.
//! - [`arrangement`]: what the Arrangement View shows (zoom, scroll, Follow,
//!   the editing grid, track heights, the insert marker and selection),
//!   changed by its view commands (WP-1.8a), and its edits: selecting,
//!   moving, copying, resizing, splitting, deleting, duplicating, and the
//!   clipboard ([`arrangement::editing`], WP-1.8b), which gestures run as
//!   one undo step through [`Shell::gesture`].
//! - [`browser`]: the browser (places, browsing, search, preview, adding
//!   files to the Arrangement), changed by its commands (WP-1.10).
//! - [`devices`]: the Device View (WP-2.4): the selected device, folded
//!   devices, the device clipboard, hot-swapping, and the `device.*`
//!   commands that run on the shell (adding a device as a person does,
//!   cut, copy, paste, duplicate, and presets).
//! - [`export`]: exporting audio (WP-1.12): a range rendered offline
//!   through the engine and written to a WAV, AIFF, or FLAC file, which
//!   `export.audio` runs.
//! - [`mixer_view`] and [`mixer_controls`]: what the mixers show in the
//!   Session and Arrangement Views (shown, scrolled, which sections),
//!   changed by the `mixer_view.*` commands, and what a strip's controls
//!   show and the `mixer.*` commands each change runs (WP-1.9).
//! - [`midi`]: the MIDI ports, their switches, and the computer MIDI
//!   keyboard (WP-2.2), which the `midi.*` commands and the Settings
//!   window's MIDI ports table run.
//! - [`perception`]: audio previews of a range or a track, rendered
//!   through the export's offline render, and their analysis, which the
//!   `perception.*` commands run for the agent API (WP-1.16).
//! - [`permission`]: a call's permission class decided from its arguments,
//!   for the agent API: placing clips over others is destructive
//!   (WP-1.15b).
//! - [`plugins`]: plug-in devices (WP-2.6): the `plugin.*` commands'
//!   reports, plug-ins' state taken into the set before a save or a copy,
//!   and the changes plug-ins make themselves as undo steps. The CLAP host
//!   itself is `bl-plugin-host`'s; [`Shell::poll`] runs its main-thread
//!   work.
//! - [`search`]: the search field (WP-6.42, IMP-16 and IMP-18): command search
//!   and track search, which the `search.*` commands drive.
//! - [`tracks`]: Duplicate Track Without Clips (WP-2.19b, IMP-12), which
//!   `track.duplicate_without_clips` runs.
//! - [`transport`]: the Control Bar's transport section: what its fields
//!   show, and the command each change runs (WP-1.18). The transport's
//!   commands themselves are `bl-engine`'s, which [`Shell`] registers and
//!   sends to the engine the audio host runs.
//!
//! The UI (`bl-ui`) draws the shell and the app (`beatlayer`) wires it to
//! windows, native menus, and file dialogs. Nothing here depends on a UI
//! crate, so every command and setting can be driven and tested headless.

pub mod agent;
pub mod analysed_media;
pub mod arrangement;
pub mod assistant;
pub mod audio;
pub mod auto_warp;
mod automation_record;
pub mod autosample;
pub mod browser;
mod chord_learn;
pub mod clip_defaults;
pub mod clip_drag;
pub mod clip_envelope_edit;
pub mod clip_files;
pub mod clip_view;
pub mod colours;
pub mod dawproject;
pub mod detail;
pub mod devices;
pub mod export;
pub mod export_dialog;
pub mod files;
pub mod link;
pub mod midi;
pub mod midi_files;
pub mod midi_tools;
pub mod mixer_controls;
pub mod mixer_view;
mod modulation;
pub mod multi_clip;
pub mod note_filters;
pub mod perception;
pub mod permission;
pub mod placeholders;
pub mod plugins;
pub mod processed;
pub mod realtime;
pub mod record_quantize;
pub mod recording;
pub mod resources;
pub mod routing;
pub mod sample_editor;
pub mod scales;
pub mod scene_fields;
pub mod search;
mod session_capture;
pub mod session_clips;
pub mod session_view;
pub mod set_exchange;
pub mod settings;
mod shell;
pub mod stems;
pub mod tap_tempo;
pub mod tempo_follower;
#[doc(hidden)]
pub mod testing;
pub mod tracks;
pub mod transport;
/// Moving a file to the system's trash (WP-6.63).
pub mod trash;

/// The Arrangement View's command IDs and their argument and result types.
pub use bl_shell_spec::arrangement as arrangement_commands;
/// The Audio Utilities panel's command IDs and their argument and result
/// types (WP-4.10b).
pub use bl_shell_spec::audio_panel as audio_panel_commands;
/// The automation editing command IDs and their argument and result types
/// (WP-6.25).
pub use bl_shell_spec::automation as automation_commands;
/// The autosampler's command IDs and its argument and result types
/// (WP-5.16).
pub use bl_shell_spec::autosampler as autosampler_commands;
/// The browser's command IDs and their argument and result types.
pub use bl_shell_spec::browser as browser_commands;
/// Capture MIDI's command ID and its result types (WP-3.14).
pub use bl_shell_spec::capture_midi as capture_midi_commands;
/// The Envelope Editor's editing command IDs and their argument and result
/// types (WP-6.25c).
pub use bl_shell_spec::clip_envelope_editing as clip_envelope_commands;
pub use bl_shell_spec::clip_properties as clip_properties_commands;
/// The Control Surfaces table's command IDs and their argument and result
/// types (WP-7.5).
pub use bl_shell_spec::control_surface as control_surface_commands;
/// The DAWproject commands' IDs and their argument and result types
/// (WP-6.36, WP-6.37).
pub use bl_shell_spec::dawproject as dawproject_commands;
/// The Device View's command IDs and their argument and result types
/// (WP-2.4).
pub use bl_shell_spec::devices as device_commands;
/// The Arrangement's editing command IDs and their argument and result
/// types (WP-1.8b).
pub use bl_shell_spec::editing as editing_commands;
/// The Link commands' IDs and their argument and result types (WP-7.14,
/// WP-7.15).
pub use bl_shell_spec::link as link_commands;
/// The Tempo Follower commands' IDs and their argument and result types
/// (WP-9.13).
pub use bl_shell_spec::tempo_follower as tempo_follower_commands;

/// The envelope keys' command IDs and their argument and result types
/// (WP-6.26).
pub use bl_shell_spec::envelope_keys as envelope_commands;
/// The export command's ID and its argument and result types (WP-1.12).
pub use bl_shell_spec::export as export_commands;
/// The File Manager's command IDs and their argument and result types
/// (WP-6.63).
pub use bl_shell_spec::file_manager as file_manager_commands;
/// The grooves' commands that need the shell, and the Groove Pool's view
/// (WP-6.19, WP-6.20), for the UI to run by ID.
pub use bl_shell_spec::groove as groove_commands;
/// The Arrangement's locator command IDs and their argument and result
/// types (WP-6.24).
pub use bl_shell_spec::locator as locator_commands;
/// The MIDI file commands' IDs and their argument and result types
/// (WP-2.21).
pub use bl_shell_spec::midi_files as midi_file_commands;
/// The MIDI ports' and the computer MIDI keyboard's command IDs and their
/// argument and result types (WP-2.2).
pub use bl_shell_spec::midi_io as midi_io_commands;
/// The MIDI Tools' command IDs and their argument and result types
/// (WP-6.11).
pub use bl_shell_spec::midi_tools as midi_tool_commands;
/// The Mixer view's command IDs and their argument and result types
/// (WP-1.9).
pub use bl_shell_spec::mixer_view as mixer_view_commands;
/// The multisampler's commands that run on the shell (WP-5.13).
pub use bl_shell_spec::multisampler as multisampler_commands;
/// Find and Select Notes' filters and the arguments of
/// `clip_view.note_filters` (WP-6.4).
pub use bl_shell_spec::note_filters as note_filter_commands;
/// The Clip View's note-editing command IDs and their argument and result
/// types (WP-2.3).
pub use bl_shell_spec::notes as note_commands;
/// A drum rack's pad view and the commands on pads (WP-5.6).
pub use bl_shell_spec::pads as pad_commands;
/// The perception commands' IDs and their argument and result types
/// (WP-1.16).
pub use bl_shell_spec::perception as perception_commands;
/// The plug-in command IDs and their argument and result types (WP-2.6).
pub use bl_shell_spec::plugins as plugin_commands;
pub use bl_shell_spec::remote as remote_commands;
/// The Sample Editor's view command IDs and their argument and result types
/// (WP-4.10).
pub use bl_shell_spec::sample_editor as sample_editor_commands;
/// The scale commands' IDs and the scale list's result (WP-6.7).
pub use bl_shell_spec::scales as scales_commands;
/// The search field's command IDs and their argument and result types
/// (WP-6.42).
pub use bl_shell_spec::search as search_commands;
/// The Session View's launch quantisation command IDs and their argument and
/// result types (WP-3.2).
pub use bl_shell_spec::session_launch as session_launch_commands;
/// The Session View grid's command IDs and their argument and result types
/// (WP-3.7).
pub use bl_shell_spec::session_view as session_view_commands;
/// The set exchange commands' IDs and their argument and result types
/// (WP-6.35).
pub use bl_shell_spec::set_exchange as set_exchange_commands;
pub use bl_shell_spec::settings::ZOOM_STEPS;
pub use bl_shell_spec::slice as slice_commands;
/// The shell's track command IDs and their argument and result types
/// (WP-2.19b).
/// Stem separation's command IDs and arguments (IMP-11).
pub use bl_shell_spec::stems as stems_commands;
pub use bl_shell_spec::tracks as track_commands;
/// The transport's command IDs the shell's menus and controls run, and
/// `transport.state`'s result.
pub use bl_shell_spec::transport as transport_commands;
pub use bl_shell_spec::tunings as tuning_commands;
pub use bl_shell_spec::{
    audio_editing, bounce, commands, context_menus, freeze, keymap, menus, register,
};
/// The autosampler's progress (WP-5.16).
pub use shell::autosample::Status as AutosampleStatus;
pub use shell::{
    Area, Effect, ExportPrefs, GesturePhase, InfoTarget, Invocation, KEY_TRACK_NUMBER, MainView,
    Message, Shell, ShellError, TransientList, WindowKind,
};
