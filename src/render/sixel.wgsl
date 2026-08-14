// Sixel image shader — draws a decoded sixel RGBA texture over the cell
// quad it occupies. Uses the same vertex/instance layout as the terminal
// shader (unit quad + GlyphInstance) so the same vertex buffers work; only
// the fragment sampling differs (full-image texture instead of atlas glyph).

struct VertexInput {
    @location(0) quad_pos: vec2<f32>, // 0..1 unit square corner
}

struct InstanceInput {
    @location(1) cell_pos: vec2<f32>,    // pixel position of top-left corner
    @location(2) cell_size: vec2<f32>,   // pixel size of the quad
    @location(3) uv_min: vec2<f32>,      // texture UV top-left
    @location(4) uv_max: vec2<f32>,      // texture UV bottom-right
    @location(5) fg_color: vec4<f32>,    // unused
    @location(6) bg_color: vec4<f32>,    // unused
    @location(7) mode: u32,              // unused
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct Uniforms {
    viewport: vec2<f32>, // width, height in pixels
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var image_texture: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    // Map unit quad to the image rect in pixel space.
    let px = instance.cell_pos + vertex.quad_pos * instance.cell_size;

    // NDC: (-1,-1) bottom-left, (1,1) top-right. Screen y increases downward.
    let ndc = vec2<f32>(
        px.x / uniforms.viewport.x * 2.0 - 1.0,
        1.0 - px.y / uniforms.viewport.y * 2.0,
    );

    var out: VertexOutput;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = instance.uv_min + vertex.quad_pos * (instance.uv_max - instance.uv_min);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Straight-alpha image; the pipeline's alpha blending composites it over
    // the terminal content. The sRGB texture format decodes to linear here.
    return textureSample(image_texture, image_sampler, in.uv);
}
