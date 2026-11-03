//! LV2 hosting (WP-5.39): Beatlayer's own host on the system's lilv
//! library, beside the CLAP, VST3 and Audio Unit hosts. A [`World`] finds
//! the LV2 plug-ins on the machine and instantiates one at the engine's
//! rate as an [`Instance`]: audio and control ports connected, MIDI and
//! the transport in and MIDI out through Atom ports, latency read from the
//! plug-in's latency port, its worker run on a thread of its own, and state
//! saved and restored as the text lilv writes.
//!
//! lilv is loaded from the system when a [`World`] is made, not linked, so
//! a machine without it runs Beatlayer with LV2 unavailable
//! ([`Lv2Error::Unavailable`]); only Linux looks for it, the other
//! platforms always answer so. The device ([`Lv2Host`], [`Lv2Node`]), the
//! scanner and the editors build on this (the LV2 hosting spec).
//!
//! An [`Instance`] is made, activated and dropped off the audio thread;
//! [`Instance::run`], [`Instance::set_control`] and [`Instance::midi_out`]
//! allocate nothing and take no lock. Saving state needs no more than a
//! [`StateHandle`], which LV2 lets run beside the audio thread; restoring
//! it is a main-thread call on an instance no one is running, and so is
//! every call on a [`World`]: lilv's world isn't thread-safe, so each one
//! holds the world's lock.

mod describe;
mod features;
mod host;
mod node;
mod sys;
mod ui;

use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bl_model::{DeviceId, ParamUnit};
use rtrb::Consumer;

use features::{Features, Msg, WORKER_INTERFACE, WorkerThread};
use sys::{Api, Urid};

pub use host::{Loan, Lv2Factory, Lv2Host, Lv2Type, ParamChange, PluginStatus, bundle_plugins};
pub use node::Lv2Node;

const CORE: &str = "http://lv2plug.in/ns/lv2core#";
const ATOM: &str = "http://lv2plug.in/ns/ext/atom#";
const PORT_PROPS: &str = "http://lv2plug.in/ns/ext/port-props#";
const UNITS: &str = "http://lv2plug.in/ns/extensions/units#";
const TIME: &str = "http://lv2plug.in/ns/ext/time#";
const UI: &str = "http://lv2plug.in/ns/extensions/ui#";
/// What lilv's state calls take: the plug-in's data is plain old data and
/// portable (`LV2_STATE_IS_POD | LV2_STATE_IS_PORTABLE`).
const STATE_FLAGS: u32 = 3;
/// The state's own URI in the text; lilv won't write one without.
const STATE_URI: &CStr = c"urn:beatlayer:state";
/// The room an Atom port gets, in 64-bit words (16 KiB): about 600 MIDI
/// events a block.
const ATOM_WORDS: usize = 2048;

/// Why an LV2 call failed. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lv2Error {
    /// This machine has no lilv, or isn't Linux.
    Unavailable,
    /// lilv or the plug-in refused, and what.
    Failed(String),
    /// A block longer than the instance was made for, or a buffer shorter
    /// than the block.
    BadBlock,
    /// The block's MIDI doesn't fit the plug-in's Atom port.
    TooManyEvents,
    /// The file isn't an LV2 bundle Beatlayer can load, and why.
    Load(String),
    /// No plug-in of that URI is known.
    UnknownPlugin(String),
    /// The device has no running plug-in: it isn't an LV2 device, or its
    /// plug-in couldn't be made.
    NoInstance(DeviceId),
    /// A host call off the main thread.
    WrongThread,
    /// The plug-ins are lent to another offline render already.
    Lent,
}

impl fmt::Display for Lv2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("LV2 isn't available: lilv wasn't found"),
            Self::Failed(why) => f.write_str(why),
            Self::BadBlock => f.write_str("the block is longer than the instance's maximum"),
            Self::TooManyEvents => f.write_str("the MIDI events don't fit the Atom port"),
            Self::Load(why) => write!(f, "couldn't load the plug-in: {why}"),
            Self::UnknownPlugin(uri) => write!(f, "no LV2 plug-in {uri} is known"),
            Self::NoInstance(device) => write!(f, "{device} has no running LV2 plug-in"),
            Self::WrongThread => f.write_str("LV2's main-thread calls run on the main thread"),
            Self::Lent => f.write_str("the plug-ins are lent to another render"),
        }
    }
}

impl std::error::Error for Lv2Error {}

fn failed(why: impl Into<String>) -> Lv2Error {
    Lv2Error::Failed(why.into())
}

/// What a port carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    Audio,
    Control,
    /// An Atom port, and whether it takes (or sends) MIDI events.
    Atom {
        midi: bool,
    },
    /// Anything else (a control voltage port, say): connected to silence or
    /// to a scratch buffer.
    Other,
}

/// One port of a plug-in.
#[derive(Clone, Debug, PartialEq)]
pub struct PortInfo {
    pub index: u32,
    pub symbol: String,
    pub name: String,
    pub kind: PortKind,
    pub input: bool,
    /// A control port's range and default; zero where the plug-in gives none.
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// `lv2:toggled`, `lv2:integer`, `lv2:enumeration` and
    /// `pprops:logarithmic`: how a control port's value reads.
    pub toggled: bool,
    pub integer: bool,
    pub enumeration: bool,
    pub logarithmic: bool,
    /// `pprops:notOnGUI`: a port the plug-in keeps off its panel.
    pub hidden: bool,
    /// `pprops:notAutomatic`: a port automation shouldn't move.
    pub not_automatic: bool,
    /// `lv2:isSideChain`: an audio input that isn't the main one.
    pub sidechain: bool,
    /// An Atom input that takes `time:Position`.
    pub position: bool,
    /// What a control port's value measures (`units:unit`).
    pub unit: ParamUnit,
    /// An enumeration's values and their labels.
    pub scale_points: Vec<(f32, String)>,
}

/// An editor the plug-in brings: an X11 UI and where its code is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiInfo {
    pub uri: String,
    pub binary: PathBuf,
    pub bundle: PathBuf,
}

/// One plug-in a [`World`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginInfo {
    pub uri: String,
    pub name: String,
    pub ports: Vec<PortInfo>,
    /// Its author, if it names one.
    pub vendor: Option<String>,
    /// It is an `lv2:InstrumentPlugin`.
    pub instrument: bool,
    /// The plug-in classes it declares, by their names in LV2's core
    /// vocabulary, such as `ReverbPlugin`.
    pub classes: Vec<String>,
    /// The bundle folder it's in.
    pub bundle: PathBuf,
    /// Its X11 UI, if it has one.
    pub ui: Option<UiInfo>,
}

