//! The app shell's headless half (WP-1.7): everything the Beatlayer window
//! does that doesn't draw.
//!
//! - [`commands`]: the shell's commands in the command registry: files
//!   (new, open, save, quit), windows and views, keyboard focus, the
//!   Settings window, and the audio device. Their specs are pure data, so
//!   `cargo xtask` checks them and lists them in `docs/commands.md`.
//! - [`Shell`]: the shell's state (the document, the set's file, the view,
//!   the preferences, the audio device) and the one place every command
//!   runs, the document's included. A person's menu choice or key press
//!   goes through [`Shell::invoke`]; an agent or a test through
//!   [`Shell::run`]. Things only the platform can do (dialogs, quitting,
//!   full screen) come back as [`Effect`]s.
//! - [`audio`]: the audio host that applies device, rate, buffer, and
//!   channel changes without a restart, and the CPU meter.
//! - [`menus`]: the menu bar as data, in Live's order.
//! - [`settings`]: preferences and where they're kept.
//! - [`files`]: set files and the factory default set.
//!
//! The UI (`bl-ui`) draws the shell and the app (`beatlayer`) wires it to
//! windows, native menus, and file dialogs. Nothing here depends on a UI
//! crate, so every command and setting can be driven and tested headless.

pub mod audio;
pub mod commands;
pub mod files;
pub mod menus;
pub mod settings;
mod shell;

pub use commands::register;
pub use shell::{
    Area, Effect, Invocation, MainView, Message, Shell, ShellError, WindowKind, ZOOM_STEPS,
};
