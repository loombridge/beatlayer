//! View captures for the agent API (WP-1.16): an agent's `capture_view`
//! call gets the view as the person sees it, as a PNG image.
//!
//! The host takes each request from the agent API
//! ([`Agent::take_capture_requests`](bl_agent::Agent::take_capture_requests))
//! and checks the view is on screen ([`window_of`]). Right after that
//! window's next frame, the host draws the same frame again into an
//! offscreen texture with the window's own renderer and reads it back
//! ([`render_frame`]), so the capture is exactly what the window drew,
//! whether or not the system presented it, and notes where the view is in
//! it ([`region`]). Cropping, scaling, and encoding ([`encode`]) run on a
//! thread of their own, so the window's thread does no more than draw one
//! frame twice and wait for the GPU to finish it; the audio thread isn't
//! touched at all. The image goes back to the agent API with
//! [`Agent::finish_capture`](bl_agent::Agent::finish_capture).

use bl_agent::Captured;
use bl_shell::perception_commands::View;
use bl_shell::{MainView, Shell, WindowKind, commands as ids};
use bl_ui::main_window::MainWindowUi;
use egui::{ColorImage, Rect};
use egui_wgpu::wgpu;

/// Which window shows `view`, if it's on screen now: the Settings window
/// for `settings`, and the main window for the rest. Otherwise, why not,
/// and the command that shows it.
///
/// # Errors
///
/// A message for the agent naming the view and how to show it.
pub fn window_of(view: View, shell: &Shell, settings_open: bool) -> Result<WindowKind, String> {
    let front = shell.main_view();
    let hidden = |what: &str, command: &str| {
        Err(format!(
            "the {what} isn't showing, so it can't be captured; `{command}` shows it, or \
             capture `window` to see what the person sees"
        ))
    };
    match view {
        View::Window | View::Detail => Ok(WindowKind::Main),
        View::Arrangement if front != MainView::Arrangement => hidden(
            "Arrangement View (the Session View is in front)",
            ids::VIEW_TOGGLE_SESSION_ARRANGEMENT,
        ),
        View::Session if front != MainView::Session => hidden(
            "Session View (the Arrangement View is in front)",
            ids::VIEW_TOGGLE_SESSION_ARRANGEMENT,
        ),
        View::Arrangement | View::Session => Ok(WindowKind::Main),
        View::Browser if !shell.browser_visible() => hidden("browser", ids::VIEW_TOGGLE_BROWSER),
        View::Browser => Ok(WindowKind::Main),
        View::Mixer if !shell.mixer_view().of(shell.front_mixer()).shown => {
            hidden("mixer", bl_shell::mixer_view_commands::TOGGLE)
        }
        View::Mixer => Ok(WindowKind::Main),
        View::Settings if !settings_open => hidden("Settings window", ids::APP_SETTINGS),
        View::Settings => Ok(WindowKind::Settings),
    }
}

/// Where `view` is in its window's last frame, in points: `None` for the
/// whole window.
///
/// # Errors
///
/// A message if the main window hasn't drawn the view.
pub fn region(view: View, shell: &Shell, main: &MainWindowUi) -> Result<Option<Rect>, String> {
    if matches!(view, View::Window | View::Settings) {
        return Ok(None);
    }
    let layout = main
        .drawn_layout()
        .ok_or("the main window hasn't been drawn yet")?;
    let rect = match view {
        View::Arrangement | View::Session => Some(layout.main_view),
        View::Browser => layout.browser,
        View::Detail => Some(layout.detail),
        View::Mixer => match shell.main_view() {
            MainView::Arrangement => main.arrangement().mixer_rect(),
            MainView::Session => Some(layout.main_view),
        },
        View::Window | View::Settings => None,
    };
    rect.map(Some)
        .ok_or_else(|| format!("the {} isn't drawn in the window", view.id()))
}

/// Draws a frame's tessellated `primitives` into an offscreen texture of
/// `size` pixels with `renderer` (whose textures hold the frame's), and
/// reads it back. `--screenshot` renders through it, and so do view
/// captures, with the window's own renderer right after the window's frame,
/// so a capture doesn't depend on the system presenting that frame (macOS
/// skips presenting a covered window's). The readback waits for the GPU to
/// finish the one frame.
///
/// # Errors
///
/// A message if the GPU fails or the format can't be read back.
#[expect(clippy::too_many_arguments, reason = "a frame's whole description")]
pub fn render_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
    primitives: &[egui::ClippedPrimitive],
    size: [u32; 2],
    pixels_per_point: f32,
    clear: egui::Rgba,
) -> Result<ColorImage, String> {
    let [width, height] = size;
    let order = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => [0, 1, 2, 3],
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => [2, 1, 0, 3],
        other => return Err(format!("frames in {other:?} can't be read back")),
    };
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture"),
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
    let mut commands = renderer.update_buffers(device, queue, &mut encoder, primitives, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("capture"),
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
        renderer.render(&mut pass, primitives, &screen);
    }
    let padded = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture"),
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
        for p in row[..(width * 4) as usize].chunks(4) {
            pixels.push(egui::Color32::from_rgba_premultiplied(
                p[order[0]],
                p[order[1]],
                p[order[2]],
                p[order[3]],
            ));
        }
    }
    Ok(ColorImage::new([width as usize, height as usize], pixels))
}

