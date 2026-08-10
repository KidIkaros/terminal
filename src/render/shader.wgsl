// Vertex shader for terminal glyph rendering.
//
// Each instance encodes one terminal cell: screen position, UV into
// the glyph atlas, and fg/bg colors packed as RGBA.
//
// The vertex buffer is a unit quad [0,1]×[0,1]. The instance buffer
// scales and translates it to cell size and position.

struct VertexInput {
    @location(0) quad_pos: vec2<f32>,   // 0..1 unit square corner
}

struct InstanceInput {
    @location(1) cell_pos: vec2<f32>,    // pixel position of top-left corner
    @location(2) cell_size: vec2<f32>,   // pixel size of cell
    @location(3) atlas_uv_min: vec2<f32>, // UV top-left in atlas
    @location(4) atlas_uv_max: vec2<f32>, // UV bottom-right in atlas
    @location(5) fg_color: vec4<f32>,
    @location(6) bg_color: vec4<f32>,
    @location(7) mode: u32,              // 0 = bg rect, 1 = glyph alpha
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
    @location(3) @interpolate(flat) mode: u32,
}

struct Uniforms {
    viewport: vec2<f32>,  // width, height in pixels
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    // Map unit quad to cell rect in pixel space
    let px = instance.cell_pos + vertex.quad_pos * instance.cell_size;

    // NDC: (-1,-1) bottom-left, (1,1) top-right.  Screen y increases downward.
    let ndc = vec2<f32>(
        px.x / uniforms.viewport.x * 2.0 - 1.0,
        1.0 - px.y / uniforms.viewport.y * 2.0,
    );

    // Interpolate UV across the atlas sub-rect
    let uv = instance.atlas_uv_min + vertex.quad_pos * (instance.atlas_uv_max - instance.atlas_uv_min);

    var out: VertexOutput;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.fg_color = instance.fg_color;
    out.bg_color = instance.bg_color;
    out.mode = instance.mode;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.mode == 0u {
        // Background rectangle — solid color, no texture lookup
        return in.bg_color;
    } else {
        // Glyph — sample atlas (grayscale coverage in R channel)
        let coverage = textureSample(atlas_texture, atlas_sampler, in.uv).r;
        // Blend fg over bg using glyph alpha
        return mix(in.bg_color, in.fg_color, coverage);
    }
}
