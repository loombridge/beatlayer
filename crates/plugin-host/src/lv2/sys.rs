//! The part of lilv's C API the host calls, and the LV2 structs it passes,
//! written by hand and loaded from the system's `liblilv-0` when the host
//! starts (so a machine without lilv runs Beatlayer, with LV2 unavailable).

#![allow(non_camel_case_types, reason = "the C names")]

use std::ffi::{c_char, c_void};

use libloading::Library;

pub type World = *mut c_void;
pub type Node = *mut c_void;
pub type Plugins = *const c_void;
pub type Plugin = *const c_void;
pub type Port = *const c_void;
pub type State = *mut c_void;
pub type Iter = *mut c_void;
pub type Urid = u32;

/// `LV2_Feature`.
#[repr(C)]
pub struct Feature {
    pub uri: *const c_char,
    pub data: *mut c_void,
}

/// `LV2_URID_Map`.
#[repr(C)]
pub struct UridMap {
    pub handle: *mut c_void,
    pub map: unsafe extern "C" fn(*mut c_void, *const c_char) -> Urid,
}

/// `LV2_URID_Unmap`.
#[repr(C)]
pub struct UridUnmap {
    pub handle: *mut c_void,
    pub unmap: unsafe extern "C" fn(*mut c_void, Urid) -> *const c_char,
}

/// `LV2_Descriptor`.
#[repr(C)]
pub struct Descriptor {
    pub uri: *const c_char,
    pub instantiate: *const c_void,
    pub connect_port: unsafe extern "C" fn(*mut c_void, u32, *mut c_void),
    pub activate: Option<unsafe extern "C" fn(*mut c_void)>,
    pub run: unsafe extern "C" fn(*mut c_void, u32),
    pub deactivate: Option<unsafe extern "C" fn(*mut c_void)>,
    pub cleanup: *const c_void,
    pub extension_data: *const c_void,
}

/// `LilvInstance`, whose fields lilv's inline functions read; so does the
/// host, as they do.
#[repr(C)]
pub struct Instance {
    pub descriptor: *const Descriptor,
    pub handle: *mut c_void,
    pub pimpl: *mut c_void,
}

pub type GetPortValue =
    unsafe extern "C" fn(*const c_char, *mut c_void, *mut u32, *mut u32) -> *const c_void;
pub type SetPortValue = unsafe extern "C" fn(*const c_char, *mut c_void, *const c_void, u32, u32);

/// Declares the loaded table: one function pointer per lilv function.
macro_rules! lilv_api {
    ($($name:ident($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        pub struct Api {
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            // Last, so the pointers above never outlive the library.
            _library: Library,
        }

        impl Api {
            /// # Safety
            ///
            /// `library` must be lilv 0.24 or later.
            pub unsafe fn load(library: Library) -> Result<Api, libloading::Error> {
                Ok(Api {
                    // SAFETY: the signatures below are lilv.h's.
                    $($name: *unsafe { library.get(concat!(stringify!($name), "\0").as_bytes()) }?,)*
                    _library: library,
                })
            }
        }
    };
}

lilv_api! {
    lilv_world_new() -> World;
    lilv_world_free(World);
    lilv_world_load_all(World);
    lilv_world_load_bundle(World, Node);
    lilv_world_get_all_plugins(World) -> Plugins;
    lilv_new_uri(World, *const c_char) -> Node;
    lilv_new_file_uri(World, *const c_char, *const c_char) -> Node;
    lilv_node_free(Node);
    lilv_node_as_string(Node) -> *const c_char;
    lilv_node_is_float(Node) -> bool;
    lilv_node_is_int(Node) -> bool;
    lilv_node_as_float(Node) -> f32;
    lilv_plugins_begin(Plugins) -> Iter;
    lilv_plugins_is_end(Plugins, Iter) -> bool;
    lilv_plugins_get(Plugins, Iter) -> Plugin;
    lilv_plugins_next(Plugins, Iter) -> Iter;
    lilv_plugins_get_by_uri(Plugins, Node) -> Plugin;
    lilv_plugin_get_uri(Plugin) -> Node;
    lilv_plugin_get_name(Plugin) -> Node;
    lilv_plugin_get_num_ports(Plugin) -> u32;
    lilv_plugin_get_port_by_index(Plugin, u32) -> Port;
    lilv_plugin_has_latency(Plugin) -> bool;
    lilv_plugin_get_latency_port_index(Plugin) -> u32;
    lilv_plugin_instantiate(Plugin, f64, *const *const Feature) -> *mut Instance;
    lilv_instance_free(*mut Instance);
    lilv_port_is_a(Plugin, Port, Node) -> bool;
    lilv_port_supports_event(Plugin, Port, Node) -> bool;
    lilv_port_get_symbol(Plugin, Port) -> Node;
    lilv_port_get_range(Plugin, Port, *mut Node, *mut Node, *mut Node);
    lilv_state_new_from_instance(
        Plugin, *mut Instance, *mut UridMap, *const c_char, *const c_char, *const c_char,
        *const c_char, Option<GetPortValue>, *mut c_void, u32, *const *const Feature
    ) -> State;
    lilv_state_new_from_string(World, *mut UridMap, *const c_char) -> State;
    lilv_state_to_string(
        World, *mut UridMap, *mut UridUnmap, State, *const c_char, *const c_char
    ) -> *mut c_char;
    lilv_state_restore(
        State, *mut Instance, Option<SetPortValue>, *mut c_void, u32, *const *const Feature
    );
    lilv_state_free(State);
    lilv_free(*mut c_void);
}
