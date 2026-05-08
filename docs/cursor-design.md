# pj001 — Cursor 고도화 설계 (M7)

**상태**: 설계 초안 (2026-05-08). 코드 미반영.
**범위**: A 모양(DECSCUSR) + B 가시성(DECTCEM) + C 포커스(focused/outline) + D 깜빡임(DECSCUSR 짝/홀수) + E saved cursor(DECSC/DECRC).
**제외**: F 커서 색 분리(OSC 12) — 미관 가치 낮음.

---

## 0. 현재 상태 (M6 시점)

| 항목 | 현재 |
|---|---|
| 모양 | block 고정 |
| 깜빡임 | ON 500ms (정책 D 변경 — 원래 OFF였음) |
| 가시성 | 항상 표시 (scrollback view 시 X) |
| 포커스 | 미반영 (창 비활성화 시도 그대로) |
| saved cursor | alt screen 진입/종료 시만 자동 저장. DECSC/DECRC 미처리 |
| 위치 | preedit 있으면 preedit 끝, 없으면 `term.cursor()` |
| 렌더 | `geometry::build_instances`의 `cursor: Option<(usize,usize)>` — 해당 cell의 fg/bg swap |

핵심 한계: 모양 단일, 가시성 통제 불가, vim insert/normal 모드 시각 구분 X.

---

## 1. 운영 정책 D 갱신안

| 차원 | 기본값 | 앱 통제 |
|---|---|---|
| 모양 | Block | DECSCUSR `CSI Ps SP q` |
| 깜빡임 | ON, 500ms | DECSCUSR 짝수=깜빡, 홀수=정지 |
| 가시성 | true | DECTCEM `CSI ? 25 h/l` |
| 포커스 | 창 활성화 시 solid, 비활성 시 outline + 깜빡임 정지 | 자동 |

기본값은 정책 D 현행 유지 (block + blink ON), 앱이 sequence로 override 가능.

---

## 2. 타입 변경

### 2.1 `CursorShape` enum (신규)

```rust
// src/grid/mod.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underscore,
    Bar,
}

impl Default for CursorShape {
    fn default() -> Self { CursorShape::Block }
}
```

### 2.2 `Cursor` 구조 확장

```rust
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub blinking: bool,   // 기본 true (정책 D)
    pub visible: bool,    // DECTCEM. 기본 true
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
```

`derive(Default)`는 bool을 false로 만들어 정책 D와 모순. 명시 impl로 blinking=true, visible=true 강제.

### 2.3 `SavedCursorState` (DECSC용)

```rust
#[derive(Debug, Clone, Copy)]
pub struct SavedCursorState {
    pub row: usize,
    pub col: usize,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub shape: CursorShape,
    pub blinking: bool,
    pub visible: bool,    // xterm DECSC는 DECTCEM 상태도 저장
}
```

xterm 표준은 visible(DECTCEM 모드)도 DECSC에 포함. 일부 앱은 cursor 숨김 → DECSC → 작업 → DECRC 흐름에서 cursor가 다시 숨겨진 상태로 복원되기를 기대.

기존 `saved_main_cursor`/`saved_alt_cursor`는 alt screen용으로 유지(별개 메커니즘). DECSC/DECRC용 새 필드 추가:

```rust
pub struct Term {
    // ... 기존
    saved_main_cursor: Cursor,        // alt screen용 (기존)
    saved_alt_cursor: Cursor,         // alt screen용 (기존)
    decsc_main: Option<SavedCursorState>,  // DECSC용 (신규)
    decsc_alt: Option<SavedCursorState>,   // DECSC용 (신규, alt screen에서 별도)
}
```

DECSC 시 `use_alt`에 따라 어디에 저장할지 결정. xterm 표준 동작.

---

## 3. VT 처리

### 3.1 DECSCUSR — `CSI Ps SP q` (intermediates=" ", action='q')

```rust
// vt/perform.rs::csi_dispatch에 추가
if intermediates == b" " && action == 'q' {
    let n = arg_at(params, 0, 0);
    if let Some((shape, blink)) = decscusr_to_shape(n) {
        self.term.set_cursor_shape(shape, blink);
    }
    return;
}

fn decscusr_to_shape(n: usize) -> Option<(CursorShape, bool)> {
    match n {
        0 | 1 => Some((CursorShape::Block, true)),     // default + blink
        2 => Some((CursorShape::Block, false)),         // steady
        3 => Some((CursorShape::Underscore, true)),
        4 => Some((CursorShape::Underscore, false)),
        5 => Some((CursorShape::Bar, true)),
        6 => Some((CursorShape::Bar, false)),
        _ => None,                                      // 알 수 없는 값 무시
    }
}
```

### 3.2 DECTCEM — `CSI ? 25 h/l`

