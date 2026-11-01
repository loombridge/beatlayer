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

use bl_model::{AraSupport, Device, DeviceDescription, DeviceId, DeviceKind, DeviceRole};

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

    /// The extra audio outputs the devices of `kind` offer past their main
    /// one (WP-5.45), by their names, in the order they declare them: a
    /// CLAP plug-in's audio output ports, a native instrument's individual
    /// outputs. The default offers none.
    fn outputs(&self, kind: &DeviceKind) -> Vec<String> {
        let _ = kind;
        Vec::new()
    }

    /// What `device` does as an ARA plug-in (IMP-20), if it is one: a track
    /// whose chain holds such a device leaves the clips the plug-in renders
    /// to it ([`bl_model::ara_renders`]). The default says none is.
    fn ara_support(&self, device: &Device) -> Option<AraSupport> {
        let _ = device;
        None
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
    /// Whether its devices work with hardware outside the computer
    /// (WP-5.34): they have an output port, a channel, the interface inputs
    /// the sound comes back on, and a Hardware Latency
    /// ([`bl_model::External`]).
    pub external: bool,
    /// Whether its devices send their audio to the interface's outputs and
    /// hear it back (WP-5.35): an external effect, with an Audio To beside
    /// its Audio From.
    pub external_output: bool,
    /// The names of the CV outputs its devices have, in their order
    /// (`docs/specs/cv-tools.md`, R3). A type with any is a CV device.
    pub cv_outputs: Vec<String>,
    /// The names of the CV inputs its devices have, in their order.
    pub cv_inputs: Vec<String>,
    /// Whether its devices have a MIDI sidechain input (WP-5.27): the
    /// notes and channel messages of a MIDI track reach the device in its
    /// block's notes ([`Block::notes`](crate::Block::notes)).
    pub midi_sidechain: bool,
    /// Whether that MIDI sidechain has a tapping point, Pre FX, Post FX or
    /// Post Mixer, Post FX in a new device (the routing spec, R39).
    pub midi_tap: bool,
    /// Whether its devices send notes on from a note output (IMP-24): a CLAP
    /// plug-in's note output port, a VST3 class's event output bus, or an
    /// Audio Unit's MIDI output. A device whose type has one can sit in the
    /// MIDI-effect position of a MIDI track's chain, before the instrument;
    /// one whose type has none can't. Every MIDI-effect type has one.
    pub note_output: bool,
    /// The extra audio outputs its devices offer past their main one, by the
    /// names the device gives them (WP-5.45): another track can take one as
    /// its input ([`Input::Track`](bl_model::Input::Track)), and the node
    /// renders each into a buffer of the block it is given
    /// ([`Block::outputs_mut`](crate::Block::outputs_mut)). Empty for a
    /// device that offers none.
    pub outputs: Vec<String>,
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
            external: false,
            external_output: false,
            cv_outputs: Vec::new(),
            cv_inputs: Vec::new(),
            midi_sidechain: false,
            midi_tap: false,
            note_output: role == DeviceRole::MidiEffect,
            outputs: Vec::new(),
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

    /// The same device type, working with external hardware.
    #[must_use]
    pub fn with_external(mut self) -> Self {
        self.external = true;
        self
    }

    /// The same device type, sending its audio to the interface and hearing
    /// it back: an external effect (WP-5.35).
    #[must_use]
    pub fn with_external_output(mut self) -> Self {
        self.external = true;
        self.external_output = true;
        self
    }

    /// The same device type, with CV outputs and inputs by name
    /// (`docs/specs/cv-tools.md`, R3, R8): its devices have the routing of an
    /// external device with these choosers, and its nodes read and write
    /// them through [`Block::cv_in`](crate::Block::cv_in) and
    /// [`Block::cv_out_mut`](crate::Block::cv_out_mut).
    ///
    /// # Panics
    ///
    /// If it has more than [`MAX_CV_PORTS`](bl_model::MAX_CV_PORTS) of
    /// either.
    #[must_use]
    pub fn with_cv(mut self, outputs: &[&str], inputs: &[&str]) -> Self {
        assert!(
            outputs.len().max(inputs.len()) <= bl_model::MAX_CV_PORTS,
            "a device has at most {} CV outputs or inputs",
            bl_model::MAX_CV_PORTS
        );
        self.external = true;
        self.cv_outputs = outputs.iter().map(|&n| n.to_owned()).collect();
        self.cv_inputs = inputs.iter().map(|&n| n.to_owned()).collect();
        self
    }

    /// The same device type, declaring a MIDI sidechain input.
    #[must_use]
    pub fn with_midi_sidechain(mut self) -> Self {
        self.midi_sidechain = true;
        self
    }

    /// The same device type, declaring a MIDI sidechain input with a
    /// tapping point (the routing spec, R39).
    #[must_use]
    pub fn with_midi_sidechain_tap(mut self) -> Self {
        self.midi_sidechain = true;
        self.midi_tap = true;
        self
    }

    /// The same device type, declaring a note output (IMP-24): its devices
    /// send notes on, so they can sit in the MIDI-effect position of a MIDI
    /// track's chain.
    #[must_use]
    pub fn with_note_output(mut self) -> Self {
        self.note_output = true;
        self
    }

    /// The same device type, its devices' extra audio outputs past the main
    /// one, named as the device names them (WP-5.45).
    #[must_use]
    pub fn with_outputs(mut self, outputs: impl IntoIterator<Item = String>) -> Self {
        self.outputs = outputs.into_iter().collect();
        self
    }

    /// A new, switched-on device of this type with default parameters, and
    /// a new sidechain input, MIDI sidechain input or routing to hardware
    /// if the type declares them.
    #[must_use]
    pub fn device(&self, id: DeviceId) -> Device {
        let mut device = Device::new(id, self.name.clone(), self.kind.clone()).with_role(self.role);
        if self.sidechain {
            device = device.with_sidechain();
        }
        if self.external {
            device = device.with_external();
        }
        if self.external_output {
            device = device.with_external_output();
        }
        if !self.cv_outputs.is_empty() || !self.cv_inputs.is_empty() {
            device = device.with_cv(&self.cv_outputs, &self.cv_inputs);
        }
        if self.midi_tap {
            device = device.with_midi_sidechain_tap();
        } else if self.midi_sidechain {
            device = device.with_midi_sidechain();
        }
        device
    }
}

