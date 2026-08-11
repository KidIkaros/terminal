//! wgpu render pipeline — Layer 4.
//!
//! Owns the GPU device, swap chain surface, instance buffers, atlas texture,
//! and the WGSL shader pipeline.  Every frame it:
//!
//!   1. Builds a `Vec<GlyphInstance>` from the grid snapshot.
//!   2. Uploads it to a `wgpu::Buffer`.
//!   3. Issues a single instanced draw call.
//!
//! Two passes per frame:
//!   - Pass 0: background rectangles (one per cell, mode=0)
//!   - Pass 1: glyph quads (one per non-space cell, mode=1)
//!
//! They are merged into one draw call because the shader selects the path
//! via the `mode` field.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::config::ColorConfig;
use crate::grid::{Color, ColorPalette, Grid};
use crate::render::font::{GlyphAtlas, ShapedGlyph, ATLAS_SIZE};
use crate::search::{SearchMode, SearchState};
use crate::selection::Selection;
use crate::tab_bar::{TabBar, TabBarTarget};

// ---------------------------------------------------------------------------
// Instance data uploaded to the GPU
// ---------------------------------------------------------------------------

/// One instance = one cell background rect OR one glyph quad.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlyphInstance {
    pub cell_pos: [f32; 2],
    pub cell_size: [f32; 2],
    pub atlas_uv_min: [f32; 2],
    pub atlas_uv_max: [f32; 2],
    pub fg_color: [f32; 4],
    pub bg_color: [f32; 4],
    /// 0 = background rect, 1 = glyph
    pub mode: u32,
    pub _pad: [u32; 3],
}

// ---------------------------------------------------------------------------
// Uniforms
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

// ---------------------------------------------------------------------------
// Color palette — xterm 256-color
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

