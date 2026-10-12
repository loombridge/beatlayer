//! `--screenshot FILE`: draws the main window (or, with `--settings`, the
//! Settings window) offscreen at its size in points and twice that in
//! pixels, as on a Retina screen, and saves it as a PNG. No window opens,
//! so a capture never takes the screen or the keyboard from anyone. The
//! side-by-side comparison with Live uses it, and it renders exactly what
//! the window would, at the interface zoom in the preferences or set with
//! `--command view.set_zoom='{"percent":150}'` (WP-1.17): the window is
//! then no smaller than its minimum at that zoom.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, commands as ids};
use bl_ui::layout::{TITLE_BAR, apply_zoom, minimum_window, settings_window};
use bl_ui::main_window::{MainWindowUi, UiEvent};
use bl_ui::settings_window::SettingsUi;
use bl_ui::theme::{Theme, fonts};
use egui::{ColorImage, Pos2, RawInput, Rect, ViewportId, vec2};
use egui_wgpu::wgpu;
use serde_json::json;

use crate::Args;

/// The pixels per point of the capture, as on a Retina screen.
const SCALE: f32 = 2.0;

/// Draws a window offscreen and writes it to the file `--screenshot` names.
pub fn run(args: &Args, path: &Path) -> Result<(), String> {
    let config_dir = args.config_dir.clone();
    let prefs = match config_dir.as_deref().map(Preferences::load) {
        Some(Ok(prefs)) => prefs,
        _ => Preferences::default(),
    };
    let audio = if args.null_audio {
        AudioHost::null(prefs.audio.clone())
    } else {
        AudioHost::new(prefs.audio.clone())
    };
    let mut main = MainWindowUi::new();
    let mut shell = if args.benchmark_scene {
        // The Arrangement benchmark scene (WP-1.8a), with its peaks.
        let scene = bl_ui::arrangement::benchmark::scene(200, 25, 64);
        main.arrangement_mut()
            .set_peak_source(Box::new(scene.peaks), None);
        bl_ui::arrangement::benchmark::shell_with(scene.editor)
    } else {
        Shell::new(prefs, None, audio, Box::new(ProjectFiles::new()))
    };
    shell.start_audio();
    if !args.benchmark_scene {
        // The browser's User Library: the folder `--user-library` names, or
        // where it would be, without making it (a capture changes nothing
        // on disk).
        shell.set_user_library(
            args.user_library
                .clone()
                .or_else(|| prefs_library(args))
                .or_else(bl_shell::browser::places::default_user_library),
        );
    }
    if let Some(set) = &args.set {
        shell
            .invoke(ids::SET_OPEN, json!({ "path": set.to_string_lossy() }))
            .map_err(|e| e.to_string())?;
    }
    let settings = args.open_settings;
    if settings {
        let _ = shell.invoke(ids::APP_SETTINGS, json!({}));
        let _ = shell.take_effects();
    }
    // The content area: on macOS the whole window, with Live's title bar
    // drawn in it (D22), as the app draws it; elsewhere the window's size
    // less the title bar, which layout.md's measurements include and the
    // system draws there.
    let title_bar = cfg!(target_os = "macos");
    let below = if title_bar { 0.0 } else { TITLE_BAR };
    // The interface zoom, from the preferences or a `--command`: it changes
    // the window's size only where the window would be under its minimum.
    let mut zoom = shell.zoom();
    let window_size = |zoom: f32| {
        if settings {
            settings_window(zoom, title_bar)
        } else {
            let (w, h) = args.window_size.unwrap_or((1600.0, 1000.0));
            vec2(w, h - below).max(minimum_window(zoom, title_bar))
        }
    };
    let mut size = window_size(zoom);

    let ctx = egui::Context::default();
    ctx.set_fonts(fonts());
    let theme = Theme::builtin(shell.theme());
    theme.apply(&ctx);
    let mut settings_ui = SettingsUi::new();
    let mut output: Option<egui::FullOutput> = None;
    // Every frame's texture changes, in order: the first frame uploads the
    // font atlas.
    let mut textures = egui::TexturesDelta::default();
    // Let the audio meter take a reading, and fonts and layout settle.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let _ = shell.audio_mut().sample_cpu_load();
    // The browser's index and any preview finish in the background.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while shell.browser_view().index_state().indexing && std::time::Instant::now() < deadline {
        shell.poll();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    shell.poll();
    for frame in 0..4 {
        settings_ui.prepare(&mut shell);
        if frame == 1 && !args.commands.is_empty() {
            // Let a command that starts or moves the transport take effect
            // and play a moment, so the capture shows it.
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        // Each frame draws the playhead the engine last published, as the
        // window does.
        shell.poll_engine();
        // Each layout point is `zoom` points of the window, and each of
        // those SCALE pixels.
        if (shell.zoom() - zoom).abs() > 1e-6 {
            zoom = shell.zoom();
            size = window_size(zoom);
        }
        apply_zoom(&ctx, zoom);
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size / zoom)),
            time: Some(f64::from(frame) / 60.0),
            ..Default::default()
        };
        input
            .viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(SCALE);
        let shell_ref = &shell;
        let mut events = Vec::new();
        let mut frame_output = ctx.run_ui(input, |ui| {
            events = if settings {
                settings_ui.show(ui, shell_ref, &theme, title_bar)
            } else {
                main.show(ui, shell_ref, &theme, title_bar, None)
            };
        });
        // The lanes report their size on the first frame; commands asked
        // for on the command line run once it's known.
        for event in events {
            match event {
                UiEvent::ArrangementViewport { width, height } => {
                    shell.set_arrangement_viewport(width, height);
                }
                UiEvent::Invoke(_)
                | UiEvent::FocusArea(_)
                | UiEvent::FocusTrack(_)
                | UiEvent::FocusValue(_)
                | UiEvent::BeginGesture(_)
                | UiEvent::BeginNamedGesture(_)
                | UiEvent::EndGesture
                | UiEvent::Gesture { .. } => {}
            }
        }
        if frame == 0 {
            for (id, args) in &args.commands {
                shell
                    .invoke(id, args.clone())
                    .map_err(|e| format!("--command {id}: {e}"))?;
            }
            // A preview the commands started is decoded in the background.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while shell.browser_preview().loading.is_some() && std::time::Instant::now() < deadline
            {
                shell.poll();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        shell.poll();
        textures.append(std::mem::take(&mut frame_output.textures_delta));
        output = Some(frame_output);
    }
    let output = output.ok_or("nothing was drawn")?;
    let image = render(&ctx, output, textures, size, &theme)?;
    write_png(path, &image)?;
    println!(
        "screenshot: {} ({} x {} pixels, {} x {} points, zoom {}%)",
        path.display(),
        image.size[0],
        image.size[1],
        size.x,
        size.y,
        shell.zoom_percent()
    );
    shell.audio_mut().stop();
    Ok(())
}

