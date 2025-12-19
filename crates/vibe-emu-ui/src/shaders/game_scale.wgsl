struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>,
}

struct VertexOutput {
    @location(0) tex_coord: vec2<f32>,
    @builtin(position) position: vec4<f32>,
}

struct Locals {
    // Half-size of destination rect in NDC units.
    scale: vec2<f32>,
    // Center of destination rect in NDC units.
    offset: vec2<f32>,
}

@group(0) @binding(0) var r_tex_color: texture_2d<f32>;
@group(0) @binding(1) var r_tex_sampler: sampler;
@group(0) @binding(2) var<uniform> r_locals: Locals;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coord = in.tex_coord;
    let pos = in.position * r_locals.scale + r_locals.offset;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(@location(0) tex_coord: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(r_tex_color, r_tex_sampler, tex_coord);
}
