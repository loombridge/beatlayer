//! The Arrangement View's frame-time benchmark (WP-1.8a; ADR-001,
//! "Consequences": each UI package reports its CPU per frame against the
//! benchmark scene).
//!
//! It draws the main window with the benchmark scene (200 tracks, 5,000
//! audio clips with waveforms) offscreen on the GPU at 1600 × 1000 points
//! and 2 pixels per point, as on a Retina screen, so no window opens. Each
//! frame moves the view as the UI spikes' scripted scenario did: the zoom
//! follows a raised cosine on a log scale with a 5-second period, from 8 bars
//! across the lanes out to all 512 and back, while the left edge sweeps from
//! bar 1 to the end. Frames run back to back with simulated 60 Hz time.
//!
//! For every frame it records the main thread's CPU time from the start of
//! the frame to the GPU submission (the view commands, egui's pass with the
//! Arrangement's instance building, tessellation, the paint callback's
//! uploads, encoding, and submitting), and the GPU's time for the frame's
//! render pass from timestamp queries where the adapter has them.
//!
//! ```sh
//! cargo run --release -p bl-ui --example arrangement_bench -- [--frames N]
//!     [--config standard|all-tracks|whole] [--accesskit] [--png FILE]
//! ```
//!
//! - `standard`: tracks at their default height, so a few are on screen.
//! - `all-tracks`: Optimize Height, so all 200 tracks are on screen and,
//!   zoomed out, all 5,000 clips: the architecture's target.
//! - `whole`: all 200 tracks and all 512 bars on screen in every frame, so
//!   every frame builds all 5,000 clips and their waveforms: the worst case.
//! - `drag` (WP-1.8b): all 200 tracks on screen, 64 bars across, while a
//!   drag moves one clip on every track: each frame runs the move inside
//!   the drag's transaction (`Shell::gesture`) from where it began, so the
//!   set changes every frame and the view rebuilds its clip index and
//!   lanes.
//! - `drag-all` (WP-1.8b): as `drag`, moving all 5,000 clips.

use std::time::{Duration, Instant};

use bl_core::time::Ticks;
use bl_shell::arrangement_commands as ids;
use bl_ui::arrangement::benchmark;
use bl_ui::main_window::{MainWindowUi, UiEvent};
use bl_ui::theme::{Theme, fonts};
use egui::{Pos2, RawInput, Rect, vec2};
use egui_wgpu::wgpu;
use serde_json::json;

/// Pixels per point, as on a Retina screen.
const SCALE: f32 = 2.0;

/// Frames run before measuring: pipelines, the font atlas, first uploads.
const WARM_UP: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Config {
    Standard,
    AllTracks,
    Whole,
    Drag,
    DragAll,
}

impl Config {
    fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::AllTracks => "all-tracks",
            Self::Whole => "whole",
            Self::Drag => "drag",
            Self::DragAll => "drag-all",
        }
    }
}

