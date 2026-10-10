//! WP-0.8b: the egui UI spike for Beatlayer.
//!
//! The spike owns the winit event loop and drives egui through `egui-winit`
//! and `egui-wgpu` directly rather than through eframe, so it can create a
//! second, non-egui native window (the plug-in editor stand-in) while the loop
//! runs, and so it can time the whole frame (layout, tessellation, and paint).
//!
//! Run `ui-egui-spike --help` for the options.

mod bench;
mod child_view;
mod cputime;
mod dense;
mod fonts;
mod paint_cpu;
mod paint_gpu;
mod scene;
mod ui;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui::{ViewportId, ViewportInfo};
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{CursorGrabMode, Window, WindowId, WindowLevel};

use bench::{FrameSample, Phase, Script};
use ui::{App, DenseMode, PointerLock};

const USAGE: &str = "\
Usage: ui-egui-spike [options]

  --dense gpu|painter   Draw clips, waveforms, and meters with a wgpu paint
                        callback (default) or with egui's own mesh painting
  --bench               Run the scripted 30-second scenario and exit
  --bench-seconds N     Length of the scenario (default 30)
  --all-tracks          In the bench, shrink tracks so all 200 are visible
  --bench-out FILE      Write every frame's timing to FILE as CSV
  --meters-off          Start with the meters stopped (for idle measurements)
  --open-plugin-window  Open the plug-in editor stand-in at start-up
  --on-top              Keep the main window above others (implied by
                        --bench, so the window is never occluded mid-run)
  --no-activate         On macOS, don't make the spike the active app
  --x11                 On Linux, use X11 (XWayland) rather than Wayland
";

#[derive(Clone, Debug)]
struct Args {
    mode: DenseMode,
    bench: bool,
    bench_seconds: f64,
    all_tracks: bool,
    bench_out: Option<PathBuf>,
    meters_off: bool,
    open_plugin_window: bool,
    on_top: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    no_activate: bool,
    #[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
    x11: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        mode: DenseMode::Gpu,
        bench: false,
        bench_seconds: 30.0,
        all_tracks: false,
        bench_out: None,
        meters_off: false,
        open_plugin_window: false,
        on_top: false,
        no_activate: false,
        x11: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dense" => {
                args.mode = match it.next().as_deref() {
                    Some("gpu") => DenseMode::Gpu,
                    Some("painter") => DenseMode::Painter,
                    other => return Err(format!("--dense takes gpu or painter, not {other:?}")),
                }
            }
            "--bench" => args.bench = true,
            "--bench-seconds" => {
                args.bench_seconds = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--bench-seconds takes a number")?;
            }
            "--all-tracks" => args.all_tracks = true,
            "--bench-out" => {
                args.bench_out = Some(it.next().ok_or("--bench-out takes a path")?.into())
            }
            "--meters-off" => args.meters_off = true,
            "--open-plugin-window" => args.open_plugin_window = true,
            "--on-top" => args.on_top = true,
            "--no-activate" => args.no_activate = true,
            "--x11" => args.x11 = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}\n\n{USAGE}")),
        }
    }
    args.on_top |= args.bench;
    Ok(args)
}

/// Frames egui-wgpu acquired no surface texture for, so never presented.
static SKIPPED_FRAMES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
enum UserEvent {
    AccessKit(egui_winit::accesskit_winit::Event),
}

impl From<egui_winit::accesskit_winit::Event> for UserEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        Self::AccessKit(event)
    }
}

struct MainWindow {
    window: Arc<Window>,
    state: egui_winit::State,
    painter: Painter,
    info: ViewportInfo,
}

struct PluginWindow {
    // Held for its Drop, which runs before the window that hosts it closes.
    _child: Option<child_view::ChildView>,
    window: Window,
}

/// How the pointer is held during a knob drag.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    None,
    /// The platform locks the pointer in place (macOS, Wayland).
    Locked,
    /// The platform only confines it (Windows, X11), so the host warps it
    /// back to where the drag started after every move.
    Confined(winit::dpi::PhysicalPosition<f64>),
}

struct Host {
    args: Args,
    ctx: egui::Context,
    app: App,
    proxy: EventLoopProxy<UserEvent>,
    main: Option<MainWindow>,
    plugin: Option<PluginWindow>,
    repaint_at: Option<Instant>,
    grab: Grab,
    last_pointer: Option<winit::dpi::PhysicalPosition<f64>>,
    last_frame_start: Option<Instant>,
    samples: Vec<FrameSample>,
    started: Instant,
    first_frame_logged: bool,
    bench_finished: bool,
    skipped_at_start: Option<usize>,
}

