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
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, action: char) {
        if action == 'm' {
            self.handle_sgr(params.iter());
        }
    }

    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
}

impl<'a> TermPerform<'a> {
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
