//! Dense views through a wgpu paint callback.
//!
//! The peaks for every clip live in one storage buffer, uploaded once. Each
//! frame the UI hands over a [`DenseList`]; `prepare` copies its instances into
//! per-region vertex buffers, and `paint` draws them with two pipelines inside
//! egui's own render pass, so egui text can still be drawn on top.

use std::borrow::Cow;

use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt as _;
use egui_wgpu::{CallbackResources, CallbackTrait, RenderState, ScreenDescriptor};

use crate::dense::{DenseList, RectInst, WaveInst};
use crate::scene::Scene;

/// One region of the window with its own instance buffers.
#[derive(Clone, Copy, Debug)]
pub enum Slot {
    Timeline = 0,
    Meters = 1,
}

const SLOTS: usize = 2;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen_px: [f32; 2],
    pixels_per_point: f32,
    _pad: f32,
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

/// The GPU side of the dense views, kept in egui-wgpu's callback resources.
pub struct GpuDense {
    rect_pipeline: wgpu::RenderPipeline,
    wave_pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    rects: Vec<InstanceBuffer>,
    waves: Vec<InstanceBuffer>,
}

impl GpuDense {
    /// Creates the pipelines, uploads the scene's peaks, and stores the result
    /// in the renderer's callback resources.
    pub fn install(render_state: &RenderState, scene: &Scene) {
        let device = &render_state.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dense"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("dense.wgsl"))),
        });

        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dense globals"),
            contents: bytemuck::bytes_of(&Globals::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let peaks = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("peaks"),
            contents: &scene.peaks,
            usage: wgpu::BufferUsages::STORAGE,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dense"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dense"),
            layout: &layout,
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
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dense"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

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
                            format: render_state.target_format,
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
            "dense rects",
            "rect_vs",
            "rect_fs",
            size_of::<RectInst>() as u64,
            &wgpu::vertex_attr_array![0 => Float32x4, 1 => Unorm8x4],
        );
        let wave_pipeline = pipeline(
            "dense waves",
            "wave_vs",
            "wave_fs",
            size_of::<WaveInst>() as u64,
            &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Uint32x2, 3 => Unorm8x4],
        );

        let rects = (0..SLOTS)
            .map(|_| InstanceBuffer::new(device, "dense rects", 64 * 1024))
            .collect();
        let waves = (0..SLOTS)
            .map(|_| InstanceBuffer::new(device, "dense waves", 64 * 1024))
            .collect();

        let resources = Self {
            rect_pipeline,
            wave_pipeline,
            globals,
            bind_group,
            rects,
            waves,
        };
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(resources);
    }
}

/// One region's instances for one frame.
pub struct DenseCallback {
    pub slot: Slot,
    pub list: DenseList,
}

impl CallbackTrait for DenseCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<GpuDense>() else {
            return Vec::new();
        };
        let globals = Globals {
            screen_px: [
                screen.size_in_pixels[0] as f32,
                screen.size_in_pixels[1] as f32,
            ],
            pixels_per_point: screen.pixels_per_point,
            _pad: 0.0,
        };
        queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
        let slot = self.slot as usize;
        gpu.rects[slot].write(
            device,
            queue,
            "dense rects",
            bytemuck::cast_slice(&self.list.rects),
        );
        gpu.waves[slot].write(
            device,
            queue,
            "dense waves",
            bytemuck::cast_slice(&self.list.waves),
        );
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(gpu) = resources.get::<GpuDense>() else {
            return;
        };
        // egui sets the viewport to the callback's rect; the shaders work in
        // whole-window pixels, so widen it again. The scissor stays egui's.
        let [w, h] = info.screen_size_px;
        pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
        pass.set_bind_group(0, &gpu.bind_group, &[]);
        let slot = self.slot as usize;
        for (rects, waves) in self.list.layers() {
            if !rects.is_empty() {
                pass.set_pipeline(&gpu.rect_pipeline);
                pass.set_vertex_buffer(0, gpu.rects[slot].buffer.slice(..));
                pass.draw(0..6, rects.start as u32..rects.end as u32);
            }
            if !waves.is_empty() {
                pass.set_pipeline(&gpu.wave_pipeline);
                pass.set_vertex_buffer(0, gpu.waves[slot].buffer.slice(..));
                pass.draw(0..6, waves.start as u32..waves.end as u32);
            }
        }
    }
}