/// CPU time the calling thread has used.
#[cfg(unix)]
fn thread_cpu_time() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec, and CLOCK_THREAD_CPUTIME_ID
    // is supported on macOS 10.12 and later and on Linux.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return Duration::ZERO;
    }
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Wall time stands in where thread CPU time isn't read.
#[cfg(not(unix))]
fn thread_cpu_time() -> Duration {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

struct Args {
    frames: usize,
    configs: Vec<Config>,
    accesskit: bool,
    png: Option<std::path::PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut args = Args {
        frames: 1800,
        configs: vec![Config::Standard, Config::AllTracks, Config::Whole],
        accesskit: false,
        png: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--frames" => {
                args.frames = it
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--frames takes a number")?;
            }
            "--config" => {
                let name = it.next().ok_or("--config takes a name")?;
                args.configs = vec![match name.as_str() {
                    "standard" => Config::Standard,
                    "all-tracks" => Config::AllTracks,
                    "whole" => Config::Whole,
                    "drag" => Config::Drag,
                    "drag-all" => Config::DragAll,
                    other => return Err(format!("no config called {other}")),
                }];
            }
            "--accesskit" => args.accesskit = true,
            "--png" => args.png = Some(it.next().ok_or("--png takes a file")?.into()),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(args)
}

/// The GPU, offscreen.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    view: wgpu::TextureView,
    texture: wgpu::Texture,
    timestamps: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
    period_ns: f32,
    width: u32,
    height: u32,
    adapter: String,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn gpu(size: egui::Vec2) -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|e| format!("no GPU adapter: {e}"))?;
    // Timestamps written by the encoder around the render pass, where the
    // adapter can (Metal on Apple silicon writes pass-boundary timestamps
    // only now and then).
    let wanted = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let timestamps = adapter.features().contains(wanted);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: if timestamps {
            wanted
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    }))
    .map_err(|e| format!("no GPU device: {e}"))?;
    let mut renderer =
        egui_wgpu::Renderer::new(&device, FORMAT, egui_wgpu::RendererOptions::default());
    bl_ui::arrangement::gpu::install(&device, FORMAT, &mut renderer.callback_resources);
    let width = (size.x * SCALE).round() as u32;
    let height = (size.y * SCALE).round() as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("benchmark"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let timestamps = timestamps.then(|| {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("benchmark"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamps"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamps read"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        (set, resolve, read)
    });
    let info = adapter.get_info();
    Ok(Gpu {
        period_ns: queue.get_timestamp_period(),
        device,
        queue,
        renderer,
        view,
        texture,
        timestamps,
        width,
        height,
        adapter: format!("{} ({:?})", info.name, info.backend),
    })
}

impl Gpu {
    /// Renders a frame's output and submits it; returns the CPU time spent
    /// here. The GPU's time is read by [`Gpu::finish`].
    fn render(&mut self, ctx: &egui::Context, output: egui::FullOutput) {
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.width, self.height],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut commands = self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        if let Some((set, _, _)) = &self.timestamps {
            encoder.write_timestamp(set, 0);
        }
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("benchmark"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &primitives, &screen);
        }
        if let Some((set, resolve, read)) = &self.timestamps {
            encoder.write_timestamp(set, 1);
            encoder.resolve_query_set(set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
        }
        commands.push(encoder.finish());
        self.queue.submit(commands);
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }

    /// Waits for the GPU and returns the frame's render-pass time.
    /// Waits for the GPU. Returns the frame's render-pass time from the
    /// timestamps, if the adapter writes them, and how long the wait for the
    /// frame to finish took after it was submitted (an upper bound on the
    /// GPU's work, which includes the driver's own latency).
    fn finish(&self) -> (Option<Duration>, Duration) {
        let waited = Instant::now();
        let Some((_, _, read)) = self.timestamps.as_ref() else {
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            return (None, waited.elapsed());
        };
        let slice = read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        if self
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .is_err()
        {
            return (None, waited.elapsed());
        }
        let waited = waited.elapsed();
        let ticks = slice.get_mapped_range().ok().and_then(|data| {
            let begin = u64::from_le_bytes(data[0..8].try_into().ok()?);
            let end = u64::from_le_bytes(data[8..16].try_into().ok()?);
            Some(end.saturating_sub(begin))
        });
        read.unmap();
        let pass = ticks
            .filter(|t| *t > 0)
            .map(|t| Duration::from_nanos((t as f64 * f64::from(self.period_ns)) as u64));
        (pass, waited)
    }

    /// Reads the target back as an image.
    fn image(&self) -> Result<egui::ColorImage, String> {
        let padded = (self.width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("benchmark image"),
            size: u64::from(padded) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut pixels = Vec::with_capacity((self.width * self.height) as usize);
        for row in data.chunks(padded as usize) {
            for rgba in row[..(self.width * 4) as usize].chunks(4) {
                pixels.push(egui::Color32::from_rgba_premultiplied(
                    rgba[0], rgba[1], rgba[2], rgba[3],
                ));
            }
        }
        Ok(egui::ColorImage::new(
            [self.width as usize, self.height as usize],
            pixels,
        ))
    }
}

fn write_png(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let [w, h] = image.size;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|p| p.to_srgba_unmultiplied())
        .collect();
    writer.write_image_data(&bytes).map_err(|e| e.to_string())
}

