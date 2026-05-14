# pj001 — 키보드 입력 고도화 설계 (M8)

**상태**: 설계 초안 (2026-05-08). 코드 미반영.
**범위**: 화살표 + Home/End + PageUp/Down 정책 + Insert/Delete + F1-F12 + DECCKM + Ctrl+letter 명시 매핑 + Shift+Tab + Backspace 정책 명시 + **macOS Cmd 단축키 정책** + **Window title(OSC 0/2)**.
**제외 (M9 자리 비움)**: Shift/Ctrl/Alt 모디파이어 조합 인코딩(`CSI 1;Pm A`), DECPAM/DECPNM keypad mode, macOS Option(Alt as Meta) 정책, Numpad 키, Wide cursor, KeyEvent.repeat 정책.
**제외 (M10 자리 비움)**: bracketed paste mode, focus reporting mode, DSR (`CSI 6n` cursor position report 등 device reports). VT shadow modes 영역.
**제외 (M11+)**: clipboard 통합 (Cmd+C/V — selection 모듈 신설 필요).

---

## 0. 현재 상태 (M7 시점)

`src/app/input.rs::encode_key`:

```rust
match &event.logical_key {
    Key::Named(NamedKey::Enter) => Some(b"\r".to_vec()),
    Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
    Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
    Key::Named(NamedKey::Escape) => Some(b"\x1b".to_vec()),
    _ => event.text.as_ref().map(|s| s.as_bytes().to_vec()),
}
```

처리됨: Enter, Backspace, Tab, Escape, 일반 문자(text 필드).
**미처리**: 화살표/Home/End/PageUp/PageDown/Insert/Delete/F1-F12 — 모두 `text=None`이라 PTY 전송 안 됨.

PageUp/PageDown은 `app/mod.rs::window_event`에서 별도 처리하여 scrollback view 스크롤로 점유 중(M6-8).

DECCKM(application cursor mode) 미지원 — vim/less가 활성화해도 무시됨.

---

## 1. 설계 결정 요약

| 결정 | 값 | 이유 |
|---|---|---|
| 화살표 인코딩 | DECCKM off → `CSI A/B/C/D`, DECCKM on → `SS3 A/B/C/D` | xterm/VT100 표준 |
| Home/End | `CSI H` / `CSI F` | xterm 표준 (`CSI 1~`/`CSI 4~`은 일부 변종) |
| PageUp/Down 정책 | **하이브리드**: alt screen이면 PTY 전송, main screen이면 scrollback | 다음 § 5에서 상세 |
| Insert/Delete | `CSI 2~` / `CSI 3~` | xterm 표준 |
| F1-F4 | `SS3 P/Q/R/S` | VT100 PF1-PF4 |
| F5-F12 | `CSI 15~/17~/18~/19~/20~/21~/23~/24~` | F6 다음 16, F11 다음 22 결번이 표준 |
| DECCKM | `CSI ?1 h` 활성, `CSI ?1 l` 비활성. 기본 비활성 | xterm 표준 |
| 키 → byte 결정은 `Term`의 상태에 의존 | DECCKM, alt screen 두 가지만 | 단순화 |
| ESC, CSI, SS3 정의 | `ESC = \x1b`, `CSI = \x1b[`, `SS3 = \x1bO` | 표준 |

---

## 2. 핵심 인코딩 표

### 2.1 화살표 (DECCKM 의존)

| 키 | DECCKM off (default) | DECCKM on |
|---|---|---|
| ArrowUp | `ESC [ A` (`\x1b[A`) | `ESC O A` (`\x1bOA`) |
| ArrowDown | `ESC [ B` | `ESC O B` |
| ArrowRight | `ESC [ C` | `ESC O C` |
| ArrowLeft | `ESC [ D` | `ESC O D` |

### 2.2 위치 키

