//! The keymap test (the architecture, "The keymap"; build plan, "Every UI
//! package"): every shortcut in the UI parity reference is in the default
//! keymap with the reference's keys, and is bound to a registered command,
//! handled by the platform or the focused control, or marked with the
//! milestone (and for M1 the package) that binds it. The shortcuts of the
//! views the shell builds are bound.

mod device_view;
mod editors;
mod helpers;
mod reference;
mod resolve;
mod views;
