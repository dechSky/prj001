# Multi-window timing 확장 (M-W-5)

작성일: 2026-05-15
상태: design + 본 구현
선행: `multi-window-design.md` (M-W-1/2/3 완료), commit `7efa6f3`

## 1. 배경

`7efa6f3` 시점의 `App::about_to_wait`은 timing 항목 중 **bell drain만** 모든 윈도우를
순회하고, 나머지 5개(bell flash fade / pending_resize / quick_spawn timeout / cursor blink /
auto-scroll)는 `active_state_mut()` 한 윈도우만 처리한다.

```rust
// 현재 (7efa6f3)
for (&id, state) in self.windows.iter_mut() {
    if state.drain_bell_pending() { ... }     // ✅ 모든 윈도우
}
let Some(state) = self.active_state_mut() else { return };
if let Some(start) = state.last_bell_at { ... }   // ❌ active만
if let Some(size) = state.pending_resize.take() { ... } // ❌ active만
if state.quick_spawn_timed_out() { ... }                // ❌ active만
// cursor blink ...                                     // ❌ active만
// auto-scroll ...                                      // ❌ active만
event_loop.set_control_flow(ControlFlow::WaitUntil(next));
```

결과적으로 **background 윈도우는** :
- bell flash가 시작은 되지만 fade가 멎어 250ms 후에도 화면이 inverted 상태.
- WindowEvent::Resized burst가 누적된 채 처리 안 됨 — 화면이 깨지거나 마지막 size 미반영.
- Cmd+Option+N (Quick spawn)이 timeout 안 되어 멈춤.
- cursor blink가 멈춤(focus 잃으면 깜빡 stop은 본래 OK지만, 일관성 위해 모든 윈도우 자체 blink는 따로 유지해도 좋음).
- drag 중 다른 윈도우로 focus 전환되면 auto-scroll이 멎는 것은 OK지만, drag 시작 윈도우에서는 계속 처리되어야.

## 2. 목표

`about_to_wait`이 모든 윈도우의 timing을 처리하고, `ControlFlow`는 모든 윈도우의
다음 wake-up 시각 중 **가장 이른 것**을 채택한다.

## 3. 설계

### 3.1 항목별 정책

| 항목 | 대상 윈도우 | 비고 |
|---|---|---|
| bell drain | 전체 (현재 OK) | background의 출력에서 BEL 인식 필수 |
| bell flash fade | 전체 | active만 flash 시작했더라도 fade는 자체 윈도우에서 |
| pending_resize | 전체 | winit Resized burst는 윈도우별 누적, 처리도 윈도우별 |
| quick_spawn timeout | 전체 | 각 윈도우의 자체 deadline |
| cursor blink | focused 윈도우만 | unfocused는 cursor_visible=true 유지 (기존 정책) |
| auto-scroll | drag 중인 윈도우만 | `is_auto_scrolling()` true인 윈도우 순회 |

### 3.2 ControlFlow 결정

각 윈도우가 "다음에 깨워야 할 시각" Option<Instant> 반환. 전체 윈도우의
deadline을 모아 가장 이른 것을 `WaitUntil`. 즉시 Poll 필요(flash active)면 Poll.

```rust
enum WindowNextWake {
    Poll,                  // 즉시 다시 깨움 (bell flash active 등)
    At(Instant),           // 해당 시각에 깨움
    Idle,                  // 깨울 필요 없음 (focus 없음 + blink 정지 + flash 없음 등)
}
```

`about_to_wait`는:
1. bell drain 모든 윈도우 (현재 그대로)
2. 모든 윈도우 순회하면서 `WindowState::tick(&mut self) -> WindowNextWake` 호출
3. 전체 결과를 fold:
   - 어느 하나라도 Poll → Poll
   - 그 외엔 min(At(t)) → WaitUntil(t)
   - 전부 Idle → Wait

### 3.3 WindowState::tick

