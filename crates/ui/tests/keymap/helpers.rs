//! The repository path, registry and modifier helpers the keymap tests share.

use bl_commands::Registry;
use bl_ui::keymap::Mods;
use std::path::PathBuf;

pub(crate) fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub(crate) fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    bl_shell::register(&mut registry).unwrap();
    // The transport's commands (WP-1.5), which the playback keys bind.
    bl_engine::transport::register(&mut registry).unwrap();
    // The Session View's launching and stopping (WP-3.2), which its grid
    // runs and its keys reach.
    bl_engine::session::register(&mut registry).unwrap();
    registry
}

pub(crate) fn mods(cmd: bool, ctrl: bool, alt: bool, shift: bool) -> Mods {
    Mods {
        ctrl,
        alt,
        shift,
        cmd,
    }
}
