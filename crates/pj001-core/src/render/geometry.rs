use bytemuck::{Pod, Zeroable};
use unicode_width::UnicodeWidthChar;

use crate::grid::{Attrs, Color, CursorShape, Term};

use super::atlas::GlyphAtlas;
use super::theme::ThemePalette;

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
    /// M7-5 flags bitfield:
    /// - bit 0 (0x01): cursor overlay instance 표시
    /// - bit 1-2 (0x06): cursor shape (0=Block, 1=Underscore, 2=Bar)
    /// - bit 3 (0x08): focused (1=focused 일반 shape, 0=outline)
    pub flags: u32,
    pub _pad: [f32; 2],
}

#[derive(Clone, Copy)]
pub struct CursorRender {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub focused: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

/// Phase 4b-2b: 카드 bg overlay 정보. row range[start..=end] 안 cell의 bg를 약간 변경.
/// Phase 4b-2c-1: border_color 추가 — row range의 top/bottom row + col 0/cols-1 edge cell의
/// bg를 border_color로 stamp. cell 단위 border (1 cell 두께). radius 없음 — 4b-2c-2에서 SDF.
/// gutter 영역 카드 bg 확장은 별도 sub-step (visible_blocks가 row range만 보유).
#[derive(Clone, Copy, Debug)]
pub struct BlockOverlay {
    pub visible_row_start: usize,
    pub visible_row_end: usize,
    pub bg: [f32; 4],
    pub border_color: [f32; 4],
}

impl BlockOverlay {
    /// row range 안 cell이면 색 반환, 아니면 None. edge cell(top/bottom row, 좌우 끝 col)은
    /// border_color, 안쪽 cell은 bg.
    pub fn cell_color(&self, row: usize, col: usize, cols: usize) -> Option<[f32; 4]> {
        if row < self.visible_row_start || row > self.visible_row_end {
            return None;
        }
        let is_edge = row == self.visible_row_start
            || row == self.visible_row_end
            || col == 0
            || col + 1 == cols;
        Some(if is_edge { self.border_color } else { self.bg })
    }
}

pub fn build_instances_at(
    term: &Term,
    atlas: &GlyphAtlas,
    baseline: f32,
    cursor: Option<CursorRender>,
    selection: Option<SelectionRange>,
    col_offset: usize,
    row_offset: usize,
    palette: &ThemePalette,
    block_overlays: &[BlockOverlay],
) -> Vec<CellInstance> {
    let mut out = Vec::new();
    for r in 0..term.rows() {
        for c in 0..term.cols() {
            let cell = term.cell(r, c);
            if cell.attrs.contains(Attrs::WIDE_CONT) {
                continue;
            }

            // cursor 위치 cell도 다른 cell과 동일하게 main 렌더. cursor overlay instance가
            // 그 위에 별도로 shape 영역만 덮음 (Model A — cursor-design.md §5.0).
            let reversed = cell.attrs.contains(Attrs::REVERSE);
            let selected = selection.is_some_and(|selection| selection.contains(r, c));
            let hyperlink = cell.attrs.contains(Attrs::HYPERLINK);
            // Phase 4b-2b: cell이 어떤 block overlay 범위에 속하면 bg는 overlay.bg로 대체.
            // Phase 4b-2c-1: row range edge cell이면 border_color로 대체 (cell 단위 border).
            // selection > reversed > hyperlink > block overlay > default.
            let block_bg = block_overlays
                .iter()
                .find_map(|b| b.cell_color(r, c, term.cols()));
            let (fg, bg) = if selected {
                (palette.fg, palette.selection_bg)
            } else if reversed {
                (
                    resolve(cell.bg, false, palette),
                    resolve(cell.fg, true, palette),
                )
            } else if hyperlink {
                // 슬라이스 6.3b: hyperlink cells는 theme의 ANSI 12(밝은 파랑) — bg는 일반대로.
                (
                    palette.ansi[12],
                    block_bg.unwrap_or_else(|| resolve(cell.bg, false, palette)),
                )
            } else {
                let fg = resolve(cell.fg, true, palette);
                let bg = block_bg.unwrap_or_else(|| resolve(cell.bg, false, palette));
                (fg, bg)
            };

            let entry = if cell.ch == ' ' || (cell.ch as u32) < 0x20 {
                None
            } else {
                atlas.get(cell.ch).filter(|e| e.width > 0 && e.height > 0)
            };

            let bg_is_default = bg == palette.bg;
            if entry.is_none() && bg_is_default && !selected {
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
                cell_xy: [(c + col_offset) as f32, (r + row_offset) as f32],
                uv_min,
                uv_max,
                glyph_offset,
                glyph_size,
                fg,
                bg,
                cell_span,
                flags: 0,
                _pad: [0.0; 2],
            });
        }
    }

