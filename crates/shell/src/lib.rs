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
//! - [`arrangement`]: what the Arrangement View shows (zoom, scroll, Follow,
//!   the editing grid, track heights, the insert marker and selection),
//!   changed by its view commands (WP-1.8a), and its edits: selecting,
//!   moving, copying, resizing, splitting, deleting, duplicating, and the
//!   clipboard ([`arrangement::editing`], WP-1.8b), which gestures run as
//!   one undo step through [`Shell::gesture`].
//!
//! The UI (`bl-ui`) draws the shell and the app (`beatlayer`) wires it to
//! windows, native menus, and file dialogs. Nothing here depends on a UI
//! crate, so every command and setting can be driven and tested headless.

pub mod arrangement;
pub mod audio;
pub mod files;
pub mod settings;
mod shell;

/// The Arrangement View's command IDs and their argument and result types.
pub use bl_shell_spec::arrangement as arrangement_commands;
/// The Arrangement's editing command IDs and their argument and result
/// types (WP-1.8b).
pub use bl_shell_spec::editing as editing_commands;
pub use bl_shell_spec::{commands, context_menus, menus, register};
pub use shell::{
    Area, Effect, GesturePhase, Invocation, MainView, Message, Shell, ShellError, WindowKind,
    ZOOM_STEPS,
};
