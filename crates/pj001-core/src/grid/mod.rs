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
        /// 슬라이스 6.3b: OSC 8 hyperlink active 동안 print된 cells.
        /// 렌더 시점에 theme의 ANSI 12(밝은 파랑)로 fg 치환 → 시각적으로 링크 강조.
        const HYPERLINK  = 1 << 6;
    }
}

bitflags! {
    /// 행 단위 메타. M17 reflow 인프라.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct RowFlags: u8 {
        /// 이 row는 다음 row로 wrap continuation. print overflow에서 마킹.
        const WRAPPED = 1 << 0;
    }
}

/// scrollback에 보관되는 row. cells와 row 단위 flag.
/// flags는 M17-3/M17-4 reflow 진입 후 사용. push 시점부터 같이 보관.
#[derive(Debug, Clone)]
pub(crate) struct ScrollbackRow {
    pub cells: Vec<Cell>,
    #[allow(dead_code)]
    pub flags: RowFlags,
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
    /// M17 reflow 인프라. len == rows. cells와 동기 관리.
    row_flags: Vec<RowFlags>,
    cols: usize,
    rows: usize,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols * rows],
            row_flags: vec![RowFlags::empty(); rows],
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
        // row_flags도 같이 truncate/extend. M17-1 단계는 truncate-only(M17-3에서 reflow가 분기).
        let mut new_flags = vec![RowFlags::empty(); rows];
        let copy = rows.min(self.row_flags.len());
        new_flags[..copy].copy_from_slice(&self.row_flags[..copy]);
        self.row_flags = new_flags;
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
            self.row_flags[r] = self.row_flags[r + n];
        }
        for r in (bottom - n)..bottom {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = Cell::default();
            }
            self.row_flags[r] = RowFlags::empty();
        }
    }

    fn scroll_down(&mut self, top: usize, bottom: usize, n: usize) {
        let n = n.min(bottom.saturating_sub(top));
        for r in (top + n..bottom).rev() {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = self.cells[(r - n) * self.cols + c];
            }
            self.row_flags[r] = self.row_flags[r - n];
        }
        for r in top..(top + n) {
            for c in 0..self.cols {
                self.cells[r * self.cols + c] = Cell::default();
            }
            self.row_flags[r] = RowFlags::empty();
        }
    }
}

/// 정책 B: scrollback hard cap 10,000 rows.
const SCROLLBACK_CAP: usize = 10_000;

/// M11-4: DEC Special Character and Line Drawing Set 매핑 (xterm 표준).
/// 7-bit 0x5f..=0x7e 영역만 변환. 그 외 입력은 통과.
/// 출처: xterm ctlseqs "DEC Special Character and Line Drawing Set".
fn dec_special_translate(ch: char) -> char {
    match ch {
        '_' => ' ',      // 0x5f blank
        '`' => '◆',      // diamond
        'a' => '▒',      // checkerboard
        'b' => '␉',      // HT
        'c' => '␌',      // FF
        'd' => '␍',      // CR
        'e' => '␊',      // LF
        'f' => '°',      // degree
        'g' => '±',      // plus/minus
        'h' => '␤',      // NL
        'i' => '␋',      // VT
        'j' => '┘',      // lower-right corner
        'k' => '┐',      // upper-right corner
        'l' => '┌',      // upper-left corner
        'm' => '└',      // lower-left corner
        'n' => '┼',      // crossing
        'o' => '⎺',      // horizontal scan line 1
        'p' => '⎻',      // horizontal scan line 3
        'q' => '─',      // horizontal scan line 5 (middle)
        'r' => '⎼',      // horizontal scan line 7
        's' => '⎽',      // horizontal scan line 9
        't' => '├',      // left T
        'u' => '┤',      // right T
        'v' => '┴',      // bottom T
        'w' => '┬',      // top T
        'x' => '│',      // vertical bar
        'y' => '≤',      // less-or-equal
        'z' => '≥',      // greater-or-equal
        '{' => 'π',      // pi
        '|' => '≠',      // not-equal
        '}' => '£',      // sterling
        '~' => '·',      // centered dot
        _ => ch,
    }
}

/// M17 reflow 결과.
#[derive(Debug)]
pub(crate) struct RewrapResult {
    /// 새 row 시퀀스. 각 row의 cells.len() ≤ new_cols. WRAPPED row는 항상 == new_cols.
    pub new_rows: Vec<(Vec<Cell>, RowFlags)>,
    /// 새 시퀀스 내 cursor의 global row 인덱스.
    pub cursor_global_row: usize,
    /// 새 시퀀스 내 cursor의 col (eager wrap pending 시 cells.len()과 같을 수 있음).
    pub cursor_new_col: usize,
}

/// logical line 안에서 cursor offset → (relative row, col) 매핑.
fn map_cursor_in_line(line_rows: &[(Vec<Cell>, RowFlags)], offset: usize) -> (usize, usize) {
    if line_rows.is_empty() {
        return (0, 0);
    }
    let mut cumulative = 0;
    for (i, (cells, _)) in line_rows.iter().enumerate() {
        let len = cells.len();
        if offset < cumulative + len {
            return (i, offset - cumulative);
        }
        cumulative += len;
    }
    // cursor가 logical line 끝 (eager wrap pending). 마지막 row의 col = cells.len().
    let last = line_rows.len() - 1;
    (last, line_rows[last].0.len())
}

