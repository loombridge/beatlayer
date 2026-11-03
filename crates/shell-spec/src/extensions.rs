//! The extension commands (WP-9.29; the extensions spec, R6, R7 and R16):
//! installing, switching on and off, and removing extensions, and reading
//! what is installed.
//!
//! [`register`] adds the commands' specs; `bl-shell` runs them through
//! `bl-ext`, which `xtask` never links.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `extension.install`: install an extension package.
pub const INSTALL: &str = "extension.install";
/// `extension.remove`: remove an extension.
pub const REMOVE: &str = "extension.remove";
/// `extension.set_enabled`: switch an extension on or off.
pub const SET_ENABLED: &str = "extension.set_enabled";
/// `extension.list`: the installed extensions.
pub const LIST: &str = "extension.list";
/// `extension.describe`: one extension's manifest and components.
pub const DESCRIBE: &str = "extension.describe";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[INSTALL, REMOVE, SET_ENABLED, LIST, DESCRIBE];

/// Whether `id` is one of the extension commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `extension.install`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallArgs {
    /// The path of the `.blext` package to install.
    pub path: String,
}

/// Arguments of `extension.remove` and `extension.describe`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    /// The extension's ID, as `extension.list` gives it.
    pub id: String,
}

/// Arguments of `extension.set_enabled`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetEnabledArgs {
    /// The extension's ID, as `extension.list` gives it.
    pub id: String,
    /// Whether it is on.
    pub enabled: bool,
}

/// An installed extension, as `extension.list` gives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtensionRow {
    /// Its ID, which the other commands take.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// What its components do, once each: `midi-tool`, `control-surface`,
    /// `audio-effect`, `instrument` or `midi-effect`.
    pub kinds: Vec<String>,
    /// Whether it is on.
    pub enabled: bool,
    /// Why a component of it was stopped, if one was (R10 to R12): kept
    /// until the extension is switched on again or Beatlayer restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
}

/// What `extension.list` and the commands that change the list return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtensionList {
    /// The User Library folder the extensions are installed in.
    pub folder: String,
    /// The installed extensions, by ID.
    pub extensions: Vec<ExtensionRow>,
}

/// A component of an extension, as its manifest gives it (R1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComponentRow {
    /// Its file in the extension's folder.
    pub file: String,
    /// What it does.
    pub kind: String,
    /// The version of the `beatlayer:ext` world it targets.
    pub world: String,
    /// The imports it asks for.
    pub imports: Vec<String>,
    /// The memory its instances reserve up front, in MiB.
    pub memory_mib: u32,
}

/// What `extension.describe` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtensionDescription {
    /// Its ID.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Who made it.
    pub author: String,
    /// The licence it's under.
    pub licence: String,
    /// What it does.
    pub description: String,
    /// Whether it is on.
    pub enabled: bool,
    /// Why a component of it was stopped, if one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    /// Its components.
    pub components: Vec<ComponentRow>,
    /// The registered commands it may call, each with its permission class
    /// (R44). None until extensions reach the set (R38).
    pub calls: Vec<String>,
}

/// Adds the extension commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(INSTALL, "Install Extension")
            .description(
                "Installs the extension package at `path` (a `.blext` file: a ZIP holding \
                 `extension.toml` and its WebAssembly components) into the User Library's \
                 `Extensions` folder, switched on. The whole package is checked first and \
                 nothing is kept if any check fails: an unsafe path, a manifest that doesn't \
                 parse or lacks a field, a world version this Beatlayer doesn't accept, a \
                 component that is missing, isn't valid, doesn't export its kind's world or \
                 imports more than its world gives it, or an extension of the same ID at an \
                 equal or later version. The reason comes back as the error. A later version \
                 of an installed extension replaces it. Returns the installed extensions. It \
                 is `external` because it reads a file outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<InstallArgs>()
            .result::<ExtensionList>(),
    )?;
    registry.register(
        CommandSpec::new(REMOVE, "Remove Extension")
            .description(
                "Removes the extension `id` (`extension.list` gives the IDs): its folder in the \
                 User Library and its compiled code. Returns the installed extensions.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<IdArgs>()
            .result::<ExtensionList>(),
    )?;
    registry.register(
        CommandSpec::new(SET_ENABLED, "Switch Extension On or Off")
            .description(
                "Switches the extension `id` on or off. The choice is kept in the preferences. \
                 Switching an extension on also clears the failure that stopped it. Returns the \
                 installed extensions.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetEnabledArgs>()
            .result::<ExtensionList>(),
    )?;
    registry.register(
        CommandSpec::new(LIST, "Extensions")
            .description(
                "Reads each installed extension: its ID, name and version, what its components \
                 do, whether it is on, and why a component of it was stopped, if one was; and \
                 the User Library folder they are installed in.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ExtensionList>(),
    )?;
    registry.register(
        CommandSpec::new(DESCRIBE, "Describe Extension")
            .description(
                "Reads the extension `id`: its manifest (name, version, author, licence and \
                 description), each component with its kind, world version, imports and \
                 memory, whether it is on and its last failure, and the commands it may call.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<IdArgs>()
            .result::<ExtensionDescription>(),
    )
}
