use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Attrs: u8 {
        const BOLD      = 1 << 0;
        const ITALIC    = 1 << 1;
        const UNDERLINE = 1 << 2;
        const REVERSE   = 1 << 3;
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

#[derive(Debug, Clone, Copy, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

pub struct Term {
    cells: Vec<Cell>,
    cols: usize,
    rows: usize,
    cursor: Cursor,
    cur_fg: Color,
    cur_bg: Color,
    cur_attrs: Attrs,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols * rows],
            cols,
            rows,
            cursor: Cursor::default(),
            cur_fg: Color::Default,
            cur_bg: Color::Default,
            cur_attrs: Attrs::empty(),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn rows(&self) -> usize {
        self.rows
    }
    #[allow(dead_code)]
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
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
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));
    }

    pub fn print(&mut self, ch: char) {
        if self.cursor.col >= self.cols {
            self.newline();
            self.cursor.col = 0;
        }
        let idx = self.cursor.row * self.cols + self.cursor.col;
        self.cells[idx] = Cell {
            ch,
            fg: self.cur_fg,
            bg: self.cur_bg,
            attrs: self.cur_attrs,
        };
        self.cursor.col += 1;
    }

    pub fn newline(&mut self) {
        if self.cursor.row + 1 >= self.rows {
            self.scroll_up(1);
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

    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.rows);
        for r in 0..(self.rows - n) {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = self.cells[(r + n) * self.cols + c];
            }
        }
        for r in (self.rows - n)..self.rows {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = Cell::default();
            }
        }
    }

    // SGR 상태
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
        let mut s = String::with_capacity(self.cells.len() + self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                s.push(self.cells[r * self.cols + c].ch);
            }
            s.push('\n');
        }
        s
    }
}
