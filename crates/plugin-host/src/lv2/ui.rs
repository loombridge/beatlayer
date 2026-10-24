//! LV2 editors on X11 (the LV2 hosting spec, R13): the plug-in's X11 UI,
//! loaded from its own binary and embedded in Beatlayer's X11 host window as
//! a child of it (`ui:parent`), with the UI told when the window is resized
//! and asked to resize it (`ui:resize`), run by the host's idle calls
//! (`ui:idleInterface`), and its port changes going both ways: a control
//! input's value reaches the UI as a `port_event`, and what the UI writes
//! goes to the plug-in and, as a gesture (`ui:touch`) or alone, to the set as
//! a change of the device's parameter.
//!
//! The editor itself works anywhere a UI can be loaded; the window it is
//! embedded in is the X11 platforms' (a Wayland session serves it through
//! XWayland, as other plug-in editors are).

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::sync::{Arc, Mutex, PoisonError};

use libloading::Library;

use super::host::{Entry, Main, ParamChange};
use super::sys::{
    self, Feature, OptionsOption, UiDescriptor, UiIdle, UiPortMap, UiResize, UiTouch,
};
use super::{Lv2Error, PluginInfo, PortInfo, PortKind, StateHandle, UiInfo, failed};
use crate::window::{EditorWindow, GuiSize, WindowEvent};
use bl_model::{DeviceId, ParamId};

const UI: &str = "http://lv2plug.in/ns/extensions/ui#";
const OPTIONS: &str = "http://lv2plug.in/ns/ext/options#";
const ATOM: &str = "http://lv2plug.in/ns/ext/atom#";
const PARAMETERS: &str = "http://lv2plug.in/ns/ext/parameters#";
/// How often the UI is told to update, in hertz: a hint, as the shell calls
/// [`pump`] once a frame.
const UPDATE_RATE: f32 = 25.0;

/// What an editor needs of the host's world and of the plug-in.
pub(super) struct Support {
    pub(super) map: *mut sys::UridMap,
    pub(super) unmap: *mut sys::UridUnmap,
    /// The plug-in's own handle, for the UIs that ask for instance access.
    pub(super) instance: *mut c_void,
}

/// What a UI did since the host last looked.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    /// It wrote a control port's value.
    Write(u32, f32),
    /// It took hold of a control, or let go.
    Touch(u32, bool),
}

/// What the UI's callbacks reach: made with the UI, so its address is stable.
struct Controller {
    ports: Arc<[PortInfo]>,
    events: Mutex<Vec<Event>>,
    resize: Mutex<Option<(i32, i32)>>,
}

