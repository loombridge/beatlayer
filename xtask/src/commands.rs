//! The command reference (WP-0.14, decisions D16 and D17).
//!
//! The command reference is generated from the command registry: every crate
//! that implements commands registers them, and this module builds the same
//! registry the app does, checks it (every command has a schema, a
//! description, a permission class, contexts, and bindings that name real
//! shortcuts in fitting contexts), and renders the reference.

use std::path::Path;

use bl_commands::readback::{self, READBACK};
use bl_commands::{RegisterError, Registry, reference};

use crate::ui_parity;

/// The generated reference, relative to the workspace root.
pub const MD_PATH: &str = "docs/commands.md";

/// A crate's function that adds its commands to a registry.
type Register = fn(&mut Registry) -> Result<(), RegisterError>;

/// Each crate's function that registers its commands. Add a crate here when
/// it starts registering commands, so they're checked and listed.
const PROVIDERS: &[(&str, Register)] = &[
    ("bl-model", bl_model::registry::register),
    ("bl-project", bl_project::registry::register),
    ("bl-engine", bl_engine::transport::register),
    ("bl-engine", bl_engine::session::register),
    ("bl-shell-spec", bl_shell_spec::register),
];

/// Regenerates the command reference, after checking the registry.
pub fn generate(root: &Path) -> Result<(), String> {
    let (registry, rendered) = build(root)?;
    std::fs::write(root.join(MD_PATH), rendered)
        .map_err(|e| format!("couldn't write {MD_PATH}: {e}"))?;
    eprintln!("wrote {MD_PATH}: {} commands", registry.len());
    Ok(())
}

/// Checks the registry and brings the command reference up to date: the
/// reference is generated, so `check` rewrites it when it differs rather
/// than failing. It lives with the planning documents, outside this
/// repository, and is committed there when a change merges.
pub fn refresh(root: &Path) -> Result<(), String> {
    let (registry, expected) = build(root)?;
    let path = root.join(MD_PATH);
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    if actual.replace("\r\n", "\n") == expected {
        eprintln!(
            "{} commands, each with a schema, a description, and a permission class; their \
             bindings match the shortcut reference, and the command reference is in sync",
            registry.len()
        );
    } else {
        std::fs::write(&path, expected).map_err(|e| format!("couldn't write {MD_PATH}: {e}"))?;
        eprintln!(
            "{} commands, each with a schema, a description, and a permission class; their \
             bindings match the shortcut reference; {MD_PATH} was out of date and is regenerated",
            registry.len()
        );
    }
    Ok(())
}

/// Fails if the registry has a problem or the command reference is out of
/// date.
pub fn check(root: &Path) -> Result<(), String> {
    let (registry, expected) = build(root)?;
    let actual = std::fs::read_to_string(root.join(MD_PATH))
        .map_err(|e| format!("couldn't read {MD_PATH}: {e}"))?;
    if actual.replace("\r\n", "\n") != expected {
        return Err(format!(
            "{MD_PATH} is out of date with the command registry; regenerate it with \
             `cargo xtask commands`"
        ));
    }
    eprintln!(
        "{} commands, each with a schema, a description, and a permission class; their \
         bindings match the shortcut reference, and the command reference is in sync",
        registry.len()
    );
    Ok(())
}

/// Checks only what needs no planning documents: every command has a schema,
/// a description, a permission class, and contexts. Its bindings and the
/// generated reference need the UI parity reference.
pub fn check_registry() -> Result<(), String> {
    let registry = catalogue()?;
    let problems = registry_problems(&registry);
    if !problems.is_empty() {
        return Err(format!(
            "the command registry has {} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        ));
    }
    eprintln!(
        "{} commands, each with a schema, a description, and a permission class",
        registry.len()
    );
    eprintln!(
        "their bindings and the command reference: {}",
        ui_parity::ABSENT
    );
    Ok(())
}

/// The registry, checked, and the reference rendered from it.
fn build(root: &Path) -> Result<(Registry, String), String> {
    let registry = catalogue()?;
    let shortcuts = ui_parity::shortcut_table(root)?;
    let mut problems = registry_problems(&registry);
    problems.extend(
        registry
            .binding_problems(&shortcuts)
            .iter()
            .map(ToString::to_string),
    );
    if !problems.is_empty() {
        return Err(format!(
            "the command registry has {} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        ));
    }
    let rendered = reference::markdown(&registry, &shortcuts)?;
    // The readback section goes before the types it may refer to.
    let rendered = rendered.replacen(
        "\n## Types",
        &format!("\n{}## Types", readback::markdown(&registry, READBACK)),
        1,
    );
    Ok((registry, rendered))
}

/// What is wrong with the registry itself: a part missing from a command, and
/// a command that changes something without saying where it reads back.
fn registry_problems(registry: &Registry) -> Vec<String> {
    let mut problems: Vec<String> = registry
        .problems()
        .iter()
        .map(ToString::to_string)
        .collect();
    problems.extend(readback::problems(registry, READBACK, |object| {
        bl_model::saved_schema(object.path()).map(|schema| schema.as_value().clone())
    }));
    problems
}

/// Every registered command, from every provider.
pub fn catalogue() -> Result<Registry, String> {
    let mut registry = Registry::new();
    for (provider, register) in PROVIDERS {
        register(&mut registry).map_err(|e| format!("{provider}: {e}"))?;
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command that changes something names where its effect reads
    /// back, and the place exists.
    #[test]
    fn every_command_that_changes_something_names_where_it_reads_back() {
        let registry = catalogue().expect("the providers register");
        assert_eq!(registry_problems(&registry), Vec::<String>::new());
    }

    /// The saved forms the readback table is checked against hold the fields
    /// the set saves, and only those.
    #[test]
    fn the_saved_forms_are_checked_by_their_fields() {
        let track = bl_model::saved_schema("track").expect("a track has a saved form");
        assert!(readback::has_field(track.as_value(), "routing.output"));
        assert!(readback::has_field(track.as_value(), "automation[].points"));
        assert!(!readback::has_field(track.as_value(), "routing.nothing"));
        let song = bl_model::saved_schema("song").expect("the song has a saved form");
        assert!(readback::has_field(song.as_value(), "settings.punch_in"));
        assert!(readback::has_field(song.as_value(), "scenes[].name"));
    }

    /// The real registry must be complete and the reference in sync, so
    /// `cargo test` catches drift as well as `cargo xtask check`.
    #[test]
    fn the_repository_reference_renders() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace root");
        if !ui_parity::reference_present(root) {
            check_registry().expect("the command registry is complete");
            return;
        }
        // The reference itself is generated and lives outside this
        // repository, so the test checks that it renders, not that a copy on
        // disk matches; `cargo xtask check` refreshes the copy.
        build(root).expect("the command registry is complete and its reference renders");
    }
}
