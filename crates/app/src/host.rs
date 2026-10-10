//! The window host: Beatlayer's own winit event loop, driving egui through
//! egui-winit and egui-wgpu (ADR-001), with the main window and the
//! Settings window, native menus, and file dialogs.
//!
//! Every key press, menu choice, and click becomes a command invocation on
//! the shell ([`Shell::invoke`]); what only the platform can do comes back
//! as effects, which the host carries out here.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_shell::audio::AudioHost;
use bl_shell::files::NoSetFiles;
use bl_shell::menus::{Menu, MenuPlatform, menu_bar};
use bl_shell::settings::{Preferences, ThemeChoice, default_config_dir};
use bl_shell::{Effect, Invocation, Shell, WindowKind, commands as ids};
use bl_ui::input::KeyDispatcher;
use bl_ui::keymap::{Keymap, Platform};
use bl_ui::layout::{MINIMUM, SETTINGS_WINDOW, TITLE_BAR};
use bl_ui::main_window::{DrawnMenuBar, MainWindowUi, UiEvent};
use bl_ui::settings_window::SettingsUi;
use bl_ui::theme::{Theme, fonts};
use egui::{ViewportId, ViewportInfo};
use egui_wgpu::winit::Painter;
use egui_wgpu::{
    RendererOptions, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew, WgpuSetupExisting,
};
use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

use crate::Args;

/// How often the CPU meter reads the engine's load.
const CPU_METER_PERIOD: Duration = Duration::from_millis(500);

/// Events sent into the loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// From AccessKit: a screen reader asked for the tree or an action.
    AccessKit(egui_winit::accesskit_winit::Event),
    /// A native menu item was chosen.
    #[cfg(any(target_os = "macos", windows))]
    Menu(muda::MenuEvent),
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
}

