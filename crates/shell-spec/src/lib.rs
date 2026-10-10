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
//! - [`arrangement`]: the Arrangement View's view commands (WP-1.8a): zoom,
//!   scroll, Follow, the editing grid, track heights, and the selection
//!   the zoom commands work from.
//! - [`browser`]: the browser's commands (WP-1.10): places, browsing,
//!   search, preview, and adding files to the Arrangement.
//!
//! `cargo xtask` checks the commands, lists them in `docs/commands.md`, and
//! builds the keymap from this crate, so none of its tasks links an audio
//! backend (which needs ALSA and JACK headers on Linux). `bl-shell`
//! re-exports everything here.

pub mod arrangement;
pub mod audio;
pub mod browser;
pub mod commands;
pub mod menus;
pub mod settings;

use bl_commands::{RegisterError, Registry};

/// Adds the shell's commands, the Arrangement View's, and the browser's to a
/// registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    commands::register(registry)?;
    arrangement::register(registry)?;
    browser::register(registry)
}
