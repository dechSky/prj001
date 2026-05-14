---
status: v1 freeze (2026-05-14)
scope: themes-handoff.md §5 Phase 4 (OSC 133 + Block 모델)
depends_on:
  - docs/themes-handoff.md (Phase 4 정의)
  - docs/reflow-design.md (reflow 알고리즘 보강 대상)
  - crates/pj001-core/src/grid/mod.rs (OSC 133 메타 인프라, commit `8b8c6ca`)
prerequisites: M13 BSP layout (완료), 6-theme palette (완료), OSC 133 A/B/C/D 메타 추적 (완료)
---

# Block UI Phase 1 Design v1

handoff 디자인의 명령 블록 카드 UI(themes-handoff.md §5 Phase 4) 본 구현 설계서. 현 commit `7ef057d`(212 tests)에서 OSC 133 인프라는 있으나 시각 표현 0. 본 설계가 step-by-step 가이드.

## 1. 배경 / 목표

handoff 정체성 핵심: 명령어를 카드 형태로 그루핑하고 prompt marker로 시각 구분. zsh+starship 등 OSC 133 supporting shell이 prompt 전후를 마킹 → pj001이 이를 메타화하고 theme별 카드로 렌더.

목표:
- VT cell grid 레이아웃 **불변** (PTY layout / mouse hit-test / selection / reporting 전부 그대로)
- shell이 OSC 133 보내면 자동 ON, 안 보내면 raw VT (기존 동작) — fallback 안전
- block_mode = `auto` / `off` 토글
- reflow / scrollback eviction / alt screen 진입 등 모든 lifecycle event에서 block boundary 일관성 유지

## 2. 핵심 발견 — OSC 133 절대 행 좌표 drift

현 코드(grid/mod.rs:543)의 OSC 133 절대 행 계산:

```rust
let absolute = self.scrollback.len() as u64 + self.cursor.row as u64;
```

이건 **scrollback이 SCROLLBACK_CAP에 도달하기 전까지만 정확**. cap 도달 후 `pop_front()`(line 1132-1134)가 호출되면 `scrollback.len()`은 CAP에 고정되고 실제 세션 행은 계속 증가 → 누적 drift. 메타데이터를 누가 절대 행으로 읽는 코드는 아직 없어 user-visible 회귀는 없지만 잠재 critical bug.

해법: `Term.oldest_kept_abs: u64` 도입 + `pop_front` 시점마다 `oldest_kept_abs += 1`. 모든 절대 행 = `oldest_kept_abs + scrollback.len() + cursor.row`로 재정의. reflow 종료 후 SCROLLBACK_CAP 재적용 front drop도 동일 경로.

## 3. 자료 구조

신규 모듈 `crates/pj001-core/src/block.rs`:

```rust
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct BlockId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub enum BlockState {
    Prompt,                                  // A 수신, B 미수신
    Command,                                 // B 수신, C 미수신
    Running,                                 // C 수신, D 미수신
    Completed { exit_code: Option<i32> },    // D 수신
    Abandoned { reason: AbandonReason },     // 비정상 종료
}

#[derive(Clone, Debug, PartialEq)]
pub enum AbandonReason {
    AltScreen,   // alt screen 진입 (vim/htop 등)
    NewPrompt,   // 다음 A 수신했는데 prior block D 미수신
    Reset,       // DECSTR (CSI ! p) / RIS (ESC c)
    Evicted,     // scrollback eviction (prompt_start_abs < oldest_kept_abs)
                 // 이 enum 자체는 BlockStream에서 drop 직전 잠깐 거치는 transitional state
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    pub prompt_start_abs: u64,
    pub command_start_abs: Option<u64>,
    pub output_start_abs: Option<u64>,
    pub output_end_abs: Option<u64>,
    pub started_at: Option<Instant>,  // B 수신 시점 (duration 측정 시작)
    pub ended_at: Option<Instant>,    // D 수신 시점 (duration 측정 끝)
    pub state: BlockState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowBlockTag {
    pub block_id: BlockId,
    pub kind: BlockBoundary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockBoundary {
    PromptStart,   // OSC 133;A
    CommandStart,  // OSC 133;B
    OutputStart,   // OSC 133;C
    OutputEnd,     // OSC 133;D
}

pub struct BlockStream {
    blocks: Vec<Block>,          // append-only, oldest first
    next_id: u64,                // monotonic, no-reuse
}
```

