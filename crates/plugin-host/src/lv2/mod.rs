//! LV2 hosting, part 1 (WP-5.39): Beatlayer's own host on the system's lilv
//! library, beside the CLAP, VST3 and Audio Unit hosts. A [`World`] finds
//! the LV2 plug-ins on the machine and instantiates one at the engine's
//! rate as an [`Instance`]: audio and control ports connected, MIDI in and
//! out through Atom ports, latency read from the plug-in's latency port,
//! and state saved and restored as the text lilv writes.
//!
//! lilv is loaded from the system when a [`World`] is made, not linked, so
//! a machine without it runs Beatlayer with LV2 unavailable
//! ([`Lv2Error::Unavailable`]); only Linux looks for it, the other
//! platforms always answer so. The device, the scanner and the browser are
//! the next part's (the LV2 hosting spec).
//!
//! An [`Instance`] is made, activated and dropped off the audio thread;
//! [`Instance::run`], [`Instance::set_control`] and [`Instance::midi_out`]
//! allocate nothing and take no lock. Saving and restoring state are
//! main-thread calls, and so is every call on a [`World`]: lilv's world isn't
//! thread-safe, so each one holds the world's lock.

mod sys;

use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt;
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sys::{Api, Feature, Urid};

const CORE: &str = "http://lv2plug.in/ns/lv2core#";
const ATOM: &str = "http://lv2plug.in/ns/ext/atom#";
const URID: &str = "http://lv2plug.in/ns/ext/urid#";
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
}

impl fmt::Display for Lv2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("LV2 isn't available: lilv wasn't found"),
            Self::Failed(why) => f.write_str(why),
            Self::BadBlock => f.write_str("the block is longer than the instance's maximum"),
            Self::TooManyEvents => f.write_str("the MIDI events don't fit the Atom port"),
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
    pub kind: PortKind,
    pub input: bool,
    /// A control port's range and default; zero where the plug-in gives none.
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

/// One plug-in a [`World`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginInfo {
    pub uri: String,
    pub name: String,
    pub ports: Vec<PortInfo>,
}

/// A MIDI message on a frame of the block: a channel message of up to
/// three bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    pub frame: u32,
    pub len: u8,
    pub data: [u8; 3],
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

/// lilv's world, the library it was loaded from, and the class nodes the
/// host asks ports about.
struct Shared {
    api: Api,
    world: sys::World,
    /// Held across every call that reaches the world, its plug-ins or its
    /// nodes.
    lock: Mutex<()>,
    /// The map's handle: kept here so it outlives the plug-ins.
    _urids: Box<Urids>,
    map: sys::UridMap,
    unmap: sys::UridUnmap,
    audio: sys::Node,
    control: sys::Node,
    input: sys::Node,
    atom: sys::Node,
    midi: sys::Node,
    midi_event: Urid,
    sequence: Urid,
    chunk: Urid,
    float: Urid,
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

    /// A numeric node's value, and the node freed; `fallback` for no node.
    fn take_number(&self, node: sys::Node, fallback: f32) -> f32 {
        if node.is_null() {
            return fallback;
        }
        // SAFETY: `node` is one lilv gave to free.
        unsafe {
            let number = ((self.api.lilv_node_is_float)(node) || (self.api.lilv_node_is_int)(node))
                .then(|| (self.api.lilv_node_as_float)(node));
            (self.api.lilv_node_free)(node);
            number.unwrap_or(fallback)
        }
    }

    fn describe(&self, plugin: sys::Plugin) -> PluginInfo {
        let api = &self.api;
        // SAFETY: `plugin` is one of the world's, and the caller holds the lock.
        unsafe {
            let name = (api.lilv_plugin_get_name)(plugin);
            let info = PluginInfo {
                uri: self.string((api.lilv_plugin_get_uri)(plugin)),
                name: self.string(name),
                ports: (0..(api.lilv_plugin_get_num_ports)(plugin))
                    .map(|i| {
                        self.describe_port(
                            plugin,
                            (api.lilv_plugin_get_port_by_index)(plugin, i),
                            i,
                        )
                    })
                    .collect(),
            };
            if !name.is_null() {
                (api.lilv_node_free)(name);
            }
            info
        }
    }

