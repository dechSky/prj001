use bytemuck::{Pod, Zeroable};
use unicode_width::UnicodeWidthChar;

use crate::grid::{Attrs, Color, Term};

use super::atlas::GlyphAtlas;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct CellInstance {
    pub cell_xy: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub glyph_offset: [f32; 2],
    pub glyph_size: [f32; 2],
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub cell_span: f32,
    pub _pad: [f32; 3],
}

const FG_DEFAULT: [f32; 4] = [0.86, 0.86, 0.86, 1.0];
const BG_DEFAULT: [f32; 4] = [0.05, 0.05, 0.07, 1.0];

pub fn build_instances(
    term: &Term,
    atlas: &GlyphAtlas,
    baseline: f32,
    cursor: Option<(usize, usize)>,
) -> Vec<CellInstance> {
    let mut out = Vec::new();
    for r in 0..term.rows() {
        for c in 0..term.cols() {
            let cell = term.cell(r, c);
            if cell.attrs.contains(Attrs::WIDE_CONT) {
                continue;
            }

            let is_cursor = cursor == Some((r, c));
            // cursor는 cell의 REVERSE flag 토글과 동등한 효과.
            let reversed = cell.attrs.contains(Attrs::REVERSE) ^ is_cursor;
            let (fg, bg) = if reversed {
                (resolve(cell.bg, false), resolve(cell.fg, true))
            } else {
                (resolve(cell.fg, true), resolve(cell.bg, false))
            };

            let entry = if cell.ch == ' ' || (cell.ch as u32) < 0x20 {
                None
            } else {
                atlas
                    .get(cell.ch)
                    .filter(|e| e.width > 0 && e.height > 0)
            };

            let bg_is_default = bg == BG_DEFAULT;
            if !is_cursor && entry.is_none() && bg_is_default {
                continue;
            }

            let cell_span = if cell.attrs.contains(Attrs::WIDE) {
                2.0
            } else {
                1.0
            };

            let (uv_min, uv_max, glyph_offset, glyph_size) = if let Some(e) = entry {
                (
                    e.uv_min,
                    e.uv_max,
                    [e.placement_left as f32, baseline - e.placement_top as f32],
                    [e.width as f32, e.height as f32],
                )
            } else {
                ([0.0; 2], [0.0; 2], [0.0; 2], [0.0; 2])
            };

            out.push(CellInstance {
                cell_xy: [c as f32, r as f32],
                uv_min,
                uv_max,
                glyph_offset,
                glyph_size,
                fg,
                bg,
                cell_span,
                _pad: [0.0; 3],
            });
        }
    }
    out
}

/// IME preedit overlay. Term grid는 건드리지 않고 cursor (start_col, start_row) 위치부터
/// preedit string을 dim된 fg로 그려넣는다. cell 경계 넘어가면 truncate.
pub fn build_preedit_instances(
    preedit: &str,
    start_col: usize,
    start_row: usize,
    cols: usize,
    atlas: &GlyphAtlas,
    baseline: f32,
) -> Vec<CellInstance> {
    let mut out = Vec::new();
    let mut col = start_col;
    let dim_fg = mix(FG_DEFAULT, BG_DEFAULT, 0.4);
    for ch in preedit.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(1);
        if w == 0 {
            continue;
        }
        if col + w > cols {
            break;
        }
        let entry = if ch == ' ' || (ch as u32) < 0x20 {
            None
        } else {
            atlas.get(ch).filter(|e| e.width > 0 && e.height > 0)
        };
        let cell_span = if w == 2 { 2.0 } else { 1.0 };
        let (uv_min, uv_max, glyph_offset, glyph_size) = if let Some(e) = entry {
            (
                e.uv_min,
                e.uv_max,
                [e.placement_left as f32, baseline - e.placement_top as f32],
                [e.width as f32, e.height as f32],
            )
        } else {
            ([0.0; 2], [0.0; 2], [0.0; 2], [0.0; 2])
        };
        out.push(CellInstance {
            cell_xy: [col as f32, start_row as f32],
            uv_min,
            uv_max,
            glyph_offset,
            glyph_size,
            fg: dim_fg,
            bg: BG_DEFAULT,
            cell_span,
            _pad: [0.0; 3],
        });
        col += w;
    }
    out
}

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] * (1.0 - t) + b[0] * t,
        a[1] * (1.0 - t) + b[1] * t,
        a[2] * (1.0 - t) + b[2] * t,
        a[3] * (1.0 - t) + b[3] * t,
    ]
}

fn resolve(c: Color, is_fg: bool) -> [f32; 4] {
    match c {
        Color::Default => {
            if is_fg {
                FG_DEFAULT
            } else {
                BG_DEFAULT
            }
        }
        Color::Indexed(n) => indexed(n),
        Color::Rgb(r, g, b) => [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            1.0,
        ],
    }
}

fn indexed(n: u8) -> [f32; 4] {
    if n < 16 {
        ANSI_16[n as usize]
    } else if n < 232 {
        let i = n - 16;
        let r = i / 36;
        let g = (i % 36) / 6;
        let b = i % 6;
        [cube(r), cube(g), cube(b), 1.0]
    } else {
        let v = (8 + (n - 232) as u32 * 10) as f32 / 255.0;
        [v, v, v, 1.0]
    }
}

fn cube(x: u8) -> f32 {
    if x == 0 {
        0.0
    } else {
        (55.0 + x as f32 * 40.0) / 255.0
    }
}

const ANSI_16: [[f32; 4]; 16] = [
    [0.0, 0.0, 0.0, 1.0],
    [0.502, 0.0, 0.0, 1.0],
    [0.0, 0.502, 0.0, 1.0],
    [0.502, 0.502, 0.0, 1.0],
    [0.0, 0.0, 0.502, 1.0],
    [0.502, 0.0, 0.502, 1.0],
    [0.0, 0.502, 0.502, 1.0],
    [0.753, 0.753, 0.753, 1.0],
    [0.502, 0.502, 0.502, 1.0],
    [1.0, 0.0, 0.0, 1.0],
    [0.0, 1.0, 0.0, 1.0],
    [1.0, 1.0, 0.0, 1.0],
    [0.0, 0.0, 1.0, 1.0],
    [1.0, 0.0, 1.0, 1.0],
    [0.0, 1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0, 1.0],
];