### Term 추가 필드

```rust
pub blocks: BlockStream,         // 모든 (Prompt|Command|Running|Completed) block
pub oldest_kept_abs: u64,        // scrollback eviction 보정 — front drop 시 +1
pub block_capable: bool,         // latch: 첫 OSC 133 A/B/C/D 수신 시 true
```

`block_capable`은 한번 true가 되면 false로 돌아가지 않음 (자동 OFF는 UX 후퇴 — 일관성 우선).

### Grid 추가 필드

```rust
// row 당 multiple boundary 가능 (한 줄 prompt+command 등) — Vec<RowBlockTag>
row_block_tags: Vec<Vec<RowBlockTag>>,
```

### ScrollbackRow 추가 필드

```rust
pub struct ScrollbackRow {
    pub cells: Vec<Cell>,
    pub flags: RowFlags,
    pub block_tags: Vec<RowBlockTag>,  // 신규
}
```

## 4. 좌표 모델 — Absolute Row

**정의**: `abs_row = oldest_kept_abs + scrollback.len() + cursor.row` (alt screen 미진입 시 main grid 한정).

- 새 Term 생성 시 `oldest_kept_abs = 0`
- `newline()` 내 scrollback cap eviction `pop_front` 1회마다 `oldest_kept_abs += 1`
- reflow 종료 후 SCROLLBACK_CAP 재적용 front drop 시에도 동일 경로 — `Term::reflow()`가 reflow_lines 결과를 scrollback에 push back 한 후 `while > CAP pop_front` 루프에서 같이 `oldest_kept_abs += 1`
- alt screen 진입 시 OSC 133 활성화 안 함 (alt에선 BlockStream 갱신 안 됨)

## 5. Reflow Remap 알고리즘 (4a 핵심)

기존 reflow (docs/reflow-design.md): logical line 단위 평탄화 + rewrap.

추가 변경:

1. **logical_offset 추적** — `rewrap_lines`가 원본 row의 각 tag를 `(logical_offset_within_line, tag)`로 기록. logical_offset = 해당 row 시작 위치를 logical line 내 byte/cell offset으로 환산. WIDE cell도 1 cell로 카운트.
2. **rewrap 결과에 재배치** — rewrap된 새 row들이 동일 logical line을 column rewrap한 결과. 각 new row의 시작 logical_offset 알 수 있음 → 원본 tag의 logical_offset이 [new_row.start_offset, new_row.start_offset + new_row.cols) 범위에 들어가는 new row에 push.
3. **multi-tag 보존** — 한 row에 A+B 같이 있으면 둘 다 같은 logical_offset 또는 매우 가까움. rewrap이 logical line을 분할해도 둘 중 어느 쪽에 갈지 logical_offset으로 결정.
4. **block.*_abs 재계산** — reflow 종료 후 BlockStream 순회하면서 tag의 새 위치를 기반으로 `prompt_start_abs / command_start_abs / output_start_abs / output_end_abs` 갱신.

엣지 케이스:
- tag가 evicted row에 있던 경우(reflow가 wide cols → narrow로 갈 때 SCROLLBACK_CAP 초과 evict) → `Block.state = Abandoned { Evicted }` 후 drop
- WIDE 분할 금지 정책으로 tag가 padding row에 떨어지면 → tag는 다음 non-padding row로 carry

## 6. Block Lifecycle 전이