impl Controller {
    fn push(&self, event: Event) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

unsafe extern "C" fn write(
    controller: *mut c_void,
    port: u32,
    size: u32,
    protocol: u32,
    data: *const c_void,
) {
    // Only the float protocol, to a control port: the others are ignored, as
    // the UI specification asks of a host that doesn't know them.
    if protocol != 0 || size != 4 || data.is_null() {
        return;
    }
    // SAFETY: `controller` is the `Controller` the UI was made with, and
    // `data` holds a float.
    unsafe {
        (*controller.cast::<Controller>())
            .push(Event::Write(port, data.cast::<f32>().read_unaligned()));
    }
}

unsafe extern "C" fn touch(handle: *mut c_void, port: u32, grabbed: bool) {
    // SAFETY: `handle` is the `Controller` the feature was made with.
    unsafe { (*handle.cast::<Controller>()).push(Event::Touch(port, grabbed)) };
}

unsafe extern "C" fn resize(handle: *mut c_void, width: i32, height: i32) -> i32 {
    // SAFETY: as `touch`.
    let controller = unsafe { &*handle.cast::<Controller>() };
    *controller
        .resize
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some((width, height));
    0
}

unsafe extern "C" fn port_index(handle: *mut c_void, symbol: *const c_char) -> u32 {
    // SAFETY: as `touch`; `symbol` is a C string.
    let (controller, symbol) = unsafe { (&*handle.cast::<Controller>(), CStr::from_ptr(symbol)) };
    let symbol = symbol.to_string_lossy();
    controller
        .ports
        .iter()
        .find(|p| p.symbol == symbol)
        .map_or(u32::MAX, |p| p.index)
}

/// The features one UI is instantiated with, which outlive it.
struct UiFeatures {
    list: Vec<*const Feature>,
    _uris: Vec<CString>,
    _features: Vec<Feature>,
    _resize: Box<UiResize>,
    _touch: Box<UiTouch>,
    _port_map: Box<UiPortMap>,
    _options: Box<[OptionsOption]>,
    _floats: Box<[f32; 3]>,
}

impl UiFeatures {
    fn new(
        support: &Support,
        controller: *mut c_void,
        parent: usize,
        scale: f32,
        rate: f32,
    ) -> Self {
        // SAFETY: the host's map outlives the editor (it belongs to the world
        // the plug-in's handle keeps alive).
        let map = unsafe { &*support.map };
        let urid = |prefix: &str, name: &str| {
            let uri = CString::new(format!("{prefix}{name}")).expect("a URI without NUL");
            // SAFETY: the map is the host's own.
            unsafe { (map.map)(map.handle, uri.as_ptr()) }
        };
        let mut floats = Box::new([scale, UPDATE_RATE, rate]);
        let float = urid(ATOM, "Float");
        let option = |key: u32, value: *const f32| OptionsOption {
            context: 0,
            subject: 0,
            key,
            size: 4,
            type_: float,
            value: value.cast(),
        };
        let at = floats.as_mut_ptr();
        // SAFETY: `at` points at the three floats just made.
        let options: Box<[OptionsOption]> = unsafe {
            vec![
                option(urid(UI, "scaleFactor"), at),
                option(urid(UI, "updateRate"), at.add(1)),
                option(urid(PARAMETERS, "sampleRate"), at.add(2)),
                OptionsOption {
                    context: 0,
                    subject: 0,
                    key: 0,
                    size: 0,
                    type_: 0,
                    value: ptr::null(),
                },
            ]
        }
        .into_boxed_slice();
        let mut resize_feature = Box::new(UiResize {
            handle: controller,
            ui_resize: resize,
        });
        let mut touch_feature = Box::new(UiTouch {
            handle: controller,
            touch,
        });
        let mut port_map = Box::new(UiPortMap {
            handle: controller,
            port_index,
        });
        let uris: Vec<CString> = [
            "http://lv2plug.in/ns/ext/urid#map".to_owned(),
            "http://lv2plug.in/ns/ext/urid#unmap".to_owned(),
            format!("{UI}parent"),
            format!("{UI}resize"),
            format!("{UI}idleInterface"),
            format!("{UI}portMap"),
            format!("{UI}touch"),
            "http://lv2plug.in/ns/ext/instance-access".to_owned(),
            format!("{OPTIONS}options"),
        ]
        .into_iter()
        .map(|uri| CString::new(uri).expect("a URI without NUL"))
        .collect();
        let data: [*mut c_void; 9] = [
            support.map.cast(),
            support.unmap.cast(),
            parent as *mut c_void,
            ptr::from_mut(&mut *resize_feature).cast(),
            ptr::null_mut(),
            ptr::from_mut(&mut *port_map).cast(),
            ptr::from_mut(&mut *touch_feature).cast(),
            support.instance,
            options.as_ptr().cast_mut().cast(),
        ];
        let features: Vec<Feature> = uris
            .iter()
            .zip(data)
            .map(|(uri, data)| Feature {
                uri: uri.as_ptr(),
                data,
            })
            .collect();
        let mut list: Vec<*const Feature> = features.iter().map(ptr::from_ref).collect();
        list.push(ptr::null());
        Self {
            list,
            _uris: uris,
            _features: features,
            _resize: resize_feature,
            _touch: touch_feature,
            _port_map: port_map,
            _options: options,
            _floats: floats,
        }
    }
}

/// A plug-in's UI, instantiated: its code, its handle, and what it asked
/// the host for.
struct Ui {
    descriptor: *const UiDescriptor,
    handle: *mut c_void,
    /// The X window the UI made inside the parent, 0 where it made none.
    #[cfg(all(unix, not(target_os = "macos")))]
    widget: usize,
    idle: Option<UiIdle>,
    resizable: Option<UiResize>,
    controller: Box<Controller>,
    /// The values last given the UI, by port.
    sent: Vec<Option<f32>>,
    _features: UiFeatures,
    /// Dropped last: the UI's code unloads after it has been cleaned up.
    _library: Library,
}

impl Ui {
    /// Loads and instantiates the UI `info` names, embedded in `parent` (an
    /// X window, or 0 for none).
    fn open(
        info: &UiInfo,
        plugin_uri: &str,
        support: &Support,
        ports: Arc<[PortInfo]>,
        parent: usize,
        (scale, rate): (f32, f32),
    ) -> Result<Self, Lv2Error> {
        // SAFETY: loading a UI runs its code in this process, as loading a
        // plug-in does (the architecture, "Plug-in hosting").
        let library = unsafe { Library::new(&info.binary) }
            .map_err(|e| failed(format!("couldn't load its editor: {e}")))?;
        let descriptor = {
            // SAFETY: the UI exports it or it doesn't.
            let find = unsafe {
                library
                    .get::<unsafe extern "C" fn(u32) -> *const UiDescriptor>(b"lv2ui_descriptor\0")
            }
            .map_err(|_| failed("its editor has no lv2ui_descriptor"))?;
            (0..)
                .map(|index| {
                    // SAFETY: the UI answers for any index with a descriptor
                    // or null.
                    unsafe { find(index) }
                })
                .take_while(|d| !d.is_null())
                // SAFETY: a descriptor the UI gave stays valid while it is
                // loaded.
                .find(|d| unsafe { CStr::from_ptr((**d).uri) }.to_string_lossy() == info.uri)
                .ok_or_else(|| failed("its editor's binary doesn't hold the editor"))?
        };
        let controller = Box::new(Controller {
            ports: Arc::clone(&ports),
            events: Mutex::new(Vec::new()),
            resize: Mutex::new(None),
        });
        let features = UiFeatures::new(
            support,
            ptr::from_ref::<Controller>(&controller).cast_mut().cast(),
            parent,
            scale,
            rate,
        );
        let uri = CString::new(plugin_uri).map_err(|_| failed("the URI has a NUL"))?;
        let mut bundle = info.bundle.to_string_lossy().into_owned();
        bundle.push('/');
        let bundle = CString::new(bundle).map_err(|_| failed("the bundle's path has a NUL"))?;
        let mut widget: *mut c_void = ptr::null_mut();
        // SAFETY: the UI's own entry point, given the features it asked for;
        // the controller and features outlive it.
        let handle = unsafe {
            ((*descriptor).instantiate)(
                descriptor,
                uri.as_ptr(),
                bundle.as_ptr(),
                write,
                ptr::from_ref::<Controller>(&controller).cast_mut().cast(),
                &raw mut widget,
                features.list.as_ptr(),
            )
        };
        if handle.is_null() {
            return Err(failed("its editor won't open"));
        }
        let extension = |uri: &CStr| {
            // SAFETY: the UI answers `extension_data` for any URI.
            unsafe { (*descriptor).extension_data.map(|f| f(uri.as_ptr())) }
                .filter(|data| !data.is_null())
        };
        // SAFETY: the interfaces a UI gives are valid while it is.
        let (idle, resizable) = unsafe {
            (
                extension(c"http://lv2plug.in/ns/extensions/ui#idleInterface")
                    .map(|d| ptr::read(d.cast::<UiIdle>())),
                extension(c"http://lv2plug.in/ns/extensions/ui#resize")
                    .map(|d| ptr::read(d.cast::<UiResize>())),
            )
        };
        Ok(Self {
            descriptor,
            handle,
            #[cfg(all(unix, not(target_os = "macos")))]
            widget: widget as usize,
            idle,
            resizable,
            sent: vec![None; ports.len()],
            controller,
            _features: features,
            _library: library,
        })
    }

