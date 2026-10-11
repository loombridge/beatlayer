//! The Arrangement View's own wgpu pipelines, drawn inside egui's render
//! pass through a paint callback (ADR-001): instanced rectangles, and a
//! shader that draws waveforms from peak data.
//!
//! The UI builds each frame's instances into a [`Shared`] block that the
//! callback also holds; the callback's `prepare` uploads a region's
//! instances only when they changed, and peak data only when new peaks
//! arrived, and `paint` draws them. The host installs the pipelines once
//! per GPU device with [`install`], before the first frame.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu::util::DeviceExt as _;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};

use super::peaks::Atlas;

/// One rectangle: its corners in points and its colour, premultiplied, in
/// gamma space as egui's colours are.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct RectInst {
    /// Left, top, right, bottom, in points.
    pub rect: [f32; 4],
    /// RGBA.
    pub color: [u8; 4],
}

/// One waveform band: one channel of one clip, over the part of the clip
/// on screen.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WaveInst {
    /// The part to draw: left, top, right, bottom, in points.
    pub draw: [f32; 4],
    /// The band's top and bottom in points, the peaks under one pixel, and
    /// the fraction of a peak at the left edge.
    pub band: [f32; 4],
    /// The run's first peak in the peak buffer, its length in peaks, the
    /// whole peak at the left edge, and nothing.
    pub run: [u32; 4],
    /// RGBA, as for [`RectInst`].
    pub color: [u8; 4],
}

/// The regions the view draws, each with its own instance buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// The scrolling track lanes.
    Lanes = 0,
    /// The Overview strip.
    Overview = 1,
}

/// How many [`Slot`]s there are.
pub const SLOTS: usize = 2;

/// One region's instances for the frame, and a version that changes when
/// they do, so an unchanged region isn't uploaded again.
#[derive(Clone, Debug, Default)]
pub struct SlotData {
    /// Rectangles, drawn first, in order.
    pub rects: Vec<RectInst>,
    /// Waveforms, drawn over the rectangles.
    pub waves: Vec<WaveInst>,
    /// Changes whenever `rects` or `waves` do.
    pub version: u64,
}

/// What the UI and the callback share: each region's instances and the
/// peak data to upload.
#[derive(Debug, Default)]
pub struct Shared {
    /// Each region's instances.
    pub slots: [SlotData; SLOTS],
    /// The peak buffer's contents.
    pub atlas: Atlas,
}

/// The shared block, as the UI and the callback hold it.
pub type SharedFrame = Arc<Mutex<Shared>>;

/// Locks the shared block, whatever a panic elsewhere left in it.
pub fn lock(shared: &SharedFrame) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen_px: [f32; 2],
    pixels_per_point: f32,
    linear_target: u32,
}

struct InstanceBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
}

impl InstanceBuffer {
    fn new(device: &wgpu::Device, label: &str, capacity: u64) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, capacity }
    }

    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let needed = bytes.len() as u64;
        if needed > self.capacity {
            *self = Self::new(device, label, needed.next_power_of_two());
        }
        queue.write_buffer(&self.buffer, 0, bytes);
    }
}

struct SlotGpu {
    rects: InstanceBuffer,
    waves: InstanceBuffer,
    rect_count: u32,
    wave_count: u32,
    version: Option<u64>,
}

/// The pipelines and buffers, kept in egui-wgpu's callback resources.
pub struct ArrangementGpu {
    rect_pipeline: wgpu::RenderPipeline,
    wave_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    peaks: wgpu::Buffer,
    peak_capacity: u64,
    max_peak_bytes: u64,
    bind_group: wgpu::BindGroup,
    linear_target: bool,
    slots: Vec<SlotGpu>,
    /// The atlas epoch and last entry uploaded.
    uploaded: Option<(u64, u64)>,
}

/// The peak buffer's starting size.
const INITIAL_PEAK_BYTES: u64 = 1 << 20;

/// The most peak data the view uploads: wgpu's default limit for one
/// storage buffer binding, about 37 hours of 48 kHz stereo.
pub const MAX_PEAK_BYTES: u64 = 128 << 20;

