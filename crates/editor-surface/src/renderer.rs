//! wgpu renderer: two instanced pipelines (rect + glyph) sharing pixel-space
//! Globals. Coordinates are logical px; the surface is physical px and the NDC
//! mapping (resolution = logical size) handles HiDPI scaling for free
//! (AGENT_RUST_PORT.md §5).

use std::sync::Arc;

use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use nvim_core::grid::GridStateStore;
use winit::window::Window;

use crate::animation::AnimationState;
use crate::atlas::GlyphAtlas;
use crate::frame::{DrawLists, FloatCache, FrameBuilder, GlyphInstance, RectInstance, sync_float_cache};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    resolution: [f32; 2],
    offset: [f32; 2],
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
        InstanceBuffer { buffer, capacity }
    }

    fn upload<T: Pod>(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, data: &[T]) {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        if bytes.len() as u64 > self.capacity {
            let new_cap = (bytes.len() as u64).next_power_of_two().max(1024);
            *self = InstanceBuffer::new(device, label, new_cap);
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytes);
        }
    }
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    scale: f32,

    globals_buf: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,

    rect_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    atlas_bind_group: wgpu::BindGroup,

    rect_buf: InstanceBuffer,
    glyph_buf: InstanceBuffer,

    float_cache: FloatCache,
    pub atlas: GlyphAtlas,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        font_family: Option<&str>,
        font_size: f32,
        line_height: f32,
    ) -> Result<Self> {
        pollster::block_on(Self::new_async(window, font_family, font_size, line_height))
    }

    async fn new_async(
        window: Arc<Window>,
        font_family: Option<&str>,
        font_size: f32,
        line_height: f32,
    ) -> Result<Self> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;

        let mut inst_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        inst_desc.backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(inst_desc);
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| anyhow!("no suitable GPU adapter found: {e}"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("failed to create GPU device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        // Globals (group 0).
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals-bg"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        // Atlas (group 1).
        let atlas = GlyphAtlas::new(&device, font_family, font_size, line_height, scale)?;
        let atlas_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let atlas_bind_group = make_atlas_bind_group(&device, &atlas_bind_group_layout, &atlas);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });

        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let targets = [Some(wgpu::ColorTargetState {
            format,
            blend: Some(blend),
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let rect_attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let rect_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RectInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &rect_attrs,
        };
        let glyph_attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4];
        let glyph_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &glyph_attrs,
        };

        let rect_pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rect-pl"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let glyph_pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyph-pl"),
            bind_group_layouts: &[Some(&globals_layout), Some(&atlas_bind_group_layout)],
            immediate_size: 0,
        });

        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rect-pipeline"),
            layout: Some(&rect_pl_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("rect_vs"),
                compilation_options: Default::default(),
                buffers: &[rect_layout],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("rect_fs"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph-pipeline"),
            layout: Some(&glyph_pl_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("glyph_vs"),
                compilation_options: Default::default(),
                buffers: &[glyph_layout],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("glyph_fs"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        let rect_buf = InstanceBuffer::new(&device, "rect-instances", 1 << 16);
        let glyph_buf = InstanceBuffer::new(&device, "glyph-instances", 1 << 16);

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            scale,
            globals_buf,
            globals_bind_group,
            rect_pipeline,
            glyph_pipeline,
            atlas_bind_group_layout,
            atlas_bind_group,
            rect_buf,
            glyph_buf,
            float_cache: FloatCache::new(),
            atlas,
        })
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Logical cell metrics for grid sizing / hit testing.
    pub fn cell_size(&self) -> (f32, f32) {
        (self.atlas.cell_w, self.atlas.cell_h)
    }

    pub fn logical_size(&self) -> (f32, f32) {
        (self.config.width as f32 / self.scale, self.config.height as f32 / self.scale)
    }

    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        if (scale - self.scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.atlas.reconfigure(&self.device, self.atlas.size_px, self.atlas.line_height, scale);
            self.atlas_bind_group =
                make_atlas_bind_group(&self.device, &self.atlas_bind_group_layout, &self.atlas);
        }
    }

    /// Change font size/family/line-height; resets the atlas (§13.7).
    pub fn set_font(&mut self, size_px: f32, line_height: f32) {
        self.atlas.reconfigure(&self.device, size_px, line_height, self.scale);
        self.atlas_bind_group =
            make_atlas_bind_group(&self.device, &self.atlas_bind_group_layout, &self.atlas);
    }

    pub fn render(
        &mut self,
        store: &GridStateStore,
        anim: &mut AnimationState,
        overlay: Option<&str>,
    ) -> Result<()> {
        let shake = anim.shake_offset();
        let (lw, lh) = self.logical_size();
        let globals = Globals {
            resolution: [lw, lh],
            offset: shake,
        };
        self.queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        sync_float_cache(store, &mut self.float_cache);

        let mut lists: DrawLists =
            FrameBuilder::build(store, anim, &mut self.atlas, &self.queue, &self.float_cache);
        for id in anim.fading_out_float_ids() {
            if anim.float_opacity(id) <= 0.01 {
                self.float_cache.remove(&id);
            }
        }
        if let Some(text) = overlay {
            self.push_overlay(&mut lists, text);
        }
        // Atlas may have grown into a new texture? It only resets on font/dpi
        // change (handled elsewhere); the bind group stays valid here.

        self.rect_buf.upload(&self.device, &self.queue, "rect-instances", &lists.rects);
        self.glyph_buf.upload(&self.device, &self.queue, "glyph-instances", &lists.glyphs);

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    Cst::Success(f) | Cst::Suboptimal(f) => f,
                    _ => return Ok(()),
                }
            }
            // Timeout / Occluded / Validation: skip this frame.
            _ => return Ok(()),
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("encoder") });
        {
            let clear = lists.clear;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if !lists.rects.is_empty() {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, self.rect_buf.buffer.slice(..));
                pass.draw(0..6, 0..lists.rects.len() as u32);
            }
            if !lists.glyphs.is_empty() {
                pass.set_pipeline(&self.glyph_pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, self.glyph_buf.buffer.slice(..));
                pass.draw(0..6, 0..lists.glyphs.len() as u32);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// Append a top-right overlay string (e.g. the FPS meter) to the draw lists.
    fn push_overlay(&mut self, lists: &mut DrawLists, text: &str) {
        let (lw, _) = self.logical_size();
        let adv = self.atlas.cell_w;
        let ch = self.atlas.cell_h;
        let margin = 8.0;
        let count = text.chars().count() as f32;
        let width = count * adv;
        let x0 = (lw - width - margin).max(0.0);
        let y0 = margin;

        // Translucent backdrop for legibility.
        lists.rects.push(RectInstance {
            pos: [x0 - 5.0, y0 - 3.0],
            size: [width + 10.0, ch + 6.0],
            color: [0.0, 0.0, 0.0, 0.55],
        });

        let color = [1.0, 0.82, 0.18, 1.0];
        let mut x = x0;
        for c in text.chars() {
            if c != ' ' {
                if let Some(info) = self.atlas.glyph(&self.queue, c) {
                    lists.glyphs.push(GlyphInstance {
                        pos: [x + info.left, y0 + info.top],
                        size: [info.width, info.height],
                        uv_min: info.uv_min,
                        uv_max: info.uv_max,
                        color,
                    });
                }
            }
            x += adv;
        }
    }
}

fn make_atlas_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    atlas: &GlyphAtlas,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlas-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&atlas.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&atlas.sampler),
            },
        ],
    })
}