    /// Runs the UI's idle call. Returns whether the UI asks to be closed.
    fn idle(&self) -> bool {
        // SAFETY: the UI's own idle call, on the main thread.
        self.idle
            .as_ref()
            .is_some_and(|idle| unsafe { (idle.idle)(self.handle) } != 0)
    }

    /// Tells the UI its window is `size` now, if it takes that.
    fn resized(&self, size: GuiSize) {
        if let Some(resizable) = &self.resizable {
            // SAFETY: the UI's own interface, given its handle.
            unsafe {
                (resizable.ui_resize)(
                    self.handle,
                    i32::try_from(size.width).unwrap_or(i32::MAX),
                    i32::try_from(size.height).unwrap_or(i32::MAX),
                );
            }
        }
    }

    fn take_events(&self) -> Vec<Event> {
        std::mem::take(
            &mut *self
                .controller
                .events
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    fn take_resize(&self) -> Option<(i32, i32)> {
        self.controller
            .resize
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Gives the UI each control input's value that it doesn't have yet,
    /// except a port whose value the UI wrote and the plug-in hasn't taken
    /// yet: its value would be the old one.
    fn send_values(&mut self, handle: &StateHandle, awaiting: &BTreeMap<u32, (f32, f32)>) {
        // SAFETY: the descriptor is the UI's own, valid while it is loaded.
        let Some(port_event) = (unsafe { (*self.descriptor).port_event }) else {
            return;
        };
        for port in handle.ports() {
            if port.kind != PortKind::Control || !port.input || awaiting.contains_key(&port.index) {
                continue;
            }
            let value = handle.control(port.index);
            let slot = &mut self.sent[port.index as usize];
            if *slot != Some(value) {
                *slot = Some(value);
                // SAFETY: the UI's own call, with a float.
                unsafe {
                    port_event(self.handle, port.index, 4, 0, ptr::from_ref(&value).cast());
                }
            }
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        // SAFETY: the UI is cleaned up once, before its library goes.
        unsafe { ((*self.descriptor).cleanup)(self.handle) };
    }
}

/// One open editor.
struct Editor {
    ui: Ui,
    /// Keeps the plug-in alive as long as its editor is, and gives its
    /// values.
    handle: StateHandle,
    window: Option<EditorWindow>,
    hidden: bool,
    /// The controls a person is holding, with the last value written, if
    /// any: one change of the set when they let go, or the editor closes.
    held: BTreeMap<u32, Option<f32>>,
    /// The ports the UI wrote that the plug-in hasn't taken yet, with the
    /// value written and the one the port had.
    awaiting: BTreeMap<u32, (f32, f32)>,
}

/// The open editors, by device. Declared before the entries in `Main`, so
/// an editor is cleaned up while its plug-in is still there.
#[derive(Default)]
pub(super) struct Editors(BTreeMap<DeviceId, Editor>);

/// A host window for an editor, on the platforms that have X11 windows.
#[cfg(all(unix, not(target_os = "macos")))]
fn new_window(title: &str) -> Result<Option<EditorWindow>, Lv2Error> {
    EditorWindow::open(
        title,
        GuiSize {
            width: 240,
            height: 120,
        },
        true,
    )
    .map(Some)
    .map_err(|why| failed(format!("couldn't open a window for its editor: {why}")))
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn new_window(_: &str) -> Result<Option<EditorWindow>, Lv2Error> {
    Err(failed(
        "LV2 editors are shown in X11 windows, which this platform doesn't host",
    ))
}

/// Opens the device's editor in a window of Beatlayer's own, titled `title`.
pub(super) fn open(main: &mut Main, device: DeviceId, title: &str) -> Result<(), Lv2Error> {
    if main.editors.0.contains_key(&device) {
        return Ok(());
    }
    open_with(main, device, new_window(title)?)
}

/// Opens the device's editor in `window` (or in none, where a test has no
/// display).
pub(super) fn open_with(
    main: &mut Main,
    device: DeviceId,
    window: Option<EditorWindow>,
) -> Result<(), Lv2Error> {
    if main.editors.0.contains_key(&device) {
        return Ok(());
    }
    let entry = main
        .entries
        .get(&device)
        .ok_or(Lv2Error::NoInstance(device))?;
    let info: &PluginInfo = &entry.info;
    let ui_info = info
        .ui
        .as_ref()
        .ok_or_else(|| failed("it has no X11 editor"))?;
    let handle = entry
        .handle
        .clone()
        .ok_or_else(|| failed("its plug-in isn't running yet"))?;
    let rate = entry.config.map_or(48_000.0, |c| c.sample_rate.hz() as f32);
    #[cfg(all(unix, not(target_os = "macos")))]
    let (parent, scale) = window
        .as_ref()
        .map_or((0, 1.0), |w| (w.x11_id() as usize, w.scale() as f32));
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let (parent, scale) = (0, 1.0);
    let ui = Ui::open(
        ui_info,
        &entry.plugin_id,
        &handle.ui_support(),
        handle.ports_shared(),
        parent,
        (scale, rate),
    )?;
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(window) = &window {
        // The window takes the size the UI made its view.
        if let Some(size) = u32::try_from(ui.widget)
            .ok()
            .and_then(|widget| window.child_size(widget))
        {
            window.resize(size);
        }
        window.set_visible(true);
    }
    let mut editor = Editor {
        ui,
        handle,
        window,
        hidden: false,
        held: BTreeMap::new(),
        awaiting: BTreeMap::new(),
    };
    editor.ui.send_values(&editor.handle, &editor.awaiting);
    main.editors.0.insert(device, editor);
    Ok(())
}

/// Shows or hides the device's editor window, without closing it.
pub(super) fn set_visible(main: &mut Main, device: DeviceId, shown: bool) {
    if let Some(editor) = main.editors.0.get_mut(&device) {
        editor.hidden = !shown;
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(window) = &editor.window {
            window.set_visible(shown);
        }
    }
}

/// Closes the device's editor, if it's open.
pub(super) fn close(main: &mut Main, device: DeviceId) {
    if let Some(mut editor) = main.editors.0.remove(&device)
        && let Some(entry) = main.entries.get(&device)
    {
        editor.finish(device, entry, &mut main.changes);
    }
}

/// Makes the device's editor window `size`, as dragging its edge does, and
/// tells the UI.
pub(super) fn resize_window(
    main: &mut Main,
    device: DeviceId,
    size: GuiSize,
) -> Result<(), Lv2Error> {
    let editor = main
        .editors
        .0
        .get_mut(&device)
        .ok_or_else(|| failed("its editor isn't open"))?;
    let window = editor
        .window
        .as_ref()
        .ok_or_else(|| failed("its editor isn't in a host window"))?;
    window.resize(size);
    editor.ui.resized(size);
    Ok(())
}

/// The size of the device's editor window, while it is open in one.
pub(super) fn size(main: &Main, device: DeviceId) -> Option<GuiSize> {
    main.editors
        .0
        .get(&device)?
        .window
        .as_ref()
        .map(EditorWindow::size)
}

/// The devices whose editor is open.
pub(super) fn open_devices(main: &mut Main) -> Vec<DeviceId> {
    main.editors.0.keys().copied().collect()
}

/// Whether the device's editor is open, and hidden.
pub(super) fn state(main: &Main, device: DeviceId) -> (bool, bool) {
    main.editors
        .0
        .get(&device)
        .map_or((false, false), |editor| (true, editor.hidden))
}

/// The editors' main-thread work: the window's events, the UIs' idle calls,
/// what the UIs wrote, and the plug-ins' values for the UIs. Returns whether
/// anything an editor shows or the set holds may have changed.
pub(super) fn pump(main: &mut Main) -> bool {
    let Main {
        editors,
        entries,
        changes,
        ..
    } = &mut *main;
    let mut changed = false;
    let mut closed = Vec::new();
    // Editors whose plug-in was replaced by a new instance (a state loaded,
    // a new node): they open again on the new one, in the same window.
    let mut replaced = Vec::new();
    for (&device, editor) in &mut editors.0 {
        let Some(entry) = entries.get_mut(&device) else {
            closed.push(device);
            continue;
        };
        if entry
            .handle
            .as_ref()
            .is_some_and(|handle| !Arc::ptr_eq(&handle.core, &editor.handle.core))
        {
            replaced.push(device);
            continue;
        }
        let (keep, active) = editor.pump(device, entry, changes);
        changed |= active;
        if !keep {
            closed.push(device);
        }
    }
    for device in closed {
        close(main, device);
        changed = true;
    }
    for device in replaced {
        if let Some(mut editor) = main.editors.0.remove(&device) {
            if let Some(entry) = main.entries.get(&device) {
                editor.finish(device, entry, &mut main.changes);
            }
            let Editor {
                ui, window, hidden, ..
            } = editor;
            drop(ui);
            if open_with(main, device, window).is_ok() && hidden {
                set_visible(main, device, false);
            }
        }
        changed = true;
    }
    changed
}

impl Editor {
    /// One look at the editor. Returns whether to keep it open, and whether
    /// anything happened.
    fn pump(
        &mut self,
        device: DeviceId,
        entry: &mut Entry,
        changes: &mut Vec<ParamChange>,
    ) -> (bool, bool) {
        let mut active = false;
        if let Some(window) = &self.window {
            while let Some(event) = window.poll() {
                match event {
                    WindowEvent::Closed => return (false, true),
                    WindowEvent::Resized(size) => self.ui.resized(size),
                    WindowEvent::ScaleChanged(_) => {}
                }
            }
        }
        if self.ui.idle() {
            return (false, true);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if let (Some((width, height)), Some(window)) = (self.ui.take_resize(), &self.window) {
            window.resize(GuiSize {
                width: u32::try_from(width).unwrap_or(1).max(1),
                height: u32::try_from(height).unwrap_or(1).max(1),
            });
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = self.ui.take_resize();
        for event in self.ui.take_events() {
            active = true;
            match event {
                Event::Write(port, value) => {
                    // The plug-in hears it at the next block's first frame,
                    // and the UI isn't told its own value back.
                    if let Some(ends) = entry.ends.as_mut() {
                        let _ = ends.from_editor.push((port, value));
                    }
                    if let Some(slot) = self.ui.sent.get_mut(port as usize) {
                        *slot = Some(value);
                    }
                    let old = self.handle.control(port);
                    if old == value {
                        self.awaiting.remove(&port);
                    } else {
                        self.awaiting
                            .entry(port)
                            .and_modify(|(written, _)| *written = value)
                            .or_insert((value, old));
                    }
                    if let Some(held) = self.held.get_mut(&port) {
                        *held = Some(value);
                    } else {
                        changes.extend(param_change(entry, device, port, value));
                    }
                }
                Event::Touch(port, true) => {
                    self.held.entry(port).or_insert(None);
                }
                Event::Touch(port, false) => {
                    if let Some(Some(value)) = self.held.remove(&port) {
                        changes.extend(param_change(entry, device, port, value));
                    }
                }
            }
        }
        // A port is awaited until its value moves off the one it had.
        self.awaiting
            .retain(|&port, &mut (_, old)| self.handle.control(port) == old);
        self.ui.send_values(&self.handle, &self.awaiting);
        (true, active)
    }
}

impl Editor {
    /// Ends the gestures still held, as the editor closes: each control
    /// written during one is a change of the set.
    fn finish(&mut self, device: DeviceId, entry: &Entry, changes: &mut Vec<ParamChange>) {
        for (port, value) in std::mem::take(&mut self.held) {
            if let Some(value) = value {
                changes.extend(param_change(entry, device, port, value));
            }
        }
    }
}

/// The change of the device's parameter that is port `port` taking `value`,
/// if the port is one.
fn param_change(entry: &Entry, device: DeviceId, port: u32, value: f32) -> Option<ParamChange> {
    let param = entry.params.iter().find(|p| p.map.port == port)?;
    Some(ParamChange {
        device,
        param: ParamId(port),
        value: param.map.plain_value(value),
    })
}

#[cfg(test)]
mod tests {
    //! The editor with no window (there is no display to count on): what
    //! passes between the host and the test plug-in's scripted UI. They run
    //! where lilv is: on Linux, and elsewhere when `BL_TEST_LILV` names a
    //! lilv library. The window itself is `tests/editor.rs`'s.

    use bl_core::id::Id;
    use bl_core::time::SampleRate;
    use bl_engine::{
        Block, Node, NodeConfig, NodeEvent, NodeFactory, NoteBuffer, ParamValue, TransportInfo,
    };
    use bl_model::{Device, DeviceId, DeviceKind, ParamId, PluginFormat};
    use bl_test_lv2::{GAIN, MODE, PLUGIN_NAME, PLUGIN_URI, STEPS, TOGGLE};

    use super::super::host::ParamChange;
    use super::super::{Lv2Error, Lv2Host};

    const FRAMES: usize = 64;

    /// A host with the test plug-in loaded, and a running node for a device
    /// of it, or `None` where there is no lilv.
    fn running() -> Option<(Lv2Host, DeviceId, Box<dyn Node>)> {
        let host = Lv2Host::new();
        if let Some(library) = std::env::var_os("BL_TEST_LILV") {
            host.use_library(library);
        }
        match host.load(bl_test_lv2::bundle()) {
            Ok(_) => {}
            Err(Lv2Error::Unavailable) if !cfg!(target_os = "linux") => return None,
            Err(e) => panic!("the test LV2 plug-in should load: {e}"),
        }
        let device = Device::new(
            DeviceId(Id::new(1).unwrap()),
            PLUGIN_NAME,
            DeviceKind::Plugin {
                format: PluginFormat::Lv2,
                plugin_id: PLUGIN_URI.to_owned(),
            },
        );
        let config = NodeConfig {
            round_trip_latency: 0,
            sample_rate: SampleRate::HZ_48000,
            max_frames: FRAMES,
            channels: 2,
        };
        let node = host.factory().create(&device, &config).unwrap();
        Some((host, device.id, node))
    }

    /// Runs a block of 1.0 through `node` with the parameters set at its
    /// first frame, and returns its first sample.
    fn run(node: &mut dyn Node, set: &[(u32, f64)]) -> f32 {
        let events: Vec<NodeEvent> = set
            .iter()
            .map(|&(param, value)| NodeEvent {
                offset: 0,
                param: ParamId(param),
                value: ParamValue::Set(value),
            })
            .collect();
        let mut data = vec![1.0f32; FRAMES * 2];
        let mut out = NoteBuffer::with_capacity(8);
        {
            let mut block = Block::new(
                &mut data,
                2,
                FRAMES,
                FRAMES,
                &events,
                TransportInfo::default(),
                SampleRate::HZ_48000,
            )
            .with_notes(&[], &mut out);
            node.process(&mut block);
        }
        data[0]
    }

    // Spec: lv2-hosting R13. Values reach the editor and what it writes goes
    // to the plug-in and the set; a gesture is one change.
    #[test]
    fn values_and_writes_pass_between_the_host_and_the_editor() {
        let Some((host, device, mut node)) = running() else {
            return;
        };
        run(node.as_mut(), &[]);
        host.open_editor_headless(device).expect("it opens");
        assert_eq!(host.open_editors(), [device]);
        assert!(host.status(device).unwrap().editor_open);
        assert!(host.status(device).unwrap().has_editor || !cfg!(target_os = "linux"));

        // A new gain reaches the editor, which answers it with the steps.
        run(node.as_mut(), &[(GAIN, 0.5)]);
        host.idle();
        host.idle();
        assert_eq!(
            host.take_param_changes(),
            [ParamChange {
                device,
                param: ParamId(STEPS),
                value: 2.0
            }]
        );
        // What it wrote is the plug-in's at the next block.
        run(node.as_mut(), &[]);
        let status = host.status(device).unwrap();
        assert!(status.values.contains(&(ParamId(STEPS), 2.0)), "{status:?}");

        // A gesture (hold, write, write, let go) is one change, with its last
        // value, and the plug-in has it.
        run(node.as_mut(), &[(TOGGLE, 1.0)]);
        host.idle();
        host.idle();
        let change = |param, value| ParamChange {
            device,
            param: ParamId(param),
            value,
        };
        // The editor answers its own gain, as it does any.
        assert_eq!(
            host.take_param_changes(),
            [change(GAIN, 1.75), change(STEPS, 7.0)]
        );
        // The plug-in hasn't taken the gain yet, and the editor isn't told
        // the old one meanwhile, which it would answer.
        host.idle();
        host.idle();
        assert!(host.take_param_changes().is_empty());
        assert_eq!(run(node.as_mut(), &[]), 1.75, "the gain the editor wrote");
        host.idle();
        host.idle();
        assert!(host.take_param_changes().is_empty());

        // The editor asks to be closed.
        run(node.as_mut(), &[(MODE, 2.0)]);
        host.idle();
        host.idle();
        assert!(host.open_editors().is_empty());
        assert!(!host.status(device).unwrap().editor_open);
    }

    // Spec: lv2-hosting R13. A state loaded makes a new instance, and the
    // open editor follows it: values still reach the editor.
    #[test]
    fn an_editor_follows_its_plugin_into_a_new_instance() {
        let Some((host, device, mut node)) = running() else {
            return;
        };
        run(node.as_mut(), &[]);
        host.open_editor_headless(device).expect("it opens");
        let state = host.save_state(device).expect("it saves");
        host.load_state(device, &state.data).expect("it loads");
        run(node.as_mut(), &[]);
        host.idle();
        host.idle();
        host.take_param_changes();
        assert_eq!(host.open_editors(), [device]);

        run(node.as_mut(), &[(GAIN, 0.5)]);
        host.idle();
        host.idle();
        assert_eq!(
            host.take_param_changes(),
            [ParamChange {
                device,
                param: ParamId(STEPS),
                value: 2.0
            }]
        );
    }

    // Spec: lv2-hosting R13. An editor closes with its device's plug-in, and
    // is hidden and shown without closing.
    #[test]
    fn an_editor_hides_and_closes_with_its_plugin() {
        let Some((host, device, mut node)) = running() else {
            return;
        };
        run(node.as_mut(), &[]);
        host.open_editor_headless(device).expect("it opens");
        host.set_editor_visible(device, false).unwrap();
        let status = host.status(device).unwrap();
        assert!(status.editor_open && status.editor_hidden);
        host.set_editor_visible(device, true).unwrap();
        assert!(!host.status(device).unwrap().editor_hidden);
        host.clear();
        assert!(host.open_editors().is_empty());
        drop(node);
    }

    // Spec: lv2-hosting R13. A plug-in that isn't running has no editor to
    // open.
    #[test]
    fn a_plugin_that_is_not_running_has_no_editor_to_open() {
        let Some((host, device, _node)) = running() else {
            return;
        };
        host.clear();
        let refused = host.open_editor_headless(device);
        assert!(
            matches!(refused, Err(Lv2Error::NoInstance(_))),
            "{refused:?}"
        );
    }
}