/// A MIDI message on a frame of the block: a channel message of up to
/// three bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    pub frame: u32,
    pub len: u8,
    pub data: [u8; 3],
}

/// Where the song is, for the plug-ins whose Atom port takes
/// `time:Position`: sent at the first frame of each block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    /// The song position in frames.
    pub frame: i64,
    pub playing: bool,
    pub bpm: f32,
    /// The bar the position is in, from 0, and the beat in it.
    pub bar: i64,
    pub bar_beat: f32,
    pub beats_per_bar: f32,
    pub beat_unit: u32,
}

/// The URIDs the host maps itself: URIs to numbers, for lilv and for the
/// plug-ins' `urid:map` and `urid:unmap`.
struct Urids(Mutex<Vec<CString>>);

impl Urids {
    fn map(&self, uri: &CStr) -> Urid {
        let mut names = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let at = names.iter().position(|n| n.as_c_str() == uri);
        let at = at.unwrap_or_else(|| {
            names.push(uri.to_owned());
            names.len() - 1
        });
        Urid::try_from(at + 1).unwrap_or(0)
    }

    fn uri(&self, prefix: &str, name: &str) -> Urid {
        self.map(&CString::new(format!("{prefix}{name}")).expect("a URI without NUL"))
    }
}

unsafe extern "C" fn urid_map(handle: *mut c_void, uri: *const c_char) -> Urid {
    // SAFETY: `handle` is the `Urids` of the `Shared` that owns this map, and
    // `uri` a C string the caller gives.
    unsafe { (*handle.cast::<Urids>()).map(CStr::from_ptr(uri)) }
}

unsafe extern "C" fn urid_unmap(handle: *mut c_void, urid: Urid) -> *const c_char {
    // SAFETY: as `urid_map`; a `CString`'s buffer stays where it is when the
    // list grows.
    let urids = unsafe { &*handle.cast::<Urids>() };
    let names = urids.0.lock().unwrap_or_else(PoisonError::into_inner);
    urid.checked_sub(1)
        .and_then(|at| names.get(at as usize))
        .map_or(ptr::null(), |name| name.as_ptr())
}

/// The nodes the host asks lilv about: classes and properties.
struct Nodes {
    audio: sys::Node,
    control: sys::Node,
    input: sys::Node,
    atom: sys::Node,
    midi: sys::Node,
    position: sys::Node,
    toggled: sys::Node,
    integer: sys::Node,
    enumeration: sys::Node,
    logarithmic: sys::Node,
    not_on_gui: sys::Node,
    not_automatic: sys::Node,
    sidechain: sys::Node,
    unit: sys::Node,
    rdf_type: sys::Node,
    instrument: sys::Node,
    x11_ui: sys::Node,
}

impl Nodes {
    fn all(&self) -> [sys::Node; 17] {
        [
            self.audio,
            self.control,
            self.input,
            self.atom,
            self.midi,
            self.position,
            self.toggled,
            self.integer,
            self.enumeration,
            self.logarithmic,
            self.not_on_gui,
            self.not_automatic,
            self.sidechain,
            self.unit,
            self.rdf_type,
            self.instrument,
            self.x11_ui,
        ]
    }
}

/// The URIDs the host writes into Atom buffers.
struct Known {
    midi_event: Urid,
    sequence: Urid,
    chunk: Urid,
    float: Urid,
    int: Urid,
    long: Urid,
    object: Urid,
    position: Urid,
    frame: Urid,
    speed: Urid,
    bar: Urid,
    bar_beat: Urid,
    beat_unit: Urid,
    beats_per_bar: Urid,
    beats_per_minute: Urid,
}

/// lilv's world, the library it was loaded from, and the class nodes the
/// host asks ports about.
struct Shared {
    api: Api,
    world: sys::World,
    /// Held across every call that reaches the world, its plug-ins or its
    /// nodes.
    lock: Mutex<()>,
    /// The map's handle: kept here so it outlives the plug-ins.
    urids: Box<Urids>,
    map: sys::UridMap,
    unmap: sys::UridUnmap,
    nodes: Nodes,
    known: Known,
}

