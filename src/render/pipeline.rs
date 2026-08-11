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
use crate::render::font::{ATLAS_SIZE, GlyphAtlas};
use crate::selection::Selection;
use crate::tab_bar::TabBar;

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

/// Parse a hex color string (#RRGGBB or #RRGGBBAA) into [f32; 4].
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
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]
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
            table.push([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]);
        }
    }

    // Fill remaining entries from palette
    for i in 16..256 {
        let idx = i as u8;
        let (r, g, b) = palette.get_color(idx).unwrap_or((0, 0, 0));
        table.push([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]);
    }

    table
}

/// Convert a terminal Color to RGBA, using config colors for defaults.
fn color_to_rgba(color: Color, is_fg: bool, config: &ColorConfig, palette: &ColorPalette) -> [f32; 4] {
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
                [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
            } else {
                let table = build_color_table(config, palette);
                let [r, g, b] = table[i as usize];
                [r, g, b, 1.0]
            }
        }
        Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
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

    /// Unit square vertex buffer (6 vertices, two triangles).
    vertex_buf: wgpu::Buffer,

    /// Double-buffered instance buffers
    instance_bufs: [Option<wgpu::Buffer>; 2],
    instance_capacity: [usize; 2],
    current_buffer: usize,
}

impl TerminalPipeline {
    pub async fn new(
        window: Arc<winit::window::Window>,
        atlas: &GlyphAtlas,
        vsync: bool,
    ) -> Self {
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
                wgpu::PresentMode::Fifo  // VSync enabled
            } else {
                wgpu::PresentMode::Immediate  // No VSync (tearing possible)
            },
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // ---- Atlas texture ----
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
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
            wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
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
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() },
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
                wgpu::VertexAttribute { offset: 0,  shader_location: 1, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 8,  shader_location: 2, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 16, shader_location: 3, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 24, shader_location: 4, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 32, shader_location: 5, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 48, shader_location: 6, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 64, shader_location: 7, format: wgpu::VertexFormat::Uint32 },
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
            vertex_buf,
            instance_bufs: [None, None],
            instance_capacity: [0, 0],
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

        let uniforms = Uniforms { viewport: [width as f32, height as f32], _pad: [0.0; 2] };
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
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
            wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
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
        let region = atlas.get_or_rasterize(ch, false, false)
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

    pub fn render(&mut self, params: RenderParams<'_>) {
        let RenderParams { grid, atlas, cursor_visible, colors, selection, tab_bar, tab_bar_height } = params;
        let cw = atlas.cell_width as f32;
        let ch = atlas.cell_height as f32;
        let baseline = atlas.baseline;

        // Partial dirty tracking with LoadOp::Clear doesn't work: the whole
        // frame is cleared, so unchanged cells would not be re-rendered. Render
        // all cells every frame for correctness (dirty tracking can be
        // re-introduced later with LoadOp::Load).
        let _ = grid.take_dirty_cells(); // clear flags; kept for future use

        // Build instance list — tab bar rects first, then all terminal cells + cursor
        let mut instances: Vec<GlyphInstance> = Vec::with_capacity(grid.rows * grid.cols * 2 + 1);

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
                let color = if tab.active { active_color } else { inactive_color };
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
                        let gy = text_y + baseline as f32 - r.metrics.height as f32 - r.metrics.ymin as f32;
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
                    Self::add_glyph_button(&mut instances, atlas, baseline, '×', cx, cy, cw_btn, ch_btn, text_color, color);
                }
            }

            // New tab (+) and search (S) buttons, right-aligned
            let (new_x, new_y, new_w, new_h) = tb.new_tab_button_rect(self.surface_config.width);
            Self::add_glyph_button(&mut instances, atlas, baseline, '+', new_x, new_y, new_w, new_h, text_color, header_color);

            let (search_x, search_y, search_w, search_h) = tb.search_button_rect(self.surface_config.width);
            Self::add_glyph_button(&mut instances, atlas, baseline, 'S', search_x, search_y, search_w, search_h, text_color, header_color);
        }

        for row in 0..grid.rows {
            for col in 0..grid.cols {
                let cell = grid.cell(col, row);
                let px = col as f32 * cw;
                let py = tab_bar_height + row as f32 * ch;

            let fg_raw = color_to_rgba(cell.fg, true, colors, &grid.palette);
            let bg_raw = color_to_rgba(cell.bg, false, colors, &grid.palette);

            let (mut fg, mut bg) = if cell.attrs.inverse {
                (bg_raw, fg_raw)
            } else {
                (fg_raw, bg_raw)
            };

            // Apply selection highlighting
            if selection.contains(row, col) {
                let sel_bg = parse_hex_color(&colors.selection_bg);
                let sel_fg = parse_hex_color(&colors.selection_fg);
                bg = sel_bg;
                fg = sel_fg;
            }

            // 1. Background rectangle
            instances.push(GlyphInstance {
                cell_pos: [px, py],
                cell_size: [cw, ch],
                atlas_uv_min: [0.0; 2],
                atlas_uv_max: [0.0; 2],
                fg_color: bg,  // For background rect, use bg color
                bg_color: bg,
                mode: 0,
                _pad: [0; 3],
            });

            // 2. Glyph (skip space and wide fillers)
            if cell.wide_filler || cell.ch == ' ' {
                continue;
            }

            let region = atlas.get_or_rasterize(cell.ch, cell.attrs.bold, cell.attrs.italic);
            if let Some(r) = region {
                // Position the glyph within the cell
                let gx = px + r.metrics.xmin as f32;
                let gy = py + baseline as f32 - r.metrics.height as f32 - r.metrics.ymin as f32;

                instances.push(GlyphInstance {
                    cell_pos: [gx, gy],
                    cell_size: [r.metrics.width as f32, r.metrics.height as f32],
                    atlas_uv_min: r.uv_min,
                    atlas_uv_max: r.uv_max,
                    fg_color: fg,
                    bg_color: bg,
                    mode: 1,
                    _pad: [0; 3],
                });
            }
            }
        }

        // Cursor block (DECTCEM) - only render if cursor is enabled AND visible (blink)
        if grid.cursor_visible && cursor_visible {
            let col = grid.cursor.col.min(grid.cols - 1);
            let row = grid.cursor.row.min(grid.rows - 1);
            let px = col as f32 * cw;
            let py = tab_bar_height + row as f32 * ch;
            let cursor_color = parse_hex_color(&colors.cursor);
            instances.push(GlyphInstance {
                cell_pos: [px, py + ch - 2.0],
                cell_size: [cw, 2.0],
                atlas_uv_min: [0.0; 2],
                atlas_uv_max: [0.0; 2],
                fg_color: cursor_color,
                bg_color: cursor_color,
                mode: 0,
                _pad: [0; 3],
            });
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
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });

        let bg_color = parse_hex_color(&colors.background);

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal_pass"),
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

            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, &self.bind_group, &[]);
            rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));

            if let Some(buf) = &self.instance_bufs[buf_idx] {
                rpass.set_vertex_buffer(1, buf.slice(..));
                rpass.draw(0..6, 0..inst_count as u32);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        
        // Swap buffers for next frame (double buffering)
        self.current_buffer = 1 - self.current_buffer;
    }
}