    // cursor overlay instance — 끝에 push해서 마지막에 그려짐. main instance의 reverse 버전.
    // shader에서 shape 외 영역은 discard로 main이 그대로 보이고, shape 영역만 reverse 적용.
    if let Some(cur) = cursor {
        // M9-2: cursor 위치 cell이 WIDE면 cursor도 2 cell 차지. WIDE_CONT(짝 cell) 위에
        // cursor가 있으면 한 cell 왼쪽으로 보정해서 WIDE 본체 위로 정렬.
        let (cur_row, cur_col) =
            if term.cell(cur.row, cur.col).attrs.contains(Attrs::WIDE_CONT) && cur.col > 0 {
                (cur.row, cur.col - 1)
            } else {
                (cur.row, cur.col)
            };
        let cell = term.cell(cur_row, cur_col);
        let cur_span = if cell.attrs.contains(Attrs::WIDE) {
            2.0
        } else {
            1.0
        };
        let (orig_fg, orig_bg) = if cell.attrs.contains(Attrs::REVERSE) {
            (
                resolve(cell.bg, false, palette),
                resolve(cell.fg, true, palette),
            )
        } else {
            (
                resolve(cell.fg, true, palette),
                resolve(cell.bg, false, palette),
            )
        };
        // reverse: overlay에서 fg ↔ bg swap.
        let overlay_fg = orig_bg;
        let overlay_bg = orig_fg;
        // 글리프 정보도 함께 (shape 영역 안에서 글리프가 reversed 색으로 보이도록).
        let entry = if cell.ch == ' ' || (cell.ch as u32) < 0x20 {
            None
        } else {
            atlas.get(cell.ch).filter(|e| e.width > 0 && e.height > 0)
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
        let shape_bits: u32 = match cur.shape {
            CursorShape::Block => 0,
            CursorShape::Underscore => 1,
            CursorShape::Bar => 2,
        };
        let mut flags: u32 = 0x01;
        flags |= shape_bits << 1;
        if cur.focused {
            flags |= 0x08;
        }
        out.push(CellInstance {
            cell_xy: [(cur_col + col_offset) as f32, (cur_row + row_offset) as f32],
            uv_min,
            uv_max,
            glyph_offset,
            glyph_size,
            fg: overlay_fg,
            bg: overlay_bg,
            cell_span: cur_span,
            flags,
            _pad: [0.0; 2],
        });
    }

    out
}

impl SelectionRange {
    /// Phase 2: caret 모델. `anchor`/`head`는 caret 좌표 (row, caret_col).
    /// caret_col은 글자 사이 위치 (0..=cols). cell N의 왼쪽 boundary = caret N,
    /// cell N의 오른쪽 boundary = caret N+1. 표준 macOS Terminal/iTerm2 모델.
    pub fn new(anchor: (usize, usize), head: (usize, usize)) -> Self {
        if anchor <= head {
            Self {
                start: anchor,
                end: head,
            }
        } else {
            Self {
                start: head,
                end: anchor,
            }
        }
    }

    /// cell (row, col)이 selection에 포함되는지. caret 모델 half-open [start, end):
    /// row == start.row이면 col >= start.col, row == end.row이면 col < end.col.
    /// start == end (caret 동일)이면 empty range.
    fn contains(self, row: usize, col: usize) -> bool {
        let after_start = row > self.start.0 || (row == self.start.0 && col >= self.start.1);
        let before_end = row < self.end.0 || (row == self.end.0 && col < self.end.1);
        after_start && before_end
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockOverlay, SelectionRange};

    const CARD_BG: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
    const CARD_BORDER: [f32; 4] = [0.3, 0.3, 0.4, 1.0];

    fn overlay(start: usize, end: usize) -> BlockOverlay {
        BlockOverlay {
            visible_row_start: start,
            visible_row_end: end,
            bg: CARD_BG,
            border_color: CARD_BORDER,
        }
    }

    #[test]
    fn block_overlay_cell_color_outside_range_none() {
        let o = overlay(2, 5);
        assert!(o.cell_color(0, 3, 10).is_none());
        assert!(o.cell_color(1, 3, 10).is_none());
        assert!(o.cell_color(6, 3, 10).is_none());
    }

    #[test]
    fn block_overlay_cell_color_inside_uses_bg() {
        let o = overlay(2, 5);
        // row 3, col 5 — top/bottom 모두 아니고 좌우 edge 아님 → bg.
        assert_eq!(o.cell_color(3, 5, 10), Some(CARD_BG));
        assert_eq!(o.cell_color(4, 1, 10), Some(CARD_BG));
    }