```rust
// vt/perform.rs::handle_dec_private에 추가
match (code, action) {
    (25, 'h') => self.term.set_cursor_visible(true),
    (25, 'l') => self.term.set_cursor_visible(false),
    // 기존 (1049, 'h'/'l') 등 유지
    _ => {}
}
```

### 3.3 DECSC/DECRC — `ESC 7` / `ESC 8`

```rust
// vt/perform.rs::esc_dispatch (현재 비어있음)
fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
    match byte {
        b'7' => self.term.decsc(),
        b'8' => self.term.decrc(),
        _ => {}
    }
}
```

### 3.4 Term 메서드 (신규)

```rust
impl Term {
    pub fn set_cursor_shape(&mut self, shape: CursorShape, blinking: bool);
    pub fn set_cursor_visible(&mut self, visible: bool);
    pub fn decsc(&mut self);    // 현재 cursor + SGR 상태 저장
    pub fn decrc(&mut self);    // 저장된 상태 복원 (없으면 noop)
}
```

`decsc`/`decrc` 구현 핵심:
- DECSC: 현재 use_alt 따라 `decsc_main` 또는 `decsc_alt`에 SavedCursorState 저장.
- DECRC: 같은 위치에서 복원. 복원 항목은 row/col/fg/bg/attrs/shape/blinking. visible은 미저장.

---

## 4. 포커스 관리

### 4.1 AppState 필드

```rust
struct AppState {
    // ... 기존
    focused: bool,  // 기본 true
}
```

### 4.2 WindowEvent::Focused 처리

```rust
// app/mod.rs::window_event
WindowEvent::Focused(focused) => {
    state.focused = focused;
    state.window.request_redraw();
}
```

### 4.3 깜빡임 정지

`about_to_wait`에서 `state.focused == false`이면 cursor_visible toggle 안 함. unfocused 시 깜빡임 정지(표준 터미널 동작).

**lock 캐시**: `about_to_wait`은 매 tick 발화. 매번 `state.term` lock 잡으면 PTY 리더 thread와 contention. `AppState.cursor_blinking_cache: bool` 추가하고 DECSCUSR/Ime/SGR 핸들러에서 cursor 변경 시 갱신.

```rust
struct AppState {
    // ...
    focused: bool,
    cursor_blinking_cache: bool,  // term.cursor().blinking 미러
}

fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    let Some(state) = self.state.as_mut() else { return };
    if !state.focused || !state.cursor_blinking_cache {
        // 깜빡임 정지: cursor_visible은 true 유지 (focused이면 solid block,
        // unfocused이면 outline으로 그려짐 — 어느 쪽이든 항상 visible).
        state.cursor_visible = true;
        return;
    }
    // ... 기존 toggle 로직 (500ms 주기)
}
```

DECSCUSR 처리 시 `state.cursor_blinking_cache = blinking` 갱신. WindowEvent::Focused 처리 시 별도 갱신 불필요(focused 자체가 토글됨).

### 4.4 outline 렌더

focused=false면 cell 전체를 fg로 swap하지 않고 가장자리(border) 1px만 fg. 빈 cell 위에 1px 외곽선만 그린다.

---

## 5. 렌더 변경 (모델 결정)

### 5.0 모델 — Model A (overlay)

advisor 검토 결과: Model A(별도 overlay instance)와 Model B(main instance에 shape mask 통합) 중 **A 채택**.

- 이유: cursor 위치 cell 한 개만 overdraw. main 렌더 흐름과 cursor 로직 분리. 단일 pipeline 유지.
- 단점: cursor cell에서 main + overlay 두 fragment write (1 cell 한정. GPU에 무시할 비용).

### 5.1 cursor 정보 풍부화

`build_instances`의 `cursor: Option<(usize,usize)>` → `cursor: Option<CursorRender>`로 확장:

```rust
#[derive(Clone, Copy)]
pub struct CursorRender {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub focused: bool,    // false면 outline
}
```

`Some(_)`이면 cursor overlay instance 한 개 추가. `None`이면 안 추가 (visible=false, blink off phase, scrollback view 등). 가시성 판정은 호출자(app::render) 책임.

### 5.2 CellInstance.flags 확장 (기존 _pad 자리 활용)

```rust
pub struct CellInstance {
    pub cell_xy: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub glyph_offset: [f32; 2],
    pub glyph_size: [f32; 2],
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub cell_span: f32,
    pub flags: u32,        // 신규: 기존 _pad 12바이트 중 4바이트 활용
    pub _pad: [f32; 2],    // 8바이트 유지
}
```

flags bitfield:
- bit 0 (`0x01`): is_cursor_overlay (1이면 cursor instance)
- bit 1-2 (`0x06`): shape (0=Block, 1=Underscore, 2=Bar)
- bit 3 (`0x08`): focused

