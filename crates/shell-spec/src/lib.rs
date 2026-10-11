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
//!
//! `cargo xtask` checks the commands, lists them in the command reference, and
//! builds the keymap from this crate, so none of its tasks links an audio
//! backend (which needs ALSA and JACK headers on Linux). `bl-shell`
//! re-exports everything here.

pub mod agent;
pub mod arrangement;
pub mod audio;
pub mod commands;
pub mod context_menus;
pub mod editing;
pub mod menus;
pub mod settings;

use bl_commands::{RegisterError, Registry};

/// Adds the shell's commands and the Arrangement View's (its view and its
/// editing commands) to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    commands::register(registry)?;
    arrangement::register(registry)?;
    editing::register(registry)
}
