struct Uniforms {
    viewport: vec2<f32>,
    cell: vec2<f32>,
    fg: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_smp: sampler;

struct VsIn {
    @builtin(vertex_index) vid: u32,
    @location(0) cell_xy: vec2<f32>,
    @location(1) uv_min: vec2<f32>,
    @location(2) uv_max: vec2<f32>,
    @location(3) glyph_offset: vec2<f32>,
    @location(4) glyph_size: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(input: VsIn) -> VsOut {
    let corner = vec2<f32>(
        f32((input.vid & 1u) != 0u),
        f32((input.vid >> 1u) != 0u),
    );
    let pixel = input.cell_xy * u.cell + input.glyph_offset + corner * input.glyph_size;
    let ndc = vec2<f32>(
        pixel.x / u.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / u.viewport.y * 2.0,
    );
    let uv = mix(input.uv_min, input.uv_max, corner);
    var o: VsOut;
    o.clip = vec4<f32>(ndc, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let alpha = textureSample(atlas_tex, atlas_smp, in.uv).r;
    return vec4<f32>(u.fg.rgb, u.fg.a * alpha);
}
