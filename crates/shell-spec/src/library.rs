//! Beatlayer's library (WP-9.46; the core library spec, R5 to R14): its
//! packages, downloaded through the Packages manager, and the credits of
//! the items they hold.
//!
//! [`register`] adds the commands' specs; `bl-shell` runs them.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::packages::PackageStatus;
use crate::stems::JobStatus;

/// `library.state`: the library's packages and how the download is going.
pub const STATE: &str = "library.state";
/// `library.install`: download the library's packages.
pub const INSTALL: &str = "library.install";
/// `library.cancel`: stop the library's download.
pub const CANCEL: &str = "library.cancel";
/// `library.credits`: the credits of the installed items, or of a set's.
pub const CREDITS: &str = "library.credits";
/// `library.export_credits`: write the credits to a text file.
pub const EXPORT_CREDITS: &str = "library.export_credits";
/// `library.show_credits`: open the Library Credits window.
pub const SHOW_CREDITS: &str = "library.show_credits";
/// `library.close_credits`: close the Library Credits window.
pub const CLOSE_CREDITS: &str = "library.close_credits";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    STATE,
    INSTALL,
    CANCEL,
    CREDITS,
    EXPORT_CREDITS,
    SHOW_CREDITS,
    CLOSE_CREDITS,
];

/// Whether `id` is one of the library's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What `library.state` and the commands that change it return.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LibraryState {
    /// The folder the installed packages' items are in, read-only.
    pub folder: String,
    /// Whether any package is installed: the browser's Core Library label
    /// lists items once it is, and shows the download page before.
    pub installed: bool,
    /// The library's packages for this platform, installed or not, each
    /// with its item count (`items`).
    pub packages: Vec<PackageStatus>,
    /// How the download is going.
    pub install: JobStatus,
    /// Whether the Library Credits window is open.
    pub credits_open: bool,
}

/// Arguments of `library.credits`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreditsArgs {
    /// Give only the credits of the library items the open set uses.
    #[serde(default)]
    pub set: bool,
}

/// One item's credit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Credit {
    /// The item's ID in the library.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its credit: title, creator, licence and its link, source and changes.
    pub credit: String,
}

/// The credits of one installed package's items.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageCredits {
    /// The package's ID in the library.
    pub package: String,
    /// Its name.
    pub name: String,
    /// Its items' credits, in ID order.
    pub items: Vec<Credit>,
}

/// What `library.credits` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Credits {
    /// The credits by package.
    pub packages: Vec<PackageCredits>,
    /// The same, as the text Export Credits writes.
    pub text: String,
}

/// Arguments of `library.export_credits`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportCreditsArgs {
    /// The text file to write; a person's menu choice leaves it out and
    /// asks where.
    #[serde(default)]
    pub path: String,
    /// Write only the credits of the library items the open set uses.
    #[serde(default)]
    pub set: bool,
}

/// What `library.export_credits` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportedCredits {
    /// The file written.
    pub path: String,
    /// How many items it credits.
    pub items: u32,
}

/// Adds the library's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Library")
            .description(
                "Reads Beatlayer's own sound library: its packages for this platform (Core, \
                 Instruments, Drums, Loops), each installed or not with its size, its item \
                 count and the share downloaded, the folder they install in, how the download \
                 is going, and whether the Library Credits window is open.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<LibraryState>(),
    )?;
    registry.register(
        CommandSpec::new(INSTALL, "Download Required Packages Now")
            .description(
                "Downloads and installs the library's packages through the Packages manager, \
                 as `packages.install` does for the ones that aren't installed. Returns at \
                 once; `library.state` reports the progress. Changes nothing in the set.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<NoArgs>()
            .result::<LibraryState>(),
    )?;
    registry.register(
        CommandSpec::new(CANCEL, "Cancel")
            .description(
                "Stops the library's download and takes its packages off the queue. If the \
                 library isn't installed and the Browser shows its page, the Browser goes \
                 back to All. Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<LibraryState>(),
    )?;
    registry.register(
        CommandSpec::new(CREDITS, "Library Credits")
            .description(
                "Reads the credit of each installed library item (its title, creator, licence \
                 with its link, source and changes made), by package, and the same as the \
                 text Export Credits writes. With `set`, only the library items the open set \
                 uses.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<CreditsArgs>()
            .result::<Credits>(),
    )?;
    registry.register(
        CommandSpec::new(EXPORT_CREDITS, "Export Credits...")
            .description(
                "Writes the credits `library.credits` gives to a text file, with `set` those of \
                 the library items the open set uses. Without a `path` a person is asked \
                 where. Writes a file outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<ExportCreditsArgs>()
            .result::<ExportedCredits>(),
    )?;
    registry.register(
        CommandSpec::new(SHOW_CREDITS, "Library Credits...")
            .description(
                "Opens the Library Credits window, which lists every installed item's credit \
                 by package. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<LibraryState>(),
    )?;
    registry.register(
        CommandSpec::new(CLOSE_CREDITS, "Close Library Credits")
            .description("Closes the Library Credits window. Changes nothing in the set.")
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<LibraryState>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
