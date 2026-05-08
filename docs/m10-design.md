# M10 — VT shadow modes (paste / focus / DSR + DA)

**상태**: 설계 v1 (2026-05-08). 코드 미진입.
**목적**: vim/zsh가 가정하는 VT shadow modes 응답. **vim startup hang 사라짐**이 가장 큰 사용자 가시 이득(DA1 미응답으로 ~1초 timeout 후 fallback 진행하던 것 즉응).
**범위**: bracketed paste(?2004) + focus reporting(?1004) + DSR(`CSI 6n`) + DA1(`CSI c`) + DA2(`CSI > c`) + Cmd+V via arboard.
**선행**: M8/M9 완료 코드 위. M17 영향 없음.
**스타일**: M7/M8/M17 패턴. 단계 분해 7개.

---

## 0. 배경

현재 `handle_dec_private`가 다루는 항목: 1049/1047 alt screen, 25 DECTCEM, 1 DECCKM. **paste/focus/DSR/DA 누락**.

가시 영향:
- `vim`: 시작 시 DA1(`CSI c`) probe → 응답 없으면 ~1초 timeout. M10-5 fix하면 즉시 시작.
- `zsh + tmux`: bracketed paste 활성화 시도. 우리가 mode 안 추적해 paste가 raw로 PTY 들어감.
- `Cmd+V`: 현재 swallow. clipboard 미동작.
- vim의 focus tracking: `:set autoread` 등이 focus event로 reload. 미동작.

또한 **PTY 응답 채널 인프라**가 본 마일스톤에서 처음 도입됨. M11(VT 인프라 보강)의 IL/DL/ICH 등이 같은 채널 이용 예정.

## 1. 핵심 결정

| 항목 | 결정 | 출처 |
|---|---|---|
| PTY 응답 채널 | `Term.pending_responses: Vec<Vec<u8>>` (mpsc 안 씀) | advisor — Term lock 재사용 |
| DSR 응답 형식 | `\x1b[<row>;<col>R` (1-based, terminfo u6) | terminfo |
| DA1 응답 | `\x1b[?1;2c` | terminfo u8 (xterm-256color) |
| DA2 응답 | `\x1b[>41;0;0c` (Pp=41 xterm) | xterm spec |
| Focus in/out | `\x1b[I` / `\x1b[O` | xterm spec |
| Bracketed paste 마커 | `\x1b[200~` ... `\x1b[201~` | xterm spec |
| arboard 버전 | 3.6.1 (2025-08-23) | crates.io 검증 |
| Cmd+V 정책 | swallow 정책 변경 → arboard로 paste | keyboard-design §13 패치 |
| Focus 송신 경로 | app::WindowEvent::Focused → 직접 `pty.write` (main thread) | advisor — main이 writer 보유 |

## 2. 자료구조 변경

### 2.1 Term flag 필드

```rust
pub struct Term {
    // ... 기존 ...
    /// M10-2: bracketed paste mode (CSI ?2004 h/l). app이 paste 시 wrap 여부 판정에 사용.
    bracketed_paste: bool,
    /// M10-3: focus reporting mode (CSI ?1004 h/l). app이 focus change 시 송신 여부 판정.
    focus_reporting: bool,
    /// M10-1: vt가 PTY로 보낼 응답을 누적. main이 drain해서 pty.write.
    pending_responses: Vec<Vec<u8>>,
}
```

### 2.2 API

```rust
impl Term {
    pub fn bracketed_paste(&self) -> bool;
    pub fn set_bracketed_paste(&mut self, on: bool);
    pub fn focus_reporting(&self) -> bool;
    pub fn set_focus_reporting(&mut self, on: bool);
    pub fn push_response(&mut self, bytes: Vec<u8>);
    pub fn drain_responses(&mut self) -> Vec<Vec<u8>>;
}
```

`drain_responses`는 vec를 비우고 반환. main thread가 about_to_wait 또는 redraw 시점에 호출.

## 3. vt::perform 분기

### 3.1 `handle_dec_private` 추가 분기

```rust
match (code, action) {
    // 기존: 1049, 1047, 25, 1
    (2004, 'h') => self.term.set_bracketed_paste(true),
    (2004, 'l') => self.term.set_bracketed_paste(false),
    (1004, 'h') => self.term.set_focus_reporting(true),
    (1004, 'l') => self.term.set_focus_reporting(false),
    _ => {}
}
```

### 3.2 `csi_dispatch` 신규 분기

action `n` (DSR), `c` (DA1):

