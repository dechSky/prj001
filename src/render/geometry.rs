use bytemuck::{Pod, Zeroable};

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
}

const FG_DEFAULT: [f32; 4] = [0.86, 0.86, 0.86, 1.0];
const BG_DEFAULT: [f32; 4] = [0.05, 0.05, 0.07, 1.0];

pub fn build_instances(term: &Term, atlas: &GlyphAtlas, baseline: f32) -> Vec<CellInstance> {
    let mut out = Vec::new();
    for r in 0..term.rows() {
        for c in 0..term.cols() {
            let cell = term.cell(r, c);

            let (fg, bg) = if cell.attrs.contains(Attrs::REVERSE) {
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
            if entry.is_none() && bg_is_default {
                continue;
            }

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
            });
        }
    }
    out
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
