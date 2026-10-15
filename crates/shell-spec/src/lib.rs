//! The app shell's commands and menu bar as data (WP-1.7): what `bl-shell`
//! runs and the app draws, without the audio host, files, or windows.
//!
//! - [`commands`]: the shell's commands in the command registry: files
//!   (new, open, save, quit), windows and views, keyboard focus, the
//!   Settings window, and the audio device, with their argument and result
//!   types.
//! - [`menus`]: the menu bar in Live's order, each item tied to the command
//!   it runs.
//! - [`audio`] and [`settings`]: the data those commands take and return.
//! - [`agent`]: the agent server's settings and commands (WP-1.15).
//! - [`arrangement`]: the Arrangement View's view commands (WP-1.8a): zoom,
//!   scroll, Follow, the editing grid, track heights, and the selection
//!   the zoom commands work from.
//! - [`editing`]: the Arrangement's editing commands (WP-1.8b): selecting,
//!   moving, copying, resizing, splitting, deleting, and duplicating clips,
//!   and the clipboard; [`context_menus`], the clip and time-selection
//!   menus that reach them.
//! - [`notes`]: the Clip View's note-editing commands (WP-2.3): selecting
//!   the MIDI Note Editor's notes, drawing, deleting, moving and resizing
//!   them.
//! - [`browser`]: the browser's commands (WP-1.10): places, browsing,
//!   search, preview, and adding files to the Arrangement.
//! - [`mixer_view`]: the Mixer view's view commands (WP-1.9): showing and
//!   hiding the mixer and its sections in both views, scrolling it, and
//!   moving the focus to it.
//! - [`devices`]: the Device View's commands (WP-2.4): what it shows,
//!   selecting and folding devices, adding them, the device clipboard,
//!   presets, and hot-swapping; [`context_menus`] holds the device title
//!   bar's menu.
//! - [`tracks`]: Duplicate Track Without Clips (WP-2.19b, IMP-12).
//! - [`transport`]: what the Control Bar's transport section shows
//!   (`transport.state`), and the IDs of the transport's commands, which
//!   `bl-engine` and `bl-model` register (WP-1.18).
//! - [`export`]: exporting audio (`export.audio`, WP-1.12).
//! - [`midi_files`]: importing and exporting Standard MIDI Files
//!   (`midi.import_file`, `midi.export_clip`, WP-2.21).
//! - [`perception`]: how agents hear and see the set (`perception.*`,
//!   WP-1.16): audio previews, analysis, and view captures.
//!
//! `cargo xtask` checks the commands, lists them in the command reference, and
//! builds the keymap from this crate, so none of its tasks links an audio
//! backend (which needs ALSA and JACK headers on Linux). `bl-shell`
//! re-exports everything here.

pub mod agent;
pub mod arrangement;
pub mod audio;
pub mod browser;
pub mod commands;
pub mod context_menus;
pub mod devices;
pub mod editing;
pub mod export;
pub mod menus;
pub mod midi_files;
pub mod mixer_view;
pub mod notes;
pub mod perception;
pub mod plugins;
pub mod session_view;
pub mod settings;
pub mod tracks;
pub mod transport;

use bl_commands::{RegisterError, Registry};

/// Adds the shell's commands, the Arrangement View's (its view and its
/// editing commands), the transport query, the Mixer view's, the browser's,
/// the export command, and the perception commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    commands::register(registry)?;
    arrangement::register(registry)?;
    editing::register(registry)?;
    notes::register(registry)?;
    transport::register(registry)?;
    mixer_view::register(registry)?;
    browser::register(registry)?;
    export::register(registry)?;
    midi_files::register(registry)?;
    devices::register(registry)?;
    tracks::register(registry)?;
    session_view::register(registry)?;
    plugins::register(registry)?;
    perception::register(registry)
}
