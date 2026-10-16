//! Audio Unit hosting where the platform has none: Windows and Linux, where
//! the macOS implementation ([the `mac` module](super::mac)) compiles out.
//!
//! The types answer for it so the rest of Beatlayer compiles unchanged:
//! loading says Audio Units are macOS only, and the factory offers no
//! device types and makes no nodes, so an AU device a set carries from a
//! Mac is unhosted and passes its input through.

use std::collections::BTreeSet;
use std::path::Path;
use std::thread::ThreadId;

use bl_engine::{DeviceType, Node, NodeConfig, NodeFactory};
use bl_model::{Device, DeviceId, PluginState};

use super::AuError;
pub use crate::clap::{ParamChange, PluginStatus};

/// The Audio Units a `.component` bundle declares: none here, since this
/// platform hosts none.
///
/// # Errors
///
/// [`AuError::Unavailable`] always.
pub(super) fn scan_file(_path: &Path) -> Result<Vec<crate::scan::ScannedPlugin>, AuError> {
    Err(AuError::Unavailable)
}

/// An Audio Unit Beatlayer can make devices of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuType {
    /// Its device type: kind, name, role, and its parameters.
    pub device_type: DeviceType,
    /// Its vendor, if it gives one.
    pub vendor: Option<String>,
    /// Its version, if it gives one.
    pub version: Option<String>,
}

/// The Audio Unit host: on this platform, nothing hosts Audio Units.
#[derive(Clone)]
pub struct AuHost {
    owner: ThreadId,
}

impl Default for AuHost {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AuHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuHost").finish_non_exhaustive()
    }
}

impl AuHost {
    /// A host, on the thread that made it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            owner: std::thread::current().id(),
        }
    }

    /// The factory for the engine's registry: this platform makes no Audio
    /// Unit nodes, so an AU device in a set is unhosted.
    #[must_use]
    pub fn factory(&self) -> AuFactory {
        AuFactory
    }

    /// Audio Units need macOS: this always fails with
    /// [`AuError::Unavailable`], and the commands report its message.
    ///
    /// # Errors
    ///
    /// [`AuError::Unavailable`] on this platform, and
    /// [`AuError::WrongThread`] off the host's own thread.
    pub fn load(
        &self,
        _kind: &str,
        _subtype: &str,
        _manufacturer: &str,
    ) -> Result<AuType, AuError> {
        if std::thread::current().id() != self.owner {
            return Err(AuError::WrongThread);
        }
        Err(AuError::Unavailable)
    }

    /// The Audio Units loaded so far: none, on this platform.
    #[must_use]
    pub fn types(&self) -> Vec<AuType> {
        Vec::new()
    }

    /// Does the main thread's work: nothing, on this platform.
    pub fn idle(&self) -> bool {
        false
    }

    /// The device's unit's own state: it has none, on this platform.
    ///
    /// # Errors
    ///
    /// [`AuError::NoInstance`]: no device has a running unit here.
    pub fn save_state(&self, device: DeviceId) -> Result<PluginState, AuError> {
        Err(AuError::NoInstance(device))
    }

    /// What the device's unit is doing: nothing, on this platform.
    #[must_use]
    pub fn status(&self, _device: DeviceId) -> Option<PluginStatus> {
        None
    }

    /// Lets go of every unit whose device isn't in `keep`: nothing here.
    pub fn retain(&self, _keep: &BTreeSet<DeviceId>) {}

    /// Lets go of every unit: nothing here.
    pub fn clear(&self) {}

    /// Counts instances started and let go of: stays zero here.
    #[must_use]
    pub fn generation(&self) -> u64 {
        0
    }

    /// Opens a unit's own editor: there is none here.
    ///
    /// # Errors
    ///
    /// [`AuError::Unavailable`] on this platform.
    pub fn open_editor(&self, _device: DeviceId, _title: &str) -> Result<(), AuError> {
        Err(AuError::Unavailable)
    }

    /// Closes a unit's own editor, if it's open: there is none here.
    ///
    /// # Errors
    ///
    /// [`AuError::NoInstance`]: no device has a running unit here.
    pub fn close_editor(&self, device: DeviceId) -> Result<(), AuError> {
        Err(AuError::NoInstance(device))
    }

    /// Shows or hides a unit's editor window without closing it: there is
    /// none here.
    ///
    /// # Errors
    ///
    /// [`AuError::NoInstance`]: no device has a running unit here.
    pub fn set_editor_visible(&self, device: DeviceId, _shown: bool) -> Result<(), AuError> {
        Err(AuError::NoInstance(device))
    }

    /// The devices whose unit's editor window is open: none here.
    #[must_use]
    pub fn open_editors(&self) -> Vec<DeviceId> {
        Vec::new()
    }

    /// The parameter changes units made themselves since the last call:
    /// none here.
    #[must_use]
    pub fn take_param_changes(&self) -> Vec<ParamChange> {
        Vec::new()
    }
}

/// The Audio Units' factory for the engine's registry: it offers no device
/// types and makes no nodes on this platform.
#[derive(Clone, Copy, Debug, Default)]
pub struct AuFactory;

impl NodeFactory for AuFactory {
    fn create(&self, _device: &Device, _config: &NodeConfig) -> Option<Box<dyn Node>> {
        None
    }

    fn device_types(&self) -> Vec<DeviceType> {
        Vec::new()
    }
}
