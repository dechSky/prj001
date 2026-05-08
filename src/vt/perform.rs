use vte::{Params, Perform};

use crate::grid::Term;

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

    fn csi_dispatch(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
}
