use std::collections::VecDeque;

use bitflags::bitflags;
use unicode_width::UnicodeWidthChar;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Attrs: u8 {
        const BOLD       = 1 << 0;
        const ITALIC     = 1 << 1;
        const UNDERLINE  = 1 << 2;
        const REVERSE    = 1 << 3;
        const WIDE       = 1 << 4;
        const WIDE_CONT  = 1 << 5;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underscore,
    Bar,
}

impl Default for CursorShape {
    fn default() -> Self {
        CursorShape::Block
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub blinking: bool,
    pub visible: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            shape: CursorShape::Block,
            blinking: true,
            visible: true,
        }
    }
}

#[derive(Debug, Clone)]
struct Grid {
    cells: Vec<Cell>,
    cols: usize,
    rows: usize,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols * rows],
            cols,
            rows,
        }
    }

    fn idx(&self, row: usize, col: usize) -> usize {
        row * self.cols + col
    }

    fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[self.idx(row, col)]
    }

    fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        let i = self.idx(row, col);
        &mut self.cells[i]
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut new = vec![Cell::default(); cols * rows];
        let copy_cols = cols.min(self.cols);
        let copy_rows = rows.min(self.rows);
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                new[r * cols + c] = self.cells[r * self.cols + c];
            }
        }
        self.cells = new;
        self.cols = cols;
        self.rows = rows;
    }

    /// scroll region [top, bottom) 내에서 위로 n행 밀어냄. 빈 행은 default cell로.
    fn scroll_up(&mut self, top: usize, bottom: usize, n: usize) {
        let n = n.min(bottom.saturating_sub(top));
        for r in top..(bottom - n) {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = self.cells[(r + n) * self.cols + c];
            }
        }
        for r in (bottom - n)..bottom {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = Cell::default();
            }
        }
    }

    fn scroll_down(&mut self, top: usize, bottom: usize, n: usize) {
        let n = n.min(bottom.saturating_sub(top));
        for r in (top + n..bottom).rev() {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = self.cells[(r - n) * self.cols + c];
            }
        }
        for r in top..(top + n) {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = Cell::default();
            }
        }
    }
}

/// 정책 B: scrollback hard cap 10,000 rows.
const SCROLLBACK_CAP: usize = 10_000;

/// M7-4: DECSC/DECRC로 저장되는 cursor 상태. xterm 표준에 따라 visible까지 포함.
#[derive(Debug, Clone, Copy)]
pub struct SavedCursorState {
    pub row: usize,
    pub col: usize,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub shape: CursorShape,
    pub blinking: bool,
    pub visible: bool,
}