    #[test]
    fn block_overlay_cell_color_top_bottom_edge_is_border() {
        let o = overlay(2, 5);
        // top row(=2), bottom row(=5) 안쪽 cells.
        assert_eq!(o.cell_color(2, 5, 10), Some(CARD_BORDER));
        assert_eq!(o.cell_color(5, 5, 10), Some(CARD_BORDER));
    }

    #[test]
    fn block_overlay_cell_color_left_right_edge_is_border() {
        let o = overlay(2, 5);
        // col 0 (좌측), col 9 (=cols-1, 우측). 가운데 row.
        assert_eq!(o.cell_color(3, 0, 10), Some(CARD_BORDER));
        assert_eq!(o.cell_color(3, 9, 10), Some(CARD_BORDER));
    }

    #[test]
    fn block_overlay_cell_color_corner_is_border() {
        let o = overlay(2, 5);
        // 4 모서리 — top+left, top+right, bottom+left, bottom+right.
        assert_eq!(o.cell_color(2, 0, 10), Some(CARD_BORDER));
        assert_eq!(o.cell_color(2, 9, 10), Some(CARD_BORDER));
        assert_eq!(o.cell_color(5, 0, 10), Some(CARD_BORDER));
        assert_eq!(o.cell_color(5, 9, 10), Some(CARD_BORDER));
    }

    #[test]
    fn block_overlay_single_row_card_all_border() {
        // row range[3..=3]는 한 행만. 모든 cell이 top+bottom edge → border_color.
        let o = overlay(3, 3);
        for col in 0..10 {
            assert_eq!(o.cell_color(3, col, 10), Some(CARD_BORDER));
        }
    }

    #[test]
    fn selection_range_normalizes_drag_direction() {
        // caret 모델: SelectionRange((1, 2), (3, 8))는 caret 좌표.
        // row 1: cell col >= 2, row 2: 모든 col, row 3: cell col < 8.
        let range = SelectionRange::new((3, 8), (1, 2));

        assert_eq!(range.start, (1, 2));
        assert_eq!(range.end, (3, 8));
        assert!(range.contains(1, 2));
        assert!(range.contains(2, 0));
        assert!(range.contains(3, 7)); // half-open: end caret 8 미포함
        assert!(!range.contains(3, 8));
        assert!(!range.contains(1, 1));
        assert!(!range.contains(3, 9));
    }

    #[test]
    fn selection_range_empty_when_anchor_equals_head() {
        // caret 동일 → empty range (단순 클릭 / drag 0 cell).
        let range = SelectionRange::new((1, 5), (1, 5));
        assert!(!range.contains(1, 5));
        assert!(!range.contains(1, 4));
    }

    #[test]
    fn selection_range_single_cell_when_carets_one_apart() {
        // cell N 한 글자 선택은 caret [N, N+1).
        let range = SelectionRange::new((1, 5), (1, 6));
        assert!(range.contains(1, 5));
        assert!(!range.contains(1, 6));
        assert!(!range.contains(1, 4));
    }
}

/// IME preedit overlay. Term grid는 건드리지 않고 cursor (start_col, start_row) 위치부터
/// preedit string을 dim된 fg로 그려넣는다. cell 경계 넘어가면 truncate.
pub fn build_preedit_instances_at(
    preedit: &str,
    start_col: usize,
    start_row: usize,
    cols: usize,
    atlas: &GlyphAtlas,
    baseline: f32,
    col_offset: usize,
    row_offset: usize,
    palette: &ThemePalette,
) -> Vec<CellInstance> {
    let mut out = Vec::new();
    let mut col = start_col;
    let dim_fg = mix(palette.fg, palette.bg, 0.4);
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
            cell_xy: [(col + col_offset) as f32, (start_row + row_offset) as f32],
            uv_min,
            uv_max,
            glyph_offset,
            glyph_size,
            fg: dim_fg,
            bg: palette.bg,
            cell_span,
            flags: 0,
            _pad: [0.0; 2],
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

fn resolve(c: Color, is_fg: bool, palette: &ThemePalette) -> [f32; 4] {
    match c {
        Color::Default => {
            if is_fg {
                palette.fg
            } else {
                palette.bg
            }
        }
        Color::Indexed(n) => indexed(n, palette),
        Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
    }
}

fn indexed(n: u8, palette: &ThemePalette) -> [f32; 4] {
    if n < 16 {
        palette.ansi[n as usize]
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
