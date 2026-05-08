use vte::{Params, ParamsIter, Perform};

use crate::grid::{Attrs, Color, Term};

pub struct TermPerform<'a> {
    term: &'a mut Term,
}

impl<'a> TermPerform<'a> {
    pub fn new(term: &'a mut Term) -> Self {
        Self { term }
    }
}

impl<'a> Perform for TermPerform<'a> {
    fn print(&mut self, c: char) {
        self.term.print(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x0A => self.term.newline(),
            0x0D => self.term.carriage_return(),
            0x08 => self.term.backspace(),
            0x09 => self.term.tab(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _: bool, action: char) {
        // DEC private modes: ESC [ ? Pn h/l
        if intermediates == b"?" {
            self.handle_dec_private(params, action);
            return;
        }
        if !intermediates.is_empty() {
            return;
        }
        match action {
            'm' => self.handle_sgr(params.iter()),
            'A' => self.term.cursor_up(arg1(params, 1)),
            'B' | 'e' => self.term.cursor_down(arg1(params, 1)),
            'C' | 'a' => self.term.cursor_right(arg1(params, 1)),
            'D' => self.term.cursor_left(arg1(params, 1)),
            'H' | 'f' => {
                // CUP: ESC [ Pr ; Pc H — 1-based
                let row = arg1(params, 1).saturating_sub(1);
                let col = arg_at(params, 1, 1).saturating_sub(1);
                self.term.set_cursor(row, col);
            }
            'G' | '`' => {
                // CHA: 절대 column, 1-based
                let col = arg1(params, 1).saturating_sub(1);
                let cur_row = self.term.cursor().row;
                self.term.set_cursor(cur_row, col);
            }
            'd' => {
                // VPA: 절대 row, 1-based
                let row = arg1(params, 1).saturating_sub(1);
                let cur_col = self.term.cursor().col;
                self.term.set_cursor(row, cur_col);
            }
            'J' => self.term.erase_display(arg_at(params, 0, 0) as u16),
            'K' => self.term.erase_line(arg_at(params, 0, 0) as u16),
            'S' => self.term.scroll_up_n(arg1(params, 1)),
            'T' => self.term.scroll_down_n(arg1(params, 1)),
            'r' => {
                // DECSTBM: ESC [ top ; bottom r — 1-based, default = 전체
                let rows = self.term.rows();
                let top = arg1(params, 1).saturating_sub(1);
                let bottom = arg_at(params, 1, rows);
                self.term.set_scroll_region(top, bottom);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
}

impl<'a> TermPerform<'a> {
    fn handle_dec_private(&mut self, params: &Params, action: char) {
        for p in params.iter() {
            let code = p.first().copied().unwrap_or(0);
            match (code, action) {
                (1049, 'h') => self.term.switch_alt_screen(true),
                (1049, 'l') => self.term.switch_alt_screen(false),
                // 1047/1048도 alt screen 변종이지만 1049가 표준
                (1047, 'h') => self.term.switch_alt_screen(true),
                (1047, 'l') => self.term.switch_alt_screen(false),
                _ => {}
            }
        }
    }

    fn handle_sgr(&mut self, mut iter: ParamsIter<'_>) {
        let mut saw_any = false;
        while let Some(p) = iter.next() {
            saw_any = true;
            let code = p.first().copied().unwrap_or(0);
            match code {
                0 => self.term.reset_sgr(),
                1 => self.term.add_attr(Attrs::BOLD),
                3 => self.term.add_attr(Attrs::ITALIC),
                4 => self.term.add_attr(Attrs::UNDERLINE),
                7 => self.term.add_attr(Attrs::REVERSE),
                22 => self.term.remove_attr(Attrs::BOLD),
                23 => self.term.remove_attr(Attrs::ITALIC),
                24 => self.term.remove_attr(Attrs::UNDERLINE),
                27 => self.term.remove_attr(Attrs::REVERSE),
                30..=37 => self.term.set_fg(Color::Indexed((code - 30) as u8)),
                39 => self.term.set_fg(Color::Default),
                40..=47 => self.term.set_bg(Color::Indexed((code - 40) as u8)),
                49 => self.term.set_bg(Color::Default),
                90..=97 => self.term.set_fg(Color::Indexed((code - 90 + 8) as u8)),
                100..=107 => self.term.set_bg(Color::Indexed((code - 100 + 8) as u8)),
                38 => {
                    if let Some(c) = parse_extended_color(&mut iter) {
                        self.term.set_fg(c);
                    }
                }
                48 => {
                    if let Some(c) = parse_extended_color(&mut iter) {
                        self.term.set_bg(c);
                    }
                }
                _ => {}
            }
        }
        if !saw_any {
            self.term.reset_sgr();
        }
    }
}

fn parse_extended_color(iter: &mut ParamsIter<'_>) -> Option<Color> {
    let mode_p = iter.next()?;
    let mode = mode_p.first().copied().unwrap_or(0);
    match mode {
        5 => {
            let n_p = iter.next()?;
            let n = n_p.first().copied().unwrap_or(0) as u8;
            Some(Color::Indexed(n))
        }
        2 => {
            let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
            let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
            let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

/// 첫 번째 param 값. 없거나 0이면 default.
fn arg1(params: &Params, default: usize) -> usize {
    arg_at(params, 0, default)
}

fn arg_at(params: &Params, idx: usize, default: usize) -> usize {
    let v = params
        .iter()
        .nth(idx)
        .and_then(|p| p.first().copied())
        .unwrap_or(0);
    if v == 0 { default } else { v as usize }
}