impl Host {
    fn create_main_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        // AccessKit's adapter must exist before the window is first shown.
        let level = if self.args.on_top {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        };
        let attributes = Window::default_attributes()
            .with_title("Beatlayer egui spike")
            .with_inner_size(LogicalSize::new(1600.0, 1000.0))
            .with_window_level(level)
            .with_visible(false);
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| e.to_string())?,
        );
        let mut state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        state.init_accesskit(event_loop, &window, self.proxy.clone());
        window.set_visible(true);

        let default_status = WgpuConfiguration::default().on_surface_status;
        let config = WgpuConfiguration {
            wgpu_setup: WgpuSetup::CreateNew(WgpuSetupCreateNew::from_display_handle(
                event_loop.owned_display_handle(),
            )),
            // Count frames that weren't presented (occluded, timed out, lost)
            // so the bench can't report skipped frames as fast ones.
            on_surface_status: Arc::new(move |status| {
                SKIPPED_FRAMES.fetch_add(1, Ordering::Relaxed);
                default_status(status)
            }),
            ..Default::default()
        };
        let mut painter = pollster::block_on(Painter::new(
            self.ctx.clone(),
            config,
            false,
            RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .map_err(|e| e.to_string())?;
        if let Some(side) = painter.max_texture_side() {
            state.set_max_texture_side(side);
        }
        let render_state = painter.render_state().ok_or("no wgpu render state")?;
        paint_gpu::GpuDense::install(&render_state, &self.app.scene);

        let info = render_state.adapter.get_info();
        let size = window.inner_size();
        println!(
            "window: {}x{} px at scale {} ({}x{} points); adapter {} ({:?})",
            size.width,
            size.height,
            window.scale_factor(),
            (f64::from(size.width) / window.scale_factor()).round(),
            (f64::from(size.height) / window.scale_factor()).round(),
            info.name,
            info.backend
        );
        println!("pid {}; {}", std::process::id(), native_window_id(&window));

        let mut info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &self.ctx, &window, true);
        self.main = Some(MainWindow {
            window,
            state,
            painter,
            info,
        });
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(main) = self.main.as_mut() else {
            return;
        };
        let start = Instant::now();
        let cpu_start = cputime::thread_cpu_time();

        egui_winit::update_viewport_info(&mut main.info, &self.ctx, &main.window, false);
        let mut input = main.state.take_egui_input(&main.window);
        input.viewports.insert(ViewportId::ROOT, main.info.clone());
        let app = &mut self.app;
        let output = self.ctx.run_ui(input, |ui| app.ui(ui));

        main.state.handle_platform_output_with_event_loop(
            &main.window,
            event_loop,
            output.platform_output,
        );
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut textures = output.textures_delta;
        main.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            [0.11, 0.11, 0.11, 1.0],
            &primitives,
            &mut textures,
            Vec::new(),
            &main.window,
        );

        let mut repaint_delay = Duration::MAX;
        if let Some(viewport) = output.viewport_output.get(&ViewportId::ROOT) {
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(
                &self.ctx,
                &mut main.info,
                viewport.commands.iter().cloned(),
                &main.window,
                &mut actions,
            );
            repaint_delay = viewport.repaint_delay;
        }

        let cpu = cputime::thread_cpu_time() - cpu_start;
        let wall = start.elapsed();
        if let Phase::Running(_) = self.app.bench_phase
            && self.skipped_at_start.is_none()
        {
            self.skipped_at_start = Some(SKIPPED_FRAMES.load(Ordering::Relaxed));
        }
        if let (Phase::Running(_), Some(previous)) = (self.app.bench_phase, self.last_frame_start) {
            self.samples.push(FrameSample {
                interval: (start - previous).as_secs_f64() * 1000.0,
                cpu: cpu.as_secs_f64() * 1000.0,
                wall: wall.as_secs_f64() * 1000.0,
            });
        }
        self.last_frame_start = Some(start);
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            println!(
                "first frame after {:.0} ms",
                self.started.elapsed().as_secs_f64() * 1000.0
            );
        }

        if repaint_delay.is_zero() {
            main.window.request_redraw();
            self.repaint_at = None;
        } else {
            self.repaint_at = Instant::now().checked_add(repaint_delay);
        }

        self.apply_pointer_lock();
        if self.app.plugin_window_requested {
            self.app.plugin_window_requested = false;
            self.open_plugin_window(event_loop);
        }
        if self.app.bench_phase == Phase::Done && !self.bench_finished {
            self.bench_finished = true;
            self.finish_bench();
            event_loop.exit();
        }
    }

    /// Locks (or confines) and releases the pointer for knob drags.
    fn apply_pointer_lock(&mut self) {
        let Some(request) = self.app.pointer_lock_request.take() else {
            return;
        };
        let Some(main) = &self.main else {
            return;
        };
        match request {
            PointerLock::Lock => {
                if main.window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
                    self.grab = Grab::Locked;
                    self.app.pointer_lock_status = "pointer locked".into();
                } else if let (Ok(()), Some(at)) = (
                    main.window.set_cursor_grab(CursorGrabMode::Confined),
                    self.last_pointer,
                ) {
                    self.grab = Grab::Confined(at);
                    self.app.pointer_lock_status = "pointer confined and re-centred".into();
                } else {
                    self.app.pointer_lock_status = "pointer lock unavailable".into();
                }
                println!("knob drag: {}", self.app.pointer_lock_status);
            }
            PointerLock::Unlock => {
                let _ = main.window.set_cursor_grab(CursorGrabMode::None);
                self.grab = Grab::None;
                self.app.pointer_lock_status = "pointer released".into();
            }
        }
    }

    fn open_plugin_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.plugin.is_some() {
            return;
        }
        // Created from inside the running loop: winit 0.30 hands every
        // ApplicationHandler callback an ActiveEventLoop, which is all that's
        // needed. (eframe's `App` never sees one; eframe 0.36's
        // `create_native` would let a host wrap it and do the same.)
        let attributes = Window::default_attributes()
            .with_title("Plug-in editor (stand-in)")
            .with_inner_size(LogicalSize::new(520.0, 360.0))
            .with_resizable(false);
        let window = match event_loop.create_window(attributes) {
            Ok(w) => w,
            Err(e) => {
                self.app.plugin_window_status = format!("plug-in window failed: {e}");
                return;
            }
        };
        let frame = child_view::Frame {
            x: 20.0,
            y: 20.0,
            width: 480.0,
            height: 320.0,
        };
        let child = match child_view::embed(&window, window.scale_factor(), frame) {
            Ok(child) => {
                println!("plug-in window: {}", child.description);
                println!("plug-in window: {}", native_window_id(&window));
                self.app.plugin_window_status = "child view embedded".into();
                Some(child)
            }
            Err(e) => {
                println!("plug-in window: embedding failed: {e}");
                self.app.plugin_window_status = format!("embedding failed: {e}");
                None
            }
        };
        self.app.plugin_window_open = true;
        self.plugin = Some(PluginWindow {
            _child: child,
            window,
        });
    }

    fn finish_bench(&mut self) {
        let summary = bench::summarise(&self.samples);
        let script = self.app.bench.as_ref().expect("bench is running");
        println!(
            "bench: dense={:?} all_tracks={} duration={}s",
            self.app.mode, script.all_tracks, script.duration
        );
        println!("{summary}");
        let skipped =
            SKIPPED_FRAMES.load(Ordering::Relaxed) - self.skipped_at_start.unwrap_or_default();
        println!("frames not presented   {skipped}");
        println!("load average           {}", load_average());
        if skipped > 0 {
            println!(
                "WARNING: the window was occluded or lost its surface; these numbers are not valid"
            );
        }
        // Any display paces frames at 4 ms or slower; much faster means
        // nothing waited for vsync (the display slept or the screen locked).
        if summary.interval_median < 4.0 {
            println!(
                "WARNING: frames weren't paced by the display (asleep or locked?); \
                 these numbers are not valid"
            );
        }
        if let Some(path) = &self.args.bench_out {
            match bench::write_csv(path, &self.samples) {
                Ok(()) => println!("frames written to {}", path.display()),
                Err(e) => eprintln!("couldn't write {}: {e}", path.display()),
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.main.is_some() {
            return;
        }
        if let Err(e) = self.create_main_window(event_loop) {
            eprintln!("couldn't create the main window: {e}");
            event_loop.exit();
            return;
        }
        if self.args.open_plugin_window {
            self.app.plugin_window_requested = true;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.plugin.as_ref().is_some_and(|p| p.window.id() == id) {
            if matches!(event, WindowEvent::CloseRequested) {
                self.plugin = None;
                self.app.plugin_window_open = false;
                self.app.plugin_window_status = "plug-in window closed".into();
                if let Some(main) = &self.main {
                    main.window.request_redraw();
                }
            }
            return;
        }
        let Some(main) = self.main.as_mut() else {
            return;
        };
        if main.window.id() != id {
            return;
        }
        let response = main.state.on_window_event(&main.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                self.plugin = None;
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                if let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    main.painter.on_window_resized(ViewportId::ROOT, w, h);
                }
            }
            WindowEvent::Occluded(occluded) => {
                println!("main window occluded: {occluded}");
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.grab == Grab::None {
                    self.last_pointer = Some(position);
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            _ => {}
        }
        if response.repaint {
            main.window.request_redraw();
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let DeviceEvent::MouseMotion { delta } = event else {
            return;
        };
        let Some(main) = self.main.as_mut() else {
            return;
        };
        if main.state.on_mouse_motion(delta) && self.grab != Grab::None {
            if let Grab::Confined(at) = self.grab {
                let _ = main.window.set_cursor_position(at);
            }
            main.window.request_redraw();
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::AccessKit(event) = event;
        let Some(main) = self.main.as_mut() else {
            return;
        };
        use egui_winit::accesskit_winit::WindowEvent as Ak;
        match event.window_event {
            Ak::InitialTreeRequested => {
                println!("accessibility: initial tree requested; AccessKit enabled");
                self.ctx.enable_accesskit();
            }
            Ak::ActionRequested(request) => main.state.on_accesskit_action_request(request),
            Ak::AccessibilityDeactivated => self.ctx.disable_accesskit(),
        }
        main.window.request_redraw();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.repaint_at {
            Some(at) if at <= Instant::now() => {
                self.repaint_at = None;
                if let Some(main) = &self.main {
                    main.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// The 1, 5, and 15-minute load averages, to show how busy the machine was.
fn load_average() -> String {
    #[cfg(unix)]
    {
        let mut loads = [0.0f64; 3];
        // SAFETY: `loads` has room for the three values requested.
        let n = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
        if n == 3 {
            return format!("{:.2} {:.2} {:.2}", loads[0], loads[1], loads[2]);
        }
    }
    "unavailable".into()
}

/// The native id tools use to find a window (a CGWindowID on macOS).
fn native_window_id(window: &Window) -> String {
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = window.window_handle()
            && let RawWindowHandle::AppKit(h) = handle.as_raw()
        {
            // SAFETY: the handle's view belongs to this live window, and
            // winit calls us on the main thread.
            let view: &objc2_app_kit::NSView = unsafe { h.ns_view.cast().as_ref() };
            if let Some(w) = view.window() {
                return format!("window number {}", w.windowNumber());
            }
        }
        "window number unknown".into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        format!("window {:?}", window.id())
    }
}

fn main() {
    let started = Instant::now();
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    let scene = scene::Scene::generate(scene::Config::BENCH);
    println!(
        "scene: {} tracks, {} clips, {} peak bytes, {} files; fingerprint {:016x}; generated in {:.0} ms",
        scene.tracks.len(),
        scene.clips.len(),
        scene.peaks.len(),
        scene.files.len(),
        scene.fingerprint(),
        started.elapsed().as_secs_f64() * 1000.0
    );

    let ctx = egui::Context::default();
    match fonts::system_fonts() {
        Ok((fonts, path)) => {
            println!("font: {path}");
            ctx.set_fonts(fonts);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
    ctx.set_theme(egui::Theme::Dark);

    let mut app = App::new(scene, args.mode);
    app.meters_running = !args.meters_off;
    if args.bench {
        app.bench = Some(Script::new(args.bench_seconds, args.all_tracks));
    }

    let mut builder = EventLoop::<UserEvent>::with_user_event();
    // Activate like a normal launch; --no-activate leaves keyboard focus with
    // whatever the person at the Mac is using.
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS as _;
        builder.with_activate_ignoring_other_apps(!args.no_activate);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if args.x11 {
        use winit::platform::x11::EventLoopBuilderExtX11 as _;
        builder.with_x11();
    }
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            eprintln!("couldn't create the event loop: {e}");
            std::process::exit(1);
        }
    };
    let mut host = Host {
        args,
        ctx,
        app,
        proxy: event_loop.create_proxy(),
        main: None,
        plugin: None,
        repaint_at: None,
        grab: Grab::None,
        last_pointer: None,
        last_frame_start: None,
        samples: Vec::new(),
        started,
        first_frame_logged: false,
        bench_finished: false,
        skipped_at_start: None,
    };
    if let Err(e) = event_loop.run_app(&mut host) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}