/// The scripted view at time `t` seconds: the span across the lanes and
/// its left edge, in bars.
fn scripted(t: f64, duration: f64) -> (f64, f64) {
    let phase = (t % 5.0) / 5.0;
    let wave = 0.5 - 0.5 * (std::f64::consts::TAU * phase).cos();
    let span = (8f64.ln() + wave * (512f64.ln() - 8f64.ln())).exp();
    let left = (t / duration) * (512.0 - span).max(0.0);
    (left, span)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn main() -> Result<(), String> {
    let args = args()?;
    // The whole window, with Live's title bar drawn in it, as on macOS (D22).
    let size = vec2(1600.0, 1000.0);
    let built = Instant::now();
    let scene = benchmark::scene(200, 25, 64);
    eprintln!(
        "scene: {} tracks, {} clips, {} media files, built in {:.2} s",
        scene.tracks.len(),
        scene.clips,
        scene.peaks.len(),
        built.elapsed().as_secs_f64()
    );
    let mut gpu = gpu(size)?;
    eprintln!("GPU: {}", gpu.adapter);
    println!(
        "| Config | Frames | Clips built (mean, max) | Waveform bands (mean, max) | CPU median / p99 / max | Wall median / p99 | GPU pass median / p99 | GPU wait median / p99 |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for config in &args.configs {
        let scene = benchmark::scene(200, 25, 64);
        // The clips a drag moves: the first on each track, or all of them.
        let dragged: Vec<bl_model::ClipId> = {
            let doc = scene.editor.document();
            scene
                .tracks
                .iter()
                .flat_map(|t| {
                    let mut clips: Vec<_> = doc.track(*t).unwrap().clips().collect();
                    clips.sort_by_key(|c| c.start);
                    let take = if *config == Config::DragAll {
                        clips.len()
                    } else {
                        1
                    };
                    clips
                        .into_iter()
                        .take(take)
                        .map(|c| c.id)
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let mut shell = benchmark::shell_with(scene.editor);
        let mut main = MainWindowUi::new();
        main.arrangement_mut()
            .set_peak_source(Box::new(scene.peaks), None);
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts());
        if args.accesskit {
            ctx.enable_accesskit();
        }
        let theme = Theme::builtin(shell.theme());
        theme.apply(&ctx);
        let mut cpu = Vec::with_capacity(args.frames);
        let mut gpu_times = Vec::with_capacity(args.frames);
        let mut clips = Vec::with_capacity(args.frames);
        let mut waveforms = Vec::with_capacity(args.frames);
        let mut walls = Vec::with_capacity(args.frames);
        let mut waits = Vec::with_capacity(args.frames);
        let duration = args.frames as f64 / 60.0;
        for frame in 0..WARM_UP + args.frames {
            let t = frame.saturating_sub(WARM_UP) as f64 / 60.0;
            let start = thread_cpu_time();
            let wall_start = Instant::now();
            // The view for this frame.
            if frame == 1 && *config != Config::Standard {
                shell
                    .run(ids::OPTIMIZE_HEIGHT, json!({ "on": true }))
                    .map_err(|e| e.to_string())?;
            }
            let (left, span) = match config {
                Config::Whole => (0.0, 512.0 + (frame % 2) as f64 * 0.01),
                Config::Drag | Config::DragAll => (0.0, 64.0),
                _ => scripted(t, duration),
            };
            if matches!(config, Config::Drag | Config::DragAll) && frame >= 1 {
                // A drag swinging between 0 and 8 bars later, a sixteenth at
                // a time.
                let sixteenths = (64.0 * (1.0 - (t * 1.3).cos())).round() as i64;
                let invocation = bl_shell::Invocation::new(
                    bl_shell::editing_commands::MOVE_CLIPS,
                    json!({ "clips": dragged, "by": Ticks::SIXTEENTH.get() * sixteenths }),
                );
                shell
                    .gesture(bl_shell::GesturePhase::Update, Some(&invocation))
                    .map_err(|e| e.to_string())?;
            }
            let bar = Ticks::WHOLE.get() as f64;
            shell
                .run(
                    ids::SHOW_RANGE,
                    json!({
                        "start": (left * bar) as i64,
                        "end": ((left + span) * bar) as i64,
                    }),
                )
                .map_err(|e| e.to_string())?;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            ctx.set_pixels_per_point(SCALE);
            let shell_ref = &shell;
            let mut events = Vec::new();
            let output = ctx.run_ui(input, |ui| {
                events = main.show(ui, shell_ref, &theme, true, None);
            });
            for event in events {
                if let UiEvent::ArrangementViewport { width, height } = event {
                    shell.set_arrangement_viewport(width, height);
                }
            }
            gpu.render(&ctx, output);
            let spent = thread_cpu_time() - start;
            let wall = wall_start.elapsed();
            let (pass, waited) = gpu.finish();
            if frame >= WARM_UP {
                cpu.push(spent.as_secs_f64() * 1000.0);
                walls.push(wall.as_secs_f64() * 1000.0);
                if let Some(pass) = pass {
                    gpu_times.push(pass.as_secs_f64() * 1000.0);
                }
                waits.push(waited.as_secs_f64() * 1000.0);
                let stats = main.arrangement().stats();
                clips.push(stats.clips);
                waveforms.push(stats.waveforms);
            }
        }
        for list in [&mut cpu, &mut walls, &mut gpu_times, &mut waits] {
            list.sort_by(f64::total_cmp);
        }
        let mean = |v: &[usize]| v.iter().sum::<usize>() as f64 / v.len().max(1) as f64;
        let max = |v: &[usize]| v.iter().copied().max().unwrap_or(0);
        let gpu_column = if gpu_times.is_empty() {
            "no timestamps".to_owned()
        } else {
            format!(
                "{:.2} / {:.2} ms",
                percentile(&gpu_times, 50.0),
                percentile(&gpu_times, 99.0)
            )
        };
        println!(
            "| {} | {} | {:.0}, {} | {:.0}, {} | {:.2} / {:.2} / {:.2} ms | {:.2} / {:.2} ms | {gpu_column} | {:.2} / {:.2} ms |",
            config.name(),
            cpu.len(),
            mean(&clips),
            max(&clips),
            mean(&waveforms),
            max(&waveforms),
            percentile(&cpu, 50.0),
            percentile(&cpu, 99.0),
            cpu.last().copied().unwrap_or(f64::NAN),
            percentile(&walls, 50.0),
            percentile(&walls, 99.0),
            percentile(&waits, 50.0),
            percentile(&waits, 99.0),
        );
        if let Some(path) = &args.png
            && Some(config) == args.configs.last()
        {
            write_png(path, &gpu.image()?)?;
            eprintln!("wrote {}", path.display());
        }
    }
    Ok(())
}