/// Creates the pipelines for a target format and keeps them in a renderer's
/// callback resources. Call once per renderer, before the first frame that
/// shows the Arrangement View.
pub fn install(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    resources: &mut CallbackResources,
) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("arrangement"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("arrangement.wgsl"))),
    });
    let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("arrangement globals"),
        contents: bytemuck::bytes_of(&Globals::zeroed()),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("arrangement"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let peaks = peak_buffer(device, INITIAL_PEAK_BYTES);
    let bind_group = bind_group(device, &layout, &globals, &peaks);
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("arrangement"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    // Premultiplied alpha, as egui blends.
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let pipeline =
        |label: &str, vs: &str, fs: &str, stride: u64, attributes: &[wgpu::VertexAttribute]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: stride,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
    let rect_pipeline = pipeline(
        "arrangement rects",
        "rect_vs",
        "rect_fs",
        size_of::<RectInst>() as u64,
        &wgpu::vertex_attr_array![0 => Float32x4, 1 => Unorm8x4],
    );
    let wave_pipeline = pipeline(
        "arrangement waves",
        "wave_vs",
        "wave_fs",
        size_of::<WaveInst>() as u64,
        &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Uint32x4, 3 => Unorm8x4],
    );
    let slots = (0..SLOTS)
        .map(|_| SlotGpu {
            rects: InstanceBuffer::new(device, "arrangement rects", 64 * 1024),
            waves: InstanceBuffer::new(device, "arrangement waves", 64 * 1024),
            rect_count: 0,
            wave_count: 0,
            version: None,
        })
        .collect();
    let max_peak_bytes = MAX_PEAK_BYTES
        .min(device.limits().max_storage_buffer_binding_size)
        .min(device.limits().max_buffer_size);
    resources.insert(ArrangementGpu {
        rect_pipeline,
        wave_pipeline,
        layout,
        globals,
        peaks,
        peak_capacity: INITIAL_PEAK_BYTES,
        max_peak_bytes,
        bind_group,
        linear_target: format.is_srgb(),
        slots,
        uploaded: None,
    });
}

fn peak_buffer(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("arrangement peaks"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    peaks: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("arrangement"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: peaks.as_entire_binding(),
            },
        ],
    })
}

impl ArrangementGpu {
    /// Uploads the peak data the atlas has that the buffer doesn't, growing
    /// the buffer (and uploading everything again) when it must.
    fn upload_peaks(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: &mut Atlas) {
        atlas.set_limit(self.max_peak_bytes);
        let needed = atlas.used_bytes().max(4);
        let mut everything = self
            .uploaded
            .is_none_or(|(epoch, _)| epoch != atlas.epoch());
        if needed > self.peak_capacity {
            let capacity = needed
                .next_power_of_two()
                .min(self.max_peak_bytes)
                .max(needed);
            self.peaks = peak_buffer(device, capacity);
            self.peak_capacity = capacity;
            self.bind_group = bind_group(device, &self.layout, &self.globals, &self.peaks);
            everything = true;
        }
        let after = if everything {
            None
        } else {
            self.uploaded.map(|(_, seq)| seq)
        };
        let mut last = after.unwrap_or(0);
        for entry in atlas.entries_after(after) {
            queue.write_buffer(&self.peaks, entry.byte_offset(), entry.peaks.data());
            last = last.max(entry.seq);
        }
        self.uploaded = Some((atlas.epoch(), last.max(atlas.last_seq())));
    }
}

/// One region's drawing for one frame.
pub struct ArrangementCallback {
    /// What the UI built.
    pub shared: SharedFrame,
    /// The region.
    pub slot: Slot,
}

impl CallbackTrait for ArrangementCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<ArrangementGpu>() else {
            return Vec::new();
        };
        let globals = Globals {
            screen_px: [
                screen.size_in_pixels[0] as f32,
                screen.size_in_pixels[1] as f32,
            ],
            pixels_per_point: screen.pixels_per_point,
            linear_target: u32::from(gpu.linear_target),
        };
        queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
        let mut shared = lock(&self.shared);
        let shared = &mut *shared;
        gpu.upload_peaks(device, queue, &mut shared.atlas);
        let data = &shared.slots[self.slot as usize];
        let slot = &mut gpu.slots[self.slot as usize];
        if slot.version != Some(data.version) {
            slot.rects.write(
                device,
                queue,
                "arrangement rects",
                bytemuck::cast_slice(&data.rects),
            );
            slot.waves.write(
                device,
                queue,
                "arrangement waves",
                bytemuck::cast_slice(&data.waves),
            );
            slot.rect_count = data.rects.len() as u32;
            slot.wave_count = data.waves.len() as u32;
            slot.version = Some(data.version);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(gpu) = resources.get::<ArrangementGpu>() else {
            return;
        };
        // egui sets the viewport to the callback's rectangle; the shaders
        // work in whole-target pixels, so widen it again. The scissor stays
        // egui's, which clips to the region.
        let [w, h] = info.screen_size_px;
        pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
        pass.set_bind_group(0, &gpu.bind_group, &[]);
        let slot = &gpu.slots[self.slot as usize];
        if slot.rect_count > 0 {
            pass.set_pipeline(&gpu.rect_pipeline);
            pass.set_vertex_buffer(0, slot.rects.buffer.slice(..));
            pass.draw(0..6, 0..slot.rect_count);
        }
        if slot.wave_count > 0 {
            pass.set_pipeline(&gpu.wave_pipeline);
            pass.set_vertex_buffer(0, slot.waves.buffer.slice(..));
            pass.draw(0..6, 0..slot.wave_count);
        }
    }
}
