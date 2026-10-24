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
pub type Nodes = *mut c_void;
pub type ScalePoints = *mut c_void;
pub type ScalePoint = *const c_void;
pub type Uis = *mut c_void;
pub type Ui = *const c_void;

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
    pub extension_data: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
}

/// `LilvInstance`, whose fields lilv's inline functions read; so does the
/// host, as they do.
#[repr(C)]
pub struct Instance {
    pub descriptor: *const Descriptor,
    pub handle: *mut c_void,
    pub pimpl: *mut c_void,
}

/// `LV2_Options_Option`.
#[repr(C)]
pub struct OptionsOption {
    pub context: u32,
    pub subject: u32,
    pub key: Urid,
    pub size: u32,
    pub type_: Urid,
    pub value: *const c_void,
}

/// `LV2_Worker_Schedule`.
#[repr(C)]
pub struct WorkerSchedule {
    pub handle: *mut c_void,
    pub schedule_work: unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> i32,
}

pub type WorkerRespond = unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> i32;

/// `LV2_Worker_Interface`.
#[repr(C)]
pub struct WorkerInterface {
    pub work:
        unsafe extern "C" fn(*mut c_void, WorkerRespond, *mut c_void, u32, *const c_void) -> i32,
    pub work_response: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> i32>,
    pub end_run: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}

/// `LV2_Log_Log`. Its `printf` is variadic in C; the host's never reads the
/// arguments, so a plain function of the fixed ones stands in for it.
#[repr(C)]
pub struct LogLog {
    pub handle: *mut c_void,
    pub printf: unsafe extern "C" fn(*mut c_void, Urid, *const c_char) -> i32,
    pub vprintf: unsafe extern "C" fn(*mut c_void, Urid, *const c_char, *mut c_void) -> i32,
}

/// `LV2UI_Write_Function`.
pub type UiWrite = unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const c_void);

/// `LV2UI_Descriptor`.
#[repr(C)]
pub struct UiDescriptor {
    pub uri: *const c_char,
    pub instantiate: unsafe extern "C" fn(
        *const UiDescriptor,
        *const c_char,
        *const c_char,
        UiWrite,
        *mut c_void,
        *mut *mut c_void,
        *const *const Feature,
    ) -> *mut c_void,
    pub cleanup: unsafe extern "C" fn(*mut c_void),
    pub port_event: Option<unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const c_void)>,
    pub extension_data: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
}

/// `LV2UI_Resize`, a feature of the host's and an interface of the UI's.
#[repr(C)]
pub struct UiResize {
    pub handle: *mut c_void,
    pub ui_resize: unsafe extern "C" fn(*mut c_void, i32, i32) -> i32,
}

/// `LV2UI_Port_Map`.
#[repr(C)]
pub struct UiPortMap {
    pub handle: *mut c_void,
    pub port_index: unsafe extern "C" fn(*mut c_void, *const c_char) -> u32,
}

/// `LV2UI_Touch`.
#[repr(C)]
pub struct UiTouch {
    pub handle: *mut c_void,
    pub touch: unsafe extern "C" fn(*mut c_void, u32, bool),
}

/// `LV2UI_Idle_Interface`.
#[repr(C)]
pub struct UiIdle {
    pub idle: unsafe extern "C" fn(*mut c_void) -> i32,
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
    lilv_port_get_name(Plugin, Port) -> Node;
    lilv_port_has_property(Plugin, Port, Node) -> bool;
    lilv_port_get_value(Plugin, Port, Node) -> Nodes;
    lilv_port_get_scale_points(Plugin, Port) -> ScalePoints;
    lilv_scale_points_free(ScalePoints);
    lilv_scale_points_begin(ScalePoints) -> Iter;
    lilv_scale_points_is_end(ScalePoints, Iter) -> bool;
    lilv_scale_points_get(ScalePoints, Iter) -> ScalePoint;
    lilv_scale_points_next(ScalePoints, Iter) -> Iter;
    lilv_scale_point_get_label(ScalePoint) -> Node;
    lilv_scale_point_get_value(ScalePoint) -> Node;
    lilv_nodes_free(Nodes);
    lilv_nodes_begin(Nodes) -> Iter;
    lilv_nodes_is_end(Nodes, Iter) -> bool;
    lilv_nodes_get(Nodes, Iter) -> Node;
    lilv_nodes_contains(Nodes, Node) -> bool;
    lilv_node_is_uri(Node) -> bool;
    lilv_node_get_path(Node, *mut *mut c_char) -> *mut c_char;
    lilv_plugin_get_value(Plugin, Node) -> Nodes;
    lilv_plugin_get_author_name(Plugin) -> Node;
    lilv_plugin_get_bundle_uri(Plugin) -> Node;
    lilv_plugin_get_uis(Plugin) -> Uis;
    lilv_uis_free(Uis);
    lilv_uis_begin(Uis) -> Iter;
    lilv_uis_is_end(Uis, Iter) -> bool;
    lilv_uis_get(Uis, Iter) -> Ui;
    lilv_uis_next(Uis, Iter) -> Iter;
    lilv_ui_get_uri(Ui) -> Node;
    lilv_ui_is_a(Ui, Node) -> bool;
    lilv_ui_get_binary_uri(Ui) -> Node;
    lilv_ui_get_bundle_uri(Ui) -> Node;
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