// SAFETY: the raw pointers belong to lilv's world, which the `lock` guards.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn map_ptr(&self) -> *mut sys::UridMap {
        ptr::from_ref(&self.map).cast_mut()
    }

    fn unmap_ptr(&self) -> *mut sys::UridUnmap {
        ptr::from_ref(&self.unmap).cast_mut()
    }

    fn node(&self, uri: &CStr) -> sys::Node {
        // SAFETY: the world is alive; callers hold the lock or are `new`.
        unsafe { (self.api.lilv_new_uri)(self.world, uri.as_ptr()) }
    }

    /// A node's text, copied. The node stays the caller's.
    fn string(&self, node: sys::Node) -> String {
        if node.is_null() {
            return String::new();
        }
        // SAFETY: a live node's string is a C string.
        unsafe { CStr::from_ptr((self.api.lilv_node_as_string)(node)) }
            .to_string_lossy()
            .into_owned()
    }

    /// A node's text, and the node freed; empty for no node.
    fn take_string(&self, node: sys::Node) -> String {
        let text = self.string(node);
        if !node.is_null() {
            // SAFETY: `node` is one lilv gave to free.
            unsafe { (self.api.lilv_node_free)(node) };
        }
        text
    }

    /// A numeric node's value, without freeing it.
    fn number(&self, node: sys::Node) -> Option<f32> {
        if node.is_null() {
            return None;
        }
        // SAFETY: a live node.
        unsafe {
            ((self.api.lilv_node_is_float)(node) || (self.api.lilv_node_is_int)(node))
                .then(|| (self.api.lilv_node_as_float)(node))
        }
    }

    /// A numeric node's value, and the node freed; `fallback` for no node.
    fn take_number(&self, node: sys::Node, fallback: f32) -> f32 {
        let number = self.number(node);
        if !node.is_null() {
            // SAFETY: `node` is one lilv gave to free.
            unsafe { (self.api.lilv_node_free)(node) };
        }
        number.unwrap_or(fallback)
    }

    /// The folder a file URI node names, without its trailing slash.
    fn path(&self, node: sys::Node) -> Option<PathBuf> {
        if node.is_null() {
            return None;
        }
        // SAFETY: a live node; the path lilv gives is freed here.
        unsafe {
            let path = (self.api.lilv_node_get_path)(node, ptr::null_mut());
            if path.is_null() {
                return None;
            }
            let text = CStr::from_ptr(path).to_string_lossy().into_owned();
            (self.api.lilv_free)(path.cast());
            let text = text.strip_suffix('/').map_or(text.as_str(), |t| t);
            Some(PathBuf::from(if text.is_empty() { "/" } else { text }))
        }
    }

    fn describe(&self, plugin: sys::Plugin) -> PluginInfo {
        let api = &self.api;
        // SAFETY: `plugin` is one of the world's, and the caller holds the lock.
        unsafe {
            let author = (api.lilv_plugin_get_author_name)(plugin);
            let vendor = self.take_string(author);
            let types = (api.lilv_plugin_get_value)(plugin, self.nodes.rdf_type);
            let instrument =
                !types.is_null() && (api.lilv_nodes_contains)(types, self.nodes.instrument);
            let mut classes = Vec::new();
            if !types.is_null() {
                let mut at = (api.lilv_nodes_begin)(types);
                while !(api.lilv_nodes_is_end)(types, at) {
                    let uri = self.string((api.lilv_nodes_get)(types, at));
                    if let Some(class) = uri.strip_prefix(CORE)
                        && class.ends_with("Plugin")
                        && class != "Plugin"
                    {
                        classes.push(class.to_owned());
                    }
                    at = (api.lilv_nodes_next)(types, at);
                }
                (api.lilv_nodes_free)(types);
            }
            PluginInfo {
                uri: self.string((api.lilv_plugin_get_uri)(plugin)),
                name: self.take_string((api.lilv_plugin_get_name)(plugin)),
                ports: (0..(api.lilv_plugin_get_num_ports)(plugin))
                    .map(|i| {
                        self.describe_port(
                            plugin,
                            (api.lilv_plugin_get_port_by_index)(plugin, i),
                            i,
                        )
                    })
                    .collect(),
                vendor: Some(vendor).filter(|v| !v.is_empty()),
                instrument,
                classes,
                bundle: self
                    .path((api.lilv_plugin_get_bundle_uri)(plugin))
                    .unwrap_or_default(),
                ui: self.x11_ui(plugin),
            }
        }
    }

    /// The plug-in's first X11 UI, if it has one.
    fn x11_ui(&self, plugin: sys::Plugin) -> Option<UiInfo> {
        let api = &self.api;
        // SAFETY: as `describe`.
        unsafe {
            let uis = (api.lilv_plugin_get_uis)(plugin);
            if uis.is_null() {
                return None;
            }
            let mut found = None;
            let mut it = (api.lilv_uis_begin)(uis);
            while found.is_none() && !(api.lilv_uis_is_end)(uis, it) {
                let ui = (api.lilv_uis_get)(uis, it);
                if (api.lilv_ui_is_a)(ui, self.nodes.x11_ui) {
                    found = Some(UiInfo {
                        uri: self.string((api.lilv_ui_get_uri)(ui)),
                        binary: self
                            .path((api.lilv_ui_get_binary_uri)(ui))
                            .unwrap_or_default(),
                        bundle: self
                            .path((api.lilv_ui_get_bundle_uri)(ui))
                            .unwrap_or_default(),
                    });
                }
                it = (api.lilv_uis_next)(uis, it);
            }
            (api.lilv_uis_free)(uis);
            found
        }
    }

    fn describe_port(&self, plugin: sys::Plugin, port: sys::Port, index: u32) -> PortInfo {
        let api = &self.api;
        // SAFETY: as `describe`.
        unsafe {
            let is = |class| (api.lilv_port_is_a)(plugin, port, class);
            let has = |property| (api.lilv_port_has_property)(plugin, port, property);
            let kind = if is(self.nodes.audio) {
                PortKind::Audio
            } else if is(self.nodes.control) {
                PortKind::Control
            } else if is(self.nodes.atom) {
                PortKind::Atom {
                    midi: (api.lilv_port_supports_event)(plugin, port, self.nodes.midi),
                }
            } else {
                PortKind::Other
            };
            let (mut default, mut min, mut max) =
                (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            (api.lilv_port_get_range)(plugin, port, &raw mut default, &raw mut min, &raw mut max);
            let mut scale_points = Vec::new();
            let points = (api.lilv_port_get_scale_points)(plugin, port);
            if !points.is_null() {
                let mut it = (api.lilv_scale_points_begin)(points);
                while !(api.lilv_scale_points_is_end)(points, it) {
                    let point = (api.lilv_scale_points_get)(points, it);
                    if let Some(value) = self.number((api.lilv_scale_point_get_value)(point)) {
                        scale_points
                            .push((value, self.string((api.lilv_scale_point_get_label)(point))));
                    }
                    it = (api.lilv_scale_points_next)(points, it);
                }
                (api.lilv_scale_points_free)(points);
            }
            scale_points.sort_by(|a, b| a.0.total_cmp(&b.0));
            let units = (api.lilv_port_get_value)(plugin, port, self.nodes.unit);
            let mut unit = ParamUnit::None;
            if !units.is_null() {
                let first = (api.lilv_nodes_begin)(units);
                if !(api.lilv_nodes_is_end)(units, first) {
                    let node = (api.lilv_nodes_get)(units, first);
                    if (api.lilv_node_is_uri)(node) {
                        unit = describe::unit(self.string(node).strip_prefix(UNITS).unwrap_or(""));
                    }
                }
                (api.lilv_nodes_free)(units);
            }
            PortInfo {
                index,
                symbol: self.string((api.lilv_port_get_symbol)(plugin, port)),
                name: self.take_string((api.lilv_port_get_name)(plugin, port)),
                kind,
                input: is(self.nodes.input),
                default: self.take_number(default, 0.0),
                min: self.take_number(min, 0.0),
                max: self.take_number(max, 1.0),
                toggled: has(self.nodes.toggled),
                integer: has(self.nodes.integer),
                enumeration: has(self.nodes.enumeration),
                logarithmic: has(self.nodes.logarithmic),
                hidden: has(self.nodes.not_on_gui),
                not_automatic: has(self.nodes.not_automatic),
                sidechain: has(self.nodes.sidechain),
                position: matches!(kind, PortKind::Atom { .. })
                    && (api.lilv_port_supports_event)(plugin, port, self.nodes.position),
                unit,
                scale_points,
            }
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the nodes and the world are this value's.
        unsafe {
            for node in self.nodes.all() {
                (self.api.lilv_node_free)(node);
            }
            (self.api.lilv_world_free)(self.world);
        }
    }
}

/// The LV2 plug-ins on this machine, as lilv finds them.
#[derive(Clone)]
pub struct World(Arc<Shared>);

impl World {
    /// A world on the system's lilv.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Unavailable`] where lilv isn't installed, and always off
    /// Linux.
    pub fn new() -> Result<World, Lv2Error> {
        if cfg!(target_os = "linux") {
            Self::with_library(Path::new("liblilv-0.so.0"))
        } else {
            Err(Lv2Error::Unavailable)
        }
    }

    /// A world on the lilv library at `library`.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Unavailable`] when the library won't load or lacks a lilv
    /// function.
    pub fn with_library(library: &Path) -> Result<World, Lv2Error> {
        // SAFETY: loading lilv runs its initialisers, which do nothing but
        // set up the library.
        let api = unsafe {
            libloading::Library::new(library)
                .and_then(|library| Api::load(library))
                .map_err(|_| Lv2Error::Unavailable)?
        };
        // SAFETY: lilv's constructor.
        let world = unsafe { (api.lilv_world_new)() };
        if world.is_null() {
            return Err(failed("lilv made no world"));
        }
        let urids = Box::new(Urids(Mutex::new(Vec::new())));
        let handle = ptr::from_ref::<Urids>(&urids).cast_mut().cast::<c_void>();
        let node = |uri: &str| {
            let uri = CString::new(uri).expect("a URI without NUL");
            // SAFETY: the world was just made.
            unsafe { (api.lilv_new_uri)(world, uri.as_ptr()) }
        };
        let nodes = Nodes {
            audio: node(&format!("{CORE}AudioPort")),
            control: node(&format!("{CORE}ControlPort")),
            input: node(&format!("{CORE}InputPort")),
            atom: node(&format!("{ATOM}AtomPort")),
            midi: node("http://lv2plug.in/ns/ext/midi#MidiEvent"),
            position: node(&format!("{TIME}Position")),
            toggled: node(&format!("{CORE}toggled")),
            integer: node(&format!("{CORE}integer")),
            enumeration: node(&format!("{CORE}enumeration")),
            logarithmic: node(&format!("{PORT_PROPS}logarithmic")),
            not_on_gui: node(&format!("{PORT_PROPS}notOnGUI")),
            not_automatic: node(&format!("{PORT_PROPS}notAutomatic")),
            sidechain: node(&format!("{CORE}isSideChain")),
            unit: node(&format!("{UNITS}unit")),
            rdf_type: node("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            instrument: node(&format!("{CORE}InstrumentPlugin")),
            x11_ui: node(&format!("{UI}X11UI")),
        };
        let known = Known {
            midi_event: urids.uri("http://lv2plug.in/ns/ext/midi#", "MidiEvent"),
            sequence: urids.uri(ATOM, "Sequence"),
            chunk: urids.uri(ATOM, "Chunk"),
            float: urids.uri(ATOM, "Float"),
            int: urids.uri(ATOM, "Int"),
            long: urids.uri(ATOM, "Long"),
            object: urids.uri(ATOM, "Object"),
            position: urids.uri(TIME, "Position"),
            frame: urids.uri(TIME, "frame"),
            speed: urids.uri(TIME, "speed"),
            bar: urids.uri(TIME, "bar"),
            bar_beat: urids.uri(TIME, "barBeat"),
            beat_unit: urids.uri(TIME, "beatUnit"),
            beats_per_bar: urids.uri(TIME, "beatsPerBar"),
            beats_per_minute: urids.uri(TIME, "beatsPerMinute"),
        };
        let shared = Shared {
            nodes,
            known,
            map: sys::UridMap {
                handle,
                map: urid_map,
            },
            unmap: sys::UridUnmap {
                handle,
                unmap: urid_unmap,
            },
            urids,
            api,
            world,
            lock: Mutex::new(()),
        };
        Ok(World(Arc::new(shared)))
    }

    /// Finds every bundle in the folders `LV2_PATH` names, or the standard
    /// ones when it's unset.
    pub fn load_all(&self) {
        let _lock = self.0.lock();
        // SAFETY: the world is alive and locked.
        unsafe { (self.0.api.lilv_world_load_all)(self.0.world) }
    }

    /// Loads the bundle folder `bundle`, wherever it is.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] when the path isn't text.
    pub fn load_bundle(&self, bundle: &Path) -> Result<(), Lv2Error> {
        let mut path = bundle
            .to_str()
            .ok_or_else(|| failed("the bundle's path isn't text"))?
            .to_owned();
        if !path.ends_with('/') {
            path.push('/');
        }
        let path = CString::new(path).map_err(|_| failed("the bundle's path has a NUL"))?;
        let _lock = self.0.lock();
        let api = &self.0.api;
        // SAFETY: the world is alive and locked; the node is freed here.
        unsafe {
            let uri = (api.lilv_new_file_uri)(self.0.world, ptr::null(), path.as_ptr());
            (api.lilv_world_load_bundle)(self.0.world, uri);
            (api.lilv_node_free)(uri);
        }
        Ok(())
    }

    /// Every plug-in found so far.
    pub fn plugins(&self) -> Vec<PluginInfo> {
        let _lock = self.0.lock();
        let api = &self.0.api;
        let mut found = Vec::new();
        // SAFETY: the world is alive and locked; the iterator is lilv's.
        unsafe {
            let all = (api.lilv_world_get_all_plugins)(self.0.world);
            let mut it = (api.lilv_plugins_begin)(all);
            while !(api.lilv_plugins_is_end)(all, it) {
                found.push(self.0.describe((api.lilv_plugins_get)(all, it)));
                it = (api.lilv_plugins_next)(all, it);
            }
        }
        found
    }

    /// The plug-in `uri`, if the world knows it.
    pub fn plugin(&self, uri: &str) -> Option<PluginInfo> {
        let uri = CString::new(uri).ok()?;
        let _lock = self.0.lock();
        let api = &self.0.api;
        // SAFETY: the world is alive and locked; the node is freed here.
        unsafe {
            let node = self.0.node(&uri);
            let plugin =
                (api.lilv_plugins_get_by_uri)((api.lilv_world_get_all_plugins)(self.0.world), node);
            (api.lilv_node_free)(node);
            (!plugin.is_null()).then(|| self.0.describe(plugin))
        }
    }

    /// Instantiates the plug-in `uri` at `sample_rate`, for blocks of up to
    /// `max_frames`, with its ports connected and the plug-in activated.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] when no such plug-in is known or it won't
    /// instantiate (it may need a feature the host lacks).
    pub fn instantiate(
        &self,
        uri: &str,
        sample_rate: f64,
        max_frames: usize,
    ) -> Result<Instance, Lv2Error> {
        let shared = &self.0;
        let uri_c = CString::new(uri).map_err(|_| failed("the URI has a NUL"))?;
        let _lock = shared.lock();
        let api = &shared.api;
        let mut features = Features::new(
            &shared.urids,
            shared.map_ptr(),
            shared.unmap_ptr(),
            sample_rate,
            max_frames,
            ATOM_WORDS * 8,
        );
        // SAFETY: the world is alive and locked; the features outlive the call.
        let (plugin, info, raw) = unsafe {
            let node = shared.node(&uri_c);
            let plugin =
                (api.lilv_plugins_get_by_uri)((api.lilv_world_get_all_plugins)(shared.world), node);
            (api.lilv_node_free)(node);
            if plugin.is_null() {
                return Err(failed(format!("no LV2 plug-in {uri}")));
            }
            let raw = (api.lilv_plugin_instantiate)(plugin, sample_rate, features.list.as_ptr());
            if raw.is_null() {
                return Err(failed(format!("{uri} won't instantiate")));
            }
            (plugin, shared.describe(plugin), raw)
        };
        // SAFETY: as above.
        let latency = unsafe {
            (api.lilv_plugin_has_latency)(plugin)
                .then(|| (api.lilv_plugin_get_latency_port_index)(plugin) as usize)
        };
        // A plug-in with a worker gets a thread for it, stopped before the
        // plug-in is freed.
        // SAFETY: `raw` is live; `extension_data` answers for the plug-in.
        let interface = unsafe {
            (*(*raw).descriptor)
                .extension_data
                .map_or(ptr::null(), |f| f(WORKER_INTERFACE.as_ptr()))
        }
        .cast::<sys::WorkerInterface>();
        // SAFETY: `raw` is live.
        let instance_handle = unsafe { (*raw).handle };
        let (worker, responses) = match (
            interface.is_null(),
            features.jobs.take(),
            features.responses_in.take(),
        ) {
            (false, Some(jobs), Some(responses_in)) => (
                Some(WorkerThread::start(
                    interface,
                    instance_handle,
                    jobs,
                    responses_in,
                )),
                features.responses_out.take(),
            ),
            _ => (None, None),
        };
        let has_worker = worker.is_some();
        let ports: Arc<[PortInfo]> = info.ports.into();
        let core = Arc::new(Core {
            shared: Arc::clone(shared),
            plugin,
            raw,
            features,
            worker,
        });
        let controls: Vec<f32> = ports.iter().map(|p| p.default).collect();
        let mirror: Arc<[AtomicU32]> = controls
            .iter()
            .map(|c| AtomicU32::new(c.to_bits()))
            .collect();
        let mut instance = Instance {
            core,
            controls,
            mirror,
            atoms: Vec::new(),
            silence: vec![0.0; max_frames].into(),
            sink: vec![0.0; max_frames].into(),
            latency,
            max_frames,
            transport: None,
            responses,
            interface: if has_worker { interface } else { ptr::null() },
            ports,
        };
        for (at, port) in instance.ports.iter().enumerate() {
            if let PortKind::Atom { midi } = port.kind {
                instance.atoms.push(AtomPort {
                    port: at,
                    input: port.input,
                    midi,
                    position: port.position,
                    words: vec![0; ATOM_WORDS].into(),
                });
            }
        }
        instance.connect(&[], &mut []);
        // SAFETY: the instance is live and its ports are connected.
        unsafe {
            if let Some(activate) = (*(*raw).descriptor).activate {
                activate((*raw).handle);
            }
        }
        Ok(instance)
    }
}

/// A lilv instance, freed when the last holder lets go: the [`Instance`]
/// that runs it, and the [`StateHandle`]s that save its state.
struct Core {
    shared: Arc<Shared>,
    plugin: sys::Plugin,
    raw: *mut sys::Instance,
    /// What the plug-in was given; it must outlive the plug-in.
    features: Features,
    worker: Option<WorkerThread>,
}

// SAFETY: `run` is called by one thread at a time (the `Instance`'s owner),
// and the state interface's `save` may run beside it (the state extension).
unsafe impl Send for Core {}
// SAFETY: as above.
unsafe impl Sync for Core {}

impl Drop for Core {
    fn drop(&mut self) {
        // The worker stops before the plug-in goes.
        self.worker.take();
        // Freeing touches the world's libraries, and this may run on the
        // thread that drops nodes.
        let _lock = self.shared.lock();
        // SAFETY: `raw` is this value's, live until freed here.
        unsafe {
            if let Some(deactivate) = (*(*self.raw).descriptor).deactivate {
                deactivate((*self.raw).handle);
            }
            (self.shared.api.lilv_instance_free)(self.raw);
        }
    }
}

struct AtomPort {
    /// The index into the instance's ports.
    port: usize,
    input: bool,
    midi: bool,
    position: bool,
    /// 64-bit words, so the atoms in it are aligned.
    words: Box<[u64]>,
}

/// One running plug-in. Not `Sync`: one thread runs it at a time.
pub struct Instance {
    core: Arc<Core>,
    ports: Arc<[PortInfo]>,
    /// One slot per port; only control ports' slots are connected.
    controls: Vec<f32>,
    /// The control slots as of the end of the last block, for the threads
    /// that don't run the plug-in.
    mirror: Arc<[AtomicU32]>,
    atoms: Vec<AtomPort>,
    silence: Box<[f32]>,
    sink: Box<[f32]>,
    latency: Option<usize>,
    max_frames: usize,
    transport: Option<Transport>,
    /// The worker's answers, delivered at the start of each block.
    responses: Option<Consumer<Msg>>,
    /// The plug-in's worker interface, if it has one.
    interface: *const sys::WorkerInterface,
}

// SAFETY: the plug-in's handle is used by one thread at a time (`&mut self`
// runs it), and moving it between threads is what LV2 hosts do.
unsafe impl Send for Instance {}

impl Instance {
    /// The plug-in's ports, in index order.
    pub fn ports(&self) -> &[PortInfo] {
        &self.ports
    }

    /// Sets a control input port's value for the next block.
    pub fn set_control(&mut self, index: u32, value: f32) {
        if let Some(slot) = self.controls.get_mut(index as usize) {
            *slot = value;
            self.mirror[index as usize].store(value.to_bits(), Relaxed);
        }
    }

    /// A control port's value: what the host set, or an output the
    /// plug-in wrote in its last block.
    pub fn control(&self, index: u32) -> f32 {
        self.controls.get(index as usize).copied().unwrap_or(0.0)
    }

    /// The latency the plug-in reports, in frames; none if it has no latency
    /// port. Plug-ins write it when they activate or run.
    pub fn latency(&self) -> u32 {
        self.latency
            .map_or(0, |port| self.controls[port].max(0.0) as u32)
    }

    /// Where the song is for the next block's `time:Position`, for the
    /// plug-ins that take it; none sends none.
    pub fn set_transport(&mut self, transport: Option<Transport>) {
        self.transport = transport;
    }

    /// A handle that saves this plug-in's state while it runs, and reads
    /// what it did in its last block, from any thread.
    pub fn handle(&self) -> StateHandle {
        StateHandle {
            core: Arc::clone(&self.core),
            ports: Arc::clone(&self.ports),
            mirror: Arc::clone(&self.mirror),
            latency: self.latency,
        }
    }

    fn connect(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        let (mut next_in, mut next_out) = (0, 0);
        let raw = self.core.raw;
        // SAFETY: `raw` is live. Every buffer is at least `max_frames` long
        // (or the caller's, checked by `run`) and outlives the block; the
        // plug-in reads or writes each only inside `run`.
        unsafe {
            let (descriptor, handle) = ((*raw).descriptor, (*raw).handle);
            for (at, port) in self.ports.iter().enumerate() {
                let data: *mut c_void = match port.kind {
                    PortKind::Control => self.controls.as_mut_ptr().add(at).cast(),
                    PortKind::Atom { .. } => self
                        .atoms
                        .iter_mut()
                        .find(|a| a.port == at)
                        .map_or(ptr::null_mut(), |a| a.words.as_mut_ptr().cast()),
                    PortKind::Audio if port.input => {
                        next_in += 1;
                        inputs
                            .get(next_in - 1)
                            .map_or(self.silence.as_ptr(), |s| s.as_ptr())
                            .cast_mut()
                            .cast()
                    }
                    PortKind::Audio => {
                        next_out += 1;
                        outputs
                            .get_mut(next_out - 1)
                            .map_or(self.sink.as_mut_ptr(), |s| s.as_mut_ptr())
                            .cast()
                    }
                    PortKind::Other if port.input => self.silence.as_ptr().cast_mut().cast(),
                    PortKind::Other => self.sink.as_mut_ptr().cast(),
                };
                ((*descriptor).connect_port)(handle, port.index, data);
            }
        }
    }

    /// Runs the plug-in over `frames` frames: its audio input ports read
    /// `inputs` in order, its audio output ports write `outputs`, and its
    /// MIDI input port gets `midi` (events in frame order). A port with no
    /// buffer left hears silence, or writes where nothing reads. No
    /// allocation, no lock.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::BadBlock`] and [`Lv2Error::TooManyEvents`], before the
    /// plug-in runs.
    pub fn run(
        &mut self,
        frames: usize,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
        midi: &[MidiEvent],
    ) -> Result<(), Lv2Error> {
        if frames > self.max_frames
            || inputs.iter().any(|s| s.len() < frames)
            || outputs.iter().any(|s| s.len() < frames)
        {
            return Err(Lv2Error::BadBlock);
        }
        let shared = Arc::clone(&self.core.shared);
        let known = &shared.known;
        let transport = self.transport;
        for atom in &mut self.atoms {
            if atom.input && (atom.midi || atom.position) {
                let position = transport.filter(|_| atom.position);
                let midi = if atom.midi { midi } else { &[] };
                write_events(&mut atom.words, known, position.as_ref(), midi)?;
            } else if atom.input {
                write_header(&mut atom.words, known.sequence, 8);
            } else {
                write_header(&mut atom.words, known.chunk, (ATOM_WORDS * 8 - 8) as u32);
            }
        }
        self.connect(inputs, outputs);
        let raw = self.core.raw;
        // The worker's answers come first, in the thread that runs the
        // plug-in, and it is told when they are all in.
        // SAFETY: `raw` is live and activated, its ports connected above;
        // the interface is the plug-in's own.
        unsafe {
            let handle = (*raw).handle;
            let interface = self.interface.as_ref();
            if let (Some(interface), Some(responses)) = (interface, self.responses.as_mut()) {
                while let Ok(msg) = responses.pop() {
                    if let Some(response) = interface.work_response {
                        response(
                            handle,
                            msg.bytes().len() as u32,
                            msg.bytes().as_ptr().cast(),
                        );
                    }
                }
            }
            ((*(*raw).descriptor).run)(handle, frames as u32);
            if let Some(end_run) = interface.and_then(|i| i.end_run) {
                end_run(handle);
            }
        }
        for (slot, value) in self.mirror.iter().zip(&self.controls) {
            slot.store(value.to_bits(), Relaxed);
        }
        Ok(())
    }

    /// The MIDI the plug-in sent in its last block, in frame order.
    pub fn midi_out(&self) -> MidiOut<'_> {
        let known = &self.core.shared.known;
        let bytes = self
            .atoms
            .iter()
            .find(|a| !a.input && a.midi)
            .map_or(&[][..], |a| bytes_of(&a.words));
        let type_ = u32::from_ne_bytes(
            bytes
                .get(4..8)
                .map_or([0; 4], |b| b.try_into().expect("4 bytes")),
        );
        let size = u32::from_ne_bytes(
            bytes
                .get(0..4)
                .map_or([0; 4], |b| b.try_into().expect("4 bytes")),
        );
        // After the atom's header and the sequence's own: the events.
        let end = (8 + size as usize).min(bytes.len());
        MidiOut {
            bytes: if type_ == known.sequence {
                &bytes[..end]
            } else {
                &[]
            },
            at: 16,
            midi_event: known.midi_event,
        }
    }

    /// The plug-in's state as text: its control inputs and whatever its own
    /// state interface stores. LV2 lets a plug-in save while it runs, so
    /// this can be called from any thread through a
    /// [`handle`](Self::handle) too.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] when lilv can't read the state or write it.
    pub fn save_state(&self) -> Result<String, Lv2Error> {
        save(&self.core, &self.ports, self.controls.as_ptr().cast_mut())
    }

    /// Restores a state [`save_state`](Self::save_state) wrote: the control
    /// inputs, and the plug-in's own through its state interface. Not while
    /// another thread runs the plug-in.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] when the text isn't a state.
    pub fn restore_state(&mut self, text: &str) -> Result<(), Lv2Error> {
        let text = CString::new(text).map_err(|_| failed("the state has a NUL"))?;
        let mut ctx = PortValues {
            ports: &self.ports,
            controls: self.controls.as_mut_ptr(),
            float: self.core.shared.known.float,
        };
        let shared = &self.core.shared;
        let api = &shared.api;
        let _lock = shared.lock();
        // SAFETY: as `save`.
        unsafe {
            let state =
                (api.lilv_state_new_from_string)(shared.world, shared.map_ptr(), text.as_ptr());
            if state.is_null() {
                return Err(failed("the state isn't one lilv can read"));
            }
            (api.lilv_state_restore)(
                state,
                self.core.raw,
                Some(set_port_value),
                ptr::from_mut(&mut ctx).cast(),
                0,
                self.core.features.state_list.as_ptr(),
            );
            (api.lilv_state_free)(state);
        }
        for (slot, value) in self.mirror.iter().zip(&self.controls) {
            slot.store(value.to_bits(), Relaxed);
        }
        Ok(())
    }
}

/// Saves a plug-in's state through lilv, its control inputs read from
/// `controls`.
fn save(core: &Core, ports: &[PortInfo], controls: *mut f32) -> Result<String, Lv2Error> {
    let mut ctx = PortValues {
        ports,
        controls,
        float: core.shared.known.float,
    };
    let shared = &core.shared;
    let api = &shared.api;
    let _lock = shared.lock();
    // SAFETY: the world is alive and locked; `ctx` outlives the calls, and
    // lilv gives back a state and a string it owns until freed here.
    unsafe {
        let state = (api.lilv_state_new_from_instance)(
            core.plugin,
            core.raw,
            shared.map_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            Some(get_port_value),
            ptr::from_mut(&mut ctx).cast(),
            STATE_FLAGS,
            core.features.state_list.as_ptr(),
        );
        if state.is_null() {
            return Err(failed("the plug-in's state couldn't be read"));
        }
        let text = (api.lilv_state_to_string)(
            shared.world,
            shared.map_ptr(),
            shared.unmap_ptr(),
            state,
            STATE_URI.as_ptr(),
            ptr::null(),
        );
        (api.lilv_state_free)(state);
        if text.is_null() {
            return Err(failed("the plug-in's state couldn't be written"));
        }
        let saved = CStr::from_ptr(text).to_string_lossy().into_owned();
        (api.lilv_free)(text.cast());
        Ok(saved)
    }
}

/// What the main thread keeps of a running [`Instance`]: it saves the
/// plug-in's state beside the audio thread (LV2's state extension allows
/// it) and reads the control values the plug-in had after its last block.
/// It keeps the plug-in alive, so it never dangles.
#[derive(Clone)]
pub struct StateHandle {
    core: Arc<Core>,
    ports: Arc<[PortInfo]>,
    mirror: Arc<[AtomicU32]>,
    latency: Option<usize>,
}

impl StateHandle {
    /// The plug-in's ports, in index order.
    pub fn ports(&self) -> &[PortInfo] {
        &self.ports
    }

    fn ports_shared(&self) -> Arc<[PortInfo]> {
        Arc::clone(&self.ports)
    }

    /// A control port's value as of the end of the plug-in's last block.
    pub fn control(&self, index: u32) -> f32 {
        self.mirror
            .get(index as usize)
            .map_or(0.0, |slot| f32::from_bits(slot.load(Relaxed)))
    }

    /// The latency the plug-in reported in its last block, in frames.
    pub fn latency(&self) -> u32 {
        self.latency
            .map_or(0, |port| self.control(port as u32).max(0.0) as u32)
    }

    /// What an editor of the plug-in is given: the URID map and unmap, and the
    /// plug-in's own handle, for the UIs that ask for instance access.
    fn ui_support(&self) -> ui::Support {
        ui::Support {
            map: self.core.shared.map_ptr(),
            unmap: self.core.shared.unmap_ptr(),
            // SAFETY: `raw` is live as long as the core is.
            instance: unsafe { (*self.core.raw).handle },
        }
    }

    /// The plug-in's state as text, as [`Instance::save_state`].
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] when lilv can't read the state or write it.
    pub fn save_state(&self) -> Result<String, Lv2Error> {
        let mut controls: Vec<f32> = self
            .mirror
            .iter()
            .map(|slot| f32::from_bits(slot.load(Relaxed)))
            .collect();
        save(&self.core, &self.ports, controls.as_mut_ptr())
    }
}

