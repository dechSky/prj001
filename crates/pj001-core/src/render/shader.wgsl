struct Uniforms {
    viewport: vec2<f32>,
    cell: vec2<f32>,
    fg: vec4<f32>,
    palette_bg: vec4<f32>,
    marker_kind: u32,
    bg_opacity: f32,
    /// Visual Bell flash intensity (0.0=normal, 1.0=fully inverted).
    bell_flash: f32,
    _pad0: u32,
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
    @location(8) flags: u32,
    @location(9) block_border_color: vec4<f32>,
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
    // 정수 type은 fragment로 보낼 때 flat interpolation 필수.
    @location(7) @interpolate(flat) flags: u32,
    @location(8) cell_span: f32,
    @location(9) block_border_color: vec4<f32>,
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
    o.flags = input.flags;
    o.cell_span = input.cell_span;
    o.block_border_color = input.block_border_color;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // M7-5: cursor overlay 분기 — shape 외 영역은 discard, 영역 안은 reverse(fg/bg swap)된
    // 일반 cell처럼 글리프 + 배경을 함께 그림.
    if ((in.flags & 1u) != 0u) {
        let shape_bits = (in.flags >> 1u) & 3u;
        let focused = (in.flags & 8u) != 0u;
        let cell_w = u.cell.x * in.cell_span;
        let cell_h = u.cell.y;
        let underscore_thick = max(2.0, cell_h * 0.12);
        // bar 두께는 일반 텍스트 caret 수준으로 slim. cell_w*0.10이고 1~2px floor.
        let bar_thick = max(1.0, min(2.0, cell_w * 0.10));
        let outline_w = max(1.0, cell_w * 0.05);
        let outline_h = max(1.0, cell_h * 0.05);
        var in_shape = false;
        if (focused) {
            if (shape_bits == 0u) {
                in_shape = true;
            } else if (shape_bits == 1u) {
                in_shape = in.cell_pixel.y >= cell_h - underscore_thick;
            } else if (shape_bits == 2u) {
                in_shape = in.cell_pixel.x <= bar_thick;
            }
        } else {
            in_shape = in.cell_pixel.x < outline_w
                || in.cell_pixel.x > cell_w - outline_w
                || in.cell_pixel.y < outline_h
                || in.cell_pixel.y > cell_h - outline_h;
        }
        if (!in_shape) {
            discard;
        }
        // shape 영역 안: reversed 색으로 글리프 + bg.
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

    // Phase 5: SCROLLBAR_THUMB path. cell 우측 가장자리 약 3px만 thumb 색, 나머지는
    // palette_bg (cell content 안 그리고 단순 thin band).
    // Phase 3 step 3 (Codex 권 2): thumb 외 영역 palette_bg를 bg_opacity로 곱해 vibrancy
    // 통과. thumb 자체는 in.bg 그대로 (가독성 위해).
    if ((in.flags & 0x400u) != 0u) {
        let cell_w = u.cell.x * in.cell_span;
        let thumb_w = max(2.0, cell_w * 0.30);
        if (in.cell_pixel.x >= cell_w - thumb_w) {
            return in.bg;
        }
        return vec4<f32>(u.palette_bg.rgb, u.palette_bg.a * u.bg_opacity);
    }

    // Phase 4b-2c-4b: BLOCK_CARD SDF path. bit 0x10 set이면 카드 cell —
    // edge bit(0x20 top / 0x40 bottom / 0x80 left / 0x100 right)으로 border band 판정,
    // corner(두 edge 동시 set)는 SDF로 곡선 그리기. corner 곡선의 외부는 palette_bg
    // (clear color 그대로 보이게 explicit write).
    if ((in.flags & 0x10u) != 0u) {
        let cell_w = u.cell.x * in.cell_span;
        let cell_h = u.cell.y;
        let top = (in.flags & 0x20u) != 0u;
        let bottom = (in.flags & 0x40u) != 0u;
        let left = (in.flags & 0x80u) != 0u;
        let right = (in.flags & 0x100u) != 0u;
        let radius = min(cell_w, cell_h) * 0.4;
        let border_w = max(1.5, cell_h * 0.05);

        let h_corner = top || bottom;
        let v_corner = left || right;
        let is_corner_cell = h_corner && v_corner;

        var card_color = in.bg;
        var on_border = false;
        var outside_card = false;

        if (is_corner_cell) {
            // corner의 곡선 center 위치 (cell 내부 좌표).
            var cx: f32 = radius;
            var cy: f32 = radius;
            if (right) { cx = cell_w - radius; }
            if (bottom) { cy = cell_h - radius; }
            let dx = in.cell_pixel.x - cx;
            let dy = in.cell_pixel.y - cy;
            // cell 모서리 쪽 outside quadrant 판정.
            let out_quad_x = (left && in.cell_pixel.x < cx) || (right && in.cell_pixel.x > cx);
            let out_quad_y = (top && in.cell_pixel.y < cy) || (bottom && in.cell_pixel.y > cy);

            if (out_quad_x && out_quad_y) {
                // corner 곡선 영역
                let dist = sqrt(dx * dx + dy * dy);
                if (dist > radius) {
                    outside_card = true;
                } else if (dist > radius - border_w) {
                    on_border = true;
                }
            } else {
                // corner cell의 inner edge (다른 한 방향이 edge band)
                if ((top && in.cell_pixel.y < border_w && !out_quad_x)
                    || (bottom && in.cell_pixel.y > cell_h - border_w && !out_quad_x)
                    || (left && in.cell_pixel.x < border_w && !out_quad_y)
                    || (right && in.cell_pixel.x > cell_w - border_w && !out_quad_y)) {
                    on_border = true;
                }
            }
        } else {
            // non-corner edge cell: 단순 border band.
            if ((top && in.cell_pixel.y < border_w)
                || (bottom && in.cell_pixel.y > cell_h - border_w)
                || (left && in.cell_pixel.x < border_w)
                || (right && in.cell_pixel.x > cell_w - border_w)) {
                on_border = true;
            }
        }

        if (outside_card) {
            // Phase 3 step 3 (Codex 권 3): rounded corner 바깥도 vibrancy 통과.
            return vec4<f32>(u.palette_bg.rgb, u.palette_bg.a * u.bg_opacity);
        }
        if (on_border) {
            card_color = in.block_border_color;
        }

        // Phase 4b-3 + 4d: prompt marker — theme별 SDF shape (uniform marker_kind 분기).
        // 0=RoundedSquare, 1=Hex, 2=Dollar (cross of bars), 3=RunChip, 4=Bubble.
        if ((in.flags & 0x200u) != 0u) {
            let cx = cell_w * 0.5;
            let cy = cell_h * 0.5;
            let marker_half = min(cell_w, cell_h) * 0.4;
            let dx_signed = in.cell_pixel.x - cx;
            let dy_signed = in.cell_pixel.y - cy;
            let dx = abs(dx_signed);
            let dy = abs(dy_signed);
            var hit = false;
            switch u.marker_kind {
                case 1u: {
                    // Hex (pointy-top): SDF for regular hexagon. 단순 근사.
                    let k = vec3<f32>(-0.866025404, 0.5, 0.57735);
                    var px = dx;
                    var py = dy;
                    let t = 2.0 * min(k.x * px + k.y * py, 0.0);
                    px -= t * k.x;
                    py -= t * k.y;
                    let clamp_x = clamp(px, -k.z * marker_half, k.z * marker_half);
                    let len_x = px - clamp_x;
                    let len_y = py - marker_half;
                    let d = sqrt(len_x * len_x + len_y * len_y) * sign(py - marker_half);
                    hit = d < 0.0;
                }
                case 2u: {
                    // Dollar: 수직 막대 + 가로 가운데 줄(단순화된 $ 형태)
                    let bar_w = max(1.5, marker_half * 0.25);
                    let v_bar = dx < bar_w && dy < marker_half;
                    let h_bar = dy < bar_w && dx < marker_half * 0.8;
                    hit = v_bar || h_bar;
                }
                case 3u: {
                    // RunChip: square chip (rounded square, 더 큰 라운드 X).
                    hit = dx < marker_half && dy < marker_half * 0.8;
                }
                case 4u: {
                    // Bubble: circle + outer ring 두 톤. 안쪽만 fg, ring은 fg dim.
                    let dist = sqrt(dx_signed * dx_signed + dy_signed * dy_signed);
                    hit = dist < marker_half;
                    if (hit && dist > marker_half * 0.7) {
                        // ring 부분 — dim fg
                        card_color = mix(in.bg, in.fg, 0.6);
                        // glyph layer 건너뛰기 위해 직접 return
                        return card_color;
                    }
                }
                default: {
                    // RoundedSquare (0 또는 unknown)
                    let marker_radius = max(1.5, marker_half * 0.25);
                    let qx = max(dx - (marker_half - marker_radius), 0.0);
                    let qy = max(dy - (marker_half - marker_radius), 0.0);
                    let m_dist = sqrt(qx * qx + qy * qy) - marker_radius;
                    hit = m_dist < 0.0;
                }
            }
            if (hit) {
                card_color = in.fg;
            }
        }

        // glyph layer
        if (in.glyph_size.x > 0.0 && in.glyph_size.y > 0.0) {
            let rel = in.cell_pixel - in.glyph_offset;
            if (rel.x >= 0.0 && rel.x < in.glyph_size.x
                && rel.y >= 0.0 && rel.y < in.glyph_size.y) {
                let glyph_uv01 = rel / in.glyph_size;
                let atlas_uv = mix(in.uv_min, in.uv_max, glyph_uv01);
                let alpha = textureSample(atlas_tex, atlas_smp, atlas_uv).r;
                card_color = mix(card_color, in.fg, alpha);
            }
        }
        return card_color;
    }

    // 일반 cell 렌더 (기존).
    // Phase 3 step 3: cell.bg.a를 u.bg_opacity로 곱해 NSVisualEffectView 뒤에 비치게,
    // glyph가 그려진 비율만큼 alpha를 1.0으로 끌어올려 텍스트 가독성 유지.
    var color = in.bg;
    var out_alpha = color.a * u.bg_opacity;
    if (in.glyph_size.x > 0.0 && in.glyph_size.y > 0.0) {
        let rel = in.cell_pixel - in.glyph_offset;
        if (rel.x >= 0.0 && rel.x < in.glyph_size.x
            && rel.y >= 0.0 && rel.y < in.glyph_size.y) {
            let glyph_uv01 = rel / in.glyph_size;
            let atlas_uv = mix(in.uv_min, in.uv_max, glyph_uv01);
            let alpha = textureSample(atlas_tex, atlas_smp, atlas_uv).r;
            color = mix(in.bg, in.fg, alpha);
            out_alpha = mix(out_alpha, color.a, alpha);
        }
    }
    // Visual Bell flash — bell_flash > 0이면 cell 색을 inverted color로 점진 mix.
    // 250ms 동안 1.0 → 0.0 fade out (WindowState가 elapsed로 set_bell_flash 호출).
    if (u.bell_flash > 0.0) {
        let inverted = vec3<f32>(1.0 - color.r, 1.0 - color.g, 1.0 - color.b);
        color = vec4<f32>(mix(color.rgb, inverted, u.bell_flash), color.a);
        // flash 중에는 alpha를 1.0으로 끌어올려 vibrancy 통과 없이 강한 시각.
        out_alpha = mix(out_alpha, 1.0, u.bell_flash);
    }
    return vec4<f32>(color.rgb, out_alpha);
}
