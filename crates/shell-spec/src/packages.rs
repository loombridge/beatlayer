//! The Packages manager (WP-9.1; the stem separation spec, R15 to R21): the
//! on-device models and runtimes every feature downloads on first use, from
//! one manifest built into Beatlayer.
//!
//! [`register`] adds the commands' specs; `bl-shell` runs them. A feature's
//! own commands (`stems.install`) run [`INSTALL`] for the packages the
//! feature needs.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `packages.list`: each package of the manifest for this platform.
pub const LIST: &str = "packages.list";
/// `packages.install`: download and install packages by ID.
pub const INSTALL: &str = "packages.install";
/// `packages.remove`: remove installed packages by ID.
pub const REMOVE: &str = "packages.remove";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[LIST, INSTALL, REMOVE];

/// Whether `id` is one of the Packages manager's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Where a package is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    /// Not on this computer.
    #[default]
    NotInstalled,
    /// Waiting for the download before it.
    Queued,
    /// Downloading: see `progress`.
    Downloading,
    /// Installed.
    Installed,
    /// Removed, but its library is loaded: its files go the next time
    /// Beatlayer starts.
    RemovedAtNextLaunch,
}

/// A package of the manifest, as `packages.list` and `stems.status` give it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PackageStatus {
    /// Its ID, which `packages.install` and `packages.remove` take.
    pub id: String,
    /// Its name.
    pub name: String,
    /// What it does.
    pub purpose: String,
    /// Its version.
    pub version: String,
    /// Its project's page.
    pub url: String,
    /// The download's size in bytes.
    pub size: u64,
    /// Whether it's installed.
    pub installed: bool,
    /// The role it fills: features share a package by role.
    pub role: String,
    /// Where it is.
    pub state: PackageState,
    /// While it downloads, how far it is, from 0 to 1.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub progress: f32,
    /// Installed, but a library in its role is already loaded, so it takes
    /// effect the next time Beatlayer starts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub at_next_launch: bool,
    /// The features that need it, by ID.
    pub features: Vec<String>,
}

fn is_zero(x: &f32) -> bool {
    *x == 0.0
}

/// What `packages.list` and the other commands return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PackagesStatus {
    /// Where the packages are installed.
    pub folder: String,
    /// The packages of the manifest for this platform.
    pub packages: Vec<PackageStatus>,
}

/// Arguments of `packages.install` and `packages.remove`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdsArgs {
    /// The packages' IDs, as `packages.list` gives them.
    pub ids: Vec<String>,
}

/// Adds the Packages manager's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(LIST, "Packages")
            .description(
                "Reads each package of Beatlayer's manifest for this platform (the on-device \
                 models and runtimes its features download on first use): its ID, name, \
                 version, project page, what it does, download size and role, whether it's \
                 installed, queued, downloading (with its share) or removed at the next \
                 launch, and the features that need it; and the folder they install in.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<PackagesStatus>(),
    )?;
    registry.register(
        CommandSpec::new(INSTALL, "Download Packages")
            .description(
                "Downloads and installs packages by ID (`packages.list` gives them), one at a \
                 time on a thread of its own, each checked against the size and SHA-256 \
                 Beatlayer expects before it's installed. Returns at once; `packages.list` \
                 reports the progress. A package that's installed, or already downloading or \
                 waiting, isn't downloaded again. Changes nothing in the set.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<IdsArgs>()
            .result::<PackagesStatus>(),
    )?;
    registry.register(
        CommandSpec::new(REMOVE, "Remove Packages")
            .description(
                "Removes installed packages by ID: their files and record. A package whose \
                 library is loaded is removed the next time Beatlayer starts and shows as \
                 `removed_at_next_launch` until then. A feature that needs a removed package \
                 asks for it again the next time it runs. Changes nothing in the set.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<IdsArgs>()
            .result::<PackagesStatus>(),
    )
}