/// Convert an sRGB byte component to the linear value expected by an sRGB
/// render target.
fn srgb_to_linear(component: u8) -> f32 {
    let value = component as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Parse a hex color string (#RRGGBB or #RRGGBBAA) into linear RGBA.
fn parse_hex_color(s: &str) -> [f32; 4] {
    let s = s.trim_start_matches('#');
    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
    let a = if s.len() >= 8 {
        u8::from_str_radix(&s[6..8], 16).unwrap_or(255)
    } else {
        255
    };
    [
        srgb_to_linear(r),
        srgb_to_linear(g),
        srgb_to_linear(b),
        a as f32 / 255.0,
    ]
}

fn adjust_surface_color(mut color: [f32; 4], hovered: bool, pressed: bool) -> [f32; 4] {
    let factor = if pressed {
        0.88
    } else if hovered {
        1.12
    } else {
        1.0
    };
    color[0] = (color[0] * factor).min(1.0);
    color[1] = (color[1] * factor).min(1.0);
    color[2] = (color[2] * factor).min(1.0);
    color
}

/// Build a 256-entry color lookup table from config colors + palette.
fn build_color_table(config: &ColorConfig, palette: &ColorPalette) -> Vec<[f32; 3]> {
    let mut table = Vec::with_capacity(256);

    // First 16 entries from config.ansi
    for i in 0..16 {
        if i < config.ansi.len() {
            let [r, g, b, _] = parse_hex_color(&config.ansi[i]);
            table.push([r, g, b]);
        } else {
            // Fallback to palette
            let (r, g, b) = palette.get_color(i as u8).unwrap_or((0, 0, 0));
            table.push([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]);
        }
    }

    // Fill remaining entries from palette
    for i in 16..256 {
        let idx = i as u8;
        let (r, g, b) = palette.get_color(idx).unwrap_or((0, 0, 0));
        table.push([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]);
    }

    table
}

/// Convert a terminal Color to RGBA, using config colors for defaults.
fn needs_full_redraw(
    offscreen_initialized: bool,
    cursor_visible: bool,
    selection_active: bool,
    search_active: bool,
    tab_bar_present: bool,
    scrolled: bool,
    dirty_cells: usize,
    total_cells: usize,
) -> bool {
    !offscreen_initialized
        || cursor_visible
        || selection_active
        || search_active
        || tab_bar_present
        || scrolled
        || dirty_cells > total_cells / 2
}

fn color_to_rgba(
    color: Color,
    is_fg: bool,
    config: &ColorConfig,
    palette: &ColorPalette,
) -> [f32; 4] {
    match color {
        Color::Default => {
            if is_fg {
                parse_hex_color(&config.foreground)
            } else {
                parse_hex_color(&config.background)
            }
        }
        Color::Indexed(i) => {
            if let Some((r, g, b)) = palette.get_color(i) {
                [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0]
            } else {
                let table = build_color_table(config, palette);
                let [r, g, b] = table[i as usize];
                [r, g, b, 1.0]
            }
        }
        Color::Rgb(r, g, b) => [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0],
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Parameters for a single frame render.
pub struct RenderParams<'a> {
    pub grid: &'a mut Grid,
    pub atlas: &'a mut GlyphAtlas,
    pub cursor_visible: bool,
    pub colors: &'a ColorConfig,
    pub selection: &'a Selection,
    pub search: Option<&'a SearchState>,
    pub tab_bar: Option<&'a TabBar>,
    pub tab_bar_height: f32,
}

pub struct TerminalPipeline {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,

    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    bind_group_layout: wgpu::BindGroupLayout,

    atlas_texture: wgpu::Texture,
    atlas_view: wgpu::TextureView,

    /// Persistent terminal framebuffer. Unlike the swapchain surface, this
    /// texture is guaranteed to retain prior contents between passes.
    offscreen_texture: wgpu::Texture,
    offscreen_view: wgpu::TextureView,
    offscreen_initialized: bool,
    composite_pipeline: wgpu::RenderPipeline,
    composite_bind_group_layout: wgpu::BindGroupLayout,
    composite_bind_group: wgpu::BindGroup,
    composite_sampler: wgpu::Sampler,

    /// Unit square vertex buffer (6 vertices, two triangles).
    vertex_buf: wgpu::Buffer,

    /// Double-buffered instance buffers
    instance_bufs: [Option<wgpu::Buffer>; 2],
    instance_capacity: [usize; 2],
    /// Reused CPU-side instance storage; avoids reallocating every frame.
    instances: Vec<GlyphInstance>,
    /// Number of terminal cells reported dirty by the most recent frame.
    last_dirty_cells: usize,
    current_buffer: usize,
}

impl TerminalPipeline {
    pub async fn new(window: Arc<winit::window::Window>, atlas: &GlyphAtlas, vsync: bool) -> Self {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance.create_surface(window).expect("create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("terminal"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await
            .expect("request device failed");

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: if vsync {
                wgpu::PresentMode::Fifo // VSync enabled
            } else {
                wgpu::PresentMode::Immediate // No VSync (tearing possible)
            },
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let offscreen_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminal_offscreen"),
            size: wgpu::Extent3d {
                width: size.width.max(1),
                height: size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let offscreen_view = offscreen_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // ---- Atlas texture ----
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            atlas_texture.as_image_copy(),
            &atlas.bitmap,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE),
                rows_per_image: Some(ATLAS_SIZE),
            },
            wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
        );

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // ---- Uniforms ----
        let uniforms = Uniforms {
            viewport: [size.width as f32, size.height as f32],
            _pad: [0.0; 2],
        };
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // ---- Bind group layout ----
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        // ---- Shader + pipeline ----
        let shader_src = include_str!("shader.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let instance_attrs = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 48,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 64,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Uint32,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminal_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[
                    // Vertex buffer: unit quad positions
                    wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        }],
                    },
                    instance_attrs,
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Unit square — two triangles (6 vertices)
        let quad: [[f32; 2]; 6] = [
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0],
        ];
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad_vb"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let composite_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terminal_composite_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let composite_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("terminal_composite_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let composite_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminal_composite_bg"),
            layout: &composite_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&offscreen_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&composite_sampler),
                },
            ],
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal_composite_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminal_composite_layout"),
            bind_group_layouts: &[&composite_bind_group_layout],
            push_constant_ranges: &[],
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminal_composite_pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x2,
                    }],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        TerminalPipeline {
            device,
            queue,
            surface,
            surface_config,
            pipeline,
            uniform_buf,
            bind_group,
            bind_group_layout,
            atlas_texture,
            atlas_view,
            offscreen_texture,
            offscreen_view,
            offscreen_initialized: false,
            composite_pipeline,
            composite_bind_group_layout,
            composite_bind_group,
            composite_sampler,
            vertex_buf,
            instance_bufs: [None, None],
            instance_capacity: [0, 0],
            instances: Vec::with_capacity(4096),
            last_dirty_cells: 0,
            current_buffer: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Resize
    // -----------------------------------------------------------------------

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);

        self.offscreen_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminal_offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.surface_config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.offscreen_view = self
            .offscreen_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.composite_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminal_composite_bg"),
            layout: &self.composite_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.offscreen_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.composite_sampler),
                },
            ],
        });
        self.offscreen_initialized = false;

        let uniforms = Uniforms {
            viewport: [width as f32, height as f32],
            _pad: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Number of terminal cells marked dirty during the last render call.
    pub fn last_dirty_cells(&self) -> usize {
        self.last_dirty_cells
    }

    // -----------------------------------------------------------------------
    // Re-upload atlas after a cache miss
    // -----------------------------------------------------------------------

    pub fn upload_atlas(&self, atlas: &GlyphAtlas) {
        self.queue.write_texture(
            self.atlas_texture.as_image_copy(),
            &atlas.bitmap,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE),
                rows_per_image: Some(ATLAS_SIZE),
            },
            wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
        );
    }

    // -----------------------------------------------------------------------
    // Render frame
    // -----------------------------------------------------------------------

    /// Center a single glyph `ch` inside a `button_w x button_h` rectangle and
    /// append it to `instances`. Falls back to a plain ASCII alternative if the
    /// requested glyph is not in the atlas.
    fn add_glyph_button(
        instances: &mut Vec<GlyphInstance>,
        atlas: &mut GlyphAtlas,
        baseline: i32,
        ch: char,
        button_x: f32,
        button_y: f32,
        button_w: f32,
        button_h: f32,
        fg_color: [f32; 4],
        bg_color: [f32; 4],
    ) {
        let cw = atlas.cell_width as f32;
        let ch_h = atlas.cell_height as f32;
        let region = atlas
            .get_or_rasterize(ch, false, false)
            .or_else(|| atlas.get_or_rasterize('x', false, false));
        if let Some(r) = &region {
            let text_x = button_x + (button_w - cw) / 2.0;
            let text_y = button_y + (button_h - ch_h) / 2.0;
            let gx = text_x + r.metrics.xmin as f32;
            let gy = text_y + baseline as f32 - r.metrics.height as f32 - r.metrics.ymin as f32;
            instances.push(GlyphInstance {
                cell_pos: [gx, gy],
                cell_size: [r.metrics.width as f32, r.metrics.height as f32],
                atlas_uv_min: r.uv_min,
                atlas_uv_max: r.uv_max,
                fg_color,
                bg_color,
                mode: 1,
                _pad: [0; 3],
            });
        }
    }

    fn add_button_surface(
        instances: &mut Vec<GlyphInstance>,
        rect: (f32, f32, f32, f32),
        color: [f32; 4],
    ) {
        let (x, y, width, height) = rect;
        instances.push(GlyphInstance {
            cell_pos: [x, y],
            cell_size: [width, height],
            atlas_uv_min: [0.0; 2],
            atlas_uv_max: [0.0; 2],
            fg_color: color,
            bg_color: color,
            mode: 0,
            _pad: [0; 3],
        });
    }

    pub fn render(&mut self, params: RenderParams<'_>) {
        let RenderParams {
            grid,
            atlas,
            cursor_visible,
            colors,
            selection,
            search,
            tab_bar,
            tab_bar_height,
        } = params;
        let cw = atlas.cell_width as f32;
        let ch = atlas.cell_height as f32;
        let baseline = atlas.baseline;

        // Render the complete grid each frame. Swapchain surfaces are not
        // guaranteed to preserve their previous contents between frames, so
        // partial rendering with LoadOp::Load loses unchanged terminal cells.
        // Keep the damage count for profiling and drive the guarded persistent
        // target path. UI overlays, selection, scrollback, cursor visibility,
        // and large damage regions use a correctness-first full redraw.
        let dirty_cells = grid.take_dirty_cells();
        self.last_dirty_cells = dirty_cells.len();
        let full_redraw = needs_full_redraw(
            self.offscreen_initialized,
            cursor_visible,
            selection.active,
            search.is_some_and(|state| state.active),
            tab_bar.is_some(),
            grid.view_scrollback_lines() > 0,
            dirty_cells.len(),
            grid.rows * grid.cols,
        );
        let dirty_set: std::collections::HashSet<(usize, usize)> =
            dirty_cells.into_iter().collect();

        // Reuse instance storage — tab bar rects first, then terminal cells
        // and cursor. Capacity grows only when a larger frame requires it.
        let mut instances = std::mem::take(&mut self.instances);
        instances.clear();
        instances.reserve(grid.rows * grid.cols * 2 + 256);

        // --- Tab bar background rectangles ---
        if let Some(tb) = tab_bar {
            let tab_width = 150.0f32;
            let active_color = crate::tab_bar::color_to_floats(&tb.active_color);
            let inactive_color = crate::tab_bar::color_to_floats(&tb.inactive_color);
            let text_color = parse_hex_color(&colors.foreground);
            let header_color = crate::tab_bar::color_to_floats(&tb.bg_color);
            let screen_w = self.surface_config.width as f32;

            // Header bar background (fills the full width at the top)
            instances.push(GlyphInstance {
                cell_pos: [0.0, 0.0],
                cell_size: [screen_w, tab_bar_height],
                atlas_uv_min: [0.0; 2],
                atlas_uv_max: [0.0; 2],
                fg_color: header_color,
                bg_color: header_color,
                mode: 0,
                _pad: [0; 3],
            });

            for (i, tab) in tb.tabs.iter().enumerate() {
                let x = i as f32 * tab_width;
                let target = TabBarTarget::Tab(i);
                let color = adjust_surface_color(
                    if tab.active {
                        active_color
                    } else {
                        inactive_color
                    },
                    tb.is_hovered(target),
                    tb.is_pressed(target),
                );
                instances.push(GlyphInstance {
                    cell_pos: [x, 0.0],
                    cell_size: [tab_width, tab_bar_height],
                    atlas_uv_min: [0.0; 2],
                    atlas_uv_max: [0.0; 2],
                    fg_color: color,
                    bg_color: color,
                    mode: 0,
                    _pad: [0; 3],
                });

                // Render tab title text using the glyph atlas
                let mut text_x = x + 8.0;
                let text_y = (tab_bar_height - ch) / 2.0;
                for ch_title in tab.title.chars().take(18) {
                    if ch_title == ' ' {
                        text_x += cw / 2.0;
                        continue;
                    }
                    let region = atlas.get_or_rasterize(ch_title, false, false);
                    if let Some(r) = &region {
                        let gx = text_x + r.metrics.xmin as f32;
                        let gy = text_y + baseline as f32
                            - r.metrics.height as f32
                            - r.metrics.ymin as f32;
                        instances.push(GlyphInstance {
                            cell_pos: [gx, gy],
                            cell_size: [r.metrics.width as f32, r.metrics.height as f32],
                            atlas_uv_min: r.uv_min,
                            atlas_uv_max: r.uv_max,
                            fg_color: text_color,
                            bg_color: color,
                            mode: 1,
                            _pad: [0; 3],
                        });
                        text_x += cw;
                    }
                }

                // Close button (×) on the right of the tab
                if let Some((cx, cy, cw_btn, ch_btn)) = tb.close_button_rect(i) {
                    let close_target = TabBarTarget::Close(i);
                    let close_color = adjust_surface_color(
                        color,
                        tb.is_hovered(close_target),
                        tb.is_pressed(close_target),
                    );
                    Self::add_button_surface(&mut instances, (cx, cy, cw_btn, ch_btn), close_color);
                    Self::add_glyph_button(
                        &mut instances,
                        atlas,
                        baseline,
                        '×',
                        cx,
                        cy,
                        cw_btn,
                        ch_btn,
                        text_color,
                        close_color,
                    );
                }
            }

            // New tab (+) and search (S) buttons, right-aligned
            let new_target = TabBarTarget::NewTab;
            let new_rect = tb.new_tab_button_rect(self.surface_config.width);
            let new_color = adjust_surface_color(
                header_color,
                tb.is_hovered(new_target),
                tb.is_pressed(new_target),
            );
            Self::add_button_surface(&mut instances, new_rect, new_color);
            Self::add_glyph_button(
                &mut instances,
                atlas,
                baseline,
                '+',
                new_rect.0,
                new_rect.1,
                new_rect.2,
                new_rect.3,
                text_color,
                new_color,
            );

            let search_target = TabBarTarget::Search;
            let search_rect = tb.search_button_rect(self.surface_config.width);
            let search_color = adjust_surface_color(
                header_color,
                tb.is_hovered(search_target),
                tb.is_pressed(search_target),
            );
            Self::add_button_surface(&mut instances, search_rect, search_color);
            Self::add_glyph_button(
                &mut instances,
                atlas,
                baseline,
                'S',
                search_rect.0,
                search_rect.1,
                search_rect.2,
                search_rect.3,
                text_color,
                search_color,
            );
        }

        if let Some(search) = search.filter(|search| search.active) {
            let overlay_width = 320.0f32.min(self.surface_config.width as f32 - 16.0);
            let overlay_height = 36.0;
            let overlay_x = self.surface_config.width as f32 - overlay_width - 8.0;
            let overlay_y = tab_bar_height + 8.0;
            let overlay_bg = parse_hex_color("#313244");
            let overlay_fg = parse_hex_color("#CDD6F4");
            Self::add_button_surface(
                &mut instances,
                (overlay_x, overlay_y, overlay_width, overlay_height),
                overlay_bg,
            );

            let direction = if search.mode == SearchMode::Reverse {
                "reverse"
            } else {
                "search"
            };
            let status = format!(
                "/{}  {}  {}/{}",
                search.query,
                direction,
                if search.matches.is_empty() {
                    0
                } else {
                    search.current_match + 1
                },
                search.matches.len()
            );
            let mut text_x = overlay_x + 12.0;
            let text_y = overlay_y + (overlay_height - ch) / 2.0;
            for character in status.chars().take(40) {
                let Some(region) = atlas.get_or_rasterize(character, false, false) else {
                    continue;
                };
                instances.push(GlyphInstance {
                    cell_pos: [
                        text_x + region.metrics.xmin as f32,
                        text_y + baseline as f32
                            - region.metrics.height as f32
                            - region.metrics.ymin as f32,
                    ],
                    cell_size: [region.metrics.width as f32, region.metrics.height as f32],
                    atlas_uv_min: region.uv_min,
                    atlas_uv_max: region.uv_max,
                    fg_color: overlay_fg,
                    bg_color: overlay_bg,
                    mode: 1,
                    _pad: [0; 3],
                });
                text_x += cw;
            }
        }

        // Scrollback view offset (T1-4): when the user has scrolled up, the
        // top rows are served from the scrollback buffer and the live grid
        // shifts down. `cell_at_view` handles the mapping; offset 0 keeps
        // this an identity mapping.
        let view_offset = grid.view_scrollback_lines();

        for row in 0..grid.rows {
            for col in 0..grid.cols {
                if !full_redraw && !dirty_set.contains(&(row, col)) {
                    continue;
                }
                let Some(cell) = grid.cell_at_view(col, row) else {
                    continue;
                };
                let line_mode = grid.line_mode(row);
                if line_mode == 6 && col % 2 == 1 {
                    continue;
                }
                let render_cw = if line_mode == 6 { cw * 2.0 } else { cw };
                let px = if line_mode == 6 {
                    (col / 2) as f32 * render_cw
                } else {
                    col as f32 * cw
                };
                let py = tab_bar_height + row as f32 * ch;

                let fg_raw = color_to_rgba(cell.fg, true, colors, &grid.palette);
                let bg_raw = color_to_rgba(cell.bg, false, colors, &grid.palette);

                // DECSCNM (?5) screen-reverse flips the whole display; combine
                // with per-cell SGR inverse (T3-4).
                let inverse = cell.attrs.inverse() != grid.screen_reverse;
                let (mut fg, mut bg) = if inverse {
                    (bg_raw, fg_raw)
                } else {
                    (fg_raw, bg_raw)
                };

                // Apply selection highlighting — selection coordinates address
                // live grid rows, so shift by the view offset.
                if row >= view_offset && selection.contains(row - view_offset, col) {
                    let sel_bg = parse_hex_color(&colors.selection_bg);
                    let sel_fg = parse_hex_color(&colors.selection_fg);
                    bg = sel_bg;
                    fg = sel_fg;
                }

                // 1. Background rectangle
                instances.push(GlyphInstance {
                    cell_pos: [px, py],
                    cell_size: [render_cw, ch],
                    atlas_uv_min: [0.0; 2],
                    atlas_uv_max: [0.0; 2],
                    fg_color: bg, // For background rect, use bg color
                    bg_color: bg,
                    mode: 0,
                    _pad: [0; 3],
                });

                // 2. Glyph (skip space and wide fillers; a space may still
                // carry combining marks that need rendering).
                if cell.wide_filler || (cell.ch == ' ' && cell.combining.is_none()) {
                    continue;
                }

                // T4-1: SGR style handling.
                // invisible: render only the background, no glyph.
                // dim: halve foreground intensity (xterm convention).
                // blink: real terminals flash on a timer; we approximate by
                //   rendering at half intensity so blinking text is at least
                //   visually distinct. A time-based on/off toggle is future work.
                if cell.attrs.invisible() {
                    continue;
                }
                let glyph_fg = if cell.attrs.dim() || cell.attrs.blink() {
                    [fg[0] * 0.5, fg[1] * 0.5, fg[2] * 0.5, fg[3]]
                } else {
                    fg
                };

                let shaped_glyphs = if let Some(cluster_tail) = &cell.combining {
                    let cluster = format!("{}{}", cell.ch, cluster_tail);
                    atlas.shape_cluster(&cluster, cell.attrs.bold(), cell.attrs.italic())
                } else if cell.ch != ' ' {
                    atlas
                        .get_or_rasterize(cell.ch, cell.attrs.bold(), cell.attrs.italic())
                        .into_iter()
                        .map(|region| ShapedGlyph {
                            region,
                            x_offset: 0.0,
                            y_offset: 0.0,
                            x_advance: 0.0,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                for glyph in shaped_glyphs {
                    let r = glyph.region;
                    let gx = px + glyph.x_offset + r.metrics.xmin as f32;
                    let gy = py + baseline as f32
                        - glyph.y_offset
                        - r.metrics.height as f32
                        - r.metrics.ymin as f32;

                    instances.push(GlyphInstance {
                        cell_pos: [gx, gy],
                        cell_size: [r.metrics.width as f32, r.metrics.height as f32],
                        atlas_uv_min: r.uv_min,
                        atlas_uv_max: r.uv_max,
                        fg_color: glyph_fg,
                        bg_color: bg,
                        mode: 1,
                        _pad: [0; 3],
                    });
                }

                // Extended underline styles: single, double, curly, dotted,
                // and dashed. Curly/dotted/dashed use lightweight geometry
                // approximations while retaining the requested distinction.
                let underline_color = glyph_fg;
                let mut add_underline = |x: f32, y: f32, width: f32, height: f32| {
                    instances.push(GlyphInstance {
                        cell_pos: [x, y],
                        cell_size: [width, height],
                        atlas_uv_min: [0.0; 2],
                        atlas_uv_max: [0.0; 2],
                        fg_color: underline_color,
                        bg_color: underline_color,
                        mode: 0,
                        _pad: [0; 3],
                    });
                };
                match cell.attrs.underline_style() {
                    1 => add_underline(px, py + ch - 2.0, render_cw, 2.0),
                    2 => {
                        add_underline(px, py + ch - 4.0, render_cw, 1.0);
                        add_underline(px, py + ch - 1.0, render_cw, 1.0);
                    }
                    3 => {
                        add_underline(px, py + ch - 3.0, render_cw * 0.5, 1.0);
                        add_underline(px + render_cw * 0.5, py + ch - 2.0, render_cw * 0.5, 1.0);
                    }
                    4 => {
                        add_underline(px, py + ch - 2.0, render_cw * 0.25, 2.0);
                        add_underline(px + render_cw * 0.5, py + ch - 2.0, render_cw * 0.25, 2.0);
                    }
                    5 => add_underline(px, py + ch - 2.0, render_cw * 0.6, 2.0),
                    _ => {}
                }

                // T4-1: strikethrough — thin rect through the vertical middle.
                if cell.attrs.strikethrough() {
                    instances.push(GlyphInstance {
                        cell_pos: [px, py + ch * 0.4],
                        cell_size: [cw, 2.0],
                        atlas_uv_min: [0.0; 2],
                        atlas_uv_max: [0.0; 2],
                        fg_color: glyph_fg,
                        bg_color: glyph_fg,
                        mode: 0,
                        _pad: [0; 3],
                    });
                }
            }
        }

        // Cursor (DECTCEM) - only render if cursor is enabled AND visible (blink).
        // When scrolled up, the live grid (and its cursor) shifts down by the
        // view offset; hide the cursor entirely if that pushes it off-screen.
        // DECSCUSR (T3-6): shape 0/1/2 = block, 3/4 = underline, 5/6 = bar.
        if grid.cursor_visible && cursor_visible {
            let col = grid.cursor.col.min(grid.cols - 1);
            let live_row = grid.cursor.row.min(grid.rows - 1);
            let screen_row = live_row + view_offset;
            if screen_row < grid.rows {
                let px = col as f32 * cw;
                let py = tab_bar_height + screen_row as f32 * ch;
                let cursor_color = parse_hex_color(&colors.cursor);
                // Geometry by DECSCUSR shape (0/1 fall back to block — the
                // app side already gates blinking via cursor_visible).
                let (rect_pos, rect_size) = match grid.cursor_shape {
                    3 | 4 => ([px, py + ch - 2.0], [cw, 2.0]), // underline
                    5 | 6 => ([px, py], [2.0, ch]),            // bar
                    _ => ([px, py], [cw, ch]),                 // block
                };
                instances.push(GlyphInstance {
                    cell_pos: rect_pos,
                    cell_size: rect_size,
                    atlas_uv_min: [0.0; 2],
                    atlas_uv_max: [0.0; 2],
                    fg_color: cursor_color,
                    bg_color: cursor_color,
                    mode: 0,
                    _pad: [0; 3],
                });
            }
        }

        if atlas.take_dirty() {
            self.upload_atlas(atlas);
        }

        // Upload instance buffer using double buffering
        let buf_idx = self.current_buffer;
        let inst_count = instances.len();
        let inst_bytes = bytemuck::cast_slice::<GlyphInstance, u8>(&instances);
        if inst_count > self.instance_capacity[buf_idx] {
            self.instance_bufs[buf_idx] = Some(self.device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("instance_buf"),
                    contents: inst_bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                },
            ));
            self.instance_capacity[buf_idx] = inst_count;
        } else if let Some(buf) = &self.instance_bufs[buf_idx] {
            self.queue.write_buffer(buf, 0, inst_bytes);
        }

        // Draw
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(e) => {
                log::warn!("surface error: {e:?}");
                self.instances = instances;
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let bg_color = parse_hex_color(&colors.background);
        let offscreen_load = if self.offscreen_initialized {
            wgpu::LoadOp::Load
        } else {
            wgpu::LoadOp::Clear(wgpu::Color {
                r: bg_color[0] as f64,
                g: bg_color[1] as f64,
                b: bg_color[2] as f64,
                a: bg_color[3] as f64,
            })
        };

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal_offscreen_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.offscreen_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: offscreen_load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, &self.bind_group, &[]);
            rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));

            if let Some(buf) = &self.instance_bufs[buf_idx] {
                rpass.set_vertex_buffer(1, buf.slice(..));
                rpass.draw(0..6, 0..inst_count as u32);
            }
        }

        // Composite the persistent terminal framebuffer into the swapchain.
        // The surface is always cleared; only the offscreen target relies on
        // preserved contents between frames.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal_composite_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg_color[0] as f64,
                            g: bg_color[1] as f64,
                            b: bg_color[2] as f64,
                            a: bg_color[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(&self.composite_pipeline);
            rpass.set_bind_group(0, &self.composite_bind_group, &[]);
            rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            rpass.draw(0..6, 0..1);
        }

        self.offscreen_initialized = true;
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        // Return CPU storage for reuse on the next frame.
        self.instances = instances;
        // Swap buffers for next frame (double buffering)
        self.current_buffer = 1 - self.current_buffer;
    }
}

#[cfg(test)]
mod tests {
    use super::needs_full_redraw;

    #[test]
    fn damage_path_requires_full_redraw_for_initial_frame_and_dynamic_overlays() {
        assert!(needs_full_redraw(
            false, false, false, false, false, false, 0, 100
        ));
        assert!(needs_full_redraw(
            true, false, true, false, false, false, 1, 100
        ));
        assert!(needs_full_redraw(
            true, false, false, true, false, false, 1, 100
        ));
        assert!(needs_full_redraw(
            true, false, false, false, true, false, 1, 100
        ));
    }

    #[test]
    fn damage_path_allows_small_terminal_only_updates() {
        assert!(!needs_full_redraw(
            true, false, false, false, false, false, 1, 100
        ));
        assert!(needs_full_redraw(
            true, false, false, false, false, false, 51, 100
        ));
        assert!(needs_full_redraw(
            true, false, false, false, false, true, 1, 100
        ));
    }
}
