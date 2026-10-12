//! Where the compiler gets a node for each device, and the audio for each
//! clip; and the device interface that native devices (WP-2.5) and plug-in
//! hosts (WP-2.6 to 2.8) implement.
//!
//! A source of devices is a [`NodeFactory`]: it lists the device types it
//! offers ([`DeviceType`]: what a device is, its name, and its role in a
//! chain), and makes a [`Node`] for a device of one of them, set to the
//! device's parameter values. The node does the rest: it processes blocks
//! in place, takes parameter changes at their frames, takes them without
//! processing while the device is switched off ([`Node::flush`]), and
//! reports its latency ([`Node::latency`], for delay compensation in
//! WP-2.11). A device type may declare a sidechain input
//! ([`DeviceType::with_sidechain`]); its node then reads what its detector
//! hears from [`Block::sidechain`](crate::Block::sidechain). A device's state is its parameter values, which the document
//! holds and presets save; a plug-in's own state arrives with WP-2.6.

use std::sync::Arc;

use bl_model::{Device, DeviceDescription, DeviceId, DeviceKind, DeviceRole};

use crate::clips::MediaLibrary;
use crate::node::{Node, NodeConfig};
use crate::nodes::{BEAT_TONE, BeatTone, TEST_TONE, TestTone};

/// Makes nodes for devices. The compiler asks each factory in a
/// [`Registry`] in turn. It runs off the audio thread, so a factory may
/// allocate and take its time.
pub trait NodeFactory: Send + Sync {
    /// A new node for `device`, set to the device's parameter values, or
    /// `None` if this factory doesn't make that kind of device.
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>>;

    /// The device types this factory offers, for the browser to list and
    /// the shell to insert. The default offers none: a factory that only
    /// makes nodes for devices already in a set.
    fn device_types(&self) -> Vec<DeviceType> {
        Vec::new()
    }
}

/// A kind of device a factory offers: what the browser lists and a new
/// device in a chain starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceType {
    /// What it is: a native device's type ID, or a plug-in's format and ID.
    pub kind: DeviceKind,
    /// Its own name, such as `Utility`, which a new device's title shows.
    pub name: String,
    /// Where it goes in a chain.
    pub role: DeviceRole,
    /// Its parameters' descriptors and its layout (WP-2.5), which the
    /// generic UI draws, presets are checked against, and agents read.
    /// Empty for a type that doesn't describe its parameters.
    pub description: DeviceDescription,
    /// Whether its devices have a sidechain input (WP-2.19b): other
    /// tracks can route into it, and its node reads what its detector
    /// hears with [`Block::sidechain`](crate::Block::sidechain).
    pub sidechain: bool,
}

impl DeviceType {
    /// A native device type.
    #[must_use]
    pub fn native(device_type: &str, name: &str, role: DeviceRole) -> Self {
        Self {
            kind: DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
            name: name.to_owned(),
            role,
            description: DeviceDescription::default(),
            sidechain: false,
        }
    }

    /// The same type, with its parameters described.
    #[must_use]
    pub fn with_description(mut self, description: DeviceDescription) -> Self {
        self.description = description;
        self
    }

    /// The same device type, declaring a sidechain input.
    #[must_use]
    pub fn with_sidechain(mut self) -> Self {
        self.sidechain = true;
        self
    }

    /// A new, switched-on device of this type with default parameters, and
    /// a new sidechain input if the type declares one.
    #[must_use]
    pub fn device(&self, id: DeviceId) -> Device {
        let device = Device::new(id, self.name.clone(), self.kind.clone()).with_role(self.role);
        if self.sidechain {
            device.with_sidechain()
        } else {
            device
        }
    }
}

/// A factory for one native device type, from a function.
struct Native<F> {
    device_type: String,
    offered: Option<DeviceType>,
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

    fn device_types(&self) -> Vec<DeviceType> {
        self.offered.iter().cloned().collect()
    }
}

/// The node factories the compiler uses, asked in the order they were added,
/// and the media library its clips' audio comes from.
///
/// [`Registry::new`] has the engine's own devices; `bl-devices` (Phase 2) and
/// the plug-in host add theirs. A device no factory makes compiles to
/// nothing: its track's audio passes through where it would be. Without a
/// media library ([`Registry::set_media`]), every audio clip's media is
/// missing, and clips play silence.
pub struct Registry {
    factories: Vec<Box<dyn NodeFactory>>,
    media: Option<Arc<dyn MediaLibrary>>,
}

impl Registry {
    /// The engine's own devices: the test tone ([`TEST_TONE`]) and the beat
    /// tone ([`BEAT_TONE`]). No media library.
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Self::empty();
        registry.add_native(TEST_TONE, |device, config| {
            Box::new(TestTone::new(device, config))
        });
        registry.add_native(BEAT_TONE, |device, _| Box::new(BeatTone::new(device)));
        registry
    }

    /// No factories at all, and no media library.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            factories: Vec::new(),
            media: None,
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
            offered: None,
            make,
        });
    }

    /// Adds a native device type that the browser offers, made by `make`.
    ///
    /// # Panics
    ///
    /// If `device_type` isn't a native device type.
    pub fn add_device<F>(&mut self, device_type: DeviceType, make: F)
    where
        F: Fn(&Device, &NodeConfig) -> Box<dyn Node> + Send + Sync + 'static,
    {
        let DeviceKind::Native { device_type: id } = &device_type.kind else {
            panic!("add_device takes native device types");
        };
        self.add(Native {
            device_type: id.clone(),
            offered: Some(device_type),
            make,
        });
    }

    /// Every device type the factories offer, in the order they were added.
    #[must_use]
    pub fn device_types(&self) -> Vec<DeviceType> {
        self.factories
            .iter()
            .flat_map(|factory| factory.device_types())
            .collect()
    }

    /// The offered device type of `kind`, if a factory offers one.
    #[must_use]
    pub fn device_type(&self, kind: &DeviceKind) -> Option<DeviceType> {
        self.device_types().into_iter().find(|t| t.kind == *kind)
    }

    /// Sets where clips' audio comes from: the project's media, streamed
    /// ([`MediaStreams`](crate::MediaStreams)), or a
    /// [`MediaMap`](crate::MediaMap) of audio made in memory.
    pub fn set_media(&mut self, media: Arc<dyn MediaLibrary>) {
        self.media = Some(media);
    }

    /// The same registry, with clips' audio from `media`.
    #[must_use]
    pub fn with_media(mut self, media: Arc<dyn MediaLibrary>) -> Self {
        self.set_media(media);
        self
    }

    /// A node for `device` from the first factory that makes one, as the
    /// compiler makes it: off the audio thread. Tests of devices use it to
    /// run one on its own.
    #[must_use]
    pub fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        self.factories
            .iter()
            .find_map(|factory| factory.create(device, config))
    }

    /// Where clips' audio comes from, if anywhere.
    pub(crate) fn media(&self) -> Option<&dyn MediaLibrary> {
        self.media.as_deref()
    }

    /// The same, shared, for the loader thread.
    pub(crate) fn shared_media(&self) -> Option<Arc<dyn MediaLibrary>> {
        self.media.clone()
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
            .field("media", &self.media.is_some())
            .finish()
    }
}