    fn describe_port(&self, plugin: sys::Plugin, port: sys::Port, index: u32) -> PortInfo {
        let api = &self.api;
        // SAFETY: as `describe`.
        unsafe {
            let is = |class| (api.lilv_port_is_a)(plugin, port, class);
            let kind = if is(self.audio) {
                PortKind::Audio
            } else if is(self.control) {
                PortKind::Control
            } else if is(self.atom) {
                PortKind::Atom {
                    midi: (api.lilv_port_supports_event)(plugin, port, self.midi),
                }
            } else {
                PortKind::Other
            };
            let (mut default, mut min, mut max) =
                (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            (api.lilv_port_get_range)(plugin, port, &raw mut default, &raw mut min, &raw mut max);
            PortInfo {
                index,
                symbol: self.string((api.lilv_port_get_symbol)(plugin, port)),
                kind,
                input: is(self.input),
                default: self.take_number(default, 0.0),
                min: self.take_number(min, 0.0),
                max: self.take_number(max, 1.0),
            }
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the nodes and the world are this value's.
        unsafe {
            for node in [self.audio, self.control, self.input, self.atom, self.midi] {
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
        let node = |uri: String| {
            let uri = CString::new(uri).expect("a URI without NUL");
            // SAFETY: the world was just made.
            unsafe { (api.lilv_new_uri)(world, uri.as_ptr()) }
        };
        let shared = Shared {
            audio: node(format!("{CORE}AudioPort")),
            control: node(format!("{CORE}ControlPort")),
            input: node(format!("{CORE}InputPort")),
            atom: node(format!("{ATOM}AtomPort")),
            midi: node("http://lv2plug.in/ns/ext/midi#MidiEvent".into()),
            midi_event: urids.map(c"http://lv2plug.in/ns/ext/midi#MidiEvent"),
            sequence: urids.map(c"http://lv2plug.in/ns/ext/atom#Sequence"),
            chunk: urids.map(c"http://lv2plug.in/ns/ext/atom#Chunk"),
            float: urids.map(c"http://lv2plug.in/ns/ext/atom#Float"),
            map: sys::UridMap {
                handle,
                map: urid_map,
            },
            unmap: sys::UridUnmap {
                handle,
                unmap: urid_unmap,
            },
            _urids: urids,
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
        let map_uri = CString::new(format!("{URID}map")).expect("a URI");
        let unmap_uri = CString::new(format!("{URID}unmap")).expect("a URI");
        let features = [
            Feature {
                uri: map_uri.as_ptr(),
                data: shared.map_ptr().cast(),
            },
            Feature {
                uri: unmap_uri.as_ptr(),
                data: ptr::from_ref(&shared.unmap).cast_mut().cast(),
            },
        ];
        let list = [&raw const features[0], &raw const features[1], ptr::null()];
        // SAFETY: the world is alive and locked; the features outlive the call.
        let (plugin, info, raw) = unsafe {
            let node = shared.node(&uri_c);
            let plugin =
                (api.lilv_plugins_get_by_uri)((api.lilv_world_get_all_plugins)(shared.world), node);
            (api.lilv_node_free)(node);
            if plugin.is_null() {
                return Err(failed(format!("no LV2 plug-in {uri}")));
            }
            let raw = (api.lilv_plugin_instantiate)(plugin, sample_rate, list.as_ptr());
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
        let mut instance = Instance {
            shared: Arc::clone(shared),
            plugin,
            raw,
            controls: info.ports.iter().map(|p| p.default).collect(),
            atoms: Vec::new(),
            silence: vec![0.0; max_frames].into(),
            sink: vec![0.0; max_frames].into(),
            latency,
            max_frames,
            active: false,
            ports: info.ports,
        };
        for (at, port) in instance.ports.iter().enumerate() {
            if let PortKind::Atom { midi } = port.kind {
                let role = match (midi, port.input) {
                    (true, true) => Role::MidiIn,
                    (true, false) => Role::MidiOut,
                    _ => Role::Other,
                };
                instance.atoms.push(AtomPort {
                    port: at,
                    role,
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
        instance.active = true;
        Ok(instance)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    MidiIn,
    MidiOut,
    /// An Atom port the host doesn't use: an empty sequence in, a chunk out.
    Other,
}

struct AtomPort {
    /// The index into the instance's ports.
    port: usize,
    role: Role,
    /// 64-bit words, so the atoms in it are aligned.
    words: Box<[u64]>,
}

/// One running plug-in. Not `Sync`: one thread runs it at a time.
pub struct Instance {
    shared: Arc<Shared>,
    plugin: sys::Plugin,
    raw: *mut sys::Instance,
    ports: Vec<PortInfo>,
    /// One slot per port; only control ports' slots are connected.
    controls: Vec<f32>,
    atoms: Vec<AtomPort>,
    silence: Box<[f32]>,
    sink: Box<[f32]>,
    latency: Option<usize>,
    max_frames: usize,
    active: bool,
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

    fn connect(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        let (mut next_in, mut next_out) = (0, 0);
        // SAFETY: `raw` is live. Every buffer is at least `max_frames` long
        // (or the caller's, checked by `run`) and outlives the block; the
        // plug-in reads or writes each only inside `run`.
        unsafe {
            let (descriptor, handle) = ((*self.raw).descriptor, (*self.raw).handle);
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
        let (sequence, chunk) = (self.shared.sequence, self.shared.chunk);
        for atom in &mut self.atoms {
            match atom.role {
                Role::MidiIn => {
                    write_events(&mut atom.words, sequence, self.shared.midi_event, midi)?
                }
                Role::MidiOut => write_header(&mut atom.words, chunk, (ATOM_WORDS * 8 - 8) as u32),
                Role::Other if self.ports[atom.port].input => {
                    write_header(&mut atom.words, sequence, 8)
                }
                Role::Other => write_header(&mut atom.words, chunk, (ATOM_WORDS * 8 - 8) as u32),
            }
        }
        self.connect(inputs, outputs);
        // SAFETY: `raw` is live and activated, its ports connected above.
        unsafe { ((*(*self.raw).descriptor).run)((*self.raw).handle, frames as u32) };
        Ok(())
    }

    /// The MIDI the plug-in sent in its last block, in frame order.
    pub fn midi_out(&self) -> MidiOut<'_> {
        let bytes = self
            .atoms
            .iter()
            .find(|a| a.role == Role::MidiOut)
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
            bytes: if type_ == self.shared.sequence {
                &bytes[..end]
            } else {
                &[]
            },
            at: 16,
            midi_event: self.shared.midi_event,
        }
    }

    /// The plug-in's state as text: its control inputs and whatever its own
    /// state interface stores. Not while the audio thread runs the plug-in,
    /// unless it saves state thread-safely.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] when lilv can't read the state or write it.
    pub fn save_state(&self) -> Result<String, Lv2Error> {
        let mut ctx = PortValues {
            ports: &self.ports,
            controls: self.controls.as_ptr().cast_mut(),
            float: self.shared.float,
        };
        let shared = &self.shared;
        let api = &shared.api;
        let _lock = shared.lock();
        // SAFETY: the world is alive and locked; `ctx` outlives the calls,
        // and lilv gives back a state and a string it owns until freed here.
        unsafe {
            let state = (api.lilv_state_new_from_instance)(
                self.plugin,
                self.raw,
                shared.map_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                Some(get_port_value),
                ptr::from_mut(&mut ctx).cast(),
                STATE_FLAGS,
                ptr::null(),
            );
            if state.is_null() {
                return Err(failed("the plug-in's state couldn't be read"));
            }
            let text = (api.lilv_state_to_string)(
                shared.world,
                shared.map_ptr(),
                ptr::from_ref(&shared.unmap).cast_mut(),
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

    /// Restores a state [`save_state`](Self::save_state) wrote: the control
    /// inputs, and the plug-in's own through its state interface.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] when the text isn't a state.
    pub fn restore_state(&mut self, text: &str) -> Result<(), Lv2Error> {
        let text = CString::new(text).map_err(|_| failed("the state has a NUL"))?;
        let mut ctx = PortValues {
            ports: &self.ports,
            controls: self.controls.as_mut_ptr(),
            float: self.shared.float,
        };
        let shared = &self.shared;
        let api = &shared.api;
        let _lock = shared.lock();
        // SAFETY: as `save_state`.
        unsafe {
            let state =
                (api.lilv_state_new_from_string)(shared.world, shared.map_ptr(), text.as_ptr());
            if state.is_null() {
                return Err(failed("the state isn't one lilv can read"));
            }
            (api.lilv_state_restore)(
                state,
                self.raw,
                Some(set_port_value),
                ptr::from_mut(&mut ctx).cast(),
                0,
                ptr::null(),
            );
            (api.lilv_state_free)(state);
        }
        Ok(())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: `raw` is this value's, live until freed here.
        unsafe {
            if self.active
                && let Some(deactivate) = (*(*self.raw).descriptor).deactivate
            {
                deactivate((*self.raw).handle);
            }
            (self.shared.api.lilv_instance_free)(self.raw);
        }
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

/// An `atom:Sequence` of `events`, each a `midi:MidiEvent` on its frame.
fn write_events(
    words: &mut [u64],
    sequence: Urid,
    midi_event: Urid,
    events: &[MidiEvent],
) -> Result<(), Lv2Error> {
    let bytes = bytes_of_mut(words);
    let mut at = 16;
    for event in events {
        let len = usize::from(event.len.min(3));
        if at + 16 + 8 > bytes.len() {
            return Err(Lv2Error::TooManyEvents);
        }
        bytes[at..at + 8].copy_from_slice(&i64::from(event.frame).to_ne_bytes());
        bytes[at + 8..at + 12].copy_from_slice(&(len as u32).to_ne_bytes());
        bytes[at + 12..at + 16].copy_from_slice(&midi_event.to_ne_bytes());
        bytes[at + 16..at + 24].fill(0);
        bytes[at + 16..at + 16 + len].copy_from_slice(&event.data[..len]);
        at += 24;
    }
    write_header(words, sequence, (at - 8) as u32);
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