main instance는 모두 flags=0. cursor overlay instance만 set. **CellInstance 크기 불변** (기존 padding 자리 활용).

### 5.3 shape별 영역 + shader

cursor overlay instance는 cell 전체 quad. shader가 fragment의 cell-relative 좌표 `rel ∈ [0,1]²`로 shape 영역 판정:

- Block focused: 전체 (모든 fragment fg)
- Underscore focused: `rel.y >= underscore_top`
- Bar focused: `rel.x <= bar_right`
- Block unfocused (outline): `rel.x < bw || rel.x > 1-bw || rel.y < bh || rel.y > 1-bh` (border만)

영역 외 fragment는 `discard` — main instance가 그린 글자가 그대로 보임.

```wgsl
@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    if ((in.flags & 1u) != 0u) {
        // cursor overlay
        let shape = (in.flags >> 1u) & 3u;
        let focused = (in.flags & 8u) != 0u;
        let rel = in.cell_pixel / cell_size;  // 0..1
        let in_shape = ...;  // shape별 판정
        if (!in_shape) { discard; }
        return in.fg;  // reversed 색 (build_instances에서 swap)
    }
    // 기존 main 로직
}
```

shape 영역 비율은 cell.height/width % 기반 + **최소 픽셀 floor**:
- underscore_top = `1.0 - max(2.0, cell_h * 0.12) / cell_h`
- bar_right = `max(2.0, cell_w * 0.15) / cell_w`
- outline 두께 bw/bh = `max(1.0, cell_h * 0.05) / cell_h` (1-2px)

작은 폰트에서 shape이 사라지지 않도록 floor 강제.

### 5.4 cursor overlay 색상

cursor overlay instance의 fg는 cursor 위치 cell의 *원래* bg, bg는 원래 fg (reverse). build_instances가 `term.cell(row, col)`로 원래 cell 읽고 swap하여 overlay instance 생성:

```rust
let cell = term.cell(cur.row, cur.col);
let (orig_fg, orig_bg) = resolve_cell_colors(&cell);
let overlay_fg = orig_bg;  // reverse
let overlay_bg = orig_fg;  // 안 쓰이지만 채워둠 (discard로 보이지 않음)
```

main instance는 변경 없음 (현재 그대로). overlay는 별도 push.

main instance의 REVERSE 처리는 그대로 유지(기존 동작). cursor 위치 cell은 main 렌더 + overlay가 shape 영역만 덮음.

### 5.5 cross-cut

- **preedit + cursor**: cursor 위치 = preedit 끝(현재 그대로). shape도 일반대로 적용. preedit dim color 위에 cursor overlay.
- **scrollback view**: cursor overlay instance 추가 안 함 (`CursorRender = None` 전달). 단 Term의 cursor.shape/blinking/visible 상태는 평소처럼 mutate(scrollback view 중에도 PTY가 DECSCUSR 보낼 수 있음). 단지 렌더만 hide.
- **resize**: cursor.shape/blinking/visible은 자동 보존 (Term 필드라 resize에 영향 X).

---

## 6. 호출 흐름 (요약)

```
[키 입력 / PTY output]
        ↓
[Term: cursor.row/col 갱신, SGR/DECSCUSR/DECTCEM/DECSC/DECRC 처리]
        ↓
[app::render(): cursor 가시성 판정 (focused/blinking phase/visible/scrollback)]
        ↓
[render: build_instances + cursor instance 추가]
        ↓
[shader: shape별 quad 영역 fg 칠]
```

---

## 7. 단계 분해 (구현 시)

| 단계 | 내용 | 의존 | 검증 |
|---|---|---|---|
| **M7-1** | CursorShape + Cursor 확장 + DECSCUSR 처리. 렌더는 모양 무시(block만 그림) | 독립 | `printf '\033[5 q'` 후 stderr trace에 `(Bar, true)` 같은 (shape, blinking) tuple log 확인. **렌더 검증 없음** — 명시 |
| **M7-2** | DECTCEM `CSI ? 25 h/l` + 렌더 가시성 판정 | M7-1 | vim에서 cursor 깜빡이다 hide 후 다시 보임. 가시성만 검증, shape는 block 유지 |
| **M7-3** | 포커스 관리 (focused 필드 + about_to_wait 깜빡임 정지 + cursor_blinking_cache 도입) | M7-2 | 다른 창 클릭 시 깜빡임 정지. solid block 유지(outline은 M7-5) |
| **M7-4** | DECSC/DECRC (`ESC 7`/`ESC 8`) | M7-1 | 직접 printf로 save → 위치 변경 → restore 검증 |
| **M7-5** | shape별 quad 렌더 + outline 렌더 (geometry + shader 변경 + flags bitfield 도입) | M7-1, M7-3 | block/underscore/bar/outline 모두 시각 검증 |

