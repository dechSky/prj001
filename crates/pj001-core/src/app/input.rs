use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// 키 입력 인코딩 시점에 참조되는 Term/WindowState 상태 스냅샷.
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
    // 1. NamedKey 매핑 시도 (modifier 조합 포함).
    if let Key::Named(named) = &event.logical_key {
        if let Some(bytes) = encode_named_key(named, mode) {
            return Some(bytes);
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
    // 3. text fallback. winit 0.30.13 macOS Korean IME 회귀(setMarkedText 미호출, insertText
    // 직접) 케이스에서 자모 단위 자모 텍스트가 KeyboardInput.text로 들어와 PTY에 그대로 흘러감.
    // → non-ASCII는 IME path(WindowEvent::Ime::Commit)만 신뢰하도록 차단.
    // ASCII는 그대로 통과 (영문 입력 등 정상).
    event.text.as_ref().and_then(|s| {
        if s.chars().all(|c| c.is_ascii()) {
            Some(s.as_bytes().to_vec())
        } else {
            log::debug!(
                "input: drop non-ASCII KeyboardInput.text={:?} (IME path only)",
                s
            );
            None
        }
    })
}

/// xterm modifier 파라미터 (Pm). Shift=1, Alt=2, Ctrl=4 비트 합 + 1.
/// modifier 없으면 None.
fn modifier_param(mods: ModifiersState) -> Option<u8> {
    let mut bits = 0u8;
    if mods.shift_key() {
        bits |= 1;
    }
    if mods.alt_key() {
        bits |= 2;
    }
    if mods.control_key() {
        bits |= 4;
    }
    if bits == 0 { None } else { Some(1 + bits) }
}

/// NamedKey 매핑. modifier 조합 시 xterm CSI 1;Pm 형식 적용.
fn encode_named_key(key: &NamedKey, mode: InputMode) -> Option<Vec<u8>> {
    let app = mode.cursor_keys_application;
    let shift = mode.modifiers.shift_key();
    let pm = modifier_param(mode.modifiers);

    // modifier가 있으면 modified form 우선.
    if let Some(pm) = pm {
        if let Some(bytes) = encode_modified(key, pm) {
            return Some(bytes);
        }
        // modifier 적용 안 되는 키(Enter/Backspace/Tab 등)는 unmodified로 진행.
    }

    let unmodified: &'static [u8] = match key {
        NamedKey::Enter => b"\r",
        NamedKey::Backspace => &[0x7F],
        // Shift+Tab → CSI Z (CBT, back tab). 일반 Tab은 \t.
        NamedKey::Tab if shift => b"\x1b[Z",
        NamedKey::Tab => b"\t",
        NamedKey::Escape => b"\x1b",
        // 화살표 — DECCKM 분기.
        NamedKey::ArrowUp => {
            if app {
                b"\x1bOA"
            } else {
                b"\x1b[A"
            }
        }
        NamedKey::ArrowDown => {
            if app {
                b"\x1bOB"
            } else {
                b"\x1b[B"
            }
        }
        NamedKey::ArrowRight => {
            if app {
                b"\x1bOC"
            } else {
                b"\x1b[C"
            }
        }
        NamedKey::ArrowLeft => {
            if app {
                b"\x1bOD"
            } else {
                b"\x1b[D"
            }
        }
        // 위치 키 — Home/End는 SS3 form (macOS Terminal.app/iTerm2 표준, zsh default binding).
        NamedKey::Home => b"\x1bOH",
        NamedKey::End => b"\x1bOF",
        NamedKey::Insert => b"\x1b[2~",
        NamedKey::Delete => b"\x1b[3~",
        NamedKey::PageUp => b"\x1b[5~",
        NamedKey::PageDown => b"\x1b[6~",
        // F1-F4 — VT100 PF1-PF4 (SS3 prefix). xterm default unconditional.
        NamedKey::F1 => b"\x1bOP",
        NamedKey::F2 => b"\x1bOQ",
        NamedKey::F3 => b"\x1bOR",
        NamedKey::F4 => b"\x1bOS",
        // F5-F12 — CSI ~ form. F6 다음 16, F11 다음 22가 결번 (xterm 표준).
        NamedKey::F5 => b"\x1b[15~",
        NamedKey::F6 => b"\x1b[17~",
        NamedKey::F7 => b"\x1b[18~",
        NamedKey::F8 => b"\x1b[19~",
        NamedKey::F9 => b"\x1b[20~",
        NamedKey::F10 => b"\x1b[21~",
        NamedKey::F11 => b"\x1b[23~",
        NamedKey::F12 => b"\x1b[24~",
        _ => return None,
    };
    Some(unmodified.to_vec())
}