/// The User Library the preferences name, if any.
fn prefs_library(args: &Args) -> Option<std::path::PathBuf> {
    args.config_dir
        .as_deref()
        .and_then(|dir| Preferences::load(dir).ok())
        .and_then(|prefs| prefs.browser.user_library)
}

/// Renders egui's output into an image on the GPU, without a surface. The
/// view captures' test renders through it too (WP-1.16).
pub(crate) fn render(
    ctx: &egui::Context,
    mut output: egui::FullOutput,
    mut textures: egui::TexturesDelta,
    size: egui::Vec2,
    theme: &Theme,
) -> Result<ColorImage, String> {
    // The textures to upload, taken out first: epaint panics if a delta is
    // dropped unapplied, and every early return (no GPU adapter, as on CI's
    // Linux machines) would drop one.
    textures.append(std::mem::take(&mut output.textures_delta));
    let uploads = std::mem::take(&mut textures.set);
    textures.clear();
    let width = (size.x * SCALE).round() as u32;
    let height = (size.y * SCALE).round() as u32;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|e| format!("no GPU adapter: {e}"))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("no GPU device: {e}"))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer =
        egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
    // The Arrangement View's own pipelines (WP-1.8a).
    bl_ui::arrangement::gpu::install(&device, format, &mut renderer.callback_resources);
    for (id, deltas) in &uploads {
        for delta in deltas {
            renderer.update_texture(&device, &queue, *id, delta);
        }
    }
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    crate::capture::render_frame(
        &device,
        &queue,
        &mut renderer,
        format,
        &primitives,
        [width, height],
        output.pixels_per_point,
        egui::Rgba::from(theme.colors.window.0),
    )
}

/// Writes an egui image as an RGBA PNG.
pub fn write_png(path: &Path, image: &ColorImage) -> Result<(), String> {
    let file =
        File::create(path).map_err(|e| format!("couldn't create {}: {e}", path.display()))?;
    let [width, height] = image.size;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        u32::try_from(width).map_err(|e| e.to_string())?,
        u32::try_from(height).map_err(|e| e.to_string())?,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    let bytes: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|p| p.to_srgba_unmultiplied())
        .collect();
    writer
        .write_image_data(&bytes)
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))
}
