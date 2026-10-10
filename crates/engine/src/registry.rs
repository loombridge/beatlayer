//! Where the compiler gets a node for each device.

use bl_model::{Device, DeviceKind};

use crate::node::{Node, NodeConfig};
use crate::nodes::{TEST_TONE, TestTone};

/// Makes nodes for devices. The compiler asks each factory in a
/// [`Registry`] in turn. It runs off the audio thread, so a factory may
/// allocate and take its time.
pub trait NodeFactory: Send + Sync {
    /// A new node for `device`, set to the device's parameter values, or
    /// `None` if this factory doesn't make that kind of device.
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>>;
}

/// A factory for one native device type, from a function.
struct Native<F> {
    device_type: String,
    make: F,
}

impl<F> NodeFactory for Native<F>
where
    F: Fn(&Device, &NodeConfig) -> Box<dyn Node> + Send + Sync,
{
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        match &device.kind {
            DeviceKind::Native { device_type } if *device_type == self.device_type => {
                Some((self.make)(device, config))
            }
            _ => None,
        }
    }
}

/// The node factories the compiler uses, asked in the order they were added.
///
/// [`Registry::new`] has the engine's own devices; `bl-devices` (Phase 2) and
/// the plug-in host add theirs. A device no factory makes compiles to
/// nothing: its track's audio passes through where it would be.
pub struct Registry {
    factories: Vec<Box<dyn NodeFactory>>,
}

impl Registry {
    /// The engine's own devices: the test tone ([`TEST_TONE`]).
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Self::empty();
        registry.add_native(TEST_TONE, |device, config| {
            Box::new(TestTone::new(device, config))
        });
        registry
    }

    /// No factories at all.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            factories: Vec::new(),
        }
    }

    /// Adds a factory, asked after the ones already here.
    pub fn add(&mut self, factory: impl NodeFactory + 'static) {
        self.factories.push(Box::new(factory));
    }

    /// Adds a factory for the native device type `device_type`, such as
    /// `beatlayer.utility`, that calls `make`.
    pub fn add_native<F>(&mut self, device_type: &str, make: F)
    where
        F: Fn(&Device, &NodeConfig) -> Box<dyn Node> + Send + Sync + 'static,
    {
        self.add(Native {
            device_type: device_type.to_owned(),
            make,
        });
    }

    /// A node for `device` from the first factory that makes one.
    pub(crate) fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        self.factories
            .iter()
            .find_map(|factory| factory.create(device, config))
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("factories", &self.factories.len())
            .finish()
    }
}