/// Crops `image` (the window's frame, in pixels) to `region` (points, at
/// `pixels_per_point`), scales it down to fit `max_size` pixels on its
/// longer side, averaging each output pixel's area, and encodes it as a
/// PNG file.
///
/// # Errors
///
/// A message if the region is outside the frame or the PNG can't be
/// written.
pub fn encode(
    image: &ColorImage,
    region: Option<Rect>,
    pixels_per_point: f32,
    max_size: u32,
) -> Result<Captured, String> {
    let [width, height] = image.size;
    let (x0, y0, x1, y1) = match region {
        None => (0, 0, width, height),
        Some(rect) => {
            let px = |v: f32| (v * pixels_per_point).round().max(0.0) as usize;
            (
                px(rect.min.x).min(width),
                px(rect.min.y).min(height),
                px(rect.max.x).min(width),
                px(rect.max.y).min(height),
            )
        }
    };
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if w == 0 || h == 0 {
        return Err("the view is outside the window's frame".to_owned());
    }
    let scale = (f64::from(max_size) / w.max(h) as f64).min(1.0);
    let out_w = ((w as f64 * scale).round() as usize).max(1);
    let out_h = ((h as f64 * scale).round() as usize).max(1);
    let mut rgba = Vec::with_capacity(out_w * out_h * 4);
    for oy in 0..out_h {
        let sy0 = y0 + oy * h / out_h;
        let sy1 = (y0 + (oy + 1) * h / out_h).max(sy0 + 1);
        for ox in 0..out_w {
            let sx0 = x0 + ox * w / out_w;
            let sx1 = (x0 + (ox + 1) * w / out_w).max(sx0 + 1);
            let mut sum = [0u32; 4];
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    let pixel = image.pixels[y * width + x].to_srgba_unmultiplied();
                    for (s, p) in sum.iter_mut().zip(pixel) {
                        *s += u32::from(p);
                    }
                }
            }
            let count = ((sy1 - sy0) * (sx1 - sx0)) as u32;
            rgba.extend(sum.iter().map(|s| ((s + count / 2) / count) as u8));
        }
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, out_w as u32, out_h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("couldn't encode the image: {e}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| format!("couldn't encode the image: {e}"))?;
    }
    Ok(Captured {
        png,
        width: out_w as u32,
        height: out_h as u32,
        pixels_per_point: f64::from(pixels_per_point) * scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_shell::perception_commands::View;
    use bl_ui::theme::{Theme, fonts};
    use egui::{Color32, Pos2, RawInput, pos2, vec2};

    /// Reads a PNG back: its size and RGBA bytes.
    fn decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        buffer.truncate(info.buffer_size());
        (info.width, info.height, buffer)
    }

    #[test]
    fn a_region_is_cropped_scaled_to_fit_and_encoded() {
        // Spec: agent-perception R8. A 400 x 200 frame at 2 pixels a
        // point: red on the left half, blue on the right.
        let pixels: Vec<Color32> = (0..200)
            .flat_map(|_| (0..400).map(|x| if x < 200 { Color32::RED } else { Color32::BLUE }))
            .collect();
        let image = ColorImage::new([400, 200], pixels);
        // The right half, 100 x 100 points.
        let right = Rect::from_min_size(pos2(100.0, 0.0), vec2(100.0, 100.0));
        let captured = encode(&image, Some(right), 2.0, 1568).unwrap();
        assert_eq!((captured.width, captured.height), (200, 200));
        let (w, h, rgba) = decode(&captured.png);
        assert_eq!((w, h), (200, 200));
        assert!(rgba.chunks(4).all(|p| p == [0, 0, 255, 255]));
        // The whole frame, scaled to fit 100 pixels: 100 x 50, red then
        // blue, half a pixel a point.
        let small = encode(&image, None, 2.0, 100).unwrap();
        assert_eq!((small.width, small.height), (100, 50));
        assert!((small.pixels_per_point - 0.5).abs() < 1e-9);
        let (_, _, rgba) = decode(&small.png);
        assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[rgba.len() - 4..], &[0, 0, 255, 255]);
        // A region outside the frame is refused.
        let outside = Rect::from_min_size(pos2(500.0, 500.0), vec2(10.0, 10.0));
        assert!(encode(&image, Some(outside), 2.0, 1568).is_err());
    }

    /// The main window drawn once, as the app draws it, without a GPU.
    fn drawn(shell: &Shell) -> MainWindowUi {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts());
        let theme = Theme::builtin(shell.theme());
        theme.apply(&ctx);
        let mut main = MainWindowUi::new();
        for frame in 0..2 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 977.0))),
                time: Some(f64::from(frame) / 60.0),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                main.show(ui, shell, &theme, false, None);
            });
            // Nothing draws it: no GPU here.
            output.textures_delta.clear();
        }
        main
    }

    #[test]
    fn views_on_screen_are_found_and_hidden_ones_say_how_to_show_them() {
        // Spec: agent-perception R8.
        let mut shell = Shell::headless();
        let main = drawn(&shell);
        for view in [View::Window, View::Arrangement, View::Browser, View::Detail] {
            assert_eq!(
                window_of(view, &shell, false),
                Ok(WindowKind::Main),
                "{view:?}"
            );
            let rect = region(view, &shell, &main).unwrap();
            assert_eq!(rect.is_none(), view == View::Window, "{view:?}");
        }
        let session = window_of(View::Session, &shell, false).unwrap_err();
        assert!(
            session.contains(ids::VIEW_TOGGLE_SESSION_ARRANGEMENT),
            "{session}"
        );
        let settings = window_of(View::Settings, &shell, false).unwrap_err();
        assert!(settings.contains(ids::APP_SETTINGS), "{settings}");
        assert_eq!(
            window_of(View::Settings, &shell, true),
            Ok(WindowKind::Settings)
        );
        let main_view = region(View::Arrangement, &shell, &main).unwrap().unwrap();
        let detail = region(View::Detail, &shell, &main).unwrap().unwrap();
        assert!(
            detail.top() >= main_view.bottom() - 1.0,
            "{main_view:?} {detail:?}"
        );

        // In the Session View, the mixer is the view.
        shell
            .run(ids::VIEW_TOGGLE_SESSION_ARRANGEMENT, serde_json::json!({}))
            .unwrap();
        let main = drawn(&shell);
        assert_eq!(
            window_of(View::Session, &shell, false),
            Ok(WindowKind::Main)
        );
        assert!(window_of(View::Arrangement, &shell, false).is_err());
        if window_of(View::Mixer, &shell, false).is_ok() {
            assert_eq!(
                region(View::Mixer, &shell, &main).unwrap(),
                region(View::Session, &shell, &main).unwrap()
            );
        }
    }

    #[test]
    fn each_view_captures_from_a_frame_rendered_on_the_gpu() {
        // Spec: agent-perception R8. The main window drawn offscreen on the
        // GPU, as `--screenshot` draws it, at 2 pixels a point, and each
        // view on screen cut from the frame. Without a GPU adapter (some CI
        // machines), it says so and checks nothing.
        let shell = Shell::headless();
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts());
        let theme = Theme::builtin(shell.theme());
        theme.apply(&ctx);
        let mut main = MainWindowUi::new();
        let size = vec2(1600.0, 977.0);
        let mut textures = egui::TexturesDelta::default();
        let mut last = None;
        for frame in 0..3 {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(f64::from(frame) / 60.0),
                ..Default::default()
            };
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .native_pixels_per_point = Some(2.0);
            let mut output = ctx.run_ui(input, |ui| {
                main.show(ui, &shell, &theme, false, None);
            });
            textures.append(std::mem::take(&mut output.textures_delta));
            last = Some(output);
        }
        let image = match crate::screenshot::render(&ctx, last.unwrap(), textures, size, &theme) {
            Ok(image) => image,
            Err(error) if error.starts_with("no GPU") => {
                println!("skipped: {error}");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        assert_eq!(image.size, [3200, 1954]);
        for view in [
            View::Window,
            View::Arrangement,
            View::Browser,
            View::Detail,
            View::Mixer,
        ] {
            if window_of(view, &shell, false).is_err() {
                continue;
            }
            let rect = region(view, &shell, &main).unwrap();
            let captured = encode(&image, rect, 2.0, 4096).unwrap();
            let points = rect.map_or(size, |r| r.size());
            assert!(
                (f64::from(captured.width) - f64::from(points.x) * 2.0).abs() <= 1.0
                    && (f64::from(captured.height) - f64::from(points.y) * 2.0).abs() <= 1.0,
                "{view:?}: {} x {} for {points:?}",
                captured.width,
                captured.height
            );
            let (_, _, rgba) = decode(&captured.png);
            let first = &rgba[..4];
            assert!(
                rgba.chunks(4).any(|p| p != first),
                "{view:?} is one flat colour"
            );
            println!(
                "{view:?}: {} x {} pixels, {} bytes of PNG",
                captured.width,
                captured.height,
                captured.png.len()
            );
        }
    }
}