```rust
fn csi_dispatch(&mut self, params, intermediates, ignored_intermediates, action) {
    match action {
        // ... 기존 ...
        'n' => self.handle_dsr(params),
        'c' => self.handle_da(params, intermediates),
        _ => {}
    }
}

fn handle_dsr(&mut self, params: &Params) {
    let p = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
    if p == 6 {
        let cur = self.term.cursor();
        let resp = format!("\x1b[{};{}R", cur.row + 1, cur.col + 1);
        self.term.push_response(resp.into_bytes());
    }
    // p == 5 (DSR status)는 표준 "OK" 응답 \x1b[0n. M11 cleanup.
}

fn handle_da(&mut self, params: &Params, intermediates: &[u8]) {
    // intermediates 비어있으면 DA1, '>'가 있으면 DA2.
    let p = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
    if intermediates.is_empty() {
        // DA1: p == 0 (default) → xterm-256color 응답
        if p == 0 {
            self.term.push_response(b"\x1b[?1;2c".to_vec());
        }
    } else if intermediates == b">" {
        // DA2: p == 0 (default) → xterm 41 응답
        if p == 0 {
            self.term.push_response(b"\x1b[>41;0;0c".to_vec());
        }
    }
}
```

vte::Perform의 csi_dispatch 시그니처 확인 필요 — `intermediates: &[u8]`, `ignored_intermediates: bool`, `action: char`. 이미 `'q'` (DECSCUSR)는 `intermediates == b" "`로 분기 중.

## 4. app layer

### 4.1 main thread drain

```rust
// app::render 진입부
{
    let mut term = state.term.lock().unwrap();
    let responses = term.drain_responses();
    drop(term); // lock 해제 후 PTY write
    for resp in responses {
        let _ = state.pty.write(&resp);
    }
}
```

위치: `render()`의 시작부. lock drop 후 write.

**Drain 트리거**: 응답이 누적되는 시점은 vt::perform의 csi_dispatch 안 (즉 PTY reader thread 안). 같은 chunk 처리 끝에서 reader가 `EventLoopProxy::send_event(UserEvent::Repaint)`를 이미 보냄(현재 구조). main이 깨어나 redraw → render → drain. **PTY가 idle한데 응답만 누적되는 경로 없음**(응답이 PTY rx 처리에서만 생성). 따라서 별도 wakeup 안 추가해도 동작. 명시.

대안 (안 채택): push_response 시 EventLoopProxy::send_event 호출. Term이 proxy 보유해야 하는데 의존성 늘어남. 현재 구조로 충분.

**lock contention**: render 매 frame에 term lock + drain. 빈 vec drain은 nanoseconds 비용. 측정 안 해도 OK. 부가 영향 없음.

### 4.2 WindowEvent::Focused 처리

```rust
WindowEvent::Focused(focused) => {
    state.focused = focused;
    state.window.request_redraw();
    // M10-3: focus reporting on이면 PTY로 송신.
    if let Ok(term) = state.term.lock() {
        if term.focus_reporting() {
            let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            drop(term);
            let _ = state.pty.write(bytes);
        }
    }
}
```

### 4.3 Cmd+V 처리

`app::input::encode_key`가 modifier swallow 분기에서 Cmd+V 케이스 분리. 또는 app::window_event에서 KeyboardInput 직전 분기.

```rust
// app::window_event KeyboardInput 분기
if state.modifiers.super_key() {
    if let Key::Character(c) = &event.logical_key {
        if c == "v" || c == "V" {
            // M10-6: clipboard paste
            if event.state == ElementState::Pressed {
                state.handle_paste();
            }
            return;
        }
    }
    // 기존 Cmd+Q / Cmd+W / 그 외 swallow
}
```

`handle_paste`:

```rust
fn handle_paste(&mut self) {
    let text = match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("clipboard read failed: {e}");
            return;
        }
    };
    let bracketed = self.term.lock().map(|t| t.bracketed_paste()).unwrap_or(false);
    if bracketed {
        let _ = self.pty.write(b"\x1b[200~");
        let _ = self.pty.write(text.as_bytes());
        let _ = self.pty.write(b"\x1b[201~");
    } else {
        let _ = self.pty.write(text.as_bytes());
    }
}
```

**arboard 호출 비용**: `Clipboard::new()` 매번 호출은 macOS NSPasteboard 핸들 새로 잡음. 빠름. 캐시 안 함.

## 5. 단계 분해

### M10-1 — pending_responses + flag 필드 (refactor only, 기능 변화 0)
- Term에 필드 추가 + getter/setter/drain.
- `app::render` 진입부에 drain + write 패턴 추가 (현재는 항상 빈 vec라 noop).
- 기존 단위 테스트 통과 확인.

### M10-2 — bracketed paste mode
- `handle_dec_private`에 (2004, h/l) 분기.
- 단위 테스트: `\x1b[?2004h` 후 `term.bracketed_paste()` true.