/// M17 reflow 핵심 알고리즘. logical line 단위로 분할 → re-wrap.
/// 외부 의존 없음 — 헤드리스 unit test 가능.
pub(crate) fn rewrap_lines(
    rows: &[(Vec<Cell>, RowFlags)],
    cursor_row: usize,
    cursor_col: usize,
    new_cols: usize,
) -> RewrapResult {
    let mut result = RewrapResult {
        new_rows: Vec::new(),
        cursor_global_row: 0,
        cursor_new_col: 0,
    };
    if rows.is_empty() || new_cols == 0 {
        return result;
    }

    let mut cursor_set = false;
    let mut i = 0;
    while i < rows.len() {
        // logical line: i..=end. WRAPPED 끊어지는 row 또는 시퀀스 끝까지.
        let mut j = i;
        while j < rows.len() && rows[j].1.contains(RowFlags::WRAPPED) && j + 1 < rows.len() {
            j += 1;
        }
        let end = j;

        // cells 평탄화 + cursor offset 추적
        let mut combined: Vec<Cell> = Vec::new();
        let mut cursor_offset_in_line: Option<usize> = None;
        for k in i..=end {
            if k == cursor_row {
                cursor_offset_in_line = Some(combined.len() + cursor_col);
            }
            combined.extend_from_slice(&rows[k].0);
        }

        // trim: 마지막 row가 not WRAPPED + cursor 보호
        let last_wrapped = rows[end].1.contains(RowFlags::WRAPPED);
        let min_keep = cursor_offset_in_line.unwrap_or(0);
        if !last_wrapped {
            while combined.len() > min_keep && combined.last() == Some(&Cell::default()) {
                combined.pop();
            }
        }

        // re-wrap to new_cols
        let line_start = result.new_rows.len();
        let mut row_buffer: Vec<Cell> = Vec::with_capacity(new_cols);
        let mut idx = 0;
        while idx < combined.len() {
            let cell = combined[idx];
            let is_wide = cell.attrs.contains(Attrs::WIDE);
            let glyph_w = if is_wide { 2 } else { 1 };

            // 1 col 안전망: WIDE 표시 불가 → skip + WIDE_CONT 동반 skip
            if is_wide && new_cols < 2 {
                idx += if idx + 1 < combined.len()
                    && combined[idx + 1].attrs.contains(Attrs::WIDE_CONT)
                {
                    2
                } else {
                    1
                };
                continue;
            }

            // 현재 row가 가득 차 다음 글자가 안 들어감 → flush + 다음 row
            if row_buffer.len() + glyph_w > new_cols {
                // WIDE 경계: 마지막 1칸이 비고 다음 글자가 WIDE면 빈 default padding
                while row_buffer.len() < new_cols {
                    row_buffer.push(Cell::default());
                }
                result
                    .new_rows
                    .push((std::mem::take(&mut row_buffer), RowFlags::WRAPPED));
            }

            row_buffer.push(cell);
            if is_wide {
                if idx + 1 < combined.len() && combined[idx + 1].attrs.contains(Attrs::WIDE_CONT) {
                    row_buffer.push(combined[idx + 1]);
                    idx += 2;
                } else {
                    idx += 1;
                }
            } else {
                idx += 1;
            }
        }

        // 마지막 row buffer flush (빈 logical line이라도 row 1개 emit)
        if !row_buffer.is_empty() || combined.is_empty() {
            result.new_rows.push((row_buffer, RowFlags::empty()));
        }

        // cursor 매핑: 이 logical line이 만들어낸 NewRow 슬라이스에서 offset → (rel_row, col)
        if let Some(offset) = cursor_offset_in_line {
            if !cursor_set {
                let line_rows = &result.new_rows[line_start..];
                let (rel_row, col) = map_cursor_in_line(line_rows, offset);
                result.cursor_global_row = line_start + rel_row;
                result.cursor_new_col = col;
                cursor_set = true;
            }
        }

        i = end + 1;
    }

    result
}

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
    /// M17: ScrollbackRow { cells, flags }로 변경. WRAPPED flag도 함께 push.
    scrollback: VecDeque<ScrollbackRow>,
    /// scrollback view offset. 0 = 현재(scrollback 안 보임), n = n rows 위.
    view_offset: usize,
    /// DECSC/DECRC용 saved state. main / alt 별도.
    decsc_main: Option<SavedCursorState>,
    decsc_alt: Option<SavedCursorState>,
    /// DECCKM (M8-4). false = normal cursor keys (CSI), true = application (SS3).
    cursor_keys_application: bool,
    /// DECPAM/DECPNM (M9-3). false = numeric keypad, true = application keypad.
    /// 현재 numpad 키 자체는 미처리 — mode 추적만 (M9 향후 numpad 처리 시 참조).
    keypad_application: bool,
    /// M8-7: 창 타이틀 (OSC 0/2). PTY가 보낼 때마다 갱신.
    title: String,
    title_dirty: bool,
    /// M10-2: bracketed paste mode (CSI ?2004 h/l). app이 paste 시 wrap 여부 판정.
    bracketed_paste: bool,
    /// M10-3: focus reporting mode (CSI ?1004 h/l). app이 focus change 시 송신 판정.
    focus_reporting: bool,
    /// M10-1: vt가 PTY로 보낼 응답 누적. main이 render에서 drain → pty.write.
    /// DSR/DA 응답, M11+에서 OSC query 응답 등.
    pending_responses: Vec<Vec<u8>>,
    /// M11-4: G0 charset (LS0 default). DEC special graphics(line drawing)일 때
    /// `print()`가 7-bit input(0x60..=0x7e)을 Unicode box drawing 글리프로 변환.
    /// G1/G2/G3 + SS2/SS3는 미지원 (post-MVP+ cleanup).
    g0_charset: Charset,
    /// OSC 7로 받은 현재 작업 디렉터리. shell의 chpwd hook이 보낸 file URL을
    /// path로 디코드해 보관. None이면 미수신/미파싱. block UI(M13+)와 pane 헤더가 사용.
    cwd: Option<String>,
    /// OSC 8 active hyperlink URI. None = 일반 텍스트. 차후 cell 단위 매핑은
    /// 사이드테이블로 분리 (1차는 추적만).
    hyperlink_uri: Option<String>,
    /// OSC 133;A — 최근 prompt start row (절대 = scrollback rows + main grid row).
    /// 차후 Cmd+↑/↓ "prev/next prompt" 점프, block UI 카드 경계 결정에 사용.
    last_prompt_row: Option<u64>,
    /// OSC 133;A 누적 카운터 — 디버깅 + 테스트 검증용.
    prompts_seen: u64,
    /// OSC 133;D 종료 코드. None이면 미수신 / running.
    last_command_exit: Option<i32>,
    /// 슬라이스 6.6: xterm 마우스 reporting 모드. None = 보고 안 함.
    mouse_protocol: MouseProtocol,
    /// CSI ?1006: SGR encoding. true면 `CSI < b;c;r M/m`, false면 legacy 1-byte 인코딩.
    mouse_sgr_encoding: bool,
}

/// 마우스 reporting 강도. `?1000` < `?1002` < `?1003` 순으로 이벤트 범위 확장.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseProtocol {
    /// 보고 안 함 (기본).
    Off,
    /// CSI ?1000: 버튼 press/release만.
    Button,
    /// CSI ?1002: 버튼 + 버튼 누른 채 드래그.
    ButtonEvent,
    /// CSI ?1003: 모든 모션 + 버튼.
    AnyEvent,
}