```
[create on A]    Prompt
       │
       ▼  B 수신
    Command
       │
       ▼  C 수신
    Running
       │
       ▼  D 수신
   Completed { exit_code }    [최종]

분기:
- 어느 단계에서든 다음 A 수신 → 현 block.state = Abandoned { NewPrompt }, 새 Block 생성
- 어느 단계에서든 alt screen 진입(CSI ?1049h) → Abandoned { AltScreen }
- DECSTR / RIS 수신 → 진행 중 block만 Abandoned { Reset }
  - completed block history는 scrollback eviction 전까지 유지
- scrollback eviction으로 prompt_start_abs < oldest_kept_abs → Evicted로 마킹 후 drop
- clear_scrollback (Cmd+Shift+K) → prompt가 잘리는 block은 모두 drop. completed 포함.
- clear_buffer (Cmd+K) → 현재 그려진 buffer만 clear, scrollback/Block은 유지
```

## 7. block_mode 설정

TOML schema:

```toml
[block]
mode = "auto"   # "auto" | "off"
```

CLI overlay: `--block-mode off`

semantics:
- `auto`: OSC 133 첫 수신(`Term.block_capable = true`) 후 visual ON. 미수신 세션은 raw VT.
- `off`: 절대 visual ON 안 함. metadata 수집은 여전히 (디버깅 가능)
- 4a 단계: 파싱/저장만, visual output identical. 4b부터 실제 시각 분기.

## 8. PaneContentGutter

`PaneViewport` (M12 도입) 확장:

```rust
pub struct PaneViewport {
    // 기존
    pub x_px: u32, pub y_px: u32, pub width_px: u32, pub height_px: u32,
    pub cols: u16, pub rows: u16, pub col_offset: u16, pub status_row: bool,

    // 4a 신규 (값 0 강제)
    pub gutter_px: u16,
    pub content_x_px: u32,   // = x_px + gutter_px
}
```

- 4a: `gutter_px = 0` 강제. `content_x_px == x_px`. mouse/selection은 `content_x_px` 기준으로 모두 변경 (보정).
- 4b: `block_mode == "auto" && block_capable == true` 시 `gutter_px = font_size + 4`(약 18~22px). 첫 발동 시 cell 시작점이 우측 shift — 시각 변화. 카드 bg도 같이 그려져서 자연스럽게.
- PTY `cols`는 gutter 제외. PTY layout 영향 0.
- block bg는 gutter까지 확장 가능 (4b). glyph는 content 영역만.

## 9. Render Z-order (4b 이후)

```
1. pane background / surface (Phase 3 backdrop blur)
2. block card bg + border (이번 Phase, gutter 포함)
3. selection bg (기존)
4. prompt marker (gutter 중앙, 16×16)
5. glyph layer (기존)
6. cursor + preedit (기존)
7. status badge (✓/× + ms, 카드 우측 또는 상단)
8. chrome (tab bar, find overlay, etc.)
```

cell grid 좌표계 불변. 카드는 row range 기반 별도 instance batch.

## 10. Prompt Marker 렌더 정책

handoff 6 shape (themes-handoff.md §3):
- aurora / obsidian: 16×16 rounded square gradient (SDF 또는 quad+fragment)
- vellum: `$` glyph (text atlas 재사용)
- holo: hex polygon (SDF)
- bento: `RUN` 검정 chip badge (text atlas + bg quad)
- crystal: radial bubble + specular (fragment radial gradient + sampling)

**hybrid 전략**: SDF 강제 X. `PromptMarkerInstance` batch + `kind: MarkerKind` 분기 fragment. text atlas는 기존 glyph batch 재사용 가능.

```rust
pub struct PromptMarkerInstance {
    pub center_x_px: f32,
    pub center_y_px: f32,
    pub size_px: f32,         // default 16
    pub kind: MarkerKind,     // RoundedSquare / Hex / Glyph(char) / Bubble
    pub color_a: [f32; 4],    // gradient start / fill
    pub color_b: [f32; 4],    // gradient end / glow
}
```

4b는 1 marker (rounded square, obsidian only). 4d에서 6 shape 다 도입.

## 11. Status Badge + Duration

- duration = `D received time - B received time`. `D-B`가 의미상 정확 — `C`가 늦거나 출력 없으면 `D-C`는 흔들림. starship `cmd_duration` 중복은 검색/스크롤 맥락 가치 있음.
- running 표시: subdued `RUN` chip, ms tick 없음 (frame noise 회피)
- completed 표시: `123ms` fixed, exit_code != 0이면 빨간 `× 1` (FailKind 추가 검토 4c 단계)
- abandoned 표시: 회색 `--` (4c)

