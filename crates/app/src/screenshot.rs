//! `--screenshot FILE`: draws the main window (or, with `--settings`, the
//! Settings window) offscreen at its size in points and twice that in
//! pixels, as on a Retina screen, and saves it as a PNG. No window opens,
//! so a capture never takes the screen or the keyboard from anyone. The
//! side-by-side comparison with Live uses it, and it renders exactly what
//! the window would.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, commands as ids};
use bl_ui::layout::{SETTINGS_WINDOW, TITLE_BAR};
use bl_ui::main_window::MainWindowUi;
use bl_ui::settings_window::SettingsUi;
use bl_ui::theme::{Theme, fonts};
use egui::{ColorImage, Pos2, RawInput, Rect, vec2};
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
    let mut shell = Shell::new(prefs, None, audio, Box::new(ProjectFiles::new()));
    shell.start_audio();
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
    let size = if settings {
        SETTINGS_WINDOW - vec2(0.0, below)
    } else {
        let (w, h) = args.window_size.unwrap_or((1600.0, 1000.0));
        vec2(w, h - below)
    };

    let ctx = egui::Context::default();
    ctx.set_fonts(fonts());
    let theme = Theme::builtin(shell.theme());
    theme.apply(&ctx);
    let mut main = MainWindowUi::new();
    let mut settings_ui = SettingsUi::new();
    let mut output: Option<egui::FullOutput> = None;
    // Every frame's texture changes, in order: the first frame uploads the
    // font atlas.
    let mut textures = egui::TexturesDelta::default();
    // Let the audio meter take a reading, and fonts and layout settle.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let _ = shell.audio_mut().sample_cpu_load();
    for frame in 0..4 {
        settings_ui.prepare(&mut shell);
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(f64::from(frame) / 60.0),
            ..Default::default()
        };
        ctx.set_pixels_per_point(SCALE);
        let shell = &shell;
        let mut frame_output = ctx.run_ui(input, |ui| {
            let _ = if settings {
                settings_ui.show(ui, shell, &theme, title_bar)
            } else {
                main.show(ui, shell, &theme, title_bar, None)
            };
        });
        textures.append(std::mem::take(&mut frame_output.textures_delta));
        output = Some(frame_output);
    }
    let output = output.ok_or("nothing was drawn")?;
    let image = render(&ctx, output, textures, size, &theme)?;
    write_png(path, &image)?;
    println!(
        "screenshot: {} ({} x {} pixels, {} x {} points)",
        path.display(),
        image.size[0],
        image.size[1],
        size.x,
        size.y
    );
    shell.audio_mut().stop();
    Ok(())
}

/// Renders egui's output into an image on the GPU, without a surface.
fn render(
    ctx: &egui::Context,
    output: egui::FullOutput,
    mut textures: egui::TexturesDelta,
    size: egui::Vec2,
    theme: &Theme,
) -> Result<ColorImage, String> {
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
    for (id, deltas) in &textures.set {
        for delta in deltas {
            renderer.update_texture(&device, &queue, *id, delta);
        }
    }
    textures.clear();
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: output.pixels_per_point,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let mut commands = renderer.update_buffers(&device, &queue, &mut encoder, &primitives, &screen);
    let clear = egui::Rgba::from(theme.colors.window.0);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("screenshot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(clear.r()),
                        g: f64::from(clear.g()),
                        b: f64::from(clear.b()),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        let mut pass = pass.forget_lifetime();
        renderer.render(&mut pass, &primitives, &screen);
    }
    let padded = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    commands.push(encoder.finish());
    queue.submit(commands);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("the GPU didn't finish: {e}"))?;
    let data = slice
        .get_mapped_range()
        .map_err(|e| format!("couldn't read the image back: {e}"))?;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for row in data.chunks(padded as usize) {
        for rgba in row[..(width * 4) as usize].chunks(4) {
            pixels.push(egui::Color32::from_rgba_premultiplied(
                rgba[0], rgba[1], rgba[2], rgba[3],
            ));
        }
    }
    Ok(ColorImage::new([width as usize, height as usize], pixels))
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