/// modified form: xterm `CSI 1;Pm <final>` 또는 `CSI N;Pm ~` 형식.
/// modifier 적용 안 되는 키(Enter, Tab 등)는 None — 호출자가 unmodified로 fallback.
fn encode_modified(key: &NamedKey, pm: u8) -> Option<Vec<u8>> {
    // 화살표/Home/End: CSI 1;Pm <final>
    let final_char: Option<u8> = match key {
        NamedKey::ArrowUp => Some(b'A'),
        NamedKey::ArrowDown => Some(b'B'),
        NamedKey::ArrowRight => Some(b'C'),
        NamedKey::ArrowLeft => Some(b'D'),
        NamedKey::Home => Some(b'H'),
        NamedKey::End => Some(b'F'),
        // F1-F4: CSI 1;Pm P/Q/R/S (xterm modifyFunctionKeys default)
        NamedKey::F1 => Some(b'P'),
        NamedKey::F2 => Some(b'Q'),
        NamedKey::F3 => Some(b'R'),
        NamedKey::F4 => Some(b'S'),
        _ => None,
    };
    if let Some(fc) = final_char {
        return Some(format!("\x1b[1;{pm}{}", fc as char).into_bytes());
    }
    // CSI N;Pm ~ 형식 (Insert/Delete/PageUp/Down/F5-F12)
    let n: Option<u8> = match key {
        NamedKey::Insert => Some(2),
        NamedKey::Delete => Some(3),
        NamedKey::PageUp => Some(5),
        NamedKey::PageDown => Some(6),
        NamedKey::F5 => Some(15),
        NamedKey::F6 => Some(17),
        NamedKey::F7 => Some(18),
        NamedKey::F8 => Some(19),
        NamedKey::F9 => Some(20),
        NamedKey::F10 => Some(21),
        NamedKey::F11 => Some(23),
        NamedKey::F12 => Some(24),
        _ => None,
    };
    n.map(|n| format!("\x1b[{n};{pm}~").into_bytes())
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

    fn vec(s: &[u8]) -> Option<Vec<u8>> {
        Some(s.to_vec())
    }

    #[test]
    fn arrow_normal() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), vec(b"\x1b[A"));
        assert_eq!(encode_named_key(&NamedKey::ArrowDown, m), vec(b"\x1b[B"));
        assert_eq!(encode_named_key(&NamedKey::ArrowRight, m), vec(b"\x1b[C"));
        assert_eq!(encode_named_key(&NamedKey::ArrowLeft, m), vec(b"\x1b[D"));
    }

    #[test]
    fn arrow_application_mode() {
        let m = mode(true);
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), vec(b"\x1bOA"));
        assert_eq!(encode_named_key(&NamedKey::ArrowDown, m), vec(b"\x1bOB"));
        assert_eq!(encode_named_key(&NamedKey::ArrowRight, m), vec(b"\x1bOC"));
        assert_eq!(encode_named_key(&NamedKey::ArrowLeft, m), vec(b"\x1bOD"));
    }

    #[test]
    fn enter_backspace_tab_escape() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::Enter, m), vec(b"\r"));
        assert_eq!(encode_named_key(&NamedKey::Backspace, m), vec(&[0x7F]));
        assert_eq!(encode_named_key(&NamedKey::Tab, m), vec(b"\t"));
        assert_eq!(encode_named_key(&NamedKey::Escape, m), vec(b"\x1b"));
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
        assert_eq!(encode_named_key(&NamedKey::Home, m), vec(b"\x1bOH"));
        assert_eq!(encode_named_key(&NamedKey::End, m), vec(b"\x1bOF"));
        assert_eq!(encode_named_key(&NamedKey::Insert, m), vec(b"\x1b[2~"));
        assert_eq!(encode_named_key(&NamedKey::Delete, m), vec(b"\x1b[3~"));
        assert_eq!(encode_named_key(&NamedKey::PageUp, m), vec(b"\x1b[5~"));
        assert_eq!(encode_named_key(&NamedKey::PageDown, m), vec(b"\x1b[6~"));
    }

    #[test]
    fn shift_tab_back() {
        let mut m = mode(false);
        m.modifiers = ModifiersState::SHIFT;
        assert_eq!(encode_named_key(&NamedKey::Tab, m), vec(b"\x1b[Z"));
    }

    #[test]
    fn function_keys_f1_f4() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::F1, m), vec(b"\x1bOP"));
        assert_eq!(encode_named_key(&NamedKey::F2, m), vec(b"\x1bOQ"));
        assert_eq!(encode_named_key(&NamedKey::F3, m), vec(b"\x1bOR"));
        assert_eq!(encode_named_key(&NamedKey::F4, m), vec(b"\x1bOS"));
    }

    #[test]
    fn function_keys_f5_f12() {
        let m = mode(false);
        assert_eq!(encode_named_key(&NamedKey::F5, m), vec(b"\x1b[15~"));
        assert_eq!(encode_named_key(&NamedKey::F6, m), vec(b"\x1b[17~"));
        assert_eq!(encode_named_key(&NamedKey::F7, m), vec(b"\x1b[18~"));
        assert_eq!(encode_named_key(&NamedKey::F8, m), vec(b"\x1b[19~"));
        assert_eq!(encode_named_key(&NamedKey::F9, m), vec(b"\x1b[20~"));
        assert_eq!(encode_named_key(&NamedKey::F10, m), vec(b"\x1b[21~"));
        assert_eq!(encode_named_key(&NamedKey::F11, m), vec(b"\x1b[23~"));
        assert_eq!(encode_named_key(&NamedKey::F12, m), vec(b"\x1b[24~"));
    }

    // M9-1: modifier 조합 인코딩 (xterm CSI 1;Pm 형식).

    #[test]
    fn modifier_param_mapping() {
        assert_eq!(modifier_param(ModifiersState::empty()), None);
        assert_eq!(modifier_param(ModifiersState::SHIFT), Some(2));
        assert_eq!(modifier_param(ModifiersState::ALT), Some(3));
        assert_eq!(
            modifier_param(ModifiersState::SHIFT | ModifiersState::ALT),
            Some(4)
        );
        assert_eq!(modifier_param(ModifiersState::CONTROL), Some(5));
        assert_eq!(
            modifier_param(ModifiersState::SHIFT | ModifiersState::CONTROL),
            Some(6)
        );
        assert_eq!(
            modifier_param(ModifiersState::ALT | ModifiersState::CONTROL),
            Some(7)
        );
        assert_eq!(
            modifier_param(ModifiersState::SHIFT | ModifiersState::ALT | ModifiersState::CONTROL),
            Some(8)
        );
    }

    #[test]
    fn shift_arrow_modifier_form() {
        let mut m = mode(false);
        m.modifiers = ModifiersState::SHIFT;
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), vec(b"\x1b[1;2A"));
        assert_eq!(encode_named_key(&NamedKey::ArrowDown, m), vec(b"\x1b[1;2B"));
        assert_eq!(
            encode_named_key(&NamedKey::ArrowRight, m),
            vec(b"\x1b[1;2C")
        );
        assert_eq!(encode_named_key(&NamedKey::ArrowLeft, m), vec(b"\x1b[1;2D"));
    }

    #[test]
    fn ctrl_arrow_modifier_form() {
        let mut m = mode(false);
        m.modifiers = ModifiersState::CONTROL;
        assert_eq!(
            encode_named_key(&NamedKey::ArrowRight, m),
            vec(b"\x1b[1;5C")
        );
    }

    #[test]
    fn modified_home_end_use_csi_form() {
        // unmodified는 SS3 form이지만 modified는 CSI 1;Pm 형식.
        let mut m = mode(false);
        m.modifiers = ModifiersState::SHIFT;
        assert_eq!(encode_named_key(&NamedKey::Home, m), vec(b"\x1b[1;2H"));
        assert_eq!(encode_named_key(&NamedKey::End, m), vec(b"\x1b[1;2F"));
    }

    #[test]
    fn modified_tilde_keys() {
        let mut m = mode(false);
        m.modifiers = ModifiersState::CONTROL;
        assert_eq!(encode_named_key(&NamedKey::Insert, m), vec(b"\x1b[2;5~"));
        assert_eq!(encode_named_key(&NamedKey::Delete, m), vec(b"\x1b[3;5~"));
        assert_eq!(encode_named_key(&NamedKey::PageUp, m), vec(b"\x1b[5;5~"));
        assert_eq!(encode_named_key(&NamedKey::F5, m), vec(b"\x1b[15;5~"));
        assert_eq!(encode_named_key(&NamedKey::F12, m), vec(b"\x1b[24;5~"));
    }

    #[test]
    fn modified_f1_f4() {
        let mut m = mode(false);
        m.modifiers = ModifiersState::SHIFT;
        assert_eq!(encode_named_key(&NamedKey::F1, m), vec(b"\x1b[1;2P"));
        assert_eq!(encode_named_key(&NamedKey::F4, m), vec(b"\x1b[1;2S"));
    }

    #[test]
    fn shift_alt_ctrl_combination() {
        // Pm = 1 + 1(shift) + 2(alt) + 4(ctrl) = 8
        let mut m = mode(false);
        m.modifiers = ModifiersState::SHIFT | ModifiersState::ALT | ModifiersState::CONTROL;
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), vec(b"\x1b[1;8A"));
    }
}
