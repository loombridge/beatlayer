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
//!
//! `cargo xtask` checks the commands, lists them in `docs/commands.md`, and
//! builds the keymap from this crate, so none of its tasks links an audio
//! backend (which needs ALSA and JACK headers on Linux). `bl-shell`
//! re-exports everything here.

pub mod agent;
pub mod audio;
pub mod commands;
pub mod menus;
pub mod settings;

pub use commands::register;
