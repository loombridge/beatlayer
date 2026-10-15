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
pub mod arrangement;
pub mod audio;
pub mod browser;
pub mod clip_view;
pub mod devices;
pub mod export;
pub mod files;
pub mod midi_files;
pub mod mixer_controls;
pub mod mixer_view;
pub mod perception;
pub mod permission;
pub mod plugins;
pub mod routing;
pub mod settings;
mod shell;
pub mod tracks;
pub mod transport;

/// The Arrangement View's command IDs and their argument and result types.
pub use bl_shell_spec::arrangement as arrangement_commands;
/// The browser's command IDs and their argument and result types.
pub use bl_shell_spec::browser as browser_commands;
/// The Device View's command IDs and their argument and result types
/// (WP-2.4).
pub use bl_shell_spec::devices as device_commands;
/// The Arrangement's editing command IDs and their argument and result
/// types (WP-1.8b).
pub use bl_shell_spec::editing as editing_commands;
/// The export command's ID and its argument and result types (WP-1.12).
pub use bl_shell_spec::export as export_commands;
/// The Mixer view's command IDs and their argument and result types
/// (WP-1.9).
pub use bl_shell_spec::mixer_view as mixer_view_commands;
/// The Clip View's note-editing command IDs and their argument and result
/// types (WP-2.3).
pub use bl_shell_spec::notes as note_commands;
/// The perception commands' IDs and their argument and result types
/// (WP-1.16).
pub use bl_shell_spec::perception as perception_commands;
/// The plug-in command IDs and their argument and result types (WP-2.6).
pub use bl_shell_spec::plugins as plugin_commands;
pub use bl_shell_spec::settings::ZOOM_STEPS;
/// The shell's track command IDs and their argument and result types
/// (WP-2.19b).
pub use bl_shell_spec::tracks as track_commands;
/// The transport's command IDs the shell's menus and controls run, and
/// `transport.state`'s result.
pub use bl_shell_spec::transport as transport_commands;
pub use bl_shell_spec::{commands, context_menus, menus, register};
pub use shell::{
    Area, Effect, GesturePhase, Invocation, KEY_TRACK_NUMBER, MainView, Message, Shell, ShellError,
    WindowKind,
};
