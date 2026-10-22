//! The window host: Beatlayer's own winit event loop, driving egui through
//! egui-winit and egui-wgpu (ADR-001), with the main window and the
//! Settings window, native menus, and file dialogs.
//!
//! Every key press, menu choice, and click becomes a command invocation on
//! the shell ([`Shell::invoke`]); what only the platform can do comes back
//! as effects, which the host carries out here.
//!
//! Each floating device (WP-2.5b, IMP-14) has a window of its own, which
//! the host opens, closes, shows, and hides to match the shell's
//! `device.windows` after every command: it stays above the main window,
//! as Live's plug-in windows do; moving or resizing it records its frame
//! (`device.set_window`, no undo step); and closing it docks the device.
//!
//! While the person has the agent server switched on (Settings, Tempo &
//! MIDI), the host runs it (`bl-agent`, WP-1.15): its threads hand each
//! request to this event loop, which runs it on the same shell, and asks
//! the person when an agent connects without its own token or wants to run
//! a command that asks first. A call that comes while the person drags
//! waits, and runs after the drop as its own undo step (WP-1.15b). The
//! engine the shell plays the set through is the agent API's too
//! ([`Agent::sync_engine`]): agents' transport commands and scheduled
//! commands go on the queue the person's transport commands take, and
//! fired scheduled commands come back as the agents' undo steps. An
//! agent's view capture (WP-1.16) is read back from the window's next frame
//! and encoded off this thread ([`crate::capture`]).

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};
use std::time::{Duration, Instant};

use bl_agent::http::{HttpConfig, HttpServer, Parked, Served};
use bl_agent::{Agent, Captured, RenderOutcome, RenderRequest};
use bl_shell::agent::AgentServerStatus;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::menus::{Menu, MenuPlatform, menu_bar};
use bl_shell::perception_commands::View;
use bl_shell::settings::{Preferences, SettingsPage, ThemeLook, default_config_dir};
use bl_shell::{Effect, Invocation, Shell, WindowKind, commands as ids};
use bl_ui::assistant_panel::AssistantPanelUi;
use bl_ui::device_view::DeviceViewUi;
use bl_ui::input::KeyDispatcher;
use bl_ui::keymap::{Keymap, Platform};
use bl_ui::layout::{TITLE_BAR, apply_zoom, fit_main_window, follow_zoom, settings_window};
use bl_ui::main_window::{DrawnMenuBar, MainWindowUi, UiEvent};
use bl_ui::settings_window::SettingsUi;
use bl_ui::theme::{Theme, fonts};
use bl_ui::theme_library::ThemeLibrary;
use egui::{ViewportId, ViewportInfo};
use egui_wgpu::winit::Painter;
use egui_wgpu::{
    RendererOptions, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew, WgpuSetupExisting,
};
use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

use crate::Args;

/// How often the CPU meter reads the engine's load.
const CPU_METER_PERIOD: Duration = Duration::from_millis(500);

/// How often the loop hands the engine's reports to the agent API while
/// agents' scheduled commands wait to fire, so the set follows what played
/// within a few milliseconds (WP-1.15b).
const AGENT_ENGINE_PERIOD: Duration = Duration::from_millis(10);

/// How long a view capture waits for its window to draw a frame before it
/// fails (WP-1.16): a minimised or hidden window draws none.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the loop asks a window to draw while a capture waits for a
/// frame.
const CAPTURE_PERIOD: Duration = Duration::from_millis(16);

/// How long after a command the loop keeps looking at the engine's
/// transport between frames, so a start or stop shows as soon as the engine
/// has made it (WP-1.18), and how often it looks meanwhile.
const TRANSPORT_WATCH: Duration = Duration::from_millis(400);
const TRANSPORT_WATCH_PERIOD: Duration = Duration::from_millis(4);

/// Whether Beatlayer's windows draw Live's compact title bar themselves
/// (D22): on macOS, where layout.md measured Live's. Windows and Linux keep
/// the system's title bar, since nothing has been observed there yet.
const DRAWN_TITLE_BAR: bool = cfg!(target_os = "macos");

/// The content height the system's title bar leaves, for a window `height`
/// points tall with Live's title bar: the whole height where Beatlayer
/// draws the title bar, and less [`TITLE_BAR`] where the system does.
fn content_height(height: f32) -> f32 {
    if DRAWN_TITLE_BAR {
        height
    } else {
        height - TITLE_BAR
    }
}

/// Whether a window draws the title bar now: not in full screen, where
/// Live shows none (L-WIN-002).
fn draws_title_bar(window: &Window) -> bool {
    DRAWN_TITLE_BAR && window.fullscreen().is_none()
}

/// The largest inner size, in points, a window opens at on a screen of
/// `screen` points: room is left for the menu bar, the Dock, or the taskbar.
fn screen_limit(screen: LogicalSize<f32>) -> egui::Vec2 {
    egui::vec2(screen.width * 0.95, screen.height * 0.9)
}

/// The screen limit of the screen a window is on.
fn window_screen_limit(window: &Window) -> Option<egui::Vec2> {
    let monitor = window
        .current_monitor()
        .or_else(|| window.primary_monitor())?;
    Some(screen_limit(
        monitor.size().to_logical::<f32>(monitor.scale_factor()),
    ))
}

/// A size brought within a screen limit, if there is one.
fn within(size: egui::Vec2, limit: Option<egui::Vec2>) -> egui::Vec2 {
    limit.map_or(size, |limit| size.min(limit))
}

/// Gives the main window its smallest size at `zoom`, and grows it to that
/// if it's smaller, as far as its screen allows (interface-zoom R7). A
/// screen too small for the zoom's minimum gets a window as big as it
/// allows, showing the layout's minimum from its top-left corner.
fn resize_main_window(window: &Window, zoom: f32, drawn_menus: bool) {
    let inner = window.inner_size().to_logical::<f32>(window.scale_factor());
    let inner = egui::vec2(inner.width, inner.height);
    let (minimum, wanted) = fit_main_window(
        inner,
        zoom,
        DRAWN_TITLE_BAR,
        drawn_menus,
        window_screen_limit(window),
    );
    window.set_min_inner_size(Some(LogicalSize::new(minimum.x, minimum.y)));
    if window.fullscreen().is_some() {
        return;
    }
    if (wanted - inner).length() > 0.5 {
        let _ = window.request_inner_size(LogicalSize::new(wanted.x, wanted.y));
    }
}

/// Sizes the Settings window for `zoom`: Live's 562 × 617 points at 100%,
/// its content scaled, within its screen (interface-zoom R8).
fn fit_settings_window(window: &Window, zoom: f32) {
    let size = within(
        settings_window(zoom, DRAWN_TITLE_BAR),
        window_screen_limit(window),
    );
    let _ = window.request_inner_size(LogicalSize::new(size.x, size.y));
}

/// Window attributes with the system's title bar made way for Beatlayer's
/// (macOS: a full-size content view under a transparent, untitled title
/// bar, whose buttons and dragging stay the system's).
fn with_title_bar(attributes: WindowAttributes) -> WindowAttributes {
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowAttributesExtMacOS as _;
        attributes
            .with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_fullsize_content_view(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        attributes
    }
}

/// Fits the system's title bar to Live's after the window opens or
/// resizes (macOS; nothing elsewhere).
fn fit_title_bar(window: &Window) {
    #[cfg(target_os = "macos")]
    {
        crate::title_bar::fit(window);
        if std::env::var_os("BEATLAYER_TRACE").is_some() {
            eprintln!("title bar: {}", crate::title_bar::describe(window));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// The height, in points, of the title bar a floating device's window
/// draws itself: a utility window's short title bar on macOS (WP-2.5b), and
/// none elsewhere, where the system draws a tool window's.
#[cfg(target_os = "macos")]
const DEVICE_TITLE_STRIP: f32 = crate::title_bar::UTILITY_TITLE_BAR;
#[cfg(not(target_os = "macos"))]
const DEVICE_TITLE_STRIP: f32 = 0.0;

/// Fits a floating device's window's title bar after it opens or resizes
/// (macOS; nothing elsewhere).
fn fit_device_title_bar(window: &Window) {
    #[cfg(target_os = "macos")]
    crate::title_bar::fit_utility(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Window attributes for a floating device's window: a utility (tool)
/// window, as Live's plug-in windows are (observed in Live 12.4.6). On
/// macOS its content runs under a transparent, untitled title bar that
/// [`fit_device_title_bar`] makes short; on Windows it's owned by the main
/// window `owner`, so it stays above it and out of the taskbar (the tool
/// window style is set once it exists: [`make_tool_window`]); on X11 it's
/// a utility window, which window managers keep above its app's windows.
fn device_window_attributes(
    attributes: WindowAttributes,
    owner: Option<&Window>,
) -> WindowAttributes {
    #[cfg(target_os = "macos")]
    {
        let _ = owner;
        with_title_bar(attributes)
    }
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
        use winit::platform::windows::WindowAttributesExtWindows as _;
        let hwnd = owner.and_then(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
            _ => None,
        });
        let attributes = attributes.with_skip_taskbar(true);
        match hwnd {
            Some(hwnd) => attributes.with_owner_window(hwnd),
            None => attributes,
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        use winit::platform::x11::{WindowAttributesExtX11 as _, WindowType};
        let _ = owner;
        attributes.with_x11_window_type(vec![WindowType::Utility])
    }
}

/// Gives a floating device's window Windows' tool window style: a short
/// title bar with only a close button (Windows only; nothing elsewhere).
fn make_tool_window(window: &Window) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_EX_TOOLWINDOW,
        };
        let Some(hwnd) = window.window_handle().ok().and_then(|h| match h.as_raw() {
            RawWindowHandle::Win32(handle) => {
                Some(handle.hwnd.get() as windows_sys::Win32::Foundation::HWND)
            }
            _ => None,
        }) else {
            return;
        };
        // SAFETY: the handle is the live window's, used on its own thread.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_TOOLWINDOW as isize);
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// Events sent into the loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// From AccessKit: a screen reader asked for the tree or an action.
    AccessKit(egui_winit::accesskit_winit::Event),
    /// A native menu item was chosen.
    #[cfg(any(target_os = "macos", windows))]
    Menu(muda::MenuEvent),
    /// The agent server has a request for the main thread.
    Agent,
    /// Waveform peaks became ready or were invalidated, so the Arrangement
    /// View redraws (WP-1.8a).
    Peaks,
    /// The browser's background work has news: its index grew, or a
    /// preview was decoded (WP-1.10).
    Browser,
    /// A view capture has been encoded (WP-1.16).
    Captured,
    /// An agent audio preview or analysis finished rendering (WP-2.28).
    Rendered,
}

impl From<egui_winit::accesskit_winit::Event> for UserEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        Self::AccessKit(event)
    }
}

