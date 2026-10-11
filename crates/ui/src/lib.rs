//! Views, widgets, GPU canvases, theming, and accessibility.
//!
//! The app shell's UI (WP-1.7), drawn with egui (ADR-001):
//!
//! - [`layout`]: the layout layer, which turns the UI parity reference's
//!   rows into rectangles, so placement isn't scattered through the code.
//! - [`theme`]: Beatlayer's colours, sizes, and bundled typeface, as tokens
//!   from theme files, dark and light.
//! - [`keymap`] and [`input`]: the default keymap (`keymaps/live12.toml`,
//!   generated from the parity reference) and the dispatcher that turns key
//!   presses into commands for the contexts that apply.
//! - [`main_window`], [`settings_window`], [`menus`], [`widgets`]: the
//!   windows and controls. Every control has a role, a name, and a value
//!   for assistive technology through AccessKit.
//!
//! Nothing here owns a window or an event loop: the app (`beatlayer`)
//! drives these from its winit loop, and tests drive them headless with an
//! `egui::Context` and synthetic input.

pub mod arrangement;
pub mod input;
pub mod keymap;
pub mod layout;
pub mod main_window;
pub mod menus;
pub mod settings_window;
pub mod theme;
pub mod widgets;
