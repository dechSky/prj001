use vte::{Params, ParamsIter, Perform};

use crate::grid::{Attrs, Color, CursorShape, Term};

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
        // DECSCUSR: ESC [ Ps SP q (intermediates=" ", action='q')
        if intermediates == b" " && action == 'q' {
            let n = arg_at(params, 0, 0);
            if let Some((shape, blinking)) = decscusr_to_shape(n) {
                log::info!(
                    "decscusr: n={} → shape={:?} blinking={}",
                    n,
                    shape,
                    blinking
                );
                self.term.set_cursor_shape(shape, blinking);
            } else {
                log::debug!("decscusr: unknown n={}", n);
            }
            return;
        }
        // M10-5: DA2 (Device Attributes secondary): CSI > c (intermediates=">", action='c')
        if intermediates == b">" && action == 'c' {
            let p = arg_at(params, 0, 0);
            if p == 0 {
                // xterm Pp=41 표준
                self.term.push_response(b"\x1b[>41;0;0c".to_vec());
            }
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
            // M10-4: DSR (Device Status Report)
            'n' => {
                let p = arg_at(params, 0, 0);
                if p == 6 {
                    // DSR cursor position: \x1b[<row>;<col>R (1-based)
                    let cur = self.term.cursor();
                    let resp = format!("\x1b[{};{}R", cur.row + 1, cur.col + 1);
                    self.term.push_response(resp.into_bytes());
                }
                // p == 5 (status report \x1b[0n)는 M11 cleanup
            }
            // M10-5: DA1 (Device Attributes primary)
            'c' => {
                let p = arg_at(params, 0, 0);
                if p == 0 {
                    // DA1 default: xterm-256color terminfo u8 = \x1b[?1;2c
                    self.term.push_response(b"\x1b[?1;2c".to_vec());
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // M7-4: DECSC `ESC 7`, DECRC `ESC 8`.
            b'7' => self.term.decsc(),
            b'8' => self.term.decrc(),
            // M9-3: DECPAM (`ESC =`) application keypad, DECPNM (`ESC >`) numeric.
            b'=' => self.term.set_keypad_application(true),
            b'>' => self.term.set_keypad_application(false),
            _ => {}
        }
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() < 2 {
            return;
        }
        let code = params[0];
        // M8-7: OSC 0 / OSC 2 → 창 타이틀 직접 지정.
        if code == b"0" || code == b"2" {
            if let Ok(title) = std::str::from_utf8(params[1]) {
                self.term.set_title(title.to_string());
            }
            return;
        }
        // M8-7 보강: OSC 7 → working directory file URL → 타이틀로 변환.
        // macOS zsh의 update_terminal_cwd hook이 보내는 시퀀스.
        // 형식: file://hostname/encoded/path
        if code == b"7" {
            if let Ok(url) = std::str::from_utf8(params[1]) {
                if let Some(rest) = url.strip_prefix("file://") {
                    // hostname/path 분리. hostname은 무시, path만.
                    let path_encoded = rest.splitn(2, '/').nth(1).unwrap_or("");
                    let path = url_decode(path_encoded);
                    let display = home_relative(&path);
                    self.term.set_title(display);
                }
            }
        }
    }
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
                // M7-2: DECTCEM cursor visibility
                (25, 'h') => self.term.set_cursor_visible(true),
                (25, 'l') => self.term.set_cursor_visible(false),
                // M8-4: DECCKM cursor keys application mode
                (1, 'h') => self.term.set_cursor_keys_application(true),
                (1, 'l') => self.term.set_cursor_keys_application(false),
                // M10-3: focus reporting mode (CSI ?1004 h/l)
                (1004, 'h') => self.term.set_focus_reporting(true),
                (1004, 'l') => self.term.set_focus_reporting(false),
                // M10-2: bracketed paste mode (CSI ?2004 h/l)
                (2004, 'h') => self.term.set_bracketed_paste(true),
                (2004, 'l') => self.term.set_bracketed_paste(false),
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

/// percent-encoded URL path → 일반 path. `%20` 등을 단일 byte로.
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = (bytes[i + 1] as char).to_digit(16);
            let h2 = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h1), Some(h2)) = (h1, h2) {
                out.push((h1 * 16 + h2) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// $HOME 접두사면 "~"로 치환, 아니면 절대 path 그대로.
fn home_relative(abs_path: &str) -> String {
    let abs = format!("/{}", abs_path.trim_start_matches('/'));
    if let Ok(home) = std::env::var("HOME") {
        if abs.starts_with(&home) {
            let suffix = &abs[home.len()..];
            return if suffix.is_empty() {
                "~".to_string()
            } else {
                format!("~{suffix}")
            };
        }
    }
    abs
}

fn decscusr_to_shape(n: usize) -> Option<(CursorShape, bool)> {
    match n {
        0 | 1 => Some((CursorShape::Block, true)),
        2 => Some((CursorShape::Block, false)),
        3 => Some((CursorShape::Underscore, true)),
        4 => Some((CursorShape::Underscore, false)),
        5 => Some((CursorShape::Bar, true)),
        6 => Some((CursorShape::Bar, false)),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Color, CursorShape, Term};
    use vte::Parser;

    fn run(term: &mut Term, bytes: &[u8]) {
        let mut parser = Parser::new();
        let mut perform = TermPerform::new(term);
        parser.advance(&mut perform, bytes);
    }

    #[test]
    fn decscusr_sets_shape_and_blinking() {
        let mut term = Term::new(80, 24);
        // 기본값 검증
        assert_eq!(term.cursor().shape, CursorShape::Block);
        assert!(term.cursor().blinking);

        // n=5: blink bar
        run(&mut term, b"\x1b[5 q");
        assert_eq!(term.cursor().shape, CursorShape::Bar);
        assert!(term.cursor().blinking);

        // n=4: steady underscore
        run(&mut term, b"\x1b[4 q");
        assert_eq!(term.cursor().shape, CursorShape::Underscore);
        assert!(!term.cursor().blinking);

        // n=2: steady block
        run(&mut term, b"\x1b[2 q");
        assert_eq!(term.cursor().shape, CursorShape::Block);
        assert!(!term.cursor().blinking);

        // n=0: default (blink block)
        run(&mut term, b"\x1b[0 q");
        assert_eq!(term.cursor().shape, CursorShape::Block);
        assert!(term.cursor().blinking);
    }

    #[test]
    fn decscusr_unknown_n_ignored() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b[5 q"); // bar, blink
        run(&mut term, b"\x1b[99 q"); // 알 수 없음 — 무시
        assert_eq!(term.cursor().shape, CursorShape::Bar);
        assert!(term.cursor().blinking);
    }

    #[test]
    fn dectcem_toggles_visibility() {
        let mut term = Term::new(80, 24);
        assert!(term.cursor().visible);

        run(&mut term, b"\x1b[?25l"); // hide
        assert!(!term.cursor().visible);

        run(&mut term, b"\x1b[?25h"); // show
        assert!(term.cursor().visible);
    }

    #[test]
    fn decsc_decrc_preserves_position_and_sgr() {
        let mut term = Term::new(80, 24);
        // 위치를 (5, 10)로, fg=blue, bold + bar+blink
        run(&mut term, b"\x1b[6;11H"); // CUP 1-based: row 6 → 0-based 5, col 11 → 10
        run(&mut term, b"\x1b[1;34m"); // bold + blue fg
        run(&mut term, b"\x1b[5 q"); // bar, blink
        assert_eq!(term.cursor().row, 5);
        assert_eq!(term.cursor().col, 10);
        assert_eq!(term.cursor().shape, CursorShape::Bar);

        // DECSC
        run(&mut term, b"\x1b7");

        // 다른 위치/SGR로 변경
        run(&mut term, b"\x1b[1;1H"); // 0,0
        run(&mut term, b"\x1b[0m"); // reset SGR
        run(&mut term, b"\x1b[2 q"); // steady block
        assert_eq!(term.cursor().row, 0);
        assert_eq!(term.cursor().col, 0);
        assert_eq!(term.cursor().shape, CursorShape::Block);

        // DECRC
        run(&mut term, b"\x1b8");
        assert_eq!(term.cursor().row, 5);
        assert_eq!(term.cursor().col, 10);
        assert_eq!(term.cursor().shape, CursorShape::Bar);
        assert!(term.cursor().blinking);
    }

    #[test]
    fn decrc_without_decsc_is_noop() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b[6;11H"); // (5, 10)
        run(&mut term, b"\x1b8"); // DECRC without prior DECSC
        // 변경 없이 그대로
        assert_eq!(term.cursor().row, 5);
        assert_eq!(term.cursor().col, 10);
    }

    #[test]
    fn decsc_separates_main_and_alt_screens() {
        let mut term = Term::new(80, 24);
        // main에서 (3, 5) 저장
        run(&mut term, b"\x1b[4;6H");
        run(&mut term, b"\x1b7");

        // alt screen 진입 + 다른 위치 저장
        run(&mut term, b"\x1b[?1049h"); // alt screen on
        run(&mut term, b"\x1b[10;20H");
        run(&mut term, b"\x1b7");

        // alt에서 위치 변경 후 DECRC → alt saved 복원
        run(&mut term, b"\x1b[1;1H");
        run(&mut term, b"\x1b8");
        assert_eq!(term.cursor().row, 9);
        assert_eq!(term.cursor().col, 19);

        // main 복귀 후 DECRC → main saved 복원
        run(&mut term, b"\x1b[?1049l"); // alt off
        run(&mut term, b"\x1b[1;1H");
        run(&mut term, b"\x1b8");
        assert_eq!(term.cursor().row, 3);
        assert_eq!(term.cursor().col, 5);
    }

    #[test]
    fn osc_7_sets_title_from_file_url() {
        let mut term = Term::new(80, 24);
        // file://hostname/Users/derek/Documents → /Users/derek/Documents
        run(
            &mut term,
            b"\x1b]7;file://Derek-Mac/Users/derek/Documents\x07",
        );
        let t = term.take_title_if_changed().unwrap();
        // HOME에 따라 ~/... 또는 /Users/...
        assert!(t.contains("Documents"), "title was {:?}", t);
    }

    #[test]
    fn osc_7_url_decodes_spaces() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b]7;file://h/Users/foo%20bar/baz\x07");
        let t = term.take_title_if_changed().unwrap();
        assert!(t.contains("foo bar"), "title was {:?}", t);
    }

    #[test]
    fn osc_2_sets_window_title() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b]2;hello world\x07"); // OSC 2 ; "hello world" BEL
        assert_eq!(
            term.take_title_if_changed(),
            Some("hello world".to_string())
        );
        // 두 번째 호출은 None (dirty 클리어됨)
        assert_eq!(term.take_title_if_changed(), None);
    }

    #[test]
    fn osc_0_sets_window_title() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b]0;abc\x07");
        assert_eq!(term.take_title_if_changed(), Some("abc".to_string()));
    }

    #[test]
    fn decckm_toggle() {
        let mut term = Term::new(80, 24);
        assert!(!term.cursor_keys_application());
        run(&mut term, b"\x1b[?1h");
        assert!(term.cursor_keys_application());
        run(&mut term, b"\x1b[?1l");
        assert!(!term.cursor_keys_application());
    }

    #[test]
    fn dsr_cursor_position_response() {
        let mut term = Term::new(80, 24);
        // cursor (5, 10) — CUP은 1-based이므로 \x1b[6;11H = (row 5, col 10) 0-based
        run(&mut term, b"\x1b[6;11H");
        assert_eq!(term.cursor().row, 5);
        assert_eq!(term.cursor().col, 10);
        run(&mut term, b"\x1b[6n");
        let responses = term.drain_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[6;11R");
        // 두 번째 drain은 빈
        let responses2 = term.drain_responses();
        assert!(responses2.is_empty());
    }

    #[test]
    fn dsr_unknown_param_ignored() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b[5n"); // status report — M10에서 미처리
        assert!(term.drain_responses().is_empty());
    }

    #[test]
    fn da1_response() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b[c");
        let responses = term.drain_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[?1;2c");
    }

    #[test]
    fn da2_response() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b[>c");
        let responses = term.drain_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[>41;0;0c");
    }

    #[test]
    fn da1_with_nonzero_param_ignored() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b[1c"); // 비표준
        assert!(term.drain_responses().is_empty());
    }

    #[test]
    fn bracketed_paste_mode_toggle() {
        let mut term = Term::new(80, 24);
        assert!(!term.bracketed_paste());
        run(&mut term, b"\x1b[?2004h");
        assert!(term.bracketed_paste());
        run(&mut term, b"\x1b[?2004l");
        assert!(!term.bracketed_paste());
    }

    #[test]
    fn focus_reporting_mode_toggle() {
        let mut term = Term::new(80, 24);
        assert!(!term.focus_reporting());
        run(&mut term, b"\x1b[?1004h");
        assert!(term.focus_reporting());
        run(&mut term, b"\x1b[?1004l");
        assert!(!term.focus_reporting());
    }

    #[test]
    fn decpam_decpnm_toggle() {
        let mut term = Term::new(80, 24);
        assert!(!term.keypad_application());
        run(&mut term, b"\x1b="); // DECPAM
        assert!(term.keypad_application());
        run(&mut term, b"\x1b>"); // DECPNM
        assert!(!term.keypad_application());
    }

    #[test]
    fn dectcem_default_visible_after_decsc_decrc() {
        let mut term = Term::new(80, 24);
        run(&mut term, b"\x1b[?25l"); // hide
        run(&mut term, b"\x1b7"); // save (visible=false 상태)
        run(&mut term, b"\x1b[?25h"); // show
        run(&mut term, b"\x1b8"); // restore
        // xterm 표준: visible=false 복원
        assert!(!term.cursor().visible);
        // 컬러는 변하지 않아야
        let _ = Color::Default; // 사용 표시
    }
}