/// A factory for one native device type, from a function.
struct Native<F> {
    device_type: String,
    offered: Option<DeviceType>,
    outputs: Vec<String>,
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

    fn outputs(&self, kind: &DeviceKind) -> Vec<String> {
        match kind {
            DeviceKind::Native { device_type } if *device_type == self.device_type => {
                self.outputs.clone()
            }
            _ => Vec::new(),
        }
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
            outputs: Vec::new(),
            make,
        });
    }

    /// Adds a native device type the browser doesn't offer that declares the
    /// extra audio outputs `outputs` past its main one (WP-5.45), made by
    /// `make`. For tests of a multi-output instrument.
    ///
    /// # Panics
    ///
    /// If `device_type` isn't a native device type.
    pub fn add_native_with_outputs<F>(&mut self, device_type: &str, outputs: &[&str], make: F)
    where
        F: Fn(&Device, &NodeConfig) -> Box<dyn Node> + Send + Sync + 'static,
    {
        self.add(Native {
            device_type: device_type.to_owned(),
            offered: None,
            outputs: outputs.iter().map(|name| (*name).to_owned()).collect(),
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
        let outputs = device_type.outputs.clone();
        self.add(Native {
            device_type: id.clone(),
            offered: Some(device_type),
            outputs,
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

    /// What `device` does as an ARA plug-in, if a factory says it is one
    /// (IMP-20).
    #[must_use]
    pub fn ara_support(&self, device: &Device) -> Option<AraSupport> {
        self.factories
            .iter()
            .find_map(|factory| factory.ara_support(device))
    }

    /// The extra audio outputs the devices of `kind` offer past their main
    /// one (WP-5.45), by their names, in the order they declare them; empty
    /// for a device whose factory declares none.
    #[must_use]
    pub fn device_outputs(&self, kind: &DeviceKind) -> Vec<String> {
        self.factories
            .iter()
            .map(|factory| factory.outputs(kind))
            .find(|outputs| !outputs.is_empty())
            .unwrap_or_default()
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