/// What lilv's port-value callbacks reach: the instance's control slots by
/// port symbol.
struct PortValues<'a> {
    ports: &'a [PortInfo],
    controls: *mut f32,
    float: Urid,
}

impl PortValues<'_> {
    fn slot(&self, symbol: *const c_char) -> Option<*mut f32> {
        // SAFETY: lilv passes a C string.
        let symbol = unsafe { CStr::from_ptr(symbol) }.to_str().ok()?;
        let at = self
            .ports
            .iter()
            .position(|p| p.symbol == symbol && p.kind == PortKind::Control)?;
        // SAFETY: `controls` has a slot for every port.
        Some(unsafe { self.controls.add(at) })
    }
}

unsafe extern "C" fn get_port_value(
    symbol: *const c_char,
    user: *mut c_void,
    size: *mut u32,
    type_: *mut u32,
) -> *const c_void {
    // SAFETY: `user` is the `PortValues` of the call that passed this.
    let ctx = unsafe { &*user.cast::<PortValues<'_>>() };
    ctx.slot(symbol).map_or(ptr::null(), |slot| {
        // SAFETY: lilv gives valid out-pointers.
        unsafe {
            *size = 4;
            *type_ = ctx.float;
        }
        slot.cast_const().cast()
    })
}

unsafe extern "C" fn set_port_value(
    symbol: *const c_char,
    user: *mut c_void,
    value: *const c_void,
    size: u32,
    type_: u32,
) {
    // SAFETY: as `get_port_value`; `value` holds `size` bytes.
    unsafe {
        let ctx = &*user.cast::<PortValues<'_>>();
        if let Some(slot) = ctx.slot(symbol)
            && type_ == ctx.float
            && size == 4
        {
            *slot = value.cast::<f32>().read_unaligned();
        }
    }
}

fn bytes_of(words: &[u64]) -> &[u8] {
    // SAFETY: any `u64`s are valid bytes, and a byte slice needs less
    // alignment.
    unsafe { slice::from_raw_parts(words.as_ptr().cast(), words.len() * 8) }
}

fn bytes_of_mut(words: &mut [u64]) -> &mut [u8] {
    // SAFETY: as `bytes_of`.
    unsafe { slice::from_raw_parts_mut(words.as_mut_ptr().cast(), words.len() * 8) }
}

/// An atom's header: its body's size and type.
fn write_header(words: &mut [u64], type_: Urid, size: u32) {
    let bytes = bytes_of_mut(words);
    bytes[..4].copy_from_slice(&size.to_ne_bytes());
    bytes[4..8].copy_from_slice(&type_.to_ne_bytes());
    // The sequence body's unit (0 is frames) and padding.
    bytes[8..16].fill(0);
}

/// One property of an `atom:Object`, at `at`: its key, and a value of
/// `type_` that is `value`'s bytes. Returns where the next one goes.
fn write_property(bytes: &mut [u8], at: usize, key: Urid, type_: Urid, value: &[u8]) -> usize {
    bytes[at..at + 4].copy_from_slice(&key.to_ne_bytes());
    // The property's context, unused.
    bytes[at + 4..at + 8].fill(0);
    bytes[at + 8..at + 12].copy_from_slice(&(value.len() as u32).to_ne_bytes());
    bytes[at + 12..at + 16].copy_from_slice(&type_.to_ne_bytes());
    bytes[at + 16..at + 16 + value.len()].copy_from_slice(value);
    let end = at + 16 + value.len();
    let padded = end.next_multiple_of(8);
    bytes[end..padded].fill(0);
    padded
}

/// The `time:Position` object for `transport` as a sequence event at frame
/// 0, written at `at`. Returns where the next event goes.
fn write_position(bytes: &mut [u8], mut at: usize, known: &Known, transport: &Transport) -> usize {
    let start = at;
    bytes[at..at + 8].copy_from_slice(&0i64.to_ne_bytes());
    // The object atom's header: its size is filled in below.
    bytes[at + 12..at + 16].copy_from_slice(&known.object.to_ne_bytes());
    // The object's body: no ID, and its type.
    bytes[at + 16..at + 20].fill(0);
    bytes[at + 20..at + 24].copy_from_slice(&known.position.to_ne_bytes());
    at += 24;
    let long = |value: i64| value.to_ne_bytes();
    let float = |value: f32| {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(&value.to_ne_bytes());
        bytes
    };
    at = write_property(bytes, at, known.frame, known.long, &long(transport.frame));
    at = write_property(
        bytes,
        at,
        known.speed,
        known.float,
        &float(if transport.playing { 1.0 } else { 0.0 }),
    );
    at = write_property(bytes, at, known.bar, known.long, &long(transport.bar));
    at = write_property(
        bytes,
        at,
        known.bar_beat,
        known.float,
        &float(transport.bar_beat),
    );
    at = write_property(
        bytes,
        at,
        known.beat_unit,
        known.int,
        &transport.beat_unit.to_ne_bytes(),
    );
    at = write_property(
        bytes,
        at,
        known.beats_per_bar,
        known.float,
        &float(transport.beats_per_bar),
    );
    at = write_property(
        bytes,
        at,
        known.beats_per_minute,
        known.float,
        &float(transport.bpm),
    );
    let size = (at - start - 16) as u32;
    bytes[start + 8..start + 12].copy_from_slice(&size.to_ne_bytes());
    at
}

/// An `atom:Sequence` of the position (when there is one) and `events`,
/// each a `midi:MidiEvent` on its frame.
fn write_events(
    words: &mut [u64],
    known: &Known,
    position: Option<&Transport>,
    events: &[MidiEvent],
) -> Result<(), Lv2Error> {
    let bytes = bytes_of_mut(words);
    let mut at = 16;
    if let Some(transport) = position {
        at = write_position(bytes, at, known, transport);
    }
    for event in events {
        let len = usize::from(event.len.min(3));
        if at + 16 + 8 > bytes.len() {
            return Err(Lv2Error::TooManyEvents);
        }
        bytes[at..at + 8].copy_from_slice(&i64::from(event.frame).to_ne_bytes());
        bytes[at + 8..at + 12].copy_from_slice(&(len as u32).to_ne_bytes());
        bytes[at + 12..at + 16].copy_from_slice(&known.midi_event.to_ne_bytes());
        bytes[at + 16..at + 24].fill(0);
        bytes[at + 16..at + 16 + len].copy_from_slice(&event.data[..len]);
        at += 24;
    }
    write_header(words, known.sequence, (at - 8) as u32);
    Ok(())
}

/// The MIDI events in an Atom sequence a plug-in wrote.
pub struct MidiOut<'a> {
    bytes: &'a [u8],
    at: usize,
    midi_event: Urid,
}

impl Iterator for MidiOut<'_> {
    type Item = MidiEvent;

    fn next(&mut self) -> Option<MidiEvent> {
        while self.at + 16 <= self.bytes.len() {
            let head = &self.bytes[self.at..self.at + 16];
            let frame = i64::from_ne_bytes(head[..8].try_into().expect("8 bytes"));
            let size = u32::from_ne_bytes(head[8..12].try_into().expect("4 bytes")) as usize;
            let type_ = u32::from_ne_bytes(head[12..16].try_into().expect("4 bytes"));
            let body = self.bytes.get(self.at + 16..self.at + 16 + size);
            self.at += (16 + size).next_multiple_of(8);
            if let (true, Some(body), 1..=3) = (type_ == self.midi_event, body, size) {
                let mut data = [0; 3];
                data[..size].copy_from_slice(body);
                return Some(MidiEvent {
                    frame: u32::try_from(frame).unwrap_or(0),
                    len: size as u8,
                    data,
                });
            }
        }
        None
    }
}
