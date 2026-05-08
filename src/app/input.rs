use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// 키 입력 인코딩 시점에 참조되는 Term/AppState 상태 스냅샷.
/// (advisor lock-race 가이드: 키 입력당 한 번만 lock 잡고 채워서 사용)
#[derive(Default, Clone, Copy)]
pub struct InputMode {
    /// DECCKM (M8-4). 활성 시 화살표 → SS3 인코딩.
    pub cursor_keys_application: bool,
    /// alt screen 여부 (M8-5 PageUp/Down 분기용).
    #[allow(dead_code)]
    pub alt_screen: bool,
    /// modifier — M9 인프라. M8은 Ctrl/Super만 일부 사용.
    pub modifiers: ModifiersState,
}

pub fn encode_key(event: &KeyEvent, mode: InputMode) -> Option<Vec<u8>> {
    if event.state != ElementState::Pressed {
        return None;
    }
    // 1. NamedKey 매핑 시도
    if let Key::Named(named) = &event.logical_key {
        if let Some(bytes) = encode_named_key(named, mode) {
            return Some(bytes.to_vec());
        }
    }
    // 2. Ctrl + 글자 명시 매핑 (winit text 의존을 줄임).
    if mode.modifiers.control_key() {
        if let Key::Character(s) = &event.logical_key {
            if let Some(c) = s.chars().next() {
                if let Some(b) = encode_ctrl_char(c) {
                    return Some(vec![b]);
                }
            }
        }
    }
    // 3. text fallback (한글 IME unicode 등).
    event.text.as_ref().map(|s| s.as_bytes().to_vec())
}

/// NamedKey만 처리. Key::Character 등은 호출자가 별도 처리.
fn encode_named_key(key: &NamedKey, mode: InputMode) -> Option<&'static [u8]> {
    let app = mode.cursor_keys_application;
    let shift = mode.modifiers.shift_key();
    match key {
        NamedKey::Enter => Some(b"\r"),
        NamedKey::Backspace => Some(&[0x7F]),
        // Shift+Tab → CSI Z (CBT, back tab). 일반 Tab은 \t.
        NamedKey::Tab if shift => Some(b"\x1b[Z"),
        NamedKey::Tab => Some(b"\t"),
        NamedKey::Escape => Some(b"\x1b"),
        // 화살표 — DECCKM 분기.
        NamedKey::ArrowUp => Some(if app { b"\x1bOA" } else { b"\x1b[A" }),
        NamedKey::ArrowDown => Some(if app { b"\x1bOB" } else { b"\x1b[B" }),
        NamedKey::ArrowRight => Some(if app { b"\x1bOC" } else { b"\x1b[C" }),
        NamedKey::ArrowLeft => Some(if app { b"\x1bOD" } else { b"\x1b[D" }),
        // 위치 키 — Home/End는 SS3 form (macOS Terminal.app/iTerm2 표준, zsh default binding).
        // 단순화로 DECCKM 무관 SS3 고정. 비호환 앱 발견 시 M9에서 분기 추가.
        NamedKey::Home => Some(b"\x1bOH"),
        NamedKey::End => Some(b"\x1bOF"),
        NamedKey::Insert => Some(b"\x1b[2~"),
        NamedKey::Delete => Some(b"\x1b[3~"),
        // PageUp/Down byte 매핑. 분기(scrollback vs PTY)는 M8-5에서 호출자가 결정.
        NamedKey::PageUp => Some(b"\x1b[5~"),
        NamedKey::PageDown => Some(b"\x1b[6~"),
        // F1-F4 — VT100 PF1-PF4 (SS3 prefix). xterm default unconditional.
        NamedKey::F1 => Some(b"\x1bOP"),
        NamedKey::F2 => Some(b"\x1bOQ"),
        NamedKey::F3 => Some(b"\x1bOR"),
        NamedKey::F4 => Some(b"\x1bOS"),
        // F5-F12 — CSI ~ form. F6 다음 16, F11 다음 22가 결번 (xterm 표준).
        NamedKey::F5 => Some(b"\x1b[15~"),
        NamedKey::F6 => Some(b"\x1b[17~"),
        NamedKey::F7 => Some(b"\x1b[18~"),
        NamedKey::F8 => Some(b"\x1b[19~"),
        NamedKey::F9 => Some(b"\x1b[20~"),
        NamedKey::F10 => Some(b"\x1b[21~"),
        NamedKey::F11 => Some(b"\x1b[23~"),
        NamedKey::F12 => Some(b"\x1b[24~"),
        _ => None,
    }
}