/// M11-4: 7-bit input의 charset 매핑. G0만 추적.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Ascii,
    DecSpecialGraphics,
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
            cursor_keys_application: false,
            keypad_application: false,
            title: String::new(),
            title_dirty: false,
            bracketed_paste: false,
            focus_reporting: false,
            pending_responses: Vec::new(),
            g0_charset: Charset::Ascii,
            cwd: None,
            hyperlink_uri: None,
            last_prompt_row: None,
            prompts_seen: 0,
            last_command_exit: None,
            mouse_protocol: MouseProtocol::Off,
            mouse_sgr_encoding: false,
        }
    }

    pub fn mouse_protocol(&self) -> MouseProtocol {
        self.mouse_protocol
    }
    pub fn set_mouse_protocol(&mut self, p: MouseProtocol) {
        self.mouse_protocol = p;
    }
    pub fn mouse_sgr_encoding(&self) -> bool {
        self.mouse_sgr_encoding
    }
    pub fn set_mouse_sgr_encoding(&mut self, on: bool) {
        self.mouse_sgr_encoding = on;
    }

    // OSC 133 — semantic prompt 이벤트. 1차 cut은 메타데이터 추적만(block UI 인프라).
    /// `OSC 133;A` 시점에 호출. 현재 cursor row를 절대 행 번호로 보관.
    pub fn semantic_prompt_start(&mut self) {
        let absolute = self.scrollback.len() as u64 + self.cursor.row as u64;
        self.last_prompt_row = Some(absolute);
        self.prompts_seen = self.prompts_seen.saturating_add(1);
    }
    /// `OSC 133;D[;exit]` 시점에 호출. exit는 None이면 unknown.
    pub fn semantic_command_end(&mut self, exit: Option<i32>) {
        self.last_command_exit = exit;
    }
    pub fn last_prompt_row(&self) -> Option<u64> {
        self.last_prompt_row
    }
    pub fn prompts_seen(&self) -> u64 {
        self.prompts_seen
    }
    pub fn last_command_exit(&self) -> Option<i32> {
        self.last_command_exit
    }

    /// M11-4: G0 charset 지정 (ESC ( B = ASCII, ESC ( 0 = DEC special graphics).
    pub fn set_g0_charset(&mut self, charset: Charset) {
        self.g0_charset = charset;
    }
    pub fn g0_charset(&self) -> Charset {
        self.g0_charset
    }

    /// OSC 7 cwd. shell의 chpwd hook이 file URL을 보낼 때마다 갱신.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }
    pub fn set_cwd(&mut self, path: impl Into<String>) {
        self.cwd = Some(path.into());
    }

    /// OSC 8 hyperlink — 현재 active URI. None이면 normal text.
    /// `print()` 시 attrs에 HYPERLINK 플래그 켜고 별도 사이드테이블에 URI 매핑(차후).
    /// 1차 cut은 단순 plain text — URI는 추적만, 시각적 표현 X.
    pub fn hyperlink_uri(&self) -> Option<&str> {
        self.hyperlink_uri.as_deref()
    }
    pub fn set_hyperlink_uri(&mut self, uri: Option<String>) {
        self.hyperlink_uri = uri;
    }

    // M10-2: bracketed paste mode getter/setter.
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }
    pub fn set_bracketed_paste(&mut self, on: bool) {
        self.bracketed_paste = on;
    }

    // M10-3: focus reporting mode getter/setter.
    pub fn focus_reporting(&self) -> bool {
        self.focus_reporting
    }
    pub fn set_focus_reporting(&mut self, on: bool) {
        self.focus_reporting = on;
    }

    // M10-1: PTY 응답 채널.
    pub fn push_response(&mut self, bytes: Vec<u8>) {
        self.pending_responses.push(bytes);
    }
    pub fn drain_responses(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pending_responses)
    }

    // M8-7: title API.
    pub fn set_title(&mut self, t: String) {
        if self.title != t {
            self.title = t;
            self.title_dirty = true;
        }
    }
    pub fn take_title_if_changed(&mut self) -> Option<String> {
        if self.title_dirty {
            self.title_dirty = false;
            Some(self.title.clone())
        } else {
            None
        }
    }

    // M8-4 / M8-5 노출.
    pub fn cursor_keys_application(&self) -> bool {
        self.cursor_keys_application
    }
    pub fn set_cursor_keys_application(&mut self, on: bool) {
        self.cursor_keys_application = on;
    }
    pub fn is_alt_screen(&self) -> bool {
        self.use_alt
    }

    // M9-3: DECPAM/DECPNM keypad application mode.
    #[allow(dead_code)]
    pub fn keypad_application(&self) -> bool {
        self.keypad_application
    }
    pub fn set_keypad_application(&mut self, on: bool) {
        self.keypad_application = on;
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
        if self.use_alt { &self.alt } else { &self.main }
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
    /// (resize로 col이 변한 경우의 truncate-on-read; reflow는 M17-4 이후.)
    pub fn cell(&self, row: usize, col: usize) -> Cell {
        let scrollback_visible = self.view_offset.min(self.scrollback.len());
        if row < scrollback_visible {
            let sb_idx = self.scrollback.len() - scrollback_visible + row;
            return self
                .scrollback
                .get(sb_idx)
                .and_then(|r| r.cells.get(col).copied())
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

    /// view_offset 직접 설정. scrollback 길이로 클램프.
    pub fn set_view_offset(&mut self, offset: usize) {
        self.view_offset = offset.min(self.scrollback.len());
    }

    pub fn clear_scrollback(&mut self) {
        self.scrollback.clear();
        self.view_offset = 0;
    }

    pub fn clear_visible(&mut self) {
        self.erase_display(2);
        self.cursor.row = 0;
        self.cursor.col = 0;
    }

    pub fn clear_buffer(&mut self) {
        self.clear_scrollback();
        self.clear_visible();
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        // alt 모드: alt만 resize, main은 frozen.
        // main 모드: main reflow + alt도 resize (다음 alt 진입 시 정합).
        if self.use_alt {
            if cols != self.alt.cols || rows != self.alt.rows {
                self.alt.resize(cols, rows);
            }
        } else {
            let need = cols != self.main.cols || rows != self.main.rows;
            if need {
                // M17-4: scrollback + main 통합 reflow.
                self.reflow_all(cols, rows);
            }
            // alt도 항상 같은 사이즈 유지.
            if cols != self.alt.cols || rows != self.alt.rows {
                self.alt.resize(cols, rows);
            }
        }
        self.scroll_top = 0;
        self.scroll_bottom = rows;
        // cursor clamp는 reflow_all가 처리. alt 모드에선 saved_main_cursor가 frozen 좌표.
        // 안전망: 현재 cursor는 활성 grid 기준이라 한 번 더 clamp.
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        // cursor.col은 cols와 같을 수 있음(eager wrap pending). cols 초과만 clamp.
        if self.cursor.col > cols {
            self.cursor.col = cols;
        }
        self.view_offset = self.view_offset.min(self.scrollback.len());
        debug_assert_eq!(self.main.cells.len(), self.main.cols * self.main.rows);
        debug_assert_eq!(self.main.row_flags.len(), self.main.rows);
        debug_assert_eq!(self.alt.cells.len(), self.alt.cols * self.alt.rows);
        debug_assert_eq!(self.alt.row_flags.len(), self.alt.rows);
    }

    /// M17-4: scrollback + main 통합 reflow.
    /// 새 buffer + swap 패턴: 도중 panic 시 self 부분 mutate 방지.
    fn reflow_all(&mut self, new_cols: usize, new_rows: usize) {
        let sb_before = self.scrollback.len();
        let old_cols = self.main.cols;
        let old_rows = self.main.rows;

        // 평탄화 입력: scrollback rows + main rows 합쳐 한 시퀀스.
        let mut input: Vec<(Vec<Cell>, RowFlags)> = Vec::with_capacity(sb_before + old_rows);
        for sb_row in &self.scrollback {
            input.push((sb_row.cells.clone(), sb_row.flags));
        }
        for r in 0..old_rows {
            let start = r * old_cols;
            let end = start + old_cols;
            let cells = self.main.cells[start..end].to_vec();
            input.push((cells, self.main.row_flags[r]));
        }

        let cursor_row_input = sb_before + self.cursor.row.min(old_rows.saturating_sub(1));
        let cursor_col = self.cursor.col;
        let result = rewrap_lines(&input, cursor_row_input, cursor_col, new_cols);

        let total = result.new_rows.len();

        // partition: cursor를 main 안에 두고, 위쪽은 scrollback으로.
        let main_start = if total <= new_rows {
            0
        } else if result.cursor_global_row >= total - new_rows {
            total - new_rows
        } else {
            result
                .cursor_global_row
                .saturating_sub(new_rows.saturating_sub(1))
        };

        // 새 scrollback 빌드: NewRow[0..main_start]를 ScrollbackRow로.
        let mut new_scrollback: VecDeque<ScrollbackRow> = VecDeque::with_capacity(main_start);
        for src_idx in 0..main_start {
            let (cells, flags) = &result.new_rows[src_idx];
            new_scrollback.push_back(ScrollbackRow {
                cells: cells.clone(),
                flags: *flags,
            });
        }
        while new_scrollback.len() > SCROLLBACK_CAP {
            new_scrollback.pop_front();
        }

        // 새 main cells / row_flags 빌드.
        let mut new_cells = vec![Cell::default(); new_cols * new_rows];
        let mut new_flags = vec![RowFlags::empty(); new_rows];
        for r in 0..new_rows {
            let src_idx = main_start + r;
            if src_idx < total {
                let (cells, flag) = &result.new_rows[src_idx];
                let copy_len = cells.len().min(new_cols);
                new_cells[r * new_cols..r * new_cols + copy_len]
                    .copy_from_slice(&cells[..copy_len]);
                new_flags[r] = *flag;
            }
        }

        // 안전망: main 마지막 row의 WRAPPED는 chain 의미 없음(다음 row 없음). 클리어.
        if new_rows > 0 {
            new_flags[new_rows - 1].remove(RowFlags::WRAPPED);
        }

        // swap (panic safety).
        self.scrollback = new_scrollback;
        self.main.cells = new_cells;
        self.main.row_flags = new_flags;
        self.main.cols = new_cols;
        self.main.rows = new_rows;

        // cursor 매핑.
        let new_cursor_row = result
            .cursor_global_row
            .saturating_sub(main_start)
            .min(new_rows.saturating_sub(1));
        let new_cursor_col = result.cursor_new_col.min(new_cols);
        self.cursor.row = new_cursor_row;
        self.cursor.col = new_cursor_col;

        // view_offset 정책: resize 시 항상 snap to bottom.
        // (정밀 매핑은 §4.8 한계로 보류. resize는 명시적 액션이라 view reset이 자연스러움.)
        self.view_offset = 0;

        log::debug!(
            "reflow_all: {}x{} -> {}x{}, NewRows={}, main_start={}, cursor=({},{})->({},{}), sb {}->{}",
            old_cols,
            old_rows,
            new_cols,
            new_rows,
            total,
            main_start,
            cursor_row_input.saturating_sub(sb_before),
            cursor_col,
            new_cursor_row,
            new_cursor_col,
            sb_before,
            self.scrollback.len(),
        );
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
            for f in self.alt.row_flags.iter_mut() {
                *f = RowFlags::empty();
            }
            self.cursor = self.saved_alt_cursor;
        } else {
            self.saved_alt_cursor = self.cursor;
            self.use_alt = false;
            // 순서 중요: cursor를 saved_main_cursor로 복원(frozen 좌표) → 그 후 사이즈 mismatch면 reflow.
            self.cursor = self.saved_main_cursor;
            // alt 모드 중 viewport는 alt에 반영됨. main이 alt와 사이즈 다르면 reflow.
            if self.main.cols != self.alt.cols || self.main.rows != self.alt.rows {
                self.reflow_all(self.alt.cols, self.alt.rows);
            }
            self.scroll_top = 0;
            self.scroll_bottom = self.main.rows;
        }
    }

    pub fn print(&mut self, ch: char) {
        // M11-4: G0가 DEC special graphics일 때 0x60..=0x7e 영역 7-bit 글자를
        // 박스 드로잉 글리프로 변환. 그 외는 그대로.
        let ch = match self.g0_charset {
            Charset::Ascii => ch,
            Charset::DecSpecialGraphics => dec_special_translate(ch),
        };
        let w = UnicodeWidthChar::width(ch).unwrap_or(1);
        if w == 0 {
            return; // 결합 문자(combining)는 M5 범위 외
        }
        if self.cursor.col + w > self.cols() {
            // M17-2: wrap 발생 — 현재 row를 WRAPPED로 마크.
            // newline 직전 마킹: scroll_up이 발생해도 row_flags가 같이 shift됨.
            let row = self.cursor.row;
            self.grid_mut().row_flags[row].insert(RowFlags::WRAPPED);
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
        // 슬라이스 6.3b: OSC 8 active 동안 cells에 HYPERLINK 마킹.
        if self.hyperlink_uri.is_some() {
            attrs |= Attrs::HYPERLINK;
        }
        let g = self.grid_mut();
        *g.cell_mut(row, col) = Cell { ch, fg, bg, attrs };
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
                let cells: Vec<Cell> = self.main.cells[..cols].to_vec();
                let flags = self.main.row_flags[0];
                self.scrollback.push_back(ScrollbackRow { cells, flags });
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
        let old = self.cursor.row;
        self.cursor.row = self.cursor.row.saturating_sub(n);
        if self.cursor.row != old {
            self.break_wrap_chain_above_cursor();
        }
    }
    pub fn cursor_down(&mut self, n: usize) {
        let old = self.cursor.row;
        self.cursor.row = (self.cursor.row + n).min(self.rows().saturating_sub(1));
        if self.cursor.row != old {
            self.break_wrap_chain_above_cursor();
        }
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
        // CUP은 항상 "이 위치에서 다시 시작" 신호 → 위 chain 끊기.
        // (cursor_up/down은 row 변경 시만, set_cursor는 row 같아도 끊음)
        self.break_wrap_chain_above_cursor();
    }

    /// M17-2 보강: cursor가 row K로 점프했을 때 row K-1의 WRAPPED chain을 끊는다.
    /// 이유: WRAPPED semantic = "이 row의 마지막 cell이 다음 row의 첫 cell로 wrap continuation".
    /// cursor가 K로 점프해 K부터 새로 그려지면 K-1과의 continuation은 깨진 것.
    /// 이게 빠지면 reflow 시 stale chain으로 두 logical line이 잘못 합쳐짐.
    /// main grid의 row 0으로 점프하는 경우, 그 위는 scrollback last → 그것도 클리어.
    fn break_wrap_chain_above_cursor(&mut self) {
        let row = self.cursor.row;
        if row > 0 {
            let g = self.grid_mut();
            g.row_flags[row - 1].remove(RowFlags::WRAPPED);
        } else if !self.use_alt {
            // main grid row 0으로 점프: scrollback last와 chain이었다면 끊어야.
            if let Some(last) = self.scrollback.back_mut() {
                last.flags.remove(RowFlags::WRAPPED);
            }
        }
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
                // M17-2: 덮어쓴 영역의 WRAPPED flag 클리어.
                g.row_flags[cur_row] = RowFlags::empty();
                for r in (cur_row + 1)..rows {
                    g.row_flags[r] = RowFlags::empty();
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
                // M17-2: cursor row 위 모든 + cursor row 클리어.
                for r in 0..=cur_row {
                    g.row_flags[r] = RowFlags::empty();
                }
            }
            2 | 3 => {
                for c in g.cells.iter_mut() {
                    *c = Cell::default();
                }
                // M17-2: 모든 row flag 클리어.
                for f in g.row_flags.iter_mut() {
                    *f = RowFlags::empty();
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
        // M17-2: 어떤 mode든 line이 변경되면 WRAPPED 의미 잃음.
        if mode <= 2 {
            g.row_flags[cur_row] = RowFlags::empty();
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

    /// ICH — Insert N chars: cursor 위치부터 cells를 오른쪽으로 N칸 밀고, 비워진 N칸은
    /// blank로 채움. 행 끝을 넘어가는 cells는 truncate. cursor는 이동 안 함.
    pub fn insert_chars(&mut self, n: usize) {
        let cols = self.cols();
        let (row, col) = (self.cursor.row, self.cursor.col);
        if col >= cols || n == 0 {
            return;
        }
        let shift = n.min(cols - col);
        let g = self.grid_mut();
        for c in (col + shift..cols).rev() {
            *g.cell_mut(row, c) = *g.cell(row, c - shift);
        }
        for c in col..(col + shift) {
            *g.cell_mut(row, c) = Cell::default();
        }
        g.row_flags[row] = RowFlags::empty();
    }

    /// DCH — Delete N chars: cursor 위치의 cells를 N개 삭제, 우측을 왼쪽으로 끌어당기고
    /// 비워진 행 끝 N칸은 blank로 채움. cursor 이동 안 함.
    pub fn delete_chars(&mut self, n: usize) {
        let cols = self.cols();
        let (row, col) = (self.cursor.row, self.cursor.col);
        if col >= cols || n == 0 {
            return;
        }
        let shift = n.min(cols - col);
        let g = self.grid_mut();
        for c in col..(cols - shift) {
            *g.cell_mut(row, c) = *g.cell(row, c + shift);
        }
        for c in (cols - shift)..cols {
            *g.cell_mut(row, c) = Cell::default();
        }
        g.row_flags[row] = RowFlags::empty();
    }

    /// ECH — Erase N chars: cursor 위치부터 N개 cell을 blank로 덮어씀. shift 없음.
    pub fn erase_chars(&mut self, n: usize) {
        let cols = self.cols();
        let (row, col) = (self.cursor.row, self.cursor.col);
        if col >= cols || n == 0 {
            return;
        }
        let end = (col + n).min(cols);
        let g = self.grid_mut();
        for c in col..end {
            *g.cell_mut(row, c) = Cell::default();
        }
        g.row_flags[row] = RowFlags::empty();
    }

    /// IL — Insert N blank lines at cursor row within scroll region.
    /// cursor가 region 밖이면 no-op. cursor 자체는 이동 안 함(xterm 동작).
    pub fn insert_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if !(self.scroll_top..self.scroll_bottom).contains(&row) || n == 0 {
            return;
        }
        let bottom = self.scroll_bottom;
        // cursor row를 sub-region의 top으로 보고 그 안에서 scroll_down 호출.
        self.grid_mut().scroll_down(row, bottom, n);
    }

    /// DL — Delete N lines at cursor row within scroll region.
    /// 잔여 행은 위로 올라오고, 영역 하단 N행은 blank.
    pub fn delete_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if !(self.scroll_top..self.scroll_bottom).contains(&row) || n == 0 {
            return;
        }
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_up(row, bottom, n);
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

    /// DECSTR — Soft Terminal Reset (`CSI ! p`). xterm/VT220 표준.
    /// settable mode + SGR + saved cursor만 default로. 화면 콘텐츠/tab stop/title은 보존.
    pub fn soft_reset(&mut self) {
        // SGR
        self.reset_sgr();
        // mode flags
        self.cursor_keys_application = false;
        self.keypad_application = false;
        self.bracketed_paste = false;
        self.focus_reporting = false;
        // cursor 가시성/모양은 default (block, blink, visible)
        self.cursor.visible = true;
        self.cursor.shape = CursorShape::Block;
        self.cursor.blinking = true;
        // scroll region full
        let rows = self.rows();
        self.scroll_top = 0;
        self.scroll_bottom = rows;
        // DECSC saved slot 초기화 (xterm: 둘 다 home)
        self.saved_main_cursor = Cursor::default();
        self.saved_alt_cursor = Cursor::default();
        // G0 charset → ASCII (M11-4)
        self.g0_charset = Charset::Ascii;
        // 슬라이스 6.6: mouse reporting reset
        self.mouse_protocol = MouseProtocol::Off;
        self.mouse_sgr_encoding = false;
    }

    /// RIS — Reset to Initial State (`ESC c`). 화면 + cursor + 모든 mode 풀 리셋.
    /// DECSTR + screen erase + cursor to (0,0) + alt→main. scrollback은 보존 (xterm 동작).
    pub fn full_reset(&mut self) {
        // alt screen 진입 중이면 main 복귀
        if self.use_alt {
            self.switch_alt_screen(false);
        }
        self.soft_reset();
        // 화면 클리어
        self.erase_display(2);
        // cursor home
        self.cursor.row = 0;
        self.cursor.col = 0;
        // title 유지 (xterm 기본)
        // pending_responses는 그대로 (이미 큐된 응답은 보낸다)
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

    // M17-2: 테스트/디버깅용 row_flags 접근자. 외부 노출 안 함.
    #[cfg(test)]
    fn row_flags(&self, row: usize) -> RowFlags {
        self.grid().row_flags[row]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print_str(term: &mut Term, s: &str) {
        for ch in s.chars() {
            term.print(ch);
        }
    }

    #[test]
    fn print_overflow_marks_wrapped() {
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789ABCDEF"); // 16 chars in 10 cols
        // row 0은 wrap된 source → WRAPPED set
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        // row 1은 continuation의 시작 → WRAPPED 안 set (다음 wrap 없음)
        assert!(!term.row_flags(1).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn print_no_overflow_no_wrapped() {
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789"); // 10 chars exactly, no overflow yet
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
        // 한 글자 더 → wrap 발생
        term.print('X');
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn erase_line_clears_wrapped() {
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789AB"); // wrap 발생
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        // cursor를 row 0로
        term.set_cursor(0, 0);
        term.erase_line(2);
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn erase_display_2_clears_all_wrapped() {
        let mut term = Term::new(10, 5);
        // row 0, row 1 다 wrapped 상태로
        print_str(&mut term, "0123456789ABCDEFGHIJKLM"); // 23 chars, 3 rows wrap
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        assert!(term.row_flags(1).contains(RowFlags::WRAPPED));
        term.erase_display(2);
        for r in 0..5 {
            assert!(!term.row_flags(r).contains(RowFlags::WRAPPED), "row {}", r);
        }
    }

    #[test]
    fn clear_scrollback_preserves_visible_grid() {
        let mut term = Term::new(4, 2);
        print_str(&mut term, "abcd");
        term.newline();
        term.print('e');
        assert!(term.scrollback_len() > 0);
        let mut visible_before = Vec::new();
        for row in 0..term.rows() {
            for col in 0..term.cols() {
                visible_before.push(term.cell(row, col));
            }
        }

        term.clear_scrollback();

        assert_eq!(term.scrollback_len(), 0);
        assert_eq!(term.view_offset(), 0);
        let mut visible_after = Vec::new();
        for row in 0..term.rows() {
            for col in 0..term.cols() {
                visible_after.push(term.cell(row, col));
            }
        }
        assert_eq!(visible_after, visible_before);
    }

    #[test]
    fn clear_buffer_clears_scrollback_and_visible_grid() {
        let mut term = Term::new(4, 2);
        print_str(&mut term, "abcd");
        term.newline();
        term.print('e');
        assert!(term.scrollback_len() > 0);

        term.clear_buffer();

        assert_eq!(term.scrollback_len(), 0);
        assert_eq!(term.view_offset(), 0);
        assert_eq!(term.cursor().row, 0);
        assert_eq!(term.cursor().col, 0);
        for row in 0..term.rows() {
            for col in 0..term.cols() {
                assert_eq!(term.cell(row, col), Cell::default());
            }
        }
    }

    #[test]
    fn scroll_up_shifts_wrapped() {
        // scroll_up이 row_flags도 같이 시프트한다는 것을 grid 내 시프트로 직접 검증.
        // (scrollback push 동반 케이스는 scrollback_push_preserves_wrapped에서 검증)
        let mut term = Term::new(10, 3);
        // row 1을 WRAPPED로 만들기 위해 cursor를 row 1로 옮기고 wrap 발생시킴.
        term.set_cursor(1, 0);
        print_str(&mut term, "0123456789X"); // 11 chars → row 1 WRAPPED, row 2[0]='X'
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
        assert!(term.row_flags(1).contains(RowFlags::WRAPPED));
        assert!(!term.row_flags(2).contains(RowFlags::WRAPPED));

        term.scroll_up_n(1); // scroll region 0..3 위로 1행 시프트
        // 결과: row 0 = 이전 row 1 (WRAPPED), row 1 = 이전 row 2 (not), row 2 = empty
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        assert!(!term.row_flags(1).contains(RowFlags::WRAPPED));
        assert!(!term.row_flags(2).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn scroll_down_shifts_wrapped() {
        let mut term = Term::new(10, 3);
        // row 0을 WRAPPED로
        print_str(&mut term, "0123456789X"); // 11 chars → row 0 WRAPPED, row 1[0]='X'
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));

        term.scroll_down_n(1); // 위로 1행 시프트(아래로 미는 게 아니라 region 안에서 row 0 → row 1)
        // 결과: row 0 = empty, row 1 = 이전 row 0 (WRAPPED), row 2 = 이전 row 1
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
        assert!(term.row_flags(1).contains(RowFlags::WRAPPED));
        assert!(!term.row_flags(2).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn scrollback_push_preserves_wrapped() {
        let mut term = Term::new(10, 2); // 2 rows로 scrollback push 빨리
        print_str(&mut term, "0123456789AB"); // 12 chars, row 0 WRAPPED, row 1 partial
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        // 한 줄 더로 scroll 트리거
        term.newline(); // cursor row 1→? 1이 마지막이라 scroll_up + push row 0
        // scrollback에 row 0(WRAPPED) push되었어야
        assert_eq!(term.scrollback.len(), 1);
        assert!(term.scrollback[0].flags.contains(RowFlags::WRAPPED));
    }

    // M17-3 — main grid reflow + cursor 매핑

    fn dump_main_chars(term: &Term) -> Vec<String> {
        let mut out = Vec::new();
        for r in 0..term.main.rows {
            let mut s = String::new();
            for c in 0..term.main.cols {
                s.push(term.main.cell(r, c).ch);
            }
            out.push(s);
        }
        out
    }

    #[test]
    fn reflow_widen_merges_wrapped_line() {
        // T5: 좁→넓 — wrapped 한 logical line이 합쳐짐.
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789ABCDE"); // 15 chars: row 0 WRAPPED, row 1 "ABCDE"
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        // 20 cols로 확장
        term.resize(20, 5);
        // logical line "0123456789ABCDE"가 한 row로 합쳐져야
        let dump = dump_main_chars(&term);
        assert_eq!(&dump[0][..15], "0123456789ABCDE");
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn reflow_narrow_splits_long_line() {
        // T6: 넓→좁 — 한 row가 N rows로 wrap + 마지막 제외 모두 WRAPPED.
        // M17-3 한정: scrollback push 안 함 → partition 잘림 회피 위해 rows를 충분히.
        let mut term = Term::new(20, 5);
        print_str(&mut term, "0123456789ABCDE"); // 15 chars, row 0 not wrapped
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
        // 5 cols, 10 rows로 — partition 위쪽 잘림 회피
        term.resize(5, 10);
        let dump = dump_main_chars(&term);
        assert_eq!(&dump[0], "01234");
        assert_eq!(&dump[1], "56789");
        assert_eq!(&dump[2], "ABCDE");
        // 0, 1 WRAPPED. 2는 logical line의 끝이라 not WRAPPED.
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        assert!(term.row_flags(1).contains(RowFlags::WRAPPED));
        assert!(!term.row_flags(2).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn reflow_cursor_in_wrapped_middle() {
        // T7: cursor가 wrapped logical line 중간 → reflow 후 정확.
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789ABCDE"); // row 0 WRAPPED, row 1: "ABCDE", cursor row 1, col 5
        assert_eq!(term.cursor.row, 1);
        assert_eq!(term.cursor.col, 5);
        // 20 cols로 확장: logical line "0123456789ABCDE"가 row 0에 합쳐짐. cursor offset 15.
        term.resize(20, 5);
        // cursor는 row 0, col 15여야 (cells.len() == 15, eager wrap pending)
        assert_eq!(term.cursor.row, 0);
        assert_eq!(term.cursor.col, 15);
    }

    #[test]
    fn reflow_eager_wrap_pending_cursor() {
        // T13: cursor.col == cols 상태에서 reflow.
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789"); // 10 chars, cursor.col == 10 (eager wrap pending), no overflow yet
        assert_eq!(term.cursor.col, 10);
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
        // 5 cols, 10 rows로 — partition 잘림 회피
        term.resize(5, 10);
        // logical line "0123456789" → 5+5 wrap. row 0 "01234" WRAPPED, row 1 "56789" not (cells.len()=5).
        // cursor offset 10 = combined.len() → map_cursor_in_line이 마지막 row의 col=5 반환.
        // 즉 cursor (1, 5) — eager wrap pending in new size.
        assert_eq!(term.cursor.row, 1);
        assert_eq!(term.cursor.col, 5);
    }

    #[test]
    fn reflow_alt_mode_freezes_main() {
        // T15: alt 모드 중 main frozen + 종료 시 reflow.
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789ABCDE"); // row 0 WRAPPED, row 1 "ABCDE"
        let main_dump_before = dump_main_chars(&term);

        term.switch_alt_screen(true);
        // alt 진입 후 resize: alt만 변경, main frozen.
        term.resize(20, 5);
        // main grid 그대로
        assert_eq!(term.main.cols, 10);
        assert_eq!(term.main.rows, 5);
        for (r, line) in main_dump_before.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                assert_eq!(term.main.cell(r, c).ch, ch, "main row {} col {}", r, c);
            }
        }

        // alt 종료: main이 alt 사이즈로 reflow. logical line이 합쳐짐.
        term.switch_alt_screen(false);
        assert_eq!(term.main.cols, 20);
        assert_eq!(term.main.rows, 5);
        let dump = dump_main_chars(&term);
        assert_eq!(&dump[0][..15], "0123456789ABCDE");
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
    }

    // M17-4 — scrollback 통합 reflow

    #[test]
    fn reflow_widen_pulls_from_scrollback() {
        // 작은 grid에 많은 내용 → scrollback 채워짐 → 넓히면 main 위가 scrollback에서 채워져야.
        let mut term = Term::new(10, 3);
        // 5줄 출력 → 위 2줄은 scrollback으로 push.
        for i in 0..5 {
            print_str(&mut term, &format!("line{i}"));
            term.print('\n'); // CR 없이 LF만이라도 newline 트리거 안 됨, 직접 호출.
            term.newline();
            term.carriage_return();
        }
        let sb_before = term.scrollback.len();
        assert!(
            sb_before > 0,
            "scrollback should have rows; got {sb_before}"
        );

        // 넓히기 + rows도 키워서 scrollback 일부가 main으로 끌려옴.
        term.resize(20, 8);
        // scrollback 줄어들고 main 채워짐.
        let sb_after = term.scrollback.len();
        assert!(sb_after < sb_before, "sb {sb_before} -> {sb_after}");
    }

    #[test]
    fn reflow_evicts_when_over_cap() {
        // SCROLLBACK_CAP 초과 시 reflow 후 cap 적용 → 오래된 게 drop.
        let mut term = Term::new(10, 3);
        // cap 살짝 넘게 채움 (10010 줄).
        for i in 0..(SCROLLBACK_CAP + 10) {
            print_str(&mut term, &format!("L{i}"));
            term.newline();
            term.carriage_return();
        }
        // print 도중에도 newline의 push 단계에서 cap이 동작하므로 여기서 이미 cap.
        assert!(term.scrollback.len() <= SCROLLBACK_CAP);

        // 좁히면 logical line이 늘어나 reflow 후에도 cap 유지.
        term.resize(5, 3);
        assert!(term.scrollback.len() <= SCROLLBACK_CAP);
        // 마지막 출력은 main 또는 scrollback 끝쪽에 보존되어야.
        let last_str = format!("L{}", SCROLLBACK_CAP + 9);
        let dump = dump_main_chars(&term);
        let in_main = dump.iter().any(|s| s.contains(&last_str[..2]));
        let in_sb = term.scrollback.iter().rev().take(5).any(|r| {
            r.cells
                .iter()
                .any(|c| c.ch == last_str.chars().next().unwrap())
        });
        assert!(in_main || in_sb, "최신 줄 보존");
    }

    #[test]
    fn reflow_view_offset_snaps_to_bottom() {
        // 정책: resize 시 view_offset 0으로 snap (§4.8 정책 변경).
        let mut term = Term::new(10, 3);
        for i in 0..10 {
            print_str(&mut term, &format!("L{i}"));
            term.newline();
            term.carriage_return();
        }
        term.scroll_view_by(3);
        assert!(term.view_offset() > 0);
        term.resize(5, 3);
        assert_eq!(term.view_offset(), 0);
    }

    #[test]
    fn cup_to_main_row0_breaks_scrollback_last_wrap() {
        // scrollback last가 WRAPPED인 상태에서 main row 0으로 CUP 점프 → scrollback last의 WRAPPED 끊겨야.
        let mut term = Term::new(10, 2);
        // 12자 print → row 0 WRAPPED, row 1 partial. 한 줄 더 → scroll_up + scrollback push.
        print_str(&mut term, "0123456789AB");
        term.newline();
        term.carriage_return();
        // scrollback last가 WRAPPED여야 (push 시점 row 0가 WRAPPED)
        assert!(
            term.scrollback
                .back()
                .unwrap()
                .flags
                .contains(RowFlags::WRAPPED)
        );
        // CUP으로 row 0으로 점프
        term.set_cursor(0, 0);
        // scrollback last의 WRAPPED는 클리어되었어야
        assert!(
            !term
                .scrollback
                .back()
                .unwrap()
                .flags
                .contains(RowFlags::WRAPPED)
        );
    }

    #[test]
    fn cup_breaks_wrap_chain_above() {
        // 30 cols로 wrap 만든 후 CUP으로 chain 안쪽 row로 점프 → row K-1 WRAPPED 클리어 확인.
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789ABCDE"); // row 0 WRAPPED, row 1 "ABCDE"
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        // CUP으로 row 1로 점프 (col=0)
        term.set_cursor(1, 0);
        // row 0의 WRAPPED는 클리어되었어야
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn reflow_does_not_merge_stale_wrap_after_cup() {
        // 정확 재현 시나리오 (advisor 가설):
        //   1. 좁은 grid(10 cols)에서 30 a 출력 → row 0,1 WRAPPED, row 2 "aaaaaaaaaa" (chain의 끝, not WRAPPED)
        //   2. CUP으로 row 2(chain의 마지막 row)로 점프해 그 위에 PROMPT 그림
        //      → 만약 row 1 WRAPPED 클리어 안 되면 chain "0..A"+"BCDEF"+"PROMPT..."가 합쳐짐
        //   3. resize → 단일 logical line으로 잘못 합쳐지면 안 됨
        use vte::Parser;
        let mut term = Term::new(10, 5);
        let mut parser = Parser::new();
        let mut perform = crate::vt::TermPerform::new(&mut term);
        // 30 a (CR/LF 없이 — 자연 wrap)
        parser.advance(&mut perform, b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        // row 0: 10a WRAPPED, row 1: 10a WRAPPED, row 2: 10a not WRAPPED, cursor (3, 0)
        // CUP으로 row 2 col 0으로 점프 (1-based: 3,1) → row 1의 WRAPPED chain 끊겨야
        parser.advance(&mut perform, b"\x1b[3;1HPROMPT> ");
        drop(perform);

        // resize 좁힘 → 넓힘 사이클
        term.resize(40, 5);
        let dump = dump_main_chars(&term);
        // a 줄 다음에 P가 바로 붙어있으면(merged) fail.
        for (r, line) in dump.iter().enumerate() {
            let trimmed = line.trim_end();
            assert!(
                !trimmed.contains("aP"),
                "row {r} merged stale wrap: {line:?}"
            );
        }
    }

    #[test]
    fn reflow_invariants_hold_after_resize() {
        // T14 — invariant 유지.
        let mut term = Term::new(10, 5);
        print_str(&mut term, "0123456789ABCDE");
        term.resize(20, 7);
        assert_eq!(term.main.cells.len(), term.main.cols * term.main.rows);
        assert_eq!(term.main.row_flags.len(), term.main.rows);
        assert_eq!(term.alt.cells.len(), term.alt.cols * term.alt.rows);
        assert_eq!(term.alt.row_flags.len(), term.alt.rows);
        assert!(term.cursor.row < term.main.rows);
        assert!(term.cursor.col <= term.main.cols);
    }

    // M17-5 — WIDE 경계

    #[test]
    fn reflow_wide_at_boundary_padding() {
        // 3 cols. "한a한" = WIDE+a+WIDE.
        // 입력: 4 cols grid에서 출력 후 3 cols로 reflow.
        let mut term = Term::new(4, 3);
        term.print('한'); // (0, 0..2) WIDE+WIDE_CONT
        term.print('a'); // (0, 2)
        term.print('한'); // 마지막 1칸 남음 → wrap → (1, 0..2) WIDE
        // row 0: 한 a, row 1: 한
        let dump_old = dump_main_chars(&term);
        assert!(dump_old[0].contains('한'), "{:?}", dump_old);

        term.resize(3, 5);
        // 3 cols로: 한(2 cells) + a(1) → row 0 가득. 다음 한은 row 0 안 들어감(2 cells 필요). row 0 WRAPPED + row 1 한.
        let dump = dump_main_chars(&term);
        // row 0의 cells 2개가 한, 마지막 1칸은 a 또는 default.
        // row 1은 한.
        let row0_chars: String = dump[0].chars().collect();
        let row1_chars: String = dump[1].chars().collect();
        assert!(row0_chars.contains('한'), "row0: {row0_chars:?}");
        assert!(row1_chars.contains('한'), "row1: {row1_chars:?}");
    }

    #[test]
    fn reflow_wide_split_avoided_by_padding() {
        // WIDE 분할 금지: cols=3에서 row 마지막 1칸이 비고 다음 글자가 WIDE면 padding default + WIDE 다음 row.
        // partition으로 'aa'는 scrollback, 한은 main.
        let mut term = Term::new(10, 10);
        term.print('a');
        term.print('a');
        term.print('한');
        term.resize(3, 10);
        // 한 row 검증: main에 한이 분할 없이 (cell 0 = 한, cell 1 = WIDE_CONT).
        let dump = dump_main_chars(&term);
        let han_row_idx = dump
            .iter()
            .position(|r| r.starts_with('한'))
            .expect(&format!("한 row missing in main: {dump:?}"));
        // 한 row의 cell 1이 WIDE_CONT (분할 없음).
        let main_grid_row = han_row_idx;
        let cell1 = term.main.cell(main_grid_row, 1);
        assert!(
            cell1.attrs.contains(Attrs::WIDE_CONT),
            "WIDE 분할 발생: cell1 = {cell1:?}"
        );

        // 'aa' row가 scrollback에 있어야. 마지막 push된 row는 wrap의 첫 부분.
        let aa_in_sb = term
            .scrollback
            .iter()
            .any(|r| r.cells.iter().take(2).all(|c| c.ch == 'a'));
        assert!(
            aa_in_sb,
            "'aa' row missing in scrollback: sb={:?}",
            term.scrollback
        );
    }

    #[test]
    fn alt_screen_clears_row_flags() {
        let mut term = Term::new(10, 3);
        print_str(&mut term, "0123456789AB"); // row 0 WRAPPED
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        term.switch_alt_screen(true);
        // alt grid row_flags 모두 비어있어야
        for r in 0..3 {
            assert!(
                !term.row_flags(r).contains(RowFlags::WRAPPED),
                "alt row {}",
                r
            );
        }
        term.switch_alt_screen(false);
        // main 복귀: row 0 WRAPPED 그대로
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
    }

    fn row_chars(term: &Term, row: usize) -> String {
        (0..term.cols()).map(|c| term.cell(row, c).ch).collect()
    }

    #[test]
    fn insert_chars_shifts_right_and_blanks() {
        let mut term = Term::new(8, 2);
        print_str(&mut term, "abcdef");
        term.set_cursor(0, 2);
        term.insert_chars(2);
        // "abcdef__" → cursor=col2 → "ab  cdef" (마지막 2칸 truncate 없음, len 6 → 8)
        assert_eq!(row_chars(&term, 0), "ab  cdef");
    }

    #[test]
    fn insert_chars_truncates_at_end() {
        let mut term = Term::new(5, 2);
        print_str(&mut term, "abcde");
        term.set_cursor(0, 1);
        term.insert_chars(3);
        // "abcde" + col 1에서 3 insert → "a   b" (cde truncate)
        assert_eq!(row_chars(&term, 0), "a   b");
    }

    #[test]
    fn insert_chars_clears_wrapped() {
        let mut term = Term::new(5, 2);
        print_str(&mut term, "abcdef"); // wrap
        assert!(term.row_flags(0).contains(RowFlags::WRAPPED));
        term.set_cursor(0, 1);
        term.insert_chars(1);
        assert!(!term.row_flags(0).contains(RowFlags::WRAPPED));
    }

    #[test]
    fn delete_chars_shifts_left_and_blanks_end() {
        let mut term = Term::new(8, 2);
        print_str(&mut term, "abcdef");
        term.set_cursor(0, 1);
        term.delete_chars(2);
        // "abcdef" → cursor=1, delete 2 → "adef    "
        assert_eq!(row_chars(&term, 0), "adef    ");
    }

    #[test]
    fn delete_chars_more_than_remaining_blanks_rest() {
        let mut term = Term::new(5, 2);
        print_str(&mut term, "abcde");
        term.set_cursor(0, 2);
        term.delete_chars(99);
        assert_eq!(row_chars(&term, 0), "ab   ");
    }

    #[test]
    fn erase_chars_blanks_without_shift() {
        let mut term = Term::new(8, 2);
        print_str(&mut term, "abcdef");
        term.set_cursor(0, 1);
        term.erase_chars(3);
        // shift 없음 — "abcdef" → "a   ef"
        assert_eq!(row_chars(&term, 0), "a   ef  ");
    }

    #[test]
    fn erase_chars_clamps_to_line_end() {
        let mut term = Term::new(5, 2);
        print_str(&mut term, "abcde");
        term.set_cursor(0, 3);
        term.erase_chars(99);
        assert_eq!(row_chars(&term, 0), "abc  ");
    }

    #[test]
    fn insert_lines_shifts_lines_down_within_region() {
        let mut term = Term::new(4, 4);
        print_str(&mut term, "AAAA");
        term.set_cursor(1, 0);
        print_str(&mut term, "BBBB");
        term.set_cursor(2, 0);
        print_str(&mut term, "CCCC");
        // cursor를 row 1로 보내고 IL 1
        term.set_cursor(1, 0);
        term.insert_lines(1);
        assert_eq!(row_chars(&term, 0), "AAAA");
        assert_eq!(row_chars(&term, 1), "    "); // 새 빈 row
        assert_eq!(row_chars(&term, 2), "BBBB"); // 밀려남
        assert_eq!(row_chars(&term, 3), "CCCC"); // 밀려남
    }

    #[test]
    fn delete_lines_shifts_lines_up_within_region() {
        let mut term = Term::new(4, 4);
        print_str(&mut term, "AAAA");
        term.set_cursor(1, 0);
        print_str(&mut term, "BBBB");
        term.set_cursor(2, 0);
        print_str(&mut term, "CCCC");
        term.set_cursor(3, 0);
        print_str(&mut term, "DDDD");
        term.set_cursor(1, 0);
        term.delete_lines(1);
        assert_eq!(row_chars(&term, 0), "AAAA"); // 변함 없음
        assert_eq!(row_chars(&term, 1), "CCCC"); // BBBB 삭제, CCCC 끌어올림
        assert_eq!(row_chars(&term, 2), "DDDD");
        assert_eq!(row_chars(&term, 3), "    "); // 빈 row
    }

    #[test]
    fn insert_lines_noop_outside_scroll_region() {
        let mut term = Term::new(4, 5);
        term.set_scroll_region(1, 4); // 1..4
        term.set_cursor(0, 0); // 영역 밖
        print_str(&mut term, "AAAA");
        let snap = row_chars(&term, 0);
        term.insert_lines(2);
        assert_eq!(row_chars(&term, 0), snap);
    }

    #[test]
    fn delete_lines_noop_when_n_zero() {
        let mut term = Term::new(4, 3);
        print_str(&mut term, "AAAA");
        term.set_cursor(0, 0);
        term.delete_lines(0);
        assert_eq!(row_chars(&term, 0), "AAAA");
    }
}