## 12. 4 단계 분해 + Acceptance

| step | scope | acceptance |
|---|---|---|
| **4a** | block.rs 모듈 + BlockStream + RowBlockTag + Grid.row_block_tags + ScrollbackRow.block_tags + Term.oldest_kept_abs + Term.block_capable latch + reflow remap (logical_offset carry) + block_mode TOML/CLI + PaneViewport.gutter_px=0 강제 | 시각 회귀 0 (Derek 수동 확인) + cargo test 240±5 통과 + clippy 새 경고 0 + OSC 133 fixture parsing + reflow 좁힘/넓힘 BlockId 보존 + eviction → oldest_kept_abs 증가 + 신규 25 test |
| **4b** | obsidian 카드 (bg/border/radius), gutter_px>0 발동 (font_size+4), 1 prompt marker (rounded square + 1 color stop) | obsidian 시각 검증 (Derek). 다른 5 테마는 raw로 fallback 정상. Z-order 검증 (글자가 카드 위로 안 잘림) |
| **4c** | status badge ✓/× + fixed duration ms, Running `RUN` chip, Abandoned `--` | starship+zsh 셋업한 환경에서 모든 block에 status 표시. exit_code 0/130 시각 검증 |
| **4d** | 6 marker variants (hex/glyph/bubble), Vellum ordinal, Bento stripe + `PromptMarkerInstance` batch | 6 테마 마커 시각 검증 (Derek). 마커 사이즈 16×16 의도된 크기 |

### 4a Acceptance 상세

1. cargo test 240 ± 5 통과 (현 212 + 신규 25 = 237; 여유 8)
2. clippy 새 경고 0
3. `cargo run` 시각 변화 0 (Derek 수동 확인)
4. OSC 133 fixture parsing OK (FinalTerm style `\x1b]133;A\x1b\\` 등)
5. reflow 좁힘/넓힘에서 BlockId 보존 검증 (unit test)
6. scrollback eviction → `oldest_kept_abs` 증가 + block drop OK (unit test)
7. reflow 후 CAP 재적용 front drop도 oldest_kept_abs 갱신
8. config `[block].mode = "off"` 파싱 OK (값 저장만, 시각 영향 0)
9. memory/projects/pj001/overview.md 진행 상태 업데이트 + 본 design doc 링크
10. archive `docs/architecture/` 정합 (또는 본 doc 단독 정본)

### 신규 25 test 분포 (target)

| 범주 | 수 |
|---|---|
| BlockStream API (push, get_by_id, iter, drop) | 5 |
| RowBlockTag carry / multi-tag per row | 4 |
| reflow remap (좁힘/넓힘/병합/분할) | 5 |
| BlockState 전이 (A→B→C→D, A→A, alt, DECSTR, RIS) | 5 |
| oldest_kept_abs (newline eviction, reflow CAP) | 3 |
| block_mode config 파싱 | 2 |
| 엣지 (alt screen 중 OSC 133 무시, evicted block drop) | 1 |

## 13. 미해결 / 결정 보류 (4b+ 영역)

- 카드 horizontal padding 픽셀 값 (theme별 다름) — 4b 진입 시 themes-handoff.md §3 색 토큰 매핑으로 확정
- Backdrop blur (Phase 3)와의 인터랙션 — block 카드 bg가 blur 위에 그려져야. wgpu render pass 순서
- Floating window (Phase 5) 진입 시 block 카드도 별도 창에서 렌더? — Phase 5에서 결정
- partial-evicted block (prompt evicted but output 보임)의 카드 표현 — 1차는 drop, 추후 fade-out
- hyperlink pool GC와 block GC 정합 — 둘 다 scrollback eviction 따라가는 정책 통일

## 14. 설계 history

- v1 freeze 2026-05-14. 본 doc은 2nd opinion 리뷰 거친 합의 산출.

## 15. 다음 액션

- 4a 코드 진입은 별도 세션 (advisor 검토 후 commit by commit)