/// Ctrl + char → ASCII 컨트롤 코드 (0x00-0x1F, 0x7F).
fn encode_ctrl_char(c: char) -> Option<u8> {
    let lower = c.to_ascii_lowercase();
    match lower {
        'a'..='z' => Some(lower as u8 - b'a' + 1),
        '@' => Some(0x00),
        '[' => Some(0x1B),
        '\\' => Some(0x1C),
        ']' => Some(0x1D),
        '^' => Some(0x1E),
        '_' => Some(0x1F),
        '?' => Some(0x7F),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(app: bool) -> InputMode {
        InputMode {
            cursor_keys_application: app,
            alt_screen: false,
            modifiers: ModifiersState::empty(),
        }
    }

    #[test]
    fn arrow_normal() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), Some(&b"\x1b[A"[..]));
        assert_eq!(encode_named_key(&NamedKey::ArrowDown, m), Some(&b"\x1b[B"[..]));
        assert_eq!(encode_named_key(&NamedKey::ArrowRight, m), Some(&b"\x1b[C"[..]));
        assert_eq!(encode_named_key(&NamedKey::ArrowLeft, m), Some(&b"\x1b[D"[..]));
    }

    #[test]
    fn arrow_application_mode() {
        let m = mode(true);
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), Some(&b"\x1bOA"[..]));
        assert_eq!(encode_named_key(&NamedKey::ArrowDown, m), Some(&b"\x1bOB"[..]));
        assert_eq!(encode_named_key(&NamedKey::ArrowRight, m), Some(&b"\x1bOC"[..]));
        assert_eq!(encode_named_key(&NamedKey::ArrowLeft, m), Some(&b"\x1bOD"[..]));
    }

    #[test]
    fn enter_backspace_tab_escape() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::Enter, m), Some(&b"\r"[..]));
        assert_eq!(encode_named_key(&NamedKey::Backspace, m), Some(&[0x7F][..]));
        assert_eq!(encode_named_key(&NamedKey::Tab, m), Some(&b"\t"[..]));
        assert_eq!(encode_named_key(&NamedKey::Escape, m), Some(&b"\x1b"[..]));
    }

    #[test]
    fn ctrl_letter_to_ascii_control() {
        // Ctrl+A → 0x01, Ctrl+C → 0x03, Ctrl+Z → 0x1A
        assert_eq!(encode_ctrl_char('a'), Some(0x01));
        assert_eq!(encode_ctrl_char('A'), Some(0x01)); // 대소문자 무관
        assert_eq!(encode_ctrl_char('c'), Some(0x03));
        assert_eq!(encode_ctrl_char('z'), Some(0x1A));
    }

    #[test]
    fn ctrl_special_chars() {
        assert_eq!(encode_ctrl_char('@'), Some(0x00));
        assert_eq!(encode_ctrl_char('['), Some(0x1B));
        assert_eq!(encode_ctrl_char('\\'), Some(0x1C));
        assert_eq!(encode_ctrl_char(']'), Some(0x1D));
        assert_eq!(encode_ctrl_char('^'), Some(0x1E));
        assert_eq!(encode_ctrl_char('_'), Some(0x1F));
        assert_eq!(encode_ctrl_char('?'), Some(0x7F));
    }

    #[test]
    fn ctrl_unknown_char_returns_none() {
        assert_eq!(encode_ctrl_char('1'), None);
        assert_eq!(encode_ctrl_char('!'), None);
    }

    #[test]
    fn position_keys() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::Home, m), Some(&b"\x1bOH"[..]));
        assert_eq!(encode_named_key(&NamedKey::End, m), Some(&b"\x1bOF"[..]));
        assert_eq!(encode_named_key(&NamedKey::Insert, m), Some(&b"\x1b[2~"[..]));
        assert_eq!(encode_named_key(&NamedKey::Delete, m), Some(&b"\x1b[3~"[..]));
        assert_eq!(encode_named_key(&NamedKey::PageUp, m), Some(&b"\x1b[5~"[..]));
        assert_eq!(encode_named_key(&NamedKey::PageDown, m), Some(&b"\x1b[6~"[..]));
    }

    #[test]
    fn shift_tab_back() {
        let mut m = mode(false);
        m.modifiers = ModifiersState::SHIFT;
        assert_eq!(encode_named_key(&NamedKey::Tab, m), Some(&b"\x1b[Z"[..]));
    }

    #[test]
    fn function_keys_f1_f4() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::F1, m), Some(&b"\x1bOP"[..]));
        assert_eq!(encode_named_key(&NamedKey::F2, m), Some(&b"\x1bOQ"[..]));
        assert_eq!(encode_named_key(&NamedKey::F3, m), Some(&b"\x1bOR"[..]));
        assert_eq!(encode_named_key(&NamedKey::F4, m), Some(&b"\x1bOS"[..]));
    }

    #[test]
    fn function_keys_f5_f12() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::F5, m), Some(&b"\x1b[15~"[..]));
        assert_eq!(encode_named_key(&NamedKey::F6, m), Some(&b"\x1b[17~"[..]));
        assert_eq!(encode_named_key(&NamedKey::F7, m), Some(&b"\x1b[18~"[..]));
        assert_eq!(encode_named_key(&NamedKey::F8, m), Some(&b"\x1b[19~"[..]));
        assert_eq!(encode_named_key(&NamedKey::F9, m), Some(&b"\x1b[20~"[..]));
        assert_eq!(encode_named_key(&NamedKey::F10, m), Some(&b"\x1b[21~"[..]));
        assert_eq!(encode_named_key(&NamedKey::F11, m), Some(&b"\x1b[23~"[..]));
        assert_eq!(encode_named_key(&NamedKey::F12, m), Some(&b"\x1b[24~"[..]));
    }
}