각 단계 30분~1.5h. 총 4-5h.

**M7-5 진입 직전 사용자 결정 필요** (지금 deferred 상태인 § 11 항목들):
- outline 두께 (1px vs 2px)
- focused=false 시 cursor 행동 (outline + 깜빡임 정지 vs 완전 숨김)
- shape 두께 % (underscore 12%, bar 15% 기본 후 조정)

이 셋은 M7-5 시작 시 AskUserQuestion으로 한 번에 결정.

**체크포인트**: M7-1, M7-2, M7-3 끝나면 vim/zsh 일반 작업 회귀 없는지 확인 후 M7-4/M7-5 진입.

---

## 8. 결정·트레이드오프

### 결정
- **shape는 grid 모듈에 enum으로**: cursor 정보는 Term이 진실 source. render는 읽기만.
- **DECSC/DECRC와 alt screen save는 분리**: 두 메커니즘이 별개 sequence. 함께 사용 가능.
- **focused unfocused는 outline + 깜빡임 정지**: xterm/iTerm 표준 따름.
- **cursor 가시성 판정은 app 레이어**: Term은 visible/blinking 상태만 보유, 실제 그릴지 결정은 app::render에서 (focused, blink phase, scrollback view 종합).
- **shader는 단일 pipeline 유지**: instance에 cursor_mode 필드 추가, fragment 분기.

### 트레이드오프 (보류 결정)
- **shape별 cell 비율**: underscore/bar의 두께를 cell.height/width의 % vs 절대 픽셀. **% 기반 권장** (다른 폰트 크기에 robust).
- **outline 두께**: 1px vs 2px. **1px** 시작 (M7-5에서 시각 검증으로 확정).
- **cursor.blinking 기본값**: 정책 D는 ON. 그대로 유지. DECSCUSR 0/1이 default + blink 매핑이라 자연스럽게 ON.
- **CSI ?12 h/l (cursor blink)**: 일부 터미널 지원. **MVP+ 미지원** (DECSCUSR로 충분).

### 보류
- **OSC 12 cursor color**: 다음 단계.
- **DECSCUSR n=0 vs n=1**: 둘 다 default(=block + blink)로 처리. xterm은 0=default(설정에 따름), 1=blink block. 학습 단계는 단순화.
- **CSI s/u (alternate save)**: 일부 앱이 사용. 우선 DECSC/DECRC만, 필요 시 추가.

---

## 9. 영향 범위

| 파일 | 변경 |
|---|---|
| `src/grid/mod.rs` | CursorShape enum, Cursor 필드 확장, SavedCursorState, decsc/decrc/set_cursor_shape/set_cursor_visible |
| `src/vt/perform.rs` | csi_dispatch (DECSCUSR), handle_dec_private (DECTCEM), esc_dispatch (DECSC/DECRC) |
| `src/app/mod.rs` | AppState.focused, WindowEvent::Focused, cursor 가시성 판정, about_to_wait blink 정지 |
| `src/render/geometry.rs` | CursorRender struct, build_instances cursor 인자 변경, cursor instance push |
| `src/render/mod.rs` | CellInstance에 cursor_mode 필드, update_term cursor 인자 변경 |
| `src/render/shader.wgsl` | cursor_mode 분기 (shape별 quad rect, outline) |
| `src/render/atlas.rs` | 변경 없음 |
| `src/pty/` | 변경 없음 |

shader 변경이 가장 위험. 검증 시 main cell 렌더에 회귀 없는지 우선 확인.

---

## 10. 운영 정책 D 최종안 (architecture.md / overview.md 갱신 필요)

```
| D | 커서 모양 / 깜빡임 / 가시성 / 포커스 |
| - | 기본: Block, blink ON 500ms, visible true. |
| - | 앱 제어: DECSCUSR (모양·깜빡임), DECTCEM (가시성). |
| - | 포커스: unfocused 시 outline + 깜빡임 정지. |
| - | DECSC/DECRC로 cursor 상태 저장/복원 지원. |
```

---

## 11. 미해결 / 사용자 결정 필요

§ 7의 M7-5 진입 시 단일 AskUserQuestion으로 결정:

- **outline 두께**: 1px vs 2px (기본 1px 후 시각 검증)
- **shape % 비율**: underscore 12%, bar 15% (최소 floor 2px). 폰트 따라 조정 가능
- **focused=false 시 행동**: outline + 깜빡임 정지(표준) vs 완전 숨김(일부 사용자 선호)

M7 진입 시점 자체는 별도 결정 — 다른 작업(마우스 인코딩, OSC 8 hyperlink 등)과 우선순위 비교.