| 키 | byte | DECCKM 의존 |
|---|---|---|
| Home | `ESC O H` (`\x1bOH`, SS3 form) | **무관** — macOS Terminal.app/iTerm2 표준 + zsh default binding 호환 (M8-7 시각 검증 시 `\x1b[H`로는 zsh 인식 안 됨 발견 → SS3로 변경) |
| End | `ESC O F` (`\x1bOF`) | 무관 |
| PageUp | `ESC [ 5 ~` (PTY 전송 시. § 5 정책 참고) | 무관 |
| PageDown | `ESC [ 6 ~` | 무관 |
| Insert | `ESC [ 2 ~` | 무관 |
| Delete | `ESC [ 3 ~` | 무관 |

xterm 표준은 Home/End가 DECCKM 의존(off=`CSI H/F`, on=`SS3 H/F`)이지만 거의 모든 앱이 두 형식 모두 받아들이므로 단순화. 비표준 호환 이슈 발견 시 M9에서 보강.

### 2.2.1 Ctrl + letter (M8 편입)

ASCII 0x01-0x1A. xterm 표준은 modifier로 자동 변환:

| 입력 | byte |
|---|---|
| Ctrl+A ~ Ctrl+Z | `0x01` ~ `0x1A` |
| Ctrl+@ | `0x00` (NUL) |
| Ctrl+[ | `0x1B` (ESC) |
| Ctrl+\ | `0x1C` (FS) |
| Ctrl+] | `0x1D` (GS) |
| Ctrl+^ | `0x1E` (RS) |
| Ctrl+_ | `0x1F` (US) |
| Ctrl+? | `0x7F` (DEL) |

**현재 우연히 동작 중**: winit이 macOS native에서 Ctrl+C 누름 시 `event.text = Some("\x03")` 같이 자동 변환된 값을 줌. 우리 fallback이 그걸 PTY로 보냄. M5에서 vim Ctrl+C 작동 검증됨.