/// A window drawn by egui: its own context, input state, and painter.
struct Surface {
    window: Arc<Window>,
    ctx: egui::Context,
    state: egui_winit::State,
    painter: Painter,
    info: ViewportInfo,
    repaint_at: Option<Instant>,
    /// The interface zoom the window draws at: the shell's, except while
    /// the Settings window waits for a mouse button to be released.
    zoom: f32,
}

struct Host {
    args: Args,
    shell: Shell,
    /// The themes found, and the theme drawn with the settings (and the
    /// appearance, `true` for dark) it was made from (themes R8).
    library: ThemeLibrary,
    theme: Theme,
    drawn: (ThemeLook, bool),
    /// Whether the Theme & Colors page was showing, so opening it reads the
    /// theme files again (themes R9).
    on_theme_page: bool,
    keymap: Keymap,
    /// The shell's keymap revision the keymap was built at, so a change
    /// rebuilds it and the native menu (the custom-shortcuts spec, R10).
    keymap_revision: u64,
    platform: Platform,
    menus: Vec<Menu>,
    proxy: EventLoopProxy<UserEvent>,
    main: Option<Surface>,
    settings: Option<Surface>,
    /// The assistant's panel in a window of its own, while it floats open
    /// (WP-2.23, part 3; D24), with what it draws.
    assistant: Option<Surface>,
    assistant_ui: AssistantPanelUi,
    /// Floating devices' windows (WP-2.5b), each with what it draws.
    devices: BTreeMap<bl_model::DeviceId, (Option<Surface>, DeviceViewUi)>,
    /// Always `None`: what [`Host::surface`] gives for a device window
    /// that isn't open.
    no_surface: Option<Surface>,
    device_keys: KeyDispatcher,
    assistant_keys: KeyDispatcher,
    main_ui: MainWindowUi,
    settings_ui: SettingsUi,
    main_keys: KeyDispatcher,
    settings_keys: KeyDispatcher,
    #[cfg(any(target_os = "macos", windows))]
    native_menu: Option<crate::native_menu::NativeMenu>,
    shown_revision: Option<u64>,
    next_cpu_sample: Instant,
    /// Until when the loop looks at the transport between frames.
    watch_transport_until: Option<Instant>,
    quitting: bool,
    agent: Agent,
    /// The agent server while it runs, with the settings it started with.
    agent_server: Option<(HttpServer, HttpConfig)>,
    /// The settings the agent server last failed to start with, so it isn't
    /// retried until they change.
    agent_failed: Option<HttpConfig>,
    /// Agents' calls waiting for the person's gesture to end.
    agent_parked: Vec<Parked>,
    /// Agents' view captures waiting for a frame (WP-1.16).
    captures: Vec<PendingCapture>,
    /// Captures encoded off the main thread, coming back.
    captured: (mpsc::Sender<Encoded>, mpsc::Receiver<Encoded>),
    /// Audio renders off the main thread, coming back (WP-2.28).
    rendered: (mpsc::Sender<RenderedEvent>, mpsc::Receiver<RenderedEvent>),
    /// Audio render jobs waiting for the background preview worker (WP-2.28).
    render_jobs: mpsc::Sender<RenderJob>,
    /// Cancellation flags for queued and active preview renders (WP-2.28).
    render_cancels: Vec<Arc<AtomicBool>>,
}

/// An audio render job for the background preview worker thread (WP-2.28).
struct RenderJob {
    ticket: u64,
    request: RenderRequest,
    cancel: Arc<AtomicBool>,
}

/// A view capture encoded off the main thread: its ticket, and the image
/// or why there's none (WP-1.16).
type Encoded = (u64, Result<Captured, String>);

/// An agent audio render finished off the main thread: its ticket, and the outcome
/// or why there's none (WP-2.28).
type RenderedEvent = (u64, Result<RenderOutcome, String>);

/// An agent's view capture waiting for its window's frame (WP-1.16).
struct PendingCapture {
    ticket: u64,
    view: View,
    max_size: u32,
    window: WindowKind,
    /// When it fails if no frame has come.
    deadline: Instant,
    /// When the loop last asked its window to draw.
    redrawn: Instant,
}

/// Runs the app until it quits.
pub fn run(args: Args) -> Result<(), String> {
    let config_dir = args.config_dir.clone().or_else(default_config_dir);
    let (prefs, prefs_error) = match config_dir.as_deref().map(Preferences::load) {
        Some(Ok(prefs)) => (prefs, None),
        Some(Err(error)) => (Preferences::default(), Some(error)),
        None => (Preferences::default(), None),
    };
    let audio = if args.null_audio {
        AudioHost::null(prefs.audio.clone())
    } else {
        AudioHost::new(prefs.audio.clone())
    };
    let mut main_ui = MainWindowUi::new();
    let mut shell = if args.benchmark_scene {
        // The Arrangement benchmark scene (WP-1.8a), with its peaks in
        // memory.
        let scene = bl_ui::arrangement::benchmark::scene(200, 25, 64);
        main_ui
            .arrangement_mut()
            .set_peak_source(Box::new(scene.peaks), None);
        bl_ui::arrangement::benchmark::shell_with(scene.editor)
    } else {
        let keep_tokens = config_dir.is_some();
        let mut shell = Shell::new(prefs, config_dir, audio, Box::new(ProjectFiles::new()));
        // The agent server's tokens live in the OS keychain, beside the
        // preferences they go with; without a config folder nothing is kept.
        if keep_tokens {
            shell.use_agent_keychain(bl_shell::agent::os_keychain());
        }
        shell
    };
    // A headless run (`--null-audio`) opens no MIDI device either.
    if !args.null_audio {
        shell.use_midi_backend(Box::new(bl_midi_io::MidirBackend::new()));
    }
    // The files' analyses are kept beside the peaks (WP-4.3).
    shell.use_platform_analysis_cache();
    shell.start_audio();
    if let Some(error) = prefs_error {
        shell.set_message(error, true);
    }
    if !args.benchmark_scene {
        open_user_library(&mut shell, &args);
    }
    if let Some(set) = &args.set {
        let _ = shell.invoke(ids::SET_OPEN, json!({ "path": set.to_string_lossy() }));
    }

    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        // Beatlayer installs its own menu bar.
        use winit::platform::macos::EventLoopBuilderExtMacOS as _;
        builder.with_default_menu(false);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("couldn't start the event loop: {e}"))?;
    let proxy = event_loop.create_proxy();
    if !args.benchmark_scene {
        start_peaks(&shell, &mut main_ui, &proxy);
    }
    {
        // The browser's index and preview decoder wake the loop.
        let proxy = std::sync::Mutex::new(proxy.clone());
        shell.set_waker(Arc::new(move || {
            if let Ok(proxy) = proxy.lock() {
                let _ = proxy.send_event(UserEvent::Browser);
            }
        }));
    }
    if !args.benchmark_scene {
        // Live scans its plug-ins at launch: so does Beatlayer, in the
        // background, so the first frame doesn't wait (WP-2.9b).
        shell.start_plugin_scan();
    }
    for (id, command_args) in &args.commands {
        if let Err(error) = shell.invoke(id, command_args.clone()) {
            eprintln!("beatlayer: --command {id}: {error}");
        }
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        let menu_proxy = proxy.clone();
        muda::MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_proxy.send_event(UserEvent::Menu(event));
        }));
    }

    let mut library = ThemeLibrary::bundled();
    library.refresh(&mut shell);
    let drawn = (shell.look().clone(), shell.theme_dark());
    let platform = Platform::current();
    let menus = menu_bar(MenuPlatform::current());
    let (job_sender, job_receiver) = mpsc::channel::<RenderJob>();
    let (rendered_sender, rendered_receiver) = mpsc::channel();
    let render_proxy = proxy.clone();
    let worker_sender = rendered_sender.clone();
    let _ = std::thread::Builder::new()
        .name("bl-preview-worker".to_owned())
        .spawn(move || {
            while let Ok(job) = job_receiver.recv() {
                if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ =
                        worker_sender.send((job.ticket, Err("render was cancelled".to_owned())));
                    let _ = render_proxy.send_event(UserEvent::Rendered);
                    continue;
                }
                let result = job.request.execute_with_cancel(&job.cancel);
                let _ = worker_sender.send((job.ticket, result));
                let _ = render_proxy.send_event(UserEvent::Rendered);
            }
        });
    // The keymap in use: Live 12's shortcuts with the person's own layered
    // over them (the custom-shortcuts spec, R3).
    let keymap = Keymap::from_data(shell.keymap().clone());
    let keymap_revision = shell.keymap_revision();
    let mut host = Host {
        args,
        shell,
        theme: library.resolve(&drawn.0, drawn.1),
        library,
        drawn,
        on_theme_page: false,
        keymap,
        keymap_revision,
        platform,
        menus,
        proxy,
        main: None,
        settings: None,
        assistant: None,
        assistant_ui: AssistantPanelUi::default(),
        devices: BTreeMap::new(),
        no_surface: None,
        device_keys: KeyDispatcher::new(platform),
        assistant_keys: KeyDispatcher::new(platform),
        main_ui,
        settings_ui: SettingsUi::new(),
        main_keys: KeyDispatcher::new(platform),
        settings_keys: KeyDispatcher::new(platform),
        #[cfg(any(target_os = "macos", windows))]
        native_menu: None,
        shown_revision: None,
        next_cpu_sample: Instant::now() + CPU_METER_PERIOD,
        watch_transport_until: None,
        quitting: false,
        agent: Agent::for_app(),
        agent_server: None,
        agent_failed: None,
        agent_parked: Vec::new(),
        captures: Vec::new(),
        captured: mpsc::channel(),
        rendered: (rendered_sender, rendered_receiver),
        render_jobs: job_sender,
        render_cancels: Vec::new(),
    };
    host.sync_agent_server();
    event_loop
        .run_app(&mut host)
        .map_err(|e| format!("the event loop failed: {e}"))?;
    host.cancel_renders();
    host.agent.set_view_capture(false);
    host.shell.audio_mut().stop();
    if let Some((server, _)) = host.agent_server.take() {
        server.stop();
    }
    Ok(())
}

