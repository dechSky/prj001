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
    @location(5) fg: vec4<f32>,
    @location(6) bg: vec4<f32>,
    @location(7) cell_span: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) cell_pixel: vec2<f32>,
    @location(1) uv_min: vec2<f32>,
    @location(2) uv_max: vec2<f32>,
    @location(3) glyph_offset: vec2<f32>,
    @location(4) glyph_size: vec2<f32>,
    @location(5) fg: vec4<f32>,
    @location(6) bg: vec4<f32>,
};

@vertex
fn vs(input: VsIn) -> VsOut {
    let corner = vec2<f32>(
        f32((input.vid & 1u) != 0u),
        f32((input.vid >> 1u) != 0u),
    );
    let cell_size = vec2<f32>(u.cell.x * input.cell_span, u.cell.y);
    let cell_origin = input.cell_xy * u.cell;
    let pixel = cell_origin + corner * cell_size;
    let ndc = vec2<f32>(
        pixel.x / u.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / u.viewport.y * 2.0,
    );
    var o: VsOut;
    o.clip = vec4<f32>(ndc, 0.0, 1.0);
    o.cell_pixel = corner * cell_size;
    o.uv_min = input.uv_min;
    o.uv_max = input.uv_max;
    o.glyph_offset = input.glyph_offset;
    o.glyph_size = input.glyph_size;
    o.fg = input.fg;
    o.bg = input.bg;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var color = in.bg;
    if (in.glyph_size.x > 0.0 && in.glyph_size.y > 0.0) {
        let rel = in.cell_pixel - in.glyph_offset;
        if (rel.x >= 0.0 && rel.x < in.glyph_size.x
            && rel.y >= 0.0 && rel.y < in.glyph_size.y) {
            let glyph_uv01 = rel / in.glyph_size;
            let atlas_uv = mix(in.uv_min, in.uv_max, glyph_uv01);
            let alpha = textureSample(atlas_tex, atlas_smp, atlas_uv).r;
            color = mix(in.bg, in.fg, alpha);
        }
    }
    return color;
}