**M8에서 명시화**: `event.text` 의존 대신 `Modifiers` + `logical_key` 직접 매핑. winit/macOS 변경에 robust + Ctrl+@ / Ctrl+[ 등 text 필드가 안 주는 경우 보강.

```rust
// encode_named_key 또는 별도 ctrl 분기
fn encode_ctrl_letter(c: char, ctrl: bool) -> Option<&'static [u8]> {
    if !ctrl { return None; }
    match c.to_ascii_lowercase() {
        'a'..='z' => Some(&[(c.to_ascii_lowercase() as u8) - b'a' + 1]),
        '@' => Some(&[0x00]),
        '[' => Some(&[0x1B]),
        '\\' => Some(&[0x1C]),
        ']' => Some(&[0x1D]),
        '^' => Some(&[0x1E]),
        '_' => Some(&[0x1F]),
        '?' => Some(&[0x7F]),
        _ => None,
    }
}
```

`InputMode.modifiers`에서 ctrl 플래그 읽음. modifier 인프라(M9 미루지 않고 M8-1)에서 함께 도입한 `ModifiersState` 활용.

### 2.2.2 Shift + Tab (M8 편입)

| 입력 | byte |
|---|---|
| Shift+Tab | `ESC [ Z` (CBT — back tab) |

emacs/일부 TUI가 사용. modifier 검사 후 NamedKey::Tab + shift이면 `\x1b[Z` 송신.

### 2.2.3 Backspace 정책 (현재 그대로)

- 현재 동작: `0x7F` (DEL) 송신.
- xterm 표준: `0x7F` (default), 옵션으로 `0x08` (BS) 가능.
- **M8 정책**: `0x7F` 유지. 일부 앱이 BS 기대 시 `stty erase ^H`로 사용자 측에서 변경 가능. 코드 변경 없음, 정책만 명시.

### 2.3 Function 키

xterm default. F1-F4가 SS3, F5+가 CSI ~인 게 historical.

| 키 | byte |
|---|---|
| F1 | `ESC O P` (`\x1bOP`) |
| F2 | `ESC O Q` |
| F3 | `ESC O R` |
| F4 | `ESC O S` |
| F5 | `ESC [ 1 5 ~` |
| F6 | `ESC [ 1 7 ~` |
| F7 | `ESC [ 1 8 ~` |
| F8 | `ESC [ 1 9 ~` |
| F9 | `ESC [ 2 0 ~` |
| F10 | `ESC [ 2 1 ~` |
| F11 | `ESC [ 2 3 ~` |
| F12 | `ESC [ 2 4 ~` |

(F6 다음 16, F11 다음 22가 결번인 것은 xterm/VT 역사적 표준.)

---

## 3. DECCKM (application cursor key mode)

**시퀀스 처리**:

```
CSI ? 1 h → DECCKM on  (application mode)
CSI ? 1 l → DECCKM off (normal/cursor mode, default)
```

`vt::handle_dec_private`에 `(1, 'h')` / `(1, 'l')` 분기 추가.

**Term 변경**:

```rust
pub struct Term {
    // ...
    cursor_keys_application: bool,  // DECCKM. 기본 false
}

pub fn set_cursor_keys_application(&mut self, on: bool);
pub fn cursor_keys_application(&self) -> bool;
```

`encode_key`가 `Term`을 참조해 인코딩 분기. `app::input::encode_key`는 `Term` 또는 작은 모드 struct를 인자로 받도록 변경:

```rust
pub struct InputMode {
    pub cursor_keys_application: bool,
    pub alt_screen: bool,
}

pub fn encode_key(event: &KeyEvent, mode: InputMode) -> Option<Vec<u8>>;
```

`mode`는 `app::render` 시점이 아닌 **키 누를 때마다** Term lock 잡고 한 번 읽음. 짧은 critical section.

---

## 4. SS3 / CSI 상수

`src/app/input.rs` 또는 별도 `src/app/keys.rs`:

```rust
const ESC: &[u8] = b"\x1b";
const CSI: &[u8] = b"\x1b[";
const SS3: &[u8] = b"\x1bO";
```

key → byte 함수는 위 상수 + suffix 조합:

```rust
fn arrow_up(app_mode: bool) -> &'static [u8] {
    if app_mode { b"\x1bOA" } else { b"\x1b[A" }
}
fn home() -> &'static [u8] { b"\x1b[H" }
fn end() -> &'static [u8] { b"\x1b[F" }
fn f5() -> &'static [u8] { b"\x1b[15~" }
// ...
```

`&'static [u8]` 반환으로 alloc 0회. `encode_key`가 `Vec<u8>` 반환이므로 호출처에서 `.to_vec()` 한 번.

---

## 5. PageUp/PageDown 정책 (충돌 해결)

### 5.1 현재 문제

M6-8에서 PageUp/PageDown을 scrollback view 스크롤에 점유. 일반 앱(less, vim)이 PageUp 누를 때 PTY로 byte가 가야 하는데 우리는 scrollback view만 움직이고 PTY 안 보냄.

### 5.2 옵션 비교

| 옵션 | 동작 | 장점 | 단점 |
|---|---|---|---|
| A. **scrollback 전용 유지** | PTY로 안 보냄 | 단순 | less/vim 페이지 이동 불가 |
| B. **PTY 전송 + Shift 모디파이어로 scrollback** | 기본 PTY, Shift+PageUp/Down은 scrollback | xterm/iTerm 호환 | macbook fn+ArrowUp이 이미 PageUp이라 Shift+fn+ArrowUp 어색 |
| C. **alt screen 자동 분기** | alt screen이면 PTY, main screen이면 scrollback | 직관적(vim/less는 alt screen, shell은 main) | bash less/man 등은 alt screen이라 OK, 단 일부 less 구현은 main screen에서도 PageUp 사용 |
| D. **하이브리드 C + B 결합** | 기본은 C(alt면 PTY, main이면 scrollback), Shift+PageUp/Down은 강제 scrollback | 가장 사용자 친화적 | 복잡도 약간 증가 |

### 5.3 권장: 옵션 C (단순 자동 분기)

- alt screen 진입 = PTY 전송 (vim/less에서 PageUp 정상 작동)
- main screen 유지 = scrollback view (M6-8 기본 동작)
- modifier 도입은 M9 (Shift+PageUp 강제 scrollback)

**알려진 한계**:
- `less` 기본 flag는 alt screen 사용해서 OK이지만 `less -X` 또는 `LESS=-X` 환경 변수 사용 시 main screen 유지 → 우리 정책상 PageUp이 PTY 전송 안 되어 less 페이지 이동 불가. M9에서 Shift modifier로 분리해 해결 예정.
- main screen에서 PageUp을 PTY로 보내야 하는 다른 TUI 앱(드물지만 존재)도 동일.

**M6-8과의 정책 변경**: M6-8은 PageUp/Down을 항상 scrollback에 점유. M8-5에서 alt screen이면 PTY 전송으로 변경. commit message에 명시 — 회귀로 오인되지 않게.

**디버그 로그**: 첫 PageUp 분기 시점에 `log::info!("page key dispatch: target=PTY"/"target=scrollback")` 한 번 남김(one-shot 플래그). 사용자 리포트 시 즉시 분기 행위 확인 가능.

### 5.4 코드 위치

`app::window_event::KeyboardInput`의 PageUp/PageDown 분기:

```rust
if matches!(named, NamedKey::PageUp | NamedKey::PageDown) {
    let alt_screen = state.term.lock().map(|t| t.is_alt_screen()).unwrap_or(false);
    if alt_screen {
        // PTY 전송 — fall through to encode_key
    } else {
        // scrollback (기존 M6-8 동작)
        // ...
        return;
    }
}
```

`Term::is_alt_screen()` 메서드 추가 필요 (현재 `use_alt`는 private).

---

## 6. encode_key 시그니처 변경

advisor 권고: M9에서 modifier 추가될 때 시그니처 두 번 안 바꾸도록 **`Modifiers`도 InputMode에 미리 포함**. M8은 무시, M9에서 사용.

```rust
// src/app/input.rs
use winit::keyboard::ModifiersState;

#[derive(Default, Clone, Copy)]
pub struct InputMode {
    pub cursor_keys_application: bool,  // M8-1, M8-4에서 사용
    pub alt_screen: bool,                // M8-5에서 PageUp 분기용
    pub modifiers: ModifiersState,       // M9 placeholder. M8은 무시
}

pub fn encode_key(event: &KeyEvent, mode: InputMode) -> Option<Vec<u8>>;
```

`AppState.modifiers`도 M8-1에서 도입하고 `WindowEvent::ModifiersChanged`로 추적 시작 (5줄). M8 코드는 그 값을 사용하지 않지만 나중에 M9가 가져다 쓸 인프라.

### 6.1 single snapshot 패턴 (advisor lock race 방지)

PageUp 분기와 encode_key의 mode 읽기를 **동일 lock 스냅샷**으로:

```rust
// app::window_event::KeyboardInput
let mode = {
    let term = state.term.lock().unwrap();
    InputMode {
        cursor_keys_application: term.cursor_keys_application(),
        alt_screen: term.is_alt_screen(),
        modifiers: state.modifiers,
    }
};
// 이후 모든 분기는 mode만 참조 — Term lock 안 잡음.

// PageUp/Down: alt_screen이면 encode_key가 byte 반환, 아니면 scrollback.
// PTY 리더 thread가 alt screen 토글하더라도 한 키 입력 안에서는 일관됨.
```

---

## 7. 단계 분해 (구현 시)

| 단계 | 내용 | 의존 | 검증 |
|---|---|---|---|
| **M8-1** | InputMode(modifiers 포함) + ModifiersChanged 추적 + encode_named_key 헬퍼 + 화살표 인코딩 + **Ctrl+letter 명시 매핑**(§ 2.2.1) | 독립 | unit test: arrow → `CSI A/B/C/D` + Ctrl+a → `0x01`, Ctrl+[ → `0x1B` |
| **M8-2** | Home/End/Insert/Delete + **Shift+Tab `CSI Z`**(§ 2.2.2) | M8-1 | unit test: 각 키 + Shift+Tab |
| **M8-3** | F1-F12 | M8-1 | unit test: F1~F12 매핑 |
| **M8-4** | DECCKM 처리 (vt + Term) + 화살표 DECCKM 분기 | M8-1 | unit test: `\x1b[?1h` 후 InputMode.cursor_keys_application=true에서 arrow → SS3 |
| **M8-5** | PageUp/Down 분기 (alt screen → PTY) + Term::is_alt_screen 노출 | M8-1, M8-4 | 시각: vim/less(-X 외)에서 PageUp 작동, main에서 scrollback 유지 |
| **M8-6** | macOS Cmd 단축키 정책 — § 13 | M8-1 | 시각: Cmd+Q/Cmd+W로 앱 종료. 다른 Cmd 조합은 PTY 안 보냄(swallow) |
| **M8-7** | Window title (OSC 0/2) 처리 — § 14 | 독립 (vt) | 시각: shell prompt가 cwd 바뀔 때 창 타이틀바 갱신 |

각 단계 30분~1h. 총 3-4h. unit test 자동 검증 가능 — 사용자 시각 검증은 M8-5에서만.

**검증 한계**: encode_key 통합 path(KeyEvent 생성 + Term lock + PTY write)는 unit test로 다루기 어려워 시각 검증(vim 화살표 작동 등)에 의존. encode_named_key 단위는 KeyEvent 없이 직접 테스트.

체크포인트: M8-1 통과 후 vim/zsh 일반 작업에서 화살표 정상 작동 확인 후 다음 단계.

---

## 8. unit test 전략

`KeyEvent` 직접 생성은 winit의 비공개 필드 때문에 까다로움. 핵심 우회: **logical_key 처리를 분리**.

**계약**:
- `encode_named_key(key: &NamedKey, mode: InputMode) -> Option<&'static [u8]>` — `Key::Named` 매핑만. 알 수 없는 named key는 None.
- `encode_key(event: &KeyEvent, mode: InputMode) -> Option<Vec<u8>>` — wrapper:
  1. `event.state == Pressed`만 처리
  2. `event.logical_key`가 `Key::Named(named)`이면 `encode_named_key(named, mode)` 시도
  3. None이면 `event.text`로 fallback (Korean IME unicode 등)

테스트는 `encode_named_key`만 (KeyEvent 없이):

```rust
#[cfg(test)]
mod tests {
    use winit::keyboard::NamedKey;
    use super::{encode_named_key, InputMode};

    #[test]
    fn arrow_up_normal() {
        let m = InputMode::default();
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), Some(&b"\x1b[A"[..]));
    }
    #[test]
    fn arrow_up_app_mode() {
        let m = InputMode { cursor_keys_application: true, ..InputMode::default() };
        assert_eq!(encode_named_key(&NamedKey::ArrowUp, m), Some(&b"\x1bOA"[..]));
    }
    // Home/End/F-keys/Insert/Delete 등...
}
```

`encode_key` 통합 path(text fallback 포함)는 unit test 안 함 — 시각 검증.

---

## 9. 영향 범위

| 파일 | 변경 |
|---|---|
| `src/app/input.rs` | `InputMode`, `encode_named_key`, encode_key 시그니처 변경 |
| `src/app/mod.rs` | window_event에서 mode 구성 + PageUp/Down 분기 변경 |
| `src/grid/mod.rs` | `cursor_keys_application` 필드 + getter/setter, `is_alt_screen()` 메서드 |
| `src/vt/perform.rs` | handle_dec_private에 (1,'h')/(1,'l') 추가 |

shader, render 모듈 변경 없음.

---

## 10. 미래 단계 (자리 비움)

### 10.1 M9 — 완료 (2026-05-08)

xterm 모디파이어 인코딩(파라미터 Pm):

```
ESC [ 1 ; Pm A   (modified ArrowUp / arrow / Home / End / F1-F4)
ESC [ N ; Pm ~   (modified Insert / Delete / PageUp/Down / F5-F12)
```

| Pm | 의미 |
|---|---|
| 2 | Shift |
| 3 | Alt |
| 4 | Shift + Alt |
| 5 | Ctrl |
| 6 | Shift + Ctrl |
| 7 | Alt + Ctrl |
| 8 | Shift + Alt + Ctrl |

**구현**: `encode_named_key`가 `modifier_param` 결과로 modified form 우선. modified form은 `encode_modified` 헬퍼가 동적 byte (`Vec<u8>`) 생성. unmodified form은 기존 `&'static [u8]` 후 `to_vec()`. unit test 7개로 매핑 검증.

- **DECPAM/DECPNM** keypad mode (`ESC =` / `ESC >`): M9-3에서 처리. esc_dispatch에 `b'='` / `b'>'` 분기. `Term.keypad_application` 필드 추적. 현재 numpad 키 인코딩 미구현이라 시각 영향 X — 향후 numpad 처리 시 이 mode 참조.
- **Wide cursor (M9-2)**: cursor 위치 cell이 WIDE 속성이면 cursor overlay의 `cell_span=2.0`. WIDE_CONT(짝 cell) 위 cursor면 한 cell 왼쪽으로 보정해서 본체 위로 정렬.

**남은 자리 비움 (M9 미적용)**:
- **PageUp/Down Shift 분기** (옵션 D — `less -X` 케이스 보강). modifier 인프라는 마련됐지만 PageUp/Down 분기는 alt screen 자동 분기로 충분히 작동 → 보류.
- **Numpad 키** — macbook 단독 사용에 영향 X. 외장 키보드 사용자가 요청 시 추가.

### 10.2 M9 — Option(Alt) / KeyEvent.repeat 정책 (M9-4/5)

#### Alt as Meta
- xterm 표준: Alt+a → `ESC a` (ESC prefix).
- macOS 표준: Option+e → `é` (compose / unicode 변환).
- **현재 정책**: Alt 단독은 winit `event.text` 의존. macOS native 동작 그대로 (Option+e=é). 다른 modifier(Shift/Ctrl)와 조합 시 modifier_param이 처리.
- **M9-4 결정**: 명시적 Meta 변환 미구현. 사용자가 `\x1b a` 형식 필요 시 별도 옵션 추가 필요(M9 향후 자리 비움).

#### KeyEvent.repeat
- winit이 키 누름 유지 시 `KeyEvent.repeat=true`로 반복 이벤트 발생.
- **현재 정책**: repeat 이벤트도 그대로 PTY 송신. shell이 알아서 처리(자체 repeat rate). 빠른 입력 시 PTY 부하 가능하지만 실측 문제 없음.
- **M9-5 결정**: 코드 변경 없이 정책 명시만. 향후 PTY 부하 발견 시 throttling 도입.

### 10.2 M10 — VT shadow modes (구현 완료, 2026-05-08)

자세한 설계는 `docs/m10-design.md`. 본 항목은 keyboard-design 차원에서의 cross-cut만.

- **Bracketed paste (`CSI ?2004 h/l`)**: 구현. zsh가 활성화 (line editor 시작 시 `?2004h` 송신 확인). paste 시 `\e[200~` ... `\e[201~`로 wrap.
- **Focus reporting (`CSI ?1004 h/l`)**: 구현. WindowEvent::Focused에서 `\e[I` / `\e[O` PTY 송신.
- **DSR `CSI 6n`**: 구현. cursor pos 응답 `\e[<r>;<c>R`.
- **DA1 `CSI c`**: 구현. `\e[?1;2c` 응답. **vim startup hang(~1초) 사라짐**.
- **DA2 `CSI > c`**: 구현. `\e[>41;0;0c` 응답.

**§13 단축키 정책 갱신**: Cmd+V는 swallow 아닌 **clipboard paste 동작**(arboard 3.6).

---

## 11. 미해결 / 사용자 결정 필요

- **PageUp/Down 정책 (§ 5)**: 권장 옵션 C(자동 분기). 사용자 확정 필요.
- **OSC 2 자동 송신은 shell 설정 의존**: zsh 기본은 `cd` 시 OSC 2 자동 안 보냄. 사용자가 `.zshrc`에 `precmd() { print -Pn "\e]2;%~\a" }` 같은 hook 추가해야 자동 갱신. M8-7은 OSC 2 byte 처리 자체는 정상(`printf '\033]2;TEST\007'` 직접 송신 시 타이틀 변경 확인).
- **Numpad 키 (M9 자리 비움)**: 한 번도 안 다룸. text 필드 fallback에 의존. macbook 단독 사용에는 영향 X.
- **macOS Option(Alt as Meta) 키 (M9 자리 비움)**: xterm 표준은 Alt+a → `ESC a` (ESC prefix). 현재 winit이 macOS native에서 text 필드를 어떻게 채우는지 미검증. 두 가지 정책 가능: (1) Meta=ESC prefix(전통적), (2) Option=unicode/dead key(macOS native). M9에서 결정.
- **KeyEvent.repeat 처리 (M9 자리 비움)**: 현재 repeat 키도 모두 PTY 송신. shell이 알아서 처리. 정상 동작이지만 빠른 입력 시 PTY 부하 가능. 명시적 정책 미정.
- **Wide cursor (M9 자리 비움)**: M7-5에서 `cell_span=1.0` 단순화로 wide cell(한글 등) 위 cursor가 1 cell만 차지. 표준은 wide cell 전체. cursor-design.md에도 한계 명시.
- **DSR (Device Status Report — M10 자리 비움)**: `CSI 6 n` (cursor position report), `CSI c` (device attributes) 등. vim 등이 사용. 미응답 시 일부 앱이 timeout 후 fallback.

---

## 13. macOS Cmd 단축키 정책 (M8-6)

winit가 macOS native에서 Cmd 키를 modifier로 알려줌(`ModifiersState::SUPER`). 우리가 명시 처리 안 하면 일반 KeyEvent로 들어와서 logical_key로 fallthrough → text 필드로 글자 PTY 송신될 수 있음. 정책 정해서 *swallow* 또는 동작.

| 조합 | 정책 | 이유 |
|---|---|---|
| **Cmd+Q** | 앱 종료 (`event_loop.exit()`) | macOS 표준 |
| **Cmd+W** | active pane 닫기. active tab에 pane이 1개면 tab 닫기. 마지막 tab이면 앱 종료 | M14 tabs 정책 |
| **Cmd+Shift+W** | 현재 tab 강제 닫기. 마지막 tab이면 앱 종료 | M14 tabs 정책 |
| **Cmd+C** | 드래그 선택이 있으면 arboard clipboard에 선택 텍스트 복사. 선택이 없으면 swallow | M12 selection clipboard 첫 slice |
| **Cmd+V** | **clipboard paste** (arboard로 읽어 PTY 송신, bracketed paste mode면 `\e[200~`...`\e[201~` wrap). M10-6에서 구현 (2026-05-08) | 일상 사용 필수 |
| **우클릭** | 클릭한 pane 활성화. 선택이 있으면 clipboard copy. paste는 실수 명령 실행 위험 때문에 Cmd+V로 유지 | M12 mouse selection 첫 slice |
| **Cmd+← / Cmd+→** | shell line editor Home/End byte 송신 (`ESC OH` / `ESC OF`) | macOS text-field convention |
| **Cmd+↑ / Cmd+↓** | main screen scrollback 맨 위/아래 이동 | macOS terminal convention |
| **Cmd+T** | 새 tab 생성 | M14 tabs |
| **Cmd+1..9** | n번째 tab 전환 | M14 tabs |
| **Cmd+Option+1..9** | active tab 안 n번째 pane 전환 | `Cmd+Shift+숫자`는 macOS/global shortcut 충돌 가능성이 있어 제외 |
| **Cmd+[ / Cmd+]** | active tab 안 이전/다음 pane 전환 | M13 layout |
| **Cmd+Shift+[ / Cmd+Shift+]** | 이전/다음 tab 전환 | M14 tabs |
| **Cmd+N** | active tab에 새 shell pane 추가 (vertical split) | M15 dynamic spawn 첫 slice |
| **Cmd+Shift+N → key** | quick spawn sequence. 기본 `s=shell`; 추가 preset은 외부에서 주입(`Config.quick_spawn_presets`). 3초 내 입력 없으면 자동 취소 | M15 dynamic spawn, core는 preset data만 사용 |
| **Cmd+R** | active pane의 session을 같은 command로 재시작. SessionId는 새로 발급되고 scrollback은 초기화 | M15 respawn 첫 slice |
| **Cmd+= / Cmd+- / Cmd+0** | 폰트 크기 확대/축소/기본값 복귀. logical font size는 6~72pt로 clamp하고 monitor scale factor는 렌더 직전 적용 | M15 font zoom 첫 slice |
| **Cmd+K** | active pane의 scrollback을 clear하고 PTY에 `Ctrl+L`을 보내 shell/readline이 화면과 prompt를 다시 그리게 함. alt screen에서는 TUI 화면 보호를 위해 scrollback만 clear | Terminal clear UX |
| **Cmd+Shift+K / Cmd+Option+K** | active pane의 scrollback만 clear, visible buffer 유지. `Cmd+Shift+K`가 global shortcut에 잡히는 환경에서는 `Cmd+Option+K` 사용 | Clear Scrollback Buffer convention |
| **마우스 드래그** | 보이는 active pane 텍스트 셀 선택 하이라이트. Cmd+C로 선택 텍스트 복사 | M12 selection 첫 slice |
| **더블클릭 / 트리플클릭** | 더블클릭은 단어 선택, 트리플클릭은 줄 선택 | M12 selection UX |
| **파일 드롭** | Finder 파일/폴더를 active pane에 shell-quoted path로 입력. 개행 없이 trailing space만 추가하고 bracketed paste mode를 존중. non-UTF8/control-char path는 안전상 거부 | M12-5 file drag-and-drop |
| **그 외 Cmd+key** | swallow (안전 기본값) | shell이 Cmd modifier 의미 가질 가능성 0에 가까움. PTY 보내면 의도 없는 byte |

구현:

```rust
// app::window_event::KeyboardInput
if state.modifiers.super_key() {
    match &event.logical_key {
        Key::Character(c) if c == "q" || c == "w" => {
            event_loop.exit();
            return;
        }
        _ => return, // swallow — PTY 안 보냄
    }
}
```

**알려진 한계**: Cmd+C로 selection 복사가 표준 macOS UX이지만 우리는 selection 미구현 → swallow. 사용자가 "복사 안 됨" 인식할 가능성. M11에서 selection + clipboard 통합 시 해결.

## 14. Window title (OSC 0/2) — M8-7

xterm OSC 시퀀스로 shell이 창 타이틀 변경:

```
ESC ] 0 ; <title> BEL          (창+icon 타이틀)
ESC ] 2 ; <title> BEL          (창 타이틀만)
ESC ] 0 ; <title> ESC \         (ST 종료자도 표준)
```

zsh/bash가 `precmd`에서 보냄 → 창 타이틀바에 cwd 표시.

**vt 처리**: `osc_dispatch(params, bell_terminated)` 활용 (현재 빈 default impl).

```rust
fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
    if params.len() < 2 { return; }
    let code = params[0];
    let title = std::str::from_utf8(params[1]).unwrap_or("");
    if code == b"0" || code == b"2" {
        // Term에 제목 저장. app이 읽어서 winit Window::set_title 호출.
        self.term.set_title(title.to_string());
    }
}
```

**Term**:
```rust
pub struct Term {
    // ...
    title: String,
    title_dirty: bool,  // 변경 시점 마커
}
pub fn set_title(&mut self, t: String) { self.title = t; self.title_dirty = true; }
pub fn take_title_if_changed(&mut self) -> Option<String> { ... }
```

**app**: 매 redraw 또는 about_to_wait에서 `take_title_if_changed()` 확인 후 `window.set_title(&t)`. lock 안에서 짧게.

**검증**: shell prompt에서 `cd ~/somewhere` 후 창 타이틀 변화.

**알려진 한계**: OSC 1 (icon 타이틀만) 미처리. OSC 4/10/11/12 등 색상 시퀀스는 M10+. ST 종료자(`ESC \`)도 vte가 자동 처리하므로 우리 코드 변경 불필요.

## 15. 단순 검증용 cheat 시퀀스

구현 후 사용자 검증 시:

```bash
# 화살표
printf 'press arrow keys, expect cursor move in shell\n'

# DECCKM 토글 (vim 등이 자동으로 보냄, 직접 테스트도 가능)
printf '\033[?1h'  # app mode on
printf '\033[?1l'  # app mode off

# Home/End — shell line editor에서 검증
# vim에서 :PageDown 등으로 alt screen PageUp 검증
```