기존 `about_to_wait` 본문의 active-only 로직을 WindowState 메서드로 옮긴다.
- self는 단일 윈도우 컨텍스트
- 외부 의존 (event_loop)은 redraw_request만 — `self.window.request_redraw()`로 처리
- ControlFlow 자체는 외부에서 fold

리턴값으로 `Option<Instant>` + `bool poll_needed`를 묶은 `WindowNextWake` 사용.

### 3.4 cursor blink — focused 윈도우만

기존 코드:
```rust
if !state.focused || !state.cursor_blinking_cache {
    if !state.cursor_visible { state.cursor_visible = true; state.window.request_redraw(); }
    // WaitUntil(quick_spawn_deadline) or Wait
    return;
}
```

`!focused` 윈도우는 blink 정지가 본래 정책. WindowState::tick 안에서 focused
케이스만 blink 처리, unfocused는 quick_spawn_deadline만 반영.

## 4. 회귀 위험

- 단일 윈도우(현재 정상 상태)의 동작이 변하면 안 됨. WindowState::tick의 단일
  호출 결과가 기존 active-only 로직과 일치하는지 unit test.
- ControlFlow fold가 잘못되면 idle wake-up 폭주 또는 영원히 깨어나지 않는 deadlock.
- bell drain 위치 유지 — tick 안으로 들어가면 active 외 윈도우의 BEL이 처리 누락될 수 있음.
  → bell drain은 `about_to_wait` 본체에 그대로 두고, tick은 timing(fade/resize/blink/scroll/quick_spawn)만.

## 5. 변경 영향

- `crates/pj001-core/src/app/mod.rs`
  - `WindowNextWake` enum + `merge` 메서드 추가
  - `impl WindowState`에 `tick_at(&mut self, now: Instant) -> WindowNextWake` 메서드 추가
    - Codex M-W-5 1차 개선: `now` 외부 주입 — 한 cycle 모든 윈도우 동일 시각 기준
  - `App::about_to_wait`:
    - bell drain은 모든 윈도우 (기존 그대로) + Codex 개선: visible/audible OR 집계
    - 모든 윈도우 `tick_at(now)` 호출 + `WindowNextWake::merge` fold
    - fold 결과로 `ControlFlow` (Poll / WaitUntil / Wait) 단일 결정
- unit test 추가 (`WindowNextWake` fold 정책만):
  - `window_next_wake_merge_poll_dominates`: Poll이 항상 우선
  - `window_next_wake_merge_takes_earliest_at`: At끼리 min
  - `window_next_wake_idle_yields_to_at_and_self`: Idle은 양보
  - `window_next_wake_fold_across_multiple_windows`: 다중 윈도우 시뮬레이션 (Poll/earliest/all-Idle)
- 자동 검증 사각지대 (Codex M-W-5 1차):
  - `tick_at` 자체의 background 윈도우 동작 (bell fade, pending_resize, quick_spawn cancel) —
    단위 테스트가 wgpu Queue/Renderer 의존이라 본 milestone에서는 manual QA 의존.
  - design doc은 fold 정책 테스트만 반영하도록 정정 (이전 초안의 "tick 단일/다중 테스트" 표현 제거).

## 6. 비목표

- create_new_window의 `pollster::block_on` freeze — 별도 cut (wgpu Instance 공유 + async init).
- Cmd+M minimize — 별도 cut (NSWindow.miniaturize).
- per-window 다른 테마 — 비목표.
- per-window 다른 폰트 크기 — 현재 Config 일괄, 별도 cut.

## 7. 작업 분해

1. `WindowNextWake` enum + `WindowState::tick` 추출 (회귀 0, 단일 호출 보존).
2. `App::about_to_wait`을 모든 윈도우 순회 + fold로 교체.
3. unit test 3개 + 회귀 cargo test.
4. Codex 리뷰 1차.
5. 보강 + commit.