### M10-3 — focus reporting
- `handle_dec_private`에 (1004, h/l) 분기.
- `WindowEvent::Focused` 분기에 송신 코드.
- 단위 테스트: mode toggle. PTY 송신은 시각 검증.

### M10-4 — DSR cursor position
- `csi_dispatch` `'n'` 분기 + `handle_dsr`.
- 단위 테스트: cursor (5, 10) 위치, `\x1b[6n` → drain하면 `\x1b[6;11R`.

### M10-5 — DA1 + DA2
- `csi_dispatch` `'c'` 분기 + `handle_da`.
- 단위 테스트: DA1 default → `\x1b[?1;2c`, DA2 default → `\x1b[>41;0;0c`.
- 시각 검증: vim 즉시 시작 (timeout 사라짐).

### M10-6 — Cmd+V via arboard
- `Cargo.toml`에 `arboard = "3.6.1"` 추가.
- `app::window_event` Cmd+V 분기.
- `handle_paste` 메서드.
- keyboard-design.md §13 라인 451 갱신 ("Cmd+V swallow" → "M10에서 paste 처리").

### M10-7 — 시각 검증 + docs 패치
- vim 즉시 시작 (V1).
- bracketed paste: zsh에서 Cmd+V → 입력. tmux/zsh가 `\e[200~` 인식 (cursor 이동 안 하고 한 줄로).
- focus reporting: vim 안에서 `:set autoread` + 외부 파일 변경 후 창 focus → vim이 reload.
- docs:
  - keyboard-design.md §10.2 ("M10 자리 비움" → "구현 (2026-05-08)").
  - architecture.md §8 (해당 항목 갱신).
  - reflow-design.md cross-cut 영향 없음.

## 6. 테스트 케이스

| # | 케이스 | 단계 |
|---|---|---|
| T1 | `?2004h` → bracketed_paste true, `?2004l` → false | M10-2 |
| T2 | `?1004h` → focus_reporting true, `?1004l` → false | M10-3 |
| T3 | cursor (5, 10), `CSI 6n` → drain == `\x1b[6;11R` | M10-4 |
| T4 | DA1 default → drain == `\x1b[?1;2c` | M10-5 |
| T5 | DA2 default (intermediates `>`) → drain == `\x1b[>41;0;0c` | M10-5 |
| T6 | DA1 with non-zero param → 응답 없음 (보수적) | M10-5 |
| T7 | drain 후 빈 vec, 두 번째 drain은 빈 | M10-1 |

## 7. 알려진 한계 / 보류

- **DSR 5 (status report `\x1b[0n`)**: 무응답 시 일부 앱이 timeout. M11 cleanup 후보로 명시.
- **DECSTR / RIS reset**: paste/focus/DSR mode를 reset해야 표준이지만 M11.
- **OSC 4/10/11/12 query 응답**: M13에서 같은 pending_responses 채널 이용.
- **large paste 청크 분할**: 대용량 paste는 한 번에 PTY write. PTY buffer 한계 시 partial write 가능. 안전망 필요 시 chunked write loop.
- **arboard 실패 시 fallback**: 현재는 log::warn 후 무시. 사용자 알림은 future.
- **Bracketed paste + 특수 byte**: paste 안에 `\x1b[201~`이 들어있으면 shell이 paste 종료로 오인. xterm 표준은 escape 안 함(사용자 책임, 드물어 OK).
- **Focus race**: focus 변경 직후 mode turn off 케이스. 송신 시점에 mode 다시 확인하므로 race 안전. 단 송신과 mode toggle이 microsecond 단위로 인접하면 한 쪽 누락 가능 — 표준 xterm 동작과 동등.
- **Cmd+V 분기 위치**: keyboard-design §13 단축키 분기와 같은 자리(window_event KeyboardInput). encode_key 안 거침 — paste는 키 인코딩이 아님.

## 8. cross-cut

- **M11 (VT 인프라 보강)**: 같은 pending_responses 채널 이용 (DSR 5, OSC query 등). 본 M10이 인프라 1차 도입.
- **M12 (selection + clipboard)**: arboard 의존성 공유. selection → Cmd+C도 같은 패턴.
- **M13 (OSC bundle)**: OSC 4/10/11/12 query 응답에 같은 채널.

## 9. 진입 직전 docs.rs 재확인

- vte 0.15: `csi_dispatch(params, intermediates, ignored_intermediates, action)` 시그니처 — 이미 사용 중.
- arboard 3.6.1 macOS API: `Clipboard::new()` + `get_text()`. 첫 진입 시 docs.rs 확인.
- portable-pty `master.take_writer()` — 이미 보유.

---

**다음**: M10-1 진입.