pub struct Term {
    main: Grid,
    alt: Grid,
    use_alt: bool,
    cursor: Cursor,
    saved_main_cursor: Cursor,
    saved_alt_cursor: Cursor,
    scroll_top: usize,    // inclusive
    scroll_bottom: usize, // exclusive
    cur_fg: Color,
    cur_bg: Color,
    cur_attrs: Attrs,
    /// main grid에서 scroll_up으로 밀려난 row를 보관. 가장 오래된 게 front.
    scrollback: VecDeque<Vec<Cell>>,
    /// scrollback view offset. 0 = 현재(scrollback 안 보임), n = n rows 위.
    view_offset: usize,
    /// DECSC/DECRC용 saved state. main / alt 별도.
    decsc_main: Option<SavedCursorState>,
    decsc_alt: Option<SavedCursorState>,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            main: Grid::new(cols, rows),
            alt: Grid::new(cols, rows),
            use_alt: false,
            cursor: Cursor::default(),
            saved_main_cursor: Cursor::default(),
            saved_alt_cursor: Cursor::default(),
            scroll_top: 0,
            scroll_bottom: rows,
            cur_fg: Color::Default,
            cur_bg: Color::Default,
            cur_attrs: Attrs::empty(),
            scrollback: VecDeque::new(),
            view_offset: 0,
            decsc_main: None,
            decsc_alt: None,
        }
    }

    // M7-4: DECSC `ESC 7` — cursor 위치 + SGR + shape/blinking/visible 저장.
    pub fn decsc(&mut self) {
        let saved = SavedCursorState {
            row: self.cursor.row,
            col: self.cursor.col,
            fg: self.cur_fg,
            bg: self.cur_bg,
            attrs: self.cur_attrs,
            shape: self.cursor.shape,
            blinking: self.cursor.blinking,
            visible: self.cursor.visible,
        };
        if self.use_alt {
            self.decsc_alt = Some(saved);
        } else {
            self.decsc_main = Some(saved);
        }
    }

    // M7-4: DECRC `ESC 8` — 저장된 상태 복원. 저장된 게 없으면 noop.
    pub fn decrc(&mut self) {
        let saved = if self.use_alt {
            self.decsc_alt
        } else {
            self.decsc_main
        };
        if let Some(s) = saved {
            self.cursor.row = s.row.min(self.rows().saturating_sub(1));
            self.cursor.col = s.col.min(self.cols().saturating_sub(1));
            self.cur_fg = s.fg;
            self.cur_bg = s.bg;
            self.cur_attrs = s.attrs;
            self.cursor.shape = s.shape;
            self.cursor.blinking = s.blinking;
            self.cursor.visible = s.visible;
        }
    }

    fn grid(&self) -> &Grid {
        if self.use_alt {
            &self.alt
        } else {
            &self.main
        }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.use_alt {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    pub fn cols(&self) -> usize {
        self.grid().cols
    }
    pub fn rows(&self) -> usize {
        self.grid().rows
    }
    #[allow(dead_code)]
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }
    /// view_offset 반영해서 cell을 반환. scrollback row가 col 부족하면 default.
    /// (resize로 col이 변한 경우의 truncate-on-read; reflow는 안 함.)
    pub fn cell(&self, row: usize, col: usize) -> Cell {
        let scrollback_visible = self.view_offset.min(self.scrollback.len());
        if row < scrollback_visible {
            let sb_idx = self.scrollback.len() - scrollback_visible + row;
            return self
                .scrollback
                .get(sb_idx)
                .and_then(|r| r.get(col).copied())
                .unwrap_or_default();
        }
        let main_row = row - scrollback_visible;
        if main_row >= self.grid().rows {
            return Cell::default();
        }
        *self.grid().cell(main_row, col)
    }

    #[allow(dead_code)]
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn view_offset(&self) -> usize {
        self.view_offset
    }

    /// scrollback view 스크롤. delta > 0 = 위로, delta < 0 = 아래로.
    pub fn scroll_view_by(&mut self, delta: isize) {
        let max = self.scrollback.len();
        let new = if delta >= 0 {
            self.view_offset.saturating_add(delta as usize).min(max)
        } else {
            self.view_offset.saturating_sub((-delta) as usize)
        };
        self.view_offset = new;
    }

    pub fn snap_to_bottom(&mut self) {
        self.view_offset = 0;
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols() && rows == self.rows() {
            return;
        }
        self.main.resize(cols, rows);
        self.alt.resize(cols, rows);
        self.scroll_top = 0;
        self.scroll_bottom = rows;
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));
        // view_offset clamp + scrollback row의 col mismatch는 truncate-on-read로 처리.
        self.view_offset = self.view_offset.min(self.scrollback.len());
    }

    pub fn switch_alt_screen(&mut self, on: bool) {
        if on == self.use_alt {
            return;
        }
        // alt screen 전환 시 scrollback view는 항상 bottom으로 (alt에서 scrollback 안 봄).
        self.view_offset = 0;
        if on {
            self.saved_main_cursor = self.cursor;
            self.use_alt = true;
            // alt screen 진입 시 alt grid clear + cursor (0,0)
            for c in self.alt.cells.iter_mut() {
                *c = Cell::default();
            }
            self.cursor = self.saved_alt_cursor;
        } else {
            self.saved_alt_cursor = self.cursor;
            self.use_alt = false;
            self.cursor = self.saved_main_cursor;
        }
    }

    pub fn print(&mut self, ch: char) {
        let w = UnicodeWidthChar::width(ch).unwrap_or(1);
        if w == 0 {
            return; // 결합 문자(combining)는 M5 범위 외
        }
        if self.cursor.col + w > self.cols() {
            self.newline();
            self.cursor.col = 0;
        }
        let row = self.cursor.row;
        let col = self.cursor.col;
        let fg = self.cur_fg;
        let bg = self.cur_bg;
        let base_attrs = self.cur_attrs;
        let mut attrs = base_attrs;
        if w == 2 {
            attrs |= Attrs::WIDE;
        }
        let g = self.grid_mut();
        *g.cell_mut(row, col) = Cell {
            ch,
            fg,
            bg,
            attrs,
        };
        if w == 2 {
            *g.cell_mut(row, col + 1) = Cell {
                ch: ' ',
                fg,
                bg,
                attrs: base_attrs | Attrs::WIDE_CONT,
            };
        }
        self.cursor.col += w;
    }

    pub fn newline(&mut self) {
        if self.cursor.row + 1 >= self.scroll_bottom {
            let top = self.scroll_top;
            let bottom = self.scroll_bottom;
            // 풀스크린 + main screen 스크롤일 때만 top row를 scrollback에 push.
            // 부분 스크롤 영역(vim status bar 등)은 scrollback 오염 방지.
            if !self.use_alt && top == 0 && bottom == self.main.rows {
                let cols = self.main.cols;
                let row_cells: Vec<Cell> = self.main.cells[..cols].to_vec();
                self.scrollback.push_back(row_cells);
                while self.scrollback.len() > SCROLLBACK_CAP {
                    self.scrollback.pop_front();
                }
            }
            self.grid_mut().scroll_up(top, bottom, 1);
        } else {
            self.cursor.row += 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    /// HT — Horizontal Tab. cursor를 다음 tab stop(8칸 단위)으로 이동.
    /// cells는 변경하지 않음 (vt100 표준).
    pub fn tab(&mut self) {
        let cols = self.cols();
        let max = cols.saturating_sub(1);
        if self.cursor.col >= max {
            return;
        }
        let next = ((self.cursor.col / 8) + 1) * 8;
        self.cursor.col = next.min(max);
    }

    // CSI 커서 이동 — 모두 0-based 입력 기대(vt 레이어가 1→0 변환)
    pub fn cursor_up(&mut self, n: usize) {
        self.cursor.row = self.cursor.row.saturating_sub(n);
    }
    pub fn cursor_down(&mut self, n: usize) {
        self.cursor.row = (self.cursor.row + n).min(self.rows().saturating_sub(1));
    }
    pub fn cursor_left(&mut self, n: usize) {
        self.cursor.col = self.cursor.col.saturating_sub(n);
    }
    pub fn cursor_right(&mut self, n: usize) {
        self.cursor.col = (self.cursor.col + n).min(self.cols().saturating_sub(1));
    }
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor.row = row.min(self.rows().saturating_sub(1));
        self.cursor.col = col.min(self.cols().saturating_sub(1));
    }

    /// ED — Erase in Display: 0=cursor부터 끝, 1=처음부터 cursor까지, 2=전체
    pub fn erase_display(&mut self, mode: u16) {
        let cols = self.cols();
        let rows = self.rows();
        let (cur_row, cur_col) = (self.cursor.row, self.cursor.col);
        let g = self.grid_mut();
        match mode {
            0 => {
                for c in cur_col..cols {
                    *g.cell_mut(cur_row, c) = Cell::default();
                }
                for r in (cur_row + 1)..rows {
                    for c in 0..cols {
                        *g.cell_mut(r, c) = Cell::default();
                    }
                }
            }
            1 => {
                for r in 0..cur_row {
                    for c in 0..cols {
                        *g.cell_mut(r, c) = Cell::default();
                    }
                }
                for c in 0..=cur_col.min(cols.saturating_sub(1)) {
                    *g.cell_mut(cur_row, c) = Cell::default();
                }
            }
            2 | 3 => {
                for c in g.cells.iter_mut() {
                    *c = Cell::default();
                }
            }
            _ => {}
        }
    }

    /// EL — Erase in Line: 0=cursor부터 끝, 1=처음부터 cursor까지, 2=전체
    pub fn erase_line(&mut self, mode: u16) {
        let cols = self.cols();
        let (cur_row, cur_col) = (self.cursor.row, self.cursor.col);
        let g = self.grid_mut();
        match mode {
            0 => {
                for c in cur_col..cols {
                    *g.cell_mut(cur_row, c) = Cell::default();
                }
            }
            1 => {
                for c in 0..=cur_col.min(cols.saturating_sub(1)) {
                    *g.cell_mut(cur_row, c) = Cell::default();
                }
            }
            2 => {
                for c in 0..cols {
                    *g.cell_mut(cur_row, c) = Cell::default();
                }
            }
            _ => {}
        }
    }

    pub fn scroll_up_n(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_up(top, bottom, n);
    }
    pub fn scroll_down_n(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_down(top, bottom, n);
    }

    /// DECSTBM — top/bottom 모두 0-based 입력
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let rows = self.rows();
        if top < bottom && bottom <= rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = rows;
        }
        self.cursor = Cursor::default();
    }

    // M7-1: cursor shape/blinking 변경 (DECSCUSR).
    pub fn set_cursor_shape(&mut self, shape: CursorShape, blinking: bool) {
        self.cursor.shape = shape;
        self.cursor.blinking = blinking;
    }

    // M7-2: cursor 가시성 (DECTCEM).
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor.visible = visible;
    }

    // SGR
    pub fn reset_sgr(&mut self) {
        self.cur_fg = Color::Default;
        self.cur_bg = Color::Default;
        self.cur_attrs = Attrs::empty();
    }
    pub fn set_fg(&mut self, c: Color) {
        self.cur_fg = c;
    }
    pub fn set_bg(&mut self, c: Color) {
        self.cur_bg = c;
    }
    pub fn add_attr(&mut self, a: Attrs) {
        self.cur_attrs.insert(a);
    }
    pub fn remove_attr(&mut self, a: Attrs) {
        self.cur_attrs.remove(a);
    }

    #[allow(dead_code)]
    pub fn debug_dump(&self) -> String {
        let g = self.grid();
        let mut s = String::with_capacity(g.cells.len() + g.rows);
        for r in 0..g.rows {
            for c in 0..g.cols {
                s.push(g.cells[r * g.cols + c].ch);
            }
            s.push('\n');
        }
        s
    }
}