/// Gives the browser its User Library (browser R3): the folder the
/// preferences or `--user-library` name, or the platform's default place,
/// made with its starting folders the first time.
fn open_user_library(shell: &mut Shell, args: &Args) {
    let Some(library) = args
        .user_library
        .clone()
        .or_else(|| shell.preferences().browser.user_library.clone())
        .or_else(bl_shell::browser::places::default_user_library)
    else {
        return;
    };
    if let Err(error) = bl_shell::browser::places::create_user_library(&library) {
        shell.set_message(
            format!(
                "The User Library couldn't be made at {}: {error}",
                library.display()
            ),
            true,
        );
    }
    shell.set_user_library(Some(library));
}

/// Gives the Arrangement View its waveform peaks: `bl-media`'s background
/// peak cache, saving peak files in the platform's cache folder, over the
/// shell's media store, which the browser (WP-1.10) and set loading fill. A
/// thread passes the cache's events on and wakes the loop, so a waveform
/// appears when its peaks are ready and is redrawn when they're invalidated.
fn start_peaks(shell: &Shell, main_ui: &mut MainWindowUi, proxy: &EventLoopProxy<UserEvent>) {
    let store = shell.media_store();
    let cache =
        match bl_media::PeakCache::new(store, bl_media::PeakCacheOptions::platform_default()) {
            Ok(cache) => Arc::new(cache),
            Err(error) => {
                eprintln!("beatlayer: waveforms are off: {error}");
                return;
            }
        };
    let events = cache.subscribe();
    let (forward, received) = std::sync::mpsc::channel();
    let proxy = proxy.clone();
    let spawned = std::thread::Builder::new()
        .name("bl-peak-events".to_owned())
        .spawn(move || {
            // Ends when the cache, and with it the sender, is dropped.
            for event in events {
                if forward.send(event).is_err() || proxy.send_event(UserEvent::Peaks).is_err() {
                    break;
                }
            }
        });
    let events = spawned.is_ok().then_some(received);
    main_ui
        .arrangement_mut()
        .set_peak_source(Box::new(cache), events);
}

impl Host {
    /// Whether the menu bar is drawn in the window rather than native.
    fn drawn_menus(&self) -> bool {
        self.args.drawn_menus || cfg!(not(any(target_os = "macos", windows)))
    }

    fn create_surface(
        &self,
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
        share: Option<&Surface>,
    ) -> Result<Surface, String> {
        // AccessKit's adapter must exist before the window is first shown.
        let window = Arc::new(
            event_loop
                .create_window(attributes.with_visible(false))
                .map_err(|e| format!("couldn't open a window: {e}"))?,
        );
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts());
        self.theme.apply(&ctx);
        let zoom = self.shell.zoom();
        apply_zoom(&ctx, zoom);
        let mut state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        state.init_accesskit(event_loop, &window, self.proxy.clone());
        window.set_visible(true);

        let setup = match share.and_then(|s| s.painter.render_state()) {
            Some(render) => WgpuSetup::Existing(WgpuSetupExisting {
                instance: render.instance.clone(),
                adapter: render.adapter.clone(),
                device: render.device.clone(),
                queue: render.queue.clone(),
            }),
            None => WgpuSetup::CreateNew(WgpuSetupCreateNew::from_display_handle(
                event_loop.owned_display_handle(),
            )),
        };
        let config = WgpuConfiguration {
            wgpu_setup: setup,
            ..Default::default()
        };
        let mut painter = pollster::block_on(Painter::new(
            ctx.clone(),
            config,
            false,
            RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .map_err(|e| format!("couldn't draw in the window: {e}"))?;
        if let Some(side) = painter.max_texture_side() {
            state.set_max_texture_side(side);
        }
        let mut info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &ctx, &window, true);
        Ok(Surface {
            window,
            ctx,
            state,
            painter,
            info,
            repaint_at: None,
            zoom,
        })
    }

