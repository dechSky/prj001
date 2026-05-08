use bytemuck::{Pod, Zeroable};

use crate::grid::Term;

use super::atlas::GlyphAtlas;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct CellInstance {
    pub cell_xy: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub glyph_offset: [f32; 2],
    pub glyph_size: [f32; 2],
}

pub fn build_instances(term: &Term, atlas: &GlyphAtlas, baseline: f32) -> Vec<CellInstance> {
    let mut out = Vec::new();
    for r in 0..term.rows() {
        for c in 0..term.cols() {
            let cell = term.cell(r, c);
            if cell.ch == ' ' || (cell.ch as u32) < 0x20 {
                continue;
            }
            let Some(entry) = atlas.get(cell.ch) else {
                continue;
            };
            if entry.width == 0 || entry.height == 0 {
                continue;
            }
            out.push(CellInstance {
                cell_xy: [c as f32, r as f32],
                uv_min: entry.uv_min,
                uv_max: entry.uv_max,
                glyph_offset: [
                    entry.placement_left as f32,
                    baseline - entry.placement_top as f32,
                ],
                glyph_size: [entry.width as f32, entry.height as f32],
            });
        }
    }
    out
}