struct Host {
    args: Args,
    shell: Shell,
    theme_choice: ThemeChoice,
    theme: Theme,
    zoom: f32,
    keymap: Keymap,
    platform: Platform,
    menus: Vec<Menu>,
    proxy: EventLoopProxy<UserEvent>,
    main: Option<Surface>,
    settings: Option<Surface>,
    main_ui: MainWindowUi,
    settings_ui: SettingsUi,
    main_keys: KeyDispatcher,
    settings_keys: KeyDispatcher,
    #[cfg(any(target_os = "macos", windows))]
    native_menu: Option<crate::native_menu::NativeMenu>,
    shown_revision: Option<u64>,
    next_cpu_sample: Instant,
    quitting: bool,
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
    let mut shell = Shell::new(prefs, config_dir, audio, Box::new(NoSetFiles));
    shell.start_audio();
    if let Some(error) = prefs_error {
        shell.set_message(error, true);
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
    #[cfg(any(target_os = "macos", windows))]
    {
        let menu_proxy = proxy.clone();
        muda::MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_proxy.send_event(UserEvent::Menu(event));
        }));
    }

    let theme_choice = shell.theme();
    let platform = Platform::current();
    let menus = menu_bar(MenuPlatform::current());
    let mut host = Host {
        zoom: shell.zoom(),
        args,
        shell,
        theme_choice,
        theme: Theme::builtin(theme_choice),
        keymap: Keymap::live12(),
        platform,
        menus,
        proxy,
        main: None,
        settings: None,
        main_ui: MainWindowUi::new(),
        settings_ui: SettingsUi::new(),
        main_keys: KeyDispatcher::new(platform),
        settings_keys: KeyDispatcher::new(platform),
        #[cfg(any(target_os = "macos", windows))]
        native_menu: None,
        shown_revision: None,
        next_cpu_sample: Instant::now() + CPU_METER_PERIOD,
        quitting: false,
    };
    event_loop
        .run_app(&mut host)
        .map_err(|e| format!("the event loop failed: {e}"))?;
    host.shell.audio_mut().stop();
    Ok(())
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
        ctx.set_zoom_factor(self.zoom);
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
        })
    }

    fn open_main_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        // The window opens at its last size; the first time, at the
        // reference size Live is measured at (1600 x 1000 points with the
        // title bar), shrunk to fit a smaller screen.
        let (outer, inner) = match (self.args.window_size, self.shell.preferences().window_size) {
            (Some((w, h)), _) => (Some((w, h)), (w, h - TITLE_BAR)),
            (None, Some((w, h))) => (None, (w, h)),
            (None, None) => (Some((1600.0, 1000.0)), (1600.0, 1000.0 - TITLE_BAR)),
        };
        let (mut width, mut height) = inner;
        if let Some(monitor) = event_loop.primary_monitor() {
            let size = monitor.size().to_logical::<f32>(monitor.scale_factor());
            width = width.min(size.width * 0.95);
            height = height.min(size.height * 0.9);
        }
        let attributes = Window::default_attributes()
            .with_title(self.shell.set_name())
            .with_inner_size(LogicalSize::new(width, height))
            .with_min_inner_size(LogicalSize::new(MINIMUM.x, MINIMUM.y));
        let surface = self.create_surface(event_loop, attributes, None)?;
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
            let wanted = (h - decoration).min(height);
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

    fn open_settings_window(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(settings) = &self.settings {
            settings.window.focus_window();
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Settings")
            .with_inner_size(LogicalSize::new(
                SETTINGS_WINDOW.x,
                SETTINGS_WINDOW.y - TITLE_BAR,
            ))
            .with_resizable(false);
        match self.create_surface(event_loop, attributes, self.main.as_ref()) {
            Ok(surface) => {
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
        }
    }

    fn kind_of(&self, id: WindowId) -> Option<WindowKind> {
        if self.main.as_ref().is_some_and(|s| s.window.id() == id) {
            Some(WindowKind::Main)
        } else if self.settings.as_ref().is_some_and(|s| s.window.id() == id) {
            Some(WindowKind::Settings)
        } else {
            None
        }
    }

    fn redraw(&mut self, kind: WindowKind, event_loop: &ActiveEventLoop) {
        let Some(mut surface) = self.surface(kind).take() else {
            return;
        };
        egui_winit::update_viewport_info(&mut surface.info, &surface.ctx, &surface.window, false);
        let mut input = surface.state.take_egui_input(&surface.window);
        input
            .viewports
            .insert(ViewportId::ROOT, surface.info.clone());

        let text_focused = surface.ctx.text_edit_focused();
        let keys = match kind {
            WindowKind::Main => &mut self.main_keys,
            WindowKind::Settings => &mut self.settings_keys,
        };
        let invocations = keys.take_bound_keys(&mut input, &self.keymap, &self.shell, text_focused);
        if kind == WindowKind::Settings {
            self.settings_ui.prepare(&mut self.shell);
        }

        let drawn = (kind == WindowKind::Main && self.drawn_menus()).then_some(DrawnMenuBar {
            menus: &self.menus,
            keymap: &self.keymap,
            platform: self.platform,
        });
        let shell = &self.shell;
        let theme = &self.theme;
        let main_ui = &mut self.main_ui;
        let settings_ui = &mut self.settings_ui;
        let mut ui_events = Vec::new();
        let output = surface.ctx.run_ui(input, |ui| {
            ui_events = match kind {
                WindowKind::Main => main_ui.show(ui, shell, theme, drawn),
                WindowKind::Settings => settings_ui.show(ui, shell, theme),
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
            }
        }
        self.after_commands(event_loop);
    }

    fn invoke(&mut self, invocation: &Invocation) {
        // Errors show in the Status Bar; the shell has put them there.
        let _ = self.shell.invoke(&invocation.id, invocation.args.clone());
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
        if self.shell.theme() != self.theme_choice || (self.shell.zoom() - self.zoom).abs() > 1e-6 {
            self.theme_choice = self.shell.theme();
            self.theme = Theme::builtin(self.theme_choice);
            self.zoom = self.shell.zoom();
            for surface in [&self.main, &self.settings].into_iter().flatten() {
                self.theme.apply(&surface.ctx);
                surface.ctx.set_zoom_factor(self.zoom);
            }
        }
        if self.shown_revision != Some(self.shell.revision()) {
            self.shown_revision = Some(self.shell.revision());
            #[cfg(any(target_os = "macos", windows))]
            if let Some(menu) = &self.native_menu {
                menu.refresh(&self.shell, &self.keymap, self.platform);
            }
            for surface in [&self.main, &self.settings].into_iter().flatten() {
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
            Effect::PickOpenPath => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Open Set")
                    .add_filter("Beatlayer Set", &["blset"]);
                if let Some(main) = &self.main {
                    dialog = dialog.set_parent(main.window.as_ref());
                }
                if let Some(path) = dialog.pick_file() {
                    self.invoke(&Invocation::new(
                        ids::SET_OPEN,
                        json!({ "path": path.to_string_lossy() }),
                    ));
                }
            }
            Effect::PickSavePath { then } => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save Set As")
                    .add_filter("Beatlayer Set", &["blset"])
                    .set_file_name(format!("{}.blset", self.shell.set_name()));
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
            },
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
            .set_title("Save Set As")
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
            response.repaint
        };
        match event {
            WindowEvent::CloseRequested => {
                match kind {
                    WindowKind::Main => self.invoke(&Invocation::new(ids::APP_QUIT, json!({}))),
                    WindowKind::Settings => self.shell.close_settings(),
                }
                self.after_commands(event_loop);
            }
            WindowEvent::Focused(true) => {
                self.shell.set_focused_window(kind);
                if kind == WindowKind::Main {
                    self.shell.set_focused_area(self.shell.focused_area());
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
        for surface in [&mut self.main, &mut self.settings].into_iter().flatten() {
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