    fn open_main_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        // The window opens at its last size; the first time, at the
        // reference size Live is measured at (1600 x 1000 points with the
        // title bar), shrunk to fit a smaller screen.
        let (outer, inner) = match (self.args.window_size, self.shell.preferences().window_size) {
            (Some((w, h)), _) => (Some((w, h)), (w, content_height(h))),
            (None, Some((w, h))) => (None, (w, h)),
            (None, None) => (Some((1600.0, 1000.0)), (1600.0, content_height(1000.0))),
        };
        // No smaller than its minimum at the interface zoom, as far as the
        // screen allows (interface-zoom R7).
        let limit = event_loop
            .primary_monitor()
            .map(|monitor| screen_limit(monitor.size().to_logical(monitor.scale_factor())));
        let (minimum, size) = fit_main_window(
            within(egui::vec2(inner.0, inner.1), limit),
            self.shell.zoom(),
            DRAWN_TITLE_BAR,
            self.drawn_menus(),
            limit,
        );
        let (width, height) = (size.x, size.y);
        let attributes = Window::default_attributes()
            .with_title(self.shell.set_name())
            .with_inner_size(LogicalSize::new(width, height))
            .with_min_inner_size(LogicalSize::new(minimum.x, minimum.y));
        let surface = self.create_surface(event_loop, with_title_bar(attributes), None)?;
        if let Some(render) = surface.painter.render_state() {
            // The Arrangement View's own pipelines (WP-1.8a).
            bl_ui::arrangement::gpu::install(
                &render.device,
                render.target_format,
                &mut render.renderer.write().callback_resources,
            );
        }
        fit_title_bar(&surface.window);
        if let Some((_, h)) = outer {
            // Make the outer frame W x H, whatever the title bar's height on
            // this system (layout.md measures Live's window that way).
            let scale = surface.window.scale_factor();
            let outer_size = surface.window.outer_size().to_logical::<f32>(scale);
            let inner_size = surface.window.inner_size().to_logical::<f32>(scale);
            let decoration = outer_size.height - inner_size.height;
            if std::env::var_os("BEATLAYER_TRACE").is_some() {
                eprintln!(
                    "main window: outer {} x {} points, content {} x {}, title bar {decoration}",
                    outer_size.width, outer_size.height, inner_size.width, inner_size.height
                );
            }
            let wanted = (h - decoration).min(height).max(minimum.y);
            if decoration > 0.0 && (inner_size.height - wanted).abs() > 0.5 {
                let _ = surface
                    .window
                    .request_inner_size(LogicalSize::new(width, wanted));
            }
        }
        #[cfg(any(target_os = "macos", windows))]
        if !self.drawn_menus() {
            let menu = crate::native_menu::NativeMenu::build(
                &self.menus,
                &self.shell,
                &self.keymap,
                self.platform,
            )?;
            menu.attach(&surface.window)?;
            self.native_menu = Some(menu);
        }
        self.main = Some(surface);
        Ok(())
    }

    /// Builds the native menu bar again, after a binding changed, so its
    /// key equivalents are the ones in use (the custom-shortcuts spec,
    /// R10).
    #[cfg(any(target_os = "macos", windows))]
    fn rebuild_native_menu(&mut self, window: &winit::window::Window) {
        let built = crate::native_menu::NativeMenu::build(
            &self.menus,
            &self.shell,
            &self.keymap,
            self.platform,
        );
        let menu = match built {
            Ok(menu) => menu,
            Err(error) => {
                self.shell.set_message(error, true);
                return;
            }
        };
        if let Err(error) = menu.attach(window) {
            self.shell.set_message(error, true);
            return;
        }
        self.native_menu = Some(menu);
    }

    fn open_settings_window(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(settings) = &self.settings {
            settings.window.focus_window();
            return;
        }
        // Live's size at 100%, its content scaled by the interface zoom
        // (interface-zoom R8), within the screen.
        let limit = event_loop
            .primary_monitor()
            .map(|monitor| screen_limit(monitor.size().to_logical(monitor.scale_factor())));
        let size = within(settings_window(self.shell.zoom(), DRAWN_TITLE_BAR), limit);
        let attributes = Window::default_attributes()
            .with_title("Settings")
            .with_inner_size(LogicalSize::new(size.x, size.y))
            .with_resizable(false);
        match self.create_surface(event_loop, with_title_bar(attributes), self.main.as_ref()) {
            Ok(surface) => {
                fit_title_bar(&surface.window);
                surface.window.focus_window();
                self.settings = Some(surface);
                self.settings_ui.refresh();
            }
            Err(error) => {
                self.shell.close_settings();
                let _ = self.shell.take_effects();
                self.shell.set_message(error, true);
            }
        }
    }

    fn surface(&mut self, kind: WindowKind) -> &mut Option<Surface> {
        match kind {
            WindowKind::Main => &mut self.main,
            WindowKind::Settings => &mut self.settings,
            WindowKind::Assistant => &mut self.assistant,
            WindowKind::Device(device) => match self.devices.get_mut(&device) {
                Some((surface, _)) => surface,
                None => {
                    self.no_surface = None;
                    &mut self.no_surface
                }
            },
        }
    }

    fn kind_of(&self, id: WindowId) -> Option<WindowKind> {
        if self.main.as_ref().is_some_and(|s| s.window.id() == id) {
            Some(WindowKind::Main)
        } else if self.settings.as_ref().is_some_and(|s| s.window.id() == id) {
            Some(WindowKind::Settings)
        } else if self.assistant.as_ref().is_some_and(|s| s.window.id() == id) {
            Some(WindowKind::Assistant)
        } else {
            self.devices
                .iter()
                .find(|(_, (s, _))| s.as_ref().is_some_and(|s| s.window.id() == id))
                .map(|(device, _)| WindowKind::Device(*device))
        }
    }

    /// Every open window's surface.
    /// Makes the theme again if the Theme & Colors settings or the system's
    /// mode changed since it was made. Returns whether it did.
    fn sync_theme(&mut self) -> bool {
        let drawn = (self.shell.look().clone(), self.shell.theme_dark());
        if drawn == self.drawn {
            return false;
        }
        self.theme = self.library.resolve(&drawn.0, drawn.1);
        self.drawn = drawn;
        true
    }

    fn surfaces(&self) -> impl Iterator<Item = &Surface> {
        [&self.main, &self.settings, &self.assistant]
            .into_iter()
            .flatten()
            .chain(self.devices.values().filter_map(|(s, _)| s.as_ref()))
    }

    /// Opens, closes, shows, hides, and titles floating devices' windows to
    /// match the shell's (WP-2.5b).
    fn sync_device_windows(&mut self, event_loop: &ActiveEventLoop) {
        let wanted = self.shell.floating_windows().windows;
        let gone: Vec<_> = self
            .devices
            .keys()
            .filter(|d| !wanted.iter().any(|w| w.device == **d))
            .copied()
            .collect();
        for device in gone {
            // Docked or deleted: its window closes.
            self.devices.remove(&device);
            if self.shell.focused_window() == WindowKind::Device(device) {
                self.shell.set_focused_window(WindowKind::Main);
            }
        }
        let zoom = self.shell.zoom();
        for (index, floating) in wanted.iter().enumerate() {
            // The window holds its own title bar, if it draws one, above
            // the device.
            let least = LogicalSize::new(
                floating.min_width * zoom,
                floating.min_height * zoom + DEVICE_TITLE_STRIP,
            );
            // A side nothing grows along stays at the docked size (IMP-14).
            let most = LogicalSize::new(
                if floating.grows_across {
                    f32::from(u16::MAX)
                } else {
                    least.width
                },
                if floating.grows_down {
                    f32::from(u16::MAX)
                } else {
                    least.height
                },
            );
            if let Some((Some(surface), _)) = self.devices.get(&floating.device) {
                let window = &surface.window;
                window.set_title(&floating.title);
                window.set_min_inner_size(Some(least));
                window.set_max_inner_size(Some(most));
                window.set_visible(floating.shown);
                continue;
            }
            if self.devices.contains_key(&floating.device) {
                continue;
            }
            let (position, size) = match floating.frame {
                Some(frame) => (
                    LogicalPosition::new(frame.x as f32, frame.y as f32),
                    LogicalSize::new(frame.width as f32, frame.height as f32 + DEVICE_TITLE_STRIP),
                ),
                None => {
                    // A new window opens beside the main window's top-left
                    // corner, each a step further down and across.
                    let origin = self
                        .main
                        .as_ref()
                        .and_then(|m| {
                            m.window
                                .outer_position()
                                .ok()
                                .map(|p| p.to_logical::<f32>(m.window.scale_factor()))
                        })
                        .unwrap_or(LogicalPosition::new(0.0, 0.0));
                    #[expect(clippy::cast_precision_loss, reason = "a handful of windows")]
                    let step = 24.0 * (index % 10) as f32;
                    (
                        LogicalPosition::new(origin.x + 120.0 + step, origin.y + 120.0 + step),
                        least,
                    )
                }
            };
            let attributes = Window::default_attributes()
                .with_title(floating.title.clone())
                .with_position(position)
                .with_inner_size(size)
                .with_min_inner_size(least)
                .with_max_inner_size(most)
                .with_resizable(floating.grows_across || floating.grows_down)
                .with_visible(floating.shown);
            let attributes =
                device_window_attributes(attributes, self.main.as_ref().map(|m| &*m.window));
            match self.create_surface(event_loop, attributes, self.main.as_ref()) {
                Ok(surface) => {
                    if !floating.shown {
                        surface.window.set_visible(false);
                    }
                    make_tool_window(&surface.window);
                    fit_device_title_bar(&surface.window);
                    #[cfg(target_os = "macos")]
                    crate::title_bar::float_above_main(&surface.window);
                    if floating.frame.is_none() {
                        self.record_device_frame(floating.device, &surface.window);
                    }
                    self.devices
                        .insert(floating.device, (Some(surface), DeviceViewUi::new()));
                }
                Err(error) => self.shell.set_message(error, true),
            }
        }
    }

    /// Opens and closes the assistant's own window to match the shell's: it
    /// is open while the panel is open and floating (D24).
    fn sync_assistant_window(&mut self, event_loop: &ActiveEventLoop) {
        if !self.shell.assistant_window_open() {
            if self.assistant.take().is_some()
                && self.shell.focused_window() == WindowKind::Assistant
            {
                self.shell.set_focused_window(WindowKind::Main);
            }
            return;
        }
        if self.assistant.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Assistant")
            .with_inner_size(LogicalSize::new(380.0, 600.0))
            .with_min_inner_size(LogicalSize::new(240.0, 240.0));
        match self.create_surface(event_loop, attributes, self.main.as_ref()) {
            Ok(surface) => self.assistant = Some(surface),
            Err(error) => self.shell.set_message(error, true),
        }
    }

    /// Records where a floating device's window sits, for the set to save
    /// (`device.set_window`: no undo step).
    fn record_device_frame(&mut self, device: bl_model::DeviceId, window: &Window) {
        let scale = window.scale_factor();
        let Ok(position) = window.outer_position() else {
            return;
        };
        let position = position.to_logical::<f64>(scale);
        let size = window.inner_size().to_logical::<f64>(scale);
        // The frame is the device's contents, below the title bar the
        // window draws itself.
        let frame = bl_model::WindowFrame {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height - f64::from(DEVICE_TITLE_STRIP),
        };
        let _ = self.shell.run(
            bl_shell::device_commands::SET_WINDOW,
            json!({ "device": device, "frame": frame }),
        );
    }

    fn redraw(&mut self, kind: WindowKind, event_loop: &ActiveEventLoop) {
        if kind == WindowKind::Main {
            // The browser's background work: index updates, a decoded
            // preview, a preview that ended.
            self.shell.poll();
        }
        let Some(mut surface) = self.surface(kind).take() else {
            return;
        };
        // The person's own shortcuts changed: the keymap the window matches
        // keys against, and the native menu's key equivalents, follow (the
        // custom-shortcuts spec, R10).
        if self.keymap_revision != self.shell.keymap_revision() {
            self.keymap_revision = self.shell.keymap_revision();
            self.keymap = Keymap::from_data(self.shell.keymap().clone());
            #[cfg(any(target_os = "macos", windows))]
            if kind == WindowKind::Main {
                self.rebuild_native_menu(&surface.window);
            }
        }
        egui_winit::update_viewport_info(&mut surface.info, &surface.ctx, &surface.window, false);
        let mut input = surface.state.take_egui_input(&surface.window);
        input
            .viewports
            .insert(ViewportId::ROOT, surface.info.clone());

        // A value being typed into the Control Bar takes the keys as a
        // text field does (control-bar-transport R10).
        let text_focused = surface.ctx.text_edit_focused()
            || (kind == WindowKind::Main && self.main_ui.is_typing());
        let keys = match kind {
            WindowKind::Main => &mut self.main_keys,
            WindowKind::Settings => &mut self.settings_keys,
            WindowKind::Assistant => &mut self.assistant_keys,
            WindowKind::Device(_) => &mut self.device_keys,
        };
        let invocations = if kind == WindowKind::Settings && self.settings_ui.recording() {
            // The keys being recorded on the Shortcuts page are the
            // person's, not the app's (the custom shortcuts spec, R8).
            self.settings_ui
                .take_recorded_key(&mut input, &self.shell)
                .into_iter()
                .collect()
        } else {
            keys.take_bound_keys(&mut input, &self.keymap, &self.shell, text_focused)
        };
        if kind == WindowKind::Settings {
            self.settings_ui.prepare(&mut self.shell);
        } else {
            // The frame draws the playhead the engine last published
            // (control-bar-transport R14).
            self.shell.poll_engine();
        }

        let drawn = (kind == WindowKind::Main && self.drawn_menus()).then_some(DrawnMenuBar {
            menus: &self.menus,
            keymap: &self.keymap,
            platform: self.platform,
        });
        // A floating device's window keeps the system's title bar.
        let title_bar = !matches!(kind, WindowKind::Device(_) | WindowKind::Assistant)
            && draws_title_bar(&surface.window);
        let shell = &self.shell;
        let theme = &self.theme;
        let main_ui = &mut self.main_ui;
        let settings_ui = &mut self.settings_ui;
        let assistant_ui = &mut self.assistant_ui;
        let mut device_ui = match kind {
            WindowKind::Device(device) => self.devices.get_mut(&device).map(|(_, ui)| ui),
            _ => None,
        };
        let floating = match kind {
            WindowKind::Device(device) => shell
                .floating_windows()
                .windows
                .into_iter()
                .find(|w| w.device == device),
            _ => None,
        };
        let zoom = surface.zoom;
        let mut ui_events = Vec::new();
        let output = surface.ctx.run_ui(input, |ui| {
            ui_events = match kind {
                WindowKind::Main => main_ui.show(ui, shell, theme, title_bar, drawn),
                WindowKind::Settings => settings_ui.show(ui, shell, theme, title_bar),
                WindowKind::Assistant => {
                    let mut events = Vec::new();
                    let rect = ui.max_rect();
                    assistant_ui.show(ui, shell, theme, rect, false, &mut events);
                    events
                }
                WindowKind::Device(_) => {
                    let mut events = Vec::new();
                    if let (Some(device_ui), Some(floating)) =
                        (device_ui.as_deref_mut(), floating.as_ref())
                    {
                        let strip = DEVICE_TITLE_STRIP / zoom;
                        device_ui.show_window(ui, shell, theme, floating, strip, &mut events);
                    }
                    events
                }
            };
        });

        surface.state.handle_platform_output_with_event_loop(
            &surface.window,
            event_loop,
            output.platform_output,
        );
        let primitives = surface
            .ctx
            .tessellate(output.shapes, output.pixels_per_point);
        let mut textures = output.textures_delta;
        let clear = egui::Rgba::from(self.theme.colors.window.0).to_array();
        surface.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            clear,
            &primitives,
            &mut textures,
            Vec::new(),
            &surface.window,
        );
        // Agents' view captures (WP-1.16): this frame, drawn again offscreen.
        self.capture_frame(kind, &surface, &primitives, output.pixels_per_point);
        let mut repaint_delay = Duration::MAX;
        if let Some(viewport) = output.viewport_output.get(&ViewportId::ROOT) {
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(
                &surface.ctx,
                &mut surface.info,
                viewport.commands.iter().cloned(),
                &surface.window,
                &mut actions,
            );
            repaint_delay = viewport.repaint_delay;
        }
        if repaint_delay.is_zero() {
            surface.window.request_redraw();
            surface.repaint_at = None;
        } else {
            surface.repaint_at = Instant::now().checked_add(repaint_delay);
        }
        *self.surface(kind) = Some(surface);

        for invocation in invocations {
            self.invoke(&invocation);
        }
        for event in ui_events {
            match event {
                UiEvent::Invoke(invocation) => self.invoke(&invocation),
                UiEvent::FocusArea(area) => self.shell.set_focused_area(area),
                UiEvent::FocusTrack(track) => self.shell.set_focused_track(track),
                UiEvent::FocusValue(focused) => self.shell.set_value_focused(focused),
                UiEvent::StatusHint(hint) => self.shell.set_hint(hint),
                UiEvent::ArrangementViewport { width, height } => {
                    self.shell.set_arrangement_viewport(width, height);
                }
                UiEvent::Gesture { phase, invocation } => {
                    // A drag's frame that's refused (a clip pushed before
                    // 1.1.1) keeps the last one that worked; there's
                    // nothing to report.
                    let _ = self.shell.gesture(phase, invocation.as_ref());
                }
                UiEvent::BeginGesture(label) => self.shell.begin_gesture(label),
                UiEvent::BeginNamedGesture(label) => self.shell.begin_gesture(&label),
                UiEvent::EndGesture => self.shell.end_gesture(),
            }
        }
        self.after_commands(event_loop);
    }

    fn invoke(&mut self, invocation: &Invocation) {
        // Errors show in the Status Bar; the shell has put them there.
        let _ = self.shell.invoke(&invocation.id, invocation.args.clone());
        // The engine acts at its next block: look until it has.
        self.watch_transport_until = Some(Instant::now() + TRANSPORT_WATCH);
    }

    /// Carries out what commands asked for and brings the windows and menus
    /// up to date.
    fn after_commands(&mut self, event_loop: &ActiveEventLoop) {
        loop {
            let effects = self.shell.take_effects();
            if effects.is_empty() {
                break;
            }
            for effect in effects {
                self.effect(effect, event_loop);
            }
        }
        let on_theme_page =
            self.shell.settings_open() && self.shell.settings_page() == SettingsPage::ThemeColors;
        if on_theme_page && !self.on_theme_page {
            self.library.refresh(&mut self.shell);
            // A theme file may have changed under the same name: make the
            // theme again.
            self.drawn.0.theme.clear();
        }
        self.on_theme_page = on_theme_page;
        if self.sync_theme() {
            for surface in self.surfaces() {
                self.theme.apply(&surface.ctx);
                surface.window.request_redraw();
            }
        }
        // The interface zoom (interface-zoom R1, R4, R7, R8): the main
        // window follows at once and keeps its size unless that's under its
        // minimum at the new zoom; the Settings window follows once no mouse
        // button is down in it, and takes its size at the new zoom.
        let zoom = self.shell.zoom();
        let drawn_menus = self.drawn_menus();
        if let Some(main) = &mut self.main
            && (main.zoom - zoom).abs() > 1e-6
        {
            apply_zoom(&main.ctx, zoom);
            main.zoom = zoom;
            resize_main_window(&main.window, zoom, drawn_menus);
            main.window.request_redraw();
        }
        let mut rezoomed = false;
        for surface in self
            .devices
            .values_mut()
            .filter_map(|(s, _)| s.as_mut())
            .chain(self.assistant.as_mut())
        {
            if (surface.zoom - zoom).abs() > 1e-6 {
                apply_zoom(&surface.ctx, zoom);
                surface.zoom = zoom;
                surface.window.request_redraw();
                rezoomed = true;
            }
        }
        if rezoomed {
            // Their smallest sizes follow the zoom.
            self.sync_device_windows(event_loop);
        }
        if let Some(settings) = &mut self.settings {
            let applied = follow_zoom(&settings.ctx, settings.zoom, zoom);
            if (applied - settings.zoom).abs() > 1e-6 {
                settings.zoom = applied;
                fit_settings_window(&settings.window, applied);
                settings.window.request_redraw();
            }
        }
        self.sync_agent_server();
        self.take_captures();
        self.take_renders();
        self.resume_agents();
        self.agent.sync_engine(&mut self.shell);
        if self.shown_revision != Some(self.shell.revision()) {
            self.sync_device_windows(event_loop);
            self.sync_assistant_window(event_loop);
            self.shown_revision = Some(self.shell.revision());
            #[cfg(any(target_os = "macos", windows))]
            if let Some(menu) = &self.native_menu {
                menu.refresh(&self.shell, &self.keymap, self.platform);
            }
            for surface in self.surfaces() {
                surface.window.request_redraw();
            }
            if let Some(main) = &self.main {
                main.window.set_title(&self.shell.set_name());
            }
        }
    }

    fn effect(&mut self, effect: Effect, event_loop: &ActiveEventLoop) {
        match effect {
            Effect::ConfirmDiscard { then } => self.confirm_discard(&then),
            Effect::PickOpenPath { discard_changes } => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Open Beatlayer Set")
                    .add_filter("Beatlayer Set", &["blset"]);
                if let Some(main) = &self.main {
                    dialog = dialog.set_parent(main.window.as_ref());
                }
                if let Some(path) = dialog.pick_file() {
                    let open =
                        Invocation::new(ids::SET_OPEN, json!({ "path": path.to_string_lossy() }));
                    if discard_changes {
                        // The person chose Don't Save before picking it.
                        let _ = self.shell.discard_and_invoke(&open);
                    } else {
                        self.invoke(&open);
                    }
                }
            }
            Effect::PickSavePath { then } => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save Beatlayer Set As")
                    .add_filter("Beatlayer Set", &["blset"])
                    .set_file_name(format!("{}.blset", self.shell.set_name()));
                // Spec: app-shell R22.
                if let Some(folder) = self.shell.save_dialog_folder() {
                    dialog = dialog.set_directory(folder);
                }
                if let Some(main) = &self.main {
                    dialog = dialog.set_parent(main.window.as_ref());
                }
                if let Some(path) = dialog.save_file() {
                    let saved = self
                        .shell
                        .invoke(ids::SET_SAVE_AS, json!({ "path": path.to_string_lossy() }));
                    if saved.is_ok()
                        && let Some(then) = then
                    {
                        let _ = self.shell.discard_and_invoke(&then);
                    }
                }
            }
            Effect::Quit => {
                if let Some(main) = &self.main {
                    let size = main
                        .window
                        .inner_size()
                        .to_logical::<f32>(main.window.scale_factor());
                    self.shell.set_window_size(size.width, size.height);
                }
                self.shell.save_preferences();
                self.cancel_renders();
                self.agent.set_view_capture(false);
                self.quitting = true;
                event_loop.exit();
            }
            Effect::Hide => {
                #[cfg(target_os = "macos")]
                {
                    use winit::platform::macos::ActiveEventLoopExtMacOS as _;
                    event_loop.hide_application();
                }
            }
            Effect::ShowAbout => {
                let _ = rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Info)
                    .set_title("About Beatlayer")
                    .set_description(format!(
                        "Beatlayer {}\n\nAn open-source digital audio workstation, licensed \
                         under the GNU General Public License, version 3 or later.\n\nIts \
                         interface typeface is Inter, by the Inter Project Authors, under the \
                         SIL Open Font License 1.1.",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
            }
            Effect::PickFolder => {
                let mut dialog = rfd::FileDialog::new().set_title("Add Folder to Browser");
                if let Some(main) = &self.main {
                    dialog = dialog.set_parent(main.window.as_ref());
                }
                if let Some(path) = dialog.pick_folder() {
                    self.invoke(&Invocation::new(
                        bl_shell::browser_commands::ADD_FOLDER,
                        json!({ "path": path.to_string_lossy() }),
                    ));
                }
            }
            Effect::PickTemporaryFolder => {
                let dialog = rfd::FileDialog::new().set_title("Temporary Folder");
                if let Some(path) = dialog.pick_folder() {
                    self.invoke(&Invocation::new(
                        bl_shell::commands::SETTINGS_SET_TEMPORARY_FOLDER,
                        json!({ "path": path.to_string_lossy() }),
                    ));
                }
            }
            Effect::PickSampleEditor => {
                // Spec: sample-editor R16. The File & Folder page's Sample
                // Editor row names the program Edit opens a clip's file in.
                let dialog = rfd::FileDialog::new().set_title("Sample Editor");
                if let Some(path) = dialog.pick_file() {
                    self.invoke(&Invocation::new(
                        bl_shell::commands::SETTINGS_SET_SAMPLE_EDITOR,
                        json!({ "path": path.to_string_lossy() }),
                    ));
                }
            }
            Effect::PickSearchFolder => {
                // Spec: file-management R14. The File Manager's Set Folder…
                // button names the folder the Automatic Search looks in.
                let mut dialog = rfd::FileDialog::new().set_title("Search Folder");
                if let Some(main) = &self.main {
                    dialog = dialog.set_parent(main.window.as_ref());
                }
                if let Some(path) = dialog.pick_folder() {
                    self.invoke(&Invocation::new(
                        bl_shell::file_manager_commands::SET_SEARCH_FOLDER,
                        json!({ "folder": path.to_string_lossy() }),
                    ));
                }
            }
            Effect::PickPluginListPath { default_name } => {
                // Spec: plugin-list R7. The list's Export… button asks
                // where to save the list.
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Export the Set's Plug-In List")
                    .add_filter("Text", &["txt"])
                    .set_file_name(default_name);
                // Spec: plugin-list R7. The list goes in the set's project
                // folder, where the system suggests for a set never saved.
                if let Some(folder) = self.shell.save_dialog_folder() {
                    dialog = dialog.set_directory(folder);
                }
                if let Some(main) = &self.main {
                    dialog = dialog.set_parent(main.window.as_ref());
                }
                if let Some(path) = dialog.save_file() {
                    self.invoke(&Invocation::new(
                        bl_shell::plugin_commands::EXPORT_LIST,
                        json!({ "path": path.to_string_lossy() }),
                    ));
                }
            }
            Effect::OpenSettings => self.open_settings_window(event_loop),
            Effect::CloseSettings => {
                self.settings = None;
                if let Some(main) = &self.main {
                    main.window.focus_window();
                }
            }
            Effect::SetFullScreen(on) => {
                if let Some(main) = &self.main {
                    main.window
                        .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
                }
            }
            Effect::RenameDevice(_)
            | Effect::RenameChain(_)
            | Effect::RenameMacro(_, _)
            | Effect::RenameVariation(_, _)
            | Effect::RenameTrack(_) => {
                if let Some(main) = &self.main {
                    self.main_ui.apply_effect(&main.ctx, &effect);
                    main.window.request_redraw();
                }
            }
            Effect::FocusArea(area) => {
                if let Some(main) = &self.main {
                    main.window.focus_window();
                    self.main_ui
                        .apply_effect(&main.ctx, &Effect::FocusArea(area));
                }
            }
            Effect::MoveFocus { .. } => match self.shell.focused_window() {
                WindowKind::Main => {
                    if let Some(main) = &self.main {
                        self.main_ui.apply_effect(&main.ctx, &effect);
                        main.window.request_redraw();
                    }
                }
                WindowKind::Settings => {
                    self.settings_ui.apply_effect(&effect);
                    if let Some(settings) = &self.settings {
                        settings.window.request_redraw();
                    }
                }
                WindowKind::Device(_) | WindowKind::Assistant => {}
            },
        }
    }

    /// Starts, stops, or moves the agent server to match the settings, and
    /// tells the shell how it's doing, for the Settings window.
    fn sync_agent_server(&mut self) {
        let settings = self.shell.agent_settings();
        let wanted = (settings.server && !settings.token.is_empty()).then(|| HttpConfig {
            port: settings.port,
            token: settings.token.expose().to_owned(),
        });
        let running = self.agent_server.as_ref().map(|(_, config)| config);
        if running != wanted.as_ref() {
            if let Some((server, _)) = self.agent_server.take() {
                server.stop();
                // Their connections have closed.
                self.agent_parked.clear();
            }
            if let Some(config) = wanted.filter(|c| self.agent_failed.as_ref() != Some(c)) {
                let proxy = Mutex::new(self.proxy.clone());
                let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                    if let Ok(proxy) = proxy.lock() {
                        let _ = proxy.send_event(UserEvent::Agent);
                    }
                });
                match HttpServer::start(&config, wake) {
                    Ok(server) => {
                        self.agent_failed = None;
                        self.agent_server = Some((server, config));
                    }
                    Err(error) => {
                        self.agent_failed = Some(config);
                        self.shell.set_agent_status(AgentServerStatus {
                            listening: None,
                            error: Some(error.to_string()),
                            sessions: 0,
                        });
                        return;
                    }
                }
            }
        }
        if !self.shell.agent_settings().server {
            self.agent_failed = None;
        }
        // Each allowed agent's own token speaks for it (WP-1.15b).
        if let Some((server, _)) = &self.agent_server {
            server.set_agent_tokens(self.shell.agent_settings());
        }
        // Agents the person forgot lose their sessions.
        let forgotten: Vec<String> = self
            .agent
            .sessions()
            .filter(|(_, s)| s.protocol.is_some())
            .filter_map(|(_, s)| s.agent.clone())
            .filter(|name| self.shell.agent_settings().client(name).is_none())
            .collect();
        for name in forgotten {
            self.agent.close_sessions_of(&name);
        }
        let status = match &self.agent_server {
            Some((server, _)) => AgentServerStatus {
                listening: Some(server.local_addr().to_string()),
                error: None,
                sessions: self.agent.sessions().count(),
            },
            None if self.agent_failed.is_some() => self.shell.agent_status().clone(),
            None => AgentServerStatus::default(),
        };
        self.shell.set_agent_status(status);
    }

    /// Runs the agent server's waiting requests on the shell, asking the
    /// person where a request needs it.
    fn serve_agents(&mut self) {
        let Some((server, _)) = &self.agent_server else {
            return;
        };
        let jobs: Vec<_> = std::iter::from_fn(|| server.try_job()).collect();
        // What the engine has done, so `status` and scheduling see it.
        self.agent.sync_engine(&mut self.shell);
        for job in jobs {
            let served = bl_agent::http::serve(&mut self.agent, &mut self.shell, job);
            self.agent_served(served);
            // What a call asked of the engine goes to it straight away.
            self.agent.sync_engine(&mut self.shell);
        }
    }

    /// Carries out what serving an agent's request came to: asking the
    /// person, or keeping a call until the person's gesture ends.
    fn agent_served(&mut self, served: Served) {
        match served {
            Served::Done => {}
            Served::Waiting(pending) => {
                let allow = self.ask_for_agent(&pending.question().text());
                pending.finish(&mut self.agent, &mut self.shell, allow);
            }
            Served::Parked(parked) => self.agent_parked.push(parked),
        }
    }

    /// Once the person's gesture has ended, runs the agents' calls that
    /// waited for it, in the order they came, each as its own undo step
    /// (WP-1.15b).
    fn resume_agents(&mut self) {
        // Answered view captures come back even mid-gesture; the agent API
        // holds everything else until the gesture ends.
        if !self.agent.holds_calls() {
            return;
        }
        while let Some(served) =
            bl_agent::http::resume(&mut self.agent, &mut self.shell, &mut self.agent_parked)
        {
            self.agent_served(served);
            self.agent.sync_engine(&mut self.shell);
        }
    }

    /// Takes the view captures agents asked for (WP-1.16): a view that isn't
    /// on screen, or a minimised window, fails at once; otherwise its
    /// window draws a frame that's read back.
    fn take_captures(&mut self) {
        for request in self.agent.take_capture_requests() {
            let window =
                crate::capture::window_of(request.view, &self.shell, self.settings.is_some())
                    .and_then(|window| {
                        let surface = match window {
                            WindowKind::Main => self.main.as_ref(),
                            WindowKind::Settings => self.settings.as_ref(),
                            WindowKind::Assistant => self.assistant.as_ref(),
                            WindowKind::Device(device) => {
                                self.devices.get(&device).and_then(|(s, _)| s.as_ref())
                            }
                        }
                        .ok_or("that window isn't open")?;
                        if surface.window.is_minimized() == Some(true) {
                            return Err(
                                "the window is minimised, so it isn't drawn; ask the person \
                                    to bring it back"
                                    .to_owned(),
                            );
                        }
                        surface.window.request_redraw();
                        Ok(window)
                    });
            match window {
                Ok(window) => self.captures.push(PendingCapture {
                    ticket: request.ticket,
                    view: request.view,
                    max_size: request.max_size,
                    window,
                    deadline: Instant::now() + CAPTURE_TIMEOUT,
                    redrawn: Instant::now(),
                }),
                Err(message) => self.agent.finish_capture(request.ticket, Err(message)),
            }
        }
    }

    /// Captures the views agents asked for from the frame `kind`'s window
    /// just drew: the frame's `primitives` drawn again offscreen with the
    /// window's renderer and read back, then each view cropped, scaled, and
    /// encoded on a thread of its own; [`UserEvent::Captured`] brings the
    /// images back.
    fn capture_frame(
        &mut self,
        kind: WindowKind,
        surface: &Surface,
        primitives: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
    ) {
        if !self.captures.iter().any(|c| c.window == kind) {
            return;
        }
        let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut self.captures)
            .into_iter()
            .partition(|c| c.window == kind);
        self.captures = others;
        let size = surface.window.inner_size();
        let frame = surface
            .painter
            .render_state()
            .ok_or_else(|| "the window has no renderer".to_owned())
            .and_then(|render| {
                crate::capture::render_frame(
                    &render.device,
                    &render.queue,
                    &mut render.renderer.write(),
                    render.target_format,
                    primitives,
                    [size.width.max(1), size.height.max(1)],
                    pixels_per_point,
                    egui::Rgba::from(self.theme.colors.window.0),
                )
            })
            .map(Arc::new);
        for capture in mine {
            let job = frame.clone().and_then(|image| {
                crate::capture::region(capture.view, &self.shell, &self.main_ui)
                    .map(|region| (image, region))
            });
            let (image, region) = match job {
                Ok(job) => job,
                Err(message) => {
                    self.agent.finish_capture(capture.ticket, Err(message));
                    continue;
                }
            };
            let sender = self.captured.0.clone();
            let proxy = self.proxy.clone();
            let ticket = capture.ticket;
            let max_size = capture.max_size;
            let spawned = std::thread::Builder::new()
                .name("view capture".to_owned())
                .spawn(move || {
                    let result = crate::capture::encode(&image, region, pixels_per_point, max_size);
                    let _ = sender.send((ticket, result));
                    let _ = proxy.send_event(UserEvent::Captured);
                });
            if spawned.is_err() {
                self.agent
                    .finish_capture(ticket, Err("couldn't start encoding the image".to_owned()));
            }
        }
    }

    /// Hands encoded captures back to the agent API, and fails captures
    /// whose window drew no frame in time.
    fn finish_captures(&mut self) {
        while let Ok((ticket, result)) = self.captured.1.try_recv() {
            self.agent.finish_capture(ticket, result);
        }
        let now = Instant::now();
        let (late, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.captures)
            .into_iter()
            .partition(|c| c.deadline <= now);
        self.captures = waiting;
        for capture in late {
            self.agent.finish_capture(
                capture.ticket,
                Err(
                    "the window drew no frame to capture in time: it may be hidden or \
                     minimised"
                        .to_owned(),
                ),
            );
        }
    }

    /// Takes the audio render requests agents asked for (WP-2.28): queues
    /// each on the background worker thread (`bl-preview-worker`); [`UserEvent::Rendered`]
    /// brings the results back.
    fn take_renders(&mut self) {
        for request in self.agent.take_render_requests() {
            let cancel = Arc::new(AtomicBool::new(false));
            self.render_cancels.push(Arc::clone(&cancel));
            let ticket = request.ticket;
            if self
                .render_jobs
                .send(RenderJob {
                    ticket,
                    request,
                    cancel,
                })
                .is_err()
            {
                self.agent.finish_render(
                    &mut self.shell,
                    ticket,
                    Err("couldn't send request to the preview worker".to_owned()),
                );
            }
        }
    }

    /// Hands finished audio renders back to the agent API (WP-2.28).
    fn finish_renders(&mut self) {
        while let Ok((ticket, result)) = self.rendered.1.try_recv() {
            self.agent.finish_render(&mut self.shell, ticket, result);
        }
        self.render_cancels
            .retain(|cancel| Arc::strong_count(cancel) > 1);
    }

    /// Cancels any preview renders currently queued or executing (WP-2.28).
    fn cancel_renders(&mut self) {
        for cancel in self.render_cancels.drain(..) {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Asks the person a question for an agent: whether it may connect, or
    /// run a command that asks first. The call waits for the answer.
    fn ask_for_agent(&self, text: &str) -> bool {
        let mut dialog = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Beatlayer")
            .set_description(text)
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                "Allow".to_owned(),
                "Don't Allow".to_owned(),
            ));
        if let Some(main) = &self.main {
            main.window
                .request_user_attention(Some(winit::window::UserAttentionType::Critical));
            dialog = dialog.set_parent(main.window.as_ref());
        }
        match dialog.show() {
            rfd::MessageDialogResult::Ok | rfd::MessageDialogResult::Yes => true,
            rfd::MessageDialogResult::Custom(choice) => choice == "Allow",
            _ => false,
        }
    }

    /// Asks whether to save unsaved changes before `then` (Live asks the
    /// same before a new set, opening one, or quitting).
    fn confirm_discard(&mut self, then: &Invocation) {
        let verb = match then.id.as_str() {
            ids::APP_QUIT => "quitting",
            ids::SET_OPEN => "opening another set",
            _ => "starting a new set",
        };
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Beatlayer")
            .set_description(format!(
                "Save changes to \u{201c}{}\u{201d} before {verb}?",
                self.shell.set_name()
            ))
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                "Save".to_owned(),
                "Don't Save".to_owned(),
                "Cancel".to_owned(),
            ))
            .show();
        match answer {
            rfd::MessageDialogResult::Yes => self.save_then(then),
            rfd::MessageDialogResult::Custom(choice) if choice == "Save" => self.save_then(then),
            rfd::MessageDialogResult::No => {
                let _ = self.shell.discard_and_invoke(then);
            }
            rfd::MessageDialogResult::Custom(choice) if choice == "Don't Save" => {
                let _ = self.shell.discard_and_invoke(then);
            }
            _ => {}
        }
    }

    /// Saves the set, asking where if it has no file yet, then runs `then`
    /// if the save worked.
    fn save_then(&mut self, then: &Invocation) {
        if self.shell.set_path().is_some() {
            if self.shell.invoke(ids::SET_SAVE, json!({})).is_ok() {
                let _ = self.shell.discard_and_invoke(then);
            }
        } else {
            self.save_as_then(then.clone());
        }
    }

    fn save_as_then(&mut self, then: Invocation) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save Beatlayer Set As")
            .add_filter("Beatlayer Set", &["blset"])
            .set_file_name(format!("{}.blset", self.shell.set_name()));
        if let Some(main) = &self.main {
            dialog = dialog.set_parent(main.window.as_ref());
        }
        if let Some(path) = dialog.save_file() {
            let saved = self
                .shell
                .invoke(ids::SET_SAVE_AS, json!({ "path": path.to_string_lossy() }));
            if saved.is_ok() {
                let _ = self.shell.discard_and_invoke(&then);
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.main.is_some() {
            return;
        }
        if let Err(error) = self.open_main_window(event_loop) {
            eprintln!("beatlayer: {error}");
            event_loop.exit();
            return;
        }
        // Follow System starts from the system's mode (themes R3).
        if let Some(theme) = self.main.as_ref().and_then(|main| main.window.theme()) {
            self.shell
                .set_system_dark(theme == winit::window::Theme::Dark);
            self.after_commands(event_loop);
        }
        // Agents can capture views now there's a window (WP-1.16).
        self.agent.set_view_capture(true);
        if self.args.open_settings {
            self.invoke(&Invocation::new(ids::APP_SETTINGS, json!({})));
            self.after_commands(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(kind) = self.kind_of(id) else {
            return;
        };
        if event == WindowEvent::RedrawRequested {
            self.redraw(kind, event_loop);
            return;
        }
        let repaint = {
            let Some(surface) = self.surface(kind).as_mut() else {
                return;
            };
            let response = surface.state.on_window_event(&surface.window, &event);
            if let WindowEvent::Resized(size) = &event
                && let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
            {
                surface.painter.on_window_resized(ViewportId::ROOT, w, h);
            }
            if matches!(
                event,
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
            ) {
                // AppKit lays its title bar out again on a resize and on
                // leaving full screen.
                if matches!(kind, WindowKind::Device(_)) {
                    fit_device_title_bar(&surface.window);
                } else if kind != WindowKind::Assistant {
                    fit_title_bar(&surface.window);
                }
            }
            response.repaint
        };
        match event {
            WindowEvent::CloseRequested => {
                match kind {
                    WindowKind::Main => self.invoke(&Invocation::new(ids::APP_QUIT, json!({}))),
                    WindowKind::Settings => self.shell.close_settings(),
                    // Closing the floating assistant closes the panel.
                    WindowKind::Assistant => self.invoke(&Invocation::new(
                        bl_shell::assistant::VIEW_TOGGLE_ASSISTANT,
                        json!({}),
                    )),
                    // Closing a floating device's window docks the device
                    // (IMP-14): one undo step.
                    WindowKind::Device(device) => self.invoke(&Invocation::new(
                        bl_shell::device_commands::SET_FLOATING,
                        json!({ "device": device, "floating": false }),
                    )),
                }
                self.after_commands(event_loop);
            }
            WindowEvent::Moved(_) | WindowEvent::Resized(_)
                if matches!(kind, WindowKind::Device(_)) =>
            {
                if let WindowKind::Device(device) = kind
                    && let Some((Some(surface), _)) = self.devices.get(&device)
                {
                    let window = Arc::clone(&surface.window);
                    if window.is_visible() != Some(false) {
                        self.record_device_frame(device, &window);
                    }
                }
            }
            // Follow System (themes R3).
            WindowEvent::ThemeChanged(theme) => {
                self.shell
                    .set_system_dark(theme == winit::window::Theme::Dark);
                self.after_commands(event_loop);
            }
            WindowEvent::Focused(true) => {
                self.shell.set_focused_window(kind);
                if kind == WindowKind::Main {
                    self.shell.set_focused_area(self.shell.focused_area());
                    // Spec: sample-editor R16. A file opened in the external
                    // editor is looked at again when Beatlayer comes to the
                    // front.
                    self.shell.check_edited_samples();
                }
                self.after_commands(event_loop);
            }
            WindowEvent::Resized(_) if kind == WindowKind::Main => {
                if let Some(main) = &self.main {
                    let size = main
                        .window
                        .inner_size()
                        .to_logical::<f32>(main.window.scale_factor());
                    self.shell.set_window_size(size.width, size.height);
                    let full = main.window.fullscreen().is_some();
                    self.shell.set_full_screen_state(full);
                }
            }
            _ => {}
        }
        if repaint && let Some(surface) = self.surface(kind).as_ref() {
            surface.window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        // Only the native menus' events need the loop.
        #[cfg(not(any(target_os = "macos", windows)))]
        let _ = event_loop;
        match event {
            UserEvent::AccessKit(event) => {
                let Some(kind) = self.kind_of(event.window_id) else {
                    return;
                };
                let Some(surface) = self.surface(kind).as_mut() else {
                    return;
                };
                use egui_winit::accesskit_winit::WindowEvent as Ak;
                match event.window_event {
                    Ak::InitialTreeRequested => surface.ctx.enable_accesskit(),
                    Ak::ActionRequested(request) => {
                        surface.state.on_accesskit_action_request(request)
                    }
                    Ak::AccessibilityDeactivated => surface.ctx.disable_accesskit(),
                }
                surface.window.request_redraw();
            }
            UserEvent::Agent => {
                self.serve_agents();
                self.after_commands(event_loop);
            }
            UserEvent::Peaks => {
                if let Some(main) = &self.main {
                    main.window.request_redraw();
                }
            }
            UserEvent::Browser => {
                self.shell.poll();
                if let Some(main) = &self.main {
                    main.window.request_redraw();
                }
            }
            UserEvent::Captured => {
                self.finish_captures();
                self.after_commands(event_loop);
            }
            UserEvent::Rendered => {
                self.finish_renders();
                self.after_commands(event_loop);
            }
            #[cfg(any(target_os = "macos", windows))]
            UserEvent::Menu(event) => {
                let invocation = self.native_menu.as_ref().and_then(|menu| {
                    menu.invocation(event.id(), &self.shell, &self.keymap, self.platform)
                });
                if let Some(invocation) = invocation {
                    self.invoke(&invocation);
                    self.after_commands(event_loop);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quitting {
            return;
        }
        let now = Instant::now();
        if now >= self.next_cpu_sample {
            self.next_cpu_sample = now + CPU_METER_PERIOD;
            if self.shell.audio_mut().sample_cpu_load().is_some()
                && let Some(main) = &self.main
            {
                main.window.request_redraw();
            }
        }
        let mut wake = self
            .shell
            .audio()
            .is_running()
            .then_some(self.next_cpu_sample);
        // Agents' scheduled commands that fire come back as their undo
        // steps (WP-1.15b): hand the engine's reports over, and look again
        // soon while any wait.
        let revision = self.shell.revision();
        self.agent.sync_engine(&mut self.shell);
        if self.shell.revision() != revision {
            self.after_commands(event_loop);
        }
        if self.agent.scheduled_waiting() > 0 && self.shell.audio().engine_running() {
            let next = now + AGENT_ENGINE_PERIOD;
            wake = Some(wake.map_or(next, |w| w.min(next)));
        }
        // View captures (WP-1.16): a window is asked to draw until it has,
        // and a capture whose window draws nothing fails in time.
        if !self.captures.is_empty() {
            let before = self.captures.len();
            self.finish_captures();
            if self.captures.len() != before {
                self.after_commands(event_loop);
            }
            for capture in &mut self.captures {
                if now.duration_since(capture.redrawn) < CAPTURE_PERIOD {
                    continue;
                }
                capture.redrawn = now;
                let surface = match capture.window {
                    WindowKind::Main => self.main.as_ref(),
                    WindowKind::Settings => self.settings.as_ref(),
                    WindowKind::Assistant => self.assistant.as_ref(),
                    WindowKind::Device(device) => {
                        self.devices.get(&device).and_then(|(s, _)| s.as_ref())
                    }
                };
                if let Some(surface) = surface {
                    surface.window.request_redraw();
                }
            }
            if !self.captures.is_empty() {
                let next = now + CAPTURE_PERIOD;
                wake = Some(wake.map_or(next, |w| w.min(next)));
            }
        }
        if let Some(until) = self.watch_transport_until {
            if now < until {
                // A start, stop, or jump shows at once: a change the
                // engine has made refreshes the window and the menus, and
                // while the song plays the window redraws every frame.
                let revision = self.shell.revision();
                self.shell.poll_engine();
                if self.shell.revision() != revision {
                    self.after_commands(event_loop);
                }
                let next = now + TRANSPORT_WATCH_PERIOD;
                wake = Some(wake.map_or(next, |w| w.min(next)));
            } else {
                self.watch_transport_until = None;
            }
        }
        let devices = self.devices.values_mut().filter_map(|(s, _)| s.as_mut());
        for surface in [&mut self.main, &mut self.settings]
            .into_iter()
            .flatten()
            .chain(devices)
        {
            match surface.repaint_at {
                Some(at) if at <= now => {
                    surface.repaint_at = None;
                    surface.window.request_redraw();
                }
                Some(at) => wake = Some(wake.map_or(at, |w| w.min(at))),
                None => {}
            }
        }
        event_loop.set_control_flow(wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}
